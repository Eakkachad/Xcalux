//! Marching ants: the selection outline as GPU hairlines whose dash pattern
//! moves by a per-frame uniform (a standalone `egui_wgpu::CallbackTrait`).
//!
//! The outline's line-list vertices (`x, y, arc length`, 12 B each, every
//! level of detail in one buffer) are uploaded once per selection revision;
//! each frame only the uniform changes: doc → screen, zoom and dash phase.
//! The fragment shader picks black or white from
//! `fract((arc·zoom + phase) / 16) < 0.5`, so dashes are 8 screen px.

use std::ops::Range;
use std::sync::Arc;

use arty_core::contour::Contours;
use bytemuck::{Pod, Zeroable};
use egui_wgpu::wgpu;

use crate::view::Affine2;

/// Screen px of one black + white dash pair.
pub const DASH_PERIOD: f32 = 16.0;

const SHADER: &str = r#"
struct Uniforms {
    // doc px -> clip space, rows of a 2x3 affine
    row0: vec4<f32>,
    row1: vec4<f32>,
    // screen px per doc px of arc length, dash phase (screen px), _, _
    misc: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) arc: f32,
};

@vertex
fn vs_main(@location(0) p: vec2<f32>, @location(1) arc: f32) -> VsOut {
    var out: VsOut;
    let h = vec3<f32>(p, 1.0);
    out.pos = vec4<f32>(dot(u.row0.xyz, h), dot(u.row1.xyz, h), 0.0, 1.0);
    out.arc = arc;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let t = fract((in.arc * u.misc.x + u.misc.y) / 16.0);
    return select(vec4<f32>(1.0), vec4<f32>(0.0, 0.0, 0.0, 1.0), t < 0.5);
}
"#;

/// One line-list vertex: a document point and the arc length (doc px)
/// from the start of its outline.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct AntsVertex {
    pub pos: [f32; 2],
    pub arc: f32,
}

/// The line-list vertices of every level of detail of one outline.
#[derive(Debug, Default)]
pub struct AntsGeometry {
    /// Identifies the outline (the caller's selection revision key); the
    /// GPU buffer is re-uploaded only when it changes.
    pub key: u64,
    pub vertices: Vec<AntsVertex>,
    /// Vertex range of each level of detail.
    pub lods: [Range<u32>; 3],
}

impl AntsGeometry {
    pub fn build(key: u64, c: &Contours) -> Self {
        let mut vertices = Vec::with_capacity(c.lods.iter().flatten().map(|p| 2 * p.segments()).sum());
        let mut lods: [Range<u32>; 3] = Default::default();
        for (k, lod) in c.lods.iter().enumerate() {
            let start = vertices.len() as u32;
            for p in lod {
                let n = p.pts.len();
                let mut arc = 0.0;
                for i in 0..p.segments() {
                    let (a, b) = (p.pts[i], p.pts[(i + 1) % n]);
                    vertices.push(AntsVertex { pos: a, arc });
                    arc += ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
                    vertices.push(AntsVertex { pos: b, arc });
                }
            }
            lods[k] = start..vertices.len() as u32;
        }
        Self { key, vertices, lods }
    }

    /// Segments at level `lod`.
    pub fn segments(&self, lod: usize) -> usize {
        self.lods[lod].len() / 2
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    row0: [f32; 4],
    row1: [f32; 4],
    misc: [f32; 4],
}

/// GPU state of the ants, kept in egui's callback resources (see
/// [`AntsGpu::install`]).
pub struct AntsGpu {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniforms: wgpu::Buffer,
    vertices: Option<(u64, wgpu::Buffer)>,
    uploads: u64,
}

impl AntsGpu {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("arty ants"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("arty ants"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("arty ants"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("arty ants"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<AntsVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32],
                })],
            },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::LineList, ..Default::default() },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("arty ants uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("arty ants"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() }],
        });
        Self { pipeline, bind_group, uniforms, vertices: None, uploads: 0 }
    }

    /// Put the ants' GPU state into the renderer's callback resources, where
    /// [`paint_callback`] finds it. Without it the callback draws nothing.
    pub fn install(render: &egui_wgpu::RenderState) {
        let gpu = AntsGpu::new(&render.device, render.target_format);
        render.renderer.write().callback_resources.insert(gpu);
    }

    /// Vertex buffer uploads so far (one per outline key).
    pub fn uploads(&self) -> u64 {
        self.uploads
    }

    fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, screen: [u32; 2], cb: &AntsCallback) {
        let g = &cb.geom;
        if self.vertices.as_ref().is_none_or(|(key, _)| *key != g.key) {
            self.vertices = (!g.vertices.is_empty()).then(|| {
                let buf = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("arty ants vertices"),
                    size: std::mem::size_of_val(g.vertices.as_slice()) as u64,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                queue.write_buffer(&buf, 0, bytemuck::cast_slice(&g.vertices));
                (g.key, buf)
            });
            self.uploads += 1;
        }
        // Screen pixels → clip space: x' = 2x/w − 1, y' = 1 − 2y/h.
        let sx = 2.0 / screen[0].max(1) as f32;
        let sy = -2.0 / screen[1].max(1) as f32;
        let m = cb.m;
        let u = Uniforms {
            row0: [m.a * sx, m.b * sx, m.tx * sx - 1.0, 0.0],
            row1: [m.c * sy, m.d * sy, m.ty * sy + 1.0, 0.0],
            misc: [cb.zoom, cb.phase.rem_euclid(DASH_PERIOD), 0.0, 0.0],
        };
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&u));
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'static>, screen: [u32; 2], cb: &AntsCallback) {
        let Some((key, buf)) = &self.vertices else { return };
        let range = cb.geom.lods[cb.lod.min(2)].clone();
        if *key != cb.geom.key || range.is_empty() {
            return;
        }
        // Our transform targets the whole window; egui's scissor clips.
        pass.set_viewport(0.0, 0.0, screen[0] as f32, screen[1] as f32, 0.0, 1.0);
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, buf.slice(..));
        pass.draw(range, 0..1);
    }
}

/// One frame of ants: `geom` at level `lod`, drawn with `doc_to_screen`
/// (physical px, window-relative; a transform session's `view·xf`).
pub struct AntsCallback {
    pub geom: Arc<AntsGeometry>,
    pub lod: usize,
    pub m: Affine2,
    /// Screen px per doc px along the outline.
    pub zoom: f32,
    /// Dash phase in screen px.
    pub phase: f32,
}

/// The paint callback drawing `cb` (needs [`AntsGpu::install`]).
pub fn paint_callback(rect: egui::Rect, cb: AntsCallback) -> egui::PaintCallback {
    egui_wgpu::Callback::new_paint_callback(rect, cb)
}

impl egui_wgpu::CallbackTrait for AntsCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(gpu) = resources.get_mut::<AntsGpu>() {
            gpu.prepare(device, queue, screen.size_in_pixels, self);
        }
        Vec::new()
    }

    fn paint(
        &self,
        info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        if let Some(gpu) = resources.get::<AntsGpu>() {
            gpu.draw(pass, info.screen_size_px, self);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arty_core::contour::Polyline;
    use egui_wgpu::CallbackTrait;
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    fn contours(lods: [Vec<Polyline>; 3]) -> Contours {
        Contours { lods, ..Default::default() }
    }

    #[test]
    fn geometry_is_a_line_list_with_arc_lengths() {
        let square = Polyline { pts: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]], closed: true };
        let open = Polyline { pts: vec![[0.0, 0.0], [3.0, 4.0]], closed: false };
        let g = AntsGeometry::build(7, &contours([vec![square.clone(), open], vec![square], vec![]]));
        assert_eq!((g.key, g.segments(0), g.segments(1), g.segments(2)), (7, 5, 4, 0));
        assert_eq!(g.lods, [0..10, 10..18, 18..18]);
        let arcs: Vec<f32> = g.vertices[..10].iter().map(|v| v.arc).collect();
        assert_eq!(arcs, [0.0, 10.0, 10.0, 20.0, 20.0, 30.0, 30.0, 40.0, 0.0, 5.0]);
        assert_eq!(g.vertices[7].pos, [0.0, 0.0], "the closing segment returns to the start");
        assert_eq!(std::mem::size_of::<AntsVertex>(), 12);
    }

    fn ready<F: Future>(f: F) -> F::Output {
        match pin!(f).poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(v) => v,
            Poll::Pending => panic!("wgpu future not ready"),
        }
    }

    /// Draw a horizontal outline into an offscreen target and read back the
    /// dash colours; skipped (passes) without a wgpu adapter.
    #[test]
    fn ants_draw_dashes_and_upload_once() {
        let instance = wgpu::Instance::default();
        let Ok(adapter) = ready(instance.request_adapter(&wgpu::RequestAdapterOptions::default())) else { return };
        let Ok((device, queue)) = ready(adapter.request_device(&wgpu::DeviceDescriptor::default())) else { return };
        let (w, h) = (128u32, 32u32);
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let mut resources = egui_wgpu::CallbackResources::default();
        resources.insert(AntsGpu::new(&device, format));
        // Doc (x, y) → screen (x + 10, y + 10.5): one px per doc px.
        let line = Polyline { pts: vec![[0.0, 6.0], [100.0, 6.0]], closed: false };
        let geom = Arc::new(AntsGeometry::build(1, &contours([vec![line], vec![], vec![]])));
        let m = Affine2 { a: 1.0, b: 0.0, c: 0.0, d: 1.0, tx: 10.0, ty: 10.5 };
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
        let view = target.create_view(&Default::default());
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: [w, h], pixels_per_point: 1.0 };
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w as f32, h as f32));
        let info = || egui::PaintCallbackInfo { viewport: rect, clip_rect: rect, pixels_per_point: 1.0, screen_size_px: [w, h] };
        let mut encoder = device.create_command_encoder(&Default::default());
        for frame in 0..3 {
            let cb = AntsCallback { geom: geom.clone(), lod: 0, m, zoom: 1.0, phase: 0.0 };
            let _ = cb.prepare(&device, &queue, &screen, &mut encoder, &mut resources);
            if frame == 2 {
                let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::RED), store: wgpu::StoreOp::Store },
                    })],
                    ..Default::default()
                });
                cb.paint(info(), &mut pass.forget_lifetime(), &resources);
            }
        }
        assert_eq!(resources.get::<AntsGpu>().unwrap().uploads(), 1, "the outline uploads once");

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
        let px = |x: u32, y: u32| {
            let i = (y * row + x * 4) as usize;
            [data[i], data[i + 1], data[i + 2]]
        };
        // Screen row 16 holds the line from x = 10 to 110: 8 px black, 8 white.
        assert_eq!(px(13, 16), [0, 0, 0]);
        assert_eq!(px(21, 16), [255, 255, 255]);
        assert_eq!(px(29, 16), [0, 0, 0]);
        assert_eq!(px(13, 8), [255, 0, 0], "nothing drawn off the line");
        assert_eq!(px(120, 16), [255, 0, 0], "nothing drawn past its end");
    }
}
