//! SAI-style brush size palette: one click picks a common size.

use egui::{Color32, CornerRadius, Sense, Stroke, Vec2};

use crate::studio::{Studio, Tool};

const SIZES: &[f32] = &[
    0.7, 1.0, 1.5, 2.0, 3.0, 4.0, 5.0, 6.0, 8.0, 10.0, 12.0, 15.0, 20.0, 25.0, 30.0, 40.0, 50.0, 60.0, 80.0, 100.0,
    150.0, 200.0, 300.0, 500.0,
];

pub fn ui(ui: &mut egui::Ui, studio: &mut Studio) {
    if !matches!(studio.tool, Tool::Brush(_)) {
        ui.weak("Select a brush tool");
        return;
    }
    let current = studio.preset().size;
    let cell = 40.0;
    let cols = ((ui.available_width() / (cell + 4.0)).floor() as usize).max(1);
    let accent = ui.visuals().selection.stroke.color;
    let mut picked = None;
    egui::Grid::new("brush-sizes").spacing([4.0, 4.0]).show(ui, |ui| {
        for (i, &size) in SIZES.iter().enumerate() {
            let (rect, resp) = ui.allocate_exact_size(Vec2::splat(cell), Sense::click());
            let v = ui.visuals();
            let selected = (current - size).abs() < 0.05;
            let fill = if selected {
                v.selection.bg_fill
            } else if resp.hovered() {
                v.widgets.hovered.bg_fill
            } else {
                v.faint_bg_color
            };
            let p = ui.painter();
            p.rect_filled(rect, CornerRadius::same(4), fill);
            if selected {
                p.rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.0, accent), egui::StrokeKind::Inside);
            }
            // Dot area grows with sqrt so 0.7…500 all fit legibly.
            let r = (size.sqrt() * 1.1).clamp(0.8, 11.0);
            let dot = rect.center() - Vec2::new(0.0, 5.0);
            p.circle_filled(dot, r, v.text_color());
            p.text(
                rect.center_bottom() - Vec2::new(0.0, 3.0),
                egui::Align2::CENTER_BOTTOM,
                if size < 1.0 || size.fract() != 0.0 { format!("{size}") } else { format!("{size:.0}") },
                egui::FontId::proportional(9.5),
                if selected { v.strong_text_color() } else { Color32::from_gray(150) },
            );
            if resp.clicked() {
                picked = Some(size);
            }
            if (i + 1) % cols == 0 {
                ui.end_row();
            }
        }
    });
    if let Some(size) = picked {
        studio.preset_mut().size = size;
    }
}
