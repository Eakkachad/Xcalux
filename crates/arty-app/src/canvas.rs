//! The canvas tab: pen/mouse input, view navigation and page display.
//!
//! Every pointer event of a frame is turned into a brush sample (the legacy
//! app sampled once per frame, which made fast lines jagged). Pen input
//! arrives as `Touch` events carrying pressure; when a frame has any, mouse
//! events are ignored for painting so the pen's simulated mouse doesn't
//! double-feed the stroke.

use std::rc::Rc;

use arty_brush::InputSample;
use arty_pen::{PenQueue, PenStats};
use arty_render::{CanvasGpu, CanvasSync, View};
use egui::{Color32, CursorIcon, Event, PointerButton, Pos2, Rect, Sense, Shape, Stroke, TouchDeviceId, TouchId, TouchPhase, Vec2};

use crate::shell::Shell;
use crate::studio::{Studio, Tool};

#[derive(Clone, Copy, PartialEq)]
enum Nav {
    Pan,
    /// `temporary` = entered with the Shift+Space chord, where Shift is
    /// already held, so Ctrl snaps instead.
    Rotate { start_angle: f32, start_rotation: f32, temporary: bool },
    Zoom { anchor: Pos2, start_x: f32, start_zoom: f32, moved: bool },
    Pick,
}

/// Input that owns the brush stroke in progress.
#[derive(Clone, Copy, PartialEq)]
enum StrokeSrc {
    Mouse,
    /// Pen or finger contact; other contacts can't feed or end the stroke.
    Touch(TouchDeviceId, TouchId),
}

pub struct CanvasPane {
    pub gpu: Option<CanvasGpu>,
    pub render: Option<egui_wgpu::RenderState>,
    sync: CanvasSync,
    /// Set while a brush stroke is in progress.
    stroke: Option<StrokeSrc>,
    nav: Option<Nav>,
    last_input_time: f64,
    events: Vec<Event>,
    #[allow(dead_code)] // TRACK PEN
    pen: Option<Rc<PenQueue>>,
    pen_stats: PenStats,
}

impl CanvasPane {
    pub fn new(render: Option<egui_wgpu::RenderState>, pen: Option<Rc<PenQueue>>) -> Self {
        let gpu = render.as_ref().map(|r| CanvasGpu::new(&r.device, r.target_format));
        Self {
            gpu,
            render,
            sync: CanvasSync::default(),
            stroke: None,
            nav: None,
            last_input_time: 0.0,
            events: Vec::new(),
            pen,
            pen_stats: PenStats::default(),
        }
    }

    pub fn pen_stats(&self) -> PenStats {
        self.pen_stats
    }

    pub fn is_busy(&self) -> bool {
        self.stroke.is_some() || self.nav.is_some()
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, studio: &mut Studio, shell: &mut Shell) {
        let rect = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        let ppp = ui.ctx().pixels_per_point();
        let origin = [rect.center().x * ppp, rect.center().y * ppp];
        shell.canvas_center_px = origin;

        if studio.fit_pending && rect.width() > 10.0 {
            studio.view.fit(studio.doc.width() as f32, studio.doc.height() as f32, rect.width() * ppp, rect.height() * ppp);
            studio.fit_pending = false;
        }

        let tool = self.effective_tool(ui, studio.tool);
        self.handle_input(ui, &response, rect, ppp, origin, tool, studio, shell);

        // Upload whatever the input changed, then draw it this same frame.
        if let (Some(gpu), Some(render)) = (self.gpu.as_mut(), self.render.as_ref())
            && let Some(stats) = self.sync.sync(&mut studio.doc, gpu, &render.device, &render.queue) {
                shell.last_sync = stats;
            }
        self.paint(ui, rect, ppp, origin, studio, shell, tool, &response);
    }

    /// Tool after temporary modifiers (Space = hand, etc.).
    fn effective_tool(&self, ui: &egui::Ui, tool: Tool) -> Tool {
        if ui.ctx().egui_wants_keyboard_input() {
            return tool;
        }
        ui.input(|i| {
            let m = i.modifiers;
            if i.key_down(egui::Key::Space) {
                if m.shift {
                    Tool::Rotate
                } else if m.command {
                    Tool::Zoom
                } else {
                    Tool::Hand
                }
            } else if m.alt && matches!(tool, Tool::Brush(_)) {
                Tool::Eyedropper
            } else {
                tool
            }
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_input(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        rect: Rect,
        ppp: f32,
        origin: [f32; 2],
        tool: Tool,
        studio: &mut Studio,
        shell: &mut Shell,
    ) {
        let (now, frame_dt) = ui.input(|i| (i.time, i.stable_dt as f64));
        let to_doc = |view: &View, p: Pos2| view.screen_to_doc(origin).apply([p.x * ppp, p.y * ppp]);
        let hovered = response.hovered();
        shell.cursor_doc = response.hover_pos().map(|p| to_doc(&studio.view, p));

        self.events.clear();
        ui.input(|i| self.events.extend(i.events.iter().cloned()));
        let has_touch = self.events.iter().any(|e| matches!(e, Event::Touch { .. }));
        let stroke_count = sample_count(&self.events, has_touch, self.stroke == Some(StrokeSrc::Mouse));
        let t0 = if self.last_input_time > 0.0 { self.last_input_time.max(now - frame_dt.max(0.001)) } else { now };
        let mut k = 0usize;
        let mut sample_time = || {
            k += 1;
            t0 + (now - t0) * (k as f64 / stroke_count as f64)
        };

        // ----- brush strokes ------------------------------------------------
        if let Tool::Brush(_) = tool {
            let events = std::mem::take(&mut self.events);
            for e in &events {
                match *e {
                    Event::Touch { device_id, id, phase, pos, force } => {
                        let t = sample_time();
                        let pressure = studio.shape_pressure(force.unwrap_or(1.0));
                        let [x, y] = to_doc(&studio.view, pos);
                        let s = InputSample { x, y, pressure, time: t, ..Default::default() };
                        let owner = self.stroke == Some(StrokeSrc::Touch(device_id, id));
                        match phase {
                            TouchPhase::Start if self.stroke.is_none() && self.nav.is_none() && rect.contains(pos) && hovered
                                && studio.begin_stroke(s) => {
                                    self.stroke = Some(StrokeSrc::Touch(device_id, id));
                                }
                            TouchPhase::Move if owner => studio.feed_stroke(s),
                            TouchPhase::End | TouchPhase::Cancel if owner => {
                                studio.feed_stroke(InputSample { pressure: 0.0, ..s });
                                studio.end_stroke();
                                self.stroke = None;
                            }
                            _ => {}
                        }
                    }
                    Event::PointerButton { pos, button: PointerButton::Primary, pressed, .. } if !has_touch => {
                        let t = sample_time();
                        let [x, y] = to_doc(&studio.view, pos);
                        let p = studio.input.mouse_pressure;
                        let s = InputSample { x, y, pressure: p, time: t, ..Default::default() };
                        if pressed && self.stroke.is_none() && self.nav.is_none() && hovered {
                            if studio.begin_stroke(s) {
                                self.stroke = Some(StrokeSrc::Mouse);
                            }
                        } else if !pressed && self.stroke == Some(StrokeSrc::Mouse) {
                            studio.feed_stroke(s);
                            studio.end_stroke();
                            self.stroke = None;
                        }
                    }
                    Event::PointerMoved(pos) if !has_touch && self.stroke == Some(StrokeSrc::Mouse) => {
                        let t = sample_time();
                        let [x, y] = to_doc(&studio.view, pos);
                        let p = studio.input.mouse_pressure;
                        studio.feed_stroke(InputSample { x, y, pressure: p, time: t, ..Default::default() });
                    }
                    _ => {}
                }
            }
            self.events = events;
            // Release can be lost (e.g. focus change): never leave a stroke hanging.
            if self.stroke == Some(StrokeSrc::Mouse) && !ui.input(|i| i.pointer.primary_down()) {
                studio.end_stroke();
                self.stroke = None;
            }
        } else if self.stroke.is_some() {
            studio.end_stroke();
            self.stroke = None;
        }

        // ----- navigation ---------------------------------------------------
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let primary_pressed = response.drag_started_by(PointerButton::Primary) || response.clicked();
        if self.stroke.is_none() && primary_pressed && self.nav.is_none() {
            let press = response.interact_pointer_pos().unwrap_or(rect.center());
            self.nav = match tool {
                Tool::Hand => Some(Nav::Pan),
                Tool::Rotate => Some(Nav::Rotate {
                    start_angle: angle_from(rect.center(), press),
                    start_rotation: studio.view.rotation,
                    temporary: ui.input(|i| i.key_down(egui::Key::Space)),
                }),
                Tool::Zoom => Some(Nav::Zoom { anchor: press, start_x: press.x, start_zoom: studio.view.zoom, moved: false }),
                Tool::Eyedropper => Some(Nav::Pick),
                Tool::Brush(_) => None,
            };
        }
        if let (Some(nav), Some(p)) = (self.nav.as_mut(), pointer) {
            match nav {
                Nav::Pan => {
                    let d = response.drag_delta();
                    studio.view.pan(origin, [d.x * ppp, d.y * ppp]);
                }
                Nav::Rotate { start_angle, start_rotation, temporary } => {
                    let mut r = *start_rotation + (angle_from(rect.center(), p) - *start_angle);
                    if ui.input(|i| if *temporary { i.modifiers.command } else { i.modifiers.shift }) {
                        let step = 15f32.to_radians();
                        r = (r / step).round() * step;
                    }
                    studio.view.rotate_at(origin, origin, r);
                }
                Nav::Zoom { anchor, start_x, start_zoom, moved } => {
                    let dx = p.x - *start_x;
                    if dx.abs() > 3.0 {
                        *moved = true;
                    }
                    if *moved {
                        let z = *start_zoom * (dx * 0.01).exp();
                        studio.view.zoom_at(origin, [anchor.x * ppp, anchor.y * ppp], z);
                    }
                }
                Nav::Pick => {
                    let [x, y] = to_doc(&studio.view, p);
                    if let Some(c) = studio.sample_color(x, y) {
                        studio.set_main_color(c);
                    }
                }
            }
        }
        let primary_down = ui.input(|i| i.pointer.primary_down());
        if self.nav.is_some() && !primary_down {
            if let Some(Nav::Zoom { anchor, moved: false, .. }) = self.nav {
                let out = ui.input(|i| i.modifiers.alt);
                let z = studio.view.next_zoom_step(!out);
                studio.view.zoom_at(origin, [anchor.x * ppp, anchor.y * ppp], z);
            }
            self.nav = None;
        }

        // Middle-drag pans with any tool.
        if response.dragged_by(PointerButton::Middle) {
            let d = response.drag_delta();
            studio.view.pan(origin, [d.x * ppp, d.y * ppp]);
        }

        // Wheel: zoom at cursor; Alt+wheel rotates; pinch zooms.
        if hovered
            && let Some(p) = response.hover_pos() {
                let at = [p.x * ppp, p.y * ppp];
                for e in &self.events {
                    match *e {
                        Event::MouseWheel { unit, delta, modifiers, .. } => {
                            let dy = match unit {
                                egui::MouseWheelUnit::Point => delta.y,
                                egui::MouseWheelUnit::Line => delta.y * 40.0,
                                egui::MouseWheelUnit::Page => delta.y * rect.height(),
                            };
                            if dy == 0.0 {
                                continue;
                            }
                            if modifiers.alt {
                                let r = studio.view.rotation + dy.signum() * 5f32.to_radians();
                                studio.view.rotate_at(origin, at, r);
                            } else {
                                let z = studio.view.zoom * (dy * 0.003).exp();
                                studio.view.zoom_at(origin, at, z);
                            }
                        }
                        Event::Zoom(f) => {
                            let z = studio.view.zoom * f;
                            studio.view.zoom_at(origin, at, z);
                        }
                        _ => {}
                    }
                }
            }

        self.last_input_time = now;
        if self.stroke.is_some() || self.nav.is_some() {
            ui.ctx().request_repaint();
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint(
        &self,
        ui: &egui::Ui,
        rect: Rect,
        ppp: f32,
        origin: [f32; 2],
        studio: &Studio,
        shell: &Shell,
        tool: Tool,
        response: &egui::Response,
    ) {
        let pal = shell.theme.palette();
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, pal.workspace);

        let m = studio.view.doc_to_screen(origin);
        let (w, h) = (studio.doc.width() as f32, studio.doc.height() as f32);
        let corners: Vec<Pos2> = [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]]
            .iter()
            .map(|&p| {
                let s = m.apply(p);
                Pos2::new(s[0] / ppp, s[1] / ppp)
            })
            .collect();
        let shadow: Vec<Pos2> = corners.iter().map(|p| *p + Vec2::new(3.0, 4.0)).collect();
        painter.add(Shape::convex_polygon(shadow, pal.page_shadow, Stroke::NONE));

        match self.gpu.as_ref().and_then(|g| g.paint_callback(rect, m, 8.0)) {
            Some(cb) => {
                painter.add(cb);
            }
            None => {
                painter.add(Shape::convex_polygon(corners.clone(), Color32::WHITE, Stroke::NONE));
                painter.text(rect.center(), egui::Align2::CENTER_CENTER, "GPU canvas unavailable", egui::FontId::proportional(14.0), Color32::GRAY);
            }
        }
        painter.add(Shape::closed_line(corners, Stroke::new(1.0, Color32::from_black_alpha(70))));

        // Cursor.
        if let Some(hover) = response.hover_pos() {
            let ctx = ui.ctx();
            match tool {
                Tool::Brush(_) if self.nav.is_none() => {
                    ctx.set_cursor_icon(CursorIcon::None);
                    let r = studio.preset().size * 0.5 * studio.view.zoom / ppp;
                    if r > 2.0 {
                        painter.circle_stroke(hover, r, Stroke::new(1.0, Color32::from_black_alpha(160)));
                        painter.circle_stroke(hover, r + 1.0, Stroke::new(1.0, Color32::from_white_alpha(150)));
                    }
                    let c = 4.0;
                    for (a, b) in [(Vec2::new(-c, 0.0), Vec2::new(c, 0.0)), (Vec2::new(0.0, -c), Vec2::new(0.0, c))] {
                        painter.line_segment([hover + a, hover + b], Stroke::new(1.0, Color32::from_white_alpha(200)));
                    }
                    painter.circle_filled(hover, 0.8, Color32::BLACK);
                }
                Tool::Hand => ctx.set_cursor_icon(if self.nav.is_some() { CursorIcon::Grabbing } else { CursorIcon::Grab }),
                Tool::Zoom => ctx.set_cursor_icon(CursorIcon::ZoomIn),
                Tool::Rotate => ctx.set_cursor_icon(CursorIcon::AllScroll),
                Tool::Eyedropper => ctx.set_cursor_icon(CursorIcon::Crosshair),
                _ => {}
            }
        }
    }
}

fn angle_from(center: Pos2, p: Pos2) -> f32 {
    (p.y - center.y).atan2(p.x - center.x)
}

/// Number of events in a frame that become brush samples, so sample times
/// spread evenly over the frame. Mirrors the brush match arms: with pen input
/// only `Touch` counts (egui-winit adds a simulated pointer event per touch),
/// otherwise primary button events and moves while the mouse button is down.
fn sample_count(events: &[Event], has_touch: bool, mouse_stroking: bool) -> usize {
    let mut down = mouse_stroking;
    let mut n = 0;
    for e in events {
        match e {
            Event::Touch { .. } => n += 1,
            Event::PointerButton { button: PointerButton::Primary, pressed, .. } if !has_touch => {
                down = *pressed;
                n += 1;
            }
            Event::PointerMoved(_) if !has_touch && down => n += 1,
            _ => {}
        }
    }
    n.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeKind;
    use arty_core::{Document, TileCoord};
    use egui::{Key, Modifiers, RawInput, pos2};

    /// Headless egui frames driving a GPU-less canvas.
    struct Harness {
        ctx: egui::Context,
        pane: CanvasPane,
        studio: Studio,
        shell: Shell,
        time: f64,
    }

    impl Harness {
        fn new() -> Self {
            let mut h = Self {
                ctx: egui::Context::default(),
                pane: CanvasPane::new(None, None),
                studio: Studio::new(Document::new(512, 512, 72)),
                shell: Shell::new(ThemeKind::Dark),
                time: 0.0,
            };
            h.frame(vec![Event::PointerMoved(pos2(200.0, 150.0))]); // lay out and fit the page
            h
        }

        fn frame(&mut self, events: Vec<Event>) {
            self.time += 1.0 / 60.0;
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0))),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let Self { ctx, pane, studio, shell, .. } = self;
            ctx.run_ui(input, |ui| pane.ui(ui, studio, shell)).drop_without_applying_deltas();
        }

        fn screen(&self, doc: [f32; 2]) -> Pos2 {
            let [x, y] = self.studio.view.doc_to_screen(self.shell.canvas_center_px).apply(doc);
            pos2(x, y)
        }

        fn painted(&self) -> Vec<TileCoord> {
            self.studio.doc.active_layer().raster().unwrap().coords().collect()
        }
    }

    fn touch(id: u64, phase: TouchPhase, pos: Pos2) -> Event {
        Event::Touch { device_id: TouchDeviceId(7), id: TouchId(id), phase, pos, force: Some(0.6) }
    }

    fn primary(pos: Pos2, pressed: bool, modifiers: Modifiers) -> Event {
        Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers }
    }

    #[test]
    fn second_touch_neither_feeds_nor_ends_pen_stroke() {
        let mut h = Harness::new();
        let (pen, pen2, finger) = (h.screen([100.0, 100.0]), h.screen([120.0, 100.0]), h.screen([450.0, 450.0]));
        h.frame(vec![Event::PointerMoved(pen), touch(1, TouchPhase::Start, pen)]);
        assert!(h.studio.engine.is_stroking());
        h.frame(vec![
            touch(2, TouchPhase::Start, finger),
            touch(1, TouchPhase::Move, pen2),
            touch(2, TouchPhase::Move, finger + Vec2::new(4.0, 4.0)),
            touch(2, TouchPhase::End, finger + Vec2::new(4.0, 4.0)),
        ]);
        assert!(h.studio.engine.is_stroking(), "another contact's End ended the pen stroke");
        h.frame(vec![touch(1, TouchPhase::End, pen2)]);
        assert!(!h.studio.engine.is_stroking());
        let tiles = h.painted();
        assert!(tiles.contains(&TileCoord::from_pixel(110, 100)));
        assert!(tiles.iter().all(|c| c.x <= 2 && c.y <= 2), "another contact's Move was fed into the stroke: {tiles:?}");
    }

    #[test]
    fn sample_count_skips_simulated_and_idle_pointer_events() {
        let m = Modifiers::NONE;
        let p = pos2(1.0, 1.0);
        // egui-winit pairs every pen/touch event with a simulated pointer event.
        let pen = [
            Event::PointerMoved(p),
            touch(1, TouchPhase::Move, p),
            Event::PointerMoved(p),
            touch(1, TouchPhase::Move, p),
            primary(p, false, m),
            touch(1, TouchPhase::End, p),
        ];
        assert_eq!(sample_count(&pen, true, false), 3);
        // Moves only count while the mouse button is down.
        let mouse = [
            Event::PointerMoved(p),
            primary(p, true, m),
            Event::PointerMoved(p),
            Event::PointerMoved(p),
            primary(p, false, m),
            Event::PointerMoved(p),
        ];
        assert_eq!(sample_count(&mouse, false, false), 4);
        assert_eq!(sample_count(&[Event::PointerMoved(p)], false, true), 1);
        assert_eq!(sample_count(&[], false, false), 1);
    }

    /// Drags the view by `deg` around the canvas center, holding `mods`
    /// (and Space when `space`), and returns the resulting rotation.
    fn drag_rotate(tool: Tool, space: bool, mods: Modifiers, deg: f32) -> f32 {
        let mut h = Harness::new();
        h.studio.select_tool(tool);
        let c = pos2(h.shell.canvas_center_px[0], h.shell.canvas_center_px[1]);
        let a = c + Vec2::new(100.0, 0.0);
        let b = c + Vec2::angled(deg.to_radians()) * 100.0;
        let mut first = vec![Event::ModifiersChanged(mods)];
        if space {
            first.push(Event::Key { key: Key::Space, physical_key: None, pressed: true, repeat: false, modifiers: mods });
        }
        first.push(Event::PointerMoved(a));
        h.frame(first);
        h.frame(vec![primary(a, true, mods)]);
        // Leave the click distance so the drag starts at `a`, then rotate to `b`.
        h.frame(vec![Event::PointerMoved(a + Vec2::new(10.0, 0.0)), Event::PointerMoved(a)]);
        h.frame(vec![Event::PointerMoved(b)]);
        let r = h.studio.view.rotation.to_degrees();
        h.frame(vec![primary(b, false, mods)]);
        r
    }

    #[test]
    fn temporary_rotate_snaps_only_with_ctrl() {
        let pen = Tool::Brush(arty_brush::BrushGroup::Pen);
        let free = drag_rotate(pen, true, Modifiers::SHIFT, 10.0);
        assert!((free - 10.0).abs() < 0.5, "Shift+Space rotate snapped: {free}");
        let snapped = drag_rotate(pen, true, Modifiers::SHIFT | Modifiers::COMMAND, 10.0);
        assert!((snapped - 15.0).abs() < 0.01, "{snapped}");
        // The Rotate tool itself still snaps with Shift.
        let tool = drag_rotate(Tool::Rotate, false, Modifiers::SHIFT, 10.0);
        assert!((tool - 15.0).abs() < 0.01, "{tool}");
    }
}
