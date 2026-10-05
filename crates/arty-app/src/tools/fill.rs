//! Fill tool (G), Fill Selection and reference layers. Owned by FILL.

use std::time::Instant;

use arty_core::fill::{self, FillBlend, FillParams, FillRef, FillScratch, ScaleMode};
use arty_core::{LayerId, fix15};
use egui::Slider;
use serde::{Deserialize, Serialize};

use super::{CanvasTool, ToolCtx, ToolInput};
use crate::commands::Command;
use crate::shell::Shell;
use crate::studio::Studio;

/// After a fill this slow, the next one waits a frame behind a wait cursor.
const SLOW_MS: f64 = 100.0;

#[derive(Default)]
pub struct FillTool {
    scratch: FillScratch,
    /// How long the last fill took.
    last_ms: f64,
    /// A click held back one frame so the wait cursor shows first, and
    /// whether that frame has been drawn.
    pending: Option<((i32, i32), bool)>,
}

impl FillTool {
    /// Fill at document pixel `seed` on the active layer with the main
    /// colour: one history step, or a notice when the layer refuses.
    pub fn fill_at(&mut self, studio: &mut Studio, seed: (i32, i32)) {
        if studio.engine.is_stroking() {
            return;
        }
        studio.commit_transform();
        let Some(id) = fill_target(studio) else { return };
        preselect_blend(studio);
        // Behind only adds alpha, which a locked alpha forbids (the core
        // refuses it too): say so instead of filling nothing.
        if studio.opts.fill.blend == FillBlend::Behind && studio.doc.layer(id).is_some_and(|l| l.props.lock_alpha) {
            studio.notice = Some("Layer transparency is locked".into());
            return;
        }
        let o = &studio.opts.fill;
        let (p, opacity, blend) = (o.params(studio.doc.dpi()), o.opacity.clamp(0.0, 1.0), o.blend);
        let color = main_color(studio);
        let start = Instant::now();
        let edit = fill::fill_region(&studio.doc, seed, &p, &mut self.scratch)
            .and_then(|region| fill::apply_fill(&mut studio.doc, id, &region, color, opacity, blend));
        self.last_ms = start.elapsed().as_secs_f64() * 1e3;
        if let Some(edit) = edit {
            studio.record_edit(edit);
        }
    }
}

impl CanvasTool for FillTool {
    fn press(&mut self, ctx: &mut ToolCtx, input: ToolInput) {
        let seed = (input.doc[0].floor() as i32, input.doc[1].floor() as i32);
        if self.last_ms > SLOW_MS {
            self.pending = Some((seed, false));
        } else {
            self.fill_at(ctx.studio, seed);
        }
    }

    fn tick(&mut self, ctx: &mut ToolCtx, _now: f64) {
        match self.pending {
            Some((seed, true)) => {
                self.pending = None;
                self.fill_at(ctx.studio, seed);
            }
            Some((seed, false)) => self.pending = Some((seed, true)),
            None => {}
        }
    }

    fn cancel(&mut self, _: &mut ToolCtx) {
        self.pending = None;
    }

    fn paint(&self, _: &Studio, painter: &egui::Painter, _origin: [f32; 2], _ppp: f32) {
        if self.pending.is_some() {
            painter.ctx().request_repaint();
        }
    }

    fn cursor(&self, _: &Studio) -> egui::CursorIcon {
        if self.pending.is_some() { egui::CursorIcon::Wait } else { egui::CursorIcon::Crosshair }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FillOptions {
    pub reference: FillRef,
    /// Colour margin, 0..=1 of the channel range.
    pub tolerance: f32,
    /// Close gap: 0 (off) ..= 5.
    pub gap_level: u8,
    /// Area scaling, −10..=10 px.
    pub area_scale: i8,
    pub to_darkest: bool,
    pub contiguous: bool,
    pub antialias: bool,
    pub opacity: f32,
    pub blend: FillBlend,
    /// The target the blend was last preselected for: (layer, is reference).
    #[serde(skip)]
    blend_for: Option<(LayerId, bool)>,
}

impl Default for FillOptions {
    fn default() -> Self {
        Self {
            reference: FillRef::Active,
            tolerance: 0.1,
            gap_level: 0,
            area_scale: 0,
            to_darkest: false,
            contiguous: true,
            antialias: true,
            opacity: 1.0,
            blend: FillBlend::Normal,
            blend_for: None,
        }
    }
}

impl FillOptions {
    /// The fill tool's parameters at `dpi` (the selection always limits it).
    pub fn params(&self, dpi: u32) -> FillParams {
        FillParams {
            reference: self.reference,
            tolerance: fix15::from_f32(self.tolerance),
            gap_px: fill::gap_radius(self.gap_level, dpi),
            area_scale: self.area_scale.clamp(-10, 10),
            scale_mode: if self.to_darkest { ScaleMode::ToDarkest } else { ScaleMode::Plain },
            contiguous: self.contiguous,
            antialias: self.antialias,
            use_selection: true,
        }
    }
}

/// The main colour as an opaque fix15 pixel.
fn main_color(studio: &Studio) -> [u16; 4] {
    let [r, g, b] = studio.color.main.map(fix15::from_f32);
    [r, g, b, fix15::ONE_U16]
}

/// The active layer when it can take a fill (not a folder, locked or
/// hidden); otherwise a notice.
fn fill_target(studio: &mut Studio) -> Option<LayerId> {
    let id = studio.doc.active();
    let layer = studio.doc.layer(id)?;
    let refusal = if layer.is_folder() {
        "Select a raster layer to fill"
    } else if layer.props.locked {
        "Layer is locked"
    } else if !layer.props.visible {
        // As the brushes refuse it: the fill would not be seen.
        "Layer is hidden"
    } else {
        return Some(id);
    };
    studio.notice = Some(refusal.into());
    None
}

/// Behind is preselected whenever the fill target is a reference layer
/// (filling under its line art), Normal when the target changes to any
/// other layer.
fn preselect_blend(studio: &mut Studio) {
    let id = studio.doc.active();
    let is_ref = studio.doc.layer(id).is_some_and(|l| l.props.reference);
    let o = &mut studio.opts.fill;
    if o.blend_for != Some((id, is_ref)) {
        o.blend = if is_ref { FillBlend::Behind } else { FillBlend::Normal };
        o.blend_for = Some((id, is_ref));
    }
}

/// FillSelection and ToggleReferenceLayer.
pub fn execute(cmd: Command, studio: &mut Studio, _shell: &mut Shell) {
    if studio.engine.is_stroking() {
        return;
    }
    match cmd {
        Command::FillSelection => {
            studio.commit_transform();
            if !studio.doc.has_selection() {
                studio.notice = Some("Nothing is selected".into());
                return;
            }
            let Some(id) = fill_target(studio) else { return };
            let color = main_color(studio);
            if let Some(edit) = fill::fill_selection(&mut studio.doc, id, color) {
                studio.record_edit(edit);
            }
        }
        Command::ToggleReferenceLayer => {
            let id = studio.doc.active();
            if let Some(layer) = studio.doc.layer(id) {
                let mut p = layer.props.clone();
                p.reference = !p.reference;
                studio.set_layer_props(id, p, false);
            }
        }
        _ => {}
    }
}

/// Tool Property for Fill.
pub fn property_ui(ui: &mut egui::Ui, studio: &mut Studio, _shell: &mut Shell) {
    preselect_blend(studio);
    let o = &mut studio.opts.fill;
    egui::Grid::new("fill-props").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label("Refer to");
        let label = |r: FillRef| match r {
            FillRef::Active => "Editing layer",
            FillRef::AllVisible => "All layers",
            FillRef::Reference => "Reference layers",
        };
        egui::ComboBox::from_id_salt("fill-ref").selected_text(label(o.reference)).show_ui(ui, |ui| {
            for r in [FillRef::Active, FillRef::AllVisible, FillRef::Reference] {
                ui.selectable_value(&mut o.reference, r, label(r));
            }
        });
        ui.end_row();

        ui.label("Tolerance");
        ui.add(percent(&mut o.tolerance));
        ui.end_row();

        ui.label("Close gap");
        let gap = |l: u8| if l == 0 { "Off".to_string() } else { format!("Level {l}") };
        egui::ComboBox::from_id_salt("fill-gap").selected_text(gap(o.gap_level)).show_ui(ui, |ui| {
            for l in 0..=5 {
                ui.selectable_value(&mut o.gap_level, l, gap(l));
            }
        });
        ui.end_row();

        ui.label("Area scaling");
        ui.add(Slider::new(&mut o.area_scale, -10..=10).suffix(" px"));
        ui.end_row();

        ui.label("");
        ui.checkbox(&mut o.to_darkest, "To darkest pixel")
            .on_hover_text("Grow only towards darker pixels, stopping at the core of the line");
        ui.end_row();

        ui.label("Contiguous");
        ui.checkbox(&mut o.contiguous, "").on_hover_text("Off: fill every matching pixel on the page");
        ui.end_row();

        ui.label("Antialiasing");
        ui.checkbox(&mut o.antialias, "");
        ui.end_row();

        ui.label("Opacity");
        ui.add(percent(&mut o.opacity));
        ui.end_row();

        ui.label("Blend");
        let blend = |b: FillBlend| match b {
            FillBlend::Normal => "Normal",
            FillBlend::Behind => "Behind",
        };
        egui::ComboBox::from_id_salt("fill-blend").selected_text(blend(o.blend)).show_ui(ui, |ui| {
            for b in [FillBlend::Normal, FillBlend::Behind] {
                ui.selectable_value(&mut o.blend, b, blend(b));
            }
        });
        ui.end_row();
    });
    if o.reference == FillRef::AllVisible {
        ui.label(
            egui::RichText::new("Filling against all layers composites the page. Marking the line art as a reference layer is faster.")
                .small()
                .weak(),
        );
    }
}

fn percent(v: &mut f32) -> Slider<'_> {
    Slider::new(v, 0.0..=1.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)).custom_parser(|s| {
        s.trim_end_matches('%').trim().parse::<f64>().ok().map(|v| v / 100.0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands;
    use crate::theme::ThemeKind;
    use arty_core::{Document, Selection, TileCoord, selection::full_mask};

    fn input(x: f32, y: f32) -> ToolInput {
        ToolInput { screen: [x, y], doc: [x, y], mods: egui::Modifiers::NONE, double: false }
    }

    fn click(tool: &mut FillTool, studio: &mut Studio, shell: &mut Shell, x: f32, y: f32) {
        let mut ctx = ToolCtx { studio, shell, origin: [0.0; 2], ppp: 1.0 };
        tool.press(&mut ctx, input(x, y));
        tool.release(&mut ctx, input(x, y));
        tool.tick(&mut ctx, 0.0);
        tool.tick(&mut ctx, 0.0);
    }

    fn pixel(studio: &Studio, x: i32, y: i32) -> [u16; 4] {
        let c = TileCoord::from_pixel(x, y);
        let (ox, oy) = c.origin();
        let grid = studio.doc.active_layer().raster().unwrap();
        grid.get(c).map_or([0; 4], |t| t[(y - oy) as usize][(x - ox) as usize])
    }

    #[test]
    fn fl15_fill_click_is_one_step_and_marks_the_layer() {
        let mut studio = Studio::new(Document::new(200, 150, 350));
        let mut shell = Shell::new(ThemeKind::Dark);
        let mut tool = FillTool::default();
        let id = studio.doc.active();
        studio.set_main_color([1.0, 0.0, 0.0]);
        let (steps, epoch, rev) = (studio.history.undo_len(), studio.epochs().pixels(id), studio.doc.revision());
        click(&mut tool, &mut studio, &mut shell, 20.5, 30.5);
        assert_eq!(studio.history.undo_len(), steps + 1, "one step");
        assert!(studio.epochs().pixels(id) > epoch, "the layer's thumbnail epoch moved");
        assert!(studio.doc.revision() != rev);
        assert_eq!(pixel(&studio, 199, 149), [fix15::ONE_U16, 0, 0, fix15::ONE_U16]);
        studio.undo();
        assert!(studio.doc.active_layer().raster().unwrap().is_empty());

        // Off the page: nothing. A locked or folder target: a notice, no step.
        click(&mut tool, &mut studio, &mut shell, -4.0, 10.0);
        assert_eq!(studio.history.undo_len(), steps);
        let mut p = studio.doc.layer(id).unwrap().props.clone();
        p.locked = true;
        studio.doc.set_props(id, p);
        click(&mut tool, &mut studio, &mut shell, 10.0, 10.0);
        assert_eq!(studio.notice.take().as_deref(), Some("Layer is locked"));
        studio.edit_structure(|d| d.add_folder().is_some());
        let steps = studio.history.undo_len();
        click(&mut tool, &mut studio, &mut shell, 10.0, 10.0);
        assert_eq!(studio.notice.take().as_deref(), Some("Select a raster layer to fill"));
        assert_eq!(studio.history.undo_len(), steps);
    }

    /// A hidden layer is refused like the brushes refuse it, for the Fill
    /// tool and Fill Selection alike.
    #[test]
    fn fill_refuses_a_hidden_layer() {
        let mut studio = Studio::new(Document::new(200, 150, 350));
        let mut shell = Shell::new(ThemeKind::Dark);
        let mut tool = FillTool::default();
        let id = studio.doc.active();
        let mut p = studio.doc.layer(id).unwrap().props.clone();
        p.visible = false;
        studio.doc.set_props(id, p);
        let mut sel = Selection::new();
        sel.insert_tile(TileCoord::new(0, 0), full_mask().clone());
        studio.set_selection(sel);
        let (steps, rev) = (studio.history.undo_len(), studio.doc.revision());
        click(&mut tool, &mut studio, &mut shell, 10.0, 10.0);
        assert_eq!(studio.notice.take().as_deref(), Some("Layer is hidden"));
        commands::execute(Command::FillSelection, &mut studio, &mut shell);
        assert_eq!(studio.notice.take().as_deref(), Some("Layer is hidden"));
        assert_eq!((studio.history.undo_len(), studio.doc.revision()), (steps, rev), "no step, no change");
        assert!(studio.doc.active_layer().raster().unwrap().is_empty());
    }

    /// Behind (preselected on a reference layer) on a layer with locked
    /// transparency fills nothing: the click says why.
    #[test]
    fn behind_on_locked_transparency_leaves_a_notice() {
        let mut studio = Studio::new(Document::new(200, 150, 350));
        let mut shell = Shell::new(ThemeKind::Dark);
        let mut tool = FillTool::default();
        let id = studio.doc.active();
        let mut p = studio.doc.layer(id).unwrap().props.clone();
        (p.reference, p.lock_alpha) = (true, true);
        studio.doc.set_props(id, p);
        let (steps, rev) = (studio.history.undo_len(), studio.doc.revision());
        click(&mut tool, &mut studio, &mut shell, 10.0, 10.0);
        assert_eq!(studio.opts.fill.blend, FillBlend::Behind, "preselected for the reference layer");
        assert_eq!(studio.notice.take().as_deref(), Some("Layer transparency is locked"));
        assert_eq!((studio.history.undo_len(), studio.doc.revision()), (steps, rev));
        // Normal recolours what is there, as alpha lock allows.
        studio.opts.fill.blend = FillBlend::Normal;
        click(&mut tool, &mut studio, &mut shell, 10.0, 10.0);
        assert_eq!(studio.notice, None);
    }

    #[test]
    fn fl15_slow_fill_waits_a_frame_behind_the_wait_cursor() {
        let mut studio = Studio::new(Document::new(64, 64, 350));
        let mut shell = Shell::new(ThemeKind::Dark);
        let mut tool = FillTool { last_ms: 500.0, ..Default::default() };
        let mut ctx = ToolCtx { studio: &mut studio, shell: &mut shell, origin: [0.0; 2], ppp: 1.0 };
        tool.press(&mut ctx, input(5.0, 5.0));
        tool.tick(&mut ctx, 0.0);
        assert_eq!(tool.cursor(ctx.studio), egui::CursorIcon::Wait);
        assert_eq!(ctx.studio.history.undo_len(), 0, "drawn first");
        tool.tick(&mut ctx, 0.0);
        assert_eq!(ctx.studio.history.undo_len(), 1);
        assert_eq!(tool.cursor(ctx.studio), egui::CursorIcon::Crosshair);
    }

    #[test]
    fn fl15_fill_respects_the_selection_and_fill_selection_is_one_step() {
        let mut studio = Studio::new(Document::new(200, 150, 350));
        let mut shell = Shell::new(ThemeKind::Dark);
        let mut sel = Selection::new();
        sel.insert_tile(TileCoord::new(1, 1), full_mask().clone());
        studio.set_selection(sel);
        let steps = studio.history.undo_len();
        let mut tool = FillTool::default();
        click(&mut tool, &mut studio, &mut shell, 70.0, 70.0);
        assert_eq!(studio.history.undo_len(), steps + 1);
        assert_eq!(studio.doc.active_layer().raster().unwrap().len(), 1, "only the selected tile");
        click(&mut tool, &mut studio, &mut shell, 10.0, 10.0);
        assert_eq!(studio.history.undo_len(), steps + 1, "a seed outside the selection fills nothing");

        studio.undo();
        commands::execute(Command::FillSelection, &mut studio, &mut shell);
        assert_eq!(studio.history.undo_len(), steps + 1);
        assert_eq!(pixel(&studio, 100, 100)[3], fix15::ONE_U16);
        assert_eq!(pixel(&studio, 10, 10), [0; 4]);
        studio.set_selection(Selection::new());
        let steps = studio.history.undo_len();
        commands::execute(Command::FillSelection, &mut studio, &mut shell);
        assert_eq!(studio.notice.take().as_deref(), Some("Nothing is selected"));
        assert_eq!(studio.history.undo_len(), steps);
    }

    #[test]
    fn fl15_toggle_reference_is_one_props_step_and_preselects_behind() {
        let mut studio = Studio::new(Document::new(64, 64, 350));
        let mut shell = Shell::new(ThemeKind::Dark);
        let id = studio.doc.active();
        preselect_blend(&mut studio);
        assert_eq!(studio.opts.fill.blend, FillBlend::Normal);
        commands::execute(Command::ToggleReferenceLayer, &mut studio, &mut shell);
        assert!(studio.doc.layer(id).unwrap().props.reference);
        assert_eq!(studio.history.undo_len(), 1);
        preselect_blend(&mut studio);
        assert_eq!(studio.opts.fill.blend, FillBlend::Behind, "a reference target preselects Behind");
        studio.opts.fill.blend = FillBlend::Normal;
        preselect_blend(&mut studio);
        assert_eq!(studio.opts.fill.blend, FillBlend::Normal, "the user's choice stands for the same target");
        studio.undo();
        assert!(!studio.doc.layer(id).unwrap().props.reference);
        commands::execute(Command::ToggleReferenceLayer, &mut studio, &mut shell);
        commands::execute(Command::ToggleReferenceLayer, &mut studio, &mut shell);
        assert_eq!(studio.history.undo_len(), 2, "each toggle is its own step");
    }

    #[test]
    fn fl15_options_map_to_params_and_persist() {
        let o = FillOptions { tolerance: 0.5, gap_level: 3, area_scale: 40, to_darkest: true, ..Default::default() };
        let p = o.params(600);
        assert_eq!((p.tolerance, p.gap_px, p.area_scale, p.scale_mode), (fix15::ONE_U16 / 2, 8, 10, ScaleMode::ToDarkest));
        assert!(p.use_selection && p.contiguous && p.antialias);
        let ron = ron_round_trip(&o);
        assert_eq!((ron.tolerance, ron.gap_level, ron.to_darkest), (0.5, 3, true));
    }

    fn ron_round_trip(o: &FillOptions) -> FillOptions {
        let mut storage = crate::studio::MemStorage::default();
        eframe::set_value(&mut storage, "fill", o);
        eframe::get_value(&storage, "fill").unwrap()
    }
}
