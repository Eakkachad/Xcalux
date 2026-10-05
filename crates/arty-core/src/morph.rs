//! Selection morphology: grow, shrink and feather.

use serde::{Deserialize, Serialize};

use crate::selection::Selection;

/// Structuring element of grow and shrink.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MorphShape {
    #[default]
    Circle,
    Square,
}

/// Dilate by `r` px on a `w`×`h` page.
pub fn grow(sel: &Selection, _r: u16, _shape: MorphShape, _w: u32, _h: u32) -> Selection {
    // SEL-CORE
    sel.clone()
}

/// Erode by `r` px on a `w`×`h` page.
pub fn shrink(sel: &Selection, _r: u16, _shape: MorphShape, _w: u32, _h: u32) -> Selection {
    // SEL-CORE
    sel.clone()
}

/// Soften the mask edge (σ = `sigma` px) on a `w`×`h` page.
pub fn feather(sel: &Selection, _sigma: u16, _w: u32, _h: u32) -> Selection {
    // SEL-CORE
    sel.clone()
}
