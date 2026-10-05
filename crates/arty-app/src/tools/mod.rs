//! Page tools on the canvas: selection, fill, transform, frames and page
//! setup. Each track owns one submodule; the canvas drives them through
//! [`CanvasTool`] (see `canvas.rs` for the dispatch).

pub mod fill;
pub mod frame;
pub mod page;
pub mod select;
pub mod transform;

use serde::{Deserialize, Serialize};

use crate::shell::Shell;
use crate::studio::Studio;

/// One pointer event for a tool.
#[derive(Debug, Clone, Copy)]
pub struct ToolInput {
    /// Physical px.
    pub screen: [f32; 2],
    /// Document px.
    pub doc: [f32; 2],
    pub mods: egui::Modifiers,
    /// The press is the second of a double click.
    pub double: bool,
}

/// What a tool may change while handling input. `origin` (canvas centre)
/// and `ppp` turn document points into egui points as the canvas does.
pub struct ToolCtx<'a> {
    pub studio: &'a mut Studio,
    // No page tool opens a dialog from the canvas yet.
    #[allow(dead_code)]
    pub shell: &'a mut Shell,
    pub origin: [f32; 2],
    pub ppp: f32,
}

/// A canvas tool. Every method defaults to doing nothing.
pub trait CanvasTool {
    fn press(&mut self, _: &mut ToolCtx, _: ToolInput) {}
    /// Once per pointer-move event while pressed, in order.
    fn drag(&mut self, _: &mut ToolCtx, _: ToolInput) {}
    fn release(&mut self, _: &mut ToolCtx, _: ToolInput) {}
    fn hover(&mut self, _: &Studio, _: ToolInput) {}
    /// Enter, Esc or Backspace while [`Self::gesture_active`]; true when used.
    fn key(&mut self, _: &mut ToolCtx, _: egui::Key) -> bool {
        false
    }
    /// Abandon the gesture: Esc (unused by `key`), tool switch, focus loss.
    fn cancel(&mut self, _: &mut ToolCtx) {}
    /// Every frame (idle timers).
    fn tick(&mut self, _: &mut ToolCtx, _now: f64) {}
    /// Overlays, after the page and the selection outline.
    fn paint(&self, _: &Studio, _: &egui::Painter, _origin: [f32; 2], _ppp: f32) {}
    fn cursor(&self, _: &Studio) -> egui::CursorIcon {
        egui::CursorIcon::Crosshair
    }
    /// A drag or an unclosed polygon is in progress: global shortcuts wait.
    fn gesture_active(&self) -> bool {
        false
    }
}

#[derive(Default)]
pub struct ToolStates {
    pub select: select::SelectTool,
    pub fill: fill::FillTool,
    pub transform: transform::TransformTool,
    pub frame: frame::FrameTool,
}

/// Tool options, saved with the app settings.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolOptions {
    pub select: select::SelectOptions,
    pub fill: fill::FillOptions,
    pub transform: transform::TransformOptions,
    pub frame: frame::FrameOptions,
    pub page: page::PageViewOptions,
}

#[cfg(test)]
mod tests {
    use egui::{Key, KeyboardShortcut, Modifiers};

    use crate::commands::{self, Command, SHORTCUTS, shortcut_for};
    use crate::studio::{FrameMode, Tool};

    /// No key fires two commands (a command may have two keys).
    #[test]
    fn shortcuts_are_unique() {
        for (i, (a, ca)) in SHORTCUTS.iter().enumerate() {
            for (b, cb) in &SHORTCUTS[i + 1..] {
                assert!(a != b, "{a:?} is bound to both {ca:?} and {cb:?}");
            }
        }
        let ctrl = Modifiers::COMMAND;
        for (sc, cmd) in [
            (KeyboardShortcut::new(Modifiers::NONE, Key::M), Command::SelectTool(Tool::Select)),
            (KeyboardShortcut::new(Modifiers::NONE, Key::W), Command::SelectTool(Tool::MagicWand)),
            (KeyboardShortcut::new(Modifiers::NONE, Key::G), Command::SelectTool(Tool::Fill)),
            (KeyboardShortcut::new(Modifiers::NONE, Key::K), Command::SelectTool(Tool::Move)),
            (KeyboardShortcut::new(Modifiers::NONE, Key::O), Command::SelectTool(Tool::Frame(FrameMode::Edit))),
            (KeyboardShortcut::new(ctrl, Key::A), Command::SelectAll),
            (KeyboardShortcut::new(ctrl, Key::D), Command::Deselect),
            (KeyboardShortcut::new(ctrl.plus(Modifiers::SHIFT), Key::I), Command::InvertSelection),
            (KeyboardShortcut::new(ctrl, Key::T), Command::Transform),
            (KeyboardShortcut::new(Modifiers::NONE, Key::Enter), Command::CommitTransform),
            (KeyboardShortcut::new(Modifiers::NONE, Key::Escape), Command::CancelTransform),
        ] {
            assert_eq!(shortcut_for(cmd), Some(sc), "{cmd:?}");
        }
        for cmd in [Command::SelectTool(Tool::Frame(FrameMode::Rect)), Command::SelectTool(Tool::Frame(FrameMode::Cut))] {
            assert_eq!(shortcut_for(cmd), None, "{cmd:?} has no default key");
        }
        // egui matches Shift loosely: Ctrl+Shift combos come before plain Ctrl ones.
        let first_plain_ctrl = SHORTCUTS.iter().position(|(s, _)| s.modifiers == ctrl).unwrap();
        let invert = SHORTCUTS.iter().position(|(_, c)| *c == Command::InvertSelection).unwrap();
        assert!(invert < first_plain_ctrl);
    }

    // ----- post-merge integration (spec §10.2) ------------------------------
    // 4 and 5 (files) are in `arty-io/tests/m3_integration.rs`, 6
    // (autosave) in `files.rs`, where the save machinery's helpers are.

    use arty_brush::InputSample;
    use arty_core::frame::add_frame_folder;
    use arty_core::raster::rasterize_polygon;
    use arty_core::transform::transform_mask;
    use arty_core::{BorderStyle, Document, FrameShape, Panel, RectF, Selection, TileCoord, contour, fix15};

    use super::{CanvasTool, ToolCtx, ToolInput};
    use crate::shell::Shell;
    use crate::studio::Studio;
    use crate::theme::ThemeKind;

    fn rect_sel(x: f32, y: f32, w: f32, h: f32, doc: &Document) -> Selection {
        rasterize_polygon(&[[x, y], [x + w, y], [x + w, y + h], [x, y + h]], doc.width(), doc.height(), true)
    }

    fn alpha_at(studio: &Studio, x: i32, y: i32) -> u16 {
        let grid = studio.doc.active_layer().raster().unwrap();
        let c = TileCoord::from_pixel(x, y);
        let (ox, oy) = c.origin();
        grid.get_ref(c).map_or(0, |t| t[(y - oy) as usize][(x - ox) as usize][3])
    }

    /// §10.2 test 1: wand, then a fill limited by its selection; two undos restore the
    /// pixels and then the selection.
    #[test]
    fn m3_wand_then_fill_undoes_in_two_steps() {
        let mut studio = Studio::new(Document::new(256, 256, 72));
        let mut shell = Shell::new(ThemeKind::Dark);
        // Line art: a black wall at x 100..104 on the bottom layer.
        let ink = studio.doc.active();
        let (grid, _) = studio.doc.paint_target(ink).unwrap();
        for y in 0..256 {
            for x in 100..104 {
                let c = TileCoord::from_pixel(x, y);
                let (ox, oy) = c.origin();
                grid.get_mut_or_create(c)[(y - oy) as usize][(x - ox) as usize] = [0, 0, 0, fix15::ONE_U16];
            }
        }
        let colour = studio.doc.add_raster_layer().unwrap();
        studio.doc.set_active(ink);

        // The wand on the ink layer selects the left of the wall.
        studio.select_tool(Tool::MagicWand);
        let mut wand = super::select::SelectTool::default();
        let input = ToolInput { screen: [0.0; 2], doc: [50.5, 50.5], mods: Modifiers::NONE, double: false };
        let mut ctx = ToolCtx { studio: &mut studio, shell: &mut shell, origin: [0.0; 2], ppp: 1.0 };
        wand.press(&mut ctx, input);
        wand.release(&mut ctx, input);
        assert_eq!(studio.history.undo_len(), 1, "the wand is one step");
        let sel = studio.doc.selection().clone();
        assert_eq!((sel.value(50, 50), sel.value(150, 50)), (255, 0));

        // A fill on the empty colour layer covers the whole page, cut to the selection.
        studio.doc.set_active(colour);
        studio.select_tool(Tool::Fill);
        super::fill::FillTool::default().fill_at(&mut studio, (50, 200));
        assert_eq!(studio.history.undo_len(), 2, "the fill is one step");
        assert_eq!(alpha_at(&studio, 10, 10), fix15::ONE_U16);
        assert_eq!(alpha_at(&studio, 150, 10), 0, "outside the selection");
        assert_eq!(alpha_at(&studio, 250, 250), 0);

        studio.undo();
        assert!(studio.doc.active_layer().raster().unwrap().is_empty(), "first undo removes the fill");
        assert!(studio.doc.selection().shares_storage(&sel));
        studio.undo();
        assert!(!studio.doc.has_selection(), "second undo removes the selection");
        assert!(!studio.history.can_undo());
    }

    /// §10.2 test 2: lasso, Ctrl+T translate, commit: the ants go back to the plain
    /// view and outline the moved selection; one undo restores both.
    #[test]
    fn m3_lasso_transform_commit_and_undo() {
        let mut studio = Studio::new(Document::new(256, 256, 72));
        let mut shell = Shell::new(ThemeKind::Dark);
        let id = studio.doc.active();
        let (grid, _) = studio.doc.paint_target(id).unwrap();
        grid.get_mut_or_create(TileCoord::new(0, 0))[20][20] = [0, 0, 0, fix15::ONE_U16];
        let orig = studio.doc.active_layer().raster().unwrap().clone();
        let lasso = [[10.0, 10.0], [40.0, 12.0], [44.0, 40.0], [12.0, 42.0]];
        let sel = rasterize_polygon(&lasso, 256, 256, true);
        studio.set_selection(sel.clone());

        commands::execute(Command::Transform, &mut studio, &mut shell);
        let st = studio.transform.as_mut().expect("a session on the selection");
        let p = arty_core::transform::XfParams { t: [64.0, 0.0], ..st.session.params() };
        st.request(p);
        commands::execute(Command::CommitTransform, &mut studio, &mut shell);
        // Without a session `paint_ants` draws through the plain view.
        assert!(studio.transform.is_none());
        assert_eq!(studio.history.undo_len(), 2);
        assert_eq!(alpha_at(&studio, 84, 20), fix15::ONE_U16, "moved");
        assert_eq!(alpha_at(&studio, 20, 20), 0, "lifted");
        let want = transform_mask(&sel, &p.affine(), 256, 256);
        assert!((0..256).all(|y| (0..256).all(|x| studio.doc.selection().value(x, y) == want.value(x, y))));
        // Re-extracted ants outline the moved selection.
        let minx = |s: &Selection| {
            contour::extract(s, 256, 256).lods[0].iter().flat_map(|p| p.pts.iter()).map(|q| q[0]).fold(f32::MAX, f32::min)
        };
        assert!((minx(studio.doc.selection()) - (minx(&sel) + 64.0)).abs() < 0.5);

        studio.undo();
        assert!(studio.doc.selection().shares_storage(&sel), "undo restores the selection");
        let grid = studio.doc.active_layer().raster().unwrap();
        assert_eq!(grid.iter().count(), orig.iter().count());
        assert!(orig.iter().all(|(c, t)| grid.get_ref(c).is_some_and(|g| std::sync::Arc::ptr_eq(g, t))), "and the pixels");
        studio.redo();
        assert_eq!(alpha_at(&studio, 84, 20), fix15::ONE_U16);
    }

    /// §10.2 test 3: a stroke in a frame folder's raster child: the selection limits
    /// what is written, the panel what is shown.
    #[test]
    fn m3_masked_stroke_in_a_frame_folder() {
        let mut studio = Studio::new(Document::new(256, 256, 72));
        let top = Panel::rect(RectF { x: 0.0, y: 0.0, w: 256.0, h: 128.0 }).unwrap();
        let border = BorderStyle { width: 0.0, color: [0, 0, 0, fix15::ONE_U16] };
        add_frame_folder(&mut studio.doc, FrameShape { panels: vec![top], border }).unwrap();
        let left = rect_sel(0.0, 0.0, 128.0, 256.0, &studio.doc);
        studio.set_selection(left);
        studio.select_tool(Tool::Brush(arty_brush::BrushGroup::Pen));
        // Two strokes crossing all four quadrants.
        for (a, b) in [([8.0, 64.0], [248.0, 64.0]), ([8.0, 192.0], [248.0, 192.0])] {
            let at = |t: f32| InputSample {
                x: a[0] + (b[0] - a[0]) * t,
                y: a[1] + (b[1] - a[1]) * t,
                pressure: 1.0,
                time: f64::from(t),
                ..Default::default()
            };
            assert!(studio.begin_stroke(at(0.0)));
            for i in 1..=64 {
                studio.feed_stroke(at(i as f32 / 64.0));
            }
            studio.end_stroke();
        }
        // Written only inside the selection.
        assert!(alpha_at(&studio, 64, 64) > 0 && alpha_at(&studio, 64, 192) > 0);
        assert_eq!((alpha_at(&studio, 192, 64), alpha_at(&studio, 192, 192)), (0, 0));
        let grid = studio.doc.active_layer().raster().unwrap();
        assert!(grid.coords().all(|c| c.x < 2), "no tile created right of the selection");
        // Shown only inside the panel.
        let paper = studio.sample_color(200.0, 230.0).unwrap();
        assert_ne!(studio.sample_color(64.0, 64.0).unwrap(), paper, "selected, in the panel");
        assert_eq!(studio.sample_color(64.0, 192.0).unwrap(), paper, "selected, outside the panel");
        assert_eq!(studio.sample_color(192.0, 64.0).unwrap(), paper, "in the panel, not selected");
    }

    /// §10.2 test 7: every M3 command can be reached from a menu, the toolbar or (the
    /// Grow / Shrink / Feather values) its modal.
    #[test]
    fn m3_every_new_command_has_a_menu_or_button() {
        let menus = include_str!("../app.rs");
        let toolbar = include_str!("../panels/toolbar.rs");
        let modal = include_str!("select.rs");
        for cmd in [
            "Command::SelectAll",
            "Command::Deselect",
            "Command::InvertSelection",
            "Command::SelectionDialog(",
            "SelModify::Grow",
            "SelModify::Shrink",
            "SelModify::Feather",
            "Command::FillSelection",
            "Command::ToggleReferenceLayer",
            "Command::Transform,",
            "Command::CommitTransform",
            "Command::CancelTransform",
            "Command::FlipTransform { horizontal: true }",
            "Command::FlipTransform { horizontal: false }",
            "Command::RotateTransform90 { cw: true }",
            "Command::RotateTransform90 { cw: false }",
            "Command::PageSetup",
            "Command::TogglePageGuides",
            "Command::ToggleTrimShade",
            "Command::NewFrameFolder",
            "Command::DeletePanel",
        ] {
            assert!(menus.contains(cmd), "{cmd} has no menu entry");
        }
        for tool in [
            "Tool::Select,",
            "Tool::MagicWand,",
            "Tool::Fill,",
            "Tool::Move,",
            "Tool::Frame(FrameMode::Rect)",
            "Tool::Frame(FrameMode::Cut)",
            "Tool::Frame(FrameMode::Edit)",
        ] {
            assert!(toolbar.contains(tool), "{tool} has no toolbar button");
        }
        for cmd in ["Command::GrowSelection {", "Command::ShrinkSelection {", "Command::FeatherSelection {"] {
            assert!(modal.contains(cmd), "{cmd} is not built by the modal");
        }
    }
}
