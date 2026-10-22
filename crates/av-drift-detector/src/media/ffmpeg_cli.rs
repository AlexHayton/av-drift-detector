//! Reading and writing files by running the `ffmpeg` and `ffprobe` programs,
//! so nothing links against FFmpeg's libraries.
//!
//! The results match the linked path ([`analyze`](crate::analyze()) and
//! `encode::generate`): frames keep their exact timestamps (FFmpeg's `showinfo`
//! filter reports them as integers in the stream's time base), and audio is
//! put back on the container's timeline chunk by chunk, with the same gap and
//! overlap handling.
//!
//! The programs are found on `PATH`, or at the paths in the environment
//! variables [`FFMPEG_ENV`] and [`FFPROBE_ENV`]. FFmpeg 5.1 or later is
//! needed; writing H.264 needs a build with libx264, as most packages are.

use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use serde_json::Value;

use super::{
    AudioCodec, AudioTrack, AudioTrackBuilder, GenerateOptions, MediaScan, Progress, ScanWindow,
    VideoCodec, VideoSample,
};
use crate::analyze::{AnalyzeOptions, analyze_scan, scan_window};
use crate::audio::{ToneCode, ToneSynth};
use crate::pattern::canvas::{LumaCanvas, LumaView};
use crate::pattern::{PatternRenderer, decode_frame};
use crate::report::DriftReport;
use crate::{Error, FrameRate, Result};

/// Environment variable naming the `ffmpeg` program to run (default: `ffmpeg` on `PATH`).
pub const FFMPEG_ENV: &str = "AV_DRIFT_FFMPEG";
/// Environment variable naming the `ffprobe` program to run (default: `ffprobe` on `PATH`).
pub const FFPROBE_ENV: &str = "AV_DRIFT_FFPROBE";

/// Measures the A/V drift of every frame in a capture, like
/// [`analyze`](crate::analyze()), decoding with the `ffmpeg` program.
pub fn analyze(path: &Path, opts: &AnalyzeOptions) -> Result<DriftReport> {
    let window = scan_window(opts)?;
    let scan = scan(path, window, opts.use_qr)?;
    analyze_scan(&scan, opts, path.display().to_string())
}

/// Decodes the video stamps and mono audio in `window`, like `decode::scan`.
pub fn scan(path: &Path, window: ScanWindow, try_qr: bool) -> Result<MediaScan> {
    let probe = probe(path)?;
    let video = probe.video.ok_or_else(|| Error::MissingStream {
        path: path.to_path_buf(),
        kind: "video",
    })?;
    // As the linked path does: start a second early, so the decoder has a keyframe.
    let seek = (window.start - window.audio_margin - 1.0).max(0.0);
    let (frames, width, height) = read_video(path, &video, seek, window, try_qr)?;
    let audio = match &probe.audio {
        Some(stream) => read_audio(path, stream, seek, window)?,
        None => None,
    };
    Ok(MediaScan {
        video: frames,
        audio,
        width: width.unwrap_or(video.width),
        height: height.unwrap_or(video.height),
        stream_fps: video.avg_fps,
    })
}

/// Writes a test file like `encode::generate`, by piping frames to the `ffmpeg` program.
pub fn generate(opts: &GenerateOptions, mut on_progress: impl FnMut(Progress)) -> Result<()> {
    opts.validate()?;
    let (fps, rate) = (opts.fps, opts.sample_rate);
    let (w, h) = (opts.width as usize, opts.height as usize);
    let tones = ToneCode::new(fps, rate)?;
    let renderer =
        PatternRenderer::new(w, h, tones.clone())?.with_total_frames(opts.total_frames());
    let total_frames = opts.total_frames();
    let total_samples = fps.frame_start_sample(total_frames as i64, rate) as u64;

    // The audio goes first, into a raw file that ffmpeg reads beside the piped video.
    let audio = TempFile::new("f32")?;
    {
        let mut out = BufWriter::new(File::create(&audio.0)?);
        let mut synth = ToneSynth::new(tones, opts.audio_delay);
        if opts.music {
            synth = synth.with_music();
        }
        let mut chunk = vec![0.0f32; 4800];
        let mut left = total_samples;
        while left > 0 {
            let n = (left as usize).min(chunk.len());
            synth.fill(&mut chunk[..n]);
            for s in &chunk[..n] {
                out.write_all(&s.to_le_bytes())?;
            }
            left -= n as u64;
        }
        out.flush()?;
    }

    let mut cmd = Command::new(program(FFMPEG_ENV, "ffmpeg"));
    cmd.args(["-hide_banner", "-nostats", "-loglevel", "error", "-y"])
        .args(["-f", "rawvideo", "-pix_fmt", "yuv420p"])
        .args(["-s", &format!("{w}x{h}")])
        .args(["-framerate", &format!("{}/{}", fps.num(), fps.den())])
        .args(["-i", "pipe:0"])
        .args(["-f", "f32le", "-ar", &rate.to_string(), "-ac", "1", "-i"])
        .arg(&audio.0)
        .args(["-map", "0:v", "-map", "1:a"]);
    match opts.video_codec {
        VideoCodec::H264 => cmd.args(["-c:v", "libx264", "-crf", "18", "-preset", "fast"]),
        VideoCodec::Ffv1 => cmd.args(["-c:v", "ffv1"]),
    };
    // One-second closed GOPs, BT.709 limited range: as `encode::generate`.
    cmd.args(["-g", &fps.nominal().to_string(), "-flags", "+cgop"])
        .args([
            "-pix_fmt",
            "yuv420p",
            "-color_range",
            "tv",
            "-colorspace",
            "bt709",
        ]);
    match opts.audio_codec {
        AudioCodec::Aac => cmd.args(["-c:a", "aac", "-b:a", "256k"]),
        AudioCodec::Pcm => cmd.args(["-c:a", "pcm_s16le"]),
    };
    cmd.arg(&opts.output)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = spawn(&mut cmd, FFMPEG_ENV, "ffmpeg")?;
    let log = collect_log(&mut child);
    let mut stdin = child.stdin.take().expect("piped stdin");

    // Y plane from the renderer; grey chroma.
    let chroma = w.div_ceil(2) * h.div_ceil(2);
    let mut frame = vec![128u8; w * h + 2 * chroma];
    let mut written = Ok(());
    for n in 0..total_frames {
        renderer.render(n as u32, &mut LumaCanvas::new(&mut frame[..w * h], w, h, w))?;
        // A failed write means ffmpeg stopped; its own message explains why.
        written = stdin.write_all(&frame);
        if written.is_err() {
            break;
        }
        on_progress(Progress {
            frames_done: n + 1,
            frames_total: total_frames,
        });
    }
    drop(stdin);
    let status = child.wait()?;
    let log = log.join().unwrap_or_default();
    if !status.success() {
        return Err(Error::FfmpegCommand(format!(
            "ffmpeg couldn't write {}: {}",
            opts.output.display(),
            log.trim()
        )));
    }
    written?;
    Ok(())
}

fn program(env: &str, default: &str) -> PathBuf {
    std::env::var_os(env).map_or_else(|| default.into(), PathBuf::from)
}

/// The error for a program that couldn't be started.
fn launch_error(e: &io::Error, env: &str, default: &str) -> Error {
    let name = program(env, default).display().to_string();
    if e.kind() == io::ErrorKind::NotFound {
        Error::FfmpegCommand(format!(
            "couldn't find `{name}`: install FFmpeg 5.1 or later and put `ffmpeg` and \
             `ffprobe` on your PATH, or set {FFMPEG_ENV} and {FFPROBE_ENV}"
        ))
    } else {
        Error::FfmpegCommand(format!("couldn't run `{name}`: {e}"))
    }
}

fn spawn(cmd: &mut Command, env: &str, default: &str) -> Result<Child> {
    cmd.spawn().map_err(|e| launch_error(&e, env, default))
}

/// Collects a child's stderr on a thread, so it can't fill the pipe and stall.
fn collect_log(child: &mut Child) -> JoinHandle<String> {
    let mut stderr = child.stderr.take().expect("piped stderr");
    thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    })
}

/// A temporary file, deleted when dropped.
struct TempFile(PathBuf);

impl TempFile {
    fn new(extension: &str) -> Result<Self> {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let name = format!("av-drift-{}-{nanos}-{n}.{extension}", std::process::id());
        Ok(TempFile(std::env::temp_dir().join(name)))
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

// ---------------------------------------------------------------- probing --

struct VideoStream {
    index: u64,
    width: u32,
    height: u32,
    avg_fps: Option<FrameRate>,
}

struct AudioStream {
    index: u64,
    /// Seconds per tick of the stream's time base, for timestamp rounding.
    time_base: f64,
}

struct Probe {
    video: Option<VideoStream>,
    audio: Option<AudioStream>,
}

fn ratio(value: &Value) -> Option<(i64, i64)> {
    let (num, den) = value.as_str()?.split_once('/')?;
    Some((num.parse().ok()?, den.parse().ok()?))
}

/// The best video and audio streams, as `ffprobe` describes them.
fn probe(path: &Path) -> Result<Probe> {
    let output = Command::new(program(FFPROBE_ENV, "ffprobe"))
        .args(["-v", "error", "-of", "json", "-show_entries"])
        .arg("stream=index,codec_type,width,height,avg_frame_rate,time_base:stream_disposition=attached_pic")
        .arg("-i")
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| launch_error(&e, FFPROBE_ENV, "ffprobe"))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr);
        return Err(Error::FfmpegCommand(format!("ffprobe: {}", message.trim())));
    }
    let json: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| Error::FfmpegCommand(format!("unreadable ffprobe output: {e}")))?;
    let streams = json["streams"].as_array().cloned().unwrap_or_default();
    let usable = |s: &&Value, kind: &str| {
        s["codec_type"] == kind && s["disposition"]["attached_pic"].as_i64() != Some(1)
    };
    let video = streams
        .iter()
        .find(|s| usable(s, "video"))
        .map(|s| VideoStream {
            index: s["index"].as_u64().unwrap_or(0),
            width: s["width"].as_u64().unwrap_or(0) as u32,
            height: s["height"].as_u64().unwrap_or(0) as u32,
            avg_fps: ratio(&s["avg_frame_rate"])
                .filter(|&(n, d)| n > 0 && d > 0)
                .and_then(|(n, d)| FrameRate::new(n as u32, d as u32).ok()),
        });
    let audio = streams
        .iter()
        .find(|s| usable(s, "audio"))
        .map(|s| AudioStream {
            index: s["index"].as_u64().unwrap_or(0),
            time_base: ratio(&s["time_base"])
                .filter(|&(n, d)| n > 0 && d > 0)
                .map_or(1e-6, |(n, d)| n as f64 / d as f64),
        });
    Ok(Probe { video, audio })
}

// -------------------------------------------------------------- decoding --

/// `key:value` from a showinfo line, where the value may be padded with spaces.
fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let start = line.find(&format!(" {key}:"))? + key.len() + 2;
    line[start..].split_whitespace().next()
}

/// What `showinfo` and `ashowinfo` say about each frame, in output order.
enum Info {
    /// The video filter's input time base.
    TimeBase(i64, i64),
    Frame {
        pts: Option<i64>,
        pts_time: Option<f64>,
        size: Option<(usize, usize)>,
        rate: Option<u32>,
        channels: Option<usize>,
        samples: Option<usize>,
    },
}

fn parse_info(line: &str) -> Option<Info> {
    if !line.contains("showinfo") {
        return None;
    }
    if let Some(rest) = line.split("config in time_base:").nth(1) {
        let tb = rest.split(',').next()?.trim();
        let (n, d) = tb.split_once('/')?;
        return Some(Info::TimeBase(n.parse().ok()?, d.parse().ok()?));
    }
    field(line, "n")?;
    let pts = field(line, "pts")?;
    Some(Info::Frame {
        pts: pts.parse().ok(),
        pts_time: field(line, "pts_time").and_then(|t| t.parse().ok()),
        size: field(line, "s").and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        }),
        rate: field(line, "rate").and_then(|r| r.parse().ok()),
        channels: field(line, "channels").and_then(|c| c.parse().ok()),
        samples: field(line, "nb_samples").and_then(|n| n.parse().ok()),
    })
}

/// Runs a decoding `ffmpeg`, sending each showinfo report down a channel and
/// keeping the rest of its log for error messages.
struct Decoder {
    child: Child,
    info: mpsc::Receiver<Info>,
    log: JoinHandle<String>,
}

impl Decoder {
    fn start(path: &Path, seek: f64, outputs: &[&str]) -> Result<Self> {
        let mut cmd = Command::new(program(FFMPEG_ENV, "ffmpeg"));
        cmd.args(["-nostdin", "-hide_banner", "-nostats", "-loglevel", "info"])
            .args(["-noautorotate", "-copyts"]);
        if seek > 0.0 {
            cmd.args(["-ss", &format!("{seek:.6}")]);
        }
        cmd.arg("-i")
            .arg(path)
            .args(outputs)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = spawn(&mut cmd, FFMPEG_ENV, "ffmpeg")?;
        let stderr = child.stderr.take().expect("piped stderr");
        let (tx, info) = mpsc::channel();
        let log = thread::spawn(move || {
            let mut other = Vec::new();
            for line in BufReader::new(stderr).lines().map_while(io::Result::ok) {
                match parse_info(&line) {
                    Some(i) => {
                        let _ = tx.send(i);
                    }
                    None if !line.contains("showinfo") => other.push(line),
                    None => {}
                }
            }
            // The end of the log, where ffmpeg explains a failure.
            other[other.len().saturating_sub(5)..].join("\n")
        });
        Ok(Decoder { child, info, log })
    }

    /// Stops early (`stopped`) or waits for the end, reporting a failure.
    fn finish(mut self, stopped: bool, path: &Path) -> Result<()> {
        if stopped {
            let _ = self.child.kill();
        }
        drop(self.child.stdout.take());
        let status = self.child.wait()?;
        let log = self.log.join().unwrap_or_default();
        if stopped || status.success() {
            return Ok(());
        }
        Err(Error::FfmpegCommand(format!(
            "ffmpeg couldn't decode {}: {}",
            path.display(),
            log.trim()
        )))
    }
}

type Frames = (Vec<VideoSample>, Option<u32>, Option<u32>);

fn read_video(
    path: &Path,
    stream: &VideoStream,
    seek: f64,
    window: ScanWindow,
    try_qr: bool,
) -> Result<Frames> {
    let map = format!("0:{}", stream.index);
    let mut decoder = Decoder::start(
        path,
        seek,
        &[
            "-map",
            &map,
            "-fps_mode",
            "passthrough",
            "-vf",
            "showinfo",
            "-pix_fmt",
            "gray",
            "-f",
            "rawvideo",
            "pipe:1",
        ],
    )?;
    let mut stdout = decoder.child.stdout.take().expect("piped stdout");
    let (mut samples, mut size) = (Vec::new(), (None, None));
    let mut time_base = None;
    let mut luma = Vec::new();
    let mut stopped = false;
    while let Ok(info) = decoder.info.recv() {
        let (pts, pts_time, (w, h)) = match info {
            Info::TimeBase(n, d) => {
                time_base = (n > 0 && d > 0).then_some((n, d));
                continue;
            }
            Info::Frame {
                pts,
                pts_time,
                size: Some(s),
                ..
            } => (pts, pts_time, s),
            Info::Frame { .. } => continue,
        };
        luma.resize(w * h, 0);
        if stdout.read_exact(&mut luma).is_err() {
            break;
        }
        let time = match (pts, time_base) {
            (Some(pts), Some((n, d))) => pts as f64 * n as f64 / d as f64,
            _ => match pts_time {
                Some(t) => t,
                None => continue, // no timestamp: skipped, as the linked decoder does
            },
        };
        if time < window.start - 1e-6 {
            continue;
        }
        if window.end.is_some_and(|end| time >= end - 1e-6) {
            stopped = true;
            break;
        }
        size = (Some(w as u32), Some(h as u32));
        let stamp = decode_frame(&LumaView::new(&luma, w, h, w), try_qr);
        samples.push(VideoSample { time, stamp });
    }
    decoder.child.stdout = Some(stdout);
    decoder.finish(stopped, path)?;
    Ok((samples, size.0, size.1))
}

fn read_audio(
    path: &Path,
    stream: &AudioStream,
    seek: f64,
    window: ScanWindow,
) -> Result<Option<AudioTrack>> {
    let map = format!("0:{}", stream.index);
    let mut decoder = Decoder::start(
        path,
        seek,
        &[
            "-map",
            &map,
            "-af",
            "ashowinfo",
            "-c:a",
            "pcm_f32le",
            "-f",
            "f32le",
            "pipe:1",
        ],
    )?;
    let mut stdout = decoder.child.stdout.take().expect("piped stdout");
    // As the linked path: mismatches within a tick and a half of the stream's
    // time base (MKV rounds to 1 ms) are rounding, not gaps.
    let tolerance = 0.002f64.max(1.5 * stream.time_base);
    let mut builder = AudioTrackBuilder::new(
        window.start - window.audio_margin,
        window.end.map(|e| e + window.audio_margin),
        tolerance,
    );
    let mut bytes = Vec::new();
    let mut stopped = false;
    while let Ok(info) = decoder.info.recv() {
        let Info::Frame {
            pts,
            pts_time,
            rate: Some(rate),
            channels: Some(channels),
            samples: Some(count),
            ..
        } = info
        else {
            continue;
        };
        let channels = channels.max(1);
        bytes.resize(count * channels * 4, 0);
        if stdout.read_exact(&mut bytes).is_err() {
            break;
        }
        // The filter's time base is 1/rate; check against the printed time.
        let time = match (pts, pts_time) {
            (Some(pts), Some(t)) => {
                let exact = pts as f64 / f64::from(rate);
                if (exact - t).abs() <= 1e-5 * t.abs().max(1.0) {
                    exact
                } else {
                    t
                }
            }
            (Some(pts), None) => pts as f64 / f64::from(rate),
            (None, Some(t)) => t,
            (None, None) => continue,
        };
        // Average the channels, as the linked path's downmix does.
        let mono: Vec<f32> = bytes
            .chunks_exact(4 * channels)
            .map(|frame| {
                frame
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| f32::from_le_bytes(*b))
                    .sum::<f32>()
                    / channels as f32
            })
            .collect();
        builder.push(time, rate, &mono)?;
        if builder.is_full() {
            stopped = true;
            break;
        }
    }
    decoder.child.stdout = Some(stdout);
    decoder.finish(stopped, path)?;
    Ok(builder.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_how_to_get_ffmpeg() {
        let missing = io::Error::from(io::ErrorKind::NotFound);
        let message = launch_error(&missing, FFMPEG_ENV, "ffmpeg").to_string();
        assert!(message.contains("install FFmpeg 5.1 or later"), "{message}");
        assert!(
            message.contains(FFMPEG_ENV) && message.contains(FFPROBE_ENV),
            "{message}"
        );
    }

    #[test]
    fn parses_showinfo_lines() {
        let config = "[Parsed_showinfo_0 @ 0xa39012100] config in time_base: 1/30000, frame_rate: 30000/1001";
        assert!(matches!(parse_info(config), Some(Info::TimeBase(1, 30000))));
        let video = "[Parsed_showinfo_0 @ 0xa39012100] n:  12 pts:  12012 pts_time:0.4004  duration:   1001 \
                     duration_time:0.0333667 fmt:yuv420p cl:left sar:0/1 s:1280x720 i:P iskey:0 type:P";
        let Some(Info::Frame { pts, size, .. }) = parse_info(video) else {
            panic!("not a frame");
        };
        assert_eq!((pts, size), (Some(12012), Some((1280, 720))));
        let audio = "[Parsed_ashowinfo_0 @ 0xb86c30a80] n:1 pts:1024 pts_time:0.0213333 fmt:fltp \
                     channels:2 chlayout:stereo rate:48000 nb_samples:1024 checksum:7346A0E8";
        let Some(Info::Frame {
            pts,
            pts_time,
            rate,
            channels,
            samples,
            ..
        }) = parse_info(audio)
        else {
            panic!("not a frame");
        };
        assert_eq!(
            (pts, rate, channels, samples),
            (Some(1024), Some(48000), Some(2), Some(1024))
        );
        assert!((pts_time.unwrap() - 0.0213333).abs() < 1e-9);
        let nopts = "[Parsed_showinfo_0 @ 0x1] n:   3 pts:NOPTS pts_time:NOPTS s:640x360";
        assert!(matches!(
            parse_info(nopts),
            Some(Info::Frame { pts: None, .. })
        ));
        assert!(parse_info("Stream mapping:").is_none());
        assert!(parse_info("[Parsed_showinfo_0 @ 0x1]   side data - H.26[45] User Data").is_none());
    }
}
