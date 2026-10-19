//! Per-frame QR code: a self-describing stamp that survives being filmed.
//!
//! The barcode strip needs a pixel-aligned capture. A QR code can be found
//! anywhere in a camera image, at any angle, so it carries everything an
//! analyser needs, without prior knowledge, to read a phone recording of a
//! screen:
//!
//! ```text
//! AVD:2 F:00001234 R:30000/1001 A:T4X16G2K21 T:00:00:41.04
//! ```
//!
//! | field | meaning |
//! |-------|---------|
//! | `AVD:2` | payload format version |
//! | `F` | frame number (zero-padded, so the code never changes size) |
//! | `R` | exact frame rate |
//! | `A` | audio tone plan (see [`ToneCode::descriptor`]) |
//! | `T` | SMPTE timecode, `.` before the frames for drop-frame |
//!
//! Only QR "alphanumeric" characters are used (upper case, digits, space,
//! `:` `/` `.`). They pack about 45% denser than bytes, which leaves room
//! for error-correction level Q (25% of the code can be lost) at a small
//! version with large modules.

use qrcode::{Color, EcLevel, QrCode};

use super::canvas::{BLACK, LumaCanvas, LumaImage, LumaView, Rect, WHITE};
use super::timecode::Timecode;
use crate::audio::ToneCode;
use crate::{Error, FrameRate, Result};

const PREFIX: &str = "AVD:2";
const QUIET_ZONE: usize = 4;

/// The stamp recovered from a QR code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QrStamp {
    pub frame_id: u32,
    pub fps: FrameRate,
    /// Base harmonic of the audio tone plan, if the code carried a supported one.
    pub tone_base_harmonic: Option<u32>,
}

/// Builds the QR payload for one frame.
pub fn payload(frame_id: u32, fps: FrameRate, tones: &ToneCode, timecode: &Timecode) -> String {
    let tc = timecode.to_string().replace(';', ".");
    format!(
        "{PREFIX} F:{frame_id:08} R:{fps} A:{} T:{tc}",
        tones.descriptor()
    )
}

pub fn parse_payload(text: &str) -> Option<QrStamp> {
    let mut fields = text.split(' ');
    if fields.next()? != PREFIX {
        return None;
    }
    let (mut frame_id, mut fps, mut tones) = (None, None, None);
    for field in fields {
        match field.split_once(':') {
            Some(("F", v)) => frame_id = v.parse().ok(),
            Some(("R", v)) => fps = v.parse().ok(),
            Some(("A", v)) => tones = ToneCode::parse_descriptor(v).ok(),
            _ => {}
        }
    }
    Some(QrStamp {
        frame_id: frame_id?,
        fps: fps?,
        tone_base_harmonic: tones,
    })
}

/// Draws the QR code for `data`, centred in `area` with a white quiet zone.
pub fn draw(canvas: &mut LumaCanvas<'_>, area: Rect, data: &str) -> Result<()> {
    let code = QrCode::with_error_correction_level(data, EcLevel::Q)
        .map_err(|e| Error::Qr(e.to_string()))?;
    let modules = code.width();
    let span = modules + 2 * QUIET_ZONE;
    let module = (area.width().min(area.height()) as usize / span) as i32;
    if module == 0 {
        return Err(Error::Qr("frame too small for the QR code".into()));
    }
    let side = module * span as i32;
    let x0 = area.x0 + (area.width() - side) / 2;
    let y0 = area.y0 + (area.height() - side) / 2;
    canvas.fill_rect(Rect::new(x0, y0, side, side), WHITE);
    let origin = QUIET_ZONE as i32 * module;
    for (i, color) in code.to_colors().into_iter().enumerate() {
        if color == Color::Dark {
            let (mx, my) = ((i % modules) as i32, (i / modules) as i32);
            canvas.fill_rect(
                Rect::new(
                    x0 + origin + mx * module,
                    y0 + origin + my * module,
                    module,
                    module,
                ),
                BLACK,
            );
        }
    }
    Ok(())
}

/// Frames wider than this are box-downscaled before the QR search, which is
/// much faster and loses nothing: a filmed code still has several pixels per module.
#[cfg(feature = "qr-decode")]
const MAX_SEARCH_WIDTH: usize = 1280;

/// Searches the whole frame for an av-drift-detector QR code.
#[cfg(feature = "qr-decode")]
pub fn read(view: &LumaView<'_>) -> Option<QrStamp> {
    let factor = view.width().div_ceil(MAX_SEARCH_WIDTH);
    if factor > 1 {
        let small = downscale(view, factor);
        return search(&small.view());
    }
    search(view)
}

#[cfg(feature = "qr-decode")]
fn search(view: &LumaView<'_>) -> Option<QrStamp> {
    let mut img =
        rqrr::PreparedImage::prepare_from_greyscale(view.width(), view.height(), |x, y| {
            view.get(x, y)
        });
    img.detect_grids()
        .into_iter()
        .filter_map(|grid| grid.decode().ok())
        .find_map(|(_, text)| parse_payload(&text))
}

/// Averages `factor`×`factor` blocks.
pub fn downscale(view: &LumaView<'_>, factor: usize) -> LumaImage {
    let (w, h) = (view.width() / factor, view.height() / factor);
    let mut out = LumaImage::new(w, h, 0);
    let area = (factor * factor) as u32;
    for y in 0..h {
        for x in 0..w {
            let mut sum = 0u32;
            for dy in 0..factor {
                for dx in 0..factor {
                    sum += u32::from(view.get(x * factor + dx, y * factor + dy));
                }
            }
            out.data[y * w + x] = (sum / area) as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tones(fps: FrameRate) -> ToneCode {
        ToneCode::new(fps, 48000).unwrap()
    }

    #[test]
    fn payload_roundtrip() {
        let fps: FrameRate = "29.97".parse().unwrap();
        let text = payload(1234, fps, &tones(fps), &Timecode::from_frame(1234, fps));
        assert_eq!(
            text,
            "AVD:2 F:00001234 R:30000/1001 A:T4X16G2K21 T:00:00:41.04"
        );
        let stamp = parse_payload(&text).unwrap();
        assert_eq!(
            stamp,
            QrStamp {
                frame_id: 1234,
                fps,
                tone_base_harmonic: Some(21)
            }
        );
        assert_eq!(parse_payload("hello"), None);
        assert_eq!(parse_payload("AVD:2 F:12"), None);
    }

    #[test]
    fn payload_is_alphanumeric_and_fits_version_4q() {
        let fps: FrameRate = "119.88".parse().unwrap();
        let text = payload(
            16_777_215,
            fps,
            &tones(fps),
            &Timecode::from_frame(16_777_215, fps),
        );
        let charset = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:";
        assert!(text.chars().all(|c| charset.contains(c)), "{text}");
        let code = QrCode::with_error_correction_level(&text, EcLevel::Q).unwrap();
        assert_eq!(code.width(), 33, "expected a version 4 code for {text}");
    }

    #[cfg(feature = "qr-decode")]
    #[test]
    fn draw_and_read() {
        let fps: FrameRate = "60".parse().unwrap();
        let mut img = LumaImage::new(640, 360, BLACK);
        let id = 9_876_543;
        let text = payload(
            id,
            fps,
            &tones(fps),
            &Timecode::from_frame(u64::from(id), fps),
        );
        draw(&mut img.canvas(), Rect::new(380, 100, 240, 240), &text).unwrap();
        let expected = QrStamp {
            frame_id: id,
            fps,
            tone_base_harmonic: Some(10),
        };
        assert_eq!(read(&img.view()), Some(expected));
    }

    #[cfg(feature = "qr-decode")]
    #[test]
    fn reads_large_frames_via_downscale() {
        let fps: FrameRate = "25".parse().unwrap();
        let mut img = LumaImage::new(3840, 2160, BLACK);
        let text = payload(77, fps, &tones(fps), &Timecode::from_frame(77, fps));
        draw(&mut img.canvas(), Rect::new(2000, 600, 900, 900), &text).unwrap();
        assert_eq!(read(&img.view()).map(|s| s.frame_id), Some(77));
    }
}
