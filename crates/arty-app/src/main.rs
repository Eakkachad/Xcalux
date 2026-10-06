//! ARTY — manga and illustration painting workstation.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod about;
mod app;
mod bench;
mod canvas;
mod commands;
mod demo;
mod export;
mod files;
pub mod gpu_setup;
mod home;
mod logging;
mod machine;
mod panels;
mod shell;
mod studio;
pub mod text;
mod theme;
mod tools;

// `files` tests check that an idle frame does not allocate; `--features heap-stats`
// adds live/peak heap bytes to the ARTY_BENCH lines (bench.rs; 0 without it).
// Off in normal builds: the counters cost atomics on every allocation.
#[cfg(any(test, feature = "heap-stats"))]
#[global_allocator]
static ALLOC: arty_testkit::CountingAllocator = arty_testkit::CountingAllocator;

fn main() -> eframe::Result<()> {
    let bench_active = bench::active();
    logging::init(bench_active);
    init_rayon();

    let safe_gpu = std::env::args().any(|arg| arg == "--safe-gpu");
    let storage_dir = eframe::storage_dir(APP_NAME);

    let (wgpu_setup, backend) = gpu_setup::init_gpu(storage_dir.as_deref(), safe_gpu, bench_active);
    let fast_vsync_ok = backend == gpu_setup::GpuBackend::Dx12 || backend == gpu_setup::GpuBackend::Warp;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ARTY")
            .with_inner_size([1600.0, 960.0])
            .with_min_inner_size([960.0, 600.0]),
        renderer: eframe::Renderer::Wgpu,
        // Bench runs leave app.ron alone (bench.rs).
        persist_window: !bench_active,
        // LOW_LATENCY unless the user picked another Display sync (eframe's default is LOW_LATENCY too).
        wgpu_options: egui_wgpu::WgpuConfiguration {
            wgpu_setup,
            ..Default::default()
        }
        .with_surface_config(saved_display_sync().surface_config(fast_vsync_ok)),
        ..Default::default()
    };
    if let Err(e) = eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |cc| Ok(Box::new(app::ArtyApp::new(cc, backend)))),
    ) {
        // The backend may have worked last time but not now (driver change):
        // the next start probes again, and the crash marker skips this one.
        if let Some(dir) = storage_dir.as_deref() {
            gpu_setup::forget_last_working(dir);
        }
        log::error!("could not run the window: {e}");
        logging::flush();
        return Err(e);
    }
    logging::flush();
    // A bench hook that failed (bench.rs) quits with its own code.
    match bench::exit_code() {
        0 => Ok(()),
        code => std::process::exit(code),
    }
}

/// Configures Rayon's global thread pool at start-up:
/// - Sized to logical CPUs usable by this process (affinity-aware), capped at 16,
///   unless `RAYON_NUM_THREADS` is set (leaving Rayon's default handling).
/// - Thread names set to "arty-rayon-N" so plans/bench/B006_threads.ps1 can count them.
fn init_rayon() {
    let mut builder = rayon::ThreadPoolBuilder::new().thread_name(|idx| format!("arty-rayon-{idx}"));
    if std::env::var("RAYON_NUM_THREADS").ok().filter(|s| !s.trim().is_empty()).is_none() {
        let logical = arty_io::usable_cpus().logical;
        builder = builder.num_threads(arty_io::default_rayon_threads(logical));
    }
    if let Err(e) = builder.build_global() {
        log::debug!("could not initialize rayon global thread pool: {e}");
    }
}

/// eframe's storage folder is named after this.
pub const APP_NAME: &str = "ARTY";

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
