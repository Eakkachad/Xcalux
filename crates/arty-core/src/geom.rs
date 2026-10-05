//! Small geometry types shared by selections, fills, transforms and frames.
//!
//! All coordinates are document pixels unless a name says otherwise.

use crate::tile::TileCoord;

/// A point in document pixels.
pub type Pt = [f32; 2];

/// An axis-aligned rectangle in document pixels (`x, y` is the top-left
/// corner; `w, h` are never negative when built by this module).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RectF {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl RectF {
    /// Grown by `d` on every side (shrunk when `d < 0`; the size stops at 0).
    pub fn expand(self, d: f32) -> RectF {
        RectF { x: self.x - d, y: self.y - d, w: (self.w + 2.0 * d).max(0.0), h: (self.h + 2.0 * d).max(0.0) }
    }

    /// Half-open: the left and top edges are inside, the right and bottom are not.
    pub fn contains(self, p: Pt) -> bool {
        p[0] >= self.x && p[1] >= self.y && p[0] < self.x + self.w && p[1] < self.y + self.h
    }

    /// The overlap, or `None` when it has no area.
    pub fn intersect(self, o: RectF) -> Option<RectF> {
        let (x0, y0) = (self.x.max(o.x), self.y.max(o.y));
        let (x1, y1) = ((self.x + self.w).min(o.x + o.w), (self.y + self.h).min(o.y + o.h));
        (x1 > x0 && y1 > y0).then_some(RectF { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
    }
}

/// A rectangle of tiles; `x1` and `y1` are exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileRect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl TileRect {
    /// The smallest rectangle holding both.
    pub fn union(self, o: TileRect) -> TileRect {
        TileRect { x0: self.x0.min(o.x0), y0: self.y0.min(o.y0), x1: self.x1.max(o.x1), y1: self.y1.max(o.y1) }
    }

    pub fn contains(self, c: TileCoord) -> bool {
        c.x >= self.x0 && c.y >= self.y0 && c.x < self.x1 && c.y < self.y1
    }
}

/// `x' = m[0]x + m[1]y + m[2]`, `y' = m[3]x + m[4]y + m[5]`.
///
/// f64 so composed transforms don't drift. Named so it does not clash with
/// `arty_render::view::Affine2` (f32, doc ↔ screen).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine64 {
    pub m: [f64; 6],
}

impl Affine64 {
    pub const IDENTITY: Self = Self { m: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0] };

    pub fn translate(x: f64, y: f64) -> Self {
        Self { m: [1.0, 0.0, x, 0.0, 1.0, y] }
    }

    /// A negative factor flips that axis.
    pub fn scale(sx: f64, sy: f64) -> Self {
        Self { m: [sx, 0.0, 0.0, 0.0, sy, 0.0] }
    }

    /// Counter-clockwise in a y-up frame, which is clockwise on screen
    /// (document y points down).
    pub fn rotate(theta: f64) -> Self {
        let (s, c) = theta.sin_cos();
        Self { m: [c, -s, 0.0, s, c, 0.0] }
    }

    /// `self` first, then `o`: `a.then(b).apply(p) == b.apply(a.apply(p))`.
    pub fn then(self, o: Self) -> Self {
        let [a0, a1, a2, a3, a4, a5] = self.m;
        let [b0, b1, b2, b3, b4, b5] = o.m;
        Self {
            m: [
                b0 * a0 + b1 * a3,
                b0 * a1 + b1 * a4,
                b0 * a2 + b1 * a5 + b2,
                b3 * a0 + b4 * a3,
                b3 * a1 + b4 * a4,
                b3 * a2 + b4 * a5 + b5,
            ],
        }
    }

    /// `None` when the map is singular (or not finite).
    pub fn inverse(self) -> Option<Self> {
        let [a, b, c, d, e, f] = self.m;
        let det = a * e - b * d;
        if det == 0.0 || !det.is_finite() {
            return None;
        }
        let inv = 1.0 / det;
        let m = [e * inv, -b * inv, (b * f - c * e) * inv, -d * inv, a * inv, (c * d - a * f) * inv];
        m.iter().all(|v| v.is_finite()).then_some(Self { m })
    }

    pub fn apply(self, p: [f64; 2]) -> [f64; 2] {
        let [a, b, c, d, e, f] = self.m;
        [a * p[0] + b * p[1] + c, d * p[0] + e * p[1] + f]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f64; 2], b: [f64; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
    }

    #[test]
    fn then_applies_left_to_right() {
        let t = Affine64::translate(10.0, -4.0);
        let s = Affine64::scale(2.0, -3.0);
        let r = Affine64::rotate(0.7);
        let p = [3.5, -1.25];
        assert!(close(t.then(s).apply(p), s.apply(t.apply(p))));
        assert!(close(s.then(t).apply(p), t.apply(s.apply(p))));
        assert!(close(t.then(s).then(r).apply(p), r.apply(s.apply(t.apply(p)))));
        assert!(close(t.then(s).then(r).apply(p), t.then(s.then(r)).apply(p)), "associative");
        assert!(close(Affine64::IDENTITY.then(r).apply(p), r.apply(p)));
        assert!(close(r.then(Affine64::IDENTITY).apply(p), r.apply(p)));
        // A quarter turn maps +x to +y (down on screen).
        assert!(close(Affine64::rotate(std::f64::consts::FRAC_PI_2).apply([1.0, 0.0]), [0.0, 1.0]));
    }

    #[test]
    fn inverse_undoes_the_map() {
        let a = Affine64::translate(-7.0, 3.0).then(Affine64::rotate(-1.1)).then(Affine64::scale(0.5, -4.0));
        let inv = a.inverse().unwrap();
        for p in [[0.0, 0.0], [12.0, -5.5], [-1e3, 2e3]] {
            assert!(close(inv.apply(a.apply(p)), p), "{p:?}");
            assert!(close(a.apply(inv.apply(p)), p), "{p:?}");
        }
        let id = a.then(inv);
        for (x, y) in id.m.iter().zip(Affine64::IDENTITY.m) {
            assert!((x - y).abs() < 1e-12);
        }
        assert_eq!(Affine64::scale(0.0, 1.0).inverse(), None);
        assert_eq!(Affine64::scale(f64::NAN, 1.0).inverse(), None);
        assert_eq!(Affine64::IDENTITY.inverse(), Some(Affine64::IDENTITY));
    }

    #[test]
    fn rects() {
        let r = RectF { x: 1.0, y: 2.0, w: 10.0, h: 4.0 };
        assert!(r.contains([1.0, 2.0]) && !r.contains([11.0, 3.0]) && !r.contains([5.0, 6.0]));
        assert_eq!(r.expand(1.0), RectF { x: 0.0, y: 1.0, w: 12.0, h: 6.0 });
        assert_eq!(r.expand(-3.0).h, 0.0);
        let o = RectF { x: 5.0, y: 0.0, w: 20.0, h: 3.0 };
        assert_eq!(r.intersect(o), Some(RectF { x: 5.0, y: 2.0, w: 6.0, h: 1.0 }));
        assert_eq!(r.intersect(RectF { x: 11.0, ..o }), None, "touching edges have no area");
        let a = TileRect { x0: 0, y0: 0, x1: 2, y1: 1 };
        let b = TileRect { x0: -1, y0: 3, x1: 1, y1: 5 };
        assert_eq!(a.union(b), TileRect { x0: -1, y0: 0, x1: 2, y1: 5 });
        assert!(a.contains(TileCoord::new(1, 0)) && !a.contains(TileCoord::new(2, 0)));
    }
}
