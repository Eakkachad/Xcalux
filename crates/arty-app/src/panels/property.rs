//! Tool Property: settings of the active sub tool.

use arty_brush::{MAX_BRUSH_SIZE, MIN_BRUSH_SIZE, Stabilizer};
use egui::Slider;

use super::section;
use crate::shell::Shell;
use crate::studio::{Studio, Tool};
use crate::tools;

pub fn ui(ui: &mut egui::Ui, studio: &mut Studio, shell: &mut Shell) {
    // A transform session shows its own settings whatever the tool.
    if studio.transform.is_some() {
        ui.label(egui::RichText::new("Transform").strong());
        tools::transform::property_ui(ui, studio, shell);
        return;
    }
    if !matches!(studio.tool, Tool::Brush(_)) {
        ui.label(egui::RichText::new(studio.tool.label()).strong());
        let hint = |text: &str, ui: &mut egui::Ui| {
            ui.label(text);
        };
        match studio.tool {
            Tool::Eyedropper => hint("Click or drag on the canvas to pick the displayed color.", ui),
            Tool::Hand => hint("Drag to scroll. Middle mouse drags with any tool.", ui),
            Tool::Rotate => hint("Drag to rotate the view. Hold Shift to snap to 15° (Ctrl with Shift+Space).", ui),
            Tool::Zoom => hint("Click to zoom in, Alt+click to zoom out, drag to zoom smoothly.", ui),
            Tool::Select | Tool::MagicWand => tools::select::property_ui(ui, studio, shell),
            Tool::Fill => tools::fill::property_ui(ui, studio, shell),
            Tool::Move => tools::transform::property_ui(ui, studio, shell),
            Tool::Frame(_) => tools::frame::property_ui(ui, studio, shell),
            Tool::Brush(_) => {}
        }
        super::pen_settings::ui(ui, studio);
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

        section(ui, "STARTING AND ENDING");
        egui::Grid::new("brush-shape").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            ui.label("Taper in");
            ui.add(Slider::new(&mut p.taper_in, 0.0..=500.0).suffix(" px").max_decimals(0))
                .on_hover_text("Uses this sub tool's pressure settings (Min size / Min opacity)");
            ui.end_row();
            ui.label("Taper out");
            ui.add(Slider::new(&mut p.taper_out, 0.0..=500.0).suffix(" px").max_decimals(0))
                .on_hover_text("Applied when the pen lifts. Uses Min size / Min opacity");
            ui.end_row();
            ui.label("Post correction");
            ui.add(Slider::new(&mut p.post_correction, 0..=arty_brush::shape::MAX_CORRECTION))
                .on_hover_text("Smooths the finished line when the pen lifts, relative to the current zoom");
            ui.end_row();
        });
    });

    if p != *studio.preset() {
        *studio.preset_mut() = p;
    }

    ui.add_space(6.0);
    super::pen_settings::ui(ui, studio);
}

pub(super) fn percent(v: &mut f32) -> Slider<'_> {
    Slider::new(v, 0.0..=1.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)).custom_parser(|s| {
        s.trim_end_matches('%').trim().parse::<f64>().ok().map(|v| v / 100.0)
    })
}
