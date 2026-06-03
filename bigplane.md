# ARTY / Xcalux Digital Painting Workstation  
## Full Consolidated Product, UI, Feature, and Technical Development Plan

This document consolidates everything discussed so far into one complete English specification. It is written as a practical development roadmap for building **ARTY / Xcalux** into a clean, fast, lightweight digital painting application inspired by **PaintTool SAI**, while keeping the existing Rust/WGPU architecture efficient and maintainable.

---

# 1. Product Vision

ARTY / Xcalux should not try to become a full Photoshop or Krita clone. Its strongest direction is:

> **A lightweight, responsive, artist-focused raster painting program written in Rust, inspired by PaintTool SAI, with smooth brush strokes, simple UI, strong layer workflow, fast canvas navigation, and low memory overhead.**

The core design goals should be:

1. **Clean UI**
   - Minimal visual noise.
   - Common painting actions visible.
   - Advanced settings hidden inside collapsible panels or dialogs.

2. **Fast brush feel**
   - Low-latency input.
   - Strong stabilization.
   - Zero-allocation drawing hot path.
   - Smooth pressure response.

3. **Lightweight architecture**
   - Sparse tiled canvas.
   - Incremental GPU tile upload.
   - Dirty-flag-based updates.
   - Avoid per-frame rebuilds where possible.

4. **SAI-like workflow**
   - Simple brush preset box.
   - Quick view controls: zoom, rotate, mirror.
   - Fast layer operations.
   - Selection, transform, fill, and clipping features.

5. **Artist reliability**
   - Autosave.
   - Crash recovery.
   - Export formats.
   - Tablet diagnostics.
   - Undo/redo for all important operations.

---

# 2. Current Architecture Summary

Based on the provided technical documentation, ARTY already has a strong foundation.

## Existing Core Systems

| System | Current State |
|---|---|
| UI | egui / eframe three-panel layout |
| Rendering | WGPU renderer with GPU compositing |
| Canvas | Sparse infinite tiled canvas |
| Tile size | 64×64 pixels |
| Pixel format | fix15 premultiplied RGBA |
| GPU cache | 4096 tile texture slots |
| Brush engine | Hokusai / libmypaint-based stroke engine |
| Stabilizer | EMA and Spring-Mass-Damper modes |
| Tablet input | winit pressure + optional RealTimeStylus |
| Layers | Raster layers, folders, opacity, blend modes, clipping |
| Brush presets | Dynamic `Vec<BrushPreset>` |
| Undo | Tile snapshot history with object pool |
| Save/load | Native `.arty` background save/load |
| Performance | Stress test with zero-allocation hot path verification |

This is already a good architecture for a lightweight painting application.

---

# 3. High-Level Feature Roadmap

The most important features should be developed in this order:

## Priority 0 — Required for Real Artwork

These are the features that make the application usable for actual illustration work.

| Feature | Purpose |
|---|---|
| New document dialog | Create proper canvas sizes |
| Export PNG | Essential output format |
| Autosave | Prevent data loss |
| Fill bucket | Essential for coloring |
| Rectangle selection | Basic editing |
| Lasso selection | Free-form editing |
| Transform tool | Move, scale, rotate layers/selections |
| Layer duplicate | Basic layer workflow |
| Merge down / merge visible | Basic layer workflow |
| Fit to screen / 100% view | Basic navigation |
| Brush preset duplicate / rename | Brush workflow |
| Shortcut editor | Artist customization |

---

## Priority 1 — SAI-Like Core

These features make ARTY feel closer to PaintTool SAI.

| Feature | Purpose |
|---|---|
| Quick Bar | Fast access to common operations |
| Dynamic Tool Options | UI changes depending on selected tool |
| Brush preset box | SAI-like brush workflow |
| Per-brush stabilizer | Different stabilization per tool |
| Lock Alpha / Clipping UX polish | Anime/manga workflow |
| Color history | Fast color reuse |
| Palette import/export | Save color sets |
| Layer thumbnails | Better layer navigation |
| Minimal UI mode | Distraction-free drawing |
| Navigator viewport rectangle | Better navigation |
| Brush cursor preview | Accurate drawing feedback |

---

## Priority 2 — Professional Usability

| Feature | Purpose |
|---|---|
| Layer masks | Non-destructive editing |
| Magic wand | Selection by color |
| Selection feather/grow/shrink | Better selection control |
| Reference image panel | Artist workflow |
| OpenRaster `.ora` export/import | Interoperability |
| Tablet diagnostics | Troubleshoot pressure problems |
| Performance HUD | Debug frame and tile performance |
| Symmetry ruler | Useful drawing aid |
| Workspace presets | UI customization |
| Command palette | Fast command access |

---

# 4. Recommended Final UI Structure

The ideal ARTY interface should be:

```text
+--------------------------------------------------------------------------------+
| Menu Bar: File | Edit | Canvas | Layer | Selection | View | Window | Help       |
+--------------------------------------------------------------------------------+
| Quick Bar: Undo Redo | Save | Select | Transform | Zoom | Rotate | Mirror      |
+----------------------+-------------------------------------+-------------------+
| Left Sidebar         | Central Canvas                      | Right Sidebar     |
| Tools                | WGPU painting viewport              | Navigator         |
| Brush Presets        | Canvas overlays                     | Color Selector    |
| Dynamic Tool Options | Selection / transform handles       | Palette           |
| Brush Settings       | Brush cursor                        | Layers            |
| Stabilizer           |                                     | Reference Images  |
+----------------------+-------------------------------------+-------------------+
| Status Bar: tool | brush size | pressure | zoom | rotation | layer | autosave   |
+--------------------------------------------------------------------------------+
```

This keeps the interface familiar to SAI/Krita/CSP users while staying simple.

---

# 5. Menu Bar Specification

The menu bar should contain:

```text
File | Edit | Canvas | Layer | Selection | View | Window | Help
```

---

## 5.1 File Menu

```text
File
├─ New...                          Ctrl+N
├─ Open...                         Ctrl+O
├─ Open Recent
│  ├─ recent_file_1.arty
│  ├─ recent_file_2.arty
│  └─ Clear Recent List
├─ Save                            Ctrl+S
├─ Save As...                      Ctrl+Shift+S
├─ Autosave Now
├─ Export
│  ├─ PNG...
│  ├─ JPG...
│  ├─ OpenRaster .ora...
│  └─ Flattened Image...
├─ Import
│  ├─ Image as New Layer...
│  ├─ Reference Image...
│  └─ Brush Preset...
├─ Document Info...
├─ Preferences...
└─ Exit
```

### Required Supporting Systems

```rust
pub enum FileCommand {
    NewDocument,
    Open,
    OpenRecent(PathBuf),
    Save,
    SaveAs,
    AutosaveNow,
    ExportPng,
    ExportJpeg,
    ExportOra,
    ImportImageAsLayer,
    ImportReferenceImage,
    ImportBrushPreset,
    DocumentInfo,
    Preferences,
    Exit,
}
```

---

## 5.2 Edit Menu

```text
Edit
├─ Undo                            Ctrl+Z
├─ Redo                            Ctrl+Y / Ctrl+Shift+Z
├─ Cut                             Ctrl+X
├─ Copy                            Ctrl+C
├─ Copy Merged                     Ctrl+Shift+C
├─ Paste                           Ctrl+V
├─ Paste as New Layer              Ctrl+Shift+V
├─ Clear                           Delete
├─ Fill                            Alt+Backspace
├─ Stroke Selection...
├─ Transform                       Ctrl+T
├─ Free Transform                  Ctrl+Shift+T
├─ Preferences...
└─ Keyboard Shortcuts...
```

### Required Systems

- Clipboard image support.
- Selection-aware copy/cut/paste.
- Command history.
- Transform state.
- Shortcut editor.

---

## 5.3 Canvas Menu

```text
Canvas
├─ Resize Canvas...
├─ Resize Image...
├─ Crop to Selection
├─ Trim Transparent Pixels
├─ Rotate Canvas View Left
├─ Rotate Canvas View Right
├─ Reset Rotation
├─ Flip View Horizontal            H
├─ Flip Canvas Horizontal
├─ Flip Canvas Vertical
├─ Zoom In                         Ctrl++
├─ Zoom Out                        Ctrl+-
├─ Fit to Screen                   Ctrl+0
├─ Actual Size                     Ctrl+1
└─ Reset View
```

Important distinction:

| Command | Meaning |
|---|---|
| Flip View Horizontal | Only changes the view. Pixel data is not modified. |
| Flip Canvas Horizontal | Actually modifies image pixels. |
| Rotate Canvas View | Only rotates viewport. |
| Rotate Image | Actually modifies image pixels. |

This distinction is very important for painting programs.

---

## 5.4 Layer Menu

```text
Layer
├─ New Raster Layer                Ctrl+Shift+N
├─ New Vector Layer                optional / later
├─ New Folder
├─ Duplicate Layer
├─ Delete Layer
├─ Rename Layer
├─ Merge Down                      Ctrl+E
├─ Merge Visible                   Ctrl+Shift+E
├─ Flatten Image
├─ Clear Layer
├─ Fill Layer
├─ Layer Properties...
├─ Add Layer Mask
├─ Apply Layer Mask
├─ Delete Layer Mask
├─ Enable Layer Mask
├─ Invert Layer Mask
├─ Lock Alpha
├─ Clipping Group
├─ Convert to Raster Layer
└─ Transform Layer
```

### Suggested Command Enum

```rust
pub enum LayerCommand {
    NewRaster,
    NewVector,
    NewFolder,
    Duplicate,
    Delete,
    Rename,
    MergeDown,
    MergeVisible,
    Flatten,
    Clear,
    Fill,
    Properties,
    AddMask,
    ApplyMask,
    DeleteMask,
    ToggleMask,
    InvertMask,
    ToggleLockAlpha,
    ToggleClipping,
    ConvertToRaster,
    Transform,
}
```

---

## 5.5 Selection Menu

```text
Selection
├─ Select All                      Ctrl+A
├─ Deselect                        Ctrl+D
├─ Reselect                        Ctrl+Shift+D
├─ Invert Selection                Ctrl+I
├─ Grow...
├─ Shrink...
├─ Feather...
├─ Smooth
├─ Border...
├─ Transform Selection
├─ Save Selection as Mask
├─ Load Selection from Layer Alpha
└─ Selection Display
   ├─ Marching Ants
   ├─ Mask Overlay
   └─ Hidden
```

---

## 5.6 View Menu

```text
View
├─ Show Navigator
├─ Show Color Panel
├─ Show Layers
├─ Show Brush Presets
├─ Show Tool Options
├─ Show Reference Images
├─ Show Status Bar
├─ Show Grid
├─ Snap to Grid
├─ Show Rulers
├─ Show Symmetry Axis
├─ Fullscreen                      F11
├─ Minimal UI                      Tab
└─ Reset Workspace
```

---

## 5.7 Window Menu

```text
Window
├─ Workspace
│  ├─ Default
│  ├─ Compact
│  ├─ Painting
│  ├─ Inking
│  └─ Save Current Workspace...
├─ UI Scale
│  ├─ 80%
│  ├─ 100%
│  ├─ 125%
│  └─ 150%
├─ Theme
│  ├─ Light
│  ├─ Gray
│  └─ Dark
└─ Panels...
```

---

## 5.8 Help Menu

```text
Help
├─ Quick Start
├─ Keyboard Shortcuts
├─ Tablet Diagnostics
├─ Performance HUD
├─ About ARTY / Xcalux
└─ Open Config Folder
```

---

# 6. Quick Bar Specification

The current top bar is too minimal. It should become a real quick-access bar similar to PaintTool SAI’s quick bar.

## Recommended Quick Bar

```text
[Undo] [Redo] [Save] |
[Cut] [Copy] [Paste] |
[Select All] [Deselect] [Invert] |
[Transform] |
[Fit] [100%] [Reset View] |
Zoom: [-] [75% ▼] [+] |
Rotate: [-15°] [0°] [+15°] |
[Mirror H] |
Stabilizer: [Off ▼] Mode: [Spring ▼] |
Autosave: OK
```

## Quick Bar Buttons

| Button | Shortcut | Function |
|---|---:|---|
| Undo | Ctrl+Z | Undo command |
| Redo | Ctrl+Y | Redo command |
| Save | Ctrl+S | Save document |
| Cut | Ctrl+X | Cut selection |
| Copy | Ctrl+C | Copy selection |
| Paste | Ctrl+V | Paste as new layer |
| Select All | Ctrl+A | Select whole canvas |
| Deselect | Ctrl+D | Clear selection |
| Invert Selection | Ctrl+I | Invert selection |
| Transform | Ctrl+T | Transform layer/selection |
| Fit | Ctrl+0 | Fit canvas to viewport |
| 100% | Ctrl+1 | Actual size |
| Reset View | — | Reset pan/zoom/rotation |
| Zoom dropdown | — | 25%, 50%, 75%, 100%, 200% |
| Rotate -15° | — | Rotate view left |
| Rotate 0° | 0 | Reset rotation |
| Rotate +15° | — | Rotate view right |
| Mirror H | H | Flip view horizontally |
| Stabilizer | — | Off, 1–10, S-1 to S-5 |
| Autosave indicator | — | Shows save/recovery state |

---

# 7. Left Sidebar Specification

The left sidebar should contain:

```text
TOOLS
BRUSH PRESETS
TOOL OPTIONS
BRUSH SETTINGS
STABILIZER
```

The most important UI improvement is:

> **Tool Options should change depending on the selected tool.**

Do not always show brush settings if the user has selected Fill, Lasso, Transform, or Magic Wand.

---

# 8. Tools Panel

## Recommended Tools

```text
TOOLS
[Brush]       [Eraser]
[Fill]        [Gradient]
[Rect Select] [Lasso]
[Wand]        [Move]
[Transform]   [Color Picker]
[Hand]        [Zoom]
[Rotate]      [Line]
[Shape]       [Reference]
```

## Tool List

| Tool | Priority | Shortcut | Purpose |
|---|---:|---:|---|
| Brush | Required | B | Normal painting |
| Eraser | Required | E | Erase with brush engine |
| Fill Bucket | Required | G | Fill areas |
| Gradient | Recommended | Shift+G | Linear/radial fills |
| Rect Select | Required | M | Rectangular selection |
| Ellipse Select | Recommended | Shift+M | Elliptical selection |
| Lasso | Required | L | Freehand selection |
| Polygon Lasso | Recommended | Shift+L | Polygonal selection |
| Magic Wand | Required | W | Select by color |
| Move | Required | V | Move layer/selection |
| Transform | Required | Ctrl+T | Scale/rotate/free transform |
| Color Picker | Required | I / Alt | Pick color |
| Hand/Pan | Required | Space | Pan canvas |
| Zoom | Required | Z | Zoom canvas |
| Rotate View | Required | R | Rotate view |
| Line Tool | Recommended | U | Draw straight lines |
| Shape Tool | Optional | U | Rectangle/ellipse shapes |
| Reference Tool | Optional | — | Move reference image |
| Text Tool | Later | T | Text layers |

---

# 9. Dynamic Tool Options

## 9.1 Brush / Eraser Tool Options

```text
TOOL OPTIONS: Brush
Preset: [Pencil ▼]
Size:      [slider] 2.7 px
Opacity:   [slider] 95%
Hardness:  [slider] 95%
Min Size:  [slider] 80%
Blending:  [slider] 0%
Dilution:  [slider] 0%
Texture:   [None ▼]
Bristle:   [slider] 0%
[ ] Eraser Mode
[ ] Lock Alpha Aware
[ ] Use Selection Mask
[ ] Stabilizer per brush

[Reset Brush] [Duplicate] [Save Preset]
```

---

## 9.2 Fill Bucket Tool Options

```text
TOOL OPTIONS: Fill
Target: [Current Layer ▼]
Reference: [Current Layer ▼ / All Visible / Reference Layer]
Tolerance: [slider] 32
Expand: [slider] 1 px
Close Gap: [slider] 0
[✓] Anti-alias
[✓] Respect Selection
[ ] Fill Transparent Only
[ ] Treat Alpha as Boundary
```

Suggested structure:

```rust
pub struct FillToolOptions {
    pub target: FillTarget,
    pub reference: FillReference,
    pub tolerance: u8,
    pub expand_px: u8,
    pub close_gap: u8,
    pub anti_alias: bool,
    pub respect_selection: bool,
    pub fill_transparent_only: bool,
    pub alpha_boundary: bool,
}
```

---

## 9.3 Selection Tool Options

```text
TOOL OPTIONS: Selection
Mode: [Replace] [Add] [Subtract] [Intersect]
Shape: [Rect] [Ellipse]
Feather: [slider] 0 px
[✓] Anti-alias
[ ] Snap to Canvas Bounds

[Select All] [Deselect] [Invert]
[Grow] [Shrink] [Feather]
```

---

## 9.4 Lasso Tool Options

```text
TOOL OPTIONS: Lasso
Mode: [Replace] [Add] [Subtract] [Intersect]
Type: [Freehand ▼ / Polygon]
Smoothing: [slider] 2
Feather: [slider] 0 px
[✓] Anti-alias
```

---

## 9.5 Magic Wand Tool Options

```text
TOOL OPTIONS: Magic Wand
Mode: [Replace] [Add] [Subtract] [Intersect]
Reference: [Current Layer / All Visible]
Tolerance: [slider] 32
[✓] Contiguous
[✓] Anti-alias
[ ] Sample Transparent
[ ] Close Gap
```

---

## 9.6 Transform Tool Options

```text
TOOL OPTIONS: Transform
Target: [Selection ▼ / Active Layer / Layer Group]
Mode: [Scale] [Rotate] [Free]
Interpolation: [Nearest / Bilinear / Bicubic]
Anchor: [Center ▼]

X: [input]  Y: [input]
W: [input]  H: [input]
Angle: [input]

[Apply] [Cancel] [Reset]
```

---

## 9.7 Color Picker Tool Options

```text
TOOL OPTIONS: Color Picker
Sample: [Current Layer / All Visible / Reference]
Radius: [1 px / 3 px / 5 px / 11 px]
[ ] Pick Alpha
[ ] Add to Color History
```

---

## 9.8 Move Tool Options

```text
TOOL OPTIONS: Move
Target: [Selection / Active Layer / Pick Layer]
[ ] Auto Select Layer
[ ] Move Group
[ ] Snap to Grid
```

---

# 10. Brush Preset System

The existing dynamic `Vec<BrushPreset>` is a good direction. It should become a full preset manager.

## Current Core Structure

```rust
pub struct BrushPreset {
    pub id: u64,
    pub name: String,
    pub icon: PresetIcon,
    pub radius_log: f32,
    pub opacity: f32,
    pub hardness: f32,
    pub min_size_fraction: f32,
    pub color_blending: f32,
    pub dilution: f32,
    pub texture_id: u8,
    pub texture_scale: f32,
    pub bristle_id: u8,
    pub is_eraser: bool,
}
```

## Recommended Extended Structure

```rust
pub struct BrushPreset {
    pub id: u64,
    pub name: String,
    pub icon: PresetIcon,

    pub radius_log: f32,
    pub opacity: f32,
    pub hardness: f32,
    pub min_size_fraction: f32,
    pub color_blending: f32,
    pub dilution: f32,

    pub texture_id: u8,
    pub texture_scale: f32,
    pub bristle_id: u8,
    pub is_eraser: bool,

    pub stabilizer_level: u8,
    pub stabilizer_mode: StabilizerMode,
    pub shortcut: Option<KeyBinding>,
    pub favorite: bool,
}
```

## Brush Preset Panel UI

```text
BRUSH PRESETS
[ Pencil   ] [ Ink Pen  ]
[ Brush    ] [ Water    ]
[ Smudge   ] [ Eraser   ]

[+ Add] [Duplicate] [Menu ⋯]
```

## Preset Context Menu

```text
Preset Context Menu
├─ Select
├─ Rename
├─ Duplicate
├─ Delete
├─ Export...
├─ Set Shortcut...
├─ Use as Eraser
├─ Reset to Default
└─ Show in Brush Folder
```

## Required Preset Features

| Feature | Priority |
|---|---:|
| Duplicate preset | Required |
| Rename preset | Required |
| Delete preset | Required |
| Import `.artybrush` | Required |
| Export `.artybrush` | Required |
| Favorite preset | Recommended |
| Per-brush stabilizer | Recommended |
| Per-brush shortcut | Recommended |
| Reset default presets | Recommended |

---

# 11. Brush Settings Panel

The brush panel should be compact and readable.

```text
BRUSH SETTINGS
Preview:   ○

Size:      2.7 px    [slider]
Opacity:   95%       [slider]
Hardness:  95%       [slider]
Min Size:  80%       [slider]
Density:   100%      [slider]
Blending:  0%        [slider]
Dilution:  0%        [slider]

Texture:   [None ▼]
Scale:     100%      [slider]
Bristle:   0%        [slider]

[ ] Eraser Mode
[ ] Lock Alpha Aware
[ ] Save changes to preset

[Test Stroke Preview]
```

## Pressure Curve Preview

A small pressure preview would be very useful:

```text
thin ─────── thick
light ────── opaque
```

This helps the user understand how size and opacity react to pen pressure.

---

# 12. Stabilizer UI

The stabilizer should be visible in the Quick Bar and configurable in the left panel.

## Quick Bar

```text
Stabilizer: [Off ▼]  Mode: [Spring Physics ▼]
```

## Detailed Panel

```text
STABILIZER
Level: [Off ▼]
Mode: [EMA / Spring Physics]
[ ] Per Brush
Strength: [slider]
Delay: [slider]
Pressure Smoothing: [slider]
```

## Levels

```text
Off
1 2 3 4 5 6 7 8 9 10
S-1 S-2 S-3 S-4 S-5
```

Suggested behavior:

| Level | Mode |
|---|---|
| 1–10 | EMA |
| S-1 to S-5 | Spring-Mass-Damper |

---

# 13. Center Canvas UI

The canvas area should stay visually clean but include important overlays.

## Canvas Overlays

| Overlay | Purpose |
|---|---|
| Brush cursor | Shows actual brush radius |
| Crosshair optional | Helps precision |
| Canvas bounds | Shows actual document boundary |
| Selection marching ants | Shows active selection |
| Selection mask overlay | Alternative selection display |
| Transform bounding box | Shows handles for transform |
| Symmetry axis | For symmetry drawing |
| Ruler/guide lines | For assisted drawing |
| Temporary HUD | Shows zoom/rotation during interaction |
| Pan/rotate indicator | Helps when rotating or panning |

---

## Canvas Context Menu

Right-click on canvas:

```text
Canvas Context Menu
├─ Pick Layer Here
├─ Clear Selection
├─ Transform
├─ Flip View Horizontal
├─ Reset View
├─ Fit to Screen
├─ Add Reference Image
├─ Paste
└─ Canvas Properties
```

If selection is active:

```text
Selection Context Menu
├─ Cut
├─ Copy
├─ Paste
├─ Clear
├─ Fill
├─ Transform Selection
├─ Invert Selection
├─ Feather...
└─ Deselect
```

---

## Canvas Input Controls

| Input | Action |
|---|---|
| Left drag | Use active tool |
| Right drag | Pan or context menu depending on preference |
| Middle drag | Pan |
| Space + drag | Pan |
| R + drag | Rotate view |
| Ctrl + Space + drag | Zoom |
| Alt + click | Color picker |
| Shift + brush drag | Straight line stroke |
| Ctrl + click | Pick layer |
| Mouse wheel | Zoom |
| Shift + wheel | Brush size |
| Ctrl + wheel | Zoom |
| Alt + wheel | Rotate |

---

# 14. Right Sidebar Specification

The right sidebar should contain:

```text
NAVIGATOR
COLOR
PALETTE
LAYERS
REFERENCE
```

Optional later:

```text
HISTORY
BRUSH HISTORY
PERFORMANCE
```

---

# 15. Navigator Panel

Current navigator should be improved with a viewport rectangle and controls.

```text
NAVIGATOR
+----------------+
| thumbnail      |
| viewport rect  |
+----------------+

Zoom: 75% [slider]
[Fit] [100%] [Reset]

Rotate: 0°
[-15°] [0°] [+15°]

[Mirror H]
```

## Required Navigator Features

| Feature | Purpose |
|---|---|
| Thumbnail preview | Shows whole canvas |
| Viewport rectangle | Shows current visible region |
| Drag viewport rectangle | Pan canvas |
| Zoom slider | Fast zoom control |
| Fit / 100% / Reset | Common navigation |
| Rotate buttons | Quick view rotation |
| Mirror toggle | Composition check |

---

# 16. Color Selector Panel

The current color wheel is good. It should be expanded slightly.

```text
COLOR
[Wheel] [HSV] [RGB] [History]

Foreground: [■■] #191919
Background: [□□] #FFFFFF

[Swap] [Default B/W]

HSV:
H [slider]
S [slider]
V [slider]

RGB:
R [slider]
G [slider]
B [slider]
A [slider]

Hex: [#191919]
```

## Color Features

| Feature | Priority |
|---|---:|
| HSV wheel | Existing |
| RGB sliders | Required |
| HSV sliders | Required |
| Hex input | Required |
| Foreground/background color | Required |
| Swap FG/BG | Required |
| Default black/white | Recommended |
| Color history | Required |
| Add to palette | Required |

---

# 17. Palette Panel

```text
PALETTE
Palette: [Default ▼]

[+ Color] [-] [Save] [Import] [Export]

[swatch grid...]
```

## Swatch Context Menu

```text
Swatch Context Menu
├─ Use Color
├─ Replace with Current Color
├─ Rename
├─ Delete
├─ Move Left
└─ Move Right
```

---

# 18. Layers Manager

The layer panel is one of the most important UI areas.

## Recommended Layer Panel

```text
LAYERS

Blend: [Normal ▼]
Opacity: [100% slider]

[✓] Lock Alpha
[ ] Clipping
[ ] Preserve Transparency

[+ Raster] [+ Folder] [+ Mask]
[Duplicate] [Merge] [Delete]

Layer List:
☰  👁  [thumb] [mask]  Layer 3        100% Normal
☰  👁  [thumb]         Layer 2        65% Multiply
☰  👁  [folder]        Folder 1
☰  👁  [thumb]         Layer 1
```

## Layer Buttons

| Button | Priority | Purpose |
|---|---:|---|
| New Raster | Required | Create paint layer |
| New Folder | Required | Group layers |
| New Vector | Later | Linework layer |
| Delete | Required | Delete layer |
| Duplicate | Required | Duplicate selected layer |
| Merge Down | Required | Merge into layer below |
| Merge Visible | Recommended | Merge visible layers |
| Add Mask | Recommended | Add non-destructive mask |
| Apply Mask | Recommended | Bake mask into alpha |
| Lock Alpha | Existing | Paint only existing opacity |
| Clipping Group | Existing | Clip to lower layer |
| Move Up/Down | Required | Reorder layers |
| Folder expand/collapse | Required | Folder UX |
| Layer thumbnail | Required | Visual recognition |
| Visibility eye | Required | Show/hide layer |
| Solo layer | Recommended | Alt-click eye |
| Blend mode dropdown | Existing | Layer blend mode |
| Opacity slider | Existing | Layer opacity |

---

## Layer Context Menu

```text
Layer Context Menu
├─ Rename
├─ Duplicate
├─ Delete
├─ Merge Down
├─ Merge Visible
├─ New Layer Above
├─ New Layer Below
├─ Add Layer Mask
├─ Apply Layer Mask
├─ Delete Layer Mask
├─ Lock Alpha
├─ Clipping Group
├─ Convert to Raster
├─ Select Opaque Pixels
├─ Layer Properties
└─ Export Layer as PNG
```

---

# 19. Blend Modes

Current blend modes:

| ID | Mode |
|---:|---|
| 0 | Normal |
| 1 | Multiply |
| 2 | Screen |
| 3 | Overlay |
| 4 | Luminosity / Shine |
| 5 | Shade |
| 6 | Paper Canvas |

Recommended additional modes:

| Mode | Purpose |
|---|---|
| Add / Linear Dodge | Light effects |
| Color Dodge | Highlights |
| Color Burn | Strong shadows |
| Soft Light | Soft shading |
| Hard Light | Contrast effects |
| Difference | Alignment/debug |
| Erase | Non-destructive erase mode |
| Pass Through | Folder compositing |

Recommended enum:

```rust
pub enum BlendMode {
    Normal = 0,
    Multiply = 1,
    Screen = 2,
    Overlay = 3,
    Luminosity = 4,
    Shade = 5,
    Paper = 6,
    Add = 7,
    ColorDodge = 8,
    ColorBurn = 9,
    SoftLight = 10,
    HardLight = 11,
    Difference = 12,
}
```

Use W3C Compositing and Blending formulas for standard modes.

---

# 20. Reference Image Panel

Reference images are extremely useful for artists and should be added.

```text
REFERENCE
[+ Add Image] [Hide All]

List:
[eye] ref_01.png  opacity 80%
[eye] ref_02.png  opacity 100%

Selected Reference:
Opacity [slider]
Scale [slider]
Rotation [slider]

[Pin to View] [Pin to Canvas]
[Remove]
```

## Data Model

```rust
pub struct ReferenceImage {
    pub id: u64,
    pub name: String,
    pub visible: bool,
    pub opacity: f32,
    pub pinned_to_view: bool,
    pub world_pos: Vec2,
    pub scale: f32,
    pub rotation: f32,
}
```

---

# 21. Bottom Status Bar

The status bar should show live state.

```text
Tool: Pencil | Size: 2.7 px | Opacity: 95% | Pressure: 0.42 |
Canvas: 1024×1024 | Zoom: 75% | Rotation: 0° | Mirror: Off |
X: 512 Y: 320 | Layer: Layer 1 | Autosaved 00:32 ago
```

## Status Items

| Item | Purpose |
|---|---|
| Active tool | Prevent confusion |
| Brush size | Quick feedback |
| Brush opacity | Quick feedback |
| Pressure raw/smooth | Tablet debugging |
| Canvas size | Document info |
| Zoom | Navigation |
| Rotation | Navigation |
| Mirror state | Prevent accidental mirrored drawing |
| Cursor coordinate | Precision |
| Active layer | Prevent painting on wrong layer |
| Autosave state | Reliability |
| Dirty state | Shows unsaved changes |

---

# 22. Dialogs and Popups

## 22.1 New Document Dialog

```text
New Document

Name: [Untitled]

Width:  [1024] px
Height: [1024] px
DPI:    [300]

Background:
  ( ) Transparent
  ( ) White
  ( ) Custom Color

Presets:
  [1024 Square]
  [A4 300dpi]
  [HD 1920x1080]

[Create] [Cancel]
```

---

## 22.2 Preferences Dialog

```text
Preferences
├─ General
├─ Interface
├─ Canvas
├─ Tablet
├─ Performance
├─ Shortcuts
├─ Autosave
├─ Files
└─ Advanced
```

### General

```text
Startup:
[ ] Open last document
[ ] Check autosave recovery

Default document:
Width / Height / DPI
```

### Interface

```text
Theme: Light / Gray / Dark
UI Scale: 100%
Panel Width
Icon Size
[ ] Show tooltips
[ ] Compact sliders
```

### Tablet

```text
Input Source:
( ) Winit
( ) Windows Ink
( ) RealTimeStylus

Pressure Curve: [graph]
Min Pressure: [slider]
Max Pressure: [slider]

[Calibrate Pressure]
[Test Area]
```

### Performance

```text
GPU Backend: Auto / DX12 / Vulkan / Metal
Tile Cache Slots: 4096
Undo Memory Limit: [slider]
[ ] Use low latency mode
[ ] Limit navigator update rate
```

### Autosave

```text
[✓] Enable Autosave
Interval: [3 min]
Keep versions: [5]
Autosave folder: [...]

[Open Autosave Folder]
```

### Shortcuts

```text
Search: [brush]

Command              Shortcut
Brush Tool           B
Eraser               E
Undo                 Ctrl+Z

[Reset Defaults] [Export] [Import]
```

---

## 22.3 Brush Editor Popup

```text
Brush Editor: Pencil

Basic:
  Size
  Opacity
  Hardness
  Min Size
  Stabilizer

Pressure:
  Size curve
  Opacity curve
  Hardness curve

Texture:
  Texture image
  Scale
  Strength

Advanced:
  Spacing
  Jitter
  Angle
  Tilt response
  Smudge
  Dilution

[Save] [Save As New] [Cancel]
```

---

## 22.4 Transform Overlay Dialog

```text
Transform

X: 120  Y: 80
W: 400  H: 300
Angle: 15°

[Apply] [Cancel] [Reset]
```

Input behavior:

| Key | Action |
|---|---|
| Enter | Apply transform |
| Esc | Cancel transform |

---

## 22.5 Export PNG Dialog

```text
Export PNG

File: [...]

Area:
  ( ) Canvas Bounds
  ( ) Artwork Bounds
  ( ) Selection

Background:
  ( ) Transparent
  ( ) White

Scale: 100%

[Export] [Cancel]
```

---

# 23. Command Palette

To keep the UI clean while still supporting many features, add a command palette.

Shortcut:

```text
Ctrl+P
or
Ctrl+Shift+P
```

UI:

```text
Command Palette

[ search command... ]

Brush Tool                 B
Toggle Mirror View         H
Export PNG...
Add Layer Mask
Open Tablet Diagnostics
```

Data model:

```rust
pub struct CommandInfo {
    pub id: CommandId,
    pub name: &'static str,
    pub category: CommandCategory,
    pub shortcut: Option<KeyBinding>,
}
```

---

# 24. Keyboard Shortcuts

## Core

| Shortcut | Action |
|---|---|
| Ctrl+N | New |
| Ctrl+O | Open |
| Ctrl+S | Save |
| Ctrl+Shift+S | Save As |
| Ctrl+Z | Undo |
| Ctrl+Y | Redo |
| Ctrl+Shift+Z | Redo alternative |
| Ctrl+X | Cut |
| Ctrl+C | Copy |
| Ctrl+Shift+C | Copy merged |
| Ctrl+V | Paste |
| Delete | Clear |

---

## Tools

| Shortcut | Tool |
|---|---|
| B | Brush |
| E | Eraser |
| G | Fill |
| M | Rect Select |
| L | Lasso |
| W | Magic Wand |
| V | Move |
| Ctrl+T | Transform |
| I | Color Picker |
| H | Flip View Horizontal |
| R + drag | Rotate View |
| Space + drag | Pan |
| Z | Zoom |
| `[` | Brush size down |
| `]` | Brush size up |
| Shift + `[` | Opacity down |
| Shift + `]` | Opacity up |

---

## Selection

| Shortcut | Action |
|---|---|
| Ctrl+A | Select All |
| Ctrl+D | Deselect |
| Ctrl+I | Invert Selection |
| Ctrl+Shift+D | Reselect |

---

## View

| Shortcut | Action |
|---|---|
| Ctrl+0 | Fit to Screen |
| Ctrl+1 | 100% |
| - | Rotate Left |
| = | Rotate Right |
| 0 | Reset Rotation |
| Tab | Minimal UI |
| F11 | Fullscreen |

---

## Layers

| Shortcut | Action |
|---|---|
| Ctrl+Shift+N | New Layer |
| Ctrl+E | Merge Down |
| Ctrl+Shift+E | Merge Visible |
| Alt+] | Select layer above |
| Alt+[ | Select layer below |

---

# 25. Selection System

The existing selection mask concept should become a complete subsystem.

## Supported Tools

1. Rectangle Select
2. Ellipse Select
3. Lasso Select
4. Polygon Lasso
5. Magic Wand
6. Select All
7. Deselect
8. Invert Selection
9. Grow
10. Shrink
11. Feather
12. Transform Selection

---

## Data Model

```rust
pub struct Selection {
    pub tiles: HashMap<TileCoord, SelectionTile>,
    pub bounds: Option<IntRect>,
    pub feather_radius: f32,
    pub antialias: bool,
    pub dirty_tiles: Vec<TileCoord>,
}

pub struct SelectionTile {
    pub mask: [u8; TILE_SIZE * TILE_SIZE],
}
```

## Selection Operations

```rust
pub enum SelectionOp {
    Replace,
    Add,
    Subtract,
    Intersect,
}
```

## Brush + Selection

When painting with an active selection:

```text
final_alpha = brush_alpha * selection_mask_alpha
```

If no selection exists, selection mask alpha is treated as `1.0`.

---

# 26. Magic Wand System

## Options

```rust
pub struct MagicWandOptions {
    pub tolerance: u8,
    pub contiguous: bool,
    pub sample_all_layers: bool,
    pub antialias: bool,
}
```

## Algorithm

1. Sample the clicked pixel color.
2. Use tile-aware flood fill.
3. Compare color distance.
4. Write result into selection mask tiles.
5. Mark selection dirty.

Example color distance:

```rust
fn color_distance(a: [u8; 4], b: [u8; 4]) -> u32 {
    let dr = a[0] as i32 - b[0] as i32;
    let dg = a[1] as i32 - b[1] as i32;
    let db = a[2] as i32 - b[2] as i32;
    let da = a[3] as i32 - b[3] as i32;
    (dr * dr + dg * dg + db * db + da * da) as u32
}
```

---

# 27. Fill Bucket System

The Fill Bucket is essential for anime/manga coloring.

## Required Features

| Feature | Purpose |
|---|---|
| Fill current layer | Basic fill |
| Fill under line art | Coloring workflow |
| Tolerance | Control color similarity |
| Expand fill | Prevent gaps |
| Close gap | Fill imperfect line art |
| Anti-alias | Smooth edges |
| Respect selection | Controlled editing |
| Sample current/all visible layers | Flexible fill reference |

## Options

```rust
pub struct FillOptions {
    pub tolerance: u8,
    pub expand_px: u8,
    pub close_gap: u8,
    pub antialias: bool,
    pub sample_all_layers: bool,
    pub respect_selection: bool,
}
```

## Recommended Algorithm

Use scanline flood fill instead of naive pixel BFS.

```rust
pub fn flood_fill(
    canvas: &mut Canvas,
    layer_id: LayerId,
    start: IVec2,
    color: PremulColor,
    options: FillOptions,
) -> DirtyTileSet {
    // 1. sample target color
    // 2. scanline fill spans
    // 3. write affected pixels
    // 4. mark dirty tiles
    // 5. return changed tile set
}
```

---

# 28. Transform System

Transform should be split into two phases:

1. **GPU preview**
2. **CPU commit to tiles**

This keeps interaction smooth.

## Transform Targets

```rust
pub enum TransformTarget {
    ActiveLayer,
    Selection,
    LayerGroup,
}
```

## Transform State

```rust
pub struct TransformState {
    pub active: bool,
    pub target: TransformTarget,
    pub original_bounds: IntRect,
    pub matrix: [[f32; 3]; 3],
    pub preview_texture_id: Option<u64>,
    pub interpolation: InterpolationMode,
}

pub enum InterpolationMode {
    Nearest,
    Bilinear,
    Bicubic,
}
```

## Workflow

### Start Transform

1. Snapshot source pixels.
2. Hide or ghost original pixels.
3. Create preview state.

### During Drag

1. Do not modify tile pixels.
2. Update transform matrix.
3. Render preview through WGPU.

### Apply

1. Rasterize transformed result into canvas tiles.
2. Mark affected tiles dirty.
3. Push transform command to history.

### Cancel

1. Restore original state.
2. Discard preview.

---

# 29. Layer Mask System

Layer masks should be added for non-destructive editing.

## Data Model

```rust
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    pub visible: bool,
    pub opacity: f32,
    pub blend_mode: BlendMode,
    pub tiles: HashMap<TileCoord, Tile>,

    pub alpha_locked: bool,
    pub clipping: bool,

    pub mask: Option<LayerMask>,
}

pub struct LayerMask {
    pub enabled: bool,
    pub tiles: HashMap<TileCoord, MaskTile>,
}

pub struct MaskTile {
    pub data: [u8; TILE_SIZE * TILE_SIZE],
}
```

## Compositing Formula

```text
src_alpha = src_alpha * layer_opacity * mask_alpha
```

## UI Actions

```text
Add Mask
Delete Mask
Enable/Disable Mask
Apply Mask
Invert Mask
```

---

# 30. Command History System

Current tile snapshot history is good for strokes. However, non-brush actions need command history.

## Recommended Enum-Based Command System

```rust
pub enum HistoryCommand {
    Stroke(TileStrokeCommand),
    CreateLayer(CreateLayerCommand),
    DeleteLayer(DeleteLayerCommand),
    MoveLayer(MoveLayerCommand),
    RenameLayer(RenameLayerCommand),
    Transform(TransformCommand),
    Fill(FillCommand),
    Selection(SelectionCommand),
}
```

## Why Enum Instead of Trait Objects?

- Easier to control allocation.
- More predictable memory behavior.
- Better for a performance-sensitive painting app.

---

# 31. Autosave and Crash Recovery

Autosave is essential.

## Behavior

- Autosave every 2–5 minutes.
- Only autosave if document is dirty.
- Save asynchronously.
- Keep multiple versions.
- Detect recovery files on startup.

## Folder Example

```text
ARTY/autosave/
  project_name.autosave.arty
  project_name.autosave.meta
```

## Save Request Enum

```rust
pub enum SaveRequest {
    SaveAs(PathBuf, DocumentSnapshot),
    Autosave(PathBuf, DocumentSnapshot),
    ExportPng(PathBuf, ExportOptions),
}
```

## Recovery Dialog

```text
Recovered autosave found:

project_name.autosave.arty
Last autosave: 2 minutes ago

[Open Recovery] [Discard] [Show Folder]
```

---

# 32. File Export and Import

## Required Formats

| Format | Priority | Purpose |
|---|---:|---|
| `.arty` | Existing | Native format |
| PNG | Required | Standard export |
| JPG | Recommended | Flattened export |
| ORA | Recommended | Layered exchange format |
| PSD | Later | Complex compatibility |

---

## OpenRaster `.ora`

OpenRaster is useful for interoperability with Krita, GIMP, and MyPaint-like workflows.

Typical `.ora` layout:

```text
example.ora
├─ mimetype
├─ stack.xml
├─ data/
│  ├─ layer0.png
│  ├─ layer1.png
├─ Thumbnails/
│  └─ thumbnail.png
└─ mergedimage.png
```

## Export Pipeline

```rust
pub fn export_ora(doc: &Document, path: &Path) -> Result<()> {
    // 1. flatten sparse tiles per layer to bounded PNG
    // 2. write stack.xml
    // 3. write layer pngs
    // 4. write thumbnail
    // 5. zip package
}
```

---

# 33. Document Bounds

Even though the internal canvas is sparse/infinite, the document needs bounds for export and UI.

```rust
pub struct Document {
    pub width: u32,
    pub height: u32,
    pub dpi: f32,
    pub canvas: Canvas,
}
```

---

# 34. Color System

## Phase 1

- sRGB only.
- HSV color wheel.
- RGB sliders.
- HSV sliders.
- Hex input.
- Color history.
- Palette swatches.

## Phase 2

- Basic color profile groundwork.
- Optional OpenColorIO support later.

## Data Model

```rust
pub struct ColorState {
    pub fg: RgbaF32,
    pub bg: RgbaF32,
    pub history: VecDeque<RgbaF32>,
    pub palette: Vec<PaletteSwatch>,
}
```

---

# 35. Symmetry and Rulers

## Symmetry Modes

```rust
pub enum SymmetryMode {
    None,
    Horizontal,
    Vertical,
    Radial { segments: u8 },
}
```

When painting:

```rust
for point in symmetry.map_points(input_point) {
    brush.stroke_to(point.x, point.y, pressure, tilt);
}
```

Important: avoid duplicate undo snapshots for the same tile.

## Ruler Modes

```rust
pub enum RulerMode {
    None,
    StraightLine { angle: f32 },
    Parallel { angle: f32 },
}
```

Start simple with straight-line and symmetry tools.

---

# 36. Tablet Diagnostics

Tablet problems are common in drawing apps. Add diagnostics.

## UI

```text
Tablet Input

Source: Winit / Windows Ink / RealTimeStylus
Pressure raw: 0.43
Pressure smooth: 0.39
Tilt X: ...
Tilt Y: ...
Packets/sec: ...

[Enable Windows Ink fallback]
[Test Pressure Area]
[Calibrate Pressure]
```

## Useful Values

| Value | Purpose |
|---|---|
| Input source | Debug backend |
| Raw pressure | Check hardware input |
| Smoothed pressure | Check stabilizer |
| Tilt | Check pen data |
| Packet rate | Detect input lag |
| Event latency | Performance debugging |

---

# 37. Performance HUD

A performance HUD should be hidden by default but available from Help or Debug.

```rust
pub struct PerfStats {
    pub frame_ms: f32,
    pub compose_ms: f32,
    pub tile_upload_ms: f32,
    pub dirty_tiles: usize,
    pub uploaded_tiles: usize,
    pub gpu_tile_slots_used: usize,
    pub allocations_in_stroke: usize,
}
```

## UI

```text
Performance HUD

Frame: 1.6 ms
Compose: 0.4 ms
Tile Upload: 0.2 ms
Dirty Tiles: 12
Uploaded Tiles: 8
GPU Tile Slots: 382 / 4096
Stroke Allocations: 0
```

---

# 38. Recommended Rust Module Structure

Current structure:

```text
src/
├─ main.rs
├─ app.rs
├─ renderer.rs
├─ canvas.rs
├─ input.rs
├─ history.rs
├─ brush_io.rs
├─ save.rs
├─ stress_test.rs
```

Recommended expanded structure:

```text
src/
├─ main.rs
├─ app.rs
├─ renderer.rs
├─ canvas.rs
├─ input.rs
├─ history.rs
├─ brush_io.rs
├─ save.rs
├─ stress_test.rs
│
├─ ui/
│  ├─ mod.rs
│  ├─ menu.rs
│  ├─ quick_bar.rs
│  ├─ left_panel.rs
│  ├─ right_panel.rs
│  ├─ status_bar.rs
│  ├─ dialogs.rs
│  └─ command_palette.rs
│
├─ tools/
│  ├─ mod.rs
│  ├─ brush.rs
│  ├─ fill.rs
│  ├─ selection.rs
│  ├─ transform.rs
│  ├─ move_tool.rs
│  ├─ color_picker.rs
│  └─ ruler.rs
│
├─ selection.rs
├─ transform.rs
├─ fill.rs
├─ mask.rs
├─ commands.rs
├─ palette.rs
├─ reference.rs
│
├─ export/
│  ├─ mod.rs
│  ├─ png.rs
│  └─ ora.rs
│
├─ color/
│  ├─ mod.rs
│  ├─ hsv.rs
│  └─ ocio.rs
│
└─ tablet/
   ├─ mod.rs
   ├─ winit.rs
   ├─ windows_ink.rs
   └─ realtime_stylus.rs
```

---

# 39. UI State Design

```rust
pub struct UiState {
    pub active_tool: ToolId,
    pub left_panel_width: f32,
    pub right_panel_width: f32,
    pub show_left_panel: bool,
    pub show_right_panel: bool,
    pub show_status_bar: bool,
    pub show_debug: bool,
    pub minimal_ui: bool,

    pub open_dialog: Option<DialogId>,
    pub active_popup: Option<PopupId>,

    pub selected_brush_preset: usize,
    pub selected_layer_id: LayerId,

    pub tool_options: ToolOptions,
}
```

## Tool Enum

```rust
pub enum ToolId {
    Brush,
    Eraser,
    Fill,
    Gradient,
    RectSelect,
    EllipseSelect,
    Lasso,
    PolygonLasso,
    MagicWand,
    Move,
    Transform,
    ColorPicker,
    Hand,
    Zoom,
    RotateView,
    Line,
    Shape,
    Reference,
}
```

## Tool Options Enum

```rust
pub enum ToolOptions {
    Brush(BrushToolOptions),
    Fill(FillToolOptions),
    Selection(SelectionToolOptions),
    MagicWand(MagicWandOptions),
    Transform(TransformToolOptions),
    Move(MoveToolOptions),
    ColorPicker(ColorPickerOptions),
}
```

---

# 40. Performance Rules

To keep ARTY lightweight, follow these rules.

## 40.1 Keep Brush Hot Path Allocation-Free

Already good. Keep it this way.

Avoid:

- Creating new `Vec`s during stroke.
- Rebuilding brush settings every frame.
- Allocating tile buffers during dab placement.

Keep:

- Object pools.
- Preallocated staging buffers.
- Dirty flags.
- Ring buffers.

---

## 40.2 Do Not Commit Transform Every Frame

During transform:

- GPU preview only.
- Commit to tile pixels only when user presses Apply.

---

## 40.3 Do Not Update Thumbnails Every Frame

Use dirty flags.

```rust
pub struct LayerThumbnail {
    pub texture_id: TextureId,
    pub dirty: bool,
    pub last_update_frame: u64,
}
```

---

## 40.4 Respect WGPU Resource Usage Rules

Avoid using the same texture as both:

- `TEXTURE_BINDING`
- `RENDER_ATTACHMENT`

in the same render pass.

Use swap textures for:

- Layer compositing.
- Navigator.
- Thumbnail generation.
- Transform preview.
- Reference compositing if needed.

---

# 41. Immediate UI Fixes From the Screenshot

Based on the current screenshot, these are the best immediate improvements.

## 41.1 Reduce Stabilizer Duplication

Currently stabilizer appears both in the top bar and left panel.

Recommended:

- Keep compact stabilizer in Quick Bar.
- Move detailed stabilizer settings into collapsible panel.

```text
▸ Stabilizer Advanced
```

---

## 41.2 Add Real Quick Bar

Current top bar should become:

```text
[Undo] [Redo] [Save] |
[Select] [Deselect] [Transform] |
[Fit] [100%] [Mirror] |
[Stabilizer]
```

---

## 41.3 Improve Brush Slider Layout

Current sliders are functional but cramped. Use clearer labels:

```text
Size       2.7 px   ━━━━━○━━━━
Opacity    95%      ━━━━━━━○━━
Hardness   95%      ━━━━━━━○━━
Min Size   80%      ━━━━━━○━━━
```

---

## 41.4 Improve Layer Buttons

Current:

```text
+ Raster + Folder + Vector - Delete
```

Recommended:

```text
[+ Raster] [+ Folder] [+ Mask]
[Duplicate] [Merge] [Delete]
```

If vector layers are not implemented yet, hide the Vector button or mark it experimental.

---

## 41.5 Add Navigator Viewport Rect

The navigator should show the current visible area and allow dragging it.

---

## 41.6 Add Dynamic Tool Options

Brush configuration should not be shown for all tools. It should change depending on active tool.

---

# 42. Development Milestones

## Milestone 1 — Core Drawing Usability

Goal: ARTY becomes usable for real painting.

- [ ] New Document dialog
- [ ] Export PNG
- [ ] Autosave indicator
- [ ] Quick Bar
- [ ] Fit / 100% / Reset View
- [ ] Fill Bucket
- [ ] Rect Selection
- [ ] Lasso Selection
- [ ] Transform Layer
- [ ] Transform Selection
- [ ] Layer Duplicate
- [ ] Merge Down
- [ ] Layer context menu
- [ ] Improved status bar

---

## Milestone 2 — SAI-Like Workflow

Goal: ARTY feels clean and fast like SAI.

- [ ] Dynamic Tool Options
- [ ] Brush preset manager
- [ ] Brush duplicate/rename/export
- [ ] Per-brush stabilizer
- [ ] Color history
- [ ] Palette import/export
- [ ] Layer thumbnails
- [ ] Minimal UI mode
- [ ] Command palette
- [ ] Shortcut editor

---

## Milestone 3 — Better Editing

Goal: ARTY supports common illustration editing workflows.

- [ ] Magic Wand
- [ ] Selection grow/shrink/feather
- [ ] Layer masks
- [ ] Merge visible
- [ ] Flatten image
- [ ] Reference image panel
- [ ] More blend modes
- [ ] Symmetry
- [ ] Basic rulers

---

## Milestone 4 — Reliability and Interoperability

Goal: ARTY becomes stable as a daily tool.

- [ ] Crash recovery
- [ ] OpenRaster `.ora` export
- [ ] OpenRaster `.ora` import
- [ ] Tablet diagnostics
- [ ] Performance HUD
- [ ] Workspace presets
- [ ] Config folder management
- [ ] Stress test CI

---

# 43. Complete Feature Checklist

## Drawing

- [x] Brush tool
- [x] Eraser tool
- [x] Smudge base
- [ ] Fill bucket
- [ ] Gradient
- [ ] Brush cursor
- [ ] Straight line assist
- [ ] Brush preset duplicate
- [ ] Brush preset rename
- [ ] Brush preset import/export
- [ ] Brush editor popup
- [ ] Per-brush stabilizer

---

## Selection

- [ ] Rect select
- [ ] Ellipse select
- [ ] Lasso
- [ ] Polygon lasso
- [ ] Magic wand
- [ ] Add/Subtract/Intersect selection
- [ ] Deselect
- [ ] Invert
- [ ] Grow
- [ ] Shrink
- [ ] Feather
- [ ] Selection mask overlay
- [ ] Transform selection

---

## Transform

- [ ] Move selected area
- [ ] Move layer
- [ ] Scale
- [ ] Rotate
- [ ] Flip horizontal/vertical
- [ ] Free transform
- [ ] Apply/Cancel overlay
- [ ] GPU preview
- [ ] CPU commit to tiles

---

## Layers

- [x] Raster layer
- [x] Folder
- [x] Blend mode
- [x] Opacity
- [x] Lock alpha
- [x] Clipping group
- [ ] Layer mask
- [ ] Duplicate
- [ ] Merge down
- [ ] Merge visible
- [ ] Flatten
- [ ] Layer thumbnail
- [ ] Layer context menu
- [ ] Multi-select layers
- [ ] Folder pass-through

---

## Canvas View

- [x] Zoom
- [x] Pan
- [x] Rotate view
- [x] Mirror view
- [ ] Fit to screen
- [ ] Actual size 100%
- [ ] Reset view
- [ ] Grid
- [ ] Guides
- [ ] Rulers
- [ ] Symmetry
- [ ] Fullscreen
- [ ] Minimal UI

---

## Color

- [x] Color wheel
- [x] Swatches
- [ ] HSV sliders
- [ ] RGB sliders
- [ ] Hex input
- [ ] Foreground/background colors
- [ ] Swap foreground/background
- [ ] Default black/white reset
- [ ] Color history
- [ ] Palette import/export
- [ ] Palette rename/delete
- [ ] Swatch context menu

---

## File / Document

- [x] Native `.arty` save/load base
- [ ] New document dialog
- [ ] Open document
- [ ] Open recent
- [ ] Save As
- [ ] Autosave
- [ ] Crash recovery
- [ ] Export PNG
- [ ] Export JPG
- [ ] Export OpenRaster `.ora`
- [ ] Import image as layer
- [ ] Import reference image
- [ ] Document info dialog
- [ ] Resize canvas
- [ ] Resize image
- [ ] Crop to selection
- [ ] Trim transparent pixels

---

## Tablet / Input

- [x] Winit pressure input
- [x] Optional RealTimeStylus / octotablet fallback
- [x] EMA stabilizer
- [x] Spring-Mass-Damper stabilizer
- [ ] Per-brush stabilizer
- [ ] Tablet diagnostics panel
- [ ] Pressure calibration curve
- [ ] Tilt diagnostics
- [ ] Packet rate display
- [ ] Input latency display
- [ ] Windows Ink fallback UI
- [ ] Configurable pen button actions

---

## Performance / Debug

- [x] Zero-allocation stroke hot path stress test
- [x] GPU tile cache
- [x] Dirty tile upload
- [ ] Performance HUD
- [ ] Tile cache visualization
- [ ] Dirty tile debug overlay
- [ ] GPU timing stats
- [ ] CPU frame timing stats
- [ ] Allocation counter display
- [ ] Memory usage estimate
- [ ] Layer memory estimate
- [ ] Undo memory estimate
- [ ] Stress test CI integration

---

# 44. Recommended Implementation Order

This section gives a practical order for implementation, assuming you want maximum user-visible improvement with minimum architectural risk.

---

## Phase 1: UI Foundation Polish

Start here because it makes the application immediately feel more professional.

### Implement first:

1. **Quick Bar**
2. **Better Status Bar**
3. **Fit / 100% / Reset View**
4. **Improved Layer Panel Buttons**
5. **Dynamic Tool Options foundation**
6. **New Document dialog**
7. **Export PNG dialog**

### Why this first?

These features do not require changing the brush engine deeply. They improve workflow quickly and prepare the UI for later tools.

---

## Phase 2: Command System

Before adding Fill, Selection, Transform, and Layer operations, unify undo/redo.

### Implement:

```rust
pub enum HistoryCommand {
    Stroke(TileStrokeCommand),
    Fill(FillCommand),
    Transform(TransformCommand),
    Selection(SelectionCommand),
    CreateLayer(CreateLayerCommand),
    DeleteLayer(DeleteLayerCommand),
    DuplicateLayer(DuplicateLayerCommand),
    MergeLayer(MergeLayerCommand),
    RenameLayer(RenameLayerCommand),
    ChangeLayerOpacity(ChangeLayerOpacityCommand),
    ChangeLayerBlendMode(ChangeLayerBlendModeCommand),
}
```

### Why this second?

If you add tools before command history, undo/redo will become inconsistent. A painting app must make everything undoable.

---

## Phase 3: Selection Base

Implement selection before transform and fill polish.

### Implement:

1. Rectangular selection
2. Deselect
3. Select all
4. Invert selection
5. Selection overlay
6. Selection mask application to brush strokes

### Selection data:

```rust
pub struct Selection {
    pub tiles: HashMap<TileCoord, SelectionTile>,
    pub bounds: Option<IntRect>,
    pub feather_radius: f32,
    pub antialias: bool,
    pub dirty_tiles: Vec<TileCoord>,
}
```

### Why selection before fill and transform?

Because Fill, Copy, Cut, Clear, Transform, and Brush masking all need selection awareness.

---

## Phase 4: Fill Bucket

Implement Fill Bucket as the first non-brush editing tool.

### Implement:

1. Current-layer fill
2. Tolerance
3. Respect selection
4. Dirty tile marking
5. Undo support
6. Sample all visible layers
7. Expand fill
8. Close gap later

### Suggested order:

```text
Basic fill
→ Tile-aware scanline fill
→ Selection-aware fill
→ Undo snapshots
→ Sample all layers
→ Expand fill
→ Close gap
```

---

## Phase 5: Transform Tool

Implement transform with GPU preview first.

### Minimum transform:

1. Move layer
2. Move selected area
3. Scale
4. Rotate
5. Apply / Cancel
6. Undo support

### Later:

1. Free transform
2. Perspective transform
3. Bicubic interpolation
4. Numeric transform fields

---

## Phase 6: Brush Preset Manager

After core editing tools exist, polish the brush workflow.

### Implement:

1. Duplicate preset
2. Rename preset
3. Delete preset
4. Save preset changes
5. Import/export `.artybrush`
6. Favorite presets
7. Per-brush stabilizer
8. Brush editor popup

---

## Phase 7: Layer Workflow

### Implement:

1. Duplicate layer
2. Merge down
3. Merge visible
4. Flatten image
5. Layer thumbnails
6. Layer context menu
7. Layer masks
8. Folder pass-through mode

---

## Phase 8: Reliability

### Implement:

1. Autosave
2. Crash recovery
3. Open recent
4. Document info
5. Preferences storage
6. Shortcut editor

---

## Phase 9: Artist Tools

### Implement:

1. Reference image panel
2. Symmetry
3. Rulers
4. Magic wand
5. Selection feather/grow/shrink
6. ORA import/export

---

# 45. Suggested UI Component Breakdown

To avoid `app.rs` becoming too large, split UI drawing into separate modules.

```text
src/ui/
├─ mod.rs
├─ menu.rs
├─ quick_bar.rs
├─ left_panel.rs
├─ right_panel.rs
├─ status_bar.rs
├─ canvas_overlay.rs
├─ dialogs.rs
├─ tool_options.rs
├─ layer_panel.rs
├─ color_panel.rs
├─ navigator.rs
├─ palette_panel.rs
├─ reference_panel.rs
└─ command_palette.rs
```

---

## 45.1 `ui/menu.rs`

Responsible for:

- File menu
- Edit menu
- Canvas menu
- Layer menu
- Selection menu
- View menu
- Window menu
- Help menu

Example:

```rust
pub fn draw_menu_bar(app: &mut PaintApp, ctx: &egui::Context) {
    egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
        egui::menu::bar(ui, |ui| {
            draw_file_menu(app, ui);
            draw_edit_menu(app, ui);
            draw_canvas_menu(app, ui);
            draw_layer_menu(app, ui);
            draw_selection_menu(app, ui);
            draw_view_menu(app, ui);
            draw_window_menu(app, ui);
            draw_help_menu(app, ui);
        });
    });
}
```

---

## 45.2 `ui/quick_bar.rs`

Responsible for:

- Undo/redo buttons
- Save button
- Selection shortcuts
- Transform button
- Zoom controls
- Rotation controls
- Mirror toggle
- Stabilizer selector
- Autosave indicator

Example:

```rust
pub fn draw_quick_bar(app: &mut PaintApp, ctx: &egui::Context) {
    egui::TopBottomPanel::top("quick_bar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            if ui.button("↶ Undo").clicked() {
                app.command(CommandId::Undo);
            }

            if ui.button("↷ Redo").clicked() {
                app.command(CommandId::Redo);
            }

            ui.separator();

            if ui.button("Save").clicked() {
                app.command(CommandId::Save);
            }

            ui.separator();

            if ui.button("Fit").clicked() {
                app.command(CommandId::FitToScreen);
            }

            if ui.button("100%").clicked() {
                app.command(CommandId::ActualSize);
            }

            if ui.button("Reset View").clicked() {
                app.command(CommandId::ResetView);
            }
        });
    });
}
```

---

## 45.3 `ui/left_panel.rs`

Responsible for:

- Tools panel
- Brush presets
- Dynamic tool options
- Brush settings
- Stabilizer settings

Structure:

```rust
pub fn draw_left_panel(app: &mut PaintApp, ctx: &egui::Context) {
    egui::SidePanel::left("left_panel")
        .resizable(true)
        .default_width(240.0)
        .show(ctx, |ui| {
            draw_tools_panel(app, ui);
            ui.separator();

            draw_brush_presets(app, ui);
            ui.separator();

            draw_dynamic_tool_options(app, ui);
            ui.separator();

            if matches!(app.ui.active_tool, ToolId::Brush | ToolId::Eraser) {
                draw_brush_settings(app, ui);
            }

            draw_stabilizer_panel(app, ui);
        });
}
```

---

## 45.4 `ui/right_panel.rs`

Responsible for:

- Navigator
- Color selector
- Palette
- Layers
- Reference images

```rust
pub fn draw_right_panel(app: &mut PaintApp, ctx: &egui::Context) {
    egui::SidePanel::right("right_panel")
        .resizable(true)
        .default_width(280.0)
        .show(ctx, |ui| {
            draw_navigator(app, ui);
            ui.separator();

            draw_color_panel(app, ui);
            ui.separator();

            draw_palette_panel(app, ui);
            ui.separator();

            draw_layer_panel(app, ui);
            ui.separator();

            draw_reference_panel(app, ui);
        });
}
```

---

## 45.5 `ui/status_bar.rs`

Responsible for live state display.

```rust
pub fn draw_status_bar(app: &mut PaintApp, ctx: &egui::Context) {
    egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label(format!("Tool: {:?}", app.ui.active_tool));
            ui.separator();

            ui.label(format!("Size: {:.1}px", app.current_brush_radius()));
            ui.separator();

            ui.label(format!("Pressure: {:.2}", app.input.smoothed_pressure));
            ui.separator();

            ui.label(format!("Zoom: {:.0}%", app.viewport.zoom * 100.0));
            ui.separator();

            ui.label(format!("Rotation: {:.0}°", app.viewport.rotation_degrees()));
            ui.separator();

            ui.label(format!(
                "Mirror: {}",
                if app.viewport.mirrored { "On" } else { "Off" }
            ));

            ui.separator();

            ui.label(format!("Layer: {}", app.active_layer_name()));

            ui.separator();

            ui.label(app.autosave_status_text());
        });
    });
}
```

---

# 46. Internal Command Dispatcher

As the UI grows, avoid directly modifying state everywhere. Use a command dispatcher.

```rust
pub enum CommandId {
    NewDocument,
    Open,
    Save,
    SaveAs,
    ExportPng,

    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    Clear,

    SelectAll,
    Deselect,
    InvertSelection,

    Transform,
    ApplyTransform,
    CancelTransform,

    NewRasterLayer,
    NewFolder,
    DuplicateLayer,
    DeleteLayer,
    MergeDown,
    MergeVisible,

    FitToScreen,
    ActualSize,
    ResetView,
    ToggleMirrorView,
    RotateViewLeft,
    RotateViewRight,
    ResetRotation,

    ToggleMinimalUi,
    OpenPreferences,
    OpenShortcutEditor,
    OpenTabletDiagnostics,
    OpenPerformanceHud,
}
```

Then:

```rust
impl PaintApp {
    pub fn command(&mut self, command: CommandId) {
        match command {
            CommandId::Undo => self.history.undo(&mut self.document),
            CommandId::Redo => self.history.redo(&mut self.document),

            CommandId::Save => self.request_save(),
            CommandId::ExportPng => self.open_dialog(DialogId::ExportPng),

            CommandId::FitToScreen => self.viewport.fit_to_document(&self.document),
            CommandId::ActualSize => self.viewport.zoom = 1.0,
            CommandId::ResetView => self.viewport.reset(),

            CommandId::ToggleMirrorView => {
                self.viewport.mirrored = !self.viewport.mirrored;
            }

            CommandId::NewRasterLayer => self.create_raster_layer(),
            CommandId::DeleteLayer => self.delete_active_layer(),

            _ => {}
        }
    }
}
```

Benefits:

- Cleaner UI code.
- Easier shortcut system.
- Easier command palette.
- Easier undo integration.
- Easier testing.

---

# 47. Shortcut System Design

## Key Binding Data Model

```rust
pub struct KeyBinding {
    pub key: KeyCode,
    pub modifiers: Modifiers,
}

pub struct ShortcutEntry {
    pub command: CommandId,
    pub primary: Option<KeyBinding>,
    pub secondary: Option<KeyBinding>,
}
```

## Shortcut Map

```rust
pub struct ShortcutMap {
    pub entries: Vec<ShortcutEntry>,
}
```

## Shortcut Processing

```rust
impl ShortcutMap {
    pub fn find_command(
        &self,
        key: KeyCode,
        modifiers: Modifiers,
    ) -> Option<CommandId> {
        self.entries
            .iter()
            .find(|entry| {
                entry.primary.as_ref().is_some_and(|b| b.matches(key, modifiers))
                    || entry.secondary.as_ref().is_some_and(|b| b.matches(key, modifiers))
            })
            .map(|entry| entry.command)
    }
}
```

---

# 48. Preferences System

Preferences should be stored separately from documents.

## Suggested Config Files

```text
config/
├─ preferences.toml
├─ shortcuts.toml
├─ workspace.toml
├─ recent_files.toml
├─ palettes/
│  └─ default.arty_palette
└─ brushes/
   └─ default.artybrush
```

## Preferences Structure

```rust
pub struct Preferences {
    pub interface: InterfacePreferences,
    pub tablet: TabletPreferences,
    pub performance: PerformancePreferences,
    pub autosave: AutosavePreferences,
    pub files: FilePreferences,
}

pub struct InterfacePreferences {
    pub theme: Theme,
    pub ui_scale: f32,
    pub show_tooltips: bool,
    pub compact_sliders: bool,
    pub left_panel_width: f32,
    pub right_panel_width: f32,
}

pub struct TabletPreferences {
    pub input_source: TabletInputSource,
    pub pressure_min: f32,
    pub pressure_max: f32,
    pub pressure_curve: f32,
    pub enable_realtime_stylus: bool,
}

pub struct PerformancePreferences {
    pub gpu_backend: GpuBackendPreference,
    pub tile_cache_slots: usize,
    pub undo_memory_limit_mb: usize,
    pub limit_navigator_update_rate: bool,
}

pub struct AutosavePreferences {
    pub enabled: bool,
    pub interval_seconds: u64,
    pub keep_versions: usize,
    pub folder: PathBuf,
}
```

Use `serde` with `toml`, `ron`, or `json`.

For human-editable settings, `toml` is a good option.

---

# 49. Layer Thumbnail System

Layer thumbnails are important but must not update every frame.

## Data Model

```rust
pub struct LayerThumbnail {
    pub texture_id: Option<egui::TextureId>,
    pub dirty: bool,
    pub last_update_frame: u64,
    pub size: [usize; 2],
}
```

## Update Rule

Update when:

- Layer pixels changed.
- Layer opacity changed.
- Layer mask changed.
- Layer visibility changed.
- Blend mode changed.
- Layer was transformed.
- Layer was filled.
- Layer was merged.

Do not update:

- Every frame.
- While a stroke is actively drawing, unless throttled.

## Throttling

```rust
const THUMBNAIL_UPDATE_INTERVAL_FRAMES: u64 = 10;
```

or:

```rust
const THUMBNAIL_UPDATE_INTERVAL_MS: u64 = 150;
```

---

# 50. Navigator Update Strategy

Navigator should also be dirty-flagged.

Update navigator when:

- Layer pixels changed.
- Layer visibility changed.
- Layer order changed.
- Layer blend mode/opacity changed.
- Document background changed.
- Transform committed.

Do not recompose navigator every frame if nothing changed.

---

# 51. Selection Overlay Rendering

Selection overlay can be rendered in multiple modes:

```rust
pub enum SelectionDisplayMode {
    MarchingAnts,
    MaskOverlay,
    Hidden,
}
```

## Marching Ants

Good for standard selection display.

Data needed:

- Selection mask edges.
- Animated dash phase.
- Screen-space line rendering.

## Mask Overlay

Good for soft selections and feathering.

Render selected area as translucent blue/red overlay:

```text
overlay_color = vec4(0.2, 0.5, 1.0, 0.25)
```

## Hidden

Useful for artists who want distraction-free painting.

---

# 52. Transform Overlay Rendering

Transform handles:

```text
top-left      top       top-right

left          center    right

bottom-left   bottom    bottom-right

rotation handle above top-center
```

## Interaction

| Handle | Action |
|---|---|
| Corner handle | Scale both axes |
| Edge handle | Scale one axis |
| Center | Move |
| Rotation handle | Rotate |
| Shift drag | Preserve aspect ratio |
| Alt drag | Scale from center |
| Enter | Apply |
| Esc | Cancel |

---

# 53. Fill Bucket Technical Notes

## Tile-Aware Scanline Fill

A scanline fill generally performs better than pixel-by-pixel BFS.

Suggested span:

```rust
pub struct FillSpan {
    pub y: i32,
    pub x_start: i32,
    pub x_end: i32,
}
```

Work queue:

```rust
pub struct FillWorkQueue {
    pub spans: Vec<FillSpan>,
}
```

For performance, reuse the queue:

```rust
pub struct FillWorkspace {
    pub spans: Vec<FillSpan>,
    pub visited_tiles: HashMap<TileCoord, VisitedTile>,
}
```

Visited tile:

```rust
pub struct VisitedTile {
    pub bits: [u64; 64],
}
```

Since a tile is 64×64 pixels, one `u64` per row can store visited pixels.

---

# 54. Selection Mask Technical Notes

A selection tile uses:

```rust
pub struct SelectionTile {
    pub mask: [u8; 4096],
}
```

Memory cost:

```text
64 × 64 = 4096 bytes per selected tile
```

This is acceptable because selection tiles are sparse.

---

# 55. Layer Mask Technical Notes

A layer mask tile also uses:

```rust
pub struct MaskTile {
    pub data: [u8; 4096],
}
```

Because layer masks are sparse, memory stays reasonable.

Potential optimization:

- Treat missing mask tile as fully white.
- Store only tiles that differ from fully white.
- If mask is empty/disabled, skip mask path.

---

# 56. Brush Engine Integration Notes

Current brush sync logic is good:

- Use `brush_settings_dirty`.
- Rebuild Hokusai/libmypaint settings only when dirty.
- Do not rebuild pressure curves every frame.

Keep this rule:

```text
Brush settings should only be synchronized when:
- brush slider changes
- color changes
- texture changes
- preset changes
- eraser toggle changes
- keyboard brush size shortcut changes
```

---

# 57. Pressure Curve UI

Expose pressure control without overwhelming users.

## Simple Mode

```text
Pressure:
Size Response:    [slider]
Opacity Response: [slider]
Min Pressure:     [slider]
```

## Advanced Mode

Show editable curve:

```text
Pressure Curve
0.0 ─●───────
    ───●─────
    ──────●──
1.0 ───────●
```

Internally, keep the current mathematical model:

### Radius curve

Minimum size fraction:

```text
offset at pressure 0 = ln(M)
```

### Opacity floor

```text
floor = (1 - opacity) × 0.55 + 0.05
Δ at pressure 0 = -opacity × (1 - min(floor, 0.90))
```

### OpaqueMultiply S-curve

| Pressure | Multiplier |
|---:|---:|
| 0.0 | 0.00 |
| 0.3 | 0.55 |
| 0.6 | 0.85 |
| 1.0 | 1.00 |

This is good for preventing overly opaque low-pressure strokes.

---

# 58. Canvas View Mathematics

The existing coordinate system should be preserved.

## GPU Rendering Transform

World to NDC:

```text
x_ndc = ((x_world - x_offset) × zoom) / (W / 2) - 1
y_ndc = 1 - ((y_world - y_offset) × zoom) / (H / 2)
```

Horizontal mirror:

```text
x'_ndc = -x_ndc
```

Rotation:

```text
x'' = x' cosθ - y sinθ
y'' = x' sinθ + y cosθ
```

## Input Inverse Transform

Screen to NDC:

```text
n_x = (s_x - c_x) / (W / 2)
n_y = -(s_y - c_y) / (H / 2)
```

Inverse rotation:

```text
p_x = n_x cosθ + n_y sinθ
p_y = -n_x sinθ + n_y cosθ
```

Inverse mirror:

```text
if mirrored:
    p_x = -p_x
```

Back to world:

```text
w_x = ((p_x + 1) × (W / 2)) / zoom + x_offset
w_y = ((1 - p_y) × (H / 2)) / zoom + y_offset
```

This is important for accurate drawing when the view is rotated or mirrored.

---

# 59. WGPU Compositing Rules

The current renderer already handles an important WGPU rule:

> A texture must not be used as both a sampled texture and render attachment in the same render pass.

Keep using:

- Swap textures.
- Blank texture view for passes that do not sample.
- Separate source and destination textures.

This applies to:

- Main composition.
- Navigator.
- Layer thumbnails.
- Transform preview.
- Reference image compositing.

---

# 60. Suggested MVP Definition

If you want a clear “first public alpha” target, define MVP as:

## ARTY Alpha 1

### Required:

- Brush / Eraser / Smudge
- Stable pressure input
- Stabilizer
- Layers with opacity/blend/lock alpha/clipping
- Pan / zoom / rotate / mirror
- Quick Bar
- New / Open / Save / Save As
- Export PNG
- Autosave
- Fill bucket
- Rect selection
- Lasso selection
- Transform layer/selection
- Undo/redo for stroke, fill, transform, layer operations
- Brush preset duplicate/rename/save
- Basic preferences
- Performance HUD
- Tablet diagnostics

### Not required for Alpha 1:

- Vector layers
- PSD import/export
- Advanced color management
- Perspective transform
- Text layers
- Plugin system
- Animation

---

# 61. Suggested Beta Definition

## ARTY Beta 1

### Add:

- Magic wand
- Layer masks
- Reference images
- ORA export/import
- Layer thumbnails
- Command palette
- Shortcut editor
- Palette import/export
- Color history
- Symmetry
- Ruler tools
- Workspace presets
- Recovery dialog

---

# 62. Features to Avoid Early

To keep the application lightweight and avoid scope explosion, avoid these until the core is polished:

| Feature | Why delay it? |
|---|---|
| Full PSD support | Very complex format |
| Text engine | Font shaping/layout complexity |
| Animation timeline | Large separate workflow |
| Plugin system | API stability problem |
| Full color management | Complex, can come later |
| Advanced vector system | Requires separate editing model |
| 3D reference models | Heavy and outside SAI-like scope |
| Full docking system | UI complexity; fixed panels are enough early |

---

# 63. Design Principles

Use these principles as decision rules.

## 63.1 Keep the Canvas Fast

Any feature that touches pixels should be:

```text
tile-aware
dirty-flagged
undo-aware
GPU-preview-first if interactive
CPU-commit-later if destructive
```

---

## 63.2 Keep the UI Clean

Do not show all settings at once.

Use:

- Collapsible sections.
- Dynamic tool options.
- Context menus.
- Command palette.
- Preferences dialog.
- Advanced panels hidden by default.

---

## 63.3 Keep Brush Settings Lazy

Do not rebuild brush engine settings every frame.

Use:

```rust
brush_settings_dirty: bool
```

---

## 63.4 Make Every Destructive Action Undoable

Every destructive command should produce a history entry:

- Stroke
- Fill
- Clear
- Cut
- Paste
- Transform
- Merge
- Delete layer
- Apply mask
- Resize canvas
- Resize image

---

## 63.5 Separate View Transform from Pixel Transform

Always distinguish:

| View operation | Pixel operation |
|---|---|
| Rotate View | Rotate Image |
| Mirror View | Flip Canvas |
| Zoom View | Resize Image |

This prevents accidental destructive edits.

---

# 64. Suggested Naming Conventions

To keep code consistent:

## UI modules

```text
draw_* for UI rendering functions
```

Example:

```rust
draw_menu_bar()
draw_quick_bar()
draw_left_panel()
draw_layer_panel()
draw_status_bar()
```

## Commands

```text
CommandId::VerbNoun
```

Example:

```rust
CommandId::NewDocument
CommandId::ExportPng
CommandId::ToggleMirrorView
CommandId::MergeDown
```

## Tools

```text
ToolId::Noun
```

Example:

```rust
ToolId::Brush
ToolId::Fill
ToolId::MagicWand
```

## Options

```text
*Options
```

Example:

```rust
FillToolOptions
SelectionToolOptions
TransformToolOptions
```

---

# 65. Example Final UI Mockup

```text
+--------------------------------------------------------------------------------+
| File Edit Canvas Layer Selection View Window Help                               |
+--------------------------------------------------------------------------------+
| ↶ Undo  ↷ Redo  Save | Cut Copy Paste | Select Deselect Invert | Transform      |
| Fit 100% Reset | Zoom 75% | Rotate -15 0 +15 | Mirror H | Stabilizer S-3       |
+----------------------+-------------------------------------+-------------------+
| TOOLS                |                                     | NAVIGATOR         |
| [Brush] [Eraser]     |                                     | +---------------+ |
| [Fill]  [Gradient]   |                                     | | thumbnail     | |
| [Select][Lasso]      |                                     | | viewport rect | |
| [Wand]  [Move]       |                                     | +---------------+ |
| [Trans][Picker]      |                                     | Fit 100 Reset     |
|                      |                                     |                   |
| BRUSH PRESETS        |                                     | COLOR             |
| [Pencil] [Ink]       |         CANVAS VIEWPORT             | [Wheel HSV RGB]   |
| [Brush]  [Water]     |                                     | FG #191919        |
| [Smudge] [Eraser]    |                                     | BG #FFFFFF        |
| [+] [Duplicate]      |                                     |                   |
|                      |                                     | PALETTE           |
| TOOL OPTIONS         |                                     | [swatches...]     |
| Size 2.7 px          |                                     |                   |
| Opacity 95%          |                                     | LAYERS            |
| Hardness 95%         |                                     | Blend Normal      |
| Min Size 80%         |                                     | Opacity 100%      |
| Texture None         |                                     | +Raster +Folder   |
|                      |                                     | Duplicate Merge   |
| STABILIZER           |                                     | 👁 Layer 3        |
| Level S-3            |                                     | 👁 Layer 2        |
| Mode Spring          |                                     | 👁 Layer 1        |
+----------------------+-------------------------------------+-------------------+
| Tool: Pencil | Size: 2.7px | Pressure: 0.42 | Zoom: 75% | Rot: 0° | Autosaved  |
+--------------------------------------------------------------------------------+
```

---

# 66. References

Below are the reference sources discussed and relevant to the design direction.

---

## PaintTool SAI UI and Workflow References

### 1. PaintTool SAI — Main Window

Used as reference for the general UI structure:

- Color Panel
- Tool Panel
- Quick Bar
- Navigator
- Layer Panel

URL:  
https://en.saipainttool.com/manual/main-window/

---

### 2. PaintTool SAI — Quick Bar

Used as reference for quick access to:

- Undo/redo
- Selection operations
- Zoom
- Rotation
- Flipping
- Stabilizer

URL:  
https://en.saipainttool.com/manual/quick-bar/

---

### 3. PaintTool SAI — Layer Panel

Used as reference for:

- Layer list
- Visibility control
- Layer operations
- Layer movement
- Basic layer workflow

URL:  
https://en.saipainttool.com/manual/layer-panel/

---

### 4. PaintTool SAI — Common Tools

Used as reference for:

- Rectangular selection
- Transform image
- Common editing tools

URL:  
https://en.saipainttool.com/manual/common-tools/

---

### 5. PaintTool SAI — Tools

Used as reference for:

- Tool panel
- Painting tools
- Brush/tool parameters

URL:  
https://en.saipainttool.com/manual/tools/

---

### 6. PaintTool SAI — Painting Tools

Used as reference for:

- Brush blending
- Water/dilution-like behavior
- Painting tool parameters

URL:  
https://en.saipainttool.com/manual/painting-tools/

---

## Krita References

### 7. Krita Manual — Dockers

Used as reference for panel/docking concepts:

- Color selector
- Layers
- Palette
- Reference images
- Overview/navigator-like panels

URL:  
https://docs.krita.org/en/reference_manual/dockers.html

---

### 8. Krita Manual — Fill Tool

Used as reference for fill tool behavior and fill options.

URL:  
https://docs.krita.org/en/reference_manual/tools/fill.html

---

### 9. Krita Manual — Layers and Masks

Used as reference for:

- Layers
- Masks
- Non-destructive editing
- Layer workflows

URL:  
https://docs.krita.org/en/user_manual/layers_and_masks.html

---

### 10. Krita Manual — Clipping Masks and Alpha Inheritance

Used as reference for clipping and alpha inheritance workflow.

URL:  
https://docs.krita.org/en/tutorials/clipping_masks_and_alpha_inheritance.html

---

### 11. Krita Manual — MyPaint Brush Engine

Used as reference for MyPaint/libmypaint brush behavior in a painting application.

URL:  
https://docs.krita.org/en/reference_manual/brushes/brush_engines/mypaint_engine.html

---

## Clip Studio Paint References

### 12. Clip Studio Paint — Palettes

Used as reference for palette/panel organization.

URL:  
https://help.clip-studio.com/en-us/manual_en/690_interface/Palettes.htm

---

### 13. Clip Studio Paint — How to Use Tools

Used as reference for:

- Tool palette
- Sub tool palette
- Tool options workflow

URL:  
https://help.clip-studio.com/en-us/manual_en/150_tools/How_to_use_tools.htm

---

## Brush Engine References

### 14. MyPaint Brush Engine Documentation

Used as reference for:

- Brush engine design
- Tablet-friendly brush behavior
- Configurable brush settings

URL:  
https://www.mypaint.app/en/docs/backend/brush-engine/

---

### 15. libmypaint GitHub

Used as reference for the brush engine library.

URL:  
https://github.com/mypaint/libmypaint

---

### 16. libmypaint Wiki — Using Brushlib

Used as reference for:

- Brush settings
- Input mapping
- Pressure/velocity/random mappings

URL:  
https://github.com/mypaint/libmypaint/wiki/Using-Brushlib

---

## Rust / GUI / GPU References

### 17. egui GitHub

Used as reference for the immediate-mode GUI library.

URL:  
https://github.com/emilk/egui

---

### 18. eframe Documentation

Used as reference for building native applications with egui.

URL:  
https://docs.rs/eframe/latest/eframe/

---

### 19. wgpu Documentation

Used as reference for WGPU rendering concepts.

URL:  
https://wgpu.rs/doc/wgpu/  
https://docs.rs/wgpu/latest/wgpu/

---

### 20. WebGPU Specification

Used as reference for GPU resource usage rules, especially avoiding conflicting texture usages in render passes.

URL:  
https://www.w3.org/TR/webgpu/

---

## Blend Mode References

### 21. W3C Compositing and Blending Level 1

Used as reference for standard blend formulas:

- Normal
- Multiply
- Screen
- Overlay
- Color Dodge
- Color Burn
- Soft Light
- Hard Light
- Luminosity

URL:  
https://www.w3.org/TR/compositing-1/

---

## File Format References

### 22. OpenRaster Specification

Used as reference for layered raster file exchange.

URL:  
https://www.openraster.org/

---

### 23. OpenRaster File Layout Specification

Used as reference for `.ora` archive structure:

- `mimetype`
- `stack.xml`
- `data/`
- `Thumbnails/`
- `mergedimage.png`

URL:  
https://www.openraster.org/baseline/file-layout-spec.html

---

## Tablet / Pen Input References

### 24. Wacom Windows Ink Documentation

Used as reference for Windows pen input and pressure data.

URL:  
https://developer-docs.wacom.com/docs/icbt/windows/windows-ink/windows-ink-overview/

---

### 25. Wacom Wintab Developer Support

Used as reference for tablet APIs and pen data access.

URL:  
https://developer-support.wacom.com/hc/en-us/articles/12844524637975-Wintab

---

## Color Management References

### 26. OpenColorIO

Used as a future reference for advanced color management.

URL:  
https://opencolorio.org/

---

# 67. Final Summary

ARTY / Xcalux already has a very strong technical foundation:

- Sparse infinite tiled canvas.
- 64×64 tiles.
- fix15 premultiplied RGBA.
- WGPU compositing.
- Dirty tile upload.
- GPU tile LRU cache.
- Hokusai/libmypaint brush engine.
- EMA and Spring-Mass-Damper stabilizers.
- Zero-allocation active stroke loop.
- Background save/load.
- Layer compositing with blend modes.

The best path forward is not to add huge complex features immediately. Instead, build a clean SAI-like workflow around the existing engine.

The highest-impact next steps are:

1. **Quick Bar**
2. **Dynamic Tool Options**
3. **New Document and Export PNG**
4. **Selection system**
5. **Fill Bucket**
6. **Transform Tool**
7. **Layer duplicate/merge/context menu**
8. **Autosave and recovery**
9. **Brush preset manager**
10. **Layer masks**
11. **Reference image panel**
12. **Tablet diagnostics and performance HUD**

The guiding rule should be:

> Add features in a way that keeps the drawing loop fast, the UI clean, and the canvas architecture tile-aware.

For every new system, prefer:

```text
Sparse data
Dirty flags
Object pools
GPU preview
CPU commit only when needed
Undo-aware commands
Minimal visible UI
Advanced options hidden by default
```

If ARTY follows this direction, it can become a lightweight Rust-native digital painting workstation with the smooth, focused feel of PaintTool SAI while retaining modern GPU acceleration and strong internal engineering.
