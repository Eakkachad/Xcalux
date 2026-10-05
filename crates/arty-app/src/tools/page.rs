//! Page setup: the Page Setup and Export dialogs, the manuscript presets
//! of the New dialog and the page guide overlays. Owned by FRAMES.

use serde::{Deserialize, Serialize};

use crate::commands::Command;
use crate::export::ExportCrop;
use crate::shell::Shell;
use crate::studio::Studio;

/// Guide overlays (view state, never exported).
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PageViewOptions {
    pub show_guides: bool,
    pub shade_outside_trim: bool,
}

impl Default for PageViewOptions {
    fn default() -> Self {
        Self { show_guides: true, shade_outside_trim: false }
    }
}

/// PageSetup, TogglePageGuides and ToggleTrimShade.
pub fn execute(_cmd: Command, _studio: &mut Studio, _shell: &mut Shell) {
    // FRAMES
}

// FRAMES: page setup has no tool of its own; this is for a page section in
// a tool's properties if one is wanted.
#[allow(dead_code)]
pub fn property_ui(_ui: &mut egui::Ui, _studio: &mut Studio, _shell: &mut Shell) {
    // FRAMES
}

/// Trim, bleed, safe and inner-frame guides, drawn right after the page.
pub fn paint_guides(_painter: &egui::Painter, _studio: &Studio, _opts: &PageViewOptions, _origin: [f32; 2], _ppp: f32) {
    // FRAMES
}

/// The Page Setup modal (`shell.page_setup_open`).
pub fn dialogs(_ctx: &egui::Context, _studio: &mut Studio, _shell: &mut Shell) {
    // FRAMES
}

/// The Export PNG modal, shown while `shell.export_dialog` is set. Returns
/// the chosen crop once.
pub fn export_dialog(_ctx: &egui::Context, _studio: &mut Studio, shell: &mut Shell) -> Option<ExportCrop> {
    // FRAMES: ask for the crop (default Trim when there is a page setup).
    std::mem::take(&mut shell.export_dialog).then_some(ExportCrop::Canvas)
}

/// The "Manga manuscript" presets in the New dialog. Returns the canvas
/// `(width, height, dpi)` of a picked preset and leaves its page setup in
/// `shell.new_doc_page`.
pub fn new_doc_ui(_ui: &mut egui::Ui, _shell: &mut Shell) -> Option<(u32, u32, u32)> {
    // FRAMES
    None
}
