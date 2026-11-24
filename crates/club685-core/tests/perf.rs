mod common;
use club685_core::*;
use std::time::Instant;

/// Steady-state and cold timings of `analyze` on a real 2560-wide frame. The 4 ms budget is asserted in release
/// builds only (`cargo test --release`); in debug it just prints.
#[test]
fn analyze_time() {
    let fx = common::load("3band-playing-a");
    let f = fx.frame();
    let mut st = TrackerState::new();
    let t = Instant::now();
    let _ = analyze(&f, &mut st);
    let cold = t.elapsed();
    for _ in 0..3 {
        analyze(&f, &mut st);
    }
    let n = 100;
    let t = Instant::now();
    for _ in 0..n {
        std::hint::black_box(analyze(&f, &mut st));
    }
    let per = t.elapsed() / n;
    println!("analyze: cold (layout discovery) {cold:?}, steady {per:?} per frame");
    if !cfg!(debug_assertions) {
        assert!(per.as_micros() < 4000, "steady {per:?}");
    }
}
