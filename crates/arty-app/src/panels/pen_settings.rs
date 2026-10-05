//! Pen and mouse input settings, shown under the Tool Property panel.

use egui::Slider;

use super::property::percent;
use crate::studio::Studio;

pub fn ui(ui: &mut egui::Ui, studio: &mut Studio) {
    egui::CollapsingHeader::new("Pen & mouse").default_open(false).show(ui, |ui| {
        egui::Grid::new("input-settings").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            ui.label("Pressure curve");
            ui.add(Slider::new(&mut studio.input.pressure_gamma, 0.3..=3.0).logarithmic(true).max_decimals(2))
                .on_hover_text("Above 1 needs a firmer press; below 1 is more sensitive");
            ui.end_row();
            ui.label("Mouse pressure");
            ui.add(percent(&mut studio.input.mouse_pressure));
            ui.end_row();
            ui.label("Native pen");
            ui.checkbox(&mut studio.input.native_pen, "").on_hover_text(
                "Read Windows Ink directly: full-rate pressure, tilt and eraser end. Turn off if your tablet driver misbehaves.",
            );
            ui.end_row();
            ui.label("Eraser end");
            ui.checkbox(&mut studio.input.eraser_end_switch, "")
                .on_hover_text("Flipping the pen switches to the eraser end's tool");
            ui.end_row();
        });
    });
}
