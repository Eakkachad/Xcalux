//! Selection tools (M: rect, ellipse, lasso, polygon; W: magic wand), the
//! marching ants and the selection commands. Owned by SEL-UI.
//!
//! Shapes are built in screen space (a rect stays screen-aligned on a
//! rotated canvas, an ellipse is flattened on screen) and mapped to the
//! document with `View::screen_to_doc`; the core rasterizer only sees
//! document polygons. Every finished gesture is one `Studio::set_selection`.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use arty_core::contour::{self, Contours, LOD_TOL};
use arty_core::fill::{self, FillParams, FillRef, FillScratch, ScaleMode};
use arty_core::morph::{self, MorphShape};
use arty_core::raster::rasterize_polygon;
use arty_core::transform::XfTarget;
use arty_core::{Affine64, Pt, SelectOp, Selection, fix15};
use arty_render::Affine2;
use arty_render::ants::{self, AntsCallback, AntsGeometry};
use egui::{Color32, Modifiers, Pos2, Shape, Stroke};
use serde::{Deserialize, Serialize};

use super::{CanvasTool, ToolCtx, ToolInput};
use crate::commands::{self, Command, SelModify};
use crate::panels::fill_slider;
use crate::shell::Shell;
use crate::studio::{Studio, Tool};
use crate::text::{Key, t};

/// A drag shorter than this (points) is a click.
const CLICK_PX: f32 = 2.0;
/// A polygon click this close (points) to the first vertex closes it.
const CLOSE_PX: f32 = 8.0;
/// Lasso points closer than this (physical px) to the last one are skipped.
const LASSO_STEP_PX: f32 = 1.0;
/// Most segments the egui fallback draws per frame.
const CPU_SEGMENTS: usize = 20_000;
/// The ants march 2 px per tick.
const ANTS_TICK: f64 = 0.12;

/// Shape of the Selection tool (a tool option, so M brings back the last).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SelShape {
    #[default]
    Rect,
    Ellipse,
    Lasso,
    Polygon,
}

impl SelShape {
    const ALL: [SelShape; 4] = [SelShape::Rect, SelShape::Ellipse, SelShape::Lasso, SelShape::Polygon];

    pub(crate) fn label(self) -> &'static str {
        match self {
            SelShape::Rect => t(Key::SelShapeRect),
            SelShape::Ellipse => t(Key::SelShapeEllipse),
            SelShape::Lasso => t(Key::SelShapeLasso),
            SelShape::Polygon => t(Key::SelShapePolygon),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WandOptions {
    pub reference: FillRef,
    /// 0..=1 of the full channel range.
    pub tolerance: f32,
    pub contiguous: bool,
    /// Close-gap level 0 (off) ..= 5.
    pub gap: u8,
    pub antialias: bool,
}

impl Default for WandOptions {
    fn default() -> Self {
        Self { reference: FillRef::Active, tolerance: 0.1, contiguous: true, gap: 0, antialias: true }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SelectOptions {
    pub shape: SelShape,
    /// The mode used when no modifier is held at press.
    pub mode: SelectOp,
    pub antialias: bool,
    pub wand: WandOptions,
    /// Structuring element of Grow and Shrink.
    pub morph: MorphShape,
    /// Last values of the Grow / Shrink / Feather dialog (px).
    pub grow_px: u16,
    pub shrink_px: u16,
    pub feather_px: u16,
}

impl Default for SelectOptions {
    fn default() -> Self {
        Self {
            shape: SelShape::Rect,
            mode: SelectOp::Replace,
            antialias: true,
            wand: WandOptions::default(),
            morph: MorphShape::Circle,
            grow_px: 4,
            shrink_px: 4,
            feather_px: 4,
        }
    }
}

/// The mode a press with `mods` held selects with: Shift adds, Alt
/// subtracts, both intersect; otherwise the tool's `default` mode.
pub fn op_for(mods: Modifiers, default: SelectOp) -> SelectOp {
    match (mods.shift, mods.alt) {
        (true, true) => SelectOp::Intersect,
        (true, false) => SelectOp::Add,
        (false, true) => SelectOp::Subtract,
        (false, false) => default,
    }
}

/// Segments of an ellipse of screen radius `r` (physical px) so the chord
/// error stays under 0.1 px: `ceil(π / acos(1 − 0.1/r))`, 8 ..= 4096.
pub fn ellipse_segments(r: f32) -> usize {
    if r <= 0.1 {
        return 8;
    }
    let n = (std::f32::consts::PI / (1.0 - 0.1 / r).acos()).ceil();
    (n as usize).clamp(8, 4096)
}

/// The document polygon of a rect or ellipse dragged on screen from
/// `start` to `cur` (physical px). `square` constrains to a square or
/// circle, `centre` draws from `start` outwards.
pub fn shape_polygon(
    shape: SelShape,
    start: [f32; 2],
    cur: [f32; 2],
    square: bool,
    centre: bool,
    screen_to_doc: Affine2,
) -> Vec<Pt> {
    let (mut dx, mut dy) = (cur[0] - start[0], cur[1] - start[1]);
    if square {
        let s = dx.abs().max(dy.abs());
        (dx, dy) = (s.copysign(dx), s.copysign(dy));
    }
    let (p0, p1) = if centre {
        ([start[0] - dx, start[1] - dy], [start[0] + dx, start[1] + dy])
    } else {
        (start, [start[0] + dx, start[1] + dy])
    };
    let screen: Vec<[f32; 2]> = match shape {
        SelShape::Ellipse => {
            let (cx, cy) = ((p0[0] + p1[0]) * 0.5, (p0[1] + p1[1]) * 0.5);
            let (rx, ry) = ((p1[0] - p0[0]).abs() * 0.5, (p1[1] - p0[1]).abs() * 0.5);
            let n = ellipse_segments(rx.max(ry));
            (0..n)
                .map(|i| {
                    let t = i as f32 / n as f32 * std::f32::consts::TAU;
                    [cx + rx * t.cos(), cy + ry * t.sin()]
                })
                .collect()
        }
        _ => vec![p0, [p1[0], p0[1]], p1, [p0[0], p1[1]]],
    };
    screen.into_iter().map(|p| screen_to_doc.apply(p)).collect()
}

/// Combine `shape` into the selection with `op`, as one undo step.
fn apply_shape(studio: &mut Studio, shape: &Selection, op: SelectOp) {
    let mut sel = studio.doc.selection().clone();
    sel.combine(shape, op);
    // Subtracting from a full edge tile leaves 255 past the page.
    sel.clip_to_page(studio.doc.width(), studio.doc.height());
    studio.set_selection(sel);
}

fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

enum Gesture {
    /// Rect, ellipse or lasso, while the button is down.
    Drag {
        shape: SelShape,
        op: SelectOp,
        /// Physical px.
        start: [f32; 2],
        cur: [f32; 2],
        mods: Modifiers,
        /// Shift / Alt held since the press: they chose the mode and do not
        /// constrain the shape until released once.
        held: (bool, bool),
        /// Moved further than a click.
        moved: bool,
        /// Lasso points (document px) and the last one's screen position.
        lasso: Vec<Pt>,
        last: [f32; 2],
    },
    /// Polygon vertices (document px) and the pointer, until closed.
    Polygon { op: SelectOp, pts: Vec<Pt>, hover: Option<Pt> },
}

/// The selection outline: contours extracted off the UI thread per
/// selection revision, and the vertex buffer built from them.
#[derive(Default)]
struct Ants {
    /// (document epoch, selection revision) of `contours`.
    key: Option<(u64, u64)>,
    pending: Option<((u64, u64), Receiver<Contours>)>,
    contours: Option<Arc<Contours>>,
    /// Per-polyline document bbox `[x0, y0, x1, y1]` of each level.
    bboxes: [Vec<[f32; 4]>; 3],
    geom: Option<Arc<AntsGeometry>>,
    /// Vertex buffers built (one per selection revision).
    builds: u64,
    /// Draw through the GPU callback ([`SelectTool::attach_gpu`]).
    gpu: bool,
}

#[derive(Default)]
pub struct SelectTool {
    gesture: Option<Gesture>,
    wand: FillScratch,
    ants: Ants,
}

impl SelectTool {
    /// Draw the ants on the GPU: installs their pipeline in `render`'s
    /// callback resources. Without it the egui painter draws them.
    /// `CanvasPane::new` calls this with its render state.
    pub fn attach_gpu(&mut self, render: &egui_wgpu::RenderState) {
        ants::AntsGpu::install(render);
        self.ants.gpu = true;
    }

    /// Vertices of the unclosed polygon.
    #[cfg(test)]
    pub(crate) fn polygon_len(&self) -> Option<usize> {
        match &self.gesture {
            Some(Gesture::Polygon { pts, .. }) => Some(pts.len()),
            _ => None,
        }
    }

    fn close_polygon(&mut self, ctx: &mut ToolCtx) {
        if let Some(Gesture::Polygon { op, pts, .. }) = self.gesture.take()
            && pts.len() >= 3
        {
            let (w, h) = (ctx.studio.doc.width(), ctx.studio.doc.height());
            let shape = rasterize_polygon(&pts, w, h, ctx.studio.opts.select.antialias);
            apply_shape(ctx.studio, &shape, op);
        }
    }

    fn wand_click(&mut self, ctx: &mut ToolCtx, input: ToolInput) {
        let doc = &ctx.studio.doc;
        let (x, y) = (input.doc[0].floor() as i32, input.doc[1].floor() as i32);
        if x < 0 || y < 0 || x >= doc.width() as i32 || y >= doc.height() as i32 {
            return;
        }
        let o = &ctx.studio.opts.select;
        let params = FillParams {
            reference: o.wand.reference,
            tolerance: (o.wand.tolerance.clamp(0.0, 1.0) * fix15::ONE as f32).round() as u16,
            gap_px: fill::gap_radius(o.wand.gap, doc.dpi()),
            area_scale: 0,
            scale_mode: ScaleMode::Plain,
            contiguous: o.wand.contiguous,
            antialias: o.wand.antialias,
            // The wand ignores the current selection (D11).
            use_selection: false,
        };
        let op = op_for(input.mods, o.mode);
        if let Some(region) = fill::fill_region(doc, (x, y), &params, &mut self.wand) {
            apply_shape(ctx.studio, &region, op);
        }
    }

    /// Bring the ants up to date with the selection: start an extraction
    /// for a new revision, or take a finished one (`block`: wait for it).
    fn sync_ants(&mut self, studio: &Studio, block: bool) {
        let a = &mut self.ants;
        if !studio.doc.has_selection() {
            *a = Ants { gpu: a.gpu, builds: a.builds, ..Ants::default() };
            return;
        }
        let key = (studio.doc_epoch, studio.doc.selection_rev());
        if a.key == Some(key) {
            return;
        }
        if a.pending.as_ref().is_none_or(|(k, _)| *k != key) {
            let (tx, rx) = mpsc::channel();
            let (sel, w, h) = (studio.doc.selection().clone(), studio.doc.width(), studio.doc.height());
            rayon::spawn(move || {
                // The receiver is gone when a newer revision replaced it.
                let _ = tx.send(contour::extract(&sel, w, h));
            });
            a.pending = Some((key, rx));
        }
        let Some((_, rx)) = &a.pending else { return };
        let c = if block {
            rx.recv().unwrap_or_default()
        } else {
            match rx.try_recv() {
                Ok(c) => c,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => Contours::default(),
            }
        };
        a.pending = None;
        a.key = Some(key);
        a.builds += 1;
        a.bboxes = std::array::from_fn(|k| {
            c.lods[k]
                .iter()
                .map(|p| {
                    p.pts.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |b, q| {
                        [b[0].min(q[0]), b[1].min(q[1]), b[2].max(q[0]), b[3].max(q[1])]
                    })
                })
                .collect()
        });
        a.geom = Some(Arc::new(AntsGeometry::build(a.builds, &c)));
        a.contours = Some(Arc::new(c));
    }
}

impl CanvasTool for SelectTool {
    fn press(&mut self, ctx: &mut ToolCtx, input: ToolInput) {
        if ctx.studio.tool == Tool::MagicWand {
            self.wand_click(ctx, input);
            return;
        }
        if let Some(Gesture::Polygon { pts, .. }) = &mut self.gesture {
            let first = ctx.studio.view.doc_to_screen(ctx.origin).apply(pts[0]);
            if input.double || (pts.len() >= 3 && dist(first, input.screen) <= CLOSE_PX * ctx.ppp) {
                self.close_polygon(ctx);
            } else {
                pts.push(input.doc);
            }
            return;
        }
        let o = &ctx.studio.opts.select;
        let op = op_for(input.mods, o.mode);
        self.gesture = Some(match o.shape {
            SelShape::Polygon => Gesture::Polygon { op, pts: vec![input.doc], hover: None },
            shape => Gesture::Drag {
                shape,
                op,
                start: input.screen,
                cur: input.screen,
                mods: input.mods,
                held: (input.mods.shift, input.mods.alt),
                moved: false,
                lasso: if shape == SelShape::Lasso { vec![input.doc] } else { Vec::new() },
                last: input.screen,
            },
        });
    }

    fn drag(&mut self, ctx: &mut ToolCtx, input: ToolInput) {
        match &mut self.gesture {
            Some(Gesture::Drag { shape, start, cur, mods, held, moved, lasso, last, .. }) => {
                *cur = input.screen;
                *mods = input.mods;
                held.0 &= input.mods.shift;
                held.1 &= input.mods.alt;
                *moved |= dist(*start, *cur) >= CLICK_PX * ctx.ppp;
                if *shape == SelShape::Lasso && dist(*last, input.screen) >= LASSO_STEP_PX {
                    lasso.push(input.doc);
                    *last = input.screen;
                }
            }
            Some(Gesture::Polygon { hover, .. }) => *hover = Some(input.doc),
            None => {}
        }
    }

    fn release(&mut self, ctx: &mut ToolCtx, input: ToolInput) {
        if matches!(self.gesture, Some(Gesture::Polygon { .. })) {
            return;
        }
        self.drag(ctx, input);
        let Some(Gesture::Drag { shape, op, start, cur, mods, held, moved, mut lasso, .. }) = self.gesture.take() else { return };
        if !moved {
            // A click: New deselects (CSP); with a modifier it does nothing.
            if op == SelectOp::Replace {
                ctx.studio.set_selection(Selection::default());
            }
            return;
        }
        let pts = if shape == SelShape::Lasso {
            lasso.push(input.doc);
            lasso
        } else {
            let to_doc = ctx.studio.view.screen_to_doc(ctx.origin);
            shape_polygon(shape, start, cur, mods.shift && !held.0, mods.alt && !held.1, to_doc)
        };
        let (w, h) = (ctx.studio.doc.width(), ctx.studio.doc.height());
        let sel = rasterize_polygon(&pts, w, h, ctx.studio.opts.select.antialias);
        apply_shape(ctx.studio, &sel, op);
    }

    fn hover(&mut self, _: &Studio, input: ToolInput) {
        if let Some(Gesture::Polygon { hover, .. }) = &mut self.gesture {
            *hover = Some(input.doc);
        }
    }

    fn key(&mut self, ctx: &mut ToolCtx, key: egui::Key) -> bool {
        match (&mut self.gesture, key) {
            (Some(Gesture::Polygon { pts, .. }), egui::Key::Backspace) => {
                pts.pop();
                if pts.is_empty() {
                    self.gesture = None;
                }
                true
            }
            (Some(Gesture::Polygon { .. }), egui::Key::Enter) => {
                self.close_polygon(ctx);
                self.gesture = None;
                true
            }
            (Some(_), egui::Key::Escape) => {
                self.gesture = None;
                true
            }
            _ => false,
        }
    }

    fn cancel(&mut self, _: &mut ToolCtx) {
        self.gesture = None;
    }

    fn tick(&mut self, ctx: &mut ToolCtx, _now: f64) {
        // Selection and Magic Wand share this state: a switch to the wand
        // (toolbar) abandons a shape in progress.
        if ctx.studio.tool != Tool::Select {
            self.gesture = None;
        }
    }

    fn paint(&self, studio: &Studio, painter: &egui::Painter, origin: [f32; 2], ppp: f32) {
        let Some(g) = &self.gesture else { return };
        let to_screen = studio.view.doc_to_screen(origin);
        let pos = |p: [f32; 2]| Pos2::new(p[0] / ppp, p[1] / ppp);
        let doc_pos = |p: Pt| pos(to_screen.apply(p));
        let (path, closed): (Vec<Pos2>, bool) = match g {
            Gesture::Drag { shape: SelShape::Lasso, lasso, cur, .. } => {
                (lasso.iter().map(|&p| doc_pos(p)).chain(std::iter::once(pos(*cur))).collect(), true)
            }
            Gesture::Drag { shape, start, cur, mods, held, .. } => {
                let doc = shape_polygon(
                    *shape,
                    *start,
                    *cur,
                    mods.shift && !held.0,
                    mods.alt && !held.1,
                    studio.view.screen_to_doc(origin),
                );
                (doc.into_iter().map(doc_pos).collect(), true)
            }
            Gesture::Polygon { pts, hover, .. } => {
                let mut path: Vec<Pos2> = pts.iter().map(|&p| doc_pos(p)).collect();
                path.extend(hover.map(doc_pos));
                // Hint that a click here closes the polygon.
                if let (Some(&first), Some(h)) = (path.first(), hover.map(doc_pos))
                    && pts.len() >= 3
                    && first.distance(h) <= CLOSE_PX
                {
                    painter.circle_stroke(first, CLOSE_PX * 0.5, Stroke::new(1.0, Color32::WHITE));
                }
                (path, false)
            }
        };
        if path.len() < 2 {
            return;
        }
        let mut ring = path;
        if closed {
            ring.push(ring[0]);
        }
        painter.add(Shape::line(ring.clone(), Stroke::new(1.0, Color32::BLACK)));
        painter.extend(Shape::dashed_line(&ring, Stroke::new(1.0, Color32::WHITE), 4.0, 4.0));
    }

    fn gesture_active(&self) -> bool {
        self.gesture.is_some()
    }
}

/// SelectAll, Deselect, InvertSelection, SelectionDialog and
/// Grow/Shrink/FeatherSelection.
pub fn execute(cmd: Command, studio: &mut Studio, shell: &mut Shell) {
    if studio.engine.is_stroking() {
        return;
    }
    // A selection-target session moves the selection on commit: derive the
    // new one from where it went, not where it was.
    studio.commit_transform();
    let (w, h) = (studio.doc.width(), studio.doc.height());
    let shape = studio.opts.select.morph;
    let modify = |studio: &mut Studio, px: u16, f: &dyn Fn(&Selection) -> Selection| {
        if px > 0 && studio.doc.has_selection() {
            let s = f(studio.doc.selection());
            studio.set_selection(s);
        }
    };
    match cmd {
        Command::SelectAll => studio.set_selection(Selection::all(w, h)),
        Command::Deselect => studio.set_selection(Selection::default()),
        Command::InvertSelection => {
            let s = studio.doc.selection().inverted(w, h);
            studio.set_selection(s);
        }
        Command::SelectionDialog(m) => {
            if studio.doc.has_selection() {
                shell.sel_dialog = Some(m);
            } else {
                studio.notice = Some(t(Key::NoticeNothingSelected).into());
            }
        }
        Command::GrowSelection { px } => modify(studio, px, &|s| morph::grow(s, px, shape, w, h)),
        Command::ShrinkSelection { px } => modify(studio, px, &|s| morph::shrink(s, px, shape, w, h)),
        Command::FeatherSelection { px } => modify(studio, px, &|s| morph::feather(s, px, w, h)),
        _ => {}
    }
}

fn mode_ui(ui: &mut egui::Ui, mode: &mut SelectOp) {
    ui.horizontal(|ui| {
        for (op, label, tip) in [
            (SelectOp::Replace, t(Key::SelOpNew), t(Key::SelOpNewTip)),
            (SelectOp::Add, t(Key::SelOpAdd), t(Key::SelOpAddTip)),
            (SelectOp::Subtract, t(Key::SelOpSubtract), t(Key::SelOpSubtractTip)),
            (SelectOp::Intersect, t(Key::SelOpIntersect), t(Key::SelOpIntersectTip)),
        ] {
            ui.selectable_value(mode, op, label).on_hover_text(tip);
        }
    });
}

/// Tool Property for Selection and Magic Wand.
pub fn property_ui(ui: &mut egui::Ui, studio: &mut Studio, _shell: &mut Shell) {
    let wand = studio.tool == Tool::MagicWand;
    let o = &mut studio.opts.select;
    egui::Grid::new("select-props").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        if !wand {
            ui.label(t(Key::SelShape));
            ui.horizontal(|ui| {
                for s in SelShape::ALL {
                    ui.selectable_value(&mut o.shape, s, s.label());
                }
            });
            ui.end_row();
        }
        ui.label(t(Key::SelMode));
        mode_ui(ui, &mut o.mode);
        ui.end_row();
        if wand {
            ui.label(t(Key::FillReferTo));
            egui::ComboBox::from_id_salt("wand-ref")
                .selected_text(match o.wand.reference {
                    FillRef::Active => t(Key::FillRefActive),
                    FillRef::AllVisible => t(Key::FillRefAllVisible),
                    FillRef::Reference => t(Key::FillRefReference),
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut o.wand.reference, FillRef::Active, t(Key::FillRefActive));
                    ui.selectable_value(&mut o.wand.reference, FillRef::AllVisible, t(Key::FillRefAllVisible));
                    ui.selectable_value(&mut o.wand.reference, FillRef::Reference, t(Key::FillRefReference));
                });
            ui.end_row();
            ui.label(t(Key::FillTolerance));
            fill_slider(
                ui,
                egui::Slider::new(&mut o.wand.tolerance, 0.0..=1.0)
                    .custom_formatter(|v, _| format!("{:.0}%", v * 100.0))
                    .custom_parser(|s| s.trim_end_matches('%').trim().parse::<f64>().ok().map(|v| v / 100.0)),
            );
            ui.end_row();
            ui.label(t(Key::FillCloseGap));
            fill_slider(
                ui,
                egui::Slider::new(&mut o.wand.gap, 0..=5)
                    .custom_formatter(|v, _| if v == 0.0 { t(Key::CommonOff).into() } else { format!("{v:.0}") }),
            );
            ui.end_row();
            ui.label(t(Key::FillContiguous));
            ui.checkbox(&mut o.wand.contiguous, "");
            ui.end_row();
            ui.label(t(Key::FillAntialiasing));
            ui.checkbox(&mut o.wand.antialias, "");
            ui.end_row();
        } else {
            ui.label(t(Key::FillAntialiasing));
            ui.checkbox(&mut o.antialias, "");
            ui.end_row();
        }
    });
    ui.add_space(4.0);
    ui.weak(if wand {
        t(Key::HintSelectWand)
    } else {
        match o.shape {
            SelShape::Rect | SelShape::Ellipse => t(Key::HintSelectDrag),
            SelShape::Lasso => t(Key::HintSelectLasso),
            SelShape::Polygon => t(Key::HintSelectPolygon),
        }
    });
}

/// `view` after the doc → doc map `xf` (a transform session's preview).
fn compose(view: Affine2, xf: Affine64) -> Affine2 {
    let [x0, x1, x2, x3, x4, x5] = xf.m.map(|v| v as f32);
    Affine2 {
        a: view.a * x0 + view.b * x3,
        b: view.a * x1 + view.b * x4,
        tx: view.a * x2 + view.b * x5 + view.tx,
        c: view.c * x0 + view.d * x3,
        d: view.c * x1 + view.d * x4,
        ty: view.c * x2 + view.d * x5 + view.ty,
    }
}

fn inverse(m: Affine2) -> Option<Affine2> {
    let det = m.a * m.d - m.b * m.c;
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    let (a, b, c, d) = (m.d / det, -m.b / det, -m.c / det, m.a / det);
    Some(Affine2 { a, b, c, d, tx: -(a * m.tx + b * m.ty), ty: -(c * m.tx + d * m.ty) })
}

/// The coarsest level whose error stays under half a screen px at `zoom`
/// (screen px per doc px).
fn lod_for(zoom: f32) -> usize {
    (0..3).rev().find(|&k| LOD_TOL[k] * zoom <= 0.5).unwrap_or(0)
}

/// Document → screen for the ants. During a selection-target session they
/// follow the floating pixels; after its commit, the outline on hand (of
/// the selection before the move) keeps that affine until the moved one's
/// is extracted, so it does not jump back for those frames.
fn ants_matrix(a: &Ants, studio: &Studio, origin: [f32; 2]) -> Affine2 {
    let view = studio.view.doc_to_screen(origin);
    if let Some(t) = &studio.transform
        && t.session.target() == XfTarget::Selection
    {
        return compose(view, t.session.affine());
    }
    let now = (studio.doc_epoch, studio.doc.selection_rev());
    match studio.ants_carry {
        Some((k, xf)) if k == now && a.key != Some(now) => compose(view, xf),
        _ => view,
    }
}

/// The selection outline, drawn after the page guides.
pub fn paint_ants(st: &mut SelectTool, painter: &egui::Painter, studio: &Studio, origin: [f32; 2], ppp: f32) {
    st.sync_ants(studio, false);
    let ctx = painter.ctx();
    if st.ants.pending.is_some() {
        // Poll for the extraction.
        ctx.request_repaint_after(Duration::from_millis(16));
    }
    let m = ants_matrix(&st.ants, studio, origin);
    let a = &st.ants;
    let (Some(c), Some(geom)) = (&a.contours, &a.geom) else { return };
    let zoom = (m.a * m.d - m.b * m.c).abs().sqrt();
    let phase = (ctx.input(|i| i.time) / ANTS_TICK).floor() as f32 * 2.0;
    let lod = lod_for(zoom);
    if a.gpu {
        let cb = AntsCallback { geom: geom.clone(), lod, m, zoom, phase };
        painter.add(ants::paint_callback(painter.clip_rect(), cb));
    } else {
        painter.add(Shape::mesh(ants_mesh(c, &a.bboxes, lod, m, zoom, phase, painter.clip_rect(), ppp)));
    }
    if c.truncated {
        let at = painter.clip_rect().left_bottom() + egui::vec2(8.0, -8.0);
        let text = t(Key::NoticeSelectionTooDetailed);
        painter.text(at, egui::Align2::LEFT_BOTTOM, text, egui::FontId::proportional(12.0), Color32::from_rgb(230, 160, 40));
    }
    if ctx.input(|i| i.focused) {
        ctx.request_repaint_after(Duration::from_secs_f64(ANTS_TICK));
    }
}

/// The egui fallback: the outline at `lod` (coarser while more than
/// [`CPU_SEGMENTS`] segments are visible) as 1-px quads, split into 8-px
/// black and white dashes.
fn ants_mesh(
    c: &Contours,
    bboxes: &[Vec<[f32; 4]>; 3],
    lod: usize,
    m: Affine2,
    zoom: f32,
    phase: f32,
    clip: egui::Rect,
    ppp: f32,
) -> egui::Mesh {
    let mut mesh = egui::Mesh::default();
    // The clip rect in document space, for culling.
    let view_box = inverse(m).map(|inv| {
        let corners = [clip.left_top(), clip.right_top(), clip.right_bottom(), clip.left_bottom()]
            .map(|p| inv.apply([p.x * ppp, p.y * ppp]));
        corners.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |b, q| {
            [b[0].min(q[0]), b[1].min(q[1]), b[2].max(q[0]), b[3].max(q[1])]
        })
    });
    let visible = |b: &[f32; 4]| view_box.is_none_or(|v| b[0] <= v[2] && b[2] >= v[0] && b[1] <= v[3] && b[3] >= v[1]);
    let count = |k: usize| c.lods[k].iter().zip(&bboxes[k]).filter(|(_, b)| visible(b)).map(|(p, _)| p.segments()).sum::<usize>();
    let mut lod = lod;
    while lod < 2 && count(lod) > CPU_SEGMENTS {
        lod += 1;
    }
    let half = 0.5 / ppp;
    let mut budget = CPU_SEGMENTS;
    let quad = |mesh: &mut egui::Mesh, a: Pos2, b: Pos2, color: Color32| {
        let d = b - a;
        let len = d.length();
        if len <= 0.0 {
            return;
        }
        let n = egui::vec2(-d.y, d.x) * (half / len);
        let i = mesh.vertices.len() as u32;
        for p in [a + n, b + n, b - n, a - n] {
            mesh.colored_vertex(p, color);
        }
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i, i + 2, i + 3);
    };
    let half_period = ants::DASH_PERIOD * 0.5;
    'outer: for (p, _) in c.lods[lod].iter().zip(&bboxes[lod]).filter(|(_, b)| visible(b)) {
        let n = p.pts.len();
        let mut arc = 0.0f32;
        for i in 0..p.segments() {
            if budget == 0 {
                break 'outer;
            }
            budget -= 1;
            let (da, db) = (p.pts[i], p.pts[(i + 1) % n]);
            let (sa, sb) = (m.apply(da), m.apply(db));
            let (a, b) = (Pos2::new(sa[0] / ppp, sa[1] / ppp), Pos2::new(sb[0] / ppp, sb[1] / ppp));
            let len = ((db[0] - da[0]).powi(2) + (db[1] - da[1]).powi(2)).sqrt() * zoom;
            // Split at every dash boundary (multiples of half a period).
            let (s0, s1) = (arc + phase, arc + phase + len);
            let mut from = s0;
            while from < s1 {
                let to = (((from / half_period).floor() + 1.0) * half_period).min(s1);
                let black = (from / half_period).floor() as i64 % 2 == 0;
                let (t0, t1) = ((from - s0) / len, (to - s0) / len);
                quad(&mut mesh, a + (b - a) * t0, a + (b - a) * t1, if black { Color32::BLACK } else { Color32::WHITE });
                from = to;
            }
            arc += len;
        }
    }
    mesh
}

/// The Grow / Shrink / Feather modal (`shell.sel_dialog`).
pub fn dialogs(ctx: &egui::Context, studio: &mut Studio, shell: &mut Shell) {
    let Some(kind) = shell.sel_dialog else { return };
    let o = &mut studio.opts.select;
    // The largest values the core applies, so the field shows what is done.
    let (title, px, max) = match kind {
        SelModify::Grow => (t(Key::CmdGrowSelectionDialog), &mut o.grow_px, morph::MAX_RADIUS),
        SelModify::Shrink => (t(Key::CmdShrinkSelectionDialog), &mut o.shrink_px, morph::MAX_RADIUS),
        SelModify::Feather => (t(Key::CmdFeatherSelectionDialog), &mut o.feather_px, morph::MAX_SIGMA),
    };
    let modal = egui::Modal::new(egui::Id::new("sel-modify")).show(ctx, |ui| {
        ui.set_width(260.0);
        ui.heading(title);
        ui.add_space(6.0);
        egui::Grid::new("sel-modify-grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label(if kind == SelModify::Feather { t(Key::SelRadius) } else { t(Key::SelAmount) });
            ui.add(egui::DragValue::new(px).range(1..=max).suffix(" px"));
            ui.end_row();
            if kind != SelModify::Feather {
                ui.label(t(Key::SelShape));
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut o.morph, MorphShape::Circle, t(Key::SelShapeCircle));
                    ui.selectable_value(&mut o.morph, MorphShape::Square, t(Key::SelShapeSquare));
                });
                ui.end_row();
            }
        });
        ui.add_space(10.0);
        ui.horizontal(|ui| (ui.button(egui::RichText::new(t(Key::CommonOk)).strong()).clicked(), ui.button(t(Key::NewDocCancel)).clicked())).inner
    });
    let (ok, cancel) = modal.inner;
    if ok {
        shell.sel_dialog = None;
        let o = &studio.opts.select;
        let cmd = match kind {
            SelModify::Grow => Command::GrowSelection { px: o.grow_px },
            SelModify::Shrink => Command::ShrinkSelection { px: o.shrink_px },
            SelModify::Feather => Command::FeatherSelection { px: o.feather_px },
        };
        commands::execute(cmd, studio, shell);
    } else if cancel || modal.should_close() {
        shell.sel_dialog = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeKind;
    use arty_core::selection::full_mask;
    use arty_core::{Document, MaskView, TileCoord};

    const ORIGIN: [f32; 2] = [400.0, 300.0];
    const ID: Affine2 = Affine2 { a: 1.0, b: 0.0, c: 0.0, d: 1.0, tx: 0.0, ty: 0.0 };

    fn setup(tool: Tool) -> (Studio, Shell, SelectTool) {
        let mut studio = Studio::new(Document::new(256, 192, 72));
        studio.view.center = [128.0, 96.0];
        studio.select_tool(tool);
        (studio, Shell::new(ThemeKind::Dark), SelectTool::default())
    }

    fn at(studio: &Studio, screen: [f32; 2], mods: Modifiers, double: bool) -> ToolInput {
        ToolInput { screen, doc: studio.view.screen_to_doc(ORIGIN).apply(screen), mods, double }
    }

    fn ctx<'a>(studio: &'a mut Studio, shell: &'a mut Shell) -> ToolCtx<'a> {
        ToolCtx { studio, shell, origin: ORIGIN, ppp: 1.0 }
    }

    fn full_tiles(xs: &[i32]) -> Selection {
        let mut s = Selection::new();
        for &x in xs {
            s.insert_tile(TileCoord::new(x, 0), full_mask().clone());
        }
        s
    }

    /// Equal coverage everywhere.
    fn same(a: &Selection, b: &Selection) -> bool {
        a.tile_count() == b.tile_count()
            && a.tiles().all(|(c, _)| match (a.get(c), b.get(c)) {
                (MaskView::Full, MaskView::Full) => true,
                (MaskView::Partial(x), MaskView::Partial(y)) => x == y,
                _ => false,
            })
    }

    fn click(st: &mut SelectTool, studio: &mut Studio, shell: &mut Shell, p: [f32; 2], mods: Modifiers, double: bool) {
        let i = at(studio, p, mods, double);
        let mut c = ctx(studio, shell);
        st.press(&mut c, i);
        st.release(&mut c, i);
    }

    #[test]
    fn su07_rect_on_rotated_view_is_the_inverse_mapped_corners() {
        let (mut studio, mut shell, mut st) = setup(Tool::Select);
        studio.view.rotation = 30f32.to_radians();
        let to_doc = studio.view.screen_to_doc(ORIGIN);
        let (s, c) = ([350.0, 260.0], [470.0, 330.0]);
        let poly = shape_polygon(SelShape::Rect, s, c, false, false, to_doc);
        let want = [s, [c[0], s[1]], c, [s[0], c[1]]].map(|p| to_doc.apply(p));
        assert_eq!(poly.len(), 4);
        for (p, q) in poly.iter().zip(&want) {
            assert!(dist(*p, *q) < 1e-3, "{p:?} vs {q:?}");
        }
        assert!((poly[0][1] - poly[1][1]).abs() > 10.0, "screen-aligned, so rotated in the document");

        // Shift squares it, Alt draws from the centre.
        let sq = shape_polygon(SelShape::Rect, s, [s[0] + 50.0, s[1] + 20.0], true, false, ID);
        assert_eq!(sq[2], [s[0] + 50.0, s[1] + 50.0]);
        let ctr = shape_polygon(SelShape::Rect, [100.0, 100.0], [110.0, 120.0], false, true, ID);
        assert_eq!((ctr[0], ctr[2]), ([90.0, 80.0], [110.0, 120.0]));

        // An ellipse is flattened on screen: its points map back onto it.
        let e = shape_polygon(SelShape::Ellipse, s, c, false, false, to_doc);
        assert_eq!(e.len(), ellipse_segments(60.0));
        let to_screen = studio.view.doc_to_screen(ORIGIN);
        for p in &e {
            let q = to_screen.apply(*p);
            let r = ((q[0] - 410.0) / 60.0).powi(2) + ((q[1] - 295.0) / 35.0).powi(2);
            assert!((r - 1.0).abs() < 1e-3, "{q:?} is off the screen ellipse");
        }
        assert_eq!(ellipse_segments(100.0), 71);
        assert_eq!(ellipse_segments(0.0), 8);

        // Through the tool: that polygon is rasterized and selected.
        let mut c2 = ctx(&mut studio, &mut shell);
        let (pi, ci) = (at(c2.studio, s, Modifiers::NONE, false), at(c2.studio, c, Modifiers::NONE, false));
        st.press(&mut c2, pi);
        st.drag(&mut c2, ci);
        assert!(st.gesture_active());
        st.release(&mut c2, ci);
        assert!(!st.gesture_active());
        let want = rasterize_polygon(&poly, 256, 192, true);
        assert!(same(studio.doc.selection(), &want));
        assert_eq!(studio.history.undo_len(), usize::from(!want.is_empty()));
    }

    #[test]
    fn su08_modifiers_pick_the_mode() {
        let m = |shift, alt| Modifiers { shift, alt, ..Modifiers::NONE };
        for default in [SelectOp::Replace, SelectOp::Add, SelectOp::Subtract, SelectOp::Intersect] {
            assert_eq!(op_for(m(true, false), default), SelectOp::Add);
            assert_eq!(op_for(m(false, true), default), SelectOp::Subtract);
            assert_eq!(op_for(m(true, true), default), SelectOp::Intersect);
            assert_eq!(op_for(m(false, false), default), default);
            assert_eq!(op_for(Modifiers::COMMAND, default), default, "Ctrl is not a selection modifier");
        }
    }

    #[test]
    fn su09_selection_commands_are_one_step_each() {
        let (mut studio, mut shell, _) = setup(Tool::Select);
        execute(Command::Deselect, &mut studio, &mut shell);
        assert_eq!(studio.history.undo_len(), 0, "deselect with no selection is no step");

        let a = full_tiles(&[1]);
        studio.set_selection(a.clone());
        let steps = studio.history.undo_len();
        execute(Command::Deselect, &mut studio, &mut shell);
        assert!(!studio.doc.has_selection());
        assert_eq!(studio.history.undo_len(), steps + 1);
        studio.undo();
        assert!(studio.doc.selection().shares_storage(&a), "undo restores the selection");

        for cmd in [Command::SelectAll, Command::InvertSelection] {
            let before = studio.doc.selection().clone();
            let steps = studio.history.undo_len();
            execute(cmd, &mut studio, &mut shell);
            assert_eq!(studio.history.undo_len(), steps + 1, "{cmd:?}");
            studio.undo();
            assert!(studio.doc.selection().shares_storage(&before), "{cmd:?}: undo restores");
            studio.redo();
        }
        execute(Command::SelectAll, &mut studio, &mut shell);
        assert!(same(studio.doc.selection(), &Selection::all(256, 192)));
    }

    #[test]
    fn su10_click_deselects_and_modifier_click_does_nothing() {
        let (mut studio, mut shell, mut st) = setup(Tool::Select);
        studio.set_selection(full_tiles(&[0, 1]));
        let sel = studio.doc.selection().clone();
        for mods in [Modifiers::SHIFT, Modifiers::ALT, Modifiers::SHIFT | Modifiers::ALT] {
            click(&mut st, &mut studio, &mut shell, [400.0, 300.0], mods, false);
            assert!(studio.doc.selection().shares_storage(&sel), "{mods:?}");
        }
        // A wiggle under 2 px is still a click.
        let mut c = ctx(&mut studio, &mut shell);
        let (p, q) = (at(c.studio, [400.0, 300.0], Modifiers::NONE, false), at(c.studio, [401.0, 301.0], Modifiers::NONE, false));
        st.press(&mut c, p);
        st.drag(&mut c, q);
        st.release(&mut c, q);
        assert!(!studio.doc.has_selection(), "a New-mode click deselects");
        assert_eq!(studio.history.undo_len(), 2);
        // With nothing selected it is no step.
        click(&mut st, &mut studio, &mut shell, [400.0, 300.0], Modifiers::NONE, false);
        assert_eq!(studio.history.undo_len(), 2);
    }

    #[test]
    fn su11_polygon_closes_pops_and_cancels() {
        let (mut studio, mut shell, mut st) = setup(Tool::Select);
        studio.opts.select.shape = SelShape::Polygon;
        let tri = [[350.0, 250.0], [450.0, 260.0], [420.0, 340.0]];
        let none = Modifiers::NONE;

        // Enter closes, as one step.
        studio.set_selection(full_tiles(&[3]));
        let steps = studio.history.undo_len();
        for p in tri {
            click(&mut st, &mut studio, &mut shell, p, none, false);
        }
        assert_eq!(st.polygon_len(), Some(3));
        assert!(st.gesture_active(), "an unclosed polygon holds the shortcuts");
        assert!(st.key(&mut ctx(&mut studio, &mut shell), egui::Key::Enter));
        assert!(!st.gesture_active());
        assert_eq!(studio.history.undo_len(), steps + 1);
        let doc_tri: Vec<Pt> = tri.iter().map(|&p| studio.view.screen_to_doc(ORIGIN).apply(p)).collect();
        assert!(same(studio.doc.selection(), &rasterize_polygon(&doc_tri, 256, 192, true)));

        // A click within 8 px of the first point closes; so does a double click.
        for (close, double) in [([353.0, 254.0], false), ([419.0, 338.0], true)] {
            studio.set_selection(full_tiles(&[3]));
            let steps = studio.history.undo_len();
            for p in tri {
                click(&mut st, &mut studio, &mut shell, p, none, false);
            }
            click(&mut st, &mut studio, &mut shell, close, none, double);
            assert!(!st.gesture_active(), "closed by {close:?}");
            assert_eq!(studio.history.undo_len(), steps + 1);
        }

        // Backspace pops a vertex; popping the last ends the polygon.
        for p in tri {
            click(&mut st, &mut studio, &mut shell, p, none, false);
        }
        assert!(st.key(&mut ctx(&mut studio, &mut shell), egui::Key::Backspace));
        assert_eq!(st.polygon_len(), Some(2));
        for _ in 0..2 {
            st.key(&mut ctx(&mut studio, &mut shell), egui::Key::Backspace);
        }
        assert_eq!(st.polygon_len(), None);

        // Cancel and Esc leave no step.
        let (steps, sel) = (studio.history.undo_len(), studio.doc.selection().clone());
        for p in tri {
            click(&mut st, &mut studio, &mut shell, p, none, false);
        }
        st.cancel(&mut ctx(&mut studio, &mut shell));
        for p in tri {
            click(&mut st, &mut studio, &mut shell, p, none, false);
        }
        assert!(st.key(&mut ctx(&mut studio, &mut shell), egui::Key::Escape));
        assert!(!st.gesture_active());
        // Switching to the wand (same tool state) abandons the polygon too.
        for p in tri {
            click(&mut st, &mut studio, &mut shell, p, none, false);
        }
        studio.select_tool(Tool::MagicWand);
        st.tick(&mut ctx(&mut studio, &mut shell), 0.0);
        assert!(!st.gesture_active());
        assert_eq!(studio.history.undo_len(), steps);
        assert!(studio.doc.selection().shares_storage(&sel));
    }

    #[test]
    fn su12_grow_routes_to_morph_and_zero_is_a_no_op() {
        let (mut studio, mut shell, _) = setup(Tool::Select);
        execute(Command::GrowSelection { px: 3 }, &mut studio, &mut shell);
        assert_eq!(studio.history.undo_len(), 0, "nothing selected: no step");

        let mut m = [[0u8; 64]; 64];
        for row in &mut m[20..40] {
            row[10..30].fill(255);
        }
        let mut sel = Selection::new();
        sel.insert_tile(TileCoord::new(1, 1), Arc::new(m));
        studio.set_selection(sel.clone());
        let steps = studio.history.undo_len();
        for cmd in [Command::GrowSelection { px: 0 }, Command::ShrinkSelection { px: 0 }, Command::FeatherSelection { px: 0 }] {
            execute(cmd, &mut studio, &mut shell);
            assert_eq!(studio.history.undo_len(), steps, "{cmd:?} by 0 px is a no-op");
        }
        for shape in [MorphShape::Circle, MorphShape::Square] {
            studio.opts.select.morph = shape;
            let want = morph::grow(&sel, 3, shape, 256, 192);
            execute(Command::GrowSelection { px: 3 }, &mut studio, &mut shell);
            assert!(same(studio.doc.selection(), &want), "{shape:?}");
            assert_eq!(studio.history.undo_len(), steps + 1, "one step");
            studio.undo();
            assert!(studio.doc.selection().shares_storage(&sel));
        }
        execute(Command::FeatherSelection { px: 2 }, &mut studio, &mut shell);
        assert!(same(studio.doc.selection(), &morph::feather(&sel, 2, 256, 192)));
    }

    #[test]
    fn su13_ants_rebuild_only_on_a_new_revision() {
        let (mut studio, _, mut st) = setup(Tool::Select);
        let ui_ctx = egui::Context::default();
        let frame = |st: &mut SelectTool, studio: &Studio| {
            ui_ctx
                .run_ui(egui::RawInput::default(), |ui| paint_ants(st, ui.painter(), studio, ORIGIN, 1.0))
                .drop_without_applying_deltas();
        };
        frame(&mut st, &studio);
        assert_eq!(st.ants.builds, 0, "no selection, no outline");

        studio.set_selection(full_tiles(&[1]));
        st.sync_ants(&studio, true);
        assert_eq!(st.ants.builds, 1);
        assert_eq!(st.ants.contours.as_ref().unwrap().segments, 4);
        for _ in 0..5 {
            frame(&mut st, &studio);
        }
        studio.view.zoom = 0.05;
        studio.view.rotation = 1.0;
        frame(&mut st, &studio);
        assert_eq!(st.ants.builds, 1, "frames and view changes reuse the vertex buffer");

        studio.set_selection(full_tiles(&[1, 2]));
        st.sync_ants(&studio, true);
        frame(&mut st, &studio);
        assert_eq!(st.ants.builds, 2);
        // A new document restarts the revisions: still a new outline.
        studio.new_document(256, 192, 72);
        studio.set_selection(full_tiles(&[0]));
        st.sync_ants(&studio, true);
        assert_eq!(st.ants.builds, 3);
        assert_eq!(st.ants.geom.as_ref().unwrap().segments(0), 4);
    }

    /// A selection-target session moved by 64 px to the right: pixels in
    /// tile (1, 1), the selection on it.
    fn moved_session(studio: &mut Studio) {
        let id = studio.doc.active();
        let (g, _) = studio.doc.paint_target(id).unwrap();
        g.get_mut_or_create(TileCoord::new(1, 1))[5][5] = [0, 0, 0, fix15::ONE_U16];
        studio.set_selection(full_tiles_at(&[(1, 1)]));
        assert!(studio.begin_transform(false));
        let t = studio.transform.as_mut().unwrap();
        assert_eq!(t.session.target(), XfTarget::Selection);
        let p = t.session.params();
        t.request(arty_core::transform::XfParams { t: [64.0, 0.0], ..p });
    }

    fn full_tiles_at(cs: &[(i32, i32)]) -> Selection {
        let mut s = Selection::new();
        for &(x, y) in cs {
            s.insert_tile(TileCoord::new(x, y), full_mask().clone());
        }
        s
    }

    /// Invert, Grow, Shrink and Feather during a selection-target session
    /// work on where the selection went.
    #[test]
    fn selection_commands_during_a_session_use_the_moved_selection() {
        for cmd in [Command::InvertSelection, Command::GrowSelection { px: 2 }, Command::ShrinkSelection { px: 2 }] {
            let (mut studio, mut shell, _) = setup(Tool::Select);
            moved_session(&mut studio);
            let steps = studio.history.undo_len();
            execute(cmd, &mut studio, &mut shell);
            assert!(studio.transform.is_none(), "{cmd:?} committed the session");
            assert_eq!(studio.history.undo_len(), steps + 2, "{cmd:?}: the commit, then the command");
            let sel = studio.doc.selection();
            let (old, new) = (sel.value(64 + 32, 64 + 32), sel.value(128 + 32, 64 + 32));
            if cmd == Command::InvertSelection {
                assert_eq!((old, new), (255, 0), "the inverse of the moved selection");
            } else {
                assert_eq!((old, new), (0, 255), "{cmd:?} of the moved selection");
            }
        }
    }

    /// Subtracting or inverting the whole page on a page whose sides are not
    /// multiples of 64 deselects (no tiles left past the page).
    #[test]
    fn subtracting_the_whole_page_deselects() {
        let mut studio = Studio::new(Document::new(300, 130, 72));
        studio.set_selection(Selection::all(300, 130));
        let beyond: [Pt; 4] = [[-10.0, -10.0], [400.0, -10.0], [400.0, 200.0], [-10.0, 200.0]];
        let shape = rasterize_polygon(&beyond, 300, 130, true);
        apply_shape(&mut studio, &shape, SelectOp::Subtract);
        assert!(!studio.doc.has_selection(), "deselected");
        studio.set_selection(shape);
        execute(Command::InvertSelection, &mut studio, &mut Shell::new(ThemeKind::Dark));
        assert!(!studio.doc.has_selection());
    }

    /// After a selection-target commit the ants keep the session's affine
    /// until the moved selection's outline is extracted.
    #[test]
    fn ants_stay_moved_until_the_new_outline_lands() {
        let (mut studio, _, mut st) = setup(Tool::Select);
        moved_session(&mut studio);
        st.sync_ants(&studio, true);
        let view = studio.view.doc_to_screen(ORIGIN);
        let moved = compose(view, studio.transform.as_ref().unwrap().session.affine());
        assert_eq!(ants_matrix(&st.ants, &studio, ORIGIN), moved, "during the session");
        studio.commit_transform();
        // The extraction for the committed selection has not landed yet.
        assert_ne!(st.ants.key, Some((studio.doc_epoch, studio.doc.selection_rev())));
        assert_eq!(ants_matrix(&st.ants, &studio, ORIGIN), moved, "still where the content went");
        st.sync_ants(&studio, true);
        assert_eq!(ants_matrix(&st.ants, &studio, ORIGIN), view, "the moved outline, drawn as is");
        // Undo is a new selection: no carry.
        studio.undo();
        assert_eq!(ants_matrix(&st.ants, &studio, ORIGIN), view);
    }

    #[test]
    fn su_wand_click_selects_the_region_fill_finds() {
        let (mut studio, mut shell, mut st) = setup(Tool::MagicWand);
        let p = [400.0, 300.0];
        let seed = at(&studio, p, Modifiers::NONE, false).doc.map(|v| v.floor() as i32);
        let o = WandOptions::default();
        let params = FillParams {
            reference: o.reference,
            tolerance: (o.tolerance * fix15::ONE as f32).round() as u16,
            gap_px: fill::gap_radius(o.gap, 72),
            contiguous: o.contiguous,
            antialias: o.antialias,
            use_selection: false,
            ..FillParams::default()
        };
        let want = fill::fill_region(&studio.doc, (seed[0], seed[1]), &params, &mut FillScratch::default());
        click(&mut st, &mut studio, &mut shell, p, Modifiers::NONE, false);
        assert!(!st.gesture_active(), "the wand has no drag gesture");
        match want {
            Some(w) => assert!(same(studio.doc.selection(), &w)),
            None => assert_eq!(studio.history.undo_len(), 0),
        }
        let steps = studio.history.undo_len();
        click(&mut st, &mut studio, &mut shell, [5.0, 5.0], Modifiers::NONE, false);
        assert_eq!(studio.history.undo_len(), steps, "off the page nothing happens");
    }

    #[test]
    fn su_cpu_ants_alternate_eight_px_dashes() {
        let line = contour::Polyline { pts: vec![[0.0, 0.0], [40.0, 0.0]], closed: false };
        let c = Contours { lods: [vec![line], vec![], vec![]], segments: 1, ..Default::default() };
        let bboxes = [vec![[0.0, 0.0, 40.0, 0.0]], vec![], vec![]];
        let m = Affine2 { ty: 0.5, ..ID };
        let clip = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(100.0, 100.0));
        let mesh = ants_mesh(&c, &bboxes, 0, m, 1.0, 0.0, clip, 1.0);
        // 40 px is 5 dashes of 8 px: black, white, black, white, black.
        let colors: Vec<Color32> = mesh.vertices.chunks(4).map(|q| q[0].color).collect();
        assert_eq!(colors, [Color32::BLACK, Color32::WHITE, Color32::BLACK, Color32::WHITE, Color32::BLACK]);
        // The phase shifts the pattern; off-screen outlines are culled.
        assert_eq!(ants_mesh(&c, &bboxes, 0, m, 1.0, 4.0, clip, 1.0).vertices.len(), 6 * 4);
        assert!(ants_mesh(&c, &bboxes, 0, Affine2 { tx: 500.0, ..m }, 1.0, 0.0, clip, 1.0).vertices.is_empty());
        assert_eq!((lod_for(1.0), lod_for(0.25), lod_for(0.05)), (0, 1, 2));
    }

    /// B007: CPU time per frame of the ants on a B4 600 dpi page, for the
    /// egui fallback (mesh build + tessellation) and the GPU path (callback).
    ///
    /// cargo test -p arty-app --release su_bench_ants_frame -- --ignored --nocapture
    #[test]
    #[ignore]
    fn su_bench_ants_frame() {
        use std::time::Instant;
        let (w, h) = (6071u32, 8598u32);
        let mut studio = Studio::new(Document::new(w, h, 600));
        // A field of AA dots (24 px pitch, radius 8) over 3000×3000 px:
        // far more outline than the fallback's 20k-segment cap.
        let mut sel = Selection::new();
        for ty in 0..47 {
            for tx in 0..47 {
                let mut m = [[0u8; 64]; 64];
                for (y, row) in m.iter_mut().enumerate() {
                    for (x, px) in row.iter_mut().enumerate() {
                        let (gx, gy) = ((tx * 64 + x) as f32 + 0.5, (ty * 64 + y) as f32 + 0.5);
                        let (fx, fy) = ((gx / 24.0).fract() - 0.5, (gy / 24.0).fract() - 0.5);
                        let d = (fx * fx + fy * fy).sqrt() * 24.0;
                        *px = ((8.5 - d).clamp(0.0, 1.0) * 255.0).round() as u8;
                    }
                }
                sel.insert_tile(TileCoord::new(tx as i32 + 10, ty as i32 + 10), Arc::new(m));
            }
        }
        studio.set_selection(sel);
        let mut st = SelectTool::default();
        let t = Instant::now();
        st.sync_ants(&studio, true);
        let c = st.ants.contours.clone().unwrap();
        println!(
            "outline: {:.1} ms to extract, LOD0/1/2 segments {} / {} / {}",
            t.elapsed().as_secs_f64() * 1e3,
            st.ants.geom.as_ref().unwrap().segments(0),
            st.ants.geom.as_ref().unwrap().segments(1),
            st.ants.geom.as_ref().unwrap().segments(2)
        );
        assert!(!c.truncated);
        let ui_ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(1600.0, 1000.0));
        let raw = || egui::RawInput { screen_rect: Some(screen), ..Default::default() };
        let origin = [800.0, 500.0];
        println!("| path | view | ants CPU (ms/frame) | tessellation (ms/frame) | vertices |");
        println!("|---|---|---|---|---|");
        for (gpu, zoom, label) in [
            (false, 0.12, "whole page (zoom 12%)"),
            (false, 1.0, "100% on the dots"),
            (true, 0.12, "whole page (zoom 12%)"),
            (true, 1.0, "100% on the dots"),
        ] {
            st.ants.gpu = gpu;
            studio.view.zoom = zoom;
            studio.view.center = if zoom < 1.0 { [w as f32 / 2.0, h as f32 / 2.0] } else { [1500.0, 1500.0] };
            let frames = 60;
            let (mut paint, mut tess, mut verts) = (0.0, 0.0, 0);
            for _ in 0..frames {
                let mut dt = 0.0;
                let out = ui_ctx.run_ui(raw(), |ui| {
                    let t = Instant::now();
                    paint_ants(&mut st, &ui.painter().with_clip_rect(screen), &studio, origin, 1.0);
                    dt = t.elapsed().as_secs_f64();
                });
                paint += dt;
                let t = Instant::now();
                let prims = ui_ctx.tessellate(out.shapes, 1.0);
                tess += t.elapsed().as_secs_f64();
                verts = prims
                    .iter()
                    .map(|p| match &p.primitive {
                        egui::epaint::Primitive::Mesh(m) => m.vertices.len(),
                        egui::epaint::Primitive::Callback(_) => 0,
                    })
                    .sum::<usize>();
            }
            let ms = |s: f64| s * 1e3 / frames as f64;
            let path = if gpu { "GPU callback" } else { "egui fallback" };
            println!("| {path} | {label} | {:.4} | {:.4} | {verts} |", ms(paint), ms(tess));
        }
    }
}
