//! UI string table for Thai-first and English interfaces.
//!
//! Hot-path lookups are allocation-free, returning `&'static str`.
//! Thai and English translations are kept side-by-side on each entry
//! for reviewability.

use std::sync::atomic::{AtomicU8, Ordering};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lang {
    Th,
    En,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::Th, Lang::En];

    pub fn name(self) -> &'static str {
        match self {
            Lang::Th => "ไทย",
            Lang::En => "English",
        }
    }

    /// Windows UI language detection: Thai when primary language is `LANG_THAI` (0x1E).
    pub fn system_default() -> Self {
        #[cfg(windows)]
        {
            unsafe extern "system" {
                fn GetUserDefaultUILanguage() -> u16;
            }
            let lang_id = unsafe { GetUserDefaultUILanguage() };
            lang_from_win32_lang_id(lang_id)
        }
        #[cfg(not(windows))]
        {
            Lang::En
        }
    }
}

impl Default for Lang {
    fn default() -> Self {
        Self::system_default()
    }
}

/// Resolves a Win32 `LANGID` to [`Lang`]: low 10 bits (`PRIMARYLANGID`) = 0x1E is Thai.
pub fn lang_from_win32_lang_id(lang_id: u16) -> Lang {
    const LANG_THAI: u16 = 0x001e;
    if (lang_id & 0x03ff) == LANG_THAI {
        Lang::Th
    } else {
        Lang::En
    }
}

const UNSET: u8 = 0;
const TH: u8 = 1;
const EN: u8 = 2;

static CURRENT_LANG: AtomicU8 = AtomicU8::new(UNSET);

/// Current active UI language.
pub fn current_lang() -> Lang {
    match CURRENT_LANG.load(Ordering::Relaxed) {
        TH => Lang::Th,
        EN => Lang::En,
        _ => {
            let def = Lang::system_default();
            CURRENT_LANG.store(if def == Lang::Th { TH } else { EN }, Ordering::Relaxed);
            def
        }
    }
}

/// Tests that read or switch the language hold this, so parallel tests do
/// not see each other's language.
#[cfg(test)]
pub(crate) fn lang_for_test(lang: Lang) -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_current_lang(lang);
    guard
}

/// Set active UI language.
pub fn set_current_lang(lang: Lang) {
    CURRENT_LANG.store(if lang == Lang::Th { TH } else { EN }, Ordering::Relaxed);
}

/// UI string keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    // Menu bar
    MenuFile,
    MenuEdit,
    MenuLayer,
    MenuSelect,
    MenuView,
    MenuWindow,
    AutosaveMenu,
    AutosaveEnabled,
    AutosaveEvery,

    // Commands
    CmdNewDocument,
    CmdOpen,
    CmdSave,
    CmdSaveAs,
    CmdToggleAutosave,
    CmdExportPng,
    CmdQuit,
    CmdUndo,
    CmdRedo,
    CmdClearLayer,
    CmdClearSelection,
    CmdNewLayer,
    CmdNewFolder,
    CmdDuplicateLayer,
    CmdMergeDown,
    CmdDeleteLayer,
    CmdLayerUp,
    CmdLayerDown,
    CmdMoveLayer,
    CmdToggleClip,
    CmdToggleLockAlpha,
    CmdZoomIn,
    CmdZoomOut,
    CmdZoomFit,
    CmdZoom100,
    CmdRotateLeft,
    CmdRotateRight,
    CmdRotateReset,
    CmdFlipView,
    CmdSwapColors,
    CmdBrushSmaller,
    CmdBrushLarger,
    CmdToggleTheme,
    CmdResetLayout,
    CmdPenTip,
    CmdPenEraser,
    CmdSelectAll,
    CmdDeselect,
    CmdInvertSelection,
    CmdGrowSelectionDialog,
    CmdShrinkSelectionDialog,
    CmdFeatherSelectionDialog,
    CmdGrowSelection,
    CmdShrinkSelection,
    CmdFeatherSelection,
    CmdFillSelection,
    CmdToggleReferenceLayer,
    CmdTransform,
    CmdCommitTransform,
    CmdCancelTransform,
    CmdFlipTransformH,
    CmdFlipTransformV,
    CmdRotateTransformCw,
    CmdRotateTransformCcw,
    CmdPageSetup,
    CmdTogglePageGuides,
    CmdToggleTrimShade,
    CmdNewFrameFolder,
    CmdDeletePanel,

    // Tools & Frame modes
    ToolPen,
    ToolPencil,
    ToolBrush,
    ToolAirbrush,
    ToolBlend,
    ToolEraser,
    ToolEyedropper,
    ToolHand,
    ToolRotate,
    ToolZoom,
    ToolSelect,
    ToolMagicWand,
    ToolFill,
    ToolMove,
    FrameModeRect,
    FrameModeCut,
    FrameModeEdit,

    // Panels / Window Tabs
    TabCanvas,
    TabSubTool,
    TabToolProperty,
    TabBrushSize,
    TabColor,
    TabColorSet,
    TabLayers,
    TabNavigator,

    // New Page Dialog
    NewDocHeading,
    NewDocCustom,
    NewDocWidth,
    NewDocHeight,
    NewDocResolution,
    NewDocPerLayer,
    NewDocGpu,
    NewDocHeavy,
    NewDocCreate,
    NewDocCancel,

    // Page Presets
    PresetMangaB4_350,
    PresetMangaB5_350,
    PresetA4_350,
    PresetMangaB4_600,
    PresetA4_600,
    PresetWebtoon,
    PresetIllustration,
    PresetSquare,

    // Status bar & toasts
    StatusSaving,
    StatusOpening,
    StatusAutosaving,
    StatusLayers,
    StatusComposite,
    StatusTiles,
    StatusPenSystem,
    StatusPen,
    StatusInToFrame,
    StatusMax,
    StatusFrame,
    StatusNotApplied,
    StatusAfterRestart,
    StatusDropped,
    StatusUntitled,
    ToastSaved,
    ToastSavedSelectionBinarized,
    ToastSavedSelectionDropped,
    ToastExported,
    ToastExportFailed,

    // Settings
    LanguageLabel,
}

impl Key {
    pub const ALL: &'static [Key] = &[
        Key::MenuFile,
        Key::MenuEdit,
        Key::MenuLayer,
        Key::MenuSelect,
        Key::MenuView,
        Key::MenuWindow,
        Key::AutosaveMenu,
        Key::AutosaveEnabled,
        Key::AutosaveEvery,
        Key::CmdNewDocument,
        Key::CmdOpen,
        Key::CmdSave,
        Key::CmdSaveAs,
        Key::CmdToggleAutosave,
        Key::CmdExportPng,
        Key::CmdQuit,
        Key::CmdUndo,
        Key::CmdRedo,
        Key::CmdClearLayer,
        Key::CmdClearSelection,
        Key::CmdNewLayer,
        Key::CmdNewFolder,
        Key::CmdDuplicateLayer,
        Key::CmdMergeDown,
        Key::CmdDeleteLayer,
        Key::CmdLayerUp,
        Key::CmdLayerDown,
        Key::CmdMoveLayer,
        Key::CmdToggleClip,
        Key::CmdToggleLockAlpha,
        Key::CmdZoomIn,
        Key::CmdZoomOut,
        Key::CmdZoomFit,
        Key::CmdZoom100,
        Key::CmdRotateLeft,
        Key::CmdRotateRight,
        Key::CmdRotateReset,
        Key::CmdFlipView,
        Key::CmdSwapColors,
        Key::CmdBrushSmaller,
        Key::CmdBrushLarger,
        Key::CmdToggleTheme,
        Key::CmdResetLayout,
        Key::CmdPenTip,
        Key::CmdPenEraser,
        Key::CmdSelectAll,
        Key::CmdDeselect,
        Key::CmdInvertSelection,
        Key::CmdGrowSelectionDialog,
        Key::CmdShrinkSelectionDialog,
        Key::CmdFeatherSelectionDialog,
        Key::CmdGrowSelection,
        Key::CmdShrinkSelection,
        Key::CmdFeatherSelection,
        Key::CmdFillSelection,
        Key::CmdToggleReferenceLayer,
        Key::CmdTransform,
        Key::CmdCommitTransform,
        Key::CmdCancelTransform,
        Key::CmdFlipTransformH,
        Key::CmdFlipTransformV,
        Key::CmdRotateTransformCw,
        Key::CmdRotateTransformCcw,
        Key::CmdPageSetup,
        Key::CmdTogglePageGuides,
        Key::CmdToggleTrimShade,
        Key::CmdNewFrameFolder,
        Key::CmdDeletePanel,
        Key::ToolPen,
        Key::ToolPencil,
        Key::ToolBrush,
        Key::ToolAirbrush,
        Key::ToolBlend,
        Key::ToolEraser,
        Key::ToolEyedropper,
        Key::ToolHand,
        Key::ToolRotate,
        Key::ToolZoom,
        Key::ToolSelect,
        Key::ToolMagicWand,
        Key::ToolFill,
        Key::ToolMove,
        Key::FrameModeRect,
        Key::FrameModeCut,
        Key::FrameModeEdit,
        Key::TabCanvas,
        Key::TabSubTool,
        Key::TabToolProperty,
        Key::TabBrushSize,
        Key::TabColor,
        Key::TabColorSet,
        Key::TabLayers,
        Key::TabNavigator,
        Key::NewDocHeading,
        Key::NewDocCustom,
        Key::NewDocWidth,
        Key::NewDocHeight,
        Key::NewDocResolution,
        Key::NewDocPerLayer,
        Key::NewDocGpu,
        Key::NewDocHeavy,
        Key::NewDocCreate,
        Key::NewDocCancel,
        Key::PresetMangaB4_350,
        Key::PresetMangaB5_350,
        Key::PresetA4_350,
        Key::PresetMangaB4_600,
        Key::PresetA4_600,
        Key::PresetWebtoon,
        Key::PresetIllustration,
        Key::PresetSquare,
        Key::StatusSaving,
        Key::StatusOpening,
        Key::StatusAutosaving,
        Key::StatusLayers,
        Key::StatusComposite,
        Key::StatusTiles,
        Key::StatusPenSystem,
        Key::StatusPen,
        Key::StatusInToFrame,
        Key::StatusMax,
        Key::StatusFrame,
        Key::StatusNotApplied,
        Key::StatusAfterRestart,
        Key::StatusDropped,
        Key::StatusUntitled,
        Key::ToastSaved,
        Key::ToastSavedSelectionBinarized,
        Key::ToastSavedSelectionDropped,
        Key::ToastExported,
        Key::ToastExportFailed,
        Key::LanguageLabel,
    ];
}

/// Pair of (Thai, English) translation for each key.
/// Kept side-by-side on each line for easy review.
pub const fn lookup(key: Key) -> (&'static str, &'static str) {
    match key {
        // Menu bar
        Key::MenuFile => ("ไฟล์", "File"),
        Key::MenuEdit => ("แก้ไข", "Edit"),
        Key::MenuLayer => ("เลเยอร์", "Layer"),
        Key::MenuSelect => ("เลือก", "Select"),
        Key::MenuView => ("มุมมอง", "View"),
        Key::MenuWindow => ("หน้าต่าง", "Window"),
        Key::AutosaveMenu => ("บันทึกอัตโนมัติ", "Autosave"),
        Key::AutosaveEnabled => ("เปิดบันทึกอัตโนมัติ", "Autosave enabled"),
        Key::AutosaveEvery => ("ทุก", "Every"),

        // Commands
        Key::CmdNewDocument => ("สร้างใหม่…", "New…"),
        Key::CmdOpen => ("เปิด…", "Open…"),
        Key::CmdSave => ("บันทึก", "Save"),
        Key::CmdSaveAs => ("บันทึกเป็น…", "Save As…"),
        Key::CmdToggleAutosave => ("บันทึกอัตโนมัติ", "Autosave"),
        Key::CmdExportPng => ("ส่งออก PNG…", "Export PNG…"),
        Key::CmdQuit => ("ออก", "Quit"),
        Key::CmdUndo => ("เลิกทำ", "Undo"),
        Key::CmdRedo => ("ทำซ้ำ", "Redo"),
        Key::CmdClearLayer => ("ล้างเลเยอร์", "Clear Layer"),
        Key::CmdClearSelection => ("ล้างพื้นที่ที่เลือก", "Clear Selected Area"),
        Key::CmdNewLayer => ("เลเยอร์ใหม่", "New Raster Layer"),
        Key::CmdNewFolder => ("โฟลเดอร์ใหม่", "New Folder"),
        Key::CmdDuplicateLayer => ("ทำสำเนาเลเยอร์", "Duplicate Layer"),
        Key::CmdMergeDown => ("รวมเลเยอร์ด้านล่าง", "Merge Down"),
        Key::CmdDeleteLayer => ("ลบเลเยอร์", "Delete Layer"),
        Key::CmdLayerUp => ("เลื่อนเลเยอร์ขึ้น", "Move Layer Up"),
        Key::CmdLayerDown => ("เลื่อนเลเยอร์ลง", "Move Layer Down"),
        Key::CmdMoveLayer => ("ย้ายเลเยอร์", "Move Layer"),
        Key::CmdToggleClip => ("คลิปกับเลเยอร์ด้านล่าง", "Clip to Layer Below"),
        Key::CmdToggleLockAlpha => ("ล็อกพิกเซลโปร่งใส", "Lock Transparent Pixels"),
        Key::CmdZoomIn => ("ขยาย", "Zoom In"),
        Key::CmdZoomOut => ("ย่อ", "Zoom Out"),
        Key::CmdZoomFit => ("พอดีหน้าต่าง", "Fit to Window"),
        Key::CmdZoom100 => ("ขนาดจริง (100%)", "Actual Pixels (100%)"),
        Key::CmdRotateLeft => ("หมุนซ้าย 15°", "Rotate Left 15°"),
        Key::CmdRotateRight => ("หมุนขวา 15°", "Rotate Right 15°"),
        Key::CmdRotateReset => ("รีเซ็ตการหมุน", "Reset Rotation"),
        Key::CmdFlipView => ("พลิกมุมมองซ้าย-ขวา", "Flip View Horizontally"),
        Key::CmdSwapColors => ("สลับสีหลัก/สีรอง", "Swap Main/Sub Color"),
        Key::CmdBrushSmaller => ("ลดขนาดหัวแปรง", "Brush Smaller"),
        Key::CmdBrushLarger => ("เพิ่มขนาดหัวแปรง", "Brush Larger"),
        Key::CmdToggleTheme => ("สลับธีมสว่าง/มืด", "Toggle Light/Dark"),
        Key::CmdResetLayout => ("คืนค่าการจัดวางแผง", "Reset Panel Layout"),
        Key::CmdPenTip => ("หัวปากกา", "Pen Tip"),
        Key::CmdPenEraser => ("ท้ายปากกา (ยางลบ)", "Pen Eraser End"),
        Key::CmdSelectAll => ("เลือกทั้งหมด", "Select All"),
        Key::CmdDeselect => ("ยกเลิกการเลือก", "Deselect"),
        Key::CmdInvertSelection => ("กลับด้านการเลือก", "Invert Selection"),
        Key::CmdGrowSelectionDialog => ("ขยายพื้นที่เลือก…", "Grow Selection…"),
        Key::CmdShrinkSelectionDialog => ("ย่อพื้นที่เลือก…", "Shrink Selection…"),
        Key::CmdFeatherSelectionDialog => ("ทำขอบพื้นที่เลือกให้นุ่ม…", "Feather Selection…"),
        Key::CmdGrowSelection => ("ขยายพื้นที่เลือก", "Grow Selection"),
        Key::CmdShrinkSelection => ("ย่อพื้นที่เลือก", "Shrink Selection"),
        Key::CmdFeatherSelection => ("ทำขอบพื้นที่เลือกให้นุ่ม", "Feather Selection"),
        Key::CmdFillSelection => ("เทสีพื้นที่เลือก", "Fill Selection"),
        Key::CmdToggleReferenceLayer => ("เลเยอร์อ้างอิง", "Reference Layer"),
        Key::CmdTransform => ("แปลงรูปทรง", "Transform"),
        Key::CmdCommitTransform => ("ยืนยันการแปลงรูปทรง", "Commit Transform"),
        Key::CmdCancelTransform => ("ยกเลิกการแปลงรูปทรง", "Cancel Transform"),
        Key::CmdFlipTransformH => ("แปลงรูปทรง: กลับด้านแนวนอน", "Transform: Flip Horizontal"),
        Key::CmdFlipTransformV => ("แปลงรูปทรง: กลับด้านแนวตั้ง", "Transform: Flip Vertical"),
        Key::CmdRotateTransformCw => ("แปลงรูปทรง: หมุน 90° ตามเข็ม", "Transform: Rotate 90° CW"),
        Key::CmdRotateTransformCcw => ("แปลงรูปทรง: หมุน 90° ทวนเข็ม", "Transform: Rotate 90° CCW"),
        Key::CmdPageSetup => ("ตั้งค่าหน้ากระดาษ…", "Page Setup…"),
        Key::CmdTogglePageGuides => ("เส้นบอกแนวหน้ากระดาษ", "Page Guides"),
        Key::CmdToggleTrimShade => ("แรเงาขอบนอกระยะตัด", "Shade Outside Trim"),
        Key::CmdNewFrameFolder => ("โฟลเดอร์กรอบช่องใหม่", "New Frame Border Folder"),
        Key::CmdDeletePanel => ("ลบช่อง", "Delete Panel"),

        // Tools & Frame modes
        Key::ToolPen => ("ปากกา", "Pen"),
        Key::ToolPencil => ("ดินสอ", "Pencil"),
        Key::ToolBrush => ("พู่กัน", "Brush"),
        Key::ToolAirbrush => ("แอร์บรัช", "Airbrush"),
        Key::ToolBlend => ("เกลี่ยสี", "Blend"),
        Key::ToolEraser => ("ยางลบ", "Eraser"),
        Key::ToolEyedropper => ("หลอดดูดสี", "Eyedropper"),
        Key::ToolHand => ("เลื่อนภาพ", "Hand"),
        Key::ToolRotate => ("หมุนมุมมอง", "Rotate"),
        Key::ToolZoom => ("ย่อขยาย", "Zoom"),
        Key::ToolSelect => ("เลือกพื้นที่", "Selection"),
        Key::ToolMagicWand => ("ไม้กายสิทธิ์", "Magic Wand"),
        Key::ToolFill => ("เทสี", "Fill"),
        Key::ToolMove => ("ย้าย", "Move"),
        Key::FrameModeRect => ("กรอบสี่เหลี่ยม", "Rectangle Frame"),
        Key::FrameModeCut => ("ตัดแบ่งช่อง", "Divide Frame"),
        Key::FrameModeEdit => ("แก้ไขกรอบช่อง", "Frame Edit"),

        // Panels / Window Tabs
        Key::TabCanvas => ("ผืนผ้าใบ", "Canvas"),
        Key::TabSubTool => ("เครื่องมือย่อย", "Sub Tool"),
        Key::TabToolProperty => ("คุณสมบัติเครื่องมือ", "Tool Property"),
        Key::TabBrushSize => ("ขนาดหัวแปรง", "Brush Size"),
        Key::TabColor => ("สี", "Color"),
        Key::TabColorSet => ("ชุดสี", "Color Set"),
        Key::TabLayers => ("เลเยอร์", "Layer"),
        Key::TabNavigator => ("ภาพรวม", "Navigator"),

        // New Page Dialog
        Key::NewDocHeading => ("สร้างหน้าใหม่", "New Page"),
        Key::NewDocCustom => ("กำหนดเอง", "Custom"),
        Key::NewDocWidth => ("ความกว้าง", "Width"),
        Key::NewDocHeight => ("ความสูง", "Height"),
        Key::NewDocResolution => ("ความละเอียด", "Resolution"),
        Key::NewDocPerLayer => ("MB / เลเยอร์", "MB / layer"),
        Key::NewDocGpu => ("MB บน GPU", "MB GPU"),
        Key::NewDocHeavy => (
            "10 เลเยอร์จะใช้หน่วยความจำเกินครึ่งหนึ่งของเครื่องนี้ เครื่องอาจทำงานช้าลงหรือค้างได้",
            "10 layers would use over half of this computer's memory; it may slow down or freeze.",
        ),
        Key::NewDocCreate => ("สร้าง", "Create"),
        Key::NewDocCancel => ("ยกเลิก", "Cancel"),

        // Page Presets
        Key::PresetMangaB4_350 => ("มังงะ B4 · 350 dpi", "Manga B4 · 350 dpi"),
        Key::PresetMangaB5_350 => ("มังงะ B5 · 350 dpi", "Manga B5 · 350 dpi"),
        Key::PresetA4_350 => ("A4 · 350 dpi", "A4 · 350 dpi"),
        Key::PresetMangaB4_600 => ("มังงะ B4 · 600 dpi", "Manga B4 · 600 dpi"),
        Key::PresetA4_600 => ("A4 · 600 dpi", "A4 · 600 dpi"),
        Key::PresetWebtoon => ("แถบเว็บตูน 800 × 12800", "Webtoon strip 800 × 12800"),
        Key::PresetIllustration => ("ภาพวาด 3000 × 4000", "Illustration 3000 × 4000"),
        Key::PresetSquare => ("สี่เหลี่ยมจัตุรัส 2048", "Square 2048"),

        // Status bar & toasts
        Key::StatusSaving => ("กำลังบันทึก…", "Saving…"),
        Key::StatusOpening => ("กำลังเปิด…", "Opening…"),
        Key::StatusAutosaving => ("กำลังบันทึกอัตโนมัติ…", "Autosaving…"),
        Key::StatusLayers => ("เลเยอร์", "layers"),
        Key::StatusComposite => ("รวมภาพ", "composite"),
        Key::StatusTiles => ("ไทล์", "tiles"),
        Key::StatusPenSystem => ("ปากกา: ระบบ", "pen: system"),
        Key::StatusPen => ("ปากกา", "pen"),
        Key::StatusInToFrame => ("อินพุต-เฟรม", "in-frame"),
        Key::StatusMax => ("สูงสุด", "max"),
        Key::StatusFrame => ("เฟรม", "frame"),
        Key::StatusNotApplied => ("ยังไม่มีผล", "not applied"),
        Key::StatusAfterRestart => ("หลังเริ่มใหม่", "after restart"),
        Key::StatusDropped => ("ตกหล่น", "dropped"),
        Key::StatusUntitled => ("ไม่มีชื่อ", "Untitled"),
        Key::ToastSaved => ("บันทึกแล้ว", "Saved"),
        Key::ToastSavedSelectionBinarized => (
            "บันทึกแล้ว: พื้นที่เลือกมีรายละเอียดมากเกินไป ขอบฟุ้งจึงถูกปรับเป็นขอบคม",
            "Saved; the selection was too detailed and its soft edges were made hard",
        ),
        Key::ToastSavedSelectionDropped => (
            "บันทึกแล้วโดยไม่รวมพื้นที่เลือก เนื่องจากมีขนาดใหญ่เกินกว่าจะจัดเก็บ",
            "Saved without the selection, which was too large to store",
        ),
        Key::ToastExported => ("ส่งออกแล้ว", "Exported"),
        Key::ToastExportFailed => ("การส่งออกล้มเหลว", "Export failed"),

        // Settings
        Key::LanguageLabel => ("ภาษา / Language", "Language / ภาษา"),
    }
}

/// Look up a string for the specified language.
#[inline]
pub fn text_for(lang: Lang, key: Key) -> &'static str {
    let (th, en) = lookup(key);
    match lang {
        Lang::Th => th,
        Lang::En => en,
    }
}

/// Look up a string for the current active language.
#[inline]
pub fn text(key: Key) -> &'static str {
    text_for(current_lang(), key)
}

/// Terse alias for [`text`].
#[inline]
pub fn t(key: Key) -> &'static str {
    text(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use skrifa::{FontRef, MetadataProvider};

    #[test]
    fn no_key_is_missing_translation() {
        for &k in Key::ALL {
            let (th, en) = lookup(k);
            assert!(!th.trim().is_empty(), "Key {k:?} missing Thai translation");
            assert!(!en.trim().is_empty(), "Key {k:?} missing English translation");
        }
    }

    #[test]
    fn language_default_logic() {
        // Thai language identifiers
        assert_eq!(lang_from_win32_lang_id(0x041e), Lang::Th);
        assert_eq!(lang_from_win32_lang_id(0x001e), Lang::Th);
        assert_eq!(lang_from_win32_lang_id(0x081e), Lang::Th);

        // Other languages
        assert_eq!(lang_from_win32_lang_id(0x0409), Lang::En); // US English
        assert_eq!(lang_from_win32_lang_id(0x0809), Lang::En); // UK English
        assert_eq!(lang_from_win32_lang_id(0x0411), Lang::En); // Japanese
        assert_eq!(lang_from_win32_lang_id(0x0404), Lang::En); // Chinese
        assert_eq!(lang_from_win32_lang_id(0x0000), Lang::En); // Neutral
    }

    #[test]
    fn language_switch() {
        let _lang = lang_for_test(Lang::En);
        assert_eq!(current_lang(), Lang::En);
        assert_eq!(t(Key::CmdUndo), "Undo");

        set_current_lang(Lang::Th);
        assert_eq!(current_lang(), Lang::Th);
        assert_eq!(t(Key::CmdUndo), "เลิกทำ");

        set_current_lang(Lang::En);
    }

    #[test]
    fn every_string_has_glyphs_and_no_banned_chars() {
        // The UI text uses the proportional family; a glyph that only the
        // monospace face (Hack) has, such as an arrow, would render as a box.
        let defs = crate::theme::font_definitions();
        let fonts: Vec<FontRef> = defs.families[&egui::FontFamily::Proportional]
            .iter()
            .map(|name| FontRef::new(&defs.font_data[name].font).expect("valid font"))
            .collect();
        assert!(!fonts.is_empty(), "fonts must be loaded");

        for &k in Key::ALL {
            let (th, en) = lookup(k);
            for &(lang_name, s) in &[("Thai", th), ("English", en)] {
                // Rules from B014: never use U+200B or ✓; arrows fail the glyph check below
                assert!(
                    !s.contains('\u{200B}'),
                    "Banned U+200B (ZWSP) found in {lang_name} string for {k:?}: {s:?}"
                );
                assert!(
                    !s.contains('\u{2713}'),
                    "Banned U+2713 (✓) found in {lang_name} string for {k:?}: {s:?}"
                );
                for ch in s.chars() {
                    if ch.is_whitespace() || ch == '\n' || ch == '\t' {
                        continue;
                    }
                    let has_glyph = fonts.iter().any(|f| f.charmap().map(ch).is_some());
                    assert!(
                        has_glyph,
                        "Missing glyph for char '{ch}' (U+{:04X}) in {lang_name} for key {k:?}: {s:?}",
                        ch as u32
                    );
                }
            }
        }
    }
}
