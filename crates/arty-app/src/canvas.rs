//! The canvas tab: pen/mouse input, view navigation and page display.
//!
//! Every pointer event of a frame is turned into a brush sample (the legacy
//! app sampled once per frame, which made fast lines jagged). Pen input
//! arrives as `Touch` events carrying pressure; when a frame has any, mouse
//! events are ignored for painting so the pen's simulated mouse doesn't
//! double-feed the stroke.

use arty_brush::InputSample;
use arty_render::{CanvasGpu, CanvasSync, View};
use egui::{Color32, CursorIcon, Event, PointerButton, Pos2, Rect, Sense, Shape, Stroke, TouchPhase, Vec2};

use crate::shell::Shell;
use crate::studio::{Studio, Tool};

#[derive(Clone, Copy, PartialEq)]
enum Nav {
    Pan,
    Rotate { start_angle: f32, start_rotation: f32 },
    Zoom { anchor: Pos2, start_x: f32, start_zoom: f32, moved: bool },
    Pick,
}

pub struct CanvasPane {
    pub gpu: Option<CanvasGpu>,
    pub render: Option<egui_wgpu::RenderState>,
    sync: CanvasSync,
    /// `Some(is_pen)` while a brush stroke is in progress.
    stroke: Option<bool>,
    nav: Option<Nav>,
    last_input_time: f64,
    events: Vec<Event>,
}

impl CanvasPane {
    pub fn new(render: Option<egui_wgpu::RenderState>) -> Self {
        let gpu = render.as_ref().map(|r| CanvasGpu::new(&r.device, r.target_format));
        Self { gpu, render, sync: CanvasSync::default(), stroke: None, nav: None, last_input_time: 0.0, events: Vec::new() }
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
        let stroke_count = self
            .events
            .iter()
            .filter(|e| matches!(e, Event::Touch { .. } | Event::PointerMoved(_) | Event::PointerButton { .. }))
            .count()
            .max(1);
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
                    Event::Touch { phase, pos, force, .. } => {
                        let t = sample_time();
                        let pressure = studio.shape_pressure(force.unwrap_or(1.0));
                        let [x, y] = to_doc(&studio.view, pos);
                        let s = InputSample { x, y, pressure, time: t, ..Default::default() };
                        match phase {
                            TouchPhase::Start if self.stroke.is_none() && self.nav.is_none() && rect.contains(pos) && hovered
                                && studio.begin_stroke(s) => {
                                    self.stroke = Some(true);
                                }
                            TouchPhase::Move if self.stroke == Some(true) => studio.feed_stroke(s),
                            TouchPhase::End | TouchPhase::Cancel if self.stroke == Some(true) => {
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
                                self.stroke = Some(false);
                            }
                        } else if !pressed && self.stroke == Some(false) {
                            studio.feed_stroke(s);
                            studio.end_stroke();
                            self.stroke = None;
                        }
                    }
                    Event::PointerMoved(pos) if !has_touch && self.stroke == Some(false) => {
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
            if self.stroke == Some(false) && !ui.input(|i| i.pointer.primary_down()) {
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
                Nav::Rotate { start_angle, start_rotation } => {
                    let mut r = *start_rotation + (angle_from(rect.center(), p) - *start_angle);
                    if ui.input(|i| i.modifiers.shift) {
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
