//! Drift reports and their text, JSON and CSV renderings.

use std::fmt::Write as _;
use std::str::FromStr;

use serde::Serialize;

use crate::pattern::StampSource;
use crate::{Error, FrameRate};

/// How the capture was made, which decides how video frames are timed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptureMode {
    /// Decide from the stamps: camera if most had to be read from QR codes.
    #[default]
    Auto,
    /// A digital copy (re-encode, stream recording, capture card): a frame's
    /// timestamp is when it starts.
    File,
    /// A camera filming a screen: frames sample the display at unknown points
    /// within each displayed frame.
    Camera,
}

impl FromStr for CaptureMode {
    type Err = Error;

    /// `auto`, `file` or `camera`.
    fn from_str(s: &str) -> Result<Self, Error> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Ok(CaptureMode::Auto),
            "file" => Ok(CaptureMode::File),
            "camera" => Ok(CaptureMode::Camera),
            _ => Err(Error::InvalidOption(format!(
                "capture mode must be auto, file or camera, not `{s}`"
            ))),
        }
    }
}

/// What could be measured for one captured video frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameStatus {
    Ok,
    /// Neither the barcode nor the QR code could be read.
    NoVideoStamp,
    /// No decodable audio near this frame.
    NoAudio,
}

/// Drift measured at one captured video frame.
#[derive(Clone, Debug, Serialize)]
pub struct FrameDrift {
    /// Presentation time of the captured frame, in seconds.
    pub time: f64,
    /// Source frame number read from the picture.
    pub video_frame: Option<u32>,
    /// Source video position at `time`, in (fractional) frames. Equal to
    /// `video_frame` for file captures; estimated from the display timing for camera captures.
    pub video_position: Option<f64>,
    pub stamp_source: Option<StampSource>,
    /// Source position of the audio playing at `time`, in (fractional) frames.
    pub audio_frame: Option<f64>,
    /// `(video − audio)` in milliseconds: positive means the audio is late.
    pub drift_ms: Option<f64>,
    /// Source frames missing between the previous readable frame and this one.
    pub dropped_before: u32,
    /// Whether this frame repeats the previous readable frame's number.
    pub repeated: bool,
    pub status: FrameStatus,
}

/// Aggregate statistics over a report's frames.
#[derive(Clone, Debug, Default, Serialize)]
pub struct DriftSummary {
    /// Captured frames in the analysis window.
    pub frames: usize,
    /// Frames with a drift measurement.
    pub measured: usize,
    pub mean_ms: Option<f64>,
    pub median_ms: Option<f64>,
    pub min_ms: Option<f64>,
    pub max_ms: Option<f64>,
    /// Standard deviation of the drift (jitter).
    pub stddev_ms: Option<f64>,
    /// Slope of a least-squares line through drift against time.
    pub skew_ms_per_min: Option<f64>,
    /// The same slope as a clock-rate difference, in parts per million.
    pub skew_ppm: Option<f64>,
    pub dropped_frames: u64,
    pub repeated_frames: u64,
    /// Places where the frame number went backwards.
    pub backward_jumps: u64,
    pub no_video_stamp: usize,
    pub no_audio: usize,
    /// Audio frame cells decoded in the window (including the margins).
    pub audio_cells: usize,
    /// Decoded audio cells discarded because their timing disagreed with their neighbours.
    pub audio_outliers: usize,
}

/// The result of analysing a capture.
#[derive(Clone, Debug, Serialize)]
pub struct DriftReport {
    pub input: String,
    pub fps: FrameRate,
    /// Where the frame rate came from: `barcode`, `qr` or `option`.
    pub fps_source: &'static str,
    /// The capture mode used (never `auto`).
    pub capture: CaptureMode,
    /// Camera captures: ± half-width of the display timings consistent with the
    /// frames seen, which bounds the video side's contribution to the error.
    pub video_timing_uncertainty_ms: Option<f64>,
    pub width: u32,
    pub height: u32,
    pub offset: f64,
    pub duration: Option<f64>,
    pub summary: DriftSummary,
    pub frames: Vec<FrameDrift>,
}

impl DriftSummary {
    pub fn from_frames(frames: &[FrameDrift], audio_cells: usize) -> Self {
        let mut s = DriftSummary {
            frames: frames.len(),
            audio_cells,
            ..Default::default()
        };
        let mut points: Vec<(f64, f64)> = Vec::new();
        for f in frames {
            match f.status {
                FrameStatus::Ok => {}
                FrameStatus::NoVideoStamp => s.no_video_stamp += 1,
                FrameStatus::NoAudio => s.no_audio += 1,
            }
            s.dropped_frames += u64::from(f.dropped_before);
            s.repeated_frames += u64::from(f.repeated);
            if let Some(d) = f.drift_ms {
                points.push((f.time, d));
            }
        }
        s.backward_jumps = frames
            .iter()
            .filter_map(|f| f.video_frame)
            .collect::<Vec<_>>()
            .windows(2)
            .filter(|w| w[1] < w[0])
            .count() as u64;
        s.measured = points.len();
        if points.is_empty() {
            return s;
        }

        let n = points.len() as f64;
        let mean = points.iter().map(|p| p.1).sum::<f64>() / n;
        let var = points.iter().map(|p| (p.1 - mean).powi(2)).sum::<f64>() / n;
        let drifts: Vec<f64> = points.iter().map(|p| p.1).collect();
        s.mean_ms = Some(mean);
        s.median_ms = Some(median(drifts.clone()));
        s.min_ms = drifts.iter().copied().reduce(f64::min);
        s.max_ms = drifts.iter().copied().reduce(f64::max);
        s.stddev_ms = Some(var.sqrt());

        let mean_t = points.iter().map(|p| p.0).sum::<f64>() / n;
        let sxx: f64 = points.iter().map(|p| (p.0 - mean_t).powi(2)).sum();
        if points.len() >= 2 && sxx > 1e-9 {
            let sxy: f64 = points.iter().map(|p| (p.0 - mean_t) * (p.1 - mean)).sum();
            let slope = sxy / sxx; // ms of drift per second of capture
            s.skew_ms_per_min = Some(slope * 60.0);
            s.skew_ppm = Some(slope * 1000.0);
        }
        s
    }
}

/// The median of a non-empty list.
pub(crate) fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    let mid = v.len() / 2;
    if v.len().is_multiple_of(2) {
        (v[mid - 1] + v[mid]) / 2.0
    } else {
        v[mid]
    }
}

fn zeroed(v: f64, below: f64) -> f64 {
    if v.abs() < below { 0.0 } else { v }
}

fn ms(v: Option<f64>) -> String {
    v.map_or_else(
        || "-".to_string(),
        |v| format!("{:+.2} ms", zeroed(v, 0.005)),
    )
}

impl DriftReport {
    /// A short human-readable summary.
    pub fn summary_text(&self) -> String {
        let s = &self.summary;
        let mut out = String::new();
        let end = self.frames.last().map_or(self.offset, |f| f.time);
        let _ = writeln!(out, "Input    {}", self.input);
        let _ = writeln!(
            out,
            "Pattern  {} fps (from {}), {}x{}",
            self.fps.label(),
            self.fps_source,
            self.width,
            self.height
        );
        let _ = writeln!(
            out,
            "Window   {:.3}s - {:.3}s, {} frames",
            self.offset, end, s.frames
        );
        match (self.capture, self.video_timing_uncertainty_ms) {
            (CaptureMode::Camera, Some(u)) => {
                let _ = writeln!(out, "Capture  camera; display timing known to ±{u:.2} ms");
                if u > 0.25 * 1000.0 / self.fps.as_f64() {
                    let _ = writeln!(
                        out,
                        "         (frames look locked to the pattern's rate. For sub-frame video timing, film at a \
                         rate that isn't a multiple of {} fps, e.g. 25 or 24 fps, or in slow motion. If this is \
                         a cropped or letterboxed digital capture rather than a camera, use --capture file.)",
                        self.fps.label()
                    );
                }
            }
            (CaptureMode::Camera, None) => {
                let _ = writeln!(out, "Capture  camera");
            }
            _ => {}
        }
        match s.mean_ms {
            Some(mean) => {
                let _ = writeln!(
                    out,
                    "Drift    mean {}  median {}  min {}  max {}  stddev {:.2} ms",
                    ms(Some(mean)),
                    ms(s.median_ms),
                    ms(s.min_ms),
                    ms(s.max_ms),
                    s.stddev_ms.unwrap_or(0.0)
                );
                let _ = writeln!(
                    out,
                    "         = {:+.2} frames at {} fps (positive = audio late)",
                    zeroed(mean / 1000.0 * self.fps.as_f64(), 0.005),
                    self.fps.label()
                );
            }
            None => {
                let _ = writeln!(out, "Drift    no measurements");
            }
        }
        if let (Some(per_min), Some(ppm)) = (s.skew_ms_per_min, s.skew_ppm) {
            // Avoid printing "-0.000" for slopes that round to zero.
            let (per_min, ppm) = (zeroed(per_min, 0.0005), zeroed(ppm, 0.05));
            let _ = writeln!(out, "Skew     {per_min:+.3} ms/min ({ppm:+.1} ppm)");
        }
        let _ = writeln!(
            out,
            "Frames   {} measured, {} unreadable, {} without audio, {} dropped, {} repeated{}",
            s.measured,
            s.no_video_stamp,
            s.no_audio,
            s.dropped_frames,
            s.repeated_frames,
            if s.backward_jumps > 0 {
                format!(", {} backward jumps", s.backward_jumps)
            } else {
                String::new()
            }
        );
        let _ = write!(out, "Audio    {} frame cells decoded", s.audio_cells);
        if s.audio_outliers > 0 {
            let _ = write!(out, ", {} outliers discarded", s.audio_outliers);
        }
        out
    }

    /// A per-frame table.
    pub fn frames_table(&self) -> String {
        let camera = self.capture == CaptureMode::Camera;
        let mut out = String::from(if camera {
            "      time     video    video_pos        audio    drift_ms  note\n"
        } else {
            "      time     video        audio    drift_ms  note\n"
        });
        for f in &self.frames {
            let video = f.video_frame.map_or_else(|| "-".into(), |v| v.to_string());
            let position = f
                .video_position
                .map_or_else(|| "-".into(), |p| format!("{p:.3}"));
            let audio = f
                .audio_frame
                .map_or_else(|| "-".into(), |a| format!("{a:.3}"));
            let drift = f
                .drift_ms
                .map_or_else(|| "-".into(), |d| format!("{d:+.3}"));
            let mut note = Vec::new();
            match f.status {
                FrameStatus::Ok => {}
                FrameStatus::NoVideoStamp => note.push("unreadable".to_string()),
                FrameStatus::NoAudio => note.push("no audio".to_string()),
            }
            if f.dropped_before > 0 {
                note.push(format!("{} dropped", f.dropped_before));
            }
            if f.repeated {
                note.push("repeat".into());
            }
            if f.stamp_source == Some(StampSource::Qr) {
                note.push("qr".into());
            }
            if camera {
                let _ = writeln!(
                    out,
                    "{:>10.4} {video:>9} {position:>12} {audio:>12} {drift:>11}  {}",
                    f.time,
                    note.join(", ")
                );
            } else {
                let _ = writeln!(
                    out,
                    "{:>10.4} {video:>9} {audio:>12} {drift:>11}  {}",
                    f.time,
                    note.join(", ")
                );
            }
        }
        out
    }

    /// One CSV row per captured frame.
    pub fn to_csv(&self) -> String {
        let mut out = String::from(
            "time,video_frame,video_position,stamp_source,audio_frame,drift_ms,dropped_before,repeated,status\n",
        );
        for f in &self.frames {
            let opt = |v: Option<String>| v.unwrap_or_default();
            let source = f.stamp_source.map(|s| match s {
                StampSource::Barcode => "barcode".to_string(),
                StampSource::Qr => "qr".to_string(),
            });
            let status = match f.status {
                FrameStatus::Ok => "ok",
                FrameStatus::NoVideoStamp => "no_video_stamp",
                FrameStatus::NoAudio => "no_audio",
            };
            let _ = writeln!(
                out,
                "{:.6},{},{},{},{},{},{},{},{}",
                f.time,
                opt(f.video_frame.map(|v| v.to_string())),
                opt(f.video_position.map(|p| format!("{p:.6}"))),
                opt(source),
                opt(f.audio_frame.map(|a| format!("{a:.6}"))),
                opt(f.drift_ms.map(|d| format!("{d:.6}"))),
                f.dropped_before,
                f.repeated,
                status
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_capture_modes() {
        assert_eq!(
            "Camera".parse::<CaptureMode>().unwrap(),
            CaptureMode::Camera
        );
        assert_eq!("file".parse::<CaptureMode>().unwrap(), CaptureMode::File);
        assert!("phone".parse::<CaptureMode>().is_err());
    }

    fn frame(time: f64, drift: f64) -> FrameDrift {
        FrameDrift {
            time,
            video_frame: Some((time * 30.0).round() as u32),
            video_position: Some((time * 30.0).round()),
            stamp_source: Some(StampSource::Barcode),
            audio_frame: Some(0.0),
            drift_ms: Some(drift),
            dropped_before: 0,
            repeated: false,
            status: FrameStatus::Ok,
        }
    }

    #[test]
    fn summary_statistics() {
        // Drift rising 1 ms per second: 60 ms/min, 1000 ppm.
        let frames: Vec<FrameDrift> = (0..31)
            .map(|i| frame(i as f64 / 10.0, 10.0 + i as f64 / 10.0))
            .collect();
        let s = DriftSummary::from_frames(&frames, 5);
        assert_eq!((s.frames, s.measured, s.audio_cells), (31, 31, 5));
        assert!((s.mean_ms.unwrap() - 11.5).abs() < 1e-9);
        assert!((s.median_ms.unwrap() - 11.5).abs() < 1e-9);
        assert_eq!((s.min_ms, s.max_ms), (Some(10.0), Some(13.0)));
        assert!((s.skew_ms_per_min.unwrap() - 60.0).abs() < 1e-6);
        assert!((s.skew_ppm.unwrap() - 1000.0).abs() < 1e-6);
    }

    #[test]
    fn empty_summary() {
        let s = DriftSummary::from_frames(&[], 0);
        assert_eq!(s.measured, 0);
        assert!(s.mean_ms.is_none() && s.skew_ppm.is_none());
    }
}
