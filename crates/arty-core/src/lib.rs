//! ARTY document core: tiles, layers, compositing and history.
//!
//! No GPU or UI dependencies. Everything here is deterministic and
//! unit-testable; the hot paths (compositing, tile writes) are kept
//! allocation-free in steady state.

pub mod blend;
pub mod composite;
pub mod contour;
pub mod document;
pub mod fill;
pub mod fix15;
pub mod frame;
pub mod geom;
pub mod grid;
pub mod history;
pub mod layer;
pub mod morph;
pub mod page;
pub mod raster;
pub mod selection;
pub mod tile;
pub mod transform;

pub use blend::BlendMode;
pub use composite::CompositeScratch;
pub use document::{
    DirtyRegion, DocParts, Document, MAX_LAYERS, MAX_NEXT_ID, MAX_TREE_DEPTH, PAPER_WHITE, StructureSnapshot, TreeError,
};
pub use frame::{BorderStyle, Cov, Frame, FrameShape, Panel};
pub use geom::{Affine64, Pt, RectF, TileRect};
pub use grid::TileGrid;
pub use history::{Edit, History, PixelRecorder, Touch};
pub use layer::{Layer, LayerContent, LayerId, LayerProps};
pub use page::PageSetup;
pub use selection::{MaskPixels, MaskRef, MaskView, SelectOp, Selection};
pub use tile::{TILE_SIZE, TileCoord, TilePixels, TileRef};
