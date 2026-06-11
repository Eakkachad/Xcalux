#![allow(dead_code)]

pub mod fill;
pub mod selection;
pub mod transform;

use ahash::AHashMap;
use egui::{Pos2, Rect, Vec2};

// ── Re-export the canonical ToolId from app ──
pub use crate::app::ToolId;

// ── Shared context passed to every tool method ──
#[derive(Clone, Debug)]
pub struct ToolContext {
    pub viewport_offset: Vec2,
    pub viewport_zoom: f32,
    pub rotation_angle: f32,
    pub mirror_horizontal: bool,
    pub screen_rect: Rect,
    pub pointer_down: bool,
    pub pointer_clicked: bool,
    pub pointer_drag_stopped: bool,
    pub pointer_double_clicked: bool,
    pub pointer_pos: Option<Pos2>,
    pub pointer_pressure: f32,
}

impl ToolContext {
    pub fn screen_to_world(&self, screen: Pos2) -> Vec2 {
        let center = self.screen_rect.center();
        let half_w = self.screen_rect.width() * 0.5;
        let half_h = self.screen_rect.height() * 0.5;

        let dx = screen.x - center.x;
        let dy = screen.y - center.y;
        let nx = dx / half_w;
        let ny = -dy / half_h;

        let cos_rot = (-self.rotation_angle).cos();
        let sin_rot = (-self.rotation_angle).sin();
        let mut px = nx * cos_rot - ny * sin_rot;
        let py = nx * sin_rot + ny * cos_rot;

        if self.mirror_horizontal {
            px = -px;
        }

        Vec2::new(
            ((px + 1.0) * half_w) / self.viewport_zoom + self.viewport_offset.x,
            ((1.0 - py) * half_h) / self.viewport_zoom + self.viewport_offset.y,
        )
    }

    pub fn world_to_screen(&self, world: Vec2) -> Pos2 {
        let center = self.screen_rect.center();
        let half_w = self.screen_rect.width() * 0.5;
        let half_h = self.screen_rect.height() * 0.5;

        let mut px = ((world.x - self.viewport_offset.x) * self.viewport_zoom) / half_w - 1.0;
        let py = 1.0 - ((world.y - self.viewport_offset.y) * self.viewport_zoom) / half_h;

        if self.mirror_horizontal {
            px = -px;
        }

        let cos_rot = (-self.rotation_angle).cos();
        let sin_rot = (-self.rotation_angle).sin();
        let nx = px * cos_rot + py * sin_rot;
        let ny = -px * sin_rot + py * cos_rot;

        let dx = nx * half_w;
        let dy = -ny * half_h;

        Pos2::new(center.x + dx, center.y + dy)
    }
}

// ── Outcome returned by Tool::handle_event ──

pub enum ToolOutcome {
    /// Tool did not handle the event; fall through to inline dispatch.
    None,
    /// Tool handled the event; no further processing needed.
    Handled,
    /// Color picker sampled a color at the given world-space pixel.
    ColorPicked { x: i32, y: i32 },
    /// Magic wand select at the given world-space pixel.
    MagicWandSelect { x: i32, y: i32 },
    /// Polygon lasso completed with the given world-space points.
    PolygonLassoComplete { points: Vec<(f32, f32)> },
    /// Flood fill at the given world-space pixel.
    Fill { x: i32, y: i32 },
    /// Vector pen: ensure the active layer is a vector layer.
    VectorPenActivate,
    /// Curve tool completed a stroke from the given control points.
    CurveComplete { points: Vec<crate::canvas::VectorControlPoint> },
    /// EditCP: click at world position (dispatch does hit-test).
    EditCPClick { world_pos: egui::Vec2 },
    /// EditCP: drag to world position.
    EditCPDrag { world_pos: egui::Vec2 },
    /// EditCP: drag released.
    EditCPRelease,
    /// Gradient tool: drag in progress.
    GradientDrag { world_pos: egui::Vec2 },
    /// Gradient tool: drag complete; PaintApp applies the gradient.
    GradientComplete { start: egui::Vec2, end: egui::Vec2 },
    /// RectSelect / EllipseSelect: drag updated to world position.
    RectSelectUpdated { world_pos: egui::Vec2 },
    RectSelectComplete,
    LassoUpdated { world_pos: egui::Vec2 },
    LassoComplete { points: Vec<(f32, f32)> },
    /// Move/Reference: pointer clicked at screen position (dispatch checks reference hit).
    MoveClick { screen_pos: egui::Pos2 },
    MoveDrag { screen_pos: egui::Pos2 },
    /// Transform: pointer down at screen position.
    TransformDown { screen_pos: egui::Pos2 },
    /// Transform: drag update at screen position.
    TransformDrag { screen_pos: egui::Pos2 },
}

// ── Tool trait ──

pub trait Tool: Send {
    fn name(&self) -> &'static str;
    fn tool_id(&self) -> ToolId;

    /// Handle a pointer event on the canvas (no PaintApp access).
    fn handle_event(&mut self, ctx: &ToolContext) -> ToolOutcome;

    /// Handle a pointer event with full PaintApp access.
    /// Default implementation falls through to handle_event.
    /// Override for complex tools (Brush, Eraser, Transform, etc.).
    fn handle_event_full(&mut self, _app: &mut crate::app::PaintApp, ctx: &ToolContext) -> ToolOutcome {
        self.handle_event(ctx)
    }

    /// Draw tool-specific overlays on top of the canvas but below the cursor.
    fn draw_overlay(&self, painter: &egui::Painter, ctx: &ToolContext);

    /// Draw a tool-specific cursor. Return true if a custom cursor was drawn (hides the default).
    fn draw_cursor(&self, screen_pos: Pos2, painter: &egui::Painter) -> bool;
}

// ── ToolRegistry ──

pub struct ToolRegistry {
    tools: AHashMap<ToolId, Box<dyn Tool>>,
    pub active: ToolId,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: AHashMap::new(),
            active: ToolId::Brush,
        }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let id = tool.tool_id();
        self.tools.insert(id, tool);
    }

    pub fn active_tool_id(&self) -> ToolId {
        self.active
    }

    pub fn active_tool(&self) -> Option<&dyn Tool> {
        self.tools.get(&self.active).map(|t| t.as_ref())
    }

    pub fn handle_active_event(&mut self, ctx: &ToolContext) -> ToolOutcome {
        self.tools
            .get_mut(&self.active)
            .map(|t| t.handle_event(ctx))
            .unwrap_or(ToolOutcome::None)
    }

    pub fn handle_active_event_full(&mut self, app: &mut crate::app::PaintApp, ctx: &ToolContext) -> ToolOutcome {
        self.tools
            .get_mut(&self.active)
            .map(|t| t.handle_event_full(app, ctx))
            .unwrap_or(ToolOutcome::None)
    }

    pub fn draw_active_overlay(&self, painter: &egui::Painter, ctx: &ToolContext) {
        if let Some(tool) = self.tools.get(&self.active) {
            tool.draw_overlay(painter, ctx);
        }
    }

    pub fn draw_active_cursor(&self, screen_pos: egui::Pos2, painter: &egui::Painter) -> bool {
        self.tools
            .get(&self.active)
            .map(|t| t.draw_cursor(screen_pos, painter))
            .unwrap_or(false)
    }

    pub fn activate(&mut self, id: ToolId) -> bool {
        if self.tools.contains_key(&id) {
            self.active = id;
            true
        } else {
            false
        }
    }

    pub fn has_tool(&self, id: ToolId) -> bool {
        self.tools.contains_key(&id)
    }

    pub fn active_name(&self) -> &'static str {
        self.tools
            .get(&self.active)
            .map(|t| t.name())
            .unwrap_or("Unknown")
    }
}

// ── Concrete tool implementations ──

pub struct BrushTool;
impl Tool for BrushTool {
    fn name(&self) -> &'static str { "Brush" }
    fn tool_id(&self) -> ToolId { ToolId::Brush }
    fn handle_event(&mut self, _ctx: &ToolContext) -> ToolOutcome { ToolOutcome::None }
    fn draw_overlay(&self, _painter: &egui::Painter, _ctx: &ToolContext) {}
    fn draw_cursor(&self, _screen_pos: Pos2, _painter: &egui::Painter) -> bool { false }
}

pub struct EraserTool;
impl Tool for EraserTool {
    fn name(&self) -> &'static str { "Eraser" }
    fn tool_id(&self) -> ToolId { ToolId::Eraser }
    fn handle_event(&mut self, _ctx: &ToolContext) -> ToolOutcome { ToolOutcome::None }
    fn draw_overlay(&self, _painter: &egui::Painter, _ctx: &ToolContext) {}
    fn draw_cursor(&self, _screen_pos: Pos2, _painter: &egui::Painter) -> bool { false }
}

pub struct HandTool;
impl Tool for HandTool {
    fn name(&self) -> &'static str { "Hand" }
    fn tool_id(&self) -> ToolId { ToolId::Hand }

    fn handle_event(&mut self, _ctx: &ToolContext) -> ToolOutcome {
        ToolOutcome::None
    }

    fn draw_overlay(&self, _painter: &egui::Painter, _ctx: &ToolContext) {}

    fn draw_cursor(&self, _screen_pos: Pos2, _painter: &egui::Painter) -> bool { false }
}

pub struct ZoomTool;
impl Tool for ZoomTool {
    fn name(&self) -> &'static str { "Zoom" }
    fn tool_id(&self) -> ToolId { ToolId::Zoom }

    fn handle_event(&mut self, _ctx: &ToolContext) -> ToolOutcome {
        ToolOutcome::None
    }

    fn draw_overlay(&self, _painter: &egui::Painter, _ctx: &ToolContext) {}

    fn draw_cursor(&self, _screen_pos: Pos2, _painter: &egui::Painter) -> bool { false }
}

pub struct RotateViewTool;
impl Tool for RotateViewTool {
    fn name(&self) -> &'static str { "Rotate View" }
    fn tool_id(&self) -> ToolId { ToolId::RotateView }

    fn handle_event(&mut self, _ctx: &ToolContext) -> ToolOutcome {
        ToolOutcome::None
    }

    fn draw_overlay(&self, _painter: &egui::Painter, _ctx: &ToolContext) {}

    fn draw_cursor(&self, _screen_pos: Pos2, _painter: &egui::Painter) -> bool { false }
}

pub struct ColorPickerTool;
impl Tool for ColorPickerTool {
    fn name(&self) -> &'static str { "Color Picker" }
    fn tool_id(&self) -> ToolId { ToolId::ColorPicker }

    fn handle_event(&mut self, ctx: &ToolContext) -> ToolOutcome {
        if ctx.pointer_clicked || ctx.pointer_drag_stopped {
            if let Some(screen_pos) = ctx.pointer_pos {
                let world = ctx.screen_to_world(screen_pos);
                return ToolOutcome::ColorPicked { x: world.x as i32, y: world.y as i32 };
            }
        }
        ToolOutcome::None
    }

    fn draw_overlay(&self, _painter: &egui::Painter, _ctx: &ToolContext) {}

    fn draw_cursor(&self, screen_pos: Pos2, painter: &egui::Painter) -> bool {
        let (cx, cy) = (screen_pos.x, screen_pos.y);
        // Eyedropper: small circle (bulb) + stem + tip
        painter.circle_filled(
            egui::pos2(cx, cy - 8.0),
            4.0,
            egui::Color32::from_rgb(180, 180, 220),
        );
        painter.circle_stroke(
            egui::pos2(cx, cy - 8.0),
            4.0,
            egui::Stroke::new(1.0, egui::Color32::WHITE),
        );
        painter.line_segment(
            [egui::pos2(cx, cy - 4.0), egui::pos2(cx, cy + 3.0)],
            egui::Stroke::new(2.0, egui::Color32::from_rgb(180, 180, 220)),
        );
        // Tip: small filled diamond
        let tip_y = cy + 5.0;
        painter.line_segment(
            [egui::pos2(cx, tip_y + 2.0), egui::pos2(cx - 2.0, tip_y)],
            egui::Stroke::new(1.5, egui::Color32::from_rgb(180, 180, 220)),
        );
        painter.line_segment(
            [egui::pos2(cx, tip_y + 2.0), egui::pos2(cx + 2.0, tip_y)],
            egui::Stroke::new(1.5, egui::Color32::from_rgb(180, 180, 220)),
        );
        true
    }
}

pub struct PolygonLassoTool {
    points: Vec<(f32, f32)>,
    active: bool,
}

impl PolygonLassoTool {
    pub fn new() -> Self {
        Self {
            points: Vec::new(),
            active: false,
        }
    }
}

impl Tool for PolygonLassoTool {
    fn name(&self) -> &'static str { "Polygon Lasso" }
    fn tool_id(&self) -> ToolId { ToolId::PolygonLasso }

    fn handle_event(&mut self, ctx: &ToolContext) -> ToolOutcome {
        if ctx.pointer_clicked {
            if let Some(screen_pos) = ctx.pointer_pos {
                let world = ctx.screen_to_world(screen_pos);
                let wx = world.x;
                let wy = world.y;

                // Check if clicking near first point to close polygon
                let close_threshold = 8.0;
                let can_close = self.points.len() >= 3 && {
                    let first = self.points[0];
                    let dx = wx - first.0;
                    let dy = wy - first.1;
                    (dx * dx + dy * dy).sqrt() < close_threshold
                };

                if (can_close || ctx.pointer_double_clicked) && self.points.len() >= 3 {
                    // Close polygon — return points and reset internal state
                    let result = ToolOutcome::PolygonLassoComplete {
                        points: std::mem::take(&mut self.points),
                    };
                    self.active = false;
                    result
                } else {
                    // Add point
                    self.points.push((wx, wy));
                    self.active = true;
                    ToolOutcome::Handled
                }
            } else {
                ToolOutcome::None
            }
        } else {
            ToolOutcome::None
        }
    }

    fn draw_overlay(&self, painter: &egui::Painter, ctx: &ToolContext) {
        if !self.active || self.points.len() < 2 {
            return;
        }
        // Draw segment lines between consecutive points
        for i in 0..self.points.len() - 1 {
            let a = ctx.world_to_screen(egui::Vec2::new(self.points[i].0, self.points[i].1));
            let b = ctx.world_to_screen(egui::Vec2::new(self.points[i + 1].0, self.points[i + 1].1));
            painter.line_segment(
                [a, b],
                egui::Stroke::new(
                    2.0,
                    egui::Color32::from_rgba_premultiplied(0, 180, 255, 220),
                ),
            );
        }
        // Draw cursor line from last point to mouse cursor
        if let Some(ptr_pos) = ctx.pointer_pos {
            let last_world = egui::Vec2::new(
                self.points[self.points.len() - 1].0,
                self.points[self.points.len() - 1].1,
            );
            let last_screen = ctx.world_to_screen(last_world);
            painter.line_segment(
                [last_screen, ptr_pos],
                egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_premultiplied(0, 180, 255, 120),
                ),
            );
        }
        // Draw control points as filled circles
        for &(px, py) in self.points.iter() {
            let screen_pt = ctx.world_to_screen(egui::Vec2::new(px, py));
            painter.circle_filled(
                screen_pt,
                3.0,
                egui::Color32::from_rgb(0, 180, 255),
            );
            painter.circle_stroke(
                screen_pt,
                3.0,
                egui::Stroke::new(1.0, egui::Color32::WHITE),
            );
        }
        // Draw first point slightly larger to indicate close-ability
        if self.points.len() >= 3 {
            let first_pt = ctx.world_to_screen(egui::Vec2::new(self.points[0].0, self.points[0].1));
            painter.circle_stroke(
                first_pt,
                5.0,
                egui::Stroke::new(1.5, egui::Color32::from_rgb(255, 200, 0)),
            );
        }
    }

    fn draw_cursor(&self, _screen_pos: egui::Pos2, _painter: &egui::Painter) -> bool { false }
}

pub struct FillTool;
impl Tool for FillTool {
    fn name(&self) -> &'static str { "Fill" }
    fn tool_id(&self) -> ToolId { ToolId::Fill }

    fn handle_event(&mut self, ctx: &ToolContext) -> ToolOutcome {
        if ctx.pointer_clicked {
            if let Some(screen_pos) = ctx.pointer_pos {
                let world = ctx.screen_to_world(screen_pos);
                let fx = world.x as i32;
                let fy = world.y as i32;
                if fx >= 0 && fy >= 0 {
                    return ToolOutcome::Fill { x: fx, y: fy };
                }
            }
        }
        ToolOutcome::None
    }

    fn draw_overlay(&self, _painter: &egui::Painter, _ctx: &ToolContext) {}

    fn draw_cursor(&self, screen_pos: Pos2, painter: &egui::Painter) -> bool {
        let (cx, cy) = (screen_pos.x, screen_pos.y);
        // Paint bucket: trapezoid body
        let pts = [
            egui::pos2(cx - 5.0, cy - 2.0),
            egui::pos2(cx + 5.0, cy - 2.0),
            egui::pos2(cx + 4.0, cy + 6.0),
            egui::pos2(cx - 4.0, cy + 6.0),
        ];
        painter.add(egui::Shape::convex_polygon(
            pts.to_vec(),
            egui::Color32::from_rgb(100, 160, 255),
            egui::Stroke::new(1.0, egui::Color32::WHITE),
        ));
        // Handle (small arc at top)
        painter.line_segment(
            [egui::pos2(cx - 3.0, cy - 2.0), egui::pos2(cx - 3.0, cy - 5.0)],
            egui::Stroke::new(1.5, egui::Color32::from_rgb(100, 160, 255)),
        );
        painter.line_segment(
            [egui::pos2(cx - 3.0, cy - 5.0), egui::pos2(cx + 3.0, cy - 5.0)],
            egui::Stroke::new(1.5, egui::Color32::from_rgb(100, 160, 255)),
        );
        true
    }
}

pub struct MagicWandTool;
impl Tool for MagicWandTool {
    fn name(&self) -> &'static str { "Magic Wand" }
    fn tool_id(&self) -> ToolId { ToolId::MagicWand }

    fn handle_event(&mut self, ctx: &ToolContext) -> ToolOutcome {
        if ctx.pointer_clicked {
            if let Some(screen_pos) = ctx.pointer_pos {
                let world = ctx.screen_to_world(screen_pos);
                let wx = world.x as i32;
                let wy = world.y as i32;
                if wx >= 0 && wy >= 0 {
                    return ToolOutcome::MagicWandSelect { x: wx, y: wy };
                }
            }
        }
        ToolOutcome::None
    }

    fn draw_overlay(&self, _painter: &egui::Painter, _ctx: &ToolContext) {}

    fn draw_cursor(&self, screen_pos: Pos2, painter: &egui::Painter) -> bool {
        let (cx, cy) = (screen_pos.x, screen_pos.y);
        // Magic wand: 4-pointed sparkle star
        let thin = egui::Stroke::new(2.5, egui::Color32::from_rgb(255, 220, 80));
        let outer = 8.0;
        let inner = 3.0;
        for i in 0..4 {
            let angle = i as f32 * std::f32::consts::FRAC_PI_2 + std::f32::consts::FRAC_PI_4;
            let (sin_a, cos_a) = angle.sin_cos();
            let x1 = cx + cos_a * inner;
            let y1 = cy + sin_a * inner;
            let x2 = cx + cos_a * outer;
            let y2 = cy + sin_a * outer;
            painter.line_segment(
                [egui::pos2(x1, y1), egui::pos2(x2, y2)],
                thin,
            );
        }
        // Center glow
        painter.circle_filled(
            screen_pos,
            2.0,
            egui::Color32::from_rgb(255, 255, 200),
        );
        true
    }
}

pub struct MoveTool;
impl Tool for MoveTool {
    fn name(&self) -> &'static str { "Move" }
    fn tool_id(&self) -> ToolId { ToolId::Move }

    fn handle_event(&mut self, ctx: &ToolContext) -> ToolOutcome {
        if ctx.pointer_clicked || (ctx.pointer_down && ctx.pointer_drag_stopped) {
            // Click or drag start
            ctx.pointer_pos.map_or(ToolOutcome::None, |sp| ToolOutcome::MoveClick { screen_pos: sp })
        } else if ctx.pointer_down {
            // Continuous drag
            ctx.pointer_pos.map_or(ToolOutcome::None, |sp| ToolOutcome::MoveDrag { screen_pos: sp })
        } else {
            ToolOutcome::None
        }
    }

    fn draw_overlay(&self, _painter: &egui::Painter, _ctx: &ToolContext) {}

    fn draw_cursor(&self, screen_pos: Pos2, painter: &egui::Painter) -> bool {
        let (cx, cy) = (screen_pos.x, screen_pos.y);
        // Crosshair with gradient indicator: small cross + two color dots
        let cross_len = 6.0;
        let col = egui::Color32::from_rgb(100, 200, 255);
        painter.line_segment(
            [egui::pos2(cx - cross_len, cy), egui::pos2(cx + cross_len, cy)],
            egui::Stroke::new(1.0, col),
        );
        painter.line_segment(
            [egui::pos2(cx, cy - cross_len), egui::pos2(cx, cy + cross_len)],
            egui::Stroke::new(1.0, col),
        );
        // Start/end dots to indicate gradient direction
        painter.circle_filled(
            egui::pos2(cx - 3.0, cy - 3.0),
            2.0,
            egui::Color32::from_rgb(100, 200, 255),
        );
        painter.circle_filled(
            egui::pos2(cx + 3.0, cy + 3.0),
            2.0,
            egui::Color32::from_rgb(255, 200, 100),
        );
        true
    }
}

// =============================================================
// RECT SELECT TOOL
// =============================================================
pub struct RectSelectTool;
impl Tool for RectSelectTool {
    fn name(&self) -> &'static str { "Rect Select" }
    fn tool_id(&self) -> ToolId { ToolId::RectSelect }

    fn handle_event(&mut self, ctx: &ToolContext) -> ToolOutcome {
        if ctx.pointer_down {
            if let Some(screen_pos) = ctx.pointer_pos {
                let world = ctx.screen_to_world(screen_pos);
                ToolOutcome::RectSelectUpdated { world_pos: world }
            } else {
                ToolOutcome::None
            }
        } else if ctx.pointer_drag_stopped {
            ToolOutcome::RectSelectComplete
        } else {
            ToolOutcome::None
        }
    }

    fn draw_overlay(&self, _painter: &egui::Painter, _ctx: &ToolContext) {}

    fn draw_cursor(&self, _screen_pos: Pos2, _painter: &egui::Painter) -> bool { false }
}

// =============================================================
// ELLIPSE SELECT TOOL
// =============================================================
pub struct EllipseSelectTool;
impl Tool for EllipseSelectTool {
    fn name(&self) -> &'static str { "Ellipse Select" }
    fn tool_id(&self) -> ToolId { ToolId::EllipseSelect }

    fn handle_event(&mut self, ctx: &ToolContext) -> ToolOutcome {
        if ctx.pointer_down {
            if let Some(screen_pos) = ctx.pointer_pos {
                let world = ctx.screen_to_world(screen_pos);
                ToolOutcome::RectSelectUpdated { world_pos: world }
            } else {
                ToolOutcome::None
            }
        } else if ctx.pointer_drag_stopped {
            ToolOutcome::RectSelectComplete
        } else {
            ToolOutcome::None
        }
    }

    fn draw_overlay(&self, _painter: &egui::Painter, _ctx: &ToolContext) {}

    fn draw_cursor(&self, _screen_pos: Pos2, _painter: &egui::Painter) -> bool { false }
}

// =============================================================
// LASSO TOOL (freehand selection)
// =============================================================
pub struct LassoTool {
    points: Vec<(f32, f32)>,
    active: bool,
}

impl LassoTool {
    pub fn new() -> Self {
        Self {
            points: Vec::new(),
            active: false,
        }
    }
}

impl Tool for LassoTool {
    fn name(&self) -> &'static str { "Lasso" }
    fn tool_id(&self) -> ToolId { ToolId::Lasso }

    fn handle_event(&mut self, ctx: &ToolContext) -> ToolOutcome {
        if ctx.pointer_down {
            if let Some(screen_pos) = ctx.pointer_pos {
                let world = ctx.screen_to_world(screen_pos);
                if !self.active {
                    self.active = true;
                    self.points.clear();
                }
                self.points.push((world.x, world.y));
                ToolOutcome::LassoUpdated { world_pos: world }
            } else {
                ToolOutcome::None
            }
        } else if ctx.pointer_drag_stopped && self.active {
            self.active = false;
            let points = std::mem::take(&mut self.points);
            ToolOutcome::LassoComplete { points }
        } else {
            ToolOutcome::None
        }
    }

    fn draw_overlay(&self, painter: &egui::Painter, ctx: &ToolContext) {
        if !self.active || self.points.len() < 2 {
            return;
        }
        // Draw lasso path
        for i in 0..self.points.len() - 1 {
            let a = ctx.world_to_screen(egui::Vec2::new(self.points[i].0, self.points[i].1));
            let b = ctx.world_to_screen(egui::Vec2::new(self.points[i + 1].0, self.points[i + 1].1));
            painter.line_segment(
                [a, b],
                egui::Stroke::new(
                    2.0,
                    egui::Color32::from_rgba_premultiplied(0, 180, 255, 220),
                ),
            );
        }
        // Draw rubber-band line from last point to cursor
        if let Some(ptr_pos) = ctx.pointer_pos {
            let last = self.points[self.points.len() - 1];
            let last_screen = ctx.world_to_screen(egui::Vec2::new(last.0, last.1));
            painter.line_segment(
                [last_screen, ptr_pos],
                egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_premultiplied(0, 180, 255, 120),
                ),
            );
        }
    }

    fn draw_cursor(&self, _screen_pos: Pos2, _painter: &egui::Painter) -> bool { false }
}

// =============================================================
// REFERENCE TOOL (alias for move/reference image dragging)
// =============================================================
pub struct ReferenceTool;
impl Tool for ReferenceTool {
    fn name(&self) -> &'static str { "Reference" }
    fn tool_id(&self) -> ToolId { ToolId::Reference }

    fn handle_event(&mut self, ctx: &ToolContext) -> ToolOutcome {
        if ctx.pointer_clicked || (ctx.pointer_down && ctx.pointer_drag_stopped) {
            ctx.pointer_pos.map_or(ToolOutcome::None, |sp| ToolOutcome::MoveClick { screen_pos: sp })
        } else if ctx.pointer_down {
            ctx.pointer_pos.map_or(ToolOutcome::None, |sp| ToolOutcome::MoveDrag { screen_pos: sp })
        } else {
            ToolOutcome::None
        }
    }

    fn draw_overlay(&self, _painter: &egui::Painter, _ctx: &ToolContext) {}

    fn draw_cursor(&self, _screen_pos: Pos2, _painter: &egui::Painter) -> bool { false }
}

// =============================================================
// TRANSFORM TOOL
// =============================================================
pub struct TransformTool;
impl Tool for TransformTool {
    fn name(&self) -> &'static str { "Transform" }
    fn tool_id(&self) -> ToolId { ToolId::Transform }

    fn handle_event(&mut self, ctx: &ToolContext) -> ToolOutcome {
        if ctx.pointer_clicked || (ctx.pointer_down && ctx.pointer_drag_stopped) {
            ctx.pointer_pos.map_or(ToolOutcome::None, |sp| ToolOutcome::TransformDown { screen_pos: sp })
        } else if ctx.pointer_down {
            ctx.pointer_pos.map_or(ToolOutcome::None, |sp| ToolOutcome::TransformDrag { screen_pos: sp })
        } else {
            ToolOutcome::None
        }
    }

    fn draw_overlay(&self, _painter: &egui::Painter, _ctx: &ToolContext) {}

    fn draw_cursor(&self, _screen_pos: Pos2, _painter: &egui::Painter) -> bool { false }
}

// =============================================================
// VECTOR PEN TOOL
// =============================================================
pub struct VectorPenTool;
impl Tool for VectorPenTool {
    fn name(&self) -> &'static str { "Vector Pen" }
    fn tool_id(&self) -> ToolId { ToolId::VectorPen }

    fn handle_event(&mut self, ctx: &ToolContext) -> ToolOutcome {
        if ctx.pointer_clicked {
            ToolOutcome::VectorPenActivate
        } else {
            ToolOutcome::None
        }
    }

    fn draw_overlay(&self, _painter: &egui::Painter, _ctx: &ToolContext) {}

    fn draw_cursor(&self, _screen_pos: Pos2, _painter: &egui::Painter) -> bool { false }
}

// =============================================================
// CURVE TOOL
// =============================================================
pub struct CurveTool {
    points: Vec<crate::canvas::VectorControlPoint>,
}

impl CurveTool {
    pub fn new() -> Self {
        Self { points: Vec::new() }
    }
}

impl Tool for CurveTool {
    fn name(&self) -> &'static str { "Curve" }
    fn tool_id(&self) -> ToolId { ToolId::Curve }

    fn handle_event(&mut self, ctx: &ToolContext) -> ToolOutcome {
        if ctx.pointer_clicked {
            if let Some(screen_pos) = ctx.pointer_pos {
                let world = ctx.screen_to_world(screen_pos);
                self.points.push(crate::canvas::VectorControlPoint {
                    x: world.x,
                    y: world.y,
                    pressure: ctx.pointer_pressure,
                    tilt_x: 0.0,
                    tilt_y: 0.0,
                });
                if self.points.len() >= 4 {
                    ToolOutcome::CurveComplete {
                        points: std::mem::take(&mut self.points),
                    }
                } else {
                    ToolOutcome::Handled
                }
            } else {
                ToolOutcome::None
            }
        } else {
            ToolOutcome::None
        }
    }

    fn draw_overlay(&self, painter: &egui::Painter, ctx: &ToolContext) {
        if self.points.is_empty() {
            return;
        }
        let cp_color = egui::Color32::from_rgb(0, 180, 255);
        for cp in &self.points {
            let screen_pt = ctx.world_to_screen(egui::Vec2::new(cp.x, cp.y));
            painter.circle_filled(screen_pt, 4.0, cp_color);
            painter.circle_stroke(screen_pt, 4.0, egui::Stroke::new(1.0, egui::Color32::WHITE));
        }

        if self.points.len() >= 2 {
            for i in 0..self.points.len() - 1 {
                let a = ctx.world_to_screen(egui::Vec2::new(self.points[i].x, self.points[i].y));
                let b = ctx.world_to_screen(egui::Vec2::new(self.points[i + 1].x, self.points[i + 1].y));
                painter.line_segment(
                    [a, b],
                    egui::Stroke::new(1.0, egui::Color32::from_rgba_premultiplied(0, 180, 255, 100)),
                );
            }
        }

        let num_label = format!("{}/4 points placed", self.points.len());
        if let Some(cp) = self.points.last() {
            let screen_pt = ctx.world_to_screen(egui::Vec2::new(cp.x + 10.0, cp.y - 20.0));
            painter.text(
                screen_pt,
                egui::Align2::LEFT_BOTTOM,
                num_label,
                egui::FontId::proportional(12.0),
                egui::Color32::WHITE,
            );
        }
    }

    fn draw_cursor(&self, _screen_pos: Pos2, _painter: &egui::Painter) -> bool { false }
}

// =============================================================
// EDIT CONTROL POINT TOOL
// =============================================================
pub struct EditCPTool {
    pub selection: Option<(usize, usize)>,
    pub dragging: bool,
}

impl EditCPTool {
    pub fn new() -> Self {
        Self {
            selection: None,
            dragging: false,
        }
    }
}

impl Tool for EditCPTool {
    fn name(&self) -> &'static str { "Edit CP" }
    fn tool_id(&self) -> ToolId { ToolId::EditCP }

    fn handle_event(&mut self, ctx: &ToolContext) -> ToolOutcome {
        if ctx.pointer_clicked {
            if let Some(screen_pos) = ctx.pointer_pos {
                let world = ctx.screen_to_world(screen_pos);
                ToolOutcome::EditCPClick { world_pos: world }
            } else {
                ToolOutcome::None
            }
        } else if self.dragging && ctx.pointer_down {
            if let Some(screen_pos) = ctx.pointer_pos {
                let world = ctx.screen_to_world(screen_pos);
                ToolOutcome::EditCPDrag { world_pos: world }
            } else {
                ToolOutcome::None
            }
        } else if !ctx.pointer_down && self.dragging {
            ToolOutcome::EditCPRelease
        } else {
            ToolOutcome::None
        }
    }

    fn draw_overlay(&self, painter: &egui::Painter, ctx: &ToolContext) {
        // EditCP overlay requires layer data; drawn inline via PaintApp
        let _ = painter;
        let _ = ctx;
    }

    fn draw_cursor(&self, _screen_pos: Pos2, _painter: &egui::Painter) -> bool { false }
}

// =============================================================
// GRADIENT TOOL
// =============================================================
pub struct GradientTool {
    pub start: Option<egui::Vec2>,
    pub end: Option<egui::Vec2>,
    pub dragging: bool,
}

impl GradientTool {
    pub fn new() -> Self {
        Self {
            start: None,
            end: None,
            dragging: false,
        }
    }
}

impl Tool for GradientTool {
    fn name(&self) -> &'static str { "Gradient" }
    fn tool_id(&self) -> ToolId { ToolId::Gradient }

    fn handle_event(&mut self, ctx: &ToolContext) -> ToolOutcome {
        if ctx.pointer_down {
            if let Some(screen_pos) = ctx.pointer_pos {
                let world = ctx.screen_to_world(screen_pos);
                if !self.dragging {
                    self.dragging = true;
                    self.start = Some(world);
                }
                self.end = Some(world);
            }
            ToolOutcome::GradientDrag { world_pos: self.end.unwrap_or_default() }
        } else if self.dragging && !ctx.pointer_down {
            let result = ToolOutcome::GradientComplete {
                start: self.start.unwrap_or_default(),
                end: self.end.unwrap_or_default(),
            };
            self.dragging = false;
            self.start = None;
            self.end = None;
            result
        } else {
            ToolOutcome::None
        }
    }

    fn draw_overlay(&self, painter: &egui::Painter, ctx: &ToolContext) {
        if !self.dragging {
            return;
        }
        if let (Some(start), Some(end)) = (self.start, self.end) {
            let a = ctx.world_to_screen(egui::Vec2::new(start.x, start.y));
            let b = ctx.world_to_screen(egui::Vec2::new(end.x, end.y));
            painter.line_segment(
                [a, b],
                egui::Stroke::new(2.0, egui::Color32::from_rgba_premultiplied(100, 200, 255, 200)),
            );
            painter.circle_filled(a, 4.0, egui::Color32::from_rgb(100, 200, 255));
            painter.circle_filled(b, 4.0, egui::Color32::from_rgb(255, 200, 100));

            let dir = (b - a).normalized();
            let perp = egui::vec2(-dir.y, dir.x);
            for (pt, col) in [
                (a, egui::Color32::WHITE),
                (b, egui::Color32::from_gray(100)),
            ] {
                let p1 = pt + perp * 10.0;
                let p2 = pt - perp * 10.0;
                painter.line_segment([p1, p2], egui::Stroke::new(1.5, col));
            }
        }
    }

    fn draw_cursor(&self, _screen_pos: Pos2, _painter: &egui::Painter) -> bool { false }
}
