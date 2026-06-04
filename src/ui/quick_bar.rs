use crate::app::PaintApp;
use crate::commands::CommandId;
use crate::input::StabilizerLevel;

pub fn draw_quick_bar(app: &mut PaintApp, ctx: &egui::Context) {
    if app.quick_bar_visible && !app.show_minimal_ui {
        egui::TopBottomPanel::top("quick_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                // Group 1: File & History
                if ui.button("Undo").clicked() { app.command(CommandId::Undo); }
                if ui.button("Redo").clicked() { app.command(CommandId::Redo); }
                if ui.button("Save").clicked() { app.command(CommandId::Save); }
                ui.separator();

                // Group 2: Selection & Transform
                if ui.button("Select All").clicked() { app.command(CommandId::SelectAll); }
                if ui.button("Deselect").clicked() { app.command(CommandId::Deselect); }
                if ui.button("Invert").clicked() { app.command(CommandId::InvertSelection); }
                if ui.button("Transform").clicked() { app.command(CommandId::ToolTransform); }
                ui.separator();

                // Group 3: Edit Operations
                if ui.button("Cut").clicked() { app.command(CommandId::Clear); }
                if ui.button("Copy").clicked() { app.command(CommandId::Copy); }
                if ui.button("Paste").clicked() { app.command(CommandId::Paste); }
                if ui.button("Fill").clicked() { app.command(CommandId::Fill); }
                ui.separator();

                // Group 4: View Reset
                if ui.button("Fit").clicked() { app.command(CommandId::FitToScreen); }
                if ui.button("100%").clicked() { app.command(CommandId::ActualSize); }
                if ui.button("Reset").clicked() { app.command(CommandId::ResetView); }
                ui.separator();

                // Group 5: Zoom Controls
                ui.label("Zoom:");
                if ui.button("-").clicked() {
                    app.viewport_zoom = (app.viewport_zoom - 0.25).max(0.1);
                }
                let current_zoom = app.viewport_zoom;
                let zoom_pct = (current_zoom * 100.0).round();
                let zoom_changed = egui::ComboBox::from_id_source("quick_zoom")
                    .selected_text(format!("{:.0}%", zoom_pct))
                    .width(70.0)
                    .show_ui(ui, |ui| {
                        let mut changed = false;
                        if ui.selectable_label(zoom_pct == 25.0, "25%").clicked() { app.viewport_zoom = 0.25; changed = true; }
                        if ui.selectable_label(zoom_pct == 50.0, "50%").clicked() { app.viewport_zoom = 0.50; changed = true; }
                        if ui.selectable_label(zoom_pct == 75.0, "75%").clicked() { app.viewport_zoom = 0.75; changed = true; }
                        if ui.selectable_label(zoom_pct == 100.0, "100%").clicked() { app.viewport_zoom = 1.0; changed = true; }
                        if ui.selectable_label(zoom_pct == 150.0, "150%").clicked() { app.viewport_zoom = 1.50; changed = true; }
                        if ui.selectable_label(zoom_pct == 200.0, "200%").clicked() { app.viewport_zoom = 2.00; changed = true; }
                        if ui.selectable_label(zoom_pct == 300.0, "300%").clicked() { app.viewport_zoom = 3.00; changed = true; }
                        if ui.selectable_label(zoom_pct == 400.0, "400%").clicked() { app.viewport_zoom = 4.00; changed = true; }
                        if ui.selectable_label(zoom_pct == 800.0, "800%").clicked() { app.viewport_zoom = 8.00; changed = true; }
                        changed
                    }).inner.unwrap_or(false);
                if zoom_changed {
                    ctx.request_repaint();
                }
                if ui.button("+").clicked() {
                    app.viewport_zoom = (app.viewport_zoom + 0.25).min(10.0);
                }
                ui.separator();

                // Group 6: Rotation Controls
                if ui.button("-15°").clicked() {
                    app.rotation_angle = (app.rotation_angle - 15.0f32.to_radians()).rem_euclid(2.0 * std::f32::consts::PI);
                }
                let rot_text = format!("{:.0}°", app.rotation_angle.to_degrees());
                if ui.button(rot_text).on_hover_text("Click to reset rotation to 0°").clicked() {
                    app.rotation_angle = 0.0;
                }
                if ui.button("+15°").clicked() {
                    app.rotation_angle = (app.rotation_angle + 15.0f32.to_radians()).rem_euclid(2.0 * std::f32::consts::PI);
                }
                ui.separator();

                // Group 7: Mirror & Stabilizer
                let mirror_label = if app.mirror_horizontal { "Mirror: On" } else { "Mirror: Off" };
                if ui.button(mirror_label).clicked() {
                    app.mirror_horizontal = !app.mirror_horizontal;
                }
                ui.separator();
                ui.label("Stab:");
                let current_level = app.stabilizer.level;
                let text = match current_level {
                    StabilizerLevel::Off => "Off".to_string(),
                    StabilizerLevel::Level(val) => format!("L{}", val),
                    StabilizerLevel::SLevel(val) => format!("S-{}", val),
                };
                let stab_changed = egui::ComboBox::from_id_source("quick_bar_stab")
                    .selected_text(text)
                    .width(60.0)
                    .show_ui(ui, |ui| {
                        let mut changed = false;
                        if ui.selectable_label(matches!(current_level, StabilizerLevel::Off), "Off").clicked() {
                            app.stabilizer.set_level(StabilizerLevel::Off);
                            changed = true;
                        }
                        for val in 1..=15 {
                            let is_sel = matches!(current_level, StabilizerLevel::Level(v) if v == val);
                            if ui.selectable_label(is_sel, format!("L{}", val)).clicked() {
                                app.stabilizer.set_level(StabilizerLevel::Level(val));
                                changed = true;
                            }
                        }
                        for val in 1..=5 {
                            let is_sel = matches!(current_level, StabilizerLevel::SLevel(v) if v == val);
                            if ui.selectable_label(is_sel, format!("S-{}", val)).clicked() {
                                app.stabilizer.set_level(StabilizerLevel::SLevel(val));
                                changed = true;
                            }
                        }
                        changed
                    }).inner.unwrap_or(false);
                if stab_changed {
                    app.brush_settings_dirty = true;
                }
                ui.separator();

                ui.label("Stab Mode:");
                let current_mode = app.stabilizer.mode;
                let mode_text = match current_mode {
                    crate::input::StabilizerMode::Ema => "EMA",
                    crate::input::StabilizerMode::SpringMassDamper => "Spring Physics",
                };
                let mode_changed = egui::ComboBox::from_id_source("quick_bar_stab_mode")
                    .selected_text(mode_text)
                    .width(100.0)
                    .show_ui(ui, |ui| {
                        let mut changed = false;
                        if ui.selectable_label(current_mode == crate::input::StabilizerMode::Ema, "EMA").clicked() {
                            app.stabilizer.mode = crate::input::StabilizerMode::Ema;
                            changed = true;
                        }
                        if ui.selectable_label(current_mode == crate::input::StabilizerMode::SpringMassDamper, "Spring Physics").clicked() {
                            app.stabilizer.mode = crate::input::StabilizerMode::SpringMassDamper;
                            changed = true;
                        }
                        changed
                    }).inner.unwrap_or(false);
                if mode_changed {
                    app.brush_settings_dirty = true;
                }

                ui.separator();
                if !app.autosave_status.is_empty() {
                    ui.label(&app.autosave_status);
                }
            });
        });
    }
}
