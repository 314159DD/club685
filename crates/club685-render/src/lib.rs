//! CLUB685 render: split view, central view and phase meters drawn in software (spec section 7). No windows and
//! no capture here, so everything can be tested and previewed from fixtures.

pub mod compose;
pub mod geometry;
pub mod meter;
pub mod shapes;
pub mod text;

pub use compose::{Composer, Scene, SceneDeck};
pub use geometry::{
    bars_text, central_order, fit_scale, gutter, header_height, line_ticks, lit_block, meter_height_logical, meter_scale, meter_width_logical,
    split_geometry, viewport_source, viewport_width, MeterKind, SplitGeometry,
};
pub use meter::{MeterSpec, Role};
