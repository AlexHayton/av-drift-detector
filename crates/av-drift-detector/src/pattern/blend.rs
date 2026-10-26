//! Pictures that caught the display switching between two frames.
//!
//! A camera mixes the two frames in light, then applies its tone curve, so a
//! module that went from dark to light reads `f(β)` and one that went from
//! light to dark reads `1 − f(1 − β)`. Only a camera whose brightness is
//! proportional to light (as a digital copy's mix is) makes the two agree;
//! the gap between them measures the curve. The fits here model it as a
//! power, `s ≈ (a + β·(b − a))^(1/γ)`, and measure `γ` along with `β`
//! where the picture shows it: both kinds of change, and a mix far enough
//! from either frame. Elsewhere they use what the capture's pictures have
//! shown so far ([`ToneCurve`]). A curve that is symmetric about mid-grey
//! (an S-shaped contrast boost) bends both kinds of change alike, so it
//! isn't seen.

use std::collections::VecDeque;

/// A picture read as a blend of two consecutive frames: a camera whose
/// exposure spanned the display's switch, or a capture that mixed frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blend {
    /// The earlier of the two frames: the picture is a mix of `first` and `first + 1`.
    pub first: u32,
    /// The weight of `first + 1` at the stamp's middle row, from 0 to 1: the
    /// share of that row's exposure that came after the display switched.
    pub weight_next: f32,
    /// How much the weight grows from the stamp's top row to its bottom row
    /// (a rolling shutter exposes lower rows later). Zero for the barcode.
    pub slope: f32,
    /// The tone curve the fit allowed for, as a gamma: brightness (black 0,
    /// white 1) taken as light to the power `1/gamma`. 1 where brightness is
    /// proportional to light, as in a digital copy. Measured from this
    /// picture where it showed it, otherwise the capture's so far.
    pub gamma: f32,
    /// Root-mean-square error of the fit (black 0, white 1).
    pub rms: f32,
    /// The RMS error of the best fit naming a different frame, over `rms`.
    pub margin: f32,
}

impl Blend {
    /// The frame that covered most of the exposure.
    pub fn dominant(&self) -> u32 {
        if self.weight_next < 0.5 {
            self.first
        } else {
            self.first + 1
        }
    }

    /// Whether both frames are visibly present.
    pub fn is_mixed(&self) -> bool {
        (0.05..=0.95).contains(&self.weight_next)
    }
}

/// Fits worse than this (root-mean-square error, with black at 0 and white
/// at 1) aren't a reading of the stamp.
pub const MAX_RMS: f32 = 0.2;
/// The best fit must beat the best one that names a different frame by this
/// factor in RMS error.
pub const MIN_MARGIN: f32 = 1.25;

/// The gammas a fit may find: linear to well beyond any camera's.
const GAMMA_RANGE: (f64, f64) = (0.7, 3.5);
/// A picture measures the curve only if it has at least this many cells
/// that went each way.
const MIN_EACH_WAY: usize = 2;
/// And only if its mix is within this range: nearer either frame, one kind
/// of change reads close to black or white whatever the curve.
const MEASURE_WEIGHTS: std::ops::RangeInclusive<f32> = 0.12..=0.88;

/// What a capture's pictures have shown of the camera's tone curve: the
/// median gamma of the latest ones that measured it, 1 (linear) before any.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToneCurve {
    recent: VecDeque<f32>,
}

impl ToneCurve {
    /// Measurements kept: about a second of mixed frames.
    const KEEP: usize = 31;

    /// The gamma to fit with where a picture doesn't show its own.
    pub fn gamma(&self) -> f32 {
        if self.recent.is_empty() {
            return 1.0;
        }
        let mut v: Vec<f32> = self.recent.iter().copied().collect();
        v.sort_by(f32::total_cmp);
        v[v.len() / 2]
    }

    /// A capture already known to have the curve `gamma`.
    #[cfg(test)]
    pub(crate) fn known(gamma: f32) -> Self {
        let mut tone = ToneCurve::default();
        tone.observe(gamma);
        tone
    }

    fn observe(&mut self, gamma: f32) {
        if self.recent.len() == Self::KEEP {
            self.recent.pop_front();
        }
        self.recent.push_back(gamma);
    }
}

/// The QR modules or barcode cells of a picture, as a fit of one candidate
/// pair of frames sees them: grouped by row and by how they changed between
/// the two. A fit predicts the same brightness for every cell in a group,
/// so its error comes exactly from each group's count, sum and sum of
/// squares, and fitting costs the same however many cells there are.
#[derive(Clone, Debug)]
pub(crate) struct Cells {
    /// Per row, top to bottom, indexed by kind: `2·a + b` for a cell that
    /// was `a` in the earlier frame and `b` in the later one (0 dark, 1 light).
    rows: Vec<[Group; 4]>,
}

const STAYED_DARK: usize = 0;
const WENT_LIGHT: usize = 1;
const WENT_DARK: usize = 2;
const STAYED_LIGHT: usize = 3;

/// The brightness of one row's cells of one kind.
#[derive(Clone, Copy, Debug, Default)]
struct Group {
    n: f64,
    sum: f64,
    sum_sq: f64,
}

impl Group {
    /// `Σ (s − predicted)²` over the group's cells.
    fn error(&self, predicted: f64) -> f64 {
        if self.n == 0.0 {
            return 0.0;
        }
        (self.sum_sq - 2.0 * predicted * self.sum + self.n * predicted * predicted).max(0.0)
    }
}

impl Cells {
    /// No cells yet, in `rows` evenly spaced rows.
    pub fn new(rows: usize) -> Self {
        Cells {
            rows: vec![[Group::default(); 4]; rows],
        }
    }

    /// Adds a cell in `row` (0 at the top) with brightness `s`, normalised so
    /// that black is 0 and white is 1. `a` and `b` say whether it's light in
    /// the earlier frame and in the later one.
    pub fn add(&mut self, row: usize, s: f32, a: bool, b: bool) {
        let g = &mut self.rows[row][2 * usize::from(a) + usize::from(b)];
        let s = f64::from(s);
        g.n += 1.0;
        g.sum += s;
        g.sum_sq += s * s;
    }

    /// Row `r`'s position, from −½ at the top of the stamp to ½ at the bottom.
    fn y(&self, r: usize) -> f64 {
        (r as f64 + 0.5) / self.rows.len() as f64 - 0.5
    }

    fn count(&self, kind: usize) -> f64 {
        self.rows.iter().map(|g| g[kind].n).sum()
    }
}

/// Fits each candidate pair `m`, `m + 1` (whose cells `cells(m)` gives) as a
/// blend, and [`choose`]s among them. All are fitted with the curve `tone`
/// has seen so far; the best, and the best naming another frame, then
/// measure their own where they can, and the winner's measurement goes into
/// `tone`. `slope` allows for a rolling shutter.
pub(crate) fn fit_candidates(
    candidates: impl IntoIterator<Item = u32>,
    mut cells: impl FnMut(u32) -> Option<Cells>,
    slope: bool,
    tone: &mut ToneCurve,
) -> Option<Blend> {
    let prior = f64::from(tone.gamma());
    let (mut fits, groups): (Vec<Blend>, Vec<Cells>) = candidates
        .into_iter()
        .filter_map(|m| {
            let c = cells(m)?;
            Some((fit_at(&c, m, slope, prior)?, c))
        })
        .unzip();
    let mut measured: Vec<bool> = vec![false; fits.len()];
    let best = lowest(&fits, |_| true)?;
    let other = lowest(&fits, |f| f.dominant() != fits[best].dominant());
    for i in [Some(best), other].into_iter().flatten() {
        if let Some((fit, ok)) = measure(&groups[i], slope, fits[i]) {
            (fits[i], measured[i]) = (fit, ok);
        }
    }
    let winner = lowest(&fits, |_| true)?;
    let chosen = choose(&fits)?;
    if measured[winner] {
        tone.observe(fits[winner].gamma);
    }
    Some(chosen)
}

/// The index of the fit with the lowest RMS error among those `keep` allows.
fn lowest(fits: &[Blend], keep: impl Fn(&Blend) -> bool) -> Option<usize> {
    (0..fits.len())
        .filter(|&i| keep(&fits[i]))
        .min_by(|&i, &j| fits[i].rms.total_cmp(&fits[j].rms))
}

/// `fit` (made with an assumed curve) refitted with the curve that suits
/// `cells` best, if they show one; and whether they did.
fn measure(cells: &Cells, slope: bool, fit: Blend) -> Option<(Blend, bool)> {
    let first = fit.first;
    let each_way = MIN_EACH_WAY as f64;
    // A picture of one frame, whatever the curve (most pictures).
    if cells.count(WENT_LIGHT) < each_way || cells.count(WENT_DARK) < each_way || !fit.is_mixed() {
        return Some((fit, false));
    }
    // A coarse scan of log gamma, then a golden-section search around the best.
    let (lo, hi) = (GAMMA_RANGE.0.ln(), GAMMA_RANGE.1.ln());
    let error = |lg: f64| fit_at(cells, first, slope, lg.exp()).map_or(f32::INFINITY, |b| b.rms);
    const STEPS: usize = 8;
    let step = (hi - lo) / STEPS as f64;
    let scan: Vec<f32> = (0..=STEPS).map(|k| error(lo + step * k as f64)).collect();
    let k = (0..=STEPS).min_by(|&i, &j| scan[i].total_cmp(&scan[j]))?;
    let (mut a, mut b) = (
        lo + step * k.saturating_sub(1) as f64,
        lo + step * (k + 1).min(STEPS) as f64,
    );
    let r = (5f64.sqrt() - 1.0) / 2.0;
    let (mut c, mut d) = (b - r * (b - a), a + r * (b - a));
    let (mut ec, mut ed) = (error(c), error(d));
    for _ in 0..12 {
        if ec < ed {
            (b, d, ed) = (d, c, ec);
            c = b - r * (b - a);
            ec = error(c);
        } else {
            (a, c, ec) = (c, d, ed);
            d = a + r * (b - a);
            ed = error(d);
        }
    }
    let found = fit_at(cells, first, slope, ((a + b) / 2.0).exp())?;
    // The assumed curve may suit them as well as any (the search is finite).
    let found = if found.rms < fit.rms { found } else { fit };
    Some(if MEASURE_WEIGHTS.contains(&found.weight_next) {
        (found, true)
    } else {
        (fit, false)
    })
}

/// Least squares for `s ≈ (a + β(y)·(b − a))^(1/γ)` at a given `gamma`, with
/// `β(y) = β₀ + β₁·y` if `slope` (else `β₁ = 0`): linear in `β` once each
/// group's mean brightness is taken back to light. (Means rather than single
/// cells, so that noise, which the curve bends, averages out first.) The
/// error is measured on the brightness itself, so fits with different
/// curves compare fairly.
fn fit_at(cells: &Cells, first: u32, slope: bool, gamma: f64) -> Option<Blend> {
    let linear = gamma == 1.0;
    let to_light = |s: f64| if linear { s } else { s.max(0.0).powf(gamma) };
    let to_signal = |x: f64| {
        if linear {
            x
        } else {
            x.max(0.0).powf(1.0 / gamma)
        }
    };
    // The kinds that changed, with their colour in the earlier frame and `b − a`.
    let changed = [(WENT_LIGHT, 0.0, 1.0), (WENT_DARK, 1.0, -1.0)];
    let (mut s00, mut s01, mut s11, mut r0, mut r1) = (0.0f64, 0.0, 0.0, 0.0, 0.0);
    let mut n = 0.0;
    for (r, groups) in cells.rows.iter().enumerate() {
        n += groups.iter().map(|g| g.n).sum::<f64>();
        for (kind, a, d) in changed {
            let g = groups[kind];
            if g.n == 0.0 {
                continue;
            }
            let (x0, x1) = (d, if slope { d * cells.y(r) } else { 0.0 });
            let res = to_light(g.sum / g.n) - a;
            s00 += g.n * x0 * x0;
            s01 += g.n * x0 * x1;
            s11 += g.n * x1 * x1;
            r0 += g.n * x0 * res;
            r1 += g.n * x1 * res;
        }
    }
    if n == 0.0 {
        return None;
    }
    let det = s00 * s11 - s01 * s01;
    let (b0, b1) = if slope && det.abs() > 1e-9 {
        ((r0 * s11 - r1 * s01) / det, (s00 * r1 - s01 * r0) / det)
    } else if s00 > 0.0 {
        (r0 / s00, 0.0)
    } else {
        (0.0, 0.0)
    };
    let mut sq = 0.0;
    for (r, g) in cells.rows.iter().enumerate() {
        // Unchanged cells are black or white whatever the curve.
        sq += g[STAYED_DARK].error(0.0) + g[STAYED_LIGHT].error(1.0);
        let beta = (b0 + b1 * cells.y(r)).clamp(0.0, 1.0);
        if g[WENT_LIGHT].n > 0.0 {
            sq += g[WENT_LIGHT].error(to_signal(beta));
        }
        if g[WENT_DARK].n > 0.0 {
            sq += g[WENT_DARK].error(to_signal(1.0 - beta));
        }
    }
    Some(Blend {
        first,
        weight_next: b0.clamp(0.0, 1.0) as f32,
        slope: b1 as f32,
        gamma: gamma as f32,
        rms: (sq / n).sqrt() as f32,
        margin: 0.0,
    })
}

/// Chooses among fits of candidate frame pairs: the best, if it's good and
/// clearly better than any fit that names another frame.
fn choose(fits: &[Blend]) -> Option<Blend> {
    let best = fits.iter().min_by(|a, b| a.rms.total_cmp(&b.rms))?;
    let runner = fits
        .iter()
        .filter(|f| f.dominant() != best.dominant())
        .map(|f| f.rms)
        .fold(f32::INFINITY, f32::min);
    let mut fit = Blend {
        margin: runner / best.rms.max(1e-6),
        ..*best
    };
    // A picture of one frame fits as either neighbouring pair; name it the
    // same way whichever won: that frame, with nothing of the next.
    if !fit.is_mixed() && fit.weight_next >= 0.5 {
        fit.first += 1;
        fit.weight_next = 0.0;
    }
    (fit.rms < MAX_RMS && fit.margin >= MIN_MARGIN).then_some(fit)
}

/// Cameras' tone curves, for tests: light (0 black, 1 white) to signal.
#[cfg(test)]
pub(crate) mod curves {
    use crate::pattern::canvas::{BLACK, LumaImage, WHITE};

    /// A tone curve: light (0 to 1) to signal (0 to 1).
    pub type Curve = fn(f64) -> f64;

    /// The sRGB curve, as most phones roughly use.
    pub fn srgb(x: f64) -> f64 {
        if x <= 0.003_130_8 {
            12.92 * x
        } else {
            1.055 * x.powf(1.0 / 2.4) - 0.055
        }
    }

    /// A plain power curve of gamma 2.2.
    pub fn gamma22(x: f64) -> f64 {
        x.powf(1.0 / 2.2)
    }

    /// Luma `v` as light, 0 at black and 1 at white.
    pub fn light(v: f64) -> f64 {
        let (b, w) = (f64::from(BLACK), f64::from(WHITE));
        ((v - b) / (w - b)).clamp(0.0, 1.0)
    }

    /// Light encoded as luma through `curve`.
    pub fn encode(light: f64, curve: Curve) -> u8 {
        let (b, w) = (f64::from(BLACK), f64::from(WHITE));
        (b + (w - b) * curve(light.clamp(0.0, 1.0))).round() as u8
    }

    /// What a camera with tone `curve` records of a picture that's `a` for
    /// `1 − w` of the exposure and `b` for `w` of it: the two mix in light.
    pub fn mix_through(a: &LumaImage, b: &LumaImage, w: f64, curve: Curve) -> LumaImage {
        let mut out = a.clone();
        for (o, (&x, &y)) in out.data.iter_mut().zip(a.data.iter().zip(&b.data)) {
            let mixed = light(f64::from(x)) * (1.0 - w) + light(f64::from(y)) * w;
            *o = encode(mixed, curve);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture's cells, as `(row, s, a, b)`, with noise and some of each kind.
    fn picture(rows: usize, weight: f64, slope: f64, gamma: f64) -> Vec<(usize, f32, bool, bool)> {
        let mut seed = 99u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            0.05 * ((seed >> 16) as f64 / 32768.0 - 1.0)
        };
        let mut out = Vec::new();
        for r in 0..rows {
            let y = (r as f64 + 0.5) / rows as f64 - 0.5;
            let beta = (weight + slope * y).clamp(0.0, 1.0);
            for k in 0..(5 + r % 4) {
                let (a, b) = ((k + r) % 3 == 0, (k * 7 + r) % 4 < 2);
                let light = if a { 1.0 - beta } else { 0.0 } + if b { beta } else { 0.0 };
                out.push((r, (light.powf(1.0 / gamma) + noise()) as f32, a, b));
            }
        }
        out
    }

    fn grouped(rows: usize, picture: &[(usize, f32, bool, bool)]) -> Cells {
        let mut cells = Cells::new(rows);
        for &(r, s, a, b) in picture {
            cells.add(r, s, a, b);
        }
        cells
    }

    #[test]
    fn grouped_error_is_exact() {
        let rows = 25;
        for gamma in [1.0, 1.6, 2.2] {
            let picture = picture(rows, 0.4, 0.3, gamma);
            let fit = fit_at(&grouped(rows, &picture), 7, true, gamma).unwrap();
            let sq: f64 = picture
                .iter()
                .map(|&(r, s, a, b)| {
                    let y = (r as f64 + 0.5) / rows as f64 - 0.5;
                    let beta =
                        (f64::from(fit.weight_next) + f64::from(fit.slope) * y).clamp(0.0, 1.0);
                    let light = if a { 1.0 - beta } else { 0.0 } + if b { beta } else { 0.0 };
                    (f64::from(s) - light.powf(1.0 / gamma)).powi(2)
                })
                .sum();
            let rms = (sq / picture.len() as f64).sqrt();
            assert!(
                (f64::from(fit.rms) - rms).abs() < 1e-5,
                "γ {gamma}: {} vs {rms}",
                fit.rms
            );
            assert!((fit.weight_next - 0.4).abs() < 0.02, "γ {gamma}: {fit:?}");
        }
    }

    #[test]
    fn linear_fit_is_cell_by_cell_least_squares() {
        let rows = 21;
        let picture = picture(rows, 0.65, -0.2, 1.0);
        let fit = fit_at(&grouped(rows, &picture), 0, true, 1.0).unwrap();
        // The normal equations over the changed cells, one by one.
        let (mut s00, mut s01, mut s11, mut r0, mut r1) = (0.0f64, 0.0, 0.0, 0.0, 0.0);
        for &(r, s, a, b) in picture.iter().filter(|c| c.2 != c.3) {
            let y = (r as f64 + 0.5) / rows as f64 - 0.5;
            let d = if b { 1.0 } else { -1.0 };
            let res = f64::from(s) - if a { 1.0 } else { 0.0 };
            (s00, s01, s11) = (s00 + d * d, s01 + d * d * y, s11 + d * d * y * y);
            (r0, r1) = (r0 + d * res, r1 + d * y * res);
        }
        let det = s00 * s11 - s01 * s01;
        let (b0, b1) = ((r0 * s11 - r1 * s01) / det, (s00 * r1 - s01 * r0) / det);
        assert!(
            (f64::from(fit.weight_next) - b0).abs() < 1e-6,
            "{fit:?} vs {b0}"
        );
        assert!((f64::from(fit.slope) - b1).abs() < 1e-6, "{fit:?} vs {b1}");
    }

    #[test]
    fn no_cells_no_fit() {
        assert_eq!(fit_at(&Cells::new(3), 0, true, 1.0), None);
    }
}
