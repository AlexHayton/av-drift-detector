//! Matching decoded video frames against decoded audio cells.
//!
//! The audio side gives, at each decoded cell, the source position of the
//! sound playing at that moment. How the video side is timed depends on the
//! capture:
//!
//! * **File** (re-encode, stream recording, capture card): a frame's
//!   timestamp is when it starts, so the frame shown at time `t` has source
//!   position exactly `frame_id`.
//! * **Camera** (a phone filming a screen): each camera frame samples the
//!   display at an unknown point within the displayed frame. Every
//!   observation "frame N was showing at time t" bounds when the display
//!   switched frames. Intersecting the bounds over all observations pins down
//!   the display timeline to a fraction of a frame, provided the camera's
//!   rate isn't locked to the pattern's.

use std::collections::HashMap;
use std::path::Path;

use crate::audio::code::ID_MODULUS;
use crate::audio::{CellDetector, ToneCode};
use crate::media::decode::{self, AudioTrack, MediaScan, ScanWindow};
use crate::pattern::StampSource;
use crate::report::{CaptureMode, DriftReport, DriftSummary, FrameDrift, FrameStatus, median};
use crate::{Error, FrameRate, Result};

/// Audio kept either side of the video window, so the first and last frames
/// have audio cells on both sides.
const AUDIO_MARGIN: f64 = 0.5;

/// Settings for [`analyze`].
#[derive(Clone, Debug)]
pub struct AnalyzeOptions {
    /// Start of the analysis window, in seconds.
    pub offset: f64,
    /// Length of the analysis window; `None` analyses to the end of the file.
    pub duration: Option<f64>,
    /// The pattern's frame rate, overriding what the stamps say.
    pub fps: Option<FrameRate>,
    /// Fall back to the (slower) QR code when a barcode can't be read.
    pub use_qr: bool,
    pub capture: CaptureMode,
}

impl Default for AnalyzeOptions {
    fn default() -> Self {
        AnalyzeOptions {
            offset: 0.0,
            duration: None,
            fps: None,
            use_qr: true,
            capture: CaptureMode::Auto,
        }
    }
}

/// Measures the A/V drift of every frame in a capture of an av-drift-detector test file.
pub fn analyze(path: &Path, opts: &AnalyzeOptions) -> Result<DriftReport> {
    if !(opts.offset.is_finite() && opts.offset >= 0.0) {
        return Err(Error::InvalidOption(format!(
            "offset must be >= 0, got {}",
            opts.offset
        )));
    }
    if let Some(d) = opts.duration
        && !(d.is_finite() && d > 0.0)
    {
        return Err(Error::InvalidOption(format!(
            "duration must be positive, got {d}"
        )));
    }
    let window = ScanWindow {
        start: opts.offset,
        end: opts.duration.map(|d| opts.offset + d),
        audio_margin: AUDIO_MARGIN,
    };
    let scan = decode::scan(path, window, opts.use_qr)?;
    analyze_scan(&scan, opts, path.display().to_string())
}

/// The analysis itself, separated from decoding.
pub fn analyze_scan(scan: &MediaScan, opts: &AnalyzeOptions, input: String) -> Result<DriftReport> {
    let (fps, fps_source) = pattern_fps(scan, opts.fps)?;
    let frame_ms = 1000.0 / fps.as_f64();
    let capture = match opts.capture {
        CaptureMode::Auto => detect_capture(scan),
        explicit => explicit,
    };

    // Readable video frames as (time, frame id), in presentation order.
    let stamps: Vec<(f64, u32)> = scan
        .video
        .iter()
        .filter_map(|v| v.stamp.map(|s| (v.time, s.frame_id)))
        .collect();

    let (audio, audio_outliers) = match &scan.audio {
        Some(track) => decode_audio(track, scan, fps, &stamps)?,
        None => (Vec::new(), 0),
    };

    let display = match capture {
        CaptureMode::Camera => estimate_display_timing(&stamps, fps),
        _ => None,
    };

    let mut frames = Vec::with_capacity(scan.video.len());
    let mut previous: Option<u32> = None;
    for v in &scan.video {
        let mut f = FrameDrift {
            time: v.time,
            video_frame: v.stamp.map(|s| s.frame_id),
            video_position: None,
            stamp_source: v.stamp.map(|s| s.source),
            audio_frame: None,
            drift_ms: None,
            dropped_before: 0,
            repeated: false,
            status: FrameStatus::NoVideoStamp,
        };
        if let Some(id) = f.video_frame {
            // Repeats and gaps are only meaningful when frames map one-to-one.
            if capture != CaptureMode::Camera
                && let Some(prev) = previous
            {
                f.repeated = id == prev;
                f.dropped_before = id.saturating_sub(prev).saturating_sub(1);
            }
            previous = Some(id);
            let position = match &display {
                Some(d) => (v.time - d.offset) * fps.as_f64(),
                None => f64::from(id),
            };
            f.video_position = Some(position);
            // Before source position 0 the test audio is silent, so there is nothing to compare.
            f.audio_frame = audio_position_at(&audio, v.time, fps).filter(|&a| a >= 0.0);
            f.status = match f.audio_frame {
                Some(a) => {
                    f.drift_ms = Some((position - a) * frame_ms);
                    FrameStatus::Ok
                }
                None => FrameStatus::NoAudio,
            };
        }
        frames.push(f);
    }

    let mut summary = DriftSummary::from_frames(&frames, audio.len());
    summary.audio_outliers = audio_outliers;
    Ok(DriftReport {
        input,
        fps,
        fps_source,
        capture,
        video_timing_uncertainty_ms: display.map(|d| d.uncertainty * 1000.0),
        width: scan.width,
        height: scan.height,
        offset: opts.offset,
        duration: opts.duration,
        summary,
        frames,
    })
}

/// The pattern's frame rate: the explicit option, else the most common rate in the stamps.
fn pattern_fps(scan: &MediaScan, explicit: Option<FrameRate>) -> Result<(FrameRate, &'static str)> {
    if let Some(fps) = explicit {
        return Ok((fps, "option"));
    }
    let mut votes: HashMap<(FrameRate, StampSource), usize> = HashMap::new();
    for stamp in scan.video.iter().filter_map(|v| v.stamp) {
        if let Some(fps) = stamp.fps {
            *votes.entry((fps, stamp.source)).or_default() += 1;
        }
    }
    let best = votes
        .into_iter()
        .max_by_key(|&((fps, _), n)| (n, fps.num()));
    match best {
        Some(((fps, StampSource::Barcode), _)) => Ok((fps, "barcode")),
        Some(((fps, StampSource::Qr), _)) => Ok((fps, "qr")),
        None => Err(Error::UnknownFrameRate),
    }
}

/// Camera if most stamps had to come from QR codes (the barcode strip only
/// reads when the picture is pixel-aligned with the original).
fn detect_capture(scan: &MediaScan) -> CaptureMode {
    let (mut barcode, mut qr) = (0, 0);
    for stamp in scan.video.iter().filter_map(|v| v.stamp) {
        match stamp.source {
            StampSource::Barcode => barcode += 1,
            StampSource::Qr => qr += 1,
        }
    }
    if qr > barcode {
        CaptureMode::Camera
    } else {
        CaptureMode::File
    }
}

/// Decodes the audio cells into (time, source position in frames), with
/// 16-bit ids unwrapped against the video frame shown at about the same time.
/// Returns the points and the number rejected as outliers.
fn decode_audio(
    track: &AudioTrack,
    scan: &MediaScan,
    fps: FrameRate,
    stamps: &[(f64, u32)],
) -> Result<(Vec<(f64, f64)>, usize)> {
    // Prefer the tone plan the QR codes describe; else the standard plan for this rate.
    let mut votes: HashMap<u32, usize> = HashMap::new();
    for k in scan
        .video
        .iter()
        .filter_map(|v| v.stamp?.tone_base_harmonic)
    {
        *votes.entry(k).or_default() += 1;
    }
    let base = votes
        .into_iter()
        .max_by_key(|&(k, n)| (n, k))
        .map_or_else(|| ToneCode::default_base_harmonic(fps), |(k, _)| k);
    let detector = CellDetector::new(ToneCode::with_base_harmonic(fps, track.sample_rate, base)?);

    let mut points = Vec::new();
    for cell in detector.detect(&track.samples, track.start_time) {
        let Some(reference) = nearest(stamps, cell.time) else {
            break;
        };
        let id = unwrap_id(cell.id, reference);
        points.push((cell.time, id as f64 + cell.residual * fps.as_f64()));
    }
    let rejected = reject_outliers(&mut points, fps);
    Ok((points, rejected))
}

/// Drops cells whose timing disagrees with their neighbours (e.g. the first
/// cell after silence, smeared by a codec's onset handling). Drift changes
/// slowly, so each cell's offset is compared with the median of the nine
/// around it; the threshold scales with the typical scatter. Steps in drift
/// survive, because a cell either side of a step has a majority of neighbours
/// on its own side.
fn reject_outliers(points: &mut Vec<(f64, f64)>, fps: FrameRate) -> usize {
    if points.len() < 5 {
        return 0;
    }
    let d = fps.frame_duration();
    let offsets: Vec<f64> = points.iter().map(|&(t, pos)| t - pos * d).collect();
    let deviations: Vec<f64> = (0..offsets.len())
        .map(|j| {
            let window = &offsets[j.saturating_sub(4)..(j + 5).min(offsets.len())];
            offsets[j] - median(window.to_vec())
        })
        .collect();
    let mad = median(deviations.iter().map(|v| v.abs()).collect());
    let threshold = 0.0002 + 6.0 * 1.4826 * mad;
    let before = points.len();
    let mut keep = deviations.iter().map(|dev| dev.abs() <= threshold);
    points.retain(|_| keep.next().unwrap_or(false));
    before - points.len()
}

/// When the display started showing frame 0 (`offset`), so that frame `N`
/// is on screen from `offset + N/fps`, plus the half-width of the range of
/// offsets consistent with what the camera saw.
#[derive(Clone, Copy, Debug)]
struct DisplayTiming {
    offset: f64,
    uncertainty: f64,
}

fn estimate_display_timing(stamps: &[(f64, u32)], fps: FrameRate) -> Option<DisplayTiming> {
    if stamps.is_empty() {
        return None;
    }
    let d = fps.frame_duration();
    // Seeing frame N at time t means offset ∈ [t − (N+1)/fps, t − N/fps].
    let mut lower: Vec<f64> = stamps
        .iter()
        .map(|&(t, n)| t - (f64::from(n) + 1.0) * d)
        .collect();
    let mut upper: Vec<f64> = stamps.iter().map(|&(t, n)| t - f64::from(n) * d).collect();
    lower.sort_by(f64::total_cmp);
    upper.sort_by(f64::total_cmp);
    // Ignore the most extreme 2% on each side (frames caught mid-transition).
    let trim = stamps.len() / 50;
    let (lo, hi) = (lower[lower.len() - 1 - trim], upper[trim]);
    if lo <= hi {
        return Some(DisplayTiming {
            offset: (lo + hi) / 2.0,
            uncertainty: (hi - lo) / 2.0,
        });
    }
    // Inconsistent observations (judder, a stall): use the median mid-frame estimate.
    let mids: Vec<f64> = stamps
        .iter()
        .map(|&(t, n)| t - (f64::from(n) + 0.5) * d)
        .collect();
    Some(DisplayTiming {
        offset: median(mids),
        uncertainty: d / 2.0,
    })
}

/// The frame id of the stamp nearest in time to `t`.
fn nearest(stamps: &[(f64, u32)], t: f64) -> Option<u32> {
    let i = stamps.partition_point(|s| s.0 < t);
    let before = i.checked_sub(1).map(|j| stamps[j]);
    let after = stamps.get(i).copied();
    match (before, after) {
        (Some(b), Some(a)) => Some(if t - b.0 <= a.0 - t { b.1 } else { a.1 }),
        (Some(s), None) | (None, Some(s)) => Some(s.1),
        (None, None) => None,
    }
}

/// Expands a 16-bit audio id to the full frame number closest to `reference`.
fn unwrap_id(id: u16, reference: u32) -> i64 {
    let m = i64::from(ID_MODULUS);
    let delta = (i64::from(id) - i64::from(reference)).rem_euclid(m);
    let delta = if delta >= m / 2 { delta - m } else { delta };
    i64::from(reference) + delta
}

/// Source audio position (in frames) at container time `t`, interpolated
/// between the decoded cells either side.
fn audio_position_at(audio: &[(f64, f64)], t: f64, fps: FrameRate) -> Option<f64> {
    let frame = fps.frame_duration();
    let i = audio.partition_point(|a| a.0 <= t);
    let before = i.checked_sub(1).map(|j| audio[j]);
    let after = audio.get(i).copied();
    if let (Some(b), Some(a)) = (before, after)
        && a.0 - b.0 <= 2.5 * frame
    {
        return Some(b.1 + (t - b.0) / (a.0 - b.0) * (a.1 - b.1));
    }
    // Otherwise extrapolate from a cell no more than a frame and a half away,
    // assuming real-time playback.
    [before, after]
        .into_iter()
        .flatten()
        .filter(|c| (c.0 - t).abs() <= 1.5 * frame)
        .min_by(|x, y| (x.0 - t).abs().total_cmp(&(y.0 - t).abs()))
        .map(|c| c.1 + (t - c.0) / frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::synthesize;
    use crate::media::decode::VideoSample;
    use crate::pattern::FrameStamp;

    #[test]
    fn unwraps_ids_across_the_16_bit_boundary() {
        assert_eq!(unwrap_id(5, 5), 5);
        assert_eq!(unwrap_id(65530, 65540), 65530);
        assert_eq!(unwrap_id(4, 65530), 65540);
        assert_eq!(unwrap_id(100, 200_000), 196_708);
        assert_eq!(unwrap_id(0, 3), 0);
    }

    fn stamp(n: u32, fps: FrameRate, source: StampSource) -> Option<FrameStamp> {
        Some(FrameStamp {
            frame_id: n,
            fps: Some(fps),
            tone_base_harmonic: None,
            source,
        })
    }

    fn track(samples: Vec<f32>) -> Option<AudioTrack> {
        Some(AudioTrack {
            sample_rate: 48000,
            start_time: 0.0,
            samples,
        })
    }

    /// Video frames at their nominal times and the coded audio delayed by `delay` seconds.
    fn file_scan(fps: FrameRate, frames: u32, delay: f64, skip_frame: Option<u32>) -> MediaScan {
        let video = (0..frames)
            .filter(|&n| Some(n) != skip_frame)
            .map(|n| VideoSample {
                time: fps.frame_time(i64::from(n)),
                stamp: stamp(n, fps, StampSource::Barcode),
            })
            .collect();
        let code = ToneCode::new(fps, 48000).unwrap();
        let len = (fps.frame_time(i64::from(frames)) + 0.5) * 48000.0;
        MediaScan {
            video,
            audio: track(synthesize(&code, delay, 0, len as usize)),
            width: 1280,
            height: 720,
            stream_fps: Some(fps),
        }
    }

    #[test]
    fn measures_injected_delay() {
        for (fps, delay) in [("30", 0.0785), ("29.97", -0.040), ("60", 0.5), ("25", 0.0)] {
            let fps: FrameRate = fps.parse().unwrap();
            let report = analyze_scan(
                &file_scan(fps, 90, delay, None),
                &AnalyzeOptions::default(),
                "t".into(),
            )
            .unwrap();
            assert_eq!((report.fps, report.capture), (fps, CaptureMode::File));
            let s = &report.summary;
            // Frames before the delayed audio starts have no audio to measure.
            assert!(
                s.measured as f64 >= 90.0 - delay.max(0.0) * fps.as_f64() - 2.0,
                "{fps}: {s:?}"
            );
            for f in report.frames.iter().filter_map(|f| f.drift_ms) {
                assert!(
                    (f - delay * 1000.0).abs() < 0.001,
                    "{fps}: drift {f} vs {}",
                    delay * 1000.0
                );
            }
        }
    }

    #[test]
    fn reports_dropped_frames() {
        let fps: FrameRate = "30".parse().unwrap();
        let report = analyze_scan(
            &file_scan(fps, 60, 0.0, Some(30)),
            &AnalyzeOptions::default(),
            "t".into(),
        )
        .unwrap();
        assert_eq!(report.summary.dropped_frames, 1);
        assert_eq!(
            report
                .frames
                .iter()
                .find(|f| f.dropped_before == 1)
                .unwrap()
                .video_frame,
            Some(31)
        );
    }

    #[test]
    fn needs_a_frame_rate() {
        let fps: FrameRate = "15".parse().unwrap();
        let mut scan = file_scan(fps, 10, 0.0, None);
        for v in &mut scan.video {
            v.stamp.as_mut().unwrap().fps = None;
        }
        assert!(matches!(
            analyze_scan(&scan, &AnalyzeOptions::default(), "t".into()),
            Err(Error::UnknownFrameRate)
        ));
        let opts = AnalyzeOptions {
            fps: Some(fps),
            ..Default::default()
        };
        assert_eq!(
            analyze_scan(&scan, &opts, "t".into()).unwrap().fps_source,
            "option"
        );
    }

    #[test]
    fn rejects_a_stray_audio_cell() {
        let fps: FrameRate = "30".parse().unwrap();
        let mut points: Vec<(f64, f64)> = (0..40)
            .map(|j| (j as f64 / 30.0 + 0.01, j as f64))
            .collect();
        points[17].1 += 0.05; // 1.7 ms out
        assert_eq!(reject_outliers(&mut points, fps), 1);
        assert!(points.iter().all(|p| p.1.fract() == 0.0));
        // A genuine 10 ms step is kept.
        let mut step: Vec<(f64, f64)> = (0..40)
            .map(|j| (j as f64 / 30.0 + if j < 20 { 0.0 } else { 0.01 }, j as f64))
            .collect();
        assert_eq!(reject_outliers(&mut step, fps), 0);
    }

    /// Simulates a phone filming a screen: the display shows frame N from
    /// `screen_start + N/fps`; the camera samples it at its own rate; the
    /// speaker's sound reaches the microphone `drift` late, with echoes,
    /// noise and a low level.
    fn camera_scan(fps: FrameRate, camera_fps: f64, screen_start: f64, drift: f64) -> MediaScan {
        let seconds = 4.0;
        let video = (0..(seconds * camera_fps) as u32)
            .map(|k| {
                let t = 0.0123 + f64::from(k) / camera_fps;
                let shown = ((t - screen_start) * fps.as_f64()).floor();
                let stamp = (shown >= 0.0)
                    .then(|| stamp(shown as u32, fps, StampSource::Qr))
                    .flatten();
                VideoSample { time: t, stamp }
            })
            .collect();
        let code = ToneCode::new(fps, 48000).unwrap();
        let dry = synthesize(&code, screen_start + drift, 0, (seconds * 48000.0) as usize);
        // Room: decaying echoes between 4 and 120 ms, then noise and a -20 dB level.
        let mut wet: Vec<f32> = dry.iter().map(|&s| s * 0.1).collect();
        let mut seed = 99u32;
        let mut rand = || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (seed >> 8) as f32 / (1 << 24) as f32
        };
        for _ in 0..25 {
            let delay = (0.004 + 0.116 * rand()) * 48000.0;
            let gain = 0.1 * 0.4 * (-(delay / 48000.0) / 0.04).exp() * (rand() - 0.5) * 2.0;
            let delay = delay as usize;
            for i in delay..wet.len() {
                wet[i] += dry[i - delay] * gain;
            }
        }
        for s in &mut wet {
            *s += 0.003 * (rand() - 0.5);
        }
        MediaScan {
            video,
            audio: track(wet),
            width: 1920,
            height: 1080,
            stream_fps: None,
        }
    }

    #[test]
    fn camera_capture_recovers_drift() {
        for (fps, camera_fps, drift) in [
            ("30", 24.93, 0.045),
            ("25", 29.97, -0.020),
            ("60", 59.5, 0.0),
        ] {
            let fps: FrameRate = fps.parse().unwrap();
            let scan = camera_scan(fps, camera_fps, 0.2, drift);
            let report = analyze_scan(&scan, &AnalyzeOptions::default(), "t".into()).unwrap();
            assert_eq!(report.capture, CaptureMode::Camera);
            let s = &report.summary;
            let uncertainty = report.video_timing_uncertainty_ms.unwrap();
            assert!(
                uncertainty < 2.0,
                "{fps} @ {camera_fps}: video timing ±{uncertainty} ms"
            );
            assert!(
                s.measured > (3.0 * camera_fps) as usize,
                "{fps}: {}",
                report.summary_text()
            );
            let mean = s.mean_ms.unwrap();
            assert!(
                (mean - drift * 1000.0).abs() < uncertainty + 0.5,
                "{fps} @ {camera_fps}: mean {mean:.3} ms vs {:.3}\n{}",
                drift * 1000.0,
                report.summary_text()
            );
            assert_eq!(s.dropped_frames + s.repeated_frames, 0);
        }
    }

    #[test]
    fn camera_locked_to_the_pattern_reports_its_uncertainty() {
        let fps: FrameRate = "30".parse().unwrap();
        let report = analyze_scan(
            &camera_scan(fps, 30.0, 0.2, 0.0),
            &AnalyzeOptions::default(),
            "t".into(),
        )
        .unwrap();
        let uncertainty = report.video_timing_uncertainty_ms.unwrap();
        assert!(
            uncertainty > 10.0,
            "a locked camera can't resolve sub-frame timing: ±{uncertainty}"
        );
        assert!(report.summary.mean_ms.unwrap().abs() <= uncertainty + 0.5);
    }
}
