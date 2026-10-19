//! Minimal drawing and sampling on 8-bit luma planes.

/// Luma levels for limited-range ("TV") video.
pub const BLACK: u8 = 16;
pub const WHITE: u8 = 235;
pub const GREY: u8 = 96;

/// A pixel rectangle with exclusive right/bottom edges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Rect {
            x0: x,
            y0: y,
            x1: x + w,
            y1: y + h,
        }
    }

    pub fn width(&self) -> i32 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> i32 {
        self.y1 - self.y0
    }

    /// Shrinks the rectangle by `d` pixels on every side.
    pub fn inset(&self, d: i32) -> Rect {
        Rect {
            x0: self.x0 + d,
            y0: self.y0 + d,
            x1: self.x1 - d,
            y1: self.y1 - d,
        }
    }
}

/// An owned luma image, used for the cached static background and in tests.
#[derive(Clone, Debug)]
pub struct LumaImage {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

impl LumaImage {
    pub fn new(width: usize, height: usize, value: u8) -> Self {
        LumaImage {
            width,
            height,
            data: vec![value; width * height],
        }
    }

    pub fn canvas(&mut self) -> LumaCanvas<'_> {
        LumaCanvas::new(&mut self.data, self.width, self.height, self.width)
    }

    pub fn view(&self) -> LumaView<'_> {
        LumaView::new(&self.data, self.width, self.height, self.width)
    }
}

/// A mutable view of a luma plane with an arbitrary row stride (e.g. an FFmpeg Y plane).
pub struct LumaCanvas<'a> {
    data: &'a mut [u8],
    width: usize,
    height: usize,
    stride: usize,
}

impl<'a> LumaCanvas<'a> {
    pub fn new(data: &'a mut [u8], width: usize, height: usize, stride: usize) -> Self {
        assert!(stride >= width && data.len() >= stride * height.saturating_sub(1) + width);
        LumaCanvas {
            data,
            width,
            height,
            stride,
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn fill(&mut self, value: u8) {
        for y in 0..self.height {
            self.data[y * self.stride..y * self.stride + self.width].fill(value);
        }
    }

    /// Fills `rect`, clipped to the canvas.
    pub fn fill_rect(&mut self, rect: Rect, value: u8) {
        let x0 = rect.x0.clamp(0, self.width as i32) as usize;
        let x1 = rect.x1.clamp(0, self.width as i32) as usize;
        let y0 = rect.y0.clamp(0, self.height as i32) as usize;
        let y1 = rect.y1.clamp(0, self.height as i32) as usize;
        if x0 >= x1 {
            return;
        }
        for y in y0..y1 {
            self.data[y * self.stride + x0..y * self.stride + x1].fill(value);
        }
    }

    /// Draws a `thickness`-pixel outline just inside `rect`.
    pub fn stroke_rect(&mut self, rect: Rect, thickness: i32, value: u8) {
        let t = thickness;
        self.fill_rect(
            Rect {
                y1: rect.y0 + t,
                ..rect
            },
            value,
        );
        self.fill_rect(
            Rect {
                y0: rect.y1 - t,
                ..rect
            },
            value,
        );
        self.fill_rect(
            Rect {
                x1: rect.x0 + t,
                ..rect
            },
            value,
        );
        self.fill_rect(
            Rect {
                x0: rect.x1 - t,
                ..rect
            },
            value,
        );
    }

    /// Copies an image of identical dimensions into this canvas.
    pub fn copy_from(&mut self, src: &LumaView<'_>) {
        assert_eq!((src.width, src.height), (self.width, self.height));
        for y in 0..self.height {
            let row = &src.data[y * src.stride..y * src.stride + src.width];
            self.data[y * self.stride..y * self.stride + self.width].copy_from_slice(row);
        }
    }
}

/// A read-only view of a luma plane with an arbitrary row stride.
#[derive(Clone, Copy)]
pub struct LumaView<'a> {
    data: &'a [u8],
    width: usize,
    height: usize,
    stride: usize,
}

impl<'a> LumaView<'a> {
    pub fn new(data: &'a [u8], width: usize, height: usize, stride: usize) -> Self {
        assert!(stride >= width && data.len() >= stride * height.saturating_sub(1) + width);
        LumaView {
            data,
            width,
            height,
            stride,
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn get(&self, x: usize, y: usize) -> u8 {
        self.data[y * self.stride + x]
    }

    /// Mean luma of the `(2r+1)²` patch centred on `(cx, cy)`, clipped to the image.
    pub fn mean_patch(&self, cx: f64, cy: f64, r: usize) -> f64 {
        let cx = (cx.round() as i64).clamp(0, self.width as i64 - 1) as usize;
        let cy = (cy.round() as i64).clamp(0, self.height as i64 - 1) as usize;
        let (x0, x1) = (cx.saturating_sub(r), (cx + r).min(self.width - 1));
        let (y0, y1) = (cy.saturating_sub(r), (cy + r).min(self.height - 1));
        let mut sum = 0u64;
        for y in y0..=y1 {
            sum += self.data[y * self.stride + x0..=y * self.stride + x1]
                .iter()
                .map(|&v| u64::from(v))
                .sum::<u64>();
        }
        sum as f64 / ((x1 - x0 + 1) * (y1 - y0 + 1)) as f64
    }
}
