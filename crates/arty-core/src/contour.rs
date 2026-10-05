//! Selection outlines for the marching ants: marching squares at iso 127.5,
//! linked across tiles and simplified into levels of detail.

use crate::geom::Pt;
use crate::selection::Selection;

/// Douglas–Peucker tolerance of each level of detail, in document px.
pub const LOD_TOL: [f32; 3] = [0.25, 2.0, 8.0];

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Polyline {
    pub pts: Vec<Pt>,
    pub closed: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Contours {
    /// One outline set per [`LOD_TOL`] entry, finest first.
    pub lods: [Vec<Polyline>; 3],
    /// Segments at the finest level.
    pub segments: usize,
    /// The segment budget ran out; the outline is incomplete.
    pub truncated: bool,
}

/// Outlines of `sel` on a `w`×`h` page (off-page counts as unselected).
pub fn extract(_sel: &Selection, _w: u32, _h: u32) -> Contours {
    // SEL-UI
    Contours::default()
}
