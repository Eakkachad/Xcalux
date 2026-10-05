//! GPU side of the canvas: a mipmapped texture array holding the flattened
//! page, split into square chunks, drawn through an egui paint callback.
//!
//! Only the composite lives on the GPU (not every layer), so VRAM stays at
//! roughly 4 bytes × page area × 4/3 regardless of layer count.
//!
//! The shader filters by hand with `textureLoad` instead of a sampler, so
//! bilinear/trilinear taps cross chunk layers seamlessly and clamp at the
//! page edge rather than the chunk edge (texels past the page in the last
//! chunk are never written and must not be read).

use arty_core::TILE_SIZE;
use bytemuck::{Pod, Zeroable};
use egui_wgpu::wgpu;

use crate::view::Affine2;

/// Chunk edge in pixels. Must be a multiple of `TILE_SIZE`.
pub const CHUNK: u32 = 1024;
pub const TILES_PER_CHUNK: u32 = CHUNK / TILE_SIZE as u32;
/// Mip levels kept per chunk: 64px tile → 1px.
pub const MIP_LEVELS: u32 = TILE_SIZE.trailing_zeros() + 1;
pub const MAX_PAGE_SIDE: u32 = 16384;

/// Number of array layers to allocate for the page texture.
///
/// wgpu 30's GL backend (`wgpu-hal/src/gles/mod.rs:520-526`, `lib.rs:2219-2224`)
/// infers the GL texture target from the descriptor because WebGPU provides no
/// view dimension at texture creation: square 2D textures with 1 layer are
/// assumed to be `TEXTURE_2D`, 6 layers `TEXTURE_CUBE_MAP`, and layer counts
/// greater than 6 where `layers.is_multiple_of(6)` are assumed to be
/// `TEXTURE_CUBE_MAP_ARRAY`. When viewed as `D2Array`, those targets log an
/// error and fail to sample in shaders expecting `sampler2DArray`.
///
/// We pad the array layer count by 1 whenever `chunks == 1 || chunks.is_multiple_of(6)`
/// so the target is always `TEXTURE_2D_ARRAY`. Chunk addressing (`chunk_slot`),
/// upload rects, and the shader's chunk lookup (`fetch`) still use `chunks_x`
/// and `chunks_y`; the extra layer remains unused.
#[inline]
pub fn page_texture_layers(chunks: u32) -> u32 {
    let chunks = chunks.max(1);
    if chunks == 1 || chunks.is_multiple_of(6) { chunks + 1 } else { chunks }
}

/// Where block `(x, y)` lands in a chunk array `chunks_x` chunks wide, with
/// `side` blocks per chunk edge: `(layer, local x, local y)`. Used with tile
/// units for uploads; `fetch` in canvas.wgsl applies the same math to
/// texels of mip `k` (`side = CHUNK >> k`).
#[inline]
pub fn chunk_slot(x: u32, y: u32, side: u32, chunks_x: u32) -> (u32, u32, u32) {
    ((y / side) * chunks_x + x / side, x % side, y % side)
}

/// Texels per axis at mip `k` that the shader reads for a page `extent` px
/// long: every texel overlapping the page (canvas.wgsl's `sample_level`
/// clamps its taps to this). Always inside the uploaded tiles.
#[inline]
pub fn level_extent(extent: u32, k: u32) -> u32 {
    extent.div_ceil(1 << k)
}

/// Fractional mip LOD for a doc → screen transform, as hardware trilinear
/// would pick it: log2 of doc px per screen px, clamped to the kept levels.
pub fn mip_lod(m: Affine2) -> f32 {
    let zoom = (m.a * m.a + m.c * m.c).sqrt();
    // `max` maps NaN (degenerate transform) to 0.
    (-zoom.log2()).max(0.0).min((MIP_LEVELS - 1) as f32)
}

/// A `w × h` block of tiles inside one chunk layer, uploaded with one
/// `write_texture` per mip. `x`, `y` are page tile coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UploadRect {
    pub layer: u32,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    row0: [f32; 4],
    row1: [f32; 4],
    page: [f32; 4],
    misc: [f32; 4],
}

struct PageTexture {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    width: u32,
    height: u32,
    chunks_x: u32,
    chunks_y: u32,
}

pub struct CanvasGpu {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    target_is_srgb: bool,
    page: Option<PageTexture>,
}

impl CanvasGpu {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("arty canvas"),
            source: wgpu::ShaderSource::Wgsl(include_str!("canvas.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("arty canvas"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("arty canvas"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("arty canvas"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("arty canvas uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { pipeline, layout, uniforms, target_is_srgb: target_format.is_srgb(), page: None }
    }

    /// (Re)allocate page storage when the document size changes. Returns
    /// `true` when the texture was recreated and needs a full upload.
    pub fn ensure_page(&mut self, device: &wgpu::Device, width: u32, height: u32) -> bool {
        let width = width.clamp(1, MAX_PAGE_SIDE);
        let height = height.clamp(1, MAX_PAGE_SIDE);
        if self.page.as_ref().is_some_and(|p| p.width == width && p.height == height) {
            return false;
        }
        let chunks_x = width.div_ceil(CHUNK);
        let chunks_y = height.div_ceil(CHUNK);
        let layers = page_texture_layers(chunks_x * chunks_y);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("arty page"),
            size: wgpu::Extent3d { width: CHUNK, height: CHUNK, depth_or_array_layers: layers },
            mip_level_count: MIP_LEVELS,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("arty canvas"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
            ],
        });
        self.page = Some(PageTexture { texture, bind_group, width, height, chunks_x, chunks_y });
        true
    }

    /// Chunk layer holding page tile `(tx, ty)`, or `None` when it lies
    /// outside the allocated chunks (no page yet, or past `MAX_PAGE_SIDE`).
    pub fn tile_layer(&self, tx: u32, ty: u32) -> Option<u32> {
        let page = self.page.as_ref()?;
        if tx / TILES_PER_CHUNK >= page.chunks_x || ty / TILES_PER_CHUNK >= page.chunks_y {
            return None;
        }
        Some(chunk_slot(tx, ty, TILES_PER_CHUNK, page.chunks_x).0)
    }

    /// Upload a composited block of tiles and its mip chain. `levels[k]`
    /// holds the block's RGBA8 pixels at mip `k`, row-major with
    /// `r.w * (64 >> k)` pixels per row. One `write_texture` (and so one
    /// staging buffer) per mip, however many tiles the block has.
    pub fn upload_rect(&self, queue: &wgpu::Queue, r: &UploadRect, levels: &[&[u8]]) {
        let Some(page) = &self.page else { return };
        if r.layer >= page.chunks_x * page.chunks_y {
            return;
        }
        let (lx, ly) = (r.x % TILES_PER_CHUNK, r.y % TILES_PER_CHUNK);
        for (k, data) in levels.iter().enumerate() {
            let size = TILE_SIZE as u32 >> k;
            let (w, h) = (r.w * size, r.h * size);
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &page.texture,
                    mip_level: k as u32,
                    origin: wgpu::Origin3d { x: lx * size, y: ly * size, z: r.layer },
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
        }
    }

    /// Build the paint callback drawing the page with `doc_to_screen`
    /// (physical pixels, relative to the window).
    pub fn paint_callback(&self, rect: egui::Rect, doc_to_screen: Affine2, checker: f32) -> Option<egui::PaintCallback> {
        let cb = self.callback(doc_to_screen, checker)?;
        Some(egui_wgpu::Callback::new_paint_callback(rect, cb))
    }

    pub(crate) fn callback(&self, doc_to_screen: Affine2, checker: f32) -> Option<CanvasCallback> {
        let page = self.page.as_ref()?;
        Some(CanvasCallback {
            pipeline: self.pipeline.clone(),
            bind_group: page.bind_group.clone(),
            uniforms: self.uniforms.clone(),
            m: doc_to_screen,
            page: [page.width as f32, page.height as f32, CHUNK as f32, page.chunks_x as f32],
            misc: [checker.max(1.0), if self.target_is_srgb { 1.0 } else { 0.0 }, mip_lod(doc_to_screen), 0.0],
        })
    }
}

pub(crate) struct CanvasCallback {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniforms: wgpu::Buffer,
    m: Affine2,
    page: [f32; 4],
    misc: [f32; 4],
}

impl egui_wgpu::CallbackTrait for CanvasCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        _resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        // Screen pixels → clip space: x' = 2x/w − 1, y' = 1 − 2y/h.
        let sx = 2.0 / screen.size_in_pixels[0].max(1) as f32;
        let sy = -2.0 / screen.size_in_pixels[1].max(1) as f32;
        let m = self.m;
        let u = Uniforms {
            row0: [m.a * sx, m.b * sx, m.tx * sx - 1.0, 0.0],
            row1: [m.c * sy, m.d * sy, m.ty * sy + 1.0, 0.0],
            page: self.page,
            misc: self.misc,
        };
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&u));
        Vec::new()
    }

    fn paint(
        &self,
        info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        _resources: &egui_wgpu::CallbackResources,
    ) {
        // Our transform targets the whole window; egui's scissor still clips
        // drawing to the canvas widget.
        let [w, h] = info.screen_size_px;
        pass.set_viewport(0.0, 0.0, w as f32, h as f32, 0.0, 1.0);
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..6, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::View;

    #[test]
    fn chunk_slot_is_a_bijection_at_every_mip() {
        // 3×2 chunks; the shader maps mip-k texels with side CHUNK >> k.
        let (cx, cy) = (3, 2);
        for k in 0..MIP_LEVELS {
            let side = CHUNK >> k;
            let step = (side / 4) as usize;
            let mut seen = std::collections::HashSet::new();
            for y in (0..cy * side).step_by(step) {
                for x in (0..cx * side).step_by(step) {
                    for d in [0, side / 4 - 1] {
                        let (layer, lx, ly) = chunk_slot(x + d, y + d, side, cx);
                        assert!(layer < cx * cy && lx < side && ly < side);
                        let back = ((layer % cx) * side + lx, (layer / cx) * side + ly);
                        assert_eq!(back, (x + d, y + d));
                        assert!(seen.insert((layer, lx, ly)));
                    }
                }
            }
        }
        // Neighbours across a chunk edge land in adjacent layers.
        assert_eq!(chunk_slot(CHUNK - 1, 5, CHUNK, 3), (0, CHUNK - 1, 5));
        assert_eq!(chunk_slot(CHUNK, 5, CHUNK, 3), (1, 0, 5));
        assert_eq!(chunk_slot(7, CHUNK, CHUNK, 3), (3, 7, 0));
    }

    #[test]
    fn level_extent_reads_only_uploaded_texels() {
        // Taps stay in tiles that were composited (never the zeroed texels
        // past a tile-aligned page edge, e.g. 4032 px inside a 4096 chunk)
        // and still reach every texel the page touches.
        for side in (1..=MAX_PAGE_SIDE).step_by(61).chain([63, 64, 65, 1024, 4000, 4032, 4096, MAX_PAGE_SIDE]) {
            let tiles = side.div_ceil(TILE_SIZE as u32);
            for k in 0..MIP_LEVELS {
                let ext = level_extent(side, k);
                assert!(ext <= tiles * (TILE_SIZE as u32 >> k), "side {side} mip {k}");
                assert!(ext << k >= side && (ext - 1) << k < side, "side {side} mip {k}");
            }
        }
        assert_eq!(level_extent(4032, 0), 4032);
        assert_eq!(level_extent(4032, 6), 63);
    }

    #[test]
    fn mip_lod_matches_zoom() {
        let lod = |zoom: f32, rotation: f32, flip_x: bool| {
            mip_lod(View { center: [50.0, 80.0], zoom, rotation, flip_x }.doc_to_screen([400.0, 300.0]))
        };
        assert_eq!(lod(1.0, 0.0, false), 0.0);
        assert_eq!(lod(16.0, 0.3, false), 0.0);
        assert!((lod(0.25, 0.0, false) - 2.0).abs() < 1e-5);
        assert!((lod(0.5, 1.1, true) - 1.0).abs() < 1e-5);
        assert!((lod(0.35, -2.0, false) - 0.35f32.recip().log2()).abs() < 1e-5);
        assert_eq!(lod(0.001, 0.0, false), (MIP_LEVELS - 1) as f32);
    }

    #[test]
    fn canvas_shader_validates() {
        use wgpu::naga;
        let module = naga::front::wgsl::parse_str(include_str!("canvas.wgsl")).expect("canvas.wgsl parses");
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::default())
            .validate(&module)
            .expect("canvas.wgsl validates");
        let entries: Vec<_> = module.entry_points.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(entries, ["vs_main", "fs_main"]);
    }

    #[test]
    fn page_texture_layers_avoids_gl_heuristics() {
        let triggers_gl = |l: u32| l == 1 || l.is_multiple_of(6);
        let max_layers = wgpu::Limits::default().max_texture_array_layers; // 256

        for &c in &[1, 5, 6, 7, 12, 54] {
            let l = page_texture_layers(c);
            assert!(!triggers_gl(l), "count {c} -> {l} triggers GL heuristic");
            assert!(l <= max_layers, "count {c} -> {l} exceeds limit {max_layers}");
        }

        assert_eq!(page_texture_layers(1), 2);
        assert_eq!(page_texture_layers(5), 5);
        assert_eq!(page_texture_layers(6), 7);
        assert_eq!(page_texture_layers(7), 7);
        assert_eq!(page_texture_layers(12), 13);
        assert_eq!(page_texture_layers(54), 55);

        // Max ARTY page (16384 x 16384 px = 16 x 16 = 256 chunks): 256 % 6 == 4 != 0.
        let max_chunks = (MAX_PAGE_SIDE / CHUNK) * (MAX_PAGE_SIDE / CHUNK);
        let max_l = page_texture_layers(max_chunks);
        assert_eq!(max_l, 256);
        assert!(!triggers_gl(max_l));
        assert!(max_l <= max_layers);

        // Edge count 2048 (hardware / desktop GL limit): 2048 % 6 == 2 != 0.
        let l2048 = page_texture_layers(2048);
        assert_eq!(l2048, 2048);
        assert!(!triggers_gl(l2048));
        assert!(l2048 <= 2048);
    }
}
