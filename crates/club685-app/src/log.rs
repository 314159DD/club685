//! Tiny diagnostic log. Off unless `CLUB685_LOG` is set: `1` writes to `%LOCALAPPDATA%\CLUB685\club685.log`, any other
//! value is used as the file path. Only transitions and one stats line per second are written.

use std::io::Write;
use std::sync::OnceLock;
use std::time::Instant;

static PATH: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
static START: OnceLock<Instant> = OnceLock::new();

fn path() -> &'static Option<std::path::PathBuf> {
    PATH.get_or_init(|| {
        let v = std::env::var("CLUB685_LOG").ok()?;
        if v.is_empty() || v == "0" {
            return None;
        }
        if v == "1" {
            let d = std::env::var_os("LOCALAPPDATA")?;
            let dir = std::path::PathBuf::from(d).join("CLUB685");
            let _ = std::fs::create_dir_all(&dir);
            return Some(dir.join("club685.log"));
        }
        Some(std::path::PathBuf::from(v))
    })
}

pub fn enabled() -> bool {
    path().is_some()
}

pub fn line(msg: &str) {
    let Some(p) = path() else { return };
    let t = START.get_or_init(Instant::now).elapsed().as_secs_f32();
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
        let _ = writeln!(f, "[{t:8.2}] {msg}");
    }
}
