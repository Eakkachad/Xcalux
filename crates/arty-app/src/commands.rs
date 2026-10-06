//! Every user action is a [`Command`]: menus, shortcuts and toolbar buttons
//! all go through [`execute`], so behaviour and labels stay in one place.

use arty_brush::BrushGroup;
use arty_core::LayerId;
use egui::{Key, KeyboardShortcut, Modifiers};

use crate::shell::{FileRequest, Shell, UiMode};
use crate::studio::{FrameMode, Studio, Tool};
use crate::tools;

/// Which selection modal `Command::SelectionDialog` opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelModify {
    Grow,
    Shrink,
    Feather,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Back to the home screen.
    Home,
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
    /// Switch between Simple and Studio; each keeps its own panel layout.
    SetUiMode(UiMode),
    PenEnd(arty_pen::PenEnd),
    SelectAll,
    Deselect,
    InvertSelection,
    /// Open the Grow / Shrink / Feather modal.
    SelectionDialog(SelModify),
    /// Built by the Grow / Shrink / Feather modal.
    GrowSelection { px: u16 },
    ShrinkSelection { px: u16 },
    FeatherSelection { px: u16 },
    FillSelection,
    ToggleReferenceLayer,
    /// Free transform of the active layer (of the selected pixels when
    /// there is a selection).
    Transform,
    CommitTransform,
    CancelTransform,
    FlipTransform { horizontal: bool },
    RotateTransform90 { cw: bool },
    PageSetup,
    TogglePageGuides,
    ToggleTrimShade,
    NewFrameFolder,
    DeletePanel,
}

impl Command {
    pub fn label(self) -> &'static str {
        use crate::text::{Key as TKey, t};
        match self {
            Command::Home => t(TKey::CmdHome),
            Command::NewDocument => t(TKey::CmdNewDocument),
            Command::Open => t(TKey::CmdOpen),
            Command::Save => t(TKey::CmdSave),
            Command::SaveAs => t(TKey::CmdSaveAs),
            Command::ToggleAutosave => t(TKey::CmdToggleAutosave),
            Command::ExportPng => t(TKey::CmdExportPng),
            Command::Quit => t(TKey::CmdQuit),
            Command::Undo => t(TKey::CmdUndo),
            Command::Redo => t(TKey::CmdRedo),
            Command::ClearLayer => t(TKey::CmdClearLayer),
            Command::NewLayer => t(TKey::CmdNewLayer),
            Command::NewFolder => t(TKey::CmdNewFolder),
            Command::DuplicateLayer => t(TKey::CmdDuplicateLayer),
            Command::MergeDown => t(TKey::CmdMergeDown),
            Command::DeleteLayer => t(TKey::CmdDeleteLayer),
            Command::LayerUp => t(TKey::CmdLayerUp),
            Command::LayerDown => t(TKey::CmdLayerDown),
            Command::MoveLayer { .. } => t(TKey::CmdMoveLayer),
            Command::ToggleClip => t(TKey::CmdToggleClip),
            Command::ToggleLockAlpha => t(TKey::CmdToggleLockAlpha),
            Command::ZoomIn => t(TKey::CmdZoomIn),
            Command::ZoomOut => t(TKey::CmdZoomOut),
            Command::ZoomFit => t(TKey::CmdZoomFit),
            Command::Zoom100 => t(TKey::CmdZoom100),
            Command::RotateLeft => t(TKey::CmdRotateLeft),
            Command::RotateRight => t(TKey::CmdRotateRight),
            Command::RotateReset => t(TKey::CmdRotateReset),
            Command::FlipView => t(TKey::CmdFlipView),
            Command::SwapColors => t(TKey::CmdSwapColors),
            Command::BrushSmaller => t(TKey::CmdBrushSmaller),
            Command::BrushLarger => t(TKey::CmdBrushLarger),
            Command::SelectTool(t) => t.label(),
            Command::ToggleTheme => t(TKey::CmdToggleTheme),
            Command::ResetLayout => t(TKey::CmdResetLayout),
            Command::SetUiMode(UiMode::Simple) => t(TKey::CmdModeSimple),
            Command::SetUiMode(UiMode::Studio) => t(TKey::CmdModeStudio),
            Command::PenEnd(arty_pen::PenEnd::Tip) => t(TKey::CmdPenTip),
            Command::PenEnd(arty_pen::PenEnd::Eraser) => t(TKey::CmdPenEraser),
            Command::SelectAll => t(TKey::CmdSelectAll),
            Command::Deselect => t(TKey::CmdDeselect),
            Command::InvertSelection => t(TKey::CmdInvertSelection),
            Command::SelectionDialog(SelModify::Grow) => t(TKey::CmdGrowSelectionDialog),
            Command::SelectionDialog(SelModify::Shrink) => t(TKey::CmdShrinkSelectionDialog),
            Command::SelectionDialog(SelModify::Feather) => t(TKey::CmdFeatherSelectionDialog),
            Command::GrowSelection { .. } => t(TKey::CmdGrowSelection),
            Command::ShrinkSelection { .. } => t(TKey::CmdShrinkSelection),
            Command::FeatherSelection { .. } => t(TKey::CmdFeatherSelection),
            Command::FillSelection => t(TKey::CmdFillSelection),
            Command::ToggleReferenceLayer => t(TKey::CmdToggleReferenceLayer),
            Command::Transform => t(TKey::CmdTransform),
            Command::CommitTransform => t(TKey::CmdCommitTransform),
            Command::CancelTransform => t(TKey::CmdCancelTransform),
            Command::FlipTransform { horizontal: true } => t(TKey::CmdFlipTransformH),
            Command::FlipTransform { horizontal: false } => t(TKey::CmdFlipTransformV),
            Command::RotateTransform90 { cw: true } => t(TKey::CmdRotateTransformCw),
            Command::RotateTransform90 { cw: false } => t(TKey::CmdRotateTransformCcw),
            Command::PageSetup => t(TKey::CmdPageSetup),
            Command::TogglePageGuides => t(TKey::CmdTogglePageGuides),
            Command::ToggleTrimShade => t(TKey::CmdToggleTrimShade),
            Command::NewFrameFolder => t(TKey::CmdNewFrameFolder),
            Command::DeletePanel => t(TKey::CmdDeletePanel),
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
    (sc(CTRL_SHIFT, Key::I), Command::InvertSelection),
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
    (sc(CTRL, Key::A), Command::SelectAll),
    (sc(CTRL, Key::D), Command::Deselect),
    (sc(CTRL, Key::T), Command::Transform),
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
    (sc(NONE, Key::M), Command::SelectTool(Tool::Select)),
    (sc(NONE, Key::W), Command::SelectTool(Tool::MagicWand)),
    (sc(NONE, Key::G), Command::SelectTool(Tool::Fill)),
    (sc(NONE, Key::K), Command::SelectTool(Tool::Move)),
    (sc(NONE, Key::O), Command::SelectTool(Tool::Frame(FrameMode::Edit))),
    (sc(NONE, Key::Enter), Command::CommitTransform),
    (sc(NONE, Key::Escape), Command::CancelTransform),
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
    // Enter and Esc stay with egui (menus, popups) unless a transform runs.
    let transforming = studio.transform.is_some();
    let mut fired = Vec::new();
    ctx.input_mut(|i| {
        for (shortcut, cmd) in SHORTCUTS {
            let session_only = matches!(cmd, Command::CommitTransform | Command::CancelTransform);
            if (transforming || !session_only) && i.consume_shortcut(shortcut) {
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
        Command::Home => shell.file_request = Some(FileRequest::Home),
        Command::NewDocument => shell.file_request = Some(FileRequest::New),
        Command::Open => shell.file_request = Some(FileRequest::Open),
        Command::Save => shell.file_request = Some(FileRequest::Save),
        Command::SaveAs => shell.file_request = Some(FileRequest::SaveAs),
        Command::ToggleAutosave => shell.autosave.enabled = !shell.autosave.enabled,
        Command::ExportPng => shell.export_requested = true,
        Command::Quit => shell.quit_requested = true,
        Command::Undo => studio.undo(),
        Command::Redo => studio.redo(),
        // In Frame Edit with a panel of the active frame folder selected,
        // Delete removes the panel.
        Command::ClearLayer
            if studio.tool == Tool::Frame(FrameMode::Edit) && tools::frame::active_panel_sel(studio).is_some() =>
        {
            execute(Command::DeletePanel, studio, shell)
        }
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
        Command::SetUiMode(mode) => shell.set_ui_mode(mode),
        Command::PenEnd(end) => studio.switch_pen_end(end),
        Command::SelectAll
        | Command::Deselect
        | Command::InvertSelection
        | Command::SelectionDialog(_)
        | Command::GrowSelection { .. }
        | Command::ShrinkSelection { .. }
        | Command::FeatherSelection { .. } => tools::select::execute(cmd, studio, shell),
        Command::FillSelection | Command::ToggleReferenceLayer => tools::fill::execute(cmd, studio, shell),
        Command::Transform
        | Command::CommitTransform
        | Command::CancelTransform
        | Command::FlipTransform { .. }
        | Command::RotateTransform90 { .. } => tools::transform::execute(cmd, studio, shell),
        Command::PageSetup | Command::TogglePageGuides | Command::ToggleTrimShade => {
            tools::page::execute(cmd, studio, shell)
        }
        Command::NewFrameFolder | Command::DeletePanel => tools::frame::execute(cmd, studio, shell),
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

    /// P26
    #[test]
    fn pen_end_command_remembers_tool_per_end() {
        use arty_pen::PenEnd;
        let mut studio = Studio::new(Document::new(64, 64, 72));
        let mut shell = Shell::new(ThemeKind::Dark);
        let pen = Tool::Brush(BrushGroup::Pen);
        assert_eq!(Command::PenEnd(PenEnd::Tip).label(), "Pen Tip");
        assert_eq!(Command::PenEnd(PenEnd::Eraser).label(), "Pen Eraser End");
        assert_eq!(shortcut_for(Command::PenEnd(PenEnd::Eraser)), None);

        execute(Command::PenEnd(PenEnd::Eraser), &mut studio, &mut shell);
        assert_eq!((studio.pen_end(), studio.tool), (PenEnd::Eraser, Tool::Brush(BrushGroup::Eraser)));
        assert!(studio.preset().eraser);
        execute(Command::SelectTool(Tool::Hand), &mut studio, &mut shell);
        execute(Command::PenEnd(PenEnd::Eraser), &mut studio, &mut shell);
        assert_eq!(studio.tool, Tool::Hand, "same end again is a no-op");
        execute(Command::PenEnd(PenEnd::Tip), &mut studio, &mut shell);
        assert_eq!(studio.tool, pen);
        execute(Command::PenEnd(PenEnd::Eraser), &mut studio, &mut shell);
        assert_eq!(studio.tool, Tool::Hand, "the eraser end remembers its own tool");
        execute(Command::PenEnd(PenEnd::Tip), &mut studio, &mut shell);

        // No-op while stroking.
        let s = arty_brush::InputSample { x: 10.0, y: 10.0, pressure: 1.0, ..Default::default() };
        assert!(studio.begin_stroke(s));
        execute(Command::PenEnd(PenEnd::Eraser), &mut studio, &mut shell);
        assert_eq!((studio.pen_end(), studio.tool), (PenEnd::Tip, pen));
        studio.end_stroke();
        execute(Command::PenEnd(PenEnd::Eraser), &mut studio, &mut shell);
        assert_eq!((studio.pen_end(), studio.tool), (PenEnd::Eraser, Tool::Hand));
    }

    /// Every page-tool command (M3) has a label and runs; the tool commands select their tool.
    #[test]
    fn page_tool_commands_run() {
        let mut studio = Studio::new(Document::new(64, 64, 72));
        let mut shell = Shell::new(ThemeKind::Dark);
        let id = studio.doc.active();
        studio.doc.paint_target(id).unwrap().0.get_mut_or_create(TileCoord::new(0, 0))[0][0] = [1, 1, 1, 1];
        let cmds = [
            Command::SelectAll,
            Command::Deselect,
            Command::InvertSelection,
            Command::SelectionDialog(SelModify::Grow),
            Command::SelectionDialog(SelModify::Shrink),
            Command::SelectionDialog(SelModify::Feather),
            Command::GrowSelection { px: 4 },
            Command::ShrinkSelection { px: 4 },
            Command::FeatherSelection { px: 4 },
            Command::FillSelection,
            Command::ToggleReferenceLayer,
            Command::Transform,
            Command::FlipTransform { horizontal: true },
            Command::FlipTransform { horizontal: false },
            Command::RotateTransform90 { cw: true },
            Command::RotateTransform90 { cw: false },
            Command::CommitTransform,
            Command::CancelTransform,
            Command::PageSetup,
            Command::TogglePageGuides,
            Command::ToggleTrimShade,
            Command::NewFrameFolder,
            Command::DeletePanel,
        ];
        let mut labels: Vec<&str> = cmds.iter().map(|c| c.label()).collect();
        labels.sort();
        labels.dedup();
        assert_eq!(labels.len(), cmds.len(), "labels are distinct");
        for cmd in cmds {
            execute(cmd, &mut studio, &mut shell);
        }
        for tool in [
            Tool::Select,
            Tool::MagicWand,
            Tool::Fill,
            Tool::Move,
            Tool::Frame(FrameMode::Rect),
            Tool::Frame(FrameMode::Cut),
            Tool::Frame(FrameMode::Edit),
        ] {
            execute(Command::SelectTool(tool), &mut studio, &mut shell);
            assert_eq!(studio.tool, tool);
            assert!(!Command::SelectTool(tool).label().is_empty());
        }
    }

    /// Delete in Frame Edit with a panel selected deletes the panel, not the layer's pixels.
    #[test]
    fn delete_in_frame_edit_targets_the_panel() {
        let mut studio = Studio::new(Document::new(64, 64, 72));
        let mut shell = Shell::new(ThemeKind::Dark);
        let base = studio.doc.active();
        execute(Command::NewFrameFolder, &mut studio, &mut shell);
        let folder = tools::frame::target_folder(&studio).unwrap();
        let id = studio.doc.active();
        let paint = |s: &mut Studio, id| s.doc.paint_target(id).unwrap().0.get_mut_or_create(TileCoord::new(0, 0))[0][0] = [1, 1, 1, 1];
        let painted = |s: &Studio| !s.doc.active_layer().raster().unwrap().is_empty();
        let panels = |s: &Studio| s.doc.frame(folder).unwrap().shape().panels.len();
        paint(&mut studio, id);
        execute(Command::SelectTool(Tool::Frame(FrameMode::Edit)), &mut studio, &mut shell);
        studio.frame_sel = Some((folder, 0));
        // The only panel is never deleted: an empty frame would hide the folder.
        execute(Command::ClearLayer, &mut studio, &mut shell);
        assert!(painted(&studio));
        assert_eq!(panels(&studio), 1, "the last panel stays");
        assert!(studio.notice.take().is_some());
        studio.edit_frame(folder, |s| {
            let mut s = s.clone();
            s.panels.push(s.panels[0].clone());
            Some(s)
        });
        studio.frame_sel = Some((folder, 1));
        execute(Command::ClearLayer, &mut studio, &mut shell);
        assert!(painted(&studio));
        assert_eq!(panels(&studio), 1, "the panel was deleted");
        studio.undo();
        studio.undo();
        studio.frame_sel = None;
        execute(Command::ClearLayer, &mut studio, &mut shell);
        assert!(!painted(&studio), "without a selected panel Delete clears the layer");

        // A panel of a folder that is no longer active (not drawn) is not
        // what Delete or Delete Panel act on.
        studio.doc.set_active(base);
        paint(&mut studio, base);
        studio.frame_sel = Some((folder, 0));
        execute(Command::ClearLayer, &mut studio, &mut shell);
        assert!(!painted(&studio), "the active layer was cleared");
        assert_eq!(panels(&studio), 1, "the hidden selection's panel stays");
        execute(Command::DeletePanel, &mut studio, &mut shell);
        assert_eq!(panels(&studio), 1);
    }

    /// Enter and Esc stay with egui (menus, popups) unless a transform session runs.
    #[test]
    fn enter_and_escape_are_left_alone_without_a_session() {
        let ctx = egui::Context::default();
        let mut studio = Studio::new(Document::new(64, 64, 72));
        let mut shell = Shell::new(ThemeKind::Dark);
        for key in [Key::Escape, Key::Enter] {
            let press = Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: NONE };
            let mut seen = false;
            ctx.run_ui(RawInput { events: vec![press], ..Default::default() }, |ui| {
                handle_shortcuts(ui.ctx(), &mut studio, &mut shell);
                seen = ui.input(|i| i.key_pressed(key));
            })
            .drop_without_applying_deltas();
            assert!(seen, "{key:?} was consumed");
        }
    }
}
