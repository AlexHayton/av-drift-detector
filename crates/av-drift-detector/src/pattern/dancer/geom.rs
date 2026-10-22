//! 2-D vectors, easing and distance functions, in stage units (y up).

use std::f64::consts::{PI, TAU};
use std::ops::{Add, AddAssign, Mul, Neg, Sub, SubAssign};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct V2 {
    pub x: f64,
    pub y: f64,
}

pub const fn v2(x: f64, y: f64) -> V2 {
    V2 { x, y }
}

impl V2 {
    /// The unit vector at `angle` radians counter-clockwise from +x.
    pub fn polar(angle: f64) -> V2 {
        let (s, c) = angle.sin_cos();
        v2(c, s)
    }

    pub fn len(self) -> f64 {
        self.x.hypot(self.y)
    }

    pub fn dot(self, o: V2) -> f64 {
        self.x * o.x + self.y * o.y
    }

    pub fn angle(self) -> f64 {
        self.y.atan2(self.x)
    }

    /// Rotated `angle` radians counter-clockwise.
    pub fn rot(self, angle: f64) -> V2 {
        let (s, c) = angle.sin_cos();
        v2(c * self.x - s * self.y, s * self.x + c * self.y)
    }

    pub fn norm(self) -> V2 {
        let l = self.len();
        if l > 0.0 { self * (1.0 / l) } else { self }
    }

    pub fn lerp(self, o: V2, s: f64) -> V2 {
        self + (o - self) * s
    }

    pub fn min(self, o: V2) -> V2 {
        v2(self.x.min(o.x), self.y.min(o.y))
    }

    pub fn max(self, o: V2) -> V2 {
        v2(self.x.max(o.x), self.y.max(o.y))
    }
}

impl Add for V2 {
    type Output = V2;
    fn add(self, o: V2) -> V2 {
        v2(self.x + o.x, self.y + o.y)
    }
}

impl Sub for V2 {
    type Output = V2;
    fn sub(self, o: V2) -> V2 {
        v2(self.x - o.x, self.y - o.y)
    }
}

impl Mul<f64> for V2 {
    type Output = V2;
    fn mul(self, s: f64) -> V2 {
        v2(self.x * s, self.y * s)
    }
}

impl Neg for V2 {
    type Output = V2;
    fn neg(self) -> V2 {
        v2(-self.x, -self.y)
    }
}

impl AddAssign for V2 {
    fn add_assign(&mut self, o: V2) {
        *self = *self + o;
    }
}

impl SubAssign for V2 {
    fn sub_assign(&mut self, o: V2) {
        *self = *self - o;
    }
}

pub fn lerp(a: f64, b: f64, s: f64) -> f64 {
    a + (b - a) * s
}

/// Interpolates between angles the short way round.
pub fn lerp_angle(a: f64, b: f64, s: f64) -> f64 {
    a + ((b - a + PI).rem_euclid(TAU) - PI) * s
}

/// Smoothstep of `x` clamped to 0..1.
pub fn smooth(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// 1 on the beat, falling to 0 half way through it; `p` is the phase within the beat.
pub fn pulse(p: f64) -> f64 {
    0.5 + 0.5 * (TAU * p).cos()
}

/// 0 before `a`, easing up to 1 at `m` and back down to 0 at `b`.
pub fn bump(x: f64, a: f64, m: f64, b: f64) -> f64 {
    if x < m {
        smooth((x - a) / (m - a))
    } else {
        1.0 - smooth((x - m) / (b - m))
    }
}

/// Smooth interpolation through `(time, value)` keys sorted by time, holding
/// the first and last values outside them.
pub fn keyed<T: Copy>(keys: &[(f64, T)], t: f64, mix: impl Fn(T, T, f64) -> T) -> T {
    let next = keys.iter().position(|&(k, _)| k > t).unwrap_or(keys.len());
    match next {
        0 => keys[0].1,
        n if n == keys.len() => keys[n - 1].1,
        n => {
            let (t0, a) = keys[n - 1];
            let (t1, b) = keys[n];
            mix(a, b, smooth((t - t0) / (t1 - t0)))
        }
    }
}

/// Distance from `p` to the segment `a`–`b`.
pub fn segment(p: V2, a: V2, b: V2) -> f64 {
    let (pa, ba) = (p - a, b - a);
    let h = (pa.dot(ba) / ba.dot(ba).max(1e-12)).clamp(0.0, 1.0);
    (pa - ba * h).len()
}

/// Signed distance from `p` to an axis-aligned box centred on the origin.
pub fn sd_box(p: V2, half: V2) -> f64 {
    let q = v2(p.x.abs() - half.x, p.y.abs() - half.y);
    v2(q.x.max(0.0), q.y.max(0.0)).len() + q.x.max(q.y).min(0.0)
}

/// Approximate signed distance from `p` to an axis-aligned ellipse centred on the origin.
pub fn sd_ellipse(p: V2, radii: V2) -> f64 {
    let k0 = v2(p.x / radii.x, p.y / radii.y).len();
    let k1 = v2(p.x / (radii.x * radii.x), p.y / (radii.y * radii.y)).len();
    if k1 < 1e-12 {
        -radii.x.min(radii.y)
    } else {
        k0 * (k0 - 1.0) / k1
    }
}

/// Distance from `p` to the arc of radius `r` around `c` running
/// counter-clockwise from angle `a0` to `a1`.
pub fn arc(p: V2, c: V2, r: f64, a0: f64, a1: f64) -> f64 {
    let d = p - c;
    let a = a0 + (d.angle() - a0).rem_euclid(TAU);
    if a <= a1 {
        (d.len() - r).abs()
    } else {
        let e0 = c + V2::polar(a0) * r;
        let e1 = c + V2::polar(a1) * r;
        (p - e0).len().min((p - e1).len())
    }
}

/// A deterministic pseudo-random number in 0..1 for `(seed, i)` (SplitMix64).
pub fn random(seed: u64, i: u64) -> f64 {
    let mut z = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(i.wrapping_mul(0xBF58_476D_1CE4_E5B9))
        .wrapping_add(0x94D0_49BB_1331_11EB);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distances() {
        assert!((segment(v2(0.0, 1.0), v2(-1.0, 0.0), v2(1.0, 0.0)) - 1.0).abs() < 1e-12);
        assert!((segment(v2(3.0, 0.0), v2(-1.0, 0.0), v2(1.0, 0.0)) - 2.0).abs() < 1e-12);
        assert!((sd_box(v2(0.0, 0.0), v2(1.0, 2.0)) + 1.0).abs() < 1e-12);
        assert!((sd_box(v2(2.0, 0.0), v2(1.0, 2.0)) - 1.0).abs() < 1e-12);
        assert!(sd_ellipse(v2(0.0, 0.0), v2(2.0, 1.0)) < 0.0);
        assert!((sd_ellipse(v2(3.0, 0.0), v2(2.0, 1.0)) - 1.0).abs() < 0.3);
        // A semicircle over the top.
        assert!(arc(v2(0.0, 1.0), V2::default(), 1.0, 0.0, PI) < 1e-12);
        assert!((arc(v2(0.0, -1.0), V2::default(), 1.0, 0.0, PI) - 2f64.sqrt()).abs() < 1e-12);
    }

    #[test]
    fn easing_and_keys() {
        assert_eq!(smooth(-1.0), 0.0);
        assert_eq!(smooth(2.0), 1.0);
        assert!(
            (lerp_angle(3.0, -3.0, 0.5) - PI).abs() < 0.3,
            "the short way"
        );
        let keys = [(0.0, 1.0), (1.0, 3.0), (2.0, 3.0)];
        let mix = |a: f64, b: f64, s: f64| lerp(a, b, s);
        assert_eq!(keyed(&keys, -1.0, mix), 1.0);
        assert_eq!(keyed(&keys, 0.5, mix), 2.0);
        assert_eq!(keyed(&keys, 5.0, mix), 3.0);
        let r: Vec<f64> = (0..1000).map(|i| random(7, i)).collect();
        assert!(r.iter().all(|x| (0.0..1.0).contains(x)));
        let mean = r.iter().sum::<f64>() / 1000.0;
        assert!((mean - 0.5).abs() < 0.05);
    }
}
