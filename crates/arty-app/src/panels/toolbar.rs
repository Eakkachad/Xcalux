//! Narrow vertical tool bar (CSP "Tool" palette) with main/sub colors.

use arty_brush::BrushGroup;
use egui::{Color32, CornerRadius, Rect, Sense, Stroke, Vec2};
use egui_phosphor::regular as icon;

use crate::commands::{self, Command};
use crate::shell::Shell;
use crate::studio::{Studio, Tool};

const TOOLS: &[(Tool, &str, &str)] = &[
    (Tool::Brush(BrushGroup::Pen), icon::PEN_NIB, "Pen (P)"),
    (Tool::Brush(BrushGroup::Pencil), icon::PENCIL_SIMPLE, "Pencil (N)"),
    (Tool::Brush(BrushGroup::Brush), icon::PAINT_BRUSH, "Brush (B)"),
    (Tool::Brush(BrushGroup::Airbrush), icon::SPRAY_BOTTLE, "Airbrush (J)"),
    (Tool::Brush(BrushGroup::Blend), icon::DROP_HALF, "Blend (U)"),
    (Tool::Brush(BrushGroup::Eraser), icon::ERASER, "Eraser (E)"),
];

const VIEW_TOOLS: &[(Tool, &str, &str)] = &[
    (Tool::Eyedropper, icon::EYEDROPPER, "Eyedropper (I) · Alt while painting"),
    (Tool::Hand, icon::HAND, "Hand (H) · hold Space"),
    (Tool::Rotate, icon::ARROWS_CLOCKWISE, "Rotate (R) · Shift+Space"),
    (Tool::Zoom, icon::MAGNIFYING_GLASS, "Zoom (Z) · Ctrl+Space, Alt-click zooms out"),
];

pub fn ui(ui: &mut egui::Ui, studio: &mut Studio, shell: &mut Shell) {
    let pal = shell.theme.palette();
    ui.vertical_centered(|ui| {
        ui.add_space(4.0);
        for group in [TOOLS, VIEW_TOOLS] {
            for &(tool, glyph, tip) in group {
                let selected = studio.tool == tool;
                if tool_button(ui, glyph, selected, pal.accent).on_hover_text(tip).clicked() {
                    commands::execute(Command::SelectTool(tool), studio, shell);
                }
            }
            ui.add_space(4.0);
            ui.separator();
        }
        ui.add_space(6.0);
        color_chips(ui, studio, shell);
    });
}

fn tool_button(ui: &mut egui::Ui, glyph: &str, selected: bool, accent: Color32) -> egui::Response {
    let size = Vec2::splat(32.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let v = ui.visuals();
    let fill = if selected {
        v.selection.bg_fill
    } else if response.hovered() {
        v.widgets.hovered.bg_fill
    } else {
        Color32::TRANSPARENT
    };
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(5), fill);
    if selected {
        p.rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0, accent), egui::StrokeKind::Inside);
    }
    let color = if selected { v.strong_text_color() } else { v.text_color() };
    p.text(rect.center(), egui::Align2::CENTER_CENTER, glyph, egui::FontId::proportional(18.0), color);
    response
}

/// Overlapping main/sub color squares; click the sub chip or press X to swap.
fn color_chips(ui: &mut egui::Ui, studio: &mut Studio, shell: &mut Shell) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(36.0, 36.0), Sense::hover());
    let main = Rect::from_min_size(rect.min, Vec2::splat(22.0));
    let sub = Rect::from_min_size(rect.min + Vec2::splat(13.0), Vec2::splat(22.0));
    let to32 = |c: [f32; 3]| Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8);
    let border = ui.visuals().widgets.noninteractive.bg_stroke.color;
    let sub_resp = ui.interact(sub, ui.id().with("sub-color"), Sense::click()).on_hover_text("Sub color — click to swap (X)");
    let p = ui.painter();
    for (r, c) in [(sub, studio.color.sub), (main, studio.color.main)] {
        p.rect_filled(r, CornerRadius::same(3), to32(c));
        p.rect_stroke(r, CornerRadius::same(3), Stroke::new(1.5, border), egui::StrokeKind::Outside);
    }
    if sub_resp.clicked() {
        commands::execute(Command::SwapColors, studio, shell);
    }
}
