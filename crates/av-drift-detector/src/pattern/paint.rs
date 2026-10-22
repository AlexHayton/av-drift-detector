//! Antialiased shapes from signed distance functions, for the dancer.

use super::canvas::{LumaCanvas, Rect};

/// Paints shapes onto a canvas, clipped to a rectangle.
pub(crate) struct Painter<'c, 'a> {
    canvas: &'c mut LumaCanvas<'a>,
    clip: Rect,
}

impl<'c, 'a> Painter<'c, 'a> {
    pub fn new(canvas: &'c mut LumaCanvas<'a>, clip: Rect) -> Self {
        let clip = Rect {
            x0: clip.x0.max(0),
            y0: clip.y0.max(0),
            x1: clip.x1.min(canvas.width() as i32),
            y1: clip.y1.min(canvas.height() as i32),
        };
        Painter { canvas, clip }
    }

    /// Fills `rect` with `value`, with hard edges.
    pub fn rect(&mut self, rect: Rect, value: u8) {
        let c = self.clip;
        let clipped = Rect {
            x0: rect.x0.max(c.x0),
            y0: rect.y0.max(c.y0),
            x1: rect.x1.min(c.x1),
            y1: rect.y1.min(c.y1),
        };
        if clipped.x0 < clipped.x1 && clipped.y0 < clipped.y1 {
            self.canvas.fill_rect(clipped, value);
        }
    }

    /// Blends `value` into the shape whose signed distance (in pixels,
    /// negative inside) `sdf` gives at each pixel centre, with a one-pixel
    /// antialiased edge. Only pixels within `bounds` (`[x0, y0, x1, y1]`, in
    /// pixels) are visited, so the bounds must contain the whole shape.
    pub fn fill(
        &mut self,
        bounds: [f64; 4],
        value: f64,
        alpha: f64,
        sdf: impl Fn(f64, f64) -> f64,
    ) {
        if alpha <= 0.0 || bounds.iter().any(|b| !b.is_finite()) {
            return;
        }
        let c = self.clip;
        let x0 = (bounds[0].floor() as i32 - 1).max(c.x0);
        let y0 = (bounds[1].floor() as i32 - 1).max(c.y0);
        let x1 = (bounds[2].ceil() as i32 + 1).min(c.x1);
        let y1 = (bounds[3].ceil() as i32 + 1).min(c.y1);
        for y in y0..y1 {
            for x in x0..x1 {
                let coverage = (0.5 - sdf(f64::from(x) + 0.5, f64::from(y) + 0.5)).clamp(0.0, 1.0);
                if coverage > 0.0 {
                    self.canvas
                        .blend(x as usize, y as usize, value, coverage * alpha.min(1.0));
                }
            }
        }
    }
}
