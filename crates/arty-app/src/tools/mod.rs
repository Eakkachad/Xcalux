//! Page tools on the canvas: selection, fill, transform, frames and page
//! setup. Each track owns one submodule; the canvas drives them through
//! [`CanvasTool`] (see `canvas.rs` for the dispatch).

pub mod fill;
pub mod frame;
pub mod page;
pub mod select;
pub mod transform;

use serde::{Deserialize, Serialize};

use crate::shell::Shell;
use crate::studio::Studio;

/// One pointer event for a tool.
// The page tools read these once their tracks fill in the gestures.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct ToolInput {
    /// Physical px.
    pub screen: [f32; 2],
    /// Document px.
    pub doc: [f32; 2],
    pub mods: egui::Modifiers,
    /// The press is the second of a double click.
    pub double: bool,
}

/// What a tool may change while handling input. `origin` (canvas centre)
/// and `ppp` turn document points into egui points as the canvas does.
// The page tools read these once their tracks fill in the gestures.
#[allow(dead_code)]
pub struct ToolCtx<'a> {
    pub studio: &'a mut Studio,
    pub shell: &'a mut Shell,
    pub origin: [f32; 2],
    pub ppp: f32,
}

/// A canvas tool. Every method defaults to doing nothing.
pub trait CanvasTool {
    fn press(&mut self, _: &mut ToolCtx, _: ToolInput) {}
    /// Once per pointer-move event while pressed, in order.
    fn drag(&mut self, _: &mut ToolCtx, _: ToolInput) {}
    fn release(&mut self, _: &mut ToolCtx, _: ToolInput) {}
    fn hover(&mut self, _: &Studio, _: ToolInput) {}
    /// Enter, Esc or Backspace while [`Self::gesture_active`]; true when used.
    fn key(&mut self, _: &mut ToolCtx, _: egui::Key) -> bool {
        false
    }
    /// Abandon the gesture: Esc (unused by `key`), tool switch, focus loss.
    fn cancel(&mut self, _: &mut ToolCtx) {}
    /// Every frame (idle timers).
    fn tick(&mut self, _: &mut ToolCtx, _now: f64) {}
    /// Overlays, after the page and the selection outline.
    fn paint(&self, _: &Studio, _: &egui::Painter, _origin: [f32; 2], _ppp: f32) {}
    fn cursor(&self, _: &Studio) -> egui::CursorIcon {
        egui::CursorIcon::Crosshair
    }
    /// A drag or an unclosed polygon is in progress: global shortcuts wait.
    fn gesture_active(&self) -> bool {
        false
    }
}

#[derive(Default)]
pub struct ToolStates {
    pub select: select::SelectTool,
    pub fill: fill::FillTool,
    pub transform: transform::TransformTool,
    pub frame: frame::FrameTool,
}

/// Tool options, saved with the app settings.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolOptions {
    pub select: select::SelectOptions,
    pub fill: fill::FillOptions,
    pub transform: transform::TransformOptions,
    pub frame: frame::FrameOptions,
    pub page: page::PageViewOptions,
}

#[cfg(test)]
mod tests {
    use egui::{Key, KeyboardShortcut, Modifiers};

    use crate::commands::{Command, SHORTCUTS, shortcut_for};
    use crate::studio::{FrameMode, Tool};

    /// No key fires two commands (a command may have two keys).
    #[test]
    fn shortcuts_are_unique() {
        for (i, (a, ca)) in SHORTCUTS.iter().enumerate() {
            for (b, cb) in &SHORTCUTS[i + 1..] {
                assert!(a != b, "{a:?} is bound to both {ca:?} and {cb:?}");
            }
        }
        let ctrl = Modifiers::COMMAND;
        for (sc, cmd) in [
            (KeyboardShortcut::new(Modifiers::NONE, Key::M), Command::SelectTool(Tool::Select)),
            (KeyboardShortcut::new(Modifiers::NONE, Key::W), Command::SelectTool(Tool::MagicWand)),
            (KeyboardShortcut::new(Modifiers::NONE, Key::G), Command::SelectTool(Tool::Fill)),
            (KeyboardShortcut::new(Modifiers::NONE, Key::K), Command::SelectTool(Tool::Move)),
            (KeyboardShortcut::new(Modifiers::NONE, Key::O), Command::SelectTool(Tool::Frame(FrameMode::Edit))),
            (KeyboardShortcut::new(ctrl, Key::A), Command::SelectAll),
            (KeyboardShortcut::new(ctrl, Key::D), Command::Deselect),
            (KeyboardShortcut::new(ctrl.plus(Modifiers::SHIFT), Key::I), Command::InvertSelection),
            (KeyboardShortcut::new(ctrl, Key::T), Command::Transform),
            (KeyboardShortcut::new(Modifiers::NONE, Key::Enter), Command::CommitTransform),
            (KeyboardShortcut::new(Modifiers::NONE, Key::Escape), Command::CancelTransform),
        ] {
            assert_eq!(shortcut_for(cmd), Some(sc), "{cmd:?}");
        }
        for cmd in [Command::SelectTool(Tool::Frame(FrameMode::Rect)), Command::SelectTool(Tool::Frame(FrameMode::Cut))] {
            assert_eq!(shortcut_for(cmd), None, "{cmd:?} has no default key");
        }
        // egui matches Shift loosely: Ctrl+Shift combos come before plain Ctrl ones.
        let first_plain_ctrl = SHORTCUTS.iter().position(|(s, _)| s.modifiers == ctrl).unwrap();
        let invert = SHORTCUTS.iter().position(|(_, c)| *c == Command::InvertSelection).unwrap();
        assert!(invert < first_plain_ctrl);
    }
}
