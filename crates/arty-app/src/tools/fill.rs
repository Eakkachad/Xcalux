//! Fill tool (G), Fill Selection and reference layers. Owned by FILL.

use serde::{Deserialize, Serialize};

use super::CanvasTool;
use crate::commands::Command;
use crate::shell::Shell;
use crate::studio::Studio;

#[derive(Default)]
pub struct FillTool {}

impl CanvasTool for FillTool {}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FillOptions {}

/// FillSelection and ToggleReferenceLayer.
pub fn execute(_cmd: Command, _studio: &mut Studio, _shell: &mut Shell) {
    // FILL
}

/// Tool Property for Fill.
pub fn property_ui(_ui: &mut egui::Ui, _studio: &mut Studio, _shell: &mut Shell) {
    // FILL
}
