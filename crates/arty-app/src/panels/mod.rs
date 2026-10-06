//! Dockable panels (egui_dock tabs) and the default layout of each UI mode.

mod brush_size;
mod color;
mod curve_editor;
mod layers;
mod navigator;
mod pen_settings;
mod property;
mod subtool;
mod thumbs;
pub mod toolbar;

use egui::{Id, WidgetText};
use egui_dock::{DockState, NodeIndex, TabViewer};
use egui_phosphor::regular as icon;
use serde::{Deserialize, Serialize};

use crate::canvas::CanvasPane;
use crate::shell::{Shell, UiMode};
use crate::studio::Studio;
use crate::theme::Palette;
pub use property::simple_sliders;
pub use subtool::PreviewCache;
pub use thumbs::ThumbCache;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tab {
    Canvas,
    SubTool,
    ToolProperty,
    BrushSize,
    Color,
    ColorSet,
    Layers,
    Navigator,
}

impl Tab {
    pub const PANELS: [Tab; 7] =
        [Tab::SubTool, Tab::ToolProperty, Tab::BrushSize, Tab::Color, Tab::ColorSet, Tab::Layers, Tab::Navigator];

    pub fn title(self) -> String {
        use crate::text::{Key, t};
        let (i, name) = match self {
            Tab::Canvas => (icon::IMAGE_SQUARE, t(Key::TabCanvas)),
            Tab::SubTool => (icon::PEN_NIB, t(Key::TabSubTool)),
            Tab::ToolProperty => (icon::SLIDERS_HORIZONTAL, t(Key::TabToolProperty)),
            Tab::BrushSize => (icon::CIRCLE_HALF, t(Key::TabBrushSize)),
            Tab::Color => (icon::PALETTE, t(Key::TabColor)),
            Tab::ColorSet => (icon::SQUARES_FOUR, t(Key::TabColorSet)),
            Tab::Layers => (icon::STACK, t(Key::TabLayers)),
            Tab::Navigator => (icon::COMPASS, t(Key::TabNavigator)),
        };
        format!("{i}  {name}")
    }
}

pub fn default_layout(mode: UiMode) -> DockState<Tab> {
    match mode {
        UiMode::Simple => simple_layout(),
        UiMode::Studio => studio_layout(),
    }
}

/// Canvas plus one column: color wheel (with recent colors) above layers.
fn simple_layout() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Canvas]);
    let s = dock.main_surface_mut();
    let [_, right] = s.split_right(NodeIndex::root(), 0.78, vec![Tab::Color]);
    s.split_below(right, 0.45, vec![Tab::Layers]);
    dock
}

/// Clip Studio-style arrangement: sub tool + properties on the left,
/// navigator / color / layers on the right.
fn studio_layout() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Canvas]);
    let s = dock.main_surface_mut();
    let [canvas, left] = s.split_left(NodeIndex::root(), 0.19, vec![Tab::SubTool]);
    s.split_below(left, 0.42, vec![Tab::ToolProperty, Tab::BrushSize]);
    let [_, right] = s.split_right(canvas, 0.76, vec![Tab::Color, Tab::ColorSet]);
    let [color, _] = s.split_above(right, 0.24, vec![Tab::Navigator]);
    s.split_below(color, 0.42, vec![Tab::Layers]);
    dock
}

/// One dock arrangement per UI mode: switching modes leaves the other as it was.
#[derive(Clone)]
pub struct Layouts {
    pub simple: DockState<Tab>,
    pub studio: DockState<Tab>,
}

impl Default for Layouts {
    fn default() -> Self {
        Self { simple: default_layout(UiMode::Simple), studio: default_layout(UiMode::Studio) }
    }
}

impl Layouts {
    pub fn get_mut(&mut self, mode: UiMode) -> &mut DockState<Tab> {
        match mode {
            UiMode::Simple => &mut self.simple,
            UiMode::Studio => &mut self.studio,
        }
    }
}

pub struct Viewer<'a> {
    pub studio: &'a mut Studio,
    pub shell: &'a mut Shell,
    pub canvas: &'a mut CanvasPane,
    pub previews: &'a mut PreviewCache,
    pub thumbs: &'a mut ThumbCache,
}

impl TabViewer for Viewer<'_> {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> Id {
        Id::new(("arty-tab", *tab))
    }

    fn title(&mut self, tab: &mut Tab) -> WidgetText {
        tab.title().into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        match tab {
            Tab::Canvas => self.canvas.ui(ui, self.studio, self.shell),
            Tab::SubTool => subtool::ui(ui, self.studio, self.shell, self.previews),
            Tab::ToolProperty => property::ui(ui, self.studio, self.shell),
            Tab::BrushSize => brush_size::ui(ui, self.studio),
            Tab::Color => {
                color::wheel_ui(ui, self.studio);
                // Simple has no Color Set tab: recent colors sit under the wheel.
                if self.shell.ui_mode == UiMode::Simple {
                    ui.add_space(6.0);
                    color::recent_ui(ui, self.studio, UiMode::Simple);
                }
            }
            Tab::ColorSet => color::swatches_ui(ui, self.studio),
            Tab::Layers => layers::ui(ui, self.studio, self.shell, self.thumbs),
            Tab::Navigator => navigator::ui(ui, self.studio, self.shell),
        }
    }

    fn is_closeable(&self, tab: &Tab) -> bool {
        *tab != Tab::Canvas
    }

    fn allowed_in_windows(&self, tab: &mut Tab) -> bool {
        *tab != Tab::Canvas
    }

    fn clear_background(&self, tab: &Tab) -> bool {
        *tab != Tab::Canvas
    }

    /// The canvas surround reaches the panel edges.
    fn tab_style_override(&self, tab: &Tab, global: &egui_dock::TabStyle) -> Option<egui_dock::TabStyle> {
        (*tab == Tab::Canvas).then(|| {
            let mut s = global.clone();
            s.tab_body.inner_margin = egui::Margin::ZERO;
            s
        })
    }

    fn scroll_bars(&self, tab: &Tab) -> [bool; 2] {
        match tab {
            Tab::Canvas | Tab::Layers | Tab::SubTool => [false, false],
            _ => [false, true],
        }
    }
}

/// Width of a slider's value box, so value boxes line up in a column.
pub const VALUE_W: f32 = 52.0;
/// Shortest slider rail before the row is allowed to overflow.
const MIN_RAIL: f32 = 36.0;

/// A slider whose rail fills the rest of the row, value box included.
pub fn fill_slider(ui: &mut egui::Ui, slider: egui::Slider<'_>) -> egui::Response {
    ui.scope(|ui| {
        let rail = (ui.available_width() - VALUE_W - ui.spacing().item_spacing.x).max(MIN_RAIL);
        let s = ui.spacing_mut();
        s.interact_size.x = VALUE_W;
        s.slider_width = rail;
        ui.add(slider)
    })
    .inner
}

/// Width of each of `n` buttons sharing the row evenly; icon buttons
/// get a narrow padding so a row of them fits a narrow panel.
pub fn even_width(ui: &mut egui::Ui, n: usize) -> f32 {
    ui.spacing_mut().button_padding.x = 2.0;
    ui.spacing_mut().item_spacing.x = 3.0;
    let n = n.max(1) as f32;
    ((ui.available_width() - ui.spacing().item_spacing.x * (n - 1.0)) / n).floor().max(0.0)
}

/// Joined toggle buttons, one of `options` selected; returns the one
/// clicked. Laid out in the parent's direction, read left to right.
pub fn segmented<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    pal: &Palette,
    current: T,
    options: &[(T, &str)],
    tip: &str,
) -> Option<T> {
    // Own colors: the menu bar makes buttons frameless.
    let frame = egui::Frame::new()
        .fill(pal.button)
        .stroke(egui::Stroke::new(1.0, pal.separator))
        .corner_radius(5)
        .inner_margin(2);
    frame
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.spacing_mut().interact_size.y -= 4.0;
            let mut picked = None;
            let mut add = |ui: &mut egui::Ui, &(value, label): &(T, &str)| {
                let r = ui.selectable_label(value == current, egui::RichText::new(label).strong()).on_hover_text(tip);
                if r.clicked() && value != current {
                    picked = Some(value);
                }
            };
            if ui.layout().prefer_right_to_left() {
                options.iter().rev().for_each(|o| add(ui, o));
            } else {
                options.iter().for_each(|o| add(ui, o));
            }
            picked
        })
        .inner
}

/// The one-click [ไทย | EN] switch; applies at once and is saved with the settings.
pub fn lang_switch(ui: &mut egui::Ui, shell: &mut Shell) {
    let langs = crate::text::Lang::ALL.map(|l| (l, l.short()));
    let pal = shell.theme.palette();
    if let Some(l) = segmented(ui, &pal, shell.lang, &langs, crate::text::t(crate::text::Key::LanguageLabel)) {
        shell.set_lang(l);
    }
}

/// Section header used inside panels.
pub fn section(ui: &mut egui::Ui, text: &str) {
    ui.add_space(2.0);
    ui.label(egui::RichText::new(text).small().strong().color(ui.visuals().weak_text_color()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{self, Command};
    use crate::studio::Studio;
    use crate::theme::ThemeKind;
    use egui_dock::SurfaceIndex;

    fn tabs(dock: &DockState<Tab>) -> Vec<Tab> {
        let mut v: Vec<Tab> = dock.iter_all_tabs().map(|(_, t)| *t).collect();
        v.sort_by_key(|t| *t as u8);
        v
    }

    #[test]
    fn layouts_hold_the_expected_tabs() {
        for mode in UiMode::ALL {
            let dock = default_layout(mode);
            assert_eq!(dock.surfaces_count(), 1, "{mode:?}: no floating windows");
            let all = tabs(&dock);
            let mut once = all.clone();
            once.dedup();
            assert_eq!(all, once, "{mode:?}: a tab shows twice");
            assert!(dock.find_tab(&Tab::Canvas).is_some_and(|p| p.surface == SurfaceIndex::main()), "{mode:?}");
        }
        assert_eq!(tabs(&default_layout(UiMode::Simple)), [Tab::Canvas, Tab::Color, Tab::Layers]);
        let mut studio = vec![Tab::Canvas];
        studio.extend(Tab::PANELS);
        studio.sort_by_key(|t| *t as u8);
        assert_eq!(tabs(&default_layout(UiMode::Studio)), studio);
    }

    /// Simple: the canvas, then one column with color above layers.
    #[test]
    fn simple_is_one_right_hand_column() {
        let dock = default_layout(UiMode::Simple);
        let node = |tab| dock.find_tab(&tab).expect("tab").node;
        let column = NodeIndex::root().right();
        assert_eq!(node(Tab::Canvas), NodeIndex::root().left());
        assert_eq!(node(Tab::Color), column.left());
        assert_eq!(node(Tab::Layers), column.right());
    }

    /// Switching modes shows the other mode's arrangement and leaves this one as it was.
    #[test]
    fn switching_keeps_each_mode_layout() {
        let mut studio = Studio::new(arty_core::Document::new(64, 64, 72));
        let mut shell = Shell::new(ThemeKind::Dark);
        let mut layouts = Layouts::default();
        commands::execute(Command::SetUiMode(UiMode::Studio), &mut studio, &mut shell);
        let dock = layouts.get_mut(shell.ui_mode);
        let path = dock.find_tab(&Tab::Navigator).expect("navigator");
        dock.remove_tab(path);

        commands::execute(Command::SetUiMode(UiMode::Simple), &mut studio, &mut shell);
        assert_eq!(shell.ui_mode, UiMode::Simple);
        let dock = layouts.get_mut(shell.ui_mode);
        assert_eq!(tabs(dock), [Tab::Canvas, Tab::Color, Tab::Layers]);
        dock.add_window(vec![Tab::Navigator]);

        commands::execute(Command::SetUiMode(UiMode::Studio), &mut studio, &mut shell);
        assert!(layouts.get_mut(shell.ui_mode).find_tab(&Tab::Navigator).is_none(), "studio edit kept");
        assert_eq!(tabs(layouts.get_mut(shell.ui_mode)).len(), Tab::PANELS.len());
        commands::execute(Command::SetUiMode(UiMode::Simple), &mut studio, &mut shell);
        assert!(layouts.get_mut(shell.ui_mode).find_tab(&Tab::Navigator).is_some(), "simple edit kept");
    }
}
