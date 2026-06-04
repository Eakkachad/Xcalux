use crate::canvas::Layer;
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
    AirBrush,
    Water,
    Marker,
    BinaryPen,
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct ClipboardData {
    pub width: u32,
    pub height: u32,
    pub x_offset: i32,
    pub y_offset: i32,
    pub pixels: Vec<[u16; 4]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SelectionDisplayMode {
    MarchingAnts,
    BlueOverlay,
    Hidden,
}

#[derive(Clone)]
#[allow(dead_code)]
pub struct ReferenceImage {
    pub id: u64,
    pub name: String,
    pub path: std::path::PathBuf,
    pub visible: bool,
    pub opacity: f32,
    pub pinned_to_view: bool,
    pub world_pos: egui::Vec2,
    pub scale: f32,
    pub rotation: f32,
    pub size: egui::Vec2,
    pub texture: Option<egui::TextureHandle>,
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
    pub stabilizer_level: StabilizerLevel,
    pub stabilizer_mode: StabilizerMode,
    pub spacing: f32,
    pub density: f32,
}

pub struct PaintApp {
    // Canvas layers and active index
    // Canvas layers and active index
    pub(crate) active_layer_id: u32,
    pub(crate) layers: AHashMap<u32, Layer>,
    pub(crate) layer_order: Vec<u32>,
    pub(crate) layer_id_counter: u32,

    // Brush presets and active selection
    pub(crate) active_preset_index: usize,
    pub(crate) presets: Vec<BrushPreset>,
    pub(crate) preset_id_counter: u64,
    pub(crate) brushes: Vec<Brush>,
    pub(crate) brush_states: Vec<BrushState>,
    pub(crate) brush_color: [f32; 3], // RGB float [0.0, 1.0]
    pub(crate) palette: Vec<[f32; 3]>,
    pub(crate) selected_palette_index: Option<usize>,

    // Sliders bound to the active brush
    pub(crate) brush_radius_log: f32, // Logarithmic radius
    pub(crate) brush_opacity: f32,
    pub(crate) brush_hardness: f32,
    pub(crate) brush_min_size_fraction: f32,
    pub(crate) brush_color_blending: f32,
    pub(crate) brush_dilution: f32,
    pub(crate) brush_spacing: f32,
    pub(crate) brush_density: f32,
    pub(crate) pressure_curve: f32,
    pub(crate) pressure_min: f32,

    // Renaming brush preset state
    pub(crate) renaming_preset_index: Option<usize>,
    pub(crate) rename_input: String,

    // Input and stabilization
    pub(crate) input_manager: Option<InputManager>,
    pub(crate) tablet_axis: TabletAxisState,
    pub(crate) egui_touch_pressure: Option<f32>,
    pub(crate) egui_touch_active: bool,
    pub(crate) stabilizer: StrokeStabilizer,
    pub(crate) last_ptr_pos: Option<Pos2>,
    pub(crate) last_ptr_pressure: f32,
    pub(crate) last_event_time: f64, // Used for stroke dtime tracking

    // Viewport transforms (infinite canvas navigation)
    pub(crate) viewport_offset: Vec2,
    pub(crate) viewport_zoom: f32,
    pub(crate) mirror_horizontal: bool,
    pub(crate) rotation_angle: f32, // in radians

    // Canvas dimensions
    pub canvas_width: u32,
    pub canvas_height: u32,

    // New Canvas Dialog State
    pub(crate) show_new_canvas_dialog: bool,
    pub(crate) new_canvas_width: u32,
    pub(crate) new_canvas_height: u32,

    // Undo/Redo history
    pub(crate) history: HistoryManager,
    pub(crate) current_stroke_snapshots: Vec<TileSnapshot>,
    pub(crate) dragging_layer_id: Option<u32>,
    pub(crate) stroke_id: u32,

    // Customization/masking fields
    pub(crate) lock_canvas_bounds: bool,
    pub(crate) selection_mask: crate::canvas::SelectionMask,
    pub(crate) brush_textures: Vec<Vec<u8>>,

    pub(crate) save_sender: std::sync::mpsc::Sender<crate::save::SaveTask>,
    pub(crate) current_vector_points: Vec<crate::canvas::VectorControlPoint>,
    pub(crate) document_path: String,
    pub(crate) brush_import_path: String,
    pub(crate) brush_texture_id: u8,
    pub(crate) brush_texture_scale: f32,
    pub(crate) brush_bristle_id: u8,

    /// Set to true whenever any brush slider/color/preset changes, so sync_brush_settings
    /// is only flushed into the Hokusai engine when genuinely needed (not every frame).
    pub(crate) brush_settings_dirty: bool,

    // GPU rendering engine
    pub(crate) renderer: Option<WgpuRenderer>,

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
    pub color_history: Vec<[f32; 3]>,
    pub color_history_max: usize,
    pub color_wheel_drag_zone: Option<u8>,

    // Layer operations
    #[allow(unused)]
    pub show_layer_properties: bool,

    // Shortcut editor state
    pub show_shortcut_editor: bool,
    pub shortcut_search: String,
    pub shortcut_edit_idx: Option<usize>,
    pub shortcut_listen_idx: Option<usize>,

    // Autosave recovery
    pub show_recovery_dialog: bool,
    pub recovery_files: Vec<String>,

    // Layer thumbnails cache (keyed by layer_id, regenerated when thumbnail_dirty)
    pub layer_thumbnails: ahash::AHashMap<u32, egui::ColorImage>,
    pub thumbnail_textures: ahash::AHashMap<u32, egui::TextureHandle>,
    #[allow(dead_code)]
    pub thumbnail_regenerate_counter: u32,

    pub last_viewport_rect: Option<egui::Rect>,
    pub last_viewport_size: egui::Vec2,

    // Clipboard and Selection display mode
    pub clipboard: Option<ClipboardData>,
    pub selection_display_mode: SelectionDisplayMode,

    // Selection operation dialogs
    pub show_grow_dialog: bool,
    pub grow_pixels: i32,
    pub show_shrink_dialog: bool,
    pub shrink_pixels: i32,
    pub show_feather_dialog: bool,
    pub feather_pixels: i32,

    // Interactive transform fields
    pub transform_active: bool,
    pub transform_orig_bounds: egui::Rect,
    pub transform_translation: egui::Vec2,
    pub transform_scale: egui::Vec2,
    pub transform_rotation: f32,
    pub transform_pivot: egui::Pos2,
    pub transform_dragging: Option<usize>,
    pub transform_drag_start_ptr: Option<egui::Pos2>,
    pub transform_drag_start_translation: egui::Vec2,
    pub transform_drag_start_scale: egui::Vec2,
    pub transform_drag_start_rotation: f32,

    // Brush test pad
    pub test_pad_image: egui::ColorImage,
    pub test_pad_texture: Option<egui::TextureHandle>,

    // Preferences
    pub show_preferences_dialog: bool,
    pub pref_theme: String,
    pub pref_ui_scale: f32,
    pub pref_canvas_bg: String,
    pub pref_autosave_enabled: bool,
    pub pref_autosave_interval_mins: u32,

    // Diagnostics & HUD
    pub show_tablet_diagnostics: bool,
    pub show_performance_hud: bool,

    // Reference Images
    pub(crate) reference_images: Vec<ReferenceImage>,
    pub(crate) selected_reference_idx: Option<usize>,
    pub(crate) ref_image_path_input: String,
    pub(crate) reference_id_counter: u64,
    pub(crate) ref_image_dragging: Option<usize>,
    pub(crate) ref_image_drag_offset: egui::Vec2,
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

        // 6. AirBrush Preset
        let mut airbrush = Brush::new();
        Self::set_constant(&mut airbrush, BrushSetting::Radius, 3.0);
        Self::set_constant(&mut airbrush, BrushSetting::Opaque, 0.35);
        Self::set_constant(&mut airbrush, BrushSetting::Hardness, 0.1);
        Self::set_constant(&mut airbrush, BrushSetting::DabsPerActualRadius, 1.5);
        Self::set_pressure_mapping(&mut airbrush, BrushSetting::Opaque, 0.35, vec![(0.0, -0.25), (0.50, -0.10), (1.0, 0.0)]);
        brushes.push(airbrush);
        brush_states.push(BrushState::default());

        // 7. Water Preset
        let mut water = Brush::new();
        Self::set_constant(&mut water, BrushSetting::Radius, 2.0);
        Self::set_constant(&mut water, BrushSetting::Opaque, 0.3);
        Self::set_constant(&mut water, BrushSetting::Hardness, 0.5);
        Self::set_constant(&mut water, BrushSetting::Smudge, 0.9);
        Self::set_constant(&mut water, BrushSetting::SmudgeLength, 0.9);
        Self::set_constant(&mut water, BrushSetting::DabsPerActualRadius, 2.0);
        brushes.push(water);
        brush_states.push(BrushState::default());

        // 8. Marker Preset
        let mut marker = Brush::new();
        Self::set_constant(&mut marker, BrushSetting::Radius, 2.2);
        Self::set_constant(&mut marker, BrushSetting::Opaque, 0.7);
        Self::set_constant(&mut marker, BrushSetting::Hardness, 0.9);
        Self::set_constant(&mut marker, BrushSetting::DabsPerActualRadius, 3.0);
        brushes.push(marker);
        brush_states.push(BrushState::default());

        // 9. Binary Pen Preset
        let mut binary_pen = Brush::new();
        Self::set_constant(&mut binary_pen, BrushSetting::Radius, 1.2);
        Self::set_constant(&mut binary_pen, BrushSetting::Opaque, 1.0);
        Self::set_constant(&mut binary_pen, BrushSetting::Hardness, 1.0); // Completely hard
        Self::set_constant(&mut binary_pen, BrushSetting::DabsPerActualRadius, 2.0);
        brushes.push(binary_pen);
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
                stabilizer_level: StabilizerLevel::default(),
                stabilizer_mode: StabilizerMode::SpringMassDamper,
                spacing: 2.0,
                density: 1.0,
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
                stabilizer_level: StabilizerLevel::default(),
                stabilizer_mode: StabilizerMode::SpringMassDamper,
                spacing: 2.5,
                density: 1.0,
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
                stabilizer_level: StabilizerLevel::default(),
                stabilizer_mode: StabilizerMode::SpringMassDamper,
                spacing: 3.0,
                density: 0.8,
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
                stabilizer_level: StabilizerLevel::default(),
                stabilizer_mode: StabilizerMode::SpringMassDamper,
                spacing: 2.0,
                density: 0.4,
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
                stabilizer_level: StabilizerLevel::default(),
                stabilizer_mode: StabilizerMode::SpringMassDamper,
                spacing: 2.0,
                density: 1.0,
            },
            BrushPreset {
                id: 6,
                name: "AirBrush".to_string(),
                icon: PresetIcon::AirBrush,
                radius_log: 3.0,
                opacity: 0.35,
                hardness: 0.1,
                min_size_fraction: 0.9,
                color_blending: 0.0,
                dilution: 0.0,
                is_eraser: false,
                texture_id: 0,
                texture_scale: 1.0,
                bristle_id: 0,
                stabilizer_level: StabilizerLevel::default(),
                stabilizer_mode: StabilizerMode::SpringMassDamper,
                spacing: 1.5,
                density: 0.5,
            },
            BrushPreset {
                id: 7,
                name: "Water".to_string(),
                icon: PresetIcon::Water,
                radius_log: 2.0,
                opacity: 0.3,
                hardness: 0.5,
                min_size_fraction: 0.5,
                color_blending: 0.9,
                dilution: 0.9,
                is_eraser: false,
                texture_id: 0,
                texture_scale: 1.0,
                bristle_id: 0,
                stabilizer_level: StabilizerLevel::default(),
                stabilizer_mode: StabilizerMode::SpringMassDamper,
                spacing: 2.0,
                density: 0.3,
            },
            BrushPreset {
                id: 8,
                name: "Marker".to_string(),
                icon: PresetIcon::Marker,
                radius_log: 2.2,
                opacity: 0.7,
                hardness: 0.9,
                min_size_fraction: 1.0,
                color_blending: 0.2,
                dilution: 0.15,
                is_eraser: false,
                texture_id: 0,
                texture_scale: 1.0,
                bristle_id: 0,
                stabilizer_level: StabilizerLevel::default(),
                stabilizer_mode: StabilizerMode::SpringMassDamper,
                spacing: 3.0,
                density: 0.8,
            },
            BrushPreset {
                id: 9,
                name: "Binary Pen".to_string(),
                icon: PresetIcon::BinaryPen,
                radius_log: 1.2,
                opacity: 1.0,
                hardness: 1.0,
                min_size_fraction: 0.3,
                color_blending: 0.0,
                dilution: 0.0,
                is_eraser: false,
                texture_id: 0,
                texture_scale: 1.0,
                bristle_id: 0,
                stabilizer_level: StabilizerLevel::default(),
                stabilizer_mode: StabilizerMode::SpringMassDamper,
                spacing: 2.0,
                density: 1.0,
            },
        ];

        let (save_sender, save_receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            crate::save::save_worker_loop(save_receiver);
        });

        let mut app = Self {
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
            brush_spacing: 2.0,
            brush_density: 1.0,
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
            autosave_path: ".autosave.arty".to_string(),
            last_autosave_time: 0.0,
            document_modified: false,
            autosave_status: "".to_string(),
            show_minimal_ui: false,
            show_grid: false,
            show_symmetry: false,
            quick_bar_visible: true,
            color_history: Vec::with_capacity(12),
            color_history_max: 12,
            color_wheel_drag_zone: None,
            show_layer_properties: false,
            show_shortcut_editor: false,
            shortcut_search: String::new(),
            shortcut_edit_idx: None,
            shortcut_listen_idx: None,
            show_recovery_dialog: false,
            recovery_files: Vec::new(),
            layer_thumbnails: ahash::AHashMap::default(),
            thumbnail_textures: ahash::AHashMap::default(),
            thumbnail_regenerate_counter: 0,
            last_viewport_rect: None,
            last_viewport_size: egui::vec2(800.0, 600.0),

            clipboard: None,
            selection_display_mode: SelectionDisplayMode::MarchingAnts,

            show_grow_dialog: false,
            grow_pixels: 5,
            show_shrink_dialog: false,
            shrink_pixels: 5,
            show_feather_dialog: false,
            feather_pixels: 5,

            transform_active: false,
            transform_orig_bounds: egui::Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::ZERO),
            transform_translation: egui::Vec2::ZERO,
            transform_scale: egui::Vec2::new(1.0, 1.0),
            transform_rotation: 0.0,
            transform_pivot: egui::Pos2::ZERO,
            transform_dragging: None,
            transform_drag_start_ptr: None,
            transform_drag_start_translation: egui::Vec2::ZERO,
            transform_drag_start_scale: egui::Vec2::new(1.0, 1.0),
            transform_drag_start_rotation: 0.0,

            test_pad_image: egui::ColorImage::new([120, 60], egui::Color32::WHITE),
            test_pad_texture: None,

            show_preferences_dialog: false,
            pref_theme: "Gray".to_string(),
            pref_ui_scale: 1.0,
            pref_canvas_bg: "Gray".to_string(),
            pref_autosave_enabled: true,
            pref_autosave_interval_mins: 3,

            show_tablet_diagnostics: false,
            show_performance_hud: false,

            reference_images: Vec::new(),
            selected_reference_idx: None,
            ref_image_path_input: String::new(),
            reference_id_counter: 0,
            ref_image_dragging: None,
            ref_image_drag_offset: egui::Vec2::ZERO,
        };

        // Check for autosave recovery files on startup
        if let Ok(entries) = std::fs::read_dir(".") {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with(".autosave.") && name.ends_with(".arty") {
                        app.recovery_files.push(name.to_string());
                    }
                }
            }
        }
        if !app.recovery_files.is_empty() {
            app.show_recovery_dialog = true;
        }

        // Load saved user preferences
        crate::preferences::load_preferences(&mut app, &cc.egui_ctx);

        app
    }

    pub(crate) fn set_constant(brush: &mut Brush, s: BrushSetting, v: f32) {
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

    pub(crate) fn remap_pressure(&self, raw: f32) -> f32 {
        let normalized = raw.clamp(0.0, 1.0).powf(self.pressure_curve);
        (self.pressure_min + normalized * (1.0 - self.pressure_min)).clamp(0.01, 1.0)
    }

    pub(crate) fn record_color(&mut self, color: [f32; 3]) {
        // Don't record if it's the same as the most recent entry
        if let Some(last) = self.color_history.last() {
            if (last[0] - color[0]).abs() < 0.001
                && (last[1] - color[1]).abs() < 0.001
                && (last[2] - color[2]).abs() < 0.001
            {
                return;
            }
        }
        // Remove existing duplicate elsewhere in history
        self.color_history.retain(|c| {
            (c[0] - color[0]).abs() > 0.001
                || (c[1] - color[1]).abs() > 0.001
                || (c[2] - color[2]).abs() > 0.001
        });
        self.color_history.push(color);
        if self.color_history.len() > self.color_history_max {
            self.color_history.remove(0);
        }
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
        preset.stabilizer_level = self.stabilizer.level;
        preset.stabilizer_mode = self.stabilizer.mode;
        preset.spacing = self.brush_spacing;
        preset.density = self.brush_density;

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

        // 5. OpaqueMultiply: scale pressure points by preset.density.
        Self::set_pressure_mapping(
            active_brush,
            BrushSetting::OpaqueMultiply,
            0.0,
            vec![
                (0.0, 0.0),
                (0.3, 0.55 * preset.density),
                (0.6, 0.85 * preset.density),
                (1.0, 1.0 * preset.density),
            ],
        );

        // Spacing: Set constant for Hokusai's brush engine spacing (DabsPerActualRadius)
        Self::set_constant(active_brush, BrushSetting::DabsPerActualRadius, preset.spacing);

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
    pub(crate) fn select_preset(&mut self, idx: usize) {
        if idx >= self.presets.len() {
            return;
        }

        // Save current stabilizer into the outgoing preset before switching
        let current_preset = &mut self.presets[self.active_preset_index];
        current_preset.stabilizer_level = self.stabilizer.level;
        current_preset.stabilizer_mode = self.stabilizer.mode;

        self.active_preset_index = idx;

        let preset = &self.presets[idx];
        self.active_tool = if preset.is_eraser {
            ToolId::Eraser
        } else {
            ToolId::Brush
        };
        self.brush_radius_log = preset.radius_log;
        self.brush_opacity = preset.opacity;
        self.brush_hardness = preset.hardness;
        self.brush_min_size_fraction = preset.min_size_fraction;
        self.brush_color_blending = preset.color_blending;
        self.brush_dilution = preset.dilution;
        self.brush_texture_id = preset.texture_id;
        self.brush_texture_scale = preset.texture_scale;
        self.brush_bristle_id = preset.bristle_id;
        self.brush_spacing = preset.spacing;
        self.brush_density = preset.density;

        // Restore per-preset stabilizer settings
        self.stabilizer.set_level(preset.stabilizer_level);
        self.stabilizer.mode = preset.stabilizer_mode;

        // Mark dirty so pressure curves are rebuilt for the newly-selected preset
        self.brush_settings_dirty = true;
    }

    /// Create a new brush preset slot dynamically
    pub(crate) fn create_preset(&mut self, icon_type: PresetIcon) {
        self.preset_id_counter += 1;
        let id = self.preset_id_counter;

        let (name, radius, opacity, hardness, min_size, blending, dilution, is_eraser, spacing, density) = match icon_type {
            PresetIcon::Pencil => ("Pencil".to_string(), 1.0, 0.95, 0.95, 0.8, 0.0, 0.0, false, 2.0, 1.0),
            PresetIcon::InkPen => ("Ink Pen".to_string(), 1.6, 1.0, 0.88, 0.2, 0.0, 0.0, false, 2.5, 1.0),
            PresetIcon::PaintBrush => ("Paint Brush".to_string(), 2.2, 0.8, 0.5, 0.3, 0.5, 0.4, false, 3.0, 0.8),
            PresetIcon::Smudge => ("Smudge".to_string(), 2.0, 0.4, 0.4, 0.4, 0.85, 0.8, false, 2.0, 0.4),
            PresetIcon::Eraser => ("Eraser".to_string(), 2.5, 1.0, 0.8, 0.5, 0.0, 0.0, true, 2.0, 1.0),
            PresetIcon::AirBrush => ("AirBrush".to_string(), 3.0, 0.35, 0.1, 0.9, 0.0, 0.0, false, 1.5, 0.5),
            PresetIcon::Water => ("Water".to_string(), 2.0, 0.3, 0.5, 0.5, 0.9, 0.9, false, 2.0, 0.3),
            PresetIcon::Marker => ("Marker".to_string(), 2.2, 0.7, 0.9, 1.0, 0.2, 0.15, false, 3.0, 0.8),
            PresetIcon::BinaryPen => ("Binary Pen".to_string(), 1.2, 1.0, 1.0, 0.3, 0.0, 0.0, false, 2.0, 1.0),
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
            stabilizer_level: StabilizerLevel::default(),
            stabilizer_mode: StabilizerMode::SpringMassDamper,
            spacing,
            density,
        };

        // Create matching Brush setting up the correct pressure curves natively
        let mut brush = Brush::new();
        Self::set_constant(&mut brush, BrushSetting::Radius, radius);
        Self::set_constant(&mut brush, BrushSetting::Opaque, opacity);
        Self::set_constant(&mut brush, BrushSetting::Hardness, hardness);

        match icon_type {
            PresetIcon::Pencil => {
                Self::set_constant(&mut brush, BrushSetting::DabsPerActualRadius, spacing);
                Self::set_pressure_mapping(&mut brush, BrushSetting::Opaque, opacity, vec![(0.0, -0.90), (0.15, -0.60), (0.35, -0.30), (0.55, -0.10), (0.80, -0.03), (1.0, 0.0)]);
                Self::set_pressure_mapping(&mut brush, BrushSetting::OpaqueMultiply, 0.0, vec![(0.0, 0.0), (1.0, 1.0)]);
            }
            PresetIcon::InkPen => {
                Self::set_constant(&mut brush, BrushSetting::DabsPerActualRadius, spacing);
                Self::set_pressure_mapping(&mut brush, BrushSetting::Opaque, opacity, vec![(0.0, -0.15), (0.20, -0.05), (0.50, 0.0), (1.0, 0.0)]);
                Self::set_pressure_mapping(&mut brush, BrushSetting::OpaqueMultiply, 0.0, vec![(0.0, 0.0), (1.0, 1.0)]);
            }
            PresetIcon::PaintBrush => {
                Self::set_constant(&mut brush, BrushSetting::DabsPerActualRadius, spacing);
                Self::set_pressure_mapping(&mut brush, BrushSetting::Opaque, opacity, vec![(0.0, -0.70), (0.15, -0.45), (0.35, -0.25), (0.55, -0.12), (0.80, -0.03), (1.0, 0.0)]);
                Self::set_pressure_mapping(&mut brush, BrushSetting::OpaqueMultiply, 0.0, vec![(0.0, 0.0), (1.0, 1.0)]);
            }
            PresetIcon::Smudge => {
                Self::set_constant(&mut brush, BrushSetting::DabsPerActualRadius, spacing);
                Self::set_pressure_mapping(&mut brush, BrushSetting::Opaque, opacity, vec![(0.0, -0.30), (0.40, -0.12), (0.70, -0.04), (1.0, 0.0)]);
            }
            PresetIcon::Eraser => {
                Self::set_constant(&mut brush, BrushSetting::Eraser, 1.0);
                Self::set_constant(&mut brush, BrushSetting::DabsPerActualRadius, spacing);
                Self::set_pressure_mapping(&mut brush, BrushSetting::Opaque, opacity, vec![(0.0, -0.60), (0.20, -0.35), (0.45, -0.15), (0.75, -0.04), (1.0, 0.0)]);
            }
            PresetIcon::AirBrush => {
                Self::set_constant(&mut brush, BrushSetting::DabsPerActualRadius, spacing);
                Self::set_pressure_mapping(&mut brush, BrushSetting::Opaque, opacity, vec![(0.0, -0.25), (0.50, -0.10), (1.0, 0.0)]);
            }
            PresetIcon::Water => {
                Self::set_constant(&mut brush, BrushSetting::DabsPerActualRadius, spacing);
                Self::set_constant(&mut brush, BrushSetting::Smudge, blending);
                Self::set_constant(&mut brush, BrushSetting::SmudgeLength, dilution);
            }
            PresetIcon::Marker => {
                Self::set_constant(&mut brush, BrushSetting::DabsPerActualRadius, spacing);
            }
            PresetIcon::BinaryPen => {
                Self::set_constant(&mut brush, BrushSetting::DabsPerActualRadius, spacing);
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
    pub(crate) fn duplicate_preset(&mut self, idx: usize) {
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
    pub(crate) fn delete_preset(&mut self, idx: usize) {
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

    pub(crate) fn cleanup_autosave(&self) {
        let autosave = std::path::Path::new(&self.autosave_path);
        if autosave.exists() {
            let _ = std::fs::remove_file(autosave);
        }
    }

    /// Regenerate thumbnails for layers marked as `thumbnail_dirty`
    fn regenerate_dirty_thumbnails(&mut self) {
        let mut new_images: ahash::AHashMap<u32, egui::ColorImage> = ahash::AHashMap::default();
        for id in &self.layer_order.clone() {
            if let Some(layer) = self.layers.get(id) {
                if layer.thumbnail_dirty {
                    let (pixels, w, h) = layer.generate_thumbnail(64);
                    if w > 0 && h > 0 {
                        let image = egui::ColorImage::from_rgba_unmultiplied(
                            [w as usize, h as usize],
                            &pixels,
                        );
                        new_images.insert(*id, image);
                        // Invalidate egui texture cache to force reload on next frame!
                        self.thumbnail_textures.remove(id);
                    }
                }
            }
        }
        if !new_images.is_empty() {
            self.layer_thumbnails.extend(new_images);
        }
        // Clear dirty flags after regeneration
        for id in &self.layer_order.clone() {
            if let Some(layer) = self.layers.get_mut(id) {
                layer.thumbnail_dirty = false;
            }
        }
    }

    /// Get or create a texture handle for a layer thumbnail
    pub(crate) fn get_layer_thumbnail_texture(&mut self, ctx: &egui::Context, layer_id: u32) -> Option<egui::TextureHandle> {
        if let Some(thumb) = self.layer_thumbnails.get(&layer_id) {
            if self.thumbnail_textures.contains_key(&layer_id) {
                return self.thumbnail_textures.get(&layer_id).cloned();
            }
            let handle = ctx.load_texture(
                &format!("layer_thumb_{}", layer_id),
                thumb.clone(),
                egui::TextureOptions::LINEAR,
            );
            self.thumbnail_textures.insert(layer_id, handle.clone());
            Some(handle)
        } else {
            None
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
            CommandId::NewDocument => {
                self.show_new_canvas_dialog = true;
            }
            CommandId::Save => {
                self.save_canvas(std::path::Path::new(&self.document_path));
                self.document_modified = false;
                self.cleanup_autosave();
            }
            CommandId::SaveAs => {
                // Would open save dialog; for now just save
                self.save_canvas(std::path::Path::new(&self.document_path));
                self.document_modified = false;
                self.cleanup_autosave();
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
                } else {
                    self.clear_entire_layer();
                }
            }
            CommandId::Fill => {
                if self.selection_mask.is_active && !self.selection_mask.is_empty() {
                    self.fill_selected_area();
                } else {
                    self.fill_entire_layer();
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
            CommandId::DuplicateLayer => {
                self.duplicate_active_layer();
                if let Some(layer) = self.layers.get_mut(&self.active_layer_id) {
                    layer.thumbnail_dirty = true;
                }
            }
            CommandId::DeleteLayer => self.delete_active_layer(),
            CommandId::MergeDown => {
                self.merge_down();
                if let Some(layer) = self.layers.get_mut(&self.active_layer_id) {
                    layer.thumbnail_dirty = true;
                }
            }
            CommandId::MergeVisible => {
                self.merge_visible();
                if let Some(layer) = self.layers.get_mut(&self.active_layer_id) {
                    layer.thumbnail_dirty = true;
                }
            }
            CommandId::FlattenImage => {
                self.flatten_image();
                if let Some(layer) = self.layers.get_mut(&self.active_layer_id) {
                    layer.thumbnail_dirty = true;
                }
            }
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
            CommandId::Cut => self.cut_selection(),
            CommandId::Copy => self.copy_selection(false),
            CommandId::CopyMerged => self.copy_selection(true),
            CommandId::Paste => self.paste_selection(false),
            CommandId::PasteAsNewLayer => self.paste_selection(true),

            CommandId::SelectionGrow => self.show_grow_dialog = true,
            CommandId::SelectionShrink => self.show_shrink_dialog = true,
            CommandId::SelectionFeather => self.show_feather_dialog = true,
            CommandId::ToggleSelectionOverlay => {
                self.selection_display_mode = match self.selection_display_mode {
                    SelectionDisplayMode::MarchingAnts => SelectionDisplayMode::BlueOverlay,
                    SelectionDisplayMode::BlueOverlay => SelectionDisplayMode::Hidden,
                    SelectionDisplayMode::Hidden => SelectionDisplayMode::MarchingAnts,
                };
            }

            CommandId::ToolTransform | CommandId::TransformSelection => {
                self.active_tool = ToolId::Transform;
                self.start_transform();
            }
            CommandId::ToolColorPicker => self.active_tool = ToolId::ColorPicker,
            CommandId::ToolHand => self.active_tool = ToolId::Hand,
            CommandId::ToolZoom => self.active_tool = ToolId::Zoom,
            CommandId::ToolRotateView => self.active_tool = ToolId::RotateView,

            // View
            CommandId::MinimalUi => self.show_minimal_ui = !self.show_minimal_ui,
            CommandId::ShowGrid => self.show_grid = !self.show_grid,
            CommandId::ShowSymmetry => self.show_symmetry = !self.show_symmetry,

            // Misc
            CommandId::Preferences => self.show_preferences_dialog = true,
            CommandId::KeyboardShortcuts => self.show_shortcut_editor = true,
            CommandId::TabletDiagnostics => self.show_tablet_diagnostics = true,
            CommandId::PerformanceHud => self.show_performance_hud = true,

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

        // Capture undo snapshots for all existing tiles
        let mut snapshots = Vec::new();
        for (&(tx, ty), tile) in &layer.tiles {
            let mut pixels = self.history.alloc_tile();
            *pixels = *tile.pixels;
            snapshots.push(crate::history::TileSnapshot {
                layer_id: layer.id,
                coords: (tx, ty),
                pixels: Some(pixels),
            });
        }

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

        if !snapshots.is_empty() {
            self.history.push_command(crate::history::UndoCommand { snapshots });
            self.document_modified = true;
        }
        if let Some(layer) = self.layers.get_mut(&self.active_layer_id) {
            layer.thumbnail_dirty = true;
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

        // Capture undo snapshots for all existing tiles
        let mut snapshots = Vec::new();
        for (&(tx, ty), tile) in &layer.tiles {
            let mut pixels = self.history.alloc_tile();
            *pixels = *tile.pixels;
            snapshots.push(crate::history::TileSnapshot {
                layer_id: layer.id,
                coords: (tx, ty),
                pixels: Some(pixels),
            });
        }

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

        if !snapshots.is_empty() {
            self.history.push_command(crate::history::UndoCommand { snapshots });
            self.document_modified = true;
        }
    }

    fn clear_entire_layer(&mut self) {
        let Some(layer) = self.layers.get_mut(&self.active_layer_id) else { return; };

        let mut snapshots = Vec::new();
        for (&(tx, ty), tile) in &layer.tiles {
            let mut pixels = self.history.alloc_tile();
            *pixels = *tile.pixels;
            snapshots.push(crate::history::TileSnapshot {
                layer_id: layer.id,
                coords: (tx, ty),
                pixels: Some(pixels),
            });
        }

        for tile in layer.tiles.values_mut() {
            for ly in 0usize..64 {
                for lx in 0usize..64 {
                    tile.pixels[ly][lx] = [0, 0, 0, 0];
                }
            }
            tile.is_dirty = true;
        }

        if !snapshots.is_empty() {
            self.history.push_command(crate::history::UndoCommand { snapshots });
            self.document_modified = true;
        }
        if let Some(layer) = self.layers.get_mut(&self.active_layer_id) {
            layer.thumbnail_dirty = true;
        }
    }

    fn fill_entire_layer(&mut self) {
        let Some(layer) = self.layers.get_mut(&self.active_layer_id) else { return; };
        let fill_color: [u16; 4] = [
            (self.brush_color[0] * 32768.0) as u16,
            (self.brush_color[1] * 32768.0) as u16,
            (self.brush_color[2] * 32768.0) as u16,
            32768,
        ];

        // If layer has no tiles, create one covering the canvas
        if layer.tiles.is_empty() {
            let tw = (self.canvas_width + 63) / 64;
            let th = (self.canvas_height + 63) / 64;
            for ty in 0..th as i32 {
                for tx in 0..tw as i32 {
                    let mut tile = crate::canvas::Tile::new();
                    for ly in 0usize..64 {
                        for lx in 0usize..64 {
                            tile.pixels[ly][lx] = fill_color;
                        }
                    }
                    tile.is_dirty = true;
                    layer.tiles.insert((tx, ty), tile);
                }
            }
            self.document_modified = true;
            if let Some(r) = &mut self.renderer {
                r.clear_cache();
            }
            return;
        }

        let mut snapshots = Vec::new();
        for (&(tx, ty), tile) in &layer.tiles {
            let mut pixels = self.history.alloc_tile();
            *pixels = *tile.pixels;
            snapshots.push(crate::history::TileSnapshot {
                layer_id: layer.id,
                coords: (tx, ty),
                pixels: Some(pixels),
            });
        }

        for tile in layer.tiles.values_mut() {
            for ly in 0usize..64 {
                for lx in 0usize..64 {
                    tile.pixels[ly][lx] = fill_color;
                }
            }
            tile.is_dirty = true;
        }

        if !snapshots.is_empty() {
            self.history.push_command(crate::history::UndoCommand { snapshots });
            self.document_modified = true;
        }
    }

    fn get_pixel(&self, layer: &Layer, x: i32, y: i32) -> [u16; 4] {
        let tx = x.div_euclid(64);
        let ty = y.div_euclid(64);
        let lx = x.rem_euclid(64) as usize;
        let ly = y.rem_euclid(64) as usize;
        if let Some(tile) = layer.tiles.get(&(tx, ty)) {
            tile.pixels[ly][lx]
        } else {
            [0, 0, 0, 0]
        }
    }

    fn get_merged_pixel(&self, x: i32, y: i32) -> [u16; 4] {
        let mut composite = [0u16; 4];
        for &id in self.layer_order.iter().rev() {
            if let Some(l) = self.layers.get(&id) {
                if l.visible {
                    let pix = self.get_pixel(l, x, y);
                    let mut scaled_pix = pix;
                    scaled_pix[3] = (scaled_pix[3] as f32 * l.opacity) as u16;
                    composite = fill::blend_colors(scaled_pix, composite);
                }
            }
        }
        composite
    }

    fn copy_selection(&mut self, merged: bool) {
        let mut min_x = i32::MAX;
        let mut min_y = i32::MAX;
        let mut max_x = i32::MIN;
        let mut max_y = i32::MIN;

        if self.selection_mask.is_active && !self.selection_mask.is_empty() {
            for (&(tx, ty), tile) in &self.selection_mask.tiles {
                for ly in 0..64 {
                    for lx in 0..64 {
                        if tile[ly * 64 + lx] > 0 {
                            let wx = tx * 64 + lx as i32;
                            let wy = ty * 64 + ly as i32;
                            min_x = min_x.min(wx);
                            min_y = min_y.min(wy);
                            max_x = max_x.max(wx);
                            max_y = max_y.max(wy);
                        }
                    }
                }
            }
        } else {
            if let Some(layer) = self.layers.get(&self.active_layer_id) {
                for &(tx, ty) in layer.tiles.keys() {
                    min_x = min_x.min(tx * 64);
                    min_y = min_y.min(ty * 64);
                    max_x = max_x.max((tx + 1) * 64 - 1);
                    max_y = max_y.max((ty + 1) * 64 - 1);
                }
            }
        }

        if min_x == i32::MAX || min_y == i32::MAX || max_x == i32::MIN || max_y == i32::MIN {
            return;
        }

        min_x = min_x.clamp(0, self.canvas_width as i32 - 1);
        min_y = min_y.clamp(0, self.canvas_height as i32 - 1);
        max_x = max_x.clamp(0, self.canvas_width as i32 - 1);
        max_y = max_y.clamp(0, self.canvas_height as i32 - 1);

        if min_x > max_x || min_y > max_y {
            return;
        }

        let width = (max_x - min_x + 1) as u32;
        let height = (max_y - min_y + 1) as u32;
        let mut pixels = vec![[0u16; 4]; (width * height) as usize];

        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let composite = if merged {
                    self.get_merged_pixel(x, y)
                } else if let Some(layer) = self.layers.get(&self.active_layer_id) {
                    self.get_pixel(layer, x, y)
                } else {
                    [0, 0, 0, 0]
                };

                let mut pix = composite;
                if self.selection_mask.is_active {
                    let sel_val = self.selection_mask.get_value(x, y);
                    let factor = sel_val as f32 / 255.0;
                    pix[3] = (pix[3] as f32 * factor) as u16;
                }

                let idx = ((y - min_y) as u32 * width + (x - min_x) as u32) as usize;
                pixels[idx] = pix;
            }
        }

        self.clipboard = Some(ClipboardData {
            width,
            height,
            x_offset: min_x,
            y_offset: min_y,
            pixels,
        });
    }

    fn cut_selection(&mut self) {
        self.copy_selection(false);
        self.command(CommandId::Clear);
    }

    fn paste_selection(&mut self, new_layer: bool) {
        let clipboard = match &self.clipboard {
            Some(c) => c.clone(),
            None => return,
        };

        if new_layer {
            self.create_raster_layer();
            if let Some(layer) = self.layers.get_mut(&self.active_layer_id) {
                layer.name = format!("Pasted Layer {}", layer.id);
            }
        }

        let Some(layer) = self.layers.get_mut(&self.active_layer_id) else { return; };

        let mut snapshots = Vec::new();
        let mut affected_tiles = ahash::AHashSet::default();
        for y in 0..clipboard.height as i32 {
            for x in 0..clipboard.width as i32 {
                let wx = x + clipboard.x_offset;
                let wy = y + clipboard.y_offset;
                let tx = wx.div_euclid(64);
                let ty = wy.div_euclid(64);
                affected_tiles.insert((tx, ty));
            }
        }

        for &(tx, ty) in &affected_tiles {
            if let Some(tile) = layer.tiles.get(&(tx, ty)) {
                let mut pixels = self.history.alloc_tile();
                *pixels = *tile.pixels;
                snapshots.push(crate::history::TileSnapshot {
                    layer_id: layer.id,
                    coords: (tx, ty),
                    pixels: Some(pixels),
                });
            } else {
                snapshots.push(crate::history::TileSnapshot {
                    layer_id: layer.id,
                    coords: (tx, ty),
                    pixels: None,
                });
            }
        }

        for y in 0..clipboard.height as i32 {
            for x in 0..clipboard.width as i32 {
                let wx = x + clipboard.x_offset;
                let wy = y + clipboard.y_offset;
                if wx < 0 || wx >= self.canvas_width as i32 || wy < 0 || wy >= self.canvas_height as i32 {
                    continue;
                }
                let idx = (y as u32 * clipboard.width + x as u32) as usize;
                let src_pixel = clipboard.pixels[idx];
                if src_pixel[3] == 0 { continue; }

                let tx = wx.div_euclid(64);
                let ty = wy.div_euclid(64);
                let lx = wx.rem_euclid(64) as usize;
                let ly = wy.rem_euclid(64) as usize;

                let tile = layer.tiles.entry((tx, ty)).or_insert_with(crate::canvas::Tile::new);
                tile.pixels[ly][lx] = fill::blend_colors(src_pixel, tile.pixels[ly][lx]);
                tile.is_dirty = true;
            }
        }

        layer.thumbnail_dirty = true;
        if !snapshots.is_empty() {
            self.history.push_command(crate::history::UndoCommand { snapshots });
            self.document_modified = true;
        }

        if let Some(r) = &mut self.renderer {
            r.clear_cache();
        }
    }

    fn start_transform(&mut self) {
        if self.transform_active { return; }
        
        let mut min_tx = i32::MAX;
        let mut min_ty = i32::MAX;
        let mut max_tx = i32::MIN;
        let mut max_ty = i32::MIN;
        
        if let Some(layer) = self.layers.get(&self.active_layer_id) {
            for (&(tx, ty), _) in &layer.tiles {
                min_tx = min_tx.min(tx);
                min_ty = min_ty.min(ty);
                max_tx = max_tx.max(tx);
                max_ty = max_ty.max(ty);
            }
        }
        
        let orig_bounds = if min_tx != i32::MAX {
            egui::Rect::from_min_max(
                egui::Pos2::new(min_tx as f32 * 64.0, min_ty as f32 * 64.0),
                egui::Pos2::new((max_tx + 1) as f32 * 64.0, (max_ty + 1) as f32 * 64.0),
            )
        } else {
            egui::Rect::from_min_max(
                egui::Pos2::new(0.0, 0.0),
                egui::Pos2::new(self.canvas_width as f32, self.canvas_height as f32),
            )
        };
        
        self.transform_active = true;
        self.transform_orig_bounds = orig_bounds;
        self.transform_translation = egui::Vec2::ZERO;
        self.transform_scale = egui::Vec2::new(1.0, 1.0);
        self.transform_rotation = 0.0;
        self.transform_pivot = orig_bounds.center();
        self.transform_dragging = None;
        if let Some(layer) = self.layers.get(&self.active_layer_id) {
            self.transform_state.snap_layer(layer);
        }
    }

    fn commit_transform(&mut self) {
        if !self.transform_active { return; }
        self.transform_active = false;
        
        let a = self.transform_scale.x * self.transform_rotation.cos();
        let b = self.transform_scale.x * self.transform_rotation.sin();
        let c = -self.transform_scale.y * self.transform_rotation.sin();
        let d = self.transform_scale.y * self.transform_rotation.cos();
        let px = self.transform_pivot.x;
        let py = self.transform_pivot.y;
        let tx = self.transform_translation.x;
        let ty = self.transform_translation.y;
        let e = px + tx - px * a - py * c;
        let f = py + ty - px * b - py * d;
        
        self.transform_state.matrix = [a, b, c, d, e, f];
        
        let mut snapshots = Vec::new();
        if let Some(layer) = self.layers.get_mut(&self.active_layer_id) {
            for (&coords, tile) in &layer.tiles {
                let mut pixels = self.history.alloc_tile();
                *pixels = *tile.pixels;
                snapshots.push(crate::history::TileSnapshot {
                    layer_id: layer.id,
                    coords,
                    pixels: Some(pixels),
                });
            }
            
            let _dirty_tiles = self.transform_state.apply_transform(layer);
            layer.thumbnail_dirty = true;
            
            if !snapshots.is_empty() {
                self.history.push_command(crate::history::UndoCommand { snapshots });
                self.document_modified = true;
            }
        }
        
        self.transform_state.source_snapshot = None;
        if let Some(r) = &mut self.renderer {
            r.clear_cache();
        }
    }

    fn cancel_transform(&mut self) {
        if !self.transform_active { return; }
        self.transform_active = false;
        
        if let Some(snapshot) = self.transform_state.source_snapshot.take() {
            if let Some(layer) = self.layers.get_mut(&self.active_layer_id) {
                layer.tiles = snapshot.tiles;
                layer.thumbnail_dirty = true;
            }
        }
        
        if let Some(r) = &mut self.renderer {
            r.clear_cache();
        }
    }

    fn transform_point(&self, p: egui::Pos2) -> egui::Pos2 {
        let px = self.transform_pivot.x;
        let py = self.transform_pivot.y;
        let rx = p.x - px;
        let ry = p.y - py;
        
        let sx = rx * self.transform_scale.x;
        let sy = ry * self.transform_scale.y;
        
        let cos = self.transform_rotation.cos();
        let sin = self.transform_rotation.sin();
        let rot_x = sx * cos - sy * sin;
        let rot_y = sx * sin + sy * cos;
        
        egui::Pos2::new(rot_x + px + self.transform_translation.x, rot_y + py + self.transform_translation.y)
    }

    fn world_to_screen(&self, p: egui::Pos2, view_rect: egui::Rect) -> egui::Pos2 {
        let center = view_rect.center();
        let half_w = view_rect.width() * 0.5;
        let half_h = view_rect.height() * 0.5;

        let mut px = ((p.x - self.viewport_offset.x) * self.viewport_zoom) / half_w - 1.0;
        let py = 1.0 - ((p.y - self.viewport_offset.y) * self.viewport_zoom) / half_h;

        if self.mirror_horizontal {
            px = -px;
        }

        let cos_rot = (-self.rotation_angle).cos();
        let sin_rot = (-self.rotation_angle).sin();

        let nx = px * cos_rot + py * sin_rot;
        let ny = -px * sin_rot + py * cos_rot;

        let dx = nx * half_w;
        let dy = -ny * half_h;

        egui::Pos2::new(center.x + dx, center.y + dy)
    }

    fn draw_dashed_line(painter: &egui::Painter, p0: egui::Pos2, p1: egui::Pos2, time: f64) {
        let dx = p1.x - p0.x;
        let dy = p1.y - p0.y;
        let len = (dx*dx + dy*dy).sqrt();
        if len < 0.1 { return; }
        
        let dash_len = 4.0;
        let gap_len = 4.0;
        let pattern_len = dash_len + gap_len;
        let speed = 15.0;
        let offset = (time * speed).rem_euclid(pattern_len as f64) as f32;
        
        let mut t = 0.0;
        while t < len {
            let dash_start = (t - offset).max(0.0);
            let dash_end = (t - offset + dash_len).min(len);
            if dash_end > dash_start {
                let start_pt = egui::Pos2::new(
                    p0.x + (dx * dash_start / len),
                    p0.y + (dy * dash_start / len),
                );
                let end_pt = egui::Pos2::new(
                    p0.x + (dx * dash_end / len),
                    p0.y + (dy * dash_end / len),
                );
                painter.line_segment([start_pt, end_pt], egui::Stroke::new(1.0, egui::Color32::WHITE));
            }
            t += pattern_len;
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

        // 0. DIALOGS
        crate::ui::dialogs::draw_dialogs(self, ctx);

        // 1. TOP MENU PANEL
        crate::ui::menu::draw_menu_bar(self, ctx);

        // 2. QUICK BAR PANEL
        crate::ui::quick_bar::draw_quick_bar(self, ctx);

        // Autosave check
        if self.autosave_enabled {
            let current_time = ctx.input(|i| i.time);
            if self.last_autosave_time == 0.0 {
                self.last_autosave_time = current_time;
            }
            let time_elapsed = current_time - self.last_autosave_time;
            if time_elapsed > self.autosave_interval_secs && self.document_modified {
                self.save_canvas(std::path::Path::new(&self.autosave_path));
                self.document_modified = false;
                self.last_autosave_time = current_time;
                self.autosave_status = "Autosaved (Clean)".to_string();
                log::info!("Autosaved to {}", self.autosave_path);
            }

            // Update status text dynamically
            let current_time = ctx.input(|i| i.time);
            let time_elapsed = current_time - self.last_autosave_time;
            if self.document_modified {
                let remaining = (self.autosave_interval_secs - time_elapsed).max(0.0);
                self.autosave_status = format!("Autosave in {:.0}s", remaining);
            } else if self.autosave_status.is_empty() {
                self.autosave_status = "Autosave: Standby".to_string();
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

            if self.transform_active {
                if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                    self.commit_transform();
                }
                if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.cancel_transform();
                }
            }

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

        // 3. SIDEBARS & PANELS
        self.regenerate_dirty_thumbnails();
        crate::ui::left_panel::draw_left_panel(self, ctx);
        crate::ui::right_panel::draw_right_panel(self, ctx);
        crate::ui::status_bar::draw_status_bar(self, ctx);

        // 3. CENTRAL PANEL (DRAWING AREA)
        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                let rect = ui.max_rect();
                self.last_viewport_rect = Some(rect);
                self.last_viewport_size = rect.size();
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
                let mut pointer_down = (response.dragged_by(egui::PointerButton::Primary)
                    || (response.is_pointer_button_down_on() && ui.input(|i| i.pointer.button_down(egui::PointerButton::Primary))))
                    && !space_down && !r_down;
                let mut pointer_clicked = response.clicked_by(egui::PointerButton::Primary) && !space_down && !r_down;

                // Reference Image Dragging Interaction
                if matches!(self.active_tool, ToolId::Move | ToolId::Reference) {
                    if ui.input(|i| i.pointer.any_pressed()) {
                        if let Some(ptr_pos) = ui.input(|i| i.pointer.press_origin()) {
                            if response.hovered() {
                                // Hit test visible reference images from top of stack to bottom
                                let mut hit_idx = None;
                                for (idx, img) in self.reference_images.iter().enumerate().rev() {
                                    if !img.visible { continue; }
                                    let quad = self.ref_image_corners(img, rect);
                                    if point_in_quad(ptr_pos, &quad) {
                                        hit_idx = Some(idx);
                                        break;
                                    }
                                }
                                if let Some(idx) = hit_idx {
                                    self.selected_reference_idx = Some(idx);
                                    self.ref_image_dragging = Some(idx);
                                    
                                    let img = &self.reference_images[idx];
                                    if img.pinned_to_view {
                                        // Position in viewport/screen space
                                        self.ref_image_drag_offset = img.world_pos - (ptr_pos - rect.min);
                                    } else {
                                        // Position in world space
                                        let ptr_world = self.screen_to_world(ptr_pos, rect);
                                        self.ref_image_drag_offset = img.world_pos - ptr_world;
                                    }
                                }
                            }
                        }
                    }
                }
                
                if let Some(idx) = self.ref_image_dragging {
                    if idx < self.reference_images.len() {
                        if ui.input(|i| i.pointer.any_down()) {
                            if let Some(curr_ptr) = ui.input(|i| i.pointer.hover_pos()) {
                                let pinned_to_view = self.reference_images[idx].pinned_to_view;
                                if pinned_to_view {
                                    let screen_drag_pos = (curr_ptr - rect.min) + self.ref_image_drag_offset;
                                    self.reference_images[idx].world_pos = screen_drag_pos;
                                } else {
                                    let curr_world = self.screen_to_world(curr_ptr, rect);
                                    let world_drag_pos = curr_world + self.ref_image_drag_offset;
                                    self.reference_images[idx].world_pos = world_drag_pos;
                                }
                                ctx.request_repaint();
                            }
                            pointer_down = false;
                            pointer_clicked = false;
                        } else {
                            self.ref_image_dragging = None;
                        }
                    } else {
                        self.ref_image_dragging = None;
                    }
                }

                if self.transform_active {
                    if let Some(ptr_pos) = ui.input(|i| i.pointer.hover_pos()) {
                        let view_rect = rect;
                        let ob = self.transform_orig_bounds;
                        let handle_radius = 8.0;
                        
                        let mut hovered_handle = None;
                        
                        // Check rotation handle
                        let rot_h_orig = egui::Pos2::new(ob.center().x, ob.min.y - 30.0 / self.viewport_zoom);
                        let rot_h_screen = self.world_to_screen(self.transform_point(rot_h_orig), view_rect);
                        if ptr_pos.distance(rot_h_screen) <= handle_radius {
                            hovered_handle = Some(8);
                        }
                        
                        // Check pivot handle
                        if hovered_handle.is_none() {
                            let pivot_screen = self.world_to_screen(self.transform_pivot + self.transform_translation, view_rect);
                            if ptr_pos.distance(pivot_screen) <= handle_radius {
                                hovered_handle = Some(9);
                            }
                        }
                        
                        // Check 8 scaling handles
                        if hovered_handle.is_none() {
                            let orig_corners = [
                                egui::Pos2::new(ob.min.x, ob.min.y), // TL (0)
                                egui::Pos2::new(ob.center().x, ob.min.y), // TC (1)
                                egui::Pos2::new(ob.max.x, ob.min.y), // TR (2)
                                egui::Pos2::new(ob.max.x, ob.center().y), // MR (3)
                                egui::Pos2::new(ob.max.x, ob.max.y), // BR (4)
                                egui::Pos2::new(ob.center().x, ob.max.y), // BC (5)
                                egui::Pos2::new(ob.min.x, ob.max.y), // BL (6)
                                egui::Pos2::new(ob.min.x, ob.center().y), // ML (7)
                            ];
                            for i in 0..8 {
                                let h_screen = self.world_to_screen(self.transform_point(orig_corners[i]), view_rect);
                                if ptr_pos.distance(h_screen) <= handle_radius {
                                    hovered_handle = Some(i);
                                    break;
                                }
                            }
                        }
                        
                        // Check inside bounds (Translate)
                        if hovered_handle.is_none() {
                            let ptr_world = self.screen_to_world(ptr_pos, view_rect);
                            let rx = ptr_world.x - (self.transform_pivot.x + self.transform_translation.x);
                            let ry = ptr_world.y - (self.transform_pivot.y + self.transform_translation.y);
                            let cos = (-self.transform_rotation).cos();
                            let sin = (-self.transform_rotation).sin();
                            let ux = rx * cos - ry * sin;
                            let uy = rx * sin + ry * cos;
                            let x_orig = ux / self.transform_scale.x + self.transform_pivot.x;
                            let y_orig = uy / self.transform_scale.y + self.transform_pivot.y;
                            
                            if x_orig >= ob.min.x && x_orig <= ob.max.x && y_orig >= ob.min.y && y_orig <= ob.max.y {
                                hovered_handle = Some(10);
                            }
                        }
                        
                        if let Some(h) = hovered_handle {
                            let cursor = match h {
                                8 => egui::CursorIcon::PointingHand,
                                9 => egui::CursorIcon::Crosshair,
                                0 | 4 => egui::CursorIcon::ResizeNwSe,
                                2 | 6 => egui::CursorIcon::ResizeNeSw,
                                1 | 5 => egui::CursorIcon::ResizeVertical,
                                3 | 7 => egui::CursorIcon::ResizeHorizontal,
                                10 => egui::CursorIcon::Move,
                                _ => egui::CursorIcon::Default,
                            };
                            ui.ctx().set_cursor_icon(cursor);
                        }
                        
                        if ui.input(|i| i.pointer.any_pressed()) {
                            if let Some(h) = hovered_handle {
                                self.transform_dragging = Some(h);
                                self.transform_drag_start_ptr = Some(ptr_pos);
                                self.transform_drag_start_translation = self.transform_translation;
                                self.transform_drag_start_scale = self.transform_scale;
                                self.transform_drag_start_rotation = self.transform_rotation;
                            }
                        }
                    }
                    
                    if let Some(h) = self.transform_dragging {
                        if ui.input(|i| i.pointer.any_down()) {
                            if let (Some(start_ptr), Some(curr_ptr)) = (self.transform_drag_start_ptr, ui.input(|i| i.pointer.hover_pos())) {
                                let start_w = self.screen_to_world(start_ptr, rect);
                                let curr_w = self.screen_to_world(curr_ptr, rect);
                                let delta_w = curr_w - start_w;
                                
                                match h {
                                    10 => {
                                        self.transform_translation = self.transform_drag_start_translation + delta_w;
                                    }
                                    9 => {
                                        let orig_pivot = self.transform_pivot;
                                        let new_pivot = orig_pivot + delta_w;
                                        let ob = self.transform_orig_bounds;
                                        self.transform_pivot = egui::Pos2::new(
                                            new_pivot.x.clamp(ob.min.x, ob.max.x),
                                            new_pivot.y.clamp(ob.min.y, ob.max.y),
                                        );
                                    }
                                    8 => {
                                        let pivot_w = self.transform_pivot + self.transform_translation;
                                        let start_vec = start_w - pivot_w.to_vec2();
                                        let curr_vec = curr_w - pivot_w.to_vec2();
                                        let start_ang = start_vec.y.atan2(start_vec.x);
                                        let curr_ang = curr_vec.y.atan2(curr_vec.x);
                                        let diff_ang = curr_ang - start_ang;
                                        self.transform_rotation = self.transform_drag_start_rotation + diff_ang;
                                    }
                                    0..=7 => {
                                        let ob = self.transform_orig_bounds;
                                        let orig_corners = [
                                            egui::Pos2::new(ob.min.x, ob.min.y), // TL (0)
                                            egui::Pos2::new(ob.center().x, ob.min.y), // TC (1)
                                            egui::Pos2::new(ob.max.x, ob.min.y), // TR (2)
                                            egui::Pos2::new(ob.max.x, ob.center().y), // MR (3)
                                            egui::Pos2::new(ob.max.x, ob.max.y), // BR (4)
                                            egui::Pos2::new(ob.center().x, ob.max.y), // BC (5)
                                            egui::Pos2::new(ob.min.x, ob.max.y), // BL (6)
                                            egui::Pos2::new(ob.min.x, ob.center().y), // ML (7)
                                        ];
                                        let orig_h = orig_corners[h];
                                        let orig_offset = orig_h - self.transform_pivot;
                                        
                                        let cos = (-self.transform_drag_start_rotation).cos();
                                        let sin = (-self.transform_drag_start_rotation).sin();
                                        let local_delta_x = delta_w.x * cos - delta_w.y * sin;
                                        let local_delta_y = delta_w.x * sin + delta_w.y * cos;
                                        
                                        let mut scale_x = self.transform_drag_start_scale.x;
                                        let mut scale_y = self.transform_drag_start_scale.y;
                                        
                                        if orig_offset.x.abs() > 0.01 {
                                            let new_local_x = orig_offset.x * self.transform_drag_start_scale.x + local_delta_x;
                                            scale_x = new_local_x / orig_offset.x;
                                        }
                                        if orig_offset.y.abs() > 0.01 {
                                            let new_local_y = orig_offset.y * self.transform_drag_start_scale.y + local_delta_y;
                                            scale_y = new_local_y / orig_offset.y;
                                        }
                                        
                                        scale_x = scale_x.max(0.05);
                                        scale_y = scale_y.max(0.05);
                                        
                                        if ui.input(|i| i.modifiers.shift) {
                                            if h == 0 || h == 2 || h == 4 || h == 6 {
                                                let avg_scale = (scale_x + scale_y) * 0.5;
                                                scale_x = avg_scale;
                                                scale_y = avg_scale;
                                            }
                                        }
                                        
                                        self.transform_scale = egui::Vec2::new(scale_x, scale_y);
                                    }
                                    _ => {}
                                }
                            }
                        } else {
                            self.transform_dragging = None;
                            self.transform_drag_start_ptr = None;
                        }
                    }
                    
                    pointer_down = false;
                    pointer_clicked = false;
                }

                // Handle selection tool dragging
                if pointer_down && matches!(self.active_tool, ToolId::RectSelect | ToolId::EllipseSelect) {
                    if let Some(ptr_pos) = response.hover_pos() {
                        let world_pos = self.screen_to_world(ptr_pos, rect);
                        if !self.is_selecting {
                            self.is_selecting = true;
                            if self.selection_mode == selection::SelectionMode::Replace {
                                selection::clear_selection(&mut self.selection_mask);
                            }
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
                            if self.selection_mode == selection::SelectionMode::Replace {
                                selection::clear_selection(&mut self.selection_mask);
                            }
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
                            let cloned_layers: Vec<Layer> = self.layers.values().cloned().collect();
                            let all_layers: Vec<&Layer> = cloned_layers.iter().collect();
                            if let Some(layer) = self.layers.get_mut(&self.active_layer_id) {
                                // Capture pre-fill snapshots of all existing tiles
                                let mut snapshots: Vec<crate::history::TileSnapshot> = Vec::new();
                                let tile_keys: Vec<(i32, i32)> = layer.tiles.keys().copied().collect();
                                for &tk in &tile_keys {
                                    if let Some(tile) = layer.tiles.get(&tk) {
                                        let mut pixels = self.history.alloc_tile();
                                        *pixels = *tile.pixels;
                                        snapshots.push(crate::history::TileSnapshot {
                                            layer_id: layer.id,
                                            coords: tk,
                                            pixels: Some(pixels),
                                        });
                                    }
                                }

                                let dirty = fill::flood_fill(
                                    layer,
                                    &all_layers,
                                    &self.selection_mask,
                                    fx, fy,
                                    fill_color,
                                    &self.fill_options,
                                    self.canvas_width as i32,
                                    self.canvas_height as i32,
                                );
                                if !dirty.is_empty() {
                                    // Capture snapshots for any newly created tiles
                                    let new_keys: Vec<(i32, i32)> = layer.tiles.keys().copied().collect();
                                    for &tk in &new_keys {
                                        if !tile_keys.contains(&tk) {
                                            snapshots.push(crate::history::TileSnapshot {
                                                layer_id: layer.id,
                                                coords: tk,
                                                pixels: None, // tile did not exist before
                                            });
                                        }
                                    }

                                    self.history.push_command(crate::history::UndoCommand { snapshots });
                                    self.document_modified = true;
                                    if let Some(r) = &mut self.renderer {
                                        r.clear_cache();
                                    }
                                }
                            }
                        }
                    }
                }

                // Handle magic wand click
                if pointer_clicked && matches!(self.active_tool, ToolId::MagicWand) {
                    if let Some(ptr_pos) = response.hover_pos() {
                        let world_pos = self.screen_to_world(ptr_pos, rect);
                        let wx = world_pos.x as i32;
                        let wy = world_pos.y as i32;
                        if wx >= 0 && wx < self.canvas_width as i32 && wy >= 0 && wy < self.canvas_height as i32 {
                            let all_layers: Vec<&Layer> = self.layer_order.iter().filter_map(|id| self.layers.get(id)).collect();
                            if let Some(active_layer) = self.layers.get(&self.active_layer_id) {
                                selection::magic_wand_select(
                                    &mut self.selection_mask,
                                    &all_layers,
                                    active_layer,
                                    wx,
                                    wy,
                                    &self.fill_options,
                                    self.selection_mode,
                                    self.canvas_width as i32,
                                    self.canvas_height as i32,
                                );
                                self.show_selection_overlay = self.selection_mask.is_active;
                            }
                        }
                    }
                }

                // Handle color picker (eyedropper) tool click/drag
                if (pointer_clicked || pointer_down) && matches!(self.active_tool, ToolId::ColorPicker) {
                    if let Some(ptr_pos) = response.hover_pos() {
                        let world_pos = self.screen_to_world(ptr_pos, rect);
                        let px = world_pos.x as i32;
                        let py = world_pos.y as i32;
                        if px >= 0 && px < self.canvas_width as i32 && py >= 0 && py < self.canvas_height as i32 {
                            if let Some(active_layer) = self.layers.get(&self.active_layer_id) {
                                let all_layers: Vec<&Layer> = self.layer_order.iter().filter_map(|id| self.layers.get(id)).collect();
                                let sampled = fill::sample_reference(&all_layers, active_layer, fill::FillReference::AllVisibleLayers, px, py);
                                let a = sampled[3] as f32 / 32768.0;
                                if a > 0.0 {
                                    self.brush_color[0] = (sampled[0] as f32 / 32768.0) / a;
                                    self.brush_color[1] = (sampled[1] as f32 / 32768.0) / a;
                                    self.brush_color[2] = (sampled[2] as f32 / 32768.0) / a;
                                } else {
                                    self.brush_color[0] = sampled[0] as f32 / 32768.0;
                                    self.brush_color[1] = sampled[1] as f32 / 32768.0;
                                    self.brush_color[2] = sampled[2] as f32 / 32768.0;
                                }
                                self.brush_color[0] = self.brush_color[0].clamp(0.0, 1.0);
                                self.brush_color[1] = self.brush_color[1].clamp(0.0, 1.0);
                                self.brush_color[2] = self.brush_color[2].clamp(0.0, 1.0);
                                self.record_color(self.brush_color);
                                self.brush_settings_dirty = true;
                            }
                        }
                    }
                }

                // Handle brush/eraser stroke drawing
                if pointer_down && matches!(self.active_tool, ToolId::Brush | ToolId::Eraser) {
                    if let Some(ptr_pos) = response.hover_pos() {
                        let world_pos = self.screen_to_world(ptr_pos, rect);
                        let cx = world_pos.x;
                        let cy = world_pos.y;

                        // Initialize the drawing state if this is a fresh stroke
                        if !self.stabilizer.is_drawing {
                            self.stabilizer.reset();
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
                }

                if !pointer_down {
                    // Finalize selection if dragging ended
                    if self.is_selecting {
                        self.is_selecting = false;
                        if let Some(rect) = self.selection_rect.take() {
                            let w = (rect.x1 - rect.x0).abs();
                            let h = (rect.y1 - rect.y0).abs();
                            if w <= 1.0 && h <= 1.0 {
                                if self.selection_mode == selection::SelectionMode::Replace {
                                    selection::clear_selection(&mut self.selection_mask);
                                }
                            } else {
                                if self.active_tool == ToolId::RectSelect {
                                    selection::apply_rect_selection(
                                        &mut self.selection_mask, rect,
                                        self.selection_mode,
                                        self.selection_feather, true,
                                    );
                                } else if self.active_tool == ToolId::EllipseSelect {
                                    selection::apply_ellipse_selection(
                                        &mut self.selection_mask, rect,
                                        self.selection_mode,
                                        self.selection_feather, true,
                                    );
                                }
                            }
                        }
                        if let Some(lasso) = self.lasso_points.take() {
                            if lasso.points.len() <= 2 {
                                if self.selection_mode == selection::SelectionMode::Replace {
                                    selection::clear_selection(&mut self.selection_mask);
                                }
                            } else if lasso.points.len() >= 3 {
                                selection::apply_lasso_selection(
                                    &mut self.selection_mask, &lasso,
                                    self.selection_mode,
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

                    let transform_preview = if self.transform_active {
                        Some(crate::renderer::TransformPreviewParams {
                            layer_id: self.active_layer_id,
                            translation: self.transform_translation,
                            scale: self.transform_scale,
                            rotation: self.transform_rotation,
                            pivot: self.transform_pivot,
                        })
                    } else {
                        None
                    };

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
                        transform_preview,
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

                    // Render Reference Images
                    for (ref_idx, ref_img) in self.reference_images.iter().enumerate() {
                        if !ref_img.visible { continue; }
                        if let Some(texture_handle) = &ref_img.texture {
                            let w = ref_img.size.x;
                            let h = ref_img.size.y;
                            let half_size = egui::vec2(w * 0.5, h * 0.5);
                            let corners = [
                                egui::vec2(-half_size.x, -half_size.y), // TL
                                egui::vec2(half_size.x, -half_size.y),  // TR
                                egui::vec2(half_size.x, half_size.y),   // BR
                                egui::vec2(-half_size.x, half_size.y),  // BL
                            ];
                            
                            // Rotate and scale corner offset locally around reference center
                            let cos_r = ref_img.rotation.cos();
                            let sin_r = ref_img.rotation.sin();
                            let transform_local = |p: egui::Vec2| -> egui::Vec2 {
                                let sx = p.x * ref_img.scale;
                                let sy = p.y * ref_img.scale;
                                egui::vec2(
                                    sx * cos_r - sy * sin_r,
                                    sx * sin_r + sy * cos_r,
                                )
                            };
                            
                            let mut screen_pos = [egui::Pos2::ZERO; 4];
                            if ref_img.pinned_to_view {
                                // Pinned to View (viewport screen-relative)
                                let center_screen = rect.min + ref_img.world_pos;
                                for i in 0..4 {
                                    screen_pos[i] = center_screen + transform_local(corners[i]);
                                }
                            } else {
                                // Pinned to Canvas (world/canvas-relative)
                                for i in 0..4 {
                                    let pt_world = ref_img.world_pos + transform_local(corners[i]);
                                    screen_pos[i] = self.world_to_screen(pt_world.to_pos2(), rect);
                                }
                            }
                            
                            // Render quad mesh
                            let uvs = [
                                egui::pos2(0.0, 0.0),
                                egui::pos2(1.0, 0.0),
                                egui::pos2(1.0, 1.0),
                                egui::pos2(0.0, 1.0),
                            ];
                            let alpha = (ref_img.opacity * 255.0).clamp(0.0, 255.0) as u8;
                            let color = egui::Color32::from_rgba_premultiplied(alpha, alpha, alpha, alpha);
                            
                            let mut mesh = egui::Mesh::with_texture(texture_handle.id());
                            for i in 0..4 {
                                mesh.vertices.push(egui::epaint::Vertex {
                                    pos: screen_pos[i],
                                    uv: uvs[i],
                                    color,
                                });
                            }
                            mesh.add_triangle(0, 1, 2);
                            mesh.add_triangle(0, 2, 3);
                            ui.painter().add(egui::Shape::mesh(mesh));
                            
                            // If selected, draw active transform / selection dashed border
                            if self.selected_reference_idx == Some(ref_idx) {
                                let border_color = egui::Color32::from_rgb(0, 120, 215);
                                let stroke = egui::Stroke::new(1.5, border_color);
                                for i in 0..4 {
                                    ui.painter().line_segment(
                                        [screen_pos[i], screen_pos[(i + 1) % 4]],
                                        stroke,
                                    );
                                }
                                // Draw a small square handle at each corner
                                for i in 0..4 {
                                    ui.painter().rect_filled(
                                        egui::Rect::from_center_size(screen_pos[i], egui::vec2(6.0, 6.0)),
                                        1.0,
                                        border_color,
                                    );
                                    ui.painter().rect_stroke(
                                        egui::Rect::from_center_size(screen_pos[i], egui::vec2(6.0, 6.0)),
                                        1.0,
                                        egui::Stroke::new(1.0, egui::Color32::WHITE),
                                    );
                                }
                            }
                        }
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

                // GRID OVERLAY (clipped to canvas bounds)
                if self.show_grid {
                    let grid_size = 64.0;
                    let canvas_left = 0.0;
                    let canvas_right = self.canvas_width as f32;
                    let canvas_top = 0.0;
                    let canvas_bottom = self.canvas_height as f32;

                    let view_start_x = (self.viewport_offset.x / grid_size).floor() * grid_size;
                    let view_start_y = (self.viewport_offset.y / grid_size).floor() * grid_size;
                    let view_end_x = self.viewport_offset.x + rect.width() / self.viewport_zoom;
                    let view_end_y = self.viewport_offset.y + rect.height() / self.viewport_zoom;

                    // Clamp to canvas bounds
                    let start_x = view_start_x.max(canvas_left);
                    let start_y = view_start_y.max(canvas_top);
                    let end_x = view_end_x.min(canvas_right);
                    let end_y = view_end_y.min(canvas_bottom);

                    let mut gx = start_x;
                    while gx <= end_x {
                        let sx = ((gx - self.viewport_offset.x) * self.viewport_zoom) + rect.min.x;
                        // Only draw vertical lines within canvas Y range
                        let top_sy = ((canvas_top - self.viewport_offset.y) * self.viewport_zoom) + rect.min.y;
                        let bot_sy = ((canvas_bottom - self.viewport_offset.y) * self.viewport_zoom) + rect.min.y;
                        ui.painter().line_segment(
                            [egui::Pos2::new(sx, top_sy), egui::Pos2::new(sx, bot_sy)],
                            egui::Stroke::new(0.5, Color32::from_black_alpha(40)),
                        );
                        gx += grid_size;
                    }
                    let mut gy = start_y;
                    while gy <= end_y {
                        let sy = ((gy - self.viewport_offset.y) * self.viewport_zoom) + rect.min.y;
                        let left_sx = ((canvas_left - self.viewport_offset.x) * self.viewport_zoom) + rect.min.x;
                        let right_sx = ((canvas_right - self.viewport_offset.x) * self.viewport_zoom) + rect.min.x;
                        ui.painter().line_segment(
                            [egui::Pos2::new(left_sx, sy), egui::Pos2::new(right_sx, sy)],
                            egui::Stroke::new(0.5, Color32::from_black_alpha(40)),
                        );
                        gy += grid_size;
                    }
                }

                // SELECTION OVERLAY (marching ants or mask)
                if self.show_selection_overlay && self.selection_mask.is_active {
                    let time = ui.input(|i| i.time);
                    if self.selection_display_mode == SelectionDisplayMode::BlueOverlay {
                        for (&(tx, ty), tile) in &self.selection_mask.tiles {
                            for ly in 0..64 {
                                for lx in 0..64 {
                                    let val = tile[ly * 64 + lx];
                                    if val > 0 {
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
                    } else if self.selection_display_mode == SelectionDisplayMode::MarchingAnts {
                        let view_min_world = self.screen_to_world(rect.min, rect);
                        let view_max_world = self.screen_to_world(rect.max, rect);
                        let tx_min = (view_min_world.x.min(view_max_world.x) as i32).div_euclid(64) - 1;
                        let tx_max = (view_min_world.x.max(view_max_world.x) as i32).div_euclid(64) + 1;
                        let ty_min = (view_min_world.y.min(view_max_world.y) as i32).div_euclid(64) - 1;
                        let ty_max = (view_min_world.y.max(view_max_world.y) as i32).div_euclid(64) + 1;

                        for (&(tx, ty), tile) in &self.selection_mask.tiles {
                            if tx < tx_min || tx > tx_max || ty < ty_min || ty > ty_max {
                                continue;
                            }
                            for ly in 0..64 {
                                for lx in 0..64 {
                                    let val = tile[ly * 64 + lx];
                                    if val > 127 {
                                        let wx = tx * 64 + lx as i32;
                                        let wy = ty * 64 + ly as i32;
                                        
                                        // Check right neighbor
                                        let r_val = self.selection_mask.get_value(wx + 1, wy);
                                        if r_val <= 127 {
                                            let p0 = self.world_to_screen(egui::Pos2::new(wx as f32 + 1.0, wy as f32), rect);
                                            let p1 = self.world_to_screen(egui::Pos2::new(wx as f32 + 1.0, wy as f32 + 1.0), rect);
                                            ui.painter().line_segment([p0, p1], egui::Stroke::new(1.0, egui::Color32::BLACK));
                                            Self::draw_dashed_line(ui.painter(), p0, p1, time);
                                        }
                                        // Check bottom neighbor
                                        let b_val = self.selection_mask.get_value(wx, wy + 1);
                                        if b_val <= 127 {
                                            let p0 = self.world_to_screen(egui::Pos2::new(wx as f32, wy as f32 + 1.0), rect);
                                            let p1 = self.world_to_screen(egui::Pos2::new(wx as f32 + 1.0, wy as f32 + 1.0), rect);
                                            ui.painter().line_segment([p0, p1], egui::Stroke::new(1.0, egui::Color32::BLACK));
                                            Self::draw_dashed_line(ui.painter(), p0, p1, time);
                                        }
                                        // Check left neighbor
                                        let l_val = self.selection_mask.get_value(wx - 1, wy);
                                        if l_val <= 127 {
                                            let p0 = self.world_to_screen(egui::Pos2::new(wx as f32, wy as f32), rect);
                                            let p1 = self.world_to_screen(egui::Pos2::new(wx as f32, wy as f32 + 1.0), rect);
                                            ui.painter().line_segment([p0, p1], egui::Stroke::new(1.0, egui::Color32::BLACK));
                                            Self::draw_dashed_line(ui.painter(), p0, p1, time);
                                        }
                                        // Check top neighbor
                                        let t_val = self.selection_mask.get_value(wx, wy - 1);
                                        if t_val <= 127 {
                                            let p0 = self.world_to_screen(egui::Pos2::new(wx as f32, wy as f32), rect);
                                            let p1 = self.world_to_screen(egui::Pos2::new(wx as f32 + 1.0, wy as f32), rect);
                                            ui.painter().line_segment([p0, p1], egui::Stroke::new(1.0, egui::Color32::BLACK));
                                            Self::draw_dashed_line(ui.painter(), p0, p1, time);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // TRANSFORM OVERLAY
                if self.transform_active {
                    let ob = self.transform_orig_bounds;
                    let stroke_blue = egui::Stroke::new(1.5, egui::Color32::from_rgb(0, 120, 215));
                    
                    let p0 = self.world_to_screen(self.transform_point(egui::Pos2::new(ob.min.x, ob.min.y)), rect);
                    let p1 = self.world_to_screen(self.transform_point(egui::Pos2::new(ob.max.x, ob.min.y)), rect);
                    let p2 = self.world_to_screen(self.transform_point(egui::Pos2::new(ob.max.x, ob.max.y)), rect);
                    let p3 = self.world_to_screen(self.transform_point(egui::Pos2::new(ob.min.x, ob.max.y)), rect);
                    
                    ui.painter().line_segment([p0, p1], stroke_blue);
                    ui.painter().line_segment([p1, p2], stroke_blue);
                    ui.painter().line_segment([p2, p3], stroke_blue);
                    ui.painter().line_segment([p3, p0], stroke_blue);
                    
                    let rot_h_orig = egui::Pos2::new(ob.center().x, ob.min.y - 30.0 / self.viewport_zoom);
                    let rot_h_screen = self.world_to_screen(self.transform_point(rot_h_orig), rect);
                    let top_center_screen = self.world_to_screen(self.transform_point(egui::Pos2::new(ob.center().x, ob.min.y)), rect);
                    ui.painter().line_segment([top_center_screen, rot_h_screen], stroke_blue);
                    
                    ui.painter().circle_filled(rot_h_screen, 6.0, egui::Color32::from_rgb(40, 200, 40));
                    ui.painter().circle_stroke(rot_h_screen, 6.0, egui::Stroke::new(1.0, egui::Color32::WHITE));
                    
                    let orig_corners = [
                        egui::Pos2::new(ob.min.x, ob.min.y), // TL (0)
                        egui::Pos2::new(ob.center().x, ob.min.y), // TC (1)
                        egui::Pos2::new(ob.max.x, ob.min.y), // TR (2)
                        egui::Pos2::new(ob.max.x, ob.center().y), // MR (3)
                        egui::Pos2::new(ob.max.x, ob.max.y), // BR (4)
                        egui::Pos2::new(ob.center().x, ob.max.y), // BC (5)
                        egui::Pos2::new(ob.min.x, ob.max.y), // BL (6)
                        egui::Pos2::new(ob.min.x, ob.center().y), // ML (7)
                    ];
                    
                    for i in 0..8 {
                        let h_screen = self.world_to_screen(self.transform_point(orig_corners[i]), rect);
                        ui.painter().rect_filled(
                            egui::Rect::from_center_size(h_screen, egui::Vec2::new(8.0, 8.0)),
                            0.0,
                            egui::Color32::WHITE,
                        );
                        ui.painter().rect_stroke(
                            egui::Rect::from_center_size(h_screen, egui::Vec2::new(8.0, 8.0)),
                            0.0,
                            stroke_blue,
                        );
                    }
                    
                    let pivot_screen = self.world_to_screen(self.transform_pivot + self.transform_translation, rect);
                    ui.painter().circle_stroke(pivot_screen, 8.0, stroke_blue);
                    ui.painter().circle_filled(pivot_screen, 2.0, egui::Color32::from_rgb(0, 120, 215));
                }

                // PERFORMANCE HUD OVERLAY
                if self.show_performance_hud {
                    let hud_rect = egui::Rect::from_min_size(
                        rect.right_top() - egui::vec2(210.0, -10.0),
                        egui::Vec2::new(200.0, 160.0),
                    );
                    ui.painter().rect_filled(hud_rect, 6.0, egui::Color32::from_rgba_premultiplied(30, 30, 30, 200));
                    ui.painter().rect_stroke(hud_rect, 6.0, egui::Stroke::new(1.0, egui::Color32::from_white_alpha(50)));
                    
                    let mut hud_ui = ui.child_ui(hud_rect.shrink(8.0), egui::Layout::top_down(egui::Align::Min));
                    
                    let mut active_tiles = 0;
                    let mut dirty_tiles = 0;
                    for layer in self.layers.values() {
                        active_tiles += layer.tiles.len();
                        dirty_tiles += layer.tiles.values().filter(|t| t.is_dirty).count();
                    }
                    
                    let clipboard_info = match &self.clipboard {
                        Some(c) => format!("{}x{} ({} px)", c.width, c.height, c.pixels.len()),
                        None => "Empty".to_string(),
                    };

                    let undo_size = self.history.undo_stack.len();
                    let redo_size = self.history.redo_stack.len();

                    hud_ui.colored_label(egui::Color32::GREEN, "PERFORMANCE HUD");
                    hud_ui.separator();
                    hud_ui.colored_label(egui::Color32::WHITE, format!("FPS: {:.1}", 1.0 / ctx.input(|i| i.predicted_dt)));
                    hud_ui.colored_label(egui::Color32::WHITE, format!("Active Tiles: {}", active_tiles));
                    hud_ui.colored_label(egui::Color32::WHITE, format!("Dirty Tiles: {}", dirty_tiles));
                    hud_ui.colored_label(egui::Color32::WHITE, format!("Undo Queue: {}", undo_size));
                    hud_ui.colored_label(egui::Color32::WHITE, format!("Redo Queue: {}", redo_size));
                    hud_ui.colored_label(egui::Color32::WHITE, format!("Clipboard: {}", clipboard_info));
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
    pub(crate) fn screen_to_world(&self, screen_pos: egui::Pos2, view_rect: egui::Rect) -> egui::Vec2 {
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

    pub fn load_reference_image(&mut self, path_str: &str, ctx: &egui::Context) -> Result<(), String> {
        let path = std::path::PathBuf::from(path_str);
        if !path.exists() {
            return Err("File does not exist".to_string());
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("Failed to read file: {}", e))?;
        
        // Decode PNG image using the image crate
        let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
            .map_err(|e| format!("Failed to decode PNG image: {}", e))?;
            
        let rgba = img.to_rgba8();
        let width = rgba.width();
        let height = rgba.height();
        
        let color_img = egui::ColorImage::from_rgba_unmultiplied(
            [width as usize, height as usize],
            rgba.as_raw(),
        );
        
        self.reference_id_counter += 1;
        let name = path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("ref_{}", self.reference_id_counter));
            
        let texture = ctx.load_texture(
            format!("ref_img_{}", self.reference_id_counter),
            color_img,
            egui::TextureOptions::default(),
        );
        
        let ref_img = ReferenceImage {
            id: self.reference_id_counter,
            name,
            path,
            visible: true,
            opacity: 1.0,
            pinned_to_view: false, // Default to pinned to canvas (world coordinates)
            world_pos: egui::vec2(self.canvas_width as f32 * 0.5, self.canvas_height as f32 * 0.5),
            scale: 1.0,
            rotation: 0.0,
            size: egui::vec2(width as f32, height as f32),
            texture: Some(texture),
        };
        
        self.reference_images.push(ref_img);
        self.selected_reference_idx = Some(self.reference_images.len() - 1);
        Ok(())
    }

    pub(crate) fn ref_image_corners(&self, ref_img: &ReferenceImage, view_rect: egui::Rect) -> [egui::Pos2; 4] {
        let w = ref_img.size.x;
        let h = ref_img.size.y;
        let half_size = egui::vec2(w * 0.5, h * 0.5);
        let corners = [
            egui::vec2(-half_size.x, -half_size.y), // TL
            egui::vec2(half_size.x, -half_size.y),  // TR
            egui::vec2(half_size.x, half_size.y),   // BR
            egui::vec2(-half_size.x, half_size.y),  // BL
        ];
        
        let cos_r = ref_img.rotation.cos();
        let sin_r = ref_img.rotation.sin();
        let transform_local = |p: egui::Vec2| -> egui::Vec2 {
            let sx = p.x * ref_img.scale;
            let sy = p.y * ref_img.scale;
            egui::vec2(
                sx * cos_r - sy * sin_r,
                sx * sin_r + sy * cos_r,
            )
        };
        
        let mut screen_pos = [egui::Pos2::ZERO; 4];
        if ref_img.pinned_to_view {
            let center_screen = view_rect.min + ref_img.world_pos;
            for i in 0..4 {
                screen_pos[i] = center_screen + transform_local(corners[i]);
            }
        } else {
            for i in 0..4 {
                let pt_world = ref_img.world_pos + transform_local(corners[i]);
                screen_pos[i] = self.world_to_screen(pt_world.to_pos2(), view_rect);
            }
        }
        screen_pos
    }
}

pub fn point_in_quad(p: egui::Pos2, quad: &[egui::Pos2; 4]) -> bool {
    let mut inside = true;
    for i in 0..4 {
        let p0 = quad[i];
        let p1 = quad[(i + 1) % 4];
        let v0 = p1 - p0;
        let v1 = p - p0;
        let cross = v0.x * v1.y - v0.y * v1.x;
        if cross < 0.0 {
            inside = false;
            break;
        }
    }
    if inside { return true; }
    
    let mut inside = true;
    for i in 0..4 {
        let p0 = quad[i];
        let p1 = quad[(i + 1) % 4];
        let v0 = p1 - p0;
        let v1 = p - p0;
        let cross = v0.x * v1.y - v0.y * v1.x;
        if cross > 0.0 {
            inside = false;
            break;
        }
    }
    inside
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

pub(crate) fn draw_hsv_color_wheel(ui: &mut egui::Ui, color: &mut [f32; 3], drag_zone: &mut Option<u8>) -> egui::Response {
    let desired_size = egui::Vec2::new(160.0, 160.0);
    let (rect, mut response) = ui.allocate_exact_size(desired_size, egui::Sense::click_and_drag());

    let center = rect.center();
    let outer_radius = rect.width() * 0.45;
    let inner_radius = rect.width() * 0.33;

    let (mut h, mut s, mut v) = rgb_to_hsv(color[0], color[1], color[2]);

    let half_side = inner_radius / 2.0f32.sqrt();
    let box_rect = egui::Rect::from_center_size(center, egui::Vec2::new(half_side * 2.0, half_side * 2.0));

    let zone_for_point = |p: egui::Pos2| -> u8 {
        let dist = p.distance(center);
        if box_rect.shrink(3.0).contains(p) {
            2 // square
        } else if dist >= inner_radius - 2.0 && dist <= outer_radius + 2.0 {
            1 // ring
        } else {
            0 // dead zone
        }
    };

    if response.drag_started() || response.clicked() || (response.is_pointer_button_down_on() && drag_zone.is_none()) {
        if let Some(p) = response.interact_pointer_pos() {
            *drag_zone = Some(zone_for_point(p));
        }
    }

    if response.is_pointer_button_down_on() || response.dragged() || response.clicked() {
        if let Some(mouse_pos) = response.interact_pointer_pos() {
            let to_mouse = mouse_pos - center;
            let zone = drag_zone.unwrap_or_else(|| {
                let z = zone_for_point(mouse_pos);
                *drag_zone = Some(z);
                z
            });

            if zone == 1 {
                // Hue ring
                let angle = to_mouse.y.atan2(to_mouse.x);
                let angle = if angle < 0.0 { angle + 2.0 * std::f32::consts::PI } else { angle };
                h = angle / (2.0 * std::f32::consts::PI);
                // If pure monochrome or too dark, automatically set Saturation/Value to 0.8 to unlock
                if s < 0.15 { s = 0.8; }
                if v < 0.20 { v = 0.8; }
            } else {
                // Sat/Val square
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
    } else {
        *drag_zone = None;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_point_in_quad() {
        let quad = [
            egui::pos2(0.0, 0.0),
            egui::pos2(10.0, 0.0),
            egui::pos2(10.0, 10.0),
            egui::pos2(0.0, 10.0),
        ];

        // Inside
        assert!(point_in_quad(egui::pos2(5.0, 5.0), &quad));
        assert!(point_in_quad(egui::pos2(1.0, 1.0), &quad));

        // Outside
        assert!(!point_in_quad(egui::pos2(-1.0, 5.0), &quad));
        assert!(!point_in_quad(egui::pos2(5.0, 11.0), &quad));
    }
}
