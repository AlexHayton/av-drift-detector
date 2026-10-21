//! Per-frame QR code: a self-describing stamp that survives being filmed.
//!
//! The barcode strip needs a pixel-aligned capture. A QR code can be found
//! anywhere in a camera image, at any angle, so it carries everything an
//! analyser needs, without prior knowledge, to read a phone recording of a
//! screen. It's also a link: scanned with an ordinary phone camera, it opens
//! the project's page, with the stamp in the URL's fragment (which the page
//! ignores):
//!
//! ```text
//! https://github.com/AlexHayton/av-drift-detector#AVD:3+F:00001234+R:30000/1001+A:T4X16G2K21
//! ```
//!
//! | field | meaning |
//! |-------|---------|
//! | `AVD:3` | payload format version |
//! | `F` | frame number (zero-padded, so the code never changes size) |
//! | `R` | exact frame rate |
//! | `A` | audio tone plan (see [`ToneCode::descriptor`]) |
//!
//! The fragment uses only QR "alphanumeric" characters (upper case, digits,
//! `:` `/` `+`), which pack about 45% denser than bytes. Error-correction
//! level M (15% of the code can be lost) keeps it to version 5 (37×37
//! modules), so the modules stay large enough to read from across a room.
//!
//! Version 2 payloads (`AVD:2 F:00001234 R:30000/1001 A:T4X16G2K21 T:...`,
//! from earlier releases) are still read.

use qrcode::{Color, EcLevel, QrCode};

use super::canvas::{BLACK, LumaCanvas, LumaImage, LumaView, Rect, WHITE};
use crate::audio::ToneCode;
use crate::{Error, FrameRate, Result};

/// Where a scanned code leads: the project's page.
pub const URL: &str = "https://github.com/AlexHayton/av-drift-detector";
const PREFIX: &str = "AVD:3";
const PREFIX_V2: &str = "AVD:2";
const QUIET_ZONE: usize = 4;
const EC_LEVEL: EcLevel = EcLevel::M;

/// The stamp recovered from a QR code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QrStamp {
    pub frame_id: u32,
    pub fps: FrameRate,
    /// Base harmonic of the audio tone plan, if the code carried a supported one.
    pub tone_base_harmonic: Option<u32>,
}

/// Builds the QR payload for one frame: the project URL, with the stamp as its fragment.
pub fn payload(frame_id: u32, fps: FrameRate, tones: &ToneCode) -> String {
    format!(
        "{URL}#{PREFIX}+F:{frame_id:08}+R:{fps}+A:{}",
        tones.descriptor()
    )
}

/// Reads a payload of either format: version 3 (a URL whose fragment is
/// `AVD:3+F:...`, whatever the URL) or version 2 (`AVD:2 F:...`).
pub fn parse_payload(text: &str) -> Option<QrStamp> {
    let mut fields: Box<dyn Iterator<Item = &str>> = match text.split_once('#') {
        Some((_, fragment)) => Box::new(fragment.split('+')),
        None => Box::new(text.split(' ')),
    };
    let version = fields.next()?;
    let expected = if text.contains('#') {
        PREFIX
    } else {
        PREFIX_V2
    };
    if version != expected {
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
    let code = QrCode::with_error_correction_level(data, EC_LEVEL)
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
    locate(view, None).map(|found| found.stamp)
}

/// A QR stamp and where in the frame it was found.
#[cfg(feature = "qr-decode")]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocatedQr {
    pub stamp: QrStamp,
    /// The code's four corners in frame pixels, in the code's own order:
    /// top left, top right, bottom right, bottom left (as printed). Accurate
    /// to about a module: the far corners can overshoot by one.
    pub corners: [(f32, f32); 4],
}

#[cfg(feature = "qr-decode")]
impl LocatedQr {
    /// The middle of the code, in frame pixels.
    pub fn centre(&self) -> (f32, f32) {
        let (sx, sy) = self
            .corners
            .iter()
            .fold((0.0, 0.0), |(sx, sy), &(x, y)| (sx + x, sy + y));
        (sx / 4.0, sy / 4.0)
    }

    /// The smallest pixel rectangle holding all four corners.
    pub fn bounds(&self) -> Rect {
        let xs = self.corners.map(|c| c.0);
        let ys = self.corners.map(|c| c.1);
        let min = |v: [f32; 4]| v.into_iter().fold(f32::INFINITY, f32::min).floor() as i32;
        let max = |v: [f32; 4]| v.into_iter().fold(f32::NEG_INFINITY, f32::max).ceil() as i32;
        Rect {
            x0: min(xs),
            y0: min(ys),
            x1: max(xs) + 1,
            y1: max(ys) + 1,
        }
    }
}

/// Searches `roi` (the whole frame if `None`) for an av-drift-detector QR
/// code, and reports where it is. Searching a small region around where the
/// code was last seen is much faster than searching the whole frame.
#[cfg(feature = "qr-decode")]
pub fn locate(view: &LumaView<'_>, roi: Option<Rect>) -> Option<LocatedQr> {
    let (area, x0, y0) = match roi {
        Some(r) => (
            view.crop(r),
            r.x0.clamp(0, view.width() as i32),
            r.y0.clamp(0, view.height() as i32),
        ),
        None => (*view, 0, 0),
    };
    if area.width() == 0 || area.height() == 0 {
        return None;
    }
    let factor = area.width().div_ceil(MAX_SEARCH_WIDTH);
    let (stamp, bounds) = if factor > 1 {
        search(&downscale(&area, factor).view())?
    } else {
        search(&area)?
    };
    // Pixel centres of the downscaled image back in the full frame.
    let scale = |v: i32, origin: i32| origin as f32 + (v as f32 + 0.5) * factor as f32 - 0.5;
    Some(LocatedQr {
        stamp,
        corners: bounds.map(|p| (scale(p.x, x0), scale(p.y, y0))),
    })
}

#[cfg(feature = "qr-decode")]
fn search(view: &LumaView<'_>) -> Option<(QrStamp, [rqrr::Point; 4])> {
    let mut img =
        rqrr::PreparedImage::prepare_from_greyscale(view.width(), view.height(), |x, y| {
            view.get(x, y)
        });
    img.detect_grids().into_iter().find_map(|grid| {
        let bounds = grid.bounds;
        let (_, text) = grid.decode().ok()?;
        Some((parse_payload(&text)?, bounds))
    })
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
        let text = payload(1234, fps, &tones(fps));
        assert_eq!(
            text,
            "https://github.com/AlexHayton/av-drift-detector#AVD:3+F:00001234+R:30000/1001+A:T4X16G2K21"
        );
        let expected = QrStamp {
            frame_id: 1234,
            fps,
            tone_base_harmonic: Some(21),
        };
        assert_eq!(parse_payload(&text), Some(expected));
        // Any URL, as long as the fragment is a stamp.
        assert_eq!(
            parse_payload("https://example.com/x#AVD:3+F:00001234+R:30000/1001+A:T4X16G2K21"),
            Some(expected)
        );
        // Version 2, from earlier releases.
        assert_eq!(
            parse_payload("AVD:2 F:00001234 R:30000/1001 A:T4X16G2K21 T:00:00:41.04"),
            Some(expected)
        );
        assert_eq!(parse_payload("hello"), None);
        assert_eq!(parse_payload(URL), None);
        assert_eq!(parse_payload("AVD:2 F:12"), None);
        assert_eq!(parse_payload("https://a.b/#AVD:2+F:1+R:30"), None);
        assert_eq!(parse_payload("https://a.b/#AVD:4+F:1+R:30"), None);
    }

    #[test]
    fn every_rate_fits_one_version_5_code() {
        for code in 1..=12 {
            let fps = super::super::barcode::fps_from_code(code).unwrap();
            for id in [0, 1234, 16_777_215] {
                let text = payload(id, fps, &tones(fps));
                let (url, fragment) = text.split_once('#').unwrap();
                assert_eq!(url, URL);
                // The stamp packs as QR alphanumeric characters.
                let charset = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:";
                assert!(fragment.chars().all(|c| charset.contains(c)), "{text}");
                let qr = QrCode::with_error_correction_level(&text, EC_LEVEL).unwrap();
                assert_eq!(qr.width(), 37, "expected a version 5 code for {text}");
            }
        }
    }

    #[cfg(feature = "qr-decode")]
    #[test]
    fn draw_and_read() {
        let fps: FrameRate = "60".parse().unwrap();
        let mut img = LumaImage::new(640, 360, BLACK);
        let id = 9_876_543;
        let text = payload(id, fps, &tones(fps));
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
    fn locates_the_code_in_a_region() {
        let fps: FrameRate = "30".parse().unwrap();
        let mut img = LumaImage::new(1280, 720, BLACK);
        let text = payload(42, fps, &tones(fps));
        let area = Rect::new(700, 200, 330, 330);
        draw(&mut img.canvas(), area, &text).unwrap();
        // 330 / (37 + 8) = 7-pixel modules. The 315-pixel square sits 7 pixels
        // in, and the code starts after a 28-pixel quiet zone.
        let (code_x0, code_y0) = (707.0 + 28.0, 207.0 + 28.0);
        let side = 37.0 * 7.0;
        for roi in [None, Some(Rect::new(600, 100, 600, 500))] {
            let found = locate(&img.view(), roi).unwrap();
            assert_eq!(found.stamp.frame_id, 42);
            let (cx, cy) = found.centre();
            // To within about a module.
            assert!(
                (cx - (code_x0 + side / 2.0)).abs() < 8.0
                    && (cy - (code_y0 + side / 2.0)).abs() < 8.0,
                "{roi:?}: centre {cx},{cy} for {found:?}"
            );
            let b = found.bounds();
            assert!((b.width() as f32 - side).abs() <= 16.0, "{b:?}");
        }
        // A region that misses the code finds nothing.
        assert_eq!(locate(&img.view(), Some(Rect::new(0, 0, 600, 720))), None);
        // Regions are clipped to the frame.
        assert!(locate(&img.view(), Some(Rect::new(500, -50, 2000, 2000))).is_some());
    }

    #[cfg(feature = "qr-decode")]
    #[test]
    fn reads_large_frames_via_downscale() {
        let fps: FrameRate = "25".parse().unwrap();
        let mut img = LumaImage::new(3840, 2160, BLACK);
        let text = payload(77, fps, &tones(fps));
        draw(&mut img.canvas(), Rect::new(2000, 600, 900, 900), &text).unwrap();
        assert_eq!(read(&img.view()).map(|s| s.frame_id), Some(77));
        // Corners come back in full-resolution pixels: 900 / 45 = 20-pixel modules.
        let (cx, cy) = locate(&img.view(), None).unwrap().centre();
        assert!(
            (cx - 2450.0).abs() < 21.0 && (cy - 1050.0).abs() < 21.0,
            "{cx},{cy}"
        );
    }
}
