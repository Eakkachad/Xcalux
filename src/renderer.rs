use crate::canvas::{BlendMode, Layer};
use ahash::AHashMap;
use std::collections::HashMap;
use std::sync::Arc;
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 2],
    tex_coords: [f32; 2],
}

// 6 vertices for two triangles forming a quad
const QUAD_VERTICES: [Vertex; 6] = [
    Vertex {
        position: [-1.0, -1.0],
        tex_coords: [0.0, 1.0],
    },
    Vertex {
        position: [1.0, -1.0],
        tex_coords: [1.0, 1.0],
    },
    Vertex {
        position: [-1.0, 1.0],
        tex_coords: [0.0, 0.0],
    },
    Vertex {
        position: [-1.0, 1.0],
        tex_coords: [0.0, 0.0],
    },
    Vertex {
        position: [1.0, -1.0],
        tex_coords: [1.0, 1.0],
    },
    Vertex {
        position: [1.0, 1.0],
        tex_coords: [1.0, 0.0],
    },
];

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct BlendUniforms {
    opacity: f32,
    blend_mode: u32,
    clipping: u32,
    padding: u32,
}

const MAX_TILE_SLOTS: usize = 4096;

pub struct WgpuRenderer {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,

    // LRU cache mapping (layer_id, tx, ty) -> slot.
    lru_cache: HashMap<(u32, i32, i32), usize>,
    lru_usage: Vec<(u32, i32, i32)>, // Back is most recently used

    // Off-screen texture targets for blending pass
    pub target_width: u32,
    pub target_height: u32,
    pub target_view: Option<wgpu::TextureView>,
    pub target_egui_id: Option<egui::TextureId>,

    // Navigator Texture targets
    #[allow(dead_code)]
    pub navigator_texture: wgpu::Texture,
    pub navigator_view: wgpu::TextureView,
    pub navigator_egui_id: Option<egui::TextureId>,

    // Navigator swap buffer for ping-pong compositing (avoids RESOURCE+COLOR_TARGET conflict)
    #[allow(dead_code)]
    navigator_swap_texture: wgpu::Texture,
    navigator_swap_view: wgpu::TextureView,

    // Buffers and pipelines
    #[allow(dead_code)]
    vertex_buffer: wgpu::Buffer,
    render_pipeline: wgpu::RenderPipeline,

    // Bind group layouts
    bind_group_layout: wgpu::BindGroupLayout,
    uniform_bind_group_layout: wgpu::BindGroupLayout,

    // Sampler for drawing textures
    sampler: wgpu::Sampler,

    // A single blank 64x64 texture representing empty tiles
    blank_view: wgpu::TextureView,

    // Off-screen canvas framebuffers. We keep two to swap back-and-forth during multi-layer blending
    canvas_textures: [wgpu::Texture; 2],
    canvas_views: [wgpu::TextureView; 2],

    // Folder framebuffer pool
    pub folder_textures: Vec<wgpu::Texture>,
    pub folder_views: Vec<wgpu::TextureView>,

    // Individual GPU Texture views for each LRU slot (to bind as 2D textures)
    slot_textures: Vec<wgpu::Texture>,
    slot_views: Vec<wgpu::TextureView>,
    upload_staging_buffer: Vec<u8>,
}

impl WgpuRenderer {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Option<Self> {
        let state = cc.wgpu_render_state.as_ref()?;
        let device = Arc::clone(&state.device);
        let queue = Arc::clone(&state.queue);

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest, // Pixel-perfect rendering for digital painting
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Create blank 64x64 texture
        let blank_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Blank Tile Texture"),
            size: wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let blank_view = blank_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Fill blank texture with 0s
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &blank_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &vec![0u8; 64 * 64 * 4],
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(64 * 4),
                rows_per_image: Some(64),
            },
            wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
        );

        let mut slot_textures = Vec::with_capacity(MAX_TILE_SLOTS);
        let mut slot_views = Vec::with_capacity(MAX_TILE_SLOTS);
        for i in 0..MAX_TILE_SLOTS {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(&format!("LRU Slot Texture {}", i)),
                size: wgpu::Extent3d {
                    width: 64,
                    height: 64,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            slot_textures.push(tex);
            slot_views.push(view);
        }

        // Initialize offscreen framebuffers (default to 800x800, will resize)
        let canvas_textures = [
            Self::create_canvas_texture(&device, 800, 800),
            Self::create_canvas_texture(&device, 800, 800),
        ];
        let canvas_views = [
            canvas_textures[0].create_view(&wgpu::TextureViewDescriptor::default()),
            canvas_textures[1].create_view(&wgpu::TextureViewDescriptor::default()),
        ];

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Quad Vertex Buffer"),
            contents: bytemuck::cast_slice(&QUAD_VERTICES),
            usage: wgpu::BufferUsages::VERTEX,
        });

        // Load shader
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Compositing WGSL Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/blending.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Compositing Bind Group Layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let uniform_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Uniforms Bind Group Layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Compositing Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout, &uniform_bind_group_layout],
            push_constant_ranges: &[],
        });

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Layer Compositing Render Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });

        // Create navigator texture (256x256) + swap buffer for ping-pong compositing
        let navigator_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Navigator Texture"),
            size: wgpu::Extent3d {
                width: 256,
                height: 256,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let navigator_view = navigator_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let navigator_swap_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Navigator Swap Texture"),
            size: wgpu::Extent3d {
                width: 256,
                height: 256,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let navigator_swap_view = navigator_swap_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Register the navigator texture with Egui
        let mut egui_renderer = state.renderer.write();
        let navigator_egui_id = Some(egui_renderer.register_native_texture(
            &device,
            &navigator_view,
            wgpu::FilterMode::Linear,
        ));

        let folder_textures = vec![
            Self::create_canvas_texture(&device, 800, 800),
            Self::create_canvas_texture(&device, 800, 800),
            Self::create_canvas_texture(&device, 800, 800),
            Self::create_canvas_texture(&device, 800, 800),
            Self::create_canvas_texture(&device, 800, 800),
            Self::create_canvas_texture(&device, 800, 800),
            Self::create_canvas_texture(&device, 800, 800),
            Self::create_canvas_texture(&device, 800, 800),
        ];
        let folder_views = folder_textures
            .iter()
            .map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()))
            .collect();

        Some(Self {
            device,
            queue,
            lru_cache: HashMap::new(),
            lru_usage: Vec::new(),
            target_width: 800,
            target_height: 800,
            target_view: None,
            target_egui_id: None,
            navigator_texture,
            navigator_view,
            navigator_egui_id,
            navigator_swap_texture,
            navigator_swap_view,
            vertex_buffer,
            render_pipeline,
            bind_group_layout,
            uniform_bind_group_layout,
            sampler,
            blank_view,
            canvas_textures,
            canvas_views,
            folder_textures,
            folder_views,
            slot_textures,
            slot_views,
            upload_staging_buffer: vec![0u8; 16384],
        })
    }

    fn create_canvas_texture(device: &wgpu::Device, w: u32, h: u32) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Offscreen Canvas Texture"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    pub fn resize_viewport(
        &mut self,
        state: &eframe::egui_wgpu::RenderState,
        width: u32,
        height: u32,
    ) {
        if width == 0
            || height == 0
            || (self.target_width == width
                && self.target_height == height
                && self.target_egui_id.is_some())
        {
            return;
        }

        self.target_width = width;
        self.target_height = height;

        // Recreate framebuffers
        self.canvas_textures = [
            Self::create_canvas_texture(&self.device, width, height),
            Self::create_canvas_texture(&self.device, width, height),
        ];
        self.canvas_views = [
            self.canvas_textures[0].create_view(&wgpu::TextureViewDescriptor::default()),
            self.canvas_textures[1].create_view(&wgpu::TextureViewDescriptor::default()),
        ];

        self.folder_textures = vec![
            Self::create_canvas_texture(&self.device, width, height),
            Self::create_canvas_texture(&self.device, width, height),
            Self::create_canvas_texture(&self.device, width, height),
            Self::create_canvas_texture(&self.device, width, height),
            Self::create_canvas_texture(&self.device, width, height),
            Self::create_canvas_texture(&self.device, width, height),
            Self::create_canvas_texture(&self.device, width, height),
            Self::create_canvas_texture(&self.device, width, height),
        ];
        self.folder_views = self.folder_textures
            .iter()
            .map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()))
            .collect();

        // Recreate Egui View Texture
        let mut renderer = state.renderer.write();

        // Evict previous texture handle to avoid VRAM leak
        if let Some(texture_id) = self.target_egui_id.take() {
            renderer.free_texture(&texture_id);
        }

        // Register the primary canvas framebuffer with Egui
        let view = self.canvas_textures[0].create_view(&wgpu::TextureViewDescriptor::default());
        let egui_id =
            renderer.register_native_texture(&self.device, &view, wgpu::FilterMode::Linear);

        self.target_view = Some(view);
        self.target_egui_id = Some(egui_id);
    }

    /// Upload a single CPU tile to a WGPU texture slot. Downsamples fix15 (premultiplied u16) to 8-bit RGBA.
    fn upload_tile(
        &mut self,
        _layer_id: u32,
        _tx: i32,
        _ty: i32,
        tile: &crate::canvas::Tile,
        slot: usize,
    ) {
        for y in 0..64 {
            for x in 0..64 {
                let pixel = tile.pixels[y][x];
                let idx = (y * 64 + x) * 4;

                // Downsample fix15 [0, 32768] -> [0, 255]
                self.upload_staging_buffer[idx] = ((pixel[0] as u32 * 255 + 16384) >> 15) as u8;
                self.upload_staging_buffer[idx + 1] = ((pixel[1] as u32 * 255 + 16384) >> 15) as u8;
                self.upload_staging_buffer[idx + 2] = ((pixel[2] as u32 * 255 + 16384) >> 15) as u8;
                self.upload_staging_buffer[idx + 3] = ((pixel[3] as u32 * 255 + 16384) >> 15) as u8;
            }
        }

        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &self.slot_textures[slot],
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &self.upload_staging_buffer,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(64 * 4),
                rows_per_image: Some(64),
            },
            wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Retrieve or allocate a slot for a tile in the GPU LRU cache
    fn get_slot(&mut self, layer_id: u32, tx: i32, ty: i32, tile: &crate::canvas::Tile) -> usize {
        let key = (layer_id, tx, ty);
        if let Some(&slot) = self.lru_cache.get(&key) {
            // Update usage order
            if let Some(pos) = self.lru_usage.iter().position(|&x| x == key) {
                self.lru_usage.remove(pos);
            }
            self.lru_usage.push(key);
            return slot;
        }

        // Cache miss: Allocate a new slot
        let slot = if self.lru_cache.len() < MAX_TILE_SLOTS {
            let new_slot = self.lru_cache.len();
            self.lru_cache.insert(key, new_slot);
            self.lru_usage.push(key);
            new_slot
        } else {
            // Evict least recently used tile
            let evicted_key = self.lru_usage.remove(0);
            let evicted_slot = self.lru_cache.remove(&evicted_key).unwrap();

            self.lru_cache.insert(key, evicted_slot);
            self.lru_usage.push(key);
            evicted_slot
        };

        // Upload the pixel data to this slot
        self.upload_tile(layer_id, tx, ty, tile, slot);
        slot
    }

    /// Incremental texture updates: scans all canvas tiles, checks dirty statuses, and writes to GPU
    pub fn update_textures(&mut self, layers: &mut [&mut Layer]) {
        for layer in layers {
            let layer_id = layer.id;
            for (&coords, tile) in layer.tiles.iter_mut() {
                if tile.is_dirty || !self.lru_cache.contains_key(&(layer_id, coords.0, coords.1)) {
                    let slot = self.get_slot(layer_id, coords.0, coords.1, tile);
                    self.upload_tile(layer_id, coords.0, coords.1, tile, slot);
                    tile.is_dirty = false;
                }
            }
        }
    }

    /// Clear the GPU LRU cache and reset all texture slots to clean VRAM
    pub fn clear_cache(&mut self) {
        self.lru_cache.clear();
        self.lru_usage.clear();

        // Zero out all texture slots
        let blank_pixels = vec![0u8; 64 * 64 * 4];
        for i in 0..self.slot_textures.len() {
            self.queue.write_texture(
                wgpu::ImageCopyTexture {
                    texture: &self.slot_textures[i],
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &blank_pixels,
                wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(64 * 4),
                    rows_per_image: Some(64),
                },
                wgpu::Extent3d {
                    width: 64,
                    height: 64,
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    fn compose_recursive(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        layers: &AHashMap<u32, Layer>,
        layer_ids: &[u32],
        depth: usize,
        viewport_offset: egui::Vec2,
        viewport_zoom: f32,
        canvas_width: u32,
        canvas_height: u32,
        mirror_horizontal: bool,
        rotation_angle: f32,
    ) -> usize {
        let acc_idx = 2 * depth;
        let swap_idx = 2 * depth + 1;

        // Clear the accumulator to transparent
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(&format!("Folder Clear Pass Depth {}", depth)),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.folder_views[acc_idx],
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }

        let mut active_idx = acc_idx;

        // Iterate layers from bottom to top
        for &layer_id in layer_ids.iter().rev() {
            let layer = match layers.get(&layer_id) {
                Some(l) => l,
                None => continue,
            };

            if !layer.visible || layer.opacity <= 0.0 {
                continue;
            }

            let next_idx = if active_idx == acc_idx { swap_idx } else { acc_idx };

            // Copy active accumulator to next buffer to preserve background
            encoder.copy_texture_to_texture(
                wgpu::ImageCopyTexture {
                    texture: &self.folder_textures[active_idx],
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::ImageCopyTexture {
                    texture: &self.folder_textures[next_idx],
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: self.target_width,
                    height: self.target_height,
                    depth_or_array_layers: 1,
                },
            );

            // Pre-compute blend uniforms
            let mode_val = match layer.blend_mode {
                BlendMode::Normal => 0u32,
                BlendMode::Multiply => 1u32,
                BlendMode::Screen => 2u32,
                BlendMode::Overlay => 3u32,
                BlendMode::Luminosity => 4u32,
                BlendMode::Shade => 5u32,
            };
            let clipping_val = if layer.is_clipping { 1u32 } else { 0u32 };

            let uniforms = BlendUniforms {
                opacity: layer.opacity,
                blend_mode: mode_val,
                clipping: clipping_val,
                padding: 0,
            };

            let uniform_buf = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Layer Blend Uniform Buffer"),
                    contents: bytemuck::cast_slice(&[uniforms]),
                    usage: wgpu::BufferUsages::UNIFORM,
                });

            let uniform_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Layer Uniforms Bind Group"),
                layout: &self.uniform_bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buf.as_entire_binding(),
                }],
            });

            match &layer.kind {
                crate::canvas::LayerType::Raster | crate::canvas::LayerType::Vector => {
                    // Draw each tile
                    let mut all_vertices: Vec<Vertex> = Vec::with_capacity(layer.tiles.len() * 6);
                    let mut tile_bind_groups: Vec<wgpu::BindGroup> = Vec::with_capacity(layer.tiles.len());

                    for (&coords, _tile) in layer.tiles.iter() {
                        let key = (layer.id, coords.0, coords.1);
                        let tile_view = if let Some(&slot) = self.lru_cache.get(&key) {
                            &self.slot_views[slot]
                        } else {
                            &self.blank_view
                        };

                        tile_bind_groups.push(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("Tile Compositing Bind Group"),
                            layout: &self.bind_group_layout,
                            entries: &[
                                wgpu::BindGroupEntry {
                                    binding: 0,
                                    resource: wgpu::BindingResource::TextureView(
                                        &self.folder_views[active_idx],
                                    ),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 1,
                                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 2,
                                    resource: wgpu::BindingResource::TextureView(tile_view),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 3,
                                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                                },
                            ],
                        }));

                        let tile_world_x = (coords.0 * 64) as f32;
                        let tile_world_y = (coords.1 * 64) as f32;
                        let tile_world_size = 64.0;

                        let left = ((tile_world_x - viewport_offset.x) * viewport_zoom)
                            / (self.target_width as f32 * 0.5)
                            - 1.0;
                        let right = (((tile_world_x + tile_world_size) - viewport_offset.x)
                            * viewport_zoom)
                            / (self.target_width as f32 * 0.5)
                            - 1.0;
                        let top = 1.0
                            - ((tile_world_y - viewport_offset.y) * viewport_zoom)
                                / (self.target_height as f32 * 0.5);
                        let bottom = 1.0
                            - (((tile_world_y + tile_world_size) - viewport_offset.y) * viewport_zoom)
                                / (self.target_height as f32 * 0.5);

                        let mut tile_verts = [
                            Vertex {
                                position: [left, bottom],
                                tex_coords: [0.0, 1.0],
                            },
                            Vertex {
                                position: [right, bottom],
                                tex_coords: [1.0, 1.0],
                            },
                            Vertex {
                                position: [left, top],
                                tex_coords: [0.0, 0.0],
                            },
                            Vertex {
                                position: [left, top],
                                tex_coords: [0.0, 0.0],
                            },
                            Vertex {
                                position: [right, bottom],
                                tex_coords: [1.0, 1.0],
                            },
                            Vertex {
                                position: [right, top],
                                tex_coords: [1.0, 0.0],
                            },
                        ];

                        let cos_theta = rotation_angle.cos();
                        let sin_theta = rotation_angle.sin();
                        for v in &mut tile_verts {
                            let mut px = v.position[0];
                            let py = v.position[1];
                            if mirror_horizontal {
                                px = -px;
                            }
                            v.position[0] = px * cos_theta - py * sin_theta;
                            v.position[1] = px * sin_theta + py * cos_theta;
                        }

                        all_vertices.extend_from_slice(&tile_verts);
                    }

                    if !all_vertices.is_empty() {
                        let tile_vertex_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("Layer Tiles Vertex Buffer"),
                            contents: bytemuck::cast_slice(&all_vertices),
                            usage: wgpu::BufferUsages::VERTEX,
                        });

                        {
                            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                                label: Some("Layer Blend Render Pass"),
                                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                    view: &self.folder_views[next_idx],
                                    resolve_target: None,
                                    ops: wgpu::Operations {
                                        load: wgpu::LoadOp::Load,
                                        store: wgpu::StoreOp::Store,
                                    },
                                })],
                                depth_stencil_attachment: None,
                                timestamp_writes: None,
                                occlusion_query_set: None,
                            });

                            rpass.set_pipeline(&self.render_pipeline);
                            rpass.set_vertex_buffer(0, tile_vertex_buf.slice(..));
                            rpass.set_bind_group(1, &uniform_bind_group, &[]);

                            for (i, bg) in tile_bind_groups.iter().enumerate() {
                                rpass.set_bind_group(0, bg, &[]);
                                let start = (i * 6) as u32;
                                rpass.draw(start..start + 6, 0..1);
                            }
                        }
                        active_idx = next_idx;
                    }
                }
                crate::canvas::LayerType::Folder { child_ids } => {
                    // Recursively compose child layers (limit depth to safe level)
                    let folder_result_idx = if depth < 3 {
                        self.compose_recursive(
                            encoder,
                            layers,
                            child_ids,
                            depth + 1,
                            viewport_offset,
                            viewport_zoom,
                            canvas_width,
                            canvas_height,
                            mirror_horizontal,
                            rotation_angle,
                        )
                    } else {
                        // Exceeded depth limit, return transparent texture index at max depth
                        2 * depth
                    };

                    // Blend folder result texture back onto the parent accumulator
                    let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("Folder Blend Bind Group"),
                        layout: &self.bind_group_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(&self.folder_views[active_idx]),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::Sampler(&self.sampler),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: wgpu::BindingResource::TextureView(&self.folder_views[folder_result_idx]),
                            },
                            wgpu::BindGroupEntry {
                                binding: 3,
                                resource: wgpu::BindingResource::Sampler(&self.sampler),
                            },
                        ],
                    });

                    {
                        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("Folder Blend Pass"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &self.folder_views[next_idx],
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Load,
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            depth_stencil_attachment: None,
                            timestamp_writes: None,
                            occlusion_query_set: None,
                        });

                        rpass.set_pipeline(&self.render_pipeline);
                        rpass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                        rpass.set_bind_group(0, &bind_group, &[]);
                        rpass.set_bind_group(1, &uniform_bind_group, &[]);
                        rpass.draw(0..6, 0..1);
                    }
                    active_idx = next_idx;
                }
            }
        }

        active_idx
    }

    /// Compose the layer stack from bottom to top using our custom blending fragment shader on WGPU
    pub fn compose_layers(
        &mut self,
        layers: &AHashMap<u32, Layer>,
        layer_order: &[u32],
        viewport_offset: egui::Vec2,
        viewport_zoom: f32,
        canvas_width: u32,
        canvas_height: u32,
        mirror_horizontal: bool,
        rotation_angle: f32,
    ) {
        if self.target_egui_id.is_none() {
            return;
        }

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Canvas Compositing Encoder"),
            });

        // 1. Recursively compose the artwork layers into the folder pool starting at depth 0
        let final_artwork_idx = self.compose_recursive(
            &mut encoder,
            layers,
            layer_order,
            0,
            viewport_offset,
            viewport_zoom,
            canvas_width,
            canvas_height,
            mirror_horizontal,
            rotation_angle,
        );

        // 2. Clear canvas_views[0] to medium grey background
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Clear Canvas Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.canvas_views[0],
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.15,
                            g: 0.15,
                            b: 0.15,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }

        // 3. Draw the solid white paper canvas sheet centered at (0,0) onto canvas_views[0]
        // We compute the paper rect in NDC, then render a white quad into a fresh render pass.
        // blend_mode=6 in the shader simply returns vec4(1,1,1,1) — so we skip binding
        // canvas_views[0] as a resource (which caused the RESOURCE+COLOR_TARGET conflict).
        {
            let left = ((0.0 - viewport_offset.x) * viewport_zoom)
                / (self.target_width as f32 * 0.5)
                - 1.0;
            let right = ((canvas_width as f32 - viewport_offset.x) * viewport_zoom)
                / (self.target_width as f32 * 0.5)
                - 1.0;
            let top = 1.0
                - ((0.0 - viewport_offset.y) * viewport_zoom) / (self.target_height as f32 * 0.5);
            let bottom = 1.0
                - ((canvas_height as f32 - viewport_offset.y) * viewport_zoom)
                    / (self.target_height as f32 * 0.5);

            let mut paper_vertices: [Vertex; 6] = [
                Vertex { position: [left, bottom], tex_coords: [0.0, 1.0] },
                Vertex { position: [right, bottom], tex_coords: [1.0, 1.0] },
                Vertex { position: [left, top], tex_coords: [0.0, 0.0] },
                Vertex { position: [left, top], tex_coords: [0.0, 0.0] },
                Vertex { position: [right, bottom], tex_coords: [1.0, 1.0] },
                Vertex { position: [right, top], tex_coords: [1.0, 0.0] },
            ];

            let cos_theta = rotation_angle.cos();
            let sin_theta = rotation_angle.sin();
            for v in &mut paper_vertices {
                let mut px = v.position[0];
                let py = v.position[1];
                if mirror_horizontal { px = -px; }
                v.position[0] = px * cos_theta - py * sin_theta;
                v.position[1] = px * sin_theta + py * cos_theta;
            }

            let paper_vertex_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Paper Canvas Vertex Buffer"),
                contents: bytemuck::cast_slice(&paper_vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });

            // Use blank_view for both texture bindings — blend_mode=6 ignores them and
            // just outputs vec4(1,1,1,1), so there is zero texture usage conflict.
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Paper Canvas Bind Group"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&self.blank_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&self.blank_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });

            let uniforms = BlendUniforms {
                opacity: 1.0,
                blend_mode: 6u32, // Paper Canvas Mode — outputs solid white, no sampling
                clipping: 0u32,
                padding: 0,
            };

            let uniform_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Paper Blend Uniform Buffer"),
                contents: bytemuck::cast_slice(&[uniforms]),
                usage: wgpu::BufferUsages::UNIFORM,
            });

            let uniform_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Paper Uniforms Bind Group"),
                layout: &self.uniform_bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buf.as_entire_binding(),
                }],
            });

            {
                let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Paper Blend Render Pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.canvas_views[0],
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });

                rpass.set_pipeline(&self.render_pipeline);
                rpass.set_vertex_buffer(0, paper_vertex_buf.slice(..));
                rpass.set_bind_group(0, &bind_group, &[]);
                rpass.set_bind_group(1, &uniform_bind_group, &[]);
                rpass.draw(0..6, 0..1);
            }
        }

        // 4. Combine: blend final transparent artwork (final_artwork_idx) on top of canvas_views[0] (paper) into canvas_views[1]
        {
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Final Combine Bind Group"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&self.canvas_views[0]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&self.folder_views[final_artwork_idx]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });

            let uniforms = BlendUniforms {
                opacity: 1.0,
                blend_mode: 0u32, // Normal
                clipping: 0u32,
                padding: 0,
            };

            let uniform_buf = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Final Combine Blend Uniform Buffer"),
                    contents: bytemuck::cast_slice(&[uniforms]),
                    usage: wgpu::BufferUsages::UNIFORM,
                });

            let uniform_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Final Combine Uniforms Bind Group"),
                layout: &self.uniform_bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buf.as_entire_binding(),
                }],
            });

            {
                let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Final Combine Pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.canvas_views[1],
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });

                rpass.set_pipeline(&self.render_pipeline);
                rpass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                rpass.set_bind_group(0, &bind_group, &[]);
                rpass.set_bind_group(1, &uniform_bind_group, &[]);
                rpass.draw(0..6, 0..1);
            }
        }

        // 5. Copy canvas_textures[1] to canvas_textures[0]
        encoder.copy_texture_to_texture(
            wgpu::ImageCopyTexture {
                texture: &self.canvas_textures[1],
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyTexture {
                texture: &self.canvas_textures[0],
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: self.target_width,
                height: self.target_height,
                depth_or_array_layers: 1,
            },
        );

        self.queue.submit(Some(encoder.finish()));

        self.compose_navigator(final_artwork_idx, canvas_width, canvas_height);
    }

    pub fn compose_navigator(
        &mut self,
        final_artwork_idx: usize,
        canvas_width: u32,
        canvas_height: u32,
    ) {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Navigator Compositing Encoder"),
            });

        // 1. Clear navigator texture to medium grey
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Clear Navigator Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.navigator_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.15,
                            g: 0.15,
                            b: 0.15,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }

        // 2. Draw white paper sheet fitting aspect ratio centered in navigator
        let canvas_aspect = canvas_width as f32 / canvas_height as f32;
        let (left, right, top, bottom) = if canvas_aspect >= 1.0 {
            (-1.0, 1.0, 1.0 / canvas_aspect, -1.0 / canvas_aspect)
        } else {
            (-canvas_aspect, canvas_aspect, 1.0, -1.0)
        };

        let paper_vertices = [
            Vertex { position: [left, bottom], tex_coords: [0.0, 1.0] },
            Vertex { position: [right, bottom], tex_coords: [1.0, 1.0] },
            Vertex { position: [left, top], tex_coords: [0.0, 0.0] },
            Vertex { position: [left, top], tex_coords: [0.0, 0.0] },
            Vertex { position: [right, bottom], tex_coords: [1.0, 1.0] },
            Vertex { position: [right, top], tex_coords: [1.0, 0.0] },
        ];

        let paper_vertex_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Navigator Paper Vertex Buffer"),
            contents: bytemuck::cast_slice(&paper_vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        // Paper pass: blend_mode=6 outputs solid white — use blank_view for both
        // texture bindings so navigator_view is only bound as COLOR_TARGET here.
        let bind_group_paper = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Navigator Paper Bind Group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.blank_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&self.blank_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });

        let uniforms_paper = BlendUniforms {
            opacity: 1.0,
            blend_mode: 6u32, // Paper Canvas Mode
            clipping: 0u32,
            padding: 0,
        };

        let uniform_buf_paper = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Navigator Paper Uniform Buffer"),
            contents: bytemuck::cast_slice(&[uniforms_paper]),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let uniform_bind_group_paper = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Navigator Paper Uniforms Bind Group"),
            layout: &self.uniform_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf_paper.as_entire_binding(),
            }],
        });

        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Navigator Paper Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.navigator_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            rpass.set_pipeline(&self.render_pipeline);
            rpass.set_vertex_buffer(0, paper_vertex_buf.slice(..));
            rpass.set_bind_group(0, &bind_group_paper, &[]);
            rpass.set_bind_group(1, &uniform_bind_group_paper, &[]);
            rpass.draw(0..6, 0..1);
        }

        // 3. Copy navigator_view -> navigator_swap_view so the Art pass can read the paper
        //    background without a RESOURCE+COLOR_TARGET conflict on navigator_view.
        encoder.copy_texture_to_texture(
            wgpu::ImageCopyTexture {
                texture: &self.navigator_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyTexture {
                texture: &self.navigator_swap_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d { width: 256, height: 256, depth_or_array_layers: 1 },
        );

        // 4. Draw the final artwork on top of the paper sheet
        //    Background = navigator_swap_view (read), target = navigator_view (write)
        let bind_group_art = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Navigator Art Bind Group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.navigator_swap_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&self.folder_views[final_artwork_idx]),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });

        let uniforms_art = BlendUniforms {
            opacity: 1.0,
            blend_mode: 0u32, // Normal
            clipping: 0u32,
            padding: 0,
        };

        let uniform_buf_art = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Navigator Art Uniform Buffer"),
            contents: bytemuck::cast_slice(&[uniforms_art]),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let uniform_bind_group_art = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Navigator Art Uniforms Bind Group"),
            layout: &self.uniform_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf_art.as_entire_binding(),
            }],
        });

        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Navigator Art Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.navigator_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            rpass.set_pipeline(&self.render_pipeline);
            rpass.set_vertex_buffer(0, paper_vertex_buf.slice(..));
            rpass.set_bind_group(0, &bind_group_art, &[]);
            rpass.set_bind_group(1, &uniform_bind_group_art, &[]);
            rpass.draw(0..6, 0..1);
        }

        self.queue.submit(Some(encoder.finish()));
    }
}
