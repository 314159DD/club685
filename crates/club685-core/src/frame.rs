//! Borrowed BGRA8 frame as produced by Windows.Graphics.Capture (rows may be padded to `stride` bytes).

#[derive(Clone, Copy, Debug)]
pub struct Bgra<'a> {
    pub data: &'a [u8],
    pub width: usize,
    pub height: usize,
    /// Bytes per row, at least `width * 4`.
    pub stride: usize,
}

/// Half-open pixel rectangle: `x0..x1`, `y0..y1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Rect {
    pub fn width(&self) -> i32 {
        self.x1 - self.x0
    }
    pub fn height(&self) -> i32 {
        self.y1 - self.y0
    }
}

impl<'a> Bgra<'a> {
    /// Returns `None` when the buffer is too small for the given geometry.
    pub fn new(data: &'a [u8], width: usize, height: usize, stride: usize) -> Option<Self> {
        if width == 0 || height == 0 || stride < width * 4 {
            return None;
        }
        if data.len() < stride * (height - 1) + width * 4 {
            return None;
        }
        Some(Self { data, width, height, stride })
    }

    #[inline]
    pub fn w(&self) -> i32 {
        self.width as i32
    }
    #[inline]
    pub fn h(&self) -> i32 {
        self.height as i32
    }

    /// RGB at (x, y). Outside the frame reads as black.
    #[inline]
    pub fn rgb(&self, x: i32, y: i32) -> (u8, u8, u8) {
        if x < 0 || y < 0 || x >= self.w() || y >= self.h() {
            return (0, 0, 0);
        }
        let i = y as usize * self.stride + x as usize * 4;
        (self.data[i + 2], self.data[i + 1], self.data[i])
    }

    /// One row of the frame, `width * 4` bytes.
    #[inline]
    pub fn row(&self, y: i32) -> &'a [u8] {
        let s = y as usize * self.stride;
        &self.data[s..s + self.width * 4]
    }
}

#[inline]
pub fn spread((r, g, b): (u8, u8, u8)) -> u8 {
    r.max(g).max(b) - r.min(g).min(b)
}

#[inline]
pub fn maxc((r, g, b): (u8, u8, u8)) -> u8 {
    r.max(g).max(b)
}
