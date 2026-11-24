//! A window-sized CPU frame that is filled only where the analysis needs pixels. Core's layout coordinates stay
//! valid because the buffer has the full window size; rows that were never copied stay black.

use club685_core::{Bgra, Rect};

#[derive(Debug, Default)]
pub struct FrameBuf {
    w: usize,
    h: usize,
    data: Vec<u8>,
}

impl FrameBuf {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn size(&self) -> (usize, usize) {
        (self.w, self.h)
    }

    /// Makes the buffer `w` x `h`. Returns true when it had to be reallocated (contents are then black).
    pub fn ensure(&mut self, w: usize, h: usize) -> bool {
        if self.w == w && self.h == h {
            return false;
        }
        self.w = w;
        self.h = h;
        self.data.clear();
        self.data.resize(w * h * 4, 0);
        true
    }

    pub fn bgra(&self) -> Option<Bgra<'_>> {
        Bgra::new(&self.data, self.w, self.h, self.w * 4)
    }

    /// Clamps `r` to the buffer.
    pub fn clamp(&self, r: Rect) -> Rect {
        Rect {
            x0: r.x0.clamp(0, self.w as i32),
            y0: r.y0.clamp(0, self.h as i32),
            x1: r.x1.clamp(0, self.w as i32),
            y1: r.y1.clamp(0, self.h as i32),
        }
    }

    /// Copies rect `r` (window coordinates) from `src`. Source pixel (x, y) of the window sits at
    /// `src[(y - origin.1) * src_stride + (x - origin.0) * 4]`. Returns the clamped rect that was copied.
    pub fn blit(&mut self, r: Rect, src: &[u8], src_stride: usize, origin: (i32, i32)) -> Rect {
        let r = self.clamp(r);
        if r.width() <= 0 || r.height() <= 0 {
            return r;
        }
        let n = r.width() as usize * 4;
        for y in r.y0..r.y1 {
            let s = (y - origin.1) as usize * src_stride + (r.x0 - origin.0) as usize * 4;
            let d = y as usize * self.w * 4 + r.x0 as usize * 4;
            self.data[d..d + n].copy_from_slice(&src[s..s + n]);
        }
        r
    }

    /// Copies rect `r` from a full-window frame (dry run and tests).
    pub fn blit_from(&mut self, r: Rect, src: &Bgra<'_>) -> Rect {
        self.blit(r, src.data, src.stride, (0, 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blit_copies_only_the_rect() {
        let (w, h) = (8usize, 6usize);
        let src: Vec<u8> = (0..w * h * 4).map(|i| (i % 251) as u8 | 1).collect();
        let sf = Bgra::new(&src, w, h, w * 4).unwrap();
        let mut b = FrameBuf::new();
        assert!(b.ensure(w, h));
        assert!(!b.ensure(w, h));
        let got = b.blit_from(Rect { x0: 2, y0: 1, x1: 5, y1: 3 }, &sf);
        assert_eq!(got, Rect { x0: 2, y0: 1, x1: 5, y1: 3 });
        let f = b.bgra().unwrap();
        assert_eq!(f.rgb(2, 1), sf.rgb(2, 1));
        assert_eq!(f.rgb(4, 2), sf.rgb(4, 2));
        assert_eq!(f.rgb(1, 1), (0, 0, 0));
        assert_eq!(f.rgb(5, 2), (0, 0, 0));
        assert_eq!(f.rgb(3, 3), (0, 0, 0));
    }

    #[test]
    fn blit_with_origin_and_clamp() {
        let mut b = FrameBuf::new();
        b.ensure(4, 4);
        // 2x2 source that represents window rect x 1..3, y 2..4
        let src = vec![9u8; 2 * 2 * 4];
        let got = b.blit(Rect { x0: 1, y0: 2, x1: 3, y1: 4 }, &src, 8, (1, 2));
        assert_eq!(got, Rect { x0: 1, y0: 2, x1: 3, y1: 4 });
        let clamped = b.blit(Rect { x0: 1, y0: 2, x1: 9, y1: 9 }, &[9u8; 64], 8, (1, 2));
        assert_eq!(clamped, Rect { x0: 1, y0: 2, x1: 4, y1: 4 });
    }
}
