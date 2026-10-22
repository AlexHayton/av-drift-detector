//! A small 7-pixel-high bitmap font for the info line: digits, `×`, `·`, `.`,
//! `:`, `f`, `p`, `s` and space. Each font pixel is drawn as a `scale`×`scale` square.

use super::canvas::{LumaCanvas, Rect};

/// Rows of font pixels in a glyph.
pub const ROWS: i32 = 7;

/// A glyph's width in font pixels and its rows, top first. Bit `width - 1` is
/// the leftmost pixel.
type Glyph = (u8, [u8; ROWS as usize]);

fn glyph(ch: char) -> Option<Glyph> {
    Some(match ch {
        '0' => (
            5,
            [
                0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
            ],
        ),
        '1' => (
            5,
            [
                0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
            ],
        ),
        '2' => (
            5,
            [
                0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
            ],
        ),
        '3' => (
            5,
            [
                0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110,
            ],
        ),
        '4' => (
            5,
            [
                0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
            ],
        ),
        '5' => (
            5,
            [
                0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110,
            ],
        ),
        '6' => (
            5,
            [
                0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
            ],
        ),
        '7' => (
            5,
            [
                0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
            ],
        ),
        '8' => (
            5,
            [
                0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
            ],
        ),
        '9' => (
            5,
            [
                0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100,
            ],
        ),
        '×' => (5, [0, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0]),
        '·' => (3, [0, 0, 0, 0b010, 0, 0, 0]),
        '.' => (2, [0, 0, 0, 0, 0, 0b11, 0b11]),
        ':' => (2, [0, 0b11, 0b11, 0, 0b11, 0b11, 0]),
        'f' => (4, [0b0011, 0b0100, 0b1111, 0b0100, 0b0100, 0b0100, 0b0100]),
        'p' => (4, [0, 0, 0b1110, 0b1001, 0b1110, 0b1000, 0b1000]),
        's' => (4, [0, 0, 0b0111, 0b1000, 0b0110, 0b0001, 0b1110]),
        ' ' => (3, [0; ROWS as usize]),
        _ => return None,
    })
}

/// Whether every character of `text` has a glyph.
#[cfg(test)]
pub fn supports(text: &str) -> bool {
    text.chars().all(|c| glyph(c).is_some())
}

/// Width in pixels of `text` at `scale`, with one font pixel between glyphs.
pub fn text_width(text: &str, scale: i32) -> i32 {
    let units: i32 = text
        .chars()
        .filter_map(glyph)
        .map(|(w, _)| i32::from(w) + 1)
        .sum();
    (units - 1).max(0) * scale
}

/// Draws `text` with its top-left corner at `(x, y)`. Characters without a
/// glyph are skipped.
pub fn draw_text(canvas: &mut LumaCanvas<'_>, x: i32, y: i32, scale: i32, text: &str, value: u8) {
    let mut cx = x;
    for (width, rows) in text.chars().filter_map(glyph) {
        let width = i32::from(width);
        for (row, bits) in (0..).zip(rows) {
            for col in 0..width {
                if bits >> (width - 1 - col) & 1 == 1 {
                    let px = Rect::new(cx + col * scale, y + row * scale, scale, scale);
                    canvas.fill_rect(px, value);
                }
            }
        }
        cx += (width + 1) * scale;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pattern::canvas::LumaImage;

    #[test]
    fn widths_add_up() {
        assert_eq!(text_width("", 3), 0);
        assert_eq!(text_width("1", 1), 5);
        assert_eq!(text_width("1.5", 2), (5 + 1 + 2 + 1 + 5) * 2);
        assert!(supports("1920×1080 · 29.97 fps · 1:00:00.5"));
        assert!(!supports("1920x1080"));
    }

    #[test]
    fn draws_within_its_box() {
        let mut img = LumaImage::new(80, 20, 0);
        let text = "12×3 · 4.5 fps";
        let (x, y, scale) = (2, 3, 1);
        draw_text(&mut img.canvas(), x, y, scale, text, 200);
        let w = text_width(text, scale);
        for py in 0..20 {
            for px in 0..80 {
                let inside =
                    (x..x + w).contains(&(px as i32)) && (y..y + ROWS).contains(&(py as i32));
                if !inside {
                    assert_eq!(img.view().get(px, py), 0, "pixel ({px}, {py})");
                }
            }
        }
        // The first glyph, `1`, has its top pixel in the middle column.
        assert_eq!(img.view().get(4, 3), 200);
    }
}
