//! Reading QR codes caught between two frames.
//!
//! A camera whose exposure spans the display's switch from frame `M` to `M+1`
//! records a blend: every module that differs between the two frames' codes
//! comes out grey, and a code full of grey modules is more than error
//! correction can fix. But once one frame has been decoded, the payload's
//! format is known, so the code of every frame can be rendered exactly.
//! Fitting the sampled module brightness as a mix of two consecutive frames'
//! codes, `s ≈ (Q(M) + β·(Q(M+1) − Q(M)))^(1/γ)`, identifies the frames and
//! measures `β`, the share of the exposure that came after the switch. That
//! share is sub-frame timing that frame numbers alone can't give: a camera
//! locked to the pattern's rate sees the same frame numbers at every phase,
//! but not the same `β`. The frames mix in light, and `γ` allows for the
//! camera's tone curve (see `pattern::blend`).
//!
//! A rolling shutter exposes the code's rows at slightly different times, so
//! `β` is allowed to vary linearly from the code's top row to its bottom row.

use std::collections::HashMap;

use super::blend::{self, Cells};
use super::canvas::LumaView;
use super::qr::{QrGrid, QrStamp};
use super::{Blend, Layout, ToneCurve, barcode};
use crate::Result;

/// The black and white levels must differ by at least this much (out of 255).
const MIN_CONTRAST: f32 = 20.0;

/// A QR code's module brightness as sampled from a frame, before it's
/// normalised against a template: luma at each module's centre, row by row,
/// NaN for modules outside the frame.
#[derive(Clone, Debug, PartialEq)]
pub struct ModuleSamples {
    pub size: usize,
    pub luma: Vec<f32>,
}

impl ModuleSamples {
    /// Samples the code in `grid`: a small patch at each module's centre,
    /// mapped through the perspective the grid's corners imply.
    pub fn take(view: &LumaView<'_>, grid: &QrGrid) -> Option<Self> {
        let n = grid.modules;
        if n < 21 {
            return None;
        }
        let e = (n + 1) as f64;
        let corners = grid.corners.map(|(x, y)| (f64::from(x), f64::from(y)));
        let h = Homography::new([(0.0, 0.0), (e, 0.0), (e, e), (0.0, e)], corners)?;
        let module = f64::from(grid.side()) / e;
        let radius = ((module * 0.2) as usize).min(3);
        let (w, ht) = (view.width() as f64, view.height() as f64);
        let luma = (0..n * n)
            .map(|i| {
                let (x, y) = h.map((i % n) as f64 + 0.5, (i / n) as f64 + 0.5);
                if x < 0.0 || y < 0.0 || x > w - 1.0 || y > ht - 1.0 {
                    f32::NAN
                } else {
                    view.mean_patch(x, y, radius) as f32
                }
            })
            .collect();
        Some(ModuleSamples { size: n, luma })
    }
}

/// The codes of a file's frames, rendered on demand and cached.
#[derive(Clone, Debug)]
pub struct QrTemplate {
    stamp: QrStamp,
    size: usize,
    /// Per frame: 1.0 for a light module, 0.0 for a dark one, row by row.
    cache: HashMap<u32, Vec<f32>>,
    /// Modules that are the same in every frame (finder, timing and alignment
    /// patterns) and their colour.
    fixed: Vec<(usize, f32)>,
}

impl QrTemplate {
    /// The template for the file a decoded `stamp` came from.
    pub fn new(stamp: &QrStamp) -> Result<Self> {
        let (size, first) = render(stamp, stamp.frame_id)?;
        let fixed = fixed_modules(size);
        let mut cache = HashMap::new();
        cache.insert(stamp.frame_id, first);
        Ok(QrTemplate {
            stamp: *stamp,
            size,
            cache,
            fixed,
        })
    }

    /// Whether `stamp` comes from the same file (rate, tone plan and format).
    pub fn matches(&self, stamp: &QrStamp) -> bool {
        QrStamp {
            frame_id: 0,
            ..self.stamp
        } == QrStamp {
            frame_id: 0,
            ..*stamp
        }
    }

    /// The stamp the template renders, for frame `frame_id`.
    pub fn stamp(&self, frame_id: u32) -> QrStamp {
        QrStamp {
            frame_id,
            ..self.stamp
        }
    }

    /// Modules per side.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Frame `id`'s modules: 1.0 light, 0.0 dark.
    fn modules(&mut self, id: u32) -> &[f32] {
        if self.cache.len() > 512 {
            // Keep the neighbourhood of the frames being read.
            self.cache.retain(|&k, _| k.abs_diff(id) < 64);
        }
        let stamp = self.stamp;
        self.cache
            .entry(id)
            .or_insert_with(|| render(&stamp, id).map(|(_, m)| m).unwrap_or_default())
    }

    /// How test-frame pixels land in the picture, through the perspective of
    /// the code found at `grid`: the payload's frame size fixes where
    /// everything on the frame is relative to the code.
    pub fn frame_to_picture(&self, grid: &QrGrid) -> Option<FrameToPicture> {
        let (w, h) = self.stamp.frame_size;
        let layout = Layout::new(w as usize, h as usize).ok()?;
        let ((ox, oy), module, _) = super::qr::placement(layout.qr, self.size)?;
        let e = (self.size + 1) as f64;
        let corners = grid.corners.map(|(x, y)| (f64::from(x), f64::from(y)));
        let code = Homography::new([(0.0, 0.0), (e, 0.0), (e, e), (0.0, e)], corners)?;
        let module = f64::from(module);
        Some(FrameToPicture {
            code,
            origin: (f64::from(ox), f64::from(oy)),
            module,
            scale: f64::from(grid.side()) / e / module,
        })
    }

    /// Reads the barcode strip through the code found at `grid`, as a blend
    /// of two consecutive frames' words (the earlier one in `candidates`),
    /// seen through the tone curve `tone` (see [`barcode::fit_blend`]).
    /// Its cells are several times larger than the code's modules, so it can
    /// read when the code is too small or blurred to fit.
    pub fn fit_barcode(
        &self,
        view: &LumaView<'_>,
        grid: &QrGrid,
        candidates: std::ops::RangeInclusive<u32>,
        tone: &mut ToneCurve,
    ) -> Option<Blend> {
        let map = self.frame_to_picture(grid)?;
        let (w, h) = self.stamp.frame_size;
        let size = (f64::from(w), f64::from(h));
        let cells = barcode::sample_mapped(view, size, map.scale, |x, y| Some(map.map(x, y)))?;
        barcode::fit_blend(&cells, Some(self.stamp.fps), candidates, tone)
    }

    /// Fits the code in `grid` as a blend of two consecutive frames, the
    /// earlier one in `candidates`, seen through a tone curve: the code's
    /// own where it shows it, else the capture's so far from `tone`, which
    /// learns from the fit.
    pub fn fit(
        &mut self,
        view: &LumaView<'_>,
        grid: &QrGrid,
        candidates: std::ops::RangeInclusive<u32>,
        tone: &mut ToneCurve,
    ) -> Option<Blend> {
        let raw = ModuleSamples::take(view, grid)?;
        self.fit_samples(&raw, candidates, tone)
    }

    /// [`fit`](Self::fit) for module brightness sampled earlier.
    pub fn fit_samples(
        &mut self,
        raw: &ModuleSamples,
        candidates: std::ops::RangeInclusive<u32>,
        tone: &mut ToneCurve,
    ) -> Option<Blend> {
        if raw.size != self.size {
            return None;
        }
        let samples = self.normalise(raw)?;
        let n = self.size;
        // The modules that can change between frames, row by row; the fixed
        // ones calibrated black and white.
        let cells = |m: u32| -> Option<Cells> {
            let next = m.checked_add(1)?;
            let qa = self.modules(m).to_vec();
            let qb = self.modules(next);
            if qa.len() != n * n || qb.len() != n * n {
                return None;
            }
            let mut cells = Cells::new(n);
            for i in (0..n * n).filter(|&i| !is_fixed(i % n, i / n, n) && samples[i].is_finite()) {
                cells.add(i / n, samples[i], qa[i] > 0.5, qb[i] > 0.5);
            }
            Some(cells)
        };
        blend::fit_candidates(candidates, cells, true, tone)
    }

    /// Module brightness normalised so that black is 0 and white is 1, using
    /// the modules that never change, with the levels fitted as planes across
    /// the code (glare and vignetting vary across it).
    fn normalise(&self, samples: &ModuleSamples) -> Option<Vec<f32>> {
        normalise(samples, &self.fixed)
    }
}

/// Module brightness normalised so that black is 0 and white is 1, from the
/// `fixed` modules' levels fitted as planes across the code.
fn normalise(samples: &ModuleSamples, fixed: &[(usize, f32)]) -> Option<Vec<f32>> {
    {
        let n = samples.size;
        let raw = &samples.luma;
        let level = |colour: f32| {
            let points: Vec<(f64, f64, f64)> = fixed
                .iter()
                .filter(|&&(i, c)| c == colour && raw[i].is_finite())
                .map(|&(i, _)| {
                    let (x, y) = ((i % n) as f64 / n as f64, (i / n) as f64 / n as f64);
                    (x, y, f64::from(raw[i]))
                })
                .collect();
            Plane::fit(&points)
        };
        let (black, white) = (level(0.0)?, level(1.0)?);
        let mut contrast_ok = true;
        let samples = raw
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let (x, y) = ((i % n) as f64 / n as f64, (i / n) as f64 / n as f64);
                let (b, wh) = (black.at(x, y), white.at(x, y));
                if wh - b < f64::from(MIN_CONTRAST) {
                    contrast_ok = false;
                }
                ((f64::from(v) - b) / (wh - b)) as f32
            })
            .collect();
        contrast_ok.then_some(samples)
    }
}

/// Decodes a code from its sampled modules, thresholded between the levels
/// of the modules that never change: midway in light, through the tone curve
/// `gamma` (1 if it isn't known). A picture caught between two frames has
/// four levels (black, white, and the two mixes), and the threshold sorts the
/// mixes by whichever frame covered more of the exposure: that frame's code,
/// exactly. A detector that thresholds pixels against their surroundings, as
/// most QR readers do, mangles them instead.
pub fn decode_samples(samples: &ModuleSamples, gamma: f32) -> Option<QrStamp> {
    let n = samples.size;
    let s = normalise(samples, &fixed_modules(n))?;
    // Midway first; then a little either side, for uneven light or a mix near
    // half; then midway for typical cameras' curves, for when `gamma` isn't
    // known yet (a camera's mixes near half both read about 0.7).
    let mid = 0.5f32.powf(1.0 / gamma.max(0.1));
    [
        mid,
        mid - 0.08,
        mid + 0.08,
        mid - 0.15,
        mid + 0.15,
        0.73,
        0.68,
        0.78,
    ]
    .into_iter()
    .find_map(|threshold| {
        let grid = rqrr::SimpleGrid::from_func(n, |x, y| s[y * n + x] < threshold);
        let (_, text) = rqrr::Grid::new(grid).decode().ok()?;
        super::qr::parse_payload(&text)
    })
}

/// The modules that are the same in every code of `n` modules per side, and
/// their colour (1.0 light, 0.0 dark): finder patterns with their
/// separators, timing patterns and (versions 2 to 6) the alignment pattern.
fn fixed_modules(n: usize) -> Vec<(usize, f32)> {
    let ring = |x: usize, y: usize, cx: usize, cy: usize| x.abs_diff(cx).max(y.abs_diff(cy));
    (0..n * n)
        .filter(|&i| is_fixed(i % n, i / n, n))
        .map(|i| {
            let (x, y) = (i % n, i / n);
            let finder = [(3, 3), (n - 4, 3), (3, n - 4)]
                .into_iter()
                .map(|(cx, cy)| ring(x, y, cx, cy))
                .find(|&r| r <= 4);
            let dark = match finder {
                // Dark centre and outer ring; light between, and the separator.
                Some(r) => r <= 1 || r == 3,
                None if n > 21 && ring(x, y, n - 7, n - 7) <= 2 => ring(x, y, n - 7, n - 7) != 1,
                // Timing patterns: dark on even positions.
                None => (if x == 6 { y } else { x }) % 2 == 0,
            };
            (i, if dark { 0.0 } else { 1.0 })
        })
        .collect()
}

/// The code for frame `id` of `template`'s file: its size and modules (1.0 light).
fn render(template: &QrStamp, id: u32) -> Result<(usize, Vec<f32>)> {
    let stamp = QrStamp {
        frame_id: id,
        ..*template
    };
    let (size, dark) = super::qr::modules(&super::qr::stamp_payload(&stamp)?)?;
    Ok((
        size,
        dark.into_iter()
            .map(|d| if d { 0.0 } else { 1.0 })
            .collect(),
    ))
}

/// Finder patterns with their separators, timing patterns, and (from
/// version 2) the bottom-right alignment pattern: the same in every frame.
fn is_fixed(x: usize, y: usize, n: usize) -> bool {
    let finder = |x: usize, y: usize| x < 8 && y < 8;
    let alignment = n > 21 && (n - 9..=n - 5).contains(&x) && (n - 9..=n - 5).contains(&y);
    finder(x, y)
        || finder(n - 1 - x, y)
        || finder(x, n - 1 - y)
        || (x == 6 && (8..n - 8).contains(&y))
        || (y == 6 && (8..n - 8).contains(&x))
        || alignment
}

/// How test-frame pixels land in a picture of it (see
/// [`QrTemplate::frame_to_picture`]).
#[derive(Clone, Copy, Debug)]
pub struct FrameToPicture {
    /// From the code's module grid to picture pixels.
    code: Homography,
    /// The test-frame pixel of the code's top-left corner.
    origin: (f64, f64),
    /// The code's module size in test-frame pixels.
    module: f64,
    /// Roughly how many picture pixels a test-frame pixel covers.
    pub scale: f64,
}

impl FrameToPicture {
    /// The picture pixel showing test-frame pixel `(x, y)`.
    pub fn map(&self, x: f64, y: f64) -> (f64, f64) {
        self.code.map(
            (x - self.origin.0) / self.module,
            (y - self.origin.1) / self.module,
        )
    }
}

/// A projective map from grid coordinates to frame pixels.
#[derive(Clone, Copy, Debug)]
struct Homography([f64; 8]);

impl Homography {
    /// The map taking each of `from` to the matching point of `to`.
    fn new(from: [(f64, f64); 4], to: [(f64, f64); 4]) -> Option<Self> {
        let mut a = [[0.0f64; 9]; 8];
        for i in 0..4 {
            let ((u, v), (x, y)) = (from[i], to[i]);
            a[2 * i] = [u, v, 1.0, 0.0, 0.0, 0.0, -u * x, -v * x, x];
            a[2 * i + 1] = [0.0, 0.0, 0.0, u, v, 1.0, -u * y, -v * y, y];
        }
        for c in 0..8 {
            let p = (c..8).max_by(|&i, &j| a[i][c].abs().total_cmp(&a[j][c].abs()))?;
            if a[p][c].abs() < 1e-12 {
                return None;
            }
            a.swap(c, p);
            let pivot = a[c];
            for (r, row) in a.iter_mut().enumerate() {
                if r != c {
                    let f = row[c] / pivot[c];
                    for (x, p) in row.iter_mut().zip(pivot).skip(c) {
                        *x -= f * p;
                    }
                }
            }
        }
        Some(Homography(std::array::from_fn(|i| a[i][8] / a[i][i])))
    }

    fn map(&self, u: f64, v: f64) -> (f64, f64) {
        let h = &self.0;
        let d = h[6] * u + h[7] * v + 1.0;
        (
            (h[0] * u + h[1] * v + h[2]) / d,
            (h[3] * u + h[4] * v + h[5]) / d,
        )
    }
}

/// `level ≈ a + b·x + c·y` across the code.
#[derive(Clone, Copy, Debug)]
struct Plane {
    a: f64,
    b: f64,
    c: f64,
}

impl Plane {
    /// Least squares through `(x, y, level)`; a flat level if there are too
    /// few points spread too little to fit a slope.
    fn fit(points: &[(f64, f64, f64)]) -> Option<Plane> {
        if points.is_empty() {
            return None;
        }
        let n = points.len() as f64;
        let mean = |f: &dyn Fn(&(f64, f64, f64)) -> f64| points.iter().map(f).sum::<f64>() / n;
        let (mx, my, mz) = (mean(&|p| p.0), mean(&|p| p.1), mean(&|p| p.2));
        let (mut sxx, mut sxy, mut syy, mut sxz, mut syz) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for &(x, y, z) in points {
            let (dx, dy, dz) = (x - mx, y - my, z - mz);
            sxx += dx * dx;
            sxy += dx * dy;
            syy += dy * dy;
            sxz += dx * dz;
            syz += dy * dz;
        }
        let det = sxx * syy - sxy * sxy;
        if points.len() < 6 || det.abs() < 1e-9 {
            return Some(Plane {
                a: mz,
                b: 0.0,
                c: 0.0,
            });
        }
        let b = (sxz * syy - syz * sxy) / det;
        let c = (syz * sxx - sxz * sxy) / det;
        Some(Plane {
            a: mz - b * mx - c * my,
            b,
            c,
        })
    }

    fn at(&self, x: f64, y: f64) -> f64 {
        self.a + self.b * x + self.c * y
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FrameRate;
    use crate::audio::ToneCode;
    use crate::pattern::PatternRenderer;
    use crate::pattern::canvas::LumaImage;
    use crate::pattern::qr;

    fn frame(renderer: &PatternRenderer, id: u32) -> LumaImage {
        let l = renderer.layout();
        let mut img = LumaImage::new(l.width, l.height, 0);
        renderer.render(id, &mut img.canvas()).unwrap();
        img
    }

    /// `a·(1 − w) + b·w`, pixel by pixel.
    fn mix(a: &LumaImage, b: &LumaImage, w: f32) -> LumaImage {
        let mut out = a.clone();
        for (o, (&x, &y)) in out.data.iter_mut().zip(a.data.iter().zip(&b.data)) {
            *o = (f32::from(x) * (1.0 - w) + f32::from(y) * w).round() as u8;
        }
        out
    }

    #[test]
    fn reads_blends_of_two_frames() {
        let fps: FrameRate = "30".parse().unwrap();
        let renderer = PatternRenderer::new(1280, 720, ToneCode::new(fps, 48000).unwrap()).unwrap();
        let clean = frame(&renderer, 500);
        let found = qr::locate(&clean.view(), None).unwrap();
        let mut template = QrTemplate::new(&found.stamp).unwrap();
        let grid = found.grid();
        let mut tone = ToneCurve::default();
        // An unmixed frame: either neighbouring pair, all the weight on 500.
        let fit = template
            .fit(&clean.view(), &grid, 495..=505, &mut tone)
            .unwrap();
        assert_eq!(fit.dominant(), 500);
        assert!(!fit.is_mixed(), "{fit:?}");
        for w in [0.2f32, 0.5, 0.7] {
            let img = mix(&frame(&renderer, 700), &frame(&renderer, 701), w);
            let fit = template
                .fit(&img.view(), &grid, 695..=705, &mut tone)
                .unwrap();
            assert_eq!(fit.first, 700, "w {w}: {fit:?}");
            assert!((fit.weight_next - w).abs() < 0.02, "w {w}: {fit:?}");
            assert!(fit.slope.abs() < 0.02, "w {w}: {fit:?}");
        }
        // Mixed in proportion to light, as a digital copy mixes.
        assert!((tone.gamma() - 1.0).abs() < 0.05, "gamma {}", tone.gamma());
        // A rolling shutter: the bottom of the code is later than the top.
        let a = frame(&renderer, 900);
        let b = frame(&renderer, 901);
        let mut img = a.clone();
        let b_box = grid.bounds();
        for y in 0..720 {
            let t = ((y as f32 - b_box.y0 as f32) / b_box.height() as f32).clamp(0.0, 1.0);
            let w = 0.3 + 0.4 * t;
            for x in 0..1280 {
                let i = y * 1280 + x;
                img.data[i] =
                    (f32::from(a.data[i]) * (1.0 - w) + f32::from(b.data[i]) * w).round() as u8;
            }
        }
        let fit = template
            .fit(&img.view(), &grid, 895..=905, &mut tone)
            .unwrap();
        assert_eq!(fit.first, 900);
        assert!((fit.weight_next - 0.5).abs() < 0.03, "{fit:?}");
        assert!((fit.slope - 0.4).abs() < 0.06, "{fit:?}");
    }

    #[test]
    fn reads_blends_through_a_camera_tone_curve() {
        use crate::pattern::blend::curves::{Curve, encode, gamma22, light, mix_through, srgb};
        let fps: FrameRate = "30".parse().unwrap();
        let renderer = PatternRenderer::new(1280, 720, ToneCode::new(fps, 48000).unwrap()).unwrap();
        let found = qr::locate(&frame(&renderer, 500).view(), None).unwrap();
        let mut template = QrTemplate::new(&found.stamp).unwrap();
        let grid = found.grid();
        let curves: [(&str, Curve); 2] = [("sRGB", srgb), ("gamma 2.2", gamma22)];
        let (a, b) = (frame(&renderer, 700), frame(&renderer, 701));
        for (name, curve) in curves {
            // Mixes near either frame don't show the curve: they're read with
            // what the mixes nearer half measured.
            let mut tone = ToneCurve::default();
            for w in [0.3, 0.5, 0.7, 0.1, 0.9] {
                let img = mix_through(&a, &b, w, curve);
                let fit = template
                    .fit(&img.view(), &grid, 695..=705, &mut tone)
                    .unwrap();
                assert_eq!(fit.first, 700, "{name} w {w}: {fit:?}");
                assert!(
                    (f64::from(fit.weight_next) - w).abs() < 0.02,
                    "{name} w {w}: {fit:?}"
                );
                assert!(fit.slope.abs() < 0.03, "{name} w {w}: {fit:?}");
                assert!(fit.rms < 0.03, "{name} w {w}: {fit:?}");
            }
            let gamma = tone.gamma();
            assert!((1.9..2.6).contains(&gamma), "{name}: gamma {gamma}");
        }
        // A rolling shutter through sRGB: the bottom of the code is later.
        let (a, b) = (frame(&renderer, 900), frame(&renderer, 901));
        let mut img = a.clone();
        let b_box = grid.bounds();
        for y in 0..720 {
            let t = ((y as f64 - b_box.y0 as f64) / b_box.height() as f64).clamp(0.0, 1.0);
            let w = 0.3 + 0.4 * t;
            for x in 0..1280 {
                let i = y * 1280 + x;
                let mixed =
                    light(f64::from(a.data[i])) * (1.0 - w) + light(f64::from(b.data[i])) * w;
                img.data[i] = encode(mixed, srgb);
            }
        }
        let mut tone = ToneCurve::default();
        let fit = template
            .fit(&img.view(), &grid, 895..=905, &mut tone)
            .unwrap();
        assert_eq!(fit.first, 900);
        assert!((fit.weight_next - 0.5).abs() < 0.03, "{fit:?}");
        assert!((fit.slope - 0.4).abs() < 0.06, "{fit:?}");
    }

    #[test]
    fn rejects_frames_out_of_range_and_other_pictures() {
        let fps: FrameRate = "25".parse().unwrap();
        let renderer = PatternRenderer::new(1280, 720, ToneCode::new(fps, 48000).unwrap()).unwrap();
        let img = frame(&renderer, 300);
        let found = qr::locate(&img.view(), None).unwrap();
        let mut template = QrTemplate::new(&found.stamp).unwrap();
        let mut tone = ToneCurve::default();
        // The true frame isn't a candidate.
        assert_eq!(
            template.fit(&img.view(), &found.grid(), 100..=110, &mut tone),
            None
        );
        // A blank picture where the code was.
        let blank = LumaImage::new(1280, 720, 128);
        assert_eq!(
            template.fit(&blank.view(), &found.grid(), 295..=305, &mut tone),
            None
        );
    }

    #[test]
    fn reads_the_barcode_through_the_code() {
        // A 1280×720 test frame shown at half size, 100 px in from the left
        // of a 1280×720 picture, mixing frames 300 and 301.
        let fps: FrameRate = "30".parse().unwrap();
        let renderer = PatternRenderer::new(1280, 720, ToneCode::new(fps, 48000).unwrap()).unwrap();
        let shown = |a: &LumaImage| {
            let small = qr::downscale(&a.view(), 2);
            let mut picture = LumaImage::new(1280, 720, 0);
            for y in 0..small.height {
                for x in 0..small.width {
                    picture.data[(y + 80) * 1280 + x + 100] = small.data[y * small.width + x];
                }
            }
            picture
        };
        let picture = shown(&mix(&frame(&renderer, 300), &frame(&renderer, 301), 0.35));
        let found = qr::locate(&shown(&frame(&renderer, 300)).view(), None).unwrap();
        assert_eq!(found.stamp.frame_size, (1280, 720));
        let template = QrTemplate::new(&found.stamp).unwrap();
        // The barcode's corner lands where the half-size frame put it, to well
        // within a cell (33 pixels here): it's extrapolated from corners found
        // to about a pixel, three code widths away.
        let map = template.frame_to_picture(&found.grid()).unwrap();
        let (x, y) = map.map(38.4, 28.8);
        assert!(
            (x - 119.2).abs() < 8.0 && (y - 94.4).abs() < 8.0,
            "{x}, {y}"
        );
        assert!((map.scale - 0.5).abs() < 0.02, "{}", map.scale);
        let fit = template
            .fit_barcode(
                &picture.view(),
                &found.grid(),
                295..=305,
                &mut ToneCurve::default(),
            )
            .unwrap();
        assert_eq!(fit.first, 300, "{fit:?}");
        assert!((fit.weight_next - 0.35).abs() < 0.03, "{fit:?}");
    }

    #[test]
    fn fixed_modules_match_real_codes() {
        // A built-in rate (version 5), and custom rates written out until one
        // takes version 6.
        let mut sizes = Vec::new();
        let rates = ["30", "24001/1001", "240001/10001", "2400001/100001"];
        for rate in rates {
            let fps: FrameRate = rate.parse().unwrap();
            let stamp = QrStamp {
                frame_id: 1234,
                fps,
                tone_base_harmonic: ToneCode::default_base_harmonic(fps),
                frame_size: (4096, 2160),
            };
            let (n, code) = render(&stamp, 1234).unwrap();
            for (i, colour) in fixed_modules(n) {
                assert_eq!(colour, code[i], "module {} {} of {n}", i % n, i / n);
            }
            sizes.push(n);
        }
        assert!(sizes.contains(&37) && sizes.contains(&41), "{sizes:?}");
    }

    #[test]
    fn decodes_blends_from_samples() {
        let fps: FrameRate = "30".parse().unwrap();
        let renderer = PatternRenderer::new(1280, 720, ToneCode::new(fps, 48000).unwrap()).unwrap();
        let grid = qr::locate(&frame(&renderer, 0).view(), None)
            .unwrap()
            .grid();
        for (w, expected) in [(0.3f32, 600), (0.65, 601)] {
            let img = mix(&frame(&renderer, 600), &frame(&renderer, 601), w);
            // Most QR readers give up on a picture this blended.
            let samples = ModuleSamples::take(&img.view(), &grid).unwrap();
            let stamp = decode_samples(&samples, 1.0).unwrap();
            assert_eq!(stamp.frame_id, expected, "w {w}");
        }
        // Through a camera's curve, both mixes near half read brighter than half.
        use crate::pattern::blend::curves::{mix_through, srgb};
        for (w, expected) in [(0.45, 600), (0.55, 601), (0.3, 600)] {
            let img = mix_through(&frame(&renderer, 600), &frame(&renderer, 601), w, srgb);
            let samples = ModuleSamples::take(&img.view(), &grid).unwrap();
            for gamma in [1.0, 2.2] {
                let stamp = decode_samples(&samples, gamma).unwrap();
                assert_eq!(stamp.frame_id, expected, "w {w} gamma {gamma}");
            }
        }
    }

    #[test]
    fn homography_maps_corners() {
        let h = Homography::new(
            [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)],
            [(10.0, 20.0), (110.0, 25.0), (105.0, 130.0), (5.0, 120.0)],
        )
        .unwrap();
        let (x, y) = h.map(1.0, 1.0);
        assert!((x - 105.0).abs() < 1e-6 && (y - 130.0).abs() < 1e-6);
        let (x, y) = h.map(0.0, 0.0);
        assert!((x - 10.0).abs() < 1e-6 && (y - 20.0).abs() < 1e-6);
    }
}
