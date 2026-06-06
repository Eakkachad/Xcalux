use crate::app::PaintApp;
use egui::Context;
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
    let toml_content = format!(
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
