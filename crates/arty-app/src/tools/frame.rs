//! Frame border tools: Rectangle Frame, Divide Frame and Frame Edit (O).
//! Owned by FRAMES.
//!
//! Every gesture is one undo step on release: a new frame folder (a
//! structure edit) or one `Edit::Frame`. During a Frame Edit drag only the
//! overlay changes; the frame is rebuilt once, on release.

use std::sync::Arc;

use arty_core::frame::{MAX_PANELS, add_frame_folder};
use arty_core::page::MM_PER_IN;
use arty_core::{BorderStyle, Document, Edit, Frame, FrameShape, LayerId, PageSetup, Panel, Pt, RectF, fix15};
use egui::{Color32, CursorIcon, Pos2, Shape, Stroke};
use serde::{Deserialize, Serialize};

use super::{CanvasTool, ToolCtx, ToolInput};
use crate::commands::{self, Command};
use crate::panels::section;
use crate::shell::Shell;
use crate::studio::{FrameMode, Rgb, Studio, Tool};

/// Edges snap within this many screen points.
pub const SNAP_PT: f32 = 8.0;
/// Vertex handle size (screen points).
const HANDLE_PT: f32 = 7.0;
/// Handles grab within this radius (screen points).
const GRAB_PT: f32 = 6.0;
/// An edge grabs within this distance (screen points).
const EDGE_PT: f32 = 4.0;
/// Rectangles smaller than this (doc px) are ignored.
const MIN_RECT_PX: f32 = 4.0;

const OUTLINE: Color32 = Color32::from_rgb(0, 168, 255);
const CUT_LINE: Color32 = Color32::from_rgb(255, 80, 60);

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
#[serde(default)]
pub struct FrameOptions {
    /// Gutter between panels side by side (mm).
    pub gutter_lr_mm: f32,
    /// Gutter between panels stacked vertically (mm).
    pub gutter_tb_mm: f32,
    /// Border of new frames (mm).
    pub border_mm: f32,
    pub border_color: Rgb,
    /// Rectangle Frame makes a frame folder per panel.
    pub new_folder_per_frame: bool,
    /// Divide Frame moves the second pieces to a new frame folder.
    pub divide_into_folders: bool,
    /// Snap to the canvas edges and the page guides.
    pub snap_guides: bool,
    /// Snap to other panels' edges ± the gutter.
    pub snap_panels: bool,
}

impl Default for FrameOptions {
    fn default() -> Self {
        Self {
            gutter_lr_mm: 2.0,
            gutter_tb_mm: 5.0,
            border_mm: 0.6,
            border_color: [0.0; 3],
            new_folder_per_frame: true,
            divide_into_folders: false,
            snap_guides: true,
            snap_panels: true,
        }
    }
}

fn mm_px(mm: f32, dpi: u32) -> f32 {
    mm / MM_PER_IN * dpi as f32
}

impl FrameOptions {
    /// `(gap_h, gap_v)` in px: between pieces stacked vertically, side by side.
    pub fn gaps(&self, dpi: u32) -> (f32, f32) {
        (mm_px(self.gutter_tb_mm, dpi), mm_px(self.gutter_lr_mm, dpi))
    }

    /// The border of new frames.
    pub fn border(&self, dpi: u32) -> BorderStyle {
        BorderStyle { width: mm_px(self.border_mm, dpi).max(0.0), color: rgb_to_fix15(self.border_color) }
    }
}

fn rgb_to_fix15(c: Rgb) -> [u16; 4] {
    [fix15::from_f32(c[0]), fix15::from_f32(c[1]), fix15::from_f32(c[2]), fix15::ONE_U16]
}

fn fix15_to_rgb(c: [u16; 4]) -> Rgb {
    let a = fix15::to_f32(c[3]);
    if a <= 0.0 { [0.0; 3] } else { [0, 1, 2].map(|i| (fix15::to_f32(c[i]) / a).min(1.0)) }
}

// ----- snapping (pure) -------------------------------------------------------

/// Lines a dragged edge snaps to: x of vertical lines, y of horizontal ones.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SnapTargets {
    pub xs: Vec<f32>,
    pub ys: Vec<f32>,
}

impl SnapTargets {
    /// The canvas edges and page guides (trim, bleed, inner frame) when
    /// `guides`, and when `panels` the edges of the panels other than
    /// `skip`, moved out by the gutters (`gap_h` between pieces stacked
    /// vertically, `gap_v` side by side).
    pub fn new(
        w: u32,
        h: u32,
        page: Option<&PageSetup>,
        others: &[Panel],
        skip: Option<usize>,
        (gap_h, gap_v): (f32, f32),
        guides: bool,
        panels: bool,
    ) -> Self {
        let mut t = SnapTargets::default();
        let rect = |t: &mut SnapTargets, r: RectF| {
            if r.w > 0.0 && r.h > 0.0 {
                t.xs.extend([r.x, r.x + r.w]);
                t.ys.extend([r.y, r.y + r.h]);
            }
        };
        if guides {
            rect(&mut t, RectF { x: 0.0, y: 0.0, w: w as f32, h: h as f32 });
            if let Some(p) = page {
                rect(&mut t, p.trim);
                rect(&mut t, p.bleed_rect(w, h));
                rect(&mut t, p.inner);
            }
        }
        if panels {
            for (i, p) in others.iter().enumerate() {
                if Some(i) == skip {
                    continue;
                }
                let b = p.bounds();
                t.xs.extend([b.x - gap_v, b.x + b.w + gap_v]);
                t.ys.extend([b.y - gap_h, b.y + b.h + gap_h]);
            }
        }
        t
    }

    pub fn snap_x(&self, x: f32, tol: f32) -> Option<f32> {
        nearest(&self.xs, x, tol)
    }

    pub fn snap_y(&self, y: f32, tol: f32) -> Option<f32> {
        nearest(&self.ys, y, tol)
    }

    /// Each side of `r` snapped on its own.
    pub fn snap_rect(&self, r: RectF, tol: f32) -> RectF {
        let x0 = self.snap_x(r.x, tol).unwrap_or(r.x);
        let x1 = self.snap_x(r.x + r.w, tol).unwrap_or(r.x + r.w);
        let y0 = self.snap_y(r.y, tol).unwrap_or(r.y);
        let y1 = self.snap_y(r.y + r.h, tol).unwrap_or(r.y + r.h);
        RectF { x: x0, y: y0, w: (x1 - x0).max(0.0), h: (y1 - y0).max(0.0) }
    }

    /// The shift moving `[lo, hi]` by `d` that lands an end on a line.
    fn snap_span(lines: &[f32], lo: f32, hi: f32, d: f32, tol: f32) -> f32 {
        let a = nearest(lines, lo + d, tol).map(|v| v - lo);
        let b = nearest(lines, hi + d, tol).map(|v| v - hi);
        match (a, b) {
            (Some(a), Some(b)) => {
                if (a - d).abs() <= (b - d).abs() {
                    a
                } else {
                    b
                }
            }
            (Some(v), None) | (None, Some(v)) => v,
            (None, None) => d,
        }
    }
}

fn nearest(lines: &[f32], v: f32, tol: f32) -> Option<f32> {
    lines.iter().copied().filter(|l| (l - v).abs() <= tol).min_by(|a, b| (a - v).abs().total_cmp(&(b - v).abs()))
}

/// The snap distance in document px at a view zoom (physical px per doc
/// px) and `ppp` physical px per screen point.
pub fn snap_tol(ppp: f32, zoom: f32) -> f32 {
    SNAP_PT * ppp / zoom.max(1e-6)
}

/// Ctrl snapping: within `tol` of a canvas edge (0 or `extent`), go
/// `width + 1` px past it so the border leaves the page (bleed panel).
pub fn snap_outside(v: f32, extent: f32, width: f32, tol: f32) -> f32 {
    let out = width.max(0.0) + 1.0;
    if v.abs() <= tol {
        -out
    } else if (v - extent).abs() <= tol {
        extent + out
    } else {
        v
    }
}

/// The rectangle of a drag from `start` to `cur`: Shift makes it square,
/// Alt draws it from the centre.
pub fn drag_rect(start: Pt, cur: Pt, square: bool, centred: bool) -> RectF {
    let (mut dx, mut dy) = (cur[0] - start[0], cur[1] - start[1]);
    if square {
        let s = dx.abs().max(dy.abs());
        (dx, dy) = (s.copysign(dx), s.copysign(dy));
    }
    if centred {
        RectF { x: start[0] - dx.abs(), y: start[1] - dy.abs(), w: 2.0 * dx.abs(), h: 2.0 * dy.abs() }
    } else {
        RectF { x: start[0].min(start[0] + dx), y: start[1].min(start[1] + dy), w: dx.abs(), h: dy.abs() }
    }
}

/// `b` turned about `a` to the nearest multiple of 45°.
fn snap_angle(a: Pt, b: Pt) -> Pt {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = dx.hypot(dy);
    let step = std::f32::consts::FRAC_PI_4;
    let t = (dy.atan2(dx) / step).round() * step;
    [a[0] + len * t.cos(), a[1] + len * t.sin()]
}

/// Whether every vertex lies within a page of the canvas (what files keep).
fn in_bleed_range(p: &Panel, w: u32, h: u32) -> bool {
    let (w, h) = (w as f32, h as f32);
    p.points().iter().all(|q| (-w..=2.0 * w).contains(&q[0]) && (-h..=2.0 * h).contains(&q[1]))
}

// ----- studio operations ----------------------------------------------------

/// The frame folder the frame tools work on: the active layer's.
pub fn target_folder(studio: &Studio) -> Option<LayerId> {
    studio.doc.frame_folder_of(studio.doc.active())
}

impl Studio {
    /// Replace the frame of folder `id` with `f(current)` as one
    /// `Edit::Frame`; no-op when `f` returns `None` or the same shape, and
    /// while stroking.
    pub fn edit_frame(&mut self, id: LayerId, f: impl FnOnce(&FrameShape) -> Option<FrameShape>) {
        if self.engine.is_stroking() {
            return;
        }
        self.commit_transform();
        let Some(cur) = self.doc.frame(id) else { return };
        let Some(shape) = f(cur.shape()).filter(|s| s != cur.shape()) else { return };
        let built = Frame::build(shape, self.doc.width(), self.doc.height());
        if let Some(old) = self.doc.set_frame(id, Some(built)) {
            self.record_edit(Edit::Frame { layer: id, frame: old });
        }
    }

    /// Show `shape` on folder `id` without recording it (live property
    /// drags); [`Self::commit_frame_preview`] records the result.
    pub fn preview_frame(&mut self, id: LayerId, shape: FrameShape) {
        if self.engine.is_stroking() || self.doc.frame(id).is_none_or(|f| *f.shape() == shape) {
            return;
        }
        let built = Frame::build(shape, self.doc.width(), self.doc.height());
        self.doc.set_frame(id, Some(built));
    }

    /// Record the previews made since the frame was `start` as one step.
    pub fn commit_frame_preview(&mut self, id: LayerId, start: Arc<Frame>) {
        if self.doc.frame(id).is_some_and(|f| !Arc::ptr_eq(f, &start)) {
            self.record_edit(Edit::Frame { layer: id, frame: Some(start) });
        }
    }
}

/// A frame shape of one panel with the option's border.
fn one_panel(studio: &Studio, p: Panel) -> FrameShape {
    FrameShape { panels: vec![p], border: studio.opts.frame.border(studio.doc.dpi()) }
}

/// Rectangle Frame on release: a new frame folder, or a panel added to the
/// active frame folder.
pub fn add_rect_panel(studio: &mut Studio, r: RectF) {
    let (w, h) = (studio.doc.width(), studio.doc.height());
    let Some(panel) = Panel::rect(r).filter(|p| in_bleed_range(p, w, h)) else { return };
    match target_folder(studio).filter(|_| !studio.opts.frame.new_folder_per_frame) {
        Some(id) => {
            if studio.doc.frame(id).is_some_and(|f| f.shape().panels.len() >= MAX_PANELS) {
                studio.notice = Some("This frame folder has the most panels it can hold".into());
                return;
            }
            studio.edit_frame(id, |s| {
                let mut s = s.clone();
                s.panels.push(panel);
                Some(s)
            });
        }
        None => {
            let shape = one_panel(studio, panel);
            studio.edit_structure(|d| add_frame_folder(d, shape).is_some());
        }
    }
}

/// Divide Frame on release: split every panel of the active frame folder
/// the segment crosses.
pub fn cut_frame(studio: &mut Studio, a: Pt, b: Pt) {
    let Some(id) = target_folder(studio) else {
        studio.notice = Some("Select a frame border folder to divide".into());
        return;
    };
    let Some(frame) = studio.doc.frame(id) else { return };
    let (gap_h, gap_v) = studio.opts.frame.gaps(studio.doc.dpi());
    let Some((shape, bs)) = frame.shape().cut(a, b, gap_h, gap_v) else {
        studio.notice = Some("Drag across a panel to divide it".into());
        return;
    };
    if studio.opts.frame.divide_into_folders && !bs.is_empty() && bs.len() < shape.panels.len() {
        studio.edit_structure(|d| divide_into_folders(d, id, shape, &bs));
    } else {
        studio.edit_frame(id, |_| Some(shape));
    }
    studio.frame_sel = None;
}

/// Keep the A pieces in `id` and move the B pieces (`bs`) to a new frame
/// folder directly above it, with an empty raster child.
fn divide_into_folders(doc: &mut Document, id: LayerId, shape: FrameShape, bs: &[usize]) -> bool {
    let (mut keep, mut moved) = (Vec::new(), Vec::new());
    for (i, p) in shape.panels.into_iter().enumerate() {
        if bs.contains(&i) { moved.push(p) } else { keep.push(p) }
    }
    let border = shape.border;
    doc.set_frame(id, Some(Frame::build(FrameShape { panels: keep, border }, doc.width(), doc.height())));
    doc.set_active(id);
    add_frame_folder(doc, FrameShape { panels: moved, border });
    true
}

/// Layer ▸ New Frame Border Folder: one panel on the inner frame, or the
/// canvas inset by 5 %.
pub fn new_frame_folder(studio: &mut Studio) {
    let (w, h) = (studio.doc.width() as f32, studio.doc.height() as f32);
    let r = match studio.doc.page_setup().map(|p| p.inner).filter(|r| r.w > 0.0 && r.h > 0.0) {
        Some(r) => r,
        None => RectF { x: w * 0.05, y: h * 0.05, w: w * 0.9, h: h * 0.9 },
    };
    let Some(panel) = Panel::rect(r) else { return };
    let shape = one_panel(studio, panel);
    studio.edit_structure(|d| add_frame_folder(d, shape).is_some());
}

/// Delete the selected panel (Frame Edit).
pub fn delete_panel(studio: &mut Studio) {
    let Some((id, i)) = studio.frame_sel else { return };
    studio.edit_frame(id, |s| {
        (i < s.panels.len()).then(|| {
            let mut s = s.clone();
            s.panels.remove(i);
            s
        })
    });
    studio.frame_sel = None;
}

/// NewFrameFolder and DeletePanel.
pub fn execute(cmd: Command, studio: &mut Studio, _shell: &mut Shell) {
    match cmd {
        Command::NewFrameFolder => new_frame_folder(studio),
        Command::DeletePanel => delete_panel(studio),
        _ => {}
    }
}

// ----- the canvas tool -------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Grab {
    Vertex(usize),
    Edge(usize),
    Move,
}

enum Gesture {
    Rect { start: Pt, rect: RectF },
    Cut { a: Pt, b: Pt },
    Edit { folder: LayerId, panel: usize, grab: Grab, start: Pt, orig: Panel, cur: Panel },
}

#[derive(Default)]
pub struct FrameTool {
    gesture: Option<Gesture>,
    hover: Option<Grab>,
    /// Physical px per screen point, as of the last frame.
    ppp: f32,
}

fn mode(studio: &Studio) -> Option<FrameMode> {
    match studio.tool {
        Tool::Frame(m) => Some(m),
        _ => None,
    }
}

fn dist_to_segment(p: Pt, a: Pt, b: Pt) -> f32 {
    let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
    let l2 = ex * ex + ey * ey;
    let t = if l2 > 0.0 { (((p[0] - a[0]) * ex + (p[1] - a[1]) * ey) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (p[0] - a[0] - ex * t).hypot(p[1] - a[1] - ey * t)
}

fn dist(a: Pt, b: Pt) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// What the pointer at `p` grabs: handles and edges of the active frame
/// folder (the selected panel first), then the inside of its panels, then
/// the inside of other frame folders' panels. `k`: doc px per screen point.
fn hit_test(studio: &Studio, p: Pt, k: f32) -> Option<(LayerId, usize, Grab)> {
    let target = target_folder(studio);
    if let Some(id) = target
        && let Some(f) = studio.doc.frame(id)
    {
        let panels = &f.shape().panels;
        let sel = studio.frame_sel.filter(|s| s.0 == id).map(|s| s.1).filter(|&i| i < panels.len());
        let order = sel.into_iter().chain((0..panels.len()).rev().filter(|&i| Some(i) != sel));
        for i in order {
            let q = &panels[i];
            let n = q.points().len();
            if let Some(v) = (0..n).find(|&v| dist(q.points()[v], p) <= GRAB_PT * k) {
                return Some((id, i, Grab::Vertex(v)));
            }
            let mid = |e: usize| {
                let (a, b) = q.edge(e);
                [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
            };
            if let Some(e) = (0..n).find(|&e| dist(mid(e), p) <= GRAB_PT * k) {
                return Some((id, i, Grab::Edge(e)));
            }
            if let Some(e) = (0..n).find(|&e| {
                let (a, b) = q.edge(e);
                dist_to_segment(p, a, b) <= EDGE_PT * k
            }) {
                return Some((id, i, Grab::Edge(e)));
            }
        }
        if let Some(i) = f.hit(p) {
            return Some((id, i, Grab::Move));
        }
    }
    // Another frame folder's panel: select it.
    fn walk(doc: &Document, ids: &[LayerId], skip: Option<LayerId>, p: Pt) -> Option<(LayerId, usize)> {
        for &id in ids.iter().rev() {
            let Some(l) = doc.layer(id) else { continue };
            if !l.props.visible {
                continue;
            }
            if Some(id) != skip
                && let Some(i) = doc.frame(id).and_then(|f| f.hit(p))
            {
                return Some((id, i));
            }
            if let Some(found) = l.children().and_then(|c| walk(doc, c, skip, p)) {
                return Some(found);
            }
        }
        None
    }
    walk(&studio.doc, studio.doc.root(), target, p).map(|(id, i)| (id, i, Grab::Move))
}

/// The panel a Frame Edit drag to `p` makes, or `None` to keep the last
/// valid one.
fn edit_candidate(
    studio: &Studio,
    folder: LayerId,
    panel: usize,
    grab: Grab,
    start: Pt,
    orig: &Panel,
    p: Pt,
    mods: egui::Modifiers,
    tol: f32,
) -> Option<Panel> {
    let (w, h) = (studio.doc.width(), studio.doc.height());
    let f = studio.doc.frame(folder)?;
    let o = &studio.opts.frame;
    let targets = SnapTargets::new(
        w,
        h,
        studio.doc.page_setup(),
        &f.shape().panels,
        Some(panel),
        o.gaps(studio.doc.dpi()),
        o.snap_guides,
        o.snap_panels,
    );
    let width = f.shape().border.width;
    let snap = |v: f32, extent: u32, x_axis: bool| {
        if mods.command {
            snap_outside(v, extent as f32, width, tol)
        } else if mods.shift {
            let s = if x_axis { targets.snap_x(v, tol) } else { targets.snap_y(v, tol) };
            s.unwrap_or(v)
        } else {
            v
        }
    };
    let out = match grab {
        Grab::Vertex(i) => orig.with_vertex(i, [snap(p[0], w, true), snap(p[1], h, false)]),
        Grab::Edge(i) => {
            let n = orig.normal(i);
            let (a, _) = orig.edge(i);
            let mut d = (p[0] - start[0]) * n[0] + (p[1] - start[1]) * n[1];
            // An axis-aligned edge snaps its coordinate.
            if n[0].abs() > 0.999 {
                d = (snap(a[0] + n[0] * d, w, true) - a[0]) / n[0];
            } else if n[1].abs() > 0.999 {
                d = (snap(a[1] + n[1] * d, h, false) - a[1]) / n[1];
            }
            orig.with_edge_offset(i, d)
        }
        Grab::Move => {
            let mut d = [p[0] - start[0], p[1] - start[1]];
            if mods.shift {
                let b = orig.bounds();
                d[0] = SnapTargets::snap_span(&targets.xs, b.x, b.x + b.w, d[0], tol);
                d[1] = SnapTargets::snap_span(&targets.ys, b.y, b.y + b.h, d[1], tol);
            }
            Some(orig.translated(d))
        }
    };
    out.filter(|q| in_bleed_range(q, w, h))
}

impl FrameTool {
    /// Doc px per screen point.
    fn k(&self, studio: &Studio, ppp: f32) -> f32 {
        let ppp = if ppp > 0.0 {
            ppp
        } else if self.ppp > 0.0 {
            self.ppp
        } else {
            1.0
        };
        ppp / studio.view.zoom.max(1e-6)
    }

    fn press_edit(&mut self, ctx: &mut ToolCtx, input: ToolInput) {
        let k = self.k(ctx.studio, ctx.ppp);
        let Some((folder, panel, grab)) = hit_test(ctx.studio, input.doc, k) else {
            ctx.studio.frame_sel = None;
            return;
        };
        if target_folder(ctx.studio) != Some(folder) {
            ctx.studio.doc.set_active(folder);
        }
        ctx.studio.frame_sel = Some((folder, panel));
        let Some(orig) = ctx.studio.doc.frame(folder).and_then(|f| f.shape().panels.get(panel).cloned()) else {
            return;
        };
        if input.double {
            // Double-click a vertex to delete it, an edge to add one.
            let changed = match grab {
                Grab::Vertex(v) => orig.without_vertex(v),
                Grab::Edge(e) => {
                    let (a, b) = orig.edge(e);
                    let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
                    let t = (((input.doc[0] - a[0]) * ex + (input.doc[1] - a[1]) * ey) / (ex * ex + ey * ey))
                        .clamp(0.05, 0.95);
                    orig.with_inserted_vertex(e, [a[0] + ex * t, a[1] + ey * t])
                }
                Grab::Move => None,
            };
            if let Some(q) = changed {
                ctx.studio.edit_frame(folder, |s| {
                    let mut s = s.clone();
                    s.panels[panel] = q;
                    Some(s)
                });
            }
            return;
        }
        self.gesture = Some(Gesture::Edit { folder, panel, grab, start: input.doc, orig: orig.clone(), cur: orig });
    }
}

impl CanvasTool for FrameTool {
    fn press(&mut self, ctx: &mut ToolCtx, input: ToolInput) {
        self.ppp = ctx.ppp;
        match mode(ctx.studio) {
            Some(FrameMode::Rect) => {
                let r = RectF { x: input.doc[0], y: input.doc[1], w: 0.0, h: 0.0 };
                self.gesture = Some(Gesture::Rect { start: input.doc, rect: r });
            }
            Some(FrameMode::Cut) => self.gesture = Some(Gesture::Cut { a: input.doc, b: input.doc }),
            Some(FrameMode::Edit) => self.press_edit(ctx, input),
            None => {}
        }
    }

    fn drag(&mut self, ctx: &mut ToolCtx, input: ToolInput) {
        let k = self.k(ctx.studio, ctx.ppp);
        let studio = &*ctx.studio;
        match &mut self.gesture {
            Some(Gesture::Rect { start, rect }) => {
                let shift = input.mods.shift;
                let r = drag_rect(*start, input.doc, shift, input.mods.alt);
                let o = &studio.opts.frame;
                *rect = if !shift && (o.snap_guides || o.snap_panels) {
                    let panels =
                        target_folder(studio).and_then(|id| studio.doc.frame(id)).map(|f| f.shape().panels.clone());
                    let t = SnapTargets::new(
                        studio.doc.width(),
                        studio.doc.height(),
                        studio.doc.page_setup(),
                        panels.as_deref().unwrap_or_default(),
                        None,
                        o.gaps(studio.doc.dpi()),
                        o.snap_guides,
                        o.snap_panels,
                    );
                    t.snap_rect(r, snap_tol(k, 1.0))
                } else {
                    r
                };
            }
            Some(Gesture::Cut { a, b }) => *b = if input.mods.shift { snap_angle(*a, input.doc) } else { input.doc },
            Some(Gesture::Edit { folder, panel, grab, start, orig, cur }) => {
                if let Some(q) = edit_candidate(
                    studio,
                    *folder,
                    *panel,
                    *grab,
                    *start,
                    orig,
                    input.doc,
                    input.mods,
                    snap_tol(k, 1.0),
                ) {
                    *cur = q;
                }
            }
            None => {}
        }
    }

    fn release(&mut self, ctx: &mut ToolCtx, input: ToolInput) {
        self.drag(ctx, input);
        let k = self.k(ctx.studio, ctx.ppp);
        match self.gesture.take() {
            Some(Gesture::Rect { rect, .. }) if rect.w >= MIN_RECT_PX && rect.h >= MIN_RECT_PX => {
                add_rect_panel(ctx.studio, rect)
            }
            // A click is not a cut.
            Some(Gesture::Cut { a, b }) if dist(a, b) >= EDGE_PT * k => cut_frame(ctx.studio, a, b),
            Some(Gesture::Edit { folder, panel, orig, cur, .. }) if cur != orig => {
                ctx.studio.edit_frame(folder, |s| {
                    let mut s = s.clone();
                    *s.panels.get_mut(panel)? = cur;
                    Some(s)
                });
            }
            _ => {}
        }
    }

    fn hover(&mut self, studio: &Studio, input: ToolInput) {
        self.hover = (mode(studio) == Some(FrameMode::Edit))
            .then(|| hit_test(studio, input.doc, self.k(studio, 0.0)).map(|h| h.2))
            .flatten();
    }

    fn cancel(&mut self, _: &mut ToolCtx) {
        self.gesture = None;
    }

    fn tick(&mut self, ctx: &mut ToolCtx, _now: f64) {
        self.ppp = ctx.ppp;
        // A panel selection that undo took away.
        if let Some((id, i)) = ctx.studio.frame_sel
            && ctx.studio.doc.frame(id).is_none_or(|f| i >= f.shape().panels.len())
        {
            ctx.studio.frame_sel = None;
        }
    }

    fn paint(&self, studio: &Studio, painter: &egui::Painter, origin: [f32; 2], ppp: f32) {
        let Some(mode) = mode(studio) else { return };
        let m = studio.view.doc_to_screen(origin);
        let pos = |p: Pt| {
            let s = m.apply(p);
            Pos2::new(s[0] / ppp, s[1] / ppp)
        };
        let ring = |p: &Panel| p.points().iter().map(|&q| pos(q)).collect::<Vec<_>>();
        let halo = Stroke::new(3.0, Color32::from_black_alpha(90));
        if let Some(id) = target_folder(studio)
            && let Some(f) = studio.doc.frame(id)
        {
            for (i, p) in f.shape().panels.iter().enumerate() {
                let live = match &self.gesture {
                    Some(Gesture::Edit { folder, panel, cur, .. }) if *folder == id && *panel == i => cur,
                    _ => p,
                };
                let selected = studio.frame_sel == Some((id, i));
                let pts = ring(live);
                painter.add(Shape::closed_line(pts.clone(), halo));
                painter.add(Shape::closed_line(pts.clone(), Stroke::new(if selected { 2.0 } else { 1.0 }, OUTLINE)));
                if mode == FrameMode::Edit {
                    let fill = if selected { OUTLINE } else { Color32::WHITE };
                    for q in &pts {
                        let r = egui::Rect::from_center_size(*q, egui::vec2(HANDLE_PT, HANDLE_PT));
                        painter.rect_filled(r, 0.0, fill);
                        painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::BLACK), egui::StrokeKind::Inside);
                    }
                    for e in 0..pts.len() {
                        let (a, b) = (pts[e], pts[(e + 1) % pts.len()]);
                        painter.circle(a.lerp(b, 0.5), 2.5, fill, Stroke::new(1.0, Color32::BLACK));
                    }
                }
            }
        }
        match &self.gesture {
            Some(Gesture::Rect { rect, .. }) if rect.w > 0.0 && rect.h > 0.0 => {
                let r = *rect;
                let pts = [[r.x, r.y], [r.x + r.w, r.y], [r.x + r.w, r.y + r.h], [r.x, r.y + r.h]].map(pos).to_vec();
                painter.add(Shape::closed_line(pts.clone(), halo));
                painter.add(Shape::closed_line(pts, Stroke::new(1.5, OUTLINE)));
            }
            Some(Gesture::Cut { a, b }) if dist(*a, *b) > 0.0 => {
                painter.line_segment([pos(*a), pos(*b)], Stroke::new(1.0, CUT_LINE.gamma_multiply(0.6)));
                let Some(f) = target_folder(studio).and_then(|id| studio.doc.frame(id)) else { return };
                let d = [b[0] - a[0], b[1] - a[1]];
                let (gap_h, gap_v) = studio.opts.frame.gaps(studio.doc.dpi());
                let g = if d[0].abs() >= d[1].abs() { gap_h } else { gap_v };
                let l = d[0].hypot(d[1]);
                let n = [-d[1] / l * g / 2.0, d[0] / l * g / 2.0];
                for p in f.shape().panels.iter().filter(|p| p.crossed_by(*a, *b)) {
                    if let Some((c0, c1)) = p.chord(*a, d) {
                        painter.line_segment([pos(c0), pos(c1)], Stroke::new(2.0, CUT_LINE));
                    }
                    for s in [1.0, -1.0] {
                        let o = [a[0] + n[0] * s, a[1] + n[1] * s];
                        if let Some((c0, c1)) = p.chord(o, d) {
                            painter.line_segment([pos(c0), pos(c1)], Stroke::new(1.0, CUT_LINE.gamma_multiply(0.7)));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn cursor(&self, studio: &Studio) -> CursorIcon {
        match (mode(studio), &self.gesture, self.hover) {
            (_, Some(Gesture::Edit { grab: Grab::Move, .. }), _) => CursorIcon::Grabbing,
            (_, Some(Gesture::Edit { .. }), _) => CursorIcon::Crosshair,
            (Some(FrameMode::Edit), None, Some(Grab::Move)) => CursorIcon::Move,
            (Some(FrameMode::Edit), None, Some(_)) => CursorIcon::PointingHand,
            (Some(FrameMode::Edit), None, None) => CursorIcon::Default,
            _ => CursorIcon::Crosshair,
        }
    }

    fn gesture_active(&self) -> bool {
        self.gesture.is_some()
    }
}

// ----- tool property ----------------------------------------------------------

/// Change the active frame folder's border: previewed while a pointer
/// button is down (a slider or colour drag), recorded at once otherwise.
fn style_change(ui: &egui::Ui, studio: &mut Studio, id: LayerId, shape: FrameShape) {
    let key = egui::Id::new(("frame-style-start", id.0));
    if ui.input(|i| i.pointer.any_down()) {
        if ui.data(|d| d.get_temp::<Arc<Frame>>(key)).is_none()
            && let Some(start) = studio.doc.frame(id).cloned()
        {
            ui.data_mut(|d| d.insert_temp(key, start));
        }
        studio.preview_frame(id, shape);
    } else {
        studio.edit_frame(id, |_| Some(shape));
    }
}

/// Record a finished style drag as one `Edit::Frame`.
fn finish_style_drag(ui: &egui::Ui, studio: &mut Studio, id: LayerId) {
    let key = egui::Id::new(("frame-style-start", id.0));
    if !ui.input(|i| i.pointer.any_down())
        && let Some(start) = ui.data(|d| d.get_temp::<Arc<Frame>>(key))
    {
        ui.data_mut(|d| d.remove::<Arc<Frame>>(key));
        studio.commit_frame_preview(id, start);
    }
}

/// Tool Property for the frame tools.
pub fn property_ui(ui: &mut egui::Ui, studio: &mut Studio, shell: &mut Shell) {
    let mode = mode(studio);
    let mut o = studio.opts.frame.clone();
    ui.label(match mode {
        Some(FrameMode::Rect) => "Drag to add a panel. Shift: square, Alt: from the centre.",
        Some(FrameMode::Cut) => "Drag across panels to divide them. Shift snaps to 45°.",
        _ => "Drag vertices, edges or panels. Double-click an edge to add a vertex, a vertex to remove it. Shift snaps; Ctrl snaps past the canvas edge.",
    });
    ui.add_space(4.0);
    section(ui, "PANELS");
    egui::Grid::new("frame-opts").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        fn mm(v: &mut f32, max: f32) -> egui::DragValue<'_> {
            egui::DragValue::new(v).range(0.0..=max).speed(0.05).max_decimals(2).suffix(" mm")
        }
        ui.label("Gutter left / right");
        ui.add(mm(&mut o.gutter_lr_mm, 50.0));
        ui.end_row();
        ui.label("Gutter top / bottom");
        ui.add(mm(&mut o.gutter_tb_mm, 50.0));
        ui.end_row();
        ui.label("New border");
        ui.horizontal(|ui| {
            ui.add(mm(&mut o.border_mm, 10.0));
            ui.color_edit_button_rgb(&mut o.border_color);
        });
        ui.end_row();
    });
    match mode {
        Some(FrameMode::Rect) => {
            ui.checkbox(&mut o.new_folder_per_frame, "New folder per frame");
        }
        Some(FrameMode::Cut) => {
            ui.checkbox(&mut o.divide_into_folders, "Divide into folders");
        }
        _ => {}
    }
    ui.checkbox(&mut o.snap_guides, "Snap to page guides");
    ui.checkbox(&mut o.snap_panels, "Snap to panels");
    if o != studio.opts.frame {
        studio.opts.frame = o;
    }

    // The active frame folder's own border.
    let Some(id) = target_folder(studio) else {
        ui.weak("The active layer is not in a frame border folder.");
        return;
    };
    border_ui(ui, studio, shell, id);
}

/// The FRAME BORDER section of frame folder `id`: border on/off, width and
/// colour. The frame tools show it for the active layer's frame folder;
/// other tools show it while the active layer is a frame folder.
pub fn border_ui(ui: &mut egui::Ui, studio: &mut Studio, shell: &mut Shell, id: LayerId) {
    let dpi = studio.doc.dpi();
    let Some(frame) = studio.doc.frame(id).cloned() else { return };
    ui.add_space(6.0);
    section(ui, "FRAME BORDER");
    let shape = frame.shape();
    let mut on = shape.border.width > 0.0;
    let mut w_mm = shape.border.width / dpi as f32 * MM_PER_IN;
    let mut rgb = fix15_to_rgb(shape.border.color);
    egui::Grid::new("frame-border").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label("Border");
        ui.checkbox(&mut on, "");
        ui.end_row();
        ui.label("Width");
        ui.add_enabled(on, egui::Slider::new(&mut w_mm, 0.05..=5.0).logarithmic(true).max_decimals(2).suffix(" mm"));
        ui.end_row();
        ui.label("Colour");
        ui.color_edit_button_rgb(&mut rgb);
        ui.end_row();
    });
    let width = match (on, shape.border.width > 0.0) {
        (false, _) => 0.0,
        // Turned back on: the tool's width.
        (true, false) => studio.opts.frame.border(dpi).width.max(1.0),
        (true, true) => mm_px(w_mm, dpi),
    };
    let border = BorderStyle {
        width,
        color: if rgb == fix15_to_rgb(shape.border.color) { shape.border.color } else { rgb_to_fix15(rgb) },
    };
    if border != shape.border {
        let mut s = shape.clone();
        s.border = border;
        style_change(ui, studio, id, s);
    }
    finish_style_drag(ui, studio, id);
    let n = studio.doc.frame(id).map_or(0, |f| f.shape().panels.len());
    ui.weak(format!("{n} panel{}", if n == 1 { "" } else { "s" }));
    if studio.frame_sel.is_some_and(|s| s.0 == id) && ui.button(Command::DeletePanel.label()).clicked() {
        commands::execute(Command::DeletePanel, studio, shell);
    }
}

#[cfg(test)]
mod tests {
    use arty_core::RectF;
    use egui::Modifiers;

    use super::*;
    use crate::theme::ThemeKind;

    fn setup(tool: FrameMode) -> (Studio, Shell, FrameTool) {
        let mut studio = Studio::new(Document::new(600, 800, 254));
        studio.select_tool(Tool::Frame(tool));
        (studio, Shell::new(ThemeKind::Dark), FrameTool::default())
    }

    fn at(doc: Pt, mods: Modifiers, double: bool) -> ToolInput {
        ToolInput { screen: doc, doc, mods, double }
    }

    /// Press at `a`, drag through `via`, release at the last point.
    fn gesture(t: &mut FrameTool, s: &mut Studio, sh: &mut Shell, pts: &[Pt], mods: Modifiers) {
        let mut ctx = ToolCtx { studio: s, shell: sh, origin: [0.0; 2], ppp: 1.0 };
        t.press(&mut ctx, at(pts[0], mods, false));
        for &p in &pts[1..] {
            t.drag(&mut ctx, at(p, mods, false));
        }
        assert!(t.gesture_active() || pts.len() == 1);
        t.release(&mut ctx, at(*pts.last().unwrap(), mods, false));
        assert!(!t.gesture_active());
    }

    fn steps(s: &Studio) -> usize {
        s.history.undo_len()
    }

    #[test]
    fn fr14_new_frame_folder_is_one_step() {
        let (mut s, mut sh, _) = setup(FrameMode::Edit);
        execute(Command::NewFrameFolder, &mut s, &mut sh);
        assert_eq!(steps(&s), 1);
        let folder = target_folder(&s).unwrap();
        let f = s.doc.frame(folder).unwrap();
        assert_eq!(f.shape().panels[0].bounds(), RectF { x: 30.0, y: 40.0, w: 540.0, h: 720.0 }, "canvas inset by 5 %");
        assert!((f.shape().border.width - 6.0).abs() < 1e-4, "0.6 mm at 254 dpi");
        s.undo();
        assert!(target_folder(&s).is_none());
        // With a page setup, the inner frame.
        let inner = RectF { x: 50.0, y: 60.0, w: 400.0, h: 500.0 };
        s.set_page_setup(Some(PageSetup {
            trim: RectF { x: 0.0, y: 0.0, w: 600.0, h: 800.0 },
            bleed: 0.0,
            safe: 0.0,
            inner,
            unit: 0,
        }));
        execute(Command::NewFrameFolder, &mut s, &mut sh);
        let id = target_folder(&s).unwrap();
        assert_eq!(s.doc.frame(id).unwrap().shape().panels[0].bounds(), inner);
    }

    #[test]
    fn fr14_edit_frame_refuses_while_stroking() {
        let (mut s, mut sh, _) = setup(FrameMode::Edit);
        execute(Command::NewFrameFolder, &mut s, &mut sh);
        let id = target_folder(&s).unwrap();
        let before = s.doc.frame(id).unwrap().clone();
        assert!(s.begin_stroke(arty_brush::InputSample { x: 100.0, y: 100.0, pressure: 1.0, ..Default::default() }));
        s.edit_frame(id, |f| {
            let mut f = f.clone();
            f.border.width = 20.0;
            Some(f)
        });
        assert!(Arc::ptr_eq(s.doc.frame(id).unwrap(), &before));
        s.end_stroke();
        let n = steps(&s);
        s.edit_frame(id, |f| Some(f.clone()));
        assert_eq!(steps(&s), n, "the same shape is no step");
    }

    #[test]
    fn fr14_each_gesture_is_one_step() {
        // Rectangle Frame, a new folder per frame (default).
        let (mut s, mut sh, mut t) = setup(FrameMode::Rect);
        gesture(&mut t, &mut s, &mut sh, &[[100.0, 100.0], [200.0, 150.0], [301.3, 251.7]], Modifiers::NONE);
        assert_eq!(steps(&s), 1);
        let a = target_folder(&s).unwrap();
        assert_eq!(s.doc.frame(a).unwrap().shape().panels.len(), 1);
        // Too small: ignored.
        gesture(&mut t, &mut s, &mut sh, &[[10.0, 10.0], [12.0, 30.0]], Modifiers::NONE);
        assert_eq!(steps(&s), 1);
        // Into the active frame folder.
        s.opts.frame.new_folder_per_frame = false;
        gesture(&mut t, &mut s, &mut sh, &[[350.0, 100.0], [500.0, 300.0]], Modifiers::NONE);
        assert_eq!(steps(&s), 2);
        assert_eq!(s.doc.frame(a).unwrap().shape().panels.len(), 2);
        s.undo();
        assert_eq!(s.doc.frame(a).unwrap().shape().panels.len(), 1);
        s.redo();

        // Divide Frame across both panels: one step.
        s.select_tool(Tool::Frame(FrameMode::Cut));
        gesture(&mut t, &mut s, &mut sh, &[[50.0, 200.0], [300.0, 210.0], [550.0, 200.0]], Modifiers::NONE);
        assert_eq!(steps(&s), 3);
        assert_eq!(s.doc.frame(a).unwrap().shape().panels.len(), 4);
        // A cut that misses: a notice and no step.
        s.notice = None;
        gesture(&mut t, &mut s, &mut sh, &[[50.0, 700.0], [550.0, 720.0]], Modifiers::NONE);
        assert_eq!(steps(&s), 3);
        assert_eq!(s.notice.as_deref(), Some("Drag across a panel to divide it"));

        // Frame Edit: a vertex drag is one step (several moves).
        s.select_tool(Tool::Frame(FrameMode::Edit));
        let p0 = s.doc.frame(a).unwrap().shape().panels[0].clone();
        let v = p0.points()[0];
        gesture(&mut t, &mut s, &mut sh, &[v, [v[0] - 5.0, v[1] - 3.0], [v[0] - 10.0, v[1] - 6.0]], Modifiers::NONE);
        assert_eq!(steps(&s), 4);
        let p1 = s.doc.frame(a).unwrap().shape().panels[0].clone();
        assert_eq!(p1.points()[0], [v[0] - 10.0, v[1] - 6.0]);
        assert_eq!(s.frame_sel, Some((a, 0)));
        // A drag that would make it concave keeps the last valid position.
        let c = p1.points()[2];
        gesture(
            &mut t,
            &mut s,
            &mut sh,
            &[p1.points()[0], [v[0] + 5.0, v[1]], [c[0] + 50.0, c[1] + 50.0]],
            Modifiers::NONE,
        );
        assert_eq!(steps(&s), 5);
        assert_eq!(s.doc.frame(a).unwrap().shape().panels[0].points()[0], [v[0] + 5.0, v[1]]);
        // Moving a panel, then deleting it.
        let inside = {
            let b = p1.bounds();
            [b.x + b.w / 2.0, b.y + b.h / 2.0]
        };
        gesture(&mut t, &mut s, &mut sh, &[inside, [inside[0] + 7.0, inside[1]]], Modifiers::NONE);
        assert_eq!(steps(&s), 6);
        execute(Command::DeletePanel, &mut s, &mut sh);
        assert_eq!(steps(&s), 7);
        assert_eq!(s.doc.frame(a).unwrap().shape().panels.len(), 3);
        assert_eq!(s.frame_sel, None);
        // A click without a move is no step.
        let q = s.doc.frame(a).unwrap().shape().panels[0].points()[1];
        gesture(&mut t, &mut s, &mut sh, &[q], Modifiers::NONE);
        assert_eq!(steps(&s), 7);
        // Double-click an edge inserts a vertex; a vertex deletes it.
        let e = s.doc.frame(a).unwrap().shape().panels[0].edge(0);
        // On the edge, away from its midpoint handle.
        let l = dist(e.0, e.1);
        let mid = [
            (e.0[0] + e.1[0]) / 2.0 + (e.1[0] - e.0[0]) / l * 10.0,
            (e.0[1] + e.1[1]) / 2.0 + (e.1[1] - e.0[1]) / l * 10.0,
        ];
        let mut ctx = ToolCtx { studio: &mut s, shell: &mut sh, origin: [0.0; 2], ppp: 1.0 };
        t.press(&mut ctx, at(mid, Modifiers::NONE, true));
        t.release(&mut ctx, at(mid, Modifiers::NONE, false));
        assert_eq!(steps(&s), 8);
        let pts = s.doc.frame(a).unwrap().shape().panels[0].points().to_vec();
        assert_eq!(pts.len(), 5);
        let mut ctx = ToolCtx { studio: &mut s, shell: &mut sh, origin: [0.0; 2], ppp: 1.0 };
        t.press(&mut ctx, at(pts[1], Modifiers::NONE, true));
        t.release(&mut ctx, at(pts[1], Modifiers::NONE, false));
        assert_eq!(steps(&s), 9);
        assert_eq!(s.doc.frame(a).unwrap().shape().panels[0].points().len(), 4);
        // Esc (cancel) during a drag leaves no step.
        let p = s.doc.frame(a).unwrap().shape().panels[0].points()[0];
        let mut ctx = ToolCtx { studio: &mut s, shell: &mut sh, origin: [0.0; 2], ppp: 1.0 };
        t.press(&mut ctx, at(p, Modifiers::NONE, false));
        t.drag(&mut ctx, at([p[0] + 20.0, p[1]], Modifiers::NONE, false));
        t.cancel(&mut ctx);
        assert!(!t.gesture_active());
        assert_eq!(steps(&s), 9);
    }

    #[test]
    fn fr14_divide_into_folders_is_one_structure_step() {
        let (mut s, mut sh, mut t) = setup(FrameMode::Rect);
        gesture(&mut t, &mut s, &mut sh, &[[100.0, 100.0], [500.0, 700.0]], Modifiers::NONE);
        let a = target_folder(&s).unwrap();
        let layers = s.doc.layer_count();
        s.opts.frame.divide_into_folders = true;
        s.select_tool(Tool::Frame(FrameMode::Cut));
        gesture(&mut t, &mut s, &mut sh, &[[50.0, 400.0], [550.0, 400.0]], Modifiers::NONE);
        assert_eq!(steps(&s), 2);
        assert_eq!(s.doc.layer_count(), layers + 2, "a frame folder and its raster child");
        let b = target_folder(&s).unwrap();
        assert_ne!(a, b);
        assert_eq!(s.doc.frame(a).unwrap().shape().panels.len(), 1);
        assert_eq!(s.doc.frame(b).unwrap().shape().panels.len(), 1);
        let (pa, pb) =
            (s.doc.frame(a).unwrap().shape().panels[0].bounds(), s.doc.frame(b).unwrap().shape().panels[0].bounds());
        assert!(pa.y != pb.y && (pa.w, pb.w) == (400.0, 400.0));
        let (parent, index) = s.doc.location(b).unwrap();
        assert_eq!(s.doc.location(a).unwrap(), (parent, index - 1), "directly above");
        s.undo();
        assert_eq!(s.doc.layer_count(), layers);
        assert_eq!(s.doc.frame(a).unwrap().shape().panels.len(), 1);
        assert_eq!(s.doc.frame(a).unwrap().shape().panels[0].bounds().h, 600.0);
    }

    #[test]
    fn fr14_a_property_drag_is_one_step() {
        let (mut s, mut sh, _) = setup(FrameMode::Edit);
        execute(Command::NewFrameFolder, &mut s, &mut sh);
        let id = target_folder(&s).unwrap();
        let start = s.doc.frame(id).unwrap().clone();
        let n = steps(&s);
        for w in [2.0, 4.0, 8.0, 12.0] {
            let mut shape = start.shape().clone();
            shape.border.width = w;
            s.preview_frame(id, shape);
            assert_eq!(s.doc.frame(id).unwrap().shape().border.width, w, "rebuilt live");
        }
        assert_eq!(steps(&s), n, "nothing recorded during the drag");
        s.commit_frame_preview(id, start.clone());
        assert_eq!(steps(&s), n + 1);
        s.undo();
        assert!(Arc::ptr_eq(s.doc.frame(id).unwrap(), &start));
        // A drag that ends where it started records nothing.
        s.commit_frame_preview(id, start);
        assert_eq!(steps(&s), n);
    }

    #[test]
    fn fr15_snapping() {
        let page = PageSetup {
            trim: RectF { x: 100.0, y: 120.0, w: 800.0, h: 1000.0 },
            bleed: 30.0,
            safe: 20.0,
            inner: RectF { x: 200.0, y: 220.0, w: 600.0, h: 800.0 },
            unit: 0,
        };
        let other = Panel::rect(RectF { x: 300.0, y: 400.0, w: 100.0, h: 100.0 }).unwrap();
        let t = SnapTargets::new(1000, 1240, Some(&page), std::slice::from_ref(&other), None, (12.0, 5.0), true, true);
        // 8 screen points at zoom 0.5 and ppp 1.5 is 24 doc px.
        let tol = snap_tol(1.5, 0.5);
        assert_eq!(tol, 24.0);
        for (x, want, what) in [
            (190.0, Some(200.0), "inner frame"),
            (110.0, Some(100.0), "trim"),
            (75.0, Some(70.0), "bleed"),
            (-20.0, Some(0.0), "canvas"),
            (1015.0, Some(1000.0), "canvas right"),
            (290.0, Some(295.0), "other panel − gutter"),
            (410.0, Some(405.0), "other panel + gutter"),
            (600.0, None, "nothing near"),
        ] {
            assert_eq!(t.snap_x(x, tol), want, "{what}");
        }
        assert_eq!(t.snap_y(510.0, tol), Some(512.0), "+ top/bottom gutter");
        assert_eq!(t.snap_y(380.0, tol), Some(388.0), "− top/bottom gutter");
        assert_eq!(t.snap_x(190.0, snap_tol(1.0, 4.0)), None, "8 points at zoom 4 are 2 px");
        let r = t.snap_rect(RectF { x: 195.0, y: 230.0, w: 600.0, h: 400.0 }, 10.0);
        assert_eq!(r, RectF { x: 200.0, y: 220.0, w: 600.0, h: 410.0 });
        let none = SnapTargets::new(1000, 1240, Some(&page), &[other], None, (12.0, 5.0), false, false);
        assert_eq!(none.snap_x(0.5, 100.0), None, "snapping off");
        // Ctrl: width + 1 px past the canvas edge.
        assert_eq!(snap_outside(3.0, 1000.0, 6.0, 10.0), -7.0);
        assert_eq!(snap_outside(995.0, 1000.0, 6.0, 10.0), 1007.0);
        assert_eq!(snap_outside(500.0, 1000.0, 6.0, 10.0), 500.0);
        // Shift square, Alt centred.
        assert_eq!(drag_rect([10.0, 10.0], [40.0, 30.0], true, false), RectF { x: 10.0, y: 10.0, w: 30.0, h: 30.0 });
        assert_eq!(drag_rect([10.0, 10.0], [0.0, 30.0], false, true), RectF { x: 0.0, y: -10.0, w: 20.0, h: 40.0 });
        assert_eq!(drag_rect([10.0, 10.0], [0.0, 0.0], false, false), RectF { x: 0.0, y: 0.0, w: 10.0, h: 10.0 });
        let b = snap_angle([0.0, 0.0], [10.0, 1.0]);
        assert!((b[1]).abs() < 1e-4 && (b[0] - 101f32.sqrt()).abs() < 1e-4);
    }

    #[test]
    fn fr15_ctrl_drag_places_the_vertex_past_the_canvas() {
        let (mut s, mut sh, mut t) = setup(FrameMode::Rect);
        gesture(&mut t, &mut s, &mut sh, &[[100.0, 100.0], [300.0, 300.0]], Modifiers::NONE);
        let a = target_folder(&s).unwrap();
        let width = s.doc.frame(a).unwrap().shape().border.width;
        s.select_tool(Tool::Frame(FrameMode::Edit));
        gesture(&mut t, &mut s, &mut sh, &[[100.0, 100.0], [4.0, 3.0]], Modifiers::COMMAND);
        assert_eq!(s.doc.frame(a).unwrap().shape().panels[0].points()[0], [-(width + 1.0), -(width + 1.0)]);
        // Shift snaps an edge to the canvas.
        let e = s.doc.frame(a).unwrap().shape().panels[0].edge(1);
        let mid = [(e.0[0] + e.1[0]) / 2.0, (e.0[1] + e.1[1]) / 2.0];
        gesture(&mut t, &mut s, &mut sh, &[mid, [mid[0] + 296.0, mid[1]]], Modifiers::SHIFT);
        assert_eq!(
            s.doc.frame(a).unwrap().shape().panels[0].bounds().x + s.doc.frame(a).unwrap().shape().panels[0].bounds().w,
            600.0
        );
    }
}
