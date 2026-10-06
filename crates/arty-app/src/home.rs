//! Home screen at start-up: three cards that put a ready page on screen
//! in one click (plans/lowend_ux_plan.md §4.2 D2).

use arty_brush::BrushGroup;
use arty_core::frame::add_frame_folder;
use arty_core::page::{MANGA_PRESETS, MangaPreset};
use arty_core::{Document, FrameShape, LayerId, PageSetup, Panel, RectF};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use egui::{Align2, Color32, ColorImage, CornerRadius, FontId, Rect, RichText, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Vec2};
use egui_phosphor::regular as icon;

use crate::files::FileController;
use crate::machine::Machine;
use crate::recent::{self, Card, Loader, Recent};
use crate::shell::{self, FileRequest, Shell};
use crate::studio::{Studio, Tool};
use crate::text::{Key, t};
use crate::theme::{Palette, ThemeKind};
use crate::tools::frame::FrameOptions;

/// Resolution of the first manga page (B5 350 dpi is 12 GPU chunks).
pub const MANGA_DPI: u32 = 350;
/// Card width range and the gap between cards (points): three cards share
/// the row evenly within the range, or stack when three narrowest ones don't fit.
const CARD_MIN: f32 = 230.0;
const CARD_MAX: f32 = 340.0;
const CARD_GAP: f32 = 16.0;
/// Widest stacked card.
const STACK_MAX: f32 = 520.0;
/// Margin around the home screen content.
const MARGIN: f32 = 24.0;
/// Card inner margin.
const CARD_PAD: i8 = 14;
/// Thumbnail art height and its label row (the whole thumbnail is the button).
const ART_H: f32 = 132.0;
const LABEL_H: f32 = 30.0;

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

/// The weak line under the header: what this machine is and how many layers
/// a first manga page fits on it (D3). `None` when nothing is known.
pub fn machine_label(m: &Machine) -> Option<String> {
    let specs = m.summary();
    if specs.is_empty() {
        return None;
    }
    let (w, h, _) = PageSetup::from_mm(b5(), MANGA_DPI);
    Some(match shell::layer_capacity(shell::page_memory(w, h), m) {
        Some(n) => t(Key::HomeMachine).replace("{specs}", &specs).replace("{n}", &n.to_string()),
        None => t(Key::HomeMachineSpecs).replace("{specs}", &specs),
    })
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
    /// Recent files' cards of this visit, made on its first frame.
    recent: Option<RecentGrid>,
    /// Later was pressed on the recovery bar.
    later: bool,
    /// Shown over a page that is open (Back to Home), not at start-up: leaving returns to it.
    resume: bool,
}

impl Home {
    /// While open, recovery files are listed on the Open card instead of
    /// the recovery prompt.
    pub fn new(open: bool, studio: &Studio, files: &mut FileController) -> Self {
        files.hold_recovery(open);
        Self { open, epoch: studio.doc_epoch, revision: studio.doc.revision(), recent: None, later: false, resume: false }
    }

    /// Back to the home screen over the current document.
    pub fn show(&mut self, studio: &Studio, files: &mut FileController) {
        *self = Self::new(true, studio, files);
        self.resume = true;
    }

    pub fn close(&mut self, files: &mut FileController) {
        self.open = false;
        self.recent = None;
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

/// Card width and whether the cards stack, for `avail` points of width.
fn card_layout(avail: f32) -> (f32, bool) {
    let share = (avail - 2.0 * CARD_GAP) / 3.0;
    if share >= CARD_MIN { (share.min(CARD_MAX), false) } else { (avail.clamp(0.0, STACK_MAX), true) }
}

/// A clickable thumbnail: `art` paints into the upper part, `label` sits below.
fn thumbnail(ui: &mut egui::Ui, pal: &Palette, width: f32, label: &str, art: impl FnOnce(&egui::Painter, Rect)) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, ART_H + LABEL_H), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let (fill, stroke) = if resp.is_pointer_button_down_on() {
        (pal.accent_weak, Stroke::new(1.5, pal.accent))
    } else if resp.hovered() || resp.has_focus() {
        (pal.row_hover, Stroke::new(1.5, pal.accent))
    } else {
        (pal.panel, Stroke::new(1.0, pal.separator))
    };
    let painter = ui.painter_at(rect.expand(2.0));
    painter.rect(rect, CornerRadius::same(6), fill, stroke, StrokeKind::Inside);
    let art_rect = Rect::from_min_max(rect.min + Vec2::new(8.0, 10.0), egui::pos2(rect.max.x - 8.0, rect.max.y - LABEL_H));
    art(&painter, art_rect);
    let font = FontId::proportional(15.0);
    painter.text(egui::pos2(rect.center().x, rect.max.y - LABEL_H * 0.5), Align2::CENTER_CENTER, label, font, pal.text);
    resp
}

/// A white portrait page (B5/A4 shape) centred in `r`, with a soft shadow.
fn page(painter: &egui::Painter, pal: &Palette, r: Rect) -> Rect {
    let h = r.height();
    let w = (h * 0.708).min(r.width());
    let page = Rect::from_center_size(r.center(), Vec2::new(w, w / 0.708));
    painter.rect_filled(page.translate(Vec2::new(2.0, 3.0)), CornerRadius::same(1), pal.page_shadow);
    painter.rect(page, CornerRadius::same(1), Color32::WHITE, Stroke::new(1.0, Color32::from_black_alpha(60)), StrokeKind::Inside);
    page
}

/// A page with `layout`'s panels drawn in ink.
fn layout_art(painter: &egui::Painter, pal: &Palette, r: Rect, layout: PanelLayout) {
    let page = page(painter, pal, r);
    let inner = page.shrink2(Vec2::new(page.width() * 0.11, page.height() * 0.08));
    let area = RectF { x: inner.min.x, y: inner.min.y, w: inner.width(), h: inner.height() };
    let ink = Stroke::new(1.5, Color32::from_gray(40));
    for p in layout.rects(area, page.height() * 0.03, page.width() * 0.04) {
        let r = Rect::from_min_size(egui::pos2(p.x, p.y), Vec2::new(p.w, p.h));
        painter.rect_stroke(r, CornerRadius::ZERO, ink, StrokeKind::Middle);
    }
}

/// A blank page with a pencil on it.
fn sketch_art(painter: &egui::Painter, pal: &Palette, r: Rect) {
    let page = page(painter, pal, r);
    let font = FontId::proportional(page.width() * 0.42);
    painter.text(page.center(), Align2::CENTER_CENTER, icon::PENCIL_SIMPLE_LINE, font, Color32::from_gray(150));
}

fn folder_art(painter: &egui::Painter, pal: &Palette, r: Rect) {
    painter.text(r.center(), Align2::CENTER_CENTER, icon::FOLDER_OPEN, FontId::proportional(r.height() * 0.62), pal.accent);
}

/// One card: title, its thumbnail buttons, a line of description. All cards
/// of a row get the height of the tallest (remembered from the last frame).
fn card(ui: &mut egui::Ui, pal: &Palette, width: f32, min_h: f32, title: Key, body: Key, add: impl FnOnce(&mut egui::Ui, f32)) -> f32 {
    let frame = egui::Frame::new()
        .fill(pal.popup)
        .stroke(Stroke::new(1.0, pal.separator))
        .corner_radius(8)
        .inner_margin(CARD_PAD);
    let inner = width - 2.0 * CARD_PAD as f32;
    frame
        .show(ui, |ui| {
            ui.set_width(inner);
            ui.set_min_height(min_h);
            // The content's own height (the minimum doesn't count).
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(8.0, 4.0);
                ui.label(RichText::new(t(title)).size(20.0).strong());
                ui.add_space(6.0);
                add(ui, inner);
                ui.add_space(6.0);
                ui.add(egui::Label::new(RichText::new(t(body)).size(14.0).color(pal.text_weak)).wrap());
            })
            .response
            .rect
            .height()
        })
        .inner
}

/// Recent file tiles: width range and gap (points), the thumbnail's height
/// per width, and the room under it for the name and the time.
const TILE_MIN: f32 = 150.0;
const TILE_MAX: f32 = 200.0;
const TILE_GAP: f32 = 12.0;
const TILE_ART_PER_W: f32 = 1.08;
const TILE_TEXT_H: f32 = 52.0;

/// Where one recent file's card stands.
enum CardState {
    Loading,
    Ready { tex: Option<TextureHandle>, modified: Option<SystemTime> },
}

struct RecentCard {
    path: PathBuf,
    state: CardState,
}

/// The cards of one visit to Home: each file is read once, on a thread.
struct RecentGrid {
    loader: Loader,
    cards: Vec<RecentCard>,
    started: Instant,
    pending: usize,
}

impl RecentGrid {
    fn start(ctx: &egui::Context, recent: &Recent) -> Self {
        let paths = recent.paths().to_vec();
        let wake = ctx.clone();
        Self {
            started: Instant::now(),
            pending: paths.len(),
            cards: paths.iter().map(|p| RecentCard { path: p.clone(), state: CardState::Loading }).collect(),
            loader: Loader::spawn(paths, move || wake.request_repaint()),
        }
    }

    /// Take the cards the thread has read; files that are gone leave the list.
    fn poll(&mut self, ctx: &egui::Context, recent: &mut Recent) {
        let before = self.pending;
        while let Some((path, card)) = self.loader.try_recv() {
            self.pending = self.pending.saturating_sub(1);
            match card {
                Card::Missing => {
                    recent.remove(&path);
                    self.cards.retain(|c| c.path != path);
                }
                Card::Ready { thumb, modified } => {
                    let Some(c) = self.cards.iter_mut().find(|c| c.path == path) else { continue };
                    let tex = thumb.map(|(w, h, px)| {
                        let image = ColorImage::from_rgba_unmultiplied([w.into(), h.into()], &px);
                        ctx.load_texture(format!("recent:{}", path.display()), image, TextureOptions::LINEAR)
                    });
                    c.state = CardState::Ready { tex, modified };
                }
            }
        }
        if before > 0 && self.pending == 0 && crate::bench::active() {
            let ms = self.started.elapsed().as_secs_f64() * 1000.0;
            eprintln!("ARTY_BENCH home_recent · {} cards · all thumbnails read {ms:.0} ms after the first Home frame", self.cards.len());
        }
    }
}

/// Columns and tile width for `avail` points.
fn tile_layout(avail: f32) -> (usize, f32) {
    let cols = (((avail + TILE_GAP) / (TILE_MIN + TILE_GAP)).floor() as usize).max(1);
    let w = (avail - (cols - 1) as f32 * TILE_GAP) / cols as f32;
    (cols, w.min(TILE_MAX))
}

/// What a click on a tile asks for.
enum TileAction {
    Open,
    ShowInFolder,
    Remove,
}

/// One text line, cut with "…" at `width`.
fn one_line(painter: &egui::Painter, text: &str, size: f32, color: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(text.to_owned(), FontId::proportional(size), color, width);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    painter.layout_job(job)
}

/// A recent file: its thumbnail, name and time. A click and the right-click menu come back as an action.
fn recent_tile(ui: &mut egui::Ui, pal: &Palette, w: f32, card: &RecentCard) -> Option<TileAction> {
    let art_h = (w * TILE_ART_PER_W).round();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, art_h + TILE_TEXT_H), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let (fill, stroke) = if resp.is_pointer_button_down_on() {
        (pal.accent_weak, Stroke::new(1.5, pal.accent))
    } else if resp.hovered() || resp.has_focus() {
        (pal.row_hover, Stroke::new(1.5, pal.accent))
    } else {
        (pal.panel, Stroke::new(1.0, pal.separator))
    };
    let painter = ui.painter_at(rect.expand(2.0));
    painter.rect(rect, CornerRadius::same(6), fill, stroke, StrokeKind::Inside);
    let art = Rect::from_min_max(rect.min + Vec2::new(8.0, 8.0), egui::pos2(rect.max.x - 8.0, rect.min.y + art_h - 4.0));
    let (tex, modified) = match &card.state {
        CardState::Ready { tex, modified } => (tex.as_ref(), *modified),
        CardState::Loading => (None, None),
    };
    match tex {
        Some(tex) => {
            // The page keeps its shape inside the art box.
            let size = tex.size_vec2();
            let r = Rect::from_center_size(art.center(), size * (art.width() / size.x).min(art.height() / size.y));
            painter.rect_filled(r.translate(Vec2::new(2.0, 3.0)), CornerRadius::same(1), pal.page_shadow);
            painter.image(tex.id(), r, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
            painter.rect_stroke(r, CornerRadius::same(1), Stroke::new(1.0, Color32::from_black_alpha(60)), StrokeKind::Inside);
        }
        None => {
            let page = page(&painter, pal, art);
            let font = FontId::proportional(page.width() * 0.4);
            painter.text(page.center(), Align2::CENTER_CENTER, icon::FILE_IMAGE, font, Color32::from_gray(190));
        }
    }
    let inner = w - 16.0;
    let name = card.path.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let name = one_line(&painter, &name, 15.0, pal.text, inner);
    let below = rect.min.y + art_h;
    let name_h = name.size().y;
    painter.galley(egui::pos2(rect.min.x + 8.0, below), name, pal.text);
    if let Some(m) = modified {
        let age = SystemTime::now().duration_since(m).map_or(0, |d| d.as_secs());
        let when = one_line(&painter, &recent::edited(age), 13.0, pal.text_weak, inner);
        painter.galley(egui::pos2(rect.min.x + 8.0, below + name_h), when, pal.text_weak);
    }
    let mut action = resp.clicked().then_some(TileAction::Open);
    resp.on_hover_text(card.path.display().to_string()).context_menu(|ui| {
        if ui.button(t(Key::HomeOpenFolder)).clicked() {
            action = Some(TileAction::ShowInFolder);
            ui.close();
        }
        if ui.button(t(Key::HomeRemoveRecent)).clicked() {
            action = Some(TileAction::Remove);
            ui.close();
        }
    });
    action
}

/// Show `path` selected in an Explorer window.
fn show_in_folder(path: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Explorer wants the path quoted by hand.
        let arg = format!("/select,\"{}\"", path.display());
        if let Err(e) = std::process::Command::new("explorer").raw_arg(arg).spawn() {
            log::warn!("explorer: {e}");
        }
    }
    #[cfg(not(windows))]
    log::info!("show in folder: {}", path.display());
}

/// The "Recent" heading and its grid, or a friendly line when there is nothing.
fn recent_section(ui: &mut egui::Ui, pal: &Palette, width: f32, home: &mut Home, shell: &mut Shell) {
    ui.label(RichText::new(t(Key::HomeRecentTitle)).size(20.0).strong());
    ui.add_space(6.0);
    let ctx = ui.ctx().clone();
    if shell.recent.paths().is_empty() {
        ui.add(egui::Label::new(RichText::new(t(Key::HomeRecentEmpty)).size(15.0).color(pal.text_weak)).wrap());
        return;
    }
    let grid = home.recent.get_or_insert_with(|| RecentGrid::start(&ctx, &shell.recent));
    grid.poll(&ctx, &mut shell.recent);
    let (cols, w) = tile_layout(width);
    let mut act = None;
    ui.spacing_mut().item_spacing = Vec2::splat(TILE_GAP);
    for row in grid.cards.chunks(cols) {
        ui.horizontal(|ui| {
            for card in row {
                if let Some(a) = recent_tile(ui, pal, w, card) {
                    act = Some((card.path.clone(), a));
                }
            }
        });
    }
    match act {
        Some((path, TileAction::Open)) => {
            shell.open_path = Some(path);
            shell.file_request = Some(FileRequest::OpenPath);
        }
        Some((path, TileAction::ShowInFolder)) => show_in_folder(&path),
        Some((path, TileAction::Remove)) => {
            shell.recent.remove(&path);
            grid.cards.retain(|c| c.path != path);
        }
        None => {}
    }
}

/// The bar above the cards: unsaved work from the last run, and what to do with it.
fn recovery_bar(ui: &mut egui::Ui, pal: &Palette, width: f32, home: &mut Home, studio: &Studio, files: &mut FileController) {
    let n = files.found_recovery().len();
    if n == 0 || home.later {
        return;
    }
    let pad = 10.0;
    egui::Frame::new()
        .fill(pal.accent_weak)
        .stroke(Stroke::new(1.0, pal.accent))
        .corner_radius(8)
        .inner_margin(pad)
        .show(ui, |ui| {
            ui.set_width(width - 2.0 * pad - 2.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(10.0, 6.0);
                let text = t(Key::HomeRecoverBar).replace("{n}", &n.to_string());
                ui.add(egui::Label::new(RichText::new(text).size(15.0).color(pal.text)).wrap());
                let h = Vec2::new(0.0, 32.0);
                if ui.add(egui::Button::new(RichText::new(t(Key::HomeRecoverNow)).strong()).min_size(h)).clicked() {
                    // The newest; the others stay listed on the Open card.
                    let dirty = files.is_dirty(&studio.doc);
                    files.restore_found(0, dirty);
                }
                if ui.add(egui::Button::new(t(Key::HomeRecoverLater)).min_size(h)).clicked() {
                    home.later = true;
                }
            });
        });
    ui.add_space(12.0);
}

/// Draw the home screen in the central area.
pub fn ui(ui: &mut egui::Ui, home: &mut Home, studio: &mut Studio, shell: &mut Shell, files: &mut FileController) {
    // Esc closes a dialog or menu first.
    let modal = files.has_modal() || shell.new_doc_open || egui::Popup::is_any_open(ui.ctx());
    if !modal && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        home.close(files);
        return;
    }
    let pal = shell.theme.palette();
    let mut close = false;
    // Heights measured last frame: the tallest card, and the whole content (to centre it).
    let card_h_id = egui::Id::new("home-card-h");
    let content_h_id = egui::Id::new("home-content-h");
    let (card_h, content_h) =
        ui.data(|d| (d.get_temp::<f32>(card_h_id).unwrap_or(0.0), d.get_temp::<f32>(content_h_id).unwrap_or(0.0)));
    let mut tallest: f32 = 0.0;
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        let full = ui.available_width();
        let (card_w, stacked) = card_layout(full - 2.0 * MARGIN);
        let content_w = if stacked { card_w } else { 3.0 * card_w + 2.0 * CARD_GAP };
        let top = ((ui.available_height() - content_h) * 0.4).max(MARGIN);
        ui.add_space(top);
        let start = ui.cursor().top();
        ui.horizontal(|ui| {
            ui.add_space(((full - content_w) * 0.5).max(0.0));
            ui.vertical(|ui| {
                ui.set_width(content_w);
                // Header: the question, then language and theme on the right.
                ui.horizontal(|ui| {
                    ui.add(egui::Label::new(RichText::new(t(Key::HomeHeading)).size(26.0).strong()).wrap());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let theme_icon = if shell.theme == ThemeKind::Dark { icon::SUN } else { icon::MOON };
                        let b = egui::Button::new(RichText::new(theme_icon).size(16.0));
                        if ui.add(b).on_hover_text(t(Key::CmdToggleTheme)).clicked() {
                            shell.toggle_theme();
                        }
                        crate::panels::lang_switch(ui, shell);
                    });
                });
                if let Some(label) = machine_label(&shell.machine) {
                    ui.add_space(2.0);
                    ui.add(egui::Label::new(RichText::new(label).size(14.0).color(pal.text_weak)).wrap());
                }
                ui.add_space(14.0);
                recovery_bar(ui, &pal, content_w, home, studio, files);
                let min_h = if stacked { 0.0 } else { card_h };
                let cards = |ui: &mut egui::Ui| {
                    ui.spacing_mut().item_spacing = Vec2::splat(CARD_GAP);
                    let h = card(ui, &pal, card_w, min_h, Key::HomeMangaTitle, Key::HomeMangaBody, |ui, w| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            let tw = (w - 10.0) * 0.5;
                            for layout in PanelLayout::ALL {
                                if thumbnail(ui, &pal, tw, layout.label(), |p, r| layout_art(p, &pal, r, layout)).clicked() {
                                    start_manga(studio, layout);
                                    close = true;
                                }
                            }
                        });
                    });
                    tallest = tallest.max(h);
                    let h = card(ui, &pal, card_w, min_h, Key::HomeSketchTitle, Key::HomeSketchBody, |ui, w| {
                        if thumbnail(ui, &pal, w, t(Key::HomeSketchStart), |p, r| sketch_art(p, &pal, r)).clicked() {
                            start_sketch(studio);
                            close = true;
                        }
                    });
                    tallest = tallest.max(h);
                    let h = card(ui, &pal, card_w, min_h, Key::HomeOpenTitle, Key::HomeOpenBody, |ui, w| {
                        if thumbnail(ui, &pal, w, t(Key::CmdOpen), |p, r| folder_art(p, &pal, r)).clicked() {
                            // The home screen closes once the file is open.
                            shell.file_request = Some(FileRequest::Open);
                        }
                        recovery_list(ui, studio, files);
                    });
                    tallest = tallest.max(h);
                };
                if stacked {
                    ui.vertical(cards);
                } else {
                    ui.horizontal_top(cards);
                }
                ui.add_space(20.0);
                recent_section(ui, &pal, content_w, home, shell);
                ui.add_space(20.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 16.0;
                    let skip = egui::Button::new(RichText::new(t(if home.resume { Key::HomeBack } else { Key::HomeSkip })).size(15.0)).min_size(Vec2::new(0.0, 32.0));
                    if ui.add(skip).clicked() {
                        close = true;
                    }
                    ui.checkbox(&mut shell.home_at_start, RichText::new(t(Key::HomeShowAtStartup)).size(15.0));
                });
            });
        });
        let h = ui.cursor().top() - start;
        ui.add_space(MARGIN);
        ui.data_mut(|d| {
            d.insert_temp(card_h_id, tallest);
            d.insert_temp(content_h_id, h);
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
    fn machine_label_counts_b5_layers() {
        use crate::machine::{GpuKind, GpuSummary, Storage, Vendor};
        let _lang = crate::text::lang_for_test(crate::text::Lang::En);
        let gpu = GpuSummary { name: "UHD".into(), vendor: Vendor::Intel, kind: GpuKind::Integrated, backend: "Vulkan" };
        let mut m = Machine { ram: Some(8 << 30), physical_cores: 4, gpu: Some(gpu), storage: Storage::Ssd };
        // B5 350 dpi: layer 73.0 MiB, GPU canvas 64.0 MiB; (4096 - 450 - 64 - 512) / 73.0 = 42.0.
        assert_eq!(
            machine_label(&m).as_deref(),
            Some("This machine: RAM 8 GB · Intel graphics · SSD — a B5 manga page fits about 42 layers")
        );
        crate::text::set_current_lang(crate::text::Lang::Th);
        assert_eq!(
            machine_label(&m).as_deref(),
            Some("เครื่องนี้: RAM 8 GB · การ์ดจอ Intel · SSD — หน้ามังงะ B5 ได้ประมาณ 42 เลเยอร์")
        );
        // Unknown RAM: the specs alone; nothing known: no line.
        crate::text::set_current_lang(crate::text::Lang::En);
        m.ram = None;
        assert_eq!(machine_label(&m).as_deref(), Some("This machine: Intel graphics · SSD"));
        m.gpu = None;
        m.storage = Storage::Unknown;
        assert_eq!(machine_label(&m), None);
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
    fn cards_share_the_row_or_stack() {
        // Three equal cards, never wider than the range nor the window.
        for avail in [700.0, 863.0, 1232.0, 1318.0, 1856.0] {
            let (w, stacked) = card_layout(avail);
            let row = if stacked { w } else { 3.0 * w + 2.0 * CARD_GAP };
            assert!(row <= avail + 1e-3, "{avail}: {row}");
            assert!(stacked || (CARD_MIN..=CARD_MAX).contains(&w), "{avail}: {w}");
        }
        assert!(!card_layout(1318.0).1, "1366 px at 100 %");
        assert!(!card_layout(1232.0).1, "1920 px at 150 %");
        assert_eq!(card_layout(1856.0), (CARD_MAX, false));
        assert_eq!(card_layout(700.0), (STACK_MAX, true));
        assert_eq!(card_layout(400.0), (400.0, true));
    }

    /// Writes sample pages for Home screenshots: `ARTY_SAMPLES_DIR=<dir> cargo test -p arty-app
    /// make_home_samples -- --ignored`. Prints the `;`-joined list for `ARTY_BENCH_RECENT`.
    #[test]
    #[ignore = "writes files; run by hand"]
    fn make_home_samples() {
        use arty_io::{Progress, SaveExtras, SaveOptions, Session, SessionId};
        use std::time::Duration;

        let Some(dir) = std::env::var_os("ARTY_SAMPLES_DIR").map(PathBuf::from) else { return };
        std::fs::create_dir_all(&dir).unwrap();
        let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
        let names = [
            "Chapter 1 - page 03",
            "ตอนที่ 2 หน้า 5",
            "sketch cat",
            "Yonkoma - morning",
            "A very long project name that cannot possibly fit in one tile",
            "page 12",
            "ฉากต่อสู้",
            "cover draft",
            "page 07",
            "ปกเล่ม 1",
            "gag strip",
            "old page",
        ];
        // Ages: now, 5 min, 2 h, 1 d, 3 d, 10 d, 40 d, 400 d ...
        let ages: [u64; 12] = [10, 300, 7_300, 90_000, 260_000, 900_000, 3_500_000, 34_600_000, 40_000, 5_000_000, 600, 70_000_000];
        let mut list = Vec::new();
        for (i, name) in names.iter().enumerate() {
            let mut studio = Studio::new(quick_sketch());
            match i % 3 {
                0 => start_manga(&mut studio, PanelLayout::Classic5),
                1 => start_manga(&mut studio, PanelLayout::Yonkoma),
                _ => start_sketch(&mut studio),
            }
            let (w, h) = (studio.doc.width() as f32, studio.doc.height() as f32);
            studio.preset_mut().size = 40.0;
            let colors = [[0.05, 0.05, 0.08], [0.85, 0.25, 0.3], [0.2, 0.45, 0.85]];
            for s in 0..6 + i % 4 {
                studio.set_main_color(colors[(s + i) % 3]);
                let (x0, y0) = (w * (0.15 + 0.1 * ((s * 7 + i * 3) % 7) as f32), h * (0.1 + 0.12 * ((s * 5 + i) % 7) as f32));
                for k in 0..=60 {
                    let t = k as f32 / 60.0;
                    let a = t * std::f32::consts::TAU * (1.0 + (i % 3) as f32 * 0.5);
                    let sample = arty_brush::InputSample {
                        x: x0 + w * 0.25 * t + w * 0.05 * a.cos(),
                        y: y0 + h * 0.12 * t + h * 0.04 * a.sin(),
                        pressure: (t * std::f32::consts::PI).sin().max(0.2),
                        time: k as f64 * 0.006,
                        ..Default::default()
                    };
                    if k == 0 {
                        studio.begin_stroke(sample);
                    } else {
                        studio.feed_stroke(sample);
                    }
                }
                studio.end_stroke();
            }
            let path = dir.join(format!("{name}.arty"));
            let ex = SaveExtras { thumb: arty_io::thumb::make(&studio.doc, &pool), title: (*name).into(), ..Default::default() };
            Session::new(SessionId([i as u8; 16]), None)
                .save_main(&studio.doc, &ex, &path, false, &SaveOptions::default(), &pool, &Progress::default())
                .unwrap();
            let when = std::time::SystemTime::now() - Duration::from_secs(ages[i]);
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(when).unwrap();
            list.push(path.display().to_string());
        }
        println!("ARTY_BENCH_RECENT={}", list.join("|"));
    }

    #[test]
    fn home_over_a_page_resumes_it() {
        let studio = Studio::new(quick_sketch());
        let mut files = FileController::new(Err(arty_io::IoError::Cancelled), Box::new(crate::files::NativeDialogs), &studio);
        let mut home = Home::new(true, &studio, &mut files);
        assert!(!home.resume, "start-up");
        home.close(&mut files);
        home.show(&studio, &mut files);
        assert!(home.open && home.resume && home.recent.is_none() && !home.later);
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
