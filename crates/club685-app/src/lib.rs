//! CLUB685 for rekordbox 6.8.5: preferences, status texts, the Chrome-skin UI renderer and (on Windows) the
//! capture worker. The window shell lives in `main.rs`.

pub mod log;
pub mod install;
pub mod preview;
pub mod prefs;
pub mod rbmod;
pub mod scene;
pub mod state;
pub mod status;
pub mod ui;
#[cfg(windows)]
pub mod thumb;
#[cfg(windows)]
pub mod worker;
