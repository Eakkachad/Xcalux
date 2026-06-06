use crate::app::PaintApp;
use crate::canvas::Layer;
use crate::history::HistoryCommand;
use crate::tools::selection;

pub fn draw_dialogs(app: &mut PaintApp, ctx: &egui::Context) {
    // Panel Layout Settings
    if app.show_panel_layout_settings {
        draw_panel_layout_settings(app, ctx);
    }
    // 0. AUTOSAVE RECOVERY DIALOG
    if app.show_recovery_dialog && !app.recovery_files.is_empty() {
        let mut close = false;
        let mut recover_file: Option<String> = None;
        egui::Window::new("Autosave Recovery")
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("Recover unsaved work?").strong());
                });
                ui.add_space(4.0);
                ui.label("The following autosave files were found from previous sessions:");
                ui.add_space(4.0);
                for file in &app.recovery_files.clone() {
                    ui.horizontal(|ui| {
                        if ui.button("Recover").clicked() {
                            recover_file = Some(file.clone());
                        }
                        ui.label(file);
                    });
                }
                ui.add_space(8.0);
                ui.label("Tip: recover a file, then Save As to keep it.");
                ui.add_space(4.0);
                if ui.button("Discard All").clicked() {
                    for file in &app.recovery_files {
                        let _ = std::fs::remove_file(file);
                    }
                    app.recovery_files.clear();
                    close = true;
                }
            });
        if let Some(file) = recover_file {
            let file_for_retain = file.clone();
            let path = std::path::PathBuf::from(&file);
            match crate::save::load_document(&path) {
                Ok(doc) => {
                    app.load_from_document(doc);
                    log::info!("Recovered from autosave: {:?}", path);
                    app.document_path = file;
                    app.autosave_status = "Recovered from autosave".to_string();
                }
                Err(e) => {
                    log::error!("Failed to recover autosave: {:?}", e);
                    app.autosave_status = "Autosave recovery failed".to_string();
                }
            }
            app.recovery_files.retain(|f| f != &file_for_retain);
            if app.recovery_files.is_empty() {
                close = true;
            }
        }
        if close {
            app.show_recovery_dialog = false;
        }
    }

    // 1. NEW CANVAS DIALOG OVERLAY
    if app.show_new_canvas_dialog {
        let mut close_dialog = false;
        let mut create_canvas = false;

        egui::Window::new("New Canvas")
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("Choose the canvas size").strong());
                    ui.add_space(8.0);
                });

                egui::Grid::new("new_canvas_grid")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("Width:");
                        ui.add(
                            egui::DragValue::new(&mut app.new_canvas_width)
                                .clamp_range(256..=4096)
                                .suffix(" px"),
                        );
                        ui.end_row();

                        ui.label("Height:");
                        ui.add(
                            egui::DragValue::new(&mut app.new_canvas_height)
                                .clamp_range(256..=4096)
                                .suffix(" px"),
                        );
                        ui.end_row();
                    });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);

                ui.label("Presets:");
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Square (1024x1024)").clicked() {
                        app.new_canvas_width = 1024;
                        app.new_canvas_height = 1024;
                    }
                    if ui.button("FullHD (1920x1080)").clicked() {
                        app.new_canvas_width = 1920;
                        app.new_canvas_height = 1080;
                    }
                    if ui.button("2K Square (2048x2048)").clicked() {
                        app.new_canvas_width = 2048;
                        app.new_canvas_height = 2048;
                    }
                    if ui.button("A4 Paper (2480x3508)").clicked() {
                        app.new_canvas_width = 2480;
                        app.new_canvas_height = 3508;
                    }
                });

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new("Create").min_size(egui::Vec2::new(100.0, 30.0)))
                        .clicked()
                    {
                        create_canvas = true;
                    }
                    if ui
                        .add(egui::Button::new("Cancel").min_size(egui::Vec2::new(100.0, 30.0)))
                        .clicked()
                    {
                        close_dialog = true;
                    }
                });
            });

        if create_canvas {
            app.cleanup_autosave();
            app.canvas_width = app.new_canvas_width;
            app.canvas_height = app.new_canvas_height;

            app.layers.clear();
            app.layers.insert(1, Layer::new(1, "Layer 1".to_string()));
            app.layer_order = vec![1];
            app.layer_id_counter = 1;
            app.active_layer_id = 1;
            app.history.undo_stack.clear();
            app.history.redo_stack.clear();

            // Centering view on create
            app.viewport_offset = egui::Vec2::ZERO;
            app.viewport_zoom = 1.0;

            if let Some(r) = &mut app.renderer {
                r.clear_cache();
            }
            app.show_new_canvas_dialog = false;
        } else if close_dialog {
            app.show_new_canvas_dialog = false;
        }
    }

    // 2. EXPORT PNG DIALOG
    if app.show_export_png_dialog {
        let mut close = false;
        let mut do_export = false;
        egui::Window::new("Export PNG")
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("Export Canvas as PNG").strong());
                });
                ui.add_space(8.0);
                egui::Grid::new("export_grid")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("File path:");
                        ui.text_edit_singleline(&mut app.export_png_path);
                        ui.end_row();
                        ui.label("Background:");
                        let mut bg_val = match app.export_png_options.background {
                            crate::export::png::ExportBackground::Transparent => 0,
                            crate::export::png::ExportBackground::White => 1,
                        };
                        egui::ComboBox::from_id_source("export_bg")
                            .selected_text(if bg_val == 0 { "Transparent" } else { "White" })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut bg_val, 0, "Transparent").changed();
                                if ui.selectable_value(&mut bg_val, 1, "White").changed() {}
                            });
                        app.export_png_options.background = if bg_val == 0 {
                            crate::export::png::ExportBackground::Transparent
                        } else {
                            crate::export::png::ExportBackground::White
                        };
                        ui.end_row();
                        ui.label("Scale:");
                        ui.add(
                            egui::Slider::new(&mut app.export_png_options.scale, 0.1..=4.0)
                                .text("x"),
                        );
                        ui.end_row();
                    });
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Export").clicked() {
                        do_export = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
        if do_export {
            let path = std::path::Path::new(&app.export_png_path).to_path_buf();
            let layers = app.layers.clone();
            let layer_order = app.layer_order.clone();
            let w = app.canvas_width;
            let h = app.canvas_height;
            let options = app.export_png_options.clone();
            std::thread::spawn(move || {
                match crate::export::png::export_png(&path, &layers, &layer_order, w, h, &options) {
                    Ok(()) => log::info!("Exported PNG to {:?}", path),
                    Err(e) => log::error!("PNG export failed: {:?}", e),
                }
            });
            app.show_export_png_dialog = false;
        }
        if close {
            app.show_export_png_dialog = false;
        }
    }

    // 2b. EXPORT ORA DIALOG
    if app.show_export_ora_dialog {
        let mut close = false;
        let mut do_export = false;
        egui::Window::new("Export OpenRaster")
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("Export Canvas as OpenRaster").strong());
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label("File path:");
                    ui.text_edit_singleline(&mut app.export_ora_path);
                });
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "ORA preserves layers, opacity, blend modes, and visibility.",
                    )
                    .weak()
                    .small(),
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Export").clicked() {
                        do_export = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
        if do_export {
            let path = std::path::Path::new(&app.export_ora_path).to_path_buf();
            let layers = app.layers.clone();
            let layer_order = app.layer_order.clone();
            let w = app.canvas_width;
            let h = app.canvas_height;
            std::thread::spawn(move || {
                match crate::export::ora::export_ora(&path, &layers, &layer_order, w, h) {
                    Ok(()) => log::info!("Exported ORA to {:?}", path),
                    Err(e) => log::error!("ORA export failed: {:?}", e),
                }
            });
            app.show_export_ora_dialog = false;
        }
        if close {
            app.show_export_ora_dialog = false;
        }
    }

    // 2c. IMPORT ORA DIALOG
    if app.show_import_ora_dialog {
        let mut close = false;
        let mut do_import = false;
        egui::Window::new("Import OpenRaster")
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("Import OpenRaster File").strong());
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label("File path:");
                    ui.text_edit_singleline(&mut app.import_ora_path);
                });
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("Warning: Importing will replace all existing layers.")
                        .weak()
                        .small(),
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Import").clicked() {
                        do_import = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
        if do_import {
            let path = std::path::Path::new(&app.import_ora_path).to_path_buf();
            match crate::export::ora::import_ora(&path) {
                Ok(imported) => {
                    app.canvas_width = imported.width;
                    app.canvas_height = imported.height;
                    crate::export::ora::apply_imported_canvas(
                        imported,
                        &mut app.layers,
                        &mut app.layer_order,
                        &mut app.layer_id_counter,
                        &mut app.active_layer_id,
                    );
                    log::info!("Imported ORA from {:?}", path);
                }
                Err(e) => {
                    log::error!("ORA import failed: {:?}", e);
                }
            }
            app.show_import_ora_dialog = false;
        }
        if close {
            app.show_import_ora_dialog = false;
        }
    }

    // 2d. ABOUT DIALOG
    if app.show_about_dialog {
        let mut close = false;
        egui::Window::new("About ARTY")
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("ARTY").heading().strong());
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("Xcalux Digital Painting Workstation").weak());
                    ui.add_space(8.0);
                    ui.label("A digital painting application inspired by Paint Tool SAI");
                    ui.add_space(4.0);
                    ui.label("Built with Rust + egui + WGPU");
                    ui.add_space(8.0);
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });
        if close {
            app.show_about_dialog = false;
        }
    }

    // 3. KEYBOARD SHORTCUT EDITOR
    if app.show_shortcut_editor {
        egui::Window::new("Keyboard Shortcuts")
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .resizable(true)
            .default_width(550.0)
            .default_height(400.0)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Search:");
                    ui.text_edit_singleline(&mut app.shortcut_search);
                });
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);

                let mut close = false;
                let mut clicked_idx = None;

                // Capture keyboard input when listening
                if app.shortcut_listen_idx.is_some() {
                    ui.add_enabled(false, egui::Button::new("Press a key... (Esc to cancel)"));
                    let captured = ctx.input(|i| {
                        for event in &i.events {
                            if let egui::Event::Key {
                                key,
                                pressed: true,
                                modifiers,
                                ..
                            } = event
                            {
                                let captured_idx = app.shortcut_listen_idx;
                                if let Some(idx) = captured_idx {
                                    if *key != egui::Key::Escape {
                                        return Some((
                                            idx,
                                            crate::shortcuts::KeyBinding::from_event(
                                                *key,
                                                modifiers.ctrl,
                                                modifiers.shift,
                                                modifiers.alt,
                                            ),
                                        ));
                                    }
                                }
                                return None; // Escape cancels
                            }
                        }
                        None
                    });
                    if let Some((idx, binding)) = captured {
                        if idx < app.shortcuts.entries.len() {
                            app.shortcuts.entries[idx].primary = Some(binding);
                        }
                        app.shortcut_listen_idx = None;
                    }
                    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                        app.shortcut_listen_idx = None;
                    }
                    // Don't render the list while listening
                    return;
                }

                egui::ScrollArea::vertical().show(ui, |ui| {
                    let search_lower = app.shortcut_search.to_lowercase();
                    for (entry_idx, entry) in app.shortcuts.entries.iter().enumerate() {
                        let name_lower = entry.name.to_lowercase();
                        let cat_lower = entry.category.to_lowercase();
                        if !search_lower.is_empty()
                            && !name_lower.contains(&search_lower)
                            && !cat_lower.contains(&search_lower)
                        {
                            continue;
                        }

                        let is_editing = app.shortcut_edit_idx == Some(entry_idx);

                        ui.horizontal(|ui| {
                            ui.label(entry.name);
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if is_editing {
                                        if let Some(ref binding) = entry.primary {
                                            if ui.button(binding.display()).clicked() {
                                                app.shortcut_listen_idx = Some(entry_idx);
                                            }
                                        } else {
                                            if ui.button("[none]").clicked() {
                                                app.shortcut_listen_idx = Some(entry_idx);
                                            }
                                        }
                                        if ui.button("Clear").clicked() {
                                            app.shortcut_edit_idx = None;
                                        }
                                    } else {
                                        if let Some(ref binding) = entry.primary {
                                            ui.label(binding.display());
                                        } else {
                                            ui.label("[none]");
                                        }
                                        if ui.button("Edit").clicked() {
                                            clicked_idx = Some(entry_idx);
                                        }
                                    }
                                },
                            );
                        });
                        ui.separator();
                    }
                });

                if let Some(idx) = clicked_idx {
                    app.shortcut_edit_idx = Some(idx);
                }

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Reset to Defaults").clicked() {
                        app.shortcuts = crate::shortcuts::ShortcutManager::new();
                        app.shortcut_edit_idx = None;
                        app.shortcut_listen_idx = None;
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });

                if close {
                    app.show_shortcut_editor = false;
                    app.shortcut_edit_idx = None;
                    app.shortcut_listen_idx = None;
                }
            });
    }

    // 4. GROW SELECTION DIALOG
    if app.show_grow_dialog {
        let mut close = false;
        let mut grow_cmd: Option<HistoryCommand> = None;
        egui::Window::new("Grow Selection")
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Grow selection by:");
                    ui.add(egui::DragValue::new(&mut app.grow_pixels).clamp_range(1..=100));
                    ui.label("pixels");
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Grow").clicked() {
                        let old_mask = Box::new(app.selection_mask.clone());
                        let grow_px = app.grow_pixels;
                        selection::grow_selection(
                            &mut app.selection_mask,
                            grow_px,
                            app.canvas_width as i32,
                            app.canvas_height as i32,
                        );
                        app.show_selection_overlay = app.selection_mask.is_active;
                        let new_mask = Box::new(app.selection_mask.clone());
                        grow_cmd = Some(HistoryCommand::SelectionChange { old_mask, new_mask });
                        close = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
        if let Some(cmd) = grow_cmd {
            app.history.push_command(cmd);
        }
        if close {
            app.show_grow_dialog = false;
        }
    }

    // 5. SHRINK SELECTION DIALOG
    if app.show_shrink_dialog {
        let mut close = false;
        let mut shrink_cmd: Option<HistoryCommand> = None;
        egui::Window::new("Shrink Selection")
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Shrink selection by:");
                    ui.add(egui::DragValue::new(&mut app.shrink_pixels).clamp_range(1..=100));
                    ui.label("pixels");
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Shrink").clicked() {
                        let old_mask = Box::new(app.selection_mask.clone());
                        let shrink_px = app.shrink_pixels;
                        selection::shrink_selection(
                            &mut app.selection_mask,
                            shrink_px,
                            app.canvas_width as i32,
                            app.canvas_height as i32,
                        );
                        app.show_selection_overlay = app.selection_mask.is_active;
                        let new_mask = Box::new(app.selection_mask.clone());
                        shrink_cmd = Some(HistoryCommand::SelectionChange { old_mask, new_mask });
                        close = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
        if let Some(cmd) = shrink_cmd {
            app.history.push_command(cmd);
        }
        if close {
            app.show_shrink_dialog = false;
        }
    }

    // 6. FEATHER SELECTION DIALOG
    if app.show_feather_dialog {
        let mut close = false;
        let mut feather_cmd: Option<HistoryCommand> = None;
        egui::Window::new("Feather Selection")
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Feather radius:");
                    ui.add(egui::DragValue::new(&mut app.feather_pixels).clamp_range(1..=100));
                    ui.label("pixels");
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Feather").clicked() {
                        let old_mask = Box::new(app.selection_mask.clone());
                        let feather_px = app.feather_pixels;
                        selection::feather_selection(
                            &mut app.selection_mask,
                            feather_px,
                            app.canvas_width as i32,
                            app.canvas_height as i32,
                        );
                        app.show_selection_overlay = app.selection_mask.is_active;
                        let new_mask = Box::new(app.selection_mask.clone());
                        feather_cmd = Some(HistoryCommand::SelectionChange { old_mask, new_mask });
                        close = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
        if let Some(cmd) = feather_cmd {
            app.history.push_command(cmd);
        }
        if close {
            app.show_feather_dialog = false;
        }
    }

    // 6.5 PRESSURE CALIBRATION DIALOG
    if app.show_pressure_calibration {
        let mut close = false;
        egui::Window::new("Pressure Calibration")
            .collapsible(false)
            .resizable(true)
            .default_size([400.0, 300.0])
            .show(ctx, |ui| {
                ui.label("Adjust the curve to map raw pressure (X) to calibrated pressure (Y).");
                ui.add_space(8.0);

                let plot_size = egui::Vec2::splat(200.0);
                let (plot_rect, plot_response) =
                    ui.allocate_exact_size(plot_size, egui::Sense::click_and_drag());

                ui.painter()
                    .rect_filled(plot_rect, 0.0, egui::Color32::from_gray(40));
                ui.painter().rect_stroke(
                    plot_rect,
                    1.0,
                    egui::Stroke::new(1.0, egui::Color32::from_gray(100)),
                );

                // Draw grid
                for i in 1..4 {
                    let x = plot_rect.left() + plot_rect.width() * (i as f32 / 4.0);
                    ui.painter().line_segment(
                        [
                            egui::Pos2::new(x, plot_rect.top()),
                            egui::Pos2::new(x, plot_rect.bottom()),
                        ],
                        egui::Stroke::new(0.5, egui::Color32::from_gray(80)),
                    );
                    let y = plot_rect.top() + plot_rect.height() * (i as f32 / 4.0);
                    ui.painter().line_segment(
                        [
                            egui::Pos2::new(plot_rect.left(), y),
                            egui::Pos2::new(plot_rect.right(), y),
                        ],
                        egui::Stroke::new(0.5, egui::Color32::from_gray(80)),
                    );
                }

                // Draw curve
                let mut sorted = app.pressure_response.points.clone();
                sorted.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
                for window in sorted.windows(2) {
                    let p0 = egui::Pos2::new(
                        plot_rect.left() + window[0].0 * plot_rect.width(),
                        plot_rect.bottom() - window[0].1 * plot_rect.height(),
                    );
                    let p1 = egui::Pos2::new(
                        plot_rect.left() + window[1].0 * plot_rect.width(),
                        plot_rect.bottom() - window[1].1 * plot_rect.height(),
                    );
                    ui.painter().line_segment(
                        [p0, p1],
                        egui::Stroke::new(2.0, egui::Color32::from_rgb(100, 200, 255)),
                    );
                }

                // Draw and handle control points
                let mut points = app.pressure_response.points.clone();
                for i in 0..points.len() {
                    let screen_pos = egui::Pos2::new(
                        plot_rect.left() + points[i].0 * plot_rect.width(),
                        plot_rect.bottom() - points[i].1 * plot_rect.height(),
                    );
                    let handle_size = 8.0;
                    let handle_rect =
                        egui::Rect::from_center_size(screen_pos, egui::Vec2::splat(handle_size));
                    let handle_resp = ui.allocate_rect(handle_rect, egui::Sense::drag());

                    let color = if i == 0 || i == points.len() - 1 {
                        egui::Color32::from_rgb(255, 200, 100)
                    } else {
                        egui::Color32::from_rgb(100, 255, 100)
                    };
                    ui.painter()
                        .circle_filled(screen_pos, handle_size * 0.5, color);
                    ui.painter().circle_stroke(
                        screen_pos,
                        handle_size * 0.5,
                        egui::Stroke::new(1.0, egui::Color32::WHITE),
                    );

                    if handle_resp.dragged() {
                        if let Some(pos) = plot_response.interact_pointer_pos() {
                            let nx =
                                ((pos.x - plot_rect.left()) / plot_rect.width()).clamp(0.0, 1.0);
                            let ny = 1.0
                                - ((pos.y - plot_rect.top()) / plot_rect.height()).clamp(0.0, 1.0);
                            points[i] = (nx, ny);
                            if i == 0 {
                                points[i].0 = 0.0;
                            }
                            if i == points.len() - 1 {
                                points[i].0 = 1.0;
                            }
                        }
                    }
                }
                app.pressure_response.points = points;

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Linear").clicked() {
                        app.pressure_response = crate::pressure::PressureCurve::new_linear();
                    }
                    if ui.button("Steep").clicked() {
                        app.pressure_response = crate::pressure::PressureCurve::new_steep();
                    }
                    if ui.button("Ease-in").clicked() {
                        app.pressure_response = crate::pressure::PressureCurve::new_ease_in();
                    }
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });
        if close {
            app.show_pressure_calibration = false;
        }
    }

    // 7. PREFERENCES DIALOG
    if app.show_preferences_dialog {
        let mut close = false;
        egui::Window::new("Preferences")
            .collapsible(false)
            .resizable(true)
            .default_size([300.0, 250.0])
            .show(ctx, |ui| {
                egui::Grid::new("pref_grid")
                    .num_columns(2)
                    .spacing([10.0, 10.0])
                    .show(ui, |ui| {
                        ui.label("Theme:");
                        let old_theme = app.pref_theme.clone();
                        egui::ComboBox::from_id_source("pref_theme")
                            .selected_text(&app.pref_theme)
                            .show_ui(ui, |ui| {
                                for theme in &["Light", "Gray", "Dark"] {
                                    ui.selectable_value(
                                        &mut app.pref_theme,
                                        theme.to_string(),
                                        *theme,
                                    );
                                }
                            });
                        if app.pref_theme != old_theme {
                            if app.pref_theme == "Light" {
                                ctx.set_visuals(egui::Visuals::light());
                            } else if app.pref_theme == "Dark" {
                                ctx.set_visuals(egui::Visuals::dark());
                            } else {
                                let mut visuals = egui::Visuals::light();
                                visuals.panel_fill = egui::Color32::from_rgb(240, 240, 240);
                                visuals.window_fill = egui::Color32::from_rgb(245, 245, 245);
                                visuals.widgets.active.bg_fill =
                                    egui::Color32::from_rgb(180, 200, 240);
                                visuals.widgets.hovered.bg_fill =
                                    egui::Color32::from_rgb(215, 225, 250);
                                visuals.widgets.inactive.bg_fill =
                                    egui::Color32::from_rgb(230, 230, 230);
                                ctx.set_visuals(visuals);
                            }
                        }
                        ui.end_row();

                        ui.label("UI Scale:");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::Slider::new(&mut app.pref_ui_scale, 0.5..=2.0).step_by(0.1),
                            );
                            if ui.button("Apply").clicked() {
                                ctx.set_pixels_per_point(app.pref_ui_scale);
                            }
                        });
                        ui.end_row();

                        ui.label("Canvas Background:");
                        egui::ComboBox::from_id_source("pref_canvas_bg")
                            .selected_text(&app.pref_canvas_bg)
                            .show_ui(ui, |ui| {
                                for bg in &["Checkerboard", "White", "Gray", "Black"] {
                                    ui.selectable_value(
                                        &mut app.pref_canvas_bg,
                                        bg.to_string(),
                                        *bg,
                                    );
                                }
                            });
                        ui.end_row();

                        ui.label("Autosave:");
                        ui.checkbox(&mut app.pref_autosave_enabled, "Enabled");
                        ui.end_row();

                        ui.label("Autosave Interval:");
                        let old_interval = app.pref_autosave_interval_mins;
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::DragValue::new(&mut app.pref_autosave_interval_mins)
                                    .clamp_range(1..=60),
                            );
                            ui.label("minutes");
                        });
                        if app.pref_autosave_interval_mins != old_interval
                            || app.pref_autosave_enabled != app.autosave_enabled
                        {
                            app.autosave_enabled = app.pref_autosave_enabled;
                            app.autosave_interval_secs =
                                (app.pref_autosave_interval_mins * 60) as f64;
                        }
                        ui.end_row();
                    });

                ui.add_space(12.0);
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
        if close {
            app.show_preferences_dialog = false;
            crate::preferences::save_preferences(app);
        }
    }

    // 8. TABLET DIAGNOSTICS DIALOG
    if app.show_tablet_diagnostics {
        let mut close = false;
        egui::Window::new("Tablet Diagnostics")
            .collapsible(false)
            .resizable(true)
            .default_size([400.0, 450.0])
            .show(ctx, |ui| {
                ui.label("RAW INPUT STATES:");
                ui.group(|ui| {
                    let pressure = app.tablet_axis.pressure;
                    let tx = app.tablet_axis.tilt_x.unwrap_or(0.0);
                    let ty = app.tablet_axis.tilt_y.unwrap_or(0.0);
                    ui.label(format!("Pressure: {:.3}", pressure));
                    ui.label(format!("Tilt X: {:.3}", tx));
                    ui.label(format!("Tilt Y: {:.3}", ty));
                    ui.label(format!("Touch Active: {}", app.egui_touch_active));
                    if let Some(force) = app.egui_touch_pressure {
                        ui.label(format!("Touch Force: {:.3}", force));
                    }
                });

                ui.add_space(8.0);
                ui.label("STABILIZATION SETTINGS:");
                ui.horizontal(|ui| {
                    ui.label("Stabilizer Level:");
                    let preset = &mut app.presets[app.active_preset_index];
                    egui::ComboBox::from_id_source("stabilizer_level_combo")
                        .selected_text(format!("{:?}", preset.stabilizer_level))
                        .show_ui(ui, |ui| {
                            for level in &[
                                crate::input::StabilizerLevel::Off,
                                crate::input::StabilizerLevel::Level(3),
                                crate::input::StabilizerLevel::Level(5),
                                crate::input::StabilizerLevel::Level(10),
                                crate::input::StabilizerLevel::Level(15),
                                crate::input::StabilizerLevel::Level(20),
                                crate::input::StabilizerLevel::Level(30),
                            ] {
                                ui.selectable_value(&mut preset.stabilizer_level, *level, format!("{:?}", level));
                            }
                        });
                });

                ui.add_space(8.0);
                ui.label("PRESSURE CURVE DIAGRAM:");
                let size = egui::Vec2::splat(120.0);
                let (rect_curve, _response_curve) = ui.allocate_exact_size(size, egui::Sense::hover());
                ui.painter().rect_filled(rect_curve, 4.0, egui::Color32::from_gray(240));
                ui.painter().rect_stroke(rect_curve, 4.0, egui::Stroke::new(1.0, egui::Color32::GRAY));
                ui.painter().line_segment([rect_curve.left_bottom(), rect_curve.right_top()], egui::Stroke::new(1.0, egui::Color32::GRAY));

                let mut pts = Vec::new();
                let curve_steps = 20;
                for i in 0..=curve_steps {
                    let x_val = i as f32 / curve_steps as f32;
                    let y_val = app.remap_pressure(x_val);
                    let sx = rect_curve.left() + x_val * rect_curve.width();
                    let sy = rect_curve.bottom() - y_val * rect_curve.height();
                    pts.push(egui::Pos2::new(sx, sy));
                }
                for i in 0..pts.len() - 1 {
                    ui.painter().line_segment([pts[i], pts[i + 1]], egui::Stroke::new(2.0, egui::Color32::from_rgb(0, 120, 215)));
                }
                for pt in &pts {
                    ui.painter().circle_filled(*pt, 3.0, egui::Color32::from_rgb(0, 120, 215));
                }

                ui.add_space(8.0);
                ui.label("Stabilizer Test Pad (Draw here):");
                let pad_size = egui::Vec2::new(380.0, 100.0);
                let (pad_rect, pad_resp) = ui.allocate_exact_size(pad_size, egui::Sense::click_and_drag());
                ui.painter().rect_filled(pad_rect, 4.0, egui::Color32::from_gray(255));
                ui.painter().rect_stroke(pad_rect, 4.0, egui::Stroke::new(1.0, egui::Color32::GRAY));

                thread_local! {
                    static DIAG_POINTS: std::cell::RefCell<Vec<egui::Pos2>> = const { std::cell::RefCell::new(Vec::new()) };
                }

                if pad_resp.dragged_by(egui::PointerButton::Primary) {
                    if let Some(hover_pos) = pad_resp.hover_pos() {
                        DIAG_POINTS.with(|pts_cell| {
                            pts_cell.borrow_mut().push(hover_pos);
                        });
                    }
                }

                DIAG_POINTS.with(|pts_cell| {
                    let points = pts_cell.borrow().clone();
                    if points.len() >= 2 {
                        for i in 0..points.len() - 1 {
                            ui.painter().line_segment([points[i], points[i + 1]], egui::Stroke::new(2.0, egui::Color32::from_rgb(200, 40, 40)));
                        }
                    }
                });

                if ui.button("Clear Test Pad").clicked() {
                    DIAG_POINTS.with(|pts_cell| {
                        pts_cell.borrow_mut().clear();
                    });
                }

                ui.add_space(12.0);
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
        if close {
            app.show_tablet_diagnostics = false;
        }
    }
}

fn draw_panel_layout_settings(app: &mut PaintApp, ctx: &egui::Context) {
    use crate::ui::layout::{PanelKind, PanelLocation};
    let mut close = false;
    egui::Window::new("Panel Layout Settings")
        .resizable(true)
        .default_size([360.0, 480.0])
        .vscroll(true)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Dock All").clicked() {
                    for panel in &mut app.workspace_layout.panels {
                        match panel.kind {
                            PanelKind::ToolsAndPresets
                            | PanelKind::BrushSettings
                            | PanelKind::ToolOptions
                            | PanelKind::Stabilizer
                            | PanelKind::Symmetry
                            | PanelKind::AdvancedDebug => {
                                panel.location = PanelLocation::Left;
                            }
                            _ => {
                                panel.location = PanelLocation::Right;
                            }
                        }
                        panel.visible = true;
                    }
                }
                if ui.button("Float All").clicked() {
                    for panel in &mut app.workspace_layout.panels {
                        panel.location = PanelLocation::Floating;
                        panel.visible = true;
                    }
                }
                if ui.button("Reset Defaults").clicked() {
                    app.workspace_layout = Default::default();
                }
            });
            ui.separator();
            egui::Grid::new("panel_layout_grid")
                .striped(true)
                .min_col_width(80.0)
                .show(ui, |ui| {
                    ui.label("Panel");
                    ui.label("Float");
                    ui.label("Side");
                    ui.label("Visible");
                    ui.end_row();

                    let panels = app.workspace_layout.panels.clone();
                    for panel in &panels {
                        let kind = panel.kind;
                        let title = &panel.title;
                        let p = app.workspace_layout.find_panel_mut(kind).unwrap();

                        ui.label(title);
                        let mut is_float = p.location == PanelLocation::Floating;
                        if ui.checkbox(&mut is_float, "").changed() {
                            if is_float {
                                p.location = PanelLocation::Floating;
                                p.visible = true;
                            } else {
                                p.location = match kind {
                                    PanelKind::ToolsAndPresets
                                    | PanelKind::BrushSettings
                                    | PanelKind::ToolOptions
                                    | PanelKind::Stabilizer
                                    | PanelKind::Symmetry
                                    | PanelKind::AdvancedDebug => PanelLocation::Left,
                                    _ => PanelLocation::Right,
                                };
                            }
                        }
                        let side_label = match p.location {
                            PanelLocation::Left => "Left",
                            PanelLocation::Right => "Right",
                            PanelLocation::Floating => "—",
                            PanelLocation::Hidden => "—",
                        };
                        if p.location == PanelLocation::Left || p.location == PanelLocation::Right {
                            let mut side_idx = if p.location == PanelLocation::Left {
                                0
                            } else {
                                1
                            };
                            egui::ComboBox::from_id_source(egui::Id::new("panel_side").with(kind))
                                .selected_text(side_label)
                                .width(60.0)
                                .show_ui(ui, |ui| {
                                    if ui.selectable_label(side_idx == 0, "Left").clicked() {
                                        side_idx = 0;
                                        p.location = PanelLocation::Left;
                                        p.visible = true;
                                    }
                                    if ui.selectable_label(side_idx == 1, "Right").clicked() {
                                        side_idx = 1;
                                        p.location = PanelLocation::Right;
                                        p.visible = true;
                                    }
                                });
                        } else {
                            ui.label(side_label);
                        }
                        let mut vis = p.visible;
                        if ui.checkbox(&mut vis, "").changed() {
                            p.visible = vis;
                            if !vis {
                                p.location = PanelLocation::Hidden;
                            }
                        }
                        ui.end_row();
                    }
                });
            ui.separator();
            if ui.button("Close").clicked() {
                close = true;
            }
        });
    if close {
        app.show_panel_layout_settings = false;
        crate::preferences::save_workspace_layout(app);
    }
}
