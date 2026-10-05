//! UI-side state shared by panels, menus and dialogs (not part of the
//! document model).

use arty_core::{LayerId, PageSetup, TILE_SIZE, TilePixels};
use arty_render::SyncStats;

use crate::commands::SelModify;
use crate::files::AutosaveSettings;
use crate::theme::ThemeKind;

pub struct NewDocForm {
    pub width: u32,
    pub height: u32,
    pub dpi: u32,
}

/// Page presets for comics and illustration (width, height in px at dpi).
/// The New dialog matches them by size, never by index.
pub const PAGE_PRESETS: &[(&str, u32, u32, u32)] = &[
    ("Manga B4 · 350 dpi", 3541, 5016, 350),
    ("Manga B5 · 350 dpi", 2508, 3541, 350),
    ("A4 · 350 dpi", 2894, 4093, 350),
    ("Manga B4 · 600 dpi", 6071, 8598, 600),
    ("A4 · 600 dpi", 4961, 7016, 600),
    ("Webtoon strip 800 × 12800", 800, 12800, 72),
    ("Illustration 3000 × 4000", 3000, 4000, 350),
    ("Square 2048", 2048, 2048, 144),
];

/// Size of a new document: A4 · 350 dpi.
pub const DEFAULT_PAGE: (u32, u32, u32) = (2894, 4093, 350);

/// The preset of this exact size, if any.
pub fn preset_for(width: u32, height: u32, dpi: u32) -> Option<usize> {
    PAGE_PRESETS.iter().position(|&(_, w, h, d)| (w, h, d) == (width, height, dpi))
}

/// Bytes of one fully painted layer (fix15 RGBA tiles) and of the GPU
/// canvas (RGBA8 chunks with their mips) for a `width` × `height` page.
pub fn page_memory(width: u32, height: u32) -> (u64, u64) {
    use arty_render::gpu::{CHUNK, MAX_PAGE_SIDE, MIP_LEVELS};
    let t = TILE_SIZE as u64;
    let layer = (width as u64).div_ceil(t) * (height as u64).div_ceil(t) * size_of::<TilePixels>() as u64;
    let side = |s: u32| s.clamp(1, MAX_PAGE_SIDE).div_ceil(CHUNK) as u64;
    let chunk: u64 = (0..MIP_LEVELS).map(|k| ((CHUNK >> k) as u64).pow(2) * 4).sum();
    (layer, side(width) * side(height) * chunk)
}

/// Bytes to the nearest MiB.
pub fn mib(bytes: u64) -> u64 {
    (bytes + (1 << 19)) >> 20
}

/// Ten painted layers plus the GPU canvas would take over half of `ram`.
pub fn page_memory_heavy((layer, gpu): (u64, u64), ram: Option<u64>) -> bool {
    ram.is_some_and(|r| layer * 10 + gpu > r / 2)
}

/// New Page dialog strings.
pub mod new_doc_text {
    pub const CUSTOM: &str = "Custom";
    pub const PER_LAYER: &str = "MB / layer";
    pub const GPU: &str = "MB GPU";
    pub const HEAVY: &str = "10 layers would use over half of this computer's memory; it may slow down or freeze.";
}

/// A File menu action waiting for the file controller (it may first ask
/// to save changes, or wait for a stroke or a save to finish).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileRequest {
    New,
    Open,
    Save,
    SaveAs,
}

impl Default for NewDocForm {
    fn default() -> Self {
        let (width, height, dpi) = DEFAULT_PAGE;
        Self { width, height, dpi }
    }
}

pub struct Shell {
    pub theme: ThemeKind,
    pub theme_dirty: bool,
    pub canvas_center_px: [f32; 2],
    pub cursor_doc: Option<[f32; 2]>,
    pub last_sync: SyncStats,
    pub new_doc_open: bool,
    pub new_doc: NewDocForm,
    pub export_requested: bool,
    pub file_request: Option<FileRequest>,
    pub autosave: AutosaveSettings,
    pub quit_requested: bool,
    pub reset_layout_requested: bool,
    /// Layer being renamed, the edit text, and whether the edit box has
    /// already been given focus (requested once, on its first frame).
    pub renaming: Option<(LayerId, String, bool)>,
    pub toast: Option<(String, f64)>,
    /// The Export PNG dialog is open (it picks the crop).
    pub export_dialog: bool,
    /// The Page Setup dialog is open.
    pub page_setup_open: bool,
    /// The Grow / Shrink / Feather dialog is open.
    pub sel_dialog: Option<SelModify>,
    /// Page setup of the manuscript preset picked in the New dialog,
    /// applied to the new document.
    pub new_doc_page: Option<PageSetup>,
}

impl Shell {
    pub fn new(theme: ThemeKind) -> Self {
        Self {
            theme,
            theme_dirty: true,
            canvas_center_px: [0.0; 2],
            cursor_doc: None,
            last_sync: SyncStats::default(),
            new_doc_open: false,
            new_doc: NewDocForm::default(),
            export_requested: false,
            file_request: None,
            autosave: AutosaveSettings::default(),
            quit_requested: false,
            reset_layout_requested: false,
            renaming: None,
            toast: None,
            export_dialog: false,
            page_setup_open: false,
            sel_dialog: None,
            new_doc_page: None,
        }
    }

    pub fn toggle_theme(&mut self) {
        self.theme = match self.theme {
            ThemeKind::Dark => ThemeKind::Light,
            ThemeKind::Light => ThemeKind::Dark,
        };
        self.theme_dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_memory_matches_known_pages() {
        // A4 350 dpi: 46 × 64 tiles, 3 × 4 chunks.
        let (layer, gpu) = page_memory(2894, 4093);
        assert_eq!(layer, 46 * 64 * 32 * 1024);
        assert_eq!((mib(layer), mib(gpu)), (92, 64));
        // B4 600 dpi: 95 × 135 tiles, 6 × 9 chunks.
        let (layer, gpu) = page_memory(6071, 8598);
        assert_eq!((layer, mib(layer), mib(gpu)), (95 * 135 * 32 * 1024, 401, 288));
        assert_eq!(page_memory(64, 64).0, 32 * 1024);
    }

    #[test]
    fn heavy_page_warns_on_small_ram() {
        let b4 = page_memory(6071, 8598);
        assert!(page_memory_heavy(b4, Some(8 << 30)));
        assert!(!page_memory_heavy(b4, Some(16 << 30)));
        assert!(!page_memory_heavy(page_memory(2894, 4093), Some(4 << 30)));
        assert!(!page_memory_heavy(b4, None));
    }

    #[test]
    fn default_page_is_a4_350_and_presets_match_by_size() {
        let f = NewDocForm::default();
        let i = preset_for(f.width, f.height, f.dpi).unwrap();
        assert_eq!(PAGE_PRESETS[i].0, "A4 · 350 dpi");
        assert_eq!(preset_for(6071, 8598, 600).map(|i| PAGE_PRESETS[i].0), Some("Manga B4 · 600 dpi"));
        assert_eq!(preset_for(2894, 4093, 300), None);
        // Color 600 dpi presets come after the 350 dpi ones; names are unique.
        assert_eq!(PAGE_PRESETS[0].3, 350);
        for (i, p) in PAGE_PRESETS.iter().enumerate() {
            assert_eq!(preset_for(p.1, p.2, p.3), Some(i), "{}", p.0);
            assert!(!p.0.contains("monochrome"));
        }
    }

    #[test]
    fn page_texture_layers_avoids_gl_heuristics_for_all_presets_and_edges() {
        use arty_render::gpu::{CHUNK, MAX_PAGE_SIDE, page_texture_layers};
        let triggers_gl_heuristic = |l: u32| l == 1 || l.is_multiple_of(6);
        let max_default_layers = egui_wgpu::wgpu::Limits::default().max_texture_array_layers; // 256

        for &(name, w, h, _dpi) in PAGE_PRESETS {
            let cx = w.div_ceil(CHUNK);
            let cy = h.div_ceil(CHUNK);
            let chunks = cx * cy;
            let layers = page_texture_layers(chunks);
            assert!(
                !triggers_gl_heuristic(layers),
                "preset {name} ({chunks} chunks -> {layers} layers) triggers GL heuristic"
            );
            assert!(
                layers <= max_default_layers,
                "preset {name} layers {layers} exceeds default limit {max_default_layers}"
            );
        }

        for &c in &[1, 5, 6, 7, 12, 54] {
            let l = page_texture_layers(c);
            assert!(!triggers_gl_heuristic(l), "edge count {c} -> {l} triggers GL heuristic");
            assert!(l <= max_default_layers, "edge count {c} -> {l} exceeds {max_default_layers}");
        }

        let max_chunks = (MAX_PAGE_SIDE / CHUNK) * (MAX_PAGE_SIDE / CHUNK);
        let max_layers = page_texture_layers(max_chunks);
        assert_eq!(max_layers, 256);
        assert!(!triggers_gl_heuristic(max_layers));
        assert!(max_layers <= max_default_layers);

        let l2048 = page_texture_layers(2048);
        assert_eq!(l2048, 2048);
        assert!(!triggers_gl_heuristic(l2048));
        assert!(l2048 <= 2048);
    }
}
