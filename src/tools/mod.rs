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
}

// ── Tool trait ──

pub trait Tool: Send {
    fn name(&self) -> &'static str;
    fn tool_id(&self) -> ToolId;

    /// Handle a pointer event on the canvas.
    fn handle_event(&mut self, ctx: &ToolContext) -> ToolOutcome;

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

    pub fn active_tool(&self) -> Option<&dyn Tool> {
        self.tools.get(&self.active).map(|t| t.as_ref())
    }

    pub fn handle_active_event(&mut self, ctx: &ToolContext) -> ToolOutcome {
        self.tools
            .get_mut(&self.active)
            .map(|t| t.handle_event(ctx))
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

    fn draw_cursor(&self, _screen_pos: Pos2, _painter: &egui::Painter) -> bool { false }
}

pub struct MoveTool;
impl Tool for MoveTool {
    fn name(&self) -> &'static str { "Move" }
    fn tool_id(&self) -> ToolId { ToolId::Move }

    fn handle_event(&mut self, _ctx: &ToolContext) -> ToolOutcome {
        ToolOutcome::None
    }

    fn draw_overlay(&self, _painter: &egui::Painter, _ctx: &ToolContext) {}

    fn draw_cursor(&self, _screen_pos: Pos2, _painter: &egui::Painter) -> bool { false }
}
