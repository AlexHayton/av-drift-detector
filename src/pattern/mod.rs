//! The visual side of the test pattern: rendering frames and reading stamps back.

pub mod barcode;
pub mod canvas;
pub mod layout;
pub mod qr;
pub mod timecode;

use serde::Serialize;

use self::canvas::{BLACK, GREY, LumaCanvas, LumaImage, LumaView, Rect, WHITE};
pub use self::layout::Layout;
use self::timecode::Timecode;
use crate::audio::ToneCode;
use crate::{FrameRate, Result};

/// Renders test-pattern frames straight into a luma plane.
///
/// Everything that never changes (border, guard cells, progress-bar slots) is
/// drawn once into a cached background; each frame copies it and draws only
/// the changing elements.
pub struct PatternRenderer {
    fps: FrameRate,
    tones: ToneCode,
    layout: Layout,
    background: LumaImage,
}

impl PatternRenderer {
    /// A renderer for frames of `width`×`height`. The QR code describes `tones`,
    /// the audio plan the file is generated with.
    pub fn new(width: usize, height: usize, tones: ToneCode) -> Result<Self> {
        let fps = tones.fps();
        let layout = Layout::new(width, height)?;
        let mut background = LumaImage::new(width, height, BLACK);
        {
            let mut c = background.canvas();
            c.stroke_rect(
                Rect::new(0, 0, width as i32, height as i32),
                layout.border,
                WHITE,
            );
            barcode::draw_guards(&mut c);
            for slot in 0..fps.nominal() {
                c.fill_rect(progress_slot(&layout, fps, slot), GREY);
            }
        }
        Ok(PatternRenderer {
            fps,
            tones,
            layout,
            background,
        })
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
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

        qr::draw(
            canvas,
            l.qr,
            &qr::payload(frame_id, self.fps, &self.tones, &tc),
        )
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
#[serde(rename_all = "lowercase")]
pub enum StampSource {
    Barcode,
    Qr,
}

/// A frame stamp decoded from a video frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameStamp {
    pub frame_id: u32,
    /// The pattern's frame rate, if it has a barcode code (or came from the QR code).
    pub fps: Option<FrameRate>,
    /// The audio tone plan's base harmonic (QR codes only).
    pub tone_base_harmonic: Option<u32>,
    pub source: StampSource,
}

/// Reads the frame stamp from a luma frame: the barcode first, then (if
/// `try_qr` and the `qr-decode` feature is enabled) the QR code.
pub fn decode_frame(view: &LumaView<'_>, try_qr: bool) -> Option<FrameStamp> {
    if let Ok(stamp) = barcode::read(view) {
        return Some(FrameStamp {
            frame_id: stamp.frame_id,
            fps: stamp.fps,
            tone_base_harmonic: None,
            source: StampSource::Barcode,
        });
    }
    #[cfg(feature = "qr-decode")]
    if try_qr && let Some(stamp) = qr::read(view) {
        return Some(FrameStamp {
            frame_id: stamp.frame_id,
            fps: Some(stamp.fps),
            tone_base_harmonic: stamp.tone_base_harmonic,
            source: StampSource::Qr,
        });
    }
    let _ = try_qr;
    None
}

#[cfg(test)]
mod tests {
    use super::*;

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
