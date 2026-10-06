//! Application model: the open document plus everything the user has
//! selected (tool, brush, colors, view). UI code reads and mutates it only
//! through these methods so history and brush state stay consistent.

use arty_brush::pressure::PressureCurve;
use arty_brush::{BrushGroup, BrushPreset, Reshape, StrokeEngine, StrokeRefused, default_presets};
use arty_core::{
    CompositeScratch, Document, Edit, Frame, History, LayerId, LayerProps, PageSetup, Selection, TileCoord, Touch, fix15,
    selection, Affine64, tile::new_tile_box,
};
use std::collections::HashMap;
use std::sync::Arc;

use arty_pen::PenEnd;
use arty_render::View;
use serde::{Deserialize, Serialize};

use crate::text::{Key, t};
use crate::tools::ToolOptions;
use crate::tools::transform::TransformState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tool {
    Brush(BrushGroup),
    Eyedropper,
    Hand,
    Rotate,
    Zoom,
    /// Rect, ellipse, lasso or polygon: the shape is a tool option, so M
    /// brings back the last one.
    Select,
    MagicWand,
    Fill,
    Move,
    /// Rect, Cut and Edit are separate tools in CSP.
    Frame(FrameMode),
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Tool::Brush(g) => match g {
                BrushGroup::Pen => t(Key::ToolPen),
                BrushGroup::Pencil => t(Key::ToolPencil),
                BrushGroup::Brush => t(Key::ToolBrush),
                BrushGroup::Airbrush => t(Key::ToolAirbrush),
                BrushGroup::Blend => t(Key::ToolBlend),
                BrushGroup::Eraser => t(Key::ToolEraser),
            },
            Tool::Eyedropper => t(Key::ToolEyedropper),
            Tool::Hand => t(Key::ToolHand),
            Tool::Rotate => t(Key::ToolRotate),
            Tool::Zoom => t(Key::ToolZoom),
            Tool::Select => t(Key::ToolSelect),
            Tool::MagicWand => t(Key::ToolMagicWand),
            Tool::Fill => t(Key::ToolFill),
            Tool::Move => t(Key::ToolMove),
            Tool::Frame(m) => m.label(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FrameMode {
    Rect,
    Cut,
    Edit,
}

impl FrameMode {
    pub fn label(self) -> &'static str {
        use crate::text::{Key, t};
        match self {
            FrameMode::Rect => t(Key::FrameModeRect),
            FrameMode::Cut => t(Key::FrameModeCut),
            FrameMode::Edit => t(Key::FrameModeEdit),
        }
    }
}

pub type Rgb = [f32; 3];

pub struct ColorState {
    pub main: Rgb,
    pub sub: Rgb,
    /// Hue/saturation are kept separately so they survive black/white/gray.
    pub hsv: [f32; 3],
    pub recent: Vec<Rgb>,
    pub swatches: Vec<Rgb>,
}

impl Default for ColorState {
    fn default() -> Self {
        let gray = |v: f32| [v, v, v];
        let hex = |h: u32| [((h >> 16) & 255) as f32 / 255.0, ((h >> 8) & 255) as f32 / 255.0, (h & 255) as f32 / 255.0];
        Self {
            main: [0.0; 3],
            sub: [1.0; 3],
            hsv: [0.0, 0.0, 0.0],
            recent: Vec::new(),
            swatches: vec![
                gray(0.0),
                gray(0.2),
                gray(0.4),
                gray(0.6),
                gray(0.8),
                gray(1.0),
                hex(0xffe3d1),
                hex(0xf6c4a8),
                hex(0xe0a07e),
                hex(0xb5735a),
                hex(0x7a4a3a),
                hex(0x3c2a26),
                hex(0xd94b4b),
                hex(0xf28c38),
                hex(0xf2d43d),
                hex(0x6fbf5a),
                hex(0x3f9bd9),
                hex(0x5b5fd9),
                hex(0x9b59c9),
                hex(0xe57ab7),
                hex(0x2b3a67),
                hex(0x6b8fb3),
                hex(0xa9c7d9),
                hex(0xe8eef2),
            ],
        }
    }
}

pub fn rgb_to_hsv(c: Rgb) -> [f32; 3] {
    let hsv = hsv_math::to_hsv(c);
    [hsv.0, hsv.1, hsv.2]
}

pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Rgb {
    hsv_math::from_hsv(h, s, v)
}

/// Small HSV helpers (hue in 0..1).
mod hsv_math {
    pub fn to_hsv(c: [f32; 3]) -> (f32, f32, f32) {
        let max = c[0].max(c[1]).max(c[2]);
        let min = c[0].min(c[1]).min(c[2]);
        let d = max - min;
        let h = if d <= 0.0 {
            0.0
        } else if max == c[0] {
            ((c[1] - c[2]) / d).rem_euclid(6.0) / 6.0
        } else if max == c[1] {
            ((c[2] - c[0]) / d + 2.0) / 6.0
        } else {
            ((c[0] - c[1]) / d + 4.0) / 6.0
        };
        let s = if max <= 0.0 { 0.0 } else { d / max };
        (h, s, max)
    }

    pub fn from_hsv(h: f32, s: f32, v: f32) -> [f32; 3] {
        let h6 = h.rem_euclid(1.0) * 6.0;
        let i = h6.floor();
        let f = h6 - i;
        let p = v * (1.0 - s);
        let q = v * (1.0 - s * f);
        let t = v * (1.0 - s * (1.0 - f));
        match i as i32 {
            0 => [v, t, p],
            1 => [q, v, p],
            2 => [p, v, t],
            3 => [p, q, v],
            4 => [t, p, v],
            _ => [v, p, q],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(from = "InputSettingsRepr")]
pub struct InputSettings {
    /// Maps raw pen pressure to brush pressure (CSP "Adjust pen pressure").
    pub pressure_curve: PressureCurve,
    /// Pressure used for mouse strokes.
    pub mouse_pressure: f32,
    /// Read Windows Ink directly: per-sample pressure, tilt, eraser end, OS timestamps.
    pub native_pen: bool,
    /// Flipping the pen to its eraser end switches to the eraser end's tool (CSP).
    pub eraser_end_switch: bool,
    pub display_sync: DisplaySync,
    /// Show pen rate / input age / frame time in the status bar.
    pub show_latency: bool,
}

impl Default for InputSettings {
    fn default() -> Self {
        Self {
            pressure_curve: PressureCurve::linear(),
            mouse_pressure: 1.0,
            native_pen: true,
            eraser_end_switch: true,
            display_sync: DisplaySync::default(),
            show_latency: false,
        }
    }
}

/// Deserialization view that also accepts saves from before the pressure curve.
///
/// No `Option` fields: eframe stores RON without `implicit_some`, so an old bare
/// value would fail to load, and a failed `input` discards the whole saved state.
/// The rule is unambiguous because new saves never write `pressure_gamma` (it
/// reads as 1.0) and old saves never contain `pressure_curve`.
#[derive(Deserialize)]
#[serde(default)]
struct InputSettingsRepr {
    /// Legacy (≤ v2 @ 0b27b25c). Never written any more.
    pressure_gamma: f32,
    pressure_curve: PressureCurve,
    mouse_pressure: f32,
    native_pen: bool,
    eraser_end_switch: bool,
    display_sync: DisplaySync,
    show_latency: bool,
}

impl Default for InputSettingsRepr {
    fn default() -> Self {
        let d = InputSettings::default();
        Self {
            pressure_gamma: 1.0,
            pressure_curve: d.pressure_curve,
            mouse_pressure: d.mouse_pressure,
            native_pen: d.native_pen,
            eraser_end_switch: d.eraser_end_switch,
            display_sync: d.display_sync,
            show_latency: d.show_latency,
        }
    }
}

impl From<InputSettingsRepr> for InputSettings {
    fn from(r: InputSettingsRepr) -> Self {
        let pressure_curve =
            if (r.pressure_gamma - 1.0).abs() > 1e-6 { PressureCurve::from_gamma(r.pressure_gamma) } else { r.pressure_curve };
        Self {
            pressure_curve,
            mouse_pressure: r.mouse_pressure,
            native_pen: r.native_pen,
            eraser_end_switch: r.eraser_end_switch,
            display_sync: r.display_sync,
            show_latency: r.show_latency,
        }
    }
}

/// How finished frames wait for the display: smoothness against input lag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DisplaySync {
    /// Vsync, two frames queued.
    Smooth,
    /// Vsync, one frame queued (eframe's own default).
    #[default]
    LowLatency,
    /// Mailbox: the newest frame replaces a queued one, no tearing.
    FastVsync,
    /// No vsync: may tear.
    Off,
}

impl DisplaySync {
    pub const ALL: [DisplaySync; 4] = [DisplaySync::Smooth, DisplaySync::LowLatency, DisplaySync::FastVsync, DisplaySync::Off];

    pub fn label(self) -> &'static str {
        match self {
            DisplaySync::Smooth => t(Key::DisplaySyncSmooth),
            DisplaySync::LowLatency => t(Key::DisplaySyncLowLatency),
            DisplaySync::FastVsync => t(Key::DisplaySyncFastVsync),
            DisplaySync::Off => t(Key::DisplaySyncOff),
        }
    }

    /// `fast_vsync_ok`: Mailbox is supported (DX12). Without it, FastVsync falls
    /// back to LowLatency, since configuring an unsupported present mode panics.
    pub fn surface_config(self, fast_vsync_ok: bool) -> egui_wgpu::SurfaceConfig {
        use egui_wgpu::wgpu::PresentMode;
        let (present_mode, latency) = match self {
            DisplaySync::Smooth => (PresentMode::AutoVsync, 2),
            DisplaySync::LowLatency => (PresentMode::AutoVsync, 1),
            DisplaySync::FastVsync if fast_vsync_ok => (PresentMode::Mailbox, 1),
            DisplaySync::FastVsync => return DisplaySync::LowLatency.surface_config(false),
            DisplaySync::Off => (PresentMode::AutoNoVsync, 1),
        };
        egui_wgpu::SurfaceConfig { present_mode, desired_maximum_frame_latency: Some(latency) }
    }

    /// The mode whose surface config is `cfg` (what the painter was started
    /// with), if any. Fast vsync without Mailbox support reads as Low latency.
    pub fn from_surface_config(cfg: egui_wgpu::SurfaceConfig, fast_vsync_ok: bool) -> Option<DisplaySync> {
        DisplaySync::ALL.into_iter().find(|s| s.surface_config(fast_vsync_ok) == cfg)
    }

    /// The `display_sync` value inside eframe's saved state (`app.ron`, where our
    /// settings are an escaped RON string), read before eframe opens the window.
    pub fn from_saved(app_ron: &str) -> Option<DisplaySync> {
        let key = "display_sync:";
        let rest = &app_ron[app_ron.find(key)? + key.len()..];
        let name = &rest[..rest.find(|c: char| !c.is_ascii_alphanumeric()).unwrap_or(rest.len())];
        DisplaySync::ALL.into_iter().find(|s| format!("{s:?}") == name)
    }
}

/// Change counters for caches derived from the document (layer
/// thumbnails). Every value comes from one clock, so a counter that moved
/// is newer than anything recorded before it. A bump means "may have
/// changed": readers confirm against the content before redoing work.
#[derive(Default)]
pub struct ContentEpochs {
    clock: u64,
    pixels: HashMap<LayerId, u64>,
    structure: u64,
    tree: u64,
}

impl ContentEpochs {
    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    /// Pixels of `id` changed (stroke, clear, pixel undo/redo).
    pub fn pixels_changed(&mut self, id: LayerId) {
        let t = self.tick();
        self.pixels.insert(id, t);
    }

    /// Some layer's settings changed.
    pub fn props_changed(&mut self) {
        self.tree = self.tick();
    }

    /// The layer tree was edited or swapped (structure edits and their
    /// undo): layers may have moved, appeared, vanished or changed pixels.
    pub fn structure_changed(&mut self) {
        let t = self.tick();
        self.structure = t;
        self.tree = t;
    }

    pub fn pixels(&self, id: LayerId) -> u64 {
        self.pixels.get(&id).copied().unwrap_or(0)
    }

    pub fn structure(&self) -> u64 {
        self.structure
    }

    /// Last change of any layer's settings or of the tree.
    pub fn tree(&self) -> u64 {
        self.tree
    }
}

pub struct Studio {
    pub doc: Document,
    pub history: History,
    pub engine: StrokeEngine,
    pub presets: Vec<BrushPreset>,
    pub active_preset: usize,
    /// Last preset used per brush group (index into `presets`).
    group_memory: Vec<(BrushGroup, usize)>,
    pub tool: Tool,
    pen_end: PenEnd,
    pen_tools: [Tool; 2],
    pub color: ColorState,
    pub view: View,
    pub input: InputSettings,
    /// The renderer supports [`DisplaySync::FastVsync`] (DX12 backend). Runtime only.
    pub fast_vsync_ok: bool,
    /// Fit the page to the canvas on the next frame.
    pub fit_pending: bool,
    brush_dirty: bool,
    /// Bumped whenever any preset changes (preview cache key).
    pub preset_rev: u64,
    pub notice: Option<String>,
    /// Bumped whenever `doc` is replaced by another document.
    pub doc_epoch: u64,
    scratch: CompositeScratch,
    epochs: ContentEpochs,
    /// Page tool options (saved with the app settings).
    pub opts: ToolOptions,
    /// The free transform in progress; while it exists the canvas sends
    /// input to its handles.
    pub transform: Option<TransformState>,
    /// The panel selected in Frame Edit: (frame folder, panel index).
    pub frame_sel: Option<(LayerId, usize)>,
    /// A frame border drag in progress: the folder and its frame before the
    /// drag. Its previews are in the document but in no step yet.
    pub(crate) frame_preview: Option<(LayerId, Arc<Frame>)>,
    /// The last selection-target commit: the (document epoch, selection
    /// revision) it made and its affine. Until that selection's outline is
    /// extracted, the ants draw the old one moved by it.
    pub ants_carry: Option<((u64, u64), Affine64)>,
}

impl Studio {
    pub fn new(doc: Document) -> Self {
        let presets = default_presets();
        let mut s = Self {
            doc,
            history: History::default(),
            engine: StrokeEngine::new(),
            group_memory: Vec::new(),
            active_preset: 0,
            presets,
            tool: Tool::Brush(BrushGroup::Pen),
            pen_end: PenEnd::Tip,
            pen_tools: [Tool::Brush(BrushGroup::Pen), Tool::Brush(BrushGroup::Eraser)],
            color: ColorState::default(),
            view: View::default(),
            input: InputSettings::default(),
            fast_vsync_ok: false,
            fit_pending: true,
            brush_dirty: true,
            preset_rev: 0,
            notice: None,
            doc_epoch: 0,
            scratch: CompositeScratch::new(),
            epochs: ContentEpochs::default(),
            opts: ToolOptions::default(),
            transform: None,
            frame_sel: None,
            frame_preview: None,
            ants_carry: None,
        };
        s.select_tool(Tool::Brush(BrushGroup::Pen));
        s
    }

    // ----- tools & presets -----------------------------------------------

    pub fn select_tool(&mut self, tool: Tool) {
        if self.engine.is_stroking() {
            return;
        }
        self.commit_transform();
        self.tool = tool;
        if let Tool::Brush(group) = tool {
            let remembered = self.group_memory.iter().find(|(g, _)| *g == group).map(|(_, i)| *i);
            let idx = remembered
                .filter(|&i| self.presets.get(i).is_some_and(|p| p.group == group))
                .or_else(|| self.presets.iter().position(|p| p.group == group));
            if let Some(i) = idx {
                self.select_preset(i);
            }
        }
    }

    pub fn select_preset(&mut self, i: usize) {
        if i >= self.presets.len() || self.engine.is_stroking() {
            return;
        }
        self.active_preset = i;
        let group = self.presets[i].group;
        self.tool = Tool::Brush(group);
        match self.group_memory.iter_mut().find(|(g, _)| *g == group) {
            Some(entry) => entry.1 = i,
            None => self.group_memory.push((group, i)),
        }
        self.brush_dirty = true;
    }

    pub fn pen_end(&self) -> PenEnd {
        self.pen_end
    }

    /// Remember the current tool for the pen end in use, then switch to `end`'s tool.
    /// No-op while stroking or when `end` is already current.
    pub fn switch_pen_end(&mut self, end: PenEnd) {
        if self.engine.is_stroking() || end == self.pen_end {
            return;
        }
        self.pen_tools[self.pen_end as usize] = self.tool;
        self.pen_end = end;
        self.select_tool(self.pen_tools[end as usize]);
    }

    /// Stop tracking pen ends (eraser-end switching or the native pen is
    /// off): the pen counts as its tip again. When the eraser end was in use,
    /// its tool is filed under the eraser end and the tip's tool comes back,
    /// so turning switching back on never loses or swaps either end's tool.
    pub fn reset_pen_end(&mut self) {
        if self.engine.is_stroking() || self.pen_end == PenEnd::Tip {
            return;
        }
        self.pen_tools[PenEnd::Eraser as usize] = self.tool;
        self.pen_end = PenEnd::Tip;
        self.select_tool(self.pen_tools[PenEnd::Tip as usize]);
    }

    pub fn preset(&self) -> &BrushPreset {
        &self.presets[self.active_preset]
    }

    /// Mutable access to the active preset; marks the brush for rebuild.
    pub fn preset_mut(&mut self) -> &mut BrushPreset {
        self.brush_dirty = true;
        self.preset_rev += 1;
        &mut self.presets[self.active_preset]
    }

    pub fn duplicate_preset(&mut self, i: usize) {
        let mut p = self.presets[i].clone();
        p.name.push_str(" copy");
        self.presets.insert(i + 1, p);
        // Presets after `i` moved up one slot; keep remembered indices on them.
        for (_, idx) in &mut self.group_memory {
            if *idx > i {
                *idx += 1;
            }
        }
        self.preset_rev += 1;
        self.select_preset(i + 1);
    }

    pub fn delete_preset(&mut self, i: usize) {
        let group = self.presets[i].group;
        if self.presets.iter().filter(|p| p.group == group).count() <= 1 {
            self.notice = Some(t(Key::NoticeKeepOneSubTool).into());
            return;
        }
        self.presets.remove(i);
        self.group_memory.retain(|(g, _)| *g != group);
        for (_, idx) in &mut self.group_memory {
            if *idx > i {
                *idx -= 1;
            }
        }
        self.preset_rev += 1;
        let next = self.presets.iter().position(|p| p.group == group).unwrap_or(0);
        self.select_preset(next);
    }

    /// Restore the default sub tools. Always leaves `active_preset` valid,
    /// even when a non-brush tool is active (it stays active).
    pub fn reset_presets(&mut self) {
        if self.engine.is_stroking() {
            return;
        }
        let group = match self.tool {
            Tool::Brush(g) => g,
            _ => self.preset().group,
        };
        self.presets = default_presets();
        self.group_memory.clear();
        self.preset_rev += 1;
        let idx = self.presets.iter().position(|p| p.group == group).unwrap_or(0);
        self.active_preset = idx;
        self.group_memory.push((self.presets[idx].group, idx));
        self.brush_dirty = true;
    }

    pub fn nudge_brush_size(&mut self, grow: bool) {
        let p = self.preset_mut();
        let f = if grow { 1.15 } else { 1.0 / 1.15 };
        p.size = (p.size * f).clamp(arty_brush::MIN_BRUSH_SIZE, arty_brush::MAX_BRUSH_SIZE);
    }

    // ----- color -----------------------------------------------------------

    pub fn set_main_color(&mut self, rgb: Rgb) {
        self.color.main = rgb;
        let hsv = rgb_to_hsv(rgb);
        // Keep hue (and saturation) when the new color has none.
        if hsv[1] > 0.0 && hsv[2] > 0.0 {
            self.color.hsv = hsv;
        } else {
            self.color.hsv[2] = hsv[2];
            if hsv[2] > 0.0 {
                self.color.hsv[1] = 0.0;
            }
        }
        self.brush_dirty = true;
    }

    pub fn set_main_hsv(&mut self, hsv: [f32; 3]) {
        self.color.hsv = hsv;
        self.color.main = hsv_to_rgb(hsv[0], hsv[1], hsv[2]);
        self.brush_dirty = true;
    }

    pub fn swap_colors(&mut self) {
        let sub = self.color.sub;
        self.color.sub = self.color.main;
        self.set_main_color(sub);
    }

    fn remember_color(&mut self) {
        let c = self.color.main;
        self.color.recent.retain(|r| r != &c);
        self.color.recent.insert(0, c);
        self.color.recent.truncate(16);
    }

    /// Composite color at a document pixel (straight sRGB), if on the page.
    pub fn sample_color(&mut self, x: f32, y: f32) -> Option<Rgb> {
        let (px, py) = (x.floor() as i32, y.floor() as i32);
        if px < 0 || py < 0 || px >= self.doc.width() as i32 || py >= self.doc.height() as i32 {
            return None;
        }
        let c = TileCoord::from_pixel(px, py);
        let mut tile = new_tile_box();
        self.doc.composite_tile(c, &mut tile, &mut self.scratch);
        let (ox, oy) = c.origin();
        let p = tile[(py - oy) as usize][(px - ox) as usize];
        let a = fix15::to_f32(p[3]);
        if a <= 0.0 {
            return Some([1.0; 3]);
        }
        Some([fix15::to_f32(p[0]) / a, fix15::to_f32(p[1]) / a, fix15::to_f32(p[2]) / a].map(|v| v.clamp(0.0, 1.0)))
    }

    // ----- strokes ---------------------------------------------------------

    /// Raw pen pressure 0..=1 through the user's curve. Mouse pressure is not shaped.
    pub fn shape_pressure(&self, raw: f32) -> f32 {
        self.input.pressure_curve.eval(raw)
    }

    pub fn begin_stroke(&mut self, s: arty_brush::InputSample) -> bool {
        self.commit_transform();
        if self.brush_dirty {
            let preset = self.presets[self.active_preset].clone();
            self.engine.configure(&preset, self.color.main);
            self.brush_dirty = false;
        }
        self.engine.set_view_zoom(self.view.zoom);
        match self.engine.begin(&mut self.doc, s) {
            Ok(()) => true,
            Err(why) => {
                self.notice = Some(
                    match why {
                        StrokeRefused::NotRaster => t(Key::NoticeSelectRasterToPaint),
                        StrokeRefused::Locked => t(Key::NoticeLayerLocked),
                        StrokeRefused::Hidden => t(Key::NoticeLayerHidden),
                        StrokeRefused::AlphaLocked => t(Key::NoticeLayerAlphaLocked),
                    }
                    .into(),
                );
                false
            }
        }
    }

    pub fn feed_stroke(&mut self, s: arty_brush::InputSample) {
        self.engine.feed(&mut self.doc, s);
    }

    pub fn end_stroke(&mut self) {
        if let Some(edit) = self.engine.end(&mut self.doc) {
            if let Edit::Pixels { layer, .. } = &edit {
                self.epochs.pixels_changed(*layer);
            }
            self.history.push(edit, &self.doc);
            if !self.preset().eraser {
                self.remember_color();
            }
        }
        if self.engine.last_reshape() == Reshape::TooLong {
            self.notice = Some(t(Key::NoticeStrokeTooLong).into());
        }
    }

    // ----- history ---------------------------------------------------------

    /// Undo one step. During a transform session this cancels the session
    /// instead (its preview was never recorded). A frame border drag is
    /// recorded first, so this undoes it.
    pub fn undo(&mut self) {
        if self.engine.is_stroking() {
            return;
        }
        if self.transform.is_some() {
            self.cancel_transform();
            return;
        }
        self.commit_frame_preview();
        if self.history.can_undo() {
            let touched = self.history.undo(&mut self.doc);
            self.history_applied(&touched);
        }
    }

    /// Redo one step; nothing during a transform session or a frame border
    /// drag.
    pub fn redo(&mut self) {
        if !self.engine.is_stroking()
            && self.transform.is_none()
            && self.frame_preview.is_none()
            && self.history.can_redo()
        {
            let touched = self.history.redo(&mut self.doc);
            self.history_applied(&touched);
        }
    }

    /// Update the caches derived from the document after history changed
    /// the parts in `touched`.
    pub fn history_applied(&mut self, touched: &[Touch]) {
        for t in touched {
            match *t {
                Touch::Pixels(id) | Touch::Props(id) => {
                    self.epochs.pixels_changed(id);
                    self.epochs.props_changed();
                }
                Touch::Structure => self.epochs.structure_changed(),
                // The ants follow `selection_rev`; page guides are drawn
                // every frame.
                Touch::Selection | Touch::Page => {}
            }
        }
    }

    /// Record an edit already applied to the document as one undo step and
    /// update the caches it affects.
    pub fn record_edit(&mut self, edit: Edit) {
        let mut touched = Vec::new();
        edit.touched(&mut |t| touched.push(t));
        self.history.push(edit, &self.doc);
        self.history_applied(&touched);
    }

    /// Replace the selection as one undo step. Refused while stroking; a
    /// transform session is committed first. No step when nothing changes.
    pub fn set_selection(&mut self, s: Selection) {
        if self.engine.is_stroking() {
            return;
        }
        self.commit_transform();
        let current = self.doc.selection();
        if (current.is_empty() && s.is_empty()) || current.shares_storage(&s) {
            return;
        }
        let old = self.doc.swap_selection(s);
        self.record_edit(Edit::Selection(Box::new(old)));
    }

    /// Replace the page setup as one undo step when it changes. Keeps a
    /// transform session open, as layer settings do (`History::push_page`
    /// may run off a step boundary); undo cancels the session first.
    pub fn set_page_setup(&mut self, s: Option<PageSetup>) {
        if let Some(old) = self.doc.set_page_setup(s) {
            self.history.push_page(old);
        }
    }

    pub fn epochs(&self) -> &ContentEpochs {
        &self.epochs
    }

    // ----- layers ----------------------------------------------------------

    /// Run a structural layer edit, recording undo when it changed anything.
    pub fn edit_structure(&mut self, f: impl FnOnce(&mut Document) -> bool) {
        if self.engine.is_stroking() {
            return;
        }
        self.commit_transform();
        let snap = self.doc.snapshot_structure();
        if f(&mut self.doc) {
            self.history.push(Edit::Structure(Box::new(snap)), &self.doc);
            self.epochs.structure_changed();
        }
    }

    /// Change a layer's settings with undo. `coalesce` merges the change into
    /// the undo step of the gesture in progress (see `History::push_props`);
    /// callers mark the gesture's start and end with
    /// `history.end_props_gesture()`. Applies mid-stroke and keeps a
    /// transform session open (a props push may run off a step boundary).
    pub fn set_layer_props(&mut self, id: LayerId, props: LayerProps, coalesce: bool) {
        if let Some(before) = self.doc.set_props(id, props) {
            self.history.push_props(id, before, coalesce);
            self.epochs.props_changed();
        }
    }

    /// Clear the active layer, or only its selected area when there is a
    /// selection (CSP). Refused while stroking.
    pub fn clear_active_layer(&mut self) {
        if self.engine.is_stroking() {
            return;
        }
        self.commit_transform();
        let id = self.doc.active();
        let Some(layer) = self.doc.layer(id) else { return };
        if layer.props.locked {
            self.notice = Some(t(Key::NoticeLayerLocked).into());
            return;
        }
        if self.doc.has_selection() {
            if let Some(edit) = selection::erase_selected(&mut self.doc, id) {
                self.record_edit(edit);
            }
            return;
        }
        let Some(grid) = layer.raster() else { return };
        let tiles: Vec<_> = grid.iter().map(|(c, t)| (c, Some(t.clone()))).collect();
        if tiles.is_empty() {
            return;
        }
        self.doc.clear_layer(id);
        self.history.push(Edit::Pixels { layer: id, tiles }, &self.doc);
        self.epochs.pixels_changed(id);
    }

    pub fn new_document(&mut self, width: u32, height: u32, dpi: u32) {
        if self.engine.is_stroking() {
            return;
        }
        self.doc = Document::new(width, height, dpi);
        // A session or panel of the old document means nothing here.
        self.transform = None;
        self.frame_sel = None;
        self.frame_preview = None;
        self.history.clear();
        self.epochs.structure_changed();
        self.fit_pending = true;
        self.doc_epoch += 1;
    }

    /// Swap in a loaded document: a stroke in progress is dropped, history
    /// is cleared, and the page is refit and redrawn.
    pub fn replace_document(&mut self, doc: Document) {
        if self.engine.is_stroking() {
            self.engine.cancel(&mut self.doc);
        }
        self.doc = doc;
        self.doc.dirty_mut().mark_all();
        self.transform = None;
        self.frame_sel = None;
        self.frame_preview = None;
        self.history.clear();
        // Layer ids restart per document: thumbnails must not match old ones.
        self.epochs.structure_changed();
        self.fit_pending = true;
        self.doc_epoch += 1;
    }
}

/// Frees dropped undo steps on a background thread ("arty-undo-free"); drops inline if
/// the thread cannot start or has gone. The thread runs below normal priority: at normal
/// priority, woken by a push, it preempted the pushing thread for milliseconds on 4 cores
/// (plans/bench/B015).
pub fn undo_release() -> UndoRelease {
    spawn_undo_free(drop).0
}

/// The hook [`History::set_release`] takes.
pub type UndoRelease = Box<dyn FnMut(Edit) + Send>;

/// [`undo_release`] and its thread, which ends once the hook is dropped.
/// The thread hands each step to `free` (tests note where it ran).
fn spawn_undo_free(
    mut free: impl FnMut(Edit) + Send + 'static,
) -> (UndoRelease, Option<std::thread::JoinHandle<()>>) {
    let (tx, rx) = std::sync::mpsc::channel::<Edit>();
    let thread = std::thread::Builder::new()
        .name("arty-undo-free".into())
        .stack_size(256 * 1024)
        .spawn(move || {
            arty_io::lower_thread_priority();
            for e in rx {
                free(e);
            }
        })
        .ok();
    let ok = thread.is_some();
    // A failed send hands the Edit back inside the error, which drops it here.
    let release = Box::new(move |e| {
        if ok {
            let _ = tx.send(e);
        }
    });
    (release, thread)
}

/// In-memory `eframe::Storage`, so tests go through eframe's real RON encoding.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct MemStorage(pub HashMap<String, String>);

#[cfg(test)]
impl eframe::Storage for MemStorage {
    fn get_string(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }

    fn set_string(&mut self, key: &str, value: String) {
        self.0.insert(key.to_owned(), value);
    }

    fn remove_string(&mut self, key: &str) {
        self.0.remove(key);
    }

    fn flush(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_round_trip() {
        for c in [[0.2, 0.5, 0.9], [1.0, 0.0, 0.0], [0.3, 0.3, 0.3], [0.0, 0.8, 0.1]] {
            let h = rgb_to_hsv(c);
            let back = hsv_to_rgb(h[0], h[1], h[2]);
            for i in 0..3 {
                assert!((back[i] - c[i]).abs() < 1e-5, "{c:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn tool_switch_remembers_sub_tool() {
        let mut s = Studio::new(Document::new(64, 64, 72));
        let mapping = s.presets.iter().position(|p| p.name == "Mapping Pen").unwrap();
        s.select_preset(mapping);
        s.select_tool(Tool::Brush(BrushGroup::Eraser));
        assert!(s.preset().eraser);
        s.select_tool(Tool::Brush(BrushGroup::Pen));
        assert_eq!(s.active_preset, mapping);
    }

    #[test]
    fn reset_presets_with_hand_tool_keeps_valid_preset() {
        let mut s = Studio::new(Document::new(64, 64, 72));
        let last = s.presets.len() - 1;
        let group = s.presets[last].group;
        s.duplicate_preset(last);
        assert_eq!(s.active_preset, last + 1);
        s.select_tool(Tool::Hand);
        s.reset_presets();
        assert_eq!(s.presets.len(), default_presets().len());
        assert!(s.active_preset < s.presets.len());
        assert_eq!(s.preset().group, group);
        assert_eq!(s.tool, Tool::Hand);
        s.select_tool(Tool::Brush(group));
        assert_eq!(s.preset().group, group);
    }

    #[test]
    fn duplicate_preset_shifts_remembered_sub_tools() {
        let mut s = Studio::new(Document::new(64, 64, 72));
        let first_pen = s.presets.iter().position(|p| p.group == BrushGroup::Pen).unwrap();
        // A remembered preset that sits after the duplicated one.
        let pencil = s.presets.iter().rposition(|p| p.group == BrushGroup::Pencil).unwrap();
        assert!(pencil > first_pen);
        assert!(s.presets.iter().filter(|p| p.group == BrushGroup::Pencil).count() > 1);
        let name = s.presets[pencil].name.clone();
        s.select_preset(pencil);
        s.select_tool(Tool::Brush(BrushGroup::Pen));
        s.duplicate_preset(first_pen);
        s.select_tool(Tool::Brush(BrushGroup::Pencil));
        assert_eq!(s.preset().name, name);
    }

    #[test]
    fn black_keeps_hue() {
        let mut s = Studio::new(Document::new(64, 64, 72));
        s.set_main_hsv([0.6, 0.8, 0.9]);
        s.set_main_color([0.0; 3]);
        assert!((s.color.hsv[0] - 0.6).abs() < 1e-6);
    }

    #[test]
    fn big_clears_trim_under_small_budget() {
        let mut s = Studio::new(Document::new(256, 256, 72));
        let id = s.doc.active();
        let layer = 16 * arty_core::TILE_BYTES;
        s.history.set_budget(layer * 5 / 2);
        let value = |s: &Studio| {
            let grid = s.doc.layer(id).unwrap().raster().unwrap();
            (grid.len(), grid.get(TileCoord::new(3, 3)).map(|t| t[5][5]))
        };
        for v in 1..=3 {
            let (grid, _) = s.doc.paint_target(id).unwrap();
            for c in (0..16).map(|i| TileCoord::new(i % 4, i / 4)) {
                grid.get_mut_or_create(c)[5][5] = [v; 4];
            }
            s.clear_active_layer();
        }
        assert_eq!(s.history.undo_len(), 2, "the oldest clear was dropped for the budget");
        assert_eq!(s.history.usage().trimmed, 1);
        s.undo();
        s.undo();
        assert!(!s.history.can_undo());
        assert_eq!(value(&s), (16, Some([2; 4])), "back to before the oldest kept clear");

        s.new_document(64, 64, 72);
        assert_eq!(s.history.budget(), layer * 5 / 2, "a new document keeps the budget");
    }

    #[test]
    fn undo_free_thread_frees_trimmed_steps_and_ends() {
        let mut s = Studio::new(Document::new(256, 256, 72));
        // Where each step is freed: never inline on the pushing thread.
        let threads = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = threads.clone();
        let (release, thread) = spawn_undo_free(move |e| {
            seen.lock().unwrap().push(std::thread::current().name().map(str::to_owned));
            drop(e);
        });
        s.history.set_release(release);
        let thread = thread.expect("the thread starts");
        let id = s.doc.active();
        s.history.set_budget(16 * arty_core::TILE_BYTES * 3 / 2);
        let mut first = Vec::new();
        for v in 1..=3 {
            let (grid, _) = s.doc.paint_target(id).unwrap();
            for c in (0..16).map(|i| TileCoord::new(i % 4, i / 4)) {
                grid.get_mut_or_create(c)[5][5] = [v; 4];
            }
            if v == 1 {
                let grid = s.doc.layer(id).unwrap().raster().unwrap();
                first.extend(grid.iter().map(|(_, t)| std::sync::Arc::downgrade(t)));
            }
            s.clear_active_layer();
        }
        assert_eq!(s.history.usage().trimmed, 2, "the first two clears were dropped");
        let freed = |w: &[std::sync::Weak<_>]| w.iter().all(|t| t.strong_count() == 0);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !freed(&first) && std::time::Instant::now() < until {
            std::thread::yield_now();
        }
        assert!(freed(&first), "the thread freed the dropped tiles");
        drop(s);
        while !thread.is_finished() && std::time::Instant::now() < until {
            std::thread::yield_now();
        }
        assert!(thread.is_finished(), "the thread ends with its History");
        let on = Some("arty-undo-free".to_owned());
        assert_eq!(*threads.lock().unwrap(), [on.clone(), on], "both trimmed steps were freed on the thread");
    }

    #[test]
    fn clear_and_props_mid_stroke() {
        let mut s = Studio::new(Document::new(256, 256, 72));
        let id = s.doc.active();
        let (grid, _) = s.doc.paint_target(id).unwrap();
        grid.get_mut_or_create(TileCoord::new(0, 0))[20][10] = [7; 4];
        let before = s.doc.layer(id).unwrap().raster().unwrap().clone();
        let at = |x: f32| arty_brush::InputSample { x, y: 10.0, pressure: 1.0, ..Default::default() };
        assert!(s.begin_stroke(at(10.0)));
        s.feed_stroke(at(30.0));
        let rev = s.doc.revision();
        s.clear_active_layer();
        assert_eq!((s.history.undo_len(), s.doc.revision()), (0, rev), "a clear waits for the stroke");
        // A rename (or any layer setting) applies at once.
        let props = LayerProps { name: "inks".into(), ..s.doc.layer(id).unwrap().props.clone() };
        s.set_layer_props(id, props, false);
        s.feed_stroke(at(40.0));
        s.end_stroke();
        assert_eq!((s.doc.layer(id).unwrap().props.name.as_str(), s.history.undo_len()), ("inks", 2));
        while s.history.can_undo() {
            s.undo();
        }
        let layer = s.doc.layer(id).unwrap();
        assert!(layer.raster().unwrap().get(TileCoord::new(0, 0)) == before.get(TileCoord::new(0, 0)));
        assert_ne!(layer.props.name, "inks");
    }

    #[test]
    fn content_epochs_follow_edits() {
        let mut s = Studio::new(Document::new(64, 64, 72));
        let id = s.doc.active();
        let e = |s: &Studio| (s.epochs().pixels(id), s.epochs().structure(), s.epochs().tree());

        s.doc.paint_target(id).unwrap().0.get_mut_or_create(TileCoord::new(0, 0))[0][0] = [1, 1, 1, 1];
        let before = e(&s);
        s.clear_active_layer();
        let cleared = e(&s);
        assert!(cleared.0 > before.0 && cleared.1 == before.1 && cleared.2 == before.2, "clear bumps only pixels");

        let mut p = s.doc.layer(id).unwrap().props.clone();
        p.opacity = 0.5;
        s.set_layer_props(id, p, false);
        let props = e(&s);
        assert!(props.0 == cleared.0 && props.1 == cleared.1 && props.2 > cleared.2, "props bump only the tree");

        s.edit_structure(|d| {
            d.add_raster_layer().unwrap();
            true
        });
        let added = e(&s);
        assert!(added.0 == props.0 && added.1 > props.1 && added.2 > props.2, "structure bumps structure and tree");

        s.undo(); // structure
        let undone = e(&s);
        assert!(undone.1 > added.1);
        s.undo(); // props on `id`
        s.undo(); // pixels on `id`
        assert!(e(&s).0 > undone.0);
        assert!(!s.history.can_undo());
        let settled = e(&s);
        s.undo(); // nothing left: no bump
        assert_eq!(e(&s), settled);
    }

    fn load_input(ron: &str) -> InputSettings {
        let mut st = MemStorage::default();
        eframe::Storage::set_string(&mut st, "input", ron.to_owned());
        eframe::get_value(&st, "input").expect("input settings load")
    }

    #[test]
    fn input_settings_ron_round_trip() {
        let custom = InputSettings {
            pressure_curve: PressureCurve::from_points(&[[0.0, 0.05], [0.3, 0.1], [0.7, 0.8], [1.0, 1.0]]),
            mouse_pressure: 0.6,
            native_pen: false,
            eraser_end_switch: false,
            display_sync: DisplaySync::Off,
            show_latency: true,
        };
        for s in [InputSettings::default(), custom] {
            let mut st = MemStorage::default();
            eframe::set_value(&mut st, "input", &s);
            let text = st.0["input"].clone();
            assert!(!text.contains("pressure_gamma"), "legacy field written: {text}");
            assert_eq!(eframe::get_value::<InputSettings>(&st, "input"), Some(s), "{text}");
        }
        // A save with only some of the fields fills the rest from the defaults.
        let partial = load_input("(display_sync:Smooth)");
        assert_eq!(partial, InputSettings { display_sync: DisplaySync::Smooth, ..Default::default() });
    }

    #[test]
    fn legacy_gamma_save_migrates() {
        let s = load_input("(pressure_gamma:1.5,mouse_pressure:0.8)");
        assert_eq!(s.mouse_pressure, 0.8);
        assert_eq!(s.pressure_curve, PressureCurve::from_gamma(1.5));
        for i in 0..=1000 {
            let x = i as f32 / 1000.0;
            let err = (s.pressure_curve.eval_exact(x) - x.powf(1.5)).abs();
            assert!(err <= 0.003, "x = {x}: off x^1.5 by {err}");
        }
        let d = InputSettings::default();
        assert_eq!((s.native_pen, s.eraser_end_switch, s.display_sync, s.show_latency), (d.native_pen, d.eraser_end_switch, d.display_sync, d.show_latency));

        let s = load_input("(pressure_gamma:1.0,mouse_pressure:1.0)");
        assert!(s.pressure_curve.is_linear());
        assert_eq!(s, InputSettings::default());
    }

    #[test]
    fn shape_pressure_uses_curve() {
        let mut s = Studio::new(Document::new(64, 64, 72));
        // Default: linear, bit-identical to the old gamma 1.0 path (`p.powf(1.0)`).
        for i in 0..=4096 {
            let p = i as f32 / 4096.0;
            assert_eq!(s.shape_pressure(p), p.clamp(0.0, 1.0).powf(1.0));
        }
        assert_eq!(s.shape_pressure(-0.5), 0.0);
        assert_eq!(s.shape_pressure(1.5), 1.0);
        assert_eq!(s.shape_pressure(f32::NAN), 0.0);

        s.input.pressure_curve = PressureCurve::from_points(&[[0.0, 0.0], [0.5, 0.2], [1.0, 1.0]]);
        assert!((s.shape_pressure(0.5) - 0.2).abs() <= 2e-3);
        for i in 0..=100 {
            let p = i as f32 / 100.0;
            assert_eq!(s.shape_pressure(p), s.input.pressure_curve.eval(p));
        }
    }

    #[test]
    fn display_sync_surface_configs() {
        use egui_wgpu::wgpu::PresentMode;
        let cfg = |m, l| egui_wgpu::SurfaceConfig { present_mode: m, desired_maximum_frame_latency: Some(l) };
        for ok in [false, true] {
            assert_eq!(DisplaySync::Smooth.surface_config(ok), cfg(PresentMode::AutoVsync, 2));
            assert_eq!(DisplaySync::LowLatency.surface_config(ok), cfg(PresentMode::AutoVsync, 1));
            assert_eq!(DisplaySync::Off.surface_config(ok), cfg(PresentMode::AutoNoVsync, 1));
        }
        assert_eq!(DisplaySync::FastVsync.surface_config(true), cfg(PresentMode::Mailbox, 1));
        assert_eq!(DisplaySync::FastVsync.surface_config(false), DisplaySync::LowLatency.surface_config(false));
        // The default is exactly what eframe used before the setting existed.
        assert_eq!(InputSettings::default().display_sync, DisplaySync::LowLatency);
        assert_eq!(DisplaySync::LowLatency.surface_config(false), egui_wgpu::SurfaceConfig::LOW_LATENCY);
    }

    #[test]
    fn display_sync_reads_from_saved_state() {
        for sync in DisplaySync::ALL {
            let mut st = MemStorage::default();
            eframe::set_value(&mut st, "input", &InputSettings { display_sync: sync, ..Default::default() });
            // eframe's app.ron holds each value as an escaped string.
            let app_ron = format!("{{\"arty-v2\":{:?}}}", format!("(theme:Dark,input:{})", st.0["input"]));
            assert_eq!(DisplaySync::from_saved(&app_ron), Some(sync), "{app_ron}");
        }
        assert_eq!(DisplaySync::from_saved(""), None);
        assert_eq!(DisplaySync::from_saved("(input:(pressure_gamma:1.5))"), None);
        assert_eq!(DisplaySync::from_saved("display_sync:Sometimes"), None);
        assert_eq!(DisplaySync::from_saved("display_sync:"), None);
    }

    #[test]
    fn clear_layer_is_undoable() {
        let mut s = Studio::new(Document::new(64, 64, 72));
        let id = s.doc.active();
        s.doc.paint_target(id).unwrap().0.get_mut_or_create(TileCoord::new(0, 0))[0][0] = [1, 1, 1, 1];
        s.clear_active_layer();
        assert!(s.doc.active_layer().raster().unwrap().is_empty());
        s.undo();
        assert!(!s.doc.active_layer().raster().unwrap().is_empty());
    }

    fn full_tile_selection(x: i32) -> Selection {
        let mut sel = Selection::new();
        sel.insert_tile(TileCoord::new(x, 0), arty_core::selection::full_mask().clone());
        sel
    }

    #[test]
    fn set_selection_is_one_step_and_skips_no_ops() {
        let mut s = Studio::new(Document::new(256, 64, 72));
        let none = s.doc.selection().clone();
        s.set_selection(Selection::new());
        assert_eq!(s.history.undo_len(), 0, "empty to empty is no step");

        let a = full_tile_selection(1);
        let rev = s.doc.revision();
        s.set_selection(a.clone());
        assert_eq!(s.history.undo_len(), 1);
        assert!(s.doc.selection().shares_storage(&a));
        assert_ne!(s.doc.revision(), rev, "a selection change is a document change");
        s.set_selection(a.clone());
        assert_eq!(s.history.undo_len(), 1, "the same selection is no step");

        s.undo();
        assert!(s.doc.selection().shares_storage(&none));
        s.redo();
        assert!(s.doc.selection().shares_storage(&a));

        // Refused while stroking.
        let smp = arty_brush::InputSample { x: 10.0, y: 10.0, pressure: 1.0, ..Default::default() };
        assert!(s.begin_stroke(smp));
        s.set_selection(full_tile_selection(2));
        assert!(s.doc.selection().shares_storage(&a));
        s.end_stroke();
    }

    #[test]
    fn set_page_setup_is_one_step() {
        let mut s = Studio::new(Document::new(64, 64, 72));
        let trim = arty_core::RectF { x: 2.0, y: 2.0, w: 60.0, h: 60.0 };
        let page = PageSetup { trim, bleed: 2.0, safe: 4.0, inner: arty_core::RectF::default(), unit: 2 };
        s.set_page_setup(Some(page));
        s.set_page_setup(Some(page));
        assert_eq!(s.history.undo_len(), 1);
        s.undo();
        assert_eq!(s.doc.page_setup(), None);
        s.redo();
        assert_eq!(s.doc.page_setup(), Some(&page));
    }

    #[test]
    fn record_edit_marks_what_the_edit_touched() {
        let mut s = Studio::new(Document::new(64, 64, 72));
        let id = s.doc.active();
        let c = TileCoord::new(0, 0);
        let (grid, _) = s.doc.paint_target(id).unwrap();
        let old = grid.get_ref(c).cloned();
        grid.get_mut_or_create(c)[0][0] = [1, 1, 1, 1];
        let before = s.epochs().pixels(id);
        s.record_edit(Edit::Pixels { layer: id, tiles: vec![(c, old)] });
        assert!(s.epochs().pixels(id) > before);
        assert_eq!(s.history.undo_len(), 1);
        s.frame_sel = Some((id, 0));
        s.new_document(64, 64, 72);
        assert_eq!(s.frame_sel, None, "a new document has no selected panel");
    }
}
