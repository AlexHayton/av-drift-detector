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
//! https://github.com/AlexHayton/av-drift-detector#AVD:4+F:00001234+R:C4+A:K21+S:1920X1080
//! ```
//!
//! | field | meaning |
//! |-------|---------|
//! | `AVD:4` | payload format version |
//! | `F` | frame number (zero-padded, so the code never changes size) |
//! | `R` | frame rate: `C` and the barcode's rate code for the built-in rates, or the exact rate (`num/den`) |
//! | `A` | audio tone plan (see [`ToneCode::descriptor`]) |
//! | `S` | the test file's frame size, which fixes where everything else on the frame is |
//!
//! The fragment uses only QR "alphanumeric" characters (upper case, digits,
//! `:` `/` `+`), which pack about 45% denser than bytes. Error-correction
//! level M (15% of the code can be lost) keeps it to version 5 (37×37
//! modules) for every built-in rate, so the modules stay large enough to read
//! from across a room. (A custom rate's `num/den` takes version 6.)
//!
//! Only this version is read. Codes from earlier releases (`AVD:2` and
//! `AVD:3`) are recognised, so that a capture of an old file gets a clear
//! error, but not decoded.

use qrcode::{Color, EcLevel, QrCode};

use super::barcode;
use super::canvas::{BLACK, LumaCanvas, LumaImage, LumaView, Rect, WHITE};
use crate::audio::ToneCode;
use crate::{Error, FrameRate, Result};

/// Where a scanned code leads: the project's page.
pub const URL: &str = "https://github.com/AlexHayton/av-drift-detector";
const PREFIX: &str = "AVD:4";
/// Payload versions from earlier releases, recognised but not read.
const OLD_PREFIXES: [&str; 2] = ["AVD:3", "AVD:2"];
const QUIET_ZONE: usize = 4;
const EC_LEVEL: EcLevel = EcLevel::M;

/// The stamp recovered from a QR code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QrStamp {
    pub frame_id: u32,
    pub fps: FrameRate,
    /// Base harmonic of the audio tone plan.
    pub tone_base_harmonic: u32,
    /// The test file's frame size, `(width, height)`.
    pub frame_size: (u32, u32),
}

/// Builds the QR payload for one frame of a `frame_size` file: the project
/// URL, with the stamp as its fragment.
pub fn payload(frame_id: u32, fps: FrameRate, tones: &ToneCode, frame_size: (u32, u32)) -> String {
    let rate = match barcode::fps_code(fps) {
        0 => fps.to_string(),
        code => format!("C{code}"),
    };
    format!(
        "{URL}#{PREFIX}+F:{frame_id:08}+R:{rate}+A:{}+S:{}X{}",
        tones.descriptor(),
        frame_size.0,
        frame_size.1
    )
}

/// Reads a payload: a URL (any URL) whose fragment is `AVD:4+F:...`.
pub fn parse_payload(text: &str) -> Option<QrStamp> {
    let (_, fragment) = text.split_once('#')?;
    let mut fields = fragment.split('+');
    if fields.next()? != PREFIX {
        return None;
    }
    let (mut frame_id, mut fps, mut tones, mut size) = (None, None, None, None);
    for field in fields {
        match field.split_once(':') {
            Some(("F", v)) => frame_id = v.parse().ok(),
            Some(("R", v)) => {
                fps = match v.strip_prefix('C') {
                    Some(code) => code.parse().ok().and_then(barcode::fps_from_code),
                    None => v.parse().ok(),
                }
            }
            Some(("A", v)) => tones = ToneCode::parse_descriptor(v).ok(),
            Some(("S", v)) => {
                size = v
                    .split_once('X')
                    .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
            }
            _ => {}
        }
    }
    Some(QrStamp {
        frame_id: frame_id?,
        fps: fps?,
        tone_base_harmonic: tones?,
        frame_size: size?,
    })
}

/// Whether `text` is a stamp from an earlier release's format.
pub fn is_old_payload(text: &str) -> bool {
    let stamp = text.split_once('#').map_or(text, |(_, fragment)| fragment);
    OLD_PREFIXES.iter().any(|p| {
        stamp
            .strip_prefix(p)
            .is_some_and(|rest| rest.starts_with(['+', ' ']))
    })
}

/// The payload for a decoded stamp: what the generator drew for that frame.
pub fn stamp_payload(stamp: &QrStamp) -> Result<String> {
    Ok(payload(
        stamp.frame_id,
        stamp.fps,
        &ToneCode::with_base_harmonic(stamp.fps, 48000, stamp.tone_base_harmonic)?,
        stamp.frame_size,
    ))
}

/// The QR code for `data`: modules per side, and whether each module is dark, row by row.
pub fn modules(data: &str) -> Result<(usize, Vec<bool>)> {
    let code = QrCode::with_error_correction_level(data, EC_LEVEL)
        .map_err(|e| Error::Qr(e.to_string()))?;
    let dark = code
        .to_colors()
        .into_iter()
        .map(|c| c == Color::Dark)
        .collect();
    Ok((code.width(), dark))
}

/// Where [`draw`] puts a code of `modules` per side in `area`: the frame pixel
/// of the code's top-left corner (module (0, 0), inside the quiet zone), the
/// module size in pixels, and the white square's bounds.
pub fn placement(area: Rect, modules: usize) -> Option<((i32, i32), i32, Rect)> {
    let span = modules + 2 * QUIET_ZONE;
    let module = (area.width().min(area.height()) as usize / span) as i32;
    if module == 0 {
        return None;
    }
    let side = module * span as i32;
    let x0 = area.x0 + (area.width() - side) / 2;
    let y0 = area.y0 + (area.height() - side) / 2;
    let origin = QUIET_ZONE as i32 * module;
    Some((
        (x0 + origin, y0 + origin),
        module,
        Rect::new(x0, y0, side, side),
    ))
}

/// Draws the QR code for `data`, centred in `area` with a white quiet zone.
pub fn draw(canvas: &mut LumaCanvas<'_>, area: Rect, data: &str) -> Result<()> {
    let (modules, dark) = self::modules(data)?;
    let ((ox, oy), module, square) = placement(area, modules)
        .ok_or_else(|| Error::Qr("frame too small for the QR code".into()))?;
    canvas.fill_rect(square, WHITE);
    for (i, is_dark) in dark.into_iter().enumerate() {
        if is_dark {
            let (mx, my) = ((i % modules) as i32, (i / modules) as i32);
            canvas.fill_rect(
                Rect::new(ox + mx * module, oy + my * module, module, module),
                BLACK,
            );
        }
    }
    Ok(())
}

/// Frames wider than this are box-downscaled before the QR search, which is
/// much faster. It's only used to *find* a code: rqrr's binarisation adapts
/// over an eighth of the searched image's width, so a code that fills a small
/// part of a big, busy frame often fails to decode there. Anything found but
/// not decoded is searched again at native resolution, in a region about
/// twice its size.
#[cfg(feature = "qr-decode")]
const MAX_SEARCH_WIDTH: usize = 1280;

/// How far a refined search region reaches beyond a grid's bounds, as a
/// fraction of the grid's side on each side.
#[cfg(feature = "qr-decode")]
pub const REGION_MARGIN: f32 = 0.5;

/// Grids found but not decoded that are searched again at native resolution.
#[cfg(feature = "qr-decode")]
const MAX_REFINED: usize = 3;

/// Searches the whole frame for an av-drift-detector QR code.
#[cfg(feature = "qr-decode")]
pub fn read(view: &LumaView<'_>) -> Option<QrStamp> {
    locate(view, None).map(|found| found.stamp)
}

/// Where a QR code's grid lies in a frame, decoded or not: its four corners
/// in frame pixels, in the code's own order (top left, top right, bottom
/// right, bottom left, as printed). Accurate to about a module: the far
/// corners can overshoot by one.
#[cfg(feature = "qr-decode")]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QrGrid {
    pub corners: [(f32, f32); 4],
    /// Modules per side, as the detector judged them (37 for version 5).
    pub modules: usize,
}

#[cfg(feature = "qr-decode")]
impl QrGrid {
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

    /// The mean length of the four edges, in pixels.
    pub fn side(&self) -> f32 {
        (0..4)
            .map(|i| {
                let (a, b) = (self.corners[i], self.corners[(i + 1) % 4]);
                ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
            })
            .sum::<f32>()
            / 4.0
    }

    /// The bounds grown by `margin` × the side on each side: the region to
    /// search for this code in the next frame, or at native resolution.
    pub fn region(&self, margin: f32) -> Rect {
        let b = self.bounds();
        let m = (margin * self.side()).ceil() as i32;
        Rect {
            x0: b.x0 - m,
            y0: b.y0 - m,
            x1: b.x1 + m,
            y1: b.y1 + m,
        }
    }
}

/// A QR stamp and where in the frame it was found.
#[cfg(feature = "qr-decode")]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocatedQr {
    pub stamp: QrStamp,
    /// The code's four corners in frame pixels; see [`QrGrid::corners`].
    pub corners: [(f32, f32); 4],
    /// Modules per side.
    pub modules: usize,
}

#[cfg(feature = "qr-decode")]
impl LocatedQr {
    pub fn grid(&self) -> QrGrid {
        QrGrid {
            corners: self.corners,
            modules: self.modules,
        }
    }

    /// The middle of the code, in frame pixels.
    pub fn centre(&self) -> (f32, f32) {
        self.grid().centre()
    }

    /// The smallest pixel rectangle holding all four corners.
    pub fn bounds(&self) -> Rect {
        self.grid().bounds()
    }
}

/// What a QR search found: the decoded code, if any, and every grid that
/// looked like a QR code but didn't decode (a blurred or blended frame, say).
#[cfg(feature = "qr-decode")]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QrSearch {
    pub found: Option<LocatedQr>,
    pub undecoded: Vec<QrGrid>,
    /// A code from an earlier release's format was decoded (and ignored).
    pub old_format: bool,
}

/// Searches `roi` (the whole frame if `None`) for an av-drift-detector QR
/// code, and reports where it is. Searching a small region around where the
/// code was last seen is much faster than searching the whole frame.
#[cfg(feature = "qr-decode")]
pub fn locate(view: &LumaView<'_>, roi: Option<Rect>) -> Option<LocatedQr> {
    search(view, roi).found
}

/// Searches `roi` (the whole frame if `None`) for an av-drift-detector QR
/// code. A region wider than 1280 pixels is searched downscaled first; grids
/// found there that don't decode are searched again at native resolution,
/// then with their contrast stretched.
#[cfg(feature = "qr-decode")]
pub fn search(view: &LumaView<'_>, roi: Option<Rect>) -> QrSearch {
    search_with(view, roi, Effort::Decode)
}

/// How hard [`search_with`] tries.
#[cfg(feature = "qr-decode")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effort {
    /// Everything [`search`] does, to decode the code if at all possible.
    Decode,
    /// Enough to find where the code is: at half resolution or less, with no
    /// retries. For a caller that reads codes another way (by fitting them),
    /// it's several times faster.
    Locate,
}

/// [`search`], with a choice of [`Effort`].
#[cfg(feature = "qr-decode")]
pub fn search_with(view: &LumaView<'_>, roi: Option<Rect>, effort: Effort) -> QrSearch {
    let (area, x0, y0) = match roi {
        Some(r) => (
            view.crop(r),
            r.x0.clamp(0, view.width() as i32),
            r.y0.clamp(0, view.height() as i32),
        ),
        None => (*view, 0, 0),
    };
    if area.width() == 0 || area.height() == 0 {
        return QrSearch::default();
    }
    let mut factor = area.width().div_ceil(MAX_SEARCH_WIDTH);
    if effort == Effort::Locate {
        factor = factor.max(2);
    }
    let at = |factor: usize| {
        if factor > 1 {
            search_area(&downscale(&area, factor).view(), x0, y0, factor)
        } else {
            search_area(&area, x0, y0, 1)
        }
    };
    let mut result = at(factor);
    // A small code in a big frame can vanish in a heavy downscale: try
    // finer once, if nothing at all turned up.
    if effort == Effort::Decode
        && factor >= 3
        && result.found.is_none()
        && result.undecoded.is_empty()
    {
        result = at(factor.div_ceil(2));
        factor = factor.div_ceil(2);
    }
    if result.found.is_some() || effort == Effort::Locate {
        return result;
    }
    // Native resolution around each grid found, then with stretched contrast.
    let grids = std::mem::take(&mut result.undecoded);
    for grid in grids.iter().take(MAX_REFINED) {
        let region = grid.region(REGION_MARGIN);
        let crop = view.crop(region);
        let (rx, ry) = (
            region.x0.clamp(0, view.width() as i32),
            region.y0.clamp(0, view.height() as i32),
        );
        let refined = if factor > 1 {
            search_area(&crop, rx, ry, 1)
        } else {
            QrSearch::default()
        };
        let refined = match refined.found {
            Some(_) => refined,
            None => search_area(&stretch_contrast(&crop).view(), rx, ry, 1),
        };
        result.old_format |= refined.old_format;
        if let Some(found) = refined.found {
            return QrSearch {
                found: Some(found),
                ..Default::default()
            };
        }
        // Keep the most precise outline of the grid for whoever comes next.
        result
            .undecoded
            .push(refined.undecoded.first().copied().unwrap_or(*grid));
    }
    result.undecoded.extend(grids.into_iter().skip(MAX_REFINED));
    result
}

/// Runs rqrr over `view`, whose pixel `(x, y)` is frame pixel
/// `(x0 + factor·x, y0 + factor·y)`, and maps what it finds back to the frame.
#[cfg(feature = "qr-decode")]
fn search_area(view: &LumaView<'_>, x0: i32, y0: i32, factor: usize) -> QrSearch {
    // Pixel centres of a downscaled image back in the full frame.
    let scale = |v: i32, origin: i32| origin as f32 + (v as f32 + 0.5) * factor as f32 - 0.5;
    let mut img =
        rqrr::PreparedImage::prepare_from_greyscale(view.width(), view.height(), |x, y| {
            view.get(x, y)
        });
    let mut result = QrSearch::default();
    for grid in img.detect_grids() {
        let corners = grid.bounds.map(|p| (scale(p.x, x0), scale(p.y, y0)));
        let modules = rqrr::BitGrid::size(&grid.grid);
        match grid.decode() {
            Ok((_, text)) => {
                result.old_format |= is_old_payload(&text);
                // Another QR code in the picture isn't ours, and isn't a lead.
                if let Some(stamp) = parse_payload(&text) {
                    result.found = Some(LocatedQr {
                        stamp,
                        corners,
                        modules,
                    });
                    return result;
                }
            }
            Err(_) => result.undecoded.push(QrGrid { corners, modules }),
        }
    }
    result
}

/// A copy of `view` with its 2nd to 98th percentile of luma stretched to the full range.
#[cfg(feature = "qr-decode")]
fn stretch_contrast(view: &LumaView<'_>) -> LumaImage {
    let (w, h) = (view.width(), view.height());
    let mut histogram = [0usize; 256];
    for y in 0..h {
        for x in 0..w {
            histogram[usize::from(view.get(x, y))] += 1;
        }
    }
    let count = w * h;
    let percentile = |p: usize| {
        let target = count * p / 100;
        let mut seen = 0;
        histogram
            .iter()
            .position(|&n| {
                seen += n;
                seen > target
            })
            .unwrap_or(255) as i32
    };
    let (lo, hi) = (percentile(2), percentile(98));
    let span = (hi - lo).max(1);
    let mut out = LumaImage::new(w, h, 0);
    for y in 0..h {
        for x in 0..w {
            let v = (i32::from(view.get(x, y)) - lo) * 255 / span;
            out.data[y * w + x] = v.clamp(0, 255) as u8;
        }
    }
    out
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
        let text = payload(1234, fps, &tones(fps), (1920, 1080));
        assert_eq!(
            text,
            "https://github.com/AlexHayton/av-drift-detector#AVD:4+F:00001234+R:C4+A:K21+S:1920X1080"
        );
        let expected = QrStamp {
            frame_id: 1234,
            fps,
            tone_base_harmonic: 21,
            frame_size: (1920, 1080),
        };
        assert_eq!(parse_payload(&text), Some(expected));
        // Any URL, as long as the fragment is a stamp.
        assert_eq!(
            parse_payload("https://example.com/x#AVD:4+F:00001234+R:C4+A:K21+S:1920X1080"),
            Some(expected)
        );
        // A rate without a code is written out.
        let custom = FrameRate::new(15, 1).unwrap();
        let text = payload(7, custom, &tones(custom), (640, 360));
        assert!(
            text.ends_with("#AVD:4+F:00000007+R:15+A:K40+S:640X360"),
            "{text}"
        );
        assert_eq!(parse_payload(&text).map(|s| s.fps), Some(custom));
        // Incomplete and foreign codes.
        assert_eq!(parse_payload("hello"), None);
        assert_eq!(parse_payload(URL), None);
        assert_eq!(parse_payload("https://a.b/#AVD:4+F:1+R:C5+A:K20"), None);
        assert_eq!(
            parse_payload("https://a.b/#AVD:4+F:1+R:C99+A:K20+S:1X1"),
            None
        );
    }

    #[test]
    fn earlier_formats_are_recognised_but_not_read() {
        for old in [
            "https://github.com/AlexHayton/av-drift-detector#AVD:3+F:00001234+R:30000/1001+A:T4X16G2K21",
            "AVD:2 F:00001234 R:30000/1001 A:T4X16G2K21 T:00:00:41.04",
        ] {
            assert_eq!(parse_payload(old), None, "{old}");
            assert!(is_old_payload(old), "{old}");
        }
        let fps: FrameRate = "30".parse().unwrap();
        assert!(!is_old_payload(&payload(1, fps, &tones(fps), (1280, 720))));
        assert!(!is_old_payload("https://example.com/"));
    }

    #[test]
    fn every_rate_fits_one_version_5_code() {
        for code in 1..=12 {
            let fps = super::super::barcode::fps_from_code(code).unwrap();
            for id in [0, 1234, 16_777_215] {
                for size in [(320, 180), (1920, 1080), (7680, 4320)] {
                    let text = payload(id, fps, &tones(fps), size);
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
    }

    #[cfg(feature = "qr-decode")]
    #[test]
    fn draw_and_read() {
        let fps: FrameRate = "60".parse().unwrap();
        let mut img = LumaImage::new(640, 360, BLACK);
        let id = 9_876_543;
        let text = payload(id, fps, &tones(fps), (640, 360));
        draw(&mut img.canvas(), Rect::new(380, 100, 240, 240), &text).unwrap();
        let expected = QrStamp {
            frame_id: id,
            fps,
            tone_base_harmonic: 10,
            frame_size: (640, 360),
        };
        assert_eq!(read(&img.view()), Some(expected));
    }

    #[cfg(feature = "qr-decode")]
    #[test]
    fn locates_the_code_in_a_region() {
        let fps: FrameRate = "30".parse().unwrap();
        let mut img = LumaImage::new(1280, 720, BLACK);
        let text = payload(42, fps, &tones(fps), (1280, 720));
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
        let text = payload(77, fps, &tones(fps), (3840, 2160));
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
