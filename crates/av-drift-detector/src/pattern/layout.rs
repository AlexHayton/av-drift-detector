//! Where each visual element goes on a frame of a given size.

use super::barcode;
use super::canvas::Rect;
use super::timecode::text_width;
use crate::{Error, Result};

pub const TIMECODE_TEMPLATE: &str = "00:00:00:00";
pub const FRAME_NUMBER_TEMPLATE: &str = "F 00000000";

/// Top-left corner and digit height of a line of seven-segment text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextBox {
    pub x: i32,
    pub y: i32,
    pub height: i32,
}

/// Pixel positions of every element on a `width`×`height` frame.
///
/// The barcode strip spans the top of the frame. Below it, the timecode, frame
/// number and frame-in-second progress bar are stacked on the left, with the
/// QR code on the right.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub width: usize,
    pub height: usize,
    pub border: i32,
    pub timecode: TextBox,
    pub frame_number: TextBox,
    pub progress: Rect,
    pub qr: Rect,
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

        let side = (bottom - top).min(0.42 * w);
        let qr = Rect::new(
            (w - margin - side) as i32,
            top as i32,
            side as i32,
            side as i32,
        );

        let left = margin;
        let avail = qr.x0 as f64 - margin - left;
        let fit = |text: &str, max_h: f64| {
            let width_per_px = f64::from(text_width(text, 1000)) / 1000.0;
            max_h.min(avail / width_per_px) as i32
        };
        let tc_h = fit(TIMECODE_TEMPLATE, 0.17 * h);
        let fn_h = fit(FRAME_NUMBER_TEMPLATE, 0.12 * h);
        let spacing = (0.07 * h) as i32;

        let timecode = TextBox {
            x: left as i32,
            y: top as i32,
            height: tc_h,
        };
        let frame_number = TextBox {
            x: left as i32,
            y: timecode.y + tc_h + spacing,
            height: fn_h,
        };
        let progress_y = frame_number.y + fn_h + spacing;
        let progress_h = ((0.06 * h) as i32).min(bottom as i32 - progress_y).max(2);
        let progress = Rect::new(left as i32, progress_y, avail as i32, progress_h);

        Ok(Layout {
            width,
            height,
            border,
            timecode,
            frame_number,
            progress,
            qr,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elements_fit_and_do_not_overlap() {
        for (w, h) in [
            (320, 180),
            (320, 240),
            (640, 480),
            (1280, 720),
            (1920, 1080),
            (3840, 2160),
        ] {
            let l = Layout::new(w, h).unwrap();
            let strip_bottom = (barcode::STRIP_BOTTOM * h as f64) as i32;
            assert!(
                l.timecode.y > strip_bottom && l.qr.y0 > strip_bottom,
                "{w}x{h}"
            );
            let tc_right = l.timecode.x + text_width(TIMECODE_TEMPLATE, l.timecode.height);
            let fn_right =
                l.frame_number.x + text_width(FRAME_NUMBER_TEMPLATE, l.frame_number.height);
            assert!(
                tc_right <= l.qr.x0 && fn_right <= l.qr.x0 && l.progress.x1 <= l.qr.x0,
                "{w}x{h}"
            );
            assert!(
                l.progress.y1 <= h as i32 && l.qr.y1 <= h as i32 && l.qr.x1 <= w as i32,
                "{w}x{h}"
            );
            assert!(l.timecode.height >= 20, "{w}x{h}: timecode too small");
        }
        assert!(Layout::new(321, 180).is_err());
        assert!(Layout::new(300, 180).is_err());
    }
}
