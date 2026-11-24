//! The plain-English status line and the LCD state word, decided from what the worker knows.

use crate::prefs::{PhaseSetting, Prefs, WaveMode};
use crate::install::ModuleState;
use crate::rbmod::Source;
use crate::state::StateKind;
use club685_overlay::GateState;

/// How far the worker got finding and attaching to rekordbox.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Attach {
    NotRunning,
    NoMainWindow,
    WrongVersion,
    DpiMismatch,
    DpiUnknown,
    CaptureFailed,
    PartlyOffScreen,
    Attached,
}

#[derive(Clone, Debug)]
pub struct StatusInputs {
    pub attach: Attach,
    pub layout: bool,
    pub gate: Option<GateState>,
    pub cursor: bool,
    pub master: bool,
    pub phase: [bool; 2],
    pub source: Source,
    pub module: ModuleState,
}

pub fn status(prefs: &Prefs, i: &StatusInputs) -> (StateKind, String) {
    if !prefs.power {
        return (StateKind::Off, "Off · rekordbox is left alone".to_string());
    }
    if i.module.needs_setup() {
        return (StateKind::Setup, "One-time setup: install the rekordbox module".to_string());
    }
    match i.attach {
        Attach::NotRunning => return (StateKind::Searching, "Start rekordbox".to_string()),
        Attach::NoMainWindow => return (StateKind::Searching, "Open the rekordbox main window".to_string()),
        Attach::WrongVersion => return (StateKind::Error, "This build is for rekordbox 6.8.5 only".to_string()),
        Attach::DpiMismatch => return (StateKind::Error, "Move rekordbox to the main display".to_string()),
        Attach::DpiUnknown => return (StateKind::Error, "Cannot read the display scale of rekordbox".to_string()),
        Attach::PartlyOffScreen => return (StateKind::Error, "rekordbox is partly off screen · move it fully onto the display".to_string()),
        Attach::CaptureFailed => return (StateKind::Error, "Cannot capture the rekordbox window · retrying".to_string()),
        Attach::Attached => {}
    }
    match i.source {
        Source::Pending => return (StateKind::Searching, "Waiting for the rekordbox module…".to_string()),
        Source::Silent => return (StateKind::Error, "rekordbox module not responding: restart rekordbox".to_string()),
        Source::Module => {}
    }
    if !i.layout {
        return (StateKind::Searching, "Looking for the waveforms · use 2Deck Horizontal".to_string());
    }
    let gate = i.gate.unwrap_or(GateState::NoLayout);
    match gate {
        GateState::Open => {}
        GateState::NotForeground => return (StateKind::Ready, "Ready · switch to rekordbox".to_string()),
        GateState::Minimized => return (StateKind::Ready, "Ready · rekordbox is minimised".to_string()),
        GateState::NotVisible | GateState::Disabled => return (StateKind::Ready, "Ready · rekordbox is busy".to_string()),
        GateState::GuiMode => return (StateKind::Ready, "Paused · a menu or a move is open".to_string()),
        GateState::NoLayout | GateState::MetricsUnreadable | GateState::MetricsChanged | GateState::LayoutStale => {
            return (StateKind::Searching, "Measuring the rekordbox window…".to_string())
        }
    }
    let meter = prefs.phase != PhaseSetting::Off;
    let both_phase = i.phase[0] && i.phase[1];
    let line = match prefs.waveform {
        WaveMode::Split => {
            if !i.cursor {
                "Split · measuring the playhead…".to_string()
            } else if meter && !i.master {
                "Split active · waiting for MASTER".to_string()
            } else if meter && !both_phase {
                "Split active · zoom the waveform in for the phase meter".to_string()
            } else {
                "Split active".to_string()
            }
        }
        WaveMode::HideBoth => {
            if meter && !i.master {
                "Waveforms hidden · waiting for MASTER".to_string()
            } else if meter && !both_phase {
                "Waveforms hidden · zoom in for the phase meter".to_string()
            } else {
                "Waveforms hidden".to_string()
            }
        }
        WaveMode::Normal => {
            if meter {
                "Locked · the phase meter needs Split or Hide Both".to_string()
            } else {
                "Locked · nothing to draw yet".to_string()
            }
        }
    };
    (StateKind::Active, line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inp() -> StatusInputs {
        StatusInputs { attach: Attach::Attached, layout: true, gate: Some(GateState::Open), cursor: true, master: true, phase: [true, true], source: Source::Module, module: ModuleState::Installed }
    }

    #[test]
    fn attach_states() {
        let p = Prefs::default();
        let mut i = inp();
        i.attach = Attach::NotRunning;
        assert_eq!(status(&p, &i).1, "Start rekordbox");
        i.attach = Attach::DpiMismatch;
        assert_eq!(status(&p, &i).1, "Move rekordbox to the main display");
        assert_eq!(status(&p, &i).0, StateKind::Error);
    }

    #[test]
    fn foreground_gate() {
        let mut i = inp();
        i.gate = Some(GateState::NotForeground);
        assert_eq!(status(&Prefs::default(), &i).1, "Ready · switch to rekordbox");
    }

    #[test]
    fn split_waits_for_master() {
        let p = Prefs { waveform: WaveMode::Split, phase: PhaseSetting::Box, ..Prefs::default() };
        let mut i = inp();
        i.master = false;
        assert_eq!(status(&p, &i).1, "Split active · waiting for MASTER");
        i.master = true;
        assert_eq!(status(&p, &i).1, "Split active");
        i.phase = [true, false];
        assert!(status(&p, &i).1.contains("zoom"));
    }

    #[test]
    fn setup_comes_before_everything_but_power() {
        let mut i = inp();
        i.module = ModuleState::Missing;
        i.attach = Attach::NotRunning;
        let (k, line) = status(&Prefs::default(), &i);
        assert_eq!((k, line.as_str()), (StateKind::Setup, "One-time setup: install the rekordbox module"));
        i.module = ModuleState::Outdated;
        assert_eq!(status(&Prefs::default(), &i).0, StateKind::Setup);
        assert_eq!(status(&Prefs { power: false, ..Prefs::default() }, &i).0, StateKind::Off);
    }

    #[test]
    fn silent_module_asks_for_a_restart() {
        let mut i = inp();
        i.source = Source::Silent;
        let (k, line) = status(&Prefs::default(), &i);
        assert_eq!(k, StateKind::Error);
        assert_eq!(line, "rekordbox module not responding: restart rekordbox");
        i.source = Source::Pending;
        assert_eq!(status(&Prefs::default(), &i).1, "Waiting for the rekordbox module…");
    }

    #[test]
    fn power_off_wins() {
        let p = Prefs { power: false, ..Prefs::default() };
        assert_eq!(status(&p, &inp()).0, StateKind::Off);
    }
}
