//! Free transform (move, scale, rotate, flip) of a layer or of the selected
//! pixels: a floating session previewed into the layer itself, committed
//! as one history step.

use serde::{Deserialize, Serialize};

use crate::document::Document;
use crate::geom::{Affine64, RectF};
use crate::grid::TileGrid;
use crate::history::Edit;
use crate::layer::LayerId;
use crate::selection::Selection;
use crate::tile::TileCoord;

/// Resampling filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Filter {
    Nearest,
    Bilinear,
    #[default]
    Bicubic,
}

/// What a session moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XfTarget {
    /// The whole layer.
    Layer,
    /// The selected pixels, lifted off the layer, and the selection itself.
    Selection,
}

/// Why [`FloatSession::begin`] refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XfRefused {
    Folder,
    Locked,
    Empty,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct XfParams {
    /// Translation, doc px.
    pub t: [f64; 2],
    /// Scale; negative flips.
    pub s: [f64; 2],
    /// Rotation, radians.
    pub theta: f64,
    /// Centre of scale and rotation, doc px.
    pub pivot: [f64; 2],
}

impl XfParams {
    pub fn identity(pivot: [f64; 2]) -> Self {
        Self { t: [0.0; 2], s: [1.0; 2], theta: 0.0, pivot }
    }

    /// `T(t)·T(p)·R(θ)·S(s)·T(−p)`: scale, then rotate, about the pivot,
    /// then translate.
    pub fn affine(&self) -> Affine64 {
        let [px, py] = self.pivot;
        Affine64::translate(-px, -py)
            .then(Affine64::scale(self.s[0], self.s[1]))
            .then(Affine64::rotate(self.theta))
            .then(Affine64::translate(px + self.t[0], py + self.t[1]))
    }
}

/// Lazily built 2× box mips of the source, for downscaling.
#[derive(Default)]
struct SrcPyramid {
    // TRANSFORM
}

/// A transform in progress. The layer shows the preview while it runs.
// TRANSFORM reads the pixel state (`orig`, `src`, `base`, …) once
// `begin`/`preview`/`commit` are implemented.
#[allow(dead_code)]
pub struct FloatSession {
    layer: LayerId,
    target: XfTarget,
    /// The layer's grid when the session began (O(1) clone).
    orig: TileGrid,
    /// The pixels being transformed.
    src: TileGrid,
    /// What stays under them (empty for a layer target).
    base: TileGrid,
    /// Tight bbox of alpha > 0 in `src`.
    src_bounds: RectF,
    sel_before: Option<Selection>,
    params: XfParams,
    /// Tiles the last preview wrote.
    touched: Vec<TileCoord>,
    pyramid: SrcPyramid,
}

impl FloatSession {
    /// Start a session on `layer`; the target is the selection when the
    /// document has one.
    pub fn begin(_doc: &mut Document, _layer: LayerId) -> Result<FloatSession, XfRefused> {
        // TRANSFORM
        Err(XfRefused::Unsupported)
    }

    pub fn layer(&self) -> LayerId {
        self.layer
    }

    pub fn target(&self) -> XfTarget {
        self.target
    }

    pub fn params(&self) -> XfParams {
        self.params
    }

    pub fn src_bounds(&self) -> RectF {
        self.src_bounds
    }

    pub fn affine(&self) -> Affine64 {
        self.params.affine()
    }

    /// Show the layer transformed by `p`.
    pub fn preview(&mut self, _doc: &mut Document, p: XfParams, _f: Filter) {
        // TRANSFORM: resample into the layer.
        self.params = p;
    }

    /// Final resample with `f`. `Edit::Pixels`, or `Edit::Batch[Pixels,
    /// Selection]` for a selection target; `None` when nothing changed.
    pub fn commit(self, _doc: &mut Document, _f: Filter) -> Option<Edit> {
        // TRANSFORM
        None
    }

    /// Put the layer back as it was. No history entry.
    pub fn cancel(self, _doc: &mut Document) {
        // TRANSFORM
    }
}

/// Maps a destination pixel centre to a source position (affine now,
/// projective later).
pub trait DestMap {
    fn src(&self, x: f32, y: f32) -> [f32; 2];
}

/// `sel` moved by `xf` (bilinear) on a `w`×`h` page.
pub fn transform_mask(sel: &Selection, _xf: &Affine64, _w: u32, _h: u32) -> Selection {
    // TRANSFORM
    sel.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_compose_about_the_pivot() {
        let pivot = [10.0, 20.0];
        assert_eq!(XfParams::identity(pivot).affine().apply([3.0, 4.0]), [3.0, 4.0]);
        let p = XfParams { t: [5.0, -1.0], s: [2.0, -1.0], theta: std::f64::consts::FRAC_PI_2, pivot };
        let a = p.affine();
        // The pivot only translates.
        let q = a.apply(pivot);
        assert!((q[0] - 15.0).abs() < 1e-9 && (q[1] - 19.0).abs() < 1e-9, "{q:?}");
        // (pivot + (1, 0)) scales to +2 in x, then turns to +2 in y.
        let q = a.apply([11.0, 20.0]);
        assert!((q[0] - 15.0).abs() < 1e-9 && (q[1] - 21.0).abs() < 1e-9, "{q:?}");
    }
}
