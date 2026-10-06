//! Visual theme: compact, low-contrast chrome so the artwork stands out.

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Visuals};
use serde::{Deserialize, Serialize};

use crate::shell::UiMode;

/// Click target height in Studio (compact).
pub const TARGET_STUDIO: f32 = 20.0;
/// Click target height in Simple: WCAG 2.2 target size (2.5.8) minimum.
pub const TARGET_SIMPLE: f32 = 24.0;
/// Width of the left tool bar: icons only (Studio) or icons with labels (Simple).
pub const TOOLBAR_STUDIO_WIDTH: f32 = 46.0;
pub const TOOLBAR_SIMPLE_WIDTH: f32 = 112.0;
/// Studio tool bar icon button (square).
pub const TOOL_BUTTON: f32 = 32.0;
/// Simple tool bar button height (icon + label row).
pub const SIMPLE_TOOL_HEIGHT: f32 = 32.0;
pub const TOOL_ICON: f32 = 18.0;
/// Slider length in the Simple slider bar under the canvas.
pub const SIMPLE_SLIDER_WIDTH: f32 = 160.0;
/// Color chip in the swatch / recent color grids (Studio).
pub const SWATCH_STUDIO: f32 = 20.0;

/// Click target height for a UI mode.
pub fn target_height(mode: UiMode) -> f32 {
    match mode {
        UiMode::Simple => TARGET_SIMPLE,
        UiMode::Studio => TARGET_STUDIO,
    }
}

/// Swatch size for a UI mode (a full click target in Simple).
pub fn swatch_size(mode: UiMode) -> f32 {
    target_height(mode).max(SWATCH_STUDIO)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThemeKind {
    #[default]
    Dark,
    Light,
}

/// Colors used by custom-painted widgets (canvas surround, layer rows…).
#[derive(Clone, Copy)]
pub struct Palette {
    pub accent: Color32,
    pub accent_weak: Color32,
    pub workspace: Color32,
    pub page_shadow: Color32,
    pub text_weak: Color32,
    pub row_selected: Color32,
    pub row_hover: Color32,
    pub clip_marker: Color32,
}

impl ThemeKind {
    pub fn palette(self) -> Palette {
        match self {
            ThemeKind::Dark => Palette {
                accent: Color32::from_rgb(76, 141, 255),
                accent_weak: Color32::from_rgb(44, 72, 120),
                workspace: Color32::from_rgb(58, 59, 63),
                page_shadow: Color32::from_black_alpha(90),
                text_weak: Color32::from_rgb(150, 152, 160),
                row_selected: Color32::from_rgb(45, 74, 125),
                row_hover: Color32::from_rgb(52, 54, 60),
                clip_marker: Color32::from_rgb(232, 96, 96),
            },
            ThemeKind::Light => Palette {
                accent: Color32::from_rgb(38, 110, 230),
                accent_weak: Color32::from_rgb(196, 214, 245),
                workspace: Color32::from_rgb(168, 170, 176),
                page_shadow: Color32::from_black_alpha(60),
                text_weak: Color32::from_rgb(110, 112, 120),
                row_selected: Color32::from_rgb(190, 210, 245),
                row_hover: Color32::from_rgb(222, 224, 230),
                clip_marker: Color32::from_rgb(210, 60, 60),
            },
        }
    }
}

pub const NOTO_SANS_THAI_UI: &[u8] =
    include_bytes!("../assets/fonts/NotoSansThaiUI-Regular-static.ttf");
pub const NOTO_SANS_THAI_UI_NAME: &str = "NotoSansThaiUI-Regular";

pub fn font_definitions() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        NOTO_SANS_THAI_UI_NAME.to_owned(),
        egui::FontData::from_static(NOTO_SANS_THAI_UI).into(),
    );
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .push(NOTO_SANS_THAI_UI_NAME.to_owned());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .push(NOTO_SANS_THAI_UI_NAME.to_owned());
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    fonts
}

pub fn install_fonts(ctx: &egui::Context) {
    ctx.set_fonts(font_definitions());
}

pub fn apply(ctx: &egui::Context, kind: ThemeKind, mode: UiMode) {
    let p = kind.palette();
    let mut v = match kind {
        ThemeKind::Dark => Visuals::dark(),
        ThemeKind::Light => Visuals::light(),
    };
    let (panel, extreme, faint, inactive, hovered, text, border) = match kind {
        ThemeKind::Dark => (
            Color32::from_rgb(38, 39, 43),
            Color32::from_rgb(27, 28, 31),
            Color32::from_rgb(44, 45, 50),
            Color32::from_rgb(52, 54, 60),
            Color32::from_rgb(64, 66, 74),
            Color32::from_rgb(222, 223, 228),
            Color32::from_rgb(24, 25, 28),
        ),
        ThemeKind::Light => (
            Color32::from_rgb(236, 237, 240),
            Color32::from_rgb(250, 250, 252),
            Color32::from_rgb(228, 229, 233),
            Color32::from_rgb(218, 220, 226),
            Color32::from_rgb(205, 208, 216),
            Color32::from_rgb(34, 35, 40),
            Color32::from_rgb(196, 198, 205),
        ),
    };
    v.panel_fill = panel;
    v.window_fill = panel;
    v.extreme_bg_color = extreme;
    v.faint_bg_color = faint;
    v.window_stroke = Stroke::new(1.0, border);
    v.window_corner_radius = CornerRadius::same(6);
    v.menu_corner_radius = CornerRadius::same(6);
    v.selection.bg_fill = p.accent_weak;
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.hyperlink_color = p.accent;
    v.override_text_color = Some(text);

    let r = CornerRadius::same(4);
    v.widgets.noninteractive.bg_fill = panel;
    v.widgets.noninteractive.weak_bg_fill = panel;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, border);
    v.widgets.noninteractive.corner_radius = r;
    for (w, fill) in [
        (&mut v.widgets.inactive, inactive),
        (&mut v.widgets.hovered, hovered),
        (&mut v.widgets.active, p.accent_weak),
        (&mut v.widgets.open, hovered),
    ] {
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.corner_radius = r;
        w.bg_stroke = Stroke::NONE;
    }
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.accent_weak);
    v.widgets.active.bg_stroke = Stroke::new(1.0, p.accent);
    v.slider_trailing_fill = true;

    ctx.set_visuals(v);
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(6.0, 3.0);
        style.spacing.interact_size.y = target_height(mode);
        style.spacing.slider_width = 120.0;
        style.text_styles = [
            (TextStyle::Small, FontId::new(10.5, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(12.5, FontFamily::Proportional)),
            (TextStyle::Heading, FontId::new(15.0, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace)),
        ]
        .into();
    });
}

/// Dock styling derived from the egui style.
pub fn dock_style(ui: &egui::Ui, kind: ThemeKind) -> egui_dock::Style {
    let p = kind.palette();
    let mut s = egui_dock::Style::from_egui(ui.style().as_ref());
    s.tab_bar.height = 24.0;
    s.tab.tab_body.inner_margin = egui::Margin::same(6);
    s.tab.active.text_color = ui.visuals().strong_text_color();
    s.tab.focused.text_color = ui.visuals().strong_text_color();
    s.tab.inactive.text_color = p.text_weak;
    s.tab.hovered.text_color = ui.visuals().text_color();
    s.separator.width = 2.0;
    s.separator.color_idle = ui.visuals().extreme_bg_color;
    s.separator.color_hovered = p.accent_weak;
    s.separator.color_dragged = p.accent;
    s
}
