use crate::app::{BrushPresetCategory, PaintApp, PresetIcon, ToolId};

fn category_for_preset(p: &crate::app::BrushPreset) -> BrushPresetCategory {
    if p.is_eraser {
        return BrushPresetCategory::Eraser;
    }
    match p.icon {
        PresetIcon::Pencil => BrushPresetCategory::Pencil,
        PresetIcon::InkPen | PresetIcon::BinaryPen => BrushPresetCategory::Pen,
        PresetIcon::PaintBrush | PresetIcon::AirBrush | PresetIcon::Marker => {
            BrushPresetCategory::Brush
        }
        PresetIcon::Smudge | PresetIcon::Water => BrushPresetCategory::Blend,
        _ => BrushPresetCategory::Utility,
    }
}

fn preset_matches_filter(app: &PaintApp, idx: usize) -> bool {
    let preset = &app.presets[idx];
    let category_ok = app.brush_ui.selected_category == BrushPresetCategory::All
        || category_for_preset(preset) == app.brush_ui.selected_category;
    let search = app.brush_ui.search.trim().to_lowercase();
    let search_ok = search.is_empty() || preset.name.to_lowercase().contains(&search);
    category_ok && search_ok
}

fn draw_preset_preview(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    preset: &crate::app::BrushPreset,
) {
    let painter = ui.painter();
    let cell = 4.0;
    let cols = ((rect.width() / cell).ceil() as i32).max(1);
    let rows = ((rect.height() / cell).ceil() as i32).max(1);
    for yi in 0..rows {
        for xi in 0..cols {
            if (xi + yi) % 2 == 1 {
                painter.rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(rect.min.x + xi as f32 * cell, rect.min.y + yi as f32 * cell),
                        egui::Vec2::splat(cell),
                    ),
                    0.0,
                    egui::Color32::from_gray(228),
                );
            }
        }
    }
    painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.0, egui::Color32::from_gray(180)));

    let num_dabs: usize = 10;
    let max_r = (2.0 + preset.radius_log.max(0.0) * 2.5).min(10.0).max(1.0);
    let stroke_opacity = preset.opacity;
    let density = preset.density;
    let hardness = preset.hardness;

    let (r_base, g_base, b_base) = if preset.is_eraser {
        (140u8, 140u8, 140u8)
    } else {
        (200u8, 70u8, 110u8)
    };

    for i in 0..num_dabs {
        let t = i as f32 / (num_dabs - 1).max(1) as f32;
        let pressure = (t * std::f32::consts::PI).sin().max(0.01);
        let r = (0.6 + pressure * max_r * density).max(0.3);
        let x = rect.min.x + 4.0 + t * (rect.width() - 8.0);
        let y = rect.center().y + (t - 0.5).sin() * 4.0;
        let alpha = ((1.0 - hardness * (1.0 - pressure) * 0.4) * stroke_opacity * 200.0) as u8 + 30;
        let col = egui::Color32::from_rgba_unmultiplied(r_base, g_base, b_base, alpha);
        painter.circle_filled(egui::pos2(x, y), r, col);
    }
    for i in 0..num_dabs.saturating_sub(1) {
        let t1 = i as f32 / (num_dabs - 1).max(1) as f32;
        let t2 = (i + 1) as f32 / (num_dabs - 1).max(1) as f32;
        let p1 = egui::pos2(
            rect.min.x + 4.0 + t1 * (rect.width() - 8.0),
            rect.center().y + (t1 - 0.5).sin() * 4.0,
        );
        let p2 = egui::pos2(
            rect.min.x + 4.0 + t2 * (rect.width() - 8.0),
            rect.center().y + (t2 - 0.5).sin() * 4.0,
        );
        let (lr, lg, lb) = if preset.is_eraser {
            (120u8, 120u8, 120u8)
        } else {
            (160u8, 40u8, 70u8)
        };
        let line_alpha = (stroke_opacity * 180.0) as u8 + 30;
        painter.line_segment(
            [p1, p2],
            egui::Stroke::new(
                0.5,
                egui::Color32::from_rgba_unmultiplied(lr, lg, lb, line_alpha),
            ),
        );
    }

    if preset.texture_id > 0 {
        let num_dots = 4;
        let dot_dist = (rect.width() * 0.15).max(4.0);
        for i in 0..num_dots {
            let a = i as f32 * std::f32::consts::TAU / num_dots as f32;
            let dp = rect.center() + egui::vec2(a.cos(), a.sin()) * dot_dist;
            let dot_alpha = (stroke_opacity * 160.0) as u8;
            let dot_col = if preset.is_eraser {
                egui::Color32::from_rgba_premultiplied(100, 100, 100, dot_alpha)
            } else {
                egui::Color32::from_rgba_premultiplied(60, 60, 60, dot_alpha)
            };
            painter.circle_filled(dp, 1.2, dot_col);
        }
    }
}

fn draw_category_tabs(ui: &mut egui::Ui, app: &mut PaintApp) {
    let categories = [
        (BrushPresetCategory::All, "All"),
        (BrushPresetCategory::Pencil, "Pencil"),
        (BrushPresetCategory::Pen, "Pen"),
        (BrushPresetCategory::Brush, "Brush"),
        (BrushPresetCategory::Eraser, "Eraser"),
        (BrushPresetCategory::Blend, "Blend"),
        (BrushPresetCategory::Utility, "Utility"),
    ];
    ui.horizontal(|ui| {
        let spacing = ui.spacing().item_spacing.x;
        let total_spacing = spacing * (categories.len() - 1) as f32;
        let tab_w = (ui.available_width() - total_spacing) / categories.len() as f32;
        for (cat, label) in &categories {
            let selected = app.brush_ui.selected_category == *cat;
            let btn = egui::Button::new(*label).selected(selected);
            let resp = ui.add_sized(egui::vec2(tab_w, 20.0), btn);
            if resp.clicked() {
                app.brush_ui.selected_category = *cat;
                app.brush_ui.search.clear();
                resp.surrender_focus();
            }
        }
    });
}

fn draw_preset_row(ui: &mut egui::Ui, app: &mut PaintApp, idx: usize) {
    let selected = app.active_preset_index == idx
        && matches!(app.active_tool(), ToolId::Brush | ToolId::Eraser);
    let row_height = if app.brush_ui.compact_rows {
        28.0
    } else {
        42.0
    };

    let preset_name = app.presets[idx].name.clone();
    let preset_icon = app.presets[idx].icon;

    let available_width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(available_width, row_height),
        egui::Sense::click_and_drag(),
    );

    if selected {
        ui.painter()
            .rect_filled(rect, 3.0, egui::Color32::from_rgb(170, 200, 240));
    } else if response.hovered() {
        ui.painter()
            .rect_filled(rect, 3.0, egui::Color32::from_rgb(225, 235, 250));
    }

    let icon_char = match preset_icon {
        PresetIcon::Pencil => "\u{270E}",
        PresetIcon::InkPen => "\u{270F}",
        PresetIcon::PaintBrush => "\u{1F58C}",
        PresetIcon::Smudge => "\u{1F32D}",
        PresetIcon::Eraser => "\u{2B1B}",
        PresetIcon::AirBrush => "\u{2601}",
        PresetIcon::Water => "\u{1F4A7}",
        PresetIcon::Marker => "\u{1F4DD}",
        PresetIcon::BinaryPen => "\u{1D11E}",
    };

    let icon_pos = egui::pos2(rect.left() + 4.0, rect.center().y);
    ui.painter().text(
        icon_pos,
        egui::Align2::LEFT_CENTER,
        icon_char,
        egui::FontId::proportional(12.0),
        egui::Color32::from_gray(40),
    );

    let preview_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left() + 20.0, rect.top() + 2.0),
        egui::vec2(rect.width() * 0.28, row_height - 4.0),
    );

    draw_preset_preview(ui, preview_rect, &app.presets[idx]);

    let label_x = rect.left() + 20.0 + preview_rect.width() + 6.0;
    let name_pos = egui::pos2(label_x, rect.center().y - 5.0);
    ui.painter().text(
        name_pos,
        egui::Align2::LEFT_CENTER,
        &preset_name,
        egui::FontId::proportional(11.0),
        egui::Color32::from_gray(20),
    );

    let size_px = app.presets[idx].radius_log.exp();
    let opacity_pct = (app.presets[idx].opacity * 100.0) as i32;
    let meta_pos = egui::pos2(label_x, rect.center().y + 7.0);
    ui.painter().text(
        meta_pos,
        egui::Align2::LEFT_CENTER,
        format!("{:.0}px / {}%", size_px, opacity_pct),
        egui::FontId::proportional(8.0),
        egui::Color32::from_gray(100),
    );

    if response.clicked() {
        app.select_preset(idx);
        response.surrender_focus();
    }

    if response.double_clicked() {
        app.renaming_preset_index = Some(idx);
        app.rename_input = app.presets[idx].name.clone();
    }

    response.context_menu(|ui| {
        if ui.button("Rename").clicked() {
            app.renaming_preset_index = Some(idx);
            app.rename_input = app.presets[idx].name.clone();
            ui.close_menu();
        }
        if ui.button("Duplicate").clicked() {
            app.duplicate_preset(idx);
            ui.close_menu();
        }
        if ui.button("Delete").clicked() {
            app.delete_preset(idx);
            ui.close_menu();
        }
        ui.separator();
        if ui.button("Move Up").clicked() {
            if idx > 0 {
                app.reorder_preset(idx, idx - 1);
            }
            ui.close_menu();
        }
        if ui.button("Move Down").clicked() {
            if idx + 1 < app.presets.len() {
                app.reorder_preset(idx, idx + 2);
            }
            ui.close_menu();
        }
    });
}

pub fn draw_sub_tool_panel_inline(ui: &mut egui::Ui, app: &mut PaintApp) {
    ui.horizontal(|ui| {
        ui.add_sized(
            egui::vec2(ui.available_width() - 20.0, 0.0),
            egui::TextEdit::singleline(&mut app.brush_ui.search)
                .hint_text("Search presets..."),
        );
        let btn_resp = ui
            .small_button("+")
            .on_hover_text("Add preset");
        if btn_resp.clicked() {
            app.create_preset(PresetIcon::PaintBrush);
        }
    });

    draw_category_tabs(ui, app);

    egui::ScrollArea::vertical()
        .max_height(240.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for idx in 0..app.presets.len() {
                if preset_matches_filter(app, idx) {
                    draw_preset_row(ui, app, idx);
                }
            }
        });
}

fn tool_button(ui: &mut egui::Ui, app: &mut PaintApp, id: ToolId, label: &str, shortcut: &str) {
    let active = app.active_tool() == id;
    let button_text = if shortcut.is_empty() {
        label.to_string()
    } else {
        format!("{} [{}]", label, shortcut)
    };

    let btn = egui::Button::new(button_text).selected(active);
    let resp = ui.add_sized([ui.available_width() / 2.0 - 4.0, 26.0], btn)
        .on_hover_text(id.name());

    if resp.clicked() {
        if id == ToolId::Brush {
            app.select_last_brush();
        } else if id == ToolId::Eraser {
            app.select_last_eraser();
        } else {
            app.set_active_tool(id);
        }
        resp.surrender_focus();
    }
}

fn select_tool_button_with_menu(
    ui: &mut egui::Ui,
    app: &mut PaintApp,
    active: bool,
    label: &str,
    shortcut: &str,
    add_contents: impl FnOnce(&mut egui::Ui, &mut PaintApp),
    hover_text: &str,
) -> egui::Response {
    let button_text = if shortcut.is_empty() {
        label.to_string()
    } else {
        format!("{} [{}]", label, shortcut)
    };

    let btn = egui::Button::new(button_text).selected(active);
    let resp = ui.add_sized([ui.available_width() / 2.0 - 4.0, 26.0], btn)
        .on_hover_text(hover_text);

    resp.context_menu(|ui| {
        add_contents(ui, app);
    });

    resp
}

fn draw_tool_palette(ui: &mut egui::Ui, app: &mut PaintApp) {
    ui.label(egui::RichText::new("Tool Palette").strong());
    egui::Grid::new("csp_tool_grid")
        .num_columns(2)
        .spacing([6.0, 6.0])
        .show(ui, |ui| {
            // Row 1: Brush / Eraser
            tool_button(ui, app, ToolId::Brush, "🎨 Brush", "B");
            tool_button(ui, app, ToolId::Eraser, "🗑 Eraser", "E");
            ui.end_row();

            // Row 2: Fill / Gradient
            tool_button(ui, app, ToolId::Fill, "Fill", "G");
            tool_button(ui, app, ToolId::Gradient, "Grad", "Shift+G");
            ui.end_row();

            // Row 3: Selection / Lasso
            let is_shape_active = app.active_tool() == ToolId::RectSelect || app.active_tool() == ToolId::EllipseSelect;
            let shape_label = if app.active_tool() == ToolId::EllipseSelect { "○ Ellipse" } else { "⬜ Rect" };
            let shape_resp = select_tool_button_with_menu(ui, app, is_shape_active, shape_label, "M", |ui, app| {
                if ui.selectable_label(app.active_tool() == ToolId::RectSelect, "Rectangle Selection").clicked() {
                    app.set_active_tool(ToolId::RectSelect);
                    ui.close_menu();
                }
                if ui.selectable_label(app.active_tool() == ToolId::EllipseSelect, "Ellipse Selection").clicked() {
                    app.set_active_tool(ToolId::EllipseSelect);
                    ui.close_menu();
                }
            }, "Selection Tool (Right-click to change shape)");
            if shape_resp.clicked() {
                let current = if app.active_tool() == ToolId::EllipseSelect { ToolId::EllipseSelect } else { ToolId::RectSelect };
                app.set_active_tool(current);
            }

            let is_lasso_active = app.active_tool() == ToolId::Lasso || app.active_tool() == ToolId::PolygonLasso;
            let lasso_label = if app.active_tool() == ToolId::PolygonLasso { "⬡ Poly Lasso" } else { "🪃 Lasso" };
            let lasso_resp = select_tool_button_with_menu(ui, app, is_lasso_active, lasso_label, "L", |ui, app| {
                if ui.selectable_label(app.active_tool() == ToolId::Lasso, "Free Lasso Selection").clicked() {
                    app.set_active_tool(ToolId::Lasso);
                    ui.close_menu();
                }
                if ui.selectable_label(app.active_tool() == ToolId::PolygonLasso, "Polygon Lasso Selection").clicked() {
                    app.set_active_tool(ToolId::PolygonLasso);
                    ui.close_menu();
                }
            }, "Lasso Tool (Right-click to change mode)");
            if lasso_resp.clicked() {
                let current = if app.active_tool() == ToolId::PolygonLasso { ToolId::PolygonLasso } else { ToolId::Lasso };
                app.set_active_tool(current);
            }
            ui.end_row();

            // Row 4: Magic Wand / Move
            tool_button(ui, app, ToolId::MagicWand, "Wand", "W");
            tool_button(ui, app, ToolId::Move, "Move", "V");
            ui.end_row();

            // Row 5: Transform / Color Picker
            tool_button(ui, app, ToolId::Transform, "Trans", "Ctrl+T");
            tool_button(ui, app, ToolId::ColorPicker, "Pick", "I");
            ui.end_row();

            // Row 6: Hand / Zoom
            tool_button(ui, app, ToolId::Hand, "Hand", "Space");
            tool_button(ui, app, ToolId::Zoom, "Zoom", "");
            ui.end_row();
        });
}

fn draw_brush_size_panel(ui: &mut egui::Ui, app: &mut PaintApp) {
    ui.label(egui::RichText::new("Brush Size").strong());
    
    let mut radius_px = app.brush_radius_log.exp();
    
    ui.horizontal(|ui| {
        let changed = ui.add(
            egui::Slider::new(&mut radius_px, 0.5..=300.0)
                .logarithmic(true)
                .show_value(false)
        ).changed();
        
        let drag_changed = ui.add(
            egui::DragValue::new(&mut radius_px)
                .speed(0.2)
                .clamp_range(0.5..=300.0)
                .suffix(" px")
        ).changed();
        
        if changed || drag_changed {
            app.brush_radius_log = radius_px.max(0.1).ln();
            app.brush_settings_dirty = true;
        }
    });

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
        for size in app.brush_ui.favorite_sizes_px.clone() {
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::click());
            let active = (app.brush_radius_log.exp() - size).abs() < 0.15;
            
            let bg_color = if active {
                ui.visuals().selection.bg_fill
            } else if resp.hovered() {
                ui.visuals().widgets.hovered.bg_fill
            } else {
                ui.visuals().widgets.inactive.bg_fill
            };
            ui.painter().rect_filled(rect, 3.0, bg_color);
            
            if active {
                ui.painter().rect_stroke(rect, 3.0, egui::Stroke::new(1.0, ui.visuals().selection.stroke.color));
            }
            
            let circle_center = rect.center();
            let max_val = app.brush_ui.favorite_sizes_px.iter().cloned().fold(1.0f32, f32::max);
            let visual_radius = (1.0 + (size.max(1.0).ln() / max_val.max(1.1).ln()) * 8.0).clamp(1.0, 10.0);
            
            let circle_color = if active {
                ui.visuals().selection.stroke.color
            } else if resp.hovered() {
                ui.visuals().widgets.hovered.fg_stroke.color
            } else {
                ui.visuals().widgets.inactive.fg_stroke.color
            };
            
            ui.painter().circle_filled(circle_center, visual_radius, circle_color);
            
            let resp = resp.on_hover_text(format!("{} px", size));
            if resp.clicked() {
                app.brush_radius_log = size.max(0.1).ln();
                app.brush_settings_dirty = true;
            }
        }
    });
}

fn draw_brush_properties_content(ui: &mut egui::Ui, app: &mut PaintApp) {
    // Basic settings
    ui.horizontal(|ui| {
        ui.label("Opacity");
        let mut opacity_pct = (app.brush_opacity * 100.0) as i32;
        let slider_changed = ui.add(
            egui::Slider::new(&mut app.brush_opacity, 0.0..=1.0)
                .show_value(false)
        ).changed();
        let drag_changed = ui.add(
            egui::DragValue::new(&mut opacity_pct)
                .speed(1)
                .clamp_range(0..=100)
                .suffix("%")
        ).changed();
        if slider_changed || drag_changed {
            if drag_changed {
                app.brush_opacity = (opacity_pct as f32 / 100.0).clamp(0.0, 1.0);
            }
            app.brush_settings_dirty = true;
        }
    });

    ui.horizontal(|ui| {
        ui.label("Hardness");
        let slider_changed = ui.add(
            egui::Slider::new(&mut app.brush_hardness, 0.0..=1.0)
                .show_value(false)
        ).changed();
        let drag_changed = ui.add(
            egui::DragValue::new(&mut app.brush_hardness)
                .speed(0.01)
                .clamp_range(0.0..=1.0)
        ).changed();
        if slider_changed || drag_changed {
            app.brush_settings_dirty = true;
        }
    });

    ui.horizontal(|ui| {
        ui.label("Min Size");
        let slider_changed = ui.add(
            egui::Slider::new(&mut app.brush_min_size_fraction, 0.0..=1.0)
                .show_value(false)
        ).changed();
        let drag_changed = ui.add(
            egui::DragValue::new(&mut app.brush_min_size_fraction)
                .speed(0.01)
                .clamp_range(0.0..=1.0)
                .suffix("%")
        ).changed();
        if slider_changed || drag_changed {
            app.brush_settings_dirty = true;
        }
    });

    ui.horizontal(|ui| {
        ui.label("Density");
        let slider_changed = ui.add(
            egui::Slider::new(&mut app.brush_density, 0.0..=1.0)
                .show_value(false)
        ).changed();
        let drag_changed = ui.add(
            egui::DragValue::new(&mut app.brush_density)
                .speed(0.01)
                .clamp_range(0.0..=1.0)
                .suffix("%")
        ).changed();
        if slider_changed || drag_changed {
            app.brush_settings_dirty = true;
        }
    });

    // Stabilizer
    ui.horizontal(|ui| {
        ui.label("Stabilizer");
        let current_level = app.stabilizer.level;
        let text = match current_level {
            crate::input::StabilizerLevel::Off => "Off".to_string(),
            crate::input::StabilizerLevel::Level(val) => format!("Level {}", val),
            crate::input::StabilizerLevel::SLevel(val) => format!("S-{}", val),
        };
        let response = egui::ComboBox::from_id_source("sai_stabilizer_level_csp")
            .selected_text(text)
            .width(80.0)
            .show_ui(ui, |ui| {
                let mut selected = false;
                if ui.selectable_label(matches!(current_level, crate::input::StabilizerLevel::Off), "Off").clicked() {
                    app.stabilizer.set_level(crate::input::StabilizerLevel::Off);
                    selected = true;
                }
                for val in 1..=15 {
                    let is_sel = matches!(current_level, crate::input::StabilizerLevel::Level(v) if v == val);
                    if ui.selectable_label(is_sel, format!("Level {}", val)).clicked() {
                        app.stabilizer.set_level(crate::input::StabilizerLevel::Level(val));
                        selected = true;
                    }
                }
                for val in 1..=5 {
                    let is_sel = matches!(current_level, crate::input::StabilizerLevel::SLevel(v) if v == val);
                    if ui.selectable_label(is_sel, format!("S-{}", val)).clicked() {
                        app.stabilizer.set_level(crate::input::StabilizerLevel::SLevel(val));
                        selected = true;
                    }
                }
                selected
            });
        if response.inner.unwrap_or(false) {
            app.brush_settings_dirty = true;
        }
    });

    ui.add_space(2.0);

    // Collapsible Advanced Settings
    egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        ui.next_auto_id(),
        false,
    )
    .show_header(ui, |ui| {
        ui.label("Advanced Properties");
    })
    .body(|ui| {
        // Blending
        ui.horizontal(|ui| {
            ui.label("Blending");
            let slider_changed = ui.add(
                egui::Slider::new(&mut app.brush_color_blending, 0.0..=1.0)
                    .show_value(false),
            ).changed();
            let drag_changed = ui.add(
                egui::DragValue::new(&mut app.brush_color_blending)
                    .speed(0.01)
                    .clamp_range(0.0..=1.0)
            ).changed();
            if slider_changed || drag_changed {
                app.brush_settings_dirty = true;
            }
        });

        // Dilution
        ui.horizontal(|ui| {
            ui.label("Dilution");
            let slider_changed = ui.add(
                egui::Slider::new(&mut app.brush_dilution, 0.0..=1.0)
                    .show_value(false)
            ).changed();
            let drag_changed = ui.add(
                egui::DragValue::new(&mut app.brush_dilution)
                    .speed(0.01)
                    .clamp_range(0.0..=1.0)
            ).changed();
            if slider_changed || drag_changed {
                app.brush_settings_dirty = true;
            }
        });

        // Spacing
        ui.horizontal(|ui| {
            ui.label("Spacing");
            let slider_changed = ui.add(
                egui::Slider::new(&mut app.brush_spacing, 0.01..=2.0)
                    .show_value(false)
            ).changed();
            let drag_changed = ui.add(
                egui::DragValue::new(&mut app.brush_spacing)
                    .speed(0.01)
                    .clamp_range(0.01..=2.0)
            ).changed();
            if slider_changed || drag_changed {
                app.brush_settings_dirty = true;
            }
        });

        // Bristle ID
        ui.horizontal(|ui| {
            ui.label("Bristle ID");
            if ui.add(
                egui::DragValue::new(&mut app.brush_bristle_id)
                    .speed(1)
                    .clamp_range(0..=5),
            ).changed() {
                app.brush_settings_dirty = true;
            }
        });

        // Texture
        let texture_name = app.brush_textures
            .get(app.brush_texture_id as usize)
            .map(|t| t.name.as_str())
            .unwrap_or("None");
        let has_texture = app.brush_texture_id > 0;
        
        ui.horizontal(|ui| {
            ui.label("Texture:");
            let mut selected_tex = app.brush_texture_id;
            let res = egui::ComboBox::from_id_source("brush_texture_combo_csp")
                .selected_text(texture_name)
                .width(100.0)
                .show_ui(ui, |ui| {
                    let mut changed = false;
                    for (idx, tex) in app.brush_textures.iter().enumerate() {
                        if ui.selectable_value(&mut selected_tex, idx as u32, &tex.name).clicked() {
                            changed = true;
                        }
                    }
                    changed
                });
            if res.inner.unwrap_or(false) {
                app.brush_texture_id = selected_tex;
                app.brush_settings_dirty = true;
            }
        });

        if has_texture {
            ui.horizontal(|ui| {
                ui.label("Scale");
                if ui.add(
                    egui::Slider::new(&mut app.brush_texture_scale, 0.1..=10.0)
                        .show_value(false),
                ).changed() {
                    app.brush_settings_dirty = true;
                }
                if ui.add(egui::DragValue::new(&mut app.brush_texture_scale).speed(0.1)).changed() {
                    app.brush_settings_dirty = true;
                }
            });
        }
    });
}

fn draw_tool_property_panel(ui: &mut egui::Ui, app: &mut PaintApp) {
    ui.label(egui::RichText::new("Tool Property").strong());

    match app.active_tool() {
        ToolId::Brush | ToolId::Eraser => {
            draw_brush_properties_content(ui, app);
        }
        ToolId::Fill => {
            ui.horizontal(|ui| {
                ui.label("Detection:");
                egui::ComboBox::from_id_source("fill_detection")
                    .selected_text(match app.fill_options.detection_mode {
                        crate::tools::fill::FillDetectionMode::TransparencyStrict => "Transp Strict",
                        crate::tools::fill::FillDetectionMode::TransparencyFuzzy => "Transp Fuzzy",
                        crate::tools::fill::FillDetectionMode::ColorDifference => "Color Diff",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut app.fill_options.detection_mode,
                            crate::tools::fill::FillDetectionMode::TransparencyStrict,
                            "Transp Strict",
                        );
                        ui.selectable_value(
                            &mut app.fill_options.detection_mode,
                            crate::tools::fill::FillDetectionMode::TransparencyFuzzy,
                            "Transp Fuzzy",
                        );
                        ui.selectable_value(
                            &mut app.fill_options.detection_mode,
                            crate::tools::fill::FillDetectionMode::ColorDifference,
                            "Color Diff",
                        );
                    });
            });
            match app.fill_options.detection_mode {
                crate::tools::fill::FillDetectionMode::ColorDifference => {
                    ui.horizontal(|ui| {
                        ui.label("Color Diff:");
                        ui.add(egui::Slider::new(&mut app.fill_options.tolerance, 0..=255));
                    });
                }
                crate::tools::fill::FillDetectionMode::TransparencyFuzzy => {
                    ui.horizontal(|ui| {
                        ui.label("Transp Diff:");
                        ui.add(egui::Slider::new(&mut app.fill_options.transp_diff, 0..=255));
                    });
                }
                crate::tools::fill::FillDetectionMode::TransparencyStrict => {}
            }
            ui.horizontal(|ui| {
                ui.label("Reference:");
                egui::ComboBox::from_id_source("fill_reference")
                    .selected_text(match app.fill_options.reference {
                        crate::tools::fill::FillReference::CurrentLayer => "Current Layer",
                        crate::tools::fill::FillReference::SelectionSourceLayers => "Reference Layers",
                        crate::tools::fill::FillReference::AllVisibleLayers => "All Visible",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut app.fill_options.reference,
                            crate::tools::fill::FillReference::CurrentLayer,
                            "Current Layer",
                        );
                        ui.selectable_value(
                            &mut app.fill_options.reference,
                            crate::tools::fill::FillReference::SelectionSourceLayers,
                            "Reference Layers",
                        );
                        ui.selectable_value(
                            &mut app.fill_options.reference,
                            crate::tools::fill::FillReference::AllVisibleLayers,
                            "All Visible",
                        );
                    });
            });
            let has_ref = app.layers.values().any(|l| l.selection_source);
            if app.fill_options.reference == crate::tools::fill::FillReference::SelectionSourceLayers && !has_ref {
                ui.colored_label(
                    egui::Color32::RED,
                    "⚠ No reference layer selected!\nEnable Ref (◎) on a lineart layer.",
                );
            }
            ui.horizontal(|ui| {
                ui.label("Expand:");
                ui.add(egui::Slider::new(&mut app.fill_options.expand_px, 0..=10));
            });
            ui.checkbox(&mut app.fill_options.contiguous, "Contiguous");
            ui.checkbox(&mut app.fill_options.antialias, "Anti-alias");
            ui.checkbox(&mut app.fill_options.respect_selection, "Respect selection");
            ui.checkbox(&mut app.fill_options.fill_transparent_only, "Fill transparent only");
        }
        ToolId::RectSelect | ToolId::EllipseSelect | ToolId::Lasso | ToolId::PolygonLasso => {
            ui.horizontal(|ui| {
                ui.label("Mode:");
                egui::ComboBox::from_id_source("sel_mode")
                    .selected_text(match app.selection_mode {
                        crate::tools::selection::SelectionMode::Replace => "Replace",
                        crate::tools::selection::SelectionMode::Add => "Add",
                        crate::tools::selection::SelectionMode::Subtract => "Subtract",
                        crate::tools::selection::SelectionMode::Intersect => "Intersect",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut app.selection_mode,
                            crate::tools::selection::SelectionMode::Replace,
                            "Replace",
                        );
                        ui.selectable_value(
                            &mut app.selection_mode,
                            crate::tools::selection::SelectionMode::Add,
                            "Add",
                        );
                        ui.selectable_value(
                            &mut app.selection_mode,
                            crate::tools::selection::SelectionMode::Subtract,
                            "Subtract",
                        );
                        ui.selectable_value(
                            &mut app.selection_mode,
                            crate::tools::selection::SelectionMode::Intersect,
                            "Intersect",
                        );
                    });
            });
            ui.horizontal(|ui| {
                ui.label("Feather:");
                ui.add(egui::Slider::new(&mut app.selection_feather, 0.0..=100.0));
            });
        }
        ToolId::MagicWand => {
            ui.horizontal(|ui| {
                ui.label("Mode:");
                egui::ComboBox::from_id_source("wand_sel_mode")
                    .selected_text(match app.selection_mode {
                        crate::tools::selection::SelectionMode::Replace => "Replace",
                        crate::tools::selection::SelectionMode::Add => "Add",
                        crate::tools::selection::SelectionMode::Subtract => "Subtract",
                        crate::tools::selection::SelectionMode::Intersect => "Intersect",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut app.selection_mode,
                            crate::tools::selection::SelectionMode::Replace,
                            "Replace",
                        );
                        ui.selectable_value(
                            &mut app.selection_mode,
                            crate::tools::selection::SelectionMode::Add,
                            "Add",
                        );
                        ui.selectable_value(
                            &mut app.selection_mode,
                            crate::tools::selection::SelectionMode::Subtract,
                            "Subtract",
                        );
                        ui.selectable_value(
                            &mut app.selection_mode,
                            crate::tools::selection::SelectionMode::Intersect,
                            "Intersect",
                        );
                    });
            });
            ui.horizontal(|ui| {
                ui.label("Detection:");
                egui::ComboBox::from_id_source("wand_detection")
                    .selected_text(match app.fill_options.detection_mode {
                        crate::tools::fill::FillDetectionMode::TransparencyStrict => "Transp Strict",
                        crate::tools::fill::FillDetectionMode::TransparencyFuzzy => "Transp Fuzzy",
                        crate::tools::fill::FillDetectionMode::ColorDifference => "Color Diff",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut app.fill_options.detection_mode,
                            crate::tools::fill::FillDetectionMode::TransparencyStrict,
                            "Transp Strict",
                        );
                        ui.selectable_value(
                            &mut app.fill_options.detection_mode,
                            crate::tools::fill::FillDetectionMode::TransparencyFuzzy,
                            "Transp Fuzzy",
                        );
                        ui.selectable_value(
                            &mut app.fill_options.detection_mode,
                            crate::tools::fill::FillDetectionMode::ColorDifference,
                            "Color Diff",
                        );
                    });
            });
            match app.fill_options.detection_mode {
                crate::tools::fill::FillDetectionMode::ColorDifference => {
                    ui.horizontal(|ui| {
                        ui.label("Color Diff:");
                        ui.add(egui::Slider::new(&mut app.fill_options.tolerance, 0..=255));
                    });
                }
                crate::tools::fill::FillDetectionMode::TransparencyFuzzy => {
                    ui.horizontal(|ui| {
                        ui.label("Transp Diff:");
                        ui.add(egui::Slider::new(&mut app.fill_options.transp_diff, 0..=255));
                    });
                }
                crate::tools::fill::FillDetectionMode::TransparencyStrict => {}
            }
            ui.horizontal(|ui| {
                ui.label("Reference:");
                egui::ComboBox::from_id_source("wand_reference")
                    .selected_text(match app.fill_options.reference {
                        crate::tools::fill::FillReference::CurrentLayer => "Current Layer",
                        crate::tools::fill::FillReference::SelectionSourceLayers => "Reference Layers",
                        crate::tools::fill::FillReference::AllVisibleLayers => "All Visible",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut app.fill_options.reference,
                            crate::tools::fill::FillReference::CurrentLayer,
                            "Current Layer",
                        );
                        ui.selectable_value(
                            &mut app.fill_options.reference,
                            crate::tools::fill::FillReference::SelectionSourceLayers,
                            "Reference Layers",
                        );
                        ui.selectable_value(
                            &mut app.fill_options.reference,
                            crate::tools::fill::FillReference::AllVisibleLayers,
                            "All Visible",
                        );
                    });
            });
            let has_ref = app.layers.values().any(|l| l.selection_source);
            if app.fill_options.reference == crate::tools::fill::FillReference::SelectionSourceLayers && !has_ref {
                ui.colored_label(
                    egui::Color32::RED,
                    "⚠ No reference layer selected!\nEnable Ref (◎) on a lineart layer.",
                );
            }
            ui.horizontal(|ui| {
                ui.label("Expand:");
                ui.add(egui::Slider::new(&mut app.fill_options.expand_px, 0..=10));
            });
            ui.checkbox(&mut app.fill_options.contiguous, "Contiguous");
            ui.checkbox(&mut app.fill_options.antialias, "Anti-alias");
        }
        ToolId::Transform => {
            ui.horizontal(|ui| {
                ui.label("Interp:");
                egui::ComboBox::from_id_source("interp")
                    .selected_text(match app.transform_state.interpolation {
                        crate::tools::transform::InterpolationMode::Nearest => "Nearest",
                        crate::tools::transform::InterpolationMode::Bilinear => "Bilinear",
                        crate::tools::transform::InterpolationMode::Bicubic => "Bicubic",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut app.transform_state.interpolation,
                            crate::tools::transform::InterpolationMode::Nearest,
                            "Nearest",
                        );
                        ui.selectable_value(
                            &mut app.transform_state.interpolation,
                            crate::tools::transform::InterpolationMode::Bilinear,
                            "Bilinear",
                        );
                        ui.selectable_value(
                            &mut app.transform_state.interpolation,
                            crate::tools::transform::InterpolationMode::Bicubic,
                            "Bicubic",
                        );
                    });
            });
            ui.add_space(4.0);
            if app.transform_active {
                ui.horizontal(|ui| {
                    if ui.button("✓ Apply").on_hover_text("Apply transform (Enter)").clicked() {
                        app.commit_transform();
                    }
                    if ui.button("❌ Cancel").on_hover_text("Cancel transform (Esc)").clicked() {
                        app.cancel_transform();
                    }
                });
            } else {
                ui.colored_label(egui::Color32::from_gray(120), "Press Ctrl+T to start transform");
            }
        }
        ToolId::Gradient => {
            ui.horizontal(|ui| {
                ui.label("Mode:");
                let mut gm = app.gradient_mode;
                egui::ComboBox::from_id_source("gradient_mode")
                    .selected_text(if gm == 0 { "Linear" } else { "Radial" })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut gm, 0u32, "Linear");
                        ui.selectable_value(&mut gm, 1u32, "Radial");
                    });
                if gm != app.gradient_mode {
                    app.gradient_mode = gm;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Type:");
                let mut gt = app.gradient_type;
                egui::ComboBox::from_id_source("gradient_type")
                    .selected_text(match gt {
                        0 => "FG→BG",
                        1 => "FG→Transparent",
                        _ => "BG→Transparent",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut gt, 0u32, "FG→BG");
                        ui.selectable_value(&mut gt, 1u32, "FG→Transparent");
                        ui.selectable_value(&mut gt, 2u32, "BG→Transparent");
                    });
                if gt != app.gradient_type {
                    app.gradient_type = gt;
                }
            });
        }
        ToolId::ColorPicker => {
            ui.colored_label(egui::Color32::from_gray(120), "Picks color from canvas");
        }
        _ => {
            ui.colored_label(egui::Color32::from_gray(120), "No properties for this tool");
        }
    }
}

pub fn draw_csp_brush_workspace(ui: &mut egui::Ui, app: &mut PaintApp) {
    draw_tool_palette(ui, app);
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);

    draw_sub_tool_panel_inline(ui, app);
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);

    draw_brush_size_panel(ui, app);
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);

    draw_tool_property_panel(ui, app);
}
