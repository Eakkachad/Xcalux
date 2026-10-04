//! GPU side of the canvas: a mipmapped texture array holding the flattened
//! page, split into square chunks, drawn through an egui paint callback.
//!
//! Only the composite lives on the GPU (not every layer), so VRAM stays at
//! roughly 4 bytes × page area × 4/3 regardless of layer count.

use arty_core::{TILE_SIZE, TileCoord};
use bytemuck::{Pod, Zeroable};
use egui_wgpu::wgpu;

use crate::view::Affine2;

/// Chunk edge in pixels. Must be a multiple of `TILE_SIZE`.
pub const CHUNK: u32 = 1024;
const TILES_PER_CHUNK: u32 = CHUNK / TILE_SIZE as u32;
/// Mip levels kept per chunk: 64px tile → 1px.
pub const MIP_LEVELS: u32 = TILE_SIZE.trailing_zeros() + 1;
pub const MAX_PAGE_SIDE: u32 = 16384;

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
    chunks: u32,
}

pub struct CanvasGpu {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
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
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("arty canvas"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            lod_max_clamp: (MIP_LEVELS - 1) as f32,
            ..Default::default()
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("arty canvas uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { pipeline, layout, sampler, uniforms, target_is_srgb: target_format.is_srgb(), page: None }
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
        let chunks = chunks_x * height.div_ceil(CHUNK);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("arty page"),
            size: wgpu::Extent3d { width: CHUNK, height: CHUNK, depth_or_array_layers: chunks },
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
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        self.page = Some(PageTexture { texture, bind_group, width, height, chunks_x, chunks });
        true
    }

    /// Upload one composited tile and its mip chain. `levels[k]` holds the
    /// `(64 >> k)²` RGBA8 pixels of mip `k`.
    pub fn upload_tile(&self, queue: &wgpu::Queue, c: TileCoord, levels: &[&[u8]]) {
        let Some(page) = &self.page else { return };
        if c.x < 0 || c.y < 0 {
            return;
        }
        let (tx, ty) = (c.x as u32, c.y as u32);
        let layer = (ty / TILES_PER_CHUNK) * page.chunks_x + tx / TILES_PER_CHUNK;
        if layer >= page.chunks {
            return;
        }
        let (lx, ly) = (tx % TILES_PER_CHUNK, ty % TILES_PER_CHUNK);
        for (k, data) in levels.iter().enumerate() {
            let size = TILE_SIZE as u32 >> k;
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &page.texture,
                    mip_level: k as u32,
                    origin: wgpu::Origin3d { x: lx * size, y: ly * size, z: layer },
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size * 4), rows_per_image: Some(size) },
                wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
            );
        }
    }

    /// Build the paint callback drawing the page with `doc_to_screen`
    /// (physical pixels, relative to the window).
    pub fn paint_callback(&self, rect: egui::Rect, doc_to_screen: Affine2, checker: f32) -> Option<egui::PaintCallback> {
        let page = self.page.as_ref()?;
        let cb = CanvasCallback {
            pipeline: self.pipeline.clone(),
            bind_group: page.bind_group.clone(),
            uniforms: self.uniforms.clone(),
            m: doc_to_screen,
            page: [page.width as f32, page.height as f32, CHUNK as f32, page.chunks_x as f32],
            misc: [checker.max(1.0), if self.target_is_srgb { 1.0 } else { 0.0 }, 0.0, 0.0],
            instances: page.chunks,
        };
        Some(egui_wgpu::Callback::new_paint_callback(rect, cb))
    }
}

struct CanvasCallback {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniforms: wgpu::Buffer,
    m: Affine2,
    page: [f32; 4],
    misc: [f32; 4],
    instances: u32,
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
        pass.draw(0..6, 0..self.instances);
    }
}
