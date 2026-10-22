//! A stick figure in shades and a top hat, dancing with a cane and a scarf on
//! a light-up floor beside a boombox.
//!
//! Each frame is drawn from its number alone. The routine repeats every 8
//! seconds (16 beats at 120 beats per minute, so a beat falls on every half
//! of the frame-in-second bar). What needs simulating, the scarf, is
//! simulated once for one loop when the dancer is made; everything else is
//! worked out in closed form: the jump and the hat's flight under gravity,
//! the landing on a damped spring, notes and dust under drag.

mod choreo;
mod draw;
mod geom;
mod physics;

use std::f64::consts::{FRAC_PI_2, TAU};

use self::choreo::{BEATS_PER_SECOND, Choreo, HAT_TOSS, LOOP_BEATS, Skeleton};
use self::draw::Brush;
use self::geom::{V2, lerp, v2};
use self::physics::{Flight, SCARF_POINTS};
use super::canvas::{LumaCanvas, Rect};
use crate::FrameRate;

/// The stage's height in dancer heights, floor included.
const STAGE_TALL: f64 = 1.45;
/// The narrowest stage worth dancing on, in dancer heights.
const STAGE_WIDE: f64 = 1.7;
/// The light-up floor's depth below the floor line.
const FLOOR: f64 = 0.07;
/// The smallest dancer worth drawing, in pixels.
const MIN_UNIT_PX: f64 = 14.0;
/// Room for the boombox, to the dancer's right.
const BOOMBOX_ROOM: f64 = 0.65;
/// The widest the dancer's own spot gets.
const SPOT_MAX: f64 = 2.8;

/// Maps stage units to pixels.
#[derive(Clone, Copy, Debug)]
struct View {
    /// Pixels per unit: the dancer's height in pixels.
    unit: f64,
    /// Pixel x of stage x = 0.
    x0: f64,
    /// Pixel y of the floor line.
    floor: f64,
}

pub(crate) struct Dancer {
    clip: Rect,
    view: View,
    /// The stage's width and its height above the floor, in units.
    size: V2,
    choreo: Choreo,
    boombox: Option<f64>,
    toss: Toss,
    nominal: u32,
    loop_frames: u64,
    /// The scarf's points at each frame of the loop.
    scarf: Vec<[V2; SCARF_POINTS]>,
}

impl Dancer {
    /// A dancer on `stage` (in pixels), or `None` if the stage is too small.
    pub fn new(stage: Rect, fps: FrameRate) -> Option<Dancer> {
        let (w, h) = (f64::from(stage.width()), f64::from(stage.height()));
        let unit = (h / STAGE_TALL).min(w / STAGE_WIDE);
        if unit < MIN_UNIT_PX {
            return None;
        }
        let width = w / unit;
        let has_boombox = width >= STAGE_WIDE + BOOMBOX_ROOM + 0.5;
        let spot = (width - if has_boombox { BOOMBOX_ROOM } else { 0.0 }).min(SPOT_MAX);
        let left = (width - spot - if has_boombox { BOOMBOX_ROOM } else { 0.0 }) / 2.0;
        let choreo = Choreo {
            home: left + spot / 2.0,
            travel: ((spot - STAGE_WIDE) / 0.8).clamp(0.0, 1.0),
        };
        let nominal = fps.nominal();
        let loop_frames = u64::from(nominal) * (LOOP_BEATS / BEATS_PER_SECOND) as u64;
        let knot_at =
            |seconds: f64| scarf_knot(&Skeleton::new(&choreo.pose(seconds * BEATS_PER_SECOND)));
        Some(Dancer {
            clip: stage,
            view: View {
                unit,
                x0: f64::from(stage.x0),
                floor: f64::from(stage.y1) - FLOOR * unit,
            },
            size: v2(width, h / unit - FLOOR),
            choreo,
            boombox: has_boombox.then_some(left + spot + BOOMBOX_ROOM / 2.0),
            toss: Toss::new(&choreo),
            nominal,
            loop_frames,
            scarf: physics::scarf(knot_at, loop_frames as usize, f64::from(nominal)),
        })
    }

    /// Draws the dancer as at frame `frame_id`.
    pub fn draw(&self, frame_id: u32, canvas: &mut LumaCanvas<'_>) {
        let frame = u64::from(frame_id) % self.loop_frames;
        let beat = frame as f64 * BEATS_PER_SECOND / f64::from(self.nominal);
        let skeleton = Skeleton::new(&self.choreo.pose(beat));
        let (hat_at, hat_angle) = self.toss.hat(beat, &skeleton);

        let mut brush = Brush::new(canvas, self.clip, self.view);
        draw::floor(&mut brush, self.size.x, beat, &skeleton);
        draw::shadow(&mut brush, &skeleton);
        if let Some(x) = self.boombox {
            draw::boombox(&mut brush, x, beat);
            draw::notes(&mut brush, x, beat, self.size.y);
        }
        draw::scarf_tail(&mut brush, &self.scarf[frame as usize]);
        draw::body(&mut brush, &skeleton);
        draw::scarf_wrap(&mut brush, &skeleton);
        draw::cane(&mut brush, &skeleton);
        draw::hat(&mut brush, hat_at, hat_angle);
        draw::dust(&mut brush, self.choreo.home, beat);
    }
}

/// Where the scarf is tied: at the neck, on the dancer's right.
fn scarf_knot(s: &Skeleton) -> V2 {
    s.neck + s.up.rot(-FRAC_PI_2) * 0.02 - s.up * 0.012
}

/// The hat's flight in bar 3: flicked off the head, it turns one somersault
/// and comes down on the head again, wherever the dancer has got to.
struct Toss {
    flight: Flight,
    /// The hat's angle at take-off and landing (a full turn apart, more or less).
    angles: (f64, f64),
}

impl Toss {
    fn new(choreo: &Choreo) -> Self {
        let (b0, b1) = HAT_TOSS;
        let (from, a0) = Skeleton::new(&choreo.pose(b0)).hat();
        let (to, a1) = Skeleton::new(&choreo.pose(b1)).hat();
        // Roll the way it travels.
        let turn = if to.x <= from.x { TAU } else { -TAU };
        Toss {
            flight: Flight::new(from, to, (b1 - b0) / BEATS_PER_SECOND),
            angles: (a0, a1 + turn),
        }
    }

    /// The hat's brim centre and angle at `beat`: in the air, or worn.
    fn hat(&self, beat: f64, worn_by: &Skeleton) -> (V2, f64) {
        let (b0, b1) = HAT_TOSS;
        if (b0..b1).contains(&beat) {
            let s = (beat - b0) / (b1 - b0);
            let at = self.flight.at(s * self.flight.time);
            (at, lerp(self.angles.0, self.angles.1, s))
        } else {
            worn_by.hat()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pattern::canvas::{BLACK, LumaImage};

    fn dancer(w: i32, h: i32, fps: &str) -> (Dancer, Rect) {
        let stage = Rect::new(20, 30, w, h);
        (Dancer::new(stage, fps.parse().unwrap()).unwrap(), stage)
    }

    #[test]
    fn needs_room() {
        let fps = "30".parse().unwrap();
        assert!(Dancer::new(Rect::new(0, 0, 40, 20), fps).is_none());
        assert!(Dancer::new(Rect::new(0, 0, 60, 40), fps).is_some());
    }

    #[test]
    fn the_boombox_comes_when_there_is_room() {
        let (wide, _) = dancer(700, 160, "30");
        let (narrow, _) = dancer(170, 160, "30");
        let box_x = wide.boombox.unwrap();
        assert!(box_x > wide.choreo.home + 1.0 && box_x < wide.size.x);
        assert!(wide.choreo.travel > 0.99);
        assert!(narrow.boombox.is_none());
        assert!(narrow.choreo.travel < 0.1);
    }

    #[test]
    fn the_hat_lands_on_the_head() {
        let (d, _) = dancer(700, 160, "30");
        let (b0, b1) = HAT_TOSS;
        for (b, eps) in [(b0, 0.0), (b0 - 1e-6, 1e-6), (b1 - 1e-6, 1e-6), (b1, 0.0)] {
            let worn = Skeleton::new(&d.choreo.pose(b)).hat();
            let (at, angle) = d.toss.hat(b, &Skeleton::new(&d.choreo.pose(b)));
            assert!(
                (at - worn.0).len() < 1e-4 + eps * 10.0,
                "beat {b}: {at:?} vs {:?}",
                worn.0
            );
            let turned = (angle - worn.1).rem_euclid(TAU);
            assert!(
                !(1e-3..=TAU - 1e-3).contains(&turned),
                "beat {b}: angle {angle} vs {}",
                worn.1
            );
        }
        let mid = d
            .toss
            .hat((b0 + b1) / 2.0, &Skeleton::new(&d.choreo.pose(b0)))
            .0;
        assert!(mid.y > 1.05 && mid.y < d.size.y - 0.12, "apex {mid:?}");
    }

    #[test]
    fn draws_only_on_the_stage_and_moves() {
        for (w, h, fps) in [
            (700, 160, "30"),
            (200, 300, "29.97"),
            (300, 300, "120"),
            (90, 50, "15"),
        ] {
            let (d, stage) = dancer(w, h, fps);
            let size = (w as usize + 60, h as usize + 80);
            let mut seen = Vec::new();
            for frame in [0, 7, 100, 239, 240, 1000] {
                let mut img = LumaImage::new(size.0, size.1, BLACK);
                d.draw(frame, &mut img.canvas());
                let view = img.view();
                let mut lit = 0;
                for y in 0..size.1 {
                    for x in 0..size.0 {
                        let inside = (stage.x0..stage.x1).contains(&(x as i32))
                            && (stage.y0..stage.y1).contains(&(y as i32));
                        let v = view.get(x, y);
                        assert!(
                            inside || v == BLACK,
                            "{w}x{h}: frame {frame} drew at ({x}, {y})"
                        );
                        lit += usize::from(v > 128);
                    }
                }
                assert!(lit > 50, "{w}x{h}: frame {frame} is nearly empty");
                seen.push(img.data);
            }
            assert_ne!(seen[0], seen[1], "{w}x{h}: frames 0 and 7 look the same");
            assert_ne!(seen[1], seen[2]);
        }
    }

    #[test]
    fn loops_every_eight_seconds() {
        for fps in ["30", "29.97", "25", "60"] {
            let (d, _) = dancer(500, 160, fps);
            let n = d.loop_frames as u32;
            let render = |f| {
                let mut img = LumaImage::new(560, 240, BLACK);
                d.draw(f, &mut img.canvas());
                img.data
            };
            assert_eq!(render(3), render(n + 3), "{fps}");
            assert_eq!(
                n,
                "8".parse::<f64>().unwrap() as u32 * fps.parse::<FrameRate>().unwrap().nominal()
            );
        }
    }
}
