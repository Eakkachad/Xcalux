//! Navigator: zoom, rotation and flip controls.

use egui::Slider;
use egui_phosphor::regular as icon;

use crate::commands::{self, Command};
use crate::shell::Shell;
use crate::studio::Studio;

pub fn ui(ui: &mut egui::Ui, studio: &mut Studio, shell: &mut Shell) {
    let origin = shell.canvas_center_px;
    egui::Grid::new("nav").num_columns(2).spacing([6.0, 6.0]).show(ui, |ui| {
        ui.label(icon::MAGNIFYING_GLASS);
        let mut pct = studio.view.zoom * 100.0;
        let r = ui.add(Slider::new(&mut pct, 1.0..=6400.0).logarithmic(true).max_decimals(1).suffix("%"));
        if r.changed() {
            studio.view.zoom_at(origin, origin, pct / 100.0);
        }
        ui.end_row();

        ui.label(icon::ARROWS_CLOCKWISE);
        let mut deg = studio.view.rotation.to_degrees();
        let r = ui.add(Slider::new(&mut deg, -180.0..=180.0).max_decimals(0).suffix("°"));
        if r.changed() {
            studio.view.rotate_at(origin, origin, deg.to_radians());
        }
        ui.end_row();
    });
    ui.horizontal_wrapped(|ui| {
        for (glyph, cmd) in [
            (icon::MAGNIFYING_GLASS_MINUS, Command::ZoomOut),
            (icon::MAGNIFYING_GLASS_PLUS, Command::ZoomIn),
            (icon::CORNERS_OUT, Command::ZoomFit),
            ("1:1", Command::Zoom100),
            (icon::ARROW_COUNTER_CLOCKWISE, Command::RotateLeft),
            (icon::ARROW_CLOCKWISE, Command::RotateRight),
            (icon::ARROW_U_UP_LEFT, Command::RotateReset),
        ] {
            if ui.button(glyph).on_hover_text(cmd.label()).clicked() {
                commands::execute(cmd, studio, shell);
            }
        }
        if ui.selectable_label(studio.view.flip_x, icon::FLIP_HORIZONTAL).on_hover_text("Flip horizontal (F)").clicked() {
            commands::execute(Command::FlipView, studio, shell);
        }
    });
}
