//! Selection tools (M: rect, ellipse, lasso, polygon; W: magic wand), the
//! marching ants and the selection commands. Owned by SEL-UI.

use serde::{Deserialize, Serialize};

use super::CanvasTool;
use crate::commands::Command;
use crate::shell::Shell;
use crate::studio::Studio;

#[derive(Default)]
pub struct SelectTool {}

impl CanvasTool for SelectTool {}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SelectOptions {}

/// SelectAll, Deselect, InvertSelection, SelectionDialog and
/// Grow/Shrink/FeatherSelection.
pub fn execute(_cmd: Command, _studio: &mut Studio, _shell: &mut Shell) {
    // SEL-UI
}

/// Tool Property for Selection and Magic Wand.
pub fn property_ui(_ui: &mut egui::Ui, _studio: &mut Studio, _shell: &mut Shell) {
    // SEL-UI
}

/// The selection outline, drawn after the page guides.
pub fn paint_ants(_st: &mut SelectTool, _painter: &egui::Painter, _studio: &Studio, _origin: [f32; 2], _ppp: f32) {
    // SEL-UI
}

/// The Grow / Shrink / Feather modal (`shell.sel_dialog`).
pub fn dialogs(_ctx: &egui::Context, _studio: &mut Studio, _shell: &mut Shell) {
    // SEL-UI
}
