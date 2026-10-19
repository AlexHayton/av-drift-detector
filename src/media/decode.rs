//! Reading captures: per-frame video stamps and a mono audio timeline.

use std::path::Path;

use ffmpeg::format::{Pixel, Sample};
use ffmpeg::software::scaling;
use ffmpeg::{Rational, codec, decoder, format, frame, media, threading};
use ffmpeg_next as ffmpeg;

use crate::pattern::canvas::LumaView;
use crate::pattern::{FrameStamp, decode_frame};
use crate::{Error, FrameRate, Result};

/// One decoded video frame.
#[derive(Clone, Copy, Debug)]
pub struct VideoSample {
    /// Presentation time in seconds (container timeline).
    pub time: f64,
    /// The stamp read from the frame, if any.
    pub stamp: Option<FrameStamp>,
}

/// Decoded audio, downmixed to mono.
#[derive(Clone, Debug)]
pub struct AudioTrack {
    pub sample_rate: u32,
    /// Container time of `samples[0]`.
    pub start_time: f64,
    pub samples: Vec<f32>,
}

/// Everything the analyser needs from a file.
#[derive(Clone, Debug)]
pub struct MediaScan {
    pub video: Vec<VideoSample>,
    pub audio: Option<AudioTrack>,
    pub width: u32,
    pub height: u32,
    /// The video stream's average frame rate, if the container reports one.
    pub stream_fps: Option<FrameRate>,
}

/// Which part of the file to read.
#[derive(Clone, Copy, Debug)]
pub struct ScanWindow {
    /// Video frames before this time (seconds) are skipped.
    pub start: f64,
    /// Video frames at or after this time are skipped; `None` reads to the end.
    pub end: Option<f64>,
    /// Extra audio to keep on either side of the video window, in seconds.
    pub audio_margin: f64,
}

/// Decodes the video stamps and mono audio in `window`.
pub fn scan(path: &Path, window: ScanWindow, try_qr: bool) -> Result<MediaScan> {
    ffmpeg::init()?;
    let mut ictx = format::input(path)?;
    let missing = |kind| Error::MissingStream {
        path: path.to_path_buf(),
        kind,
    };

    let vstream = ictx
        .streams()
        .best(media::Type::Video)
        .ok_or_else(|| missing("video"))?;
    let vindex = vstream.index();
    let vtb = vstream.time_base();
    let avg = vstream.avg_frame_rate();
    let stream_fps = (avg.numerator() > 0 && avg.denominator() > 0)
        .then(|| FrameRate::new(avg.numerator() as u32, avg.denominator() as u32).ok())
        .flatten();
    let mut vctx = codec::context::Context::from_parameters(vstream.parameters())?;
    vctx.set_threading(threading::Config::kind(threading::Type::Frame));
    let mut vdec = vctx.decoder().video()?;
    vdec.set_packet_time_base(vtb);

    let astream = ictx.streams().best(media::Type::Audio);
    let (aindex, atb) = match &astream {
        Some(s) => (Some(s.index()), s.time_base()),
        None => (None, Rational::new(1, 1)),
    };
    let mut adec = match astream {
        Some(s) => {
            let mut d = codec::context::Context::from_parameters(s.parameters())?
                .decoder()
                .audio()?;
            d.set_packet_time_base(atb);
            Some(d)
        }
        None => None,
    };

    let audio_from = window.start - window.audio_margin;
    let audio_to = window.end.map(|e| e + window.audio_margin);
    let seek_to = audio_from - 1.0;
    if seek_to > 0.0 {
        let ts = (seek_to * 1e6) as i64; // AV_TIME_BASE units
        ictx.seek(ts, ..ts)?;
    }

    let mut video = VideoReader {
        tb: vtb,
        window,
        try_qr,
        gray: None,
        samples: Vec::new(),
        done: false,
    };
    let mut audio = AudioAccumulator {
        tb: atb,
        from: audio_from,
        to: audio_to,
        track: None,
        done: aindex.is_none(),
    };

    for (stream, packet) in ictx.packets() {
        let index = stream.index();
        if index == vindex && !video.done {
            send(&mut vdec, &packet)?;
            video.receive(&mut vdec)?;
        } else if Some(index) == aindex && !audio.done {
            let dec = adec.as_mut().expect("audio decoder");
            send(dec, &packet)?;
            audio.receive(dec)?;
        }
        if video.done && audio.done {
            break;
        }
    }
    if !video.done {
        vdec.send_eof()?;
        video.receive(&mut vdec)?;
    }
    if let Some(dec) = adec.as_mut()
        && !audio.done
    {
        dec.send_eof()?;
        audio.receive(dec)?;
    }

    Ok(MediaScan {
        video: video.samples,
        audio: audio.track,
        width: vdec.width(),
        height: vdec.height(),
        stream_fps,
    })
}

/// Sends a packet, tolerating the odd corrupt one.
fn send(dec: &mut decoder::Opened, packet: &ffmpeg::Packet) -> Result<()> {
    match dec.send_packet(packet) {
        Ok(()) | Err(ffmpeg::Error::InvalidData) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

struct VideoReader {
    tb: Rational,
    window: ScanWindow,
    try_qr: bool,
    /// Lazily created converter for frames whose first plane isn't 8-bit luma.
    gray: Option<(scaling::Context, frame::Video)>,
    samples: Vec<VideoSample>,
    done: bool,
}

impl VideoReader {
    fn receive(&mut self, dec: &mut decoder::Video) -> Result<()> {
        let mut decoded = frame::Video::empty();
        while dec.receive_frame(&mut decoded).is_ok() {
            let Some(ts) = decoded.timestamp() else {
                continue;
            };
            let time = ts as f64 * f64::from(self.tb);
            if time < self.window.start - 1e-6 {
                continue;
            }
            if self.window.end.is_some_and(|end| time >= end - 1e-6) {
                self.done = true;
                continue;
            }
            let stamp = self.read_stamp(&decoded)?;
            self.samples.push(VideoSample { time, stamp });
        }
        Ok(())
    }

    fn read_stamp(&mut self, f: &frame::Video) -> Result<Option<FrameStamp>> {
        let (w, h) = (f.width() as usize, f.height() as usize);
        if has_luma_plane(f.format()) {
            let view = LumaView::new(f.data(0), w, h, f.stride(0));
            return Ok(decode_frame(&view, self.try_qr));
        }
        let stale = self.gray.as_ref().is_none_or(|(ctx, out)| {
            ctx.input().format != f.format()
                || out.width() != f.width()
                || out.height() != f.height()
        });
        if stale {
            let ctx = scaling::Context::get(
                f.format(),
                f.width(),
                f.height(),
                Pixel::GRAY8,
                f.width(),
                f.height(),
                scaling::Flags::POINT,
            )?;
            self.gray = Some((ctx, frame::Video::empty()));
        }
        let (ctx, out) = self.gray.as_mut().expect("scaler");
        ctx.run(f, out)?;
        let view = LumaView::new(out.data(0), w, h, out.stride(0));
        Ok(decode_frame(&view, self.try_qr))
    }
}

/// Pixel formats whose first plane is already 8-bit luma.
fn has_luma_plane(format: Pixel) -> bool {
    matches!(
        format,
        Pixel::YUV420P
            | Pixel::YUVJ420P
            | Pixel::YUV422P
            | Pixel::YUVJ422P
            | Pixel::YUV444P
            | Pixel::YUVJ444P
            | Pixel::YUV440P
            | Pixel::YUVJ440P
            | Pixel::YUV411P
            | Pixel::YUV410P
            | Pixel::YUVA420P
            | Pixel::NV12
            | Pixel::NV21
            | Pixel::GRAY8
    )
}

struct AudioAccumulator {
    tb: Rational,
    from: f64,
    to: Option<f64>,
    track: Option<AudioTrack>,
    done: bool,
}

impl AudioAccumulator {
    fn receive(&mut self, dec: &mut decoder::Audio) -> Result<()> {
        let mut decoded = frame::Audio::empty();
        while dec.receive_frame(&mut decoded).is_ok() {
            let Some(ts) = decoded.timestamp().or(decoded.pts()) else {
                continue;
            };
            let time = ts as f64 * f64::from(self.tb);
            let mono = downmix(&decoded)?;
            self.push(time, decoded.rate(), &mono)?;
        }
        Ok(())
    }

    /// Appends samples that start at container time `time`, keeping the
    /// timeline continuous: gaps are filled with silence and overlaps dropped.
    fn push(&mut self, time: f64, rate: u32, data: &[f32]) -> Result<()> {
        let r = f64::from(rate);
        let track = match &mut self.track {
            Some(track) => {
                if track.sample_rate != rate {
                    return Err(Error::Unsupported(
                        "audio sample rate changes mid-stream".into(),
                    ));
                }
                track
            }
            None => {
                let skip = ((self.from - time) * r).ceil().max(0.0) as usize;
                if skip >= data.len() {
                    return Ok(());
                }
                self.track = Some(AudioTrack {
                    sample_rate: rate,
                    start_time: time + skip as f64 / r,
                    samples: Vec::new(),
                });
                let track = self.track.as_mut().expect("track");
                track.samples.extend_from_slice(&data[skip..]);
                self.trim();
                return Ok(());
            }
        };
        // Containers with coarse time bases (MKV uses 1 ms) round every frame's
        // timestamp, so small mismatches are rounding, not real gaps.
        let tolerance = 0.002f64.max(1.5 * f64::from(self.tb));
        let mismatch = time - track.start_time - track.samples.len() as f64 / r;
        let gap = if mismatch.abs() <= tolerance {
            0
        } else {
            (mismatch * r).round() as i64
        };
        let data = if gap > 0 {
            track
                .samples
                .resize(track.samples.len() + gap as usize, 0.0);
            data
        } else {
            &data[((-gap) as usize).min(data.len())..]
        };
        track.samples.extend_from_slice(data);
        self.trim();
        Ok(())
    }

    fn trim(&mut self) {
        let (Some(to), Some(track)) = (self.to, self.track.as_mut()) else {
            return;
        };
        let keep = ((to - track.start_time) * f64::from(track.sample_rate))
            .ceil()
            .max(0.0) as usize;
        if track.samples.len() >= keep {
            track.samples.truncate(keep);
            self.done = true;
        }
    }
}

/// Averages all channels into one f32 channel.
fn downmix(f: &frame::Audio) -> Result<Vec<f32>> {
    let channels = usize::from(f.channels().max(1));
    let n = f.samples();
    let format = f.format();
    let mut out = vec![0.0f32; n];
    macro_rules! mix {
        ($t:ty, $scale:expr) => {{
            let scale = $scale;
            if format.is_planar() {
                for ch in 0..channels {
                    for (o, &v) in out.iter_mut().zip(&f.plane::<$t>(ch)[..n]) {
                        *o += scale(v);
                    }
                }
            } else {
                let data = &f.plane::<$t>(0)[..n * channels];
                for (o, frame) in out.iter_mut().zip(data.chunks_exact(channels)) {
                    *o = frame.iter().map(|&v| scale(v)).sum();
                }
            }
        }};
    }
    match format {
        Sample::F32(_) => mix!(f32, |v: f32| v),
        Sample::F64(_) => mix!(f64, |v: f64| v as f32),
        Sample::I16(_) => mix!(i16, |v: i16| f32::from(v) / 32768.0),
        Sample::I32(_) => mix!(i32, |v: i32| v as f32 / 2_147_483_648.0),
        Sample::U8(_) => mix!(u8, |v: u8| (f32::from(v) - 128.0) / 128.0),
        other => return Err(Error::Unsupported(format!("audio sample format {other:?}"))),
    }
    if channels > 1 {
        let k = 1.0 / channels as f32;
        out.iter_mut().for_each(|v| *v *= k);
    }
    Ok(out)
}
