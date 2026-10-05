//! Headless end-to-end checks: sync a document, draw it through the canvas
//! callback into an offscreen target and read pixels back. Each test is
//! skipped (passes) when no wgpu adapter is available.

use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use arty_core::{Document, TILE_SIZE, TileCoord, fix15};
use egui_wgpu::{CallbackTrait, wgpu};

use crate::gpu::{CHUNK, CanvasGpu};
use crate::upload::CanvasSync;
use crate::view::View;

/// wgpu-core futures resolve immediately on native.
fn ready<F: Future>(f: F) -> F::Output {
    match pin!(f).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(v) => v,
        Poll::Pending => panic!("wgpu future not ready"),
    }
}

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::default();
    let adapter = ready(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    ready(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
}

fn gl_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = wgpu::Backends::GL;
    let instance = wgpu::Instance::new(desc);
    let adapter = ready(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::None,
        force_fallback_adapter: false,
        compatible_surface: None,
        apply_limit_buckets: false,
    }))
    .ok()?;
    ready(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
}

struct LogCapture {
    records: std::sync::Mutex<Vec<(log::Level, String)>>,
}

impl log::Log for LogCapture {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }
    fn log(&self, record: &log::Record) {
        self.records.lock().unwrap().push((record.level(), record.args().to_string()));
    }
    fn flush(&self) {}
}

static LOGGER: LogCapture = LogCapture { records: std::sync::Mutex::new(Vec::new()) };

#[test]
#[ignore = "requires OpenGL hardware context on Windows"]
fn gl_page_texture_creation_has_no_cubearray_warning() {
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Debug);

    let Some((device, _queue)) = gl_device() else {
        return;
    };
    let mut gpu = CanvasGpu::new(&device, wgpu::TextureFormat::Rgba8Unorm);

    LOGGER.records.lock().unwrap().clear();

    // Affected presets with chunk counts that are multiples of 6:
    // A4 350 dpi (12 chunks), B5 350 dpi (12 chunks),
    // Illustration 3000x4000 (12 chunks), B4 600 dpi (54 chunks).
    for (w, h) in [(2894, 4093), (2508, 3541), (3000, 4000), (6071, 8598)] {
        gpu.ensure_page(&device, w, h);
    }

    let logs = LOGGER.records.lock().unwrap().clone();
    for (lvl, msg) in logs {
        assert!(
            !msg.contains("CubeArray") && !msg.contains("heuristics assumed that the view dimension"),
            "unexpected GL target warning: [{lvl}] {msg}"
        );
    }
}

/// White-paper page whose active layer is opaque gray `f(x, y)` (fix15)
/// over every page pixel.
fn gray_doc(width: u32, height: u32, f: impl Fn(u32, u32) -> u16) -> Document {
    let mut doc = Document::new(width, height, 300);
    doc.set_paper(Some([fix15::ONE_U16; 4]));
    let id = doc.active();
    let (grid, dirty) = doc.paint_target(id).unwrap();
    let ts = TILE_SIZE as u32;
    for ty in 0..height.div_ceil(ts) {
        for tx in 0..width.div_ceil(ts) {
            let c = TileCoord::new(tx as i32, ty as i32);
            for (y, row) in grid.get_mut_or_create(c).iter_mut().enumerate() {
                for (x, px) in row.iter_mut().enumerate() {
                    let (gx, gy) = (tx * ts + x as u32, ty * ts + y as u32);
                    if gx < width && gy < height {
                        let v = f(gx, gy);
                        *px = [v, v, v, fix15::ONE_U16];
                    }
                }
            }
            dirty.mark(c);
        }
    }
    doc
}

/// Upload `doc` and draw it with `view` into a `w × h` Rgba8Unorm target
/// (cleared to red); returns the red channel, row-major.
fn render(doc: &mut Document, view: View, w: u32, h: u32) -> Option<Vec<u8>> {
    let (device, queue) = device()?;
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut gpu = CanvasGpu::new(&device, format);
    CanvasSync::default().sync(doc, &mut gpu, &device, &queue)?;

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&Default::default());
    // A huge checker cell keeps the background uniform white.
    let cb = gpu.callback(view.doc_to_screen([w as f32 * 0.5, h as f32 * 0.5]), 1.0e6)?;
    let mut resources = egui_wgpu::CallbackResources::default();
    let mut encoder = device.create_command_encoder(&Default::default());
    let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: [w, h], pixels_per_point: 1.0 };
    let _ = cb.prepare(&device, &queue, &screen, &mut encoder, &mut resources);
    {
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::RED), store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w as f32, h as f32));
        let info = egui::PaintCallbackInfo { viewport: rect, clip_rect: rect, pixels_per_point: 1.0, screen_size_px: [w, h] };
        cb.paint(info, &mut pass.forget_lifetime(), &resources);
    }
    let row = (w * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (row * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);
    buf.map_async(wgpu::MapMode::Read, .., |r| r.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let data = buf.get_mapped_range(..).unwrap();
    Some((0..h).flat_map(|y| (0..w).map(move |x| (y * row + x * 4) as usize)).map(|i| data[i]).collect())
}

#[test]
fn magnified_step_ramps_across_chunk_edge() {
    // Black up to doc x = 1023, white from the next chunk on. At 16× each doc
    // px spans 16 screen px, so the step between texel centres 1023.5 and
    // 1024.5 (screen 24..40) must be one smooth ramp, not a clamped jump.
    let mut doc = gray_doc(2 * CHUNK, 64, |x, _| if x < CHUNK { 0 } else { fix15::ONE_U16 });
    let view = View { center: [CHUNK as f32, 32.0], zoom: 16.0, rotation: 0.0, flip_x: false };
    let Some(px) = render(&mut doc, view, 64, 8) else { return };
    let row = &px[4 * 64..5 * 64];
    assert!(row[..24].iter().all(|&v| v == 0), "{row:?}");
    assert!(row[40..].iter().all(|&v| v == 255), "{row:?}");
    assert!(row.windows(2).all(|p| p[0] < p[1] || p[0] == p[1] && (p[0] == 0 || p[0] == 255)), "{row:?}");
    assert!((120..=140).contains(&row[32]), "{row:?}");
}

#[test]
fn tile_aligned_page_edge_stays_opaque() {
    // 1984 px = 31 tiles: the right edge sits inside the second chunk, next to
    // texels that are never written. Magnified or on mips, the last page
    // pixels must stay pure ink instead of fading toward transparent.
    let side = 31 * TILE_SIZE as u32;
    let mut doc = gray_doc(side, side, |_, _| 0);
    for zoom in [16.0, 1.0 / 8.0, 1.0 / 40.0] {
        // Bottom-right corner at screen (48, 48) of a 64×64 view.
        let center = [side as f32 - 16.0 / zoom; 2];
        let view = View { center, zoom, rotation: 0.0, flip_x: false };
        doc.dirty_mut().mark_all();
        let Some(px) = render(&mut doc, view, 64, 64) else { return };
        for y in 0..47 {
            let row = &px[y * 64..y * 64 + 64];
            assert!(row[..47].iter().all(|&v| v == 0), "zoom {zoom} row {y}: {row:?}");
        }
    }
}

#[test]
fn batched_upload_matches_document() {
    // 4096×3072 = 12 chunks, ~67 MiB of mips: several upload batches.
    let f = |x: u32, y: u32| ((x * 7 + y * 13) % 256) as u16 * 128;
    let mut doc = gray_doc(4096, 3072, f);
    for center in [[64.0, 64.0], [2048.0, 1024.0], [4064.0, 3040.0]] {
        // Integer center at 1×: every screen pixel hits a texel centre.
        let view = View { center, zoom: 1.0, rotation: 0.0, flip_x: false };
        doc.dirty_mut().mark_all();
        let Some(px) = render(&mut doc, view, 64, 64) else { return };
        for sy in 0..64 {
            for sx in 0..64 {
                let (x, y) = ((center[0] as i32 + sx - 32) as u32, (center[1] as i32 + sy - 32) as u32);
                let want = fix15::to_u8(f(x, y));
                assert_eq!(px[(sy * 64 + sx) as usize], want, "doc ({x}, {y})");
            }
        }
    }
}

#[test]
#[ignore = "benchmark"]
fn bench_layer_padding_creation_time() {
    use crate::gpu::MIP_LEVELS;

    let iterations = 30;

    let run_bench = |name: &str, device: &wgpu::Device| {
        let mut times_12 = Vec::with_capacity(iterations);
        let mut times_13 = Vec::with_capacity(iterations);

        for _ in 0..iterations {
            let t0 = std::time::Instant::now();
            let tex12 = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("bench 12"),
                size: wgpu::Extent3d { width: CHUNK, height: CHUNK, depth_or_array_layers: 12 },
                mip_level_count: MIP_LEVELS,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            times_12.push(t0.elapsed());
            drop(tex12);

            let t1 = std::time::Instant::now();
            let tex13 = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("bench 13"),
                size: wgpu::Extent3d { width: CHUNK, height: CHUNK, depth_or_array_layers: 13 },
                mip_level_count: MIP_LEVELS,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            times_13.push(t1.elapsed());
            drop(tex13);
        }

        times_12.sort();
        times_13.sort();

        let med_12 = times_12[iterations / 2];
        let min_12 = times_12[0];
        let med_13 = times_13[iterations / 2];
        let min_13 = times_13[0];

        println!(
            "{name} Texture creation A/B ({iterations} runs interleaved):\n  12 layers (unpadded): median {med_12:?}, min {min_12:?}\n  13 layers (padded):   median {med_13:?}, min {min_13:?}"
        );
    };

    if let Some((device, _queue)) = device() {
        run_bench("Default (Vulkan/DX12)", &device);
    }
    if let Some((gl_device, _queue)) = gl_device() {
        run_bench("GL backend", &gl_device);
    }
}
