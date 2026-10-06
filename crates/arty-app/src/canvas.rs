//! The canvas tab: pen/mouse input, view navigation and page display.
//!
//! Every pointer event of a frame is turned into a brush sample (the legacy
//! app sampled once per frame, which made fast lines jagged). With the native
//! pen hook (`arty_pen`), pen strokes are fed from its queue: every Windows
//! Ink sample with its own pressure, tilt and OS timestamp, and egui's
//! `Touch` events for that pointer are ignored. Without it, pen input
//! arrives as `Touch` events carrying pressure. When a frame has any
//! `Touch`, mouse events are ignored for painting so the pen's simulated
//! mouse doesn't double-feed the stroke.

use std::rc::Rc;

use arty_brush::InputSample;
use arty_pen::{PenMeter, PenPhase, PenQueue, PenSample, PenStats};
use arty_render::{CanvasGpu, CanvasSync, View};
use egui::{Color32, CursorIcon, Event, PointerButton, Pos2, Rect, Sense, Shape, Stroke, TouchDeviceId, TouchId, TouchPhase, Vec2};

use crate::commands::{self, Command};
use crate::shell::Shell;
use crate::studio::{Studio, Tool};
use crate::text::{Key, t};
use crate::tools::{self, CanvasTool, ToolCtx, ToolInput, ToolStates};

/// Pen samples drained per frame without reallocating (~5 s at 200 Hz).
const PEN_BUF_CAP: usize = 1024;
/// A second press this soon and this close (points) is a double click (egui's values).
const DOUBLE_CLICK_SECS: f64 = 0.3;
const DOUBLE_CLICK_DIST: f32 = 6.0;

/// The tool state that takes canvas input for a page tool.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ToolSlot {
    Select,
    Fill,
    Transform,
    Frame,
}

/// The transform handles while a session runs, whatever the tool;
/// otherwise the page tool `tool` is, if any.
fn tool_slot(studio: &Studio, tool: Tool) -> Option<ToolSlot> {
    if studio.transform.is_some() {
        return Some(ToolSlot::Transform);
    }
    match tool {
        Tool::Select | Tool::MagicWand => Some(ToolSlot::Select),
        Tool::Fill => Some(ToolSlot::Fill),
        Tool::Move => Some(ToolSlot::Transform),
        Tool::Frame(_) => Some(ToolSlot::Frame),
        Tool::Brush(_) | Tool::Eyedropper | Tool::Hand | Tool::Rotate | Tool::Zoom => None,
    }
}

fn slot_tool(tools: &mut ToolStates, slot: ToolSlot) -> &mut dyn CanvasTool {
    match slot {
        ToolSlot::Select => &mut tools.select,
        ToolSlot::Fill => &mut tools.fill,
        ToolSlot::Transform => &mut tools.transform,
        ToolSlot::Frame => &mut tools.frame,
    }
}

fn slot_tool_ref(tools: &ToolStates, slot: ToolSlot) -> &dyn CanvasTool {
    match slot {
        ToolSlot::Select => &tools.select,
        ToolSlot::Fill => &tools.fill,
        ToolSlot::Transform => &tools.transform,
        ToolSlot::Frame => &tools.frame,
    }
}

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
    /// Finger, or pen without the native hook; other contacts can't feed or end the stroke.
    Touch(TouchDeviceId, TouchId),
    /// Native pen pointer id (egui's `TouchId` of the same contact).
    Pen(u32),
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
    /// Native pen queue; `None` off Windows or when the hook failed.
    pen: Option<Rc<PenQueue>>,
    /// This frame's pen samples (reused).
    pen_buf: Vec<PenSample>,
    pen_meter: PenMeter,
    pen_stats: PenStats,
    /// Selection, fill, transform and frame tool state.
    pub tools: ToolStates,
    /// The page tool, as of the last frame. Holding Space keeps it (and its
    /// gesture, e.g. a half-drawn polygon) while navigation takes the input.
    slot: Option<ToolSlot>,
    /// Space navigation is held: the page tool gets no input.
    space: bool,
    /// A Backspace that ended a polygon is still held: its key repeats are
    /// swallowed (and the global shortcuts wait) so they do not go on to
    /// Clear Layer.
    swallow_backspace: bool,
    /// The primary button went down on the canvas for the page tool.
    tool_down: bool,
    /// Time and position of the last page-tool press (double clicks).
    last_press: Option<(f64, Pos2)>,
    /// The window had focus last frame.
    focused: bool,
}

impl CanvasPane {
    pub fn new(render: Option<egui_wgpu::RenderState>, pen: Option<Rc<PenQueue>>) -> Self {
        let gpu = render.as_ref().map(|r| CanvasGpu::new(&r.device, r.target_format));
        let mut tools = ToolStates::default();
        if let Some(r) = &render {
            tools.select.attach_gpu(r);
        }
        Self {
            gpu,
            render,
            sync: CanvasSync::default(),
            stroke: None,
            nav: None,
            last_input_time: 0.0,
            events: Vec::new(),
            pen,
            pen_buf: Vec::with_capacity(PEN_BUF_CAP),
            pen_meter: PenMeter::new(),
            pen_stats: PenStats::default(),
            tools,
            slot: None,
            space: false,
            swallow_backspace: false,
            tool_down: false,
            last_press: None,
            focused: true,
        }
    }

    pub fn pen_stats(&self) -> PenStats {
        self.pen_stats
    }

    /// A stroke, a navigation drag or a page tool gesture is in progress
    /// (global shortcuts wait).
    pub fn is_busy(&self) -> bool {
        self.stroke.is_some()
            || self.nav.is_some()
            || self.swallow_backspace
            || self.slot.is_some_and(|s| slot_tool_ref(&self.tools, s).gesture_active())
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

        self.pen_buf.clear();
        if let Some(q) = &self.pen {
            q.set_enabled(studio.input.native_pen);
            q.drain_into(&mut self.pen_buf);
            if !studio.input.native_pen {
                self.pen_buf.clear();
            }
            self.pen_stats =
                self.pen_meter.update(&self.pen_buf, arty_pen::now_secs(), q.dropped(), studio.input.native_pen);
        }
        // Eraser end (CSP): decided before the tool is computed, never mid-stroke.
        if studio.input.eraser_end_switch && studio.input.native_pen {
            if self.stroke.is_none()
                && let Some(end) =
                    self.pen_buf.iter().find(|s| s.phase == PenPhase::Down).or(self.pen_buf.last()).map(|s| s.end)
                && end != studio.pen_end()
            {
                commands::execute(Command::PenEnd(end), studio, shell);
            }
        } else {
            // Ends are not tracked while switching is off: the current tool is the tip's.
            studio.reset_pen_end();
        }

        if self.swallow_backspace {
            let _ = ui.ctx().input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace));
            if !ui.input(|i| i.key_down(egui::Key::Backspace)) {
                self.swallow_backspace = false;
            }
        }

        // Space navigation keeps priority; otherwise a transform session
        // takes the input whatever the tool (so Alt is not the eyedropper).
        let space = !ui.ctx().egui_wants_keyboard_input() && ui.input(|i| i.key_down(egui::Key::Space));
        let tool = if studio.transform.is_some() && !space { studio.tool } else { self.effective_tool(ui, studio.tool) };
        // Space only suspends the page tool's input; its gesture stays.
        let slot = tool_slot(studio, if space { studio.tool } else { tool });
        let focused = ui.input(|i| i.focused);
        if slot != self.slot || (self.focused && !focused) {
            // Tool switch or focus loss: the old gesture is abandoned.
            if let Some(old) = self.slot {
                slot_tool(&mut self.tools, old).cancel(&mut ToolCtx { studio, shell, origin, ppp });
            }
            self.tool_down = false;
        }
        self.slot = slot;
        self.space = space;
        self.focused = focused;
        self.handle_input(ui, &response, rect, ppp, origin, tool, studio, shell);

        // Upload whatever the input changed, then draw it this same frame.
        if let (Some(gpu), Some(render)) = (self.gpu.as_mut(), self.render.as_ref())
            && let Some(stats) = self.sync.sync(&mut studio.doc, gpu, &render.device, &render.queue, &mut studio.overview) {
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
        // egui un-hovers every widget for a frame that releases and presses
        // again, which is what one pen contact's Up and the next one's Down in
        // the same frame look like. A pen contact may still start when the
        // canvas is under the pointer, uncovered, and nothing else is dragged.
        let pen_hovered = hovered
            || (response.contains_pointer() && ui.ctx().dragged_id().is_none_or(|id| id == response.id));
        shell.cursor_doc = response.hover_pos().map(|p| to_doc(&studio.view, p));

        self.events.clear();
        ui.input(|i| self.events.extend(i.events.iter().cloned()));
        let has_touch = self.events.iter().any(|e| matches!(e, Event::Touch { .. }));
        // Touch events of contacts the native pen reports are the same samples again.
        let pen_buf = std::mem::take(&mut self.pen_buf);
        let stroke = self.stroke;
        let stroke_count = sample_count(&self.events, has_touch, stroke == Some(StrokeSrc::Mouse), |id| {
            pen_owns(&pen_buf, stroke, id)
        });
        let t0 = if self.last_input_time > 0.0 { self.last_input_time.max(now - frame_dt.max(0.001)) } else { now };
        let mut k = 0usize;
        let mut sample_time = || {
            k += 1;
            t0 + (now - t0) * (k as f64 / stroke_count as f64)
        };

        // ----- brush strokes ------------------------------------------------
        if self.slot.is_none() && matches!(tool, Tool::Brush(_)) {
            // Native pen first: each sample keeps its own pressure, tilt and OS time.
            let m = studio.view.screen_to_doc(origin);
            for s in &pen_buf {
                let pts = Pos2::new(s.pos[0] / ppp, s.pos[1] / ppp);
                let [x, y] = m.apply(s.pos);
                let pressure = match s.pressure {
                    Some(p) => studio.shape_pressure(p),
                    None => studio.input.mouse_pressure,
                };
                let [tilt_x, tilt_y] = arty_pen::view_tilt([m.a, m.b, m.c, m.d], s.tilt);
                let smp = InputSample { x, y, pressure, tilt_x, tilt_y, time: s.time };
                let owner = self.stroke == Some(StrokeSrc::Pen(s.pointer));
                // The pen was flipped between two contacts of this frame (the
                // check in `ui` only sees the frame's first): switch before this
                // contact starts, then start it only if that end's tool paints.
                if s.phase == PenPhase::Down
                    && self.stroke.is_none()
                    && studio.input.eraser_end_switch
                    && s.end != studio.pen_end()
                {
                    commands::execute(Command::PenEnd(s.end), studio, shell);
                }
                match s.phase {
                    PenPhase::Down
                        if self.stroke.is_none()
                            && self.nav.is_none()
                            && rect.contains(pts)
                            && pen_hovered
                            && matches!(self.effective_tool(ui, studio.tool), Tool::Brush(_))
                            && studio.begin_stroke(smp) =>
                    {
                        self.stroke = Some(StrokeSrc::Pen(s.pointer));
                    }
                    PenPhase::Move if owner => studio.feed_stroke(smp),
                    PenPhase::Up | PenPhase::Cancel | PenPhase::Leave if owner => {
                        studio.feed_stroke(InputSample { pressure: 0.0, ..smp });
                        studio.end_stroke();
                        self.stroke = None;
                    }
                    _ => {}
                }
            }

            let events = std::mem::take(&mut self.events);
            for e in &events {
                match *e {
                    Event::Touch { device_id, id, phase, pos, force } if !pen_owns(&pen_buf, stroke, id) => {
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
            // Likewise when egui saw the pen lift but the native queue did not.
            if let Some(StrokeSrc::Pen(p)) = self.stroke
                && touch_lifted(&self.events, TouchId(u64::from(p)))
            {
                studio.end_stroke();
                self.stroke = None;
            }
        } else if self.stroke.is_some() {
            studio.end_stroke();
            self.stroke = None;
        }
        self.pen_buf = pen_buf;

        // ----- page tools (selection, fill, transform, frames) -----------------
        if let Some(slot) = self.slot {
            let m = studio.view.screen_to_doc(origin);
            let input = |p: Pos2, mods: egui::Modifiers, double: bool| {
                let screen = [p.x * ppp, p.y * ppp];
                ToolInput { screen, doc: m.apply(screen), mods, double }
            };
            let (mods_now, primary_down, latest) = ui.input(|i| (i.modifiers, i.pointer.primary_down(), i.pointer.latest_pos()));
            let t = slot_tool(&mut self.tools, slot);
            let mut ctx = ToolCtx { studio: &mut *studio, shell: &mut *shell, origin, ppp };
            // While Space is held, navigation takes the pointer and the keys.
            let space = self.space;
            for e in self.events.iter().filter(|_| !space) {
                match *e {
                    Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers }
                        if !self.tool_down && self.nav.is_none() && hovered && rect.contains(pos) =>
                    {
                        let double = self
                            .last_press
                            .is_some_and(|(at, p)| now - at <= DOUBLE_CLICK_SECS && p.distance(pos) <= DOUBLE_CLICK_DIST);
                        // A third press starts a new pair.
                        self.last_press = (!double).then_some((now, pos));
                        self.tool_down = true;
                        t.press(&mut ctx, input(pos, modifiers, double));
                    }
                    Event::PointerMoved(pos) if self.tool_down => t.drag(&mut ctx, input(pos, mods_now, false)),
                    Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers } if self.tool_down => {
                        self.tool_down = false;
                        t.release(&mut ctx, input(pos, modifiers, false));
                    }
                    _ => {}
                }
            }
            // Release can be lost (e.g. focus change): never leave a gesture
            // pressed. Space ends a drag the same way (as it ends a brush
            // stroke); a multi-click gesture, such as a polygon, waits.
            if self.tool_down && (!primary_down || space) {
                self.tool_down = false;
                t.release(&mut ctx, input(latest.unwrap_or(rect.center()), mods_now, false));
            }
            if !self.tool_down
                && !space
                && let Some(p) = response.hover_pos()
            {
                t.hover(ctx.studio, input(p, mods_now, false));
            }
            // Enter, Esc and Backspace belong to the gesture while it runs,
            // unless a text field or a modal dialog has the keyboard.
            let keys_free = !ui.ctx().egui_wants_keyboard_input() && ui.ctx().memory(|m| m.top_modal_layer().is_none());
            if !space && keys_free && t.gesture_active() {
                for key in [egui::Key::Enter, egui::Key::Escape, egui::Key::Backspace] {
                    if !ui.ctx().input_mut(|i| i.consume_key(egui::Modifiers::NONE, key)) {
                        continue;
                    }
                    if t.key(&mut ctx, key) {
                        // Popped the last vertex: the held key's repeats
                        // must not go on to Clear Layer.
                        if key == egui::Key::Backspace && !t.gesture_active() {
                            self.swallow_backspace = true;
                        }
                    } else if key == egui::Key::Escape {
                        t.cancel(&mut ctx);
                    }
                }
            }
            t.tick(&mut ctx, now);
        }

        // ----- navigation ---------------------------------------------------
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let primary_pressed = response.drag_started_by(PointerButton::Primary) || response.clicked();
        if self.stroke.is_none() && primary_pressed && self.nav.is_none() && (self.slot.is_none() || self.space) {
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
                Tool::Brush(_) | Tool::Select | Tool::MagicWand | Tool::Fill | Tool::Move | Tool::Frame(_) => None,
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
        if self.stroke.is_some() || self.nav.is_some() || self.tool_down {
            ui.ctx().request_repaint();
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint(
        &mut self,
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
                painter.text(rect.center(), egui::Align2::CENTER_CENTER, t(Key::GpuCanvasUnavailable), egui::FontId::proportional(14.0), Color32::GRAY);
            }
        }
        painter.add(Shape::closed_line(corners, Stroke::new(1.0, Color32::from_black_alpha(70))));

        // Overlays: page guides, the selection outline, then the page tool's own.
        tools::page::paint_guides(&painter, studio, &studio.opts.page, origin, ppp);
        tools::select::paint_ants(&mut self.tools.select, &painter, studio, origin, ppp);
        if let Some(slot) = self.slot {
            slot_tool_ref(&self.tools, slot).paint(studio, &painter, origin, ppp);
        }

        // Cursor.
        if let Some(hover) = response.hover_pos() {
            let ctx = ui.ctx();
            if let Some(slot) = self.slot.filter(|_| self.nav.is_none() && !self.space) {
                ctx.set_cursor_icon(slot_tool_ref(&self.tools, slot).cursor(studio));
                return;
            }
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

/// Whether `Touch` events of contact `id` belong to the native pen: it
/// reported that pointer this frame or owns the stroke in progress.
fn pen_owns(pen_buf: &[PenSample], stroke: Option<StrokeSrc>, id: TouchId) -> bool {
    pen_buf.iter().any(|s| u64::from(s.pointer) == id.0) || matches!(stroke, Some(StrokeSrc::Pen(p)) if u64::from(p) == id.0)
}

/// Whether this frame's egui events end contact `id` for good: an
/// `End`/`Cancel` that no later `Start` of the same id follows.
///
/// Windows keeps a pen's pointer id across contacts, so a frame can hold the
/// previous contact's `End` before the `Start` of the stroke now in progress.
/// That `End` mirrors a native `Up` the queue already applied; only an end
/// after the last start can be one the native queue missed. (A native
/// `Up`/`Cancel`/`Leave` after the stroke's own `Down` would have ended it, so
/// a pen stroke still running here never had one.)
fn touch_lifted(events: &[Event], id: TouchId) -> bool {
    events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::Touch { id: i, phase, .. } if *i == id => match phase {
                TouchPhase::End | TouchPhase::Cancel => Some(true),
                TouchPhase::Start => Some(false),
                TouchPhase::Move => None,
            },
            _ => None,
        })
        .unwrap_or(false)
}

/// Number of events in a frame that become brush samples, so sample times
/// spread evenly over the frame. Mirrors the brush match arms: with pen input
/// only `Touch` counts (egui-winit adds a simulated pointer event per touch),
/// otherwise primary button events and moves while the mouse button is down.
/// Touch events of contacts the native pen owns (`pen_owns`) are not samples.
fn sample_count(events: &[Event], has_touch: bool, mouse_stroking: bool, pen_owns: impl Fn(TouchId) -> bool) -> usize {
    let mut down = mouse_stroking;
    let mut n = 0;
    for e in events {
        match e {
            Event::Touch { id, .. } if !pen_owns(*id) => n += 1,
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
    use arty_brush::pressure::PressureCurve;
    use arty_brush::{BrushGroup, Reshape};
    use arty_core::{Document, TileCoord, tile::TilePixels};
    use arty_pen::PenEnd;
    use egui::{Key, Modifiers, RawInput, pos2};

    /// Headless egui frames driving a GPU-less canvas.
    struct Harness {
        ctx: egui::Context,
        pane: CanvasPane,
        studio: Studio,
        shell: Shell,
        time: f64,
        /// The native pen queue the pane drains (`with_pen`).
        pen: Option<Rc<PenQueue>>,
        pen_time: f64,
    }

    /// Pointer id of the test pen (= egui `TouchId` of its contact).
    const PEN: u32 = 5;

    impl Harness {
        fn new() -> Self {
            Self::build(None)
        }

        /// A canvas with a native pen queue, as `arty_pen::install` gives on Windows.
        fn with_pen() -> Self {
            Self::build(Some(Rc::new(PenQueue::new(1024))))
        }

        fn build(pen: Option<Rc<PenQueue>>) -> Self {
            let mut h = Self {
                ctx: egui::Context::default(),
                pane: CanvasPane::new(None, pen.clone()),
                studio: Studio::new(Document::new(512, 512, 72)),
                shell: Shell::new(ThemeKind::Dark),
                time: 0.0,
                pen,
                pen_time: 100.0,
            };
            h.frame(vec![Event::PointerMoved(pos2(200.0, 150.0))]); // lay out and fit the page
            h
        }

        /// A native pen sample at document point `doc`, 4 ms after the previous one.
        fn pen(&mut self, phase: PenPhase, doc: [f32; 2], pressure: f32) -> PenSample {
            self.pen_time += 0.004;
            let p = self.screen(doc);
            PenSample { pointer: PEN, phase, pos: [p.x, p.y], pressure: Some(pressure), time: self.pen_time, ..Default::default() }
        }

        /// Queue `samples` as the window proc would, then run a frame with
        /// `events` (what egui-winit made of the same messages).
        fn pen_frame(&mut self, samples: &[PenSample], events: Vec<Event>) {
            let q = self.pen.as_ref().expect("Harness::with_pen");
            for s in samples {
                q.push(*s);
            }
            self.frame(events);
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

        /// A frame as the app runs it: the global shortcuts first, unless
        /// the canvas is busy, then the canvas.
        fn app_frame(&mut self, events: Vec<Event>, modal: bool) {
            self.time += 1.0 / 60.0;
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0))),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let Self { ctx, pane, studio, shell, .. } = self;
            ctx.run_ui(input, |ui| {
                if !pane.is_busy() {
                    commands::handle_shortcuts(ui.ctx(), studio, shell);
                }
                pane.ui(ui, studio, shell);
                if modal {
                    egui::Modal::new(egui::Id::new("test-modal")).show(ui.ctx(), |ui| ui.label("modal"));
                }
            })
            .drop_without_applying_deltas();
        }

        /// Click at document point `doc` with the page tool.
        fn click(&mut self, doc: [f32; 2]) {
            let p = self.screen(doc);
            self.frame(vec![Event::PointerMoved(p), primary(p, true, Modifiers::NONE)]);
            self.frame(vec![primary(p, false, Modifiers::NONE)]);
        }

        /// A polygon selection with `pts` placed, not closed.
        fn polygon(&mut self, pts: &[[f32; 2]]) {
            self.studio.opts.select.shape = tools::select::SelShape::Polygon;
            commands::execute(Command::SelectTool(Tool::Select), &mut self.studio, &mut self.shell);
            for &p in pts {
                self.click(p);
            }
            assert_eq!(self.pane.tools.select.polygon_len(), Some(pts.len()));
        }

        fn screen(&self, doc: [f32; 2]) -> Pos2 {
            let [x, y] = self.studio.view.doc_to_screen(self.shell.canvas_center_px).apply(doc);
            pos2(x, y)
        }

        fn painted(&self) -> Vec<TileCoord> {
            self.studio.doc.active_layer().raster().unwrap().coords().collect()
        }

        /// Every painted tile with its pixels, in a stable order.
        fn tiles(&self) -> Vec<(i32, i32, TilePixels)> {
            let mut v: Vec<_> =
                self.studio.doc.active_layer().raster().unwrap().iter().map(|(c, t)| (c.y, c.x, **t)).collect();
            v.sort_by_key(|t| (t.0, t.1));
            v
        }

        fn alpha(&self, x: i32, y: i32) -> u16 {
            let c = TileCoord::from_pixel(x, y);
            let (ox, oy) = c.origin();
            let grid = self.studio.doc.active_layer().raster().unwrap();
            grid.get(c).map_or(0, |t| t[(y - oy) as usize][(x - ox) as usize][3])
        }

        /// Total alpha on the active layer.
        fn ink(&self) -> u64 {
            self.tiles().iter().flat_map(|t| t.2.iter().flatten()).map(|p| u64::from(p[3])).sum()
        }

        /// Rows of document column `x` that hold ink.
        fn column_height(&self, x: i32) -> usize {
            (0..512).filter(|&y| self.alpha(x, y) > 0).count()
        }
    }

    /// What egui-winit emits for the same pointer messages: a `Touch` per
    /// sample (force 0.6, unlike the native samples) plus the simulated mouse.
    fn mirrored(samples: &[PenSample]) -> Vec<Event> {
        let mut ev = Vec::new();
        for s in samples {
            let pos = pos2(s.pos[0], s.pos[1]);
            let t = |phase| touch(u64::from(s.pointer), phase, pos);
            match s.phase {
                PenPhase::Down => ev.extend([t(TouchPhase::Start), Event::PointerMoved(pos), primary(pos, true, Modifiers::NONE)]),
                PenPhase::Move | PenPhase::Hover => ev.extend([t(TouchPhase::Move), Event::PointerMoved(pos)]),
                PenPhase::Up => ev.extend([t(TouchPhase::End), primary(pos, false, Modifiers::NONE), Event::PointerGone]),
                PenPhase::Cancel => ev.extend([t(TouchPhase::Cancel), Event::PointerGone]),
                PenPhase::Leave => {}
            }
        }
        ev
    }

    /// Down, `n` moves along a line and Up, ramping pressure from `p0` to `p1`.
    fn pen_line(h: &mut Harness, from: [f32; 2], to: [f32; 2], n: usize, p0: f32, p1: f32) -> Vec<PenSample> {
        (0..n + 2)
            .map(|i| {
                let f = i as f32 / (n + 1) as f32;
                let phase = if i == 0 {
                    PenPhase::Down
                } else if i == n + 1 {
                    PenPhase::Up
                } else {
                    PenPhase::Move
                };
                let at = [from[0] + (to[0] - from[0]) * f, from[1] + (to[1] - from[1]) * f];
                h.pen(phase, at, p0 + (p1 - p0) * f)
            })
            .collect()
    }

    fn touch(id: u64, phase: TouchPhase, pos: Pos2) -> Event {
        Event::Touch { device_id: TouchDeviceId(7), id: TouchId(id), phase, pos, force: Some(0.6) }
    }

    fn primary(pos: Pos2, pressed: bool, modifiers: Modifiers) -> Event {
        Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers }
    }

    /// A drag with a page tool goes to that tool: no brush stroke, no
    /// navigation, and a release ends it.
    #[test]
    fn page_tools_take_pointer_input() {
        use crate::studio::FrameMode;
        for tool in [
            Tool::Select,
            Tool::MagicWand,
            Tool::Fill,
            Tool::Move,
            Tool::Frame(FrameMode::Rect),
            Tool::Frame(FrameMode::Cut),
            Tool::Frame(FrameMode::Edit),
        ] {
            let mut h = Harness::new();
            commands::execute(Command::SelectTool(tool), &mut h.studio, &mut h.shell);
            let view = (h.studio.view.zoom, h.studio.view.rotation);
            let (a, b) = (h.screen([100.0, 100.0]), h.screen([300.0, 260.0]));
            h.frame(vec![Event::PointerMoved(a)]);
            h.frame(vec![primary(a, true, Modifiers::NONE)]);
            assert!(h.pane.tool_down, "{tool:?}: the press reached the tool");
            h.frame(vec![Event::PointerMoved(a + Vec2::new(10.0, 0.0)), Event::PointerMoved(b)]);
            assert!(!h.studio.engine.is_stroking(), "{tool:?} started a brush stroke");
            assert!(h.pane.nav.is_none(), "{tool:?} started navigation");
            h.frame(vec![primary(b, false, Modifiers::NONE)]);
            assert!(!h.pane.tool_down);
            assert_eq!((h.studio.view.zoom, h.studio.view.rotation), view);
            assert!(!h.pane.is_busy());
        }
        // Brushes still paint.
        let mut h = Harness::new();
        let (a, b) = (h.screen([100.0, 100.0]), h.screen([300.0, 260.0]));
        h.frame(vec![Event::PointerMoved(a)]);
        h.frame(vec![primary(a, true, Modifiers::NONE)]);
        h.frame(vec![Event::PointerMoved(b)]);
        assert!(h.studio.engine.is_stroking());
        assert!(!h.pane.tool_down);
        h.frame(vec![primary(b, false, Modifiers::NONE)]);
        assert!(!h.painted().is_empty());
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
        assert_eq!(sample_count(&pen, true, false, |_| false), 3);
        assert_eq!(sample_count(&pen, true, false, |id| id == TouchId(1)), 1, "native pen samples itself");
        // Moves only count while the mouse button is down.
        let mouse = [
            Event::PointerMoved(p),
            primary(p, true, m),
            Event::PointerMoved(p),
            Event::PointerMoved(p),
            primary(p, false, m),
            Event::PointerMoved(p),
        ];
        assert_eq!(sample_count(&mouse, false, false, |_| false), 4);
        assert_eq!(sample_count(&[Event::PointerMoved(p)], false, true, |_| false), 1);
        assert_eq!(sample_count(&[], false, false, |_| false), 1);
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

    fn key(key: Key, pressed: bool, repeat: bool) -> Event {
        Event::Key { key, physical_key: None, pressed, repeat, modifiers: Modifiers::NONE }
    }

    /// Holding Space to pan keeps an unfinished polygon: navigation takes
    /// the pointer, and the polygon goes on after Space is released.
    #[test]
    fn space_pan_keeps_the_polygon() {
        let mut h = Harness::new();
        h.polygon(&[[100.0, 100.0], [300.0, 120.0], [250.0, 300.0]]);
        let zoom = h.studio.view.zoom;
        let (a, b) = (h.screen([200.0, 400.0]), h.screen([260.0, 430.0]));
        h.frame(vec![key(Key::Space, true, false), Event::PointerMoved(a)]);
        assert!(h.pane.space && h.pane.slot.is_some(), "the tool stays, without input");
        h.frame(vec![primary(a, true, Modifiers::NONE)]);
        h.frame(vec![Event::PointerMoved(a + Vec2::new(10.0, 0.0)), Event::PointerMoved(b)]);
        assert!(h.pane.nav.is_some(), "Space pans");
        h.frame(vec![primary(b, false, Modifiers::NONE)]);
        h.frame(vec![key(Key::Space, false, false)]);
        assert_eq!(h.pane.tools.select.polygon_len(), Some(3), "the vertices survived the pan");
        assert_eq!(h.studio.view.zoom, zoom);
        h.click([120.0, 280.0]);
        assert_eq!(h.pane.tools.select.polygon_len(), Some(4), "and the polygon goes on");
    }

    /// While Space is held, a transform session keeps its handles and
    /// preview refresh (the overlay is the tool's).
    #[test]
    fn space_keeps_the_transform_overlay() {
        let mut h = Harness::new();
        let id = h.studio.doc.active();
        h.studio.doc.paint_target(id).unwrap().0.get_mut_or_create(TileCoord::new(1, 1))[3][3] = [0, 0, 0, 0x8000];
        commands::execute(Command::Transform, &mut h.studio, &mut h.shell);
        h.frame(vec![]);
        assert_eq!(h.pane.slot, Some(ToolSlot::Transform));
        h.frame(vec![key(Key::Space, true, false)]);
        assert_eq!(h.pane.slot, Some(ToolSlot::Transform), "drawn and ticked during the pan");
        assert!(h.studio.transform.is_some());
        h.frame(vec![key(Key::Space, false, false)]);
        assert!(h.studio.transform.is_some());
    }

    /// Holding Backspace to pop polygon vertices stops at the polygon: the
    /// key's repeats do not go on to Clear Layer.
    #[test]
    fn held_backspace_stops_at_the_polygon() {
        let mut h = Harness::new();
        let id = h.studio.doc.active();
        h.studio.doc.paint_target(id).unwrap().0.get_mut_or_create(TileCoord::new(0, 0))[0][0] = [0, 0, 0, 0x8000];
        h.polygon(&[[100.0, 100.0]]);
        let steps = h.studio.history.undo_len();
        h.app_frame(vec![key(Key::Backspace, true, false)], false);
        assert_eq!(h.pane.tools.select.polygon_len(), None);
        for _ in 0..3 {
            h.app_frame(vec![key(Key::Backspace, true, true)], false);
        }
        assert!(!h.painted().is_empty(), "the layer was not cleared");
        assert_eq!(h.studio.history.undo_len(), steps);
        // Once the key is up, Backspace clears the layer again.
        h.app_frame(vec![key(Key::Backspace, false, false)], false);
        h.app_frame(vec![key(Key::Backspace, true, false)], false);
        assert!(h.painted().is_empty());
    }

    /// With a modal open, Enter, Esc and Backspace are the dialog's, not
    /// the hidden polygon's.
    #[test]
    fn a_modal_keeps_its_keys_from_the_polygon() {
        let mut h = Harness::new();
        h.polygon(&[[100.0, 100.0], [300.0, 120.0], [250.0, 300.0]]);
        h.app_frame(vec![], true);
        for k in [Key::Backspace, Key::Escape, Key::Enter] {
            h.app_frame(vec![key(k, true, false), key(k, false, false)], true);
            assert_eq!(h.pane.tools.select.polygon_len(), Some(3), "{k:?} reached the polygon");
        }
        assert!(!h.studio.doc.has_selection());
        h.app_frame(vec![], false);
        h.app_frame(vec![key(Key::Enter, true, false)], false);
        assert_eq!(h.pane.tools.select.polygon_len(), None, "closed once the modal is gone");
        assert!(h.studio.doc.has_selection());
    }

    /// P17: the native samples feed the stroke; egui's Touch events for the
    /// same contact add nothing (a pen-only run paints identical pixels).
    #[test]
    fn native_pen_feeds_once() {
        // `split`: the stroke spans two frames (Down + 6 moves, then 7 moves + Up)
        // or one, where the native Up ends it before the Touch Start is seen.
        let run = |mirror: bool, split: usize| {
            let mut h = Harness::with_pen();
            let line = pen_line(&mut h, [100.0, 100.0], [380.0, 400.0], 13, 1.0, 1.0);
            for batch in [&line[..split], &line[split..]] {
                let events = if mirror {
                    mirrored(batch)
                } else {
                    batch.iter().map(|s| Event::PointerMoved(pos2(s.pos[0], s.pos[1]))).collect()
                };
                h.pen_frame(batch, events);
            }
            assert!(!h.studio.engine.is_stroking());
            assert_eq!(h.studio.history.undo_len(), 1);
            h.tiles()
        };
        let pen_only = run(false, 7);
        assert!(!pen_only.is_empty());
        for split in [7, 15] {
            assert!(run(true, split) == pen_only, "mirrored Touch events were fed (split {split})");
        }
    }

    /// P18
    #[test]
    fn pen_down_outside_rect_or_unhovered_does_not_start() {
        // Outside the canvas rect (the screen is 400×300 points).
        let mut h = Harness::with_pen();
        let outside =
            PenSample { pointer: PEN, phase: PenPhase::Down, pos: [450.0, 100.0], pressure: Some(1.0), time: 1.0, ..Default::default() };
        h.pen_frame(&[outside], vec![]);
        assert!(!h.studio.engine.is_stroking());
        let m = h.pen(PenPhase::Move, [200.0, 200.0], 1.0);
        h.pen_frame(&[m], vec![Event::PointerMoved(pos2(m.pos[0], m.pos[1]))]);
        assert!(h.painted().is_empty(), "a stroke that never started was fed");

        // Inside, but the canvas is not hovered (e.g. a floating panel covers it).
        let mut h = Harness::with_pen();
        h.frame(vec![Event::PointerGone]);
        let down = h.pen(PenPhase::Down, [200.0, 200.0], 1.0);
        h.pen_frame(&[down], vec![]);
        assert!(!h.studio.engine.is_stroking());
        assert!(h.painted().is_empty());
    }

    /// P19: per-sample pressure inside one frame (winit gives every coalesced
    /// entry the newest pressure, which made the whole frame one width).
    #[test]
    fn per_sample_pressure_reaches_engine() {
        let mut h = Harness::with_pen();
        h.studio.preset_mut().stabilizer = 0;
        let line = pen_line(&mut h, [100.0, 250.0], [400.0, 250.0], 30, 0.1, 1.0);
        h.pen_frame(&line, mirrored(&line));
        assert!(!h.studio.engine.is_stroking());
        let (thin, wide) = (h.column_height(130), h.column_height(370));
        assert!(thin >= 1 && wide >= thin + 3, "width did not follow pressure: {thin} → {wide}");
    }

    /// P20: a contact at pressure 0 leaves no blob (winit's force `None` used
    /// to become full pressure).
    #[test]
    fn pressure_zero_contact_is_not_a_blob() {
        let ink = |p: f32| {
            let mut h = Harness::with_pen();
            let s = [
                h.pen(PenPhase::Down, [200.0, 200.0], p),
                h.pen(PenPhase::Move, [201.5, 200.0], p),
                h.pen(PenPhase::Up, [201.5, 200.0], p),
            ];
            h.pen_frame(&s, mirrored(&s));
            assert!(!h.studio.engine.is_stroking());
            h.ink()
        };
        let (light, full) = (ink(0.0), ink(1.0));
        assert!(full > 0);
        assert!(light * 20 < full, "zero-pressure contact painted {light} vs {full} at full pressure");
    }

    /// P21: with `native_pen` off the queue is disabled, its samples are
    /// discarded and the Touch path paints exactly as without a queue.
    #[test]
    fn touch_fallback_unchanged_without_queue_or_when_disabled() {
        let id = u64::from(PEN);
        let mut h = Harness::with_pen();
        h.studio.input.native_pen = false;
        // The native samples point elsewhere; only the Touch events may paint.
        let far = pen_line(&mut h, [400.0, 400.0], [450.0, 450.0], 3, 1.0, 1.0);
        let (a, b) = (h.screen([100.0, 100.0]), h.screen([130.0, 100.0]));
        h.pen_frame(&far[..2], vec![Event::PointerMoved(a), touch(id, TouchPhase::Start, a)]);
        assert!(!h.pane.pen.as_ref().unwrap().enabled());
        assert!(h.studio.engine.is_stroking(), "the Touch path did not start the stroke");
        h.pen_frame(&far[2..], vec![touch(id, TouchPhase::Move, b), touch(id, TouchPhase::End, b)]);
        assert!(!h.studio.engine.is_stroking());
        // Ink stays on the Touch line (row 1); the native line is tiles 6..7.
        let painted = h.painted();
        assert!(painted.contains(&TileCoord::from_pixel(110, 100)));
        assert!(painted.iter().all(|c| c.y == 1 && c.x <= 2), "the disabled queue painted: {painted:?}");
        assert!(h.pane.pen_buf.is_empty());
        assert!(!h.pane.pen_stats().native);

        // The same Touch input without any queue paints the same pixels.
        let mut plain = Harness::new();
        plain.frame(vec![Event::PointerMoved(a), touch(id, TouchPhase::Start, a)]);
        plain.frame(vec![touch(id, TouchPhase::Move, b), touch(id, TouchPhase::End, b)]);
        assert!(plain.tiles() == h.tiles());

        // Turning it back on re-enables the queue.
        h.studio.input.native_pen = true;
        h.frame(vec![]);
        assert!(h.pane.pen.as_ref().unwrap().enabled());
        assert!(h.pane.pen_stats().native);
    }

    /// P22
    #[test]
    fn mouse_strokes_unchanged() {
        let run = |mut h: Harness| {
            let (a, b, c) = (h.screen([100.0, 100.0]), h.screen([200.0, 150.0]), h.screen([260.0, 300.0]));
            h.frame(vec![Event::PointerMoved(a)]);
            h.frame(vec![primary(a, true, Modifiers::NONE)]);
            h.frame(vec![Event::PointerMoved(b), Event::PointerMoved(c)]);
            assert!(h.studio.engine.is_stroking());
            assert!(h.pane.stroke == Some(StrokeSrc::Mouse));
            h.frame(vec![primary(c, false, Modifiers::NONE)]);
            assert!(!h.studio.engine.is_stroking());
            h.tiles()
        };
        let plain = run(Harness::new());
        assert!(!plain.is_empty());
        assert!(run(Harness::with_pen()) == plain);
    }

    /// P23
    #[test]
    fn eraser_flip_switches_tool_and_back() {
        let mut h = Harness::with_pen();
        let hover = |h: &mut Harness, end: PenEnd| {
            let s = PenSample { end, ..h.pen(PenPhase::Hover, [200.0, 200.0], 0.0) };
            h.pen_frame(&[s], mirrored(&[s]));
        };
        let pen = Tool::Brush(BrushGroup::Pen);
        assert_eq!(h.studio.tool, pen);
        hover(&mut h, PenEnd::Eraser);
        assert_eq!(h.studio.pen_end(), PenEnd::Eraser);
        assert_eq!(h.studio.tool, Tool::Brush(BrushGroup::Eraser));
        assert!(h.studio.preset().eraser);

        // Pick Pencil while flipped; the tip brings back its own tool.
        commands::execute(Command::SelectTool(Tool::Brush(BrushGroup::Pencil)), &mut h.studio, &mut h.shell);
        hover(&mut h, PenEnd::Tip);
        assert_eq!(h.studio.tool, pen);
        hover(&mut h, PenEnd::Eraser);
        assert_eq!(h.studio.tool, Tool::Brush(BrushGroup::Pencil), "the eraser end remembers Pencil");
        hover(&mut h, PenEnd::Tip);
        assert_eq!(h.studio.tool, pen);

        // A flip reported during a stroke changes nothing until after Up.
        let down = [h.pen(PenPhase::Down, [100.0, 100.0], 1.0), h.pen(PenPhase::Move, [120.0, 100.0], 1.0)];
        h.pen_frame(&down, mirrored(&down));
        assert!(h.studio.engine.is_stroking());
        let flipped = [PenSample { end: PenEnd::Eraser, ..h.pen(PenPhase::Move, [140.0, 100.0], 1.0) }];
        h.pen_frame(&flipped, mirrored(&flipped));
        assert_eq!(h.studio.tool, pen);
        assert!(h.studio.engine.is_stroking());
        let up = [PenSample { end: PenEnd::Eraser, ..h.pen(PenPhase::Up, [140.0, 100.0], 1.0) }];
        h.pen_frame(&up, mirrored(&up));
        assert!(!h.studio.engine.is_stroking());
        assert_eq!(h.studio.tool, pen, "the switch waits for the frame after the stroke");
        assert_eq!(h.studio.history.undo_len(), 1);
        hover(&mut h, PenEnd::Eraser);
        assert_eq!(h.studio.tool, Tool::Brush(BrushGroup::Pencil));

        // Switched off: the eraser end keeps the current tool.
        hover(&mut h, PenEnd::Tip);
        h.studio.input.eraser_end_switch = false;
        hover(&mut h, PenEnd::Eraser);
        assert_eq!((h.studio.tool, h.studio.pen_end()), (pen, PenEnd::Tip));

        // Turning switching (or the native pen) off while flipped returns the
        // pen to its tip tool and files the eraser end's tool; a tool picked
        // meanwhile is the tip's, and turning it back on neither loses nor
        // swaps either end's tool.
        let (eraser, pencil) = (Tool::Brush(BrushGroup::Eraser), Tool::Brush(BrushGroup::Pencil));
        for native in [false, true] {
            // Off and on again with no tool change in between.
            let mut h = Harness::with_pen();
            let set = |h: &mut Harness, on: bool| {
                if native {
                    h.studio.input.native_pen = on;
                } else {
                    h.studio.input.eraser_end_switch = on;
                }
            };
            hover(&mut h, PenEnd::Eraser);
            set(&mut h, false);
            hover(&mut h, PenEnd::Eraser);
            assert_eq!((h.studio.tool, h.studio.pen_end()), (pen, PenEnd::Tip), "native {native}");
            set(&mut h, true);
            hover(&mut h, PenEnd::Tip);
            assert_eq!(h.studio.tool, pen, "native {native}: the tip keeps its tool");
            hover(&mut h, PenEnd::Eraser);
            assert_eq!(h.studio.tool, eraser, "native {native}: the eraser end keeps its tool");

            // Off, pick a tool, on again.
            let mut h = Harness::with_pen();
            hover(&mut h, PenEnd::Eraser);
            assert_eq!(h.studio.tool, eraser);
            set(&mut h, false);
            hover(&mut h, PenEnd::Tip);
            assert_eq!((h.studio.tool, h.studio.pen_end()), (pen, PenEnd::Tip), "native {native}");
            commands::execute(Command::SelectTool(pencil), &mut h.studio, &mut h.shell);
            set(&mut h, true);
            hover(&mut h, PenEnd::Tip);
            assert_eq!((h.studio.tool, h.studio.pen_end()), (pencil, PenEnd::Tip), "native {native}");
            hover(&mut h, PenEnd::Eraser);
            assert_eq!(h.studio.tool, eraser, "native {native}: the eraser end keeps its own tool");
            hover(&mut h, PenEnd::Tip);
            assert_eq!(h.studio.tool, pencil, "native {native}");
        }
    }

    /// A frame long enough to hold a tip contact's end, the flip and the
    /// eraser end's touch-down starts the second contact with the eraser
    /// end's tool (the per-frame check only sees the frame's first contact).
    #[test]
    fn eraser_flip_between_contacts_of_one_frame() {
        let mut h = Harness::with_pen();
        let line = pen_line(&mut h, [100.0, 200.0], [400.0, 200.0], 20, 1.0, 1.0);
        h.pen_frame(&line, mirrored(&line));
        let inked = h.alpha(250, 200);
        assert!(inked > 20000);

        let a = [h.pen(PenPhase::Down, [100.0, 100.0], 1.0), h.pen(PenPhase::Move, [140.0, 100.0], 1.0)];
        h.pen_frame(&a, mirrored(&a));
        assert!(h.studio.engine.is_stroking());
        let e = |h: &mut Harness, phase, at| PenSample { end: PenEnd::Eraser, ..h.pen(phase, at, 1.0) };
        let b = [
            h.pen(PenPhase::Move, [160.0, 100.0], 1.0),
            h.pen(PenPhase::Up, [160.0, 100.0], 1.0),
            e(&mut h, PenPhase::Hover, [250.0, 150.0]),
            e(&mut h, PenPhase::Down, [250.0, 170.0]),
            e(&mut h, PenPhase::Move, [250.0, 210.0]),
            e(&mut h, PenPhase::Move, [250.0, 260.0]),
        ];
        h.pen_frame(&b, mirrored(&b));
        assert_eq!(h.studio.tool, Tool::Brush(BrushGroup::Eraser));
        assert!(h.studio.preset().eraser);
        assert!(h.studio.engine.is_stroking());
        let c = [e(&mut h, PenPhase::Up, [250.0, 260.0])];
        h.pen_frame(&c, mirrored(&c));
        assert!(!h.studio.engine.is_stroking());
        assert_eq!(h.studio.history.undo_len(), 3);
        assert!(h.alpha(250, 200) < inked / 4, "the eraser end did not erase: {}", h.alpha(250, 200));
        assert_eq!(h.alpha(250, 240), 0, "the eraser end laid down ink");
    }

    /// P24
    #[test]
    fn touch_end_safety_net_ends_pen_stroke() {
        let mut h = Harness::with_pen();
        let s = [h.pen(PenPhase::Down, [100.0, 100.0], 1.0), h.pen(PenPhase::Move, [140.0, 110.0], 1.0)];
        h.pen_frame(&s, mirrored(&s));
        assert!(h.studio.engine.is_stroking());
        // egui saw the lift-off, the native queue did not.
        h.frame(vec![touch(u64::from(PEN), TouchPhase::End, pos2(s[1].pos[0], s[1].pos[1]))]);
        assert!(!h.studio.engine.is_stroking());
        assert!(h.pane.stroke.is_none());
        assert_eq!(h.studio.history.undo_len(), 1);

        // Another contact's End does not end it.
        let mut h = Harness::with_pen();
        let s = [h.pen(PenPhase::Down, [100.0, 100.0], 1.0)];
        h.pen_frame(&s, mirrored(&s));
        h.frame(vec![touch(9, TouchPhase::End, pos2(10.0, 10.0))]);
        assert!(h.studio.engine.is_stroking());

        // One frame holds the end of a contact and the start of the next with
        // the same pointer id (Windows keeps it while the pen is in range):
        // the earlier contact's Touch End must not end the new stroke.
        for hover in [false, true] {
            let mut h = Harness::with_pen();
            let a = [h.pen(PenPhase::Down, [100.0, 100.0], 1.0), h.pen(PenPhase::Move, [140.0, 110.0], 1.0)];
            h.pen_frame(&a, mirrored(&a));
            let mut b = vec![h.pen(PenPhase::Move, [160.0, 120.0], 1.0), h.pen(PenPhase::Up, [160.0, 120.0], 1.0)];
            if hover {
                b.push(h.pen(PenPhase::Hover, [180.0, 180.0], 0.0));
            }
            b.extend([h.pen(PenPhase::Down, [200.0, 200.0], 1.0), h.pen(PenPhase::Move, [220.0, 210.0], 1.0)]);
            h.pen_frame(&b, mirrored(&b));
            assert!(h.pane.stroke == Some(StrokeSrc::Pen(PEN)), "hover {hover}: the new stroke was ended");
            assert!(h.studio.engine.is_stroking());
            assert_eq!(h.studio.history.undo_len(), 1);
            let c = [h.pen(PenPhase::Move, [300.0, 260.0], 1.0), h.pen(PenPhase::Up, [300.0, 260.0], 1.0)];
            h.pen_frame(&c, mirrored(&c));
            assert!(!h.studio.engine.is_stroking());
            assert_eq!(h.studio.history.undo_len(), 2);
            assert!(h.column_height(290) > 0, "hover {hover}: the rest of the second stroke was lost");
        }

        // A Down refused off the canvas (a panel tap), then a contact on it in one frame.
        let mut h = Harness::with_pen();
        let off = PenSample { pointer: PEN, phase: PenPhase::Down, pos: [450.0, 100.0], pressure: Some(1.0), time: 100.0, ..Default::default() };
        h.pen_frame(&[off], mirrored(&[off]));
        assert!(h.pane.stroke.is_none());
        let b = [
            PenSample { phase: PenPhase::Up, ..off },
            h.pen(PenPhase::Down, [200.0, 200.0], 1.0),
            h.pen(PenPhase::Move, [220.0, 210.0], 1.0),
        ];
        h.pen_frame(&b, mirrored(&b));
        assert!(h.pane.stroke == Some(StrokeSrc::Pen(PEN)));
        let c = [h.pen(PenPhase::Move, [300.0, 260.0], 1.0), h.pen(PenPhase::Up, [300.0, 260.0], 1.0)];
        h.pen_frame(&c, mirrored(&c));
        assert_eq!(h.studio.history.undo_len(), 1);
        assert!(h.column_height(290) > 0);

        // The new contact's own native Up is lost, the previous one's was not:
        // its Touch End (after its Start) still ends it.
        let mut h = Harness::with_pen();
        let a = [h.pen(PenPhase::Down, [100.0, 100.0], 1.0)];
        h.pen_frame(&a, mirrored(&a));
        let b = [
            h.pen(PenPhase::Move, [160.0, 120.0], 1.0),
            h.pen(PenPhase::Up, [160.0, 120.0], 1.0),
            h.pen(PenPhase::Down, [200.0, 200.0], 1.0),
            h.pen(PenPhase::Move, [220.0, 210.0], 1.0),
        ];
        let mut events = mirrored(&b);
        events.push(touch(u64::from(PEN), TouchPhase::End, pos2(b[3].pos[0], b[3].pos[1])));
        h.pen_frame(&b, events);
        assert!(h.pane.stroke.is_none());
        assert!(!h.studio.engine.is_stroking());
        assert_eq!(h.studio.history.undo_len(), 2);
    }

    #[test]
    fn touch_lifted_honours_only_an_end_after_the_last_start() {
        use TouchPhase::{Cancel, End, Move, Start};
        let (id, p) = (TouchId(3), pos2(1.0, 1.0));
        let ev = |phases: &[TouchPhase]| phases.iter().map(|&ph| touch(3, ph, p)).collect::<Vec<_>>();
        assert!(touch_lifted(&ev(&[Start, Move, End]), id));
        assert!(touch_lifted(&ev(&[Move, Cancel, Move]), id));
        assert!(!touch_lifted(&ev(&[Move, End, Start, Move]), id));
        assert!(touch_lifted(&ev(&[End, Start, Move, End]), id));
        assert!(!touch_lifted(&ev(&[Move, Move]), id));
        assert!(!touch_lifted(&[], id));
        assert!(!touch_lifted(&[touch(4, End, p)], id), "another contact");
    }

    /// P25
    #[test]
    fn cancel_and_leave_end_the_stroke_with_one_undo() {
        for last in [PenPhase::Cancel, PenPhase::Leave] {
            let mut h = Harness::with_pen();
            let s = [h.pen(PenPhase::Down, [100.0, 100.0], 1.0), h.pen(PenPhase::Move, [160.0, 120.0], 1.0)];
            h.pen_frame(&s, mirrored(&s));
            let more = [h.pen(PenPhase::Move, [220.0, 160.0], 1.0)];
            h.pen_frame(&more, mirrored(&more));
            assert!(h.studio.engine.is_stroking());
            h.pen_frame(&[PenSample { phase: last, ..more[0] }], vec![]);
            assert!(!h.studio.engine.is_stroking(), "{last:?} left the stroke hanging");
            assert!(h.pane.stroke.is_none());
            assert_eq!(h.studio.history.undo_len(), 1, "{last:?}");
            assert!(!h.painted().is_empty());
            h.studio.undo();
            assert!(h.painted().is_empty(), "{last:?}: one undo removes the whole stroke");
        }
    }

    /// M1 integration: native pen pressure goes through the user curve, then
    /// the engine's live entry taper and pen-up exit taper (Tail replay), and
    /// the shaped stroke is one undo step. Mouse pressure skips the curve.
    #[test]
    fn pen_stroke_composes_curve_and_taper() {
        let run = |curve: PressureCurve, taper: f32| {
            let mut h = Harness::with_pen();
            h.studio.input.pressure_curve = curve;
            let p = h.studio.preset_mut();
            p.stabilizer = 0;
            p.taper_in = taper;
            p.taper_out = taper;
            let line = pen_line(&mut h, [100.0, 250.0], [400.0, 250.0], 60, 0.5, 0.5);
            h.pen_frame(&line, mirrored(&line));
            assert!(!h.studio.engine.is_stroking());
            h
        };
        let boost = PressureCurve::from_points(&[[0.0, 0.0], [0.5, 1.0], [1.0, 1.0]]);
        let linear = run(PressureCurve::linear(), 0.0);
        let curved = run(boost, 0.0);
        let mid = curved.column_height(250);
        assert!(mid > linear.column_height(250) + 1, "curve did not reach the engine: {mid}");
        assert_eq!(curved.studio.engine.last_reshape(), Reshape::Skipped);

        let mut tapered = run(boost, 80.0);
        assert!(matches!(tapered.studio.engine.last_reshape(), Reshape::Tail { .. } | Reshape::Full));
        assert!(tapered.column_height(250).abs_diff(mid) <= 1, "taper changed the middle");
        let (start, end) = (tapered.column_height(110), tapered.column_height(390));
        assert!(start < mid && end < mid, "no taper: {start} / {mid} / {end}");
        assert!(end < curved.column_height(390), "exit taper missing at pen-up");
        assert_eq!(tapered.studio.history.undo_len(), 1);
        tapered.studio.undo();
        assert!(tapered.painted().is_empty(), "one undo removes the whole shaped stroke");

        // A curve that zeroes pen pressure leaves (almost) no pen ink, but the
        // mouse still paints at `mouse_pressure`.
        let zero = PressureCurve::from_points(&[[0.0, 0.0], [1.0, 0.0]]);
        let pen_ink = run(zero, 0.0).ink();
        let mut h = Harness::with_pen();
        h.studio.input.pressure_curve = zero;
        let (a, b) = (h.screen([100.0, 250.0]), h.screen([400.0, 250.0]));
        h.frame(vec![Event::PointerMoved(a)]);
        h.frame(vec![primary(a, true, Modifiers::NONE)]);
        h.frame(vec![Event::PointerMoved(b)]);
        h.frame(vec![primary(b, false, Modifiers::NONE)]);
        assert!(pen_ink * 20 < h.ink(), "pen {pen_ink} vs mouse {}", h.ink());
    }
}
