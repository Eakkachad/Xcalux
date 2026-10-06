//! What this machine is (RAM, cores, graphics, drive) and the tier that
//! follows from it (plans/lowend_ux_plan.md §4.2 D3). Detected once at
//! start-up; only the drive arrives later, from a short-lived thread.

use std::sync::mpsc::{Receiver, channel};

use arty_io::storage::{self, DriveKind};
use egui_wgpu::wgpu;

use crate::text::{Key, t};

/// Drive under the profile and recovery folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Storage {
    Ssd,
    Hdd,
    #[default]
    Unknown,
}

impl From<DriveKind> for Storage {
    fn from(k: DriveKind) -> Self {
        match k {
            DriveKind::Ssd => Storage::Ssd,
            DriveKind::Hdd => Storage::Hdd,
            DriveKind::Unknown => Storage::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vendor {
    Intel,
    Amd,
    Nvidia,
    Other,
}

/// From the PCI vendor id the adapter reports.
pub fn vendor_from_id(id: u32) -> Vendor {
    match id {
        0x8086 | 0x8087 => Vendor::Intel,
        0x1002 | 0x1022 => Vendor::Amd,
        0x10DE => Vendor::Nvidia,
        _ => Vendor::Other,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuKind {
    Integrated,
    Discrete,
    /// WARP, llvmpipe: the CPU draws.
    Software,
    Unknown,
}

/// From the adapter's device type; a software renderer is also recognised
/// by name, as a driver may call itself virtual.
pub fn gpu_kind(device_type: wgpu::DeviceType, name: &str) -> GpuKind {
    let lower = name.to_ascii_lowercase();
    if device_type == wgpu::DeviceType::Cpu
        || ["llvmpipe", "softpipe", "swiftshader", "microsoft basic render", "warp"].iter().any(|s| lower.contains(s))
    {
        return GpuKind::Software;
    }
    match device_type {
        wgpu::DeviceType::IntegratedGpu => GpuKind::Integrated,
        wgpu::DeviceType::DiscreteGpu => GpuKind::Discrete,
        _ => GpuKind::Unknown,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuSummary {
    pub name: String,
    pub vendor: Vendor,
    pub kind: GpuKind,
    pub backend: &'static str,
}

impl GpuSummary {
    pub fn from_info(info: &wgpu::AdapterInfo) -> Self {
        let backend = match info.backend {
            wgpu::Backend::Vulkan => "Vulkan",
            wgpu::Backend::Dx12 => "DX12",
            wgpu::Backend::Gl => "OpenGL",
            _ => "other",
        };
        Self { name: info.name.clone(), vendor: vendor_from_id(info.vendor), kind: gpu_kind(info.device_type, &info.name), backend }
    }
}

/// Plan tiers T0 / T1 / T2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Low,
    Mid,
    High,
}

const GIB: u64 = 1 << 30;

/// RAM as a whole number of GiB, rounded up: Windows reports usable memory
/// (7.8 GiB on an 8 GB machine).
pub fn ram_gib(ram: u64) -> u64 {
    ram.div_ceil(GIB)
}

/// Low: under 6 GiB, at most 2 cores, or software graphics. Mid: under
/// 12 GiB or at most 4 cores. High otherwise. Unknown RAM counts as Mid.
pub fn classify(ram: Option<u64>, physical_cores: usize, gpu: Option<GpuKind>) -> Tier {
    let gib = ram.map(ram_gib);
    if gib.is_some_and(|g| g < 6) || physical_cores <= 2 || gpu == Some(GpuKind::Software) {
        Tier::Low
    } else if gib.is_none_or(|g| g < 12) || physical_cores <= 4 {
        Tier::Mid
    } else {
        Tier::High
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Machine {
    pub ram: Option<u64>,
    pub physical_cores: usize,
    pub gpu: Option<GpuSummary>,
    pub storage: Storage,
}

impl Default for Machine {
    /// A mid-range machine with nothing known about it (tests, before detection).
    fn default() -> Self {
        Self { ram: None, physical_cores: 4, gpu: None, storage: Storage::Unknown }
    }
}

impl Machine {
    /// Honours `ARTY_RAM_MB` (arty-io); the drive is filled in later by [`Probe`].
    pub fn detect(gpu: Option<GpuSummary>) -> Self {
        Self { ram: arty_io::physical_memory(), physical_cores: arty_io::usable_cpus().physical, gpu, storage: Storage::Unknown }
    }

    pub fn tier(&self) -> Tier {
        classify(self.ram, self.physical_cores, self.gpu.as_ref().map(|g| g.kind))
    }

    /// Graphics memory is system RAM, unless the card is discrete.
    pub fn gpu_shares_ram(&self) -> bool {
        self.gpu.as_ref().is_none_or(|g| g.kind != GpuKind::Discrete)
    }

    /// "machine: RAM 7.8 GiB · 4 cores · Intel UHD Graphics 770 (integrated, Vulkan) · SSD · tier Mid"
    pub fn log_line(&self) -> String {
        let mut s = String::from("machine: ");
        match self.ram {
            Some(r) => s.push_str(&format!("RAM {:.1} GiB", r as f64 / GIB as f64)),
            None => s.push_str("RAM unknown"),
        }
        s.push_str(&format!(" · {} cores", self.physical_cores));
        if let Some(g) = &self.gpu {
            let kind = match g.kind {
                GpuKind::Integrated => "integrated, ",
                GpuKind::Discrete => "discrete, ",
                GpuKind::Software => "software, ",
                GpuKind::Unknown => "",
            };
            s.push_str(&format!(" · {} ({kind}{})", g.name, g.backend));
        }
        match self.storage {
            Storage::Ssd => s.push_str(" · SSD"),
            Storage::Hdd => s.push_str(" · HDD"),
            Storage::Unknown => {}
        }
        s.push_str(&format!(" · tier {:?}", self.tier()));
        s
    }

    /// The Home screen's spec list: "RAM 8 GB · Intel graphics · SSD". Parts
    /// that are not known are left out.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(r) = self.ram {
            parts.push(format!("RAM {} GB", ram_gib(r)));
        }
        if let Some(g) = &self.gpu {
            let vendor = match g.vendor {
                Vendor::Intel => Some("Intel"),
                Vendor::Amd => Some("AMD"),
                Vendor::Nvidia => Some("NVIDIA"),
                Vendor::Other => None,
            };
            parts.push(match (g.kind, vendor) {
                (GpuKind::Software, _) => t(Key::MachineGfxSoftware).to_owned(),
                (_, Some(v)) => t(Key::MachineGfx).replace("{}", v),
                (_, None) => t(Key::MachineGfxOther).to_owned(),
            });
        }
        match self.storage {
            Storage::Ssd => parts.push(t(Key::MachineSsd).to_owned()),
            Storage::Hdd => parts.push(t(Key::MachineHdd).to_owned()),
            Storage::Unknown => {}
        }
        parts.join(" · ")
    }
}

/// The drive query running on its own thread (an HDD may have to spin up).
pub struct Probe(Receiver<Storage>);

impl Probe {
    /// Looks at the drive of the recovery folder; wakes the UI when it knows.
    pub fn spawn(repaint: impl Fn() + Send + 'static) -> Option<Self> {
        let (tx, rx) = channel();
        let path = arty_io::RecoveryDir::default_path();
        std::thread::Builder::new()
            .name("arty-storage-probe".into())
            .spawn(move || {
                let _ = tx.send(storage::drive_kind(&path).into());
                repaint();
            })
            .ok()
            .map(|_| Self(rx))
    }

    /// The answer, once it has arrived.
    pub fn poll(&self) -> Option<Storage> {
        self.0.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn gib(n: u64) -> Option<u64> {
        Some(n * GIB)
    }

    #[test]
    fn vendors_from_pci_ids() {
        assert_eq!(vendor_from_id(0x8086), Vendor::Intel);
        assert_eq!(vendor_from_id(0x1002), Vendor::Amd);
        assert_eq!(vendor_from_id(0x10DE), Vendor::Nvidia);
        assert_eq!(vendor_from_id(0x1414), Vendor::Other);
        assert_eq!(vendor_from_id(0), Vendor::Other);
    }

    #[test]
    fn gpu_kinds_from_device_types() {
        use wgpu::DeviceType as D;
        assert_eq!(gpu_kind(D::IntegratedGpu, "Intel(R) UHD Graphics 770"), GpuKind::Integrated);
        assert_eq!(gpu_kind(D::DiscreteGpu, "NVIDIA GeForce RTX 4070"), GpuKind::Discrete);
        assert_eq!(gpu_kind(D::Cpu, "llvmpipe (LLVM 15.0.7, 256 bits)"), GpuKind::Software);
        assert_eq!(gpu_kind(D::Cpu, "Microsoft Basic Render Driver"), GpuKind::Software);
        // WARP reporting itself as something else is still software.
        assert_eq!(gpu_kind(D::VirtualGpu, "Microsoft Basic Render Driver"), GpuKind::Software);
        assert_eq!(gpu_kind(D::VirtualGpu, "VMware SVGA"), GpuKind::Unknown);
        assert_eq!(gpu_kind(D::Other, "x"), GpuKind::Unknown);
    }

    #[test]
    fn tiers() {
        let hw = Some(GpuKind::Integrated);
        // RAM: under 6 GiB is Low, under 12 Mid, else High.
        assert_eq!(classify(gib(4), 8, hw), Tier::Low);
        assert_eq!(classify(Some(3 * GIB + GIB * 9 / 10), 8, hw), Tier::Low, "a 4 GB machine reports about 3.9 GiB");
        assert_eq!(classify(Some(5 * GIB + GIB * 8 / 10), 8, hw), Tier::Mid, "a 6 GB machine reports about 5.8 GiB, which is 6");
        assert_eq!(classify(Some(7 * GIB + GIB * 8 / 10), 8, hw), Tier::Mid, "an 8 GB machine reports about 7.8 GiB");
        assert_eq!(classify(gib(8), 8, hw), Tier::Mid);
        assert_eq!(classify(Some(11 * GIB + GIB * 8 / 10), 8, hw), Tier::High, "12 GB");
        assert_eq!(classify(gib(16), 8, hw), Tier::High);
        assert_eq!(classify(gib(32), 14, Some(GpuKind::Discrete)), Tier::High);
        // Cores: two or fewer is Low, four or fewer at most Mid.
        assert_eq!(classify(gib(32), 2, hw), Tier::Low);
        assert_eq!(classify(gib(32), 4, hw), Tier::Mid);
        assert_eq!(classify(gib(32), 6, hw), Tier::High);
        // N100: 4 cores, 8 GB.
        assert_eq!(classify(gib(8), 4, hw), Tier::Mid);
        // Software graphics are Low whatever else there is.
        assert_eq!(classify(gib(32), 14, Some(GpuKind::Software)), Tier::Low);
        // Unknown RAM is Mid; unknown graphics do not count against it.
        assert_eq!(classify(None, 8, None), Tier::Mid);
        assert_eq!(classify(None, 2, None), Tier::Low);
        assert_eq!(classify(gib(16), 8, None), Tier::High);
    }

    fn machine(ram: Option<u64>, storage: Storage) -> Machine {
        let gpu = GpuSummary { name: "Intel(R) UHD Graphics 770".into(), vendor: Vendor::Intel, kind: GpuKind::Integrated, backend: "Vulkan" };
        Machine { ram, physical_cores: 4, gpu: Some(gpu), storage }
    }

    #[test]
    fn log_line_names_the_machine() {
        let m = machine(Some(8 * GIB - GIB / 5), Storage::Ssd);
        assert_eq!(
            m.log_line(),
            "machine: RAM 7.8 GiB · 4 cores · Intel(R) UHD Graphics 770 (integrated, Vulkan) · SSD · tier Mid"
        );
        let m = machine(None, Storage::Unknown);
        assert_eq!(m.log_line(), "machine: RAM unknown · 4 cores · Intel(R) UHD Graphics 770 (integrated, Vulkan) · tier Mid");
    }

    #[test]
    fn summary_rounds_ram_up_and_omits_the_unknown() {
        let _lang = crate::text::lang_for_test(crate::text::Lang::En);
        assert_eq!(machine(Some(8 * GIB - GIB / 5), Storage::Ssd).summary(), "RAM 8 GB · Intel graphics · SSD");
        assert_eq!(machine(Some(4 * GIB), Storage::Unknown).summary(), "RAM 4 GB · Intel graphics");
        assert_eq!(machine(None, Storage::Hdd).summary(), "Intel graphics · HDD");
        let mut m = machine(Some(16 * GIB), Storage::Unknown);
        m.gpu.as_mut().unwrap().kind = GpuKind::Software;
        assert_eq!(m.summary(), "RAM 16 GB · Software graphics");
        crate::text::set_current_lang(crate::text::Lang::Th);
        assert_eq!(machine(Some(8 * GIB), Storage::Ssd).summary(), "RAM 8 GB · การ์ดจอ Intel · SSD");
    }

    #[test]
    fn igpu_memory_is_shared_unless_discrete() {
        let mut m = machine(Some(8 * GIB), Storage::Unknown);
        assert!(m.gpu_shares_ram());
        m.gpu.as_mut().unwrap().kind = GpuKind::Discrete;
        assert!(!m.gpu_shares_ram());
        m.gpu = None;
        assert!(m.gpu_shares_ram(), "unknown graphics count as shared");
    }
}
