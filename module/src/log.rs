//! Append-only log in %LOCALAPPDATA%\rbmod\rbmod.log, with GetTempPathW as fallback.
//! Every line is opened, written and closed again, so nothing stays buffered if the process dies.

use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use windows_sys::Win32::Storage::FileSystem::GetTempPathW;

fn log_path() -> &'static PathBuf {
    static P: OnceLock<PathBuf> = OnceLock::new();
    P.get_or_init(|| {
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Some(d) = std::env::var_os("LOCALAPPDATA").filter(|d| !d.is_empty()) {
            dirs.push(PathBuf::from(d).join("rbmod"));
        }
        let mut buf = [0u16; 520];
        let n = unsafe { GetTempPathW(buf.len() as u32, buf.as_mut_ptr()) } as usize;
        if n > 0 && n < buf.len() {
            dirs.push(PathBuf::from(String::from_utf16_lossy(&buf[..n])).join("rbmod"));
        }
        for d in &dirs {
            if std::fs::create_dir_all(d).is_ok() {
                return d.join("rbmod.log");
            }
        }
        PathBuf::from(r"C:\rbmod.log")
    })
}

pub fn dir() -> &'static std::path::Path {
    log_path().parent().unwrap_or(std::path::Path::new("C:\\"))
}

pub fn line(msg: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log_path()) {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let _ = writeln!(f, "{t} {msg}");
        let _ = f.flush();
        let _ = f.sync_all();
    }
}
