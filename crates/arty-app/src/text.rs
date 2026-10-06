//! UI string table for Thai-first and English interfaces.
//!
//! Hot-path lookups are allocation-free, returning `&'static str`.
//! Thai and English translations are kept side-by-side on each entry
//! for reviewability.

use std::sync::atomic::{AtomicU8, Ordering};
use arty_core::BlendMode;
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

    // Blend modes
    BlendNormal,
    BlendMultiply,
    BlendScreen,
    BlendOverlay,
    BlendDarken,
    BlendLighten,
    BlendColorDodge,
    BlendColorBurn,
    BlendLinearBurn,
    BlendAdd,
    BlendSoftLight,
    BlendHardLight,
    BlendDifference,
    BlendPassThrough,

    // Layer panel
    LayerLock,
    LayerRename,

    // Common / Dialogs
    CommonOk,
    CommonApply,
    CommonNone,
    CommonOff,
    CommonLevel,
    CommonDefaultSuffix,

    // Property panel & brush options
    PropName,
    PropSize,
    PropOpacity,
    PropHardness,
    PropStabilizer,
    SectionPenPressure,
    PropMinSize,
    PropMinSizeTip,
    PropMinOpacity,
    PropMinOpacityTip,
    SectionAdvanced,
    PropDensity,
    PropDensityTip,
    PropBlending,
    PropBlendingTip,
    PropPersistence,
    PropPersistenceTip,
    PropSizeJitter,
    PropEraser,
    SectionStartEnd,
    PropTaperIn,
    PropTaperInTip,
    PropTaperOut,
    PropTaperOutTip,
    PropPostCorrection,
    PropPostCorrectionTip,

    // Property panel hints
    HintEyedropper,
    HintHand,
    HintRotate,
    HintZoom,

    // Sub tool panel
    SubToolRestoreDefaults,
    SubToolDuplicate,
    SubToolDuplicateMenu,
    SubToolDeleteMenu,

    // Color & Swatches panel
    ColorSubSwapTip,
    SectionColorSet,
    ColorRemove,
    ColorAddMainTip,
    SectionHistory,
    ColorHistoryEmpty,

    // Brush size panel
    BrushSizeSelectTool,

    // Navigator panel
    NavFlipHorizontal,

    // Pen settings panel
    PenSettingsInputDisplay,
    PenSettingsMousePressure,
    PenSettingsNativePen,
    PenSettingsNativePenTip,
    PenSettingsEraserEnd,
    PenSettingsEraserEndTip,
    PenSettingsDisplaySync,
    PenSettingsLatencyOverlay,
    PenSettingsLatencyOverlayTip,
    SyncSmoothTip,
    SyncLowLatencyTip,
    SyncFastVsyncTip,
    SyncOffTip,
    SyncNeedsDx12,
    SyncNotAppliedPrefix,
    SyncAfterRestartPrefix,
    DisplaySyncSmooth,
    DisplaySyncLowLatency,
    DisplaySyncFastVsync,
    DisplaySyncOff,

    // Curve editor
    CurveMaxPoints,
    CurveInput,
    CurveOutput,
    CurveIn,
    CurveOut,
    CurveInputPct,
    CurveOutputPct,
    CurveReset,
    CurveResetTip,
    CurveTestPad,

    // Toolbar tooltips
    TooltipPen,
    TooltipPencil,
    TooltipBrush,
    TooltipAirbrush,
    TooltipBlend,
    TooltipEraser,
    TooltipMove,
    TooltipSelect,
    TooltipMagicWand,
    TooltipFill,
    TooltipFrameEdit,
    TooltipEyedropper,
    TooltipHand,
    TooltipRotate,
    TooltipZoom,

    // Fill tool
    FillReferTo,
    FillRefActive,
    FillRefAll,
    FillRefReference,
    FillTolerance,
    FillCloseGap,
    FillAreaScaling,
    FillToDarkest,
    FillToDarkestTip,
    FillContiguous,
    FillContiguousTip,
    FillAntialiasing,
    FillOpacity,
    FillBlend,
    FillBlendBehind,
    FillAllLayersNote,

    // Frame tool
    HintFrameRect,
    HintFrameCut,
    HintFrameEdit,
    SectionPanels,
    FrameGutterLr,
    FrameGutterTb,
    FrameNewBorder,
    FrameNewFolderPerFrame,
    FrameDivideIntoFolders,
    FrameSnapGuides,
    FrameSnapPanels,
    FrameNotInFolder,
    SectionFrameBorder,
    FrameBorder,
    FrameWidth,
    FrameColor,
    FramePanelCountSingle,
    FramePanelCountPlural,

    // Page tool & Page setup
    PageCanvasSize,
    PageNoSetup,
    PagePreset,
    PageUnit,
    PageTrimSize,
    PageTrimPos,
    PageCentre,
    PageBleed,
    PageSafeMargin,
    PageInnerFrame,
    PageInnerPos,
    PageCentreTrim,
    PageTrimInsideCanvas,
    ExportCropCanvas,
    ExportCropBleed,
    ExportCropTrim,
    ExportNeedPageSetup,
    ExportButton,
    PageMangaManuscript,

    // Selection tool
    SelShapeRect,
    SelShapeEllipse,
    SelShapeLasso,
    SelShapePolygon,
    SelOpNew,
    SelOpNewTip,
    SelOpAdd,
    SelOpAddTip,
    SelOpSubtract,
    SelOpSubtractTip,
    SelOpIntersect,
    SelOpIntersectTip,
    SelShape,
    SelMode,
    FillRefAllVisible,
    HintSelectWand,
    HintSelectDrag,
    HintSelectLasso,
    HintSelectPolygon,
    NoticeSelectionTooDetailed,
    SelRadius,
    SelAmount,
    SelShapeCircle,
    SelShapeSquare,

    // Transform tool
    XfInterpolation,
    FilterNearest,
    FilterBilinear,
    FilterBicubic,
    HintMoveTool,
    HintMoveTransform,
    XfTargetLayer,
    XfTargetSelection,
    XfAngle,
    XfCommit,

    // Notices & Alerts
    NoticeLayerLocked,
    NoticeLayerHidden,
    NoticeLayerAlphaLocked,
    NoticeSelectRasterToFill,
    NoticeSelectRasterToPaint,
    NoticeNothingSelected,
    NoticeKeepOneSubTool,
    NoticeStrokeTooLong,
    NoticeFrameMaxPanels,
    NoticeSelectFrameToDivide,
    NoticeGutterWiderThanPanel,
    NoticeDragAcrossPanel,
    NoticeFrameKeepOnePanel,
    NoticeXfFolder,
    NoticeXfEmpty,
    NoticeXfSelectRaster,

    // Modals in files.rs
    ModalUnsavedTitle,
    ModalUnsavedBody,
    ModalSave,
    ModalDontSave,
    ModalLossyTitle,
    ModalLossyBody,
    ModalExternalTitle,
    ModalExternalBody,
    ModalOverwrite,
    GpuCanvasUnavailable,
    MsgFilesUnavailable,
    MsgCouldNotSave,
    MsgCouldNotOpen,
    MsgCouldNotRestore,
    MsgFileError,
    MsgOpenedWithWarnings,
    ModalFinishingSave,
    RecoveryTitle,
    RecoveryBody,
    RecoveryRestore,
    RecoveryDiscard,
    RecoveryLater,
    RecoveryLaterTip,
    TimeJustNow,
    TimeMinAgo,
    TimeHourAgo,
    TimeDaysAgo,

    // Status bar latency tip
    StatusLatencyTip,

    // Default brush preset display names
    PresetDisplayGPen,
    PresetDisplayInkingPen,
    PresetDisplayMappingPen,
    PresetDisplayMarker,
    PresetDisplayPencil,
    PresetDisplaySketchPencil,
    PresetDisplayBrush,
    PresetDisplayWatercolor,
    PresetDisplayFlatColor,
    PresetDisplayAirbrush,
    PresetDisplayBlender,
    PresetDisplayHardEraser,
    PresetDisplaySoftEraser,
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
        Key::BlendNormal,
        Key::BlendMultiply,
        Key::BlendScreen,
        Key::BlendOverlay,
        Key::BlendDarken,
        Key::BlendLighten,
        Key::BlendColorDodge,
        Key::BlendColorBurn,
        Key::BlendLinearBurn,
        Key::BlendAdd,
        Key::BlendSoftLight,
        Key::BlendHardLight,
        Key::BlendDifference,
        Key::BlendPassThrough,
        Key::LayerLock,
        Key::LayerRename,
        Key::CommonOk,
        Key::CommonApply,
        Key::CommonNone,
        Key::CommonOff,
        Key::CommonLevel,
        Key::CommonDefaultSuffix,
        Key::PropName,
        Key::PropSize,
        Key::PropOpacity,
        Key::PropHardness,
        Key::PropStabilizer,
        Key::SectionPenPressure,
        Key::PropMinSize,
        Key::PropMinSizeTip,
        Key::PropMinOpacity,
        Key::PropMinOpacityTip,
        Key::SectionAdvanced,
        Key::PropDensity,
        Key::PropDensityTip,
        Key::PropBlending,
        Key::PropBlendingTip,
        Key::PropPersistence,
        Key::PropPersistenceTip,
        Key::PropSizeJitter,
        Key::PropEraser,
        Key::SectionStartEnd,
        Key::PropTaperIn,
        Key::PropTaperInTip,
        Key::PropTaperOut,
        Key::PropTaperOutTip,
        Key::PropPostCorrection,
        Key::PropPostCorrectionTip,
        Key::HintEyedropper,
        Key::HintHand,
        Key::HintRotate,
        Key::HintZoom,
        Key::SubToolRestoreDefaults,
        Key::SubToolDuplicate,
        Key::SubToolDuplicateMenu,
        Key::SubToolDeleteMenu,
        Key::ColorSubSwapTip,
        Key::SectionColorSet,
        Key::ColorRemove,
        Key::ColorAddMainTip,
        Key::SectionHistory,
        Key::ColorHistoryEmpty,
        Key::BrushSizeSelectTool,
        Key::NavFlipHorizontal,
        Key::PenSettingsInputDisplay,
        Key::PenSettingsMousePressure,
        Key::PenSettingsNativePen,
        Key::PenSettingsNativePenTip,
        Key::PenSettingsEraserEnd,
        Key::PenSettingsEraserEndTip,
        Key::PenSettingsDisplaySync,
        Key::PenSettingsLatencyOverlay,
        Key::PenSettingsLatencyOverlayTip,
        Key::SyncSmoothTip,
        Key::SyncLowLatencyTip,
        Key::SyncFastVsyncTip,
        Key::SyncOffTip,
        Key::SyncNeedsDx12,
        Key::SyncNotAppliedPrefix,
        Key::SyncAfterRestartPrefix,
        Key::DisplaySyncSmooth,
        Key::DisplaySyncLowLatency,
        Key::DisplaySyncFastVsync,
        Key::DisplaySyncOff,
        Key::CurveMaxPoints,
        Key::CurveInput,
        Key::CurveOutput,
        Key::CurveIn,
        Key::CurveOut,
        Key::CurveInputPct,
        Key::CurveOutputPct,
        Key::CurveReset,
        Key::CurveResetTip,
        Key::CurveTestPad,
        Key::TooltipPen,
        Key::TooltipPencil,
        Key::TooltipBrush,
        Key::TooltipAirbrush,
        Key::TooltipBlend,
        Key::TooltipEraser,
        Key::TooltipMove,
        Key::TooltipSelect,
        Key::TooltipMagicWand,
        Key::TooltipFill,
        Key::TooltipFrameEdit,
        Key::TooltipEyedropper,
        Key::TooltipHand,
        Key::TooltipRotate,
        Key::TooltipZoom,
        Key::FillReferTo,
        Key::FillRefActive,
        Key::FillRefAll,
        Key::FillRefReference,
        Key::FillTolerance,
        Key::FillCloseGap,
        Key::FillAreaScaling,
        Key::FillToDarkest,
        Key::FillToDarkestTip,
        Key::FillContiguous,
        Key::FillContiguousTip,
        Key::FillAntialiasing,
        Key::FillOpacity,
        Key::FillBlend,
        Key::FillBlendBehind,
        Key::FillAllLayersNote,
        Key::HintFrameRect,
        Key::HintFrameCut,
        Key::HintFrameEdit,
        Key::SectionPanels,
        Key::FrameGutterLr,
        Key::FrameGutterTb,
        Key::FrameNewBorder,
        Key::FrameNewFolderPerFrame,
        Key::FrameDivideIntoFolders,
        Key::FrameSnapGuides,
        Key::FrameSnapPanels,
        Key::FrameNotInFolder,
        Key::SectionFrameBorder,
        Key::FrameBorder,
        Key::FrameWidth,
        Key::FrameColor,
        Key::FramePanelCountSingle,
        Key::FramePanelCountPlural,
        Key::PageCanvasSize,
        Key::PageNoSetup,
        Key::PagePreset,
        Key::PageUnit,
        Key::PageTrimSize,
        Key::PageTrimPos,
        Key::PageCentre,
        Key::PageBleed,
        Key::PageSafeMargin,
        Key::PageInnerFrame,
        Key::PageInnerPos,
        Key::PageCentreTrim,
        Key::PageTrimInsideCanvas,
        Key::ExportCropCanvas,
        Key::ExportCropBleed,
        Key::ExportCropTrim,
        Key::ExportNeedPageSetup,
        Key::ExportButton,
        Key::PageMangaManuscript,
        Key::SelShapeRect,
        Key::SelShapeEllipse,
        Key::SelShapeLasso,
        Key::SelShapePolygon,
        Key::SelOpNew,
        Key::SelOpNewTip,
        Key::SelOpAdd,
        Key::SelOpAddTip,
        Key::SelOpSubtract,
        Key::SelOpSubtractTip,
        Key::SelOpIntersect,
        Key::SelOpIntersectTip,
        Key::SelShape,
        Key::SelMode,
        Key::FillRefAllVisible,
        Key::HintSelectWand,
        Key::HintSelectDrag,
        Key::HintSelectLasso,
        Key::HintSelectPolygon,
        Key::NoticeSelectionTooDetailed,
        Key::SelRadius,
        Key::SelAmount,
        Key::SelShapeCircle,
        Key::SelShapeSquare,
        Key::XfInterpolation,
        Key::FilterNearest,
        Key::FilterBilinear,
        Key::FilterBicubic,
        Key::HintMoveTool,
        Key::HintMoveTransform,
        Key::XfTargetLayer,
        Key::XfTargetSelection,
        Key::XfAngle,
        Key::XfCommit,
        Key::NoticeLayerLocked,
        Key::NoticeLayerHidden,
        Key::NoticeLayerAlphaLocked,
        Key::NoticeSelectRasterToFill,
        Key::NoticeSelectRasterToPaint,
        Key::NoticeNothingSelected,
        Key::NoticeKeepOneSubTool,
        Key::NoticeStrokeTooLong,
        Key::NoticeFrameMaxPanels,
        Key::NoticeSelectFrameToDivide,
        Key::NoticeGutterWiderThanPanel,
        Key::NoticeDragAcrossPanel,
        Key::NoticeFrameKeepOnePanel,
        Key::NoticeXfFolder,
        Key::NoticeXfEmpty,
        Key::NoticeXfSelectRaster,
        Key::ModalUnsavedTitle,
        Key::ModalUnsavedBody,
        Key::ModalSave,
        Key::ModalDontSave,
        Key::ModalLossyTitle,
        Key::ModalLossyBody,
        Key::ModalExternalTitle,
        Key::ModalExternalBody,
        Key::ModalOverwrite,
        Key::GpuCanvasUnavailable,
        Key::MsgFilesUnavailable,
        Key::MsgCouldNotSave,
        Key::MsgCouldNotOpen,
        Key::MsgCouldNotRestore,
        Key::MsgFileError,
        Key::MsgOpenedWithWarnings,
        Key::ModalFinishingSave,
        Key::RecoveryTitle,
        Key::RecoveryBody,
        Key::RecoveryRestore,
        Key::RecoveryDiscard,
        Key::RecoveryLater,
        Key::RecoveryLaterTip,
        Key::TimeJustNow,
        Key::TimeMinAgo,
        Key::TimeHourAgo,
        Key::TimeDaysAgo,
        Key::StatusLatencyTip,
        Key::PresetDisplayGPen,
        Key::PresetDisplayInkingPen,
        Key::PresetDisplayMappingPen,
        Key::PresetDisplayMarker,
        Key::PresetDisplayPencil,
        Key::PresetDisplaySketchPencil,
        Key::PresetDisplayBrush,
        Key::PresetDisplayWatercolor,
        Key::PresetDisplayFlatColor,
        Key::PresetDisplayAirbrush,
        Key::PresetDisplayBlender,
        Key::PresetDisplayHardEraser,
        Key::PresetDisplaySoftEraser,
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

        // Blend modes
        Key::BlendNormal => ("ปกติ", "Normal"),
        Key::BlendMultiply => ("คูณ", "Multiply"),
        Key::BlendScreen => ("สกรีน", "Screen"),
        Key::BlendOverlay => ("ซ้อนทับ", "Overlay"),
        Key::BlendDarken => ("Darken", "Darken"),
        Key::BlendLighten => ("Lighten", "Lighten"),
        Key::BlendColorDodge => ("Color Dodge", "Color Dodge"),
        Key::BlendColorBurn => ("Color Burn", "Color Burn"),
        Key::BlendLinearBurn => ("Linear Burn", "Linear Burn"),
        Key::BlendAdd => ("Add (Glow)", "Add (Glow)"),
        Key::BlendSoftLight => ("Soft Light", "Soft Light"),
        Key::BlendHardLight => ("Hard Light", "Hard Light"),
        Key::BlendDifference => ("Difference", "Difference"),
        Key::BlendPassThrough => ("Pass Through", "Pass Through"),

        // Layer panel
        Key::LayerLock => ("ล็อกเลเยอร์", "Lock layer"),
        Key::LayerRename => ("เปลี่ยนชื่อ", "Rename"),

        // Common / Dialogs
        Key::CommonOk => ("ตกลง", "OK"),
        Key::CommonApply => ("ใช้", "Apply"),
        Key::CommonNone => ("ไม่มี", "None"),
        Key::CommonOff => ("ปิด", "Off"),
        Key::CommonLevel => ("ระดับ", "Level"),
        Key::CommonDefaultSuffix => ("(ค่าเริ่มต้น)", "(default)"),

        // Property panel & brush options
        Key::PropName => ("ชื่อ", "Name"),
        Key::PropSize => ("ขนาด", "Size"),
        Key::PropOpacity => ("ความทึบ", "Opacity"),
        Key::PropHardness => ("ความแข็งขอบ", "Hardness"),
        Key::PropStabilizer => ("ความนิ่งของเส้น", "Stabilizer"),
        Key::SectionPenPressure => ("แรงกดปากกา", "PEN PRESSURE"),
        Key::PropMinSize => ("ขนาดต่ำสุด", "Min size"),
        Key::PropMinSizeTip => ("ขนาดเมื่อแตะเบาที่สุด (100% = ไม่ใช้แรงกดปรับขนาด)", "Size at the lightest touch (100% = no size pressure)"),
        Key::PropMinOpacity => ("ความทึบต่ำสุด", "Min opacity"),
        Key::PropMinOpacityTip => ("ความทึบเมื่อแตะเบาที่สุด (100% = ไม่ใช้แรงกดปรับความทึบ)", "Opacity at the lightest touch (100% = no opacity pressure)"),
        Key::SectionAdvanced => ("ขั้นสูง", "Advanced"),
        Key::PropDensity => ("ความหนาแน่น", "Density"),
        Key::PropDensityTip => ("จำนวนจุดแต้มต่อรัศมี: ยิ่งมากยิ่งเนียนแต่ช้าลง", "Dabs per radius: higher is smoother but slower"),
        Key::PropBlending => ("การผสมสี", "Blending"),
        Key::PropBlendingTip => ("ผสมกับสีเดิมที่มีอยู่บนผืนผ้าใบ", "Mix with color already on the canvas"),
        Key::PropPersistence => ("ความคงอยู่ของสี", "Persistence"),
        Key::PropPersistenceTip => ("ระยะเวลาที่สีที่ดูดขึ้นมายังคงอยู่", "How long picked-up color lasts"),
        Key::PropSizeJitter => ("การสุ่มขนาด", "Size jitter"),
        Key::PropEraser => ("ยางลบ", "Eraser"),
        Key::SectionStartEnd => ("หัวเรียว/ท้ายเรียว", "STARTING AND ENDING"),
        Key::PropTaperIn => ("หัวเรียว", "Taper in"),
        Key::PropTaperInTip => ("ใช้การตั้งค่าแรงกดของเครื่องมือย่อยนี้ (ขนาดต่ำสุด / ความทึบต่ำสุด)", "Uses this sub tool's pressure settings (Min size / Min opacity)"),
        Key::PropTaperOut => ("ท้ายเรียว", "Taper out"),
        Key::PropTaperOutTip => ("ปรับใช้เมื่อยกปากกาขึ้น โดยใช้ขนาดต่ำสุด / ความทึบต่ำสุด", "Applied when the pen lifts. Uses Min size / Min opacity"),
        Key::PropPostCorrection => ("ปรับเส้นหลังวาด", "Post correction"),
        Key::PropPostCorrectionTip => ("ปรับเส้นให้เรียบเนียนขึ้นเมื่อยกปากกา โดยสัมพันธ์กับระดับการซูมปัจจุบัน", "Smooths the finished line when the pen lifts, relative to the current zoom"),

        // Property panel hints
        Key::HintEyedropper => ("คลิกหรือลากบนผืนผ้าใบเพื่อดูดสีที่แสดงผล", "Click or drag on the canvas to pick the displayed color."),
        Key::HintHand => ("ลากเพื่อเลื่อนมุมมอง สามารถคลิกเมาส์กลางเพื่อเลื่อนได้ในทุกเครื่องมือ", "Drag to scroll. Middle mouse drags with any tool."),
        Key::HintRotate => ("ลากเพื่อหมุนมุมมอง กด Shift ค้างเพื่อล็อกมุมทีละ 15° (หรือ Ctrl เมื่อใช้ Shift+Space)", "Drag to rotate the view. Hold Shift to snap to 15° (Ctrl with Shift+Space)."),
        Key::HintZoom => ("คลิกเพื่อขยาย Alt+คลิกเพื่อย่อ หรือลากเพื่อซูมอย่างต่อเนื่อง", "Click to zoom in, Alt+click to zoom out, drag to zoom smoothly."),

        // Sub tool panel
        Key::SubToolRestoreDefaults => ("คืนค่าเครื่องมือย่อยเริ่มต้น", "Restore default sub tools"),
        Key::SubToolDuplicate => ("ทำสำเนาเครื่องมือย่อย", "Duplicate sub tool"),
        Key::SubToolDuplicateMenu => ("ทำสำเนา", "Duplicate"),
        Key::SubToolDeleteMenu => ("ลบ", "Delete"),

        // Color & Swatches panel
        Key::ColorSubSwapTip => ("สีรอง: คลิกเพื่อสลับ (X)", "Sub color: click to swap (X)"),
        Key::SectionColorSet => ("ชุดสี", "COLOR SET"),
        Key::ColorRemove => ("ลบ", "Remove"),
        Key::ColorAddMainTip => ("เพิ่มสีหลักลงในชุดสี", "Add main color to set"),
        Key::SectionHistory => ("ประวัติสี", "HISTORY"),
        Key::ColorHistoryEmpty => ("สีที่คุณใช้วาดจะปรากฏที่นี่", "Colors you paint with appear here"),

        // Brush size panel
        Key::BrushSizeSelectTool => ("เลือกเครื่องมือหัวแปรง", "Select a brush tool"),

        // Navigator panel
        Key::NavFlipHorizontal => ("พลิกมุมมองซ้าย-ขวา (F)", "Flip horizontal (F)"),

        // Pen settings panel
        Key::PenSettingsInputDisplay => ("อินพุตและการแสดงผล", "Input & display"),
        Key::PenSettingsMousePressure => ("แรงกดเมาส์", "Mouse pressure"),
        Key::PenSettingsNativePen => ("ปากกาของระบบ", "Native pen"),
        Key::PenSettingsNativePenTip => ("อ่านค่าจาก Windows Ink โดยตรง: รองรับแรงกดเต็มความถี่ การเอียง และท้ายปากกา ปิดใช้งานหากไดรเวอร์มีปัญหา", "Read Windows Ink directly: full-rate pressure, tilt and eraser end. Turn off if your tablet driver misbehaves."),
        Key::PenSettingsEraserEnd => ("ท้ายปากกา", "Eraser end"),
        Key::PenSettingsEraserEndTip => ("กลับด้านปากกาเพื่อสลับไปใช้เครื่องมือท้ายปากกา", "Flipping the pen switches to the eraser end's tool"),
        Key::PenSettingsDisplaySync => ("การซิงค์จอภาพ", "Display sync"),
        Key::PenSettingsLatencyOverlay => ("แสดงข้อมูลความหน่วง", "Latency overlay"),
        Key::PenSettingsLatencyOverlayTip => ("แสดงความถี่ปากกา อายุอินพุต และเวลาเฟรมในแถบสถานะ", "Show pen rate, input age and frame time in the status bar"),
        Key::SyncSmoothTip => ("เข้าคิวสองเฟรม: เฟรมเรตนิ่งที่สุด แต่หน่วงขึ้น", "Queues two frames: steadiest frame rate, more lag"),
        Key::SyncLowLatencyTip => ("เข้าคิวหนึ่งเฟรม (ค่าเริ่มต้น)", "Queues one frame (default)"),
        Key::SyncFastVsyncTip => ("เมลบ็อกซ์: ใช้เฟรมใหม่ล่าสุดเสมอ ภาพไม่ฉีก เฉพาะ DirectX 12", "Mailbox: newest frame wins, no tearing; DirectX 12 only"),
        Key::SyncOffTip => ("ไม่เปิด VSync: ความหน่วงต่ำสุด ภาพอาจฉีกขาด และใช้พลังงานมากขึ้นขณะวาด", "No vsync: lowest lag, may tear, uses more power while drawing"),
        Key::SyncNeedsDx12 => ("ต้องใช้แบ็กเอนด์ DirectX 12", "Needs the DirectX 12 backend"),
        Key::SyncNotAppliedPrefix => ("ยังไม่มีผล: ARTY จะเริ่มทำงานด้วย Low latency แทน", "Not applied yet: ARTY starts with Low latency instead."),
        Key::SyncAfterRestartPrefix => ("จะมีผลเมื่อเปิดโปรแกรม ARTY ใหม่ในครั้งถัดไป", "Takes effect the next time ARTY starts."),
        Key::DisplaySyncSmooth => ("Smooth", "Smooth"),
        Key::DisplaySyncLowLatency => ("Low latency", "Low latency"),
        Key::DisplaySyncFastVsync => ("Fast vsync", "Fast vsync"),
        Key::DisplaySyncOff => ("Off", "Off"),

        // Curve editor
        Key::CurveMaxPoints => ("สูงสุด 16 จุด", "Up to 16 points"),
        Key::CurveInput => ("แรงกดเข้า", "Input"),
        Key::CurveOutput => ("แรงกดที่ใช้", "Output"),
        Key::CurveIn => ("เข้า", "In"),
        Key::CurveOut => ("ออก", "Out"),
        Key::CurveInputPct => ("แรงกดเข้า %", "Input %"),
        Key::CurveOutputPct => ("แรงกดที่ใช้ %", "Output %"),
        Key::CurveReset => ("รีเซ็ต", "Reset"),
        Key::CurveResetTip => ("เส้นตรง: เอาต์พุต = อินพุต", "Straight line: output = input"),
        Key::CurveTestPad => ("พื้นที่ทดสอบเส้น", "Test pad"),

        // Toolbar tooltips
        Key::TooltipPen => ("ปากกา (P)", "Pen (P)"),
        Key::TooltipPencil => ("ดินสอ (N)", "Pencil (N)"),
        Key::TooltipBrush => ("พู่กัน (B)", "Brush (B)"),
        Key::TooltipAirbrush => ("แอร์บรัช (J)", "Airbrush (J)"),
        Key::TooltipBlend => ("เกลี่ยสี (U)", "Blend (U)"),
        Key::TooltipEraser => ("ยางลบ (E)", "Eraser (E)"),
        Key::TooltipMove => ("ย้าย (K)", "Move (K)"),
        Key::TooltipSelect => ("เลือกพื้นที่ (M)", "Selection (M)"),
        Key::TooltipMagicWand => ("ไม้กายสิทธิ์ (W)", "Magic Wand (W)"),
        Key::TooltipFill => ("เทสี (G)", "Fill (G)"),
        Key::TooltipFrameEdit => ("แก้ไขกรอบช่อง (O)", "Frame Edit (O)"),
        Key::TooltipEyedropper => ("หลอดดูดสี (I) · กด Alt ขณะวาด", "Eyedropper (I) · Alt while painting"),
        Key::TooltipHand => ("เลื่อนภาพ (H) · กด Space ค้าง", "Hand (H) · hold Space"),
        Key::TooltipRotate => ("หมุนมุมมอง (R) · Shift+Space", "Rotate (R) · Shift+Space"),
        Key::TooltipZoom => ("ย่อขยาย (Z) · Ctrl+Space, Alt+คลิกเพื่อย่อ", "Zoom (Z) · Ctrl+Space, Alt-click zooms out"),

        // Fill tool
        Key::FillReferTo => ("อ้างอิงจาก", "Refer to"),
        Key::FillRefActive => ("เลเยอร์ที่กำลังแก้ไข", "Editing layer"),
        Key::FillRefAll => ("ทุกเลเยอร์", "All layers"),
        Key::FillRefReference => ("เลเยอร์อ้างอิง", "Reference layers"),
        Key::FillTolerance => ("ความไวสี", "Tolerance"),
        Key::FillCloseGap => ("ปิดช่องว่าง", "Close gap"),
        Key::FillAreaScaling => ("ขยาย/ย่อพื้นที่", "Area scaling"),
        Key::FillToDarkest => ("ถึงพิกเซลที่เข้มที่สุด", "To darkest pixel"),
        Key::FillToDarkestTip => ("ขยายเฉพาะไปยังพิกเซลที่เข้มกว่า และหยุดที่แกนกลางของเส้น", "Grow only towards darker pixels, stopping at the core of the line"),
        Key::FillContiguous => ("เฉพาะพื้นที่ติดกัน", "Contiguous"),
        Key::FillContiguousTip => ("ปิด: เทสีทุกพิกเซลที่ตรงกันบนหน้ากระดาษ", "Off: fill every matching pixel on the page"),
        Key::FillAntialiasing => ("ขอบเรียบ", "Antialiasing"),
        Key::FillOpacity => ("ความทึบ", "Opacity"),
        Key::FillBlend => ("โหมดผสมสี", "Blend"),
        Key::FillBlendBehind => ("ด้านหลัง", "Behind"),
        Key::FillAllLayersNote => ("การเทสีโดยอ้างอิงทุกเลเยอร์จะประมวลผลทั้งหน้ากระดาษ การตั้งเลเยอร์เส้นเป็นเลเยอร์อ้างอิงจะเร็วกว่า", "Filling against all layers composites the page. Marking the line art as a reference layer is faster."),

        // Frame tool
        Key::HintFrameRect => ("ลากเพื่อเพิ่มช่อง กด Shift เพื่อสร้างสี่เหลี่ยมจัตุรัส กด Alt เพื่อเริ่มจากจุดศูนย์กลาง", "Drag to add a panel. Shift: square, Alt: from the centre."),
        Key::HintFrameCut => ("ลากผ่านช่องเพื่อตัดแบ่ง กด Shift เพื่อล็อกมุม 45°", "Drag across panels to divide them. Shift snaps to 45°."),
        Key::HintFrameEdit => ("ลากจุด ขอบ หรือช่อง ดับเบิลคลิกที่ขอบเพื่อเพิ่มจุด ดับเบิลคลิกที่จุดเพื่อลบ กด Shift เพื่อล็อกแนว กด Ctrl เพื่อดูดขอบเกินขอบกระดาษ", "Drag vertices, edges or panels. Double-click an edge to add a vertex, a vertex to remove it. Shift snaps; Ctrl snaps past the canvas edge."),
        Key::SectionPanels => ("กรอบช่อง", "PANELS"),
        Key::FrameGutterLr => ("ระยะห่างช่อง ซ้าย/ขวา", "Gutter left / right"),
        Key::FrameGutterTb => ("ระยะห่างช่อง บน/ล่าง", "Gutter top / bottom"),
        Key::FrameNewBorder => ("ความหนากรอบใหม่", "New border"),
        Key::FrameNewFolderPerFrame => ("สร้างโฟลเดอร์ใหม่ต่อช่อง", "New folder per frame"),
        Key::FrameDivideIntoFolders => ("แยกเป็นโฟลเดอร์ใหม่", "Divide into folders"),
        Key::FrameSnapGuides => ("ดูดติดเส้นบอกแนวหน้ากระดาษ", "Snap to page guides"),
        Key::FrameSnapPanels => ("ดูดติดกรอบช่องอื่น", "Snap to panels"),
        Key::FrameNotInFolder => ("เลเยอร์ที่เลือกไม่ได้อยู่ในโฟลเดอร์กรอบช่อง", "The active layer is not in a frame border folder."),
        Key::SectionFrameBorder => ("กรอบช่อง", "FRAME BORDER"),
        Key::FrameBorder => ("กรอบ", "Border"),
        Key::FrameWidth => ("ความหนากรอบ", "Width"),
        Key::FrameColor => ("สี", "Colour"),
        Key::FramePanelCountSingle => ("1 ช่อง", "1 panel"),
        Key::FramePanelCountPlural => ("ช่อง", "panels"),

        // Page tool & Page setup
        Key::PageCanvasSize => ("ผืนผ้าใบ", "Canvas"),
        Key::PageNoSetup => ("ไม่มีการตั้งค่าหน้ากระดาษ", "No page setup"),
        Key::PagePreset => ("พรีเซ็ต", "Preset"),
        Key::PageUnit => ("หน่วย", "Unit"),
        Key::PageTrimSize => ("ขนาดตัดเจียน", "Trim size"),
        Key::PageTrimPos => ("ตำแหน่งตัดเจียน", "Trim position"),
        Key::PageCentre => ("กึ่งกลาง", "Centre"),
        Key::PageBleed => ("ระยะตัดตก", "Bleed"),
        Key::PageSafeMargin => ("ระยะปลอดภัย", "Safe margin"),
        Key::PageInnerFrame => ("กรอบด้านใน", "Inner frame"),
        Key::PageInnerPos => ("ตำแหน่งกรอบด้านใน", "Inner position"),
        Key::PageCentreTrim => ("กึ่งกลางระยะตัดเจียน", "Centre on trim"),
        Key::PageTrimInsideCanvas => ("ระยะตัดเจียนต้องอยู่ภายในผืนผ้าใบ", "The trim must lie inside the canvas."),
        Key::ExportCropCanvas => ("ผืนผ้าใบ", "Canvas"),
        Key::ExportCropBleed => ("ระยะตัดตก", "Bleed"),
        Key::ExportCropTrim => ("ระยะตัดเจียน (ขนาดสำเร็จ)", "Trim (finished size)"),
        Key::ExportNeedPageSetup => ("ระยะตัดตกและระยะตัดเจียนต้องตั้งค่าหน้ากระดาษก่อน (ไฟล์ > ตั้งค่าหน้ากระดาษ…)", "Bleed and Trim need a page setup (File > Page Setup…)."),
        Key::ExportButton => ("ส่งออก…", "Export…"),
        Key::PageMangaManuscript => ("ต้นฉบับมังงะ", "Manga manuscript"),

        // Selection tool
        Key::SelShapeRect => ("สี่เหลี่ยมผืนผ้า", "Rectangle"),
        Key::SelShapeEllipse => ("วงรี", "Ellipse"),
        Key::SelShapeLasso => ("บ่วงบาศ", "Lasso"),
        Key::SelShapePolygon => ("รูปหลายเหลี่ยม", "Polygon"),
        Key::SelOpNew => ("สร้างใหม่", "New"),
        Key::SelOpNewTip => ("แทนที่พื้นที่ที่เลือก", "Replace the selection"),
        Key::SelOpAdd => ("เพิ่ม", "Add"),
        Key::SelOpAddTip => ("เพิ่มพื้นที่เลือก (กด Shift ค้าง)", "Add to the selection (hold Shift)"),
        Key::SelOpSubtract => ("ลบออก", "Subtract"),
        Key::SelOpSubtractTip => ("ลบออกจากพื้นที่เลือก (กด Alt ค้าง)", "Subtract from the selection (hold Alt)"),
        Key::SelOpIntersect => ("ส่วนทับซ้อน", "Intersect"),
        Key::SelOpIntersectTip => ("เก็บเฉพาะพื้นที่ซ้อนทับ (กด Shift+Alt ค้าง)", "Keep only the overlap (hold Shift+Alt)"),
        Key::SelShape => ("รูปทรง", "Shape"),
        Key::SelMode => ("โหมด", "Mode"),
        Key::FillRefAllVisible => ("ทุกเลเยอร์ที่มองเห็น", "All visible layers"),
        Key::HintSelectWand => ("คลิกเพื่อเลือกพื้นที่สี กด Shift เพื่อเพิ่ม Alt เพื่อลบออก Shift+Alt เพื่อเอาส่วนทับซ้อน", "Click to select a colour region. Shift adds, Alt subtracts, Shift+Alt intersects."),
        Key::HintSelectDrag => ("ลากเพื่อเลือกพื้นที่ กด Shift สำหรับจัตุรัส/วงกลม กด Alt เพื่อเริ่มจากจุดศูนย์กลาง", "Drag to select. Shift: square / circle, Alt: from the centre (after the press)."),
        Key::HintSelectLasso => ("ลากล้อมรอบพื้นที่ที่ต้องการเลือก", "Drag around the area to select."),
        Key::HintSelectPolygon => ("คลิกเพื่อเพิ่มจุด ดับเบิลคลิก กด Enter หรือคลิกจุดแรกเพื่อปิดรูป กด Backspace เพื่อลบจุด", "Click to add points; double-click, Enter or click the first point to close. Backspace removes a point."),
        Key::NoticeSelectionTooDetailed => ("พื้นที่เลือกมีรายละเอียดมากเกินไป: แสดงเส้นขอบเพียงบางส่วน", "Selection too detailed: outline shown in part"),
        Key::SelRadius => ("รัศมี", "Radius"),
        Key::SelAmount => ("ระยะ", "Amount"),
        Key::SelShapeCircle => ("วงกลม", "Circle"),
        Key::SelShapeSquare => ("สี่เหลี่ยม", "Square"),

        // Transform tool
        Key::XfInterpolation => ("วิธีขยายภาพ", "Interpolation"),
        Key::FilterNearest => ("Nearest", "Nearest"),
        Key::FilterBilinear => ("Bilinear", "Bilinear"),
        Key::FilterBicubic => ("Bicubic", "Bicubic"),
        Key::HintMoveTool => ("ลากบนผืนผ้าใบเพื่อย้ายเลเยอร์ปัจจุบัน หรือพิกเซลที่เลือกไว้หากมีพื้นที่เลือก", "Drag on the canvas to move the active layer, or the selected pixels when there is a selection."),
        Key::HintMoveTransform => ("แก้ไข > แปลงรูปทรง (Ctrl+T) เพื่อย่อขยายและหมุน", "Edit > Transform (Ctrl+T) scales and rotates."),
        Key::XfTargetLayer => ("เป้าหมาย: เลเยอร์", "Target: layer"),
        Key::XfTargetSelection => ("เป้าหมาย: พิกเซลที่เลือก", "Target: selected pixels"),
        Key::XfAngle => ("มุม", "Angle"),
        Key::XfCommit => ("ตกลง", "Commit"),

        // Notices & Alerts
        Key::NoticeLayerLocked => ("เลเยอร์ถูกล็อก", "Layer is locked"),
        Key::NoticeLayerHidden => ("เลเยอร์ถูกซ่อนอยู่", "Layer is hidden"),
        Key::NoticeLayerAlphaLocked => ("ล็อกพิกเซลโปร่งใสไว้", "Layer transparency is locked"),
        Key::NoticeSelectRasterToFill => ("เลือกเลเยอร์ราสเตอร์เพื่อเทสี", "Select a raster layer to fill"),
        Key::NoticeSelectRasterToPaint => ("เลือกเลเยอร์ราสเตอร์เพื่อวาด", "Select a raster layer to paint"),
        Key::NoticeNothingSelected => ("ไม่ได้เลือกพื้นที่ใดไว้", "Nothing is selected"),
        Key::NoticeKeepOneSubTool => ("แต่ละเครื่องมือต้องมีอย่างน้อยหนึ่งเครื่องมือย่อย", "Each tool keeps at least one sub tool"),
        Key::NoticeStrokeTooLong => ("เส้นยาวเกินกว่าจะปรับรูปทรงได้ จึงคงรูปเส้นตามที่วาดไว้", "Stroke too long to reshape; kept as drawn"),
        Key::NoticeFrameMaxPanels => ("โฟลเดอร์กรอบช่องนี้มีจำนวนช่องสูงสุดแล้ว", "This frame folder has the most panels it can hold"),
        Key::NoticeSelectFrameToDivide => ("เลือกโฟลเดอร์กรอบช่องที่ต้องการตัดแบ่ง", "Select a frame border folder to divide"),
        Key::NoticeGutterWiderThanPanel => ("ระยะห่างช่องกว้างกว่าขนาดของช่อง", "The gutter is wider than the panel"),
        Key::NoticeDragAcrossPanel => ("ลากผ่านช่องเพื่อตัดแบ่ง", "Drag across a panel to divide it"),
        Key::NoticeFrameKeepOnePanel => ("โฟลเดอร์กรอบช่องต้องมีอย่างน้อยหนึ่งช่อง: หากต้องการให้ลบโฟลเดอร์แทน", "A frame border folder keeps at least one panel: delete the folder instead"),
        Key::NoticeXfFolder => ("ไม่สามารถแปลงรูปทรงโฟลเดอร์ได้", "Folders can't be transformed"),
        Key::NoticeXfEmpty => ("ไม่มีสิ่งใดให้แปลงรูปทรง", "Nothing to transform"),
        Key::NoticeXfSelectRaster => ("เลือกเลเยอร์ราสเตอร์เพื่อแปลงรูปทรง", "Select a raster layer to transform"),

        // Modals in files.rs
        Key::ModalUnsavedTitle => ("บันทึกการเปลี่ยนแปลงหรือไม่?", "Save changes?"),
        Key::ModalUnsavedBody => ("มีการเปลี่ยนแปลงที่ยังไม่ได้บันทึก", "has unsaved changes."),
        Key::ModalSave => ("บันทึก", "Save"),
        Key::ModalDontSave => ("ไม่บันทึก", "Don't Save"),
        Key::ModalLossyTitle => ("บันทึกเป็นไฟล์ใหม่หรือไม่?", "Save as a new file?"),
        Key::ModalLossyBody => ("การบันทึกทับไฟล์เดิมจะทำให้ข้อมูลสูญหาย กรุณาบันทึกเป็นสำเนาแทน", "Saving over the original would lose that data. Save a copy instead."),
        Key::ModalExternalTitle => ("ไฟล์ถูกเปลี่ยนแปลงภายนอก", "File changed on disk"),
        Key::ModalExternalBody => ("ถูกเปลี่ยนแปลงโดยโปรแกรมอื่นตั้งแต่เปิดหรือบันทึกล่าสุด", "was changed by another program since it was opened or saved."),
        Key::ModalOverwrite => ("เขียนทับ", "Overwrite"),
        Key::GpuCanvasUnavailable => ("ผืนผ้าใบ GPU ไม่พร้อมใช้งาน", "GPU canvas unavailable"),
        Key::MsgFilesUnavailable => ("ไม่สามารถเข้าถึงไฟล์ได้", "Files are unavailable"),
        Key::MsgCouldNotSave => ("ไม่สามารถบันทึกได้", "Could not save"),
        Key::MsgCouldNotOpen => ("ไม่สามารถเปิดไฟล์ได้", "Could not open the file"),
        Key::MsgCouldNotRestore => ("ไม่สามารถกู้คืนเอกสารได้", "Could not restore the document"),
        Key::MsgFileError => ("ข้อผิดพลาดของไฟล์", "File error"),
        Key::MsgOpenedWithWarnings => ("เปิดไฟล์พร้อมคำเตือน", "Opened with warnings"),
        Key::ModalFinishingSave => ("กำลังบันทึกให้เสร็จสิ้น…", "Finishing save…"),
        Key::RecoveryTitle => ("กู้คืนงานที่ยังไม่ได้บันทึกหรือไม่?", "Recover unsaved work?"),
        Key::RecoveryBody => ("ARTY ปิดลงโดยไม่ได้บันทึกเอกสารเหล่านี้", "ARTY closed without saving these documents."),
        Key::RecoveryRestore => ("กู้คืน", "Restore"),
        Key::RecoveryDiscard => ("ละทิ้ง", "Discard"),
        Key::RecoveryLater => ("ไว้ทีหลัง", "Later"),
        Key::RecoveryLaterTip => ("เก็บไฟล์เหล่านี้ไว้และถามใหม่ในครั้งถัดไป", "Keep these files and ask again next time"),
        Key::TimeJustNow => ("เมื่อสักครู่", "just now"),
        Key::TimeMinAgo => ("นาทีที่แล้ว", "min ago"),
        Key::TimeHourAgo => ("ชม.ที่แล้ว", "h ago"),
        Key::TimeDaysAgo => ("วันที่แล้ว", "days ago"),

        // Status bar latency tip
        Key::StatusLatencyTip => ("อายุของตัวอย่างปากกาใหม่ล่าสุดเมื่อนำไปใช้งาน (เวลา OS ถึงเฟรม) ไม่รวมการเรนเดอร์และการแสดงผลบนหน้าจอ", "in to frame: age of the newest pen sample when the canvas used it (OS timestamp to frame). It does not include rendering, presenting or the display."),

        // Default brush preset display names
        Key::PresetDisplayGPen => ("G-Pen", "G-Pen"),
        Key::PresetDisplayInkingPen => ("ปากกาตัดเส้น", "Inking Pen"),
        Key::PresetDisplayMappingPen => ("Mapping Pen", "Mapping Pen"),
        Key::PresetDisplayMarker => ("มาร์กเกอร์", "Marker"),
        Key::PresetDisplayPencil => ("ดินสอ", "Pencil"),
        Key::PresetDisplaySketchPencil => ("ดินสอร่างภาพ", "Sketch Pencil"),
        Key::PresetDisplayBrush => ("พู่กัน", "Brush"),
        Key::PresetDisplayWatercolor => ("สีน้ำ", "Watercolor"),
        Key::PresetDisplayFlatColor => ("ลงสีพื้น", "Flat Color"),
        Key::PresetDisplayAirbrush => ("แอร์บรัช", "Airbrush"),
        Key::PresetDisplayBlender => ("เกลี่ยสี", "Blender"),
        Key::PresetDisplayHardEraser => ("ยางลบแข็ง", "Hard Eraser"),
        Key::PresetDisplaySoftEraser => ("ยางลบนุ่ม", "Soft Eraser"),
    }
}

/// Maps arty-core's [`BlendMode`] to its UI localization [`Key`].
pub fn blend_mode_key(mode: BlendMode) -> Key {
    match mode {
        BlendMode::Normal => Key::BlendNormal,
        BlendMode::Multiply => Key::BlendMultiply,
        BlendMode::Screen => Key::BlendScreen,
        BlendMode::Overlay => Key::BlendOverlay,
        BlendMode::Darken => Key::BlendDarken,
        BlendMode::Lighten => Key::BlendLighten,
        BlendMode::ColorDodge => Key::BlendColorDodge,
        BlendMode::ColorBurn => Key::BlendColorBurn,
        BlendMode::LinearBurn => Key::BlendLinearBurn,
        BlendMode::Add => Key::BlendAdd,
        BlendMode::SoftLight => Key::BlendSoftLight,
        BlendMode::HardLight => Key::BlendHardLight,
        BlendMode::Difference => Key::BlendDifference,
        BlendMode::PassThrough => Key::BlendPassThrough,
    }
}

/// Localized display name for a built-in brush preset, or `name` unchanged.
pub fn display_preset_name(name: &str) -> &str {
    match name {
        "G-Pen" => t(Key::PresetDisplayGPen),
        "Inking Pen" => t(Key::PresetDisplayInkingPen),
        "Mapping Pen" => t(Key::PresetDisplayMappingPen),
        "Marker" => t(Key::PresetDisplayMarker),
        "Pencil" => t(Key::PresetDisplayPencil),
        "Sketch Pencil" => t(Key::PresetDisplaySketchPencil),
        "Brush" => t(Key::PresetDisplayBrush),
        "Watercolor" => t(Key::PresetDisplayWatercolor),
        "Flat Color" => t(Key::PresetDisplayFlatColor),
        "Airbrush" => t(Key::PresetDisplayAirbrush),
        "Blender" => t(Key::PresetDisplayBlender),
        "Hard Eraser" => t(Key::PresetDisplayHardEraser),
        "Soft Eraser" => t(Key::PresetDisplaySoftEraser),
        _ => name,
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
