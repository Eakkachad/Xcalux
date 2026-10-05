//! Bucket fill and magic wand regions: a scanline flood on pass bits with
//! gap closing, area scaling and antialiasing, written as one pixel edit.

use serde::{Deserialize, Serialize};

use crate::document::Document;
use crate::history::Edit;
use crate::layer::LayerId;
use crate::selection::Selection;

/// What the flood compares against the seed colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FillRef {
    /// The active layer.
    #[default]
    Active,
    /// The flattened page, paper included.
    AllVisible,
    /// Layers with `LayerProps::reference`.
    Reference,
}

/// How area scaling grows the region into the line art.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ScaleMode {
    #[default]
    Plain,
    /// Advance only towards darker (stronger) pixels.
    ToDarkest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FillBlend {
    /// Over the layer.
    #[default]
    Normal,
    /// Under the layer's pixels (dst-over).
    Behind,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FillParams {
    pub reference: FillRef,
    /// Largest per-channel distance to the seed that still passes (fix15).
    pub tolerance: u16,
    /// Gap-closing radius in px (see [`gap_radius`]).
    pub gap_px: u8,
    /// Px to grow (> 0) or shrink (< 0) the region.
    pub area_scale: i8,
    pub scale_mode: ScaleMode,
    pub contiguous: bool,
    pub antialias: bool,
    /// Limit the region to the current selection (the fill tool always
    /// does; the magic wand never does).
    pub use_selection: bool,
}

impl Default for FillParams {
    fn default() -> Self {
        Self {
            reference: FillRef::Active,
            tolerance: 0,
            gap_px: 0,
            area_scale: 0,
            scale_mode: ScaleMode::Plain,
            contiguous: true,
            antialias: true,
            use_selection: true,
        }
    }
}

/// Buffers reused across fills.
#[derive(Default)]
pub struct FillScratch {
    // FILL: pools, bit tiles, composite cache, CompositeScratch.
}

/// Gap-closing radius in px for UI level 0..=5 at `dpi`
/// (`{0, 2, 4, 8, 16, 32}·dpi/600`, at most 64).
pub fn gap_radius(_level: u8, _dpi: u32) -> u8 {
    // FILL
    0
}

/// The region a click at `seed` fills, as coverage. `None` when nothing
/// would be filled.
pub fn fill_region(_doc: &Document, _seed: (i32, i32), _p: &FillParams, _s: &mut FillScratch) -> Option<Selection> {
    // FILL
    None
}

/// Paint `color` (fix15 premultiplied) over `region` on `layer`. `None`
/// when nothing changed or the layer refuses (locked, folder).
pub fn apply_fill(
    _doc: &mut Document,
    _layer: LayerId,
    _region: &Selection,
    _color: [u16; 4],
    _opacity: f32,
    _blend: FillBlend,
) -> Option<Edit> {
    // FILL
    None
}

/// Fill the whole selection on `layer` with `color`.
pub fn fill_selection(_doc: &mut Document, _layer: LayerId, _color: [u16; 4]) -> Option<Edit> {
    // FILL
    None
}
