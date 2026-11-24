//! CLUB685 overlay: click-through topmost windows drawn over rekordbox, the geometry that places them and the
//! gate that decides when they may be visible.

pub mod dpi;
pub mod gate;
pub mod geometry;
pub mod set;
pub mod window;

pub use dpi::{check_dpi, dpi_decision, DpiError};
pub use gate::{evaluate, query_inputs, query_metrics, GateInputs, GateState, LayoutStamp, WindowMetrics, MAX_LAYOUT_AGE_MS};
pub use geometry::{plan_overlays, plan_overlays_ex, to_screen, waveform_inset, waveform_mask_rect, Plan, WaveOverlay};
pub use set::OverlaySet;
pub use window::{affinity_failed, exclude_from_capture, exclude_layered_window, pump_messages, OverlayKind, OverlayWindow};
