//! `--dry`: runs the fixtures through the same pipeline as the live loop (sparse band reads into a window-sized
//! buffer, `analyze`, mask and overlay plan math) without touching rekordbox or creating any window.

use crate::pipeline::{percentile, reading_str, BpmOpt, Pipeline};
use club685_capture::FrameBuf;
use club685_core::{Bgra, Rect};
use club685_overlay::plan_overlays;
use std::path::{Path, PathBuf};

pub struct Fixture {
    pub data: Vec<u8>,
    pub stride: usize,
    pub width: usize,
    pub height: usize,
}

impl Fixture {
    pub fn frame(&self) -> Bgra<'_> {
        Bgra::new(&self.data, self.width, self.height, self.stride).unwrap()
    }
}

pub fn default_fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

/// Decodes a fixture PNG to BGRA. Only the top-left 2560x1410 is window content (same rule as core's tests).
pub fn load(path: &Path) -> Result<Fixture, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(file));
    dec.set_transformations(png::Transformations::EXPAND);
    let mut reader = dec.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let channels = match info.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        other => return Err(format!("unsupported {other:?}")),
    };
    let (w, h) = (info.width as usize, info.height as usize);
    let mut data = vec![255u8; w * h * 4];
    for i in 0..w * h {
        data[i * 4] = buf[i * channels + 2];
        data[i * 4 + 1] = buf[i * channels + 1];
        data[i * 4 + 2] = buf[i * channels];
    }
    Ok(Fixture { data, stride: w * 4, width: 2560.min(w), height: 1410.min(h) })
}

pub fn fixtures_in(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "png")).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// What one fixture produced through the sparse pipeline.
pub struct DryResult {
    pub line: String,
    pub read_px: usize,
    pub full_px: usize,
    pub steady_ms: Vec<f64>,
    /// Reading text of the full-frame analysis and of the sparse one after the same number of frames.
    pub full_text: String,
    pub sparse_text: String,
    pub sparse_masks: [Option<Rect>; 2],
    pub full_masks: [Option<Rect>; 2],
}

/// Runs one fixture: 3 frames to lock, then `extra` steady frames, all through sparse reads after discovery.
/// Also runs the same frames over the full fixture for comparison.
pub fn run_fixture(fx: &Fixture, bpm: BpmOpt, extra: usize) -> DryResult {
    let src = fx.frame();
    let (w, h) = (fx.width as i32, fx.height as i32);
    let frames = 3 + extra;

    // Sparse pipeline, exactly what the live loop does.
    let mut buf = FrameBuf::new();
    buf.ensure(fx.width, fx.height);
    let mut pipe = Pipeline::new(bpm);
    let mut steady = Vec::new();
    let mut read_px = 0usize;
    let mut last = None;
    for i in 0..frames {
        let r = pipe.next_read(w, h);
        let copied = buf.blit_from(r, &src);
        read_px = (copied.width() * copied.height()) as usize;
        let out = pipe.process(&buf, i as u64 * 16).expect("buffer is a valid frame");
        if i >= 3 {
            steady.push(out.analyze_ms);
        }
        last = Some(out);
    }
    let out = last.unwrap();

    // Reference: the same frames, always the whole fixture.
    let mut full_buf = FrameBuf::new();
    full_buf.ensure(fx.width, fx.height);
    let mut full_pipe = Pipeline::new(bpm);
    let mut full_last = None;
    for i in 0..frames {
        full_buf.blit_from(Rect { x0: 0, y0: 0, x1: w, y1: h }, &src);
        full_last = full_pipe.process(&full_buf, i as u64 * 16);
    }
    let full = full_last.unwrap();

    let band = out.reading.layout.map(|l| l.band);
    let plan = plan_overlays((0, 0), 96, w, out.bpm_masks, band, true);
    let line = format!(
        "{} bpm_masks={:?} wave={:?} read={}x{}px of {}",
        reading_str(&out.reading),
        plan.bpm.iter().map(|b| b.map(|r| (r.x0, r.y0, r.x1, r.y1))).collect::<Vec<_>>(),
        plan.wave.map(|r| (r.x0, r.y0, r.x1, r.y1)),
        pipe.next_read(w, h).width(),
        pipe.next_read(w, h).height(),
        format_args!("{w}x{h}"),
    );
    DryResult {
        line,
        read_px,
        full_px: (w * h) as usize,
        steady_ms: steady,
        full_text: reading_str(&full.reading),
        sparse_text: reading_str(&out.reading),
        sparse_masks: out.bpm_masks,
        full_masks: full.bpm_masks,
    }
}

/// Runs every fixture of `dir` and prints one line each. Returns the exit code (0 when sparse == full everywhere).
pub fn run(dir: &Path, bpm: BpmOpt) -> i32 {
    let files = fixtures_in(dir);
    if files.is_empty() {
        println!("dry: no fixtures in {}", dir.display());
        return 2;
    }
    let mut mismatches = 0;
    let mut all_ms: Vec<f64> = Vec::new();
    for p in &files {
        let name = p.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
        let fx = match load(p) {
            Ok(f) => f,
            Err(e) => {
                println!("dry {name}: {e}");
                mismatches += 1;
                continue;
            }
        };
        let r = run_fixture(&fx, bpm, 20);
        let same = r.full_text == r.sparse_text && r.full_masks == r.sparse_masks;
        if !same {
            mismatches += 1;
        }
        println!("dry {name}: {}{}", r.line, if same { "" } else { "  MISMATCH vs full-frame read" });
        println!("    sparse read {:.1}% of the frame", r.read_px as f64 * 100.0 / r.full_px as f64);
        all_ms.extend(r.steady_ms);
    }
    all_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!("dry: {} fixtures, analyze p50={:.2}ms p95={:.2}ms, mismatches={mismatches}", files.len(), percentile(&all_ms, 0.5), percentile(&all_ms, 0.95));
    if mismatches == 0 {
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_band_reads_match_full_frame_analysis() {
        let dir = default_fixture_dir();
        for name in ["3band-playing-a", "session-020", "rgb-paused"] {
            let fx = load(&dir.join(format!("{name}.png"))).unwrap();
            let r = run_fixture(&fx, BpmOpt::Mode(club685_core::BpmMaskMode::CoverFully), 5);
            assert_eq!(r.full_text, r.sparse_text, "{name}");
            assert_eq!(r.full_masks, r.sparse_masks, "{name}");
            assert!(r.read_px * 3 < r.full_px, "{name}: sparse read should be a small part of the frame");
            assert!(r.sparse_masks[0].is_some() && r.sparse_masks[1].is_some(), "{name}: both BPM masks expected");
        }
    }
}
