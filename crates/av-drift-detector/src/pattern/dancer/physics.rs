//! The dancer's physics. Times are in seconds and lengths in stage units: one
//! unit is the dancer's height, taken as 1.75 m.

use super::geom::{V2, v2};

/// Gravity in units per second squared: 9.81 m/s² for a 1.75 m dancer.
pub const GRAVITY: f64 = 9.81 / 1.75;

/// How an underdamped spring released from 1 settles towards 0 (its step
/// response, overshoot and all) after `t` seconds, at natural frequency
/// `omega` (rad/s) and damping ratio `zeta` (below 1).
pub fn spring_step(t: f64, omega: f64, zeta: f64) -> f64 {
    if t <= 0.0 {
        return 1.0;
    }
    let wd = omega * (1.0 - zeta * zeta).sqrt();
    (-zeta * omega * t).exp() * ((wd * t).cos() + zeta * omega / wd * (wd * t).sin())
}

/// How far an underdamped spring at rest is pushed by arriving at speed `v`
/// (a landing, a catch), `t` seconds later.
pub fn spring_impulse(t: f64, v: f64, omega: f64, zeta: f64) -> f64 {
    if t <= 0.0 {
        return 0.0;
    }
    let wd = omega * (1.0 - zeta * zeta).sqrt();
    v / wd * (-zeta * omega * t).exp() * (wd * t).sin()
}

/// Where a particle is `t` seconds after leaving `p0` at velocity `v0`, under
/// constant acceleration `a` and linear drag `drag` (per second).
pub fn drifting(p0: V2, v0: V2, a: V2, drag: f64, t: f64) -> V2 {
    let terminal = a * (1.0 / drag);
    let decay = (1.0 - (-drag * t).exp()) / drag;
    p0 + terminal * t + (v0 - terminal) * decay
}

/// A thrown object's free flight from `from` to `to` in `time` seconds: it
/// leaves at exactly the speed that makes gravity bring it down on target.
#[derive(Clone, Copy, Debug)]
pub struct Flight {
    pub from: V2,
    pub velocity: V2,
    pub time: f64,
}

impl Flight {
    pub fn new(from: V2, to: V2, time: f64) -> Self {
        let velocity = (to - from) * (1.0 / time) + v2(0.0, GRAVITY * time / 2.0);
        Flight {
            from,
            velocity,
            time,
        }
    }

    pub fn at(&self, t: f64) -> V2 {
        self.from + self.velocity * t - v2(0.0, GRAVITY * t * t / 2.0)
    }
}

pub const SCARF_LINKS: usize = 10;
pub const SCARF_POINTS: usize = SCARF_LINKS + 1;
const SCARF_LINK: f64 = 0.036;
/// Air drag on the scarf, per second.
const SCARF_DRAG: f64 = 1.2;
const SCARF_ITERATIONS: usize = 8;
/// Simulation steps per second.
const STEP_RATE: f64 = 240.0;

/// A breeze across the stage, repeating every 4 seconds, with flutter running
/// down the scarf: an acceleration on point `i` at time `t`.
fn breeze(t: f64, i: usize) -> V2 {
    use std::f64::consts::TAU;
    let i = i as f64;
    v2(
        1.4 + 0.8 * (TAU * t / 4.0).sin() + 0.5 * (TAU * t - 0.6 * i).sin(),
        0.3 * (TAU * t / 2.0 + 0.8 * i).sin(),
    )
}

/// Simulates a scarf tied at `anchor(t)` with Verlet integration and distance
/// constraints, and returns its points at each of `frames` frames (at `fps`)
/// of one loop.
///
/// The anchor's motion repeats every loop, so after a few loops of warm-up
/// the scarf settles into a repeating motion. Two consecutive loops are
/// recorded and cross-faded so that the last frame leads exactly into the
/// first.
pub fn scarf(anchor: impl Fn(f64) -> V2, frames: usize, fps: f64) -> Vec<[V2; SCARF_POINTS]> {
    const WARM_UP_LOOPS: usize = 3;
    let substeps = (STEP_RATE / fps).ceil().max(1.0) as usize;
    let dt = 1.0 / (fps * substeps as f64);
    let start = anchor(0.0);
    let mut pos: [V2; SCARF_POINTS] =
        std::array::from_fn(|i| start - v2(0.0, i as f64 * SCARF_LINK));
    let mut prev = pos;
    let mut recorded = Vec::with_capacity(2 * frames);

    for n in 0..(WARM_UP_LOOPS + 2) * frames {
        if n >= WARM_UP_LOOPS * frames {
            recorded.push(pos);
        }
        for s in 1..=substeps {
            let t = (n as f64 + s as f64 / substeps as f64) / fps;
            for (i, (p, q)) in pos.iter_mut().zip(&mut prev).enumerate().skip(1) {
                let velocity = *p - *q;
                let accel = breeze(t, i) - v2(0.0, GRAVITY);
                *q = *p;
                *p += velocity * (1.0 - SCARF_DRAG * dt) + accel * (dt * dt);
            }
            pos[0] = anchor(t);
            prev[0] = pos[0];
            for _ in 0..SCARF_ITERATIONS {
                for i in 0..SCARF_LINKS {
                    let d = pos[i + 1] - pos[i];
                    let len = d.len();
                    if len < 1e-9 {
                        continue;
                    }
                    let fix = d * ((len - SCARF_LINK) / len);
                    if i == 0 {
                        pos[1] -= fix;
                    } else {
                        pos[i] += fix * 0.5;
                        pos[i + 1] -= fix * 0.5;
                    }
                }
                for p in &mut pos[1..] {
                    p.y = p.y.max(0.004);
                }
            }
        }
    }

    let (a, b) = recorded.split_at(frames);
    (0..frames)
        .map(|i| {
            let s = i as f64 / frames as f64;
            std::array::from_fn(|k| b[i][k].lerp(a[i][k], s))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flight_lands_on_target() {
        let f = Flight::new(v2(0.0, 1.0), v2(-0.3, 0.9), 0.75);
        assert!((f.at(0.0) - v2(0.0, 1.0)).len() < 1e-12);
        assert!((f.at(0.75) - v2(-0.3, 0.9)).len() < 1e-12);
        let apex = (0..=75)
            .map(|i| f.at(i as f64 / 100.0).y)
            .fold(0.0, f64::max);
        assert!(apex > 1.3 && apex < 1.45, "apex {apex}");
    }

    #[test]
    fn springs_settle() {
        assert_eq!(spring_step(0.0, 30.0, 0.4), 1.0);
        let overshoot = (1..200)
            .map(|i| spring_step(i as f64 / 1000.0, 30.0, 0.4))
            .fold(1.0, f64::min);
        assert!(overshoot < -0.1, "overshoots: {overshoot}");
        assert!(spring_step(1.0, 30.0, 0.4).abs() < 1e-3);
        // Arriving at speed v, the spring starts moving at speed v.
        let dt = 1e-6;
        assert!((spring_impulse(dt, 1.4, 13.0, 0.55) / dt - 1.4).abs() < 1e-3);
        assert!(spring_impulse(2.0, 1.4, 13.0, 0.55).abs() < 1e-3);
    }

    #[test]
    fn drag_reaches_terminal_velocity() {
        let a = v2(0.0, 0.6);
        let p = |t| drifting(V2::default(), v2(1.0, 0.0), a, 2.0, t);
        let v = (p(10.001) - p(10.0)) * 1000.0;
        assert!((v - v2(0.0, 0.3)).len() < 1e-3);
    }

    #[test]
    fn the_scarf_loops_seamlessly() {
        use std::f64::consts::TAU;
        let fps = 30.0;
        let frames = 60;
        // An anchor swinging back and forth once per 2-second loop.
        let anchor = |t: f64| v2(1.0 + 0.2 * (TAU * t / 2.0).sin(), 0.8);
        let s = scarf(anchor, frames, fps);
        assert_eq!(s.len(), frames);
        let step = |a: &[V2; SCARF_POINTS], b: &[V2; SCARF_POINTS]| {
            a.iter()
                .zip(b)
                .map(|(p, q)| (*p - *q).len())
                .fold(0.0, f64::max)
        };
        let largest = s.windows(2).map(|w| step(&w[0], &w[1])).fold(0.0, f64::max);
        let seam = step(&s[frames - 1], &s[0]);
        assert!(seam < 1.5 * largest, "seam {seam} vs {largest}");
        for (i, points) in s.iter().enumerate() {
            assert!((points[0] - anchor(i as f64 / fps)).len() < 1e-9);
            for w in points.windows(2) {
                assert!(((w[1] - w[0]).len() - SCARF_LINK).abs() < 0.01);
            }
        }
    }
}
