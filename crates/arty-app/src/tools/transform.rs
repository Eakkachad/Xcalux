//! Free transform (Ctrl+T) handles and the Move tool (K). Owned by
//! TRANSFORM.

use arty_core::transform::FloatSession;
use serde::{Deserialize, Serialize};

use super::CanvasTool;
use crate::commands::Command;
use crate::shell::Shell;
use crate::studio::Studio;

#[derive(Default)]
pub struct TransformTool {}

impl CanvasTool for TransformTool {}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TransformOptions {}

/// A transform session in progress (`Studio::transform`).
pub struct TransformState {
    // TRANSFORM drives the session.
    #[allow(dead_code)]
    pub session: FloatSession,
}

impl Studio {
    /// Start a session on the active layer (`move_only`: the Move tool's
    /// translate-only drag). False, with a notice, when refused.
    // TRANSFORM: called by the Transform, Flip and Rotate commands and the Move tool.
    #[allow(dead_code)]
    pub fn begin_transform(&mut self, _move_only: bool) -> bool {
        // TRANSFORM
        false
    }

    /// Commit the session as one history step; no-op without one.
    pub fn commit_transform(&mut self) {
        // TRANSFORM
    }

    /// Drop the session, restoring the layer; no-op without one.
    pub fn cancel_transform(&mut self) {
        // TRANSFORM
    }
}

/// Transform, CommitTransform, CancelTransform, FlipTransform and
/// RotateTransform90.
pub fn execute(_cmd: Command, _studio: &mut Studio, _shell: &mut Shell) {
    // TRANSFORM
}

/// Tool Property for Move and for a transform session.
pub fn property_ui(_ui: &mut egui::Ui, _studio: &mut Studio, _shell: &mut Shell) {
    // TRANSFORM
}
