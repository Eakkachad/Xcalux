//! Pressure curve editor widget, in the style of CSP's "Adjust pen pressure".
//!
//! Press on empty plot space to add a point (and keep dragging it), drag points
//! to move them (endpoints only vertically), double-click or right-click an
//! interior point to remove it. A strip below the plot is a scratch test pad:
//! what you draw there shows the width the curve gives, without touching the
//! document.

use std::time::Duration;

use arty_brush::pressure::{MAX_CURVE_POINTS, PressureCurve};
use egui::{Align2, Color32, CornerRadius, CursorIcon, DragValue, FontId, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2};

/// Largest plot side, px.
const MAX_SIDE: f32 = 220.0;
/// Inset of the plot inside its allocation, so endpoint handles are not clipped.
const PAD: f32 = 6.0;
const POINT_R: f32 = 4.0;
const HIT_R: f32 = 7.0;
/// Samples of the drawn curve (what you see is `eval`, the hot-path LUT).
const CURVE_SAMPLES: usize = 128;
const TEST_PAD_H: f32 = 48.0;
const TEST_PAD_MAX: usize = 512;
/// How long the live pen marker stays after the last pen sample, s.
const LIVE_HOLD: f64 = 1.0;

/// Per-widget interaction state, kept in egui temp memory.
#[derive(Clone, Default)]
struct EditorState {
    /// Point being dragged: its index, the pointer's offset from it at the
    /// press (so an off-centre grab does not jump) and the pointer position
    /// last applied.
    drag: Option<(usize, Vec2, Pos2)>,
    /// Point shown in the Input/Output fields.
    selected: Option<usize>,
    /// Last pen pressure seen and when (egui time).
    live: Option<(f32, f64)>,
    /// Test pad stroke: positions relative to the pad and raw pressure.
    pad: Vec<(Vec2, f32)>,
}

/// Curve coordinates (0..=1, y up) → screen position inside `rect`.
pub fn to_screen(rect: Rect, p: [f32; 2]) -> Pos2 {
    Pos2::new(rect.left() + p[0] * rect.width(), rect.bottom() - p[1] * rect.height())
}

/// Screen position → curve coordinates, clamped to 0..=1.
pub fn from_screen(rect: Rect, pos: Pos2) -> [f32; 2] {
    [((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0), ((rect.bottom() - pos.y) / rect.height()).clamp(0.0, 1.0)]
}

/// The point nearest to `pos` within the hit radius.
pub fn pick(curve: &PressureCurve, rect: Rect, pos: Pos2) -> Option<usize> {
    curve
        .points()
        .iter()
        .enumerate()
        .map(|(i, &p)| (i, to_screen(rect, p).distance(pos)))
        .filter(|&(_, d)| d <= HIT_R)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// Pen pressure of this frame: the last `Touch` event that carries a force.
fn live_pressure(events: &[egui::Event]) -> Option<f32> {
    events.iter().rev().find_map(|e| match e {
        egui::Event::Touch { force: Some(f), .. } => Some(f.clamp(0.0, 1.0)),
        _ => None,
    })
}

/// The plot inside the widget's allocation.
fn plot_rect(outer: Rect) -> Rect {
    outer.shrink(PAD)
}

/// Edits `curve` in place; the returned (plot) response is `changed()` on any edit.
pub fn ui(ui: &mut egui::Ui, curve: &mut PressureCurve) -> egui::Response {
    let id = ui.id().with("pressure-curve");
    let mut st = ui.data_mut(|d| std::mem::take(d.get_temp_mut_or_default::<EditorState>(id)));
    let (now, frame_force) = ui.input(|i| (i.time, live_pressure(&i.events)));
    if let Some(f) = frame_force {
        st.live = Some((f, now));
    }
    let live = st.live.filter(|&(_, t)| now - t < LIVE_HOLD).map(|(f, _)| f);
    if live.is_some() {
        ui.ctx().request_repaint_after(Duration::from_secs_f64(LIVE_HOLD));
    }

    let side = ui.available_width().min(MAX_SIDE);
    let (outer, mut resp) = ui.allocate_exact_size(Vec2::splat(side), Sense::click_and_drag());
    let plot = plot_rect(outer);
    let mut changed = false;

    // Press: grab the nearest point, or add one and keep dragging it.
    let pressed = resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_pressed());
    if pressed && let Some(pos) = resp.interact_pointer_pos() {
        let hit = pick(curve, plot, pos).or_else(|| curve.insert_point(from_screen(plot, pos)).inspect(|_| changed = true));
        st.drag = hit.map(|i| (i, pos - to_screen(plot, curve.points()[i]), pos));
        if hit.is_some() {
            st.selected = hit;
        }
    }
    // Drag: the grabbed point follows the pointer (absolute, so travel past a
    // limit is not lost: the point comes back when the pointer does).
    match (st.drag, resp.interact_pointer_pos()) {
        (Some((i, off, last)), Some(pos)) if resp.is_pointer_button_down_on() => {
            if pos != last && i < curve.points().len() {
                changed |= curve.set_point(i, from_screen(plot, pos - off));
                st.drag = Some((i, off, pos));
            }
        }
        _ => st.drag = None,
    }
    // Remove: double-click or right-click an interior point.
    if resp.double_clicked() || resp.secondary_clicked() {
        let pos = resp.interact_pointer_pos().or(resp.hover_pos());
        if let Some(i) = pos.and_then(|p| pick(curve, plot, p))
            && curve.remove_point(i)
        {
            changed = true;
            st.drag = None;
            st.selected = match st.selected {
                Some(s) if s == i => None,
                Some(s) if s > i => Some(s - 1),
                s => s,
            };
        }
    }

    let hover = resp.hover_pos();
    let hovered_point = hover.and_then(|p| pick(curve, plot, p));
    if st.drag.is_some() {
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    } else if hovered_point.is_some() {
        ui.ctx().set_cursor_icon(CursorIcon::Grab);
    } else if hover.is_some() && curve.points().len() >= MAX_CURVE_POINTS {
        resp = resp.on_hover_text(format!("Up to {MAX_CURVE_POINTS} points"));
    }

    // Plot.
    let v = ui.visuals();
    let accent = v.selection.stroke.color;
    let grid = Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color);
    let weak = v.weak_text_color();
    let painter = ui.painter_at(outer);
    painter.rect_filled(plot, CornerRadius::same(2), v.extreme_bg_color);
    for k in 1..4 {
        let t = k as f32 / 4.0;
        painter.line_segment([to_screen(plot, [t, 0.0]), to_screen(plot, [t, 1.0])], grid);
        painter.line_segment([to_screen(plot, [0.0, t]), to_screen(plot, [1.0, t])], grid);
    }
    painter.rect_stroke(plot, CornerRadius::same(2), grid, StrokeKind::Inside);
    painter.line_segment([to_screen(plot, [0.0, 0.0]), to_screen(plot, [1.0, 1.0])], Stroke::new(1.0, weak.gamma_multiply(0.5)));
    let small = FontId::proportional(9.5);
    painter.text(plot.right_bottom() + Vec2::new(-3.0, -2.0), Align2::RIGHT_BOTTOM, "Input", small.clone(), weak);
    painter.text(plot.left_top() + Vec2::new(3.0, 2.0), Align2::LEFT_TOP, "Output", small, weak);

    let line: Vec<Pos2> = (0..=CURVE_SAMPLES)
        .map(|i| {
            let x = i as f32 / CURVE_SAMPLES as f32;
            to_screen(plot, [x, curve.eval(x)])
        })
        .collect();
    painter.add(Shape::line(line, Stroke::new(2.0, accent)));

    if let Some(raw) = live {
        let marker = Stroke::new(1.0, accent.gamma_multiply(0.6));
        painter.line_segment([to_screen(plot, [raw, 0.0]), to_screen(plot, [raw, 1.0])], marker);
        painter.circle_filled(to_screen(plot, [raw, curve.eval(raw)]), 3.5, accent);
    }

    let active = st.drag.map(|(i, ..)| i).or(hovered_point);
    for (i, &p) in curve.points().iter().enumerate() {
        let c = to_screen(plot, p);
        let hot = active == Some(i) || st.selected == Some(i);
        painter.circle_filled(c, POINT_R, if hot { accent } else { v.text_color() });
        painter.circle_stroke(c, POINT_R, Stroke::new(1.0, v.extreme_bg_color));
    }

    // Readout: the live pen, else the hovered input.
    let readout = live.or_else(|| hover.filter(|p| plot.contains(*p)).map(|p| from_screen(plot, p)[0]));
    ui.label(egui::RichText::new(match readout {
        Some(x) => format!("In {:.0}% → Out {:.0}%", x * 100.0, curve.eval(x) * 100.0),
        None => "In – → Out –".to_owned(),
    })
    .small()
    .color(weak));

    // Selected point fields and Reset.
    st.selected = st.selected.filter(|&i| i < curve.points().len());
    ui.horizontal(|ui| {
        if let Some(i) = st.selected {
            let [x, y] = curve.points()[i];
            let interior = i > 0 && i + 1 < curve.points().len();
            let (mut xi, mut yo) = (x * 100.0, y * 100.0);
            ui.label("Input %");
            let rx = ui.add_enabled(interior, DragValue::new(&mut xi).range(0.0..=100.0).speed(0.5).max_decimals(1));
            ui.label("Output %");
            let ry = ui.add(DragValue::new(&mut yo).range(0.0..=100.0).speed(0.5).max_decimals(1));
            if rx.changed() || ry.changed() {
                changed |= curve.set_point(i, [xi / 100.0, yo / 100.0]);
            }
        }
        if ui.button("Reset").on_hover_text("Straight line: output = input").clicked() && *curve != PressureCurve::linear() {
            *curve = PressureCurve::linear();
            st.selected = None;
            st.drag = None;
            changed = true;
        }
    });

    test_pad(ui, curve, &mut st, frame_force);

    ui.data_mut(|d| d.insert_temp(id, st));
    if changed {
        resp.mark_changed();
    }
    resp
}

/// Scratch strip: drags draw a line `1 + 7·curve(raw)` px wide (mouse: raw = 1).
fn test_pad(ui: &mut egui::Ui, curve: &PressureCurve, st: &mut EditorState, frame_force: Option<f32>) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), TEST_PAD_H), Sense::drag());
    if resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_pressed()) {
        st.pad.clear();
    }
    if resp.is_pointer_button_down_on()
        && let Some(pos) = resp.interact_pointer_pos()
        && rect.contains(pos)
        && st.pad.len() < TEST_PAD_MAX
    {
        let rel = pos - rect.min;
        if st.pad.last().is_none_or(|&(last, _)| last != rel) {
            st.pad.push((rel, frame_force.unwrap_or(1.0)));
        }
    }

    let v = ui.visuals();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(2), v.extreme_bg_color);
    painter.rect_stroke(rect, CornerRadius::same(2), Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color), StrokeKind::Inside);
    if st.pad.is_empty() {
        painter.text(rect.center(), Align2::CENTER_CENTER, "Test pad", FontId::proportional(10.0), v.weak_text_color());
        return;
    }
    let ink: Color32 = v.text_color();
    let width = |raw: f32| 1.0 + 7.0 * curve.eval(raw);
    for w in st.pad.windows(2) {
        let (a, b) = (rect.min + w[0].0, rect.min + w[1].0);
        painter.line_segment([a, b], Stroke::new(width(w[1].1), ink));
    }
    // Round joints so thick segments don't show gaps.
    for &(p, raw) in &st.pad {
        painter.circle_filled(rect.min + p, width(raw) * 0.5, ink);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Modifiers, PointerButton, RawInput, TouchDeviceId, TouchId, TouchPhase, pos2};

    /// Headless frames driving the editor alone.
    struct Harness {
        ctx: egui::Context,
        curve: PressureCurve,
        time: f64,
        /// Plot rect of the last frame.
        plot: Rect,
        /// Screen rect of the Reset button in the last frame.
        reset: Option<Rect>,
        /// Accessibility labels of the last frame's widgets.
        labels: Vec<String>,
    }

    impl Harness {
        fn new(curve: PressureCurve) -> Self {
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            let mut h = Self { ctx, curve, time: 0.0, plot: Rect::NOTHING, reset: None, labels: Vec::new() };
            h.frame(vec![]); // lay out
            h
        }

        /// Runs one frame; returns whether the editor reported a change.
        fn frame(&mut self, events: Vec<Event>) -> bool {
            self.time += 1.0 / 60.0;
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(300.0, 500.0))),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let (mut changed, mut outer) = (false, Rect::NOTHING);
            let curve = &mut self.curve;
            let mut out = self.ctx.run_ui(input, |ui| {
                let r = super::ui(ui, curve);
                changed = r.changed();
                outer = r.rect;
            });
            let update = out.platform_output.accesskit_update.take();
            out.drop_without_applying_deltas();
            self.plot = plot_rect(outer);
            let nodes = update.map(|u| u.nodes).unwrap_or_default();
            // Buttons carry their text as the label, plain labels as the value.
            self.labels = nodes.iter().filter_map(|(_, n)| n.label().or(n.value()).map(str::to_owned)).collect();
            self.reset = nodes.iter().find(|(_, n)| n.label() == Some("Reset")).and_then(|(_, n)| n.bounds()).map(|b| {
                Rect::from_min_max(pos2(b.x0 as f32, b.y0 as f32), pos2(b.x1 as f32, b.y1 as f32))
            });
            changed
        }

        fn at(&self, p: [f32; 2]) -> Pos2 {
            to_screen(self.plot, p)
        }

        fn button(&mut self, pos: Pos2, button: PointerButton, pressed: bool) -> bool {
            self.frame(vec![Event::PointerButton { pos, button, pressed, modifiers: Modifiers::NONE }])
        }

        /// Move, press, release: a click. Returns whether any frame changed the curve.
        fn click(&mut self, pos: Pos2) -> bool {
            let a = self.frame(vec![Event::PointerMoved(pos)]);
            let b = self.button(pos, PointerButton::Primary, true);
            let c = self.button(pos, PointerButton::Primary, false);
            a | b | c
        }

        fn drag(&mut self, from: Pos2, to: Pos2) -> bool {
            let path: Vec<Pos2> = (1..=4).map(|k| from + (to - from) * k as f32 / 4.0).collect();
            self.drag_path(from, &path)
        }

        /// Press at `from`, move through `path` one frame per point, release at its end.
        fn drag_path(&mut self, from: Pos2, path: &[Pos2]) -> bool {
            let mut changed = self.frame(vec![Event::PointerMoved(from)]);
            changed |= self.button(from, PointerButton::Primary, true);
            for &p in path {
                changed |= self.frame(vec![Event::PointerMoved(p)]);
            }
            changed |= self.button(*path.last().unwrap_or(&from), PointerButton::Primary, false);
            changed
        }
    }

    fn three_points() -> PressureCurve {
        PressureCurve::from_points(&[[0.0, 0.0], [0.5, 0.5], [1.0, 1.0]])
    }

    fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 0.01 && (a[1] - b[1]).abs() < 0.01
    }

    #[test]
    fn drag_interior_point_moves_it() {
        let mut h = Harness::new(three_points());
        let (from, to) = (h.at([0.5, 0.5]), h.at([0.6, 0.3]));
        assert!(h.drag(from, to));
        assert_eq!(h.curve.points().len(), 3);
        assert!(close(h.curve.points()[1], [0.6, 0.3]), "{:?}", h.curve);
        // Grabbing off-centre keeps the offset instead of snapping the point.
        let grab = h.at([0.6, 0.3]) + Vec2::new(4.0, 0.0);
        h.drag(grab, grab + Vec2::new(0.0, -h.plot.height() * 0.2));
        assert!(close(h.curve.points()[1], [0.6, 0.5]), "{:?}", h.curve);
    }

    /// Dragging past a limit and back returns the point under the pointer
    /// (deltas measured from the clamped point used to lose the overshoot).
    #[test]
    fn drag_past_a_limit_and_back_keeps_the_grab() {
        let mut h = Harness::new(PressureCurve::from_points(&[[0.0, 0.0], [0.5, 0.9], [1.0, 1.0]]));
        let start = h.at([0.5, 0.9]) + Vec2::new(3.0, 2.0);
        let off = start - h.at([0.5, 0.9]);
        // Up 100 px past the top of the plot, then back down to the start.
        let up = start + Vec2::new(0.0, -100.0);
        h.frame(vec![Event::PointerMoved(start)]);
        h.button(start, PointerButton::Primary, true);
        for k in 1..=5 {
            h.frame(vec![Event::PointerMoved(start.lerp(up, k as f32 / 5.0))]);
        }
        assert!(close(h.curve.points()[1], [0.5, 1.0]), "clamped at the top: {:?}", h.curve);
        for k in 1..=5 {
            h.frame(vec![Event::PointerMoved(up.lerp(start, k as f32 / 5.0))]);
        }
        h.button(start, PointerButton::Primary, false);
        assert!(close(h.curve.points()[1], [0.5, 0.9]), "{:?}", h.curve);
        assert!((h.at(h.curve.points()[1]) + off - start).length() < 1e-3);

        // Into a neighbour's MIN_GAP and back.
        let mut h = Harness::new(three_points());
        let start = h.at([0.5, 0.5]) + Vec2::new(-2.0, 3.0);
        let off = start - h.at([0.5, 0.5]);
        let right = start + Vec2::new(h.plot.width() * 0.8, 0.0);
        let mut path: Vec<Pos2> = (1..=5).map(|k| start.lerp(right, k as f32 / 5.0)).collect();
        path.extend((1..=5).map(|k| right.lerp(start, k as f32 / 5.0)));
        h.drag_path(start, &path);
        assert!(close(h.curve.points()[1], [0.5, 0.5]), "{:?}", h.curve);
        assert!((h.at(h.curve.points()[1]) + off - start).length() < 1e-3);
    }

    #[test]
    fn drag_endpoint_moves_only_y() {
        let mut h = Harness::new(three_points());
        assert!(h.drag(h.at([0.0, 0.0]), h.at([0.3, 0.4])));
        assert!(close(h.curve.points()[0], [0.0, 0.4]), "{:?}", h.curve);
        assert!(h.drag(h.at([1.0, 1.0]), h.at([0.7, 0.8])));
        assert!(close(h.curve.points()[2], [1.0, 0.8]), "{:?}", h.curve);
        assert_eq!(h.curve.points()[0][0], 0.0);
        assert_eq!(h.curve.points()[2][0], 1.0);
    }

    #[test]
    fn click_empty_adds_point_and_ninth_is_refused() {
        let mut h = Harness::new(PressureCurve::linear());
        assert!(h.click(h.at([0.3, 0.6])));
        assert_eq!(h.curve.points().len(), 3);
        assert!(close(h.curve.points()[1], [0.3, 0.6]), "{:?}", h.curve);
        // Clicking an existing point neither adds nor moves anything.
        h.time += 1.0;
        let before = h.curve;
        assert!(!h.click(h.at([0.3, 0.6])));
        assert_eq!(h.curve, before);
        // Add until full (8), then a ninth press does nothing.
        for x in [0.1, 0.2, 0.45, 0.6, 0.75] {
            // Waits out the double-click window so presses are independent.
            h.time += 1.0;
            assert!(h.click(h.at([x, 0.2])), "{x}");
        }
        assert_eq!(h.curve.points().len(), MAX_CURVE_POINTS);
        let full = h.curve;
        h.time += 1.0;
        assert!(!h.click(h.at([0.9, 0.5])));
        assert_eq!(h.curve, full);
    }

    #[test]
    fn double_click_removes_interior_point_not_endpoint() {
        let mut h = Harness::new(three_points());
        let mid = h.at([0.5, 0.5]);
        h.click(mid);
        assert!(h.click(mid), "second click of a double-click removes");
        assert_eq!(h.curve.points().len(), 2);
        // Endpoints survive a double-click.
        h.time += 1.0;
        let end = h.at([1.0, 1.0]);
        h.click(end);
        h.click(end);
        assert_eq!(h.curve, PressureCurve::linear());
        // Secondary click removes too.
        h.time += 1.0;
        h.curve = three_points();
        h.frame(vec![Event::PointerMoved(mid)]);
        h.button(mid, PointerButton::Secondary, true);
        assert!(h.button(mid, PointerButton::Secondary, false));
        assert_eq!(h.curve.points().len(), 2);
    }

    #[test]
    fn reset_restores_linear() {
        let mut h = Harness::new(PressureCurve::from_gamma(2.0));
        let reset = h.reset.expect("Reset button found").center();
        assert!(h.click(reset));
        assert_eq!(h.curve, PressureCurve::linear());
        // Already linear: no change reported.
        h.time += 1.0;
        assert!(!h.click(reset));
    }

    #[test]
    fn live_pressure_reads_touch_force() {
        let touch = |force| Event::Touch { device_id: TouchDeviceId(1), id: TouchId(1), phase: TouchPhase::Move, pos: pos2(5.0, 5.0), force };
        assert_eq!(live_pressure(&[]), None);
        assert_eq!(live_pressure(&[Event::PointerMoved(pos2(1.0, 1.0)), touch(None)]), None);
        let events = [touch(Some(0.3)), touch(Some(0.62)), Event::PointerMoved(pos2(1.0, 1.0)), touch(None)];
        assert_eq!(live_pressure(&events), Some(0.62));
        assert_eq!(live_pressure(&[touch(Some(1.7))]), Some(1.0));

        // The widget shows it for a second, then falls back to the hovered input.
        let mut h = Harness::new(PressureCurve::from_points(&[[0.0, 0.0], [0.5, 0.25], [1.0, 1.0]]));
        h.frame(vec![touch(Some(0.5))]);
        assert!(h.labels.iter().any(|l| l == "In 50% → Out 25%"), "{:?}", h.labels);
        h.frame(vec![]);
        assert!(h.labels.iter().any(|l| l == "In 50% → Out 25%"), "held: {:?}", h.labels);
        h.time += 1.5;
        h.frame(vec![]);
        assert!(h.labels.iter().any(|l| l == "In – → Out –"), "released: {:?}", h.labels);
    }

    #[test]
    fn to_from_screen_round_trip() {
        let rect = Rect::from_min_size(pos2(10.0, 20.0), Vec2::new(200.0, 200.0));
        for p in [[0.0, 0.0], [1.0, 1.0], [0.25, 0.75], [0.5, 0.1]] {
            let s = to_screen(rect, p);
            assert!(close(from_screen(rect, s), p));
        }
        assert_eq!(to_screen(rect, [0.0, 0.0]), rect.left_bottom());
        assert_eq!(to_screen(rect, [1.0, 1.0]), rect.right_top());
        // Outside the plot clamps.
        assert_eq!(from_screen(rect, pos2(-50.0, 900.0)), [0.0, 0.0]);
        assert_eq!(from_screen(rect, pos2(900.0, -50.0)), [1.0, 1.0]);
        // Picking uses the hit radius.
        let c = three_points();
        assert_eq!(pick(&c, rect, to_screen(rect, [0.5, 0.5]) + Vec2::new(5.0, 0.0)), Some(1));
        assert_eq!(pick(&c, rect, to_screen(rect, [0.5, 0.5]) + Vec2::new(8.0, 0.0)), None);
    }
}
