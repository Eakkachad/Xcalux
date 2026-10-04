//! Tool Property: settings of the active sub tool.

use arty_brush::{MAX_BRUSH_SIZE, MIN_BRUSH_SIZE, Stabilizer};
use egui::Slider;

use super::section;
use crate::studio::{Studio, Tool};

pub fn ui(ui: &mut egui::Ui, studio: &mut Studio) {
    if !matches!(studio.tool, Tool::Brush(_)) {
        ui.label(egui::RichText::new(studio.tool.label()).strong());
        ui.label(match studio.tool {
            Tool::Eyedropper => "Click or drag on the canvas to pick the displayed color.",
            Tool::Hand => "Drag to scroll. Middle mouse drags with any tool.",
            Tool::Rotate => "Drag to rotate the view. Hold Shift to snap to 15°.",
            Tool::Zoom => "Click to zoom in, Alt+click to zoom out, drag to zoom smoothly.",
            Tool::Brush(_) => "",
        });
        input_settings(ui, studio);
        return;
    }

    let mut p = studio.preset().clone();
    ui.horizontal(|ui| {
        ui.label("Name");
        ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(f32::INFINITY));
    });
    ui.add_space(4.0);

    egui::Grid::new("brush-props").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label("Size");
        ui.add(Slider::new(&mut p.size, MIN_BRUSH_SIZE..=MAX_BRUSH_SIZE).logarithmic(true).suffix(" px").max_decimals(1));
        ui.end_row();

        ui.label("Opacity");
        ui.add(percent(&mut p.opacity));
        ui.end_row();

        ui.label("Hardness");
        ui.add(percent(&mut p.hardness));
        ui.end_row();

        ui.label("Stabilizer");
        ui.add(Slider::new(&mut p.stabilizer, 0..=Stabilizer::MAX_LEVEL).prefix("S-"));
        ui.end_row();
    });

    ui.add_space(6.0);
    section(ui, "PEN PRESSURE");
    egui::Grid::new("brush-pressure").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label("Min size");
        ui.add(percent(&mut p.min_size)).on_hover_text("Size at the lightest touch (100% = no size pressure)");
        ui.end_row();
        ui.label("Min opacity");
        ui.add(percent(&mut p.min_opacity)).on_hover_text("Opacity at the lightest touch (100% = no opacity pressure)");
        ui.end_row();
    });

    ui.add_space(6.0);
    egui::CollapsingHeader::new("Advanced").default_open(false).show(ui, |ui| {
        egui::Grid::new("brush-adv").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            ui.label("Density");
            ui.add(Slider::new(&mut p.density, 0.5..=12.0).max_decimals(1))
                .on_hover_text("Dabs per radius — higher is smoother but slower");
            ui.end_row();
            ui.label("Blending");
            ui.add(percent(&mut p.blending)).on_hover_text("Mix with color already on the canvas");
            ui.end_row();
            ui.label("Persistence");
            ui.add(percent(&mut p.persistence)).on_hover_text("How long picked-up color lasts");
            ui.end_row();
            ui.label("Size jitter");
            ui.add(percent(&mut p.jitter));
            ui.end_row();
            ui.label("Eraser");
            ui.checkbox(&mut p.eraser, "");
            ui.end_row();
        });
    });

    if p != *studio.preset() {
        *studio.preset_mut() = p;
    }

    ui.add_space(6.0);
    input_settings(ui, studio);
}

fn percent(v: &mut f32) -> Slider<'_> {
    Slider::new(v, 0.0..=1.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)).custom_parser(|s| {
        s.trim_end_matches('%').trim().parse::<f64>().ok().map(|v| v / 100.0)
    })
}

fn input_settings(ui: &mut egui::Ui, studio: &mut Studio) {
    egui::CollapsingHeader::new("Pen & mouse").default_open(false).show(ui, |ui| {
        egui::Grid::new("input-settings").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            ui.label("Pressure curve");
            ui.add(Slider::new(&mut studio.input.pressure_gamma, 0.3..=3.0).logarithmic(true).max_decimals(2))
                .on_hover_text("Above 1 needs a firmer press; below 1 is more sensitive");
            ui.end_row();
            ui.label("Mouse pressure");
            ui.add(percent(&mut studio.input.mouse_pressure));
            ui.end_row();
        });
    });
}
