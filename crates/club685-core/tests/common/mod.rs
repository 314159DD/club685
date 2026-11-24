#![allow(dead_code)]
use club685_core::Bgra;
use std::path::PathBuf;

/// A decoded fixture: BGRA bytes with the PNG stride; only the top-left 2560x1410 is window content.
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

pub fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures").join(format!("{name}.png"))
}

pub fn load(name: &str) -> Fixture {
    let file = std::fs::File::open(fixture_path(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
    let mut dec = png::Decoder::new(std::io::BufReader::new(file));
    dec.set_transformations(png::Transformations::EXPAND);
    let mut reader = dec.read_info().unwrap();
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).unwrap();
    let channels = match info.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        other => panic!("unsupported {other:?}"),
    };
    let (w, h) = (info.width as usize, info.height as usize);
    let mut data = vec![255u8; w * h * 4];
    for i in 0..w * h {
        data[i * 4] = buf[i * channels + 2];
        data[i * 4 + 1] = buf[i * channels + 1];
        data[i * 4 + 2] = buf[i * channels];
    }
    Fixture { data, stride: w * 4, width: 2560.min(w), height: 1410.min(h) }
}

pub const ALL: [&str; 12] = [
    "3band-paused",
    "3band-playing-a",
    "3band-playing-b",
    "blue-paused",
    "rgb-paused",
    "session-000",
    "session-020",
    "session-036",
    "session-040",
    "session-045",
    "session-050",
    "session-055",
];
