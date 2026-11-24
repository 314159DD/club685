//! Frame freshness rules (spec section 1): drop frames more than 50 ms in the future, more than 250 ms old, or
//! not newer than the last accepted one. All times are 100 ns ticks on the QueryPerformanceCounter timebase,
//! which is what `SystemRelativeTime` uses.

pub const MAX_FUTURE_TICKS: i64 = 50 * 10_000;
pub const MAX_AGE_TICKS: i64 = 250 * 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Fresh,
    Future,
    Stale,
    NotNewer,
}

#[derive(Debug, Default)]
pub struct Freshness {
    last: Option<i64>,
}

impl Freshness {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.last = None;
    }

    /// Judges a frame stamped `frame` at wall time `now`; a fresh frame becomes the new "last".
    pub fn check(&mut self, frame: i64, now: i64) -> Verdict {
        if self.last.is_some_and(|l| frame <= l) {
            return Verdict::NotNewer;
        }
        if frame - now > MAX_FUTURE_TICKS {
            return Verdict::Future;
        }
        if now - frame > MAX_AGE_TICKS {
            return Verdict::Stale;
        }
        self.last = Some(frame);
        Verdict::Fresh
    }
}

/// Current time in 100 ns ticks (QueryPerformanceCounter).
pub fn now_ticks() -> i64 {
    use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
    let (mut c, mut f) = (0i64, 0i64);
    unsafe {
        let _ = QueryPerformanceCounter(&mut c);
        let _ = QueryPerformanceFrequency(&mut f);
    }
    if f <= 0 {
        return 0;
    }
    (c as i128 * 10_000_000 / f as i128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    const MS: i64 = 10_000;

    #[test]
    fn fresh_frame_passes() {
        let mut f = Freshness::new();
        assert_eq!(f.check(1000 * MS, 1010 * MS), Verdict::Fresh);
    }

    #[test]
    fn future_and_stale_limits() {
        let mut f = Freshness::new();
        assert_eq!(f.check(1051 * MS, 1000 * MS), Verdict::Future);
        assert_eq!(f.check(1050 * MS, 1000 * MS), Verdict::Fresh);
        let mut f = Freshness::new();
        assert_eq!(f.check(1000 * MS, 1251 * MS), Verdict::Stale);
        assert_eq!(f.check(1000 * MS, 1250 * MS), Verdict::Fresh);
    }

    #[test]
    fn must_be_newer_than_last() {
        let mut f = Freshness::new();
        assert_eq!(f.check(1000 * MS, 1000 * MS), Verdict::Fresh);
        assert_eq!(f.check(1000 * MS, 1001 * MS), Verdict::NotNewer);
        assert_eq!(f.check(999 * MS, 1001 * MS), Verdict::NotNewer);
        assert_eq!(f.check(1001 * MS, 1002 * MS), Verdict::Fresh);
    }

    #[test]
    fn rejected_frames_do_not_advance_last() {
        let mut f = Freshness::new();
        assert_eq!(f.check(5000 * MS, 1000 * MS), Verdict::Future);
        assert_eq!(f.check(1000 * MS, 1000 * MS), Verdict::Fresh);
    }
}
