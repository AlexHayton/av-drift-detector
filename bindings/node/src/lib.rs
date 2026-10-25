//! Node.js bindings for `av-drift-detector`: the npm package of the same name.
//!
//! Files are read and written by running the `ffmpeg` and `ffprobe` programs,
//! so the addon links nothing from FFmpeg.
//!
//! Slow work (reading or writing a file, analysing a capture) runs on libuv's
//! thread pool and returns a promise. Results are plain objects with
//! camelCase fields; `JSON.stringify` works on all of them.

use av_drift_detector as avd;
use avd::media::{AudioTrack, MediaScan, VideoSample};
use avd::pattern::canvas::{LumaImage, LumaView};
use avd::pattern::{StampSource, decode_frame};
use avd::{CaptureMode, FrameRate, FrameStatus};
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;

fn reason(e: impl std::fmt::Display) -> Error {
    Error::from_reason(e.to_string())
}

/// Async functions report bad arguments by rejecting, not by throwing: tasks
/// carry the error to `compute`.
fn take<T>(prepared: &mut Result<T>) -> Result<T> {
    std::mem::replace(prepared, Err(reason("task already ran")))
}

fn fps(text: Option<&str>) -> Result<FrameRate> {
    text.unwrap_or("30").parse().map_err(reason)
}

// ---------------------------------------------------------------- reports --

/// Drift measured at one captured video frame.
#[napi(object)]
pub struct FrameDrift {
    /// Presentation time of the captured frame, in seconds.
    pub time: f64,
    /// Source frame number read from the picture.
    pub video_frame: Option<u32>,
    /// Source video position at `time`, in (fractional) frames.
    pub video_position: Option<f64>,
    #[napi(ts_type = "'barcode' | 'barcode_blend' | 'qr' | 'qr_blend'")]
    pub stamp_source: Option<String>,
    /// How far the picture had switched to the next frame (0 to 1), where it
    /// showed a blend of two frames.
    pub video_blend: Option<f64>,
    /// Source position of the audio playing at `time`, in (fractional) frames.
    pub audio_frame: Option<f64>,
    /// Video minus audio, in milliseconds: positive means the audio is late.
    pub drift_ms: Option<f64>,
    /// Source frames missing between the previous readable frame and this one.
    pub dropped_before: u32,
    /// Whether this frame repeats the previous readable frame's number.
    pub repeated: bool,
    #[napi(ts_type = "'ok' | 'no_video_stamp' | 'no_audio'")]
    pub status: String,
}

/// Statistics over a report's frames (drift in ms; positive = audio late).
#[napi(object)]
pub struct DriftSummary {
    pub frames: u32,
    pub measured: u32,
    pub mean_ms: Option<f64>,
    pub median_ms: Option<f64>,
    pub min_ms: Option<f64>,
    pub max_ms: Option<f64>,
    /// Standard deviation of the drift (jitter).
    pub stddev_ms: Option<f64>,
    /// Slope of a least-squares line through drift against time.
    pub skew_ms_per_min: Option<f64>,
    pub skew_ppm: Option<f64>,
    pub dropped_frames: u32,
    pub repeated_frames: u32,
    pub backward_jumps: u32,
    pub no_video_stamp: u32,
    pub no_audio: u32,
    pub audio_cells: u32,
    pub audio_outliers: u32,
    /// Median level of each decoded audio cell's weakest tone, in dBFS.
    pub audio_level_dbfs: Option<f64>,
    /// Median height of each cell's weakest tone above the noise, in dB.
    pub audio_snr_db: Option<f64>,
    /// Frames where a QR code was found but couldn't be read.
    pub qr_located_unread: u32,
    /// Frames read by fitting the QR code against rendered frames' codes.
    pub qr_blend_frames: u32,
    /// Frames that caught the display switching between two frames.
    pub mixed_frames: u32,
}

/// What a capture would measure if it were the other kind (`DriftReport.alternative`).
#[napi(object)]
pub struct AlternativeReading {
    #[napi(ts_type = "'file' | 'camera'")]
    pub capture: String,
    pub mean_ms: f64,
    pub video_timing_uncertainty_ms: Option<f64>,
}

/// The result of analysing a capture: the same content as `av-drift analyze --json`.
#[napi(object)]
pub struct DriftReport {
    pub input: String,
    /// The pattern's exact frame rate, such as `30000/1001`.
    pub fps: String,
    /// The same rate as a number.
    pub fps_value: f64,
    #[napi(ts_type = "'barcode' | 'qr' | 'option'")]
    pub fps_source: String,
    #[napi(ts_type = "'file' | 'camera'")]
    pub capture: String,
    /// Camera captures: ± how well the display's frame timing is known.
    pub video_timing_uncertainty_ms: Option<f64>,
    /// Camera captures: how the display's timeline was found.
    #[napi(ts_type = "'blend' | 'bounds'")]
    pub video_timing: Option<String>,
    /// The camera's exposure time, where the camera reported it (the median)
    /// or the capture let it be measured.
    pub exposure_ms: Option<f64>,
    /// Where `exposureMs` came from: reported by the camera with each frame,
    /// or measured from the blends.
    #[napi(ts_type = "'reported' | 'measured'")]
    pub exposure_source: Option<String>,
    /// The longest exposure allowed for, where it couldn't be measured.
    pub max_exposure_ms: Option<f64>,
    /// Frames the display showed noticeably early or late.
    pub display_judder: u32,
    /// When the stamps fit both a digital copy and a camera locked to the
    /// pattern's rate: what the other reading would say.
    pub alternative: Option<AlternativeReading>,
    pub width: u32,
    pub height: u32,
    pub offset: f64,
    pub duration: Option<f64>,
    pub summary: DriftSummary,
    pub frames: Vec<FrameDrift>,
}

fn source_name(source: StampSource) -> String {
    source.name().into()
}

fn capture_name(capture: CaptureMode) -> String {
    match capture {
        CaptureMode::Auto => "auto",
        CaptureMode::File => "file",
        CaptureMode::Camera => "camera",
    }
    .into()
}

fn count(n: impl TryInto<u32>) -> u32 {
    n.try_into().unwrap_or(u32::MAX)
}

impl From<avd::DriftReport> for DriftReport {
    fn from(r: avd::DriftReport) -> Self {
        let s = r.summary;
        DriftReport {
            input: r.input,
            fps: r.fps.to_string(),
            fps_value: r.fps.as_f64(),
            fps_source: r.fps_source.into(),
            capture: capture_name(r.capture),
            video_timing_uncertainty_ms: r.video_timing_uncertainty_ms,
            video_timing: r.video_timing.map(Into::into),
            exposure_ms: r.exposure_ms,
            exposure_source: r.exposure_source.map(Into::into),
            max_exposure_ms: r.max_exposure_ms,
            display_judder: count(r.display_judder),
            alternative: r.alternative.map(|a| AlternativeReading {
                capture: capture_name(a.capture),
                mean_ms: a.mean_ms,
                video_timing_uncertainty_ms: a.video_timing_uncertainty_ms,
            }),
            width: r.width,
            height: r.height,
            offset: r.offset,
            duration: r.duration,
            summary: DriftSummary {
                frames: count(s.frames),
                measured: count(s.measured),
                mean_ms: s.mean_ms,
                median_ms: s.median_ms,
                min_ms: s.min_ms,
                max_ms: s.max_ms,
                stddev_ms: s.stddev_ms,
                skew_ms_per_min: s.skew_ms_per_min,
                skew_ppm: s.skew_ppm,
                dropped_frames: count(s.dropped_frames),
                repeated_frames: count(s.repeated_frames),
                backward_jumps: count(s.backward_jumps),
                no_video_stamp: count(s.no_video_stamp),
                no_audio: count(s.no_audio),
                audio_cells: count(s.audio_cells),
                audio_outliers: count(s.audio_outliers),
                audio_level_dbfs: s.audio_level_dbfs,
                audio_snr_db: s.audio_snr_db,
                qr_located_unread: count(s.qr_located_unread),
                qr_blend_frames: count(s.qr_blend_frames),
                mixed_frames: count(s.mixed_frames),
            },
            frames: r
                .frames
                .into_iter()
                .map(|f| FrameDrift {
                    time: f.time,
                    video_frame: f.video_frame,
                    video_position: f.video_position,
                    stamp_source: f.stamp_source.map(source_name),
                    video_blend: f.video_blend.map(f64::from),
                    audio_frame: f.audio_frame,
                    drift_ms: f.drift_ms,
                    dropped_before: f.dropped_before,
                    repeated: f.repeated,
                    status: match f.status {
                        FrameStatus::Ok => "ok",
                        FrameStatus::NoVideoStamp => "no_video_stamp",
                        FrameStatus::NoAudio => "no_audio",
                    }
                    .into(),
                })
                .collect(),
        }
    }
}

// ---------------------------------------------------------------- analyze --

/// Settings for `analyze` and `analyzeFrames`. Every field is optional.
#[napi(object)]
#[derive(Default)]
pub struct AnalyzeOptions {
    /// Start of the analysis window, in seconds (default 0).
    pub offset: Option<f64>,
    /// Length of the analysis window in seconds (default: to the end).
    pub duration: Option<f64>,
    /// The pattern's frame rate (`"29.97"`, `"30000/1001"`), overriding what the stamps say.
    pub fps: Option<String>,
    /// Fall back to the QR code when a barcode can't be read (default true).
    pub use_qr: Option<bool>,
    /// How frames are timed (default `'auto'`: camera if most stamps came from QR codes).
    #[napi(ts_type = "'auto' | 'file' | 'camera'")]
    pub capture: Option<String>,
}

impl AnalyzeOptions {
    fn to_core(&self) -> Result<avd::AnalyzeOptions> {
        let defaults = avd::AnalyzeOptions::default();
        Ok(avd::AnalyzeOptions {
            offset: self.offset.unwrap_or(defaults.offset),
            duration: self.duration,
            fps: self
                .fps
                .as_deref()
                .map(str::parse)
                .transpose()
                .map_err(reason)?,
            use_qr: self.use_qr.unwrap_or(defaults.use_qr),
            capture: match &self.capture {
                Some(c) => c.parse().map_err(reason)?,
                None => defaults.capture,
            },
        })
    }
}

pub struct AnalyzeTask {
    path: String,
    options: Result<avd::AnalyzeOptions>,
}

impl Task for AnalyzeTask {
    type Output = avd::DriftReport;
    type JsValue = DriftReport;

    fn compute(&mut self) -> Result<Self::Output> {
        let options = take(&mut self.options)?;
        avd::media::ffmpeg_cli::analyze(self.path.as_ref(), &options).map_err(reason)
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output.into())
    }
}

/// Measures the A/V drift of every frame in a capture of a test video, as
/// `av-drift analyze` does. Decodes by running the `ffmpeg` and `ffprobe`
/// programs (FFmpeg 5.1+, on `PATH` or at `AV_DRIFT_FFMPEG` and `AV_DRIFT_FFPROBE`).
#[napi(ts_return_type = "Promise<DriftReport>")]
pub fn analyze(path: String, options: Option<AnalyzeOptions>) -> AsyncTask<AnalyzeTask> {
    let options = options.unwrap_or_default().to_core();
    AsyncTask::new(AnalyzeTask { path, options })
}

// --------------------------------------------------------------- generate --

/// Settings for `generate`. Only `output` is required.
#[napi(object)]
pub struct GenerateOptions {
    /// Output file: `.mp4`, `.mov` or `.mkv`.
    pub output: String,
    /// Frame rate (default `"30"`): `"29.97"`, `"30000/1001"`, `"60"`, ...
    pub fps: Option<String>,
    /// Duration in seconds (default 10), rounded to whole frames.
    pub duration: Option<f64>,
    /// Frame size (default 1280×720).
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Audio sample rate in Hz (default 48000).
    pub sample_rate: Option<u32>,
    /// Default `'h264'`; `'ffv1'` (lossless) needs `.mov` or `.mkv`.
    #[napi(ts_type = "'h264' | 'ffv1'")]
    pub video_codec: Option<String>,
    /// Default `'aac'`; `'pcm'` needs `.mov` or `.mkv`.
    #[napi(ts_type = "'aac' | 'pcm'")]
    pub audio_codec: Option<String>,
    /// A deliberate audio delay in seconds: positive makes the audio late.
    pub audio_offset: Option<f64>,
    /// Mix music in under the code tones (default true; up to about 60 fps).
    /// It doesn't affect analysis.
    pub music: Option<bool>,
}

/// What `generate` wrote.
#[napi(object)]
pub struct GenerateSummary {
    pub output: String,
    pub frames: u32,
    pub fps: String,
    /// Seconds of video.
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    pub audio_offset: f64,
}

/// Progress through `generate`, reported about once per percent.
#[napi(object)]
pub struct GenerateProgress {
    pub frames_done: u32,
    pub frames_total: u32,
}

type ProgressCallback = ThreadsafeFunction<GenerateProgress, (), GenerateProgress, Status, false>;

pub struct GenerateTask {
    options: Result<avd::GenerateOptions>,
    on_progress: Option<ProgressCallback>,
}

impl Task for GenerateTask {
    type Output = GenerateSummary;
    type JsValue = GenerateSummary;

    fn compute(&mut self) -> Result<Self::Output> {
        let options = take(&mut self.options)?;
        let callback = self.on_progress.as_ref();
        let mut last = u64::MAX;
        avd::media::ffmpeg_cli::generate(&options, |p| {
            let Some(callback) = callback else { return };
            let percent = p.frames_done * 100 / p.frames_total.max(1);
            if percent != last || p.frames_done == p.frames_total {
                last = percent;
                let progress = GenerateProgress {
                    frames_done: count(p.frames_done),
                    frames_total: count(p.frames_total),
                };
                callback.call(progress, ThreadsafeFunctionCallMode::NonBlocking);
            }
        })
        .map_err(reason)?;
        let o = &options;
        let frames = o.total_frames();
        Ok(GenerateSummary {
            output: o.output.display().to_string(),
            frames: count(frames),
            fps: o.fps.to_string(),
            duration: frames as f64 * o.fps.frame_duration(),
            width: o.width,
            height: o.height,
            audio_offset: o.audio_delay,
        })
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

/// Writes a test video: every frame shows its number (barcode, QR code,
/// timecode) and every frame's worth of audio carries it as tones. Encodes by
/// running the `ffmpeg` program (H.264 needs a build with libx264).
/// Progress calls can arrive just after the promise settles.
#[napi(ts_return_type = "Promise<GenerateSummary>")]
pub fn generate(
    options: GenerateOptions,
    #[napi(ts_arg_type = "(progress: GenerateProgress) => void")] on_progress: Option<
        ProgressCallback,
    >,
) -> AsyncTask<GenerateTask> {
    AsyncTask::new(GenerateTask {
        options: options.to_core(),
        on_progress,
    })
}

impl GenerateOptions {
    fn to_core(&self) -> Result<avd::GenerateOptions> {
        let options = self;
        let mut core = avd::GenerateOptions::new(
            &options.output,
            fps(options.fps.as_deref())?,
            options.duration.unwrap_or(10.0),
        );
        if let Some(w) = options.width {
            core.width = w;
        }
        if let Some(h) = options.height {
            core.height = h;
        }
        if let Some(rate) = options.sample_rate {
            core.sample_rate = rate;
        }
        if let Some(codec) = &options.video_codec {
            core.video_codec = codec.parse().map_err(reason)?;
        }
        if let Some(codec) = &options.audio_codec {
            core.audio_codec = codec.parse().map_err(reason)?;
        }
        core.audio_delay = options.audio_offset.unwrap_or(0.0);
        core.music = options.music.unwrap_or(true);
        Ok(core)
    }
}

// -------------------------------------------- frames and audio in memory --

/// A picture read as a mix of two consecutive frames.
#[napi(object)]
pub struct Blend {
    /// The earlier frame: the picture mixes `first` and `first + 1`.
    pub first: u32,
    /// The weight of `first + 1` (0 to 1): the share of the exposure after the switch.
    pub weight_next: f64,
    /// How much the weight grows from the code's top row to its bottom row.
    pub slope: f64,
    /// Root-mean-square error of the fit (black 0, white 1).
    pub rms: f64,
    /// The error of the best fit naming another frame, over `rms`.
    pub margin: f64,
}

/// A frame number read from a picture.
#[napi(object)]
pub struct FrameStamp {
    pub frame_id: u32,
    /// The pattern's exact frame rate, if the stamp carries it.
    pub fps: Option<String>,
    /// The audio tone plan's base harmonic (QR codes only).
    pub tone_base_harmonic: Option<u32>,
    #[napi(ts_type = "'barcode' | 'barcode_blend' | 'qr' | 'qr_blend'")]
    pub source: String,
    /// The QR code's corners in frame pixels (top left, top right, bottom
    /// right, bottom left, as printed): `[x, y]` pairs.
    pub corners: Option<Vec<Vec<f64>>>,
    /// How the picture mixed two consecutive frames, where measured.
    pub blend: Option<Blend>,
}

impl From<avd::pattern::FrameStamp> for FrameStamp {
    fn from(s: avd::pattern::FrameStamp) -> Self {
        FrameStamp {
            frame_id: s.frame_id,
            fps: s.fps.map(|f| f.to_string()),
            tone_base_harmonic: s.tone_base_harmonic,
            source: source_name(s.source),
            corners: s.corners.map(|c| {
                c.iter()
                    .map(|&(x, y)| vec![f64::from(x), f64::from(y)])
                    .collect()
            }),
            blend: s.blend.map(|b| Blend {
                first: b.first,
                weight_next: f64::from(b.weight_next),
                slope: f64::from(b.slope),
                rms: f64::from(b.rms),
                margin: f64::from(b.margin),
            }),
        }
    }
}

impl FrameStamp {
    fn to_core(&self) -> Result<avd::pattern::FrameStamp> {
        Ok(avd::pattern::FrameStamp {
            frame_id: self.frame_id,
            fps: self
                .fps
                .as_deref()
                .map(str::parse)
                .transpose()
                .map_err(reason)?,
            tone_base_harmonic: self.tone_base_harmonic,
            source: self.source.parse().map_err(reason)?,
            corners: self
                .corners
                .as_ref()
                .map(|c| -> Result<[(f32, f32); 4]> {
                    let points: Vec<(f32, f32)> = c
                        .iter()
                        .map(|p| match p.as_slice() {
                            [x, y] => Ok((*x as f32, *y as f32)),
                            _ => Err(reason("each corner must be an [x, y] pair")),
                        })
                        .collect::<Result<_>>()?;
                    points
                        .try_into()
                        .map_err(|_| reason("corners must be four [x, y] points"))
                })
                .transpose()?,
            blend: self.blend.as_ref().map(|b| avd::pattern::Blend {
                first: b.first,
                weight_next: b.weight_next as f32,
                slope: b.slope as f32,
                rms: b.rms as f32,
                margin: b.margin as f32,
            }),
        })
    }
}

/// Reads the frame stamp from an 8-bit luma (Y) plane: the barcode, then
/// (unless `tryQr` is false) the QR code. Returns null if neither reads.
#[napi]
pub fn read_frame(
    luma: Uint8Array,
    width: u32,
    height: u32,
    stride: Option<u32>,
    try_qr: Option<bool>,
) -> Result<Option<FrameStamp>> {
    let view = plane(&luma, width, height, stride)?;
    Ok(decode_frame(&view, try_qr.unwrap_or(true)).map(Into::into))
}

/// A `width × height` luma plane, checked against its size.
fn plane(luma: &[u8], width: u32, height: u32, stride: Option<u32>) -> Result<LumaView<'_>> {
    let (w, h) = (width as usize, height as usize);
    let stride = stride.map_or(w, |s| s as usize);
    if w == 0 || h == 0 || stride < w || luma.len() < stride * (h - 1) + w {
        return Err(reason(format!(
            "a {width}×{height} plane with stride {stride} needs {} bytes, got {}",
            stride * h.saturating_sub(1) + w,
            luma.len()
        )));
    }
    Ok(LumaView::new(luma, w, h, stride))
}

/// One captured video frame: when it was shown, and its stamp (if readable).
#[napi(object)]
pub struct CapturedFrame {
    /// Seconds, on the same clock as the audio's `startTime`.
    pub time: f64,
    pub stamp: Option<FrameStamp>,
    /// Whether a QR code was found, even if it couldn't be read.
    pub qr_located: Option<bool>,
    /// Camera frames: the exposure time in seconds, if the camera reported it.
    pub exposure: Option<f64>,
    /// Camera frames: the rolling shutter's readout in seconds, if the camera
    /// reported it: row `y` began its exposure `readout · y / height` after `time`.
    pub readout: Option<f64>,
}

impl From<&VideoSample> for CapturedFrame {
    fn from(v: &VideoSample) -> Self {
        CapturedFrame {
            time: v.time,
            stamp: v.stamp.map(Into::into),
            qr_located: Some(v.qr_located),
            exposure: v.exposure,
            readout: v.readout,
        }
    }
}

/// Reads the stamps of consecutive frames of one capture. It remembers where
/// the QR code was, which is faster, and reads codes caught between two
/// frames that `readFrame` can't.
#[napi]
pub struct StampReader {
    inner: avd::pattern::StampReader,
    frames: Vec<VideoSample>,
}

#[napi]
impl StampReader {
    /// A reader that falls back to the QR code unless `tryQr` is false.
    #[napi(constructor)]
    pub fn new(try_qr: Option<bool>) -> Self {
        StampReader {
            inner: avd::pattern::StampReader::new(try_qr.unwrap_or(true)),
            frames: Vec::new(),
        }
    }

    /// Reads the next frame: an 8-bit luma (Y) plane shown at `time` seconds
    /// (for a camera, when its top row's exposure began). Pass the camera's
    /// `exposure` and rolling-shutter `readout` (seconds) if it reports them.
    #[napi]
    #[allow(clippy::too_many_arguments)]
    pub fn read(
        &mut self,
        luma: Uint8Array,
        width: u32,
        height: u32,
        time: f64,
        stride: Option<u32>,
        exposure: Option<f64>,
        readout: Option<f64>,
    ) -> Result<CapturedFrame> {
        let view = plane(&luma, width, height, stride)?;
        let sample = VideoSample {
            exposure,
            readout,
            ..self.inner.read(&view, time)
        };
        self.frames.push(sample);
        Ok((&sample).into())
    }

    /// Every frame read so far, with any that could only be identified after
    /// seeing later frames filled in: pass these to `analyzeFrames`.
    #[napi]
    pub fn finish(&mut self) -> Vec<CapturedFrame> {
        self.inner.finish(&mut self.frames);
        self.frames.iter().map(Into::into).collect()
    }
}

/// Captured audio, downmixed to mono.
#[napi(object)]
pub struct CapturedAudio {
    pub sample_rate: u32,
    /// Time of the first sample, in seconds.
    pub start_time: f64,
    pub samples: Float32Array,
}

/// A capture decoded by your own code, for `analyzeFrames`.
#[napi(object)]
pub struct Capture {
    pub frames: Vec<CapturedFrame>,
    pub audio: Option<CapturedAudio>,
    pub width: u32,
    pub height: u32,
}

pub struct AnalyzeFramesTask {
    scan: Result<MediaScan>,
    options: Result<avd::AnalyzeOptions>,
}

impl Task for AnalyzeFramesTask {
    type Output = avd::DriftReport;
    type JsValue = DriftReport;

    fn compute(&mut self) -> Result<Self::Output> {
        let (scan, options) = (take(&mut self.scan)?, take(&mut self.options)?);
        avd::analyze::analyze_scan(&scan, &options, "frames".into()).map_err(reason)
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output.into())
    }
}

/// Measures drift from frames and audio you decoded yourself (a capture card,
/// WebRTC, a camera): stamps from `readFrame`, timestamps on one clock.
#[napi(ts_return_type = "Promise<DriftReport>")]
pub fn analyze_frames(
    capture: Capture,
    options: Option<AnalyzeOptions>,
) -> AsyncTask<AnalyzeFramesTask> {
    AsyncTask::new(AnalyzeFramesTask {
        scan: capture.into_core(),
        options: options.unwrap_or_default().to_core(),
    })
}

impl Capture {
    fn into_core(self) -> Result<MediaScan> {
        let capture = self;
        let video = capture
            .frames
            .iter()
            .map(|f| {
                Ok(VideoSample {
                    time: f.time,
                    stamp: f.stamp.as_ref().map(FrameStamp::to_core).transpose()?,
                    qr_located: f.qr_located.unwrap_or(false),
                    old_format: false,
                    exposure: f.exposure,
                    readout: f.readout,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let audio = capture.audio.map(|a| AudioTrack {
            sample_rate: a.sample_rate,
            start_time: a.start_time,
            samples: a.samples.to_vec(),
        });
        Ok(MediaScan {
            video,
            audio,
            width: capture.width,
            height: capture.height,
            stream_fps: None,
        })
    }
}

/// Settings for `PatternRenderer` and `synthesizeAudio`.
#[napi(object)]
#[derive(Default)]
pub struct PatternOptions {
    /// Frame rate (default `"30"`).
    pub fps: Option<String>,
    /// Frame size (default 1280×720).
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Audio sample rate in Hz (default 48000); the QR code describes the matching tone plan.
    pub sample_rate: Option<u32>,
    /// The file's length in seconds, for the info line (rounded to whole frames).
    pub duration: Option<f64>,
}

/// Renders test-pattern frames into 8-bit luma planes (the Y plane of a
/// grey-scale YUV frame), without FFmpeg.
#[napi]
pub struct PatternRenderer {
    inner: avd::pattern::PatternRenderer,
    width: u32,
    height: u32,
}

#[napi]
impl PatternRenderer {
    #[napi(constructor)]
    pub fn new(options: Option<PatternOptions>) -> Result<Self> {
        let o = options.unwrap_or_default();
        let (width, height) = (o.width.unwrap_or(1280), o.height.unwrap_or(720));
        let rate = fps(o.fps.as_deref())?;
        let tones =
            avd::audio::ToneCode::new(rate, o.sample_rate.unwrap_or(48_000)).map_err(reason)?;
        let mut inner = avd::pattern::PatternRenderer::new(width as usize, height as usize, tones)
            .map_err(reason)?;
        if let Some(seconds) = o.duration {
            if !(seconds.is_finite() && seconds > 0.0) {
                return Err(reason(format!("duration must be positive, got {seconds}")));
            }
            inner = inner.with_total_frames(rate.frames_in(seconds));
        }
        Ok(PatternRenderer {
            inner,
            width,
            height,
        })
    }

    #[napi(getter)]
    pub fn width(&self) -> u32 {
        self.width
    }

    #[napi(getter)]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Frame `frameId` as `width × height` bytes of luma (16 = black, 235 = white).
    #[napi]
    pub fn render(&self, frame_id: u32) -> Result<Buffer> {
        let mut image = LumaImage::new(self.width as usize, self.height as usize, 0);
        self.inner
            .render(frame_id, &mut image.canvas())
            .map_err(reason)?;
        Ok(image.data.into())
    }
}

/// Settings for `synthesizeAudio`.
#[napi(object)]
pub struct AudioOptions {
    /// Frame rate (default `"30"`).
    pub fps: Option<String>,
    /// Sample rate in Hz (default 48000).
    pub sample_rate: Option<u32>,
    /// A deliberate delay in seconds: positive makes the audio late (default 0).
    pub delay: Option<f64>,
    /// Index of the first sample (default 0), to synthesize a stream in pieces.
    pub start: Option<i64>,
    /// How many samples to make.
    pub count: u32,
    /// Mix in the music `generate` adds (default true). It doesn't affect analysis.
    pub music: Option<bool>,
}

/// The test pattern's audio (mono): the tones for each frame's number, and music.
#[napi]
pub fn synthesize_audio(options: AudioOptions) -> Result<Float32Array> {
    let code = avd::audio::ToneCode::new(
        fps(options.fps.as_deref())?,
        options.sample_rate.unwrap_or(48_000),
    )
    .map_err(reason)?;
    let start = u64::try_from(options.start.unwrap_or(0)).map_err(reason)?;
    let mut synth =
        avd::audio::ToneSynth::new(code, options.delay.unwrap_or(0.0)).starting_at(start);
    if options.music.unwrap_or(true) {
        synth = synth.with_music();
    }
    let mut samples = vec![0.0; options.count as usize];
    synth.fill(&mut samples);
    Ok(Float32Array::new(samples))
}
