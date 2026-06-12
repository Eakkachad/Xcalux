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

/// Draws the sub-tool panel content without a panel_section wrapper.
/// Called from left_panel.rs which already provides the section frame.
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
