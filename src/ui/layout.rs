use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PanelKind {
    Tools,
    BrushPresets,
    BrushSettings,
    Stabilizer,
    Navigator,
    ColorWheel,
    ColorHistory,
    LayersManager,
    LayerEffect,
    Reference,
    AdvancedDebug,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    #[allow(dead_code)]
    pub fn new(kind: PanelKind, title: &str, location: PanelLocation, visible: bool, collapsed: bool) -> Self {
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

impl Default for WorkspaceLayout {
    fn default() -> Self {
        Self {
            left_panel_visible: true,
            right_panel_visible: true,
            left_panel_collapsed: false,
            right_panel_collapsed: false,
            left_panel_width: 240.0,
            right_panel_width: 280.0,
            panels: Vec::new(),
            ui_scale: 1.0,
        }
    }
}
