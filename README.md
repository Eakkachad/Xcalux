# ARTY (Xcalux) v2

A manga and coloring app in the spirit of Clip Studio Paint, with SAI-quality
brushes. Written in Rust (egui 0.36 + wgpu 30), designed to be fast and light.

## Running

```powershell
cargo run --release          # ARTY v2
$env:ARTY_DEMO=1; cargo run  # opens with sample strokes from every preset (smoke test)
cargo test --workspace       # all tests, including the zero-allocation gates
cargo run -p arty-core --release --example bench_composite   # composite benchmark
```

The old version (egui 0.27) is in `legacy/` and still runs: `cd legacy; cargo run --release`
(it uses the vendored crates in `vendor/`).

## Layout

| Crate | Purpose |
|---|---|
| `crates/arty-core` | Document model: 64×64 fix15 tiles (Arc copy-on-write), layer tree (folder/clip/blend), per-tile compositing, undo |
| `crates/arty-brush` | Pen input → stabilizer → hokusai (libmypaint) → tiles; presets for G-pen, Mapping pen, Pencil, Brush, Watercolor, Airbrush, Eraser |
| `crates/arty-render` | View transform (zoom/rotate/flip), CPU composite of dirty tiles → GPU texture array with mipmaps |
| `crates/arty-app` | `arty` app: CSP-style docking (egui_dock), tool bar, Sub Tool with real previews, color wheel, layer panel |
| `crates/arty-testkit` | `CountingAllocator` for testing that hot paths make no heap allocations |
| `hokusai-0.2.0/` | Brush engine (modified libmypaint port) |

Dependencies point one way only: `core ← brush ← app`, `core ← render ← app`.

## Engineering principles

- **No allocation on hot paths:** dab painting and tile compositing are checked by tests (`tests/alloc_gate.rs`, `tests/stroke.rs`).
- **Composite on the CPU, show on the GPU:** only the flattened page lives on the GPU, so VRAM doesn't grow with layer count.
- **Copy-on-write tiles:** undo snapshots and layer duplication share pixels until they're edited.
- **Reproducible benchmarks:** results go in `plans/bench/` with the command used.

Roadmap: `plans/v2_roadmap.md`

## Shortcuts (similar to CSP)

| Key | Action |
|---|---|
| P / N / B / J / U / E | Pen / Pencil / Brush / Airbrush / Blend / Eraser |
| I, Alt (while drawing) | Eyedropper |
| Space / Shift+Space / Ctrl+Space | Hand / Rotate / Zoom (temporary) |
| Mouse wheel, Alt+wheel | Zoom at cursor, Rotate |
| `[` `]` | Brush smaller / larger |
| X | Swap main/sub color |
| `-` `=` / F | Rotate view 15° / Flip horizontal |
| Ctrl+Z, Ctrl+Y | Undo, Redo |
| Ctrl+Shift+N, Ctrl+E, Ctrl+Alt+G | New layer, Merge down, Clip to layer below |
| Ctrl+0, Ctrl+Alt+0 | Fit to window, 100% |
