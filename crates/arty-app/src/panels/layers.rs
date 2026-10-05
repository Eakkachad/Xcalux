//! Layer panel: active layer settings, the layer list and layer actions.
//! Rows show a thumbnail of the layer and can be dragged to reorder the
//! tree or move layers in and out of folders.

use arty_core::{BlendMode, Document, LayerContent, LayerId};
use egui::{Color32, CornerRadius, DragAndDrop, Id, Rect, Sense, Stroke, Vec2};
use egui_phosphor::regular as icon;

use super::thumbs::{self, ThumbCache};
use crate::commands::{self, Command};
use crate::shell::Shell;
use crate::studio::Studio;

const ROW_H: f32 = 30.0;
/// Thumbnail box in a row; the page-shaped image is fitted inside.
const THUMB_BOX: Vec2 = Vec2::new(30.0, 22.0);
/// Pointer travel (points) before a press on a row becomes a layer drag.
const DRAG_START_DIST: f32 = 6.0;
/// Content indent of a row at nesting depth 0, and per level.
const INDENT: f32 = 24.0;
const INDENT_STEP: f32 = 14.0;

/// Drag-and-drop payload: the layer being dragged.
struct DraggedLayer(LayerId);

pub fn ui(ui: &mut egui::Ui, studio: &mut Studio, shell: &mut Shell, thumbs: &mut ThumbCache) {
    active_layer_controls(ui, studio);
    ui.separator();

    let ppp = ui.ctx().pixels_per_point();
    let size = thumbs::thumb_size([studio.doc.width(), studio.doc.height()], [THUMB_BOX.x * ppp, THUMB_BOX.y * ppp]);
    if thumbs.update(studio, size) && ui.input(|i| i.focused) {
        ui.ctx().request_repaint();
    }

    let footer_h = 28.0;
    let list_h = (ui.available_height() - footer_h).max(40.0);
    let mut rows = Vec::new();
    studio.doc.panel_rows(&mut rows);
    egui::ScrollArea::vertical().max_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
        let mut slots = Vec::with_capacity(rows.len());
        for (id, depth) in rows {
            if let Some(rect) = layer_row(ui, studio, shell, thumbs, id, depth) {
                slots.push(Slot { id, depth, rect });
            }
        }
        drop_zone(ui, studio, shell, &slots);
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

/// Draw one row and handle its clicks; returns the row's rect.
fn layer_row(
    ui: &mut egui::Ui,
    studio: &mut Studio,
    shell: &mut Shell,
    thumbs: &mut ThumbCache,
    id: LayerId,
    depth: usize,
) -> Option<Rect> {
    let layer = studio.doc.layer(id)?;
    let pal = shell.theme.palette();
    let props = layer.props.clone();
    let (folder_open, framed) = match &layer.content {
        LayerContent::Folder { expanded, frame, .. } => (Some(*expanded), frame.is_some()),
        LayerContent::Raster(_) => (None, false),
    };
    let selected = studio.doc.active() == id;
    let page = [studio.doc.width(), studio.doc.height()];

    let width = ui.available_width();
    let (_, rect) = ui.allocate_space(Vec2::new(width, ROW_H));
    // Fixed id: the drag source has to stay the same widget while its row
    // moves around the list.
    let resp = ui.interact(rect, Id::new(("layer-row", id)), Sense::click_and_drag());
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
    x += INDENT + depth as f32 * INDENT_STEP;

    if props.clip {
        ui.painter().line_segment(
            [egui::pos2(x + 1.0, rect.top() + 3.0), egui::pos2(x + 1.0, rect.bottom() - 3.0)],
            Stroke::new(3.0, pal.clip_marker),
        );
        x += 7.0;
    }

    // Folder chevron and icon, then the thumbnail.
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
            match (framed, open) {
                (true, _) => icon::LAYOUT,
                (false, true) => icon::FOLDER_OPEN,
                (false, false) => icon::FOLDER,
            },
            egui::FontId::proportional(16.0),
            ui.visuals().text_color(),
        );
        x += 22.0;
    }
    thumbnail(ui, thumbs, id, page, Rect::from_min_size(egui::pos2(x, rect.top() + 4.0), THUMB_BOX));
    x += THUMB_BOX.x + 6.0;

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
    for (on, glyph) in
        [(props.locked, icon::LOCK_SIMPLE), (props.lock_alpha, icon::CHECKERBOARD), (props.reference, icon::LIGHTHOUSE)]
    {
        if on {
            ui.painter().text(egui::pos2(bx, cy), egui::Align2::CENTER_CENTER, glyph, egui::FontId::proportional(12.0), pal.text_weak);
            bx -= 16.0;
        }
    }

    let dragged = DragAndDrop::payload::<DraggedLayer>(ui.ctx()).is_some_and(|d| d.0 == id);
    if dragged {
        ui.painter().rect_filled(rect, CornerRadius::same(3), ui.visuals().panel_fill.gamma_multiply(0.6));
    }

    // Interactions (after painting so child hit areas win).
    start_drag(ui, &resp, id);
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
    Some(rect)
}

/// The layer's thumbnail fitted into `frame` with the page's aspect.
fn thumbnail(ui: &egui::Ui, thumbs: &mut ThumbCache, id: LayerId, page: [u32; 2], frame: Rect) {
    let (w, h) = (page[0].max(1) as f32, page[1].max(1) as f32);
    let fit = (frame.width() / w).min(frame.height() / h);
    let img = Rect::from_center_size(frame.center(), Vec2::new(w * fit, h * fit));
    // Checker squares of about 3 points.
    let cell = (3.0 * ui.ctx().pixels_per_point()).round() as usize;
    match thumbs.texture(ui.ctx(), id, cell) {
        Some(tex) => {
            let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            ui.painter().image(tex, img, uv, Color32::WHITE);
        }
        // Not generated yet (frame budget).
        None => {
            ui.painter().rect_filled(img, CornerRadius::ZERO, Color32::WHITE);
        }
    }
    ui.painter().rect_stroke(img, CornerRadius::ZERO, Stroke::new(1.0, Color32::from_gray(120)), egui::StrokeKind::Outside);
}

/// Turn a press-and-move on a row into a layer drag. egui also reports a
/// drag when the pointer is held still for a while; only real movement
/// counts here. A drag cancelled with Escape stays cancelled until release.
fn start_drag(ui: &egui::Ui, resp: &egui::Response, id: LayerId) {
    let armed = Id::new("layer-drag-armed");
    let ctx = ui.ctx();
    if resp.drag_started() {
        ctx.data_mut(|d| d.insert_temp(armed, id));
    }
    if resp.dragged()
        && resp.total_drag_delta().is_some_and(|d| d.length() >= DRAG_START_DIST)
        && ctx.data(|d| d.get_temp::<LayerId>(armed)) == Some(id)
    {
        ctx.data_mut(|d| d.remove::<LayerId>(armed));
        DragAndDrop::set_payload(ctx, DraggedLayer(id));
    }
    if resp.drag_stopped() {
        ctx.data_mut(|d| d.remove::<LayerId>(armed));
    }
}

/// A laid-out row of the layer list.
struct Slot {
    id: LayerId,
    depth: usize,
    rect: Rect,
}

/// Where a dragged layer would land: `index` among the children of `parent`
/// (`None` = top level), counted before the move like
/// `Document::move_layer`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Drop {
    parent: Option<LayerId>,
    index: usize,
    marker: Marker,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Marker {
    /// Insertion line at `y`, indented like a row at `depth`.
    Line { y: f32, depth: usize },
    /// Into the folder shown in `slots[row]`.
    Into { row: usize },
}

/// Drop position for a pointer at height `y` over the list. The top and
/// bottom halves of a row drop above and below it; on a folder row the
/// middle half drops into the folder (on top of its contents). Below the
/// last row is the bottom of the page.
fn drop_target(doc: &Document, slots: &[Slot], y: f32) -> Option<Drop> {
    let last = slots.last()?;
    if y >= last.rect.bottom() {
        return Some(Drop { parent: None, index: 0, marker: Marker::Line { y: last.rect.bottom(), depth: 0 } });
    }
    let row = slots.iter().position(|s| y < s.rect.bottom())?;
    let s = &slots[row];
    let layer = doc.layer(s.id)?;
    let (parent, index) = doc.location(s.id)?;
    let frac = (y - s.rect.top()) / s.rect.height();
    let above = Drop { parent, index: index + 1, marker: Marker::Line { y: s.rect.top(), depth: s.depth } };
    let below = Drop { parent, index, marker: Marker::Line { y: s.rect.bottom(), depth: s.depth } };
    Some(match &layer.content {
        LayerContent::Raster(_) => {
            if frac < 0.5 {
                above
            } else {
                below
            }
        }
        LayerContent::Folder { children, expanded, .. } => {
            if frac < 0.25 {
                above
            } else if frac <= 0.75 {
                Drop { parent: Some(s.id), index: children.len(), marker: Marker::Into { row } }
            } else if *expanded && !children.is_empty() {
                // The line sits above the folder's top child: drop there.
                Drop {
                    parent: Some(s.id),
                    index: children.len(),
                    marker: Marker::Line { y: s.rect.bottom(), depth: s.depth + 1 },
                }
            } else {
                below
            }
        }
    })
}

/// Whether moving `id` there is allowed (as `Document::move_layer` rules:
/// not into itself or its own subfolders, nor past the depth limit) and
/// changes anything.
fn accepts(doc: &Document, id: LayerId, parent: Option<LayerId>, index: usize) -> bool {
    doc.can_move(id, parent)
        && doc.location(id).is_some_and(|(old_parent, old)| old_parent != parent || (index != old && index != old + 1))
}

/// Drop indicator, auto-scroll and the drop itself while a layer is being
/// dragged. Runs inside the list's scroll area after the rows.
fn drop_zone(ui: &mut egui::Ui, studio: &mut Studio, shell: &mut Shell, slots: &[Slot]) {
    let Some(dragged) = DragAndDrop::payload::<DraggedLayer>(ui.ctx()).map(|d| d.0) else { return };
    let pal = shell.theme.palette();
    let view = ui.clip_rect();
    let pointer = ui.ctx().pointer_latest_pos();
    let target = pointer
        .filter(|p| view.contains(*p))
        .and_then(|p| drop_target(&studio.doc, slots, p.y))
        .filter(|d| accepts(&studio.doc, dragged, d.parent, d.index));
    let (left, right) = (ui.max_rect().left(), ui.max_rect().right());
    match target.map(|d| d.marker) {
        Some(Marker::Line { y, depth }) => {
            let x = left + INDENT + depth as f32 * INDENT_STEP;
            ui.painter().hline(x..=right - 2.0, y, Stroke::new(2.0, pal.accent));
            ui.painter().circle_filled(egui::pos2(x, y), 3.5, pal.accent);
        }
        Some(Marker::Into { row }) => {
            let r = Rect::from_x_y_ranges(left..=right, slots[row].rect.y_range());
            ui.painter().rect_stroke(r, CornerRadius::same(3), Stroke::new(2.0, pal.accent), egui::StrokeKind::Inside);
        }
        None if pointer.is_some_and(|p| view.contains(p)) => ui.ctx().set_cursor_icon(egui::CursorIcon::NoDrop),
        None => {}
    }

    // Scroll when hovering near the top or bottom edge of the list.
    if let Some(p) = pointer.filter(|p| view.x_range().contains(p.x)) {
        let edge = 20.0;
        let dy = if (p.y - view.top()).abs() < edge {
            6.0
        } else if (p.y - view.bottom()).abs() < edge {
            -6.0
        } else {
            0.0
        };
        if dy != 0.0 {
            ui.scroll_with_delta(Vec2::new(0.0, dy));
            ui.ctx().request_repaint();
        }
    }

    if let (Some(p), Some(layer)) = (pointer, studio.doc.layer(dragged)) {
        egui::Area::new(Id::new("layer-drag-label"))
            .order(egui::Order::Tooltip)
            .fixed_pos(p + Vec2::new(14.0, 6.0))
            .interactable(false)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| ui.label(&layer.props.name));
            });
    }

    if ui.input(|i| i.pointer.any_released()) {
        DragAndDrop::clear_payload(ui.ctx());
        if let Some(d) = target {
            commands::execute(Command::MoveLayer { layer: dragged, parent: d.parent, index: d.index }, studio, shell);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeKind;
    use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput};

    /// Root `[a, folder{c1, c2}, b]`; panel rows top → bottom: b, folder,
    /// c2, c1, a.
    fn tree() -> (Document, [LayerId; 5]) {
        let mut doc = Document::new(64, 64, 72);
        let a = doc.active();
        let folder = doc.add_folder().unwrap();
        let c1 = doc.add_raster_layer().unwrap();
        doc.move_layer(c1, Some(folder), 0);
        let c2 = doc.add_raster_layer().unwrap();
        doc.set_active(folder);
        let b = doc.add_raster_layer().unwrap();
        (doc, [a, folder, c1, c2, b])
    }

    fn slots(doc: &Document) -> Vec<Slot> {
        let mut rows = Vec::new();
        doc.panel_rows(&mut rows);
        let row = |i: usize| Rect::from_min_size(Pos2::new(0.0, i as f32 * ROW_H), Vec2::new(200.0, ROW_H));
        rows.into_iter().enumerate().map(|(i, (id, depth))| Slot { id, depth, rect: row(i) }).collect()
    }

    fn at(doc: &Document, y: f32) -> (Option<LayerId>, usize) {
        let d = drop_target(doc, &slots(doc), y).unwrap();
        (d.parent, d.index)
    }

    #[test]
    fn drop_target_bands() {
        let (doc, [a, folder, c1, c2, b]) = tree();
        let s = slots(&doc);
        assert_eq!(s.iter().map(|s| s.id).collect::<Vec<_>>(), [b, folder, c2, c1, a]);
        assert_eq!(at(&doc, 5.0), (None, 3), "above b");
        assert_eq!(at(&doc, 20.0), (None, 2), "below b");
        assert_eq!(at(&doc, 33.0), (None, 2), "above the folder: the same gap");
        let into = drop_target(&doc, &s, 45.0).unwrap();
        assert_eq!((into.parent, into.index, into.marker), (Some(folder), 2, Marker::Into { row: 1 }));
        let first_child = drop_target(&doc, &s, 57.0).unwrap();
        assert_eq!(
            (first_child.parent, first_child.index, first_child.marker),
            (Some(folder), 2, Marker::Line { y: 60.0, depth: 1 }),
            "below an open folder's row is the top of its contents"
        );
        assert_eq!(at(&doc, 65.0), (Some(folder), 2), "above c2");
        assert_eq!(at(&doc, 115.0), (Some(folder), 0), "below c1 stays inside");
        assert_eq!(at(&doc, 125.0), (None, 1), "above a is outside");
        assert_eq!(at(&doc, 400.0), (None, 0), "past the last row");
    }

    #[test]
    fn below_a_collapsed_folder_is_outside_it() {
        let (mut doc, [_, folder, ..]) = tree();
        doc.set_folder_expanded(folder, false);
        assert_eq!(at(&doc, 57.0), (None, 1));
        assert_eq!(at(&doc, 45.0), (Some(folder), 2));
    }

    #[test]
    fn refuses_folder_into_itself_and_moves_that_change_nothing() {
        let (mut doc, [a, folder, c1, _, b]) = tree();
        let sub = doc.add_folder().unwrap();
        doc.move_layer(sub, Some(folder), 0);
        assert!(!accepts(&doc, folder, Some(folder), 0));
        assert!(!accepts(&doc, folder, Some(sub), 0), "into its own subfolder");
        assert!(accepts(&doc, sub, None, 0));
        assert_eq!(doc.location(b), Some((None, 2)));
        assert!(!accepts(&doc, b, None, 2) && !accepts(&doc, b, None, 3), "b's own gaps");
        assert!(accepts(&doc, b, None, 1));
        assert!(accepts(&doc, c1, None, 0));
        assert!(accepts(&doc, a, Some(folder), 3));
    }

    #[test]
    fn refuses_drops_past_the_depth_limit() {
        let (mut doc, [_, folder, _, _, b]) = tree();
        // A chain of folders down to the deepest level that holds a raster.
        let mut deepest = folder;
        for _ in 2..arty_core::MAX_TREE_DEPTH {
            let f = doc.add_folder().unwrap();
            assert!(doc.move_layer(f, Some(deepest), 0));
            deepest = f;
        }
        assert!(accepts(&doc, b, Some(deepest), 0), "a raster still fits");
        doc.set_active(b);
        let sub = doc.add_folder().unwrap();
        let inner = doc.add_raster_layer().unwrap();
        assert!(doc.move_layer(inner, Some(sub), 0));
        assert!(!accepts(&doc, sub, Some(deepest), 0), "its content would nest one level too deep");
        assert!(!doc.move_layer(sub, Some(deepest), 0));
    }

    struct Harness {
        ctx: egui::Context,
        studio: Studio,
        shell: Shell,
        thumbs: ThumbCache,
    }

    impl Harness {
        /// Two layers, `bottom` and `top` (active), and no history.
        fn new() -> (Self, LayerId, LayerId) {
            let mut studio = Studio::new(Document::new(64, 64, 72));
            let bottom = studio.doc.active();
            studio.edit_structure(|d| {
                d.add_raster_layer().unwrap();
                true
            });
            let top = studio.doc.active();
            studio.history.clear();
            let shell = Shell::new(ThemeKind::Dark);
            let mut h = Self { ctx: egui::Context::default(), studio, shell, thumbs: ThumbCache::default() };
            h.frame(vec![]);
            (h, bottom, top)
        }

        fn frame(&mut self, events: Vec<Event>) {
            let Self { ctx, studio, shell, thumbs } = self;
            let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(320.0, 480.0));
            ctx.run_ui(RawInput { events, screen_rect: Some(screen), ..Default::default() }, |ui| {
                super::ui(ui, studio, shell, thumbs);
            })
            .drop_without_applying_deltas();
        }

        fn row(&self, id: LayerId) -> Rect {
            self.ctx.read_response(Id::new(("layer-row", id))).expect("row shown").rect
        }

        fn button(&mut self, pos: Pos2, pressed: bool) {
            let e = Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
            self.frame(vec![Event::PointerMoved(pos), e]);
        }

        /// Press at `from`, move to `to` in steps, then `extra` events, then
        /// release.
        fn drag(&mut self, from: Pos2, to: Pos2, extra: Vec<Event>) {
            self.button(from, true);
            for k in 1..=4 {
                self.frame(vec![Event::PointerMoved(from + (to - from) * k as f32 / 4.0)]);
            }
            self.frame(extra);
            self.frame(vec![Event::PointerMoved(to)]);
            self.button(to, false);
        }
    }

    #[test]
    fn dragging_a_row_moves_the_layer_in_one_undo_step() {
        let (mut h, bottom, top) = Harness::new();
        let (from, to) = (h.row(top), h.row(bottom));
        h.drag(from.center(), egui::pos2(from.center().x, to.bottom() - 4.0), vec![]);
        assert_eq!(h.studio.doc.root(), &[top, bottom]);
        assert_eq!(h.studio.history.undo_len(), 1);
        assert_eq!(h.studio.doc.active(), top);
        h.studio.undo();
        assert_eq!(h.studio.doc.root(), &[bottom, top]);
    }

    #[test]
    fn dropping_on_a_folder_row_moves_into_it() {
        let (mut h, bottom, top) = Harness::new();
        h.studio.doc.set_active(bottom);
        h.studio.edit_structure(|d| {
            d.add_folder().unwrap();
            true
        });
        let folder = h.studio.doc.active();
        h.frame(vec![]);
        // Rows: top, folder, bottom.
        let (from, onto) = (h.row(top), h.row(folder));
        h.drag(from.center(), onto.center(), vec![]);
        assert_eq!(h.studio.doc.location(top), Some((Some(folder), 0)));
        assert_eq!(h.studio.history.undo_len(), 2);
        // A folder dropped onto itself is refused.
        h.frame(vec![]);
        let f = h.row(folder);
        h.drag(f.center(), f.center() + Vec2::new(0.0, 8.0), vec![]);
        assert_eq!(h.studio.doc.location(top), Some((Some(folder), 0)));
        assert_eq!(h.studio.history.undo_len(), 2);
    }

    #[test]
    fn small_movements_click_and_escape_cancels() {
        let (mut h, bottom, top) = Harness::new();
        let r = h.row(bottom);
        h.button(r.center(), true);
        h.frame(vec![Event::PointerMoved(r.center() + Vec2::new(2.0, 3.0))]);
        h.button(r.center() + Vec2::new(2.0, 3.0), false);
        assert_eq!(h.studio.doc.root(), &[bottom, top], "no move");
        assert_eq!(h.studio.doc.active(), bottom, "a click selects");

        let (from, to) = (h.row(top), h.row(bottom));
        let escape = Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE };
        h.drag(from.center(), egui::pos2(from.center().x, to.bottom() - 4.0), vec![escape]);
        assert_eq!(h.studio.doc.root(), &[bottom, top], "cancelled drag stays cancelled");
        assert_eq!(h.studio.history.undo_len(), 0);
    }
}
