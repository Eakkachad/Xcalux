//! ARTY document core: tiles, layers, compositing and history.
//!
//! No GPU or UI dependencies. Everything here is deterministic and
//! unit-testable; the hot paths (compositing, tile writes) are kept
//! allocation-free in steady state.

pub mod blend;
pub mod composite;
pub mod document;
pub mod fix15;
pub mod grid;
pub mod history;
pub mod layer;
pub mod tile;

pub use blend::BlendMode;
pub use composite::CompositeScratch;
pub use document::{DirtyRegion, Document, PAPER_WHITE, StructureSnapshot};
pub use grid::TileGrid;
pub use history::{Edit, History, PixelRecorder};
pub use layer::{Layer, LayerContent, LayerId, LayerProps};
pub use tile::{TILE_SIZE, TileCoord, TilePixels, TileRef};
