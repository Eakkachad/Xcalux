//! Polygon rasterization into selections: one signed-area coverage
//! accumulator for every shape (rect, ellipse, lasso, polygon), in
//! document space.

use crate::geom::Pt;
use crate::selection::Selection;

/// Coverage of the closed polygon `pts` (non-zero winding) on a `w`×`h`
/// page, in canonical form. `antialias = false` thresholds at 0.5.
pub fn rasterize_polygon(_pts: &[Pt], _w: u32, _h: u32, _antialias: bool) -> Selection {
    // SEL-CORE
    Selection::default()
}
