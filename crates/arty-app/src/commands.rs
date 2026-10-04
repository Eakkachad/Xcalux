//! Every user action is a [`Command`]: menus, shortcuts and toolbar buttons
//! all go through [`execute`], so behaviour and labels stay in one place.

use arty_brush::BrushGroup;
use arty_core::LayerId;
use egui::{Key, KeyboardShortcut, Modifiers};

use crate::shell::{FileRequest, Shell};
use crate::studio::{Studio, Tool};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    NewDocument,
    Open,
    Save,
    SaveAs,
    ToggleAutosave,
    ExportPng,
    Quit,
    Undo,
    Redo,
    ClearLayer,
    NewLayer,
    NewFolder,
    DuplicateLayer,
    MergeDown,
    DeleteLayer,
    LayerUp,
    LayerDown,
    /// Move a layer to `index` among the children of `parent` (`None` = top
    /// level), counted before the move as `Document::move_layer` does.
    MoveLayer { layer: LayerId, parent: Option<LayerId>, index: usize },
    ToggleClip,
    ToggleLockAlpha,
    ZoomIn,
    ZoomOut,
    ZoomFit,
    Zoom100,
    RotateLeft,
    RotateRight,
    RotateReset,
    FlipView,
    SwapColors,
    BrushSmaller,
    BrushLarger,
    SelectTool(Tool),
    ToggleTheme,
    ResetLayout,
}

impl Command {
    pub fn label(self) -> &'static str {
        match self {
            Command::NewDocument => "New…",
            Command::Open => "Open…",
            Command::Save => "Save",
            Command::SaveAs => "Save As…",
            Command::ToggleAutosave => "Autosave",
            Command::ExportPng => "Export PNG…",
            Command::Quit => "Quit",
            Command::Undo => "Undo",
            Command::Redo => "Redo",
            Command::ClearLayer => "Clear Layer",
            Command::NewLayer => "New Raster Layer",
            Command::NewFolder => "New Folder",
            Command::DuplicateLayer => "Duplicate Layer",
            Command::MergeDown => "Merge Down",
            Command::DeleteLayer => "Delete Layer",
            Command::LayerUp => "Move Layer Up",
            Command::LayerDown => "Move Layer Down",
            Command::MoveLayer { .. } => "Move Layer",
            Command::ToggleClip => "Clip to Layer Below",
            Command::ToggleLockAlpha => "Lock Transparent Pixels",
            Command::ZoomIn => "Zoom In",
            Command::ZoomOut => "Zoom Out",
            Command::ZoomFit => "Fit to Window",
            Command::Zoom100 => "Actual Pixels (100%)",
            Command::RotateLeft => "Rotate Left 15°",
            Command::RotateRight => "Rotate Right 15°",
            Command::RotateReset => "Reset Rotation",
            Command::FlipView => "Flip Horizontal",
            Command::SwapColors => "Swap Main/Sub Color",
            Command::BrushSmaller => "Brush Smaller",
            Command::BrushLarger => "Brush Larger",
            Command::SelectTool(t) => t.label(),
            Command::ToggleTheme => "Toggle Light/Dark",
            Command::ResetLayout => "Reset Panel Layout",
        }
    }
}

const fn sc(modifiers: Modifiers, key: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(modifiers, key)
}

const CTRL: Modifiers = Modifiers::COMMAND;
const CTRL_SHIFT: Modifiers = Modifiers::COMMAND.plus(Modifiers::SHIFT);
const CTRL_ALT: Modifiers = Modifiers::COMMAND.plus(Modifiers::ALT);
const NONE: Modifiers = Modifiers::NONE;

/// Default bindings (Clip Studio-like). More specific modifier combos come
/// first because egui matches shortcuts loosely on Shift.
pub const SHORTCUTS: &[(KeyboardShortcut, Command)] = &[
    (sc(CTRL_SHIFT, Key::Z), Command::Redo),
    (sc(CTRL_SHIFT, Key::N), Command::NewLayer),
    (sc(CTRL_SHIFT, Key::S), Command::SaveAs),
    (sc(CTRL_ALT, Key::Num0), Command::Zoom100),
    (sc(CTRL, Key::Z), Command::Undo),
    (sc(CTRL, Key::Y), Command::Redo),
    (sc(CTRL, Key::N), Command::NewDocument),
    (sc(CTRL, Key::O), Command::Open),
    (sc(CTRL, Key::S), Command::Save),
    (sc(CTRL, Key::E), Command::MergeDown),
    (sc(CTRL, Key::Num0), Command::ZoomFit),
    (sc(CTRL, Key::Plus), Command::ZoomIn),
    (sc(CTRL, Key::Equals), Command::ZoomIn),
    (sc(CTRL, Key::Minus), Command::ZoomOut),
    (sc(CTRL, Key::Q), Command::Quit),
    (sc(CTRL_ALT, Key::G), Command::ToggleClip),
    (sc(NONE, Key::Delete), Command::ClearLayer),
    (sc(NONE, Key::Backspace), Command::ClearLayer),
    (sc(NONE, Key::P), Command::SelectTool(Tool::Brush(BrushGroup::Pen))),
    (sc(NONE, Key::N), Command::SelectTool(Tool::Brush(BrushGroup::Pencil))),
    (sc(NONE, Key::B), Command::SelectTool(Tool::Brush(BrushGroup::Brush))),
    (sc(NONE, Key::J), Command::SelectTool(Tool::Brush(BrushGroup::Airbrush))),
    (sc(NONE, Key::U), Command::SelectTool(Tool::Brush(BrushGroup::Blend))),
    (sc(NONE, Key::E), Command::SelectTool(Tool::Brush(BrushGroup::Eraser))),
    (sc(NONE, Key::I), Command::SelectTool(Tool::Eyedropper)),
    (sc(NONE, Key::H), Command::SelectTool(Tool::Hand)),
    (sc(NONE, Key::R), Command::SelectTool(Tool::Rotate)),
    (sc(NONE, Key::Z), Command::SelectTool(Tool::Zoom)),
    (sc(NONE, Key::X), Command::SwapColors),
    (sc(NONE, Key::OpenBracket), Command::BrushSmaller),
    (sc(NONE, Key::CloseBracket), Command::BrushLarger),
    (sc(NONE, Key::Minus), Command::RotateLeft),
    (sc(NONE, Key::Equals), Command::RotateRight),
    (sc(NONE, Key::F), Command::FlipView),
];

pub fn shortcut_for(cmd: Command) -> Option<KeyboardShortcut> {
    SHORTCUTS.iter().find(|(_, c)| *c == cmd).map(|(s, _)| *s)
}

/// Fire commands whose shortcut was pressed this frame.
pub fn handle_shortcuts(ctx: &egui::Context, studio: &mut Studio, shell: &mut Shell) {
    // egui::Modal blocks pointer input below it but not the keyboard.
    if ctx.egui_wants_keyboard_input() || ctx.memory(|m| m.top_modal_layer().is_some()) {
        return;
    }
    let mut fired = Vec::new();
    ctx.input_mut(|i| {
        for (shortcut, cmd) in SHORTCUTS {
            if i.consume_shortcut(shortcut) {
                fired.push(*cmd);
            }
        }
    });
    for cmd in fired {
        execute(cmd, studio, shell);
    }
}

pub fn execute(cmd: Command, studio: &mut Studio, shell: &mut Shell) {
    let origin = shell.canvas_center_px;
    let step = 15f32.to_radians();
    match cmd {
        Command::NewDocument => shell.file_request = Some(FileRequest::New),
        Command::Open => shell.file_request = Some(FileRequest::Open),
        Command::Save => shell.file_request = Some(FileRequest::Save),
        Command::SaveAs => shell.file_request = Some(FileRequest::SaveAs),
        Command::ToggleAutosave => shell.autosave.enabled = !shell.autosave.enabled,
        Command::ExportPng => shell.export_requested = true,
        Command::Quit => shell.quit_requested = true,
        Command::Undo => studio.undo(),
        Command::Redo => studio.redo(),
        Command::ClearLayer => studio.clear_active_layer(),
        Command::NewLayer => {
            studio.edit_structure(|d| d.layer_count() < arty_core::MAX_LAYERS && d.add_raster_layer().is_some())
        }
        Command::NewFolder => {
            studio.edit_structure(|d| d.layer_count() < arty_core::MAX_LAYERS && d.add_folder().is_some())
        }
        Command::DuplicateLayer => studio.edit_structure(|d| d.duplicate_layer(d.active()).is_some()),
        Command::MergeDown => studio.edit_structure(|d| d.merge_down(d.active())),
        Command::DeleteLayer => studio.edit_structure(|d| d.delete_layer(d.active())),
        Command::LayerUp => studio.edit_structure(|d| d.shift_layer(d.active(), 1)),
        Command::LayerDown => studio.edit_structure(|d| d.shift_layer(d.active(), -1)),
        Command::MoveLayer { layer, parent, index } => studio.edit_structure(|d| {
            let moved = d.move_layer(layer, parent, index);
            if moved {
                d.set_active(layer);
            }
            moved
        }),
        Command::ToggleClip | Command::ToggleLockAlpha => {
            let id = studio.doc.active();
            if let Some(layer) = studio.doc.layer(id) {
                let mut p = layer.props.clone();
                if cmd == Command::ToggleClip {
                    p.clip = !p.clip;
                } else {
                    p.lock_alpha = !p.lock_alpha;
                }
                studio.set_layer_props(id, p, false);
            }
        }
        Command::ZoomIn | Command::ZoomOut => {
            let z = studio.view.next_zoom_step(cmd == Command::ZoomIn);
            studio.view.zoom_at(origin, origin, z);
        }
        Command::ZoomFit => studio.fit_pending = true,
        Command::Zoom100 => studio.view.zoom_at(origin, origin, 1.0),
        Command::RotateLeft => {
            let r = studio.view.rotation - step;
            studio.view.rotate_at(origin, origin, r);
        }
        Command::RotateRight => {
            let r = studio.view.rotation + step;
            studio.view.rotate_at(origin, origin, r);
        }
        Command::RotateReset => studio.view.rotate_at(origin, origin, 0.0),
        Command::FlipView => studio.view.toggle_flip(),
        Command::SwapColors => studio.swap_colors(),
        Command::BrushSmaller => studio.nudge_brush_size(false),
        Command::BrushLarger => studio.nudge_brush_size(true),
        Command::SelectTool(t) => studio.select_tool(t),
        Command::ToggleTheme => shell.toggle_theme(),
        Command::ResetLayout => shell.reset_layout_requested = true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeKind;
    use arty_core::{Document, TileCoord};
    use egui::{Event, RawInput};

    #[test]
    fn no_shortcuts_behind_a_modal() {
        let delete = || Event::Key { key: Key::Delete, physical_key: None, pressed: true, repeat: false, modifiers: NONE };
        for modal in [true, false] {
            let ctx = egui::Context::default();
            let mut studio = Studio::new(Document::new(64, 64, 72));
            let mut shell = Shell::new(ThemeKind::Dark);
            let id = studio.doc.active();
            studio.doc.paint_target(id).unwrap().0.get_mut_or_create(TileCoord::new(0, 0))[0][0] = [1, 1, 1, 1];
            for events in [vec![], vec![delete()]] {
                ctx.run_ui(RawInput { events, ..Default::default() }, |ui| {
                    handle_shortcuts(ui.ctx(), &mut studio, &mut shell);
                    if modal {
                        egui::Modal::new(egui::Id::new("test-modal")).show(ui.ctx(), |ui| ui.label("modal"));
                    }
                })
                .drop_without_applying_deltas();
            }
            assert_eq!(studio.doc.active_layer().raster().unwrap().is_empty(), !modal);
        }
    }

    /// The file request each shortcut leaves, with or without a modal open.
    fn file_request_after(modifiers: Modifiers, key: Key, modal: bool) -> Option<FileRequest> {
        let ctx = egui::Context::default();
        let mut studio = Studio::new(Document::new(64, 64, 72));
        let mut shell = Shell::new(ThemeKind::Dark);
        let press = Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
        for events in [vec![], vec![press]] {
            ctx.run_ui(RawInput { events, ..Default::default() }, |ui| {
                handle_shortcuts(ui.ctx(), &mut studio, &mut shell);
                if modal {
                    egui::Modal::new(egui::Id::new("test-modal")).show(ui.ctx(), |ui| ui.label("modal"));
                }
            })
            .drop_without_applying_deltas();
        }
        shell.file_request
    }

    #[test]
    fn file_shortcuts_and_save_as_before_save() {
        assert_eq!(file_request_after(CTRL_SHIFT, Key::S, false), Some(FileRequest::SaveAs));
        assert_eq!(file_request_after(CTRL, Key::S, false), Some(FileRequest::Save));
        assert_eq!(file_request_after(CTRL, Key::O, false), Some(FileRequest::Open));
        assert_eq!(file_request_after(CTRL, Key::N, false), Some(FileRequest::New));
        let pos = |cmd| SHORTCUTS.iter().position(|(_, c)| *c == cmd).unwrap();
        assert!(pos(Command::SaveAs) < pos(Command::Save), "egui matches Shift loosely");
        for (m, k) in [(CTRL_SHIFT, Key::S), (CTRL, Key::S), (CTRL, Key::O)] {
            assert_eq!(file_request_after(m, k, true), None, "blocked behind a modal");
        }
    }
}
