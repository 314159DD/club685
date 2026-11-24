//! Glue between the core's readings and the renderer's scene.

use club685_core::{Bgra, DeckReading, FrameReading, PhaseKind};
use club685_render::{MeterKind, Scene, SceneDeck};

/// Beat position 0.0..4.0 for the meter. The full grid fit gives it directly; at wide zoom only bars are drawn, so
/// the beat comes from the Bars text (whole beats only).
pub fn deck_phase(d: &DeckReading) -> Option<f32> {
    match d.phase {
        Some(p) if p.kind == PhaseKind::Grid => Some(p.phase),
        Some(_) => d.bars.map(|(_, beat)| (beat.clamp(1, 4) - 1) as f32),
        None => None,
    }
}

pub fn scene_deck(d: &DeckReading) -> SceneDeck {
    SceneDeck { phase: deck_phase(d), bars: d.bars }
}

/// The playhead used for cropping: the locked one, else this frame's measurement. Never the geometric centre.
pub fn cursor_of(r: &FrameReading) -> Option<f32> {
    r.cursor.or(r.cursor_raw)
}

pub fn build_scene<'a>(frame: &'a Bgra<'a>, r: &FrameReading, dpi: u32, meter: Option<MeterKind>) -> Option<Scene<'a>> {
    let l = r.layout.as_ref()?;
    Some(Scene {
        frame,
        rows: l.rows,
        cursor: cursor_of(r),
        decks: [scene_deck(&r.decks[0]), scene_deck(&r.decks[1])],
        master: r.master,
        dpi,
        meter,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use club685_core::PhaseReading;

    fn pr(kind: PhaseKind, phase: f32) -> PhaseReading {
        PhaseReading { kind, phase, beat: phase.floor() as u8 + 1, beat_spacing: 42.0, bar_spacing: 168.0, red: 3, grey: 9 }
    }

    #[test]
    fn grid_phase_is_used_directly() {
        let d = DeckReading { phase: Some(pr(PhaseKind::Grid, 2.4)), ..Default::default() };
        assert_eq!(deck_phase(&d), Some(2.4));
    }

    #[test]
    fn bars_only_takes_the_beat_from_the_text() {
        let d = DeckReading { phase: Some(pr(PhaseKind::BarsOnly, 1.7)), bars: Some((12, 3)), ..Default::default() };
        assert_eq!(deck_phase(&d), Some(2.0));
        let d = DeckReading { phase: Some(pr(PhaseKind::BarsOnly, 1.7)), bars: None, ..Default::default() };
        assert_eq!(deck_phase(&d), None);
        assert_eq!(deck_phase(&DeckReading::default()), None);
    }
}
