# ARTY (Xcalux) Digital Painting Workstation — Technical Documentation

This document provides a comprehensive system architecture reference and code design guide for the ARTY (Xcalux) digital painting workstation. It describes the subsystem components, mathematical models, rendering pipelines, data structures, and code design decisions implemented across the codebase.

---

## 1. System Architecture Overview

ARTY is a high-performance, hardware-accelerated desktop painting application designed to deliver zero-latency brush strokes and zero-allocation drawing loops. The system is split into four primary subsystems:

1. **User Interface Subsystem**: Powered by egui and eframe, providing a lightweight, low-overhead light-theme desktop layout with three panels (left tool sidebar, central canvas, right utility panel).
2. **Graphics & Composition Pipeline**: A low-level WGPU (WebGPU) rendering engine that updates CPU tile textures incrementally and composites layers in real-time on the GPU using a custom WGSL blending shader.
3. **Brush & Stroke Simulation Engine**: Powered by the Hokusai (libmypaint) library, which processes continuous tablet input (x, y, pressure, tilt) to paint onto tiled canvas surfaces with smooth dab interpolation.
4. **Input Handling & Stabilization Subsystem**: Integrates winit stylus events with an octotablet (RealTimeStylus/Windows Ink) fallback, using configurable EMA and Spring-Mass-Damper stabilizers to smooth raw hardware data.

```
+-----------------------------------------------------------------------+
|                         egui / eframe GUI                             |
+-------------------+--------------------+------------------------------+
                    |                    |
                    v                    v
+-------------------+---+      +---------+------------------------------+
| Input / octotablet    |      | WGPU Rendering Pipeline                |
|                       |      |                                        |
| +-------------------+ |      | +------------------------------------+ |
| | StrokeStabilizer  | |      | | incremental tile texture upload    | |
| +-------------------+ |      | +------------------+-----------------+ |
|                       |      |                    |                   |
|           |           |      |                    v                   |
|           v           |      | +------------------------------------+ |
|  inverse transform    |      | | compose_layers (mirror + rotate)   | |
|  coordinates (NDC)    |      | +------------------+-----------------+ |
|           |           |      |                    |                   |
+-----------+-----------+      +--------------------+-------------------+
            |                                       |
            v                                       v
+-----------+-----------+                  +--------+-------------------+
| Hokusai Brush Engine  |                  | target_egui_id (texture)   |
| (stroke_to on Layer)  |                  | blitted to Egui Viewport   |
+-----------------------+                  +----------------------------+
```

---

## 2. Codebase Structure and File Layout

- **src/main.rs**: Entry point. Handles `--stress-test` CLI flag, sets up DirectX 12 backend under Windows, initializes `env_logger`, and runs the eframe application loop.
- **src/app.rs**: Main application controller (`PaintApp` struct). Manages workspace state, brush preset arrays, color wheel drawing, user input dispatch, dialog boxes, and the three-panel egui layout.
- **src/renderer.rs**: WGPU renderer wrapper (`WgpuRenderer`). Handles GPU device management, viewport resizing, LRU tile texture cache, vertex/uniform buffer preparation, and multi-pass layer compositing via `compose_layers` and `compose_navigator`.
- **src/canvas.rs**: Models layers (`Layer`), tiles (`Tile`), selection masks, and blend modes (`BlendMode`). Tracks per-tile dirty flags and stores pixel data as fix15 premultiplied RGBA arrays.
- **src/input.rs**: Manages tablet coordinate polling via the octotablet COM implementation and houses the `StrokeStabilizer` with configurable EMA and Spring-Mass-Damper modes.
- **src/history.rs**: Implements `HistoryManager` with a pre-allocated object pool, supporting undo and redo without heap allocations during the active drawing path.
- **src/brush_io.rs**: Loads `.artybrush` preset files from disk and extracts brush textures from Clip Studio Paint `.sut` texture archives.
- **src/save.rs**: Background thread save/load logic for the `.arty` document format using a channel-based async pipeline.
- **src/stress_test.rs**: Performance verification harness that tracks stabilization latencies, LRU eviction ceilings, custom blend algebra, and allocation counters.

---

## 3. Core Subsystems and Technical Details

### A. Infinite Tiled Canvas and GPU LRU Cache

The canvas is modeled as a sparse, infinite grid of layers, where each layer contains a hash map of tiles:

- **Tile Dimensions**: Each tile is a $64 \times 64$ pixel square.
- **Pixel Format**: `[[u16; 4]; 64 * 64]` in fix15 premultiplied RGBA (range $0$–$32768$).
- **GPU Mapping**: `WgpuRenderer` maintains a fixed LRU cache of `MAX_TILE_SLOTS = 4096` GPU texture slots (each 64×64 `Rgba8Unorm`).
- **Incremental Upload**: Only dirty tiles are uploaded to the GPU each frame. When all 4096 slots are occupied, the least-recently-viewed slot is evicted and reassigned to the new coordinates.
- **Downsampling**: On upload, fix15 values are converted to 8-bit via the formula `(v * 255 + 16384) >> 15`.

### B. Dynamic Brush Preset System ("SAI Box")

Rather than hardcoding brush configurations, the workstation uses a dynamic `Vec<BrushPreset>`:

```rust
pub struct BrushPreset {
    pub id: u64,
    pub name: String,
    pub icon: PresetIcon,
    pub radius_log: f32,        // log-space radius; actual px = exp(radius_log)
    pub opacity: f32,
    pub hardness: f32,
    pub min_size_fraction: f32, // 0.0 = max thin-to-thick range; 1.0 = uniform width
    pub color_blending: f32,    // smudge amount
    pub dilution: f32,          // water/transparency amount
    pub texture_id: u8,         // 0=None, 1=Noise, 2=Bristle
    pub texture_scale: f32,
    pub bristle_id: u8,
    pub is_eraser: bool,
}
```

#### Dirty-Flag Caching

`PaintApp` holds a `brush_settings_dirty: bool` field. `sync_brush_settings()` **only runs when this flag is true**, avoiding per-frame Hokusai parameter rebuilds (~60× per second). The flag is set to `true` on:
- Any brush slider change (size, opacity, hardness, min size, blending, dilution)
- Color wheel or palette selection changes
- Texture or bristle ID changes
- `select_preset()` calls
- Keyboard shortcuts `[`, `]`, `E`

On completion, `sync_brush_settings()` clears the flag and rebuilds all pressure curves.

#### Dual Pressure Curve Rebuild

`sync_brush_settings()` rebuilds two independent pressure-mapped parameters on every dirty flush:

**1. Radius (thin-to-thick):**

The minimum size fraction $M$ controls the logarithmic offset at pressure $= 0$:
$$\text{offset}_{p=0} = \ln(M)$$

At $M = 1.0$ (100%), there is no size variation. At $M = 0.05$ (5%), thin-to-thick strokes span the full logarithmic range. The piecewise curve is:

| Pressure | Offset |
|----------|--------|
| 0.00 | $\ln(M)$ |
| 0.15 | $0.75 \cdot \ln(M)$ |
| 0.35 | $0.50 \cdot \ln(M)$ |
| 0.55 | $0.28 \cdot \ln(M)$ |
| 0.75 | $0.10 \cdot \ln(M)$ |
| 0.90 | $0.02 \cdot \ln(M)$ |
| 1.00 | $0$ |

**2. Opacity (translucency at light touch):**

The opacity floor at zero pressure is derived from the slider value:
$$\text{floor} = (1 - \text{opacity}) \times 0.55 + 0.05$$
$$\Delta_{p=0} = -\text{opacity} \times (1 - \min(\text{floor}, 0.90))$$

This ensures light touches produce translucent marks proportional to the opacity setting, while full pressure delivers maximum opacity regardless of slider position.

**3. OpaqueMultiply (S-curve):**

An S-shaped pressure multiplier is applied globally to avoid fully opaque marks at any pressure below ~0.60:

| Pressure | Multiplier |
|----------|-----------|
| 0.0 | 0.00 |
| 0.3 | 0.55 |
| 0.6 | 0.85 |
| 1.0 | 1.00 |

This S-curve is applied to all five startup presets (Pencil, Ink Pen, Paint Brush, Smudge, Eraser).

### C. Canvas Transformation and Input Mathematics

When horizontal mirroring or canvas rotation is applied, the GPU handles rendering transformations via vertex shaders, while the CPU performs inverse coordinate transformations to align drawing stylus events.

#### 1. Rendering (GPU Vertex Transformations)

Vertex positions are computed in Normalized Device Coordinates (NDC) relative to panning offsets and viewport zoom:

$$x_{\text{ndc}} = \frac{(x_{\text{world}} - x_{\text{offset}}) \cdot z}{W/2} - 1.0$$
$$y_{\text{ndc}} = 1.0 - \frac{(y_{\text{world}} - y_{\text{offset}}) \cdot z}{H/2}$$

The vertex shader applies:
- **Horizontal Mirroring**: $x'_{\text{ndc}} = -x_{\text{ndc}}$
- **Rotation** by angle $\theta$ around NDC origin:
  $$x''_{\text{ndc}} = x'_{\text{ndc}} \cos\theta - y_{\text{ndc}} \sin\theta$$
  $$y''_{\text{ndc}} = x'_{\text{ndc}} \sin\theta + y_{\text{ndc}} \cos\theta$$

#### 2. Input Tracking (Inverse Coordinates)

To translate screen-space pointer $(s_x, s_y)$ back to world coordinates $(w_x, w_y)$:

1. Translate to NDC relative to viewport center $(c_x, c_y)$:
   $$n_x = \frac{s_x - c_x}{W/2}, \quad n_y = -\frac{s_y - c_y}{H/2}$$
2. Apply inverse rotation by $-\theta$:
   $$px = n_x \cos\theta + n_y \sin\theta, \quad py = -n_x \sin\theta + n_y \cos\theta$$
3. Apply inverse mirroring: if mirrored, $px = -px$.
4. Convert back to world space:
   $$w_x = \frac{(px + 1.0) \cdot (W/2)}{\text{zoom}} + x_{\text{offset}}$$
   $$w_y = \frac{(1.0 - py) \cdot (H/2)}{\text{zoom}} + y_{\text{offset}}$$

#### 3. Viewport Panning

Middle/right-click panning deltas are similarly inverse-transformed before subtracting from `viewport_offset`, ensuring panning always follows the rotated and mirrored view axes naturally.

### D. WGPU Rendering Pipeline

#### Layer Compositing — `compose_layers()`

Layer compositing proceeds in 5 ordered passes within a single `CommandEncoder`:

1. **Folder Clear**: Clear the accumulator folder buffer to transparent.
2. **Recursive Compose** (`compose_recursive`): Iterates layers bottom-to-top. For each visible layer, copies the active accumulator into a swap buffer, then renders the layer's tiles via a blend render pass into the swap, then swaps active/swap indices. Folder layers recurse into child layers up to depth 3.
3. **Background Clear**: Clears `canvas_textures[0]` to the checkerboard-grey background color.
4. **Paper Quad**: Renders a white rectangle over the canvas area into `canvas_textures[0]` using `blend_mode=6` (solid white output, no texture sampling). **Important**: both texture bindings use `blank_view` to avoid a `RESOURCE + COLOR_TARGET` exclusive usage conflict.
5. **Final Combine**: Composites the artwork accumulator over the paper background into `canvas_textures[1]` using Normal blending. Copies result back to `canvas_textures[0]` via `copy_texture_to_texture`.

#### Navigator Compositing — `compose_navigator()`

The navigator is a fixed 256×256 thumbnail. To avoid the same `RESOURCE + COLOR_TARGET` conflict:

- A **`navigator_swap_texture`** (256×256) is maintained alongside `navigator_texture`.
- The **Paper Pass** renders solid white using `blank_view` for both texture bindings → writes to `navigator_view`.
- A `copy_texture_to_texture` copies `navigator_texture` → `navigator_swap_texture`.
- The **Art Pass** reads from `navigator_swap_view` (background) and `folder_views[final_artwork_idx]` (foreground) → writes to `navigator_view`.

#### Blending Shader (`blending.wgsl`)

The fragment shader (`fs_main`) supports 7 blend modes:

| Mode ID | Name | Formula |
|---------|------|---------|
| 0 | Normal | Standard premultiplied alpha over |
| 1 | Multiply | `dst.rgb * src.rgb` |
| 2 | Screen | `1 - (1-dst) * (1-src)` |
| 3 | Overlay | Conditional multiply/screen per channel |
| 4 | Luminosity (Shine) | `dst.rgb + src.rgb * src_alpha` |
| 5 | Shade | `dst.rgb * (1 - src.rgb * src_alpha * 0.5)` |
| 6 | Paper Canvas | Returns `vec4(1,1,1,1)` unconditionally |

Clipping group behavior: when `uniforms.clipping == 1`, `final_alpha = src_alpha * dst.a`, confining the layer's paint to opaque areas of the layer below.

### E. Input Stabilization

`StrokeStabilizer` in `src/input.rs` supports two modes, selectable per-session via the UI:

**EMA (Exponential Moving Average):**
$$\hat{x}_t = \alpha \cdot x_t + (1-\alpha) \cdot \hat{x}_{t-1}$$

Alpha is derived from the stabilizer level: higher levels → smaller alpha → more smoothing.

**Spring-Mass-Damper (Physics-based):**

Models the stylus tip as a mass on a spring, sub-stepped at 1ms intervals:
$$F = k(x_{\text{target}} - x) - c \cdot v$$
$$v_{t+\Delta t} = v_t + F \cdot \Delta t, \quad x_{t+\Delta t} = x_t + v_t \cdot \Delta t$$

S-Level modes (`S-1` through `S-5`) force the Spring-Mass-Damper mode with increasing inertia.

The stabilizer also smooths pressure and tilt channels simultaneously with the same coefficients, preventing pressure jitter from causing flickering opacity in strokes.

### F. Memory Management and Zero-Allocation Drawing Loop

The active stroke loop performs **zero heap allocations**:

- **Snapshot Buffers**: Modified tile pixel data is captured in `TileSnapshot` structs and pushed to the undo stack during a stroke.
- **Object Pool**: `HistoryManager` recycles `[u16; 16384]` buffers (one per tile snapshot). Overwritten undo history returns buffers to the pool for reuse.
- **Circular Buffers**: `StrokeStabilizer` uses `[f32; 128]` ring buffers for position/pressure history — no `Vec` growth.
- **Staging Buffer**: `WgpuRenderer.upload_staging_buffer` is a pre-allocated `Vec<u8>` of 16384 bytes reused for every tile upload, avoiding per-tile `vec![]` allocations.
- **Verified by stress test**: The `--stress-test` mode uses a tracking allocator to confirm exactly 0 heap allocations occur during the active drawing hot-path.

### G. Selection & Masking Subsystem

ARTY features a tiled, high-performance selection mask implementation:
- **Tiled Selection Mask**: The selection mask (`SelectionMask` struct) is structured identically to drawing layers, using sparse $64 \times 64$ byte tiles (`[u8; 4096]`) to represent selection opacity.
- **Selection Modes**: Supports `Replace`, `Add`, `Subtract`, and `Intersect` modes.
- **Deselection & Finalization**: The drag selection process is decoupled from the brush engine drawing cycles. Selection finalization occurs only when `!pointer_down` and `self.is_selecting` are true. If the selection dimensions are below $1.0 \times 1.0$ pixels (indicating a simple click), it is treated as a click-to-deselect event.
- **Selection Rendering**: Active selection pixels are rendered on the GPU utilizing a blue transparent fill. The visibility condition `val > 0` ensures that fully selected pixels ($val = 255$) as well as feathered edges are properly displayed.

### H. Color Selector & Eyedropper Subsystem

The color selection and eyedropper subsystems are calibrated for high-precision, low-friction painting operations:
- **HSV Color Wheel Dead Zone**: To avoid accidental color drift or jumps when selecting values near the square or ring borders, `zone_for_point` implements a 3px shrunken bounding area (`box_rect.shrink(3.0)`). If a coordinate falls inside the gap between the hue ring and the sat/val square, it resolves to a dead zone (Zone 0) and is ignored.
- **Continuous Eyedropper Sampling**: The color picker (eyedropper) supports real-time sampling during drag operations. Changing the activation check to `pointer_clicked || pointer_down` enables the user to drag the stylus over the canvas to preview colors continuously before committing.

### I. Custom Document Serializer & Deserializer (`.arty` Format)

To achieve maximum read/write performance, ARTY implements a custom binary file format (`.arty`) featuring DEFLATE compression and atomic renames:

1. **File Binary Layout Structure**:
   - **Magic Identifier** (4 bytes): `b"ARTY"`.
   - **Version** (4 bytes): `1u32` (little-endian).
   - **JSON Metadata Block Offset** (8 bytes): Pointer to document structure block.
   - **Tile Offset Directory Offset** (8 bytes): Pointer to directory index table.
   - **Compressed Tile Data Stream**: Array of DEFLATE-compressed `[u16; 4]` pixel tiles written sequentially.
   - **JSON Metadata Block**: UTF-8 JSON string containing canvas dimensions, layer order, blend modes, visibilities, opacity, lock alpha, folder hierarchies, and vector strokes.
   - **Tile Offset Directory Table**: An array of 24-byte entries defining the physical layout of all tiles:
     ```rust
     struct DirEntry {
         layer_id: u32,
         tx: i32,
         ty: i32,
         offset: u64,
         compressed_size: u32,
     }
     ```

2. **Atomic Safe Save Pipeline**:
   - Saves are executed asynchronously on a dedicated background worker loop (`save_worker_loop`) fed by a channel.
   - Writes first to a temporary file (`.tmp`). Upon successful write, the destination file is deleted and replaced using `std::fs::rename` to prevent corruption during system interruptions.

3. **Loader Validation Checks**:
   - Checks magic headers and validates offset coordinates against actual file size to prevent index out of bounds.
   - Restricts metadata size to 50MB and individual tile sizes to 1MB to guard against out-of-memory crashes on corrupted files.

### J. Keyboard Shortcuts & Rebinding Subsystem

Keyboard input events are mapped to commands decoupled from state representation:
- **Shortcut Entries mapping**: `ShortcutEntry` maps an abstract `CommandId` (e.g. `CommandId::Undo`) to a primary and secondary `KeyBinding` struct representing the physical key and modifier flags (`ctrl`, `shift`, `alt`).
- **Dynamic Rebinding context**: The rebinding dialog (`ShortcutEditor`) captures key press events by using egui's input queues on-the-fly. Key presses (ignoring Esc) are converted directly via `KeyBinding::from_event` and committed back to the `ShortcutManager` map.

### K. Transform & Interpolation Tool (`src/tools/transform.rs`)

The transformation tool performs arbitrary 2D spatial changes on layer tiles:
- **Affine Transformations Matrix**: Described by `[a, b, c, d, e, f]` representing the transformation equations:
  $$x' = a \cdot x + c \cdot y + e$$
  $$y' = b \cdot x + d \cdot y + f$$
- **Bounding Box Estimation**: Computes the coordinates of the 4 corners of the source content, projects them through the affine matrix, and determines the minimum and maximum tile coordinate bounds (`ttx0..=ttx1`, `tty0..=tty1`) to allocate destination tiles dynamically.
- **Inverse Coordinate Mapping**: To avoid holes/aliasing in the target canvas, it iterates target pixels and projects them back using the inverse determinant:
  $$\text{det} = a \cdot d - b \cdot c$$
  $$\text{inv\_det} = \frac{1}{\text{det}}$$
  $$s_x = \frac{d \cdot (d_x - e) - c \cdot (d_y - f)}{\text{det}}$$
  $$s_y = \frac{a \cdot (d_y - f) - b \cdot (d_x - e)}{\text{det}}$$
- **Interpolation Modes**:
  - **Nearest Neighbor**: Samples the coordinate directly using `div_euclid`/`rem_euclid` with bounds checking.
  - **Bilinear**: Samples the four neighboring pixels ($c_{00}, c_{10}, c_{01}, c_{11}$) and applies linear weights based on fractional offsets $f_x$ and $f_y$ to blend premultiplied RGBA channels.

### L. Scanline BFS Flood Fill Engine (`src/tools/fill.rs`)

The bucket fill tool uses a custom scanline-based Breadth-First Search (BFS) algorithm optimized for infinite sparse layers:
- **No-Allocation Visited Grid**: Instead of allocating a full canvas-sized boolean grid, visited status is stored in a sparse `HashMap<(i32, i32), [u64; 64]>`. Each entry represents a $64 \times 64$ tile with a 64-word bitmask. This allows lookup and marking in $O(1)$ time with zero heap allocation during search.
- **Scanline Limits Search**: For each coordinate popped from the search queue, the engine checks the left and right horizontal limits until it hits a boundary color difference, a locked selection boundary, or an already-visited pixel.
- **Fuzzy Tolerance Matching**: Color difference is evaluated across all channels using Manhattan distance:
  $$\text{dist} = |r_1 - r_2| + |g_1 - g_2| + |b_1 - b_2| + |a_1 - a_2|$$
  This distance is matched against the remapped tolerance: $\text{max\_diff} = \frac{\text{tolerance} \cdot 32768}{255}$.
- **Leak-Proof Deferred Committing**: Pixels are accumulated in a temporary vector during search and written to the canvas *only* after search is fully complete. This prevents the `expand_px` expansion pass from writing to tiles mid-scan and leaking through bounds.

---

## 4. Keyboard Shortcuts Reference

| Key | Action |
|-----|--------|
| `[` | Decrease brush radius (−0.15 log units) |
| `]` | Increase brush radius (+0.15 log units) |
| `E` | Toggle eraser mode on active preset |
| `H` | Toggle horizontal mirror |
| `Space + drag` | Pan canvas |
| `R + drag` | Rotate canvas |
| `Scroll wheel` | Zoom (centered on cursor) |
| `Ctrl+Z` | Undo |
| `Ctrl+Y` | Redo |

---

## 5. UI Layout Reference

The three-panel layout is built entirely from egui panels:

```
+--[Left Sidebar]-------+--[Central Canvas]--+--[Right Sidebar]------+
| Drawing Tools (presets)|  WGPU viewport     | Navigator (256×256)   |
| Stabilizer controls   |  Infinite canvas   | Color Selector (HSV)  |
| Brush Configuration   |  with rotation,    | Palette swatches      |
|   Size (px display)   |  mirroring, zoom   | Layer Manager         |
|   Brush preview circle|  and panning       |   Blend mode          |
|   Opacity             |                    |   Opacity             |
|   Hardness            |                    |   Lock Alpha          |
|   Min Size %          |                    |   Clipping Group      |
|   Blending / Dilution |                    |   Drag-to-reorder     |
|   Eraser [E]          |                    |                       |
|   Texture / Bristle   |                    |                       |
|   Debug (collapsed)   |                    |                       |
+------------------------+--------------------+------------------------+
|  [Bottom Status Bar: Tool name | Brush Size/Opacity | Pressure | Canvas dims | Zoom % | Rot ° | Mirror | Layer name | Autosave]  |
+--------------------------------------------------------------------------------------------------------------------------+
```

### Brush Configuration Panel Details

- **Brush preview circle**: A filled circle using the current brush color, sized by `exp(radius_log) * viewport_zoom` (clamped 3–60 px UI radius), giving an instant visual reference of the actual stroke width at current zoom.
- **Size label**: Displays actual pixel radius as `Size: X.X px` (computed as `exp(brush_radius_log)`).
- **Debug / Advanced Info** (collapsed by default): Shows raw pressure, smoothed pressure, remapped pressure values, and a live pressure bar. Also exposes the pressure response curve exponent and minimum pressure floor sliders.

---

## 6. Known Constraints and Design Decisions

- **Texture Usage Exclusivity**: WGPU (and DX12/Vulkan) forbids binding a texture as both `TEXTURE_BINDING` (shader resource) and `RENDER_ATTACHMENT` (color target) within the same render pass. All compositing passes use separate source and destination textures. Specifically: the Paper Quad pass uses `blank_view` for both texture bindings (since `blend_mode=6` ignores them), and the Navigator uses a dedicated `navigator_swap_texture` as a copy target before blending.
- **No `cd` to project root assumption**: The app loads `canvas.arty` and `brush.artybrush` relative to the working directory at launch.
- **Pressure curve startup**: Handcrafted per-preset pressure curves defined in `PaintApp::new()` are preserved on startup (`brush_settings_dirty = false`). The generic formula in `sync_brush_settings()` only activates when the user adjusts a slider or switches presets, at which point it overwrites the curves.
- **RealTimeStylus opt-in**: Native octotablet input (which uses COM/WM_POINTER) must be explicitly enabled via the `XCALUX_ENABLE_REALTIME_STYLUS=1` environment variable. By default, winit's `egui::Event::Touch::force` is used for pen pressure.
- **Shader model SM5**: The DX12 adapter reports Shader Model 5 (no SM6 wave intrinsics). All shaders must remain compatible with this level.
