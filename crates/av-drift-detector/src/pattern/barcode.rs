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

pub const VERSION: u8 = 2;
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

/// Packs a stamp into the 48-bit word drawn on screen. The frame number is
/// Gray-coded, so consecutive frames differ in a single frame-number cell (and
/// the CRC cells): a picture caught between two frames is grey in only a few
/// cells, and reads, thresholded, as one frame or the other.
pub fn pack(frame_id: u32, fps: FrameRate) -> u64 {
    assert!(
        frame_id <= MAX_FRAME_ID,
        "frame id {frame_id} exceeds 24 bits"
    );
    let gray = frame_id ^ (frame_id >> 1);
    let word = u64::from(VERSION) << 44 | u64::from(fps_code(fps)) << 40 | u64::from(gray) << 16;
    word | u64::from(crc8(&word.to_be_bytes()[2..7]))
}

/// The number whose Gray code is `gray`.
fn from_gray(gray: u32) -> u32 {
    let mut n = gray;
    let mut shift = 1;
    while shift < 32 {
        n ^= n >> shift;
        shift <<= 1;
    }
    n
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
        frame_id: from_gray(((word >> 16) & 0xFF_FFFF) as u32),
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

/// The 48 data cells' brightness, normalised so that the black guards are 0
/// and the white guards 1, or `None` if the guards don't show a barcode.
pub fn sample(view: &LumaView<'_>) -> Option<[f32; BITS]> {
    let size = (view.width() as f64, view.height() as f64);
    sample_mapped(view, size, 1.0, |x, y| Some((x, y)))
}

/// [`sample`] for a picture of a `frame_size` test frame that isn't
/// pixel-aligned (a camera's): `map` takes a test-frame pixel to the view's,
/// and `scale` is roughly how many view pixels one test-frame pixel covers.
pub fn sample_mapped(
    view: &LumaView<'_>,
    frame_size: (f64, f64),
    scale: f64,
    map: impl Fn(f64, f64) -> Option<(f64, f64)>,
) -> Option<[f32; BITS]> {
    let (w, h) = frame_size;
    let (x0, y0, x1, y1) = cell_bounds(w, h, 0, 0);
    let radius = (((x1 - x0).min(y1 - y0) * scale / 8.0) as usize).clamp(1, 6);
    let (vw, vh) = (view.width() as f64, view.height() as f64);
    let at = |row: usize, col: usize| {
        let (x0, y0, x1, y1) = cell_bounds(w, h, row, col);
        let (x, y) = map((x0 + x1) / 2.0, (y0 + y1) / 2.0)?;
        ((0.0..vw).contains(&x) && (0.0..vh).contains(&y)).then(|| view.mean_patch(x, y, radius))
    };
    let mut cells = [0.0f64; BITS];
    for (i, cell) in cells.iter_mut().enumerate() {
        *cell = at(i / DATA_COLS, 1 + i % DATA_COLS)?;
    }
    let (mut white, mut black) = (0.0, 0.0);
    for row in 0..ROWS {
        white += at(row, 0)?;
        black += at(row, COLS - 1)?;
    }
    let (white, black) = (white / ROWS as f64, black / ROWS as f64);
    if white - black < MIN_CONTRAST {
        return None;
    }
    Some(cells.map(|v| ((v - black) / (white - black)) as f32))
}

/// The word frame `frame_id` shows: `fps` is the rate the barcode carries
/// (`None` for a rate without a code).
pub fn word(frame_id: u32, fps: Option<FrameRate>) -> u64 {
    // Any rate without a code packs the same.
    let fps = fps.unwrap_or(FrameRate::new(15, 1).expect("valid rate"));
    pack(frame_id.min(MAX_FRAME_ID), fps)
}

/// Fits the cells as a blend of two consecutive frames' words, the earlier
/// one in `candidates`, as `pattern::qr_fit` does for QR codes. A
/// frame caught mid-switch fails the CRC; this reads it, and measures the mix.
/// Only a few cells change from one frame to the next, so the tone curve is
/// often the capture's so far, from `tone` (which learns from the fit).
pub fn fit_blend(
    cells: &[f32; BITS],
    fps: Option<FrameRate>,
    candidates: std::ops::RangeInclusive<u32>,
    tone: &mut super::ToneCurve,
) -> Option<super::Blend> {
    let light = |word: u64, i: usize| (word >> (BITS - 1 - i)) & 1 == 1;
    let pair = |m: u32| {
        (m < MAX_FRAME_ID).then(|| {
            let (a, b) = (word(m, fps), word(m + 1, fps));
            // The barcode is one row: a rolling shutter doesn't show across it.
            let mut groups = super::blend::Cells::new(1);
            for (i, &s) in cells.iter().enumerate() {
                groups.add(0, s, light(a, i), light(b, i));
            }
            groups
        })
    };
    super::blend::fit_candidates(candidates, pair, false, tone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pattern::ToneCurve;
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
    fn consecutive_frames_differ_in_one_frame_cell() {
        let fps = FrameRate::new(30, 1).unwrap();
        for id in [0u32, 1, 2, 4095, 65535, 1_234_567, MAX_FRAME_ID - 1] {
            let field = |w: u64| (w >> 16) & 0xFF_FFFF;
            let diff = field(pack(id, fps)) ^ field(pack(id + 1, fps));
            assert_eq!(diff.count_ones(), 1, "{id} -> {}", id + 1);
            assert_eq!(from_gray(id ^ (id >> 1)), id);
        }
    }

    #[test]
    fn older_versions_are_recognised() {
        // Version 1 (0.3 and earlier): plain binary frame number.
        let word = 1u64 << 44 | 5u64 << 40 | 1234u64 << 16;
        let word = word | u64::from(crc8(&word.to_be_bytes()[2..7]));
        assert_eq!(unpack(word), Err(BarcodeError::Version(1)));
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
    fn fits_blends_of_two_frames() {
        let fps = FrameRate::new(30, 1).unwrap();
        let frame = |id: u32| {
            let mut img = LumaImage::new(640, 360, BLACK);
            draw_guards(&mut img.canvas());
            draw(&mut img.canvas(), pack(id, fps));
            img
        };
        // 0x0FFF -> 0x1000 flips many bits: a hard case for a threshold, not for the fit.
        let (a, b) = (frame(4095), frame(4096));
        for w in [0.1f32, 0.45, 0.55, 0.8] {
            let mut img = a.clone();
            for (o, (&x, &y)) in img.data.iter_mut().zip(a.data.iter().zip(&b.data)) {
                *o = (f32::from(x) * (1.0 - w) + f32::from(y) * w).round() as u8;
            }
            let cells = sample(&img.view()).unwrap();
            let fit = fit_blend(&cells, Some(fps), 4090..=4100, &mut ToneCurve::default()).unwrap();
            assert_eq!(fit.first, 4095, "w {w}: {fit:?}");
            assert!((fit.weight_next - w).abs() < 0.02, "w {w}: {fit:?}");
        }
        // A clean frame: all the weight on one frame.
        let cells = sample(&frame(77).view()).unwrap();
        let fit = fit_blend(&cells, Some(fps), 70..=80, &mut ToneCurve::default()).unwrap();
        assert_eq!(fit.dominant(), 77);
        assert!(!fit.is_mixed());
    }

    #[test]
    fn fits_blends_through_a_tone_curve() {
        use crate::pattern::blend::curves::{gamma22, mix_through};
        let fps = FrameRate::new(30, 1).unwrap();
        let frame = |id: u32| {
            let mut img = LumaImage::new(640, 360, BLACK);
            draw_guards(&mut img.canvas());
            draw(&mut img.canvas(), pack(id, fps));
            img
        };
        // Cells that went each way, at least two of each: (rises, falls).
        let ways = |id: u32| {
            let diff = pack(id, fps) ^ pack(id + 1, fps);
            let rises = (diff & pack(id + 1, fps)).count_ones();
            (rises, diff.count_ones() - rises)
        };
        let both = (1000..2000)
            .find(|&id| ways(id).0 >= 2 && ways(id).1 >= 2)
            .unwrap();
        let one_way = (1000..2000).find(|&id| ways(id).1 == 0).unwrap();
        for id in [both, one_way] {
            for w in [0.3, 0.6] {
                let img = mix_through(&frame(id), &frame(id + 1), w, gamma22);
                let cells = sample(&img.view()).unwrap();
                let candidates = id - 3..=id + 3;
                // With the capture's curve known from earlier pictures.
                let mut tone = ToneCurve::known(2.2);
                let fit = fit_blend(&cells, Some(fps), candidates.clone(), &mut tone).unwrap();
                assert_eq!(fit.first, id, "{id} w {w}: {fit:?}");
                assert!(
                    (f64::from(fit.weight_next) - w).abs() < 0.02,
                    "{id} w {w}: {fit:?}"
                );
                // Without: only cells that went both ways show the curve.
                let mut tone = ToneCurve::default();
                let fit = fit_blend(&cells, Some(fps), candidates, &mut tone).unwrap();
                if id == both {
                    assert!((fit.gamma - 2.2).abs() < 0.2, "{id} w {w}: {fit:?}");
                    assert!(
                        (f64::from(fit.weight_next) - w).abs() < 0.02,
                        "{id} w {w}: {fit:?}"
                    );
                } else {
                    assert_eq!(fit.gamma, 1.0, "{id} w {w}: {fit:?}");
                }
            }
        }
    }

    #[test]
    fn blank_frames_do_not_decode() {
        let img = LumaImage::new(640, 360, BLACK);
        assert_eq!(read(&img.view()), Err(BarcodeError::NoContrast));
        let img = LumaImage::new(640, 360, WHITE);
        assert_eq!(read(&img.view()), Err(BarcodeError::NoContrast));
    }
}
