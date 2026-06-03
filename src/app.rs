use crate::canvas::{BlendMode, Layer};
use crate::history::{HistoryManager, TileSnapshot, UndoCommand, StrokeSurface};
use crate::input::{InputManager, StrokeStabilizer, TabletAxisState, StabilizerLevel, StabilizerMode};
use crate::renderer::WgpuRenderer;
use crate::commands::CommandId;
use crate::shortcuts::ShortcutManager;
use crate::tools::{selection, fill, transform as transform_tool};

use ahash::AHashMap;
use egui::{Color32, Pos2, Rect, Vec2, Visuals};
use hokusai::mapping::SettingValue;
use hokusai::{Brush, BrushSetting, BrushState, TiledSurface};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresetIcon {
    Pencil,
    InkPen,
    PaintBrush,
    Smudge,
    Eraser,
}

#[derive(Debug, Clone)]
pub struct BrushPreset {
    pub id: u64,
    pub name: String,
    pub icon: PresetIcon,
    pub radius_log: f32,
    pub opacity: f32,
    pub hardness: f32,
    pub min_size_fraction: f32, // Min size % (0.0 to 1.0)
    pub color_blending: f32,    // Smudge setting (0.0 to 1.0)
    pub dilution: f32,          // Smudge length setting (0.0 to 1.0)
    pub is_eraser: bool,
    pub texture_id: u8,
    pub texture_scale: f32,
    pub bristle_id: u8,
}

pub struct PaintApp {
    // Canvas layers and active index
    active_layer_id: u32,
    layers: AHashMap<u32, Layer>,
    layer_order: Vec<u32>,
    layer_id_counter: u32,

    // Brush presets and active selection
    active_preset_index: usize,
    presets: Vec<BrushPreset>,
    preset_id_counter: u64,
    brushes: Vec<Brush>,
    brush_states: Vec<BrushState>,
    brush_color: [f32; 3], // RGB float [0.0, 1.0]
    palette: Vec<[f32; 3]>,
    selected_palette_index: Option<usize>,

    // Sliders bound to the active brush
    brush_radius_log: f32, // Logarithmic radius
    brush_opacity: f32,
    brush_hardness: f32,
    brush_min_size_fraction: f32,
    brush_color_blending: f32,
    brush_dilution: f32,
    pressure_curve: f32,
    pressure_min: f32,

    // Renaming brush preset state
    renaming_preset_index: Option<usize>,
    rename_input: String,

    // Input and stabilization
    input_manager: Option<InputManager>,
    tablet_axis: TabletAxisState,
    egui_touch_pressure: Option<f32>,
    egui_touch_active: bool,
    stabilizer: StrokeStabilizer,
    last_ptr_pos: Option<Pos2>,
    last_ptr_pressure: f32,
    last_event_time: f64, // Used for stroke dtime tracking

    // Viewport transforms (infinite canvas navigation)
    viewport_offset: Vec2,
    viewport_zoom: f32,
    mirror_horizontal: bool,
    rotation_angle: f32, // in radians

    // Canvas dimensions
    pub canvas_width: u32,
    pub canvas_height: u32,

    // New Canvas Dialog State
    show_new_canvas_dialog: bool,
    new_canvas_width: u32,
    new_canvas_height: u32,

    // Undo/Redo history
    history: HistoryManager,
    current_stroke_snapshots: Vec<TileSnapshot>,
    dragging_layer_id: Option<u32>,
    stroke_id: u32,

    // Customization/masking fields
    lock_canvas_bounds: bool,
    selection_mask: crate::canvas::SelectionMask,
    brush_textures: Vec<Vec<u8>>,

    save_sender: std::sync::mpsc::Sender<crate::save::SaveTask>,
    current_vector_points: Vec<crate::canvas::VectorControlPoint>,
    document_path: String,
    brush_import_path: String,
    brush_texture_id: u8,
    brush_texture_scale: f32,
    brush_bristle_id: u8,

    /// Set to true whenever any brush slider/color/preset changes, so sync_brush_settings
    /// is only flushed into the Hokusai engine when genuinely needed (not every frame).
    brush_settings_dirty: bool,

    // GPU rendering engine
    renderer: Option<WgpuRenderer>,

    // Command dispatcher + shortcut system
    pub shortcuts: ShortcutManager,

    // Active tool
    pub active_tool: ToolId,

    // Fill tool state
    pub fill_options: fill::FillOptions,

    // Selection state
    pub selection_mode: selection::SelectionMode,
    pub selection_rect: Option<selection::SelectionRect>,
    pub lasso_points: Option<selection::LassoPoints>,
    pub is_selecting: bool,
    pub show_selection_overlay: bool,
    pub selection_feather: f32,

    // Transform state
    #[allow(unused)]
    pub transform_state: transform_tool::TransformState,

    // Export dialog
    pub show_export_png_dialog: bool,
    pub export_png_options: crate::export::png::ExportPngOptions,
    pub export_png_path: String,

    // Autosave
    pub autosave_enabled: bool,
    pub autosave_interval_secs: f64,
    pub autosave_path: String,
    pub last_autosave_time: f64,
    pub document_modified: bool,
    pub autosave_status: String,

    // UI state
    pub show_minimal_ui: bool,
    pub show_grid: bool,
    pub show_symmetry: bool,
    pub quick_bar_visible: bool,

    // Color history
    #[allow(unused)]
    pub color_history: Vec<[f32; 3]>,
    #[allow(unused)]
    pub color_history_max: usize,

    // Layer operations
    #[allow(unused)]
    pub show_layer_properties: bool,

    // Shortcut editor state
    pub show_shortcut_editor: bool,
    pub shortcut_search: String,
    pub shortcut_edit_idx: Option<usize>,
    pub shortcut_listen_idx: Option<usize>,
}

// Tool ID enum used in app
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ToolId {
    Brush, Eraser, Fill, Gradient,
    RectSelect, EllipseSelect, Lasso, PolygonLasso,
    MagicWand, Move, Transform, ColorPicker,
    Hand, Zoom, RotateView, Line, Shape, Reference,
}

impl ToolId {
    pub fn name(&self) -> &'static str {
        use ToolId::*;
        match self {
            Brush => "Brush", Eraser => "Eraser", Fill => "Fill",
            Gradient => "Gradient", RectSelect => "Rect Select",
            EllipseSelect => "Ellipse Select", Lasso => "Lasso",
            PolygonLasso => "Polygon Lasso", MagicWand => "Magic Wand",
            Move => "Move", Transform => "Transform", ColorPicker => "Color Picker",
            Hand => "Hand", Zoom => "Zoom", RotateView => "Rotate View",
            Line => "Line", Shape => "Shape", Reference => "Reference",
        }
    }
}

impl PaintApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Apply Paint Tool SAI clean, crisp light grey theme
        let mut visuals = Visuals::light();
        visuals.panel_fill = Color32::from_rgb(240, 240, 240); // Soft grey panels
        visuals.window_fill = Color32::from_rgb(245, 245, 245);
        visuals.widgets.active.bg_fill = Color32::from_rgb(180, 200, 240); // Light blue selection
        visuals.widgets.hovered.bg_fill = Color32::from_rgb(215, 225, 250);
        visuals.widgets.inactive.bg_fill = Color32::from_rgb(230, 230, 230);
        cc.egui_ctx.set_visuals(visuals);

        // Initialize brush presets programmatically
        let mut brushes = Vec::new();
        let mut brush_states = Vec::new();
        let default_palette = vec![
            [0.02, 0.02, 0.02],
            [1.0, 1.0, 1.0],
            [0.85, 0.08, 0.08],
            [1.0, 0.45, 0.08],
            [1.0, 0.86, 0.12],
            [0.1, 0.65, 0.22],
            [0.0, 0.48, 0.9],
            [0.35, 0.2, 0.85],
            [0.95, 0.25, 0.65],
            [0.45, 0.28, 0.16],
            [0.55, 0.55, 0.55],
            [0.08, 0.14, 0.22],
        ];

        // 1. Pencil Preset
        let mut pencil = Brush::new();
        Self::set_constant(&mut pencil, BrushSetting::Radius, 1.0); // radius = exp(1.0) = 2.7px
        Self::set_constant(&mut pencil, BrushSetting::Opaque, 0.95);
        Self::set_constant(&mut pencil, BrushSetting::Hardness, 0.95);
        Self::set_constant(&mut pencil, BrushSetting::DabsPerActualRadius, 2.0);
        // Opacity: very transparent at light touch, nearly opaque at medium, full at heavy
        Self::set_pressure_mapping(
            &mut pencil,
            BrushSetting::Opaque,
            0.95,
            vec![(0.0, -0.90), (0.15, -0.60), (0.35, -0.30), (0.55, -0.10), (0.80, -0.03), (1.0, 0.0)],
        );
        // Radius: very slight size change for natural pencil feel
        Self::set_pressure_mapping(
            &mut pencil,
            BrushSetting::Radius,
            1.0,
            vec![(0.0, -0.20), (0.30, -0.10), (0.60, -0.04), (1.0, 0.0)],
        );
        // OpaqueMultiply: S-curve for natural pressure response
        Self::set_pressure_mapping(
            &mut pencil,
            BrushSetting::OpaqueMultiply,
            0.0,
            vec![(0.0, 0.0), (0.3, 0.55), (0.6, 0.85), (1.0, 1.0)],
        );
        brushes.push(pencil);
        brush_states.push(BrushState::default());

        // 2. Ink Pen Preset
        let mut pen = Brush::new();
        Self::set_constant(&mut pen, BrushSetting::Radius, 1.6); // exp(1.6) = 4.95px
        Self::set_constant(&mut pen, BrushSetting::Opaque, 1.0);
        Self::set_constant(&mut pen, BrushSetting::Hardness, 0.88);
        // Ink Pen: more dabs when pressing hard for smoother thick lines
        Self::set_constant(&mut pen, BrushSetting::DabsPerActualRadius, 2.5);
        // Radius: dramatic thin-to-thick range for expressive inking
        Self::set_pressure_mapping(
            &mut pen,
            BrushSetting::Radius,
            1.6,
            vec![(0.0, -0.80), (0.10, -0.60), (0.25, -0.40), (0.45, -0.22), (0.70, -0.08), (0.90, -0.02), (1.0, 0.0)],
        );
        // Opacity: nearly constant — ink is ink
        Self::set_pressure_mapping(
            &mut pen,
            BrushSetting::Opaque,
            1.0,
            vec![(0.0, -0.15), (0.20, -0.05), (0.50, 0.0), (1.0, 0.0)],
        );
        // OpaqueMultiply: S-curve for natural pressure response
        Self::set_pressure_mapping(
            &mut pen,
            BrushSetting::OpaqueMultiply,
            0.0,
            vec![(0.0, 0.0), (0.3, 0.55), (0.6, 0.85), (1.0, 1.0)],
        );
        brushes.push(pen);
        brush_states.push(BrushState::default());

        // 3. Paint Brush Preset (soft blendy)
        let mut brush = Brush::new();
        Self::set_constant(&mut brush, BrushSetting::Radius, 2.2); // exp(2.2) = 9.0px
        Self::set_constant(&mut brush, BrushSetting::Opaque, 0.8);
        Self::set_constant(&mut brush, BrushSetting::Hardness, 0.5);
        // Paint Brush: denser dabs for better blending
        Self::set_constant(&mut brush, BrushSetting::DabsPerActualRadius, 3.0);
        // Radius: smooth organic size range
        Self::set_pressure_mapping(
            &mut brush,
            BrushSetting::Radius,
            2.2,
            vec![(0.0, -0.70), (0.10, -0.50), (0.25, -0.35), (0.45, -0.20), (0.65, -0.10), (0.85, -0.03), (1.0, 0.0)],
        );
        // Opacity: soft buildup
        Self::set_pressure_mapping(
            &mut brush,
            BrushSetting::Opaque,
            0.8,
            vec![(0.0, -0.70), (0.15, -0.45), (0.35, -0.25), (0.55, -0.12), (0.80, -0.03), (1.0, 0.0)],
        );
        // OpaqueMultiply: S-curve for natural pressure response
        Self::set_pressure_mapping(
            &mut brush,
            BrushSetting::OpaqueMultiply,
            0.0,
            vec![(0.0, 0.0), (0.3, 0.55), (0.6, 0.85), (1.0, 1.0)],
        );
        brushes.push(brush);
        brush_states.push(BrushState::default());

        // 4. Smudge Preset
        let mut smudge = Brush::new();
        Self::set_constant(&mut smudge, BrushSetting::Radius, 2.0);
        Self::set_constant(&mut smudge, BrushSetting::Opaque, 0.4);
        Self::set_constant(&mut smudge, BrushSetting::Hardness, 0.4);
        Self::set_constant(&mut smudge, BrushSetting::Smudge, 0.85);
        Self::set_constant(&mut smudge, BrushSetting::SmudgeLength, 0.8);
        Self::set_constant(&mut smudge, BrushSetting::DabsPerActualRadius, 2.0);
        // Radius: pressure controls smudge area
        Self::set_pressure_mapping(
            &mut smudge,
            BrushSetting::Radius,
            2.0,
            vec![(0.0, -0.40), (0.30, -0.20), (0.60, -0.08), (1.0, 0.0)],
        );
        // Opacity: pressure controls blending strength
        Self::set_pressure_mapping(
            &mut smudge,
            BrushSetting::Opaque,
            0.4,
            vec![(0.0, -0.30), (0.40, -0.12), (0.70, -0.04), (1.0, 0.0)],
        );
        // OpaqueMultiply: smudge still responds to pressure
        Self::set_pressure_mapping(
            &mut smudge,
            BrushSetting::OpaqueMultiply,
            0.0,
            vec![(0.0, 0.0), (0.3, 0.55), (0.6, 0.85), (1.0, 1.0)],
        );
        brushes.push(smudge);
        brush_states.push(BrushState::default());

        // 5. Eraser Preset
        let mut eraser = Brush::new();
        Self::set_constant(&mut eraser, BrushSetting::Radius, 2.5); // exp(2.5) = 12.18px
        Self::set_constant(&mut eraser, BrushSetting::Opaque, 1.0);
        Self::set_constant(&mut eraser, BrushSetting::Hardness, 0.8);
        Self::set_constant(&mut eraser, BrushSetting::Eraser, 1.0); // Enables ERASER mode
        Self::set_constant(&mut eraser, BrushSetting::DabsPerActualRadius, 2.0);
        // Radius: light pressure for detail erasing, heavy for broad strokes
        Self::set_pressure_mapping(
            &mut eraser,
            BrushSetting::Radius,
            2.5,
            vec![(0.0, -0.50), (0.25, -0.30), (0.50, -0.15), (0.80, -0.04), (1.0, 0.0)],
        );
        // Opacity: gradual erasing at light touch
        Self::set_pressure_mapping(
            &mut eraser,
            BrushSetting::Opaque,
            1.0,
            vec![(0.0, -0.60), (0.20, -0.35), (0.45, -0.15), (0.75, -0.04), (1.0, 0.0)],
        );
        // OpaqueMultiply: eraser strength tracks pressure
        Self::set_pressure_mapping(
            &mut eraser,
            BrushSetting::OpaqueMultiply,
            0.0,
            vec![(0.0, 0.0), (0.3, 0.55), (0.6, 0.85), (1.0, 1.0)],
        );
        brushes.push(eraser);
        brush_states.push(BrushState::default());

        // Create initial default Layer
        let mut layers = AHashMap::default();
        let default_layer = Layer::new(1, "Layer 1".to_string());
        layers.insert(1, default_layer);

        // Fetch WGPU state
        let renderer = WgpuRenderer::new(cc);

        // Prefer egui/winit's WM_POINTER pen pressure path. It reports pen
        // pressure through egui::Event::Touch::force without claiming
        // RealTimeStylus, which can freeze some Windows tablet drivers.
        let input_manager = if std::env::var_os("XCALUX_ENABLE_REALTIME_STYLUS").is_some() {
            unsafe {
                match InputManager::new(cc) {
                    Ok(mgr) => {
                        log::info!(
                            "[PaintApp] RealTimeStylus InputManager initialized successfully"
                        );
                        Some(mgr)
                    }
                    Err(e) => {
                        log::warn!(
                            "[PaintApp] RealTimeStylus init failed (winit pressure/fallback input remains active): {}",
                            e
                        );
                        None
                    }
                }
            }
        } else {
            log::info!(
                "[PaintApp] Using winit pen pressure; set XCALUX_ENABLE_REALTIME_STYLUS=1 for octotablet fallback"
            );
            None
        };

        // Tighten egui style layout for maximum screen real estate
        let mut style = (*cc.egui_ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(4.0, 4.0);
        style.spacing.button_padding = egui::vec2(4.0, 2.0);
        cc.egui_ctx.set_style(style);

        let initial_radius = brushes[0].get(BrushSetting::Radius).base_value;
        let initial_opacity = brushes[0].get(BrushSetting::Opaque).base_value;
        let initial_hardness = brushes[0].get(BrushSetting::Hardness).base_value;

        let presets = vec![
            BrushPreset {
                id: 1,
                name: "Pencil".to_string(),
                icon: PresetIcon::Pencil,
                radius_log: 1.0,
                opacity: 0.95,
                hardness: 0.95,
                min_size_fraction: 0.8,
                color_blending: 0.0,
                dilution: 0.0,
                is_eraser: false,
                texture_id: 0,
                texture_scale: 1.0,
                bristle_id: 0,
            },
            BrushPreset {
                id: 2,
                name: "Ink Pen".to_string(),
                icon: PresetIcon::InkPen,
                radius_log: 1.6,
                opacity: 1.0,
                hardness: 0.88,
                min_size_fraction: 0.2,
                color_blending: 0.0,
                dilution: 0.0,
                is_eraser: false,
                texture_id: 0,
                texture_scale: 1.0,
                bristle_id: 0,
            },
            BrushPreset {
                id: 3,
                name: "Paint Brush".to_string(),
                icon: PresetIcon::PaintBrush,
                radius_log: 2.2,
                opacity: 0.8,
                hardness: 0.5,
                min_size_fraction: 0.3,
                color_blending: 0.5,
                dilution: 0.4,
                is_eraser: false,
                texture_id: 0,
                texture_scale: 1.0,
                bristle_id: 0,
            },
            BrushPreset {
                id: 4,
                name: "Smudge".to_string(),
                icon: PresetIcon::Smudge,
                radius_log: 2.0,
                opacity: 0.4,
                hardness: 0.4,
                min_size_fraction: 0.4,
                color_blending: 0.85,
                dilution: 0.8,
                is_eraser: false,
                texture_id: 0,
                texture_scale: 1.0,
                bristle_id: 0,
            },
            BrushPreset {
                id: 5,
                name: "Eraser".to_string(),
                icon: PresetIcon::Eraser,
                radius_log: 2.5,
                opacity: 1.0,
                hardness: 0.8,
                min_size_fraction: 0.5,
                color_blending: 0.0,
                dilution: 0.0,
                is_eraser: true,
                texture_id: 0,
                texture_scale: 1.0,
                bristle_id: 0,
            },
        ];

        let (save_sender, save_receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            crate::save::save_worker_loop(save_receiver);
        });

        Self {
            active_layer_id: 1,
            layers,
            layer_order: vec![1],
            layer_id_counter: 1,
            active_preset_index: 0,
            presets,
            preset_id_counter: 5,
            brushes,
            brush_states,
            brush_color: [0.1, 0.1, 0.1], // Default charcoal dark
            palette: default_palette,
            selected_palette_index: None,
            brush_radius_log: initial_radius,
            brush_opacity: initial_opacity,
            brush_hardness: initial_hardness,
            brush_min_size_fraction: 0.8,
            brush_color_blending: 0.0,
            brush_dilution: 0.0,
            renaming_preset_index: None,
            rename_input: String::new(),
            pressure_curve: 0.55,
            pressure_min: 0.02,
            input_manager,
            tablet_axis: TabletAxisState::default(),
            egui_touch_pressure: None,
            egui_touch_active: false,
            stabilizer: StrokeStabilizer::new(8),
            last_ptr_pos: None,
            last_ptr_pressure: 1.0,
            last_event_time: 0.0,
            viewport_offset: Vec2::ZERO,
            viewport_zoom: 1.0,
            mirror_horizontal: false,
            rotation_angle: 0.0,
            canvas_width: 1024,
            canvas_height: 1024,
            show_new_canvas_dialog: false,
            new_canvas_width: 1024,
            new_canvas_height: 1024,
            history: HistoryManager::new(50),
            current_stroke_snapshots: Vec::with_capacity(256),
            dragging_layer_id: None,
            stroke_id: 0,
            lock_canvas_bounds: true,
            selection_mask: crate::canvas::SelectionMask::new(),
            brush_textures: vec![
                vec![255u8; 256 * 256],
                generate_noise_texture(),
                generate_bristle_texture(),
            ],
            save_sender,
            current_vector_points: Vec::with_capacity(10000),
            document_path: "canvas.arty".to_string(),
            brush_import_path: "brush.artybrush".to_string(),
            brush_texture_id: 0,
            brush_texture_scale: 1.0,
            brush_bristle_id: 0,
            brush_settings_dirty: false,
            renderer,
            shortcuts: ShortcutManager::new(),
            active_tool: ToolId::Brush,
            fill_options: fill::FillOptions::default(),
            selection_mode: selection::SelectionMode::Replace,
            selection_rect: None,
            lasso_points: None,
            is_selecting: false,
            show_selection_overlay: false,
            selection_feather: 0.0,
            transform_state: transform_tool::TransformState::new(),
            show_export_png_dialog: false,
            export_png_options: crate::export::png::ExportPngOptions::default(),
            export_png_path: "export.png".to_string(),
            autosave_enabled: true,
            autosave_interval_secs: 180.0,
            autosave_path: "autosave.arty".to_string(),
            last_autosave_time: 0.0,
            document_modified: false,
            autosave_status: "".to_string(),
            show_minimal_ui: false,
            show_grid: false,
            show_symmetry: false,
            quick_bar_visible: true,
            color_history: Vec::with_capacity(12),
            color_history_max: 12,
            show_layer_properties: false,
            show_shortcut_editor: false,
            shortcut_search: String::new(),
            shortcut_edit_idx: None,
            shortcut_listen_idx: None,
        }
    }

    fn set_constant(brush: &mut Brush, s: BrushSetting, v: f32) {
        brush.set(s, SettingValue::constant(v));
    }

    fn set_pressure_mapping(
        brush: &mut Brush,
        s: BrushSetting,
        base: f32,
        points: Vec<(f32, f32)>,
    ) {
        let mut mapping = hokusai::mapping::InputMapping::new(hokusai::input::BrushInput::Pressure);
        mapping.points = points;
        brush.set(
            s,
            SettingValue {
                base_value: base,
                inputs: vec![mapping],
                unknown_inputs: std::collections::BTreeMap::new(),
            },
        );
    }

    fn remap_pressure(&self, raw: f32) -> f32 {
        let normalized = raw.clamp(0.0, 1.0).powf(self.pressure_curve);
        (self.pressure_min + normalized * (1.0 - self.pressure_min)).clamp(0.01, 1.0)
    }

    /// Synchronize the local UI sliders back into the active brush's base parameters and rebuild curves.
    /// Only runs when `brush_settings_dirty` is set, avoiding per-frame Hokusai parameter rebuilds.
    fn sync_brush_settings(&mut self) {
        if self.presets.is_empty() || !self.brush_settings_dirty {
            return;
        }
        self.brush_settings_dirty = false;

        // Update the active preset structure with the slider values
        let preset = &mut self.presets[self.active_preset_index];
        preset.radius_log = self.brush_radius_log;
        preset.opacity = self.brush_opacity;
        preset.hardness = self.brush_hardness;
        preset.min_size_fraction = self.brush_min_size_fraction;
        preset.color_blending = self.brush_color_blending;
        preset.dilution = self.brush_dilution;
        preset.texture_id = self.brush_texture_id;
        preset.texture_scale = self.brush_texture_scale;
        preset.bristle_id = self.brush_bristle_id;

        let active_brush = &mut self.brushes[self.active_preset_index];

        // 1. Update basic constants
        active_brush.get_mut(BrushSetting::Radius).base_value = preset.radius_log;
        active_brush.get_mut(BrushSetting::Opaque).base_value = preset.opacity;
        active_brush.get_mut(BrushSetting::Hardness).base_value = preset.hardness;

        // 2. Update smudging (color blending) and dilution (water amount)
        Self::set_constant(active_brush, BrushSetting::Smudge, preset.color_blending);
        Self::set_constant(active_brush, BrushSetting::SmudgeLength, preset.dilution);

        // 3. Rebuild radius pressure curve based on minimum size percentage.
        // Minimum size fraction M controls the logarithmic offset ln(M) at pressure = 0.0.
        // At M=1.0 (100%), there is no pressure size variation. At M=0.05 (5%), thin-to-thick
        // strokes are produced. This is a direct analogue of SAI's "Min Size" slider.
        let min_size_offset = preset.min_size_fraction.max(0.01).ln();
        let radius_points = vec![
            (0.0,  min_size_offset),
            (0.15, min_size_offset * 0.75),
            (0.35, min_size_offset * 0.50),
            (0.55, min_size_offset * 0.28),
            (0.75, min_size_offset * 0.10),
            (0.90, min_size_offset * 0.02),
            (1.0,  0.0),
        ];
        Self::set_pressure_mapping(active_brush, BrushSetting::Radius, preset.radius_log, radius_points);

        // 4. Rebuild opacity pressure curve so light touches produce translucent marks.
        // The floor is (1 - opacity) * 0.6 -- at full opacity=1.0, light touches are still 40%
        // of max; at opacity=0.3, light touches approach near-zero.
        let opacity_floor = (1.0 - preset.opacity) * 0.55 + 0.05;
        let opacity_at_min_pressure = -(preset.opacity * (1.0 - opacity_floor.min(0.90)));
        let opacity_points = vec![
            (0.0,  opacity_at_min_pressure),
            (0.20, opacity_at_min_pressure * 0.60),
            (0.45, opacity_at_min_pressure * 0.25),
            (0.70, opacity_at_min_pressure * 0.07),
            (0.90, opacity_at_min_pressure * 0.01),
            (1.0,  0.0),
        ];
        Self::set_pressure_mapping(active_brush, BrushSetting::Opaque, preset.opacity, opacity_points);

        // 5. OpaqueMultiply: pure linear pressure scale so very soft strokes don't block colour.
        Self::set_pressure_mapping(
            active_brush,
            BrushSetting::OpaqueMultiply,
            0.0,
            vec![(0.0, 0.0), (0.3, 0.55), (0.6, 0.85), (1.0, 1.0)],
        );

        // 6. Set Eraser Mode
        if preset.is_eraser {
            Self::set_constant(active_brush, BrushSetting::Eraser, 1.0);
        } else {
            Self::set_constant(active_brush, BrushSetting::Eraser, 0.0);
        }

        // 7. Convert RGB color picker value to HSV for Hokusai's brush engine
        let hsv = hokusai::color::rgb_to_hsv(
            self.brush_color[0],
            self.brush_color[1],
            self.brush_color[2],
        );
        active_brush.get_mut(BrushSetting::ColorH).base_value = hsv.h;
        active_brush.get_mut(BrushSetting::ColorS).base_value = hsv.s;
        active_brush.get_mut(BrushSetting::ColorV).base_value = hsv.v;
    }

    /// Triggers when the user selects a new brush preset slot
    fn select_preset(&mut self, idx: usize) {
        if idx >= self.presets.len() {
            return;
        }
        self.active_preset_index = idx;

        let preset = &self.presets[idx];
        self.brush_radius_log = preset.radius_log;
        self.brush_opacity = preset.opacity;
        self.brush_hardness = preset.hardness;
        self.brush_min_size_fraction = preset.min_size_fraction;
        self.brush_color_blending = preset.color_blending;
        self.brush_dilution = preset.dilution;
        self.brush_texture_id = preset.texture_id;
        self.brush_texture_scale = preset.texture_scale;
        self.brush_bristle_id = preset.bristle_id;
        // Mark dirty so pressure curves are rebuilt for the newly-selected preset
        self.brush_settings_dirty = true;
    }

    /// Create a new brush preset slot dynamically
    fn create_preset(&mut self, icon_type: PresetIcon) {
        self.preset_id_counter += 1;
        let id = self.preset_id_counter;

        let (name, radius, opacity, hardness, min_size, blending, dilution, is_eraser) = match icon_type {
            PresetIcon::Pencil => ("Pencil".to_string(), 1.0, 0.95, 0.95, 0.8, 0.0, 0.0, false),
            PresetIcon::InkPen => ("Ink Pen".to_string(), 1.6, 1.0, 0.88, 0.2, 0.0, 0.0, false),
            PresetIcon::PaintBrush => ("Paint Brush".to_string(), 2.2, 0.8, 0.5, 0.3, 0.5, 0.4, false),
            PresetIcon::Smudge => ("Smudge".to_string(), 2.0, 0.4, 0.4, 0.4, 0.85, 0.8, false),
            PresetIcon::Eraser => ("Eraser".to_string(), 2.5, 1.0, 0.8, 0.5, 0.0, 0.0, true),
        };

        let preset = BrushPreset {
            id,
            name: format!("{} {}", name, id),
            icon: icon_type,
            radius_log: radius,
            opacity,
            hardness,
            min_size_fraction: min_size,
            color_blending: blending,
            dilution,
            is_eraser,
            texture_id: 0,
            texture_scale: 1.0,
            bristle_id: 0,
        };

        // Create matching Brush setting up the correct pressure curves natively
        let mut brush = Brush::new();
        Self::set_constant(&mut brush, BrushSetting::Radius, radius);
        Self::set_constant(&mut brush, BrushSetting::Opaque, opacity);
        Self::set_constant(&mut brush, BrushSetting::Hardness, hardness);

        match icon_type {
            PresetIcon::Pencil => {
                Self::set_constant(&mut brush, BrushSetting::DabsPerActualRadius, 2.0);
                Self::set_pressure_mapping(&mut brush, BrushSetting::Opaque, opacity, vec![(0.0, -0.90), (0.15, -0.60), (0.35, -0.30), (0.55, -0.10), (0.80, -0.03), (1.0, 0.0)]);
                Self::set_pressure_mapping(&mut brush, BrushSetting::OpaqueMultiply, 0.0, vec![(0.0, 0.0), (1.0, 1.0)]);
            }
            PresetIcon::InkPen => {
                Self::set_constant(&mut brush, BrushSetting::DabsPerActualRadius, 2.5);
                Self::set_pressure_mapping(&mut brush, BrushSetting::Opaque, opacity, vec![(0.0, -0.15), (0.20, -0.05), (0.50, 0.0), (1.0, 0.0)]);
                Self::set_pressure_mapping(&mut brush, BrushSetting::OpaqueMultiply, 0.0, vec![(0.0, 0.0), (1.0, 1.0)]);
            }
            PresetIcon::PaintBrush => {
                Self::set_constant(&mut brush, BrushSetting::DabsPerActualRadius, 3.0);
                Self::set_pressure_mapping(&mut brush, BrushSetting::Opaque, opacity, vec![(0.0, -0.70), (0.15, -0.45), (0.35, -0.25), (0.55, -0.12), (0.80, -0.03), (1.0, 0.0)]);
                Self::set_pressure_mapping(&mut brush, BrushSetting::OpaqueMultiply, 0.0, vec![(0.0, 0.0), (1.0, 1.0)]);
            }
            PresetIcon::Smudge => {
                Self::set_constant(&mut brush, BrushSetting::DabsPerActualRadius, 2.0);
                Self::set_pressure_mapping(&mut brush, BrushSetting::Opaque, opacity, vec![(0.0, -0.30), (0.40, -0.12), (0.70, -0.04), (1.0, 0.0)]);
            }
            PresetIcon::Eraser => {
                Self::set_constant(&mut brush, BrushSetting::Eraser, 1.0);
                Self::set_constant(&mut brush, BrushSetting::DabsPerActualRadius, 2.0);
                Self::set_pressure_mapping(&mut brush, BrushSetting::Opaque, opacity, vec![(0.0, -0.60), (0.20, -0.35), (0.45, -0.15), (0.75, -0.04), (1.0, 0.0)]);
            }
        }

        self.presets.push(preset);
        self.brushes.push(brush);
        self.brush_states.push(BrushState::default());

        // Select the newly created preset
        let new_idx = self.presets.len() - 1;
        self.select_preset(new_idx);
    }

    /// Duplicate an existing preset
    fn duplicate_preset(&mut self, idx: usize) {
        if idx >= self.presets.len() {
            return;
        }
        self.preset_id_counter += 1;
        let id = self.preset_id_counter;

        let mut preset = self.presets[idx].clone();
        preset.id = id;
        preset.name = format!("{} Copy", preset.name);

        let brush = self.brushes[idx].clone();

        self.presets.push(preset);
        self.brushes.push(brush);
        self.brush_states.push(BrushState::default());

        let new_idx = self.presets.len() - 1;
        self.select_preset(new_idx);
    }

    /// Delete a preset
    fn delete_preset(&mut self, idx: usize) {
        if self.presets.len() <= 1 || idx >= self.presets.len() {
            return;
        }
        self.presets.remove(idx);
        self.brushes.remove(idx);
        self.brush_states.remove(idx);

        let new_idx = if self.active_preset_index >= self.presets.len() {
            self.presets.len() - 1
        } else {
            self.active_preset_index
        };
        self.select_preset(new_idx);
    }

    fn catmull_rom(
        p0: &crate::canvas::VectorControlPoint,
        p1: &crate::canvas::VectorControlPoint,
        p2: &crate::canvas::VectorControlPoint,
        p3: &crate::canvas::VectorControlPoint,
        t: f32,
    ) -> crate::canvas::VectorControlPoint {
        let t2 = t * t;
        let t3 = t2 * t;

        let f1 = -0.5 * t3 + t2 - 0.5 * t;
        let f2 = 1.5 * t3 - 2.5 * t2 + 1.0;
        let f3 = -1.5 * t3 + 2.0 * t2 + 0.5 * t;
        let f4 = 0.5 * t3 - 0.5 * t2;

        crate::canvas::VectorControlPoint {
            x: p0.x * f1 + p1.x * f2 + p2.x * f3 + p3.x * f4,
            y: p0.y * f1 + p1.y * f2 + p2.y * f3 + p3.y * f4,
            pressure: p0.pressure * f1 + p1.pressure * f2 + p2.pressure * f3 + p3.pressure * f4,
            tilt_x: p0.tilt_x * f1 + p1.tilt_x * f2 + p2.tilt_x * f3 + p3.tilt_x * f4,
            tilt_y: p0.tilt_y * f1 + p1.tilt_y * f2 + p2.tilt_y * f3 + p3.tilt_y * f4,
        }
    }

    pub fn redraw_vector_layer(&mut self, layer_id: u32) {
        let mut strokes_to_draw = Vec::new();
        if let Some(layer) = self.layers.get_mut(&layer_id) {
            if layer.kind != crate::canvas::LayerType::Vector {
                return;
            }
            layer.tiles.clear();
            if let Some(v_data) = &layer.vector_data {
                strokes_to_draw = v_data.strokes.clone();
            }
        }

        for stroke in strokes_to_draw {
            let preset_idx = self
                .presets
                .iter()
                .position(|p| p.id == stroke.brush_preset_id)
                .unwrap_or(0);

            let brush = &self.brushes[preset_idx];
            let mut brush_state = BrushState::default();

            if stroke.control_points.len() < 2 {
                continue;
            }

            if let Some(layer) = self.layers.get_mut(&layer_id) {
                layer.begin_atomic();
            }

            let preset = &self.presets[preset_idx];
            let tex_idx = preset.texture_id as usize;
            let brush_texture = if tex_idx > 0 && tex_idx < self.brush_textures.len() {
                Some(self.brush_textures[tex_idx].as_slice())
            } else {
                None
            };

            let mut current_stroke_snapshots = Vec::new();

            for k in 3..=stroke.control_points.len() {
                let p0 = if k >= 4 {
                    &stroke.control_points[k - 4]
                } else {
                    &stroke.control_points[k - 3]
                };
                let p1 = &stroke.control_points[k - 3];
                let p2 = &stroke.control_points[k - 2];
                let p3 = &stroke.control_points[k - 1];

                let dist = ((p2.x - p1.x).powi(2) + (p2.y - p1.y).powi(2)).sqrt();
                let steps = ((dist / 2.0) as usize).max(2).min(100);

                let start_i = if k == 3 { 0 } else { 1 };

                for i in start_i..=steps {
                    let t = i as f32 / steps as f32;
                    let pt = Self::catmull_rom(p0, p1, p2, p3, t);

                    if let Some(layer) = self.layers.get_mut(&layer_id) {
                        let mut stroke_surface = StrokeSurface {
                            layer,
                            history: &mut self.history,
                            snapshots: &mut current_stroke_snapshots,
                            stroke_id: 0,
                            canvas_width: self.canvas_width,
                            canvas_height: self.canvas_height,
                            lock_canvas_bounds: self.lock_canvas_bounds,
                            selection_mask: Some(&self.selection_mask),
                            brush_texture,
                            brush_texture_width: 256,
                            brush_texture_height: 256,
                            brush_texture_scale: preset.texture_scale,
                        };

                        brush.stroke_to(
                            &mut brush_state,
                            &mut stroke_surface,
                            pt.x,
                            pt.y,
                            pt.pressure,
                            pt.tilt_x,
                            pt.tilt_y,
                            0.016,
                        );
                    }
                }
            }

            let len = stroke.control_points.len();
            let p0 = if len >= 3 {
                &stroke.control_points[len - 3]
            } else {
                &stroke.control_points[len - 2]
            };
            let p1 = &stroke.control_points[len - 2];
            let p2 = &stroke.control_points[len - 1];
            let p3 = &stroke.control_points[len - 1];

            let dist = ((p2.x - p1.x).powi(2) + (p2.y - p1.y).powi(2)).sqrt();
            let steps = ((dist / 2.0) as usize).max(2).min(100);

            let start_i = if len == 2 { 0 } else { 1 };
            for i in start_i..=steps {
                let t = i as f32 / steps as f32;
                let pt = Self::catmull_rom(p0, p1, p2, p3, t);

                if let Some(layer) = self.layers.get_mut(&layer_id) {
                    let mut stroke_surface = StrokeSurface {
                        layer,
                        history: &mut self.history,
                        snapshots: &mut current_stroke_snapshots,
                        stroke_id: 0,
                        canvas_width: self.canvas_width,
                        canvas_height: self.canvas_height,
                        lock_canvas_bounds: self.lock_canvas_bounds,
                        selection_mask: Some(&self.selection_mask),
                        brush_texture,
                        brush_texture_width: 256,
                        brush_texture_height: 256,
                        brush_texture_scale: preset.texture_scale,
                    };

                    brush.stroke_to(
                        &mut brush_state,
                        &mut stroke_surface,
                        pt.x,
                        pt.y,
                        pt.pressure,
                        pt.tilt_x,
                        pt.tilt_y,
                        0.016,
                    );
                }
            }

            if let Some(layer) = self.layers.get_mut(&layer_id) {
                let _dirty = layer.end_atomic();
            }
        }

        if let Some(renderer) = &mut self.renderer {
            renderer.clear_cache();
        }
    }

    pub fn save_canvas(&self, path: &std::path::Path) {
        let mut tiles_to_save = Vec::new();
        for (&layer_id, layer) in &self.layers {
            for (&coords, tile) in &layer.tiles {
                tiles_to_save.push(crate::save::TileSaveData {
                    layer_id,
                    tx: coords.0,
                    ty: coords.1,
                    pixels: tile.pixels.clone(),
                });
            }
        }

        let mut layers_meta = Vec::new();
        for &id in &self.layer_order {
            if let Some(layer) = self.layers.get(&id) {
                layers_meta.push(crate::save::LayerMetadata {
                    id: layer.id,
                    name: layer.name.clone(),
                    opacity: layer.opacity,
                    visible: layer.visible,
                    lock_alpha: layer.lock_alpha,
                    is_clipping: layer.is_clipping,
                    blend_mode: crate::save::blend_mode_to_str(layer.blend_mode).to_string(),
                    kind: crate::save::layer_type_to_str(&layer.kind).to_string(),
                    folder_child_ids: match &layer.kind {
                        crate::canvas::LayerType::Folder { child_ids } => child_ids.clone(),
                        _ => Vec::new(),
                    },
                    vector_strokes: match &layer.kind {
                        crate::canvas::LayerType::Vector => {
                            layer.vector_data.as_ref().map(|vd| vd.strokes.clone())
                        }
                        _ => None,
                    },
                });
            }
        }

        let task = crate::save::SaveTask {
            filepath: path.to_path_buf(),
            canvas_width: self.canvas_width,
            canvas_height: self.canvas_height,
            layer_order: self.layer_order.clone(),
            layers_meta,
            tiles: tiles_to_save,
        };

        if let Err(e) = self.save_sender.send(task) {
            log::error!("Failed to send save task: {:?}", e);
        }
    }

    pub fn load_from_document(&mut self, doc: crate::save::LoadedDocument) {
        self.canvas_width = doc.canvas_width;
        self.canvas_height = doc.canvas_height;
        self.layer_order = doc.layer_order;
        self.layers.clear();
        let mut max_id = 1;
        for l in doc.layers {
            let mut layer = Layer::new(l.id, l.name);
            layer.opacity = l.opacity;
            layer.visible = l.visible;
            layer.lock_alpha = l.lock_alpha;
            layer.is_clipping = l.is_clipping;
            layer.blend_mode = l.blend_mode;
            layer.kind = l.kind;
            layer.vector_data = l.vector_data;

            for t in l.tiles {
                let mut tile = crate::canvas::Tile::new();
                tile.pixels = t.pixels;
                tile.is_dirty = true;
                layer.tiles.insert((t.tx, t.ty), tile);
            }
            if l.id > max_id {
                max_id = l.id;
            }
            self.layers.insert(l.id, layer);
        }
        self.layer_id_counter = max_id;
        self.active_layer_id = self.layer_order.first().copied().unwrap_or(1);

        for id in &self.layer_order.clone() {
            let is_vector = self
                .layers
                .get(id)
                .map(|l| matches!(l.kind, crate::canvas::LayerType::Vector))
                .unwrap_or(false);
            if is_vector {
                self.redraw_vector_layer(*id);
            }
        }

        if let Some(r) = &mut self.renderer {
            r.clear_cache();
        }
    }

    pub fn command(&mut self, cmd: CommandId) {
        match cmd {
            // File
            CommandId::NewDocument => self.show_new_canvas_dialog = true,
            CommandId::Save => {
                self.save_canvas(std::path::Path::new(&self.document_path));
                self.document_modified = false;
            }
            CommandId::SaveAs => {
                // Would open save dialog; for now just save
                self.save_canvas(std::path::Path::new(&self.document_path));
                self.document_modified = false;
            }
            CommandId::ExportPng => self.show_export_png_dialog = true,
            CommandId::Exit => {
                // Would be handled by the frame
            }

            // Edit
            CommandId::Undo => { self.history.undo(&mut self.layers); }
            CommandId::Redo => { self.history.redo(&mut self.layers); }
            CommandId::SelectAll => {
                self.selection_mode = selection::SelectionMode::Replace;
                let r = selection::SelectionRect {
                    x0: 0.0, y0: 0.0,
                    x1: self.canvas_width as f32, y1: self.canvas_height as f32,
                };
                selection::apply_rect_selection(&mut self.selection_mask, r, selection::SelectionMode::Replace, 0.0, false);
            }
            CommandId::Deselect => {
                selection::clear_selection(&mut self.selection_mask);
            }
            CommandId::InvertSelection => {
                selection::invert_selection(&mut self.selection_mask, self.canvas_width, self.canvas_height);
            }
            CommandId::Clear => {
                if self.selection_mask.is_active && !self.selection_mask.is_empty() {
                    self.clear_selected_area();
                }
            }
            CommandId::Fill => {
                if self.selection_mask.is_active && !self.selection_mask.is_empty() {
                    self.fill_selected_area();
                }
            }

            // Canvas
            CommandId::FitToScreen => self.fit_to_screen(),
            CommandId::ActualSize => {
                self.viewport_zoom = 1.0;
                self.viewport_offset = Vec2::ZERO;
            }
            CommandId::ResetView => {
                self.viewport_zoom = 1.0;
                self.viewport_offset = Vec2::ZERO;
                self.rotation_angle = 0.0;
                self.mirror_horizontal = false;
            }
            CommandId::FlipViewHorizontal => self.mirror_horizontal = !self.mirror_horizontal,
            CommandId::ResetRotation => self.rotation_angle = 0.0,
            CommandId::RotateCanvasViewLeft => {
                self.rotation_angle = (self.rotation_angle - 15.0f32.to_radians()).rem_euclid(2.0 * std::f32::consts::PI);
            }
            CommandId::RotateCanvasViewRight => {
                self.rotation_angle = (self.rotation_angle + 15.0f32.to_radians()).rem_euclid(2.0 * std::f32::consts::PI);
            }

            // Layer
            CommandId::NewRasterLayer => self.create_raster_layer(),
            CommandId::NewFolder => self.create_folder_layer(),
            CommandId::DuplicateLayer => self.duplicate_active_layer(),
            CommandId::DeleteLayer => self.delete_active_layer(),
            CommandId::MergeDown => self.merge_down(),
            CommandId::MergeVisible => self.merge_visible(),
            CommandId::FlattenImage => self.flatten_image(),
            CommandId::ToggleLockAlpha => {
                if let Some(layer) = self.layers.get_mut(&self.active_layer_id) {
                    layer.lock_alpha = !layer.lock_alpha;
                }
            }
            CommandId::ToggleClipping => {
                if let Some(layer) = self.layers.get_mut(&self.active_layer_id) {
                    layer.is_clipping = !layer.is_clipping;
                }
            }

            // Tools
            CommandId::ToolBrush => self.active_tool = ToolId::Brush,
            CommandId::ToolEraser => {
                self.active_tool = ToolId::Eraser;
                if let Some(p) = self.presets.get_mut(self.active_preset_index) {
                    p.is_eraser = true;
                    self.brush_settings_dirty = true;
                }
            }
            CommandId::ToolFill => self.active_tool = ToolId::Fill,
            CommandId::ToolRectSelect => self.active_tool = ToolId::RectSelect,
            CommandId::ToolEllipseSelect => self.active_tool = ToolId::EllipseSelect,
            CommandId::ToolLasso => self.active_tool = ToolId::Lasso,
            CommandId::ToolMagicWand => self.active_tool = ToolId::MagicWand,
            CommandId::ToolMove => self.active_tool = ToolId::Move,
            CommandId::ToolTransform => self.active_tool = ToolId::Transform,
            CommandId::ToolColorPicker => self.active_tool = ToolId::ColorPicker,
            CommandId::ToolHand => self.active_tool = ToolId::Hand,
            CommandId::ToolZoom => self.active_tool = ToolId::Zoom,
            CommandId::ToolRotateView => self.active_tool = ToolId::RotateView,

            // View
            CommandId::MinimalUi => self.show_minimal_ui = !self.show_minimal_ui,
            CommandId::ShowGrid => self.show_grid = !self.show_grid,
            CommandId::ShowSymmetry => self.show_symmetry = !self.show_symmetry,

            // Misc
            CommandId::Preferences => {}
            CommandId::KeyboardShortcuts => {}
            CommandId::TabletDiagnostics => {}
            CommandId::PerformanceHud => {}

            _ => {}
        }
    }

    fn fit_to_screen(&mut self) {
        if let Some(r) = &self.renderer {
            let vp_w = r.target_width as f32;
            let vp_h = r.target_height as f32;
            if vp_w > 0.0 && vp_h > 0.0 && self.canvas_width > 0 && self.canvas_height > 0 {
                let zoom_x = vp_w / self.canvas_width as f32;
                let zoom_y = vp_h / self.canvas_height as f32;
                self.viewport_zoom = zoom_x.min(zoom_y) * 0.95;
                self.viewport_offset = Vec2::new(
                    (self.canvas_width as f32 - (vp_w / self.viewport_zoom)) * 0.5,
                    (self.canvas_height as f32 - (vp_h / self.viewport_zoom)) * 0.5,
                );
            }
        }
    }

    fn create_raster_layer(&mut self) {
        self.layer_id_counter += 1;
        let new_id = self.layer_id_counter;
        let mut new_layer = Layer::new(new_id, format!("Layer {}", new_id));
        new_layer.kind = crate::canvas::LayerType::Raster;
        self.layers.insert(new_id, new_layer);
        self.layer_order.insert(0, new_id);
        self.active_layer_id = new_id;
    }

    fn create_folder_layer(&mut self) {
        self.layer_id_counter += 1;
        let new_id = self.layer_id_counter;
        let mut new_layer = Layer::new(new_id, format!("Folder {}", new_id));
        new_layer.kind = crate::canvas::LayerType::Folder { child_ids: Vec::new() };
        self.layers.insert(new_id, new_layer);
        self.layer_order.insert(0, new_id);
        self.active_layer_id = new_id;
    }

    fn duplicate_active_layer(&mut self) {
        let Some(source) = self.layers.get(&self.active_layer_id) else { return; };
        self.layer_id_counter += 1;
        let new_id = self.layer_id_counter;
        let mut new_layer = Layer::new(new_id, format!("{} Copy", source.name));
        new_layer.opacity = source.opacity;
        new_layer.visible = source.visible;
        new_layer.lock_alpha = source.lock_alpha;
        new_layer.is_clipping = source.is_clipping;
        new_layer.blend_mode = source.blend_mode;
        new_layer.kind = source.kind.clone();
        new_layer.vector_data = source.vector_data.clone();

        // Copy tiles
        for (&coords, tile) in &source.tiles {
            let mut new_tile = crate::canvas::Tile::new();
            new_tile.pixels = tile.pixels.clone();
            new_tile.is_dirty = true;
            new_layer.tiles.insert(coords, new_tile);
        }

        self.layers.insert(new_id, new_layer);
        self.layer_order.insert(0, new_id);
        self.active_layer_id = new_id;
    }

    fn delete_active_layer(&mut self) {
        if self.layer_order.len() <= 1 { return; }
        let active_id = self.active_layer_id;
        if let Some(pos) = self.layer_order.iter().position(|&x| x == active_id) {
            self.layer_order.remove(pos);
            self.layers.remove(&active_id);
            self.active_layer_id = self.layer_order[0];
        }
    }

    fn merge_down(&mut self) {
        let active_id = self.active_layer_id;
        let pos = match self.layer_order.iter().position(|&x| x == active_id) {
            Some(p) => p,
            None => return,
        };
        if pos + 1 >= self.layer_order.len() { return; }
        let target_id = self.layer_order[pos + 1];

        let tiles_to_merge: Vec<(i32, i32, crate::canvas::Tile)> = {
            let Some(layer) = self.layers.get(&active_id) else { return; };
            layer.tiles.iter().map(|(&coords, tile)| {
                let mut new_tile = crate::canvas::Tile::new();
                new_tile.pixels = tile.pixels.clone();
                new_tile.is_dirty = true;
                (coords.0, coords.1, new_tile)
            }).collect()
        };

        if let Some(target) = self.layers.get_mut(&target_id) {
            for (tx, ty, tile) in tiles_to_merge {
                target.tiles.insert((tx, ty), tile);
            }
        }

        self.layer_order.remove(pos);
        self.layers.remove(&active_id);
        self.active_layer_id = target_id;

        if let Some(r) = &mut self.renderer {
            r.clear_cache();
        }
    }

    fn merge_visible(&mut self) {
        let visible_ids: Vec<u32> = self.layer_order.iter()
            .filter(|&&id| {
                self.layers.get(&id).map(|l| l.visible).unwrap_or(false)
            })
            .copied()
            .collect();

        if visible_ids.len() < 2 { return; }
        let top_id = visible_ids[0];

        for &id in &visible_ids[1..] {
            let tiles_to_merge: Vec<(i32, i32, crate::canvas::Tile)> = {
                let Some(layer) = self.layers.get(&id) else { continue; };
                layer.tiles.iter().map(|(&coords, tile)| {
                    let mut new_tile = crate::canvas::Tile::new();
                    new_tile.pixels = tile.pixels.clone();
                    new_tile.is_dirty = true;
                    (coords.0, coords.1, new_tile)
                }).collect()
            };

            if let Some(top) = self.layers.get_mut(&top_id) {
                for (tx, ty, tile) in tiles_to_merge {
                    top.tiles.insert((tx, ty), tile);
                }
            }
        }

        // Remove merged layers
        for &id in &visible_ids[1..] {
            self.layer_order.retain(|&x| x != id);
            self.layers.remove(&id);
        }
        self.active_layer_id = top_id;

        if let Some(r) = &mut self.renderer {
            r.clear_cache();
        }
    }

    fn flatten_image(&mut self) {
        // Merge all visible layers into the bottom-most visible layer
        let bottom_visible = self.layer_order.iter()
            .rev()
            .find(|&&id| self.layers.get(&id).map(|l| l.visible).unwrap_or(false))
            .copied();

        let Some(bottom_id) = bottom_visible else { return; };

        let visible_ids: Vec<u32> = self.layer_order.iter()
            .filter(|&&id| {
                id != bottom_id && self.layers.get(&id).map(|l| l.visible).unwrap_or(false)
            })
            .copied()
            .collect();

        for &id in &visible_ids {
            let tiles_to_merge: Vec<(i32, i32, crate::canvas::Tile)> = {
                let Some(layer) = self.layers.get(&id) else { continue; };
                layer.tiles.iter().map(|(&coords, tile)| {
                    let mut new_tile = crate::canvas::Tile::new();
                    new_tile.pixels = tile.pixels.clone();
                    new_tile.is_dirty = true;
                    (coords.0, coords.1, new_tile)
                }).collect()
            };

            if let Some(bottom) = self.layers.get_mut(&bottom_id) {
                for (tx, ty, tile) in tiles_to_merge {
                    bottom.tiles.insert((tx, ty), tile);
                }
            }
        }

        for &id in &visible_ids {
            self.layer_order.retain(|&x| x != id);
            self.layers.remove(&id);
        }
        // Keep only bottom layer
        self.layer_order.retain(|&x| x == bottom_id);
        self.active_layer_id = bottom_id;

        if let Some(r) = &mut self.renderer {
            r.clear_cache();
        }
    }

    fn clear_selected_area(&mut self) {
        if !self.selection_mask.is_active { return; }
        let Some(layer) = self.layers.get_mut(&self.active_layer_id) else { return; };
        let sel = &self.selection_mask;
        for (&(tx, ty), tile) in &mut layer.tiles {
            for ly in 0usize..64 {
                for lx in 0usize..64 {
                    let wx = tx * 64 + lx as i32;
                    let wy = ty * 64 + ly as i32;
                    if sel.get_value(wx, wy) > 0 {
                        tile.pixels[ly][lx] = [0, 0, 0, 0];
                        tile.is_dirty = true;
                    }
                }
            }
        }
    }

    fn fill_selected_area(&mut self) {
        if !self.selection_mask.is_active { return; }
        let Some(layer) = self.layers.get_mut(&self.active_layer_id) else { return; };
        let sel = &self.selection_mask;
        let fill_color: [u16; 4] = [
            (self.brush_color[0] * 32768.0) as u16,
            (self.brush_color[1] * 32768.0) as u16,
            (self.brush_color[2] * 32768.0) as u16,
            32768,
        ];
        for (&(tx, ty), tile) in &mut layer.tiles {
            for ly in 0usize..64 {
                for lx in 0usize..64 {
                    let wx = tx * 64 + lx as i32;
                    let wy = ty * 64 + ly as i32;
                    if sel.get_value(wx, wy) > 0 {
                        tile.pixels[ly][lx] = fill_color;
                        tile.is_dirty = true;
                    }
                }
            }
        }
    }
}

impl eframe::App for PaintApp {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Drop the InputManager (and its inner octotablet Manager) before the window closes.
        // This ensures the window handle remains valid for the lifetime of the tablet connection.
        self.input_manager.take();
        log::info!("[PaintApp] InputManager shut down.");
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Only flush slider values into Hokusai when something actually changed
        self.sync_brush_settings();

        // Winit emits Windows pen pressure as egui touch force, while egui-winit
        // also translates the same touch into normal primary pointer events.
        ctx.input(|i| {
            for event in &i.events {
                if let egui::Event::Touch { phase, force, .. } = event {
                    match phase {
                        egui::TouchPhase::Start | egui::TouchPhase::Move => {
                            self.egui_touch_active = true;
                            if let Some(force) = force {
                                self.egui_touch_pressure = Some(force.clamp(0.0, 1.0));
                            }
                        }
                        egui::TouchPhase::End | egui::TouchPhase::Cancel => {
                            self.egui_touch_active = false;
                            self.egui_touch_pressure = None;
                        }
                    }
                }
            }
        });

        // Pump native tablet events (pressure, tilt, proximity) via octotablet/Windows Ink
        if let Some(input_mgr) = &mut self.input_manager {
            let (axis, has_tablet_events) = input_mgr.pump();
            self.tablet_axis = axis;

            // Egui does not automatically repaint for RealTimeStylus callbacks.
            // Polling keeps pen pressure current and prevents stale callback queues.
            let interval_ms = if has_tablet_events || axis.in_proximity || axis.tip_down {
                16
            } else {
                250
            };
            ctx.request_repaint_after(std::time::Duration::from_millis(interval_ms));
        }

        // 0. NEW CANVAS DIALOG OVERLAY
        if self.show_new_canvas_dialog {
            let mut close_dialog = false;
            let mut create_canvas = false;

            egui::Window::new("New Canvas")
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .resizable(false)
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new("Choose the canvas size").strong());
                        ui.add_space(8.0);
                    });

                    egui::Grid::new("new_canvas_grid")
                        .num_columns(2)
                        .spacing([12.0, 8.0])
                        .show(ui, |ui| {
                            ui.label("Width:");
                            ui.add(
                                egui::DragValue::new(&mut self.new_canvas_width)
                                    .clamp_range(256..=4096)
                                    .suffix(" px"),
                            );
                            ui.end_row();

                            ui.label("Height:");
                            ui.add(
                                egui::DragValue::new(&mut self.new_canvas_height)
                                    .clamp_range(256..=4096)
                                    .suffix(" px"),
                            );
                            ui.end_row();
                        });

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(8.0);

                    ui.label("Presets:");
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Square (1024x1024)").clicked() {
                            self.new_canvas_width = 1024;
                            self.new_canvas_height = 1024;
                        }
                        if ui.button("FullHD (1920x1080)").clicked() {
                            self.new_canvas_width = 1920;
                            self.new_canvas_height = 1080;
                        }
                        if ui.button("2K Square (2048x2048)").clicked() {
                            self.new_canvas_width = 2048;
                            self.new_canvas_height = 2048;
                        }
                        if ui.button("A4 Paper (2480x3508)").clicked() {
                            self.new_canvas_width = 2480;
                            self.new_canvas_height = 3508;
                        }
                    });

                    ui.add_space(12.0);
                    ui.separator();
                    ui.add_space(8.0);

                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::Button::new("Create").min_size(egui::Vec2::new(100.0, 30.0)))
                            .clicked()
                        {
                            create_canvas = true;
                        }
                        if ui
                            .add(egui::Button::new("Cancel").min_size(egui::Vec2::new(100.0, 30.0)))
                            .clicked()
                        {
                            close_dialog = true;
                        }
                    });
                });

            if create_canvas {
                self.canvas_width = self.new_canvas_width;
                self.canvas_height = self.new_canvas_height;

                self.layers.clear();
                self.layers.insert(1, Layer::new(1, "Layer 1".to_string()));
                self.layer_order = vec![1];
                self.layer_id_counter = 1;
                self.active_layer_id = 1;
                self.history.undo_stack.clear();
                self.history.redo_stack.clear();

                // Centering view on create
                self.viewport_offset = egui::Vec2::ZERO;
                self.viewport_zoom = 1.0;

                if let Some(r) = &mut self.renderer {
                    r.clear_cache();
                }
                self.show_new_canvas_dialog = false;
            } else if close_dialog {
                self.show_new_canvas_dialog = false;
            }
        }

        // 1. TOP MENU PANEL
        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("New Canvas").clicked() {
                        self.show_new_canvas_dialog = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label("Path:");
                        ui.text_edit_singleline(&mut self.document_path);
                    });
                    if ui.button("Open Canvas").clicked() {
                        let path = std::path::PathBuf::from(&self.document_path);
                        match crate::save::load_document(&path) {
                            Ok(loaded_doc) => {
                                self.load_from_document(loaded_doc);
                                log::info!("Loaded document successfully from {:?}", path);
                            }
                            Err(e) => {
                                log::error!("Failed to load document: {:?}", e);
                            }
                        }
                        ui.close_menu();
                    }
                    if ui.button("Save Canvas").clicked() {
                        self.save_canvas(std::path::Path::new(&self.document_path));
                        self.document_modified = false;
                        ui.close_menu();
                    }
                    ui.separator();
                    ui.menu_button("Export", |ui| {
                        if ui.button("Export PNG...").clicked() {
                            self.show_export_png_dialog = true;
                            ui.close_menu();
                    }
                 });

                ui.menu_button("View", |ui| {
                    if ui.button("Show Grid").clicked() {
                        self.show_grid = !self.show_grid;
                        ui.close_menu();
                    }
                    if ui.button("Minimal UI (Tab)").clicked() {
                        self.show_minimal_ui = !self.show_minimal_ui;
                        ui.close_menu();
                    }
                });

                ui.menu_button("Help", |ui| {
                    if ui.button("Keyboard Shortcuts").clicked() {
                        self.show_shortcut_editor = true;
                        ui.close_menu();
                    }
                });

                ui.separator();
                    if ui.button("Exit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });

                ui.menu_button("Edit", |ui| {
                    if ui
                        .add_enabled(
                            !self.history.undo_stack.is_empty(),
                            egui::Button::new("Undo (Ctrl+Z)"),
                        )
                        .clicked()
                    {
                        self.history.undo(&mut self.layers);
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            !self.history.redo_stack.is_empty(),
                            egui::Button::new("Redo (Ctrl+Y)"),
                        )
                        .clicked()
                    {
                        self.history.redo(&mut self.layers);
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Select All (Ctrl+A)").clicked() {
                        self.command(CommandId::SelectAll);
                        ui.close_menu();
                    }
                    if ui.button("Deselect (Ctrl+D)").clicked() {
                        self.command(CommandId::Deselect);
                        ui.close_menu();
                    }
                    if ui.button("Invert Selection (Ctrl+I)").clicked() {
                        self.command(CommandId::InvertSelection);
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Clear (Delete)").clicked() {
                        self.command(CommandId::Clear);
                        ui.close_menu();
                    }
                    if ui.button("Fill (Alt+Backspace)").clicked() {
                        self.command(CommandId::Fill);
                        ui.close_menu();
                    }
                });

                ui.menu_button("Layer", |ui| {
                    if ui.button("New Raster Layer").clicked() {
                        self.command(CommandId::NewRasterLayer);
                        ui.close_menu();
                    }
                    if ui.button("New Folder").clicked() {
                        self.command(CommandId::NewFolder);
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Duplicate Layer").clicked() {
                        self.command(CommandId::DuplicateLayer);
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            self.layer_order.len() > 1,
                            egui::Button::new("Delete Layer"),
                        )
                        .clicked()
                    {
                        self.command(CommandId::DeleteLayer);
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui
                        .add_enabled(
                            self.layer_order.len() > 1,
                            egui::Button::new("Merge Down"),
                        )
                        .clicked()
                    {
                        self.command(CommandId::MergeDown);
                        ui.close_menu();
                    }
                    if ui.button("Merge Visible").clicked() {
                        self.command(CommandId::MergeVisible);
                        ui.close_menu();
                    }
                    if ui.button("Flatten Image").clicked() {
                        self.command(CommandId::FlattenImage);
                        ui.close_menu();
                    }
                });

                ui.menu_button("Canvas", |ui| {
                    if ui.button("Fit to Screen").clicked() {
                        self.command(CommandId::FitToScreen);
                        ui.close_menu();
                    }
                    if ui.button("Actual Size (100%)").clicked() {
                        self.command(CommandId::ActualSize);
                        ui.close_menu();
                    }
                    if ui.button("Reset View").clicked() {
                        self.command(CommandId::ResetView);
                        ui.close_menu();
                    }

                    ui.separator();
                    ui.label("Canvas Size:");
                    ui.horizontal(|ui| {
                        ui.label("W:");
                        if ui.add(
                            egui::DragValue::new(&mut self.canvas_width)
                                .clamp_range(256..=4096)
                                .suffix("px"),
                        ).changed() {
                            if let Some(r) = &mut self.renderer {
                                r.clear_cache();
                            }
                        }
                        ui.label("H:");
                        if ui.add(
                            egui::DragValue::new(&mut self.canvas_height)
                                .clamp_range(256..=4096)
                                .suffix("px"),
                        ).changed() {
                            if let Some(r) = &mut self.renderer {
                                r.clear_cache();
                            }
                        }
                    });

                    egui::ComboBox::from_id_source("canvas_preset_menu")
                        .selected_text(format!("Preset: {}x{}", self.canvas_width, self.canvas_height))
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(
                                self.canvas_width == 1024 && self.canvas_height == 1024,
                                "Square (1024x1024)",
                            ).clicked() {
                                self.canvas_width = 1024;
                                self.canvas_height = 1024;
                                if let Some(r) = &mut self.renderer {
                                    r.clear_cache();
                                }
                            }
                            if ui.selectable_label(
                                self.canvas_width == 1920 && self.canvas_height == 1080,
                                "FullHD (1920x1080)",
                            ).clicked() {
                                self.canvas_width = 1920;
                                self.canvas_height = 1080;
                                if let Some(r) = &mut self.renderer {
                                    r.clear_cache();
                                }
                            }
                            if ui.selectable_label(
                                self.canvas_width == 2048 && self.canvas_height == 2048,
                                "2K Square (2048x2048)",
                            ).clicked() {
                                self.canvas_width = 2048;
                                self.canvas_height = 2048;
                                if let Some(r) = &mut self.renderer {
                                    r.clear_cache();
                                }
                            }
                            if ui.selectable_label(
                                self.canvas_width == 2480 && self.canvas_height == 3508,
                                "A4 (2480x3508)",
                            ).clicked() {
                                self.canvas_width = 2480;
                                self.canvas_height = 3508;
                                if let Some(r) = &mut self.renderer {
                                    r.clear_cache();
                                }
                            }
                        });
                });

                ui.separator();
                ui.label("Stabilizer:");
                let current_level = self.stabilizer.level;
                let text = match current_level {
                    StabilizerLevel::Off => "Off".to_string(),
                    StabilizerLevel::Level(val) => format!("Level {}", val),
                    StabilizerLevel::SLevel(val) => format!("S-{}", val),
                };
                let response = egui::ComboBox::from_id_source("top_stabilizer_level")
                    .selected_text(text)
                    .width(90.0)
                    .show_ui(ui, |ui| {
                        let mut selected = false;
                        if ui.selectable_label(matches!(current_level, StabilizerLevel::Off), "Off").clicked() {
                            self.stabilizer.set_level(StabilizerLevel::Off);
                            selected = true;
                        }
                        for val in 1..=15 {
                            let is_sel = match current_level {
                                StabilizerLevel::Level(v) => v == val,
                                _ => false,
                            };
                            if ui.selectable_label(is_sel, format!("Level {}", val)).clicked() {
                                self.stabilizer.set_level(StabilizerLevel::Level(val));
                                selected = true;
                            }
                        }
                        for val in 1..=5 {
                            let is_sel = match current_level {
                                StabilizerLevel::SLevel(v) => v == val,
                                _ => false,
                            };
                            if ui.selectable_label(is_sel, format!("S-{}", val)).clicked() {
                                self.stabilizer.set_level(StabilizerLevel::SLevel(val));
                                selected = true;
                            }
                        }
                        selected
                    });
                if response.inner.unwrap_or(false) {
                    ctx.request_repaint();
                }

                ui.label("Mode:");
                let current_mode = self.stabilizer.mode;
                let mode_text = match current_mode {
                    StabilizerMode::Ema => "EMA",
                    StabilizerMode::SpringMassDamper => "Spring Physics",
                };
                let response = egui::ComboBox::from_id_source("top_stabilizer_mode")
                    .selected_text(mode_text)
                    .width(120.0)
                    .show_ui(ui, |ui| {
                        let mut selected = false;
                        if ui.selectable_label(current_mode == StabilizerMode::Ema, "EMA").clicked() {
                            self.stabilizer.mode = StabilizerMode::Ema;
                            selected = true;
                        }
                        if ui.selectable_label(current_mode == StabilizerMode::SpringMassDamper, "Spring Physics").clicked() {
                            self.stabilizer.mode = StabilizerMode::SpringMassDamper;
                            selected = true;
                        }
                        selected
                    });
                if response.inner.unwrap_or(false) {
                    ctx.request_repaint();
                }
            });
        });

        // 1b. QUICK BAR
        if self.quick_bar_visible && !self.show_minimal_ui {
            egui::TopBottomPanel::top("quick_bar").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("↶ Undo").clicked() { self.command(CommandId::Undo); }
                    if ui.button("↷ Redo").clicked() { self.command(CommandId::Redo); }
                    ui.separator();
                    if ui.button("💾 Save").clicked() { self.command(CommandId::Save); }
                    ui.separator();
                    if ui.button("Select All").clicked() { self.command(CommandId::SelectAll); }
                    if ui.button("Deselect").clicked() { self.command(CommandId::Deselect); }
                    if ui.button("Invert").clicked() { self.command(CommandId::InvertSelection); }
                    ui.separator();
                    if ui.button("✂ Cut").clicked() { self.command(CommandId::Clear); }
                    if ui.button("📋 Fill").clicked() { self.command(CommandId::Fill); }
                    ui.separator();
                    if ui.button("Fit").clicked() { self.command(CommandId::FitToScreen); }
                    if ui.button("100%").clicked() { self.command(CommandId::ActualSize); }
                    if ui.button("Reset").clicked() { self.command(CommandId::ResetView); }
                    ui.separator();
                    ui.label("Zoom:");
                    if ui.button("−").clicked() {
                        self.viewport_zoom = (self.viewport_zoom - 0.25).max(0.1);
                    }
                    ui.label(format!("{:.0}%", self.viewport_zoom * 100.0));
                    if ui.button("+").clicked() {
                        self.viewport_zoom = (self.viewport_zoom + 0.25).min(10.0);
                    }
                    ui.separator();
                    if ui.button("⟲ -15°").clicked() {
                        self.rotation_angle = (self.rotation_angle - 15.0f32.to_radians()).rem_euclid(2.0 * std::f32::consts::PI);
                    }
                    ui.label(format!("{:.0}°", self.rotation_angle.to_degrees()));
                    if ui.button("⟳ +15°").clicked() {
                        self.rotation_angle = (self.rotation_angle + 15.0f32.to_radians()).rem_euclid(2.0 * std::f32::consts::PI);
                    }
                    ui.separator();
                    let mirror_label = if self.mirror_horizontal { "Mirror: On" } else { "Mirror: Off" };
                    if ui.button(mirror_label).clicked() {
                        self.mirror_horizontal = !self.mirror_horizontal;
                    }
                    ui.separator();
                    ui.label("Stabilizer:");
                    let current_level = self.stabilizer.level;
                    let text = match current_level {
                        StabilizerLevel::Off => "Off".to_string(),
                        StabilizerLevel::Level(val) => format!("L{}", val),
                        StabilizerLevel::SLevel(val) => format!("S-{}", val),
                    };
                    egui::ComboBox::from_id_source("quick_bar_stab")
                        .selected_text(text)
                        .width(60.0)
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(matches!(current_level, StabilizerLevel::Off), "Off").clicked() {
                                self.stabilizer.set_level(StabilizerLevel::Off);
                            }
                            for val in 1..=15 {
                                let is_sel = matches!(current_level, StabilizerLevel::Level(v) if v == val);
                                if ui.selectable_label(is_sel, format!("L{}", val)).clicked() {
                                    self.stabilizer.set_level(StabilizerLevel::Level(val));
                                }
                            }
                            for val in 1..=5 {
                                let is_sel = matches!(current_level, StabilizerLevel::SLevel(v) if v == val);
                                if ui.selectable_label(is_sel, format!("S-{}", val)).clicked() {
                                    self.stabilizer.set_level(StabilizerLevel::SLevel(val));
                                }
                            }
                        });
                    ui.separator();
                    if !self.autosave_status.is_empty() {
                        ui.label(&self.autosave_status);
                    }
                });
            });
        }

        // EXPORT PNG DIALOG
        if self.show_export_png_dialog {
            let mut close = false;
            let mut do_export = false;
            egui::Window::new("Export PNG")
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .resizable(false)
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new("Export Canvas as PNG").strong());
                    });
                    ui.add_space(8.0);
                    egui::Grid::new("export_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                        ui.label("File path:");
                        ui.text_edit_singleline(&mut self.export_png_path);
                        ui.end_row();
                        ui.label("Background:");
                        let mut bg_val = match self.export_png_options.background {
                            crate::export::png::ExportBackground::Transparent => 0,
                            crate::export::png::ExportBackground::White => 1,
                        };
                        egui::ComboBox::from_id_source("export_bg")
                            .selected_text(if bg_val == 0 { "Transparent" } else { "White" })
                            .show_ui(ui, |ui| {
                                if ui.selectable_value(&mut bg_val, 0, "Transparent").changed() { }
                                if ui.selectable_value(&mut bg_val, 1, "White").changed() { }
                            });
                        self.export_png_options.background = if bg_val == 0 {
                            crate::export::png::ExportBackground::Transparent
                        } else {
                            crate::export::png::ExportBackground::White
                        };
                        ui.end_row();
                        ui.label("Scale:");
                        ui.add(egui::Slider::new(&mut self.export_png_options.scale, 0.1..=4.0).text("x"));
                        ui.end_row();
                    });
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui.button("Export").clicked() { do_export = true; }
                        if ui.button("Cancel").clicked() { close = true; }
                    });
                });
            if do_export {
                let path = std::path::Path::new(&self.export_png_path).to_path_buf();
                let layers = self.layers.clone();
                let layer_order = self.layer_order.clone();
                let w = self.canvas_width;
                let h = self.canvas_height;
                let options = self.export_png_options.clone();
                std::thread::spawn(move || {
                    match crate::export::png::export_png(&path, &layers, &layer_order, w, h, &options) {
                        Ok(()) => log::info!("Exported PNG to {:?}", path),
                        Err(e) => log::error!("PNG export failed: {:?}", e),
                    }
                });
                self.show_export_png_dialog = false;
            }
            if close {
                self.show_export_png_dialog = false;
            }
        }

        // KEYBOARD SHORTCUT EDITOR
        if self.show_shortcut_editor {
            egui::Window::new("Keyboard Shortcuts")
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .resizable(true)
                .default_width(550.0)
                .default_height(400.0)
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Search:");
                        ui.text_edit_singleline(&mut self.shortcut_search);
                    });
                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(4.0);

                    let mut close = false;
                    let mut clicked_idx = None;

                    // Capture keyboard input when listening
                    if self.shortcut_listen_idx.is_some() {
                        ui.add_enabled(false, egui::Button::new("Press a key... (Esc to cancel)"));
                        let captured = ctx.input(|i| {
                            for event in &i.events {
                                if let egui::Event::Key { key, pressed: true, modifiers, .. } = event {
                                    let captured_idx = self.shortcut_listen_idx;
                                    if let Some(idx) = captured_idx {
                                        if *key != egui::Key::Escape {
                                            return Some((idx, crate::shortcuts::KeyBinding::from_event(*key, modifiers.ctrl, modifiers.shift, modifiers.alt)));
                                        }
                                    }
                                    return None; // Escape cancels
                                }
                            }
                            None
                        });
                        if let Some((idx, binding)) = captured {
                            if idx < self.shortcuts.entries.len() {
                                self.shortcuts.entries[idx].primary = Some(binding);
                            }
                            self.shortcut_listen_idx = None;
                        }
                        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                            self.shortcut_listen_idx = None;
                        }
                        // Don't render the list while listening
                        return;
                    }

                    egui::ScrollArea::vertical().show(ui, |ui| {
                        let search_lower = self.shortcut_search.to_lowercase();
                        for (entry_idx, entry) in self.shortcuts.entries.iter().enumerate() {
                            let name_lower = entry.name.to_lowercase();
                            let cat_lower = entry.category.to_lowercase();
                            if !search_lower.is_empty() && !name_lower.contains(&search_lower) && !cat_lower.contains(&search_lower) {
                                continue;
                            }

                            let is_editing = self.shortcut_edit_idx == Some(entry_idx);

                            ui.horizontal(|ui| {
                                ui.label(entry.name);
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if is_editing {
                                        if let Some(ref binding) = entry.primary {
                                            if ui.button(binding.display()).clicked() {
                                                self.shortcut_listen_idx = Some(entry_idx);
                                            }
                                        } else {
                                            if ui.button("[none]").clicked() {
                                                self.shortcut_listen_idx = Some(entry_idx);
                                            }
                                        }
                                        if ui.button("Clear").clicked() {
                                            self.shortcut_edit_idx = None;
                                        }
                                    } else {
                                        if let Some(ref binding) = entry.primary {
                                            ui.label(binding.display());
                                        } else {
                                            ui.label("[none]");
                                        }
                                        if ui.button("Edit").clicked() {
                                            clicked_idx = Some(entry_idx);
                                        }
                                    }
                                });
                            });
                            ui.separator();
                        }
                    });

                    if let Some(idx) = clicked_idx {
                        self.shortcut_edit_idx = Some(idx);
                    }

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Reset to Defaults").clicked() {
                            self.shortcuts = crate::shortcuts::ShortcutManager::new();
                            self.shortcut_edit_idx = None;
                            self.shortcut_listen_idx = None;
                        }
                        if ui.button("Close").clicked() {
                            close = true;
                        }
                    });

                    if close {
                        self.show_shortcut_editor = false;
                        self.shortcut_edit_idx = None;
                        self.shortcut_listen_idx = None;
                    }
                });
        }

        // Autosave check
        if self.autosave_enabled {
            let current_time = ctx.input(|i| i.time);
            if current_time - self.last_autosave_time > self.autosave_interval_secs && self.document_modified {
                self.save_canvas(std::path::Path::new(&self.autosave_path));
                self.last_autosave_time = current_time;
                self.document_modified = false;
                self.autosave_status = format!("Autosaved {:?}", std::time::Duration::from_secs_f64(current_time - self.last_autosave_time));
                log::info!("Autosaved to {}", self.autosave_path);
            }
        }

        // Shortcut system: process through ShortcutManager
        if !ctx.wants_keyboard_input() {
            ctx.input(|i| {
                let ctrl = i.modifiers.command;
                let shift = i.modifiers.shift;
                let alt = i.modifiers.alt;

                // Track pressed keys
                for event in &i.events {
                    if let egui::Event::Key { key, pressed: true, .. } = event {
                        if let Some(cmd) = self.shortcuts.find_command(*key, ctrl, shift, alt) {
                            self.command(cmd);
                        }
                    }
                }
            });

            // Brush size shortcuts (always active)
            if ctx.input(|i| i.key_pressed(egui::Key::OpenBracket)) {
                self.brush_radius_log = (self.brush_radius_log - 0.15).max(-1.0);
                self.brush_settings_dirty = true;
            }
            if ctx.input(|i| i.key_pressed(egui::Key::CloseBracket)) {
                self.brush_radius_log = (self.brush_radius_log + 0.15).min(5.0);
                self.brush_settings_dirty = true;
            }
        }

        // 2. LEFT SIDEBAR TOOLPANEL (Creation inputs)
        egui::SidePanel::left("left_sidebar")
            .resizable(false)
            .default_width(220.0)
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    // Brush Tool Selection Grid (Scrollable SAI box)
                    ui.group(|ui| {
                        ui.label("DRAWING TOOLS");

                        egui::ScrollArea::vertical()
                            .max_height(140.0)
                            .show(ui, |ui| {
                                egui::Grid::new("presets_grid")
                                    .num_columns(4)
                                    .spacing([4.0, 4.0])
                                    .show(ui, |ui| {
                                        let num_presets = self.presets.len();
                                        for i in 0..16 {
                                            if i < num_presets {
                                                let preset_icon = self.presets[i].icon;
                                                let preset_name = self.presets[i].name.clone();
                                                let is_selected = self.active_preset_index == i;
                                                
                                                let icon_emoji = match preset_icon {
                                                    PresetIcon::Pencil => "✏",
                                                    PresetIcon::InkPen => "✒",
                                                    PresetIcon::PaintBrush => "🖌",
                                                    PresetIcon::Smudge => "💧",
                                                    PresetIcon::Eraser => "🧹",
                                                };
                                                
                                                let label = format!("{}\n{}", icon_emoji, preset_name);
                                                let btn = egui::Button::new(
                                                    egui::RichText::new(&label)
                                                        .size(9.0)
                                                        .line_height(Some(11.0))
                                                )
                                                .selected(is_selected);
                                                
                                                let btn_response = ui.add_sized([48.0, 48.0], btn);
                                                
                                                // Border highlight if active (contrasting deep blue)
                                                if is_selected {
                                                    ui.painter().rect_stroke(
                                                        btn_response.rect.expand(1.0),
                                                        3.0,
                                                        egui::Stroke::new(2.5, egui::Color32::from_rgb(0, 120, 215))
                                                    );
                                                }
                                                
                                                if btn_response.clicked() {
                                                    self.select_preset(i);
                                                }
                                                
                                                // Right click context menu
                                                btn_response.context_menu(|ui| {
                                                    if ui.button("Rename").clicked() {
                                                        self.renaming_preset_index = Some(i);
                                                        self.rename_input = preset_name.clone();
                                                        ui.close_menu();
                                                    }
                                                    if ui.button("Duplicate").clicked() {
                                                        self.duplicate_preset(i);
                                                        ui.close_menu();
                                                    }
                                                    ui.separator();
                                                    let can_delete = num_presets > 1;
                                                    if ui.add_enabled(can_delete, egui::Button::new("Delete")).clicked() {
                                                        self.delete_preset(i);
                                                        ui.close_menu();
                                                    }
                                                });
                                            } else {
                                                // Empty slot placeholder
                                                let btn = egui::Button::new(
                                                    egui::RichText::new("+")
                                                        .size(16.0)
                                                        .color(egui::Color32::GRAY)
                                                )
                                                .fill(egui::Color32::from_gray(245));
                                                let btn_response = ui.add_sized([48.0, 48.0], btn);
                                                
                                                // Left click or right click context menu to create
                                                let mut show_creation_menu = false;
                                                if btn_response.clicked() {
                                                    show_creation_menu = true;
                                                }
                                                btn_response.context_menu(|ui| {
                                                    ui.label("Create New Brush:");
                                                    if ui.button("✏ Pencil").clicked() {
                                                        self.create_preset(PresetIcon::Pencil);
                                                        ui.close_menu();
                                                    }
                                                    if ui.button("✒ Ink Pen").clicked() {
                                                        self.create_preset(PresetIcon::InkPen);
                                                        ui.close_menu();
                                                    }
                                                    if ui.button("🖌 Paint Brush").clicked() {
                                                        self.create_preset(PresetIcon::PaintBrush);
                                                        ui.close_menu();
                                                    }
                                                    if ui.button("💧 Smudge").clicked() {
                                                        self.create_preset(PresetIcon::Smudge);
                                                        ui.close_menu();
                                                    }
                                                    if ui.button("🧹 Eraser").clicked() {
                                                        self.create_preset(PresetIcon::Eraser);
                                                        ui.close_menu();
                                                    }
                                                    ui.separator();
                                                    ui.label("Import Brush Preset:");
                                                    ui.horizontal(|ui| {
                                                        ui.label("Path:");
                                                        ui.text_edit_singleline(&mut self.brush_import_path);
                                                    });
                                                    if ui.button("📥 Load .artybrush").clicked() {
                                                        let path = std::path::Path::new(&self.brush_import_path);
                                                        match crate::brush_io::load_artybrush(path, &mut self.brush_textures) {
                                                            Ok(mut new_preset) => {
                                                                self.preset_id_counter += 1;
                                                                new_preset.id = self.preset_id_counter;

                                                                let mut brush = Brush::new();
                                                                Self::set_constant(&mut brush, BrushSetting::Radius, new_preset.radius_log);
                                                                Self::set_constant(&mut brush, BrushSetting::Opaque, new_preset.opacity);
                                                                Self::set_constant(&mut brush, BrushSetting::Hardness, new_preset.hardness);
                                                                Self::set_constant(&mut brush, BrushSetting::Smudge, new_preset.color_blending);
                                                                Self::set_constant(&mut brush, BrushSetting::SmudgeLength, new_preset.dilution);
                                                                if new_preset.is_eraser {
                                                                    Self::set_constant(&mut brush, BrushSetting::Eraser, 1.0);
                                                                }

                                                                self.presets.push(new_preset);
                                                                self.brushes.push(brush);
                                                                self.brush_states.push(BrushState::default());

                                                                let new_idx = self.presets.len() - 1;
                                                                self.select_preset(new_idx);
                                                                log::info!("Imported .artybrush successfully!");
                                                            }
                                                            Err(e) => {
                                                                log::error!("Failed to import .artybrush: {:?}", e);
                                                            }
                                                        }
                                                        ui.close_menu();
                                                    }
                                                    if ui.button("⚡ Extract & Import .sut").clicked() {
                                                        let path = std::path::Path::new(&self.brush_import_path);
                                                        match crate::brush_io::extract_sut_texture(path) {
                                                            Ok((gray_bytes, w, h)) => {
                                                                let mut final_bytes = vec![255u8; 256 * 256];
                                                                for y in 0..h.min(256) {
                                                                    for x in 0..w.min(256) {
                                                                        final_bytes[(y * 256 + x) as usize] = gray_bytes[(y * w + x) as usize];
                                                                    }
                                                                }
                                                                self.brush_textures.push(final_bytes);
                                                                let texture_id = (self.brush_textures.len() - 1) as u8;

                                                                self.preset_id_counter += 1;
                                                                let new_preset = BrushPreset {
                                                                    id: self.preset_id_counter,
                                                                    name: path.file_stem().and_then(|s| s.to_str()).unwrap_or("SUT Brush").to_string(),
                                                                    icon: PresetIcon::PaintBrush,
                                                                    radius_log: 2.0,
                                                                    opacity: 1.0,
                                                                    hardness: 0.8,
                                                                    min_size_fraction: 0.2,
                                                                    color_blending: 0.0,
                                                                    dilution: 0.0,
                                                                    is_eraser: false,
                                                                    texture_id,
                                                                    texture_scale: 1.0,
                                                                    bristle_id: 0,
                                                                };

                                                                let mut brush = Brush::new();
                                                                Self::set_constant(&mut brush, BrushSetting::Radius, new_preset.radius_log);
                                                                Self::set_constant(&mut brush, BrushSetting::Opaque, new_preset.opacity);
                                                                Self::set_constant(&mut brush, BrushSetting::Hardness, new_preset.hardness);

                                                                self.presets.push(new_preset);
                                                                self.brushes.push(brush);
                                                                self.brush_states.push(BrushState::default());

                                                                let new_idx = self.presets.len() - 1;
                                                                self.select_preset(new_idx);
                                                                log::info!("Extracted and imported SUT brush successfully!");
                                                            }
                                                            Err(e) => {
                                                                log::error!("Failed to extract SUT: {:?}", e);
                                                            }
                                                        }
                                                        ui.close_menu();
                                                    }
                                                });
                                                
                                                if show_creation_menu {
                                                    ui.ctx().memory_mut(|mem| mem.open_popup(btn_response.id.with("context_menu")));
                                                }
                                            }
                                            
                                            if i % 4 == 3 {
                                                ui.end_row();
                                            }
                                        }
                                    });
                            });

                        // Inline renaming text box
                        if let Some(idx) = self.renaming_preset_index {
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                ui.label("Rename:");
                                let res = ui.add(egui::TextEdit::singleline(&mut self.rename_input).desired_width(100.0));
                                if res.lost_focus() || ui.button("OK").clicked() {
                                    if !self.rename_input.trim().is_empty() {
                                        self.presets[idx].name = self.rename_input.trim().to_string();
                                    }
                                    self.renaming_preset_index = None;
                                }
                                if ui.button("✕").clicked() {
                                    self.renaming_preset_index = None;
                                }
                            });
                        }

                        ui.add_space(6.0);

                        // Stabilizer configuration UI
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.label("Stabilizer:");
                                let current_level = self.stabilizer.level;
                                let text = match current_level {
                                    StabilizerLevel::Off => "Off".to_string(),
                                    StabilizerLevel::Level(val) => format!("Level {}", val),
                                    StabilizerLevel::SLevel(val) => format!("S-{}", val),
                                };
                                let response = egui::ComboBox::from_id_source("side_stabilizer_level")
                                    .selected_text(text)
                                    .width(90.0)
                                    .show_ui(ui, |ui| {
                                        let mut selected = false;
                                        if ui.selectable_label(matches!(current_level, StabilizerLevel::Off), "Off").clicked() {
                                            self.stabilizer.set_level(StabilizerLevel::Off);
                                            selected = true;
                                        }
                                        for val in 1..=15 {
                                            let is_sel = match current_level {
                                                StabilizerLevel::Level(v) => v == val,
                                                _ => false,
                                            };
                                            if ui.selectable_label(is_sel, format!("Level {}", val)).clicked() {
                                                self.stabilizer.set_level(StabilizerLevel::Level(val));
                                                selected = true;
                                            }
                                        }
                                        for val in 1..=5 {
                                            let is_sel = match current_level {
                                                StabilizerLevel::SLevel(v) => v == val,
                                                _ => false,
                                            };
                                            if ui.selectable_label(is_sel, format!("S-{}", val)).clicked() {
                                                self.stabilizer.set_level(StabilizerLevel::SLevel(val));
                                                selected = true;
                                            }
                                        }
                                        selected
                                    });
                                if response.inner.unwrap_or(false) {
                                    ctx.request_repaint();
                                }
                            });

                            ui.horizontal(|ui| {
                                ui.label("Mode:");
                                let current_mode = self.stabilizer.mode;
                                let mode_text = match current_mode {
                                    StabilizerMode::Ema => "EMA",
                                    StabilizerMode::SpringMassDamper => "Spring Physics",
                                };
                                let response = egui::ComboBox::from_id_source("side_stabilizer_mode")
                                    .selected_text(mode_text)
                                    .width(120.0)
                                    .show_ui(ui, |ui| {
                                        let mut selected = false;
                                        if ui.selectable_label(current_mode == StabilizerMode::Ema, "EMA").clicked() {
                                            self.stabilizer.mode = StabilizerMode::Ema;
                                            selected = true;
                                        }
                                        if ui.selectable_label(current_mode == StabilizerMode::SpringMassDamper, "Spring Physics").clicked() {
                                            self.stabilizer.mode = StabilizerMode::SpringMassDamper;
                                            selected = true;
                                        }
                                        selected
                                    });
                                if response.inner.unwrap_or(false) {
                                    ctx.request_repaint();
                                }
                            });
                        });
                    });

                    ui.add_space(5.0);

                    // Dynamic Tool Options - changes based on active tool
                    ui.group(|ui| {
                        ui.label("TOOL OPTIONS");
                        match self.active_tool {
                            ToolId::Fill => {
                                ui.horizontal(|ui| {
                                    ui.label("Tolerance:");
                                    ui.add(egui::Slider::new(&mut self.fill_options.tolerance, 0..=255));
                                });
                                ui.horizontal(|ui| {
                                    ui.label("Expand:");
                                    ui.add(egui::Slider::new(&mut self.fill_options.expand_px, 0..=10));
                                });
                                ui.checkbox(&mut self.fill_options.sample_all_layers, "Sample all layers");
                                ui.checkbox(&mut self.fill_options.respect_selection, "Respect selection");
                            }
                            ToolId::RectSelect | ToolId::EllipseSelect | ToolId::Lasso => {
                                ui.horizontal(|ui| {
                                    ui.label("Mode:");
                                    egui::ComboBox::from_id_source("sel_mode")
                                        .selected_text(match self.selection_mode {
                                            selection::SelectionMode::Replace => "Replace",
                                            selection::SelectionMode::Add => "Add",
                                            selection::SelectionMode::Subtract => "Subtract",
                                            selection::SelectionMode::Intersect => "Intersect",
                                        })
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(&mut self.selection_mode, selection::SelectionMode::Replace, "Replace");
                                            ui.selectable_value(&mut self.selection_mode, selection::SelectionMode::Add, "Add");
                                            ui.selectable_value(&mut self.selection_mode, selection::SelectionMode::Subtract, "Subtract");
                                            ui.selectable_value(&mut self.selection_mode, selection::SelectionMode::Intersect, "Intersect");
                                        });
                                });
                                ui.horizontal(|ui| {
                                    ui.label("Feather:");
                                    ui.add(egui::Slider::new(&mut self.selection_feather, 0.0..=100.0));
                                });
                            }
                            ToolId::MagicWand => {
                                ui.horizontal(|ui| {
                                    ui.label("Tolerance:");
                                    ui.add(egui::Slider::new(&mut self.fill_options.tolerance, 0..=255));
                                });
                                ui.checkbox(&mut self.fill_options.sample_all_layers, "Sample all layers");
                            }
                            ToolId::Transform => {
                                ui.horizontal(|ui| {
                                    ui.label("Interp:");
                                    egui::ComboBox::from_id_source("interp")
                                        .selected_text(match self.transform_state.interpolation {
                                            transform_tool::InterpolationMode::Nearest => "Nearest",
                                            transform_tool::InterpolationMode::Bilinear => "Bilinear",
                                            transform_tool::InterpolationMode::Bicubic => "Bicubic",
                                        })
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(&mut self.transform_state.interpolation, transform_tool::InterpolationMode::Nearest, "Nearest");
                                            ui.selectable_value(&mut self.transform_state.interpolation, transform_tool::InterpolationMode::Bilinear, "Bilinear");
                                            ui.selectable_value(&mut self.transform_state.interpolation, transform_tool::InterpolationMode::Bicubic, "Bicubic");
                                        });
                                });
                            }
                            ToolId::ColorPicker => {
                                ui.label("Picks color from canvas");
                            }
                            _ => {
                                ui.label("No options for this tool");
                            }
                        }
                    });

                    ui.add_space(5.0);

                    // Brush Settings Sliders (only show for brush-like tools)
                    if matches!(self.active_tool, ToolId::Brush | ToolId::Eraser) {
                        ui.group(|ui| {
                        ui.label("BRUSH CONFIGURATION");

                        // Brush preview circle + size slider
                        let pixel_radius = self.brush_radius_log.exp();
                        ui.horizontal(|ui| {
                            // Draw a circle showing the real brush size (clamped to 60px UI max)
                            let preview_r = (pixel_radius * self.viewport_zoom).clamp(3.0, 60.0);
                            let (resp, painter) = ui.allocate_painter(
                                egui::Vec2::splat(preview_r * 2.0 + 4.0),
                                egui::Sense::hover(),
                            );
                            let center = resp.rect.center();
                            let brush_color32 = egui::Color32::from_rgb(
                                (self.brush_color[0] * 255.0) as u8,
                                (self.brush_color[1] * 255.0) as u8,
                                (self.brush_color[2] * 255.0) as u8,
                            );
                            painter.circle_filled(center, preview_r, brush_color32);
                            painter.circle_stroke(
                                center,
                                preview_r,
                                egui::Stroke::new(1.0, egui::Color32::from_gray(80)),
                            );
                            ui.vertical(|ui| {
                                ui.label(format!("Size: {:.1} px", pixel_radius));
                                if ui.add(
                                    egui::Slider::new(&mut self.brush_radius_log, -1.0..=5.0)
                                        .show_value(false),
                                ).changed() {
                                    self.brush_settings_dirty = true;
                                }
                            });
                        });

                        // Opacity
                        ui.horizontal(|ui| {
                            ui.label("Opacity:");
                            if ui.add(egui::Slider::new(&mut self.brush_opacity, 0.0..=1.0)).changed() {
                                self.brush_settings_dirty = true;
                            }
                        });

                        // Hardness
                        ui.horizontal(|ui| {
                            ui.label("Hardness:");
                            if ui.add(egui::Slider::new(&mut self.brush_hardness, 0.0..=1.0)).changed() {
                                self.brush_settings_dirty = true;
                            }
                        });

                        // Min Size % — controls thin-to-thick pressure range
                        ui.horizontal(|ui| {
                            ui.label("Min Size %:");
                            if ui.add(egui::Slider::new(&mut self.brush_min_size_fraction, 0.0..=1.0)).changed() {
                                self.brush_settings_dirty = true;
                            }
                        });

                        // Color Blending (Smudge)
                        ui.horizontal(|ui| {
                            ui.label("Blending:");
                            if ui.add(egui::Slider::new(&mut self.brush_color_blending, 0.0..=1.0)).changed() {
                                self.brush_settings_dirty = true;
                            }
                        });

                        // Dilution (Water Amount)
                        ui.horizontal(|ui| {
                            ui.label("Dilution:");
                            if ui.add(egui::Slider::new(&mut self.brush_dilution, 0.0..=1.0)).changed() {
                                self.brush_settings_dirty = true;
                            }
                        });

                        // Eraser Checkbox
                        if !self.presets.is_empty() {
                            let is_eraser = &mut self.presets[self.active_preset_index].is_eraser;
                            if ui.checkbox(is_eraser, "Eraser Mode [E]").changed() {
                                self.brush_settings_dirty = true;
                            }
                        }

                        // Texture Dropdown
                        ui.horizontal(|ui| {
                            ui.label("Texture:");
                            let mut selected_tex = self.brush_texture_id;
                            let res = egui::ComboBox::from_id_source("brush_texture_combo")
                                .selected_text(match selected_tex {
                                    0 => "None",
                                    1 => "Noise",
                                    2 => "Bristle",
                                    _ => "Unknown",
                                })
                                .show_ui(ui, |ui| {
                                    let mut changed = false;
                                    if ui.selectable_value(&mut selected_tex, 0, "None").clicked() { changed = true; }
                                    if ui.selectable_value(&mut selected_tex, 1, "Noise").clicked() { changed = true; }
                                    if ui.selectable_value(&mut selected_tex, 2, "Bristle").clicked() { changed = true; }
                                    changed
                                });
                            if res.inner.unwrap_or(false) {
                                self.brush_texture_id = selected_tex;
                                self.brush_settings_dirty = true;
                            }
                        });

                        // Texture Scale Slider
                        if self.brush_texture_id > 0 {
                            ui.horizontal(|ui| {
                                ui.label("Tex Scale:");
                                if ui.add(egui::Slider::new(&mut self.brush_texture_scale, 0.1..=10.0)).changed() {
                                    self.brush_settings_dirty = true;
                                }
                            });
                        }

                        // Bristle ID Slider
                        ui.horizontal(|ui| {
                            ui.label("Bristle ID:");
                            if ui.add(egui::Slider::new(&mut self.brush_bristle_id, 0..=5)).changed() {
                                self.brush_settings_dirty = true;
                            }
                        });

                        // Lock Canvas Bounds
                        ui.checkbox(&mut self.lock_canvas_bounds, "Lock Canvas Bounds");

                        ui.add_space(5.0);

                        // Advanced / debug information collapsed by default to prevent visual noise
                        ui.collapsing("Debug / Advanced Info", |ui| {
                            ui.horizontal(|ui| {
                                ui.label("Pressure response:");
                                ui.add(
                                    egui::Slider::new(&mut self.pressure_curve, 0.25..=2.50)
                                        .text("curve"),
                                );
                            });
                            ui.horizontal(|ui| {
                                ui.label("Min pressure:");
                                ui.add(
                                    egui::Slider::new(&mut self.pressure_min, 0.00..=0.30)
                                        .text("floor"),
                                );
                            });

                            let raw_display = self.egui_touch_pressure.unwrap_or(self.tablet_axis.pressure).clamp(0.0, 1.0);
                            let raw_level = (raw_display * 8191.0).round() as u32;

                            let smoothed_display = self.stabilizer.last_smoothed_pressure.unwrap_or(raw_display).clamp(0.0, 1.0);
                            let smoothed_level = (smoothed_display * 8191.0).round() as u32;

                            let remapped_display = self.remap_pressure(smoothed_display);

                            ui.label(format!("Raw Pen:  {:.4} / 8192 ({})", raw_display, raw_level));
                            ui.label(format!("Smoothed: {:.4} / 8192 ({})", smoothed_display, smoothed_level));
                            ui.label(format!("Remapped: {:.4}", remapped_display));

                            // Visual pressure bar
                            let pressure_frac = remapped_display;
                            let bar_rect = ui.available_rect_before_wrap();
                            let bar_width = bar_rect.width().min(190.0);
                            let bar_height = 10.0;
                            let (bar_response, painter) = ui.allocate_painter(
                                egui::Vec2::new(bar_width, bar_height), egui::Sense::hover()
                            );
                            let r = bar_response.rect;
                            painter.rect_filled(r, 2.0, egui::Color32::from_gray(60));
                            let filled = egui::Rect::from_min_max(
                                r.min,
                                egui::Pos2::new(r.min.x + r.width() * pressure_frac, r.max.y),
                            );
                            painter.rect_filled(filled, 2.0, egui::Color32::from_rgb(100, 180, 255));
                        });
                    });
                }
            });
            });

        // 3. RIGHT SIDEBAR UTILITY PANEL (Asset Management & Color Picking)
        egui::SidePanel::right("right_sidebar")
            .resizable(false)
            .default_width(260.0)
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    // NAVIGATOR PANEL
                    ui.group(|ui| {
                        ui.label("NAVIGATOR");
                        ui.vertical_centered(|ui| {
                            let (rect, _response) = ui.allocate_exact_size(egui::vec2(240.0, 240.0), egui::Sense::hover());
                            if let Some(r) = &self.renderer {
                                if let Some(texture_id) = r.navigator_egui_id {
                                    ui.painter().image(
                                        texture_id,
                                        rect,
                                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                        Color32::WHITE,
                                    );
                                }
                            }
                        });
                    });
                    ui.add_space(5.0);

                    // COLOR SELECTOR
                    ui.group(|ui| {
                        ui.label("COLOR SELECTOR");

                        // Custom HSV Color Wheel
                        ui.vertical_centered(|ui| {
                            if draw_hsv_color_wheel(ui, &mut self.brush_color).changed() {
                                self.brush_settings_dirty = true;
                            }
                        });

                        ui.add_space(5.0);

                        // RGB/HEX preview and text representation
                        ui.horizontal(|ui| {
                            let mut color32 = Color32::from_rgb(
                                (self.brush_color[0] * 255.0) as u8,
                                (self.brush_color[1] * 255.0) as u8,
                                (self.brush_color[2] * 255.0) as u8,
                            );

                            if egui::color_picker::color_edit_button_srgba(
                                ui,
                                &mut color32,
                                egui::color_picker::Alpha::Opaque,
                            ).changed() {
                                self.brush_color[0] = color32.r() as f32 / 255.0;
                                self.brush_color[1] = color32.g() as f32 / 255.0;
                                self.brush_color[2] = color32.b() as f32 / 255.0;
                                self.brush_settings_dirty = true;
                            }

                            let hex_str = format!(
                                "#{:02X}{:02X}{:02X}",
                                color32.r(),
                                color32.g(),
                                color32.b()
                            );
                            ui.label(hex_str);
                        });

                        ui.add_space(4.0);
                        let mut sync_needed = false;
                        egui::Grid::new("color_palette")
                            .num_columns(6)
                            .spacing([4.0, 4.0])
                            .show(ui, |ui| {
                                for (i, color) in self.palette.iter_mut().enumerate() {
                                    let fill = Color32::from_rgb(
                                        (color[0] * 255.0) as u8,
                                        (color[1] * 255.0) as u8,
                                        (color[2] * 255.0) as u8,
                                    );
                                    let is_selected_swatch = self.selected_palette_index == Some(i);
                                    let btn_response = ui.add(
                                        egui::Button::new("")
                                            .min_size(Vec2::splat(22.0))
                                            .fill(fill),
                                    );
                                    if is_selected_swatch {
                                        ui.painter().rect_stroke(
                                            btn_response.rect.expand(1.5),
                                            1.0,
                                            egui::Stroke::new(2.0, egui::Color32::from_rgb(0, 120, 215))
                                        );
                                    }
                                    if btn_response.clicked() {
                                        self.brush_color = *color;
                                        self.selected_palette_index = Some(i);
                                        sync_needed = true;
                                    }
                                    if i % 6 == 5 {
                                        ui.end_row();
                                    }
                                }
                            });
                        if sync_needed {
                            self.brush_settings_dirty = true;
                        }
                        ui.horizontal(|ui| {
                            if ui.button("Save").clicked() {
                                if let Some(i) = self.selected_palette_index {
                                    if let Some(slot) = self.palette.get_mut(i) {
                                        *slot = self.brush_color;
                                    }
                                }
                            }
                            if ui.button("+").clicked() && self.palette.len() < 36 {
                                self.palette.push(self.brush_color);
                                self.selected_palette_index = Some(self.palette.len() - 1);
                            }
                            if ui
                                .add_enabled(
                                    self.selected_palette_index.is_some() && self.palette.len() > 1,
                                    egui::Button::new("-"),
                                )
                                .clicked()
                            {
                                if let Some(i) = self.selected_palette_index.take() {
                                    if i < self.palette.len() {
                                        self.palette.remove(i);
                                    }
                                }
                            }
                        });
                    });

                    ui.add_space(5.0);

                    // Layer Manager Tree
                    ui.group(|ui| {
                        ui.label("LAYERS MANAGER");

                        ui.horizontal(|ui| {
                            if ui.button("+ Raster").clicked() {
                                self.layer_id_counter += 1;
                                let new_id = self.layer_id_counter;
                                let mut new_layer = Layer::new(new_id, format!("Layer {}", new_id));
                                new_layer.kind = crate::canvas::LayerType::Raster;
                                self.layers.insert(new_id, new_layer);
                                self.layer_order.insert(0, new_id); // Add on top
                                self.active_layer_id = new_id;
                            }
                            if ui.button("+ Folder").clicked() {
                                self.layer_id_counter += 1;
                                let new_id = self.layer_id_counter;
                                let mut new_layer = Layer::new(new_id, format!("Folder {}", new_id));
                                new_layer.kind = crate::canvas::LayerType::Folder { child_ids: Vec::new() };
                                self.layers.insert(new_id, new_layer);
                                self.layer_order.insert(0, new_id); // Add on top
                                self.active_layer_id = new_id;
                            }
                            if ui.button("+ Vector").clicked() {
                                self.layer_id_counter += 1;
                                let new_id = self.layer_id_counter;
                                let mut new_layer = Layer::new(new_id, format!("Vector {}", new_id));
                                new_layer.kind = crate::canvas::LayerType::Vector;
                                new_layer.vector_data = Some(crate::canvas::VectorLayer { strokes: Vec::new() });
                                self.layers.insert(new_id, new_layer);
                                self.layer_order.insert(0, new_id); // Add on top
                                self.active_layer_id = new_id;
                            }

                            if ui
                                .add_enabled(
                                    self.layer_order.len() > 1,
                                    egui::Button::new("- Delete"),
                                )
                                .clicked()
                            {
                                let active_id = self.active_layer_id;
                                if let Some(pos) =
                                    self.layer_order.iter().position(|&x| x == active_id)
                                {
                                    self.layer_order.remove(pos);
                                    self.layers.remove(&active_id);
                                    self.active_layer_id = self.layer_order[0];
                                }
                            }
                        });

                        ui.add_space(5.0);

                        // Active Layer blending options
                        if let Some(active_layer) = self.layers.get_mut(&self.active_layer_id) {
                            ui.horizontal(|ui| {
                                ui.label("Mode:");
                                egui::ComboBox::from_id_source("blend_mode_dropdown")
                                    .selected_text(format!("{:?}", active_layer.blend_mode))
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut active_layer.blend_mode,
                                            BlendMode::Normal,
                                            "Normal",
                                        );
                                        ui.selectable_value(
                                            &mut active_layer.blend_mode,
                                            BlendMode::Multiply,
                                            "Multiply",
                                        );
                                        ui.selectable_value(
                                            &mut active_layer.blend_mode,
                                            BlendMode::Screen,
                                            "Screen",
                                        );
                                        ui.selectable_value(
                                            &mut active_layer.blend_mode,
                                            BlendMode::Overlay,
                                            "Overlay",
                                        );
                                        ui.selectable_value(
                                            &mut active_layer.blend_mode,
                                            BlendMode::Luminosity,
                                            "Luminosity (Shine)",
                                        );
                                        ui.selectable_value(
                                            &mut active_layer.blend_mode,
                                            BlendMode::Shade,
                                            "Shade",
                                        );
                                    });
                            });

                            ui.horizontal(|ui| {
                                ui.label("Opacity:");
                                ui.add(egui::Slider::new(&mut active_layer.opacity, 0.0..=1.0).show_value(false));
                            });

                            ui.horizontal(|ui| {
                                ui.checkbox(&mut active_layer.lock_alpha, "Lock Alpha");
                                ui.checkbox(&mut active_layer.is_clipping, "Clipping Group");
                            });
                        }

                        ui.separator();

                        // Scrollable Layer Selection List
                        egui::ScrollArea::vertical()
                            .max_height(160.0)
                            .show(ui, |ui| {
                                let order = self.layer_order.clone();
                                for id in order {
                                    let pointer_released =
                                        ui.ctx().input(|i| i.pointer.any_released());
                                    let is_active = self.active_layer_id == id;
                                    let mut row_hovered = false;

                                    ui.horizontal(|ui| {
                                        let drag_response = ui.add(
                                            egui::Label::new("::")
                                                .sense(egui::Sense::click_and_drag()),
                                        );
                                        row_hovered |= drag_response.hovered();
                                        if drag_response.drag_started() {
                                            self.dragging_layer_id = Some(id);
                                            self.active_layer_id = id;
                                        }
                                        let layer = self.layers.get_mut(&id).unwrap();

                                        // Visibility check
                                        let visible_response = ui.checkbox(&mut layer.visible, "");
                                        row_hovered |= visible_response.hovered();

                                        // Highlight active layer
                                        let prefix = match &layer.kind {
                                            crate::canvas::LayerType::Folder { .. } => "📁 ",
                                            crate::canvas::LayerType::Vector => "🖋 ",
                                            crate::canvas::LayerType::Raster => "🖼 ",
                                        };
                                        let display_name = format!("{}{}", prefix, layer.name);
                                        let label_response = ui.add(egui::SelectableLabel::new(
                                            is_active,
                                            &display_name,
                                        ));
                                        if is_active {
                                            ui.painter().rect_stroke(
                                                label_response.rect.expand(1.0),
                                                1.0,
                                                egui::Stroke::new(2.0, egui::Color32::from_rgb(0, 120, 215))
                                            );
                                        }
                                        row_hovered |= label_response.hovered();
                                        if label_response.clicked() {
                                            self.active_layer_id = id;
                                        }
                                    });

                                    if let Some(dragging_id) = self.dragging_layer_id {
                                        if dragging_id != id && row_hovered {
                                            if let (Some(from), Some(to)) = (
                                                self.layer_order
                                                    .iter()
                                                    .position(|&layer_id| layer_id == dragging_id),
                                                self.layer_order
                                                    .iter()
                                                    .position(|&layer_id| layer_id == id),
                                            ) {
                                                self.layer_order.swap(from, to);
                                            }
                                        }
                                        if pointer_released {
                                            self.dragging_layer_id = None;
                                        }
                                    }
                                }
                            });
                    });
                });
            });

        // 4. BOTTOM STATUS BAR
        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let tool_name = self.active_tool.name();
                ui.label(format!("Tool: {}", tool_name));
                ui.separator();

                if matches!(self.active_tool, ToolId::Brush | ToolId::Eraser) {
                    let px_radius = self.brush_radius_log.exp();
                    ui.label(format!("Size: {:.1}px", px_radius));
                    ui.separator();

                    let pct = (self.brush_opacity * 100.0).round();
                    ui.label(format!("Opacity: {:.0}%", pct));
                    ui.separator();
                }

                let pressure = self.last_ptr_pressure;
                ui.label(format!("Pressure: {:.2}", pressure));
                ui.separator();

                ui.label(format!("Canvas: {}x{}", self.canvas_width, self.canvas_height));
                ui.separator();

                ui.label(format!("Zoom: {:.1}%", self.viewport_zoom * 100.0));
                ui.separator();

                let angle_deg = self.rotation_angle.to_degrees().round();
                ui.label(format!("Rot: {:.0}\u{b0}", angle_deg));
                ui.separator();

                let mirror_state = if self.mirror_horizontal { "Mirror: On" } else { "Mirror: Off" };
                ui.label(mirror_state);
                ui.separator();

                let layer_name = self.layers.get(&self.active_layer_id)
                    .map(|l| l.name.as_str())
                    .unwrap_or("(none)");
                ui.label(format!("Layer: {}", layer_name));
                ui.separator();

                ui.label(&self.autosave_status);
            });
        });

        // 3. CENTRAL PANEL (DRAWING AREA)
        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                let rect = ui.max_rect();
                let response = ui.allocate_response(rect.size(), egui::Sense::click_and_drag());

                // Check for viewport resizing to adapt offscreen WGPU textures
                if let Some(renderer) = &mut self.renderer {
                    if let Some(wgpu_state) = frame.wgpu_render_state() {
                        renderer.resize_viewport(
                            wgpu_state,
                            rect.width() as u32,
                            rect.height() as u32,
                        );
                    }
                }

                let space_down = ui.input(|i| i.key_down(egui::Key::Space)) && !ui.ctx().wants_keyboard_input();
                let r_down = ui.input(|i| i.key_down(egui::Key::R)) && !ui.ctx().wants_keyboard_input();

                if space_down {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                } else if r_down {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }

                // Infinite canvas panning: drag with middle or right mouse button (transformed to view rotation/mirror)
                if response.dragged_by(egui::PointerButton::Middle)
                    || response.dragged_by(egui::PointerButton::Secondary)
                    || (space_down && response.dragged_by(egui::PointerButton::Primary))
                {
                    let delta = response.drag_delta();
                    let half_w = rect.width() * 0.5;
                    let half_h = rect.height() * 0.5;
                    
                    let nx = delta.x / half_w;
                    let ny = -delta.y / half_h;
                    
                    let cos_rot = (-self.rotation_angle).cos();
                    let sin_rot = (-self.rotation_angle).sin();
                    let mut px = nx * cos_rot - ny * sin_rot;
                    let py = nx * sin_rot + ny * cos_rot;
                    
                    if self.mirror_horizontal {
                        px = -px;
                    }
                    
                    let rx = px * half_w;
                    let ry = -py * half_h;
                    
                    self.viewport_offset -= egui::vec2(rx, ry) / self.viewport_zoom;
                    if space_down {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                    }
                }

                // Rotation dragging using R key + primary drag
                if r_down && response.dragged_by(egui::PointerButton::Primary) {
                    let drag_delta = response.drag_delta();
                    self.rotation_angle += drag_delta.x * 0.005;
                }

                // Infinite canvas zooming: mouse wheel scroll
                let scroll_delta = ui.input(|i| i.smooth_scroll_delta.y);
                if scroll_delta != 0.0 {
                    let prev_zoom = self.viewport_zoom;
                    self.viewport_zoom =
                        (self.viewport_zoom + scroll_delta * 0.005).clamp(0.1, 10.0);

                    // Keep the zoom centered on the pointer position
                    if let Some(hover_pos) = response.hover_pos() {
                        let ptr_world = (hover_pos.to_vec2() - rect.min.to_vec2()) / prev_zoom
                            + self.viewport_offset;
                        self.viewport_offset = ptr_world
                            - (hover_pos.to_vec2() - rect.min.to_vec2()) / self.viewport_zoom;
                    }
                }

                // STROKE DRAWING INTERACTION
                let pointer_down = response.dragged_by(egui::PointerButton::Primary) && !space_down && !r_down;
                let pointer_clicked = response.clicked_by(egui::PointerButton::Primary) && !space_down && !r_down;

                // Handle selection tool dragging
                if pointer_down && matches!(self.active_tool, ToolId::RectSelect | ToolId::EllipseSelect) {
                    if let Some(ptr_pos) = response.hover_pos() {
                        let world_pos = self.screen_to_world(ptr_pos, rect);
                        if !self.is_selecting {
                            self.is_selecting = true;
                            selection::clear_selection(&mut self.selection_mask);
                            self.selection_rect = Some(selection::SelectionRect {
                                x0: world_pos.x, y0: world_pos.y,
                                x1: world_pos.x, y1: world_pos.y,
                            });
                        }
                        if let Some(ref mut sr) = self.selection_rect {
                            sr.x1 = world_pos.x;
                            sr.y1 = world_pos.y;
                        }
                        ctx.request_repaint();
                    }
                }

                // Handle lasso dragging
                if pointer_down && matches!(self.active_tool, ToolId::Lasso) {
                    if let Some(ptr_pos) = response.hover_pos() {
                        let world_pos = self.screen_to_world(ptr_pos, rect);
                        if !self.is_selecting {
                            self.is_selecting = true;
                            selection::clear_selection(&mut self.selection_mask);
                            self.lasso_points = Some(selection::LassoPoints { points: Vec::new() });
                        }
                        if let Some(ref mut lp) = self.lasso_points {
                            lp.points.push((world_pos.x, world_pos.y));
                        }
                        ctx.request_repaint();
                    }
                }

                // Handle fill tool click
                if pointer_clicked && matches!(self.active_tool, ToolId::Fill) {
                    if let Some(ptr_pos) = response.hover_pos() {
                        let world_pos = self.screen_to_world(ptr_pos, rect);
                        let fx = world_pos.x as i32;
                        let fy = world_pos.y as i32;
                        if fx >= 0 && fx < self.canvas_width as i32 && fy >= 0 && fy < self.canvas_height as i32 {
                            let fill_color: [u16; 4] = [
                                (self.brush_color[0] * 32768.0) as u16,
                                (self.brush_color[1] * 32768.0) as u16,
                                (self.brush_color[2] * 32768.0) as u16,
                                32768,
                            ];
                            let all_layers: Vec<Layer> = self.layers.values().cloned().collect();
                            let all_layers_ref: Option<&[Layer]> = if self.fill_options.sample_all_layers {
                                Some(&all_layers)
                            } else {
                                None
                            };
                            if let Some(layer) = self.layers.get_mut(&self.active_layer_id) {
                                let dirty = fill::flood_fill(
                                    layer,
                                    all_layers_ref,
                                    &self.selection_mask,
                                    fx, fy,
                                    fill_color,
                                    &self.fill_options,
                                );
                                if !dirty.is_empty() {
                                    self.document_modified = true;
                                    if let Some(r) = &mut self.renderer {
                                        r.clear_cache();
                                    }
                                }
                            }
                        }
                    }
                }

                // Handle brush/eraser stroke drawing
                if pointer_down && matches!(self.active_tool, ToolId::Brush | ToolId::Eraser) {
                    if let Some(ptr_pos) = response.hover_pos() {
                        let rect_center = rect.center();
                        let half_w = rect.width() * 0.5;
                        let half_h = rect.height() * 0.5;
                        
                        let dx = ptr_pos.x - rect_center.x;
                        let dy = ptr_pos.y - rect_center.y;
                        
                        let nx = dx / half_w;
                        let ny = -dy / half_h;
                        
                        let cos_rot = (-self.rotation_angle).cos();
                        let sin_rot = (-self.rotation_angle).sin();
                        let mut px = nx * cos_rot - ny * sin_rot;
                        let py = nx * sin_rot + ny * cos_rot;
                        
                        if self.mirror_horizontal {
                            px = -px;
                        }
                        
                        let world_pos = egui::Vec2::new(
                            ((px + 1.0) * half_w) / self.viewport_zoom + self.viewport_offset.x,
                            ((1.0 - py) * half_h) / self.viewport_zoom + self.viewport_offset.y,
                        );

                        // Clamp drawing coordinates within the defined canvas bounds
                        let cx = world_pos.x.clamp(0.0, self.canvas_width as f32);
                        let cy = world_pos.y.clamp(0.0, self.canvas_height as f32);

                        // Initialize the drawing state if this is a fresh stroke
                        if !self.stabilizer.is_drawing {
                            self.stabilizer.is_drawing = true;
                            self.stroke_id = self.stroke_id.wrapping_add(1);
                            self.last_event_time = ctx.input(|i| i.time) - 0.016; // Seed last event time
                            self.current_stroke_snapshots.clear();
                            self.current_vector_points.clear();

                            // Call begin_atomic on active drawing layer
                            if let Some(active_layer) = self.layers.get_mut(&self.active_layer_id) {
                                active_layer.begin_atomic();
                            }
                        }

                        let cur_time = ctx.input(|i| i.time);
                        let dt = (cur_time - self.last_event_time).max(0.001);
                        self.last_event_time = cur_time;

                        // =============================================================
                        // PRESSURE & TILT: pen pressure with velocity simulation fallback
                        // =============================================================
                        let raw_tilt_x = self.tablet_axis.tilt_x.unwrap_or(0.0);
                        let raw_tilt_y = self.tablet_axis.tilt_y.unwrap_or(0.0);

                        let raw_pressure = if let Some(force) = self.egui_touch_pressure {
                            force.max(0.05)
                        } else if self.input_manager.is_some() {
                            // Native tablet connected — use real hardware pressure.
                            self.tablet_axis.pressure.max(0.05)
                        } else {
                            // No tablet detected — use velocity simulation as fallback based on raw cursor position
                            if let Some(last_pos) = self.last_ptr_pos {
                                let dx = cx - last_pos.x;
                                let dy = cy - last_pos.y;
                                let dist = (dx * dx + dy * dy).sqrt();
                                let velocity = dist / dt as f32;
                                // Exponential decay for natural mouse-pressure response
                                let speed_factor = (-velocity / 400.0).exp();
                                let pressure = speed_factor * 0.85 + 0.10; // Range [0.10, 0.95]
                                pressure.clamp(0.05, 0.95)
                            } else {
                                0.25 // Tapered start of stroke
                            }
                        };

                        // Stabilize position, pressure, and tilt together!
                        let (sx, sy, smoothed_pressure, smoothed_tilt_x, smoothed_tilt_y) =
                            self.stabilizer.process(cx, cy, raw_pressure, raw_tilt_x, raw_tilt_y, dt as f32);

                        // Remap pressure if it comes from real hardware
                        let pressure = if self.egui_touch_pressure.is_some() || self.input_manager.is_some() {
                            self.remap_pressure(smoothed_pressure)
                        } else {
                            smoothed_pressure
                        };
                        self.last_ptr_pressure = pressure;

                        let tilt_x = smoothed_tilt_x;
                        let tilt_y = smoothed_tilt_y;

                        let is_vector = if let Some(layer) = self.layers.get(&self.active_layer_id) {
                            layer.kind == crate::canvas::LayerType::Vector
                        } else {
                            false
                        };

                        if is_vector {
                            let cp = crate::canvas::VectorControlPoint {
                                x: sx,
                                y: sy,
                                pressure,
                                tilt_x,
                                tilt_y,
                            };
                            self.current_vector_points.push(cp);

                            self.sync_brush_settings();
                            let k = self.current_vector_points.len();
                            if k >= 3 {
                                // Draw segment between P_{k-3} and P_{k-2}
                                let p0 = if k >= 4 {
                                    &self.current_vector_points[k - 4]
                                } else {
                                    &self.current_vector_points[k - 3]
                                };
                                let p1 = &self.current_vector_points[k - 3];
                                let p2 = &self.current_vector_points[k - 2];
                                let p3 = &self.current_vector_points[k - 1];

                                let dist = ((p2.x - p1.x).powi(2) + (p2.y - p1.y).powi(2)).sqrt();
                                let steps = ((dist / 2.0) as usize).max(2).min(100);

                                let start_i = if k == 3 { 0 } else { 1 };

                                let active_brush = &self.brushes[self.active_preset_index];
                                let active_brush_state = &mut self.brush_states[self.active_preset_index];

                                if let Some(active_layer) = self.layers.get_mut(&self.active_layer_id) {
                                    let preset = &self.presets[self.active_preset_index];
                                    let tex_idx = preset.texture_id as usize;
                                    let brush_texture = if tex_idx > 0 && tex_idx < self.brush_textures.len() {
                                        Some(self.brush_textures[tex_idx].as_slice())
                                    } else {
                                        None
                                    };

                                    for i in start_i..=steps {
                                        let t = i as f32 / steps as f32;
                                        let pt = Self::catmull_rom(p0, p1, p2, p3, t);

                                        let mut stroke_surface = StrokeSurface {
                                            layer: active_layer,
                                            history: &mut self.history,
                                            snapshots: &mut self.current_stroke_snapshots,
                                            stroke_id: self.stroke_id,
                                            canvas_width: self.canvas_width,
                                            canvas_height: self.canvas_height,
                                            lock_canvas_bounds: self.lock_canvas_bounds,
                                            selection_mask: Some(&self.selection_mask),
                                            brush_texture,
                                            brush_texture_width: 256,
                                            brush_texture_height: 256,
                                            brush_texture_scale: preset.texture_scale,
                                        };

                                        active_brush.stroke_to(
                                            active_brush_state,
                                            &mut stroke_surface,
                                            pt.x,
                                            pt.y,
                                            pt.pressure,
                                            pt.tilt_x,
                                            pt.tilt_y,
                                            dt / steps as f64,
                                        );
                                    }
                                }
                            }
                        } else {
                            // Execute Hokusai Brush Stroke to the Layer!
                            self.sync_brush_settings();
                            let active_brush = &self.brushes[self.active_preset_index];
                            let active_brush_state = &mut self.brush_states[self.active_preset_index];

                            if let Some(active_layer) = self.layers.get_mut(&self.active_layer_id) {
                                let preset = &self.presets[self.active_preset_index];
                                let tex_idx = preset.texture_id as usize;
                                let brush_texture = if tex_idx > 0 && tex_idx < self.brush_textures.len() {
                                    Some(self.brush_textures[tex_idx].as_slice())
                                } else {
                                    None
                                };

                                let mut stroke_surface = StrokeSurface {
                                    layer: active_layer,
                                    history: &mut self.history,
                                    snapshots: &mut self.current_stroke_snapshots,
                                    stroke_id: self.stroke_id,
                                    canvas_width: self.canvas_width,
                                    canvas_height: self.canvas_height,
                                    lock_canvas_bounds: self.lock_canvas_bounds,
                                    selection_mask: Some(&self.selection_mask),
                                    brush_texture,
                                    brush_texture_width: 256,
                                    brush_texture_height: 256,
                                    brush_texture_scale: preset.texture_scale,
                                };

                                // Feed the stabilized stroke points to the Hokusai brush engine
                                // with REAL pressure and tilt from the Bosto 16HD!
                                active_brush.stroke_to(
                                    active_brush_state,
                                    &mut stroke_surface,
                                    sx,
                                    sy,
                                    pressure,
                                    tilt_x,
                                    tilt_y,
                                    dt,
                                );
                            }
                        }

                        self.last_ptr_pos = Some(Pos2::new(sx, sy));
                        ctx.request_repaint();
                    }
                } else {
                    // Finalize selection if dragging ended
                    if self.is_selecting {
                        self.is_selecting = false;
                        if let Some(rect) = self.selection_rect.take() {
                            if self.active_tool == ToolId::RectSelect || self.active_tool == ToolId::EllipseSelect {
                                selection::apply_rect_selection(
                                    &mut self.selection_mask, rect,
                                    selection::SelectionMode::Replace,
                                    self.selection_feather, true,
                                );
                            }
                        }
                        if let Some(lasso) = self.lasso_points.take() {
                            if lasso.points.len() >= 3 {
                                selection::apply_lasso_selection(
                                    &mut self.selection_mask, &lasso,
                                    selection::SelectionMode::Replace,
                                    self.selection_feather, true,
                                );
                            }
                        }
                        self.show_selection_overlay = self.selection_mask.is_active;
                    }

                    // Stroke ended! Save the UndoCommand and reset stabilizer
                    if self.stabilizer.is_drawing {
                        self.stabilizer.reset();
                        self.last_ptr_pos = None;

                        // Reset active brush state so the next stroke doesn't connect to the last one!
                        self.brush_states[self.active_preset_index].reset();

                        let is_vector = if let Some(active_layer) = self.layers.get(&self.active_layer_id) {
                            active_layer.kind == crate::canvas::LayerType::Vector
                        } else {
                            false
                        };

                        if is_vector && self.current_vector_points.len() >= 2 {
                            self.sync_brush_settings();
                            // Draw final segment between P_{len-2} and P_{len-1}
                            let len = self.current_vector_points.len();
                            let p0 = if len >= 3 {
                                &self.current_vector_points[len - 3]
                            } else {
                                &self.current_vector_points[len - 2]
                            };
                            let p1 = &self.current_vector_points[len - 2];
                            let p2 = &self.current_vector_points[len - 1];
                            let p3 = &self.current_vector_points[len - 1];

                            let dist = ((p2.x - p1.x).powi(2) + (p2.y - p1.y).powi(2)).sqrt();
                            let steps = ((dist / 2.0) as usize).max(2).min(100);

                            let start_i = if len == 2 { 0 } else { 1 };

                            let active_brush = &self.brushes[self.active_preset_index];
                            let active_brush_state = &mut self.brush_states[self.active_preset_index];

                            if let Some(active_layer) = self.layers.get_mut(&self.active_layer_id) {
                                let preset = &self.presets[self.active_preset_index];
                                let tex_idx = preset.texture_id as usize;
                                let brush_texture = if tex_idx > 0 && tex_idx < self.brush_textures.len() {
                                    Some(self.brush_textures[tex_idx].as_slice())
                                } else {
                                    None
                                };

                                for i in start_i..=steps {
                                    let t = i as f32 / steps as f32;
                                    let pt = Self::catmull_rom(p0, p1, p2, p3, t);

                                    let mut stroke_surface = StrokeSurface {
                                        layer: active_layer,
                                        history: &mut self.history,
                                        snapshots: &mut self.current_stroke_snapshots,
                                        stroke_id: self.stroke_id,
                                        canvas_width: self.canvas_width,
                                        canvas_height: self.canvas_height,
                                        lock_canvas_bounds: self.lock_canvas_bounds,
                                        selection_mask: Some(&self.selection_mask),
                                        brush_texture,
                                        brush_texture_width: 256,
                                        brush_texture_height: 256,
                                        brush_texture_scale: preset.texture_scale,
                                    };

                                    active_brush.stroke_to(
                                        active_brush_state,
                                        &mut stroke_surface,
                                        pt.x,
                                        pt.y,
                                        pt.pressure,
                                        pt.tilt_x,
                                        pt.tilt_y,
                                        0.016 / steps as f64,
                                    );
                                }
                            }
                        }

                        // Store the vector stroke in vector_data
                        if is_vector && !self.current_vector_points.is_empty() {
                            if let Some(active_layer) = self.layers.get_mut(&self.active_layer_id) {
                                let stroke = crate::canvas::VectorStroke {
                                    control_points: self.current_vector_points.clone(),
                                    brush_preset_id: self.presets[self.active_preset_index].id,
                                };
                                if active_layer.vector_data.is_none() {
                                    active_layer.vector_data = Some(crate::canvas::VectorLayer { strokes: Vec::new() });
                                }
                                if let Some(v_data) = &mut active_layer.vector_data {
                                    v_data.strokes.push(stroke);
                                }
                            }
                        }
                        self.current_vector_points.clear();

                        if let Some(active_layer) = self.layers.get_mut(&self.active_layer_id) {
                            let _dirty = active_layer.end_atomic();
                        }

                        // Push the stroke command to the HistoryManager
                        let snapshots = std::mem::take(&mut self.current_stroke_snapshots);
                        if !snapshots.is_empty() {
                            self.history.push_command(UndoCommand { snapshots });
                            self.document_modified = true;
                        }
                    }
                }

                // 4. RENDERING & DISPLAY VIEWPORT
                if let Some(renderer) = &mut self.renderer {
                    // Update GPU textures incrementally for dirty CPU tiles
                    let mut layer_refs: Vec<&mut Layer> = self.layers.values_mut().collect();
                    renderer.update_textures(&mut layer_refs);

                    // Re-compose the stack of visible layers using WGPU
                    renderer.compose_layers(
                        &self.layers,
                        &self.layer_order,
                        self.viewport_offset,
                        self.viewport_zoom,
                        self.canvas_width,
                        self.canvas_height,
                        self.mirror_horizontal,
                        self.rotation_angle,
                    );

                    // Blit WGPU composited output image onto the Egui viewport rect
                    if let Some(texture_id) = renderer.target_egui_id {
                        ui.painter().image(
                            texture_id,
                            rect,
                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                } else {
                    // Fallback CPU drawing to egui painter in case WGPU is unavailable (e.g. baseline safety)
                    ui.painter().rect_filled(rect, 0.0, Color32::WHITE);
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "GPU Canvas Rendering Active (WGPU)... Paint with Primary Mouse button!",
                        egui::FontId::proportional(18.0),
                        Color32::GRAY,
                    );
                }

                // GRID OVERLAY
                if self.show_grid {
                    let grid_size = 64.0;
                    let start_x = (self.viewport_offset.x / grid_size).floor() * grid_size;
                    let start_y = (self.viewport_offset.y / grid_size).floor() * grid_size;
                    let end_x = self.viewport_offset.x + rect.width() / self.viewport_zoom;
                    let end_y = self.viewport_offset.y + rect.height() / self.viewport_zoom;

                    let mut gx = start_x;
                    while gx <= end_x {
                        let sx = ((gx - self.viewport_offset.x) * self.viewport_zoom) + rect.min.x;
                        ui.painter().line_segment(
                            [egui::Pos2::new(sx, rect.min.y), egui::Pos2::new(sx, rect.max.y)],
                            egui::Stroke::new(0.5, Color32::from_black_alpha(40)),
                        );
                        gx += grid_size;
                    }
                    let mut gy = start_y;
                    while gy <= end_y {
                        let sy = ((gy - self.viewport_offset.y) * self.viewport_zoom) + rect.min.y;
                        ui.painter().line_segment(
                            [egui::Pos2::new(rect.min.x, sy), egui::Pos2::new(rect.max.x, sy)],
                            egui::Stroke::new(0.5, Color32::from_black_alpha(40)),
                        );
                        gy += grid_size;
                    }
                }

                // SELECTION OVERLAY (marching ants or mask)
                if self.show_selection_overlay && self.selection_mask.is_active {
                    for (&(tx, ty), tile) in &self.selection_mask.tiles {
                        for ly in 0..64 {
                            for lx in 0..64 {
                                let val = tile[ly * 64 + lx];
                                if val > 0 && val < 255 {
                                    let wx = (tx * 64) as f32 + lx as f32;
                                    let wy = (ty * 64) as f32 + ly as f32;

                                    let sx = ((wx - self.viewport_offset.x) * self.viewport_zoom) + rect.min.x;
                                    let sy = ((wy - self.viewport_offset.y) * self.viewport_zoom) + rect.min.y;
                                    let sw = 1.0 * self.viewport_zoom;
                                    let sh = 1.0 * self.viewport_zoom;

                                    if sx + sw >= rect.min.x && sx <= rect.max.x && sy + sh >= rect.min.y && sy <= rect.max.y {
                                        ui.painter().rect_filled(
                                            egui::Rect::from_min_size(
                                                egui::Pos2::new(sx, sy),
                                                egui::Vec2::new(sw.max(1.0), sh.max(1.0)),
                                            ),
                                            0.0,
                                            egui::Color32::from_rgba_premultiplied(60, 120, 255, (val as u32 * 80 / 255) as u8),
                                        );
                                    }
                                }
                            }
                        }
                    }
                }

                // BRUSH CURSOR + COLOR PICKER CURSOR
                if let Some(pos) = response.hover_pos() {
                    ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
                    let radius =
                        (self.brush_radius_log.exp() * self.viewport_zoom).clamp(1.0, 512.0);
                    ui.painter().circle_stroke(
                        pos,
                        radius,
                        egui::Stroke::new(1.0, Color32::from_black_alpha(220)),
                    );
                    ui.painter().circle_stroke(
                        pos,
                        radius + 1.0,
                        egui::Stroke::new(1.0, Color32::from_white_alpha(180)),
                    );
                }
            });
    }
}
impl PaintApp {
    fn screen_to_world(&self, screen_pos: egui::Pos2, view_rect: egui::Rect) -> egui::Vec2 {
        let center = view_rect.center();
        let half_w = view_rect.width() * 0.5;
        let half_h = view_rect.height() * 0.5;

        let dx = screen_pos.x - center.x;
        let dy = screen_pos.y - center.y;
        let nx = dx / half_w;
        let ny = -dy / half_h;

        let cos_rot = (-self.rotation_angle).cos();
        let sin_rot = (-self.rotation_angle).sin();
        let mut px = nx * cos_rot - ny * sin_rot;
        let py = nx * sin_rot + ny * cos_rot;

        if self.mirror_horizontal {
            px = -px;
        }

        egui::Vec2::new(
            ((px + 1.0) * half_w) / self.viewport_zoom + self.viewport_offset.x,
            ((1.0 - py) * half_h) / self.viewport_zoom + self.viewport_offset.y,
        )
    }
}

// =========================================================================
// Custom HSV Color Wheel Widget & Helpers
// =========================================================================

fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g.max(b));
    let min = r.min(g.min(b));
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    let h = (h * 60.0) / 360.0;
    let s = if max == 0.0 { 0.0 } else { d / max };
    let v = max;
    (h, s, v)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let h = h * 360.0;
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = v - c;
    let (r1, g1, b1) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    (r1 + m, g1 + m, b1 + m)
}

fn draw_hsv_color_wheel(ui: &mut egui::Ui, color: &mut [f32; 3]) -> egui::Response {
    let desired_size = egui::Vec2::new(160.0, 160.0);
    let (rect, mut response) = ui.allocate_exact_size(desired_size, egui::Sense::click_and_drag());

    let center = rect.center();
    let outer_radius = rect.width() * 0.45;
    let inner_radius = rect.width() * 0.33;

    let (mut h, mut s, mut v) = rgb_to_hsv(color[0], color[1], color[2]);

    // Handle interaction
    if response.dragged() || response.clicked() {
        if let Some(mouse_pos) = response.interact_pointer_pos() {
            let to_mouse = mouse_pos - center;
            let dist = to_mouse.length();

            if dist >= inner_radius - 6.0 && dist <= outer_radius + 6.0 {
                // Hue ring drag
                let angle = to_mouse.y.atan2(to_mouse.x); // [-PI, PI]
                let angle = if angle < 0.0 { angle + 2.0 * std::f32::consts::PI } else { angle };
                h = angle / (2.0 * std::f32::consts::PI);
            } else if dist < inner_radius {
                // Sat/Val square drag
                let half_side = inner_radius / 2.0f32.sqrt();
                let local_x = (to_mouse.x / half_side).clamp(-1.0, 1.0);
                let local_y = (to_mouse.y / half_side).clamp(-1.0, 1.0);

                s = (local_x * 0.5 + 0.5).clamp(0.0, 1.0);
                v = (0.5 - local_y * 0.5).clamp(0.0, 1.0);
            }

            let (r, g, b) = hsv_to_rgb(h, s, v);
            color[0] = r;
            color[1] = g;
            color[2] = b;
            response.mark_changed();
        }
    }

    if ui.is_rect_visible(rect) {
        let painter = ui.painter_at(rect);

        // Draw Hue Ring
        let num_segments = 64;
        for i in 0..num_segments {
            let angle1 = (i as f32) * 2.0 * std::f32::consts::PI / (num_segments as f32);
            let angle2 = ((i + 1) as f32) * 2.0 * std::f32::consts::PI / (num_segments as f32);

            let h_segment = (i as f32) / (num_segments as f32);
            let (r, g, b) = hsv_to_rgb(h_segment, 1.0, 1.0);
            let color_segment = egui::Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8);

            // Draw filled wedge segment
            let p1_inner = center + egui::Vec2::new(angle1.cos(), angle1.sin()) * inner_radius;
            let p1_outer = center + egui::Vec2::new(angle1.cos(), angle1.sin()) * outer_radius;
            let p2_inner = center + egui::Vec2::new(angle2.cos(), angle2.sin()) * inner_radius;
            let p2_outer = center + egui::Vec2::new(angle2.cos(), angle2.sin()) * outer_radius;

            painter.add(egui::Shape::convex_polygon(
                vec![p1_inner, p1_outer, p2_outer, p2_inner],
                color_segment,
                egui::Stroke::NONE,
            ));
        }

        // Draw Sat/Val box (inner square)
        let half_side = inner_radius / 2.0f32.sqrt();
        let box_rect = egui::Rect::from_center_size(center, egui::Vec2::new(half_side * 2.0, half_side * 2.0));

        // Draw gradient inside the box using a grid of 12x12 small colored squares
        let steps = 12;
        let cell_w = box_rect.width() / (steps as f32);
        let cell_h = box_rect.height() / (steps as f32);
        for y_idx in 0..steps {
            for x_idx in 0..steps {
                let cell_s = (x_idx as f32) / ((steps - 1) as f32);
                let cell_v = 1.0 - (y_idx as f32) / ((steps - 1) as f32);
                let (r, g, b) = hsv_to_rgb(h, cell_s, cell_v);
                let cell_color = egui::Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8);

                let cell_min = egui::Pos2::new(box_rect.min.x + (x_idx as f32) * cell_w, box_rect.min.y + (y_idx as f32) * cell_h);
                let cell_max = egui::Pos2::new(cell_min.x + cell_w + 0.5, cell_min.y + cell_h + 0.5); // overlapping to avoid gaps
                let cell_rect = egui::Rect::from_min_max(cell_min, cell_max);

                painter.rect_filled(cell_rect, 0.0, cell_color);
            }
        }

        // Draw outline for Sat/Val box
        painter.rect_stroke(box_rect, 0.0, egui::Stroke::new(1.5, egui::Color32::from_gray(180)));

        // Draw marker for current Hue
        let hue_angle = h * 2.0 * std::f32::consts::PI;
        let hue_marker_pos = center + egui::Vec2::new(hue_angle.cos(), hue_angle.sin()) * ((inner_radius + outer_radius) * 0.5);
        painter.circle(hue_marker_pos, 4.0, egui::Color32::WHITE, egui::Stroke::new(1.5, egui::Color32::BLACK));

        // Draw marker for current Sat/Val
        let marker_x = box_rect.min.x + s * box_rect.width();
        let marker_y = box_rect.max.y - v * box_rect.height();
        let marker_pos = egui::Pos2::new(marker_x, marker_y);
        painter.circle(marker_pos, 4.0, egui::Color32::WHITE, egui::Stroke::new(1.5, egui::Color32::BLACK));
    }

    response
}

fn generate_noise_texture() -> Vec<u8> {
    let mut data = vec![0u8; 256 * 256];
    let mut seed: u32 = 12345;
    for i in 0..data.len() {
        seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        let val = (seed >> 16) & 255;
        data[i] = val as u8;
    }
    data
}

fn generate_bristle_texture() -> Vec<u8> {
    let mut data = vec![0u8; 256 * 256];
    for y in 0..256 {
        let dy = (y as f32 - 128.0) / 128.0;
        for x in 0..256 {
            let dx = (x as f32 - 128.0) / 128.0;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist <= 1.0 {
                let angle = dy.atan2(dx);
                let bristle = ((angle * 12.0).sin() * 0.5 + 0.5) * (1.0 - dist);
                data[y * 256 + x] = (bristle.clamp(0.0, 1.0) * 255.0) as u8;
            }
        }
    }
    data
}
