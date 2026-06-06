mod app;
mod brush_io;
mod canvas;
mod commands;
mod diagnostics;
mod history;
mod input;
mod preferences;
mod pressure;
mod renderer;
mod save;
mod shortcuts;
mod stress_test;
mod ui;
mod vector;

mod tools {
    pub mod fill;
    pub mod selection;
    pub mod transform;
}

mod export;

fn main() -> eframe::Result<()> {
    // Check for command-line arguments to trigger stress testing headlessly
    let args: Vec<String> = std::env::args().collect();
    if args
        .iter()
        .any(|arg| arg == "--stress-test" || arg == "stress")
    {
        stress_test::run_stress_tests();
        return Ok(());
    }

    // Force DX12 backend on Windows to avoid Vulkan present mode compatibility issues
    // with newer NVIDIA drivers (the "Unrecognized present mode 0x1000361000" warning
    // from wgpu_hal::vulkan::conv causes window freeze / not responding).
    #[cfg(target_os = "windows")]
    if std::env::var("WGPU_BACKEND").is_err() {
        std::env::set_var("WGPU_BACKEND", "dx12");
    }

    // Initialize logging. Keep dependency logs at warn by default; wgpu can emit
    // full generated shader sources at info level, which makes startup look hung.
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,sai_painting_app=info"),
    )
    .init();
    log::info!("Starting Xcalux Digital Painting Application...");
    #[cfg(target_os = "windows")]
    log::info!(
        "WGPU_BACKEND: {}",
        std::env::var("WGPU_BACKEND").unwrap_or_else(|_| "(not set)".into())
    );

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Xcalux Digital Painting")
            .with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };

    eframe::run_native(
        "xcalux",
        native_options,
        Box::new(|cc| Box::new(app::PaintApp::new(cc)) as Box<dyn eframe::App>),
    )
}
