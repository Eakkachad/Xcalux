//! Home screen at start-up: three cards that put a ready page on screen
//! in one click (plans/lowend_ux_plan.md §4.2 D2).

use arty_brush::BrushGroup;
use arty_core::frame::add_frame_folder;
use arty_core::page::{MANGA_PRESETS, MangaPreset};
use arty_core::{Document, FrameShape, LayerId, PageSetup, Panel, RectF};
use egui::RichText;

use crate::files::FileController;
use crate::shell::{self, FileRequest, Shell};
use crate::studio::{Studio, Tool};
use crate::text::{Key, t};
use crate::tools::frame::FrameOptions;

/// Resolution of the first manga page (B5 350 dpi is 12 GPU chunks).
pub const MANGA_DPI: u32 = 350;
/// Card width and the gap between cards (points).
const CARD_W: f32 = 300.0;
const CARD_GAP: f32 = 16.0;
/// Height of the card buttons (points).
const BUTTON_H: f32 = 40.0;

/// Panel layouts offered on the First manga page card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelLayout {
    /// A wide establishing panel over two rows of two.
    Classic5,
    /// Four equal stacked panels.
    Yonkoma,
}

impl PanelLayout {
    pub const ALL: [PanelLayout; 2] = [PanelLayout::Classic5, PanelLayout::Yonkoma];

    pub fn label(self) -> &'static str {
        match self {
            PanelLayout::Classic5 => t(Key::HomeLayoutClassic),
            PanelLayout::Yonkoma => t(Key::HomeLayoutYonkoma),
        }
    }

    /// Rows as (height share, width shares right to left).
    fn rows(self) -> &'static [(f32, &'static [f32])] {
        match self {
            PanelLayout::Classic5 => &[(0.3, &[1.0]), (0.35, &[0.6, 0.4]), (0.35, &[0.4, 0.6])],
            PanelLayout::Yonkoma => &[(0.25, &[1.0]), (0.25, &[1.0]), (0.25, &[1.0]), (0.25, &[1.0])],
        }
    }

    /// Panel rects inside `r` in manga reading order (rows top to bottom,
    /// right to left in a row); `gap_h` between rows, `gap_v` between
    /// panels side by side.
    pub fn rects(self, r: RectF, gap_h: f32, gap_v: f32) -> Vec<RectF> {
        let rows = self.rows();
        let free_h = r.h - gap_h * (rows.len() - 1) as f32;
        let mut out = Vec::new();
        let mut y = r.y;
        for &(share_h, cols) in rows {
            let h = free_h * share_h;
            let free_w = r.w - gap_v * (cols.len() - 1) as f32;
            let mut right = r.x + r.w;
            for &share_w in cols {
                let w = free_w * share_w;
                out.push(RectF { x: right - w, y, w, h });
                right -= w + gap_v;
            }
            y += h + gap_h;
        }
        out
    }
}

/// The manuscript preset of the first manga page.
fn b5() -> &'static MangaPreset {
    MANGA_PRESETS.iter().find(|p| p.name == "Doujin B5").expect("B5 manuscript preset")
}

/// A B5 manuscript page at 350 dpi with its page setup, a frame folder of
/// `layout` on the inner frame whose layer, "Ink", is active. Built
/// as the new document's starting state (no undo steps).
pub fn first_manga_page(layout: PanelLayout, frame: &FrameOptions) -> Document {
    let (w, h, page) = PageSetup::from_mm(b5(), MANGA_DPI);
    let mut doc = Document::new(w, h, MANGA_DPI);
    doc.set_page_unrecorded(Some(page));
    let area = if page.inner.w > 0.0 && page.inner.h > 0.0 { page.inner } else { page.safe_rect() };
    let (gap_h, gap_v) = frame.gaps(MANGA_DPI);
    let panels = layout.rects(area, gap_h, gap_v).into_iter().filter_map(Panel::rect).collect();
    let shape = FrameShape { panels, border: frame.border(MANGA_DPI) };
    // Ink is the frame folder's own layer, so the panels clip it (as in CSP).
    if let Some(folder) = add_frame_folder(&mut doc, shape)
        && let Some(&ink) = doc.layer(folder).and_then(|l| l.children()).and_then(|c| c.first())
    {
        rename(&mut doc, ink, t(Key::HomeInkLayer));
        doc.set_active(ink);
    }
    doc
}

fn rename(doc: &mut Document, id: LayerId, name: &str) {
    if let Some(mut props) = doc.layer(id).map(|l| l.props.clone()) {
        props.name = name.to_owned();
        doc.set_props(id, props);
    }
}

/// A blank A4 350 dpi page.
pub fn quick_sketch() -> Document {
    let (w, h, dpi) = shell::DEFAULT_PAGE;
    Document::new(w, h, dpi)
}

/// Whether the home screen opens at start-up: never in bench or demo runs,
/// which keep the plain A4 page.
pub fn show_at_start(wanted: bool, bench: bool, demo: bool) -> bool {
    wanted && !bench && !demo
}

/// Default of the "Show at startup" setting (fresh and older profiles).
pub fn default_show() -> bool {
    true
}

/// The home screen; it closes once the document is replaced or edited.
pub struct Home {
    pub open: bool,
    epoch: u64,
    revision: u64,
}

impl Home {
    /// While open, recovery files are listed on the Open card instead of
    /// the recovery prompt.
    pub fn new(open: bool, studio: &Studio, files: &mut FileController) -> Self {
        files.hold_recovery(open);
        Self { open, epoch: studio.doc_epoch, revision: studio.doc.revision() }
    }

    pub fn close(&mut self, files: &mut FileController) {
        self.open = false;
        files.hold_recovery(false);
    }

    /// Close when File ▸ New, Open, a restore or a menu command changed the
    /// document behind the home screen.
    pub fn sync(&mut self, studio: &Studio, files: &mut FileController) {
        if self.open && (studio.doc_epoch != self.epoch || studio.doc.revision() != self.revision) {
            self.close(files);
        }
    }
}

/// Start the first manga page with the pen.
pub fn start_manga(studio: &mut Studio, layout: PanelLayout) {
    let doc = first_manga_page(layout, &studio.opts.frame);
    studio.replace_document(doc);
    studio.select_tool(Tool::Brush(BrushGroup::Pen));
}

/// Start a quick sketch with the pencil.
pub fn start_sketch(studio: &mut Studio) {
    studio.replace_document(quick_sketch());
    studio.select_tool(Tool::Brush(BrushGroup::Pencil));
}

fn big_button(ui: &mut egui::Ui, text: &str, strong: bool) -> bool {
    let mut label = RichText::new(text).size(18.0);
    if strong {
        label = label.strong();
    }
    ui.add(egui::Button::new(label).min_size(egui::vec2(ui.available_width(), BUTTON_H))).clicked()
}

fn card(ui: &mut egui::Ui, title: Key, body: Key, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::group(ui.style()).inner_margin(14.0).corner_radius(8.0).show(ui, |ui| {
        ui.set_width(CARD_W);
        ui.label(RichText::new(t(title)).size(24.0).strong());
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(t(body)).size(16.0)).wrap());
        ui.add_space(10.0);
        add(ui);
    });
}

/// Draw the home screen in the central area.
pub fn ui(ui: &mut egui::Ui, home: &mut Home, studio: &mut Studio, shell: &mut Shell, files: &mut FileController) {
    // Esc closes a dialog or menu first.
    let modal = files.has_modal() || shell.new_doc_open || egui::Popup::is_any_open(ui.ctx());
    if !modal && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        home.close(files);
        return;
    }
    let mut close = false;
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(24.0);
            ui.label(RichText::new(t(Key::HomeHeading)).size(28.0).strong());
            ui.add_space(16.0);
            let row = ui.available_width() >= 3.0 * (CARD_W + 30.0) + 2.0 * CARD_GAP;
            let mut cards = |ui: &mut egui::Ui| {
                ui.spacing_mut().item_spacing = egui::vec2(CARD_GAP, CARD_GAP);
                card(ui, Key::HomeMangaTitle, Key::HomeMangaBody, |ui| {
                    for layout in PanelLayout::ALL {
                        if big_button(ui, layout.label(), true) {
                            start_manga(studio, layout);
                            close = true;
                        }
                    }
                });
                card(ui, Key::HomeSketchTitle, Key::HomeSketchBody, |ui| {
                    if big_button(ui, t(Key::HomeSketchStart), true) {
                        start_sketch(studio);
                        close = true;
                    }
                });
                card(ui, Key::HomeOpenTitle, Key::HomeOpenBody, |ui| {
                    if big_button(ui, t(Key::CmdOpen), true) {
                        // The home screen closes once the file is open.
                        shell.file_request = Some(FileRequest::Open);
                    }
                    recovery_list(ui, studio, files);
                });
            };
            if row {
                // Centre the row: horizontal layouts start at the left.
                let width = 3.0 * (CARD_W + 30.0) + 2.0 * CARD_GAP;
                ui.allocate_ui_with_layout(
                    egui::vec2(width, 0.0),
                    egui::Layout::left_to_right(egui::Align::Min),
                    cards,
                );
            } else {
                cards(ui);
            }
            ui.add_space(20.0);
            if ui.add(egui::Button::new(RichText::new(t(Key::HomeSkip)).size(16.0)).min_size(egui::vec2(0.0, 32.0))).clicked() {
                close = true;
            }
            ui.add_space(8.0);
            ui.checkbox(&mut shell.home_at_start, RichText::new(t(Key::HomeShowAtStartup)).size(16.0));
            ui.add_space(24.0);
        });
    });
    if close {
        home.close(files);
    }
}

/// Recovery files found at start-up, each with Restore and Discard.
fn recovery_list(ui: &mut egui::Ui, studio: &Studio, files: &mut FileController) {
    if files.found_recovery().is_empty() {
        return;
    }
    ui.add_space(10.0);
    ui.separator();
    ui.label(RichText::new(t(Key::HomeUnsavedWork)).size(16.0).strong());
    let (mut restore, mut discard) = (None, None);
    egui::ScrollArea::vertical().max_height(160.0).id_salt("home-recovery").show(ui, |ui| {
        for (i, e) in files.found_recovery().iter().enumerate() {
            ui.add_space(4.0);
            ui.label(RichText::new(e.display_name()).strong());
            ui.horizontal(|ui| {
                let h = egui::vec2(0.0, 32.0);
                if ui.add(egui::Button::new(RichText::new(t(Key::RecoveryRestore)).strong()).min_size(h)).clicked() {
                    restore = Some(i);
                }
                if ui.add(egui::Button::new(t(Key::RecoveryDiscard)).min_size(h)).clicked() {
                    discard = Some(i);
                }
            });
        }
    });
    if let Some(i) = restore {
        let dirty = files.is_dirty(&studio.doc);
        files.restore_found(i, dirty);
    } else if let Some(i) = discard {
        files.discard_found(i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlaps(a: RectF, b: RectF) -> bool {
        a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
    }

    fn inside(r: RectF, outer: RectF) -> bool {
        const E: f32 = 1e-2;
        r.x >= outer.x - E && r.y >= outer.y - E && r.x + r.w <= outer.x + outer.w + E && r.y + r.h <= outer.y + outer.h + E
    }

    #[test]
    fn first_manga_page_is_a_b5_manuscript_with_panels() {
        let _lang = crate::text::lang_for_test(crate::text::Lang::En);
        let opts = FrameOptions::default();
        for (layout, count) in [(PanelLayout::Classic5, 5), (PanelLayout::Yonkoma, 4)] {
            let doc = first_manga_page(layout, &opts);
            // Doujin B5 at 350 dpi: 182 × 257 mm trim + 3 mm bleed.
            let (w, h, page) = PageSetup::from_mm(b5(), MANGA_DPI);
            assert_eq!((doc.width(), doc.height(), doc.dpi()), (w, h, 350));
            assert_eq!((w, h), (2590, 3623));
            assert_eq!(doc.page_setup(), Some(&page));
            assert!(page.bleed > 0.0 && page.safe > 0.0 && page.inner.w > 0.0);
            assert_eq!(page.trim.w, 2508.0);

            // Layer 1 and the frame folder, whose own layer is Ink and active.
            let root = doc.root();
            assert_eq!(root.len(), 2);
            let folder = root[1];
            let ink = doc.active();
            assert_eq!(doc.layer(ink).unwrap().props.name, "Ink");
            assert!(doc.layer(ink).unwrap().raster().is_some());
            assert_eq!(doc.frame_folder_of(ink), Some(folder), "ink is clipped by the panels");

            let frame = doc.frame(folder).expect("frame folder");
            let panels = &frame.shape().panels;
            assert_eq!(panels.len(), count, "{layout:?}");
            assert_eq!(frame.shape().border, opts.border(MANGA_DPI));
            let rects: Vec<RectF> = panels.iter().map(|p| p.bounds()).collect();
            for (i, &r) in rects.iter().enumerate() {
                assert!(inside(r, page.inner), "{layout:?} panel {i} {r:?} outside {:?}", page.inner);
                assert!(r.w > 100.0 && r.h > 100.0);
                for &o in &rects[i + 1..] {
                    assert!(!overlaps(r, o), "{layout:?}: {r:?} overlaps {o:?}");
                }
            }
        }
    }

    #[test]
    fn layouts_fill_the_area_with_gutters() {
        let r = RectF { x: 10.0, y: 20.0, w: 1000.0, h: 2000.0 };
        let y4 = PanelLayout::Yonkoma.rects(r, 50.0, 20.0);
        assert_eq!(y4.len(), 4);
        assert!(y4.iter().all(|p| p.x == 10.0 && p.w == 1000.0 && (p.h - 462.5).abs() < 1e-3));
        assert!((y4[1].y - (y4[0].y + y4[0].h) - 50.0).abs() < 1e-3);
        assert!((y4[3].y + y4[3].h - 2020.0).abs() < 1e-2);

        let c5 = PanelLayout::Classic5.rects(r, 50.0, 20.0);
        assert_eq!(c5.len(), 5);
        // Reading order: the second row starts at the right.
        assert!(c5[1].x > c5[2].x);
        assert!((c5[1].x + c5[1].w - 1010.0).abs() < 1e-3);
        assert!((c5[1].x - (c5[2].x + c5[2].w) - 20.0).abs() < 1e-3);
        assert!((c5[2].x - 10.0).abs() < 1e-3);
    }

    #[test]
    fn quick_sketch_is_a_blank_a4() {
        let doc = quick_sketch();
        assert_eq!((doc.width(), doc.height(), doc.dpi()), (2894, 4093, 350));
        assert_eq!(doc.layer_count(), 1);
        assert!(doc.page_setup().is_none());
    }

    #[test]
    fn starting_a_card_replaces_the_page_and_picks_the_tool() {
        let _lang = crate::text::lang_for_test(crate::text::Lang::En);
        let mut studio = Studio::new(quick_sketch());
        studio.select_tool(Tool::Hand);
        let epoch = studio.doc_epoch;
        start_manga(&mut studio, PanelLayout::Yonkoma);
        assert_eq!(studio.tool, Tool::Brush(BrushGroup::Pen));
        assert_eq!(studio.doc_epoch, epoch + 1);
        assert!(!studio.history.can_undo(), "a fresh document");
        assert_eq!(studio.doc.layer(studio.doc.active()).unwrap().props.name, "Ink");

        start_sketch(&mut studio);
        assert_eq!(studio.tool, Tool::Brush(BrushGroup::Pencil));
        assert_eq!(studio.doc.width(), 2894);
        assert!(!studio.history.can_undo());
    }

    #[test]
    fn bench_and_demo_runs_skip_the_home_screen() {
        assert!(default_show());
        assert!(show_at_start(true, false, false));
        assert!(!show_at_start(false, false, false), "opted out");
        assert!(!show_at_start(true, true, false), "bench run");
        assert!(!show_at_start(true, false, true), "demo run");
    }
}
