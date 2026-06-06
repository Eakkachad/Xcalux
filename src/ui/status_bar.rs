use crate::app::{PaintApp, ToolId};
use crate::tools::fill;

pub fn draw_status_bar(app: &mut PaintApp, ctx: &egui::Context) {
    egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            let tool_name = app.active_tool.name();
            ui.label(format!("Tool: {}", tool_name));
            ui.separator();

            if matches!(app.active_tool, ToolId::Brush | ToolId::Eraser) {
                let px_radius = app.brush_radius_log.exp();
                ui.label(format!("Size: {:.1}px", px_radius));
                ui.separator();

                let pct = (app.brush_opacity * 100.0).round();
                ui.label(format!("Opacity: {:.0}%", pct));
                ui.separator();
            }

            if matches!(app.active_tool, ToolId::Fill | ToolId::MagicWand) {
                let ref_text = match app.fill_options.reference {
                    fill::FillReference::CurrentLayer => "Current Layer",
                    fill::FillReference::SelectionSourceLayers => "Reference Layers",
                    fill::FillReference::AllVisibleLayers => "All Visible",
                };
                ui.label(format!("Ref: {}", ref_text));
                ui.separator();

                let mode_text = match app.fill_options.detection_mode {
                    fill::FillDetectionMode::TransparencyStrict => {
                        "Transparency Strict".to_string()
                    }
                    fill::FillDetectionMode::TransparencyFuzzy => {
                        format!("Transp Fuzzy ({})", app.fill_options.transp_diff)
                    }
                    fill::FillDetectionMode::ColorDifference => {
                        format!("Color Diff ({})", app.fill_options.tolerance)
                    }
                };
                ui.label(format!("Mode: {}", mode_text));
                ui.separator();

                ui.label(format!("Expand: {}px", app.fill_options.expand_px));
                ui.separator();
            }

            let pressure = app.last_ptr_pressure;
            ui.label(format!("Pressure: {:.2}", pressure));
            ui.separator();

            ui.label(format!(
                "Canvas: {}x{}",
                app.canvas_width, app.canvas_height
            ));
            ui.separator();

            ui.label(format!("Zoom: {:.1}%", app.viewport_zoom * 100.0));
            ui.separator();

            let angle_deg = app.rotation_angle.to_degrees().round();
            ui.label(format!("Rot: {:.0}\u{b0}", angle_deg));
            ui.separator();

            let mirror_state = if app.mirror_horizontal {
                "Mirror: On"
            } else {
                "Mirror: Off"
            };
            ui.label(mirror_state);
            ui.separator();

            let layer_name = app
                .layers
                .get(&app.active_layer_id)
                .map(|l| l.name.as_str())
                .unwrap_or("(none)");
            ui.label(format!("Layer: {}", layer_name));
            ui.separator();

            ui.label(&app.autosave_status);
        });
    });
}
