//! Page setup: the Page Setup and Export dialogs, the manuscript presets
//! of the New dialog and the page guide overlays. Owned by FRAMES.

use arty_core::page::{MANGA_PRESETS, MM_PER_IN, UNIT_IN, UNIT_MM, UNIT_PX};
use arty_core::{PageSetup, Pt, RectF};
use egui::{Color32, Pos2, RichText, Shape, Stroke};
use serde::{Deserialize, Serialize};

use crate::commands::Command;
use crate::export::{ExportCrop, crop_rect};
use crate::shell::Shell;
use crate::studio::Studio;

const TRIM: Color32 = Color32::from_rgb(76, 141, 255);
const BLEED: Color32 = Color32::from_rgb(232, 72, 72);
const SAFE: Color32 = Color32::from_rgb(40, 176, 116);
const INNER: Color32 = Color32::from_rgb(0, 150, 200);

/// Guide overlays (view state, never exported).
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PageViewOptions {
    pub show_guides: bool,
    pub shade_outside_trim: bool,
}

impl Default for PageViewOptions {
    fn default() -> Self {
        Self { show_guides: true, shade_outside_trim: false }
    }
}

/// PageSetup, TogglePageGuides and ToggleTrimShade.
pub fn execute(cmd: Command, studio: &mut Studio, shell: &mut Shell) {
    match cmd {
        Command::PageSetup => shell.page_setup_open = true,
        Command::TogglePageGuides => studio.opts.page.show_guides = !studio.opts.page.show_guides,
        Command::ToggleTrimShade => studio.opts.page.shade_outside_trim = !studio.opts.page.shade_outside_trim,
        _ => {}
    }
}

fn corners(r: RectF) -> [Pt; 4] {
    [[r.x, r.y], [r.x + r.w, r.y], [r.x + r.w, r.y + r.h], [r.x, r.y + r.h]]
}

/// Trim, bleed, safe and inner-frame guides, drawn right after the page.
pub fn paint_guides(painter: &egui::Painter, studio: &Studio, opts: &PageViewOptions, origin: [f32; 2], ppp: f32) {
    let Some(p) = studio.doc.page_setup() else { return };
    if !opts.show_guides && !opts.shade_outside_trim {
        return;
    }
    let m = studio.view.doc_to_screen(origin);
    let pos = |q: Pt| {
        let s = m.apply(q);
        Pos2::new(s[0] / ppp, s[1] / ppp)
    };
    let (w, h) = (studio.doc.width() as f32, studio.doc.height() as f32);
    if opts.shade_outside_trim {
        let t = p.trim;
        let (r, b) = (t.x + t.w, t.y + t.h);
        let bands = [
            RectF { x: 0.0, y: 0.0, w, h: t.y },
            RectF { x: 0.0, y: b, w, h: h - b },
            RectF { x: 0.0, y: t.y, w: t.x, h: t.h },
            RectF { x: r, y: t.y, w: w - r, h: t.h },
        ];
        for band in bands.into_iter().filter(|r| r.w > 0.0 && r.h > 0.0) {
            painter.add(Shape::convex_polygon(
                corners(band).map(pos).to_vec(),
                Color32::from_black_alpha(80),
                Stroke::NONE,
            ));
        }
    }
    if !opts.show_guides {
        return;
    }
    let closed = |r: RectF| {
        let c = corners(r).map(pos);
        vec![c[0], c[1], c[2], c[3], c[0]]
    };
    if p.bleed > 0.0 {
        painter.extend(Shape::dashed_line(
            &closed(p.bleed_rect(w as u32, h as u32)),
            Stroke::new(1.0, BLEED),
            6.0,
            4.0,
        ));
    }
    let safe = p.safe_rect();
    if p.safe > 0.0 && safe.w > 0.0 && safe.h > 0.0 {
        painter.extend(Shape::dashed_line(&closed(safe), Stroke::new(1.0, SAFE), 3.0, 3.0));
    }
    if p.inner.w > 0.0 && p.inner.h > 0.0 {
        painter.extend(Shape::dotted_line(&closed(p.inner), INNER, 4.0, 0.9));
    }
    painter.add(Shape::line(closed(p.trim), Stroke::new(1.0, TRIM)));
}

// ----- Page Setup dialog --------------------------------------------------------

/// The Page Setup form; lengths in px.
#[derive(Clone, Debug, PartialEq)]
struct PageForm {
    none: bool,
    unit: u8,
    preset: Option<usize>,
    trim: RectF,
    centre: bool,
    bleed: f32,
    safe: f32,
    inner: RectF,
    has_inner: bool,
    inner_centre: bool,
}

impl PageForm {
    fn from_doc(studio: &Studio) -> PageForm {
        let (w, h) = (studio.doc.width() as f32, studio.doc.height() as f32);
        let dpi = studio.doc.dpi() as f32;
        match studio.doc.page_setup() {
            Some(p) => PageForm {
                none: false,
                unit: p.unit,
                preset: None,
                trim: p.trim,
                centre: p.trim.x == ((w - p.trim.w) / 2.0).floor() && p.trim.y == ((h - p.trim.h) / 2.0).floor(),
                bleed: p.bleed,
                safe: p.safe,
                inner: p.inner,
                has_inner: p.inner.w > 0.0,
                inner_centre: p.inner.w <= 0.0 || centred_in(p.inner, p.trim) == p.inner,
            },
            None => {
                // A start that fits: 3 mm bleed inside the canvas.
                let bleed = (3.0 / MM_PER_IN * dpi).round();
                let trim = RectF { x: bleed, y: bleed, w: (w - 2.0 * bleed).max(1.0), h: (h - 2.0 * bleed).max(1.0) };
                PageForm {
                    none: true,
                    unit: UNIT_MM,
                    preset: None,
                    trim,
                    centre: true,
                    bleed,
                    safe: 5.0 / MM_PER_IN * dpi,
                    inner: RectF::default(),
                    has_inner: false,
                    inner_centre: true,
                }
            }
        }
    }

    /// The setup the form describes, before sanitizing.
    fn setup(&self, w: u32, h: u32) -> PageSetup {
        let mut trim = self.trim;
        if self.centre {
            trim.x = ((w as f32 - trim.w) / 2.0).floor();
            trim.y = ((h as f32 - trim.h) / 2.0).floor();
        }
        let inner = match (self.has_inner, self.inner_centre) {
            (false, _) => RectF::default(),
            (true, true) => centred_in(self.inner, trim),
            (true, false) => self.inner,
        };
        PageSetup { trim, bleed: self.bleed, safe: self.safe, inner, unit: self.unit }
    }

    /// Fill the form from a preset at the document's dpi, centred on the canvas.
    fn apply_preset(&mut self, i: usize, dpi: u32) {
        let Some(p) = MANGA_PRESETS.get(i) else { return };
        let (_, _, s) = PageSetup::from_mm(p, dpi);
        self.preset = Some(i);
        self.unit = s.unit;
        self.trim.w = s.trim.w;
        self.trim.h = s.trim.h;
        self.centre = true;
        self.bleed = s.bleed;
        self.safe = s.safe;
        self.has_inner = s.inner.w > 0.0;
        self.inner = s.inner;
        self.inner_centre = true;
    }
}

/// `r`'s size centred in `outer` (whole px).
fn centred_in(r: RectF, outer: RectF) -> RectF {
    RectF { x: outer.x + ((outer.w - r.w) / 2.0).floor(), y: outer.y + ((outer.h - r.h) / 2.0).floor(), w: r.w, h: r.h }
}

fn unit_name(unit: u8) -> &'static str {
    match unit {
        UNIT_IN => "in",
        UNIT_PX => "px",
        _ => "mm",
    }
}

/// A length edited in `unit`, stored in px. True when changed.
fn length(ui: &mut egui::Ui, px: &mut f32, unit: u8, dpi: u32, min_px: f32) -> bool {
    let k = match unit {
        UNIT_MM => MM_PER_IN / dpi as f32,
        UNIT_IN => 1.0 / dpi as f32,
        _ => 1.0,
    };
    let (decimals, speed) = match unit {
        UNIT_MM => (2, 0.1),
        UNIT_IN => (3, 0.005),
        _ => (0, 1.0),
    };
    let mut v = *px * k;
    let r = ui.add(
        egui::DragValue::new(&mut v)
            .range(min_px * k..=100_000.0 * k)
            .speed(speed)
            .max_decimals(decimals)
            .suffix(format!(" {}", unit_name(unit))),
    );
    if r.changed() {
        *px = v / k;
    }
    r.changed()
}

/// The Page Setup modal (`shell.page_setup_open`).
pub fn dialogs(ctx: &egui::Context, studio: &mut Studio, shell: &mut Shell) {
    let key = egui::Id::new("page-setup-form");
    if !shell.page_setup_open {
        ctx.data_mut(|d| d.remove::<PageForm>(key));
        return;
    }
    let mut form = ctx.data(|d| d.get_temp::<PageForm>(key)).unwrap_or_else(|| PageForm::from_doc(studio));
    let (w, h, dpi) = (studio.doc.width(), studio.doc.height(), studio.doc.dpi());
    let mut apply = false;
    let mut cancel = false;
    let modal = egui::Modal::new(egui::Id::new("page-setup")).show(ctx, |ui| {
        ui.set_width(380.0);
        ui.heading("Page Setup");
        ui.weak(format!("Canvas {w} × {h} px at {dpi} dpi"));
        ui.add_space(6.0);
        ui.checkbox(&mut form.none, "No page setup");
        ui.add_enabled_ui(!form.none, |ui| {
            egui::Grid::new("page-setup-grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label("Preset");
                let name = form.preset.and_then(|i| MANGA_PRESETS.get(i)).map_or("Custom", |p| p.name);
                egui::ComboBox::from_id_salt("page-setup-preset").selected_text(name).show_ui(ui, |ui| {
                    for (i, p) in MANGA_PRESETS.iter().enumerate() {
                        if ui.selectable_label(form.preset == Some(i), p.name).clicked() {
                            form.apply_preset(i, dpi);
                        }
                    }
                });
                ui.end_row();
                ui.label("Unit");
                ui.horizontal(|ui| {
                    for u in [UNIT_MM, UNIT_IN, UNIT_PX] {
                        ui.radio_value(&mut form.unit, u, unit_name(u));
                    }
                });
                ui.end_row();
                let unit = form.unit;
                let mut custom = false;
                ui.label("Trim size");
                ui.horizontal(|ui| {
                    custom |= length(ui, &mut form.trim.w, unit, dpi, 1.0);
                    ui.label("×");
                    custom |= length(ui, &mut form.trim.h, unit, dpi, 1.0);
                });
                ui.end_row();
                ui.label("Trim position");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut form.centre, "Centre");
                    if !form.centre {
                        length(ui, &mut form.trim.x, unit, dpi, 0.0);
                        length(ui, &mut form.trim.y, unit, dpi, 0.0);
                    }
                });
                ui.end_row();
                ui.label("Bleed");
                custom |= length(ui, &mut form.bleed, unit, dpi, 0.0);
                ui.end_row();
                ui.label("Safe margin");
                custom |= length(ui, &mut form.safe, unit, dpi, 0.0);
                ui.end_row();
                ui.label("Inner frame");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut form.has_inner, "");
                    if form.has_inner {
                        if form.inner.w <= 0.0 {
                            form.inner = form.trim.expand(-(15.0 / MM_PER_IN * dpi as f32));
                        }
                        custom |= length(ui, &mut form.inner.w, unit, dpi, 1.0);
                        ui.label("×");
                        custom |= length(ui, &mut form.inner.h, unit, dpi, 1.0);
                    }
                });
                ui.end_row();
                if form.has_inner {
                    ui.label("Inner position");
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut form.inner_centre, "Centre on trim");
                        if !form.inner_centre {
                            length(ui, &mut form.inner.x, unit, dpi, 0.0);
                            length(ui, &mut form.inner.y, unit, dpi, 0.0);
                        }
                    });
                    ui.end_row();
                }
                if custom {
                    form.preset = None;
                }
            });
        });
        let valid = form.none || form.setup(w, h).sanitized(w, h).is_some();
        if !valid {
            ui.colored_label(BLEED, "The trim must lie inside the canvas.");
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            apply = ui.add_enabled(valid, egui::Button::new(RichText::new("Apply").strong())).clicked();
            cancel = ui.button("Cancel").clicked();
        });
    });
    if apply {
        let setup = if form.none { None } else { form.setup(w, h).sanitized(w, h) };
        studio.set_page_setup(setup);
    }
    if apply || cancel || modal.should_close() {
        shell.page_setup_open = false;
        ctx.data_mut(|d| d.remove::<PageForm>(key));
    } else {
        ctx.data_mut(|d| d.insert_temp(key, form));
    }
}

// ----- Export dialog ------------------------------------------------------------

/// The Export PNG modal, shown while `shell.export_dialog` is set. Returns
/// the chosen crop once.
pub fn export_dialog(ctx: &egui::Context, studio: &mut Studio, shell: &mut Shell) -> Option<ExportCrop> {
    let key = egui::Id::new("export-crop");
    if !shell.export_dialog {
        ctx.data_mut(|d| d.remove::<ExportCrop>(key));
        return None;
    }
    let doc = &studio.doc;
    let has_page = doc.page_setup().is_some();
    let mut crop = ctx.data(|d| d.get_temp::<ExportCrop>(key)).unwrap_or_else(|| ExportCrop::default_for(doc));
    let (mut export, mut cancel) = (false, false);
    let modal = egui::Modal::new(egui::Id::new("export-dialog")).show(ctx, |ui| {
        ui.set_width(300.0);
        ui.heading("Export PNG");
        ui.add_space(6.0);
        for c in ExportCrop::ALL {
            let (_, _, cw, ch) = crop_rect(doc, c);
            ui.add_enabled_ui(has_page || c == ExportCrop::Canvas, |ui| {
                ui.radio_value(&mut crop, c, format!("{} · {cw} × {ch} px", c.label()));
            });
        }
        if !has_page {
            ui.weak("Bleed and Trim need a page setup (File ▸ Page Setup…).");
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            export = ui.button(RichText::new("Export…").strong()).clicked();
            cancel = ui.button("Cancel").clicked();
        });
    });
    if export || cancel || modal.should_close() {
        shell.export_dialog = false;
        ctx.data_mut(|d| d.remove::<ExportCrop>(key));
    } else {
        ctx.data_mut(|d| d.insert_temp(key, crop));
    }
    export.then_some(crop)
}

// ----- New dialog -----------------------------------------------------------------

/// The "Manga manuscript" presets in the New dialog. Returns the canvas
/// `(width, height, dpi)` of a picked preset and leaves its page setup in
/// `shell.new_doc_page`.
pub fn new_doc_ui(ui: &mut egui::Ui, shell: &mut Shell) -> Option<(u32, u32, u32)> {
    let key = egui::Id::new("new-doc-manga");
    // (preset, dpi) last picked.
    let (mut picked, mut dpi) = ui.data(|d| d.get_temp::<(usize, u32)>(key)).unwrap_or((0, 600));
    if shell.new_doc_page.is_none() {
        picked = usize::MAX;
    }
    // The size was edited after picking: keep the trim centred, or drop the
    // page setup once it no longer fits.
    let (w, h) = (shell.new_doc.width, shell.new_doc.height);
    if let Some(p) = shell.new_doc_page {
        let mut moved = p;
        moved.trim = centred_in(p.trim, RectF { x: 0.0, y: 0.0, w: w as f32, h: h as f32 });
        moved.inner = if p.inner.w > 0.0 { centred_in(p.inner, moved.trim) } else { p.inner };
        shell.new_doc_page = moved.sanitized(w, h);
    }
    let mut out = None;
    ui.horizontal(|ui| {
        ui.label("Manga manuscript");
        let name = MANGA_PRESETS.get(picked).map_or("None", |p| p.name);
        egui::ComboBox::from_id_salt("manga-preset").width(170.0).selected_text(name).show_ui(ui, |ui| {
            for (i, p) in MANGA_PRESETS.iter().enumerate() {
                if ui.selectable_label(picked == i, p.name).clicked() {
                    picked = i;
                    dpi = p.dpi;
                    out = Some(i);
                }
            }
        });
        egui::ComboBox::from_id_salt("manga-dpi").width(80.0).selected_text(format!("{dpi} dpi")).show_ui(ui, |ui| {
            for d in [350, 600, 1200] {
                if ui.selectable_label(dpi == d, format!("{d} dpi")).clicked() && dpi != d {
                    dpi = d;
                    if picked < MANGA_PRESETS.len() {
                        out = Some(picked);
                    }
                }
            }
        });
    });
    ui.data_mut(|d| d.insert_temp(key, (picked, dpi)));
    let (w, h, page) = PageSetup::from_mm(MANGA_PRESETS.get(out?)?, dpi);
    shell.new_doc_page = Some(page);
    Some((w, h, dpi))
}

#[cfg(test)]
mod tests {
    use arty_core::Document;

    use super::*;
    use crate::theme::ThemeKind;

    #[test]
    fn fr12_page_commands_and_form() {
        let mut studio = Studio::new(Document::new(2000, 3000, 600));
        let mut shell = Shell::new(ThemeKind::Dark);
        execute(Command::PageSetup, &mut studio, &mut shell);
        assert!(shell.page_setup_open);
        execute(Command::TogglePageGuides, &mut studio, &mut shell);
        assert!(!studio.opts.page.show_guides);
        execute(Command::ToggleTrimShade, &mut studio, &mut shell);
        assert!(studio.opts.page.shade_outside_trim);

        // A fresh form describes a valid setup with a 3 mm bleed.
        let form = PageForm::from_doc(&studio);
        assert!(form.none);
        let s = form.setup(2000, 3000);
        assert_eq!(s.sanitized(2000, 3000), Some(s));
        assert_eq!(s.bleed, 71.0);
        // A preset centres on the canvas and the inner frame on the trim.
        let mut form = form;
        form.apply_preset(2, 600); // A5
        let s = form.setup(2000, 3000);
        assert_eq!((s.trim.w, s.trim.h), (3496.0, 4961.0));
        assert_eq!(s.sanitized(2000, 3000), None, "an A5 trim does not fit a smaller canvas");
        let s = form.setup(4000, 5200);
        assert_eq!(s.trim.x, ((4000.0 - 3496.0) / 2.0f32).floor());
        assert_eq!(s.inner, centred_in(s.inner, s.trim));
        // An applied setup is one undo step and reopens as it was.
        studio.set_page_setup(Some(PageForm::from_doc(&studio).setup(2000, 3000)));
        assert_eq!(studio.history.undo_len(), 1);
        let back = PageForm::from_doc(&studio);
        assert!(!back.none && back.centre);
        assert_eq!(back.setup(2000, 3000), *studio.doc.page_setup().unwrap());
    }
}
