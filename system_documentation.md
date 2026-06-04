# ARTY (Xcalux) Digital Painting Workstation — Technical Documentation

This document provides a comprehensive system architecture reference and code design guide for the ARTY (Xcalux) digital painting workstation. It describes the subsystem components, mathematical models, rendering pipelines, data structures, and code design decisions implemented across the codebase.

---

## 1. System Architecture Overview

ARTY is a high-performance, hardware-accelerated desktop painting application designed to deliver zero-latency brush strokes, smooth stabilization, and zero-allocation drawing loops. The system is split into four primary subsystems:

1. **User Interface Subsystem**: Powered by `egui` and `eframe`, providing a lightweight, low-overhead light-theme desktop layout. Features slide-and-hide left/right sidebars (toggled by the `Tab` key) to support minimal UI workflow.
2. **Graphics & Composition Pipeline**: A low-level `WGPU` (WebGPU) rendering engine that updates CPU tile textures incrementally and composites layers in real-time on the GPU using a custom WGSL blending shader, supporting clipping groups, folders, and viewport transformations.
3. **Brush & Stroke Simulation Engine**: Powered by the `Hokusai` (libmypaint) library, which processes continuous tablet input (x, y, pressure, tilt) to paint onto tiled canvas surfaces with smooth dab interpolation and dynamic presets.
4. **Input Handling & Stabilization Subsystem**: Integrates `winit` stylus events with an `octotablet` (RealTimeStylus/Windows Ink) COM-based fallback, utilizing configurable Exponential Moving Average (EMA) and Spring-Mass-Damper stabilizers to filter raw pointer data.

```
+-------------------------------------------------------------------------------+
|                                egui / eframe GUI                              |
+---------------------+---------------------+-----------------------------------+
                      |                     |
                      v                     v
+---------------------+-----+       +-------+-----------------------------------+
| Input / octotablet        |       | WGPU Rendering Pipeline                   |
|                           |       |                                           |
| +-----------------------+ |       | +---------------------------------------+ |
| | StrokeStabilizer      | |       | | incremental tile texture upload (LRU) | |
| +-----------------------+ |       | +-------------------+-------------------+ |
|                           |       |                     |                     |
|             |             |       |                     v                     |
|             v             |       | +---------------------------------------+ |
|    inverse transform      |       | | compose_recursive (folder hierarchies)| |
|    coordinates (NDC)      |       | +-------------------+-------------------+ |
|             |             |       |                     |                     |
|             v             |       |                     v                     |
+-------------+-------------+       | +---------------------------------------+ |
              |                     | | compose_layers (mirror + rotate)      | |
              v                     | +-------------------+-------------------+ |
+-------------+-------------+       |                     |                     |
| Hokusai Brush Engine      |       |                     v                     |
| (stroke_to on Layer)      |       | +---------------------------------------+ |
|                           |       | | target_egui_id (egui texture handle)  | |
+---------------------------+       +---------------------+---------------------+
                                                          |
                                                          v
                                            +-------------+-------------+
                                            | Blitted to Egui Viewport  |
                                            +---------------------------+
```

---

## 2. Codebase Structure and File Layout

* **[src/main.rs](file:///d:/project/ARTY/src/main.rs)**: The application entry point. Handles parsing the `--stress-test` CLI flag, sets up the DirectX 12 backend under Windows, configures loggers, and initializes the `eframe` window environment.
* **[src/app.rs](file:///d:/project/ARTY/src/app.rs)**: The main application controller hosting the `PaintApp` struct. Manages workspace configuration, preset databases, HSV color wheel interactions, color history buffers, user input dispatch, open/save dialogs, and slide-in panels.
* **[src/commands.rs](file:///d:/project/ARTY/src/commands.rs)**: Defines the `CommandId` enum used to route all commands across the application menus, quick bars, and keyboard shortcut managers.
* **[src/shortcuts.rs](file:///d:/project/ARTY/src/shortcuts.rs)**: Implements `KeyBinding` and `ShortcutManager` structs, mapping key combinations to `CommandId` values and driving the keyboard rebinding settings window.
* **[src/renderer.rs](file:///d:/project/ARTY/src/renderer.rs)**: A thin, high-performance wrapper around `WGPU` state (`WgpuRenderer`). Manages the GPU device context, viewport resizing, quad vertex buffers, the 4096-slot tile texture cache, folder compositing texture pools, and off-screen canvas composition.
* **[src/canvas.rs](file:///d:/project/ARTY/src/canvas.rs)**: Core models for layers (`Layer`), tiles (`Tile`), selection masks (`SelectionMask`), vector strokes, and blend mode enums. Implements dirty-tile set collection and thumbnail generation downscaling.
* **[src/history.rs](file:///d:/project/ARTY/src/history.rs)**: Implements standard undo/redo states through the `HistoryManager` and `ObjectPool`, capturing modified tile states as pre-allocated heap blocks without dynamic memory allocations during the stroke drawing loop.
* **[src/input.rs](file:///d:/project/ARTY/src/input.rs)**: Handles tablet events via `octotablet` and houses `StrokeStabilizer` which implements EMA and Spring-Mass-Damper stabilizers with sub-stepped numerical integration.
* **[src/brush_io.rs](file:///d:/project/ARTY/src/brush_io.rs)**: Encapsulates decoding of `.artybrush` preset files and parses Clip Studio Paint `.sut` tool files to extract embedded PNG textures.
* **[src/save.rs](file:///d:/project/ARTY/src/save.rs)**: Background thread save and load procedures for the custom binary `.arty` format, implementing atomic renames and safety size checks.
* **[src/stress_test.rs](file:///d:/project/ARTY/src/stress_test.rs)**: Testing harness validating system stabilization latency, LRU evictions, memory allocations, and performance ceilings under high input density.
* **[src/tools/fill.rs](file:///d:/project/ARTY/src/tools/fill.rs)**: Implements the scanline flood fill (Bucket tool) using a sparseVisited lookup bitmask, tolerance calculations, area expansion, anti-aliased edge softening, and alpha compositing.
* **[src/tools/selection.rs](file:///d:/project/ARTY/src/tools/selection.rs)**: Geometric shape selections (Rectangle, Ellipse, Lasso) and the Magic Wand selection algorithm (leveraging shared fuzzy color comparison with contiguous and non-contiguous modes).
* **[src/tools/transform.rs](file:///d:/project/ARTY/src/tools/transform.rs)**: Implements affine transformations (translation, rotation, scaling) on layers and selections with Nearest Neighbor and Bilinear interpolation.
* **[src/export/png.rs](file:///d:/project/ARTY/src/export/png.rs)**: Exports layers or the flattened canvas to PNG files, converting fix15 premultiplied pixels back to standard RGBA8 bytes.

---

## 3. Core Subsystems and Technical Details

### A. Infinite Tiled Canvas and GPU LRU Cache

The canvas represents a sparse, unbounded coordinate grid. Rather than allocating fixed-resolution arrays, layers organize pixel data inside a hash map of tiles:

* **Tile Dimensions**: Tiles are $64 \times 64$ pixels.
* **Pixel Format**: Pixels are stored as `[[u16; 4]; 64 * 64]` representing fix15 premultiplied RGBA. Values range from $0$ (transparent/black) to $32768$ (fully opaque/white). Premultiplication ensures correct linear alpha blending:
  $$C_{\text{premult}} = C_{\text{color}} \times \frac{A}{32768}$$
* **GPU Mapping**: `WgpuRenderer` maintains a cache of `MAX_TILE_SLOTS = 4096` GPU textures (each a $64 \times 64$ `Rgba8Unorm` texture).
* **Cache Management**: The system maps a unique key `(layer_id, tx, ty)` to a slot index in the texture registry. When the cache is full, a Least-Recently-Used (LRU) policy evicts the oldest tile from the GPU registry and replaces it with the newly requested tile.
* **Texture Downsampling**: During CPU-to-GPU tile upload, u16 premultiplied fix15 pixels are downsampled to 8-bit integers:
  $$P_8 = \frac{P_{16} \times 255 + 16384}{32768} = (P_{16} \times 255 + 16384) \gg 15$$

---

### B. Dynamic Brush Preset System ("SAI Box")

ARTY defines the active drawing profile using a `BrushPreset` struct containing settings for size, opacity, hardness, color blending, dilution, spacing, density, and stabilization levels:

```rust
pub struct BrushPreset {
    pub id: u64,
    pub name: String,
    pub icon: PresetIcon,
    pub radius_log: f32,        // Logarithmic scale; radius in px = exp(radius_log)
    pub opacity: f32,           // Max opacity [0.0, 1.0]
    pub hardness: f32,          // Brush edge hardness [0.0, 1.0]
    pub min_size_fraction: f32, // Min size on light pressure (e.g. 0.05 = 5%)
    pub color_blending: f32,    // Smudge / paint mixing coefficient
    pub dilution: f32,          // Water / transparency blending coefficient
    pub texture_id: u8,         // 0 = None, 1 = Noise, 2 = Bristle, etc.
    pub texture_scale: f32,
    pub bristle_id: u8,
    pub is_eraser: bool,
    pub stabilizer_level: StabilizerLevel,
    pub stabilizer_mode: StabilizerMode,
    pub spacing: f32,           // Dab spacing percentage relative to radius
    pub density: f32,           // Global opacity modifier
}
```

#### Dirty-Flag Caching
To prevent recreating brush properties, coordinate structures, and pressure lookup tables on every frame (60 times per second), the application tracks settings changes with a `brush_settings_dirty` flag. The system flushes settings to the `Hokusai` brush instance **only when this flag is true**.

The flag is marked dirty on:
* Brush radius adjustments (keys `[` and `]`).
* Slider manipulations (Opacity, Hardness, Min Size, Blending, Dilution, Density, Spacing).
* Color wheel selections.
* Preset switches.
* Eraser toggling (key `E`).

#### Dual Pressure Curve Rebuild
During a dirty flush, the system computes two independent pressure curves to shape brush responses:

1. **Radius (Thin-to-Thick)**:
   The minimum size fraction $M$ defines the logarithmic size offset at zero pressure:
   $$\text{offset}_{p=0} = \ln(M)$$
   If $M = 1.0$ (uniform width), the offset remains $0.0$. For variable-width brushes, the radius scale is mapped from pressure $p \in [0, 1]$ using a piecewise curve:
   $$\text{scale}(p) = \exp(\text{offset}_{p=0} \times \text{curve}(p))$$
   
   | Pressure ($p$) | Curve Factor ($\text{curve}(p)$) |
   |---|---|
   | $0.00$ | $1.00$ |
   | $0.15$ | $0.75$ |
   | $0.35$ | $0.50$ |
   | $0.55$ | $0.28$ |
   | $0.75$ | $0.10$ |
   | $0.90$ | $0.02$ |
   | $1.00$ | $0.00$ |

2. **Opacity (Light-to-Heavy Touch)**:
   The base opacity floor at zero pressure is determined by the preset's slider value:
   $$\text{floor} = (1.0 - \text{opacity}) \times 0.55 + 0.05$$
   $$\Delta_{p=0} = -\text{opacity} \times (1.0 - \min(\text{floor}, 0.90))$$
   Light touches generate translucent strokes matching the opacity setting, while full pressure delivers the maximum preset opacity.

3. **Global Pressure S-Curve (`OpaqueMultiply`)**:
   To prevent brushes from becoming fully opaque under light pressure, an S-curve multiplier modulates final opacity:
   
   | Pressure ($p$) | Multiplier |
   |---|---|
   | $0.0$ | $0.00$ |
   | $0.3$ | $0.55$ |
   | $0.6$ | $0.85$ |
   | $1.0$ | $1.00$ |

---

### C. Viewport Navigation and Input Coordinate Math

When canvas transformations (zoom, pan, rotation, horizontal mirroring) are active, WGPU applies these transformations in vertex shaders for rendering, while the CPU performs inverse coordinate transformations to map stylus events back to canvas world coordinates.

```
       World Coordinates (Canvas Space)
                    |
                    v (CPU forward: zoom, pan, rotate, mirror)
       Normalized Device Coordinates (NDC)
                    |
                    v (GPU viewport transform)
       Screen Coordinates (Pixel Space)
```

#### 1. Rendering (GPU Vertex Transformations)
Vertex coordinates are mapped into Normalized Device Coordinates (NDC) based on panning offsets, scale factor, and screen dimensions:
$$x_{\text{ndc}} = \frac{(x_{\text{world}} - x_{\text{offset}}) \times z}{W / 2} - 1.0$$
$$y_{\text{ndc}} = 1.0 - \frac{(y_{\text{world}} - y_{\text{offset}}) \times z}{H / 2}$$

The vertex shader applies:
* **Horizontal Mirroring**:
  $$x'_{\text{ndc}} = \begin{cases} -x_{\text{ndc}} & \text{if mirrored} \\ x_{\text{ndc}} & \text{otherwise} \end{cases}$$
* **Rotation** by angle $\theta$ around the viewport center:
  $$x''_{\text{ndc}} = x'_{\text{ndc}} \cos\theta - y_{\text{ndc}} \sin\theta$$
  $$y''_{\text{ndc}} = x'_{\text{ndc}} \sin\theta + y_{\text{ndc}} \cos\theta$$

#### 2. Input Tracking (Inverse Coordinate Projection)
To translate a screen pointer event at $(s_x, s_y)$ back to canvas space $(w_x, w_y)$:
1. Map pointer to NDC relative to viewport center $(c_x, c_y)$ and dimensions $(W, H)$:
   $$n_x = \frac{s_x - c_x}{W / 2}, \quad n_y = -\frac{s_y - c_y}{H / 2}$$
2. Apply inverse rotation by $-\theta$:
   $$p_x = n_x \cos\theta + n_y \sin\theta$$
   $$p_y = -n_x \sin\theta + n_y \cos\theta$$
3. Apply inverse horizontal mirroring:
   $$p'_x = \begin{cases} -p_x & \text{if mirrored} \\ p_x & \text{otherwise} \end{cases}$$
4. Project back to world coordinates:
   $$w_x = \frac{(p'_x + 1.0) \times (W / 2)}{\text{zoom}} + x_{\text{offset}}$$
   $$w_y = \frac{(1.0 - p_y) \times (H / 2)}{\text{zoom}} + y_{\text{offset}}$$

---

### D. WGPU Rendering & Layer Compositing Pipeline

WGPU does not allow binding a texture as both a shader resource (`TEXTURE_BINDING`) and a color target (`RENDER_ATTACHMENT`) in the same render pass to avoid race conditions. ARTY implements double-buffered compositing targets and recursive folder layouts to manage complex hierarchies.

#### 1. Layer Compositing — `compose_layers()`
The layer stack is composited bottom-to-top across five stages inside a single command encoder:
1. **Folder Clearing**: Resets folder accumulation buffers to transparent (`wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)`).
2. **Recursive Compositing** (`compose_recursive`): Traverses layers from bottom to top. For each active layer, the system copies the current folder accumulator texture into a temporary swap buffer, binds it as a shader resource alongside the layer's tile texture, blends them in a render pass, and writes the output back to the active accumulator. Folder layers recursively execute this process up to a depth of 3.
3. **Background Clearing**: Clears `canvas_textures[0]` to the canvas checkerboard grey color.
4. **Paper Quad Pass**: Renders a solid white quad matching the canvas dimensions onto `canvas_textures[0]`. This pass uses `blend_mode = 6` in the blending shader, which returns `vec4(1.0)` immediately. By binding a placeholder `blank_view` texture, the system avoids texture usage conflicts.
5. **Final Combine**: Blends the composite folder accumulator texture over the white paper background on `canvas_textures[1]` using Normal blending. A final `copy_texture_to_texture` command writes this composite back to `canvas_textures[0]` for display in the egui viewport.

#### 2. Navigator Compositing — `compose_navigator()`
The navigator displays a $256 \times 256$ pixel thumbnail of the canvas. To compile this view:
1. The paper quad pass renders the canvas aspect-correct white bounding sheet onto `navigator_view`.
2. A `copy_texture_to_texture` copies the background to a dedicated `navigator_swap_texture`.
3. The art pass binds the `navigator_swap_view` (background) and the final canvas composition texture as inputs, rendering the scaled artwork on top of the paper background.

#### 3. Blending Shader Algebra (`blending.wgsl`)
The blending shader supports six composite modes alongside a paper generation mode:

| ID | Mode Name | Formula (Premultiplied RGBA) |
|---|---|---|
| **0** | **Normal** | $C_{\text{out}} = C_{\text{src}} + C_{\text{dst}} \times (1.0 - A_{\text{src}})$ |
| **1** | **Multiply** | $C_{\text{out}} = C_{\text{dst}} \times C_{\text{src}} \times A_{\text{src}} + C_{\text{dst}} \times (1.0 - A_{\text{src}})$ |
| **2** | **Screen** | $C_{\text{out}} = (C_{\text{dst}} + C_{\text{src}} - C_{\text{dst}} \times C_{\text{src}}) \times A_{\text{src}} + C_{\text{dst}} \times (1.0 - A_{\text{src}})$ |
| **3** | **Overlay** | Channel-wise $c \in \{r,g,b\}$: if $C_{\text{dst}, c} < 0.5 \times A_{\text{dst}}$:<br>$C_{\text{out}, c} = 2.0 \times C_{\text{dst}, c} \times C_{\text{src}, c} / A_{\text{dst}}$<br>otherwise:<br>$C_{\text{out}, c} = A_{\text{dst}} - 2.0 \times (A_{\text{dst}} - C_{\text{dst}, c}) \times (A_{\text{src}} - C_{\text{src}, c}) / A_{\text{dst}}$ |
| **4** | **Luminosity** | $C_{\text{out}} = C_{\text{dst}} + C_{\text{src}} \times A_{\text{src}}$ |
| **5** | **Shade** | $C_{\text{out}} = C_{\text{dst}} \times (1.0 - C_{\text{src}} \times A_{\text{src}} \times 0.5)$ |
| **6** | **Paper** | Returns `vec4<f32>(1.0, 1.0, 1.0, 1.0)` unconditionally |

**Clipping Groups**: When a layer has `clipping = 1` enabled, its opacity is scaled by the destination alpha channel:
$$A_{\text{out}} = A_{\text{src}} \times A_{\text{dst}}$$
This restricts drawing to the boundaries of the layer directly beneath it.

---

### E. Tablet Input and Stabilization

Stylus pointer events are read from window events or via the `octotablet` COM wrapper (for Windows Ink). Coordinate streams are filtered by the `StrokeStabilizer` to remove jitter.

```
       Raw Stylus Coordinates (x, y, pressure, tilt)
                         |
                         v
       Circular Ring Buffer Smoothing (Window Size)
                         |
                         v
          Selected Stabilization Mode:
          - EMA (Exponential Moving Average)
          - Spring-Mass-Damper (Physics Model)
                         |
                         v
       Stabilized Output Coordinates (x, y, pressure, tilt)
```

#### 1. EMA (Exponential Moving Average) Mode
Positions are smoothed using a weighted average of the current value and the previous step:
$$\hat{X}_t = \alpha X_t + (1.0 - \alpha) \hat{X}_{t-1}$$
The smoothing coefficient $\alpha$ is derived from the stabilizer level:
$$\alpha = \frac{1.0}{\text{level} \times 0.4 + 1.0}$$

#### 2. Spring-Mass-Damper Physics Mode
Stylus coordinates are mapped as a virtual mass connected to the physical pen tip by a spring. Jitter is filtered out using a sub-stepped Euler integration loop (16 steps per frame):
$$F_s = k \times (X_{\text{target}} - X_{\text{virtual}})$$
$$F_d = -c \times V_{\text{virtual}}$$
$$A = \frac{F_s + F_d}{m}$$
$$V_{\text{virtual}} \leftarrow V_{\text{virtual}} + A \times \Delta t$$
$$X_{\text{virtual}} \leftarrow X_{\text{virtual}} + V_{\text{virtual}} \times \Delta t$$

* **Level 1–15**: Configured with a strong spring and moderate damping:
  $$k = \frac{300.0}{\text{level}^{1.1}}, \quad c = 12.0 + \text{level} \times 0.6, \quad m = 1.0 + \text{level} \times 0.08$$
* **S-1 to S-5**: High-inertia modes designed for smooth, sweeping lines:
  $$k = \frac{15.0}{\text{level}}, \quad c = 20.0 + \text{level} \times 3.0, \quad m = 2.5 + \text{level} \times 0.8$$

To avoid pressure jitter, the pressure and tilt channels are smoothed using EMA alongside coordinates.

---

### F. Memory Management and Zero-Allocation Drawing Loop

To prevent garbage collection pauses and frame drops, the stroke drawing path does not perform heap allocations:

* **Pre-allocated Tile Pool**: The `HistoryManager` maintains an `ObjectPool` containing recycled `TilePixels` buffers (`Box<[u16; 16384]>`).
* **Tile Snapshot Buffers**: When a brush stroke edits a tile, the system pulls an empty buffer from the pool, copies the tile's current pixels into it, and pushes the snapshot to the undo stack. Evicted undo commands return their buffers to the pool.
* **Pre-allocated Staging Buffer**: `WgpuRenderer` uses a fixed-size `Vec<u8>` buffer (16KB) to upload tile data to the GPU without allocating memory per tile.
* **Fixed Ring Buffers**: Stabilizers store coordinate and pressure history in static arrays (`[f32; 128]`) managed by ring buffer indices.

---

### G. Selection & Masking Subsystem

ARTY manages selections through a coordinate-aligned mask layer:

* **Tiled Selection Mask**: The selection mask (`SelectionMask`) is structured identically to raster layers, storing selection values as sparse tiles of `[u8; 4096]` ($64 \times 64$ pixels). Opaque pixels ($255$) denote selected regions, while $0$ denotes unselected regions.
* **Selection Modes**:
  * **Replace**: Clears the mask and writes the new selection.
  * **Add**: Performs a saturating add on mask values:
    $$\text{val}_{\text{new}} = \min(\text{val}_{\text{current}} + \text{val}_{\text{input}}, 255)$$
  * **Subtract**: Subtracts the new selection from the mask:
    $$\text{val}_{\text{new}} = \text{val}_{\text{current}}.\text{saturating\_sub}(\text{val}_{\text{input}})$$
  * **Intersect**: Retains the minimum value between the current mask and the new selection:
    $$\text{val}_{\text{new}} = \min(\text{val}_{\text{current}}, \text{val}_{\text{input}})$$
* **Point-in-Polygon Test**: Lasso selections verify pixel coordinates using a ray-casting intersection loop:
  ```rust
  fn point_in_polygon(px: f32, py: f32, polygon: &[(f32, f32)]) -> bool {
      let mut inside = false;
      let mut j = polygon.len() - 1;
      for i in 0..polygon.len() {
          let (xi, yi) = polygon[i];
          let (xj, yj) = polygon[j];
          if ((yi > py) != (yj > py)) && (px < (xj - xi) * (py - yi) / (yj - yi) + xi) {
              inside = !inside;
          }
          j = i;
      }
      inside
  }
  ```
* **Magic Wand Selection**: Scans the canvas starting at a coordinate $(x,y)$ to select connected areas based on color similarity.
  * **Contiguous Search**: Performs a scanline BFS. It matches pixel colors against the starting color using Manhattan distance:
    $$\text{dist} = |r_1 - r_2| + |g_1 - g_2| + |b_1 - b_2| + |a_1 - a_2|$$
    The tolerance limit is scaled into fix15 range:
    $$\text{limit} = \frac{\text{tolerance} \times 32768}{255} \times 4$$
  * **Non-Contiguous Search**: Evaluates the color difference formula for all pixels within the canvas bounds.
  * **Area Expansion**: Applies an expansion pass to grow the selection boundary by `expand_px`.
* **Selection Modification**:
  * **Grow**: Expands the mask using a bounding radius filter.
  * **Shrink**: Erodes the mask using a local minimum filter.
  * **Feather**: Blurs the mask using a box blur filter of radius `feather_px`.

---

### H. Color Selector & Eyedropper Subsystem

The color picker and HSV color wheel are calibrated to prevent accidental input drift:

* **HSV Color Wheel Dead Zones**: The HSV color wheel renders a hue ring enclosing a saturation-value square. To prevent coordinate jumps when dragging near borders, a 3px dead zone separates the ring and the square. Pointer events inside this dead zone are ignored.
* **Continuous Eyedropper**: Eyedropper sampling runs continuously while dragging. The system samples colors from the active target (Current Layer, Reference Layers, or All Visible Layers) and adds them to a 12-swatch color history buffer.

---

### I. Custom Document Serializer & Deserializer (`.arty` Format)

To speed up reading and writing large canvases, ARTY implements a custom binary file format (`.arty`) featuring DEFLATE compression and atomic updates.

#### 1. Binary Layout Structure
```
+-----------------------------------------------------------------------------+
| Magic (4B) | Version (4B) | JSON Offset (8B) | Tile Directory Offset (8B)   |
+-----------------------------------------------------------------------------+
| Compressed Tile Data Stream (DEFLATE-compressed fix15 pixels)              |
+-----------------------------------------------------------------------------+
| JSON Metadata (Canvas dims, layers, blend modes, vector curves)             |
+-----------------------------------------------------------------------------+
| Tile Offset Directory Table (Entries matching layers and coordinates)       |
+-----------------------------------------------------------------------------+
```

The Tile Directory contains 24-byte entries defining the location and size of each compressed tile:
```rust
struct DirEntry {
    layer_id: u32,
    tx: i32,
    ty: i32,
    offset: u64,
    compressed_size: u32,
}
```

#### 2. Atomic Safe Save Pipeline
1. The canvas state is sent to a background worker thread (`save_worker_loop`) using a channel.
2. The worker writes data to a temporary file (`.tmp`).
3. If writing is successful, the worker deletes the old file and renames the `.tmp` file to the destination path using `std::fs::rename`.

#### 3. Loader Verification Checks
* Validates magic headers and offsets against the physical file size.
* Restricts metadata blocks to 50MB and individual tiles to 1MB to prevent out-of-memory errors on corrupted files.

---

### J. Keyboard Shortcuts & Rebinding Subsystem

The application maps keyboard events to abstract commands, decoupling input logic from drawing operations:

* **Shortcut Mapping**: `ShortcutEntry` maps a `CommandId` (e.g. `CommandId::Undo`) to primary and secondary `KeyBinding` configurations.
* **Rebinding**: The settings window captures key presses by reading egui's input queue. Modifier keys are detected and written to the selected command's primary or secondary binding slot.

---

### K. Transform & Interpolation Engine

The transform tool translates, rotates, and scales layer and selection regions:

* **Affine Matrices**: Spatial transformations are modeled using a 2D affine matrix:
  $$\begin{bmatrix} x' \\ y' \end{bmatrix} = \begin{bmatrix} a & c \\ b & d \end{bmatrix} \begin{bmatrix} x \\ y \end{bmatrix} + \begin{bmatrix} e \\ f \end{bmatrix}$$
* **Bounding Box Calculation**: Projects the corners of the source content through the matrix to estimate the destination bounds (`ttx0..=ttx1`, `tty0..=tty1`) and allocate tiles on the target canvas.
* **Inverse Coordinate Mapping**: To prevent holes and aliasing, the engine loops over target pixels and projects them back to source coordinates:
  $$\text{det} = ad - bc$$
  $$s_x = \frac{d(x' - e) - c(y' - f)}{\text{det}}$$
  $$s_y = \frac{a(y' - f) - b(x' - e)}{\text{det}}$$
* **Interpolation Modes**:
  * **Nearest Neighbor**: Rounds coordinate values to sample the nearest pixel.
  * **Bilinear**: Samples the four neighboring pixels ($C_{00}, C_{10}, C_{01}, C_{11}$) and blends their color channels using fractional weights:
    $$f_x = s_x - \lfloor s_x \rfloor, \quad f_y = s_y - \lfloor s_y \rfloor$$
    $$C = C_{00}(1 - f_x)(1 - f_y) + C_{10}f_x(1 - f_y) + C_{01}(1 - f_x)f_y + C_{11}f_x f_y$$

---

### L. BFS Flood Fill Engine (Bucket Tool)

The bucket tool features a scanline flood fill algorithm optimized for infinite tiled canvases:

* **Sparse Visited Matrix**: Rather than allocating a full-canvas boolean grid, visited states are tracked in a hash map of bitmasks (`HashMap<(i32, i32), [u64; 64]>`). Each entry represents a $64 \times 64$ tile using 64 64-bit words, allowing $O(1)$ lookups and insertions with zero allocations during search.
* **Fuzzy Tolerance**: Evaluations check the color distance against the tolerance limit:
  $$\text{dist} = |r_1 - r_2| + |g_1 - g_2| + |b_1 - b_2| + |a_1 - a_2|$$
  $$\text{dist} \le \text{limit} \quad \text{where} \quad \text{limit} = 4 \times \frac{\text{tolerance} \times 32768}{255}$$
* **Anti-Aliasing Edge Softening**: Remaps boundary opacities near tolerance thresholds.
  We define a core tolerance limit at $80\%$ of the threshold:
  $$\text{limit}_{\text{core}} = 0.8 \times \text{limit}$$
  If $\text{dist} \le \text{limit}_{\text{core}}$, the fill opacity weight is $1.0$.
  If $\text{limit}_{\text{core}} < \text{dist} \le \text{limit}$, the weight is interpolated linearly:
  $$\text{weight} = \frac{\text{limit} - \text{dist}}{\text{limit} - \text{limit}_{\text{core}}}$$
  Fills blend smoothly into background colors.
* **Deferred Committing**: Pixel coordinates and blending weights are stored in a temporary vector during search and written to the canvas only after search is complete, preventing pixel bleeding.
* **Reference Sources**: Supports filling from the Active Layer, Selection Source Layers (flagged with `◎`), or All Visible Layers. If no layers are flagged with `◎`, the UI displays a warning banner.

---

## 4. Keyboard Shortcuts Reference

| Key Combination | Action |
|---|---|
| `[` | Decrease brush size (moves $-0.15$ log units) |
| `]` | Increase brush size (moves $+0.15$ log units) |
| `E` | Toggle eraser mode on active brush preset |
| `H` | Toggle horizontal mirror view |
| `Space` + Drag | Pan viewport |
| `R` + Drag | Rotate viewport |
| `Scroll Wheel` | Zoom viewport (centered on cursor) |
| `Ctrl` + `Z` | Undo |
| `Ctrl` + `Y` / `Ctrl` + `Shift` + `Z` | Redo |
| `Ctrl` + `N` | New canvas |
| `Ctrl` + `O` | Open `.arty` document |
| `Ctrl` + `S` | Save `.arty` document |
| `Ctrl` + `Shift` + `S` | Save document As... |
| `Ctrl` + `D` | Deselect |
| `Ctrl` + `A` | Select all |
| `Ctrl` + `I` | Invert selection |
| `Ctrl` + `T` | Transform layer/selection |
| `Tab` | Toggle minimal UI mode (slides sidebars out of view) |
| `Backspace` / `Delete` | Clear active selection |
| `Alt` + `Backspace` | Fill active selection |

---

## 5. UI Layout Reference

The user interface uses a three-panel layout built with `egui` and `eframe`:

```
+--[Left Sidebar]-------+--[Central Canvas]--+--[Right Sidebar]------+
| Drawing Tools Grid    |  WGPU Viewport     | Navigator Viewport   |
| Tool Options Panel    |  Infinite Canvas   |   Rotated red bounds |
| Dynamic Brush Panel   |                    | Color Wheel (HSV)    |
|   Brush preview box   |                    | Color History        |
|   Radius slider       |                    | Palette Swatches     |
|   Opacity slider      |                    | Layer List           |
|   Hardness slider     |                    |   Vis (👁/⦂) buttons  |
|   Min size slider     |                    |   Ref (◎/⚬) buttons  |
|   Dilution slider     |                    |   Blend modes        |
|                       |                    |   Drag-to-reorder    |
+-----------------------+--------------------+----------------------+
| [Bottom Status Bar: Active tool | Brush details | Zoom | Rotation | Layer | Autosave status] |
+-----------------------------------------------------------------------------------------------+
```

### Sub-Panel Configuration
1. **Left Sidebar (Width: 160px)**:
   * **Tools Grid**: Compact emoji-free layout containing tools (Brush, Eraser, Fill, Rect Select, Lasso, Wand, Move, Transform, Color Picker, Zoom).
   * **Tool Options Panel**: Displays parameters for the active tool (e.g., tolerance and expansion for Bucket/Wand).
   * **Brush Config**: Active size slider, concentric falloff brush preview box, hardness, blending, and texture mapping.
2. **Central Viewport**:
   * Displays the hardware-accelerated canvas. Renders viewport rotated overlays, grid settings, and selection overlays.
3. **Right Sidebar (Width: 260px)**:
   * **Navigator**: $256 \times 256$ view containing a red bounding outline that rotates and mirrors to match the main viewport.
   * **Color Panel**: HSV wheel with dead-zone locking, a 12-slot color history grid, and custom swatches.
   * **Layer Manager**: Scrolling panel supporting visibility toggles (`👁`/`⦂`), reference layer flags (`◎`/`⚬`), opacity sliders, blend dropdowns, and drag-and-drop layer reordering.

---

## 6. Known Constraints and Design Decisions

* **WGPU Texture Multi-Bind Limits**: WGPU prevents binding textures as shader inputs and output targets in the same render pass. The Paper Quad compositing pass uses `blank_view` for input textures (as output color is solid white), and the Navigator uses `navigator_swap_texture` as a intermediate copy step before rendering.
* **DirectX 12 Shader Model 5 Compatibility**: Under Windows, the adapter runs Shader Model 5. All WGSL shaders avoid Shader Model 6 instructions (such as wave intrinsics) to maintain compatibility.
* **Stylus Fallback Env-Var**: Native RealTimeStylus pointer coordinate pumping is enabled by setting `XCALUX_ENABLE_REALTIME_STYLUS=1`. If this variable is not set, the application falls back to `winit` pointer and force inputs.
* **Zero-Allocation Hot-Path Ceiling**: The object pool caps recycled tile pixels at 512 tiles (16MB) to limit memory footprint while keeping active brush strokes allocation-free.
