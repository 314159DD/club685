#[path = "../tests/common/mod.rs"]
mod common;
use club685_core::*;

fn main() {
    for name in common::ALL {
        let fx = common::load(name);
        let f = fx.frame();
        let mut st = TrackerState::new();
        let mut r = analyze_at(&f, &mut st, 0);
        for i in 1..3 { r = analyze_at(&f, &mut st, i * 16); }
        println!("== {name}");
        println!("layout {:?}", r.layout);
        println!("cursor raw {:?} locked {:?}", r.cursor_raw, r.cursor);
        println!("master raw {:?} tracked {:?}", r.master_raw, r.master);
        for d in 0..2 {
            let dk = &r.decks[d];
            let bars: Vec<f32> = dk.markers.iter().filter(|m| m.kind == MarkerKind::Bar).map(|m| m.x).collect();
            let beats = dk.markers.iter().filter(|m| m.kind == MarkerKind::Beat).count();
            println!(" deck{} bars {:?}", d + 1, bars);
            println!("   beats {} unpaired {} phase {:?}", beats, dk.markers.iter().filter(|m| !m.both).count(), dk.phase);
            println!("   bars_text {:?} orange {} bpm_rect {:?}", dk.bars, dk.orange_px, dk.bpm_rect);
            if let Some(b) = dk.bpm_rect {
                for m in [BpmMaskMode::CoverFully, BpmMaskMode::OneDecimal, BpmMaskMode::NoDecimals] {
                    println!("   mask {:?} {:?}", m, mask_rect(&f, b, m));
                }
                println!("   glyphs {:?}", bpm_mask::find_glyphs(&f, b));
            }
        }
    }
}
