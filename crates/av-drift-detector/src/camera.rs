//! When the display showed each frame, from what a camera saw.
//!
//! A camera frame that shows pattern frame `N` was taken at some unknown
//! point while `N` was on screen. Two kinds of observation pin the display's
//! timeline down:
//!
//! * **Bounds.** Seeing `N` at time `t` means the display switched to `N` at
//!   or before `t`, and to `N+1` after it. Intersecting those bounds over many
//!   frames narrows the timeline to a fraction of a frame, provided the
//!   camera's rate isn't locked to the pattern's (a locked camera sees every
//!   frame at the same phase, and the bounds never narrow).
//! * **Blends.** A camera whose exposure spans the display's switch from `M`
//!   to `M+1` sees a mix of the two ([`Blend`]). The
//!   share `β` of the exposure after the switch says *when* the switch
//!   happened: at `t + E·(1 − β)`, for an exposure of length `E` starting at
//!   `t`. That holds at any camera rate, locked or not.
//!
//! The exposure time `E` isn't recorded in video files. When the camera's and
//! display's phases drift relative to each other during the capture, `β`
//! changes with them, and the fit of switch times against `β` measures `E`.
//! Otherwise it's only known to lie between zero and the camera's frame
//! interval, and the reported uncertainty says so. A camera that reports each
//! frame's exposure ([`VideoSample::exposure`](crate::media::VideoSample::exposure),
//! from a phone's capture metadata) removes the guesswork, frame by frame.
//!
//! Camera timestamps are taken to mark the start of the exposure of the
//! picture's top row, as phones record them. A rolling shutter exposes lower
//! rows later: by the readout the camera reports
//! ([`VideoSample::readout`](crate::media::VideoSample::readout)), if it does,
//! and otherwise by what the change in `β` down the QR code measures.

use crate::FrameRate;
use crate::media::MediaScan;
use crate::pattern::Blend;
use crate::report::median;

/// Mixed frames needed before blends are trusted to time the display.
const MIN_MIXED: usize = 5;
/// The spread of `1 − β` across the capture needed to measure the exposure.
const MIN_PHASE_SPREAD: f64 = 0.1;
/// A frame whose switch time is this far (in frames) from its neighbours'
/// is display judder: the screen held a frame longer, or showed one early.
const JUDDER_FRAMES: f64 = 0.3;

/// The display's timeline: frame `N` went on screen at `offset + N/fps`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DisplayTiming {
    pub offset: f64,
    /// ± half-width of the offsets consistent with what the camera saw, in seconds.
    pub uncertainty: f64,
    /// How the timeline was found.
    pub method: TimingMethod,
    /// What the timing knew of the camera's exposure time, where it used it.
    pub exposure: Option<Exposure>,
    /// Per-frame offsets for frames whose own switch time was measured, by
    /// index into the scan's video samples: these show the display's own
    /// irregularities (judder).
    pub frame_offsets: Vec<(usize, f64)>,
    /// Frames whose switch time stood out from their neighbours'.
    pub judder: usize,
}

/// How a display timeline was estimated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TimingMethod {
    /// From the frames that caught the display mid-switch.
    Blend,
    /// From the bounds that every frame number puts on the timeline.
    #[default]
    Bounds,
}

impl TimingMethod {
    pub fn name(self) -> &'static str {
        match self {
            TimingMethod::Blend => "blend",
            TimingMethod::Bounds => "bounds",
        }
    }
}

/// The camera's exposure time, as far as the timing knew it (seconds).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Exposure {
    /// Reported by the camera with each frame: the median. Each frame was
    /// timed with its own.
    Reported(f64),
    /// Measured from how the blends changed with the camera's phase.
    Measured(f64),
    /// Unknown, so anything up to this was allowed for (the camera's frame interval).
    Unknown { max: f64 },
}

impl Exposure {
    /// The exposure, if it was reported or measured.
    pub fn known(self) -> Option<f64> {
        match self {
            Exposure::Reported(e) | Exposure::Measured(e) => Some(e),
            Exposure::Unknown { .. } => None,
        }
    }

    /// Where a known exposure came from: `reported` or `measured`.
    pub fn source(self) -> Option<&'static str> {
        match self {
            Exposure::Reported(_) => Some("reported"),
            Exposure::Measured(_) => Some("measured"),
            Exposure::Unknown { .. } => None,
        }
    }
}

/// One frame's stamp, as the timing needs it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Observation {
    /// Index into the scan's video samples.
    pub index: usize,
    pub time: f64,
    pub frame: u32,
    /// Whether the stamp came from the QR code (decoded or fitted).
    pub qr: bool,
    pub blend: Option<Blend>,
    /// The QR code's centre row and height, in pixels, if it was read from one.
    pub code_rows: Option<(f64, f64)>,
    /// The QR code's corners, if it was read from one.
    pub corners: Option<[(f32, f32); 4]>,
    /// The exposure time the camera reported (seconds).
    pub exposure: Option<f64>,
    /// How long after the picture's top row the QR code's middle row began
    /// its exposure (seconds), from the readout the camera reported.
    pub code_delay: Option<f64>,
}

impl Observation {
    pub fn from_scan(scan: &MediaScan) -> Vec<Observation> {
        let height = f64::from(scan.height);
        scan.video
            .iter()
            .enumerate()
            .filter_map(|(index, v)| {
                let s = v.stamp?;
                let code_rows = s.corners.map(|c| {
                    let ys = c.map(|p| f64::from(p.1));
                    let top = ys.iter().copied().fold(f64::INFINITY, f64::min);
                    let bottom = ys.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                    ((top + bottom) / 2.0, (bottom - top).max(1.0))
                });
                let code_delay = v
                    .readout
                    .filter(|r| r.is_finite() && *r >= 0.0 && height > 0.0)
                    .zip(code_rows)
                    .map(|(readout, (centre, _))| readout * (centre / height).clamp(0.0, 1.0));
                Some(Observation {
                    index,
                    time: v.time,
                    frame: s.frame_id,
                    qr: s.source.is_qr(),
                    blend: s.blend,
                    code_rows,
                    corners: s.corners,
                    exposure: v.exposure.filter(|e| e.is_finite() && *e > 0.0),
                    code_delay,
                })
            })
            .collect()
    }
}

/// The median exposure the camera reported, if it reported any.
fn reported_exposure<'a>(observations: impl IntoIterator<Item = &'a Observation>) -> Option<f64> {
    let reported: Vec<f64> = observations
        .into_iter()
        .filter_map(|o| o.exposure)
        .collect();
    (!reported.is_empty()).then(|| median(reported))
}

/// The display timeline for camera observations. `camera_interval` is the
/// capture's typical frame interval (seconds), the longest the exposure can be.
pub(crate) fn display_timing(
    observations: &[Observation],
    fps: FrameRate,
    camera_interval: Option<f64>,
) -> Option<DisplayTiming> {
    blend_timing(observations, fps, camera_interval).or_else(|| {
        let reported = reported_exposure(observations);
        let exposure =
            reported.unwrap_or_else(|| typical_exposure(observations, fps, camera_interval));
        let mut timing = bound_timing(observations, fps, exposure, reported.is_some())?;
        timing.exposure = reported.map(Exposure::Reported);
        Some(timing)
    })
}

/// A rough exposure time from how often frames came out blended: a camera
/// not locked to the display spans a switch with probability `E·fps`. Zero
/// if no frame could be checked for a blend.
fn typical_exposure(
    observations: &[Observation],
    fps: FrameRate,
    camera_interval: Option<f64>,
) -> f64 {
    let checked: Vec<&Blend> = observations
        .iter()
        .filter_map(|o| o.blend.as_ref())
        .collect();
    if checked.is_empty() {
        return 0.0;
    }
    let mixed = checked.iter().filter(|b| b.is_mixed()).count() as f64;
    let e = mixed / checked.len() as f64 * fps.frame_duration();
    e.min(camera_interval.unwrap_or(f64::INFINITY))
}

/// The timeline from frames caught mid-switch, if there are enough of them.
fn blend_timing(
    observations: &[Observation],
    fps: FrameRate,
    camera_interval: Option<f64>,
) -> Option<DisplayTiming> {
    let d = fps.frame_duration();
    let mixed: Vec<(&Observation, Blend)> = observations
        .iter()
        .filter_map(|o| o.blend.filter(Blend::is_mixed).map(|b| (o, b)))
        .collect();
    if mixed.len() < MIN_MIXED || mixed.len() * 10 < observations.len() {
        return None;
    }
    // A rolling shutter is the same in every frame: the median slope of β
    // down the code, referred to the top of the picture.
    let slope = median(mixed.iter().map(|(_, b)| f64::from(b.slope)).collect());
    // The display switched to M+1 at t + delay + E·u, where the code's middle
    // row began its exposure `delay` after t, and u = 1 − β there. With
    // r = t + delay − (M+1)/fps, the offset is r + E·u.
    let points: Vec<BlendPoint> = mixed
        .iter()
        .map(|(o, b)| {
            let r = o.time - (f64::from(b.first) + 1.0) * d;
            let weight = f64::from(b.weight_next);
            let (r, u) = match (o.code_delay, o.code_rows) {
                // The code's own row, from the readout the camera reported.
                (Some(delay), _) => (r + delay, 1.0 - weight),
                // Referred to the picture's top row through the measured slope.
                (None, Some((centre, height))) => (r, 1.0 - (weight - slope * centre / height)),
                (None, None) => (r, 1.0 - weight),
            };
            BlendPoint {
                index: o.index,
                r,
                u,
                exposure: o.exposure,
            }
        })
        .collect();
    let u: Vec<f64> = points.iter().map(|p| p.u).collect();
    let max_exposure = camera_interval.unwrap_or(d);
    let exposure = match reported_exposure(mixed.iter().map(|(o, _)| *o)) {
        Some(e) => Exposure::Reported(e),
        None => {
            let spread = quantile(&u, 0.9) - quantile(&u, 0.1);
            (spread >= MIN_PHASE_SPREAD)
                .then(|| theil_sen(&points.iter().map(|p| (p.u, p.r)).collect::<Vec<_>>()))
                .flatten()
                .map(|(_, slope)| -slope)
                .filter(|&e| e > 0.0 && e <= 1.5 * max_exposure)
                .map_or(Exposure::Unknown { max: max_exposure }, Exposure::Measured)
        }
    };
    // Frames that reported their exposure are timed with their own; the
    // others with the capture's (the middle of the range, if it's unknown).
    let common = exposure.known().unwrap_or(max_exposure / 2.0);
    let switch = |p: &BlendPoint, e: f64| p.r + p.exposure.unwrap_or(e) * p.u;
    let offsets: Vec<(usize, f64)> = points
        .iter()
        .map(|p| (p.index, switch(p, common)))
        .collect();
    let values: Vec<f64> = offsets.iter().map(|p| p.1).collect();
    let offset = median(values.clone());
    // Scatter of the per-frame offsets, robustly; the median's standard
    // error is about 1.25 σ/√n.
    let mad = median(values.iter().map(|v| (v - offset).abs()).collect());
    let sigma = 1.4826 * mad;
    let n = values.len() as f64;
    let statistical = 2.0 * 1.25 * sigma / n.sqrt();
    let typical_u = median(u.clone());
    let uncertainty = match exposure {
        Exposure::Unknown { max } => statistical + typical_u * max / 2.0,
        _ => statistical + typical_u * common * 0.1,
    };
    // Judder is a jump in switch time. With the exposure unknown, judge it at
    // the longest exposure allowed, which shows the biggest jump a given
    // change in β can mean (noise in β is far smaller).
    let judder = match exposure {
        Exposure::Unknown { max } => count_judder(
            &points.iter().map(|p| switch(p, max)).collect::<Vec<_>>(),
            d,
        ),
        _ => count_judder(&values, d),
    };
    Some(DisplayTiming {
        offset,
        uncertainty,
        method: TimingMethod::Blend,
        exposure: Some(exposure),
        frame_offsets: offsets,
        judder,
    })
}

/// A frame caught mid-switch, as [`blend_timing`] uses it.
struct BlendPoint {
    /// Index into the scan's video samples.
    index: usize,
    /// When a row began its exposure (the code's middle row, or the picture's
    /// top row without a reported readout), less `(M+1)/fps` (seconds).
    r: f64,
    /// The share of that row's exposure before the switch.
    u: f64,
    /// The exposure the camera reported.
    exposure: Option<f64>,
}

/// Frames whose offset is more than [`JUDDER_FRAMES`] from the median of
/// the fifteen either side.
fn count_judder(offsets: &[f64], d: f64) -> usize {
    (0..offsets.len())
        .filter(|&i| {
            let lo = i.saturating_sub(15);
            let hi = (i + 16).min(offsets.len());
            let local = median(offsets[lo..hi].to_vec());
            (offsets[i] - local).abs() > JUDDER_FRAMES * d
        })
        .count()
}

/// The timeline from the bounds every observation puts on it. A frame read
/// from a picture is the one on screen for most of the exposure, so at its
/// middle: half the exposure (each frame's own, if the camera reported it,
/// else `exposure` seconds) after the exposure's start, where camera
/// timestamps are, and at the code's row if the camera reported its readout.
/// `reported` says the exposure is the camera's, not an estimate.
fn bound_timing(
    observations: &[Observation],
    fps: FrameRate,
    exposure: f64,
    reported: bool,
) -> Option<DisplayTiming> {
    if observations.is_empty() {
        return None;
    }
    let d = fps.frame_duration();
    let seen = |o: &Observation| {
        o.time + o.code_delay.unwrap_or(0.0) + o.exposure.unwrap_or(exposure) / 2.0
    };
    // Seeing frame N at time t means offset ∈ [t − (N+1)/fps, t − N/fps].
    let mut lower: Vec<f64> = observations
        .iter()
        .map(|o| seen(o) - (f64::from(o.frame) + 1.0) * d)
        .collect();
    let mut upper: Vec<f64> = observations
        .iter()
        .map(|o| seen(o) - f64::from(o.frame) * d)
        .collect();
    lower.sort_by(f64::total_cmp);
    upper.sort_by(f64::total_cmp);
    // Ignore the most extreme 2% on each side (frames caught mid-transition).
    let trim = observations.len() / 50;
    let (lo, hi) = (lower[lower.len() - 1 - trim], upper[trim]);
    // An estimated exposure is only roughly known; allow for half of its
    // effect again.
    let slack = if reported { 0.0 } else { exposure / 4.0 };
    if lo <= hi {
        return Some(DisplayTiming {
            offset: (lo + hi) / 2.0,
            uncertainty: (hi - lo) / 2.0 + slack,
            ..Default::default()
        });
    }
    // Inconsistent observations (judder, a stall): use the median mid-frame estimate.
    let mids: Vec<f64> = observations
        .iter()
        .map(|o| seen(o) - (f64::from(o.frame) + 0.5) * d)
        .collect();
    Some(DisplayTiming {
        offset: median(mids),
        uncertainty: d / 2.0 + slack,
        ..Default::default()
    })
}

/// How consistently frames are timestamped at the same point in their
/// frame: the length of the mean of `e^{2πi·phase}`, where `phase` is the
/// fractional frame of `t − N/fps`. 1 means every frame was caught at the
/// same phase (a copy, timestamped at frame starts, or a camera locked to the
/// pattern's rate); near 0, at all phases (a camera at another rate, or any
/// capture that samples the screen).
pub(crate) fn phase_consistency(observations: &[Observation], fps: FrameRate) -> f64 {
    if observations.is_empty() {
        return 1.0;
    }
    let d = fps.frame_duration();
    let (mut c, mut s) = (0.0, 0.0);
    for o in observations {
        let phase = (o.time - f64::from(o.frame) * d) / d;
        let angle = std::f64::consts::TAU * phase;
        c += angle.cos();
        s += angle.sin();
    }
    let n = observations.len() as f64;
    (c * c + s * s).sqrt() / n
}

/// The `q` quantile (0 to 1) of `v`, by nearest rank.
fn quantile(v: &[f64], q: f64) -> f64 {
    let mut v = v.to_vec();
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * q).round() as usize]
}

/// A line through `(x, y)` robust to outliers: the median of pairwise slopes,
/// and the median intercept for it. Returns `(intercept, slope)`.
fn theil_sen(points: &[(f64, f64)]) -> Option<(f64, f64)> {
    // Long captures are thinned evenly: a few hundred points pin a line.
    let step = points.len().div_ceil(400).max(1);
    let p: Vec<(f64, f64)> = points.iter().step_by(step).copied().collect();
    let mut slopes = Vec::with_capacity(p.len() * p.len() / 2);
    for i in 0..p.len() {
        for j in i + 1..p.len() {
            let dx = p[j].0 - p[i].0;
            if dx.abs() > 1e-6 {
                slopes.push((p[j].1 - p[i].1) / dx);
            }
        }
    }
    if slopes.is_empty() {
        return None;
    }
    let slope = median(slopes);
    let intercept = median(p.iter().map(|(x, y)| y - slope * x).collect());
    Some((intercept, slope))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blend(first: u32, w: f64) -> Blend {
        Blend {
            first,
            weight_next: w as f32,
            slope: 0.0,
            rms: 0.05,
            margin: 3.0,
        }
    }

    /// A camera at `camera_fps` filming a display that shows frame N from
    /// `start + N/fps`. The QR code's middle row begins its exposure `delay`
    /// after each picture's timestamp (a rolling shutter), and is exposed for
    /// `exposure(t)`. The camera reports both.
    fn film(
        fps: f64,
        camera_fps: f64,
        exposure: impl Fn(f64) -> f64,
        delay: f64,
        start: f64,
        seconds: f64,
    ) -> Vec<Observation> {
        (0..(seconds * camera_fps) as usize)
            .filter_map(|k| {
                let t = 0.0071 + k as f64 / camera_fps;
                let (row, e) = (t + delay, exposure(t));
                let at = |t: f64| ((t - start) * fps).floor();
                let (a, b) = (at(row), at(row + e));
                if a < 0.0 {
                    return None;
                }
                // Share of the row's exposure after the switch to a+1 (exposure shorter than a frame).
                let w = if b > a {
                    let switch = start + (a + 1.0) / fps;
                    (row + e - switch) / e
                } else {
                    0.0
                };
                let frame = if w >= 0.5 { a as u32 + 1 } else { a as u32 };
                Some(Observation {
                    index: k,
                    time: t,
                    frame,
                    qr: true,
                    blend: Some(blend(a as u32, w)),
                    code_rows: None,
                    corners: None,
                    exposure: Some(e),
                    code_delay: Some(delay),
                })
            })
            .collect()
    }

    /// The same observations from a camera that doesn't report its exposure or readout.
    fn unreported(observations: Vec<Observation>) -> Vec<Observation> {
        observations
            .into_iter()
            .map(|o| Observation {
                exposure: None,
                code_delay: None,
                ..o
            })
            .collect()
    }

    /// A camera with exposure `e` and no rolling shutter, reporting nothing.
    fn observe(fps: f64, camera_fps: f64, e: f64, start: f64, seconds: f64) -> Vec<Observation> {
        unreported(film(fps, camera_fps, |_| e, 0.0, start, seconds))
    }

    #[test]
    fn locked_camera_is_timed_by_blends() {
        // A 30 fps camera on a 30 fps display, 1/40 s exposures starting 0.9
        // of a frame in: every frame is blended at the same phase, which
        // bounds can't resolve.
        let fps: FrameRate = "30".parse().unwrap();
        let start = 0.0071 - 0.9 / 30.0 + 0.2;
        let obs = observe(30.0, 30.0, 0.025, start, 6.0);
        let t = display_timing(&obs, fps, Some(1.0 / 30.0)).unwrap();
        assert_eq!(t.method, TimingMethod::Blend);
        // The exposure can't be measured (no phase drift): assumed ≤ 1/30 s.
        assert_eq!(t.exposure, Some(Exposure::Unknown { max: 1.0 / 30.0 }));
        assert!(
            (t.offset - start).abs() <= t.uncertainty + 1e-4,
            "offset {} ± {}",
            t.offset,
            t.uncertainty
        );
        assert!(t.uncertainty < 0.01, "± {}", t.uncertainty);
        // Bounds alone are stuck at half a frame.
        let bounds = bound_timing(&obs, fps, 0.0, false).unwrap();
        assert!(bounds.uncertainty > 0.012);
    }

    #[test]
    fn a_locked_camera_that_reports_its_exposure_is_timed_exactly() {
        // As above, but the camera says how long each exposure was.
        let fps: FrameRate = "30".parse().unwrap();
        let start = 0.0071 - 0.9 / 30.0 + 0.2;
        let obs = film(30.0, 30.0, |_| 0.025, 0.0, start, 6.0);
        let t = display_timing(&obs, fps, Some(1.0 / 30.0)).unwrap();
        assert_eq!(t.method, TimingMethod::Blend);
        assert_eq!(t.exposure, Some(Exposure::Reported(0.025)));
        assert!((t.offset - start).abs() < 1e-6, "offset {}", t.offset);
        assert!(t.uncertainty < 0.001, "± {}", t.uncertainty);
    }

    #[test]
    fn drifting_camera_measures_the_exposure() {
        // 29.5 fps against 30: the phase sweeps through every value.
        let fps: FrameRate = "30".parse().unwrap();
        let obs = observe(30.0, 29.5, 0.02, 0.137, 6.0);
        let t = display_timing(&obs, fps, Some(1.0 / 29.5)).unwrap();
        assert_eq!(t.method, TimingMethod::Blend);
        let Some(Exposure::Measured(e)) = t.exposure else {
            panic!("exposure {:?}", t.exposure);
        };
        assert!((e - 0.02).abs() < 0.001, "exposure {e}");
        assert!((t.offset - 0.137).abs() < 0.0005, "offset {}", t.offset);
        assert!(t.uncertainty < 0.002, "± {}", t.uncertainty);
        assert_eq!(t.judder, 0);
    }

    #[test]
    fn reported_exposures_follow_auto_exposure() {
        // Auto-exposure lengthens the exposure from 8 to 24 ms halfway through.
        let fps: FrameRate = "30".parse().unwrap();
        let exposure = |t: f64| if t < 3.0 { 0.008 } else { 0.024 };
        let obs = film(30.0, 29.5, exposure, 0.0, 0.137, 6.0);
        // The worst error in a frame's own switch time.
        let worst = |t: &DisplayTiming| {
            t.frame_offsets
                .iter()
                .map(|&(_, a)| (a - 0.137).abs())
                .fold(0.0, f64::max)
        };
        let t = display_timing(&obs, fps, Some(1.0 / 29.5)).unwrap();
        assert_eq!(t.method, TimingMethod::Blend);
        assert!((t.offset - 0.137).abs() < 1e-6, "offset {}", t.offset);
        assert!(worst(&t) < 1e-6, "{} s", worst(&t));
        assert_eq!(t.judder, 0);
        // One exposure fitted to the whole capture can't follow the change:
        // the frames exposed for 8 ms are timed as if for 24.
        let fitted = display_timing(&unreported(obs), fps, Some(1.0 / 29.5)).unwrap();
        assert!(worst(&fitted) > 0.005, "{} s", worst(&fitted));
    }

    #[test]
    fn a_reported_readout_times_the_codes_row() {
        // The code's middle row starts its exposure 9 ms after the picture's
        // top row, and its blend is too small to show the slope.
        let fps: FrameRate = "30".parse().unwrap();
        let obs = film(30.0, 29.5, |_| 0.02, 0.009, 0.137, 6.0);
        let t = display_timing(&obs, fps, Some(1.0 / 29.5)).unwrap();
        assert!((t.offset - 0.137).abs() < 1e-6, "offset {}", t.offset);
        // Without it, the switch is timed as if the code were at the top.
        let without: Vec<Observation> = obs
            .into_iter()
            .map(|o| Observation {
                code_delay: None,
                ..o
            })
            .collect();
        let t = display_timing(&without, fps, Some(1.0 / 29.5)).unwrap();
        assert!(
            (t.offset - (0.137 - 0.009)).abs() < 1e-6,
            "offset {}",
            t.offset
        );
    }

    #[test]
    fn a_reported_exposure_centres_the_bounds() {
        // About 25 fps filming 30 with 20 ms exposures, no blends measured:
        // each picture shows the frame on screen at the middle of its exposure.
        let fps: FrameRate = "30".parse().unwrap();
        let no_blends = |obs: Vec<Observation>| -> Vec<Observation> {
            obs.into_iter()
                .map(|o| Observation { blend: None, ..o })
                .collect()
        };
        let obs = no_blends(film(30.0, 24.93, |_| 0.02, 0.0, 0.137, 6.0));
        let t = display_timing(&obs, fps, Some(0.04)).unwrap();
        assert_eq!(t.method, TimingMethod::Bounds);
        assert_eq!(t.exposure, Some(Exposure::Reported(0.02)));
        assert!((t.offset - 0.137).abs() < 0.001, "offset {}", t.offset);
        assert!(
            (t.offset - 0.137).abs() <= t.uncertainty,
            "± {}",
            t.uncertainty
        );
        // Without it, the pictures are taken as instants at their timestamps:
        // half an exposure early.
        let t = display_timing(&unreported(obs), fps, Some(0.04)).unwrap();
        assert!(
            (t.offset - (0.137 - 0.01)).abs() < 0.002,
            "offset {}",
            t.offset
        );
    }

    #[test]
    fn phase_consistency_separates_copies_from_cameras() {
        let fps: FrameRate = "30".parse().unwrap();
        let copy: Vec<Observation> = (0..60)
            .map(|n| Observation {
                index: n,
                time: 0.5 + n as f64 / 30.0,
                frame: n as u32,
                qr: false,
                blend: None,
                code_rows: None,
                corners: None,
                exposure: None,
                code_delay: None,
            })
            .collect();
        assert!(phase_consistency(&copy, fps) > 0.99);
        let camera = observe(30.0, 25.0, 0.01, 0.0, 4.0);
        assert!(phase_consistency(&camera, fps) < 0.3);
    }
}
