use crate::app::PaintApp;
use crate::canvas::{BlendMode, Layer};
use crate::commands::CommandId;

pub fn draw_right_panel(app: &mut PaintApp, ctx: &egui::Context) {
    if !app.show_minimal_ui {
        egui::SidePanel::right("right_sidebar")
            .resizable(false)
            .default_width(260.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .id_source("right_sidebar_scroll")
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            // NAVIGATOR PANEL
                            ui.group(|ui| {
                                ui.label("NAVIGATOR");
                                ui.vertical_centered(|ui| {
                                    // Change sense to click_and_drag to allow panning interaction
                                    let (rect, response) = ui.allocate_exact_size(egui::vec2(240.0, 240.0), egui::Sense::click_and_drag());
                                    
                                    // 1. Draw Navigator Texture
                                    let painter = ui.painter().with_clip_rect(rect);
                                    if let Some(r) = &app.renderer {
                                        if let Some(texture_id) = r.navigator_egui_id {
                                            painter.image(
                                                texture_id,
                                                rect,
                                                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::new(1.0, 1.0)),
                                                egui::Color32::WHITE,
                                            );
                                        }
                                    }

                                    // 2. Calculate the paper sheet bounding box inside the 240x240 navigator box
                                    let canvas_aspect = app.canvas_width as f32 / app.canvas_height as f32;
                                    let paper_rect = if canvas_aspect >= 1.0 {
                                        let paper_h = 240.0 / canvas_aspect;
                                        egui::Rect::from_center_size(rect.center(), egui::vec2(240.0, paper_h))
                                    } else {
                                        let paper_w = 240.0 * canvas_aspect;
                                        egui::Rect::from_center_size(rect.center(), egui::vec2(paper_w, 240.0))
                                    };

                                    // 3. Project Viewport outline onto navigator
                                    if let Some(view_rect) = app.last_viewport_rect {
                                        let corners = [
                                            view_rect.min, // top-left
                                            egui::pos2(view_rect.max.x, view_rect.min.y), // top-right
                                            view_rect.max, // bottom-right
                                            egui::pos2(view_rect.min.x, view_rect.max.y), // bottom-left
                                        ];

                                        let mut nav_corners = Vec::with_capacity(4);
                                        for pt in corners {
                                            let w = app.screen_to_world(pt, view_rect);
                                            let pct_x = w.x / app.canvas_width as f32;
                                            let pct_y = w.y / app.canvas_height as f32;
                                            let nav_x = paper_rect.min.x + pct_x * paper_rect.width();
                                            let nav_y = paper_rect.min.y + pct_y * paper_rect.height();
                                            nav_corners.push(egui::pos2(nav_x, nav_y));
                                        }

                                        let stroke = egui::Stroke::new(1.5, egui::Color32::from_rgb(230, 50, 50));
                                        for i in 0..4 {
                                            painter.line_segment([nav_corners[i], nav_corners[(i + 1) % 4]], stroke);
                                        }
                                    }

                                    // 4. Click/Drag Panning Interaction
                                    if response.clicked() || response.dragged() {
                                        if let Some(click_pos) = response.interact_pointer_pos() {
                                            let pct_x = ((click_pos.x - paper_rect.min.x) / paper_rect.width()).clamp(0.0, 1.0);
                                            let pct_y = ((click_pos.y - paper_rect.min.y) / paper_rect.height()).clamp(0.0, 1.0);
                                            let w_target = egui::Vec2::new(pct_x * app.canvas_width as f32, pct_y * app.canvas_height as f32);
                                            
                                            let half_w = app.last_viewport_size.x * 0.5;
                                            let half_h = app.last_viewport_size.y * 0.5;
                                            app.viewport_offset = w_target - egui::vec2(half_w, half_h) / app.viewport_zoom;
                                            ctx.request_repaint();
                                        }
                                    }
                                });

                                // 5. Utility buttons [Fit] [100%] [Reset]
                                ui.add_space(4.0);
                                ui.horizontal(|ui| {
                                    if ui.button("Fit").clicked() {
                                        app.command(CommandId::FitToScreen);
                                    }
                                    if ui.button("100%").clicked() {
                                        app.command(CommandId::ActualSize);
                                    }
                                    if ui.button("Reset").clicked() {
                                        app.command(CommandId::ResetView);
                                    }
                                });

                                // 6. Status labels under Navigator
                                ui.add_space(4.0);
                                ui.label(format!("Zoom: {:.1}%", app.viewport_zoom * 100.0));
                                let angle_deg = app.rotation_angle.to_degrees().round();
                                let mirror_state = if app.mirror_horizontal { "Mirror On" } else { "Mirror Off" };
                                ui.label(format!("Rot: {:.0}° | {}", angle_deg, mirror_state));
                            });
                            ui.add_space(5.0);

                            // COLOR SELECTOR
                            ui.group(|ui| {
                                ui.label("COLOR SELECTOR");

                                // Custom HSV Color Wheel
                                ui.vertical_centered(|ui| {
                                    let res = crate::app::draw_hsv_color_wheel(ui, &mut app.brush_color, &mut app.color_wheel_drag_zone);
                                    if res.changed() {
                                        app.brush_settings_dirty = true;
                                    }
                                    if res.drag_stopped() || res.clicked() {
                                        app.record_color(app.brush_color);
                                    }
                                });

                                ui.add_space(5.0);

                                // RGB/HEX preview and text representation
                                ui.horizontal(|ui| {
                                    let mut color32 = egui::Color32::from_rgb(
                                        (app.brush_color[0] * 255.0) as u8,
                                        (app.brush_color[1] * 255.0) as u8,
                                        (app.brush_color[2] * 255.0) as u8,
                                    );

                                    let edit_res = egui::color_picker::color_edit_button_srgba(
                                        ui,
                                        &mut color32,
                                        egui::color_picker::Alpha::Opaque,
                                    );
                                    if edit_res.changed() {
                                        app.brush_color[0] = color32.r() as f32 / 255.0;
                                        app.brush_color[1] = color32.g() as f32 / 255.0;
                                        app.brush_color[2] = color32.b() as f32 / 255.0;
                                        app.brush_settings_dirty = true;
                                    }
                                    if edit_res.drag_stopped() || edit_res.clicked() {
                                        app.record_color(app.brush_color);
                                    }

                                    let hex_str = format!(
                                        "#{:02X}{:02X}{:02X}",
                                        color32.r(),
                                        color32.g(),
                                        color32.b()
                                    );
                                    ui.label(hex_str);
                                });

                                ui.add_space(4.0);
                                let mut sync_needed = false;
                                let mut history_needed = false;
                                egui::Grid::new("color_palette")
                                    .num_columns(6)
                                    .spacing([4.0, 4.0])
                                    .show(ui, |ui| {
                                        for (i, color) in app.palette.iter_mut().enumerate() {
                                            let fill = egui::Color32::from_rgb(
                                                (color[0] * 255.0) as u8,
                                                (color[1] * 255.0) as u8,
                                                (color[2] * 255.0) as u8,
                                            );
                                            let is_selected_swatch = app.selected_palette_index == Some(i);
                                            let btn_response = ui.add(
                                                egui::Button::new("")
                                                    .min_size(egui::Vec2::splat(22.0))
                                                    .fill(fill),
                                            );
                                            if is_selected_swatch {
                                                ui.painter().rect_stroke(
                                                    btn_response.rect.expand(1.5),
                                                    1.0,
                                                    egui::Stroke::new(2.0, egui::Color32::from_rgb(0, 120, 215))
                                                );
                                            }
                                            if btn_response.clicked() {
                                                let picked = *color;
                                                app.brush_color = picked;
                                                app.selected_palette_index = Some(i);
                                                history_needed = true;
                                                sync_needed = true;
                                            }
                                            if i % 6 == 5 {
                                                ui.end_row();
                                            }
                                        }
                                    });
                                if history_needed {
                                    app.record_color(app.brush_color);
                                }
                                if sync_needed {
                                    app.brush_settings_dirty = true;
                                }

                                // Color history
                                if !app.color_history.is_empty() {
                                    ui.add_space(6.0);
                                    ui.label("HISTORY");
                                    ui.horizontal_wrapped(|ui| {
                                        let hist_len = app.color_history.len();
                                        for (i, color) in app.color_history.iter().rev().enumerate() {
                                            if i >= 12 { break; }
                                            let fill = egui::Color32::from_rgb(
                                                (color[0] * 255.0) as u8,
                                                (color[1] * 255.0) as u8,
                                                (color[2] * 255.0) as u8,
                                            );
                                            let btn = ui.add(
                                                egui::Button::new("")
                                                    .min_size(egui::Vec2::splat(16.0))
                                                    .fill(fill),
                                            );
                                            if btn.clicked() {
                                                app.brush_color = *color;
                                                app.brush_settings_dirty = true;
                                            }
                                            if i < hist_len.min(12) - 1 {
                                                ui.add_space(2.0);
                                            }
                                        }
                                    });
                                }

                                ui.horizontal(|ui| {
                                    if ui.button("Save").clicked() {
                                        if let Some(i) = app.selected_palette_index {
                                            if let Some(slot) = app.palette.get_mut(i) {
                                                *slot = app.brush_color;
                                            }
                                        }
                                    }
                                    if ui.button("+").clicked() && app.palette.len() < 36 {
                                        app.palette.push(app.brush_color);
                                        app.selected_palette_index = Some(app.palette.len() - 1);
                                    }
                                    if ui
                                        .add_enabled(
                                            app.selected_palette_index.is_some() && app.palette.len() > 1,
                                            egui::Button::new("-"),
                                        )
                                        .clicked()
                                    {
                                        if let Some(i) = app.selected_palette_index.take() {
                                            if i < app.palette.len() {
                                                app.palette.remove(i);
                                            }
                                        }
                                    }
                                });
                            });

                            ui.add_space(5.0);

                            // Layer Manager Tree
                            ui.group(|ui| {
                                ui.label("LAYERS MANAGER");

                                ui.horizontal(|ui| {
                                    if ui.button("+ Raster").clicked() {
                                        app.layer_id_counter += 1;
                                        let new_id = app.layer_id_counter;
                                        let mut new_layer = Layer::new(new_id, format!("Layer {}", new_id));
                                        new_layer.kind = crate::canvas::LayerType::Raster;
                                        app.layers.insert(new_id, new_layer);
                                        app.layer_order.insert(0, new_id); // Add on top
                                        app.active_layer_id = new_id;
                                    }
                                    if ui.button("+ Folder").clicked() {
                                        app.layer_id_counter += 1;
                                        let new_id = app.layer_id_counter;
                                        let mut new_layer = Layer::new(new_id, format!("Folder {}", new_id));
                                        new_layer.kind = crate::canvas::LayerType::Folder { child_ids: Vec::new() };
                                        app.layers.insert(new_id, new_layer);
                                        app.layer_order.insert(0, new_id); // Add on top
                                        app.active_layer_id = new_id;
                                    }
                                    if ui.button("+ Vector").clicked() {
                                        app.layer_id_counter += 1;
                                        let new_id = app.layer_id_counter;
                                        let mut new_layer = Layer::new(new_id, format!("Vector {}", new_id));
                                        new_layer.kind = crate::canvas::LayerType::Vector;
                                        new_layer.vector_data = Some(crate::canvas::VectorLayer { strokes: Vec::new() });
                                        app.layers.insert(new_id, new_layer);
                                        app.layer_order.insert(0, new_id); // Add on top
                                        app.active_layer_id = new_id;
                                    }

                                    if ui
                                        .add_enabled(
                                            app.layer_order.len() > 1,
                                            egui::Button::new("- Delete"),
                                        )
                                        .clicked()
                                    {
                                        let active_id = app.active_layer_id;
                                        if let Some(pos) =
                                            app.layer_order.iter().position(|&x| x == active_id)
                                        {
                                            app.layer_order.remove(pos);
                                            app.layers.remove(&active_id);
                                            app.active_layer_id = app.layer_order[0];
                                        }
                                    }
                                });

                                ui.add_space(5.0);

                                // Active Layer blending options
                                if let Some(active_layer) = app.layers.get_mut(&app.active_layer_id) {
                                    ui.horizontal(|ui| {
                                        ui.label("Mode:");
                                        egui::ComboBox::from_id_source("blend_mode_dropdown")
                                            .selected_text(format!("{:?}", active_layer.blend_mode))
                                            .show_ui(ui, |ui| {
                                                ui.selectable_value(
                                                    &mut active_layer.blend_mode,
                                                    BlendMode::Normal,
                                                    "Normal",
                                                );
                                                ui.selectable_value(
                                                    &mut active_layer.blend_mode,
                                                    BlendMode::Multiply,
                                                    "Multiply",
                                                );
                                                ui.selectable_value(
                                                    &mut active_layer.blend_mode,
                                                    BlendMode::Screen,
                                                    "Screen",
                                                );
                                                ui.selectable_value(
                                                    &mut active_layer.blend_mode,
                                                    BlendMode::Overlay,
                                                    "Overlay",
                                                );
                                                ui.selectable_value(
                                                    &mut active_layer.blend_mode,
                                                    BlendMode::Luminosity,
                                                    "Luminosity (Shine)",
                                                );
                                                ui.selectable_value(
                                                    &mut active_layer.blend_mode,
                                                    BlendMode::Shade,
                                                    "Shade",
                                                );
                                            });
                                    });

                                    ui.horizontal(|ui| {
                                        ui.label("Opacity:");
                                        ui.add(egui::Slider::new(&mut active_layer.opacity, 0.0..=1.0).show_value(false));
                                    });

                                    ui.horizontal(|ui| {
                                        ui.checkbox(&mut active_layer.lock_alpha, "Lock Alpha");
                                        ui.checkbox(&mut active_layer.is_clipping, "Clipping Group");
                                    });
                                }

                                ui.separator();

                                // Scrollable Layer Selection List
                                // Pre-compute thumbnail textures to avoid borrow conflicts
                                let mut thumb_textures: ahash::AHashMap<u32, egui::TextureHandle> = ahash::AHashMap::default();
                                for id in &app.layer_order.clone() {
                                    if let Some(tex) = app.get_layer_thumbnail_texture(ctx, *id) {
                                        thumb_textures.insert(*id, tex);
                                    }
                                }
                                let order = app.layer_order.clone();
                                for id in order {
                                    let pointer_released =
                                        ui.ctx().input(|i| i.pointer.any_released());
                                    let is_active = app.active_layer_id == id;
                                    let mut row_hovered = false;

                                    ui.horizontal(|ui| {
                                        let drag_response = ui.add(
                                            egui::Label::new("::")
                                                .sense(egui::Sense::click_and_drag()),
                                        );
                                        row_hovered |= drag_response.hovered();
                                        if drag_response.drag_started() {
                                            app.dragging_layer_id = Some(id);
                                            app.active_layer_id = id;
                                        }
                                        if let Some(layer) = app.layers.get_mut(&id) {
                                            // Visibility check
                                            let vis_text = if layer.visible { "👁" } else { "⦂" };
                                            let btn_vis = egui::Button::new(vis_text).frame(false);
                                            let vis_resp = ui.add(btn_vis).on_hover_text("Toggle Visibility");
                                            row_hovered |= vis_resp.hovered();
                                            if vis_resp.clicked() {
                                                layer.visible = !layer.visible;
                                            }

                                            // Selection Source toggle (for Bucket/Magic Wand reference)
                                            let ref_text = if layer.selection_source { "◎" } else { "⚬" };
                                            let btn_ref = egui::Button::new(ref_text).frame(false).selected(layer.selection_source);
                                            let ref_resp = ui.add(btn_ref).on_hover_text("Use layer as reference source for Bucket/Wand");
                                            row_hovered |= ref_resp.hovered();
                                            if ref_resp.clicked() {
                                                layer.selection_source = !layer.selection_source;
                                            }

                                            // Layer thumbnail (with white background and thin border for empty layers visibility)
                                            let thumb_size = egui::Vec2::splat(28.0);
                                            let (thumb_rect, _thumb_resp) = ui.allocate_exact_size(thumb_size, egui::Sense::hover());
                                            ui.painter().rect_filled(thumb_rect, 1.0, egui::Color32::WHITE);
                                            ui.painter().rect_stroke(thumb_rect, 1.0, egui::Stroke::new(1.0, egui::Color32::from_gray(180)));
                                            if let Some(tex) = thumb_textures.get(&id) {
                                                ui.painter().image(
                                                    tex.id(),
                                                    thumb_rect,
                                                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::new(1.0, 1.0)),
                                                    egui::Color32::WHITE,
                                                );
                                            }

                                            // Highlight active layer
                                            let prefix = match &layer.kind {
                                                crate::canvas::LayerType::Folder { .. } => "[F] ",
                                                crate::canvas::LayerType::Vector => "[V] ",
                                                crate::canvas::LayerType::Raster => "[R] ",
                                            };
                                            let display_name = format!("{}{}", prefix, layer.name);
                                            let label_response = ui.add(egui::SelectableLabel::new(
                                                is_active,
                                                &display_name,
                                            ));
                                            if is_active {
                                                ui.painter().rect_stroke(
                                                    label_response.rect.expand(1.0),
                                                    1.0,
                                                    egui::Stroke::new(2.0, egui::Color32::from_rgb(0, 120, 215))
                                                );
                                            }
                                            row_hovered |= label_response.hovered();
                                            if label_response.clicked() {
                                                app.active_layer_id = id;
                                            }
                                        }
                                    });

                                    if let Some(dragging_id) = app.dragging_layer_id {
                                        if dragging_id != id && row_hovered {
                                            if let (Some(from), Some(to)) = (
                                                app.layer_order
                                                    .iter()
                                                    .position(|&layer_id| layer_id == dragging_id),
                                                app.layer_order
                                                    .iter()
                                                    .position(|&layer_id| layer_id == id),
                                            ) {
                                                app.layer_order.swap(from, to);
                                            }
                                        }
                                        if pointer_released {
                                            app.dragging_layer_id = None;
                                        }
                                    }
                                }
                            });

                            ui.add_space(5.0);

                            // Reference Images Panel
                            ui.group(|ui| {
                                ui.label("REFERENCE");

                                ui.horizontal(|ui| {
                                    ui.label("Path:");
                                    let text_edit = egui::TextEdit::singleline(&mut app.ref_image_path_input)
                                        .hint_text("Path to PNG...");
                                    ui.add(text_edit);
                                });

                                ui.horizontal(|ui| {
                                    if ui.button("+ Add Image").clicked() {
                                        let path = app.ref_image_path_input.clone();
                                        if let Err(e) = app.load_reference_image(&path, ui.ctx()) {
                                            log::error!("Failed to load reference image: {}", e);
                                        } else {
                                            app.ref_image_path_input.clear();
                                        }
                                    }
                                    if ui.button("Hide All").clicked() {
                                        for img in &mut app.reference_images {
                                            img.visible = false;
                                        }
                                    }
                                    if ui.button("Show All").clicked() {
                                        for img in &mut app.reference_images {
                                            img.visible = true;
                                        }
                                    }
                                });

                                if !app.reference_images.is_empty() {
                                    ui.add_space(4.0);
                                    ui.label("List:");
                                    let mut to_remove_idx = None;
                                    for (idx, img) in app.reference_images.iter_mut().enumerate() {
                                        ui.horizontal(|ui| {
                                            // Eye toggle styled like layers manager
                                            let eye_text = if img.visible { "👁" } else { "⦂" };
                                            let btn_eye = egui::Button::new(eye_text).frame(false).selected(img.visible);
                                            if ui.add(btn_eye).clicked() {
                                                img.visible = !img.visible;
                                            }

                                            let is_selected = app.selected_reference_idx == Some(idx);
                                            let opacity_pct = (img.opacity * 100.0).round() as i32;
                                            let label_text = format!("{} ({}%)", img.name, opacity_pct);
                                            if ui.selectable_label(is_selected, &label_text).clicked() {
                                                app.selected_reference_idx = Some(idx);
                                            }
                                        });
                                    }

                                    if let Some(idx) = app.selected_reference_idx {
                                        if idx < app.reference_images.len() {
                                            ui.add_space(4.0);
                                            ui.separator();
                                            ui.label("Selected Reference:");

                                            // Note: since we borrow app as mutable, we temporarily pull out/operate on the selected image
                                            let canvas_w = app.canvas_width as f32;
                                            let canvas_h = app.canvas_height as f32;
                                            let img = &mut app.reference_images[idx];

                                            ui.horizontal(|ui| {
                                                ui.label("Opacity:");
                                                ui.add(egui::Slider::new(&mut img.opacity, 0.0..=1.0).show_value(true));
                                            });

                                            ui.horizontal(|ui| {
                                                ui.label("Scale:");
                                                ui.add(egui::Slider::new(&mut img.scale, 0.1..=5.0).show_value(true));
                                            });

                                            ui.horizontal(|ui| {
                                                ui.label("Rotation:");
                                                let mut degrees = img.rotation.to_degrees();
                                                if ui.add(egui::Slider::new(&mut degrees, -180.0..=180.0).show_value(true).suffix("°")).changed() {
                                                    img.rotation = degrees.to_radians();
                                                }
                                            });

                                            ui.horizontal(|ui| {
                                                if ui.selectable_label(img.pinned_to_view, "Pin to View").clicked() {
                                                    if !img.pinned_to_view {
                                                        img.pinned_to_view = true;
                                                        img.world_pos = egui::vec2(200.0, 200.0);
                                                    }
                                                }
                                                if ui.selectable_label(!img.pinned_to_view, "Pin to Canvas").clicked() {
                                                    if img.pinned_to_view {
                                                        img.pinned_to_view = false;
                                                        img.world_pos = egui::vec2(canvas_w * 0.5, canvas_h * 0.5);
                                                    }
                                                }
                                            });

                                            if ui.button("Remove").clicked() {
                                                to_remove_idx = Some(idx);
                                            }
                                        }
                                    }

                                    if let Some(remove_idx) = to_remove_idx {
                                        app.reference_images.remove(remove_idx);
                                        app.selected_reference_idx = None;
                                    }
                                }
                            });
                        });
                    });
            });
    }
}
