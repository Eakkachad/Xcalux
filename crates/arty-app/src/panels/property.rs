//! Tool Property: settings of the active sub tool.

use arty_brush::{MAX_BRUSH_SIZE, MIN_BRUSH_SIZE, Stabilizer};
use egui::Slider;

use super::{fill_slider, section};
use crate::shell::Shell;
use crate::studio::{Studio, Tool};
use crate::text::{t, Key};
use crate::{theme, tools};

pub fn ui(ui: &mut egui::Ui, studio: &mut Studio, shell: &mut Shell) {
    // A transform session shows its own settings whatever the tool.
    if studio.transform.is_some() {
        ui.label(egui::RichText::new(t(Key::CmdTransform)).strong());
        tools::transform::property_ui(ui, studio, shell);
        return;
    }
    if !matches!(studio.tool, Tool::Brush(_)) {
        ui.label(egui::RichText::new(studio.tool.label()).strong());
        let hint = |text: &str, ui: &mut egui::Ui| {
            ui.label(text);
        };
        match studio.tool {
            Tool::Eyedropper => hint(t(Key::HintEyedropper), ui),
            Tool::Hand => hint(t(Key::HintHand), ui),
            Tool::Rotate => hint(t(Key::HintRotate), ui),
            Tool::Zoom => hint(t(Key::HintZoom), ui),
            Tool::Select | Tool::MagicWand => tools::select::property_ui(ui, studio, shell),
            Tool::Fill => tools::fill::property_ui(ui, studio, shell),
            Tool::Move => tools::transform::property_ui(ui, studio, shell),
            Tool::Frame(_) => tools::frame::property_ui(ui, studio, shell),
            Tool::Brush(_) => {}
        }
        frame_border(ui, studio, shell);
        super::pen_settings::ui(ui, studio, shell);
        return;
    }

    let mut p = studio.preset().clone();
    ui.horizontal(|ui| {
        ui.label(t(Key::PropName));
        ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(f32::INFINITY));
    });
    ui.add_space(4.0);

    egui::Grid::new("brush-props").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label(t(Key::PropSize));
        fill_slider(ui, Slider::new(&mut p.size, MIN_BRUSH_SIZE..=MAX_BRUSH_SIZE).logarithmic(true).suffix(" px").max_decimals(1));
        ui.end_row();

        ui.label(t(Key::PropOpacity));
        fill_slider(ui, percent(&mut p.opacity));
        ui.end_row();

        ui.label(t(Key::PropHardness));
        fill_slider(ui, percent(&mut p.hardness));
        ui.end_row();

        ui.label(t(Key::PropStabilizer));
        fill_slider(ui, Slider::new(&mut p.stabilizer, 0..=Stabilizer::MAX_LEVEL).prefix("S-"));
        ui.end_row();
    });

    ui.add_space(6.0);
    section(ui, t(Key::SectionPenPressure));
    egui::Grid::new("brush-pressure").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label(t(Key::PropMinSize));
        fill_slider(ui, percent(&mut p.min_size)).on_hover_text(t(Key::PropMinSizeTip));
        ui.end_row();
        ui.label(t(Key::PropMinOpacity));
        fill_slider(ui, percent(&mut p.min_opacity)).on_hover_text(t(Key::PropMinOpacityTip));
        ui.end_row();
    });

    ui.add_space(6.0);
    egui::CollapsingHeader::new(t(Key::SectionAdvanced)).default_open(false).show(ui, |ui| {
        egui::Grid::new("brush-adv").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            ui.label(t(Key::PropDensity));
            fill_slider(ui, Slider::new(&mut p.density, 0.5..=12.0).max_decimals(1))
                .on_hover_text(t(Key::PropDensityTip));
            ui.end_row();
            ui.label(t(Key::PropBlending));
            fill_slider(ui, percent(&mut p.blending)).on_hover_text(t(Key::PropBlendingTip));
            ui.end_row();
            ui.label(t(Key::PropPersistence));
            fill_slider(ui, percent(&mut p.persistence)).on_hover_text(t(Key::PropPersistenceTip));
            ui.end_row();
            ui.label(t(Key::PropSizeJitter));
            fill_slider(ui, percent(&mut p.jitter));
            ui.end_row();
            ui.label(t(Key::PropEraser));
            ui.checkbox(&mut p.eraser, "");
            ui.end_row();
        });

        section(ui, t(Key::SectionStartEnd));
        egui::Grid::new("brush-shape").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            ui.label(t(Key::PropTaperIn));
            fill_slider(ui, Slider::new(&mut p.taper_in, 0.0..=500.0).suffix(" px").max_decimals(0))
                .on_hover_text(t(Key::PropTaperInTip));
            ui.end_row();
            ui.label(t(Key::PropTaperOut));
            fill_slider(ui, Slider::new(&mut p.taper_out, 0.0..=500.0).suffix(" px").max_decimals(0))
                .on_hover_text(t(Key::PropTaperOutTip));
            ui.end_row();
            ui.label(t(Key::PropPostCorrection));
            fill_slider(ui, Slider::new(&mut p.post_correction, 0..=arty_brush::shape::MAX_CORRECTION))
                .on_hover_text(t(Key::PropPostCorrectionTip));
            ui.end_row();
        });
    });

    if p != *studio.preset() {
        *studio.preset_mut() = p;
    }

    frame_border(ui, studio, shell);
    ui.add_space(6.0);
    super::pen_settings::ui(ui, studio, shell);
}

/// Simple mode's bar under the canvas: size, opacity and stability of the
/// current brush, disabled for tools that don't paint with one.
pub fn simple_sliders(ui: &mut egui::Ui, studio: &mut Studio) {
    let brush = matches!(studio.tool, Tool::Brush(_)) && studio.transform.is_none();
    let p = studio.preset();
    let (mut size, mut opacity, mut stabilizer) = (p.size, p.opacity, p.stabilizer);
    ui.add_enabled_ui(brush, |ui| {
        ui.horizontal(|ui| {
            // Three equal groups, each label + slider, up to a comfortable length.
            let sep = ui.spacing().item_spacing.x * 2.0 + 1.0;
            let w = ((ui.available_width() - 2.0 * sep) / 3.0).min(theme::SIMPLE_GROUP_MAX);
            let h = ui.spacing().interact_size.y;
            let group = |ui: &mut egui::Ui, label: Key, slider: Slider<'_>| {
                ui.allocate_ui_with_layout(egui::vec2(w, h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.label(t(label));
                    fill_slider(ui, slider);
                });
            };
            group(ui, Key::PropSize, Slider::new(&mut size, MIN_BRUSH_SIZE..=MAX_BRUSH_SIZE).logarithmic(true).suffix(" px").max_decimals(1));
            ui.separator();
            group(ui, Key::PropOpacity, percent(&mut opacity));
            ui.separator();
            group(ui, Key::PropStability, Slider::new(&mut stabilizer, 0..=Stabilizer::MAX_LEVEL));
        });
    });
    let p = studio.preset();
    if (size, opacity, stabilizer) != (p.size, p.opacity, p.stabilizer) {
        let p = studio.preset_mut();
        (p.size, p.opacity, p.stabilizer) = (size, opacity, stabilizer);
    }
}

/// The active layer's border settings when it is a frame folder (the frame
/// tools show them with their own options).
fn frame_border(ui: &mut egui::Ui, studio: &mut Studio, shell: &mut Shell) {
    let id = studio.doc.active();
    if !matches!(studio.tool, Tool::Frame(_)) && studio.doc.frame(id).is_some() {
        tools::frame::border_ui(ui, studio, shell, id);
    }
}

pub(super) fn percent(v: &mut f32) -> Slider<'_> {
    Slider::new(v, 0.0..=1.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)).custom_parser(|s| {
        s.trim_end_matches('%').trim().parse::<f64>().ok().map(|v| v / 100.0)
    })
}
