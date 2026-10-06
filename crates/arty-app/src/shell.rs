//! UI-side state shared by panels, menus and dialogs (not part of the
//! document model).

use arty_core::{LayerId, PageSetup, TILE_SIZE, TilePixels};
use arty_render::SyncStats;
use serde::{Deserialize, Serialize};

use crate::commands::SelModify;
use crate::files::AutosaveSettings;
use crate::machine::{Machine, Tier};
use crate::text::{self, Key, Lang, t};
use crate::theme::ThemeKind;

/// Simple: few tools with labels, one panel column, big targets (a fresh
/// profile starts here). Studio: every panel, Clip Studio-like.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum UiMode {
    #[default]
    Simple,
    Studio,
}

impl UiMode {
    pub const ALL: [UiMode; 2] = [UiMode::Simple, UiMode::Studio];

    /// Mode of a profile saved before modes existed: its users know Studio.
    pub fn existing_profile() -> UiMode {
        UiMode::Studio
    }

    pub fn label(self) -> &'static str {
        match self {
            UiMode::Simple => t(Key::ModeSimple),
            UiMode::Studio => t(Key::ModeStudio),
        }
    }
}

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

/// Private bytes of the app with a blank page open (plans/bench/B013).
pub const BASELINE: u64 = 450 << 20;

/// Full layers a page fits on `m`, as Procreate counts them from RAM and
/// canvas size: half of RAM (the load budget, E6) less the app's baseline,
/// the GPU canvas where graphics memory is system RAM, and the undo budget,
/// divided by one layer. `None` when the RAM is unknown.
pub fn layer_capacity((layer, gpu): (u64, u64), m: &Machine) -> Option<u32> {
    let ram = m.ram?;
    let shared = if m.gpu_shares_ram() { gpu } else { 0 };
    let used = BASELINE + shared + arty_core::undo_budget(m.ram) as u64;
    let free = (ram / 2).saturating_sub(used);
    Some((free / layer.max(1)).min(u32::MAX as u64) as u32)
}

/// How comfortably a page fits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    /// 20 layers or more.
    Roomy,
    /// 8 to 19.
    Tight,
    /// Fewer than 8.
    Heavy,
}

impl Fit {
    pub fn of(layers: u32) -> Fit {
        match layers {
            20.. => Fit::Roomy,
            8..=19 => Fit::Tight,
            _ => Fit::Heavy,
        }
    }
}

/// Layers that fit and how well, for a `width` × `height` page.
pub fn page_fit(width: u32, height: u32, m: &Machine) -> Option<(u32, Fit)> {
    layer_capacity(page_memory(width, height), m).map(|n| (n, Fit::of(n)))
}

/// Performance setting: Auto lightens on a Low tier machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PerfMode {
    #[default]
    Auto,
    Light,
    Full,
}

impl PerfMode {
    /// Whether Light mode is on for a machine of `tier`.
    pub fn light(self, tier: Tier) -> bool {
        match self {
            PerfMode::Auto => tier == Tier::Low,
            PerfMode::Light => true,
            PerfMode::Full => false,
        }
    }
}

/// The localized name of a preset at `index`.
pub fn preset_name(index: usize) -> &'static str {
    match index {
        0 => t(Key::PresetMangaB4_350),
        1 => t(Key::PresetMangaB5_350),
        2 => t(Key::PresetA4_350),
        3 => t(Key::PresetMangaB4_600),
        4 => t(Key::PresetA4_600),
        5 => t(Key::PresetWebtoon),
        6 => t(Key::PresetIllustration),
        7 => t(Key::PresetSquare),
        _ => PAGE_PRESETS.get(index).map_or("", |p| p.0),
    }
}

/// New Page dialog strings.
pub mod new_doc_text {
    use crate::text::{Key, t};

    pub fn custom() -> &'static str {
        t(Key::NewDocCustom)
    }

    pub fn per_layer() -> &'static str {
        t(Key::NewDocPerLayer)
    }

    pub fn gpu() -> &'static str {
        t(Key::NewDocGpu)
    }

    pub fn heavy() -> &'static str {
        t(Key::NewDocHeavy)
    }

    /// "This machine: about N layers".
    pub fn fit(layers: u32) -> String {
        t(if layers == 1 { Key::NewDocFitOne } else { Key::NewDocFit }).replace("{}", &layers.to_string())
    }
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
    pub ui_mode: UiMode,
    pub lang: Lang,
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
    /// Show the home screen at start-up (saved setting).
    pub home_at_start: bool,
    /// What this computer is (machine.rs).
    pub machine: Machine,
    /// Performance setting (saved).
    pub perf: PerfMode,
}

impl Shell {
    pub fn new(theme: ThemeKind) -> Self {
        Self {
            theme,
            theme_dirty: true,
            ui_mode: UiMode::Simple,
            lang: text::current_lang(),
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
            home_at_start: crate::home::default_show(),
            machine: Machine::default(),
            perf: PerfMode::default(),
        }
    }

    /// Light mode is on: no animations, a smaller thumbnail budget.
    pub fn light(&self) -> bool {
        self.perf.light(self.machine.tier())
    }

    /// Light mode changes the style, so it is applied again.
    pub fn set_perf(&mut self, perf: PerfMode) {
        if self.perf != perf {
            self.perf = perf;
            self.theme_dirty = true;
        }
    }

    /// Target sizes differ per mode, so the style is applied again.
    pub fn set_ui_mode(&mut self, mode: UiMode) {
        if self.ui_mode != mode {
            self.ui_mode = mode;
            self.theme_dirty = true;
        }
    }

    pub fn set_lang(&mut self, lang: Lang) {
        self.lang = lang;
        text::set_current_lang(lang);
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
    use crate::machine::{GpuKind, GpuSummary, Storage, Vendor};

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

    fn machine(gib: u64, kind: GpuKind) -> Machine {
        let gpu = GpuSummary { name: "test".into(), vendor: Vendor::Intel, kind, backend: "Vulkan" };
        Machine { ram: Some(gib << 30), physical_cores: 8, gpu: Some(gpu), storage: Storage::Ssd }
    }

    /// Capacity = (RAM / 2 - 450 MiB baseline - GPU canvas - undo budget) / layer, in MiB.
    #[test]
    fn layer_capacity_matches_hand_arithmetic() {
        let a4 = page_memory(2894, 4093); // layer 92.0 MiB, GPU canvas 64.0 MiB
        let b4 = page_memory(6071, 8598); // layer 400.8 MiB, GPU canvas 288.0 MiB
        // A4 350 dpi, 8 GiB, iGPU: undo 8192/16 = 512.
        // 4096 - 450 - 64 - 512 = 3070 MiB; 3070 / 92.0 = 33.4 -> 33 (Roomy).
        let n = layer_capacity(a4, &machine(8, GpuKind::Integrated));
        assert_eq!((n, Fit::of(33)), (Some(33), Fit::Roomy));
        // A4 350 dpi, 4 GiB, iGPU: undo 4096/16 = 256.
        // 2048 - 450 - 64 - 256 = 1278 MiB; 1278 / 92.0 = 13.9 -> 13 (Tight).
        assert_eq!(layer_capacity(a4, &machine(4, GpuKind::Integrated)), Some(13));
        assert_eq!(Fit::of(13), Fit::Tight);
        // B4 600 dpi, 8 GiB, iGPU: 4096 - 450 - 288 - 512 = 2846 MiB; 2846 / 400.8 = 7.1 -> 7 (Heavy).
        assert_eq!(layer_capacity(b4, &machine(8, GpuKind::Integrated)), Some(7));
        assert_eq!(Fit::of(7), Fit::Heavy);
        // B4 600 dpi, 16 GiB, discrete: undo 1024 (the cap), no shared GPU memory.
        // 8192 - 450 - 0 - 1024 = 6718 MiB; 6718 / 400.8 = 16.8 -> 16 (Tight).
        assert_eq!(layer_capacity(b4, &machine(16, GpuKind::Discrete)), Some(16));
        // The same page on an integrated GPU pays for the canvas: 6430 / 400.8 = 16.04 -> 16.
        assert_eq!(layer_capacity(b4, &machine(16, GpuKind::Integrated)), Some(16));
        // Software and unknown graphics are system RAM too.
        assert_eq!(layer_capacity(b4, &machine(8, GpuKind::Software)), Some(7));
        assert_eq!(layer_capacity(b4, &machine(8, GpuKind::Unknown)), Some(7));
    }

    #[test]
    fn layer_capacity_edges() {
        let b4 = page_memory(6071, 8598);
        // Nothing left over: clamped at 0, not wrapped around.
        assert_eq!(layer_capacity(b4, &machine(1, GpuKind::Integrated)), Some(0));
        // Unknown RAM: no figure.
        assert_eq!(layer_capacity(b4, &Machine::default()), None);
        // Fit boundaries.
        assert_eq!([Fit::of(0), Fit::of(7), Fit::of(8), Fit::of(19), Fit::of(20)], [Fit::Heavy, Fit::Heavy, Fit::Tight, Fit::Tight, Fit::Roomy]);
        assert_eq!(page_fit(2894, 4093, &machine(8, GpuKind::Integrated)), Some((33, Fit::Roomy)));
        assert_eq!(page_fit(2894, 4093, &Machine::default()), None);
    }

    #[test]
    fn auto_performance_is_light_only_on_low_tier() {
        for (mode, tier, light) in [
            (PerfMode::Auto, Tier::Low, true),
            (PerfMode::Auto, Tier::Mid, false),
            (PerfMode::Auto, Tier::High, false),
            (PerfMode::Light, Tier::High, true),
            (PerfMode::Full, Tier::Low, false),
        ] {
            assert_eq!(mode.light(tier), light, "{mode:?} on {tier:?}");
        }
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
