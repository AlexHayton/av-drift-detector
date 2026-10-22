//! The routine: 16 beats at 120 beats per minute (8 seconds) in four bars,
//! each a pose as a function of the beat. Inverse kinematics turns poses
//! into skeletons. Lengths are in units of the dancer's height; y is up from
//! the floor.

use std::f64::consts::{FRAC_PI_2, PI, TAU};

use super::geom::{V2, bump, keyed, lerp, lerp_angle, pulse, smooth, v2};
use super::physics::{GRAVITY, spring_impulse, spring_step};

pub const BEATS_PER_SECOND: f64 = 2.0;
pub const LOOP_BEATS: f64 = 16.0;
const BAR: f64 = 4.0;
/// Beats either side of a bar line over which one move fades into the next.
const BLEND: f64 = 0.25;

pub const THIGH: f64 = 0.25;
pub const SHIN: f64 = 0.25;
pub const TORSO: f64 = 0.31;
pub const NECK: f64 = 0.035;
pub const HEAD_R: f64 = 0.068;
pub const UPPER_ARM: f64 = 0.17;
pub const FOREARM: f64 = 0.16;
pub const SHOE: f64 = 0.055;
pub const CANE: f64 = 0.52;
const HIP_W: f64 = 0.03;
const SHOULDER_DROP: f64 = 0.025;
/// Pelvis height standing with soft knees.
pub const STAND: f64 = 0.475;
/// Feet apart, either side of centre.
const STANCE: f64 = 0.14;
/// Where the cane's tip rests, left of centre, while he leans on it.
const CANE_REST: f64 = 0.17;

/// When the hat leaves the head and lands back on it, in beats.
pub const HAT_TOSS: (f64, f64) = (9.0, 10.0);
/// When the star jump takes off and lands, in beats.
pub const JUMP: (f64, f64) = (14.0, 15.0);

/// A pose: where the dancer is and what each limb reaches for. Index 0 is
/// the limb on the left of the screen.
#[derive(Clone, Copy, Debug)]
pub struct Pose {
    pub pelvis: V2,
    /// Torso angle from upright, counter-clockwise.
    pub lean: f64,
    /// Head angle relative to the torso, counter-clockwise.
    pub nod: f64,
    pub feet: [V2; 2],
    /// Out-of-reach targets leave the arm straight, pointing at them.
    pub hands: [V2; 2],
    /// How far the toes point down, 0 to 1.
    pub toes: [f64; 2],
    /// The cane's direction, crook to tip, held in the left hand.
    pub cane_angle: f64,
    /// Where along the cane the hand holds it: 0 at the crook, 0.5 in the middle.
    pub cane_grip: f64,
}

impl Pose {
    fn mix(&self, o: &Pose, s: f64) -> Pose {
        let pair = |a: [V2; 2], b: [V2; 2]| [a[0].lerp(b[0], s), a[1].lerp(b[1], s)];
        Pose {
            pelvis: self.pelvis.lerp(o.pelvis, s),
            lean: lerp(self.lean, o.lean, s),
            nod: lerp(self.nod, o.nod, s),
            feet: pair(self.feet, o.feet),
            hands: pair(self.hands, o.hands),
            toes: [
                lerp(self.toes[0], o.toes[0], s),
                lerp(self.toes[1], o.toes[1], s),
            ],
            cane_angle: lerp_angle(self.cane_angle, o.cane_angle, s),
            cane_grip: lerp(self.cane_grip, o.cane_grip, s),
        }
    }
}

fn torso_up(lean: f64) -> V2 {
    V2::polar(FRAC_PI_2 + lean)
}

fn shoulder(pelvis: V2, lean: f64) -> V2 {
    pelvis + torso_up(lean) * (TORSO - SHOULDER_DROP)
}

/// Two-bone inverse kinematics: the middle joint and end of a limb from
/// `root` reaching for `target`, bending counter-clockwise if `bend` is 1 or
/// clockwise if -1.
fn ik(root: V2, target: V2, l1: f64, l2: f64, bend: f64) -> (V2, V2) {
    let d = target - root;
    let reach = d.len().clamp((l1 - l2).abs() + 1e-6, l1 + l2 - 1e-6);
    let dir = if d.len() > 1e-9 {
        d.norm()
    } else {
        v2(0.0, -1.0)
    };
    let cos = ((l1 * l1 + reach * reach - l2 * l2) / (2.0 * l1 * reach)).clamp(-1.0, 1.0);
    (root + dir.rot(bend * cos.acos()) * l1, root + dir * reach)
}

/// Every joint of a posed dancer.
#[derive(Clone, Copy, Debug)]
pub struct Skeleton {
    pub pelvis: V2,
    pub neck: V2,
    pub shoulder: V2,
    pub head: V2,
    /// The head's angle from upright, counter-clockwise.
    pub head_angle: f64,
    /// The torso's direction, pelvis to neck.
    pub up: V2,
    pub hips: [V2; 2],
    pub knees: [V2; 2],
    pub ankles: [V2; 2],
    pub toes: [V2; 2],
    pub elbows: [V2; 2],
    pub hands: [V2; 2],
    /// The cane's crook and tip.
    pub cane: (V2, V2),
}

impl Skeleton {
    pub fn new(p: &Pose) -> Self {
        let up = torso_up(p.lean);
        let right = up.rot(-FRAC_PI_2);
        let neck = p.pelvis + up * TORSO;
        let shoulder = shoulder(p.pelvis, p.lean);
        let head_angle = p.lean + p.nod;
        let head = neck + V2::polar(FRAC_PI_2 + head_angle) * (NECK + HEAD_R);
        let hips = [p.pelvis - right * HIP_W, p.pelvis + right * HIP_W];
        // Knees bend outwards, elbows downwards.
        let (knee_l, ankle_l) = ik(hips[0], p.feet[0], THIGH, SHIN, -1.0);
        let (knee_r, ankle_r) = ik(hips[1], p.feet[1], THIGH, SHIN, 1.0);
        let (elbow_l, hand_l) = ik(shoulder, p.hands[0], UPPER_ARM, FOREARM, 1.0);
        let (elbow_r, hand_r) = ik(shoulder, p.hands[1], UPPER_ARM, FOREARM, -1.0);
        let toes = [
            ankle_l + V2::polar(PI + 0.9 * p.toes[0]) * SHOE,
            ankle_r + V2::polar(-0.9 * p.toes[1]) * SHOE,
        ];
        let along = V2::polar(p.cane_angle) * CANE;
        let cane = (
            hand_l - along * p.cane_grip,
            hand_l + along * (1.0 - p.cane_grip),
        );
        Skeleton {
            pelvis: p.pelvis,
            neck,
            shoulder,
            head,
            head_angle,
            up,
            hips,
            knees: [knee_l, knee_r],
            ankles: [ankle_l, ankle_r],
            toes,
            elbows: [elbow_l, elbow_r],
            hands: [hand_l, hand_r],
            cane,
        }
    }

    /// Where a worn hat's brim sits, and the hat's angle: tilted a little to the right.
    pub fn hat(&self) -> (V2, f64) {
        let angle = self.head_angle - 0.12;
        (
            self.head + V2::polar(FRAC_PI_2 + angle) * (HEAD_R * 0.75),
            angle,
        )
    }
}

/// The routine, fitted to the room on the stage.
#[derive(Clone, Copy, Debug)]
pub struct Choreo {
    /// The centre of the dancer's spot.
    pub home: f64,
    /// How much of the routine's travelling there's room for, 0 to 1.
    pub travel: f64,
}

impl Choreo {
    /// The pose at `beat`, which wraps every [`LOOP_BEATS`]. Each bar fades
    /// into the next across the bar line.
    pub fn pose(&self, beat: f64) -> Pose {
        let b = beat.rem_euclid(LOOP_BEATS);
        let bar = ((b / BAR) as usize).min(3);
        let into = b - bar as f64 * BAR;
        let here = self.bar(bar, b);
        if into < BLEND {
            let b_before = if bar == 0 { b + LOOP_BEATS } else { b };
            let s = smooth((into + BLEND) / (2.0 * BLEND));
            self.bar((bar + 3) % 4, b_before).mix(&here, s)
        } else if into > BAR - BLEND {
            let b_after = if bar == 3 { b - LOOP_BEATS } else { b };
            let s = smooth((into - (BAR - BLEND)) / (2.0 * BLEND));
            here.mix(&self.bar((bar + 1) % 4, b_after), s)
        } else {
            here
        }
    }

    fn bar(&self, bar: usize, b: f64) -> Pose {
        match bar {
            0 => self.disco(b),
            1 => self.twirl(b - BAR),
            2 => self.hat_trick(b - 2.0 * BAR),
            _ => self.robot(b - 3.0 * BAR),
        }
    }

    fn planted(&self, x: f64) -> [V2; 2] {
        [v2(x - STANCE, 0.0), v2(x + STANCE, 0.0)]
    }

    /// Where the left hand holds the cane's crook while the tip rests on the floor.
    fn cane_rest(&self) -> V2 {
        v2(self.home - CANE_REST, CANE)
    }

    /// Bar 1: bounce on the beat and point at the sky, then the floor, leaning on the cane.
    fn disco(&self, b: f64) -> Pose {
        let p = b.rem_euclid(1.0);
        let sway = (PI * b).sin();
        let pelvis = v2(self.home + 0.025 * sway, STAND - 0.035 * pulse(p));
        let lean = -0.05 * sway;
        let s = smooth(p / 0.35);
        let raised = if b.floor().rem_euclid(2.0) == 0.0 {
            s
        } else {
            1.0 - s
        };
        // The arm swings out round the shoulder, from across the body up to the sky.
        let (down, up) = (v2(-0.13, -0.27), v2(0.22, 0.25));
        let point =
            V2::polar(lerp(down.angle(), up.angle(), raised)) * lerp(down.len(), up.len(), raised);
        Pose {
            pelvis,
            lean,
            nod: 0.12 * (1.0 - 2.0 * raised),
            feet: self.planted(self.home),
            hands: [self.cane_rest(), shoulder(pelvis, lean) + point],
            toes: [0.0; 2],
            cane_angle: -FRAC_PI_2,
            cane_grip: 0.0,
        }
    }

    /// Bar 2: step-touch out and back, twirling the cane by its middle.
    fn twirl(&self, t: f64) -> Pose {
        let step = 0.11 * self.travel;
        let marks = [0.0, step, 2.0 * step, step, 0.0];
        let offset = |t: f64| {
            let t = t.clamp(0.0, 4.0);
            let k = (t.floor() as usize).min(3);
            lerp(marks[k], marks[k + 1], smooth((t - k as f64) / 0.6))
        };
        let heading = if t < 2.0 { 1.0 } else { -1.0 };
        let foot = |side: f64| {
            if !(0.0..4.0).contains(&t) {
                return v2(self.home + side * STANCE, 0.0);
            }
            // Each beat, the foot on the side he's heading steps first and
            // the other follows; both are down again before the next beat.
            let k = t.floor() as usize;
            let start = if side * heading > 0.0 { 0.0 } else { 0.25 };
            let q = ((t - k as f64 - start) / 0.5).clamp(0.0, 1.0);
            let x = lerp(marks[k], marks[k + 1], smooth(q));
            v2(self.home + side * STANCE + x, 0.05 * (PI * q).sin())
        };
        let p = t.rem_euclid(1.0);
        let pelvis = v2(self.home + offset(t), STAND - 0.03 * pulse(p));
        let lean = -0.07 * heading * (PI * (p / 0.6).min(1.0)).sin();
        let sh = shoulder(pelvis, lean);
        let feet = [foot(-1.0), foot(1.0)];
        let spins = 3.0 * smooth((t - 0.25) / 3.5);
        Pose {
            pelvis,
            lean,
            nod: 0.08 * (PI * t).sin(),
            feet,
            hands: [sh + v2(-0.27, 0.07), sh + v2(0.2, -0.12 + 0.1 * pulse(p))],
            toes: [12.0 * feet[0].y, 12.0 * feet[1].y],
            cane_angle: -FRAC_PI_2 - TAU * spins,
            cane_grip: 0.5,
        }
    }

    /// Bar 3: flick the hat off the head, glide out from under it and back,
    /// catch it, tip it, jazz hands. (The hat's flight is worked out from the
    /// poses at [`HAT_TOSS`].)
    fn hat_trick(&self, t: f64) -> Pose {
        let glide = 0.3 * self.travel;
        let x = self.home - glide * smooth((t - 1.0) / 1.2) + glide * smooth((t - 2.5) / 1.3);
        let since_catch = (t - (HAT_TOSS.1 - 2.0 * BAR)) / BEATS_PER_SECOND;
        let hip = STAND - 0.06 * bump(t, 0.1, 0.5, 1.0) + 0.02 * bump(t, 0.5, 1.0, 1.5)
            - spring_impulse(since_catch, 0.35, 16.0, 0.5);
        let pelvis = v2(x, hip);
        let gliding = bump(t, 0.9, 1.4, 2.2).max(bump(t, 2.4, 3.0, 3.9));
        let heel = |phase: f64| gliding * (TAU * t + phase).sin().max(0.0);
        let heels = [heel(0.0), heel(PI)];
        let lean = 0.05 * gliding * (PI * t).sin();
        let sh = shoulder(pelvis, lean);
        let rest = [v2(-0.2, -0.18), v2(0.12, -0.27)];
        let ready = [v2(-0.2, -0.15), v2(0.16, -0.2)];
        let wide = [v2(-0.27, 0.13), v2(0.27, 0.13)];
        let tip = [v2(-0.27, 0.1), v2(0.1, 0.21)];
        let shake = 0.015 * (TAU * 6.0 * t).sin();
        let jazz = [v2(-0.21, 0.07 + shake), v2(0.21, 0.07 - shake)];
        let keys = [
            (0.25, rest),
            (0.6, ready),
            (1.0, wide),
            (2.0, wide),
            (2.4, tip),
            (2.9, tip),
            (3.3, jazz),
        ];
        let arms = keyed(&keys, t, |a: [V2; 2], b: [V2; 2], s| {
            [a[0].lerp(b[0], s), a[1].lerp(b[1], s)]
        });
        Pose {
            pelvis,
            lean,
            nod: 0.1 * bump(t, 3.0, 3.4, 4.2) * (TAU * 2.0 * t).sin(),
            feet: [
                v2(x - STANCE, 0.03 * heels[0]),
                v2(x + STANCE, 0.03 * heels[1]),
            ],
            hands: [sh + arms[0], sh + arms[1]],
            // Heels up, toes still on the floor.
            toes: heels.map(|h| (0.03 * h / SHOE).asin() / 0.9),
            cane_angle: -FRAC_PI_2 - 0.45,
            cane_grip: 0.0,
        }
    }

    /// Bar 4: robot arms that snap to each pose and wobble on springs, then a
    /// star jump, landing back on the cane.
    fn robot(&self, t: f64) -> Pose {
        let secs = |beats: f64| beats / BEATS_PER_SECOND;
        const ARMS: [[V2; 2]; 5] = [
            [v2(-0.21, 0.07), v2(0.21, 0.07)],
            [v2(-0.17, 0.16), v2(0.17, -0.16)],
            [v2(-0.17, -0.16), v2(0.17, 0.16)],
            [v2(-0.17, 0.16), v2(0.17, 0.16)],
            [v2(-0.06, -0.1), v2(0.17, -0.16)],
        ];
        const NODS: [f64; 5] = [0.0, 0.2, -0.2, 0.15, 0.0];
        // A new pose every half beat from the bar line, the last held.
        let k = ((2.0 * t).floor() + 1.0).clamp(0.0, 4.0) as usize;
        let settle = if k == 0 {
            0.0
        } else {
            spring_step(secs(t - (k - 1) as f64 / 2.0), 38.0, 0.45)
        };
        let prev = k.saturating_sub(1);
        let mut arms = [
            ARMS[k][0].lerp(ARMS[prev][0], settle),
            ARMS[k][1].lerp(ARMS[prev][1], settle),
        ];
        let nod = lerp(NODS[k], NODS[prev], settle);

        let (up_t, down_t) = (JUMP.0 - 3.0 * BAR, JUMP.1 - 3.0 * BAR);
        let speed = GRAVITY * secs(down_t - up_t) / 2.0;
        let drive = up_t - 0.1;
        let hip = if t < drive {
            STAND - 0.09 * smooth((t - 1.55) / 0.3)
        } else if t < up_t {
            let s = (t - drive) / (up_t - drive);
            hermite(STAND - 0.09, 0.0, STAND, speed * secs(up_t - drive), s)
        } else if t < down_t {
            let tau = secs(t - up_t);
            STAND + speed * tau - GRAVITY * tau * tau / 2.0
        } else {
            STAND - spring_impulse(secs(t - down_t), speed, 13.0, 0.55)
        };
        let pelvis = v2(self.home, hip);
        let sh = shoulder(pelvis, 0.0);
        let flight = (t - up_t) / (down_t - up_t);
        let spread = if (0.0..1.0).contains(&flight) {
            smooth(flight.min(1.0 - flight) / 0.25)
        } else {
            0.0
        };
        let planted = self.planted(self.home);
        let feet = [
            planted[0].lerp(pelvis + v2(-0.26, -0.4), spread),
            planted[1].lerp(pelvis + v2(0.26, -0.4), spread),
        ];

        let crouch = [v2(-0.2, -0.16), v2(0.14, -0.24)];
        let star = [v2(-0.25, 0.22), v2(0.25, 0.22)];
        let landed = [self.cane_rest() - sh, v2(-0.13, -0.27)];
        let mut cane = -FRAC_PI_2 - 0.3;
        for (pose, cane_angle, s) in [
            (crouch, -FRAC_PI_2 - 0.8, smooth((t - 1.55) / 0.3)),
            (
                star,
                star[0].angle() + 0.35,
                smooth((t - up_t + 0.05) / 0.35),
            ),
            (landed, -FRAC_PI_2, smooth((t - down_t) / 0.6)),
        ] {
            arms = [arms[0].lerp(pose[0], s), arms[1].lerp(pose[1], s)];
            cane = lerp_angle(cane, cane_angle, s);
        }
        Pose {
            pelvis,
            lean: 0.0,
            nod: nod * (1.0 - smooth((t - 1.55) / 0.3)),
            feet,
            hands: [sh + arms[0], sh + arms[1]],
            toes: [spread; 2],
            cane_angle: cane,
            cane_grip: 0.0,
        }
    }
}

/// Cubic Hermite interpolation from `y0` (slope `m0`) to `y1` (slope `m1`) for `s` in 0..1.
fn hermite(y0: f64, m0: f64, y1: f64, m1: f64, s: f64) -> f64 {
    let (s2, s3) = (s * s, s * s * s);
    (2.0 * s3 - 3.0 * s2 + 1.0) * y0
        + (s3 - 2.0 * s2 + s) * m0
        + (3.0 * s2 - 2.0 * s3) * y1
        + (s3 - s2) * m1
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHOREO: Choreo = Choreo {
        home: 1.5,
        travel: 1.0,
    };

    fn points(s: &Skeleton) -> Vec<V2> {
        let mut v = vec![s.pelvis, s.neck, s.head, s.cane.0, s.cane.1];
        for i in 0..2 {
            v.extend([s.knees[i], s.ankles[i], s.toes[i], s.elbows[i], s.hands[i]]);
        }
        v
    }

    #[test]
    fn limbs_keep_their_lengths() {
        for i in 0..1600 {
            let s = Skeleton::new(&CHOREO.pose(i as f64 / 100.0));
            for k in 0..2 {
                assert!(((s.knees[k] - s.hips[k]).len() - THIGH).abs() < 1e-9);
                assert!(((s.ankles[k] - s.knees[k]).len() - SHIN).abs() < 1e-9);
                assert!(((s.elbows[k] - s.shoulder).len() - UPPER_ARM).abs() < 1e-9);
                assert!(((s.hands[k] - s.elbows[k]).len() - FOREARM).abs() < 1e-9);
            }
            assert!(((s.cane.1 - s.cane.0).len() - CANE).abs() < 1e-9);
        }
    }

    #[test]
    fn moves_are_continuous_and_loop() {
        // Sampled at 480 Hz, nothing jumps: the fastest motion (the robot's
        // springs, the cane twirl) covers well under 0.03 units a sample.
        let n = 16 * 240;
        let at = |i: usize| points(&Skeleton::new(&CHOREO.pose(i as f64 / 240.0)));
        let mut last = at(n - 1);
        for i in 0..n {
            let now = at(i);
            let jump = now
                .iter()
                .zip(&last)
                .map(|(a, b)| (*a - *b).len())
                .fold(0.0, f64::max);
            assert!(jump < 0.03, "beat {:.4}: moved {jump:.4}", i as f64 / 240.0);
            last = now;
        }
    }

    #[test]
    fn stays_on_the_stage() {
        for i in 0..1600 {
            let b = i as f64 / 100.0;
            let s = Skeleton::new(&CHOREO.pose(b));
            for p in points(&s) {
                assert!(p.y > -0.01 && p.y < 1.36, "beat {b}: {p:?}");
                assert!((p.x - CHOREO.home).abs() < 0.95, "beat {b}: {p:?}");
            }
            assert!(s.ankles[0].y >= -1e-9 && s.ankles[1].y >= -1e-9);
            let tip = s.cane.1;
            if b < 3.7 {
                assert!(
                    tip.y.abs() < 0.03,
                    "beat {b}: cane tip off the floor at {tip:?}"
                );
            }
        }
    }

    #[test]
    fn jumps_and_lands() {
        let hip = |b: f64| CHOREO.pose(b).pelvis.y;
        let apex = hip((JUMP.0 + JUMP.1) / 2.0);
        assert!((apex - STAND - GRAVITY * 0.25 * 0.25 / 2.0).abs() < 1e-9);
        assert!((hip(JUMP.1) - STAND).abs() < 1e-9);
        assert!(hip(JUMP.1 + 0.3) < STAND - 0.04, "knees absorb the landing");
        let s = Skeleton::new(&CHOREO.pose((JUMP.0 + JUMP.1) / 2.0));
        assert!(
            s.ankles[0].y > 0.05 && s.ankles[1].y > 0.05,
            "both feet off the floor"
        );
    }
}
