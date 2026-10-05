//! Marching ants: the selection outline as GPU hairlines whose dash pattern
//! moves by a per-frame uniform (a standalone `egui_wgpu::CallbackTrait`).

/// GPU state of the ants.
pub struct AntsGpu;

impl AntsGpu {
    pub fn new(_device: &wgpu::Device, _format: wgpu::TextureFormat) -> Self {
        // SEL-UI: pipeline and buffers.
        AntsGpu
    }
}
