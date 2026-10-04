//! Canvas view transform: pan, zoom, rotate and horizontal flip.
//!
//! `screen = R(rotation) · S(zoom) · F(flip) · (doc − center) + origin`
//! where `origin` is the canvas widget's center in screen pixels. Zoom is
//! physical screen pixels per document pixel, so 1.0 is a true 100%.

use std::f32::consts::{PI, TAU};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine2 {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub tx: f32,
    pub ty: f32,
}

impl Affine2 {
    #[inline]
    pub fn apply(&self, p: [f32; 2]) -> [f32; 2] {
        [self.a * p[0] + self.b * p[1] + self.tx, self.c * p[0] + self.d * p[1] + self.ty]
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    /// Document point shown at the canvas center.
    pub center: [f32; 2],
    pub zoom: f32,
    /// Radians, clockwise on screen.
    pub rotation: f32,
    pub flip_x: bool,
}

impl Default for View {
    fn default() -> Self {
        Self { center: [0.0, 0.0], zoom: 1.0, rotation: 0.0, flip_x: false }
    }
}

pub const MIN_ZOOM: f32 = 0.01;
pub const MAX_ZOOM: f32 = 64.0;

/// Preset zoom stops (CSP-like), used by zoom in/out commands.
pub const ZOOM_STEPS: &[f32] = &[
    0.01, 0.02, 0.03, 0.05, 0.0625, 0.0833, 0.1, 0.125, 0.167, 0.25, 0.333, 0.5, 0.667, 1.0, 1.5, 2.0, 3.0, 4.0,
    6.0, 8.0, 12.0, 16.0, 24.0, 32.0, 48.0, 64.0,
];

impl View {
    /// Doc → screen (physical pixels), given the canvas center on screen.
    pub fn doc_to_screen(&self, origin: [f32; 2]) -> Affine2 {
        let (s, c) = self.rotation.sin_cos();
        let f = if self.flip_x { -1.0 } else { 1.0 };
        let z = self.zoom;
        // M = R·S·F
        let a = c * z * f;
        let b = -s * z;
        let cc = s * z * f;
        let d = c * z;
        let tx = origin[0] - (a * self.center[0] + b * self.center[1]);
        let ty = origin[1] - (cc * self.center[0] + d * self.center[1]);
        Affine2 { a, b, c: cc, d, tx, ty }
    }

    pub fn screen_to_doc(&self, origin: [f32; 2]) -> Affine2 {
        let m = self.doc_to_screen(origin);
        let det = m.a * m.d - m.b * m.c;
        let inv = 1.0 / det;
        let a = m.d * inv;
        let b = -m.b * inv;
        let c = -m.c * inv;
        let d = m.a * inv;
        Affine2 { a, b, c, d, tx: -(a * m.tx + b * m.ty), ty: -(c * m.tx + d * m.ty) }
    }

    /// Keep the document point under `screen` fixed while scaling.
    pub fn zoom_at(&mut self, origin: [f32; 2], screen: [f32; 2], new_zoom: f32) {
        let anchor = self.screen_to_doc(origin).apply(screen);
        self.zoom = new_zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        self.keep(origin, anchor, screen);
    }

    pub fn rotate_at(&mut self, origin: [f32; 2], screen: [f32; 2], new_rotation: f32) {
        let anchor = self.screen_to_doc(origin).apply(screen);
        self.rotation = wrap_angle(new_rotation);
        self.keep(origin, anchor, screen);
    }

    /// Flip horizontally around the canvas center.
    pub fn toggle_flip(&mut self) {
        self.flip_x = !self.flip_x;
    }

    /// Move the view by a screen-space delta.
    pub fn pan(&mut self, origin: [f32; 2], delta: [f32; 2]) {
        let inv = self.screen_to_doc(origin);
        let p0 = inv.apply(origin);
        let p1 = inv.apply([origin[0] - delta[0], origin[1] - delta[1]]);
        self.center[0] += p1[0] - p0[0];
        self.center[1] += p1[1] - p0[1];
    }

    /// Shift `center` so that doc point `anchor` lands on `screen`.
    fn keep(&mut self, origin: [f32; 2], anchor: [f32; 2], screen: [f32; 2]) {
        let now = self.doc_to_screen(origin).apply(anchor);
        self.pan(origin, [screen[0] - now[0], screen[1] - now[1]]);
    }

    /// Fit a `w × h` document into a viewport of `vw × vh` pixels with margin.
    pub fn fit(&mut self, w: f32, h: f32, vw: f32, vh: f32) {
        self.center = [w * 0.5, h * 0.5];
        self.rotation = 0.0;
        let zx = vw / (w * 1.08);
        let zy = vh / (h * 1.08);
        self.zoom = zx.min(zy).clamp(MIN_ZOOM, MAX_ZOOM);
    }

    pub fn next_zoom_step(&self, up: bool) -> f32 {
        let z = self.zoom;
        if up {
            ZOOM_STEPS.iter().copied().find(|&s| s > z * 1.001).unwrap_or(MAX_ZOOM)
        } else {
            ZOOM_STEPS.iter().rev().copied().find(|&s| s < z * 0.999).unwrap_or(MIN_ZOOM)
        }
    }
}

pub fn wrap_angle(a: f32) -> f32 {
    let mut a = a % TAU;
    if a > PI {
        a -= TAU;
    } else if a < -PI {
        a += TAU;
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
    }

    fn view() -> View {
        View { center: [300.0, 200.0], zoom: 1.7, rotation: 0.6, flip_x: true }
    }

    #[test]
    fn inverse_round_trips() {
        let v = view();
        let o = [400.0, 300.0];
        let p = [123.0, -45.0];
        let s = v.doc_to_screen(o).apply(p);
        assert!(near(v.screen_to_doc(o).apply(s), p));
    }

    #[test]
    fn center_maps_to_origin() {
        let v = view();
        assert!(near(v.doc_to_screen([400.0, 300.0]).apply(v.center), [400.0, 300.0]));
    }

    #[test]
    fn zoom_and_rotate_keep_anchor_fixed() {
        let o = [400.0, 300.0];
        let mut v = view();
        let screen = [100.0, 50.0];
        let anchor = v.screen_to_doc(o).apply(screen);
        v.zoom_at(o, screen, 4.0);
        assert!(near(v.doc_to_screen(o).apply(anchor), screen));
        v.rotate_at(o, screen, -1.2);
        assert!(near(v.doc_to_screen(o).apply(anchor), screen));
    }

    #[test]
    fn pan_moves_content_with_cursor() {
        let o = [400.0, 300.0];
        let mut v = view();
        let p = [10.0, 10.0];
        let before = v.doc_to_screen(o).apply(p);
        v.pan(o, [25.0, -5.0]);
        let after = v.doc_to_screen(o).apply(p);
        assert!(near(after, [before[0] + 25.0, before[1] - 5.0]));
    }

    #[test]
    fn zoom_steps_move_monotonically() {
        let v = View { zoom: 1.0, ..Default::default() };
        assert_eq!(v.next_zoom_step(true), 1.5);
        assert_eq!(v.next_zoom_step(false), 0.667);
    }
}
