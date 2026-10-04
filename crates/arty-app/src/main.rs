//! ARTY — manga and illustration painting workstation.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod canvas;
mod commands;
mod demo;
mod export;
mod files;
mod panels;
mod shell;
mod studio;
mod theme;

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
        ..Default::default()
    };
    eframe::run_native("ARTY", options, Box::new(|cc| Ok(Box::new(app::ArtyApp::new(cc)))))
}
