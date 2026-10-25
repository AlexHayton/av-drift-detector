//! Where each visual element goes on a frame of a given size.

use super::barcode;
use super::canvas::Rect;
use super::timecode::text_width;
use crate::{Error, Result};

pub const TIMECODE_TEMPLATE: &str = "00:00:00:00";
pub const FRAME_NUMBER_TEMPLATE: &str = "F 00000000";

/// Frames narrower than this (width over height) stack the text above the
/// stage and QR code; wider ones put the QR code beside everything else.
pub const STACK_BELOW_ASPECT: f64 = 0.7;

/// Largest timecode digit height, as a fraction of the frame height.
const TIMECODE_MAX: f64 = 0.105;
/// Smallest timecode digit height, in pixels.
const TIMECODE_MIN_PX: f64 = 20.0;
/// Sizes in the text stack, relative to the timecode's digit height.
const FRAME_NUMBER: f64 = 0.7;
const SPACING: f64 = 0.33;
const PROGRESS: f64 = 0.35;
const INFO_SPACING: f64 = 0.25;
const INFO: f64 = 0.22;
/// Height of the whole text stack plus the gap below it, per unit of timecode height.
const STACK: f64 =
    1.0 + SPACING + FRAME_NUMBER + SPACING + PROGRESS + INFO_SPACING + INFO + SPACING;

/// Top-left corner and digit height of a line of seven-segment text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextBox {
    pub x: i32,
    pub y: i32,
    pub height: i32,
}

/// Pixel positions of every element on a `width`×`height` frame.
///
/// The barcode strip always spans the top of the frame. Below it is a stack
/// of text (timecode, frame number, frame-in-second progress bar, and an info
/// line with the frame size, rate and length), a stage for the dancer, and the
/// QR code:
///
/// * landscape, square and slightly tall frames: the text stack with the
///   stage under it on the left, and the QR code on the right;
/// * tall frames (width under [`STACK_BELOW_ASPECT`] × height): the text
///   across the top, then the stage and the QR code side by side.
///
/// Under the stage, along the bottom of the frame, a small line gives the
/// project's address where there's room.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub width: usize,
    pub height: usize,
    pub border: i32,
    pub timecode: TextBox,
    pub frame_number: TextBox,
    pub progress: Rect,
    /// The info line's box; its text uses the largest whole font scale that fits.
    pub info: Rect,
    /// Where the dancer performs. May be too small to use on tiny frames.
    pub stage: Rect,
    pub qr: Rect,
    /// The band under the stage, along the bottom of the frame, for the
    /// project's address. Its text uses the largest whole font scale that
    /// fits, no larger than the info line's; on tiny frames, none does.
    pub link: Rect,
}

impl Layout {
    pub fn new(width: usize, height: usize) -> Result<Self> {
        if width < 320 || height < 180 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            return Err(Error::InvalidOption(format!(
                "frame size {width}x{height} must be even and at least 320x180"
            )));
        }
        let (w, h) = (width as f64, height as f64);
        let border = ((w.min(h) / 180.0).round() as i32).max(2);
        let margin = 0.03 * w;
        let top = (barcode::STRIP_BOTTOM + 0.045) * h;
        let bottom = 0.95 * h;
        let (content_w, content_h) = (w - 2.0 * margin, bottom - top);
        let timecode_per_px = f64::from(text_width(TIMECODE_TEMPLATE, 1000)) / 1000.0;

        let (stack, stage_top, stage_w, qr) = if w / h >= STACK_BELOW_ASPECT {
            let side = content_h.min(0.42 * w);
            let qr_x = w - margin - side;
            let column = qr_x - 2.0 * margin;
            let t = (column / timecode_per_px).min((TIMECODE_MAX * h).max(TIMECODE_MIN_PX));
            let stack = Stack::new(margin, top, column, t);
            let qr_y = top + (content_h - side) / 2.0;
            (stack, stack.bottom, column, (qr_x, qr_y, side))
        } else {
            let side_goal = 0.5 * content_w;
            let t = (content_w / timecode_per_px)
                .min((TIMECODE_MAX * h).max(TIMECODE_MIN_PX))
                .min((content_h - side_goal) / STACK);
            let stack = Stack::new(margin, top, content_w, t);
            let below = bottom - stack.bottom;
            let side = below.min(side_goal);
            let qr_x = w - margin - side;
            let qr_y = stack.bottom + (below - side) / 2.0;
            (stack, stack.bottom, qr_x - 2.0 * margin, (qr_x, qr_y, side))
        };
        let (qr_x, qr_y, side) = qr;
        let stage = Rect {
            x0: margin as i32,
            y0: stage_top.ceil() as i32,
            x1: (margin + stage_w) as i32,
            y1: bottom as i32,
        };

        // Between the content and the border, a pixel clear of each, and no
        // wider than the stage, so it keeps away from the QR code.
        let gap = (border / 2).max(1);
        let link = Rect {
            x0: stage.x0,
            y0: stage.y1 + gap,
            x1: stage.x1,
            y1: height as i32 - border - gap,
        };

        Ok(Layout {
            width,
            height,
            border,
            timecode: stack.timecode,
            frame_number: stack.frame_number,
            progress: stack.progress,
            info: stack.info,
            stage,
            qr: Rect::new(qr_x as i32, qr_y as i32, side as i32, side as i32),
            link,
        })
    }
}

/// The text elements, stacked from `top` in a column `width` wide, for
/// timecode digit height `t`.
#[derive(Clone, Copy)]
struct Stack {
    timecode: TextBox,
    frame_number: TextBox,
    progress: Rect,
    info: Rect,
    /// Where the next element may start, after a gap.
    bottom: f64,
}

impl Stack {
    fn new(x: f64, top: f64, width: f64, t: f64) -> Self {
        let tc_h = t as i32;
        let fn_h = (FRAME_NUMBER * t) as i32;
        let (x, w) = (x as i32, width as i32);
        let timecode = TextBox {
            x,
            y: top as i32,
            height: tc_h,
        };
        let frame_number = TextBox {
            x,
            y: timecode.y + tc_h + (SPACING * t) as i32,
            height: fn_h,
        };
        let progress_y = frame_number.y + fn_h + (SPACING * t) as i32;
        let progress = Rect::new(x, progress_y, w, ((PROGRESS * t) as i32).max(2));
        let info_y = progress.y1 + (INFO_SPACING * t) as i32;
        let info = Rect::new(x, info_y, w, ((INFO * t) as i32).max(super::font::ROWS));
        Stack {
            timecode,
            frame_number,
            progress,
            info,
            bottom: f64::from(info.y1) + SPACING * t,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlaps(a: Rect, b: Rect) -> bool {
        a.x0 < b.x1 && b.x0 < a.x1 && a.y0 < b.y1 && b.y0 < a.y1
    }

    fn text_rect(t: TextBox, template: &str) -> Rect {
        Rect::new(t.x, t.y, text_width(template, t.height), t.height)
    }

    const SIZES: [(usize, usize); 14] = [
        (320, 180),
        (320, 240),
        (640, 480),
        (1280, 720),
        (1920, 1080),
        (3840, 2160),
        (3840, 1080),
        (320, 320),
        (1080, 1080),
        (1080, 1350),
        (1080, 1440),
        (320, 568),
        (720, 1280),
        (1080, 1920),
    ];

    #[test]
    fn elements_fit_and_do_not_overlap() {
        for (w, h) in SIZES {
            let l = Layout::new(w, h).unwrap();
            let strip_bottom = (barcode::STRIP_BOTTOM * h as f64) as i32;
            let frame = Rect::new(0, 0, w as i32, h as i32).inset(l.border);
            let elements = [
                ("timecode", text_rect(l.timecode, TIMECODE_TEMPLATE)),
                (
                    "frame number",
                    text_rect(l.frame_number, FRAME_NUMBER_TEMPLATE),
                ),
                ("progress", l.progress),
                ("info", l.info),
                ("stage", l.stage),
                ("qr", l.qr),
                ("link", l.link),
            ];
            for (i, &(name, r)) in elements.iter().enumerate() {
                assert!(r.width() > 0 && r.height() > 0, "{w}x{h}: {name} is empty");
                if name == "link" {
                    // Below everything else; it may be too short for any text.
                    assert!(r.y0 >= l.stage.y1 && r.y0 >= l.qr.y1, "{w}x{h}: link {r:?}");
                }
                assert!(r.y0 > strip_bottom, "{w}x{h}: {name} overlaps the barcode");
                assert!(
                    r.x0 >= frame.x0 && r.y0 >= frame.y0 && r.x1 <= frame.x1 && r.y1 <= frame.y1,
                    "{w}x{h}: {name} {r:?} is outside the frame"
                );
                for &(other, s) in &elements[i + 1..] {
                    assert!(
                        !overlaps(r, s),
                        "{w}x{h}: {name} {r:?} overlaps {other} {s:?}"
                    );
                }
            }
            assert!(l.timecode.height >= 20, "{w}x{h}: timecode too small");
            let short = w.min(h) as i32;
            assert!(l.qr.width() * 3 >= short, "{w}x{h}: QR code too small");
            assert!(l.stage.height() * 6 >= short, "{w}x{h}: stage too small");
        }
        assert!(Layout::new(321, 180).is_err());
        assert!(Layout::new(300, 180).is_err());
    }

    #[test]
    fn tall_frames_stack_the_text_above_the_qr_code() {
        let wide = Layout::new(1280, 720).unwrap();
        assert!(wide.qr.y0 < wide.progress.y1, "beside the text");
        let square = Layout::new(1080, 1080).unwrap();
        assert!(square.qr.x0 > square.progress.x1);
        let tall = Layout::new(1080, 1920).unwrap();
        assert!(tall.qr.y0 > tall.info.y1, "below the text");
        assert!(
            tall.progress.width() > tall.qr.width(),
            "text spans the frame"
        );
        assert!(
            tall.stage.y0 > tall.info.y1 && tall.stage.x1 <= tall.qr.x0,
            "stage beside the QR code"
        );
    }
}
