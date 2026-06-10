use crate::app::PaintApp;
use crate::ui::layout::{
    FloatingPanelState, PanelKind, PanelLocation, PanelState, WorkspaceLayout,
};
use egui::Context;
use serde_json;
use std::fs;
use std::path::PathBuf;

pub fn get_preferences_path() -> PathBuf {
    let mut path = if let Ok(appdata) = std::env::var("APPDATA") {
        PathBuf::from(appdata)
    } else if let Ok(userprofile) = std::env::var("USERPROFILE") {
        PathBuf::from(userprofile).join("AppData").join("Roaming")
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".config")
    } else {
        PathBuf::from(".")
    };
    path.push("ARTY");
    path.push("preferences.toml");
    path
}

pub fn apply_theme(theme: &str, ctx: &Context) {
    if theme == "Light" {
        ctx.set_visuals(egui::Visuals::light());
    } else if theme == "Dark" {
        ctx.set_visuals(egui::Visuals::dark());
    } else {
        let mut visuals = egui::Visuals::light();
        visuals.panel_fill = egui::Color32::from_rgb(240, 240, 240);
        visuals.window_fill = egui::Color32::from_rgb(245, 245, 245);
        visuals.widgets.active.bg_fill = egui::Color32::from_rgb(180, 200, 240);
        visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(215, 225, 250);
        visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(230, 230, 230);
        ctx.set_visuals(visuals);
    }
}

pub fn save_preferences(app: &PaintApp) {
    let path = get_preferences_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut toml_content = format!(
        "theme = {:?}\n\
         ui_scale = {:.1}\n\
         canvas_bg = {:?}\n\
         autosave_enabled = {}\n\
         autosave_interval_mins = {}\n",
        app.pref_theme,
        app.pref_ui_scale,
        app.pref_canvas_bg,
        app.pref_autosave_enabled,
        app.pref_autosave_interval_mins
    );
    let recent_files_toml = if app.recent_files.is_empty() {
        "recent_files = []\n".to_string()
    } else {
        let mut s = "recent_files = [\n".to_string();
        for file in &app.recent_files {
            s.push_str(&format!("    {:?},\n", file));
        }
        s.push_str("]\n");
        s
    };
    toml_content.push_str(&recent_files_toml);

    if let Err(e) = fs::write(&path, toml_content) {
        log::error!("Failed to write preferences to {:?}: {}", path, e);
    } else {
        log::info!("Saved preferences to {:?}", path);
    }
}

pub fn load_preferences(app: &mut PaintApp, ctx: &Context) {
    let path = get_preferences_path();
    if !path.exists() {
        return;
    }
    match fs::read_to_string(&path) {
        Ok(content) => match content.parse::<toml_edit::DocumentMut>() {
            Ok(doc) => {
                if let Some(theme) = doc.get("theme").and_then(|i| i.as_str()) {
                    app.pref_theme = theme.to_string();
                    apply_theme(theme, ctx);
                }
                if let Some(ui_scale) = doc.get("ui_scale").and_then(|i| i.as_float()) {
                    let scale = ui_scale as f32;
                    app.pref_ui_scale = scale;
                    ctx.set_pixels_per_point(scale);
                }
                if let Some(canvas_bg) = doc.get("canvas_bg").and_then(|i| i.as_str()) {
                    app.pref_canvas_bg = canvas_bg.to_string();
                }
                if let Some(autosave_enabled) =
                    doc.get("autosave_enabled").and_then(|i| i.as_bool())
                {
                    app.pref_autosave_enabled = autosave_enabled;
                    app.autosave_enabled = autosave_enabled;
                }
                if let Some(interval) = doc
                    .get("autosave_interval_mins")
                    .and_then(|i| i.as_integer())
                {
                    let interval = interval as u32;
                    app.pref_autosave_interval_mins = interval;
                    app.autosave_interval_secs = (interval * 60) as f64;
                }
                if let Some(recent_array) = doc.get("recent_files").and_then(|i| i.as_array()) {
                    app.recent_files.clear();
                    for val in recent_array.iter() {
                        if let Some(s) = val.as_str() {
                            app.recent_files.push(s.to_string());
                        }
                    }
                }
                log::info!("Loaded preferences from {:?}", path);
            }
            Err(e) => {
                log::error!("Failed to parse preferences TOML: {}", e);
            }
        },
        Err(e) => {
            log::error!("Failed to read preferences file: {}", e);
        }
    }
}

// ── Workspace Layout Persistence ──

/// Returns the path to the workspace layout JSON file.
pub fn get_workspace_path() -> PathBuf {
    let mut path = get_preferences_path();
    path.set_file_name("workspace.json");
    path
}

/// Save the current workspace layout to disk as JSON.
pub fn save_workspace_layout(app: &PaintApp) {
    let path = get_workspace_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    match serde_json::to_string_pretty(&app.workspace_layout) {
        Ok(json) => {
            if let Err(e) = fs::write(&path, json) {
                log::error!("Failed to write workspace layout to {:?}: {}", path, e);
            } else {
                log::info!("Saved workspace layout to {:?}", path);
            }
        }
        Err(e) => log::error!("Failed to serialize workspace layout: {}", e),
    }
}

/// Load workspace layout from disk, merging with defaults for backward compat.
pub fn load_workspace_layout(app: &mut PaintApp) {
    let path = get_workspace_path();
    if !path.exists() {
        return;
    }
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            log::error!("Failed to read workspace layout from {:?}: {}", path, e);
            return;
        }
    };
    // First try direct serde_json deserialization (fast path)
    if let Ok(loaded) = serde_json::from_str::<WorkspaceLayout>(&content) {
        merge_workspace_layout(&mut app.workspace_layout, loaded);
        return;
    }
    // Fallback: try lenient parsing from generic Value, ignoring unknown panel kinds
    match serde_json::from_str::<serde_json::Value>(&content) {
        Ok(value) => {
            if let Some(loaded) = deserialize_layout_lenient(&value) {
                merge_workspace_layout(&mut app.workspace_layout, loaded);
            } else {
                log::warn!("Could not parse workspace layout, using defaults");
            }
        }
        Err(e) => {
            log::error!("Failed to parse workspace layout JSON: {}", e);
        }
    }
}

/// Merge a loaded layout into the target, preserving defaults for missing panels.
fn merge_workspace_layout(target: &mut WorkspaceLayout, loaded: WorkspaceLayout) {
    target.left_panel_visible = loaded.left_panel_visible;
    target.right_panel_visible = loaded.right_panel_visible;
    target.left_panel_collapsed = loaded.left_panel_collapsed;
    target.right_panel_collapsed = loaded.right_panel_collapsed;
    target.left_panel_width = loaded.left_panel_width;
    target.right_panel_width = loaded.right_panel_width;
    target.ui_scale = loaded.ui_scale;
    // Merge panels: for each loaded panel matching a known kind, override the default.
    // Unknown loaded panels are silently ignored; default panels not in loaded are kept.
    for loaded_panel in loaded.panels {
        if let Some(existing) = target
            .panels
            .iter_mut()
            .find(|p| p.kind == loaded_panel.kind)
        {
            *existing = loaded_panel;
        }
    }
}

/// Lenient deserialization: parse from serde_json::Value, skipping unknown PanelKind variants.
fn deserialize_layout_lenient(value: &serde_json::Value) -> Option<WorkspaceLayout> {
    let obj = value.as_object()?;
    let mut layout = WorkspaceLayout::default();
    if let Some(v) = obj.get("left_panel_visible").and_then(|v| v.as_bool()) {
        layout.left_panel_visible = v;
    }
    if let Some(v) = obj.get("right_panel_visible").and_then(|v| v.as_bool()) {
        layout.right_panel_visible = v;
    }
    if let Some(v) = obj.get("left_panel_collapsed").and_then(|v| v.as_bool()) {
        layout.left_panel_collapsed = v;
    }
    if let Some(v) = obj.get("right_panel_collapsed").and_then(|v| v.as_bool()) {
        layout.right_panel_collapsed = v;
    }
    if let Some(v) = obj.get("left_panel_width").and_then(|v| v.as_f64()) {
        layout.left_panel_width = v as f32;
    }
    if let Some(v) = obj.get("right_panel_width").and_then(|v| v.as_f64()) {
        layout.right_panel_width = v as f32;
    }
    if let Some(v) = obj.get("ui_scale").and_then(|v| v.as_f64()) {
        layout.ui_scale = v as f32;
    }
    if let Some(panels) = obj.get("panels").and_then(|v| v.as_array()) {
        for panel_val in panels {
            let panel_obj = match panel_val.as_object() {
                Some(o) => o,
                None => continue,
            };
            let kind_str = match panel_obj.get("kind").and_then(|v| v.as_str()) {
                Some(s) => s,
                None => continue,
            };
            let kind = match PanelKind::from_str(kind_str) {
                Some(k) => k,
                None => {
                    log::debug!(
                        "Ignoring unknown panel kind in workspace file: {}",
                        kind_str
                    );
                    continue;
                }
            };
            let panel = PanelState {
                kind,
                title: panel_obj
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                location: panel_obj
                    .get("location")
                    .and_then(|v| v.as_str())
                    .and_then(PanelLocation::from_str)
                    .unwrap_or(PanelLocation::Floating),
                visible: panel_obj
                    .get("visible")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true),
                collapsed: panel_obj
                    .get("collapsed")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                floating: panel_obj
                    .get("floating")
                    .and_then(|v| deserialize_floating_state(v))
                    .unwrap_or_default(),
            };
            // Override the default panel of this kind (or add if somehow missing)
            if let Some(existing) = layout.panels.iter_mut().find(|p| p.kind == kind) {
                *existing = panel;
            } else {
                layout.panels.push(panel);
            }
        }
    }
    Some(layout)
}

fn deserialize_floating_state(value: &serde_json::Value) -> Option<FloatingPanelState> {
    let obj = value.as_object()?;
    let pos = obj.get("position").and_then(|v| v.as_array())?;
    let size = obj.get("size").and_then(|v| v.as_array())?;
    Some(FloatingPanelState {
        position: [
            pos.get(0).and_then(|v| v.as_f64())? as f32,
            pos.get(1).and_then(|v| v.as_f64())? as f32,
        ],
        size: [
            size.get(0).and_then(|v| v.as_f64())? as f32,
            size.get(1).and_then(|v| v.as_f64())? as f32,
        ],
    })
}

// ── Backward-compatible string-based PanelKind/PanelLocation parsing ──

impl PanelKind {
    /// Parse a PanelKind from its serialized string name (lenient).
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "ToolsAndPresets" => Some(PanelKind::ToolsAndPresets),
            "BrushSettings" => Some(PanelKind::BrushSettings),
            "ToolOptions" => Some(PanelKind::ToolOptions),
            "Stabilizer" => Some(PanelKind::Stabilizer),
            "Symmetry" => Some(PanelKind::Symmetry),
            "AdvancedDebug" => Some(PanelKind::AdvancedDebug),
            "Navigator" => Some(PanelKind::Navigator),
            "ColorWheel" => Some(PanelKind::ColorWheel),
            "ColorSliders" => Some(PanelKind::ColorSliders),
            "ColorPalette" => Some(PanelKind::ColorPalette),
            "ColorHistory" => Some(PanelKind::ColorHistory),
            "LayersManager" => Some(PanelKind::LayersManager),
            "Reference" => Some(PanelKind::Reference),
            _ => None,
        }
    }
}

impl PanelLocation {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "Left" => Some(PanelLocation::Left),
            "Right" => Some(PanelLocation::Right),
            "Floating" => Some(PanelLocation::Floating),
            "Hidden" => Some(PanelLocation::Hidden),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_parse_preferences() {
        let toml_data = r#"
theme = "Dark"
ui_scale = 1.5
canvas_bg = "Checkerboard"
autosave_enabled = false
autosave_interval_mins = 10
"#;
        let doc = toml_data.parse::<toml_edit::DocumentMut>().unwrap();
        assert_eq!(doc.get("theme").and_then(|i| i.as_str()), Some("Dark"));
        assert_eq!(doc.get("ui_scale").and_then(|i| i.as_float()), Some(1.5));
        assert_eq!(
            doc.get("canvas_bg").and_then(|i| i.as_str()),
            Some("Checkerboard")
        );
        assert_eq!(
            doc.get("autosave_enabled").and_then(|i| i.as_bool()),
            Some(false)
        );
        assert_eq!(
            doc.get("autosave_interval_mins")
                .and_then(|i| i.as_integer()),
            Some(10)
        );
    }
}
