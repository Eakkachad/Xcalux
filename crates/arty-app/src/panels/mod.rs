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

    fn scroll_bars(&self, tab: &Tab) -> [bool; 2] {
        match tab {
            Tab::Canvas | Tab::Layers | Tab::SubTool => [false, false],
            _ => [false, true],
        }
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
