//! Writes `club685/assets/club685.ico` (PNG-compressed frames, 16 to 256 px) from the same drawing as the tray icon.
//!
//! usage: cargo run --release -p club685-app --example make_icon

use club685_app::ui::draw_icon;
use std::io::Write;

fn png_of(size: u32) -> Vec<u8> {
    let pm = draw_icon(size);
    let mut rgba = Vec::with_capacity(pm.data().len());
    for px in pm.pixels() {
        let c = px.demultiply();
        rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, size, size);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&rgba).unwrap();
    }
    out
}

fn main() {
    let sizes = [16u32, 24, 32, 48, 64, 128, 256];
    let frames: Vec<(u32, Vec<u8>)> = sizes.iter().map(|&s| (s, png_of(s))).collect();
    let mut ico: Vec<u8> = Vec::new();
    ico.extend_from_slice(&[0, 0, 1, 0]);
    ico.extend_from_slice(&(frames.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * frames.len() as u32;
    for (s, data) in &frames {
        let b = if *s >= 256 { 0u8 } else { *s as u8 };
        ico.extend_from_slice(&[b, b, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&32u16.to_le_bytes());
        ico.extend_from_slice(&(data.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, data) in &frames {
        ico.write_all(data).unwrap();
    }
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/club685.ico");
    std::fs::write(&path, &ico).unwrap();
    println!("wrote {} ({} bytes)", path.display(), ico.len());
}
