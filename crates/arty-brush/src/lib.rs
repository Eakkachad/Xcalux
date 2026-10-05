//! ARTY brush pipeline.
//!
//! Raw pen samples ([`InputSample`]) pass through the [`Stabilizer`], then
//! hokusai (a libmypaint port) turns them into dabs painted straight into
//! the document's copy-on-write tiles via [`LayerSurface`]. The
//! [`StrokeEngine`] records undo state and dirty tiles as it goes.

pub mod engine;
pub mod input;
pub mod preset;
pub mod pressure;
pub mod preview;
pub mod shape;
pub mod surface;

pub use engine::{Reshape, StrokeEngine, StrokeRefused};
pub use input::{InputSample, Stabilizer};
pub use preset::{BrushGroup, BrushPreset, MAX_BRUSH_SIZE, MIN_BRUSH_SIZE, default_presets};
pub use preview::render_preview;
pub use surface::{LayerSurface, MaskCur};
