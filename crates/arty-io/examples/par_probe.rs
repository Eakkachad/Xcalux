//! How thread pools size themselves under an affinity mask or a job
//! (plans/bench B013): prints `std::thread::available_parallelism`, the
//! rayon global pool (honours `RAYON_NUM_THREADS`), `IoConfig::new`'s io
//! pool, the process affinity mask, and the CPUs that busy rayon tasks
//! actually ran on.
//!
//! cargo run --release -p arty-io --example par_probe

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use rayon::prelude::*;

#[cfg(windows)]
fn affinity() -> String {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessAffinityMask};
    let (mut process, mut system) = (0usize, 0usize);
    // SAFETY: plain Win32 call on the current process's pseudo-handle.
    let ok = unsafe { GetProcessAffinityMask(GetCurrentProcess(), &mut process, &mut system) };
    if ok == 0 { "?".into() } else { format!("process {process:#X} of system {system:#X} ({} CPUs)", process.count_ones()) }
}

#[cfg(windows)]
fn cpu_now() -> u32 {
    // SAFETY: no arguments, no preconditions.
    unsafe { windows_sys::Win32::System::Threading::GetCurrentProcessorNumber() }
}

#[cfg(not(windows))]
fn affinity() -> String {
    "n/a".into()
}

#[cfg(not(windows))]
fn cpu_now() -> u32 {
    0
}

fn main() {
    let cpus = arty_io::usable_cpus();
    let mut builder = rayon::ThreadPoolBuilder::new().thread_name(|idx| format!("arty-rayon-{idx}"));
    if std::env::var("RAYON_NUM_THREADS").ok().filter(|s| !s.trim().is_empty()).is_none() {
        builder = builder.num_threads(arty_io::default_rayon_threads(cpus.logical));
    }
    let _ = builder.build_global();

    let io = arty_io::IoConfig::new(std::env::temp_dir());
    println!("usable_cpus physical {} · logical {}", cpus.physical, cpus.logical);
    println!("available_parallelism {}", std::thread::available_parallelism().map_or(0, |n| n.get()));
    println!("rayon global pool {} threads (RAYON_NUM_THREADS={:?})", rayon::current_num_threads(), std::env::var("RAYON_NUM_THREADS").ok());
    println!("IoConfig::new io pool {} threads", io.threads);
    println!("affinity {}", affinity());
    // Busy tasks on every rayon thread for ~0.3 s: which CPUs did they run on?
    let until = Instant::now() + Duration::from_millis(300);
    let cpus: BTreeSet<u32> = (0..rayon::current_num_threads() * 4)
        .into_par_iter()
        .flat_map_iter(|_| {
            let mut seen = BTreeSet::new();
            while Instant::now() < until {
                seen.insert(cpu_now());
            }
            seen
        })
        .collect();
    println!("rayon tasks ran on CPUs {cpus:?}");
}
