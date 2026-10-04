//! Application model: the open document plus everything the user has
//! selected (tool, brush, colors, view). UI code reads and mutates it only
//! through these methods so history and brush state stay consistent.

use arty_brush::{BrushGroup, BrushPreset, StrokeEngine, StrokeRefused, default_presets};
use arty_core::{CompositeScratch, Document, Edit, History, LayerId, LayerProps, TileCoord, fix15, tile::new_tile_box};
use arty_render::View;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tool {
    Brush(BrushGroup),
    Eyedropper,
    Hand,
    Rotate,
    Zoom,
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Tool::Brush(g) => g.label(),
            Tool::Eyedropper => "Eyedropper",
            Tool::Hand => "Hand",
            Tool::Rotate => "Rotate",
            Tool::Zoom => "Zoom",
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
pub struct InputSettings {
    /// `p' = p^gamma`: >1 needs a firmer press, <1 is lighter.
    pub pressure_gamma: f32,
    /// Pressure used for mouse strokes.
    pub mouse_pressure: f32,
}

impl Default for InputSettings {
    fn default() -> Self {
        Self { pressure_gamma: 1.0, mouse_pressure: 1.0 }
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
    pub color: ColorState,
    pub view: View,
    pub input: InputSettings,
    /// Fit the page to the canvas on the next frame.
    pub fit_pending: bool,
    brush_dirty: bool,
    /// Bumped whenever any preset changes (preview cache key).
    pub preset_rev: u64,
    pub notice: Option<String>,
    scratch: CompositeScratch,
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
            color: ColorState::default(),
            view: View::default(),
            input: InputSettings::default(),
            fit_pending: true,
            brush_dirty: true,
            preset_rev: 0,
            notice: None,
            scratch: CompositeScratch::new(),
        };
        s.select_tool(Tool::Brush(BrushGroup::Pen));
        s
    }

    // ----- tools & presets -----------------------------------------------

    pub fn select_tool(&mut self, tool: Tool) {
        if self.engine.is_stroking() {
            return;
        }
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
        self.preset_rev += 1;
        self.select_preset(i + 1);
    }

    pub fn delete_preset(&mut self, i: usize) {
        let group = self.presets[i].group;
        if self.presets.iter().filter(|p| p.group == group).count() <= 1 {
            self.notice = Some("Each tool keeps at least one sub tool".into());
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

    pub fn reset_presets(&mut self) {
        self.presets = default_presets();
        self.group_memory.clear();
        self.preset_rev += 1;
        let tool = self.tool;
        self.select_tool(tool);
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

    pub fn shape_pressure(&self, raw: f32) -> f32 {
        raw.clamp(0.0, 1.0).powf(self.input.pressure_gamma.clamp(0.2, 5.0))
    }

    pub fn begin_stroke(&mut self, s: arty_brush::InputSample) -> bool {
        if self.brush_dirty {
            let preset = self.presets[self.active_preset].clone();
            self.engine.configure(&preset, self.color.main);
            self.brush_dirty = false;
        }
        match self.engine.begin(&mut self.doc, s) {
            Ok(()) => true,
            Err(why) => {
                self.notice = Some(
                    match why {
                        StrokeRefused::NotRaster => "Select a raster layer to paint",
                        StrokeRefused::Locked => "Layer is locked",
                        StrokeRefused::Hidden => "Layer is hidden",
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
            self.history.push(edit);
            if !self.preset().eraser {
                self.remember_color();
            }
        }
    }

    // ----- history ---------------------------------------------------------

    pub fn undo(&mut self) {
        if !self.engine.is_stroking() {
            self.history.undo(&mut self.doc);
        }
    }

    pub fn redo(&mut self) {
        if !self.engine.is_stroking() {
            self.history.redo(&mut self.doc);
        }
    }

    // ----- layers ----------------------------------------------------------

    /// Run a structural layer edit, recording undo when it changed anything.
    pub fn edit_structure(&mut self, f: impl FnOnce(&mut Document) -> bool) {
        if self.engine.is_stroking() {
            return;
        }
        let snap = self.doc.snapshot_structure();
        if f(&mut self.doc) {
            self.history.push(Edit::Structure(Box::new(snap)));
        }
    }

    pub fn set_layer_props(&mut self, id: LayerId, props: LayerProps, coalesce: bool) {
        if let Some(before) = self.doc.set_props(id, props) {
            self.history.push_props(id, before, coalesce);
        }
    }

    pub fn clear_active_layer(&mut self) {
        let id = self.doc.active();
        let Some(layer) = self.doc.layer(id) else { return };
        if layer.props.locked {
            self.notice = Some("Layer is locked".into());
            return;
        }
        let Some(grid) = layer.raster() else { return };
        let tiles: Vec<_> = grid.iter().map(|(c, t)| (c, Some(t.clone()))).collect();
        if tiles.is_empty() {
            return;
        }
        self.doc.clear_layer(id);
        self.history.push(Edit::Pixels { layer: id, tiles });
    }

    pub fn new_document(&mut self, width: u32, height: u32, dpi: u32) {
        if self.engine.is_stroking() {
            return;
        }
        self.doc = Document::new(width, height, dpi);
        self.history.clear();
        self.fit_pending = true;
    }
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
    fn black_keeps_hue() {
        let mut s = Studio::new(Document::new(64, 64, 72));
        s.set_main_hsv([0.6, 0.8, 0.9]);
        s.set_main_color([0.0; 3]);
        assert!((s.color.hsv[0] - 0.6).abs() < 1e-6);
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
}
