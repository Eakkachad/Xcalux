//! Visual theme: compact, low-contrast chrome so the artwork stands out.

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Visuals};
use serde::{Deserialize, Serialize};

use crate::shell::{Fit, UiMode};

/// Click target height in Studio (compact).
pub const TARGET_STUDIO: f32 = 20.0;
/// Click target height in Simple: WCAG 2.2 target size (2.5.8) minimum.
pub const TARGET_SIMPLE: f32 = 24.0;
/// Width of the left tool bar: icons only (Studio) or icons with labels (Simple).
pub const TOOLBAR_STUDIO_WIDTH: f32 = 46.0;
pub const TOOLBAR_SIMPLE_WIDTH: f32 = 112.0;
pub const TOOLBAR_SIMPLE_MAX: f32 = 168.0;
/// Studio tool bar icon button (square).
pub const TOOL_BUTTON: f32 = 32.0;
/// Simple tool bar button height (icon + label row).
pub const SIMPLE_TOOL_HEIGHT: f32 = 32.0;
pub const TOOL_ICON: f32 = 18.0;
/// Widest group (label, slider, value) in the Simple slider bar under the canvas.
pub const SIMPLE_GROUP_MAX: f32 = 340.0;
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

/// Color tokens of a theme: egui's visuals and the custom-painted widgets
/// (canvas surround, layer rows…) take every color from here.
#[derive(Clone, Copy)]
pub struct Palette {
    /// Active tool, focused controls, drop markers.
    pub accent: Color32,
    /// Selection fill (rows, selected buttons, text selection, slider fill).
    pub accent_weak: Color32,
    /// Menu bar, status bar and dock tab bars.
    pub bar: Color32,
    /// Panel bodies.
    pub panel: Color32,
    /// Separators and widget borders, a few steps darker than panels.
    pub separator: Color32,
    /// Around the page on the canvas.
    pub workspace: Color32,
    pub page_shadow: Color32,
    pub text: Color32,
    /// Secondary text: WCAG AA (4.5:1) on `panel` and `bar`.
    pub text_weak: Color32,
    /// Buttons and combo boxes at rest, hovered.
    pub button: Color32,
    pub button_hover: Color32,
    /// Text fields, slider rails, check boxes.
    pub field: Color32,
    /// Menus and popups.
    pub popup: Color32,
    pub row_selected: Color32,
    pub row_hover: Color32,
    pub clip_marker: Color32,
    /// How well a page fits this machine (New Page, D3): green, amber, red.
    /// Dots next to a text that says the same (3:1 on panels and popups, WCAG
    /// 1.4.11); `fit_heavy` also colours its warning text (4.5:1).
    pub fit_roomy: Color32,
    pub fit_tight: Color32,
    pub fit_heavy: Color32,
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

impl Palette {
    /// Dot colour of a [`Fit`].
    pub fn fit(&self, fit: Fit) -> Color32 {
        match fit {
            Fit::Roomy => self.fit_roomy,
            Fit::Tight => self.fit_tight,
            Fit::Heavy => self.fit_heavy,
        }
    }
}

impl ThemeKind {
    pub fn palette(self) -> Palette {
        match self {
            ThemeKind::Dark => Palette {
                accent: Color32::from_rgb(76, 141, 255),
                accent_weak: Color32::from_rgb(44, 72, 120),
                bar: Color32::from_rgb(31, 32, 35),
                panel: Color32::from_rgb(38, 39, 43),
                separator: Color32::from_rgb(22, 23, 26),
                workspace: Color32::from_rgb(58, 59, 63),
                page_shadow: Color32::from_black_alpha(90),
                text: Color32::from_rgb(222, 223, 228),
                text_weak: Color32::from_rgb(162, 164, 172),
                button: Color32::from_rgb(52, 54, 60),
                button_hover: Color32::from_rgb(64, 66, 74),
                field: Color32::from_rgb(27, 28, 31),
                popup: Color32::from_rgb(44, 45, 50),
                row_selected: Color32::from_rgb(45, 74, 125),
                row_hover: Color32::from_rgb(52, 54, 60),
                clip_marker: Color32::from_rgb(232, 96, 96),
                fit_roomy: hex(0x5CC46C),
                fit_tight: hex(0xE8AD3A),
                fit_heavy: hex(0xF58076),
            },
            // Warm paper grey; the page stays white.
            ThemeKind::Light => Palette {
                accent: hex(0xD2782F),
                accent_weak: hex(0xF2D9C4),
                bar: hex(0xE9E6E1),
                panel: hex(0xEFECE7),
                separator: hex(0xD3CEC6),
                workspace: hex(0x8F8C88),
                page_shadow: Color32::from_black_alpha(70),
                text: hex(0x2B2A28),
                text_weak: hex(0x625D57),
                button: hex(0xE2DDD6),
                button_hover: hex(0xD8D2C9),
                field: hex(0xFBFAF8),
                popup: hex(0xF6F4F1),
                row_selected: hex(0xF2D9C4),
                row_hover: hex(0xE4E0DA),
                clip_marker: hex(0xC8463C),
                fit_roomy: hex(0x2E8B45),
                fit_tight: hex(0xB57000),
                fit_heavy: hex(0xB52D2D),
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

/// `light`: Light performance mode, no animations (shell.rs `PerfMode`).
pub fn apply(ctx: &egui::Context, kind: ThemeKind, mode: UiMode, light: bool) {
    let p = kind.palette();
    let mut v = match kind {
        ThemeKind::Dark => Visuals::dark(),
        ThemeKind::Light => Visuals::light(),
    };
    v.panel_fill = p.panel;
    v.window_fill = p.popup;
    v.extreme_bg_color = p.field;
    v.text_edit_bg_color = Some(p.field);
    v.faint_bg_color = p.bar;
    v.window_stroke = Stroke::new(1.0, p.separator);
    v.window_corner_radius = CornerRadius::same(6);
    v.menu_corner_radius = CornerRadius::same(6);
    v.selection.bg_fill = p.accent_weak;
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.hyperlink_color = p.accent;
    v.override_text_color = Some(p.text);
    v.weak_text_color = Some(p.text_weak);

    let r = CornerRadius::same(4);
    // Light widgets get a hairline border: their fills alone sit too close to the panel.
    let border = match kind {
        ThemeKind::Dark => Stroke::NONE,
        ThemeKind::Light => Stroke::new(1.0, p.separator),
    };
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.panel;
    w.noninteractive.weak_bg_fill = p.panel;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.separator);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = r;
    for (w, weak, fill, stroke) in [
        (&mut w.inactive, p.button, p.field, border),
        (&mut w.hovered, p.button_hover, p.field, Stroke::new(1.0, p.accent)),
        (&mut w.active, p.accent_weak, p.accent_weak, Stroke::new(1.0, p.accent)),
        (&mut w.open, p.button_hover, p.field, border),
    ] {
        w.weak_bg_fill = weak;
        w.bg_fill = fill;
        w.bg_stroke = stroke;
        w.fg_stroke = Stroke::new(1.0, p.text);
        w.corner_radius = r;
    }
    if kind == ThemeKind::Dark {
        // Dark rails and check boxes read better raised than sunken.
        v.widgets.inactive.bg_fill = p.button;
        v.widgets.hovered.bg_fill = p.button_hover;
        v.widgets.open.bg_fill = p.button_hover;
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.accent_weak);
    }
    v.slider_trailing_fill = true;

    ctx.set_visuals(v);
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(6.0, 3.0);
        style.spacing.interact_size.y = target_height(mode);
        style.spacing.slider_width = 120.0;
        style.animation_time = if light { 0.0 } else { egui::Style::default().animation_time };
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

/// Frame of the menu and status bars.
pub fn bar_frame(style: &egui::Style, kind: ThemeKind) -> egui::Frame {
    let p = kind.palette();
    egui::Frame::side_top_panel(style).fill(p.bar)
}

/// Dock styling from the theme tokens: tab bars on the bar color, the
/// active tab joined to its panel body.
pub fn dock_style(ui: &egui::Ui, kind: ThemeKind) -> egui_dock::Style {
    let p = kind.palette();
    let mut s = egui_dock::Style::from_egui(ui.style().as_ref());
    s.main_surface_border_stroke = Stroke::NONE;
    s.tab_bar.height = 24.0;
    s.tab_bar.bg_fill = p.bar;
    s.tab_bar.hline_color = p.separator;
    s.tab.tab_body.inner_margin = egui::Margin::same(6);
    s.tab.tab_body.bg_fill = p.panel;
    s.tab.tab_body.stroke = Stroke::new(1.0, p.separator);
    for (t, fill, text) in [
        (&mut s.tab.active, p.panel, p.text),
        (&mut s.tab.focused, p.panel, p.text),
        (&mut s.tab.active_with_kb_focus, p.panel, p.text),
        (&mut s.tab.focused_with_kb_focus, p.panel, p.text),
        (&mut s.tab.inactive, p.bar, p.text_weak),
        (&mut s.tab.inactive_with_kb_focus, p.bar, p.text_weak),
        (&mut s.tab.hovered, p.row_hover, p.text),
    ] {
        t.bg_fill = fill;
        t.text_color = text;
        t.outline_color = p.separator;
    }
    s.buttons.close_tab_color = p.text_weak;
    s.buttons.close_tab_active_color = p.text;
    s.buttons.close_tab_bg_fill = p.button_hover;
    s.separator.width = 2.0;
    s.separator.color_idle = p.separator;
    s.separator.color_hovered = p.accent_weak;
    s.separator.color_dragged = p.accent;
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG 2.x contrast ratio.
    fn contrast(a: Color32, b: Color32) -> f32 {
        let lum = |c: Color32| {
            let ch = |v: u8| {
                let v = v as f32 / 255.0;
                if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
        };
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn text_passes_wcag_aa_on_the_chrome() {
        for kind in [ThemeKind::Light, ThemeKind::Dark] {
            let p = kind.palette();
            for bg in [p.panel, p.bar, p.popup, p.button] {
                assert!(contrast(p.text, bg) >= 7.0, "{:?} text on {bg:?}", kind as u8);
                assert!(contrast(p.text_weak, bg) >= 4.5, "{:?} weak text on {bg:?}: {}", kind as u8, contrast(p.text_weak, bg));
            }
        }
    }

    #[test]
    fn fit_colors_read_on_both_themes() {
        for kind in [ThemeKind::Light, ThemeKind::Dark] {
            let p = kind.palette();
            for bg in [p.panel, p.bar, p.popup] {
                for c in [p.fit_roomy, p.fit_tight, p.fit_heavy] {
                    assert!(contrast(c, bg) >= 3.0, "{:?} {c:?} on {bg:?}: {}", kind as u8, contrast(c, bg));
                }
                assert!(contrast(p.fit_heavy, bg) >= 4.5, "{:?} warning text on {bg:?}", kind as u8);
            }
        }
    }

    #[test]
    fn light_mode_turns_animations_off() {
        let ctx = egui::Context::default();
        apply(&ctx, ThemeKind::Light, UiMode::Simple, true);
        assert_eq!(ctx.global_style().animation_time, 0.0);
        apply(&ctx, ThemeKind::Light, UiMode::Simple, false);
        assert_eq!(ctx.global_style().animation_time, egui::Style::default().animation_time);
    }

    #[test]
    fn light_theme_uses_the_paper_tokens() {
        let p = ThemeKind::Light.palette();
        assert_eq!(p.bar, hex(0xE9E6E1));
        assert_eq!(p.panel, hex(0xEFECE7));
        assert_eq!(p.workspace, hex(0x8F8C88));
        assert_eq!(p.text, hex(0x2B2A28));
        assert_eq!(p.accent_weak, hex(0xF2D9C4));
        // Separators a few steps darker than panels.
        assert!(contrast(p.separator, p.panel) > 1.15);
    }
}
