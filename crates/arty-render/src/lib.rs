//! ARTY canvas display.
//!
//! The document is flattened on the CPU, one dirty 64×64 tile at a time
//! ([`CanvasSync`]), into a mipmapped GPU texture array ([`CanvasGpu`]) that
//! an egui paint callback draws through the [`View`] transform.

pub mod gpu;
pub mod upload;
pub mod view;

pub use gpu::CanvasGpu;
pub use upload::{CanvasSync, SyncStats};
pub use view::{Affine2, View, ZOOM_STEPS};
