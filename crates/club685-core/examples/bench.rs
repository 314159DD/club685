//! `cargo run --release --example bench`: timing of analyze on every fixture frame (2560 wide).
#[path = "../tests/common/mod.rs"]
mod common;
use club685_core::*;
use std::time::Instant;

fn main() {
    let mut worst = std::time::Duration::ZERO;
    for name in common::ALL {
        let fx = common::load(name);
        let f = fx.frame();
        let mut st = TrackerState::new();
        let t = Instant::now();
        std::hint::black_box(analyze(&f, &mut st));
        let cold = t.elapsed();
        for _ in 0..5 {
            analyze(&f, &mut st);
        }
        let n = 300;
        let t = Instant::now();
        for _ in 0..n {
            std::hint::black_box(analyze(&f, &mut st));
        }
        let per = t.elapsed() / n;
        worst = worst.max(per);
        println!("{name:18} cold {cold:>10?}  steady {per:>10?}");
    }
    println!("worst steady {worst:?} (budget 4 ms)");
}
