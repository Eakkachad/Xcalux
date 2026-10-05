//! UI-side state shared by panels, menus and dialogs (not part of the
//! document model).

use arty_core::{LayerId, PageSetup};
use arty_render::SyncStats;

use crate::commands::SelModify;
use crate::files::AutosaveSettings;
use crate::theme::ThemeKind;

pub struct NewDocForm {
    pub width: u32,
    pub height: u32,
    pub dpi: u32,
    pub preset: usize,
}

/// Page presets for comics and illustration (width, height in px at dpi).
pub const PAGE_PRESETS: &[(&str, u32, u32, u32)] = &[
    ("Manga B4 · 600 dpi (monochrome)", 6071, 8598, 600),
    ("Manga B4 · 350 dpi", 3541, 5016, 350),
    ("Manga B5 · 350 dpi", 2508, 3541, 350),
    ("A4 · 350 dpi", 2894, 4093, 350),
    ("A4 · 600 dpi", 4961, 7016, 600),
    ("Webtoon strip 800 × 12800", 800, 12800, 72),
    ("Illustration 3000 × 4000", 3000, 4000, 350),
    ("Square 2048", 2048, 2048, 144),
];

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
        let (_, width, height, dpi) = PAGE_PRESETS[3];
        Self { width, height, dpi, preset: 3 }
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
    // FRAMES: set by the PageSetup command, read by the dialog.
    #[allow(dead_code)]
    pub page_setup_open: bool,
    /// The Grow / Shrink / Feather dialog is open.
    // SEL-UI: set by SelectionDialog, read by the dialog.
    #[allow(dead_code)]
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
