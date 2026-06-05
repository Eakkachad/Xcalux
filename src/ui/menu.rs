use crate::app::PaintApp;
use crate::commands::CommandId;
use crate::input::{StabilizerLevel, StabilizerMode};

pub fn draw_menu_bar(app: &mut PaintApp, ctx: &egui::Context) {
    egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
        egui::menu::bar(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("New Canvas").clicked() {
                    app.show_new_canvas_dialog = true;
                    ui.close_menu();
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Path:");
                    ui.text_edit_singleline(&mut app.document_path);
                });
                if ui.button("Open Canvas").clicked() {
                    let path = std::path::PathBuf::from(&app.document_path);
                    match crate::save::load_document(&path) {
                        Ok(loaded_doc) => {
                            app.load_from_document(loaded_doc);
                            log::info!("Loaded document successfully from {:?}", path);
                        }
                        Err(e) => {
                            log::error!("Failed to load document: {:?}", e);
                        }
                    }
                    ui.close_menu();
                }
                if ui.button("Save Canvas").clicked() {
                    app.save_canvas(std::path::Path::new(&app.document_path));
                    app.document_modified = false;
                    ui.close_menu();
                }
                ui.separator();
                ui.menu_button("Export", |ui| {
                    if ui.button("Export PNG...").clicked() {
                        app.show_export_png_dialog = true;
                        ui.close_menu();
                    }
                    if ui.button("Export OpenRaster (.ora)...").clicked() {
                        app.show_export_ora_dialog = true;
                        ui.close_menu();
                    }
                });
                ui.separator();
                ui.menu_button("Import", |ui| {
                    if ui.button("Import OpenRaster (.ora)...").clicked() {
                        app.show_import_ora_dialog = true;
                        ui.close_menu();
                    }
                });
                ui.separator();
                if ui.button("Exit").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });

            ui.menu_button("Edit", |ui| {
                if ui
                    .add_enabled(
                        !app.history.undo_stack.is_empty(),
                        egui::Button::new("Undo (Ctrl+Z)"),
                    )
                    .clicked()
                {
                    app.history.undo(&mut app.layers, &mut app.layer_order, &mut app.selection_mask, &mut app.active_layer_id);
                    ui.close_menu();
                }
                if ui
                    .add_enabled(
                        !app.history.redo_stack.is_empty(),
                        egui::Button::new("Redo (Ctrl+Y)"),
                    )
                    .clicked()
                {
                    app.history.redo(&mut app.layers, &mut app.layer_order, &mut app.selection_mask, &mut app.active_layer_id);
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Select All (Ctrl+A)").clicked() {
                    app.command(CommandId::SelectAll);
                    ui.close_menu();
                }
                if ui.button("Deselect (Ctrl+D)").clicked() {
                    app.command(CommandId::Deselect);
                    ui.close_menu();
                }
                if ui.button("Invert Selection (Ctrl+I)").clicked() {
                    app.command(CommandId::InvertSelection);
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Clear (Delete)").clicked() {
                    app.command(CommandId::Clear);
                    ui.close_menu();
                }
                if ui.button("Fill (Alt+Backspace)").clicked() {
                    app.command(CommandId::Fill);
                    ui.close_menu();
                }
            });

            ui.menu_button("Layer", |ui| {
                if ui.button("New Raster Layer").clicked() {
                    app.command(CommandId::NewRasterLayer);
                    ui.close_menu();
                }
                if ui.button("New Folder").clicked() {
                    app.command(CommandId::NewFolder);
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Duplicate Layer").clicked() {
                    app.command(CommandId::DuplicateLayer);
                    ui.close_menu();
                }
                if ui
                    .add_enabled(
                        app.layer_order.len() > 1,
                        egui::Button::new("Delete Layer"),
                    )
                    .clicked()
                {
                    app.command(CommandId::DeleteLayer);
                    ui.close_menu();
                }
                ui.separator();
                if ui
                    .add_enabled(
                        app.layer_order.len() > 1,
                        egui::Button::new("Merge Down"),
                    )
                    .clicked()
                {
                    app.command(CommandId::MergeDown);
                    ui.close_menu();
                }
                if ui.button("Merge Visible").clicked() {
                    app.command(CommandId::MergeVisible);
                    ui.close_menu();
                }
                if ui.button("Flatten Image").clicked() {
                    app.command(CommandId::FlattenImage);
                    ui.close_menu();
                }
                ui.separator();
                ui.menu_button("Layer Mask", |ui| {
                    let has_mask = app.layers.get(&app.active_layer_id).is_some_and(|l| l.mask.is_some());
                    if ui.add_enabled(!has_mask, egui::Button::new("Add Mask")).clicked() {
                        app.command(CommandId::AddLayerMask);
                        ui.close_menu();
                    }
                    if ui.add_enabled(has_mask, egui::Button::new("Delete Mask")).clicked() {
                        app.command(CommandId::DeleteLayerMask);
                        ui.close_menu();
                    }
                    if ui.add_enabled(has_mask, egui::Button::new("Apply Mask")).clicked() {
                        app.command(CommandId::ApplyLayerMask);
                        ui.close_menu();
                    }
                    if ui.add_enabled(has_mask, egui::Button::new("Invert Mask")).clicked() {
                        app.command(CommandId::InvertLayerMask);
                        ui.close_menu();
                    }
                    if ui.add_enabled(has_mask, egui::Button::new("Toggle Mask")).clicked() {
                        app.command(CommandId::ToggleLayerMask);
                        ui.close_menu();
                    }
                });
            });

            ui.menu_button("Canvas", |ui| {
                if ui.button("Fit to Screen").clicked() {
                    app.command(CommandId::FitToScreen);
                    ui.close_menu();
                }
                if ui.button("Actual Size (100%)").clicked() {
                    app.command(CommandId::ActualSize);
                    ui.close_menu();
                }
                if ui.button("Reset View").clicked() {
                    app.command(CommandId::ResetView);
                    ui.close_menu();
                }

                ui.separator();
                ui.label("Canvas Size:");
                ui.horizontal(|ui| {
                    ui.label("W:");
                    if ui.add(
                        egui::DragValue::new(&mut app.canvas_width)
                            .clamp_range(256..=4096)
                            .suffix("px"),
                    ).changed() {
                        if let Some(r) = &mut app.renderer {
                            r.clear_cache();
                        }
                    }
                    ui.label("H:");
                    if ui.add(
                        egui::DragValue::new(&mut app.canvas_height)
                            .clamp_range(256..=4096)
                            .suffix("px"),
                    ).changed() {
                        if let Some(r) = &mut app.renderer {
                            r.clear_cache();
                        }
                    }
                });

                egui::ComboBox::from_id_source("canvas_preset_menu")
                    .selected_text(format!("Preset: {}x{}", app.canvas_width, app.canvas_height))
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(
                            app.canvas_width == 1024 && app.canvas_height == 1024,
                            "Square (1024x1024)",
                        ).clicked() {
                            app.canvas_width = 1024;
                            app.canvas_height = 1024;
                            if let Some(r) = &mut app.renderer {
                                r.clear_cache();
                            }
                        }
                        if ui.selectable_label(
                            app.canvas_width == 1920 && app.canvas_height == 1080,
                            "FullHD (1920x1080)",
                        ).clicked() {
                            app.canvas_width = 1920;
                            app.canvas_height = 1080;
                            if let Some(r) = &mut app.renderer {
                                r.clear_cache();
                            }
                        }
                        if ui.selectable_label(
                            app.canvas_width == 2048 && app.canvas_height == 2048,
                            "2K Square (2048x2048)",
                        ).clicked() {
                            app.canvas_width = 2048;
                            app.canvas_height = 2048;
                            if let Some(r) = &mut app.renderer {
                                r.clear_cache();
                            }
                        }
                        if ui.selectable_label(
                            app.canvas_width == 2480 && app.canvas_height == 3508,
                            "A4 (2480x3508)",
                        ).clicked() {
                            app.canvas_width = 2480;
                            app.canvas_height = 3508;
                            if let Some(r) = &mut app.renderer {
                                r.clear_cache();
                            }
                        }
                    });
                ui.separator();
                if ui.button("Flip Canvas Horizontal").clicked() {
                    app.command(CommandId::FlipCanvasHorizontal);
                    ui.close_menu();
                }
                if ui.button("Flip Canvas Vertical").clicked() {
                    app.command(CommandId::FlipCanvasVertical);
                    ui.close_menu();
                }
                if ui.button("Trim Transparent Pixels").clicked() {
                    app.command(CommandId::TrimTransparent);
                    ui.close_menu();
                }
                ui.separator();
                let has_selection = app.selection_mask.is_active && !app.selection_mask.is_empty();
                if ui.add_enabled(has_selection, egui::Button::new("Crop to Selection")).clicked() {
                    app.command(CommandId::CropToSelection);
                    ui.close_menu();
                }
            });

            ui.menu_button("Selection", |ui| {
                if ui.button("Select All (Ctrl+A)").clicked() {
                    app.command(CommandId::SelectAll);
                    ui.close_menu();
                }
                if ui.button("Deselect (Ctrl+D)").clicked() {
                    app.command(CommandId::Deselect);
                    ui.close_menu();
                }
                if ui.button("Invert Selection (Ctrl+I)").clicked() {
                    app.command(CommandId::InvertSelection);
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Grow Selection...").clicked() {
                    app.command(CommandId::SelectionGrow);
                    ui.close_menu();
                }
                if ui.button("Shrink Selection...").clicked() {
                    app.command(CommandId::SelectionShrink);
                    ui.close_menu();
                }
                if ui.button("Feather Selection...").clicked() {
                    app.command(CommandId::SelectionFeather);
                    ui.close_menu();
                }
            });

            ui.menu_button("Filter", |ui| {
                if ui.button("Brightness/Contrast...").clicked() {
                    app.command(CommandId::AdjustBrightnessContrast);
                    ui.close_menu();
                }
                if ui.button("Hue/Saturation...").clicked() {
                    app.command(CommandId::AdjustHueSaturation);
                    ui.close_menu();
                }
                if ui.button("Gaussian Blur...").clicked() {
                    app.command(CommandId::FilterGaussianBlur);
                    ui.close_menu();
                }
            });

            ui.menu_button("View", |ui| {
                if ui.button("Show Grid").clicked() {
                    app.show_grid = !app.show_grid;
                    ui.close_menu();
                }
                if ui.button("Minimal UI (Tab)").clicked() {
                    app.show_minimal_ui = !app.show_minimal_ui;
                    ui.close_menu();
                }
                if ui.button("Toggle Fullscreen (F11)").clicked() {
                    app.command(CommandId::Fullscreen);
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Performance HUD (F12)").clicked() {
                    app.performance_hud.enabled = !app.performance_hud.enabled;
                    ui.close_menu();
                }
            });

            ui.menu_button("Window", |ui| {
                ui.checkbox(&mut app.show_navigator, "Navigator");
                ui.checkbox(&mut app.show_color_wheel, "Color Wheel");
                ui.checkbox(&mut app.show_rgb_sliders, "RGB Sliders");
                ui.checkbox(&mut app.show_hsv_sliders, "HSV Sliders");
                ui.checkbox(&mut app.show_color_palette, "Color Palette");
                ui.checkbox(&mut app.show_color_history, "Color History");
                ui.checkbox(&mut app.show_layers_manager, "Layers Manager");
                ui.checkbox(&mut app.show_reference_panel, "Reference Panel");
                ui.checkbox(&mut app.show_symmetry_panel, "Symmetry Panel");
                ui.checkbox(&mut app.show_tool_options, "Tool Options");
                ui.separator();
                ui.checkbox(&mut app.layer_panel_on_left, "Layer Panel on Left Side");
            });

            ui.menu_button("Help", |ui| {
                if ui.button("Keyboard Shortcuts").clicked() {
                    app.show_shortcut_editor = true;
                    ui.close_menu();
                }
                if ui.button("Tablet Diagnostics").clicked() {
                    app.tablet_diagnostics.enabled = !app.tablet_diagnostics.enabled;
                    ui.close_menu();
                }
                if ui.button("About ARTY").clicked() {
                    app.command(CommandId::About);
                    ui.close_menu();
                }
            });

            ui.separator();
            ui.label("Stabilizer:");
            let current_level = app.stabilizer.level;
            let text = match current_level {
                StabilizerLevel::Off => "Off".to_string(),
                StabilizerLevel::Level(val) => format!("Level {}", val),
                StabilizerLevel::SLevel(val) => format!("S-{}", val),
            };
            let response = egui::ComboBox::from_id_source("top_stabilizer_level")
                .selected_text(text)
                .width(90.0)
                .show_ui(ui, |ui| {
                    let mut selected = false;
                    if ui.selectable_label(matches!(current_level, StabilizerLevel::Off), "Off").clicked() {
                        app.stabilizer.set_level(StabilizerLevel::Off);
                        selected = true;
                    }
                    for val in 1..=15 {
                        let is_sel = match current_level {
                            StabilizerLevel::Level(v) => v == val,
                            _ => false,
                        };
                        if ui.selectable_label(is_sel, format!("Level {}", val)).clicked() {
                            app.stabilizer.set_level(StabilizerLevel::Level(val));
                            selected = true;
                        }
                    }
                    for val in 1..=5 {
                        let is_sel = match current_level {
                            StabilizerLevel::SLevel(v) => v == val,
                            _ => false,
                        };
                        if ui.selectable_label(is_sel, format!("S-{}", val)).clicked() {
                            app.stabilizer.set_level(StabilizerLevel::SLevel(val));
                            selected = true;
                        }
                    }
                    selected
                });
            if response.inner.unwrap_or(false) {
                ctx.request_repaint();
                app.brush_settings_dirty = true;
            }

            ui.label("Mode:");
            let current_mode = app.stabilizer.mode;
            let mode_text = match current_mode {
                StabilizerMode::Ema => "EMA",
                StabilizerMode::SpringMassDamper => "Spring Physics",
            };
            let response = egui::ComboBox::from_id_source("top_stabilizer_mode")
                .selected_text(mode_text)
                .width(120.0)
                .show_ui(ui, |ui| {
                    let mut selected = false;
                    if ui.selectable_label(current_mode == StabilizerMode::Ema, "EMA").clicked() {
                        app.stabilizer.mode = StabilizerMode::Ema;
                        selected = true;
                    }
                    if ui.selectable_label(current_mode == StabilizerMode::SpringMassDamper, "Spring Physics").clicked() {
                        app.stabilizer.mode = StabilizerMode::SpringMassDamper;
                        selected = true;
                    }
                    selected
                });
            if response.inner.unwrap_or(false) {
                ctx.request_repaint();
                app.brush_settings_dirty = true;
            }
        });
    });
}
