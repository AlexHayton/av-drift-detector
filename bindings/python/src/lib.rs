//! Python bindings for `av-drift-detector`: the `av_drift_detector._native`
//! module, wrapped by the pure-Python package in `python/av_drift_detector`.
//!
//! Reports cross as the JSON `av-drift analyze --json` writes, which the
//! Python layer parses into dicts. Slow calls release the GIL. Files are read
//! and written by running the `ffmpeg` and `ffprobe` programs, so the module
//! links nothing from FFmpeg.

use std::path::PathBuf;

use av_drift_detector as avd;
use avd::FrameRate;
use avd::media::{AudioTrack, MediaScan, VideoSample};
use avd::pattern::canvas::{LumaImage, LumaView};
use avd::pattern::{StampSource, decode_frame};
use pyo3::buffer::PyBuffer;
use pyo3::exceptions::{PyException, PyKeyError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};

pyo3::create_exception!(
    _native,
    AvDriftError,
    PyException,
    "An analysis, generation or option error from av-drift-detector."
);

fn error(e: impl std::fmt::Display) -> PyErr {
    AvDriftError::new_err(e.to_string())
}

fn frame_rate(text: &str) -> PyResult<FrameRate> {
    text.parse().map_err(error)
}

fn report_json(report: &avd::DriftReport) -> PyResult<String> {
    serde_json::to_string(report).map_err(error)
}

fn analyze_options(
    offset: f64,
    duration: Option<f64>,
    fps: Option<&str>,
    use_qr: bool,
    capture: &str,
) -> PyResult<avd::AnalyzeOptions> {
    Ok(avd::AnalyzeOptions {
        offset,
        duration,
        fps: fps.map(frame_rate).transpose()?,
        use_qr,
        capture: capture.parse().map_err(error)?,
    })
}

/// Measures drift in a capture file; returns the report as JSON.
#[pyfunction]
#[pyo3(signature = (path, *, offset=0.0, duration=None, fps=None, use_qr=true, capture="auto"))]
fn analyze(
    py: Python<'_>,
    path: PathBuf,
    offset: f64,
    duration: Option<f64>,
    fps: Option<&str>,
    use_qr: bool,
    capture: &str,
) -> PyResult<String> {
    let options = analyze_options(offset, duration, fps, use_qr, capture)?;
    let report = py.detach(|| avd::media::ffmpeg_cli::analyze(&path, &options));
    report_json(&report.map_err(error)?)
}

/// Writes a test video; returns a summary of what was written.
#[pyfunction]
#[pyo3(signature = (
    output, *, fps="30", duration=10.0, width=1280, height=720, sample_rate=48000,
    video_codec="h264", audio_codec="aac", audio_offset=0.0, on_progress=None,
))]
#[allow(clippy::too_many_arguments)]
fn generate<'py>(
    py: Python<'py>,
    output: PathBuf,
    fps: &str,
    duration: f64,
    width: u32,
    height: u32,
    sample_rate: u32,
    video_codec: &str,
    audio_codec: &str,
    audio_offset: f64,
    on_progress: Option<Py<PyAny>>,
) -> PyResult<Bound<'py, PyDict>> {
    let mut opts = avd::GenerateOptions::new(output, frame_rate(fps)?, duration);
    opts.width = width;
    opts.height = height;
    opts.sample_rate = sample_rate;
    opts.video_codec = video_codec.parse().map_err(error)?;
    opts.audio_codec = audio_codec.parse().map_err(error)?;
    opts.audio_delay = audio_offset;

    // The callback's first exception stops further calls and is raised at the end.
    let mut raised: Option<PyErr> = None;
    let result = py.detach(|| {
        let mut last = u64::MAX;
        avd::media::ffmpeg_cli::generate(&opts, |p| {
            let Some(callback) = &on_progress else { return };
            let percent = p.frames_done * 100 / p.frames_total.max(1);
            if raised.is_some() || (percent == last && p.frames_done != p.frames_total) {
                return;
            }
            last = percent;
            Python::attach(|py| {
                if let Err(e) = callback.call1(py, (p.frames_done, p.frames_total)) {
                    raised = Some(e);
                }
            });
        })
    });
    result.map_err(error)?;
    if let Some(e) = raised {
        return Err(e);
    }

    let frames = opts.total_frames();
    let summary = PyDict::new(py);
    summary.set_item("output", opts.output.display().to_string())?;
    summary.set_item("frames", frames)?;
    summary.set_item("fps", opts.fps.to_string())?;
    summary.set_item("duration", frames as f64 * opts.fps.frame_duration())?;
    summary.set_item("width", opts.width)?;
    summary.set_item("height", opts.height)?;
    summary.set_item("audio_offset", opts.audio_delay)?;
    Ok(summary)
}

fn stamp_dict<'py>(py: Python<'py>, s: avd::pattern::FrameStamp) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    d.set_item("frame_id", s.frame_id)?;
    d.set_item("fps", s.fps.map(|f| f.to_string()))?;
    d.set_item("tone_base_harmonic", s.tone_base_harmonic)?;
    d.set_item(
        "source",
        match s.source {
            StampSource::Barcode => "barcode",
            StampSource::Qr => "qr",
        },
    )?;
    Ok(d)
}

/// `obj[key]`, or None when it's missing or None.
fn optional<'py>(obj: &Bound<'py, PyAny>, key: &str) -> PyResult<Option<Bound<'py, PyAny>>> {
    match obj.get_item(key) {
        Ok(v) if v.is_none() => Ok(None),
        Ok(v) => Ok(Some(v)),
        Err(e) if e.is_instance_of::<PyKeyError>(obj.py()) => Ok(None),
        Err(e) => Err(e),
    }
}

fn stamp_from(obj: &Bound<'_, PyAny>) -> PyResult<avd::pattern::FrameStamp> {
    let fps = optional(obj, "fps")?
        .map(|f| f.extract::<String>().and_then(|f| frame_rate(&f)))
        .transpose()?;
    let source: String = obj.get_item("source")?.extract()?;
    Ok(avd::pattern::FrameStamp {
        frame_id: obj.get_item("frame_id")?.extract()?,
        fps,
        tone_base_harmonic: optional(obj, "tone_base_harmonic")?
            .map(|v| v.extract())
            .transpose()?,
        source: match source.as_str() {
            "barcode" => StampSource::Barcode,
            "qr" => StampSource::Qr,
            other => {
                return Err(error(format!(
                    "stamp source must be barcode or qr, not `{other}`"
                )));
            }
        },
    })
}

/// Reads the frame stamp from an 8-bit luma plane (any bytes-like object).
#[pyfunction]
#[pyo3(signature = (luma, width, height, stride=None, try_qr=true))]
fn read_frame<'py>(
    py: Python<'py>,
    luma: &Bound<'py, PyAny>,
    width: usize,
    height: usize,
    stride: Option<usize>,
    try_qr: bool,
) -> PyResult<Option<Bound<'py, PyDict>>> {
    let data = PyBuffer::<u8>::get(luma)?.to_vec(py)?;
    let stride = stride.unwrap_or(width);
    if width == 0 || height == 0 || stride < width || data.len() < stride * (height - 1) + width {
        return Err(error(format!(
            "a {width}×{height} plane with stride {stride} needs {} bytes, got {}",
            stride * height.saturating_sub(1) + width,
            data.len()
        )));
    }
    let stamp = py.detach(|| decode_frame(&LumaView::new(&data, width, height, stride), try_qr));
    stamp.map(|s| stamp_dict(py, s)).transpose()
}

/// Measures drift from frames and audio decoded elsewhere; returns the report as JSON.
#[pyfunction]
#[pyo3(signature = (
    frames, audio=None, *, width=0, height=0, offset=0.0, duration=None, fps=None, capture="auto",
))]
#[allow(clippy::too_many_arguments)]
fn analyze_frames(
    py: Python<'_>,
    frames: &Bound<'_, PyAny>,
    audio: Option<&Bound<'_, PyAny>>,
    width: u32,
    height: u32,
    offset: f64,
    duration: Option<f64>,
    fps: Option<&str>,
    capture: &str,
) -> PyResult<String> {
    let mut video = Vec::new();
    for frame in frames.try_iter()? {
        let frame = frame?;
        video.push(VideoSample {
            time: frame.get_item("time")?.extract()?,
            stamp: optional(&frame, "stamp")?
                .map(|s| stamp_from(&s))
                .transpose()?,
        });
    }
    let audio = audio
        .map(|a| -> PyResult<AudioTrack> {
            Ok(AudioTrack {
                sample_rate: a.get_item("sample_rate")?.extract()?,
                start_time: a.get_item("start_time")?.extract()?,
                samples: PyBuffer::<f32>::get(&a.get_item("samples")?)?.to_vec(py)?,
            })
        })
        .transpose()?;
    let scan = MediaScan {
        video,
        audio,
        width,
        height,
        stream_fps: None,
    };
    let options = analyze_options(offset, duration, fps, true, capture)?;
    let report = py.detach(|| avd::analyze::analyze_scan(&scan, &options, "frames".into()));
    report_json(&report.map_err(error)?)
}

/// Renders test-pattern frames as 8-bit luma planes, without FFmpeg.
#[pyclass(frozen, module = "av_drift_detector")]
struct PatternRenderer {
    inner: avd::pattern::PatternRenderer,
    width: u32,
    height: u32,
}

#[pymethods]
impl PatternRenderer {
    #[new]
    #[pyo3(signature = (*, fps="30", width=1280, height=720, sample_rate=48000))]
    fn new(fps: &str, width: u32, height: u32, sample_rate: u32) -> PyResult<Self> {
        let tones = avd::audio::ToneCode::new(frame_rate(fps)?, sample_rate).map_err(error)?;
        let inner = avd::pattern::PatternRenderer::new(width as usize, height as usize, tones)
            .map_err(error)?;
        Ok(PatternRenderer {
            inner,
            width,
            height,
        })
    }

    #[getter]
    fn width(&self) -> u32 {
        self.width
    }

    #[getter]
    fn height(&self) -> u32 {
        self.height
    }

    /// Frame `frame_id` as `width × height` bytes of luma (16 = black, 235 = white).
    fn render<'py>(&self, py: Python<'py>, frame_id: u32) -> PyResult<Bound<'py, PyBytes>> {
        let mut image = LumaImage::new(self.width as usize, self.height as usize, 0);
        self.inner
            .render(frame_id, &mut image.canvas())
            .map_err(error)?;
        Ok(PyBytes::new(py, &image.data))
    }

    fn __repr__(&self) -> String {
        format!(
            "PatternRenderer(width={}, height={})",
            self.width, self.height
        )
    }
}

/// The test pattern's audio as native-endian float32 bytes.
#[pyfunction]
#[pyo3(signature = (count, *, fps="30", sample_rate=48000, delay=0.0, start=0))]
fn synthesize_audio<'py>(
    py: Python<'py>,
    count: usize,
    fps: &str,
    sample_rate: u32,
    delay: f64,
    start: u64,
) -> PyResult<Bound<'py, PyBytes>> {
    let code = avd::audio::ToneCode::new(frame_rate(fps)?, sample_rate).map_err(error)?;
    let samples = py.detach(|| avd::audio::synthesize(&code, delay, start, count));
    let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_ne_bytes()).collect();
    Ok(PyBytes::new(py, &bytes))
}

#[pymodule]
#[pyo3(name = "_native")]
fn native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("AvDriftError", m.py().get_type::<AvDriftError>())?;
    m.add_function(wrap_pyfunction!(analyze, m)?)?;
    m.add_function(wrap_pyfunction!(generate, m)?)?;
    m.add_function(wrap_pyfunction!(read_frame, m)?)?;
    m.add_function(wrap_pyfunction!(analyze_frames, m)?)?;
    m.add_function(wrap_pyfunction!(synthesize_audio, m)?)?;
    m.add_class::<PatternRenderer>()?;
    Ok(())
}
