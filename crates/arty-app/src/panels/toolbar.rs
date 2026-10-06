//! Vertical tool bar with main/sub colors: every tool as an icon (Studio, CSP
//! "Tool" palette) or six labelled tools plus a "more" menu (Simple).

use arty_brush::BrushGroup;
use egui::{Color32, CornerRadius, Rect, Sense, Stroke, Vec2};
use egui_phosphor::regular as icon;

use crate::commands::{self, Command};
use crate::shell::Shell;
use crate::studio::{FrameMode, Studio, Tool};
use crate::text::{t, Key};
use crate::theme;
use crate::tools::select::SelShape;

const TOOLS: &[(Tool, &str)] = &[
    (Tool::Brush(BrushGroup::Pen), icon::PEN_NIB),
    (Tool::Brush(BrushGroup::Pencil), icon::PENCIL_SIMPLE),
    (Tool::Brush(BrushGroup::Brush), icon::PAINT_BRUSH),
    (Tool::Brush(BrushGroup::Airbrush), icon::SPRAY_BOTTLE),
    (Tool::Brush(BrushGroup::Blend), icon::DROP_HALF),
    (Tool::Brush(BrushGroup::Eraser), icon::ERASER),
];

const EDIT_TOOLS: &[(Tool, &str)] = &[
    (Tool::Move, icon::ARROWS_OUT_CARDINAL),
    (Tool::Select, icon::SELECTION),
    (Tool::MagicWand, icon::MAGIC_WAND),
    (Tool::Fill, icon::PAINT_BUCKET),
];

const FRAME_TOOLS: &[(Tool, &str)] = &[
    (Tool::Frame(FrameMode::Rect), icon::FRAME_CORNERS),
    (Tool::Frame(FrameMode::Cut), icon::SCISSORS),
    (Tool::Frame(FrameMode::Edit), icon::BOUNDING_BOX),
];

const VIEW_TOOLS: &[(Tool, &str)] = &[
    (Tool::Eyedropper, icon::EYEDROPPER),
    (Tool::Hand, icon::HAND),
    (Tool::Rotate, icon::ARROWS_CLOCKWISE),
    (Tool::Zoom, icon::MAGNIFYING_GLASS),
];

/// Simple mode's tools: (tool, selection shape it picks, icon).
pub const SIMPLE_TOOLS: [(Tool, Option<SelShape>, &str); 6] = [
    (Tool::Brush(BrushGroup::Pen), None, icon::PEN_NIB),
    (Tool::Brush(BrushGroup::Pencil), None, icon::PENCIL_SIMPLE),
    (Tool::Brush(BrushGroup::Eraser), None, icon::ERASER),
    (Tool::Fill, None, icon::PAINT_BUCKET),
    (Tool::Eyedropper, None, icon::EYEDROPPER),
    (Tool::Select, Some(SelShape::Lasso), icon::LASSO),
];

fn simple_label(i: usize) -> &'static str {
    match SIMPLE_TOOLS[i] {
        (_, Some(shape), _) => shape.label(),
        (tool, None, _) => tool.label(),
    }
}

/// Width of the Simple tool bar: its labels fit, in this language.
pub fn simple_width(ui: &egui::Ui, studio: &Studio) -> f32 {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let widest = (0..SIMPLE_TOOLS.len())
        .map(simple_label)
        .chain([more_label(studio)])
        .map(|l| ui.fonts_mut(|f| f.layout_no_wrap(l.to_owned(), font.clone(), Color32::WHITE).size().x))
        .fold(0.0, f32::max);
    // Icon column, label, end padding and the panel's side margins.
    (theme::SIMPLE_TOOL_HEIGHT + widest + 8.0 + 16.0).clamp(theme::TOOLBAR_SIMPLE_WIDTH, theme::TOOLBAR_SIMPLE_MAX)
}

/// The Simple tool that is active, if any.
pub fn simple_active(studio: &Studio) -> Option<usize> {
    SIMPLE_TOOLS
        .iter()
        .position(|&(tool, shape, _)| tool == studio.tool && shape.is_none_or(|s| s == studio.opts.select.shape))
}

/// Label of the "more" slot: the active tool when Simple doesn't list it.
pub fn more_label(studio: &Studio) -> &'static str {
    match simple_active(studio) {
        Some(_) => t(Key::ToolbarMore),
        None => studio.tool.label(),
    }
}

fn tool_tip(tool: Tool) -> &'static str {
    match tool {
        Tool::Brush(BrushGroup::Pen) => t(Key::TooltipPen),
        Tool::Brush(BrushGroup::Pencil) => t(Key::TooltipPencil),
        Tool::Brush(BrushGroup::Brush) => t(Key::TooltipBrush),
        Tool::Brush(BrushGroup::Airbrush) => t(Key::TooltipAirbrush),
        Tool::Brush(BrushGroup::Blend) => t(Key::TooltipBlend),
        Tool::Brush(BrushGroup::Eraser) => t(Key::TooltipEraser),
        Tool::Move => t(Key::TooltipMove),
        Tool::Select => t(Key::TooltipSelect),
        Tool::MagicWand => t(Key::TooltipMagicWand),
        Tool::Fill => t(Key::TooltipFill),
        Tool::Frame(FrameMode::Rect) => t(Key::FrameModeRect),
        Tool::Frame(FrameMode::Cut) => t(Key::FrameModeCut),
        Tool::Frame(FrameMode::Edit) => t(Key::TooltipFrameEdit),
        Tool::Eyedropper => t(Key::TooltipEyedropper),
        Tool::Hand => t(Key::TooltipHand),
        Tool::Rotate => t(Key::TooltipRotate),
        Tool::Zoom => t(Key::TooltipZoom),
    }
}

pub fn ui(ui: &mut egui::Ui, studio: &mut Studio, shell: &mut Shell) {
    let pal = shell.theme.palette();
    if shell.ui_mode == crate::shell::UiMode::Simple {
        simple_ui(ui, studio, shell, pal.accent);
        return;
    }
    ui.vertical_centered(|ui| {
        ui.add_space(4.0);
        for group in [TOOLS, EDIT_TOOLS, FRAME_TOOLS, VIEW_TOOLS] {
            for &(tool, glyph) in group {
                let selected = studio.tool == tool;
                if tool_button(ui, glyph, selected, pal.accent).on_hover_text(tool_tip(tool)).clicked() {
                    commands::execute(Command::SelectTool(tool), studio, shell);
                }
            }
            ui.add_space(4.0);
            ui.separator();
        }
        ui.add_space(6.0);
        color_chips(ui, studio, shell);
    });
}

fn simple_ui(ui: &mut egui::Ui, studio: &mut Studio, shell: &mut Shell, accent: Color32) {
    let active = simple_active(studio);
    ui.vertical(|ui| {
        ui.add_space(4.0);
        for (i, &(tool, shape, glyph)) in SIMPLE_TOOLS.iter().enumerate() {
            let tip = if shape == Some(SelShape::Lasso) { t(Key::HintSelectLasso) } else { tool_tip(tool) };
            if labelled_button(ui, glyph, simple_label(i), active == Some(i), accent).on_hover_text(tip).clicked() {
                if let Some(shape) = shape {
                    studio.opts.select.shape = shape;
                }
                commands::execute(Command::SelectTool(tool), studio, shell);
            }
        }
        // Every other tool is one click away; the slot names the active one
        // when it isn't above (switching modes never changes the tool).
        let more = labelled_button(ui, icon::DOTS_THREE, more_label(studio), active.is_none(), accent);
        egui::Popup::menu(&more).align(egui::RectAlign::RIGHT_START).show(|ui| {
            for group in [TOOLS, EDIT_TOOLS, FRAME_TOOLS, VIEW_TOOLS] {
                for &(tool, glyph) in group {
                    let selected = studio.tool == tool;
                    if ui.selectable_label(selected, format!("{glyph}  {}", tool.label())).clicked() {
                        commands::execute(Command::SelectTool(tool), studio, shell);
                        ui.close();
                    }
                }
                ui.separator();
            }
        });
        ui.add_space(4.0);
        ui.separator();
        ui.add_space(6.0);
        ui.vertical_centered(|ui| color_chips(ui, studio, shell));
    });
}

/// Full-width Simple tool button: icon and text label.
fn labelled_button(ui: &mut egui::Ui, glyph: &str, label: &str, selected: bool, accent: Color32) -> egui::Response {
    let size = Vec2::new(ui.available_width(), theme::SIMPLE_TOOL_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    paint_button(ui, rect, &response, selected, accent);
    let v = ui.visuals();
    let color = if selected { v.strong_text_color() } else { v.text_color() };
    let p = ui.painter().with_clip_rect(rect);
    let icon_x = rect.left() + theme::SIMPLE_TOOL_HEIGHT * 0.5;
    p.text(egui::pos2(icon_x, rect.center().y), egui::Align2::CENTER_CENTER, glyph, egui::FontId::proportional(theme::TOOL_ICON), color);
    let font = egui::TextStyle::Button.resolve(ui.style());
    p.text(egui::pos2(rect.left() + theme::SIMPLE_TOOL_HEIGHT, rect.center().y), egui::Align2::LEFT_CENTER, label, font, color);
    response
}

fn paint_button(ui: &egui::Ui, rect: Rect, response: &egui::Response, selected: bool, accent: Color32) {
    let v = ui.visuals();
    let fill = if selected {
        v.selection.bg_fill
    } else if response.hovered() {
        v.widgets.hovered.bg_fill
    } else {
        Color32::TRANSPARENT
    };
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(5), fill);
    if selected {
        p.rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0, accent), egui::StrokeKind::Inside);
    }
}

fn tool_button(ui: &mut egui::Ui, glyph: &str, selected: bool, accent: Color32) -> egui::Response {
    let size = Vec2::splat(theme::TOOL_BUTTON);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    paint_button(ui, rect, &response, selected, accent);
    let v = ui.visuals();
    let color = if selected { v.strong_text_color() } else { v.text_color() };
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, glyph, egui::FontId::proportional(theme::TOOL_ICON), color);
    response
}

/// Overlapping main/sub color squares; click the sub chip or press X to swap.
fn color_chips(ui: &mut egui::Ui, studio: &mut Studio, shell: &mut Shell) {
    // Within the tool bar, outline included.
    let side = (ui.available_width() - 4.0).clamp(24.0, 36.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
    let chip = (side * 0.61).round();
    let main = Rect::from_min_size(rect.min, Vec2::splat(chip));
    let sub = Rect::from_min_size(rect.min + Vec2::splat(side - chip), Vec2::splat(chip));
    let to32 = |c: [f32; 3]| Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8);
    let border = ui.visuals().weak_text_color();
    let sub_resp = ui.interact(sub, ui.id().with("sub-color"), Sense::click()).on_hover_text(t(Key::ColorSubSwapTip));
    let p = ui.painter();
    for (r, c) in [(sub, studio.color.sub), (main, studio.color.main)] {
        p.rect_filled(r, CornerRadius::same(3), to32(c));
        p.rect_stroke(r, CornerRadius::same(3), Stroke::new(1.5, border), egui::StrokeKind::Outside);
    }
    if sub_resp.clicked() {
        commands::execute(Command::SwapColors, studio, shell);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::UiMode;
    use crate::text::{Lang, lang_for_test};
    use crate::theme::ThemeKind;
    use egui::{Event, Modifiers, RawInput};

    fn setup() -> (Studio, Shell) {
        (Studio::new(arty_core::Document::new(64, 64, 72)), Shell::new(ThemeKind::Dark))
    }

    #[test]
    fn simple_toolbar_lists_the_six_tools() {
        let _lang = lang_for_test(Lang::En);
        let tools: Vec<Tool> = SIMPLE_TOOLS.iter().map(|t| t.0).collect();
        assert_eq!(
            tools,
            [
                Tool::Brush(BrushGroup::Pen),
                Tool::Brush(BrushGroup::Pencil),
                Tool::Brush(BrushGroup::Eraser),
                Tool::Fill,
                Tool::Eyedropper,
                Tool::Select,
            ]
        );
        assert!(SIMPLE_TOOLS[..5].iter().all(|t| t.1.is_none()));
        assert_eq!(SIMPLE_TOOLS[5].1, Some(SelShape::Lasso));
        let labels: Vec<&str> = (0..SIMPLE_TOOLS.len()).map(simple_label).collect();
        assert_eq!(labels, ["Pen", "Pencil", "Eraser", "Fill", "Eyedropper", "Lasso"]);
    }

    /// Switching to Simple keeps a tool it doesn't list; the more slot names it.
    #[test]
    fn other_tools_show_in_the_more_slot() {
        let _lang = lang_for_test(Lang::En);
        let (mut studio, mut shell) = setup();
        commands::execute(Command::SetUiMode(UiMode::Studio), &mut studio, &mut shell);
        commands::execute(Command::SelectTool(Tool::Hand), &mut studio, &mut shell);
        commands::execute(Command::SetUiMode(UiMode::Simple), &mut studio, &mut shell);
        assert_eq!(studio.tool, Tool::Hand);
        assert_eq!((simple_active(&studio), more_label(&studio)), (None, "Hand"));

        studio.opts.select.shape = SelShape::Rect;
        commands::execute(Command::SelectTool(Tool::Select), &mut studio, &mut shell);
        assert_eq!((simple_active(&studio), more_label(&studio)), (None, "Selection"));
        studio.opts.select.shape = SelShape::Lasso;
        assert_eq!((simple_active(&studio), more_label(&studio)), (Some(5), "More"));
        commands::execute(Command::SelectTool(Tool::Brush(BrushGroup::Pen)), &mut studio, &mut shell);
        assert_eq!(simple_active(&studio), Some(0));
    }

    /// WCAG 2.2 target size (24 px) for Simple's buttons, sliders and chips.
    #[test]
    fn simple_targets_are_at_least_24px() {
        const { assert!(theme::TARGET_SIMPLE >= 24.0 && theme::SIMPLE_TOOL_HEIGHT >= theme::TARGET_SIMPLE) };
        assert!(theme::swatch_size(UiMode::Simple) >= 24.0);
        let ctx = egui::Context::default();
        theme::apply(&ctx, ThemeKind::Dark, UiMode::Simple, false);
        assert!(ctx.global_style().spacing.interact_size.y >= 24.0);
        theme::apply(&ctx, ThemeKind::Dark, UiMode::Studio, false);
        assert_eq!(ctx.global_style().spacing.interact_size.y, theme::TARGET_STUDIO);
    }

    /// Simple's tool bar and slider bar draw headless; shortcuts still pick any tool.
    #[test]
    fn simple_bars_run_and_shortcuts_work() {
        let (mut studio, mut shell) = setup();
        assert_eq!(shell.ui_mode, UiMode::Simple);
        let key = |key| Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE };
        let ctx = egui::Context::default();
        for (k, tool) in [(egui::Key::H, Tool::Hand), (egui::Key::P, Tool::Brush(BrushGroup::Pen))] {
            ctx.run_ui(RawInput { events: vec![key(k)], ..Default::default() }, |ui| {
                commands::handle_shortcuts(ui.ctx(), &mut studio, &mut shell);
                super::ui(ui, &mut studio, &mut shell);
                crate::panels::simple_sliders(ui, &mut studio);
            })
            .drop_without_applying_deltas();
            assert_eq!(studio.tool, tool);
        }
    }
}
