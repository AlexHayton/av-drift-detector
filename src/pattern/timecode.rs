//! SMPTE timecode and a seven-segment renderer (no font files needed).

use std::fmt;

use super::canvas::{LumaCanvas, Rect};
use crate::FrameRate;

/// An SMPTE timecode. Drop-frame (`;` separator) is used for 29.97 and 59.94.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timecode {
    pub hours: u32,
    pub minutes: u32,
    pub seconds: u32,
    pub frames: u32,
    pub drop_frame: bool,
}

impl Timecode {
    pub fn from_frame(frame: u64, fps: FrameRate) -> Self {
        let nominal = u64::from(fps.nominal());
        let drop_frame = fps.is_drop_frame();
        let mut f = frame;
        if drop_frame {
            // Skip `drop` frame numbers at the start of every minute except each tenth.
            let drop = nominal / 15; // 2 at 29.97, 4 at 59.94
            let per_10min = nominal * 600 - drop * 9;
            let per_min = nominal * 60 - drop;
            let (tens, rem) = (f / per_10min, f % per_10min);
            f += drop * 9 * tens;
            if rem > drop {
                f += drop * ((rem - drop) / per_min);
            }
        }
        Timecode {
            frames: (f % nominal) as u32,
            seconds: (f / nominal % 60) as u32,
            minutes: (f / (nominal * 60) % 60) as u32,
            hours: (f / (nominal * 3600) % 24) as u32,
            drop_frame,
        }
    }
}

impl fmt::Display for Timecode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sep = if self.drop_frame { ';' } else { ':' };
        write!(
            f,
            "{:02}:{:02}:{:02}{sep}{:02}",
            self.hours, self.minutes, self.seconds, self.frames
        )
    }
}

// Segment bits: a=top, b=top-right, c=bottom-right, d=bottom, e=bottom-left, f=top-left, g=middle.
const A: u8 = 1;
const B: u8 = 2;
const C: u8 = 4;
const D: u8 = 8;
const E: u8 = 16;
const F: u8 = 32;
const G: u8 = 64;
const DIGITS: [u8; 10] = [
    A | B | C | D | E | F,
    B | C,
    A | B | G | E | D,
    A | B | G | C | D,
    F | G | B | C,
    A | F | G | C | D,
    A | F | G | E | C | D,
    A | B | C,
    A | B | C | D | E | F | G,
    A | B | C | D | F | G,
];

fn segments(ch: char) -> Option<u8> {
    match ch {
        '0'..='9' => Some(DIGITS[ch as usize - '0' as usize]),
        'F' => Some(A | E | F | G),
        '-' => Some(G),
        _ => None,
    }
}

fn digit_width(h: i32) -> i32 {
    h / 2
}

fn gap(h: i32) -> i32 {
    (h / 8).max(1)
}

fn thickness(h: i32) -> i32 {
    (h / 10).max(1)
}

fn advance(ch: char, h: i32) -> i32 {
    match ch {
        ':' | ';' => thickness(h) + gap(h),
        _ => digit_width(h) + gap(h),
    }
}

/// Width in pixels of `text` rendered with digit height `h`.
pub fn text_width(text: &str, h: i32) -> i32 {
    let total: i32 = text.chars().map(|c| advance(c, h)).sum();
    total - gap(h)
}

/// Draws `text` (digits, `F`, `-`, `:`, `;`, space) with its top-left at `(x, y)`.
pub fn draw_text(canvas: &mut LumaCanvas<'_>, x: i32, y: i32, h: i32, text: &str, value: u8) {
    let mut cx = x;
    for ch in text.chars() {
        draw_char(canvas, cx, y, h, ch, value);
        cx += advance(ch, h);
    }
}

fn draw_char(canvas: &mut LumaCanvas<'_>, x: i32, y: i32, h: i32, ch: char, value: u8) {
    let t = thickness(h);
    if ch == ':' || ch == ';' {
        canvas.fill_rect(Rect::new(x, y + h / 3 - t / 2, t, t), value);
        canvas.fill_rect(Rect::new(x, y + 2 * h / 3 - t / 2, t, t), value);
        if ch == ';' {
            canvas.fill_rect(Rect::new(x, y + 2 * h / 3 + t / 2, t / 2 + 1, t), value);
        }
        return;
    }
    let Some(seg) = segments(ch) else { return };
    let w = digit_width(h);
    let mid = y + h / 2;
    let parts = [
        (
            A,
            Rect {
                x0: x + t,
                y0: y,
                x1: x + w - t,
                y1: y + t,
            },
        ),
        (
            B,
            Rect {
                x0: x + w - t,
                y0: y + t,
                x1: x + w,
                y1: mid,
            },
        ),
        (
            C,
            Rect {
                x0: x + w - t,
                y0: mid,
                x1: x + w,
                y1: y + h - t,
            },
        ),
        (
            D,
            Rect {
                x0: x + t,
                y0: y + h - t,
                x1: x + w - t,
                y1: y + h,
            },
        ),
        (
            E,
            Rect {
                x0: x,
                y0: mid,
                x1: x + t,
                y1: y + h - t,
            },
        ),
        (
            F,
            Rect {
                x0: x,
                y0: y + t,
                x1: x + t,
                y1: mid,
            },
        ),
        (
            G,
            Rect {
                x0: x + t,
                y0: mid - t / 2,
                x1: x + w - t,
                y1: mid - t / 2 + t,
            },
        ),
    ];
    for (bit, rect) in parts {
        if seg & bit != 0 {
            canvas.fill_rect(rect, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tc(frame: u64, fps: &str) -> String {
        Timecode::from_frame(frame, fps.parse().unwrap()).to_string()
    }

    #[test]
    fn non_drop_frame() {
        assert_eq!(tc(0, "30"), "00:00:00:00");
        assert_eq!(tc(29, "30"), "00:00:00:29");
        assert_eq!(tc(30, "30"), "00:00:01:00");
        assert_eq!(tc(30 * 3661 + 5, "30"), "01:01:01:05");
        assert_eq!(tc(24, "23.976"), "00:00:01:00");
        assert_eq!(tc(25 * 60, "25"), "00:01:00:00");
    }

    #[test]
    fn drop_frame_29_97() {
        assert_eq!(tc(1799, "29.97"), "00:00:59;29");
        assert_eq!(tc(1800, "29.97"), "00:01:00;02");
        assert_eq!(tc(17982, "29.97"), "00:10:00;00");
        assert_eq!(tc(17981, "29.97"), "00:09:59;29");
        // One hour of 29.97 is 107892 frames.
        assert_eq!(tc(107_892, "29.97"), "01:00:00;00");
    }

    #[test]
    fn drop_frame_59_94() {
        assert_eq!(tc(3599, "59.94"), "00:00:59;59");
        assert_eq!(tc(3600, "59.94"), "00:01:00;04");
        assert_eq!(tc(35964, "59.94"), "00:10:00;00");
    }

    #[test]
    fn text_width_matches_advances() {
        let h = 100;
        assert_eq!(text_width("0", h), digit_width(h));
        assert_eq!(text_width("00", h), 2 * digit_width(h) + gap(h));
        assert_eq!(
            text_width("0:0", h),
            2 * digit_width(h) + thickness(h) + 2 * gap(h)
        );
    }
}
