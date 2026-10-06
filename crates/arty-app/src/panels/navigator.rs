//! Navigator: zoom, rotation and flip controls.

use egui::Slider;
use egui_phosphor::regular as icon;

use super::{even_width, fill_slider};
use crate::commands::{self, Command};
use crate::shell::Shell;
use crate::studio::Studio;

pub fn ui(ui: &mut egui::Ui, studio: &mut Studio, shell: &mut Shell) {
    let origin = shell.canvas_center_px;
    egui::Grid::new("nav").num_columns(2).spacing([6.0, 6.0]).show(ui, |ui| {
        ui.label(icon::MAGNIFYING_GLASS);
        let mut pct = studio.view.zoom * 100.0;
        let r = fill_slider(ui, Slider::new(&mut pct, 1.0..=6400.0).logarithmic(true).max_decimals(1).suffix("%"));
        if r.changed() {
            studio.view.zoom_at(origin, origin, pct / 100.0);
        }
        ui.end_row();

        ui.label(icon::ARROWS_CLOCKWISE);
        let mut deg = studio.view.rotation.to_degrees();
        let r = fill_slider(ui, Slider::new(&mut deg, -180.0..=180.0).max_decimals(0).suffix("°"));
        if r.changed() {
            studio.view.rotate_at(origin, origin, deg.to_radians());
        }
        ui.end_row();
    });
    ui.horizontal_wrapped(|ui| {
        // Eight buttons in one row while they fit, else two rows of four.
        let n = if even_width(ui, 8) >= 22.0 { 8 } else { 4 };
        let size = egui::vec2(even_width(ui, n), 0.0);
        for (glyph, cmd) in [
            (icon::MAGNIFYING_GLASS_MINUS, Command::ZoomOut),
            (icon::MAGNIFYING_GLASS_PLUS, Command::ZoomIn),
            (icon::CORNERS_OUT, Command::ZoomFit),
            ("1:1", Command::Zoom100),
            (icon::ARROW_COUNTER_CLOCKWISE, Command::RotateLeft),
            (icon::ARROW_CLOCKWISE, Command::RotateRight),
            (icon::ARROW_U_UP_LEFT, Command::RotateReset),
        ] {
            if ui.add(egui::Button::new(glyph).min_size(size)).on_hover_text(cmd.label()).clicked() {
                commands::execute(cmd, studio, shell);
            }
        }
        let flip = egui::Button::selectable(studio.view.flip_x, icon::FLIP_HORIZONTAL).min_size(size);
        if ui.add(flip).on_hover_text(crate::text::t(crate::text::Key::NavFlipHorizontal)).clicked() {
            commands::execute(Command::FlipView, studio, shell);
        }
    });
}
