//! Frame border tools: Rectangle Frame, Divide Frame and Frame Edit (O).
//! Owned by FRAMES.

use arty_core::{FrameShape, LayerId};
use serde::{Deserialize, Serialize};

use super::CanvasTool;
use crate::commands::Command;
use crate::shell::Shell;
use crate::studio::Studio;

#[derive(Default)]
pub struct FrameTool {}

impl CanvasTool for FrameTool {}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FrameOptions {}

impl Studio {
    /// Replace the frame of folder `id` with `f(current)` as one
    /// `Edit::Frame`; no-op when `f` returns `None`.
    // FRAMES: called by the frame tools and the border property sliders.
    #[allow(dead_code)]
    pub fn edit_frame(&mut self, _id: LayerId, _f: impl FnOnce(&FrameShape) -> Option<FrameShape>) {
        // FRAMES
    }
}

/// NewFrameFolder and DeletePanel.
pub fn execute(_cmd: Command, _studio: &mut Studio, _shell: &mut Shell) {
    // FRAMES
}

/// Tool Property for the frame tools.
pub fn property_ui(_ui: &mut egui::Ui, _studio: &mut Studio, _shell: &mut Shell) {
    // FRAMES
}
