//! The visual side of the test pattern: rendering frames and reading stamps back.

pub mod barcode;
mod blend;
pub mod canvas;
mod dancer;
mod font;
pub mod layout;
mod paint;
pub mod qr;
#[cfg(feature = "qr-decode")]
pub mod qr_fit;
mod reader;
pub mod timecode;

use serde::Serialize;

pub use self::blend::Blend;
use self::canvas::{BLACK, GREY, LumaCanvas, LumaImage, LumaView, Rect, WHITE};
use self::dancer::Dancer;
pub use self::layout::Layout;
pub use self::reader::StampReader;
use self::timecode::Timecode;
use crate::audio::ToneCode;
use crate::{FrameRate, Result};

/// Renders test-pattern frames straight into a luma plane.
///
/// Everything that never changes (border, guard cells, progress-bar slots,
/// the info line) is drawn once into a cached background; each frame copies
/// it and draws only the changing elements. A stick figure dances on the
/// stage below the text, in an 8-second loop, where there's room.
pub struct PatternRenderer {
    fps: FrameRate,
    tones: ToneCode,
    layout: Layout,
    total_frames: Option<u64>,
    background: LumaImage,
    dancer: Option<Dancer>,
}

impl PatternRenderer {
    /// A renderer for frames of `width`×`height`. The QR code describes `tones`,
    /// the audio plan the file is generated with.
    pub fn new(width: usize, height: usize, tones: ToneCode) -> Result<Self> {
        let fps = tones.fps();
        let layout = Layout::new(width, height)?;
        let dancer = Dancer::new(layout.stage, fps);
        let mut renderer = PatternRenderer {
            fps,
            tones,
            layout,
            total_frames: None,
            background: LumaImage::new(0, 0, BLACK),
            dancer,
        };
        renderer.background = renderer.draw_background();
        Ok(renderer)
    }

    /// Shows the length of the file, `frames` long, in the info line, after
    /// the frame size and rate.
    pub fn with_total_frames(mut self, frames: u64) -> Self {
        self.total_frames = Some(frames);
        self.background = self.draw_background();
        self
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// The info line: frame size, rate and (if known) length, such as
    /// `1280×720 · 29.97 fps · 6.006 s`.
    pub fn info(&self) -> String {
        let l = &self.layout;
        let mut text = format!("{}×{} · {} fps", l.width, l.height, self.fps.label());
        if let Some(frames) = self.total_frames {
            text += " · ";
            text += &duration_label(frames as f64 * self.fps.frame_duration());
        }
        text
    }

    fn draw_background(&self) -> LumaImage {
        let l = &self.layout;
        let mut background = LumaImage::new(l.width, l.height, BLACK);
        let mut c = background.canvas();
        c.stroke_rect(
            Rect::new(0, 0, l.width as i32, l.height as i32),
            l.border,
            WHITE,
        );
        barcode::draw_guards(&mut c);
        for slot in 0..self.fps.nominal() {
            c.fill_rect(progress_slot(l, self.fps, slot), GREY);
        }
        // The full info line at the largest scale that fits, or a shorter one.
        let full = self.info();
        let short = full
            .replace(" · ", " ")
            .replace(" fps", "fps")
            .replace(" s", "s");
        let box_ = l.info;
        let fit = [full, short].into_iter().find_map(|text| {
            let max_scale = box_.height() / font::ROWS;
            (1..=max_scale)
                .rev()
                .find(|&s| font::text_width(&text, s) <= box_.width())
                .map(|s| (text, s))
        });
        let info_scale = fit.as_ref().map_or(1, |f| f.1);
        if let Some((text, scale)) = fit {
            font::draw_text(&mut c, box_.x0, box_.y0, scale, &text, WHITE);
        }
        // The project's address along the bottom, no larger than the info line.
        let link = l.link;
        let text = link_text();
        let scale = (1..=(link.height() / font::ROWS).min(info_scale))
            .rev()
            .find(|&s| font::text_width(text, s) <= link.width());
        if let Some(scale) = scale {
            let y = link.y0 + (link.height() - font::ROWS * scale) / 2;
            font::draw_text(&mut c, link.x0, y, scale, text, LINK_GREY);
        }
        background
    }

    /// Draws frame `frame_id` onto `canvas`, which must match the renderer's size.
    pub fn render(&self, frame_id: u32, canvas: &mut LumaCanvas<'_>) -> Result<()> {
        let l = &self.layout;
        canvas.copy_from(&self.background.view());
        barcode::draw(canvas, barcode::pack(frame_id, self.fps));

        let tc = Timecode::from_frame(u64::from(frame_id), self.fps);
        let t = l.timecode;
        timecode::draw_text(canvas, t.x, t.y, t.height, &tc.to_string(), WHITE);
        let n = l.frame_number;
        timecode::draw_text(
            canvas,
            n.x,
            n.y,
            n.height,
            &format!("F {frame_id:08}"),
            WHITE,
        );

        let slot = frame_id % self.fps.nominal();
        canvas.fill_rect(progress_slot(l, self.fps, slot), WHITE);

        if let Some(dancer) = &self.dancer {
            dancer.draw(frame_id, canvas);
        }
        let size = (l.width as u32, l.height as u32);
        qr::draw(
            canvas,
            l.qr,
            &qr::payload(frame_id, self.fps, &self.tones, size),
        )
    }
}

/// Grey for the project's address: readable, but quieter than the white text.
const LINK_GREY: u8 = 150;

/// The project's address as drawn along the bottom of each frame: the QR
/// code's link without its scheme.
pub fn link_text() -> &'static str {
    qr::URL.trim_start_matches("https://")
}

/// A length in seconds for people, to the millisecond without trailing
/// zeros: `6 s`, `6.006 s`, `1:30`, `1:00:00.5`.
fn duration_label(seconds: f64) -> String {
    let ms = (seconds * 1000.0).round() as u64;
    let s = ms / 1000;
    let frac = format!(".{:03}", ms % 1000);
    let frac = frac.trim_end_matches('0').trim_end_matches('.');
    match s {
        0..60 => format!("{s}{frac} s"),
        60..3600 => format!("{}:{:02}{frac}", s / 60, s % 60),
        _ => format!("{}:{:02}:{:02}{frac}", s / 3600, s / 60 % 60, s % 60),
    }
}

fn progress_slot(layout: &Layout, fps: FrameRate, slot: u32) -> Rect {
    let p = layout.progress;
    let n = f64::from(fps.nominal());
    let x0 = f64::from(p.x0) + f64::from(p.width()) * f64::from(slot) / n;
    let x1 = f64::from(p.x0) + f64::from(p.width()) * f64::from(slot + 1) / n;
    let gap = i32::from(x1 - x0 >= 4.0);
    Rect {
        x0: x0.round() as i32,
        y0: p.y0,
        x1: x1.round() as i32 - gap,
        y1: p.y1,
    }
}

/// Which visual element a stamp was read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StampSource {
    Barcode,
    /// The QR code, decoded.
    Qr,
    /// The QR code, read by fitting it as a mix of two consecutive frames'
    /// codes (see `qr_fit`): a code blended between two frames or too blurred
    /// to decode, or one tracked from where the last frame had it.
    QrBlend,
    /// The barcode, too blended between two frames to pass its CRC, read by
    /// fitting it as a mix of two frames' words.
    BarcodeBlend,
}

impl StampSource {
    /// Whether the stamp came from the QR code, decoded or fitted.
    pub fn is_qr(self) -> bool {
        matches!(self, StampSource::Qr | StampSource::QrBlend)
    }

    /// The name used in reports: `barcode`, `barcode_blend`, `qr` or `qr_blend`.
    pub fn name(self) -> &'static str {
        match self {
            StampSource::Barcode => "barcode",
            StampSource::Qr => "qr",
            StampSource::QrBlend => "qr_blend",
            StampSource::BarcodeBlend => "barcode_blend",
        }
    }
}

impl std::str::FromStr for StampSource {
    type Err = crate::Error;

    /// `barcode`, `barcode_blend`, `qr` or `qr_blend`.
    fn from_str(s: &str) -> crate::Result<Self> {
        match s {
            "barcode" => Ok(StampSource::Barcode),
            "qr" => Ok(StampSource::Qr),
            "qr_blend" => Ok(StampSource::QrBlend),
            "barcode_blend" => Ok(StampSource::BarcodeBlend),
            _ => Err(crate::Error::InvalidOption(format!(
                "stamp source must be barcode, barcode_blend, qr or qr_blend, not `{s}`"
            ))),
        }
    }
}

/// A frame stamp decoded from a video frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameStamp {
    pub frame_id: u32,
    /// The pattern's frame rate, if it has a barcode code (or came from the QR code).
    pub fps: Option<FrameRate>,
    /// The audio tone plan's base harmonic (QR codes only).
    pub tone_base_harmonic: Option<u32>,
    pub source: StampSource,
    /// Where the QR code's corners were, in frame pixels (QR codes only):
    /// top left, top right, bottom right, bottom left, as printed.
    pub corners: Option<[(f32, f32); 4]>,
    /// How the picture mixed two consecutive frames, where that could be
    /// measured (QR codes read in a sequence of frames).
    pub blend: Option<Blend>,
}

impl FrameStamp {
    #[cfg(feature = "qr-decode")]
    fn from_qr(found: &qr::LocatedQr) -> Self {
        FrameStamp {
            frame_id: found.stamp.frame_id,
            fps: Some(found.stamp.fps),
            tone_base_harmonic: Some(found.stamp.tone_base_harmonic),
            source: StampSource::Qr,
            corners: Some(found.corners),
            blend: None,
        }
    }
}

/// Reads the frame stamp from a single luma frame: the barcode first, then
/// (if `try_qr` and the `qr-decode` feature is enabled) the QR code, searched
/// for across the whole frame. To read consecutive frames of a capture, a
/// [`StampReader`] is faster and finds more.
pub fn decode_frame(view: &LumaView<'_>, try_qr: bool) -> Option<FrameStamp> {
    StampReader::new(try_qr).read(view, 0.0).stamp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_show_the_project_address() {
        assert_eq!(link_text(), "github.com/AlexHayton/av-drift-detector");
        for (w, h, drawn) in [
            (1280, 720, true),
            (1080, 1920, true),
            (1080, 1080, true),
            (320, 180, false),
        ] {
            let tones = ToneCode::new("30".parse().unwrap(), 48000).unwrap();
            let renderer = PatternRenderer::new(w, h, tones).unwrap();
            let mut img = LumaImage::new(w, h, 0);
            renderer.render(0, &mut img.canvas()).unwrap();
            let link = renderer.layout().link;
            let lit = (link.y0.max(0)..link.y1)
                .flat_map(|y| (link.x0..link.x1).map(move |x| (x as usize, y as usize)))
                .filter(|&(x, y)| img.view().get(x, y) == LINK_GREY)
                .count();
            assert_eq!(lit > 0, drawn, "{w}x{h}: {lit} pixels of the link drawn");
            // Small: no taller than the info line's text.
            let rows = (link.y0..link.y1)
                .filter(|&y| {
                    (link.x0..link.x1).any(|x| img.view().get(x as usize, y as usize) == LINK_GREY)
                })
                .count() as i32;
            assert!(
                rows <= renderer.layout().info.height(),
                "{w}x{h}: {rows} rows"
            );
        }
    }

    #[test]
    fn rendered_frames_decode() {
        for fps in ["30", "29.97", "120", "15"] {
            let fps: FrameRate = fps.parse().unwrap();
            let renderer =
                PatternRenderer::new(640, 360, ToneCode::new(fps, 48000).unwrap()).unwrap();
            let mut img = LumaImage::new(640, 360, 0);
            for id in [0, 1, 59, 60, 123_456] {
                renderer.render(id, &mut img.canvas()).unwrap();
                let stamp = decode_frame(&img.view(), false).unwrap();
                assert_eq!(stamp.frame_id, id);
                assert_eq!(stamp.source, StampSource::Barcode);
                let expected = (barcode::fps_code(fps) != 0).then_some(fps);
                assert_eq!(stamp.fps, expected);
            }
        }
    }

    #[cfg(feature = "qr-decode")]
    #[test]
    fn falls_back_to_qr_when_barcode_is_damaged() {
        let fps: FrameRate = "25".parse().unwrap();
        let renderer = PatternRenderer::new(1280, 720, ToneCode::new(fps, 48000).unwrap()).unwrap();
        let mut img = LumaImage::new(1280, 720, 0);
        renderer.render(4242, &mut img.canvas()).unwrap();
        // Wipe the top of the frame, taking out the barcode strip.
        img.canvas().fill_rect(Rect::new(0, 0, 1280, 200), BLACK);
        assert_eq!(decode_frame(&img.view(), false), None);
        let stamp = decode_frame(&img.view(), true).unwrap();
        assert_eq!(
            (stamp.frame_id, stamp.fps, stamp.source),
            (4242, Some(fps), StampSource::Qr)
        );
        assert_eq!(stamp.tone_base_harmonic, Some(24));
    }
}
