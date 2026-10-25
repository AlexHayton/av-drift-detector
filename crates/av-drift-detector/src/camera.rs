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
//! interval, and the reported uncertainty says so.
//!
//! Camera timestamps are taken to mark the start of the exposure of the
//! picture's top row, as phones record them. A rolling shutter exposes lower
//! rows later; the change in `β` down the QR code measures that, and the
//! timing is referred back to the top row.

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
    /// The camera's exposure time, if it could be measured (seconds).
    pub exposure: Option<f64>,
    /// The longest exposure assumed when it couldn't be measured (seconds).
    pub max_exposure: Option<f64>,
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
}

impl Observation {
    pub fn from_scan(scan: &MediaScan) -> Vec<Observation> {
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
                Some(Observation {
                    index,
                    time: v.time,
                    frame: s.frame_id,
                    qr: s.source.is_qr(),
                    blend: s.blend,
                    code_rows,
                    corners: s.corners,
                })
            })
            .collect()
    }
}

/// The display timeline for camera observations. `camera_interval` is the
/// capture's typical frame interval (seconds), the longest the exposure can be.
pub(crate) fn display_timing(
    observations: &[Observation],
    fps: FrameRate,
    camera_interval: Option<f64>,
) -> Option<DisplayTiming> {
    blend_timing(observations, fps, camera_interval).or_else(|| {
        let exposure = typical_exposure(observations, fps, camera_interval);
        bound_timing(observations, fps, exposure)
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
    // r = t − (M+1)/fps; the switch to M+1 came at t + E·u, so offset = r + E·u.
    let points: Vec<(usize, f64, f64)> = mixed
        .iter()
        .map(|(o, b)| {
            let top_weight = match o.code_rows {
                Some((centre, height)) => f64::from(b.weight_next) - slope * centre / height,
                None => f64::from(b.weight_next),
            };
            let r = o.time - (f64::from(b.first) + 1.0) * d;
            (o.index, r, 1.0 - top_weight)
        })
        .collect();
    let u: Vec<f64> = points.iter().map(|p| p.2).collect();
    let spread = quantile(&u, 0.9) - quantile(&u, 0.1);
    let max_exposure = camera_interval.unwrap_or(d);
    let fitted = (spread >= MIN_PHASE_SPREAD)
        .then(|| theil_sen(&points.iter().map(|p| (p.2, p.1)).collect::<Vec<_>>()))
        .flatten()
        .map(|(_, slope)| -slope)
        .filter(|&e| e > 0.0 && e <= 1.5 * max_exposure);
    let exposure = fitted.unwrap_or(max_exposure / 2.0);
    let offsets: Vec<(usize, f64)> = points
        .iter()
        .map(|&(i, r, u)| (i, r + exposure * u))
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
    let uncertainty = match fitted {
        Some(_) => statistical + typical_u * exposure * 0.1,
        None => statistical + typical_u * max_exposure / 2.0,
    };
    // Judder is a jump in switch time. With the exposure unknown, judge it at
    // the longest exposure allowed, which shows the biggest jump a given
    // change in β can mean (noise in β is far smaller).
    let judder = match fitted {
        Some(_) => count_judder(&values, d),
        None => count_judder(
            &points
                .iter()
                .map(|&(_, r, u)| r + max_exposure * u)
                .collect::<Vec<_>>(),
            d,
        ),
    };
    Some(DisplayTiming {
        offset,
        uncertainty,
        method: TimingMethod::Blend,
        exposure: fitted,
        max_exposure: fitted.is_none().then_some(max_exposure),
        frame_offsets: offsets,
        judder,
    })
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
/// middle: `exposure` (seconds) moves each observation there from the
/// exposure's start, where camera timestamps are.
fn bound_timing(
    observations: &[Observation],
    fps: FrameRate,
    exposure: f64,
) -> Option<DisplayTiming> {
    if observations.is_empty() {
        return None;
    }
    let d = fps.frame_duration();
    let mid = exposure / 2.0;
    // Seeing frame N at time t means offset ∈ [t − (N+1)/fps, t − N/fps].
    let mut lower: Vec<f64> = observations
        .iter()
        .map(|o| o.time + mid - (f64::from(o.frame) + 1.0) * d)
        .collect();
    let mut upper: Vec<f64> = observations
        .iter()
        .map(|o| o.time + mid - f64::from(o.frame) * d)
        .collect();
    lower.sort_by(f64::total_cmp);
    upper.sort_by(f64::total_cmp);
    // Ignore the most extreme 2% on each side (frames caught mid-transition).
    let trim = observations.len() / 50;
    let (lo, hi) = (lower[lower.len() - 1 - trim], upper[trim]);
    // The exposure is only roughly known; allow for half of its effect again.
    let slack = mid / 2.0;
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
        .map(|o| o.time + mid - (f64::from(o.frame) + 0.5) * d)
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

    /// A camera at `camera_fps` with exposure `e`, filming a display that shows
    /// frame N from `start + N/fps`.
    fn observe(fps: f64, camera_fps: f64, e: f64, start: f64, seconds: f64) -> Vec<Observation> {
        (0..(seconds * camera_fps) as usize)
            .filter_map(|k| {
                let t = 0.0071 + k as f64 / camera_fps;
                let at = |t: f64| ((t - start) * fps).floor();
                let (a, b) = (at(t), at(t + e));
                if a < 0.0 {
                    return None;
                }
                // Share of [t, t+e] after the switch to a+1 (exposure shorter than a frame).
                let w = if b > a {
                    let switch = start + (a + 1.0) / fps;
                    (t + e - switch) / e
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
                })
            })
            .collect()
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
        assert_eq!(t.exposure, None);
        assert!(
            (t.offset - start).abs() <= t.uncertainty + 1e-4,
            "offset {} ± {}",
            t.offset,
            t.uncertainty
        );
        assert!(t.uncertainty < 0.01, "± {}", t.uncertainty);
        // Bounds alone are stuck at half a frame.
        let bounds = bound_timing(&obs, fps, 0.0).unwrap();
        assert!(bounds.uncertainty > 0.012);
    }

    #[test]
    fn drifting_camera_measures_the_exposure() {
        // 29.5 fps against 30: the phase sweeps through every value.
        let fps: FrameRate = "30".parse().unwrap();
        let obs = observe(30.0, 29.5, 0.02, 0.137, 6.0);
        let t = display_timing(&obs, fps, Some(1.0 / 29.5)).unwrap();
        assert_eq!(t.method, TimingMethod::Blend);
        let e = t.exposure.expect("exposure measured");
        assert!((e - 0.02).abs() < 0.001, "exposure {e}");
        assert!((t.offset - 0.137).abs() < 0.0005, "offset {}", t.offset);
        assert!(t.uncertainty < 0.002, "± {}", t.uncertainty);
        assert_eq!(t.judder, 0);
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
            })
            .collect();
        assert!(phase_consistency(&copy, fps) > 0.99);
        let camera = observe(30.0, 25.0, 0.01, 0.0, 4.0);
        assert!(phase_consistency(&camera, fps) < 0.3);
    }
}
