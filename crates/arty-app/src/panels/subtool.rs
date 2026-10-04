//! Sub tool list: the presets of the current tool, each with a stroke
//! preview rendered by the real brush engine.

use arty_brush::{BrushPreset, render_preview};
use egui::{Color32, CornerRadius, Sense, Stroke, TextureHandle, TextureOptions, Vec2};
use egui_phosphor::regular as icon;

use crate::shell::Shell;
use crate::studio::{Studio, Tool};

struct Cached {
    preset: BrushPreset,
    color: [f32; 3],
    size: [usize; 2],
    texture: TextureHandle,
}

/// Preview textures keyed by preset index; regenerated only when the preset,
/// preview color or row size changes.
#[derive(Default)]
pub struct PreviewCache {
    entries: Vec<Option<Cached>>,
}

impl PreviewCache {
    fn get(&mut self, ctx: &egui::Context, i: usize, preset: &BrushPreset, color: [f32; 3], size: [usize; 2]) -> &TextureHandle {
        if self.entries.len() <= i {
            self.entries.resize_with(i + 1, || None);
        }
        let fresh = matches!(&self.entries[i], Some(c) if c.preset == *preset && c.color == color && c.size == size);
        if !fresh {
            let rgba = render_preview(preset, size[0] as u32, size[1] as u32, color);
            let image = egui::ColorImage::from_rgba_premultiplied(size, &rgba);
            let texture = ctx.load_texture(format!("preset-preview-{i}"), image, TextureOptions::LINEAR);
            self.entries[i] = Some(Cached { preset: preset.clone(), color, size, texture });
        }
        &self.entries[i].as_ref().expect("filled above").texture
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

pub fn ui(ui: &mut egui::Ui, studio: &mut Studio, shell: &mut Shell, cache: &mut PreviewCache) {
    let group = match studio.tool {
        Tool::Brush(g) => g,
        _ => studio.preset().group,
    };
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(group.label()).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button(icon::ARROW_COUNTER_CLOCKWISE).on_hover_text("Restore default sub tools").clicked() {
                studio.reset_presets();
                cache.clear();
            }
            if ui.small_button(icon::COPY).on_hover_text("Duplicate sub tool").clicked() {
                studio.duplicate_preset(studio.active_preset);
                cache.clear();
            }
        });
    });
    ui.separator();

    let ppp = ui.ctx().pixels_per_point();
    let width = ui.available_width().max(60.0);
    let row_h = 46.0;
    // Quantize so resizing the panel doesn't re-render previews every frame.
    let px_w = (((width - 12.0) * ppp) as usize / 32 * 32).max(64);
    let px_h = ((row_h - 18.0) * ppp) as usize;
    let preview_color = if ui.visuals().dark_mode { [0.92, 0.92, 0.94] } else { [0.08, 0.08, 0.1] };
    let pal = shell.theme.palette();

    let mut action: Option<(usize, RowAction)> = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let indices: Vec<usize> = (0..studio.presets.len()).filter(|&i| studio.presets[i].group == group).collect();
        for i in indices {
            let selected = studio.active_preset == i && matches!(studio.tool, Tool::Brush(_));
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, row_h), Sense::click());
            let fill = if selected {
                pal.row_selected
            } else if resp.hovered() {
                pal.row_hover
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(rect, CornerRadius::same(4), fill);
            if selected {
                ui.painter().rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.0, pal.accent), egui::StrokeKind::Inside);
            }
            let preset = studio.presets[i].clone();
            ui.painter().text(
                rect.min + Vec2::new(8.0, 3.0),
                egui::Align2::LEFT_TOP,
                &preset.name,
                egui::FontId::proportional(12.0),
                ui.visuals().text_color(),
            );
            ui.painter().text(
                rect.right_top() + Vec2::new(-8.0, 3.0),
                egui::Align2::RIGHT_TOP,
                format!("{:.0}", preset.size),
                egui::FontId::proportional(10.5),
                pal.text_weak,
            );
            let tex = cache.get(ui.ctx(), i, &preset, preview_color, [px_w, px_h]);
            let img_rect = egui::Rect::from_min_size(
                rect.min + Vec2::new(6.0, 16.0),
                Vec2::new(px_w as f32 / ppp, px_h as f32 / ppp),
            );
            ui.painter().image(tex.id(), img_rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);

            if resp.clicked() {
                action = Some((i, RowAction::Select));
            }
            resp.context_menu(|ui| {
                if ui.button(format!("{}  Duplicate", icon::COPY)).clicked() {
                    action = Some((i, RowAction::Duplicate));
                    ui.close();
                }
                if ui.button(format!("{}  Delete", icon::TRASH)).clicked() {
                    action = Some((i, RowAction::Delete));
                    ui.close();
                }
            });
            ui.add_space(2.0);
        }
    });

    match action {
        Some((i, RowAction::Select)) => studio.select_preset(i),
        Some((i, RowAction::Duplicate)) => {
            studio.duplicate_preset(i);
            cache.clear();
        }
        Some((i, RowAction::Delete)) => {
            studio.delete_preset(i);
            cache.clear();
        }
        None => {}
    }
}

enum RowAction {
    Select,
    Duplicate,
    Delete,
}
