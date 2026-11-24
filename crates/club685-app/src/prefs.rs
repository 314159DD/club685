//! Preferences: JSON in `%LOCALAPPDATA%\CLUB685\preferences.json`, schema 1, written atomically (.tmp + replace),
//! at most 4 KB.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const SCHEMA: u32 = 1;
pub const MAX_BYTES: u64 = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BpmSetting {
    #[default]
    Off,
    Hide,
    OneDecimal,
    NoDecimals,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WaveMode {
    #[default]
    Normal,
    Split,
    HideBoth,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PhaseSetting {
    #[default]
    Off,
    Box,
    Line,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub schema: u32,
    pub power: bool,
    pub bpm: BpmSetting,
    pub waveform: WaveMode,
    pub phase: PhaseSetting,
    pub disco: bool,
    pub window_x: Option<i32>,
    pub window_y: Option<i32>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            schema: SCHEMA,
            power: true,
            bpm: BpmSetting::Off,
            waveform: WaveMode::Normal,
            phase: PhaseSetting::Off,
            disco: false,
            window_x: None,
            window_y: None,
        }
    }
}

impl Prefs {
    /// Parses a preferences file. Anything unreadable, oversized or of another schema gives the defaults.
    pub fn from_json(text: &str) -> Prefs {
        if text.len() as u64 > MAX_BYTES {
            return Prefs::default();
        }
        match serde_json::from_str::<Prefs>(text) {
            Ok(p) if p.schema == SCHEMA => p,
            _ => Prefs::default(),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}

/// `%LOCALAPPDATA%\CLUB685\preferences.json`.
pub fn default_path() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("CLUB685").join("preferences.json"))
}

pub fn load(path: &Path) -> Prefs {
    match std::fs::metadata(path) {
        Ok(m) if m.len() <= MAX_BYTES => std::fs::read_to_string(path).map(|t| Prefs::from_json(&t)).unwrap_or_default(),
        _ => Prefs::default(),
    }
}

/// Writes `<path>.tmp` and renames it over `path` (replace).
pub fn save(path: &Path, prefs: &Prefs) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, prefs.to_json())?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_schema() {
        let p = Prefs {
            power: false,
            bpm: BpmSetting::OneDecimal,
            waveform: WaveMode::HideBoth,
            phase: PhaseSetting::Line,
            disco: true,
            window_x: Some(-5),
            window_y: Some(40),
            ..Prefs::default()
        };
        let j = p.to_json();
        assert!(j.contains("\"schema\": 1"));
        assert!(j.contains("one_decimal"));
        assert_eq!(Prefs::from_json(&j), p);
    }

    #[test]
    fn bad_input_gives_defaults() {
        assert_eq!(Prefs::from_json("not json"), Prefs::default());
        assert_eq!(Prefs::from_json("{\"schema\":2,\"power\":false}"), Prefs::default());
        assert_eq!(Prefs::from_json(&" ".repeat(5000)), Prefs::default());
        // unknown and missing fields are tolerated
        let p = Prefs::from_json("{\"schema\":1,\"phase\":\"box\",\"future\":1}");
        assert_eq!(p.phase, PhaseSetting::Box);
        assert!(p.power);
    }

    #[test]
    fn atomic_save_replaces_and_leaves_no_tmp() {
        let dir = std::env::temp_dir().join(format!("club685-prefs-test-{}", std::process::id()));
        let path = dir.join("preferences.json");
        save(&path, &Prefs::default()).unwrap();
        let p2 = Prefs { waveform: WaveMode::Split, ..Prefs::default() };
        save(&path, &p2).unwrap();
        assert_eq!(load(&path), p2);
        assert!(!path.with_extension("json.tmp").exists());
        assert!(std::fs::metadata(&path).unwrap().len() <= MAX_BYTES);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
