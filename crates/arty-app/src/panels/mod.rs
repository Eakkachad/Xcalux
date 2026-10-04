//! Dockable panels (egui_dock tabs) and the default Clip Studio-like layout.

mod brush_size;
mod color;
mod layers;
mod navigator;
mod property;
mod subtool;
mod thumbs;
pub mod toolbar;

use egui::{Id, WidgetText};
use egui_dock::{DockState, NodeIndex, TabViewer};
use egui_phosphor::regular as icon;
use serde::{Deserialize, Serialize};

use crate::canvas::CanvasPane;
use crate::shell::Shell;
use crate::studio::Studio;
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
        let (i, name) = match self {
            Tab::Canvas => (icon::IMAGE_SQUARE, "Canvas"),
            Tab::SubTool => (icon::PEN_NIB, "Sub Tool"),
            Tab::ToolProperty => (icon::SLIDERS_HORIZONTAL, "Tool Property"),
            Tab::BrushSize => (icon::CIRCLE_HALF, "Brush Size"),
            Tab::Color => (icon::PALETTE, "Color"),
            Tab::ColorSet => (icon::SQUARES_FOUR, "Color Set"),
            Tab::Layers => (icon::STACK, "Layer"),
            Tab::Navigator => (icon::COMPASS, "Navigator"),
        };
        format!("{i}  {name}")
    }
}

/// Clip Studio-style arrangement: sub tool + properties on the left,
/// navigator / color / layers on the right.
pub fn default_layout() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Canvas]);
    let s = dock.main_surface_mut();
    let [canvas, left] = s.split_left(NodeIndex::root(), 0.19, vec![Tab::SubTool]);
    s.split_below(left, 0.42, vec![Tab::ToolProperty, Tab::BrushSize]);
    let [_, right] = s.split_right(canvas, 0.76, vec![Tab::Color, Tab::ColorSet]);
    let [color, _] = s.split_above(right, 0.24, vec![Tab::Navigator]);
    s.split_below(color, 0.42, vec![Tab::Layers]);
    dock
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
            Tab::ToolProperty => property::ui(ui, self.studio),
            Tab::BrushSize => brush_size::ui(ui, self.studio),
            Tab::Color => color::wheel_ui(ui, self.studio),
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
