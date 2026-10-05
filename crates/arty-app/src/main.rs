//! ARTY — manga and illustration painting workstation.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod bench;
mod canvas;
mod commands;
mod demo;
mod export;
mod files;
mod panels;
mod shell;
mod studio;
mod theme;
mod tools;

// `files` tests check that an idle frame does not allocate.
#[cfg(test)]
#[global_allocator]
static ALLOC: arty_testkit::CountingAllocator = arty_testkit::CountingAllocator;

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ARTY")
            .with_inner_size([1600.0, 960.0])
            .with_min_inner_size([960.0, 600.0]),
        renderer: eframe::Renderer::Wgpu,
        // Bench runs leave app.ron alone (bench.rs).
        persist_window: !bench::active(),
        // LOW_LATENCY unless the user picked another Display sync (eframe's default is LOW_LATENCY too).
        wgpu_options: egui_wgpu::WgpuConfiguration::default().with_surface_config(saved_display_sync().surface_config(false)),
        ..Default::default()
    };
    eframe::run_native(APP_NAME, options, Box::new(|cc| Ok(Box::new(app::ArtyApp::new(cc)))))?;
    // A bench hook that failed (bench.rs) quits with its own code.
    match bench::exit_code() {
        0 => Ok(()),
        code => std::process::exit(code),
    }
}

/// eframe's storage folder is named after this.
const APP_NAME: &str = "ARTY";

/// The saved Display sync, applied at start-up: eframe 0.36 keeps
/// `Frame::set_wgpu_surface_config` on the frame's copy of the render state and
/// never hands it to its painter, so a change made while running only lands here,
/// on the next start. Mailbox (Fast vsync) needs the DX12 backend, which is unknown
/// before the window exists, so it starts as Low latency.
fn saved_display_sync() -> studio::DisplaySync {
    eframe::storage_dir(APP_NAME)
        .and_then(|dir| std::fs::read_to_string(dir.join("app.ron")).ok())
        .and_then(|text| studio::DisplaySync::from_saved(&text))
        .unwrap_or_default()
}
