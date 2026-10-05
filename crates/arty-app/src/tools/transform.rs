//! Free transform (Ctrl+T) handles and the Move tool (K). Owned by
//! TRANSFORM.
//!
//! The session lives in `Studio::transform`. Handle drags only change its
//! params; [`TransformTool::tick`] previews the latest params once per frame
//! (nearest while dragging, bilinear after 150 ms idle) and the commit
//! resamples with the chosen filter. All handle math is in pure functions
//! over [`XfParams`] in document pixels.

use std::f64::consts::{FRAC_PI_2, PI, TAU};
use std::time::Instant;

use arty_core::transform::{Filter, FloatSession, XfParams, XfRefused, XfTarget};
use arty_core::{Document, RectF};
use egui::{Color32, CursorIcon, Pos2, Shape, Stroke};
use serde::{Deserialize, Serialize};

use super::{CanvasTool, ToolCtx, ToolInput};
use crate::commands::Command;
use crate::shell::Shell;
use crate::studio::{Studio, Tool};

/// Handles are hit within this many screen points.
const HIT_PT: f64 = 8.0;
/// The rotate knob sits this many screen points outside the top edge.
const KNOB_PT: f64 = 24.0;
/// The preview is refined this long after the last change (s).
const IDLE_SECS: f64 = 0.15;
/// A preview slower than this skips intermediate states while dragging (s).
const SLOW_PREVIEW_SECS: f64 = 0.033;
/// Smallest scale magnitude a handle drag produces (keeps the map invertible).
const MIN_SCALE: f64 = 1e-3;

/// What a press on the transform box grabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    /// 0 top-left, 1 top-right, 2 bottom-right, 3 bottom-left (source box).
    Corner(usize),
    /// Edge `i` runs from corner `i` to corner `i + 1`: 0 top, 1 right,
    /// 2 bottom, 3 left.
    Edge(usize),
    /// The knob above the top edge, or anywhere outside the box.
    Rotate,
    Pivot,
    /// Inside the box.
    Move,
}

struct Drag {
    handle: Handle,
    /// Press point, doc px.
    start: [f64; 2],
    /// Params at the press.
    p0: XfParams,
    /// The Move tool's drag: its session ends with the drag.
    move_tool: bool,
}

#[derive(Default)]
pub struct TransformTool {
    drag: Option<Drag>,
    hover: Option<Handle>,
    /// Pixels per point as of the last tick (0 = not seen yet).
    ppp: f32,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TransformOptions {
    /// The filter a commit resamples with.
    pub filter: Filter,
}

/// A transform session in progress (`Studio::transform`).
pub struct TransformState {
    pub session: FloatSession,
    /// The Move tool's translate-only drag.
    pub move_only: bool,
    /// The params changed since the last preview.
    dirty: bool,
    /// The filter of the preview on screen (`None` before the first).
    shown: Option<Filter>,
    /// When it was made (egui time, s).
    shown_at: f64,
    /// How long it took (s).
    cost: f64,
}

impl TransformState {
    fn new(session: FloatSession, move_only: bool) -> Self {
        Self { session, move_only, dirty: false, shown: None, shown_at: 0.0, cost: 0.0 }
    }

    /// Show `p` from the next frame on (the preview runs in the tool's tick).
    pub fn request(&mut self, p: XfParams) {
        if p != self.session.params() {
            self.session.set_params(p);
            self.dirty = true;
        }
    }

    /// Preview the latest params: nearest while they change, the idle
    /// filter once they have rested for [`IDLE_SECS`].
    fn refresh(&mut self, doc: &mut Document, now: f64, dragging: bool, final_filter: Filter) {
        let idle = idle_filter(final_filter);
        let filter = if self.dirty {
            // A slow preview skips states rather than queueing behind them.
            if dragging && self.cost > SLOW_PREVIEW_SECS && now - self.shown_at < self.cost {
                return;
            }
            Filter::Nearest
        } else if !dragging && self.shown.is_some_and(|f| f != idle) && now - self.shown_at >= IDLE_SECS {
            idle
        } else {
            return;
        };
        let started = Instant::now();
        let p = self.session.params();
        self.session.preview(doc, p, filter);
        self.cost = started.elapsed().as_secs_f64();
        self.dirty = false;
        self.shown = Some(filter);
        self.shown_at = now;
    }

    /// Seconds until the preview wants another frame, if it does.
    fn repaint_in(&self, final_filter: Filter, now: f64) -> Option<f64> {
        if self.dirty {
            return Some(0.0);
        }
        let idle = idle_filter(final_filter);
        self.shown.filter(|&f| f != idle).map(|_| (self.shown_at + IDLE_SECS - now).max(0.0))
    }
}

/// The resting preview's filter: never better than the final one.
fn idle_filter(final_filter: Filter) -> Filter {
    if final_filter == Filter::Nearest { Filter::Nearest } else { Filter::Bilinear }
}

impl Studio {
    /// Start a session on the active layer (`move_only`: the Move tool's
    /// translate-only drag). False, with a notice, when refused. With a
    /// session already running, keeps it.
    pub fn begin_transform(&mut self, move_only: bool) -> bool {
        if let Some(st) = self.transform.as_mut() {
            st.move_only &= move_only;
            return true;
        }
        if self.engine.is_stroking() {
            return false;
        }
        let id = self.doc.active();
        match FloatSession::begin(&mut self.doc, id) {
            Ok(session) => {
                self.transform = Some(TransformState::new(session, move_only));
                true
            }
            Err(why) => {
                self.notice = Some(
                    match why {
                        XfRefused::Folder => "Folders can't be transformed",
                        XfRefused::Locked => "Layer is locked",
                        XfRefused::Empty => "Nothing to transform",
                        XfRefused::Unsupported => "Select a raster layer to transform",
                    }
                    .into(),
                );
                false
            }
        }
    }

    /// Commit the session as one history step; no-op without one.
    pub fn commit_transform(&mut self) {
        let Some(st) = self.transform.take() else { return };
        let moved = (st.session.target() == XfTarget::Selection).then(|| st.session.affine());
        if let Some(edit) = st.session.commit(&mut self.doc, self.opts.transform.filter) {
            self.record_edit(edit);
            if let Some(xf) = moved {
                self.ants_carry = Some(((self.doc_epoch, self.doc.selection_rev()), xf));
            }
        }
    }

    /// Drop the session, restoring the layer; no-op without one.
    pub fn cancel_transform(&mut self) {
        if let Some(st) = self.transform.take() {
            st.session.cancel(&mut self.doc);
        }
    }
}

// ----- handle math (doc px) ------------------------------------------------

/// Source-box corners: top-left, top-right, bottom-right, bottom-left.
pub fn box_corners(r: RectF) -> [[f64; 2]; 4] {
    let (x0, y0, x1, y1) = (r.x as f64, r.y as f64, (r.x + r.w) as f64, (r.y + r.h) as f64);
    [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
}

/// Midpoint of edge `i` of the source box.
fn edge_mid(r: RectF, i: usize) -> [f64; 2] {
    let c = box_corners(r);
    let (a, b) = (c[i % 4], c[(i + 1) % 4]);
    [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5]
}

fn clamp_scale(s: f64) -> f64 {
    if !s.is_finite() {
        return 1.0;
    }
    if s.abs() < MIN_SCALE { MIN_SCALE.copysign(if s == 0.0 { 1.0 } else { s }) } else { s }
}

/// Scale so that source point `handle` follows `pointer` while source
/// point `anchor` stays where it is. `axes` picks the axes that scale;
/// `keep_aspect` scales both by the pointer's projection on the diagonal.
fn scale_about(p0: &XfParams, anchor: [f64; 2], handle: [f64; 2], pointer: [f64; 2], axes: [bool; 2], keep_aspect: bool) -> XfParams {
    let xa = p0.affine().apply(anchor);
    let (sin, cos) = p0.theta.sin_cos();
    let (dx, dy) = (pointer[0] - xa[0], pointer[1] - xa[1]);
    // The pointer in the box's frame: rotated back, still scaled.
    let d = [cos * dx + sin * dy, -sin * dx + cos * dy];
    let e = [handle[0] - anchor[0], handle[1] - anchor[1]];
    let mut s = p0.s;
    if keep_aspect {
        let e0 = [p0.s[0] * e[0], p0.s[1] * e[1]];
        let ee = e0[0] * e0[0] + e0[1] * e0[1];
        if ee > 1e-12 {
            let k = (d[0] * e0[0] + d[1] * e0[1]) / ee;
            s = [p0.s[0] * k, p0.s[1] * k];
        }
    } else {
        for k in 0..2 {
            if axes[k] && e[k].abs() > 1e-9 {
                s[k] = d[k] / e[k];
            }
        }
    }
    let s = s.map(clamp_scale);
    // Keep the anchor: R·S'·(A − p) + p + t' = xa.
    let [px, py] = p0.pivot;
    let v = [s[0] * (anchor[0] - px), s[1] * (anchor[1] - py)];
    let rv = [cos * v[0] - sin * v[1], sin * v[0] + cos * v[1]];
    XfParams { s, t: [xa[0] - px - rv[0], xa[1] - py - rv[1]], ..*p0 }
}

/// Corner `i` follows `pointer`: about the opposite corner, or the pivot
/// with `about_pivot` (Alt). `keep_aspect` is the corner default (Shift
/// toggles it).
pub fn drag_corner(p0: &XfParams, r: RectF, i: usize, pointer: [f64; 2], about_pivot: bool, keep_aspect: bool) -> XfParams {
    let c = box_corners(r);
    let anchor = if about_pivot { p0.pivot } else { c[(i + 2) % 4] };
    scale_about(p0, anchor, c[i % 4], pointer, [true, true], keep_aspect)
}

/// Edge `i` follows `pointer` along its normal: one axis of the box scales,
/// about the opposite edge or the pivot.
pub fn drag_edge(p0: &XfParams, r: RectF, i: usize, pointer: [f64; 2], about_pivot: bool) -> XfParams {
    let anchor = if about_pivot { p0.pivot } else { edge_mid(r, i + 2) };
    let axes = if i.is_multiple_of(2) { [false, true] } else { [true, false] };
    scale_about(p0, anchor, edge_mid(r, i), pointer, axes, false)
}

/// Wrap to (−π, π].
fn wrap(theta: f64) -> f64 {
    let t = (theta + PI).rem_euclid(TAU) - PI;
    if t <= -PI { t + TAU } else { t }
}

/// Rotate about the pivot by the angle the pointer swept since `start`;
/// `snap` (Shift) rounds to 15°.
pub fn drag_rotate(p0: &XfParams, start: [f64; 2], pointer: [f64; 2], snap: bool) -> XfParams {
    let c = [p0.pivot[0] + p0.t[0], p0.pivot[1] + p0.t[1]];
    let angle = |p: [f64; 2]| (p[1] - c[1]).atan2(p[0] - c[0]);
    let mut theta = p0.theta + angle(pointer) - angle(start);
    if snap {
        let step = 15f64.to_radians();
        theta = (theta / step).round() * step;
    }
    XfParams { theta: wrap(theta), ..*p0 }
}

/// Translate by the pointer's travel. `lock_axis` (Shift) keeps only the
/// larger component; `whole_px` rounds the result (the Move tool).
pub fn drag_move(p0: &XfParams, start: [f64; 2], pointer: [f64; 2], lock_axis: bool, whole_px: bool) -> XfParams {
    let mut d = [pointer[0] - start[0], pointer[1] - start[1]];
    if lock_axis {
        let k = if d[0].abs() >= d[1].abs() { 1 } else { 0 };
        d[k] = 0.0;
    }
    let mut t = [p0.t[0] + d[0], p0.t[1] + d[1]];
    if whole_px {
        t = t.map(f64::round);
    }
    XfParams { t, ..*p0 }
}

/// Move the pivot to `pointer` without moving the content.
pub fn drag_pivot(p0: &XfParams, pointer: [f64; 2]) -> XfParams {
    let Some(inv) = p0.affine().inverse() else { return *p0 };
    let pivot = inv.apply(pointer);
    XfParams { pivot, t: [pointer[0] - pivot[0], pointer[1] - pivot[1]], ..*p0 }
}

/// Where the handles are, in doc px: corners, edge midpoints, knob, pivot.
struct HandlePoints {
    corners: [[f64; 2]; 4],
    mids: [[f64; 2]; 4],
    knob: [f64; 2],
    pivot: [f64; 2],
}

/// `knob_doc`: the knob's distance from the top edge in doc px.
fn handle_points(p: &XfParams, r: RectF, knob_doc: f64) -> HandlePoints {
    let xf = p.affine();
    let corners = box_corners(r).map(|q| xf.apply(q));
    let mids: [[f64; 2]; 4] = std::array::from_fn(|i| xf.apply(edge_mid(r, i)));
    // Outward from the bottom edge through the top edge.
    let (top, bottom) = (mids[0], mids[2]);
    let (ux, uy) = (top[0] - bottom[0], top[1] - bottom[1]);
    let len = (ux * ux + uy * uy).sqrt();
    let (nx, ny) = if len > 1e-9 { (ux / len, uy / len) } else { (0.0, -1.0) };
    let knob = [top[0] + nx * knob_doc, top[1] + ny * knob_doc];
    HandlePoints { corners, mids, knob, pivot: [p.pivot[0] + p.t[0], p.pivot[1] + p.t[1]] }
}

/// The handle under `pt` (doc px); `tol` is the hit radius and `knob` the
/// knob distance, both in doc px. Outside the box everything rotates.
pub fn hit_test(p: &XfParams, r: RectF, pt: [f64; 2], tol: f64, knob: f64) -> Handle {
    let h = handle_points(p, r, knob);
    let near = |q: [f64; 2]| (q[0] - pt[0]).hypot(q[1] - pt[1]) <= tol;
    if near(h.pivot) {
        return Handle::Pivot;
    }
    if let Some(i) = h.corners.iter().position(|&q| near(q)) {
        return Handle::Corner(i);
    }
    if let Some(i) = h.mids.iter().position(|&q| near(q)) {
        return Handle::Edge(i);
    }
    if near(h.knob) {
        return Handle::Rotate;
    }
    let inside = p.affine().inverse().is_some_and(|inv| {
        let q = inv.apply(pt);
        let [x0, y0] = box_corners(r)[0];
        let [x1, y1] = box_corners(r)[2];
        q[0] >= x0 && q[0] <= x1 && q[1] >= y0 && q[1] <= y1
    });
    if inside { Handle::Move } else { Handle::Rotate }
}

/// Flip and quarter-turn results land on whole pixels when the box is
/// axis-aligned, so nearest and bicubic stay crisp.
fn snap_to_grid(p: XfParams) -> XfParams {
    let a = p.affine();
    let unit = |v: f64| v.abs() < 1e-9 || (v.abs() - 1.0).abs() < 1e-9;
    if ![a.m[0], a.m[1], a.m[3], a.m[4]].into_iter().all(unit) {
        return p;
    }
    let (dx, dy) = (a.m[2].round() - a.m[2], a.m[5].round() - a.m[5]);
    XfParams { t: [p.t[0] + dx, p.t[1] + dy], ..p }
}

impl TransformTool {
    /// Doc px per screen point.
    fn doc_per_pt(&self, studio: &Studio) -> f64 {
        let ppp = if self.ppp > 0.0 { self.ppp } else { 1.0 };
        ppp as f64 / studio.view.zoom.max(1e-6) as f64
    }

    fn handle_at(&self, studio: &Studio, pt: [f64; 2]) -> Option<Handle> {
        let st = studio.transform.as_ref()?;
        if st.move_only {
            return Some(Handle::Move);
        }
        let k = self.doc_per_pt(studio);
        Some(hit_test(&st.session.params(), st.session.src_bounds(), pt, HIT_PT * k, KNOB_PT * k))
    }

    /// The params for the drag in progress at `pointer`.
    fn dragged(&self, studio: &Studio, input: &ToolInput) -> Option<XfParams> {
        let d = self.drag.as_ref()?;
        let st = studio.transform.as_ref()?;
        let r = st.session.src_bounds();
        let pt = doc_pt(input);
        let m = input.mods;
        Some(match d.handle {
            Handle::Move => drag_move(&d.p0, d.start, pt, m.shift, d.move_tool),
            Handle::Corner(i) => drag_corner(&d.p0, r, i, pt, m.alt, !m.shift),
            Handle::Edge(i) => drag_edge(&d.p0, r, i, pt, m.alt),
            Handle::Rotate => drag_rotate(&d.p0, d.start, pt, m.shift),
            Handle::Pivot => drag_pivot(&d.p0, pt),
        })
    }
}

fn doc_pt(input: &ToolInput) -> [f64; 2] {
    [input.doc[0] as f64, input.doc[1] as f64]
}

impl CanvasTool for TransformTool {
    fn press(&mut self, ctx: &mut ToolCtx, input: ToolInput) {
        if ctx.studio.transform.is_none() && (ctx.studio.tool != Tool::Move || !ctx.studio.begin_transform(true)) {
            return;
        }
        let pt = doc_pt(&input);
        let Some(handle) = self.handle_at(ctx.studio, pt) else { return };
        let Some(st) = ctx.studio.transform.as_ref() else { return };
        self.drag = Some(Drag { handle, start: pt, p0: st.session.params(), move_tool: st.move_only });
    }

    fn drag(&mut self, ctx: &mut ToolCtx, input: ToolInput) {
        if let Some(p) = self.dragged(ctx.studio, &input)
            && let Some(st) = ctx.studio.transform.as_mut()
        {
            st.request(p);
        }
    }

    fn release(&mut self, ctx: &mut ToolCtx, input: ToolInput) {
        self.drag(ctx, input);
        let Some(d) = self.drag.take() else { return };
        if d.move_tool {
            // One step per Move drag; a zero-length drag commits nothing.
            ctx.studio.commit_transform();
        }
    }

    fn hover(&mut self, studio: &Studio, input: ToolInput) {
        self.hover = self.handle_at(studio, doc_pt(&input));
    }

    fn key(&mut self, ctx: &mut ToolCtx, key: egui::Key) -> bool {
        match key {
            egui::Key::Enter => {
                self.drag = None;
                ctx.studio.commit_transform();
                true
            }
            egui::Key::Escape => {
                self.drag = None;
                ctx.studio.cancel_transform();
                true
            }
            _ => false,
        }
    }

    fn cancel(&mut self, ctx: &mut ToolCtx) {
        // Only the drag is abandoned; the session stays (except the Move
        // tool's, which lives for one drag).
        let Some(d) = self.drag.take() else { return };
        if d.move_tool {
            ctx.studio.cancel_transform();
        } else if let Some(st) = ctx.studio.transform.as_mut() {
            st.request(d.p0);
        }
    }

    fn tick(&mut self, ctx: &mut ToolCtx, now: f64) {
        self.ppp = ctx.ppp;
        let final_filter = ctx.studio.opts.transform.filter;
        let dragging = self.drag.is_some();
        if let Some(st) = ctx.studio.transform.as_mut() {
            st.refresh(&mut ctx.studio.doc, now, dragging, final_filter);
        } else {
            self.drag = None;
        }
    }

    fn paint(&self, studio: &Studio, painter: &egui::Painter, origin: [f32; 2], ppp: f32) {
        let Some(st) = studio.transform.as_ref() else { return };
        let ctx = painter.ctx();
        let now = ctx.input(|i| i.time);
        if let Some(secs) = st.repaint_in(studio.opts.transform.filter, now) {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(secs));
        }
        let m = studio.view.doc_to_screen(origin);
        let to_screen = |p: [f64; 2]| {
            let s = m.apply([p[0] as f32, p[1] as f32]);
            Pos2::new(s[0] / ppp, s[1] / ppp)
        };
        let k = studio.view.zoom.max(1e-6) as f64 / ppp as f64;
        let h = handle_points(&st.session.params(), st.session.src_bounds(), KNOB_PT / k);
        let quad: Vec<Pos2> = h.corners.iter().map(|&p| to_screen(p)).collect();
        let (dark, light) = (Stroke::new(3.0, Color32::from_black_alpha(140)), Stroke::new(1.0, Color32::from_rgb(80, 170, 255)));
        painter.add(Shape::closed_line(quad.clone(), dark));
        painter.add(Shape::closed_line(quad, light));
        if st.move_only {
            return;
        }
        let top = to_screen(h.mids[0]);
        let knob = to_screen(h.knob);
        painter.line_segment([top, knob], light);
        painter.circle(knob, 4.5, Color32::WHITE, Stroke::new(1.0, Color32::BLACK));
        for &p in h.corners.iter().chain(&h.mids) {
            let r = egui::Rect::from_center_size(to_screen(p), egui::vec2(7.0, 7.0));
            painter.rect(r, 0.0, Color32::WHITE, Stroke::new(1.0, Color32::BLACK), egui::StrokeKind::Middle);
        }
        let pv = to_screen(h.pivot);
        painter.circle_stroke(pv, 5.0, Stroke::new(1.0, Color32::BLACK));
        painter.circle_stroke(pv, 4.0, Stroke::new(1.0, Color32::WHITE));
        for d in [egui::vec2(7.0, 0.0), egui::vec2(0.0, 7.0)] {
            painter.line_segment([pv - d, pv + d], Stroke::new(1.0, Color32::from_rgb(80, 170, 255)));
        }
    }

    fn cursor(&self, studio: &Studio) -> CursorIcon {
        if studio.transform.is_none() {
            return if studio.tool == Tool::Move { CursorIcon::Move } else { CursorIcon::Default };
        }
        match self.drag.as_ref().map(|d| d.handle).or(self.hover) {
            Some(Handle::Move) => CursorIcon::Move,
            Some(h @ (Handle::Corner(_) | Handle::Edge(_))) => resize_cursor(studio, h),
            Some(Handle::Rotate) => CursorIcon::AllScroll,
            Some(Handle::Pivot) | None => CursorIcon::Crosshair,
        }
    }

    fn gesture_active(&self) -> bool {
        self.drag.is_some()
    }
}

/// The resize cursor along the screen direction from the box centre to
/// corner or edge handle `h`, as `paint` places it (box rotation, mirroring
/// and the view's rotation and flip included).
fn resize_cursor(studio: &Studio, h: Handle) -> CursorIcon {
    let Some(st) = studio.transform.as_ref() else { return CursorIcon::Crosshair };
    let (p, r) = (st.session.params(), st.session.src_bounds());
    let pts = handle_points(&p, r, 0.0);
    let at = match h {
        Handle::Corner(i) => pts.corners[i % 4],
        Handle::Edge(i) => pts.mids[i % 4],
        _ => return CursorIcon::Crosshair,
    };
    let centre = p.affine().apply([(r.x + r.w * 0.5) as f64, (r.y + r.h * 0.5) as f64]);
    // A translation does not change directions: any origin will do.
    let m = studio.view.doc_to_screen([0.0, 0.0]);
    let (a, b) = (m.apply([centre[0] as f32, centre[1] as f32]), m.apply([at[0] as f32, at[1] as f32]));
    resize_icon(b[0] - a[0], b[1] - a[1])
}

/// The resize cursor for a screen direction (y down), to the nearest 45°.
fn resize_icon(dx: f32, dy: f32) -> CursorIcon {
    let step = std::f32::consts::FRAC_PI_4;
    match (dy.atan2(dx).rem_euclid(std::f32::consts::PI) / step).round() as i32 % 4 {
        0 => CursorIcon::ResizeHorizontal,
        // Down-right and up-left.
        1 => CursorIcon::ResizeNwSe,
        2 => CursorIcon::ResizeVertical,
        _ => CursorIcon::ResizeNeSw,
    }
}

/// Transform, CommitTransform, CancelTransform, FlipTransform and
/// RotateTransform90.
pub fn execute(cmd: Command, studio: &mut Studio, _shell: &mut Shell) {
    match cmd {
        Command::Transform => {
            studio.begin_transform(false);
        }
        Command::CommitTransform => studio.commit_transform(),
        Command::CancelTransform => studio.cancel_transform(),
        Command::FlipTransform { horizontal } => {
            if studio.begin_transform(false)
                && let Some(st) = studio.transform.as_mut()
            {
                let mut p = st.session.params();
                p.s[usize::from(!horizontal)] *= -1.0;
                st.request(snap_to_grid(p));
            }
        }
        Command::RotateTransform90 { cw } => {
            if studio.begin_transform(false)
                && let Some(st) = studio.transform.as_mut()
            {
                let mut p = st.session.params();
                // Positive θ turns clockwise on screen (doc y points down).
                p.theta = wrap(p.theta + if cw { FRAC_PI_2 } else { -FRAC_PI_2 });
                st.request(snap_to_grid(p));
            }
        }
        _ => {}
    }
}

/// Tool Property for Move and for a transform session.
pub fn property_ui(ui: &mut egui::Ui, studio: &mut Studio, _shell: &mut Shell) {
    let filter = &mut studio.opts.transform.filter;
    egui::Grid::new("transform-filter").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label("Interpolation");
        egui::ComboBox::from_id_salt("transform-interp").selected_text(filter_label(*filter)).show_ui(ui, |ui| {
            for f in [Filter::Nearest, Filter::Bilinear, Filter::Bicubic] {
                ui.selectable_value(filter, f, filter_label(f));
            }
        });
        ui.end_row();
    });
    let Some(st) = studio.transform.as_mut() else {
        ui.label("Drag on the canvas to move the active layer, or the selected pixels when there is a selection.");
        ui.label("Edit ▸ Transform (Ctrl+T) scales and rotates.");
        return;
    };
    ui.label(match st.session.target() {
        XfTarget::Layer => "Target: layer",
        XfTarget::Selection => "Target: selected pixels",
    });
    let p0 = st.session.params();
    let mut p = p0;
    // Position = where the pivot is on the page.
    let mut pos = [p.pivot[0] + p.t[0], p.pivot[1] + p.t[1]];
    let mut pct = [p.s[0] * 100.0, p.s[1] * 100.0];
    let mut deg = p.theta.to_degrees();
    egui::Grid::new("transform-props").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        let [px, py] = &mut pos;
        for (label, v) in [("X", px), ("Y", py)] {
            ui.label(label);
            ui.add(egui::DragValue::new(v).speed(1.0).suffix(" px").max_decimals(1));
            ui.end_row();
        }
        let [pw, ph] = &mut pct;
        for (label, v) in [("W", pw), ("H", ph)] {
            ui.label(label);
            ui.add(egui::DragValue::new(v).speed(0.5).suffix(" %").max_decimals(1));
            ui.end_row();
        }
        ui.label("Angle");
        ui.add(egui::DragValue::new(&mut deg).speed(0.5).suffix("°").max_decimals(1));
        ui.end_row();
    });
    // Only edited fields change: a round trip through % and degrees is
    // not exact, and a spurious change would re-render every frame.
    let (pos0, pct0, deg0) = ([p.pivot[0] + p.t[0], p.pivot[1] + p.t[1]], [p.s[0] * 100.0, p.s[1] * 100.0], p.theta.to_degrees());
    if pos != pos0 {
        p.t = [pos[0] - p.pivot[0], pos[1] - p.pivot[1]];
    }
    if pct != pct0 {
        p.s = pct.map(|v| clamp_scale(v / 100.0));
    }
    if deg != deg0 {
        p.theta = wrap(deg.to_radians());
    }
    if p != p0 {
        st.request(p);
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui.button("Commit").on_hover_text("Enter").clicked() {
            studio.commit_transform();
        }
        if ui.button("Cancel").on_hover_text("Esc").clicked() {
            studio.cancel_transform();
        }
    });
}

fn filter_label(f: Filter) -> &'static str {
    match f {
        Filter::Nearest => "Nearest",
        Filter::Bilinear => "Bilinear",
        Filter::Bicubic => "Bicubic",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::execute as run;
    use crate::theme::ThemeKind;
    use arty_core::{LayerId, Selection, TileCoord, fix15::ONE_U16, selection::full_mask};

    fn studio() -> (Studio, Shell, LayerId) {
        let mut s = Studio::new(Document::new(256, 256, 72));
        let id = s.doc.active();
        let (g, _) = s.doc.paint_target(id).unwrap();
        for y in 40..60 {
            for x in 30..70 {
                let c = TileCoord::from_pixel(x, y);
                let (ox, oy) = c.origin();
                g.get_mut_or_create(c)[(y - oy) as usize][(x - ox) as usize] = [ONE_U16; 4];
            }
        }
        (s, Shell::new(ThemeKind::Dark), id)
    }

    fn alpha(s: &Studio, x: i32, y: i32) -> u16 {
        let c = TileCoord::from_pixel(x, y);
        let (ox, oy) = c.origin();
        s.doc.active_layer().raster().unwrap().get(c).map_or(0, |t| t[(y - oy) as usize][(x - ox) as usize][3])
    }

    fn shift(s: &mut Studio, dx: f64) {
        let st = s.transform.as_mut().unwrap();
        let p = st.session.params();
        st.request(XfParams { t: [dx, 0.0], ..p });
        st.refresh(&mut s.doc, 0.0, false, Filter::Bicubic);
    }

    #[test]
    fn tr13_session_commands_and_guards() {
        let (mut s, mut shell, _) = studio();
        let rev = s.doc.revision();
        run(Command::CommitTransform, &mut s, &mut shell);
        run(Command::CancelTransform, &mut s, &mut shell);
        assert_eq!((s.history.undo_len(), s.doc.revision()), (0, rev), "no-ops without a session");

        // Undo during a session cancels it and leaves history alone.
        s.clear_active_layer();
        s.undo();
        assert_eq!((s.history.undo_len(), s.history.can_redo()), (0, true));
        let before = s.doc.active_layer().raster().unwrap().clone();
        run(Command::Transform, &mut s, &mut shell);
        assert!(s.transform.is_some());
        shift(&mut s, 30.0);
        assert_eq!(alpha(&s, 95, 50), ONE_U16, "the preview is in the layer");
        s.undo();
        assert!(s.transform.is_none());
        assert!(s.doc.active_layer().raster().unwrap().shares_storage(&before));
        assert_eq!((s.history.undo_len(), s.history.can_redo()), (0, true));
        // Redo is a no-op while a session runs.
        run(Command::Transform, &mut s, &mut shell);
        s.redo();
        assert!(s.transform.is_some() && s.history.can_redo());
        s.cancel_transform();

        // Every guard commits first: one step each, with the pixels moved.
        type Guard = fn(&mut Studio);
        let guards: [(&str, Guard); 7] = [
            ("select_tool", |s| s.select_tool(Tool::Hand)),
            ("edit_structure", |s| {
                s.edit_structure(|d| d.add_raster_layer().is_some());
            }),
            ("set_selection", |s| {
                let mut sel = Selection::new();
                sel.insert_tile(TileCoord::new(3, 3), full_mask().clone());
                s.set_selection(sel);
            }),
            ("begin_stroke", |s| {
                let smp = arty_brush::InputSample { x: 200.0, y: 200.0, pressure: 1.0, ..Default::default() };
                assert!(s.begin_stroke(smp));
                s.end_stroke();
            }),
            ("clear_active_layer", |s| s.clear_active_layer()),
            ("set_layer_props", |s| {
                let id = s.doc.active();
                let p = arty_core::LayerProps { opacity: 0.5, ..s.doc.layer(id).unwrap().props.clone() };
                s.set_layer_props(id, p, false);
            }),
            ("set_page_setup", |s| {
                let trim = RectF { x: 8.0, y: 8.0, w: 200.0, h: 200.0 };
                s.set_page_setup(Some(arty_core::PageSetup { trim, bleed: 4.0, safe: 4.0, inner: RectF::default(), unit: 0 }));
            }),
        ];
        for (name, guard) in guards {
            let (mut s, mut shell, id) = studio();
            run(Command::Transform, &mut s, &mut shell);
            shift(&mut s, 64.0);
            guard(&mut s);
            assert!(s.transform.is_none(), "{name}");
            assert!(s.history.undo_len() >= 1, "{name}");
            s.doc.set_active(id);
            if name != "clear_active_layer" {
                assert_eq!((alpha(&s, 100, 50), alpha(&s, 31, 50)), (ONE_U16, 0), "{name} kept the moved pixels");
            }
            while s.history.can_undo() {
                s.undo();
            }
            s.doc.set_active(id);
            assert_eq!((alpha(&s, 100, 50), alpha(&s, 31, 50)), (0, ONE_U16), "{name}: undo restores");
        }
        // The layer panel sets the props every frame: unchanged ones keep the session.
        let (mut s, mut shell, id) = studio();
        run(Command::Transform, &mut s, &mut shell);
        s.set_layer_props(id, s.doc.layer(id).unwrap().props.clone(), false);
        assert!(s.transform.is_some() && !s.history.can_undo());
        s.cancel_transform();

        // Flip and Rotate without a session start one.
        let (mut s, mut shell, _) = studio();
        run(Command::FlipTransform { horizontal: true }, &mut s, &mut shell);
        let p = s.transform.as_ref().unwrap().session.params();
        assert_eq!((p.s, p.theta), ([-1.0, 1.0], 0.0));
        run(Command::CancelTransform, &mut s, &mut shell);
        run(Command::RotateTransform90 { cw: true }, &mut s, &mut shell);
        let a = s.transform.as_ref().unwrap().session.affine();
        assert!((a.m[3] - 1.0).abs() < 1e-12 && a.m[0].abs() < 1e-12, "cw maps +x to +y: {a:?}");
        assert!((a.m[2] - a.m[2].round()).abs() < 1e-9 && (a.m[5] - a.m[5].round()).abs() < 1e-9, "on whole pixels");
        run(Command::CommitTransform, &mut s, &mut shell);
        assert_eq!(s.history.undo_len(), 1);
        // The 40 × 20 block about (50, 50) is now 20 × 40.
        assert_eq!((alpha(&s, 45, 32), alpha(&s, 45, 67), alpha(&s, 35, 50)), (ONE_U16, ONE_U16, 0));

        // Refusals leave a notice and no session.
        let (mut s, mut shell, _) = studio();
        s.edit_structure(|d| d.add_folder().is_some());
        s.notice = None;
        run(Command::Transform, &mut s, &mut shell);
        assert!(s.transform.is_none() && s.notice.is_some());
    }

    fn close(a: [f64; 2], b: [f64; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6
    }

    #[test]
    fn tr14_handle_math() {
        let r = RectF { x: 0.0, y: 0.0, w: 100.0, h: 50.0 };
        let p0 = XfParams::identity([50.0, 25.0]);
        // Corner 2 (bottom-right) about the opposite corner.
        let p = drag_corner(&p0, r, 2, [150.0, 100.0], false, false);
        assert!(close(p.s, [1.5, 2.0]), "{p:?}");
        assert!(close(p.affine().apply([0.0, 0.0]), [0.0, 0.0]));
        assert!(close(p.affine().apply([100.0, 50.0]), [150.0, 100.0]));
        // Alt: about the pivot.
        let p = drag_corner(&p0, r, 2, [150.0, 100.0], true, false);
        assert!(close(p.s, [2.0, 3.0]) && close(p.affine().apply([50.0, 25.0]), [50.0, 25.0]), "{p:?}");
        // Keep aspect: the projection on the diagonal.
        let p = drag_corner(&p0, r, 2, [150.0, 60.0], false, true);
        assert!(close(p.s, [1.44, 1.44]), "{p:?}");
        // Corner 0 dragged past the anchor mirrors.
        let p = drag_corner(&p0, r, 0, [200.0, 100.0], false, false);
        assert!(p.s[0] < 0.0 && p.s[1] < 0.0);

        // An edge drag on a rotated box scales one local axis only.
        let rot = XfParams { theta: 30f64.to_radians(), t: [7.0, -3.0], ..p0 };
        let xf = rot.affine();
        let target = xf.apply([130.0, 25.0]);
        let (sin, cos) = rot.theta.sin_cos();
        // Off the edge's normal by 12 px along the box's local y: ignored.
        let pointer = [target[0] - sin * 12.0, target[1] + cos * 12.0];
        let p = drag_edge(&rot, r, 1, pointer, false);
        assert!(close(p.s, [1.3, 1.0]), "{p:?}");
        assert!(close(p.affine().apply([0.0, 25.0]), xf.apply([0.0, 25.0])), "the left edge stays");
        assert!(close(p.affine().apply([100.0, 25.0]), target));

        // Rotation about the pivot; Shift snaps to 15°.
        let c = [50.0, 25.0];
        let at = |deg: f64| [c[0] + 100.0 * deg.to_radians().cos(), c[1] + 100.0 * deg.to_radians().sin()];
        let p = drag_rotate(&p0, at(0.0), at(37.0), false);
        assert!((p.theta.to_degrees() - 37.0).abs() < 1e-9);
        let p = drag_rotate(&p0, at(0.0), at(37.0), true);
        assert!((p.theta.to_degrees() - 30.0).abs() < 1e-9);
        assert!(close(p.affine().apply(c), c), "the pivot stays");
        let p = drag_rotate(&p0, at(170.0), at(-170.0), false);
        assert!((p.theta.to_degrees() - 20.0).abs() < 1e-9, "wraps: {}", p.theta.to_degrees());

        // Moving the pivot keeps the content where it is.
        let p = drag_pivot(&rot, [10.0, 90.0]);
        for q in [[0.0, 0.0], [100.0, 50.0], [33.0, 7.0]] {
            assert!(close(p.affine().apply(q), xf.apply(q)));
        }
        assert!(close([p.pivot[0] + p.t[0], p.pivot[1] + p.t[1]], [10.0, 90.0]));

        // Moves: Shift locks the axis; the Move tool lands on whole px.
        let p = drag_move(&p0, [0.0, 0.0], [10.4, 3.0], true, true);
        assert_eq!(p.t, [10.0, 0.0]);

        // Hit testing.
        let hit = |q| hit_test(&p0, r, q, 8.0, 24.0);
        assert_eq!(hit([101.0, 51.0]), Handle::Corner(2));
        assert_eq!(hit([0.0, 25.0]), Handle::Edge(3));
        assert_eq!(hit([50.0, -24.0]), Handle::Rotate);
        assert_eq!(hit([50.0, 25.0]), Handle::Pivot);
        assert_eq!(hit([20.0, 30.0]), Handle::Move);
        assert_eq!(hit([300.0, 300.0]), Handle::Rotate, "outside rotates");
        // A flip keeps the grid; a free angle is left alone.
        let flipped = snap_to_grid(XfParams { s: [-1.0, 1.0], ..XfParams::identity([10.25, 0.0]) });
        assert!((flipped.affine().m[2] - flipped.affine().m[2].round()).abs() < 1e-9);
        let free = XfParams { theta: 0.3, ..XfParams::identity([10.25, 0.0]) };
        assert_eq!(snap_to_grid(free), free);
    }

    fn input(x: f32, y: f32) -> ToolInput {
        ToolInput { screen: [x, y], doc: [x, y], mods: egui::Modifiers::NONE, double: false }
    }

    #[test]
    fn tr15_move_tool_is_one_step_per_drag() {
        let (mut s, mut shell, _) = studio();
        s.select_tool(Tool::Move);
        let mut tool = TransformTool::default();
        let mut ctx = ToolCtx { studio: &mut s, shell: &mut shell, origin: [0.0; 2], ppp: 1.0 };
        tool.press(&mut ctx, input(40.0, 45.0));
        assert!(tool.gesture_active() && ctx.studio.transform.as_ref().is_some_and(|t| t.move_only));
        tool.drag(&mut ctx, input(50.3, 47.0));
        tool.tick(&mut ctx, 0.0);
        tool.drag(&mut ctx, input(60.2, 50.0));
        tool.release(&mut ctx, input(60.2, 50.0));
        assert!(!tool.gesture_active() && ctx.studio.transform.is_none());
        assert_eq!(ctx.studio.history.undo_len(), 1);
        // Moved by (20, 5) on whole pixels.
        assert_eq!((alpha(ctx.studio, 50, 45), alpha(ctx.studio, 89, 64), alpha(ctx.studio, 90, 64)), (ONE_U16, ONE_U16, 0));
        assert_eq!(alpha(ctx.studio, 30, 40), 0);

        // A zero-length drag is no step.
        tool.press(&mut ctx, input(60.0, 50.0));
        tool.release(&mut ctx, input(60.0, 50.0));
        assert!(ctx.studio.transform.is_none());
        assert_eq!(ctx.studio.history.undo_len(), 1);
        // A jitter under half a pixel rounds away too.
        tool.press(&mut ctx, input(60.0, 50.0));
        tool.drag(&mut ctx, input(60.3, 50.2));
        tool.release(&mut ctx, input(60.3, 50.2));
        assert_eq!(ctx.studio.history.undo_len(), 1);

        // Esc during the drag cancels it with no step.
        tool.press(&mut ctx, input(60.0, 50.0));
        tool.drag(&mut ctx, input(90.0, 50.0));
        tool.tick(&mut ctx, 0.0);
        assert!(tool.key(&mut ctx, egui::Key::Escape));
        assert!(ctx.studio.transform.is_none() && !tool.gesture_active());
        assert_eq!(ctx.studio.history.undo_len(), 1);
        assert_eq!(alpha(ctx.studio, 50, 45), ONE_U16);

        // A tool switch mid-drag drops the Move tool's session.
        tool.press(&mut ctx, input(60.0, 50.0));
        tool.drag(&mut ctx, input(90.0, 50.0));
        tool.cancel(&mut ctx);
        assert!(ctx.studio.transform.is_none());
        assert_eq!(alpha(ctx.studio, 50, 45), ONE_U16);
        // Without a Move tool, a press with no session does nothing.
        ctx.studio.select_tool(Tool::Hand);
        tool.press(&mut ctx, input(60.0, 50.0));
        assert!(!tool.gesture_active() && ctx.studio.transform.is_none());
    }

    /// Resize cursors follow the handle on screen: box rotation, mirroring
    /// and the view's flip and rotation.
    #[test]
    fn resize_cursors_follow_the_handles_on_screen() {
        use CursorIcon::{ResizeHorizontal as H, ResizeNeSw as NE, ResizeNwSe as NW, ResizeVertical as V};
        let (mut s, mut shell, _) = studio();
        run(Command::Transform, &mut s, &mut shell);
        let mut tool = TransformTool::default();
        let mut at = |s: &Studio, h: Handle| {
            tool.hover = Some(h);
            tool.cursor(s)
        };
        let cursors = |s: &Studio, at: &mut dyn FnMut(&Studio, Handle) -> CursorIcon| {
            [Handle::Corner(0), Handle::Corner(1), Handle::Edge(0), Handle::Edge(1)].map(|h| at(s, h))
        };
        assert_eq!(cursors(&s, &mut at), [NW, NE, V, H]);
        // A quarter turn of the box: the right edge now runs across.
        let st = s.transform.as_mut().unwrap();
        st.request(XfParams { theta: FRAC_PI_2, ..st.session.params() });
        assert_eq!(cursors(&s, &mut at), [NE, NW, H, V]);
        // A mirrored box swaps the diagonals.
        let st = s.transform.as_mut().unwrap();
        st.request(XfParams { theta: 0.0, s: [-1.0, 1.0], ..st.session.params() });
        assert_eq!(cursors(&s, &mut at), [NE, NW, V, H]);
        // So does a flipped view; a 45° view turns edges into diagonals.
        let st = s.transform.as_mut().unwrap();
        st.request(XfParams { s: [1.0, 1.0], ..st.session.params() });
        s.view.toggle_flip();
        assert_eq!(cursors(&s, &mut at), [NE, NW, V, H]);
        s.view.toggle_flip();
        s.view.rotation = std::f32::consts::FRAC_PI_4;
        assert_eq!(cursors(&s, &mut at)[2..], [NE, NW]);
    }

    #[test]
    fn property_panel_leaves_untouched_params_alone() {
        let (mut s, mut shell, _) = studio();
        run(Command::Transform, &mut s, &mut shell);
        let st = s.transform.as_mut().unwrap();
        let p = XfParams { theta: 0.3, s: [1.37, -0.81], t: [0.1, 7.7], ..st.session.params() };
        st.session.set_params(p);
        let ctx = egui::Context::default();
        for _ in 0..2 {
            ctx.run_ui(egui::RawInput::default(), |ui| property_ui(ui, &mut s, &mut shell)).drop_without_applying_deltas();
        }
        let st = s.transform.as_ref().unwrap();
        assert!(!st.dirty && st.session.params() == p, "drawing the panel changes nothing");
    }

    #[test]
    fn transform_handles_drive_a_session_and_refine_when_idle() {
        let (mut s, mut shell, _) = studio();
        run(Command::Transform, &mut s, &mut shell);
        let mut tool = TransformTool::default();
        let mut ctx = ToolCtx { studio: &mut s, shell: &mut shell, origin: [0.0; 2], ppp: 1.0 };
        // Grab the right edge (70, 50) and pull it to x = 110.
        tool.press(&mut ctx, input(70.0, 50.0));
        assert_eq!(tool.drag.as_ref().map(|d| d.handle), Some(Handle::Edge(1)));
        tool.drag(&mut ctx, input(110.0, 50.0));
        tool.tick(&mut ctx, 1.0);
        let st = ctx.studio.transform.as_ref().unwrap();
        assert_eq!(st.shown, Some(Filter::Nearest));
        assert!((st.session.params().s[0] - 2.0).abs() < 1e-9);
        tool.release(&mut ctx, input(110.0, 50.0));
        assert!(ctx.studio.transform.is_some(), "a handle drag keeps the session");
        tool.tick(&mut ctx, 1.1);
        assert_eq!(ctx.studio.transform.as_ref().unwrap().shown, Some(Filter::Nearest));
        tool.tick(&mut ctx, 1.2);
        assert_eq!(ctx.studio.transform.as_ref().unwrap().shown, Some(Filter::Bilinear), "refined after 150 ms");
        assert!(tool.key(&mut ctx, egui::Key::Enter));
        assert!(ctx.studio.transform.is_none());
        assert_eq!(ctx.studio.history.undo_len(), 1);
        assert_eq!((alpha(ctx.studio, 105, 50), alpha(ctx.studio, 31, 50)), (ONE_U16, ONE_U16));
    }
}
