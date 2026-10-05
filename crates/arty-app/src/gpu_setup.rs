//! GPU initialization, backend fallback sequence, and crash-recovery state.
//!
//! Enforces fallback order: Vulkan -> DX12 -> GL -> WARP, one backend at a time.
//! Prevents simultaneous multi-backend enumeration and honors battery/power status.

use std::path::{Path, PathBuf};
use egui_wgpu::wgpu;

/// Marker file written immediately before GPU initialization and removed upon
/// successful first frame presentation. If present at launch, the previous run died.
pub const MARKER_FILE_NAME: &str = "gpu_marker.txt";

/// File recording the last known working GPU backend.
pub const LAST_WORKING_FILE_NAME: &str = "gpu_last_working.txt";

/// Supported GPU backends in ARTY.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuBackend {
    Vulkan,
    Dx12,
    Gl,
    Warp,
}

impl GpuBackend {
    /// Identifier string for environment variables and marker files.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Vulkan => "vulkan",
            Self::Dx12 => "dx12",
            Self::Gl => "gl",
            Self::Warp => "warp",
        }
    }

    /// Parses a backend string from env vars, markers, or flags.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "vulkan" | "vk" => Some(Self::Vulkan),
            "dx12" | "d3d12" | "directx" => Some(Self::Dx12),
            "gl" | "opengl" | "gles" => Some(Self::Gl),
            "warp" | "software" | "cpu" => Some(Self::Warp),
            _ => None,
        }
    }

    /// Corresponding wgpu backend flag.
    pub const fn backends(self) -> wgpu::Backends {
        match self {
            Self::Vulkan => wgpu::Backends::VULKAN,
            Self::Dx12 | Self::Warp => wgpu::Backends::DX12,
            Self::Gl => wgpu::Backends::GL,
        }
    }

    /// Whether this backend requires requesting the fallback adapter (e.g. WARP).
    pub const fn force_fallback(self) -> bool {
        matches!(self, Self::Warp)
    }
}

/// A 4-part driver version (e.g. 31.0.101.4575).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct DriverVersion(pub u32, pub u32, pub u32, pub u32);

impl DriverVersion {
    /// Attempts to parse a version string containing dot-separated numbers.
    pub fn parse(s: &str) -> Option<Self> {
        let mut parts = [0u32; 4];
        let mut count = 0;
        for token in s.split(|c: char| !c.is_ascii_digit()) {
            if token.is_empty() {
                continue;
            }
            if count < 4 {
                parts[count] = token.parse().ok()?;
                count += 1;
            }
        }
        if count > 0 {
            Some(Self(parts[0], parts[1], parts[2], parts[3]))
        } else {
            None
        }
    }
}

/// A driver blocklist rule matching vendor ID and optional driver version range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlocklistEntry {
    pub vendor_id: u32,
    pub min_version: Option<DriverVersion>,
    pub max_version: Option<DriverVersion>,
    pub skip_backend: GpuBackend,
}

impl BlocklistEntry {
    /// Checks whether this entry matches the given adapter info and backend.
    pub fn matches(&self, vendor_id: u32, version: Option<DriverVersion>, backend: GpuBackend) -> bool {
        if self.vendor_id != vendor_id || self.skip_backend != backend {
            return false;
        }
        match version {
            Some(v) => {
                if let Some(min) = self.min_version
                    && v < min
                {
                    return false;
                }
                if let Some(max) = self.max_version
                    && v > max
                {
                    return false;
                }
                true
            }
            None => self.min_version.is_none() && self.max_version.is_none(),
        }
    }
}

/// Active driver blocklist (empty by default, populated as driver bugs are identified).
pub static BLOCKLIST: &[BlocklistEntry] = &[];

/// Determines whether an adapter is blocklisted for the specified backend.
pub fn is_blocklisted(blocklist: &[BlocklistEntry], info: &wgpu::AdapterInfo, backend: GpuBackend) -> bool {
    let version = DriverVersion::parse(&info.driver_info);
    blocklist.iter().any(|entry| entry.matches(info.vendor, version, backend))
}

/// Resolves candidate backends in order of attempt.
pub fn resolve_backend_candidates(
    cli_safe_gpu: bool,
    env_arty_gpu: Option<&str>,
    env_wgpu_backend: Option<&str>,
    last_working: Option<GpuBackend>,
    crashed_marker: Option<GpuBackend>,
) -> Vec<GpuBackend> {
    if cli_safe_gpu {
        let mut candidates = vec![GpuBackend::Gl, GpuBackend::Warp];
        if let Some(crashed) = crashed_marker {
            candidates.retain(|&b| b != crashed);
            if candidates.is_empty() {
                candidates.push(GpuBackend::Warp);
            }
        }
        return candidates;
    }

    if let Some(b) = env_arty_gpu.and_then(GpuBackend::parse) {
        return vec![b];
    }

    if let Some(b) = env_wgpu_backend.and_then(GpuBackend::parse) {
        return vec![b];
    }

    let mut candidates = vec![
        GpuBackend::Vulkan,
        GpuBackend::Dx12,
        GpuBackend::Gl,
        GpuBackend::Warp,
    ];

    if let Some(last) = last_working
        && let Some(pos) = candidates.iter().position(|&b| b == last)
    {
        candidates.remove(pos);
        candidates.insert(0, last);
    }

    if let Some(crashed) = crashed_marker {
        log::warn!("Previous run died during {crashed:?} init; skipping backend");
        candidates.retain(|&b| b != crashed);
        if candidates.is_empty() {
            candidates.push(GpuBackend::Warp);
        }
    }

    candidates
}

/// Checks whether the system is a laptop currently running on battery.
#[cfg(windows)]
pub fn is_running_on_battery() -> bool {
    #[repr(C)]
    struct SystemPowerStatus {
        ac_line_status: u8,
        battery_flag: u8,
        battery_life_percent: u8,
        system_status_flag: u8,
        battery_life_time: u32,
        battery_full_life_time: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetSystemPowerStatus(status: *mut SystemPowerStatus) -> i32;
    }

    let mut status = SystemPowerStatus {
        ac_line_status: 255,
        battery_flag: 255,
        battery_life_percent: 255,
        system_status_flag: 0,
        battery_life_time: 0,
        battery_full_life_time: 0,
    };
    // SAFETY: pointer to local writable struct is valid.
    let ok = unsafe { GetSystemPowerStatus(&mut status) };
    ok != 0 && status.ac_line_status == 0
}

#[cfg(not(windows))]
pub fn is_running_on_battery() -> bool {
    false
}

/// Resolves adapter power preference from env or battery state.
pub fn resolve_power_preference() -> wgpu::PowerPreference {
    if let Ok(val) = std::env::var("WGPU_POWER_PREF") {
        match val.trim().to_ascii_lowercase().as_str() {
            "low" => return wgpu::PowerPreference::LowPower,
            "high" => return wgpu::PowerPreference::HighPerformance,
            "none" => return wgpu::PowerPreference::None,
            _ => {}
        }
    }
    if is_running_on_battery() {
        wgpu::PowerPreference::LowPower
    } else {
        wgpu::PowerPreference::HighPerformance
    }
}

/// Resolves memory hints from env or defaults to MemoryUsage.
pub fn resolve_memory_hints() -> wgpu::MemoryHints {
    if let Ok(val) = std::env::var("ARTY_MEMORY_HINTS") {
        match val.trim().to_ascii_lowercase().as_str() {
            "performance" | "perf" => return wgpu::MemoryHints::Performance,
            "memory" | "memory_usage" | "usage" => return wgpu::MemoryHints::MemoryUsage,
            _ => {}
        }
    }
    wgpu::MemoryHints::MemoryUsage
}

/// Path to marker file.
pub fn marker_path(storage_dir: &Path) -> PathBuf {
    storage_dir.join(MARKER_FILE_NAME)
}

/// Path to last working backend file.
pub fn last_working_path(storage_dir: &Path) -> PathBuf {
    storage_dir.join(LAST_WORKING_FILE_NAME)
}

/// Reads the crash marker if present.
pub fn read_crashed_marker(storage_dir: &Path) -> Option<GpuBackend> {
    let content = std::fs::read_to_string(marker_path(storage_dir)).ok()?;
    GpuBackend::parse(&content)
}

/// Writes the backend marker before GPU initialization.
pub fn write_marker(storage_dir: &Path, backend: GpuBackend) {
    let _ = std::fs::create_dir_all(storage_dir);
    if let Err(e) = std::fs::write(marker_path(storage_dir), backend.as_str()) {
        log::warn!("could not write gpu marker file: {e}");
    }
}

/// Deletes the crash marker after successful frame presentation.
pub fn clear_marker(storage_dir: &Path) {
    let p = marker_path(storage_dir);
    if p.exists() {
        let _ = std::fs::remove_file(p);
    }
}

/// Reads the last working backend file if present.
pub fn read_last_working(storage_dir: &Path) -> Option<GpuBackend> {
    let content = std::fs::read_to_string(last_working_path(storage_dir)).ok()?;
    GpuBackend::parse(&content)
}

/// Saves the last working backend file.
pub fn save_last_working(storage_dir: &Path, backend: GpuBackend) {
    let _ = std::fs::create_dir_all(storage_dir);
    if let Err(e) = std::fs::write(last_working_path(storage_dir), backend.as_str()) {
        log::warn!("could not write last working gpu file: {e}");
    }
}

/// Invoked when the first frame is presented.
pub fn on_first_frame_presented(storage_dir: Option<&Path>, backend: GpuBackend, bench_active: bool) {
    if let Some(dir) = storage_dir {
        clear_marker(dir);
        if !bench_active {
            save_last_working(dir, backend);
        }
    }
}

/// Attempts to initialize a single GPU backend with one instance, adapter, device, and queue.
pub fn try_init_backend(
    backend: GpuBackend,
    power_preference: wgpu::PowerPreference,
    memory_hints: wgpu::MemoryHints,
) -> Result<egui_wgpu::WgpuSetupExisting, String> {
    log::info!("Attempting GPU initialization with backend {:?}", backend);

    let instance_desc = wgpu::InstanceDescriptor {
        backends: backend.backends(),
        flags: wgpu::InstanceFlags::from_build_config().with_env(),
        backend_options: wgpu::BackendOptions::from_env_or_default(),
        memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
        display: None,
    };
    let instance = wgpu::Instance::new(instance_desc);

    let adapter_options = wgpu::RequestAdapterOptions {
        power_preference,
        compatible_surface: None,
        force_fallback_adapter: backend.force_fallback(),
        apply_limit_buckets: false,
    };

    let adapter = pollster::block_on(instance.request_adapter(&adapter_options))
        .map_err(|e| format!("no adapter found for {backend:?}: {e}"))?;

    let info = adapter.get_info();
    log::info!(
        "wgpu adapter: name: {:?}, backend: {:?}, driver_info: {:?}",
        info.name,
        info.backend,
        info.driver_info
    );

    if is_blocklisted(BLOCKLIST, &info, backend) {
        return Err(format!(
            "adapter {} (vendor 0x{:04x}, driver {}) is blocklisted for {backend:?}",
            info.name, info.vendor, info.driver_info
        ));
    }

    let base_limits = if info.backend == wgpu::Backend::Gl {
        wgpu::Limits::downlevel_webgl2_defaults()
    } else {
        wgpu::Limits::default()
    };

    let device_desc = wgpu::DeviceDescriptor {
        label: Some("arty wgpu device"),
        required_limits: wgpu::Limits {
            max_texture_dimension_2d: 8192,
            ..base_limits
        },
        memory_hints,
        ..Default::default()
    };

    let (device, queue) = pollster::block_on(adapter.request_device(&device_desc))
        .map_err(|e| format!("device creation failed for {backend:?}: {e}"))?;

    Ok(egui_wgpu::WgpuSetupExisting {
        instance,
        adapter,
        device,
        queue,
    })
}

/// Initializes GPU setup by iterating through fallback candidates.
pub fn init_gpu(
    storage_dir: Option<&Path>,
    safe_gpu: bool,
    bench_active: bool,
) -> (egui_wgpu::WgpuSetup, GpuBackend) {
    let last_working = if bench_active {
        None
    } else {
        storage_dir.and_then(read_last_working)
    };
    let crashed_marker = storage_dir.and_then(read_crashed_marker);

    let env_arty_gpu = std::env::var("ARTY_GPU").ok();
    let env_wgpu_backend = std::env::var("WGPU_BACKEND").ok();

    let candidates = resolve_backend_candidates(
        safe_gpu,
        env_arty_gpu.as_deref(),
        env_wgpu_backend.as_deref(),
        last_working,
        crashed_marker,
    );

    let power_pref = resolve_power_preference();
    let memory_hints = resolve_memory_hints();

    for &backend in &candidates {
        if let Some(dir) = storage_dir {
            write_marker(dir, backend);
        }
        match try_init_backend(backend, power_pref, memory_hints.clone()) {
            Ok(existing) => {
                log::info!("Successfully initialized GPU backend {:?}", backend);
                return (egui_wgpu::WgpuSetup::Existing(existing), backend);
            }
            Err(e) => {
                log::warn!("GPU init failed for {backend:?}: {e}; trying next fallback");
            }
        }
    }

    panic!("All GPU backend initialization attempts failed for candidates: {candidates:?}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_backend_strings() {
        assert_eq!(GpuBackend::parse("vulkan"), Some(GpuBackend::Vulkan));
        assert_eq!(GpuBackend::parse("VK"), Some(GpuBackend::Vulkan));
        assert_eq!(GpuBackend::parse("dx12"), Some(GpuBackend::Dx12));
        assert_eq!(GpuBackend::parse("D3D12"), Some(GpuBackend::Dx12));
        assert_eq!(GpuBackend::parse("gl"), Some(GpuBackend::Gl));
        assert_eq!(GpuBackend::parse("opengl"), Some(GpuBackend::Gl));
        assert_eq!(GpuBackend::parse("warp"), Some(GpuBackend::Warp));
        assert_eq!(GpuBackend::parse("software"), Some(GpuBackend::Warp));
        assert_eq!(GpuBackend::parse("unknown"), None);
    }

    #[test]
    fn default_fallback_order() {
        let candidates = resolve_backend_candidates(false, None, None, None, None);
        assert_eq!(
            candidates,
            vec![
                GpuBackend::Vulkan,
                GpuBackend::Dx12,
                GpuBackend::Gl,
                GpuBackend::Warp
            ]
        );
    }

    #[test]
    fn last_working_preferred() {
        let candidates = resolve_backend_candidates(false, None, None, Some(GpuBackend::Dx12), None);
        assert_eq!(
            candidates,
            vec![
                GpuBackend::Dx12,
                GpuBackend::Vulkan,
                GpuBackend::Gl,
                GpuBackend::Warp
            ]
        );

        let candidates_gl = resolve_backend_candidates(false, None, None, Some(GpuBackend::Gl), None);
        assert_eq!(
            candidates_gl,
            vec![
                GpuBackend::Gl,
                GpuBackend::Vulkan,
                GpuBackend::Dx12,
                GpuBackend::Warp
            ]
        );
    }

    #[test]
    fn crash_marker_skips_crashed_backend() {
        let candidates = resolve_backend_candidates(false, None, None, None, Some(GpuBackend::Vulkan));
        assert_eq!(
            candidates,
            vec![GpuBackend::Dx12, GpuBackend::Gl, GpuBackend::Warp]
        );

        // Even if last working was Vulkan, a crash marker skips it.
        let candidates_last = resolve_backend_candidates(
            false,
            None,
            None,
            Some(GpuBackend::Vulkan),
            Some(GpuBackend::Vulkan),
        );
        assert_eq!(
            candidates_last,
            vec![GpuBackend::Dx12, GpuBackend::Gl, GpuBackend::Warp]
        );
    }

    #[test]
    fn safe_gpu_flag_order() {
        let candidates = resolve_backend_candidates(true, None, None, None, None);
        assert_eq!(candidates, vec![GpuBackend::Gl, GpuBackend::Warp]);

        let candidates_crashed_gl =
            resolve_backend_candidates(true, None, None, None, Some(GpuBackend::Gl));
        assert_eq!(candidates_crashed_gl, vec![GpuBackend::Warp]);
    }

    #[test]
    fn env_override_parsing() {
        let candidates_arty = resolve_backend_candidates(false, Some("gl"), None, None, None);
        assert_eq!(candidates_arty, vec![GpuBackend::Gl]);

        let candidates_wgpu = resolve_backend_candidates(false, None, Some("dx12"), None, None);
        assert_eq!(candidates_wgpu, vec![GpuBackend::Dx12]);

        let candidates_warp = resolve_backend_candidates(false, Some("warp"), None, None, None);
        assert_eq!(candidates_warp, vec![GpuBackend::Warp]);
    }

    #[test]
    fn driver_version_parsing() {
        assert_eq!(
            DriverVersion::parse("31.0.101.4575"),
            Some(DriverVersion(31, 0, 101, 4575))
        );
        assert_eq!(
            DriverVersion::parse("551.86"),
            Some(DriverVersion(551, 86, 0, 0))
        );
        assert_eq!(DriverVersion::parse("invalid"), None);
    }

    #[test]
    fn blocklist_matching() {
        let entry = BlocklistEntry {
            vendor_id: 0x8086,
            min_version: Some(DriverVersion(30, 0, 100, 0)),
            max_version: Some(DriverVersion(30, 0, 101, 9999)),
            skip_backend: GpuBackend::Vulkan,
        };

        // Matching vendor, version in range, matching backend
        assert!(entry.matches(0x8086, Some(DriverVersion(30, 0, 100, 5000)), GpuBackend::Vulkan));

        // Matching vendor, version out of range
        assert!(!entry.matches(0x8086, Some(DriverVersion(31, 0, 101, 4575)), GpuBackend::Vulkan));

        // Different backend
        assert!(!entry.matches(0x8086, Some(DriverVersion(30, 0, 100, 5000)), GpuBackend::Dx12));

        // Different vendor
        assert!(!entry.matches(0x10DE, Some(DriverVersion(30, 0, 100, 5000)), GpuBackend::Vulkan));
    }

    #[test]
    fn marker_read_write_clear() {
        let temp_dir = std::env::temp_dir().join(format!("arty_gpu_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);

        assert_eq!(read_crashed_marker(&temp_dir), None);
        write_marker(&temp_dir, GpuBackend::Vulkan);
        assert_eq!(read_crashed_marker(&temp_dir), Some(GpuBackend::Vulkan));
        clear_marker(&temp_dir);
        assert_eq!(read_crashed_marker(&temp_dir), None);

        assert_eq!(read_last_working(&temp_dir), None);
        save_last_working(&temp_dir, GpuBackend::Dx12);
        assert_eq!(read_last_working(&temp_dir), Some(GpuBackend::Dx12));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn try_init_available_backends() {
        let power_pref = resolve_power_preference();
        let memory_hints = resolve_memory_hints();

        println!("Testing backend initialization:");
        for &backend in &[GpuBackend::Vulkan, GpuBackend::Dx12, GpuBackend::Gl, GpuBackend::Warp] {
            match try_init_backend(backend, power_pref, memory_hints.clone()) {
                Ok(setup) => {
                    let info = setup.adapter.get_info();
                    println!("  {:?}: OK -> {} ({:?}, driver: {})", backend, info.name, info.backend, info.driver_info);
                }
                Err(e) => {
                    println!("  {:?}: FAILED -> {}", backend, e);
                }
            }
        }
    }
}
