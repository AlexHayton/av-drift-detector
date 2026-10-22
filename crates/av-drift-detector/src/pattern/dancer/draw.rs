//! Drawing the dancer, his props and the stage, in stage units.

use std::f64::consts::{FRAC_PI_2, PI, TAU};

use super::View;
use super::choreo::{BEATS_PER_SECOND, HEAD_R, JUMP, LOOP_BEATS, STAND, Skeleton};
use super::geom::{V2, arc, lerp, random, sd_box, sd_ellipse, segment, smooth, v2};
use super::physics::{SCARF_POINTS, drifting, spring_impulse};
use crate::pattern::canvas::{BLACK, LumaCanvas, Rect, WHITE};
use crate::pattern::paint::Painter;

const WHITE_F: f64 = WHITE as f64;
const DARK: f64 = 22.0;
/// Limb thickness.
const LIMB: f64 = 0.026;

/// Paints shapes given in stage units.
pub(super) struct Brush<'c, 'a> {
    paint: Painter<'c, 'a>,
    view: View,
}

impl<'c, 'a> Brush<'c, 'a> {
    pub fn new(canvas: &'c mut LumaCanvas<'a>, clip: Rect, view: View) -> Self {
        Brush {
            paint: Painter::new(canvas, clip),
            view,
        }
    }

    /// Half the thinnest line worth drawing: a little over a pixel wide.
    fn min_half(&self) -> f64 {
        0.6 / self.view.unit
    }

    /// Fills the shape whose signed distance (in units, negative inside)
    /// `sdf` gives, which lies within the box `lo`–`hi`.
    fn shape(&mut self, lo: V2, hi: V2, luma: f64, alpha: f64, sdf: impl Fn(V2) -> f64) {
        let View { unit, x0, floor } = self.view;
        let bounds = [
            x0 + lo.x * unit,
            floor - hi.y * unit,
            x0 + hi.x * unit,
            floor - lo.y * unit,
        ];
        self.paint.fill(bounds, luma, alpha, |px, py| {
            sdf(v2((px - x0) / unit, (floor - py) / unit)) * unit
        });
    }

    /// Round-capped strokes `(from, to, thickness)`, as one shape.
    fn strokes(&mut self, strokes: &[(V2, V2, f64)], luma: f64, alpha: f64) {
        let min_half = self.min_half();
        let caps: Vec<(V2, V2, f64)> = strokes
            .iter()
            .map(|&(a, b, w)| (a, b, (w / 2.0).max(min_half)))
            .collect();
        let pad = caps.iter().map(|c| c.2).fold(0.0, f64::max);
        let lo = caps
            .iter()
            .fold(v2(f64::MAX, f64::MAX), |m, c| m.min(c.0).min(c.1));
        let hi = caps
            .iter()
            .fold(v2(f64::MIN, f64::MIN), |m, c| m.max(c.0).max(c.1));
        self.shape(lo - v2(pad, pad), hi + v2(pad, pad), luma, alpha, |p| {
            caps.iter()
                .map(|&(a, b, r)| segment(p, a, b) - r)
                .fold(f64::MAX, f64::min)
        });
    }

    fn line(&mut self, a: V2, b: V2, thickness: f64, luma: f64, alpha: f64) {
        self.strokes(&[(a, b, thickness)], luma, alpha);
    }

    fn disc(&mut self, c: V2, r: f64, luma: f64, alpha: f64) {
        let r = r.max(self.min_half());
        self.shape(c - v2(r, r), c + v2(r, r), luma, alpha, |p| {
            (p - c).len() - r
        });
    }

    fn ring(&mut self, c: V2, r: f64, thickness: f64, luma: f64) {
        let h = (thickness / 2.0).max(self.min_half());
        let reach = v2(r + h, r + h);
        self.shape(c - reach, c + reach, luma, 1.0, |p| {
            ((p - c).len() - r).abs() - h
        });
    }

    /// A box centred on `c` with half-size `half`, turned `angle`, corners rounded by `round`.
    fn rect(&mut self, c: V2, half: V2, angle: f64, round: f64, luma: f64) {
        let reach = half.len();
        let inner = half - v2(round, round);
        self.shape(c - v2(reach, reach), c + v2(reach, reach), luma, 1.0, |p| {
            sd_box((p - c).rot(-angle), inner) - round
        });
    }

    fn rect_outline(&mut self, c: V2, half: V2, round: f64, thickness: f64, luma: f64) {
        let h = (thickness / 2.0).max(self.min_half());
        let reach = half + v2(h, h);
        let inner = half - v2(round, round);
        self.shape(c - reach, c + reach, luma, 1.0, |p| {
            (sd_box(p - c, inner) - round).abs() - h
        });
    }

    fn ellipse(&mut self, c: V2, radii: V2, angle: f64, luma: f64, alpha: f64) {
        let reach = radii.x.max(radii.y);
        let radii = radii.max(v2(self.min_half(), self.min_half()));
        self.shape(
            c - v2(reach, reach),
            c + v2(reach, reach),
            luma,
            alpha,
            |p| sd_ellipse((p - c).rot(-angle), radii),
        );
    }

    /// The arc of radius `r` around `c`, counter-clockwise from angle `a0` to `a1`.
    fn arc(&mut self, c: V2, r: f64, (a0, a1): (f64, f64), thickness: f64, luma: f64) {
        let h = (thickness / 2.0).max(self.min_half());
        let reach = v2(r + h, r + h);
        self.shape(c - reach, c + reach, luma, 1.0, |p| {
            arc(p, c, r, a0, a1) - h
        });
    }

    /// Fills the box `lo`–`hi` with hard pixel edges.
    fn block(&mut self, lo: V2, hi: V2, luma: f64) {
        let View { unit, x0, floor } = self.view;
        let rect = Rect {
            x0: (x0 + lo.x * unit).round() as i32,
            y0: (floor - hi.y * unit).round() as i32,
            x1: (x0 + hi.x * unit).round() as i32,
            y1: (floor - lo.y * unit).round() as i32,
        };
        self.paint.rect(rect, luma.round().clamp(0.0, 255.0) as u8);
    }
}

/// Seconds since the last beat.
fn since_beat(beat: f64) -> f64 {
    beat.rem_euclid(1.0) / BEATS_PER_SECOND
}

/// The light-up dance floor: tiles that flash on the beat, and glow under a planted foot.
pub(super) fn floor(brush: &mut Brush, width: f64, beat: f64, s: &Skeleton) {
    const TILE: f64 = 0.3;
    const GAP: f64 = 0.012;
    let bar_beat = beat.floor() as u64;
    let flash = (-6.0 * since_beat(beat)).exp();
    for i in 0..(width / TILE).ceil() as u64 {
        let x = i as f64 * TILE;
        let mut luma = 52.0;
        if random(bar_beat, i) < 0.35 {
            luma += 40.0 + 80.0 * flash;
        }
        for foot in s.ankles {
            if foot.y < 0.02 && (x..x + TILE).contains(&foot.x) {
                luma += 50.0;
            }
        }
        brush.block(
            v2(x + GAP, -0.065),
            v2(x + TILE - GAP, -0.012),
            luma.min(210.0),
        );
    }
}

pub(super) fn shadow(brush: &mut Brush, s: &Skeleton) {
    let lift = (s.pelvis.y - STAND).max(0.0);
    let rx = 0.2 - 0.4 * lift;
    brush.ellipse(
        v2(s.pelvis.x, -0.012),
        v2(rx, 0.02),
        0.0,
        f64::from(BLACK),
        0.75,
    );
}

/// A boombox whose speakers kick on every beat, its antenna wobbling.
pub(super) fn boombox(brush: &mut Brush, x: f64, beat: f64) {
    let since = since_beat(beat);
    let kick = (-9.0 * since).exp();
    let trim = 200.0;
    let handle = 0.016;
    brush.strokes(
        &[
            (v2(x - 0.12, 0.23), v2(x - 0.12, 0.3), handle),
            (v2(x - 0.12, 0.3), v2(x + 0.12, 0.3), handle),
            (v2(x + 0.12, 0.3), v2(x + 0.12, 0.23), handle),
        ],
        trim,
        1.0,
    );
    let base = v2(x + 0.16, 0.24);
    let sway = -0.35 + spring_impulse(since, 2.5, 16.0, 0.15);
    let tip = base + V2::polar(FRAC_PI_2 + sway) * 0.22;
    brush.line(base, tip, 0.009, trim, 1.0);
    brush.disc(tip, 0.012, trim, 1.0);

    let body = v2(x, 0.125);
    let half = v2(0.2, 0.115);
    brush.rect(body, half, 0.0, 0.025, 62.0);
    brush.rect_outline(body, half, 0.025, 0.012, trim);
    for side in [-1.0, 1.0] {
        let c = v2(x + side * 0.105, 0.115);
        brush.ring(c, 0.072, 0.012, trim);
        brush.disc(c, 0.05 * (1.0 + 0.22 * kick), 110.0 + 70.0 * kick, 1.0);
        brush.disc(c, 0.017, WHITE_F, 1.0);
    }
    let deck = v2(x, 0.19);
    brush.rect(deck, v2(0.028, 0.022), 0.0, 0.004, 24.0);
    brush.rect_outline(deck, v2(0.028, 0.022), 0.004, 0.007, 150.0);
}

/// Music notes floating up from the speakers, one per beat: buoyant against
/// drag, wobbling as they rise, fading out.
pub(super) fn notes(brush: &mut Brush, x: f64, beat: f64, ceiling: f64) {
    const LIFE: f64 = 2.4;
    for k in 0..LOOP_BEATS as u64 {
        let age = (beat - k as f64).rem_euclid(LOOP_BEATS) / BEATS_PER_SECOND;
        if age >= LIFE {
            continue;
        }
        let side = if k % 2 == 0 { -1.0 } else { 1.0 };
        let start = v2(x + side * 0.105, 0.2);
        let v0 = v2(lerp(-0.3, 0.05, random(1, k)), lerp(0.3, 0.5, random(2, k)));
        let mut at = drifting(start, v0, v2(0.0, 0.45), 1.4, age);
        let phase = TAU * random(3, k);
        at.x += 0.03 * (TAU * 1.3 * age + phase).sin();
        let tilt = 0.3 * (TAU * 0.9 * age + phase).sin();
        let fade = smooth(age / 0.15)
            * (1.0 - smooth((age - 0.55 * LIFE) / (0.45 * LIFE)))
            * smooth((ceiling - 0.1 - at.y) / 0.15);
        if fade > 0.0 {
            note(brush, at, tilt, k % 3, fade);
        }
    }
}

/// A quaver, a pair of beamed quavers or a crotchet, its (first) head at `at`.
fn note(brush: &mut Brush, at: V2, tilt: f64, kind: u64, alpha: f64) {
    let to = |p: V2| at + p.rot(tilt);
    let stem = 0.011;
    let head = |brush: &mut Brush, c: V2| {
        brush.ellipse(to(c), v2(0.026, 0.019), tilt + 0.4, WHITE_F, alpha)
    };
    let x = 0.022;
    match kind {
        0 => {
            head(brush, v2(0.0, 0.0));
            brush.strokes(
                &[
                    (to(v2(x, 0.004)), to(v2(x, 0.095)), stem),
                    (to(v2(x, 0.095)), to(v2(x + 0.03, 0.065)), stem),
                    (to(v2(x + 0.03, 0.065)), to(v2(x + 0.026, 0.035)), stem),
                ],
                WHITE_F,
                alpha,
            );
        }
        1 => {
            head(brush, v2(0.0, 0.0));
            head(brush, v2(0.07, 0.015));
            brush.strokes(
                &[
                    (to(v2(x, 0.004)), to(v2(x, 0.095)), stem),
                    (to(v2(x + 0.07, 0.019)), to(v2(x + 0.07, 0.11)), stem),
                    (to(v2(x, 0.09)), to(v2(x + 0.07, 0.105)), 0.022),
                ],
                WHITE_F,
                alpha,
            );
        }
        _ => {
            head(brush, v2(0.0, 0.0));
            brush.line(to(v2(x, 0.004)), to(v2(x, 0.095)), stem, WHITE_F, alpha);
        }
    }
}

/// Dust kicked up by the landing: puffs that fly out, slow, settle and fade.
pub(super) fn dust(brush: &mut Brush, home: f64, beat: f64) {
    const LIFE: f64 = 0.7;
    let age = (beat - JUMP.1).rem_euclid(LOOP_BEATS) / BEATS_PER_SECOND;
    if age >= LIFE {
        return;
    }
    let fade = (1.0 - age / LIFE).powi(2);
    for (n, side) in [-1.0, 1.0].into_iter().enumerate() {
        let foot = v2(home + side * 0.17, 0.01);
        for i in 0..8 {
            let seed = 8 * n as u64 + i;
            let angle = lerp(0.15, 1.2, random(10, seed));
            let speed = lerp(0.3, 0.9, random(11, seed));
            let v0 = v2(side * angle.cos(), angle.sin()) * speed;
            let at = drifting(foot, v0, v2(0.0, -0.5), 5.0, age);
            let r = 0.012 + 0.07 * age * lerp(0.6, 1.2, random(12, seed));
            brush.disc(at, r, 150.0, 0.6 * fade);
        }
    }
}

/// The scarf's tail, in stripes, thinning to the end.
pub(super) fn scarf_tail(brush: &mut Brush, points: &[V2; SCARF_POINTS]) {
    for (i, link) in points.windows(2).enumerate() {
        let taper = 1.0 - 0.45 * i as f64 / (SCARF_POINTS - 1) as f64;
        let luma = if i % 2 == 0 { WHITE_F } else { 120.0 };
        brush.line(link[0], link[1], 0.03 * taper, luma, 1.0);
    }
}

/// The scarf wrapped round the neck, over the body.
pub(super) fn scarf_wrap(brush: &mut Brush, s: &Skeleton) {
    let right = s.up.rot(-FRAC_PI_2);
    let at = s.neck - s.up * 0.012;
    brush.line(at - right * 0.03, at + right * 0.03, 0.028, 120.0, 1.0);
}

pub(super) fn body(brush: &mut Brush, s: &Skeleton) {
    let mut strokes = vec![(s.pelvis, s.neck, LIMB), (s.hips[0], s.hips[1], LIMB)];
    for i in 0..2 {
        strokes.extend([
            (s.hips[i], s.knees[i], LIMB),
            (s.knees[i], s.ankles[i], LIMB),
            (s.ankles[i], s.toes[i], 0.036),
            (s.shoulder, s.elbows[i], LIMB),
            (s.elbows[i], s.hands[i], LIMB),
            (s.hands[i], s.hands[i], 0.05),
        ]);
    }
    brush.strokes(&strokes, WHITE_F, 1.0);
    brush.disc(s.head, HEAD_R, WHITE_F, 1.0);
    face(brush, s);
}

/// Shades and a grin, when the head is big enough to show them.
fn face(brush: &mut Brush, s: &Skeleton) {
    let head_px = HEAD_R * brush.view.unit;
    if head_px < 5.0 {
        return;
    }
    let a = s.head_angle;
    let at = |p: V2| s.head + p.rot(a);
    for side in [-1.0, 1.0] {
        brush.ellipse(at(v2(side * 0.027, 0.01)), v2(0.025, 0.016), a, DARK, 1.0);
    }
    brush.line(at(v2(-0.01, 0.017)), at(v2(0.01, 0.017)), 0.008, DARK, 1.0);
    if head_px >= 10.0 {
        brush.disc(at(v2(-0.036, 0.016)), 0.005, 210.0, 1.0);
    }
    brush.arc(
        at(v2(0.0, -0.004)),
        0.032,
        (a + 1.15 * PI, a + 1.85 * PI),
        0.008,
        DARK,
    );
}

/// A top hat, its brim centred on `at`, turned `angle` from upright.
pub(super) fn hat(brush: &mut Brush, at: V2, angle: f64) {
    let to = |p: V2| at + p.rot(angle);
    let felt = 225.0;
    brush.rect(to(v2(0.0, 0.064)), v2(0.048, 0.058), angle, 0.006, felt);
    brush.rect(to(v2(0.0, 0.026)), v2(0.049, 0.012), angle, 0.0, 60.0);
    brush.rect(to(v2(0.0, 0.006)), v2(0.086, 0.009), angle, 0.008, felt);
}

/// The cane, its crook hooked over the left hand.
pub(super) fn cane(brush: &mut Brush, s: &Skeleton) {
    const CROOK: f64 = 0.04;
    let (crook, tip) = s.cane;
    let along = (tip - crook).norm();
    let ferrule = tip - along * 0.04;
    let wood = 200.0;
    brush.line(crook, ferrule, 0.016, wood, 1.0);
    brush.line(ferrule, tip, 0.016, WHITE_F, 1.0);
    let n = along.rot(FRAC_PI_2);
    let a = n.angle();
    brush.arc(crook + n * CROOK, CROOK, (a, a + PI), 0.016, wood);
}
