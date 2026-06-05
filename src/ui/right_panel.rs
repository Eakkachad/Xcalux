use crate::app::PaintApp;
use crate::canvas::{BlendMode, Layer};
use crate::commands::CommandId;
use crate::history::{HistoryCommand, LayerPropertyChange};

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
                            // PANEL VISIBILITY SHORTCUT ROW (SAI style)
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing.x = 2.0;
                                ui.spacing_mut().item_spacing.y = 2.0;
                                
                                let nav_btn = egui::Button::new("🧭").selected(app.show_navigator);
                                if ui.add(nav_btn).on_hover_text("Toggle Navigator").clicked() {
                                    app.show_navigator = !app.show_navigator;
                                }

                                let wheel_btn = egui::Button::new("🎨").selected(app.show_color_wheel);
                                if ui.add(wheel_btn).on_hover_text("Toggle Color Wheel").clicked() {
                                    app.show_color_wheel = !app.show_color_wheel;
                                }

                                let rgb_btn = egui::Button::new("🎚").selected(app.show_rgb_sliders);
                                if ui.add(rgb_btn).on_hover_text("Toggle RGB Sliders").clicked() {
                                    app.show_rgb_sliders = !app.show_rgb_sliders;
                                }

                                let hsv_btn = egui::Button::new("🎛").selected(app.show_hsv_sliders);
                                if ui.add(hsv_btn).on_hover_text("Toggle HSV Sliders").clicked() {
                                    app.show_hsv_sliders = !app.show_hsv_sliders;
                                }

                                let pal_btn = egui::Button::new("▦").selected(app.show_color_palette);
                                if ui.add(pal_btn).on_hover_text("Toggle Color Palette").clicked() {
                                    app.show_color_palette = !app.show_color_palette;
                                }

                                let hist_btn = egui::Button::new("⏱").selected(app.show_color_history);
                                if ui.add(hist_btn).on_hover_text("Toggle Color History").clicked() {
                                    app.show_color_history = !app.show_color_history;
                                }

                                let layers_btn = egui::Button::new("🗂").selected(app.show_layers_manager);
                                if ui.add(layers_btn).on_hover_text("Toggle Layers Manager").clicked() {
                                    app.show_layers_manager = !app.show_layers_manager;
                                }

                                let ref_btn = egui::Button::new("🖼").selected(app.show_reference_panel);
                                if ui.add(ref_btn).on_hover_text("Toggle Reference Panel").clicked() {
                                    app.show_reference_panel = !app.show_reference_panel;
                                }

                                let sym_btn = egui::Button::new("🪞").selected(app.show_symmetry_panel);
                                if ui.add(sym_btn).on_hover_text("Toggle Symmetry Panel").clicked() {
                                    app.show_symmetry_panel = !app.show_symmetry_panel;
                                }

                                let tool_btn = egui::Button::new("🛠").selected(app.show_tool_options);
                                if ui.add(tool_btn).on_hover_text("Toggle Tool Options").clicked() {
                                    app.show_tool_options = !app.show_tool_options;
                                }
                            });
                            ui.separator();

                            // NAVIGATOR PANEL
                            if app.show_navigator {
                                egui::CollapsingHeader::new("NAVIGATOR")
                                    .default_open(true)
                                    .show(ui, |ui| {
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
                            }

                            // COLOR WHEEL
                            if app.show_color_wheel {
                                egui::CollapsingHeader::new("COLOR WHEEL")
                                    .default_open(true)
                                    .show(ui, |ui| {
                                         // Custom HSV Color Wheel
                                         ui.vertical_centered(|ui| {
                                             let mut active_col = app.active_color();
                                             let res = crate::app::draw_hsv_color_wheel(ui, &mut active_col, &mut app.color_wheel_drag_zone);
                                             if res.changed() {
                                                 app.set_active_color(active_col);
                                             }
                                             if res.drag_stopped() || res.clicked() {
                                                 app.record_color(active_col);
                                             }
                                         });

                                         ui.add_space(5.0);

                                          // Overlapping Foreground & Background Swatches + Swap button + Transparency
                                          ui.horizontal(|ui| {
                                              let (swatches_rect, response) = ui.allocate_exact_size(egui::vec2(50.0, 50.0), egui::Sense::click());
                                              let (trans_rect, trans_resp) = ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::click());

                                              if response.clicked() {
                                                  if let Some(click_pos) = response.interact_pointer_pos() {
                                                      let local_pos = click_pos - swatches_rect.min;
                                                      if local_pos.x >= 0.0 && local_pos.x <= 34.0 && local_pos.y >= 0.0 && local_pos.y <= 34.0 {
                                                          app.active_color_is_bg = false;
                                                          app.active_color_is_transparent = false;
                                                          app.brush_settings_dirty = true;
                                                      } else if local_pos.x >= 16.0 && local_pos.x <= 50.0 && local_pos.y >= 16.0 && local_pos.y <= 50.0 {
                                                          app.active_color_is_bg = true;
                                                          app.active_color_is_transparent = false;
                                                          app.brush_settings_dirty = true;
                                                      }
                                                  }
                                              }

                                              if trans_resp.clicked() {
                                                  app.active_color_is_transparent = true;
                                                  app.brush_settings_dirty = true;
                                              }
                                              if trans_resp.hovered() {
                                                  ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                              }

                                              let painter = ui.painter();
                                              let bg_rect = egui::Rect::from_min_size(
                                                  swatches_rect.min + egui::vec2(16.0, 16.0),
                                                  egui::vec2(34.0, 34.0),
                                              );
                                              let bg_color = egui::Color32::from_rgb(
                                                  (app.background_color[0] * 255.0) as u8,
                                                  (app.background_color[1] * 255.0) as u8,
                                                  (app.background_color[2] * 255.0) as u8,
                                              );
                                              painter.rect_filled(bg_rect, 0.0, bg_color);
                                              if app.active_color_is_bg && !app.active_color_is_transparent {
                                                  painter.rect_stroke(bg_rect, 0.0, egui::Stroke::new(2.5, egui::Color32::from_rgb(0, 120, 215)));
                                              } else {
                                                  painter.rect_stroke(bg_rect, 0.0, egui::Stroke::new(1.0, egui::Color32::GRAY));
                                              }

                                              let fg_rect = egui::Rect::from_min_size(
                                                  swatches_rect.min,
                                                  egui::vec2(34.0, 34.0),
                                              );
                                              let fg_color = egui::Color32::from_rgb(
                                                  (app.foreground_color[0] * 255.0) as u8,
                                                  (app.foreground_color[1] * 255.0) as u8,
                                                  (app.foreground_color[2] * 255.0) as u8,
                                              );
                                              painter.rect_filled(fg_rect, 0.0, fg_color);
                                              if !app.active_color_is_bg && !app.active_color_is_transparent {
                                                  painter.rect_stroke(fg_rect, 0.0, egui::Stroke::new(2.5, egui::Color32::from_rgb(0, 120, 215)));
                                              } else {
                                                  painter.rect_stroke(fg_rect, 0.0, egui::Stroke::new(1.0, egui::Color32::GRAY));
                                              }

                                              let size_w = 6.0;
                                              for row in 0..4 {
                                                  for col in 0..4 {
                                                      let sq_rect = egui::Rect::from_min_max(
                                                          trans_rect.min + egui::vec2(col as f32 * size_w, row as f32 * size_w),
                                                          trans_rect.min + egui::vec2((col + 1) as f32 * size_w, (row + 1) as f32 * size_w),
                                                      );
                                                      let color = if (row + col) % 2 == 0 {
                                                          egui::Color32::from_gray(240)
                                                      } else {
                                                          egui::Color32::from_gray(180)
                                                      };
                                                      painter.rect_filled(sq_rect, 0.0, color);
                                                  }
                                              }
                                              if app.active_color_is_transparent {
                                                  painter.rect_stroke(trans_rect, 0.0, egui::Stroke::new(2.5, egui::Color32::from_rgb(0, 120, 215)));
                                              } else {
                                                  painter.rect_stroke(trans_rect, 0.0, egui::Stroke::new(1.0, egui::Color32::GRAY));
                                              }

                                              if ui.button("⇄").on_hover_text("Swap colors (X)").clicked() {
                                                  std::mem::swap(&mut app.foreground_color, &mut app.background_color);
                                                  app.active_color_is_transparent = false;
                                                  app.brush_settings_dirty = true;
                                              }
                                          });

                                         // HEX Text Input
                                         ui.add_space(3.0);
                                         let active_col = app.active_color();
                                         let active_hex = format!(
                                             "#{:02X}{:02X}{:02X}",
                                             (active_col[0] * 255.0).round() as u8,
                                             (active_col[1] * 255.0).round() as u8,
                                             (active_col[2] * 255.0).round() as u8
                                         );
                                         ui.horizontal(|ui| {
                                             ui.label("Hex:");
                                             let hex_edit = ui.text_edit_singleline(&mut app.hex_color_input);
                                             if hex_edit.changed() {
                                                 if let Some(parsed) = PaintApp::parse_hex_color(&app.hex_color_input) {
                                                     app.set_active_color(parsed);
                                                     app.record_color(parsed);
                                                 }
                                             }
                                             if !hex_edit.has_focus() {
                                                 app.hex_color_input = active_hex;
                                             }
                                         });
                                    });
                                ui.add_space(5.0);
                            }

                            // RGB SLIDERS
                            if app.show_rgb_sliders {
                                egui::CollapsingHeader::new("RGB SLIDERS")
                                    .default_open(true)
                                    .show(ui, |ui| {
                                        let mut active_col = app.active_color();
                                        let mut r_val = (active_col[0] * 255.0).round() as u8;
                                        let mut g_val = (active_col[1] * 255.0).round() as u8;
                                        let mut b_val = (active_col[2] * 255.0).round() as u8;
                                        let mut rgb_changed = false;
                                        let mut rgb_drag_released = false;
                                        ui.horizontal(|ui| {
                                            ui.label("R:");
                                            let res = ui.add(egui::Slider::new(&mut r_val, 0..=255));
                                            if res.changed() { rgb_changed = true; }
                                            if res.drag_stopped() { rgb_drag_released = true; }
                                        });
                                        ui.horizontal(|ui| {
                                            ui.label("G:");
                                            let res = ui.add(egui::Slider::new(&mut g_val, 0..=255));
                                            if res.changed() { rgb_changed = true; }
                                            if res.drag_stopped() { rgb_drag_released = true; }
                                        });
                                        ui.horizontal(|ui| {
                                            ui.label("B:");
                                            let res = ui.add(egui::Slider::new(&mut b_val, 0..=255));
                                            if res.changed() { rgb_changed = true; }
                                            if res.drag_stopped() { rgb_drag_released = true; }
                                        });

                                        if rgb_changed {
                                            active_col[0] = r_val as f32 / 255.0;
                                            active_col[1] = g_val as f32 / 255.0;
                                            active_col[2] = b_val as f32 / 255.0;
                                            app.set_active_color(active_col);
                                        }
                                        if rgb_drag_released {
                                            app.record_color(active_col);
                                        }
                                    });
                                ui.add_space(5.0);
                            }

                            // HSV SLIDERS
                            if app.show_hsv_sliders {
                                egui::CollapsingHeader::new("HSV SLIDERS")
                                    .default_open(true)
                                    .show(ui, |ui| {
                                        let mut active_col = app.active_color();
                                        let (h, s, v) = crate::app::rgb_to_hsv(active_col[0], active_col[1], active_col[2]);
                                        let mut h_deg = (h * 360.0).round() as u32;
                                        let mut s_pct = (s * 100.0).round() as u32;
                                        let mut v_pct = (v * 100.0).round() as u32;

                                        let mut hsv_changed = false;
                                        let mut hsv_drag_released = false;
                                        ui.horizontal(|ui| {
                                            ui.label("H:");
                                            let res = ui.add(egui::Slider::new(&mut h_deg, 0..=360).suffix("°"));
                                            if res.changed() { hsv_changed = true; }
                                            if res.drag_stopped() { hsv_drag_released = true; }
                                        });
                                        ui.horizontal(|ui| {
                                            ui.label("S:");
                                            let res = ui.add(egui::Slider::new(&mut s_pct, 0..=100).suffix("%"));
                                            if res.changed() { hsv_changed = true; }
                                            if res.drag_stopped() { hsv_drag_released = true; }
                                        });
                                        ui.horizontal(|ui| {
                                            ui.label("V:");
                                            let res = ui.add(egui::Slider::new(&mut v_pct, 0..=100).suffix("%"));
                                            if res.changed() { hsv_changed = true; }
                                            if res.drag_stopped() { hsv_drag_released = true; }
                                        });

                                        if hsv_changed {
                                            let (r, g, b) = crate::app::hsv_to_rgb(h_deg as f32 / 360.0, s_pct as f32 / 100.0, v_pct as f32 / 100.0);
                                            active_col[0] = r;
                                            active_col[1] = g;
                                            active_col[2] = b;
                                            app.set_active_color(active_col);
                                        }
                                        if hsv_drag_released {
                                            app.record_color(active_col);
                                        }
                                    });
                                ui.add_space(5.0);
                            }

                            // COLOR PALETTE
                            if app.show_color_palette {
                                egui::CollapsingHeader::new("COLOR PALETTE")
                                    .default_open(true)
                                    .show(ui, |ui| {
                                        let mut clicked_palette_color = None;
                                        let mut clicked_palette_idx = None;
                                        egui::Grid::new("color_palette")
                                            .num_columns(6)
                                            .spacing([4.0, 4.0])
                                            .show(ui, |ui| {
                                                for (i, color) in app.palette.iter().enumerate() {
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
                                                        clicked_palette_color = Some(*color);
                                                        clicked_palette_idx = Some(i);
                                                    }
                                                    if i % 6 == 5 {
                                                        ui.end_row();
                                                    }
                                                }
                                            });
                                        if let Some(picked) = clicked_palette_color {
                                            app.set_active_color(picked);
                                            app.selected_palette_index = clicked_palette_idx;
                                            app.record_color(picked);
                                            app.brush_settings_dirty = true;
                                        }

                                        ui.add_space(4.0);
                                        ui.horizontal(|ui| {
                                            if ui.button("Save").clicked() {
                                                if let Some(i) = app.selected_palette_index {
                                                    if i < app.palette.len() {
                                                        app.palette[i] = app.active_color();
                                                    }
                                                }
                                            }
                                            if ui.button("+").clicked() && app.palette.len() < 36 {
                                                let active_col = app.active_color();
                                                app.palette.push(active_col);
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
                            }

                            // COLOR HISTORY
                            if app.show_color_history {
                                egui::CollapsingHeader::new("COLOR HISTORY")
                                    .default_open(true)
                                    .show(ui, |ui| {
                                        let mut clicked_history_color = None;
                                        if !app.color_history.is_empty() {
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
                                                        clicked_history_color = Some(*color);
                                                    }
                                                    if i < hist_len.min(12) - 1 {
                                                        ui.add_space(2.0);
                                                    }
                                                }
                                            });
                                        }
                                        if let Some(color) = clicked_history_color {
                                            app.set_active_color(color);
                                            app.brush_settings_dirty = true;
                                        }
                                    });
                                ui.add_space(5.0);
                            }

                            // LAYERS MANAGER (Right panel placement)
                            if app.show_layers_manager && !app.layer_panel_on_left {
                                draw_layers_manager_widget(app, ui, ctx);
                                ui.add_space(5.0);
                            }

                            // REFERENCE PANEL
                            if app.show_reference_panel {
                                egui::CollapsingHeader::new("REFERENCE")
                                    .default_open(true)
                                    .show(ui, |ui| {
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
                                                        if ui.selectable_label(img.pinned_to_view, "Pin to View").clicked()
                                                            && !img.pinned_to_view {
                                                                img.pinned_to_view = true;
                                                                img.world_pos = egui::vec2(200.0, 200.0);
                                                            }
                                                        if ui.selectable_label(!img.pinned_to_view, "Pin to Canvas").clicked()
                                                            && img.pinned_to_view {
                                                                img.pinned_to_view = false;
                                                                img.world_pos = egui::vec2(canvas_w * 0.5, canvas_h * 0.5);
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
                                ui.add_space(5.0);
                            }
                        });
                    });
            });
    }
}

pub(crate) fn draw_layers_manager_widget(app: &mut PaintApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    egui::CollapsingHeader::new("LAYERS MANAGER")
        .default_open(true)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("+ Raster").clicked() {
                    app.layer_id_counter += 1;
                    let new_id = app.layer_id_counter;
                    let mut new_layer = Layer::new(new_id, format!("Layer {}", new_id));
                    new_layer.kind = crate::canvas::LayerType::Raster;
                    let layer_clone = new_layer.clone();
                    app.layers.insert(new_id, new_layer);
                    app.layer_order.insert(0, new_id);
                    app.active_layer_id = new_id;
                    app.history.push_command(HistoryCommand::LayerCreate { layer: Box::new(layer_clone), index: 0 });
                }
                if ui.button("+ Folder").clicked() {
                    app.layer_id_counter += 1;
                    let new_id = app.layer_id_counter;
                    let mut new_layer = Layer::new(new_id, format!("Folder {}", new_id));
                    new_layer.kind = crate::canvas::LayerType::Folder { child_ids: Vec::new() };
                    let layer_clone = new_layer.clone();
                    app.layers.insert(new_id, new_layer);
                    app.layer_order.insert(0, new_id);
                    app.active_layer_id = new_id;
                    app.history.push_command(HistoryCommand::LayerCreate { layer: Box::new(layer_clone), index: 0 });
                }
                if ui.button("+ Vector").clicked() {
                    app.layer_id_counter += 1;
                    let new_id = app.layer_id_counter;
                    let mut new_layer = Layer::new(new_id, format!("Vector {}", new_id));
                    new_layer.kind = crate::canvas::LayerType::Vector;
                    new_layer.vector_data = Some(crate::canvas::VectorLayer { strokes: Vec::new(), display_mode: Default::default() });
                    let layer_clone = new_layer.clone();
                    app.layers.insert(new_id, new_layer);
                    app.layer_order.insert(0, new_id);
                    app.active_layer_id = new_id;
                    app.history.push_command(HistoryCommand::LayerCreate { layer: Box::new(layer_clone), index: 0 });
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
                        let removed = app.layers.remove(&active_id).unwrap();
                        app.layer_order.remove(pos);
                        app.active_layer_id = app.layer_order[0];
                        app.history.push_command(HistoryCommand::LayerDelete {
                            layer: Box::new(removed),
                            index: pos,
                        });
                    }
                }
                if ui.button("Clear").clicked() {
                    app.command(CommandId::ClearLayer);
                }
                if ui.button("Fill").clicked() {
                    app.command(CommandId::FillLayer);
                }
                if ui.button("Import Img").clicked() {
                    app.command(CommandId::ImportImageAsLayer);
                }
            });

            ui.add_space(5.0);

            // Active Layer blending options
            let layer_id = app.active_layer_id;
            let (old_opacity, old_blend, old_lock_alpha, old_clipping, _old_visible, old_name) =
                if let Some(l) = app.layers.get(&layer_id) {
                    (l.opacity, l.blend_mode, l.lock_alpha, l.is_clipping, l.visible, l.name.clone())
                } else {
                    (1.0, BlendMode::Normal, false, false, true, String::new())
                };

            if let Some(active_layer) = app.layers.get_mut(&app.active_layer_id) {
                ui.horizontal(|ui| {
                    ui.label("Name:");
                    ui.add(egui::TextEdit::singleline(&mut active_layer.name).hint_text("Layer name"));
                });
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

            // Push history commands for property changes
            if let Some(active_layer) = app.layers.get(&layer_id) {
                let aid = app.active_layer_id;
                let mut commands: Vec<HistoryCommand> = Vec::new();
                if (active_layer.opacity - old_opacity).abs() > f32::EPSILON {
                    commands.push(HistoryCommand::LayerProperty {
                        layer_id: aid,
                        property: LayerPropertyChange::Opacity { old: old_opacity, new: active_layer.opacity },
                    });
                }
                if active_layer.blend_mode != old_blend {
                    commands.push(HistoryCommand::LayerProperty {
                        layer_id: aid,
                        property: LayerPropertyChange::BlendMode { old: old_blend, new: active_layer.blend_mode },
                    });
                }
                if active_layer.lock_alpha != old_lock_alpha {
                    commands.push(HistoryCommand::LayerProperty {
                        layer_id: aid,
                        property: LayerPropertyChange::LockAlpha { old: old_lock_alpha, new: active_layer.lock_alpha },
                    });
                }
                if active_layer.is_clipping != old_clipping {
                    commands.push(HistoryCommand::LayerProperty {
                        layer_id: aid,
                        property: LayerPropertyChange::Clipping { old: old_clipping, new: active_layer.is_clipping },
                    });
                }
                if active_layer.name != old_name {
                    commands.push(HistoryCommand::LayerProperty {
                        layer_id: aid,
                        property: LayerPropertyChange::Rename { old: old_name, new: active_layer.name.clone() },
                    });
                }
                for cmd in commands {
                    app.history.push_command(cmd);
                }
            }

            // Vector layer display mode toggle
            let (is_vector, is_spline_mode) = app.layers.get(&app.active_layer_id).map(|l| {
                let is_v = matches!(l.kind, crate::canvas::LayerType::Vector);
                let is_spline = l.vector_data.as_ref().map(|vd| vd.display_mode == crate::canvas::VectorDisplayMode::SplineMesh).unwrap_or(false);
                (is_v, is_spline)
            }).unwrap_or((false, false));

            if is_vector {
                ui.horizontal(|ui| {
                    ui.label("Vector Display:");
                    let spline = is_spline_mode;
                    if ui.selectable_label(!spline, "Rasterized").clicked() {
                        if let Some(layer) = app.layers.get_mut(&app.active_layer_id) {
                            if let Some(vd) = &mut layer.vector_data {
                                vd.display_mode = crate::canvas::VectorDisplayMode::Rasterized;
                            }
                        }
                    }
                    if ui.selectable_label(spline, "Spline Mesh").clicked() {
                        if let Some(layer) = app.layers.get_mut(&app.active_layer_id) {
                            if let Some(vd) = &mut layer.vector_data {
                                vd.display_mode = crate::canvas::VectorDisplayMode::SplineMesh;
                            }
                        }
                    }
                });

                // Stroke width slider for selected control point
                let selected_width = app.edit_cp_selection.and_then(|(si, _)| {
                    app.layers.get(&app.active_layer_id).and_then(|l| {
                        l.vector_data.as_ref().and_then(|vd| {
                            vd.strokes.get(si).map(|s| s.width)
                        })
                    })
                });
                if let Some(cur_width) = selected_width {
                    let mut new_width = cur_width;
                    let si = app.edit_cp_selection.map(|(s, _)| s).unwrap_or(0);
                    ui.horizontal(|ui| {
                        ui.label("Stroke Width:");
                        if ui.add(egui::Slider::new(&mut new_width, 0.1..=10.0).step_by(0.1)).changed() {
                            if let Some(layer) = app.layers.get_mut(&app.active_layer_id) {
                                if let Some(vd) = &mut layer.vector_data {
                                    if si < vd.strokes.len() {
                                        vd.strokes[si].width = new_width;
                                        app.redraw_vector_layer(app.active_layer_id);
                                    }
                                }
                            }
                        }
                    });
                }

                ui.horizontal(|ui| {
                    if ui.button("Convert to Raster").clicked() {
                        app.convert_active_vector_to_raster();
                        ctx.request_repaint();
                    }
                });

                // SVG export
                ui.horizontal(|ui| {
                    if ui.button("Export SVG").clicked() {
                        if let Some(layer) = app.layers.get(&app.active_layer_id) {
                            if let Some(vd) = &layer.vector_data {
                                let svg_content = crate::vector::export_strokes_svg(
                                    &vd.strokes, app.canvas_width, app.canvas_height,
                                );
                                let svg_path = std::path::Path::new(&app.document_path)
                                    .with_extension("svg");
                                if let Err(e) = std::fs::write(&svg_path, &svg_content) {
                                    log::error!("Failed to export SVG: {}", e);
                                } else {
                                    log::info!("Exported SVG to {:?}", svg_path);
                                }
                            }
                        }
                    }
                });
            }

            ui.separator();

            // Scrollable Layer Selection List
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
                let mut drag_started = false;

                ui.horizontal(|ui| {
                    let drag_response = ui.add(
                        egui::Label::new("::")
                            .sense(egui::Sense::click_and_drag()),
                    );
                    row_hovered |= drag_response.hovered();
                    if drag_response.drag_started() {
                        app.dragging_layer_id = Some(id);
                        app.active_layer_id = id;
                        drag_started = true;
                    }
                    let mut vis_changed: Option<(bool, bool)> = None;
                    if let Some(layer) = app.layers.get_mut(&id) {
                        let old_vis = layer.visible;
                        let vis_text = if layer.visible { "👁" } else { "⦂" };
                        let btn_vis = egui::Button::new(vis_text).frame(false);
                        let vis_resp = ui.add(btn_vis).on_hover_text("Toggle Visibility");
                        row_hovered |= vis_resp.hovered();
                        if vis_resp.clicked() {
                            layer.visible = !layer.visible;
                        }
                        if old_vis != layer.visible {
                            vis_changed = Some((old_vis, layer.visible));
                        }

                        let ref_text = if layer.selection_source { "◎" } else { "⚬" };
                        let btn_ref = egui::Button::new(ref_text).frame(false).selected(layer.selection_source);
                        let ref_resp = ui.add(btn_ref).on_hover_text("Use layer as reference source for Bucket/Wand");
                        row_hovered |= ref_resp.hovered();
                        if ref_resp.clicked() {
                            layer.selection_source = !layer.selection_source;
                        }

                        let old_locked = layer.locked;
                        let lock_text = if layer.locked { "🔒" } else { "🔓" };
                        let btn_lock = egui::Button::new(lock_text).frame(false);
                        if ui.add(btn_lock).on_hover_text("Lock/Unlock Layer").clicked() {
                            layer.locked = !layer.locked;
                        }
                        if old_locked != layer.locked {
                            app.history.push_command(HistoryCommand::LayerProperty {
                                layer_id: id,
                                property: LayerPropertyChange::Locked { old: old_locked, new: layer.locked },
                            });
                        }

                        let thumb_size = egui::Vec2::splat(28.0);
                        let (thumb_rect, thumb_resp) = ui.allocate_exact_size(thumb_size, egui::Sense::click());
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
                        if is_active {
                            if thumb_resp.clicked() {
                                app.active_mask_editing = false;
                            }
                            if !app.active_mask_editing {
                                ui.painter().rect_stroke(thumb_rect.expand(1.0), 1.0, egui::Stroke::new(2.0, egui::Color32::from_rgb(0, 120, 215)));
                            }
                        }

                        let mask_has = layer.mask.is_some();
                        if mask_has {
                            let (mask_thumb_rect, mask_thumb_resp) = ui.allocate_exact_size(egui::Vec2::splat(20.0), egui::Sense::click());
                            ui.painter().rect_filled(mask_thumb_rect, 1.0, egui::Color32::WHITE);
                            ui.painter().rect_stroke(mask_thumb_rect, 1.0, egui::Stroke::new(1.0, egui::Color32::from_gray(180)));
                            if let Some(ref mask) = layer.mask {
                                if !mask.enabled {
                                    ui.painter().rect_filled(mask_thumb_rect, 1.0, egui::Color32::from_rgba_premultiplied(128, 128, 128, 64));
                                }
                            }
                            if is_active && app.active_mask_editing {
                                ui.painter().rect_stroke(mask_thumb_rect.expand(1.0), 1.0, egui::Stroke::new(2.0, egui::Color32::from_rgb(0, 200, 80)));
                            }
                            if mask_thumb_resp.clicked() {
                                app.active_mask_editing = true;
                            }
                            if mask_thumb_resp.hovered() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                        }

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
                    if let Some((old_v, new_v)) = vis_changed {
                        app.history.push_command(HistoryCommand::LayerProperty {
                            layer_id: id,
                            property: LayerPropertyChange::Visible { old: old_v, new: new_v },
                        });
                    }
                });

                if let Some(dragging_id) = app.dragging_layer_id {
                    if dragging_id == id && drag_started {
                        app.drag_start_order = Some(app.layer_order.clone());
                    }
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
                        if let Some(old_order) = app.drag_start_order.take() {
                            let new_order = app.layer_order.clone();
                            if old_order != new_order {
                                app.history.push_command(HistoryCommand::LayerReorder { old_order, new_order });
                            }
                        }
                        app.dragging_layer_id = None;
                    }
                }
            }

            // Mask actions for active layer
            let mask_state = app.layers.get(&app.active_layer_id).map(|l| (l.mask.is_some(), l.mask.as_ref().is_some_and(|m| m.enabled)));
            let has_mask = mask_state.is_some_and(|(h, _)| h);
            let mask_enabled = mask_state.is_some_and(|(_, e)| e);
            if mask_state.is_some() {
                ui.add_space(3.0);
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.add_enabled(!has_mask, egui::Button::new("Add Mask")).clicked() {
                        app.command(CommandId::AddLayerMask);
                    }
                    if ui.add_enabled(has_mask, egui::Button::new("Del Mask")).clicked() {
                        app.command(CommandId::DeleteLayerMask);
                    }
                    if ui.add_enabled(has_mask, egui::Button::new(if mask_enabled { "Disable" } else { "Enable" })).clicked() {
                        app.command(CommandId::ToggleLayerMask);
                    }
                });
                ui.horizontal(|ui| {
                    if ui.add_enabled(has_mask, egui::Button::new("Apply Mask")).clicked() {
                        app.command(CommandId::ApplyLayerMask);
                    }
                    if ui.add_enabled(has_mask, egui::Button::new("Invert Mask")).clicked() {
                        app.command(CommandId::InvertLayerMask);
                    }
                    if has_mask {
                        let label = if app.active_mask_editing { "Edit: Mask" } else { "Edit: Color" };
                        if ui.selectable_label(app.active_mask_editing, label).clicked() {
                            app.active_mask_editing = !app.active_mask_editing;
                        }
                    }
                });
            }
        });
}
