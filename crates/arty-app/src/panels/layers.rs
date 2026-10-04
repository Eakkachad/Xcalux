//! Layer panel: active layer settings, the layer list and layer actions.

use arty_core::{BlendMode, LayerContent, LayerId};
use egui::{Color32, CornerRadius, Sense, Stroke, Vec2};
use egui_phosphor::regular as icon;

use crate::commands::{self, Command};
use crate::shell::Shell;
use crate::studio::Studio;

pub fn ui(ui: &mut egui::Ui, studio: &mut Studio, shell: &mut Shell) {
    active_layer_controls(ui, studio);
    ui.separator();

    let footer_h = 28.0;
    let list_h = (ui.available_height() - footer_h).max(40.0);
    let mut rows = Vec::new();
    studio.doc.panel_rows(&mut rows);
    egui::ScrollArea::vertical().max_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
        for (id, depth) in rows {
            layer_row(ui, studio, shell, id, depth);
        }
    });

    ui.separator();
    ui.horizontal(|ui| {
        let buttons: &[(&str, Command)] = &[
            (icon::FILE_PLUS, Command::NewLayer),
            (icon::FOLDER_SIMPLE_PLUS, Command::NewFolder),
            (icon::COPY, Command::DuplicateLayer),
            (icon::ARROW_LINE_DOWN, Command::MergeDown),
            (icon::ARROW_UP, Command::LayerUp),
            (icon::ARROW_DOWN, Command::LayerDown),
            (icon::BROOM, Command::ClearLayer),
            (icon::TRASH, Command::DeleteLayer),
        ];
        for &(glyph, cmd) in buttons {
            let mut tip = cmd.label().to_string();
            if let Some(sc) = commands::shortcut_for(cmd) {
                tip = format!("{tip} ({})", ui.ctx().format_shortcut(&sc));
            }
            if ui.button(glyph).on_hover_text(tip).clicked() {
                commands::execute(cmd, studio, shell);
            }
        }
    });
}

fn active_layer_controls(ui: &mut egui::Ui, studio: &mut Studio) {
    let id = studio.doc.active();
    let Some(layer) = studio.doc.layer(id) else { return };
    let is_folder = layer.is_folder();
    let mut p = layer.props.clone();
    let mut coalesce = false;
    let (mut drag_started, mut drag_stopped) = (false, false);

    ui.horizontal(|ui| {
        let modes: Vec<BlendMode> = if is_folder {
            std::iter::once(BlendMode::PassThrough).chain(BlendMode::LAYER_MODES).collect()
        } else {
            BlendMode::LAYER_MODES.to_vec()
        };
        egui::ComboBox::from_id_salt("blend-mode").width(110.0).selected_text(p.blend.label()).show_ui(ui, |ui| {
            for m in modes {
                ui.selectable_value(&mut p.blend, m, m.label());
            }
        });
        let mut pct = p.opacity * 100.0;
        let r = ui.add(egui::Slider::new(&mut pct, 0.0..=100.0).max_decimals(0).suffix("%"));
        // One drag is one undo step. The slider senses drags only, so the
        // drag starts on the press frame; the release frame reports
        // `drag_stopped` instead of `dragged` but may still move the value.
        drag_started = r.drag_started();
        drag_stopped = r.drag_stopped();
        if r.changed() {
            p.opacity = pct / 100.0;
            coalesce = r.dragged() || drag_stopped;
        }
    });
    ui.horizontal(|ui| {
        toggle(ui, &mut p.clip, icon::ARROW_ELBOW_LEFT_DOWN, "Clip to layer below");
        if !is_folder {
            toggle(ui, &mut p.lock_alpha, icon::CHECKERBOARD, "Lock transparent pixels");
        }
        toggle(ui, &mut p.locked, icon::LOCK_SIMPLE, "Lock layer");
    });
    if drag_started {
        studio.history.end_props_gesture(); // never merge into an earlier entry
    }
    studio.set_layer_props(id, p, coalesce);
    if drag_stopped {
        studio.history.end_props_gesture();
    }
}

fn toggle(ui: &mut egui::Ui, value: &mut bool, glyph: &str, tip: &str) {
    if ui.selectable_label(*value, glyph).on_hover_text(tip).clicked() {
        *value = !*value;
    }
}

fn layer_row(ui: &mut egui::Ui, studio: &mut Studio, shell: &mut Shell, id: LayerId, depth: usize) {
    let Some(layer) = studio.doc.layer(id) else { return };
    let pal = shell.theme.palette();
    let props = layer.props.clone();
    let folder_open = match &layer.content {
        LayerContent::Folder { expanded, .. } => Some(*expanded),
        LayerContent::Raster(_) => None,
    };
    let tiles = layer.raster().map(|g| g.len());
    let selected = studio.doc.active() == id;

    let row_h = 30.0;
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, row_h), Sense::click());
    let fill = if selected {
        pal.row_selected
    } else if resp.hovered() {
        pal.row_hover
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, CornerRadius::same(3), fill);

    let mut x = rect.left() + 4.0;
    let cy = rect.center().y;

    // Visibility.
    let eye = egui::Rect::from_center_size(egui::pos2(x + 10.0, cy), Vec2::splat(20.0));
    let eye_resp = ui.interact(eye, ui.id().with(("eye", id)), Sense::click());
    ui.painter().text(
        eye.center(),
        egui::Align2::CENTER_CENTER,
        if props.visible { icon::EYE } else { icon::EYE_SLASH },
        egui::FontId::proportional(15.0),
        if props.visible { ui.visuals().text_color() } else { pal.text_weak },
    );
    x += 24.0 + depth as f32 * 14.0;

    if props.clip {
        ui.painter().line_segment(
            [egui::pos2(x + 1.0, rect.top() + 3.0), egui::pos2(x + 1.0, rect.bottom() - 3.0)],
            Stroke::new(3.0, pal.clip_marker),
        );
        x += 7.0;
    }

    // Folder chevron or thumbnail-ish marker.
    let mut chevron_resp = None;
    if let Some(open) = folder_open {
        let chev = egui::Rect::from_center_size(egui::pos2(x + 8.0, cy), Vec2::splat(18.0));
        chevron_resp = Some((ui.interact(chev, ui.id().with(("chev", id)), Sense::click()), open));
        ui.painter().text(
            chev.center(),
            egui::Align2::CENTER_CENTER,
            if open { icon::CARET_DOWN } else { icon::CARET_RIGHT },
            egui::FontId::proportional(13.0),
            ui.visuals().text_color(),
        );
        x += 18.0;
        ui.painter().text(
            egui::pos2(x + 9.0, cy),
            egui::Align2::CENTER_CENTER,
            if open { icon::FOLDER_OPEN } else { icon::FOLDER },
            egui::FontId::proportional(16.0),
            ui.visuals().text_color(),
        );
        x += 22.0;
    } else {
        let thumb = egui::Rect::from_min_size(egui::pos2(x, rect.top() + 4.0), Vec2::new(30.0, row_h - 8.0));
        ui.painter().rect_filled(thumb, CornerRadius::same(2), Color32::WHITE);
        ui.painter().rect_stroke(thumb, CornerRadius::same(2), Stroke::new(1.0, Color32::from_gray(120)), egui::StrokeKind::Inside);
        if tiles.unwrap_or(0) > 0 {
            ui.painter().text(thumb.center(), egui::Align2::CENTER_CENTER, icon::SCRIBBLE, egui::FontId::proportional(14.0), Color32::from_gray(60));
        }
        x += 36.0;
    }

    // Name (double-click to rename).
    let name_rect = egui::Rect::from_min_max(egui::pos2(x, rect.top()), egui::pos2(rect.right() - 44.0, rect.bottom()));
    let renaming = matches!(&shell.renaming, Some((rid, ..)) if *rid == id);
    if renaming {
        let mut commit = false;
        let mut cancel = false;
        if let Some((_, text, focus_requested)) = shell.renaming.as_mut() {
            let te = egui::TextEdit::singleline(text).id(ui.id().with(("rename", id)));
            let edit = ui.put(name_rect.shrink2(Vec2::new(0.0, 4.0)), te);
            // Request focus only once: Enter, Escape and clicking elsewhere all
            // drop it, and re-requesting every frame would swallow that.
            if !*focus_requested {
                edit.request_focus();
                *focus_requested = true;
            } else if edit.lost_focus() {
                commit = !ui.input(|i| i.key_pressed(egui::Key::Escape));
                cancel = !commit;
            } else if !ui.memory(|m| m.has_focus(edit.id)) {
                // Focus went away while the row wasn't shown (e.g. collapsed folder).
                cancel = true;
            }
        }
        if commit {
            if let Some((_, text, _)) = shell.renaming.take() {
                let mut p = props.clone();
                if !text.trim().is_empty() {
                    p.name = text.trim().to_string();
                    studio.set_layer_props(id, p, false);
                }
            }
        } else if cancel {
            shell.renaming = None;
        }
    } else {
        let mut sub = props.blend.label().to_string();
        if props.opacity < 1.0 {
            sub = format!("{sub} · {:.0}%", props.opacity * 100.0);
        }
        ui.painter().text(
            egui::pos2(name_rect.left(), cy - 7.0),
            egui::Align2::LEFT_CENTER,
            &props.name,
            egui::FontId::proportional(12.5),
            if selected { ui.visuals().strong_text_color() } else { ui.visuals().text_color() },
        );
        ui.painter().text(egui::pos2(name_rect.left(), cy + 7.0), egui::Align2::LEFT_CENTER, sub, egui::FontId::proportional(10.0), pal.text_weak);
    }

    // State badges.
    let mut bx = rect.right() - 12.0;
    for (on, glyph) in [(props.locked, icon::LOCK_SIMPLE), (props.lock_alpha, icon::CHECKERBOARD)] {
        if on {
            ui.painter().text(egui::pos2(bx, cy), egui::Align2::CENTER_CENTER, glyph, egui::FontId::proportional(12.0), pal.text_weak);
            bx -= 16.0;
        }
    }

    // Interactions (after painting so child hit areas win).
    if eye_resp.clicked() {
        let mut p = props.clone();
        p.visible = !p.visible;
        studio.set_layer_props(id, p, false);
    } else if let Some((r, open)) = chevron_resp.filter(|(r, _)| r.clicked()) {
        let _ = r;
        studio.doc.set_folder_expanded(id, !open);
    } else if resp.double_clicked() {
        shell.renaming = Some((id, props.name.clone(), false));
    } else if resp.clicked() {
        studio.doc.set_active(id);
    }
    resp.context_menu(|ui| {
        studio.doc.set_active(id);
        for cmd in [
            Command::NewLayer,
            Command::NewFolder,
            Command::DuplicateLayer,
            Command::MergeDown,
            Command::ToggleClip,
            Command::ToggleLockAlpha,
            Command::ClearLayer,
            Command::DeleteLayer,
        ] {
            if ui.button(cmd.label()).clicked() {
                commands::execute(cmd, studio, shell);
                ui.close();
            }
        }
        if ui.button("Rename").clicked() {
            shell.renaming = Some((id, props.name.clone(), false));
            ui.close();
        }
    });
}
