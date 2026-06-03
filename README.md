# ARTY (Xcalux) Digital Painting Workstation

ARTY (Xcalux) is a high-performance digital painting application built for Windows. It is designed around four primary core pillars: ultra-low latency brush strokes, a minimal memory footprint, a lightweight UI, and a zero-allocation drawing loop to deliver a smooth and responsive drawing experience similar to Paint Tool SAI.

## Core Features

- **Infinite Tiled Canvas & GPU Cache**: The canvas is split into a sparse grid of 64x64 pixel tiles, using a Fix15 premultiplied RGBA pixel format. It utilizes an LRU cache on the GPU to manage tile textures efficiently, enabling work on large canvases without performance degradation.
- **Dynamic Brush Engine**: Powered by the Hokusai (libmypaint) brush engine, it supports real-time color blending (smudging) and dilution to simulate painting with wet media.
- **Hardware-Accelerated Canvas Transformation**: Supports real-time horizontal mirroring, rotation, and zooming via GPU vertex shaders, with accurate inverse-coordinate transformations for stylus tracking.
- **Zero-Allocation Stroke Loop**: The active drawing path performs zero heap allocations. It utilizes a pre-allocated object pool for undo/redo history and circular buffers for input stabilization.
- **Input Stabilization**: Implements physics-based Spring-Mass-Damper stabilizers (S-levels) and Exponential Moving Average (EMA) smoothing to eliminate pen jitter.
- **Asynchronous Incremental Saving**: Automatically saves the canvas to the `.arty` format in a background thread, preventing UI lag during saving.

## Directory Structure

- **src/**: Core application source code
  - `main.rs`: Entry point and CLI argument handling
  - `app.rs`: Main application logic, UI state, and input dispatch
  - `renderer.rs`: WGPU rendering engine and layer compositing
  - `canvas.rs`: Data models for layers, tiles, and blend modes
  - `input.rs`: Stylus/tablet input handling and stabilizers
  - `history.rs`: Heap-allocation-free undo/redo manager
  - `brush_io.rs`: Preset serialization and Clip Studio Paint (`.sut`) texture extraction
  - `save.rs`: Asynchronous background saving pipeline
  - `stress_test.rs`: Performance and allocation tracking harness
- **hokusai-0.2.0/**: Local dependency containing the brush engine
- **vendor/**: Offline vendored dependency crates
- **bigplane.md**: Authoritative development plan and roadmap
- **system_documentation.md**: Detailed system architecture reference

## Setup and Installation

### Prerequisites
- Rust compiler (Stable channel recommended)
- Windows OS (required for the native RealTimeStylus/Windows Ink integration)

### Running the Application

- **Run in Debug Mode**:
  ```powershell
  cargo run
  ```

- **Run in optimized Release Mode (Recommended)**:
  ```powershell
  cargo run --release
  ```

- **Run Stress Tests**:
  ```powershell
  cargo run -- --stress-test
  ```

- **Check Compilation**:
  ```powershell
  cargo check
  ```
