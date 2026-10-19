//! The 48-bit frame-stamp barcode strip.
//!
//! Three rows of sixteen data cells sit between a white guard cell (left) and a
//! black guard cell (right). The guards give the decoder the frame's actual
//! black and white levels, so the threshold adapts to range conversion, gamma
//! and re-encoding. The bits are, most significant first:
//!
//! ```text
//! version:4 | fps_code:4 | frame_id:24 | reserved:8 | crc8:8
//! ```
//!
//! All geometry is in fractions of the frame size, so a scaled capture decodes
//! the same way.

use super::canvas::{BLACK, LumaCanvas, LumaView, Rect, WHITE};
use crate::FrameRate;

pub const VERSION: u8 = 1;
pub const ROWS: usize = 3;
pub const DATA_COLS: usize = 16;
pub const COLS: usize = DATA_COLS + 2;
pub const BITS: usize = ROWS * DATA_COLS;
/// Largest frame id the barcode can carry.
pub const MAX_FRAME_ID: u32 = (1 << 24) - 1;

const X0: f64 = 0.03;
const X1: f64 = 0.97;
const Y0: f64 = 0.04;
const ROW_H: f64 = 0.075;
/// Bottom edge of the strip as a fraction of frame height.
pub const STRIP_BOTTOM: f64 = Y0 + ROW_H * ROWS as f64;

/// Minimum white-to-black guard difference for a frame to be considered at all.
const MIN_CONTRAST: f64 = 48.0;

/// Frame rates that have a compact code in the barcode; anything else is code 0.
const FPS_CODES: [(u8, u32, u32); 12] = [
    (1, 24000, 1001),
    (2, 24, 1),
    (3, 25, 1),
    (4, 30000, 1001),
    (5, 30, 1),
    (6, 48, 1),
    (7, 50, 1),
    (8, 60000, 1001),
    (9, 60, 1),
    (10, 100, 1),
    (11, 120000, 1001),
    (12, 120, 1),
];

/// Returns the barcode code for `fps`, or 0 if it has none.
pub fn fps_code(fps: FrameRate) -> u8 {
    FPS_CODES
        .iter()
        .find(|&&(_, n, d)| n == fps.num() && d == fps.den())
        .map_or(0, |&(code, ..)| code)
}

pub fn fps_from_code(code: u8) -> Option<FrameRate> {
    FPS_CODES
        .iter()
        .find(|&&(c, ..)| c == code)
        .and_then(|&(_, n, d)| FrameRate::new(n, d).ok())
}

/// The information carried by one barcode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BarcodeStamp {
    pub frame_id: u32,
    pub fps: Option<FrameRate>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BarcodeError {
    #[error("no barcode (guard contrast too low)")]
    NoContrast,
    #[error("barcode checksum mismatch")]
    Checksum,
    #[error("unsupported barcode version {0}")]
    Version(u8),
}

/// Packs a stamp into the 48-bit word drawn on screen.
pub fn pack(frame_id: u32, fps: FrameRate) -> u64 {
    assert!(
        frame_id <= MAX_FRAME_ID,
        "frame id {frame_id} exceeds 24 bits"
    );
    let word =
        u64::from(VERSION) << 44 | u64::from(fps_code(fps)) << 40 | u64::from(frame_id) << 16;
    word | u64::from(crc8(&word.to_be_bytes()[2..7]))
}

/// Validates and unpacks a 48-bit word read from screen.
pub fn unpack(word: u64) -> Result<BarcodeStamp, BarcodeError> {
    if crc8(&word.to_be_bytes()[2..7]) != (word & 0xFF) as u8 {
        return Err(BarcodeError::Checksum);
    }
    let version = ((word >> 44) & 0xF) as u8;
    if version != VERSION {
        return Err(BarcodeError::Version(version));
    }
    Ok(BarcodeStamp {
        frame_id: ((word >> 16) & 0xFF_FFFF) as u32,
        fps: fps_from_code(((word >> 40) & 0xF) as u8),
    })
}

/// CRC-8 (polynomial 0x07, init 0, xorout 0x55), so an all-black strip never validates.
fn crc8(bytes: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in bytes {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
        }
    }
    crc ^ 0x55
}

/// Cell bounds in pixels as `(x0, y0, x1, y1)` for a frame of `w`×`h`.
fn cell_bounds(w: f64, h: f64, row: usize, col: usize) -> (f64, f64, f64, f64) {
    let cw = (X1 - X0) * w / COLS as f64;
    let x0 = X0 * w + col as f64 * cw;
    let y0 = (Y0 + row as f64 * ROW_H) * h;
    (x0, y0, x0 + cw, y0 + ROW_H * h)
}

fn cell_rect(w: usize, h: usize, row: usize, col: usize) -> Rect {
    let (x0, y0, x1, y1) = cell_bounds(w as f64, h as f64, row, col);
    let r = Rect {
        x0: x0.round() as i32,
        y0: y0.round() as i32,
        x1: x1.round() as i32,
        y1: y1.round() as i32,
    };
    // A dark gap between cells keeps codec ringing out of the sampled centres.
    r.inset((r.width().min(r.height()) / 12).max(1))
}

/// Draws the guard cells, which never change (part of the cached background).
pub fn draw_guards(canvas: &mut LumaCanvas<'_>) {
    let (w, h) = (canvas.width(), canvas.height());
    for row in 0..ROWS {
        canvas.fill_rect(cell_rect(w, h, row, 0), WHITE);
        canvas.fill_rect(cell_rect(w, h, row, COLS - 1), BLACK);
    }
}

/// Draws the data cells for `word`.
pub fn draw(canvas: &mut LumaCanvas<'_>, word: u64) {
    let (w, h) = (canvas.width(), canvas.height());
    for i in 0..BITS {
        let bit = (word >> (BITS - 1 - i)) & 1;
        let value = if bit == 1 { WHITE } else { BLACK };
        canvas.fill_rect(cell_rect(w, h, i / DATA_COLS, 1 + i % DATA_COLS), value);
    }
}

/// Reads the barcode from a luma frame of any size.
pub fn read(view: &LumaView<'_>) -> Result<BarcodeStamp, BarcodeError> {
    let (w, h) = (view.width() as f64, view.height() as f64);
    let (x0, y0, x1, y1) = cell_bounds(w, h, 0, 0);
    let radius = (((x1 - x0).min(y1 - y0) / 8.0) as usize).clamp(1, 6);
    let sample = |row: usize, col: usize| {
        let (x0, y0, x1, y1) = cell_bounds(w, h, row, col);
        view.mean_patch((x0 + x1) / 2.0, (y0 + y1) / 2.0, radius)
    };

    let (mut white, mut black) = (0.0, 0.0);
    for row in 0..ROWS {
        white += sample(row, 0);
        black += sample(row, COLS - 1);
    }
    let (white, black) = (white / ROWS as f64, black / ROWS as f64);
    if white - black < MIN_CONTRAST {
        return Err(BarcodeError::NoContrast);
    }
    let threshold = (white + black) / 2.0;

    let mut word = 0u64;
    for i in 0..BITS {
        let bit = sample(i / DATA_COLS, 1 + i % DATA_COLS) > threshold;
        word = (word << 1) | u64::from(bit);
    }
    unpack(word)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pattern::canvas::LumaImage;

    fn rates() -> Vec<FrameRate> {
        FPS_CODES
            .iter()
            .map(|&(_, n, d)| FrameRate::new(n, d).unwrap())
            .collect()
    }

    #[test]
    fn pack_roundtrips_every_rate() {
        for fps in rates() {
            for id in [0, 1, 2, 4095, 65535, 65536, 1_234_567, MAX_FRAME_ID] {
                let stamp = unpack(pack(id, fps)).unwrap();
                assert_eq!(
                    stamp,
                    BarcodeStamp {
                        frame_id: id,
                        fps: Some(fps)
                    }
                );
            }
        }
        let custom = FrameRate::new(15, 1).unwrap();
        assert_eq!(unpack(pack(7, custom)).unwrap().fps, None);
    }

    #[test]
    fn crc_catches_every_single_bit_flip() {
        let word = pack(424_242, FrameRate::new(30, 1).unwrap());
        for bit in 0..BITS {
            assert!(
                unpack(word ^ (1 << bit)).is_err(),
                "flip of bit {bit} not detected"
            );
        }
        assert!(unpack(0).is_err());
        assert!(unpack((1 << BITS) - 1).is_err());
    }

    #[test]
    fn draw_and_read_at_several_sizes() {
        let fps = FrameRate::new(30000, 1001).unwrap();
        for (w, h) in [(1280, 720), (640, 360), (1920, 1080), (320, 240)] {
            let mut img = LumaImage::new(w, h, BLACK);
            draw_guards(&mut img.canvas());
            for id in [0u32, 99, 54321, 9_999_999] {
                draw(&mut img.canvas(), pack(id, fps));
                assert_eq!(read(&img.view()).unwrap().frame_id, id, "{w}x{h} id {id}");
            }
        }
    }

    #[test]
    fn reads_through_noise_and_reduced_contrast() {
        let fps = FrameRate::new(25, 1).unwrap();
        let mut img = LumaImage::new(640, 360, BLACK);
        draw_guards(&mut img.canvas());
        draw(&mut img.canvas(), pack(777_777, fps));
        // Squash to a low-contrast range and add deterministic pseudo-random noise.
        let mut seed = 0x2545_f491_u32;
        for px in img.data.iter_mut() {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let noise = (seed % 41) as i32 - 20;
            let squashed = 60 + (i32::from(*px) - 16) * 100 / 219;
            *px = (squashed + noise).clamp(0, 255) as u8;
        }
        assert_eq!(read(&img.view()).unwrap().frame_id, 777_777);
    }

    #[test]
    fn blank_frames_do_not_decode() {
        let img = LumaImage::new(640, 360, BLACK);
        assert_eq!(read(&img.view()), Err(BarcodeError::NoContrast));
        let img = LumaImage::new(640, 360, WHITE);
        assert_eq!(read(&img.view()), Err(BarcodeError::NoContrast));
    }
}
