use serde::{Deserialize, Serialize};

/// Tracks an in-progress panel header drag for drag-to-float.
#[derive(Clone, Debug)]
pub(crate) struct PanelDragState {
    pub kind: PanelKind,
    pub drag_start_screen: egui::Pos2,
    pub detached: bool,
}

/// Tracks a floating window being dragged for drop-zone detection.
#[derive(Clone, Debug)]
pub(crate) struct FloatingDragState {
    #[allow(dead_code)]
    pub kind: PanelKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PanelKind {
    ToolsAndPresets,
    BrushPresets,
    BrushSettings,
    ToolOptions,
    Stabilizer,
    Symmetry,
    AdvancedDebug,
    Navigator,
    ColorWheel,
    ColorSliders,
    ColorPalette,
    ColorHistory,
    LayersManager,
    Reference,
}

impl PanelKind {
    pub fn default_title(&self) -> &'static str {
        match self {
            PanelKind::ToolsAndPresets => "TOOLS",
            PanelKind::BrushPresets => "BRUSH PRESETS",
            PanelKind::BrushSettings => "BRUSH SETTINGS",
            PanelKind::ToolOptions => "TOOL OPTIONS",
            PanelKind::Stabilizer => "STABILIZER",
            PanelKind::Symmetry => "SYMMETRY / DRAWING GUIDE",
            PanelKind::AdvancedDebug => "ADVANCED / DEBUG",
            PanelKind::Navigator => "NAVIGATOR",
            PanelKind::ColorWheel => "COLOR WHEEL",
            PanelKind::ColorSliders => "COLOR SLIDERS",
            PanelKind::ColorPalette => "COLOR PALETTE",
            PanelKind::ColorHistory => "COLOR HISTORY",
            PanelKind::LayersManager => "LAYERS MANAGER",
            PanelKind::Reference => "REFERENCE",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum PanelLocation {
    Left,
    Right,
    Floating,
    Hidden,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FloatingPanelState {
    pub position: [f32; 2],
    pub size: [f32; 2],
}

impl Default for FloatingPanelState {
    fn default() -> Self {
        Self {
            position: [100.0, 100.0],
            size: [300.0, 400.0],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PanelState {
    pub kind: PanelKind,
    pub title: String,
    pub location: PanelLocation,
    pub visible: bool,
    pub collapsed: bool,
    pub floating: FloatingPanelState,
}

impl PanelState {
    pub fn new(
        kind: PanelKind,
        title: &str,
        location: PanelLocation,
        visible: bool,
        collapsed: bool,
    ) -> Self {
        Self {
            kind,
            title: title.to_string(),
            location,
            visible,
            collapsed,
            floating: FloatingPanelState::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceLayout {
    pub left_panel_visible: bool,
    pub right_panel_visible: bool,
    pub left_panel_collapsed: bool,
    pub right_panel_collapsed: bool,
    pub left_panel_width: f32,
    pub right_panel_width: f32,
    pub panels: Vec<PanelState>,
    pub ui_scale: f32,
}

impl WorkspaceLayout {
    #[allow(dead_code)]
    pub fn find_panel(&self, kind: PanelKind) -> Option<&PanelState> {
        self.panels.iter().find(|p| p.kind == kind)
    }

    pub fn find_panel_mut(&mut self, kind: PanelKind) -> Option<&mut PanelState> {
        self.panels.iter_mut().find(|p| p.kind == kind)
    }

    #[allow(dead_code)]
    pub fn panel_visible(&self, kind: PanelKind) -> bool {
        self.panels
            .iter()
            .find(|p| p.kind == kind)
            .map(|p| p.visible && p.location != PanelLocation::Hidden)
            .unwrap_or(false)
    }

    pub fn is_panel_at(&self, kind: PanelKind, location: PanelLocation) -> bool {
        self.panels
            .iter()
            .find(|p| p.kind == kind)
            .map(|p| p.visible && p.location == location)
            .unwrap_or(false)
    }

    pub fn set_panel_location(&mut self, kind: PanelKind, location: PanelLocation) {
        if let Some(panel) = self.find_panel_mut(kind) {
            panel.location = location;
            if location == PanelLocation::Hidden {
                panel.visible = false;
            } else {
                panel.visible = true;
            }
        }
    }

    pub fn toggle_panel_visibility(&mut self, kind: PanelKind) {
        if let Some(panel) = self.find_panel_mut(kind) {
            panel.visible = !panel.visible;
        }
    }
}

impl Default for WorkspaceLayout {
    fn default() -> Self {
        Self {
            left_panel_visible: true,
            right_panel_visible: true,
            left_panel_collapsed: false,
            right_panel_collapsed: false,
            left_panel_width: 240.0,
            right_panel_width: 280.0,
            panels: vec![
                PanelState::new(
                    PanelKind::ToolsAndPresets,
                    "TOOLS",
                    PanelLocation::Left,
                    true,
                    false,
                ),
                PanelState::new(
                    PanelKind::BrushPresets,
                    "BRUSH PRESETS",
                    PanelLocation::Left,
                    true,
                    false,
                ),
                PanelState::new(
                    PanelKind::BrushSettings,
                    "BRUSH SETTINGS",
                    PanelLocation::Left,
                    true,
                    false,
                ),
                PanelState::new(
                    PanelKind::ToolOptions,
                    "TOOL OPTIONS",
                    PanelLocation::Left,
                    true,
                    false,
                ),
                PanelState::new(
                    PanelKind::Stabilizer,
                    "STABILIZER",
                    PanelLocation::Left,
                    true,
                    false,
                ),
                PanelState::new(
                    PanelKind::Symmetry,
                    "SYMMETRY / DRAWING GUIDE",
                    PanelLocation::Left,
                    true,
                    false,
                ),
                PanelState::new(
                    PanelKind::AdvancedDebug,
                    "ADVANCED / DEBUG",
                    PanelLocation::Left,
                    false,
                    false,
                ),
                PanelState::new(
                    PanelKind::Navigator,
                    "NAVIGATOR",
                    PanelLocation::Right,
                    true,
                    false,
                ),
                PanelState::new(
                    PanelKind::ColorWheel,
                    "COLOR WHEEL",
                    PanelLocation::Right,
                    true,
                    false,
                ),
                PanelState::new(
                    PanelKind::ColorSliders,
                    "COLOR SLIDERS",
                    PanelLocation::Right,
                    false,
                    false,
                ),
                PanelState::new(
                    PanelKind::ColorPalette,
                    "COLOR PALETTE",
                    PanelLocation::Right,
                    false,
                    false,
                ),
                PanelState::new(
                    PanelKind::ColorHistory,
                    "COLOR HISTORY",
                    PanelLocation::Right,
                    true,
                    false,
                ),
                PanelState::new(
                    PanelKind::LayersManager,
                    "LAYERS MANAGER",
                    PanelLocation::Right,
                    true,
                    false,
                ),
                PanelState::new(
                    PanelKind::Reference,
                    "REFERENCE",
                    PanelLocation::Right,
                    true,
                    false,
                ),
            ],
            ui_scale: 1.0,
        }
    }
}
