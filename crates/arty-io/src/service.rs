//! The IO thread: owns the editing [`Session`] and runs every load and save
//! in the background, one at a time, so the UI never waits on the disk.
//!
//! The UI sends [`Request`]s and polls [`IoEvent`]s each frame. Before each
//! job the thread drains its queue, and an autosave is dropped (answered
//! with `Failed { error: Cancelled }`) when a later autosave or save is
//! queued: only the newest state matters. Saves are never dropped.
//! Snapshots are dropped on this thread, so freeing tiles never stalls a
//! frame.
//!
//! Classify, encode, decode and verify run on a dedicated rayon pool of
//! `cores - 1` threads at below-normal priority, so the compositor (global
//! pool, normal priority) keeps the frame rate during a save.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

use arty_core::Document;
use rayon::ThreadPool;

use crate::error::IoError;
use crate::reader::{LoadOptions, Loaded, load};
use crate::recovery::{RecoveryDir, RecoveryEntry, SessionLock, discard_files};
use crate::writer::{SaveExtras, SaveOptions, SaveStats, Session, SessionId};
use crate::{FileKind, Progress};

/// Identifies one request; events about it carry the same ticket.
pub type Ticket = u64;

/// Installed physical memory, when the OS reports it.
pub fn physical_memory() -> Option<u64> {
    sys::physical_memory()
}

/// Run the calling thread below normal priority, so background work (io,
/// freeing undo steps) does not preempt the UI thread.
pub fn lower_thread_priority() {
    sys::lower_thread_priority()
}

/// Physical cores and logical CPUs usable by this process, respecting affinity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsableCpus {
    pub physical: usize,
    pub logical: usize,
}

/// Thread count for the I/O pool given usable physical cores (physical / 2, clamped to 1..=7).
pub fn default_io_threads(usable_physical_cores: usize) -> usize {
    (usable_physical_cores / 2).clamp(1, 7)
}

/// Thread count for the Rayon global pool given usable logical CPUs (clamped to 1..=16).
pub fn default_rayon_threads(usable_logical_cpus: usize) -> usize {
    usable_logical_cpus.clamp(1, 16)
}

/// Pure sizing logic for both thread pools given usable CPU counts.
pub fn compute_pool_sizes(cpus: UsableCpus) -> (usize, usize) {
    (default_io_threads(cpus.physical), default_rayon_threads(cpus.logical))
}

/// Number of physical cores and logical CPUs usable by this process (respecting affinity).
pub fn usable_cpus() -> UsableCpus {
    sys::usable_cpus()
}

pub struct IoConfig {
    /// Threads of the io pool.
    pub threads: usize,
    pub recovery_dir: PathBuf,
    pub load: LoadOptions,
}

impl IoConfig {
    /// Half the usable physical cores (at most 7) as io threads, respecting process affinity,
    /// unless overridden by `ARTY_IO_THREADS`. The pixel budget for loads is 75% of physical
    /// memory (at most 16 GiB).
    ///
    /// Sizing by physical cores avoids stealing frame time from painting during background
    /// saves: plans/bench/B002_io.md T9 measured +6.0 ms frame p99 with 19 threads vs +0.6 ms
    /// with 7, while a full B4 600 dpi save still takes only 1.1 s.
    pub fn new(recovery_dir: PathBuf) -> Self {
        let threads = io_threads_override().unwrap_or_else(|| default_io_threads(usable_cpus().physical));
        let mut load = LoadOptions::default();
        if let Some(ram) = physical_memory() {
            load.limits.max_decoded_bytes = load.limits.max_decoded_bytes.min(ram / 4 * 3);
        }
        Self { threads, recovery_dir, load }
    }
}

pub enum Request {
    /// Load a file; on success it becomes this session's document.
    Open { path: PathBuf },
    /// Save `doc` (revision `rev`) to `path` (see `Session::save_main`).
    Save { doc: Box<Document>, ex: SaveExtras, path: PathBuf, rev: u64, overwrite_external: bool },
    /// Write `doc` to the recovery file.
    Autosave { doc: Box<Document>, ex: SaveExtras, rev: u64 },
    /// Load a recovery file found by a scan. It is deleted after the first
    /// autosave or save of the new session.
    Restore { entry: RecoveryEntry },
    /// Delete a recovery file found by a scan.
    Discard { entry: RecoveryEntry },
    /// End the session (deleting its recovery file) and start a new one,
    /// for a new document.
    NewSession,
    /// Unpin cached tiles the document no longer uses (every minute).
    Trim,
    /// End the session, keeping its recovery file unless
    /// `discard_recovery`.
    CloseSession { discard_recovery: bool },
    /// Sort the recovery folder; answers with `RecoveryFound`.
    ScanRecovery,
    /// Stop after the requests queued before this one.
    Shutdown,
    /// Block the IO thread until the sender side is used or dropped
    /// (tests: lets requests queue up).
    #[cfg(any(test, feature = "fault-injection"))]
    Pause(Receiver<()>),
}

impl Request {
    fn is_save(&self) -> bool {
        matches!(self, Request::Autosave { .. } | Request::Save { .. })
    }
}

pub enum IoEvent {
    Saved { ticket: Ticket, path: PathBuf, rev: u64, stats: SaveStats },
    Autosaved { ticket: Ticket, rev: u64, stats: SaveStats },
    /// `path` is the main file (`None` for v1 imports, and for restored
    /// documents that never had one).
    Loaded { ticket: Ticket, path: Option<PathBuf>, loaded: Box<Loaded> },
    RecoveryFound(Vec<RecoveryEntry>),
    Failed { ticket: Ticket, op: &'static str, error: IoError },
}

/// Which job the IO thread is running, and which one the UI cancelled.
#[derive(Default)]
struct CancelState {
    current: Ticket,
    cancelled: Ticket,
}

#[derive(Default)]
struct Shared {
    progress: Progress,
    /// Requests sent and not yet finished (or dropped).
    pending: AtomicUsize,
    cancel: Mutex<CancelState>,
}

/// Handle to the IO thread. Dropping it shuts the thread down like
/// [`IoService::shutdown`].
pub struct IoService {
    tx: Sender<(Ticket, Request)>,
    events: Receiver<IoEvent>,
    shared: Arc<Shared>,
    next_ticket: AtomicU64,
    thread: Option<JoinHandle<()>>,
}

impl IoService {
    /// Start the IO thread. `wake` is called after each event is queued
    /// (the app passes `ctx.request_repaint`).
    pub fn spawn(cfg: IoConfig, wake: impl Fn() + Send + Sync + 'static) -> Result<Self, IoError> {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(cfg.threads.max(1))
            .thread_name(|i| format!("arty-io-{i}"))
            .start_handler(|_| sys::lower_thread_priority())
            .build()
            .map_err(|e| IoError::Io { op: "start the io threads", source: std::io::Error::other(e) })?;
        let (tx, rx) = channel();
        let (event_tx, events) = channel();
        let shared = Arc::new(Shared::default());
        let dir = RecoveryDir::new(cfg.recovery_dir);
        let worker = Worker {
            rx,
            events: event_tx,
            wake: Box::new(wake),
            shared: shared.clone(),
            pool,
            session: new_session(&dir),
            dir,
            load: cfg.load,
            restored: None,
        };
        let thread = std::thread::Builder::new()
            .name("arty-io".into())
            .spawn(move || worker.run())
            .map_err(IoError::io("start the io thread"))?;
        Ok(Self { tx, events, shared, next_ticket: AtomicU64::new(0), thread: Some(thread) })
    }

    /// Queue a request; events about it carry the returned ticket.
    pub fn send(&self, r: Request) -> Ticket {
        let ticket = self.next_ticket.fetch_add(1, Ordering::Relaxed) + 1;
        self.shared.pending.fetch_add(1, Ordering::SeqCst);
        if self.tx.send((ticket, r)).is_err() {
            self.shared.pending.fetch_sub(1, Ordering::SeqCst);
            log::error!("the io thread is gone");
        }
        ticket
    }

    pub fn try_recv(&self) -> Option<IoEvent> {
        self.events.try_recv().ok()
    }

    /// Progress of the running job.
    pub fn progress(&self) -> &Progress {
        &self.shared.progress
    }

    /// True while any request is queued or running. When it turns false,
    /// every event is already waiting in [`IoService::try_recv`].
    pub fn busy(&self) -> bool {
        self.shared.pending.load(Ordering::SeqCst) > 0
    }

    /// Stop the job of `ticket` at its next check (a load, save or
    /// autosave fails with `Cancelled`), or skip it if it has not started.
    pub fn cancel(&self, ticket: Ticket) {
        let mut c = self.shared.cancel.lock().unwrap_or_else(PoisonError::into_inner);
        c.cancelled = ticket;
        if c.current == ticket {
            self.shared.progress.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Finish the requests already queued, then stop the thread and wait
    /// for it. The session's lock is released; its recovery file is kept
    /// unless a `CloseSession { discard_recovery: true }` came first.
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        if let Some(thread) = self.thread.take() {
            self.send(Request::Shutdown);
            if thread.join().is_err() {
                log::error!("the io thread panicked");
            }
        }
    }
}

impl Drop for IoService {
    fn drop(&mut self) {
        self.stop();
    }
}

fn new_session(dir: &RecoveryDir) -> Session {
    let id = SessionId::random().unwrap_or_else(|e| {
        // Unique enough without the OS generator: time and process.
        log::warn!("random session id: {e}");
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let mut id = [0u8; 16];
        id[..12].copy_from_slice(&t.to_le_bytes()[..12]);
        id[12..].copy_from_slice(&std::process::id().to_le_bytes());
        SessionId(id)
    });
    Session::new(id, Some(dir.path()))
}

struct Worker {
    rx: Receiver<(Ticket, Request)>,
    events: Sender<IoEvent>,
    wake: Box<dyn Fn() + Send + Sync>,
    shared: Arc<Shared>,
    pool: ThreadPool,
    dir: RecoveryDir,
    load: LoadOptions,
    session: Session,
    /// The recovery file the document was restored from, with its lock;
    /// deleted after the first autosave or save.
    restored: Option<(PathBuf, SessionLock)>,
}

impl Worker {
    fn run(mut self) {
        let mut queue: VecDeque<(Ticket, Request)> = VecDeque::new();
        loop {
            if queue.is_empty() {
                match self.rx.recv() {
                    Ok(r) => queue.push_back(r),
                    Err(_) => break,
                }
            }
            queue.extend(self.rx.try_iter());
            let Some((ticket, req)) = queue.pop_front() else { continue };
            let superseded = matches!(req, Request::Autosave { .. }) && queue.iter().any(|(_, r)| r.is_save());
            let stop = matches!(req, Request::Shutdown);
            if superseded {
                // Answered all the same: the app tracks its autosave in
                // flight by ticket.
                self.emit(IoEvent::Failed { ticket, op: "autosave", error: IoError::Cancelled });
            } else {
                self.begin(ticket);
                self.handle(ticket, req);
                self.begin(0);
            }
            // After the event is sent: once idle, every event is queued.
            self.shared.pending.fetch_sub(1, Ordering::SeqCst);
            if stop {
                break;
            }
        }
        // Keep the recovery file: the app discards it explicitly.
        self.end_session(false);
    }

    /// Mark `ticket` as the running job, cancelled if the UI already asked.
    fn begin(&self, ticket: Ticket) {
        let mut c = self.shared.cancel.lock().unwrap_or_else(PoisonError::into_inner);
        c.current = ticket;
        let cancelled = ticket != 0 && c.cancelled == ticket;
        self.shared.progress.cancel.store(cancelled, Ordering::Relaxed);
    }

    fn emit(&self, e: IoEvent) {
        if self.events.send(e).is_ok() {
            (self.wake)();
        }
    }

    fn failed(&self, ticket: Ticket, op: &'static str, error: IoError) {
        log::warn!("{op}: {error}");
        self.emit(IoEvent::Failed { ticket, op, error });
    }

    /// End the session and start a new one. `discard` also deletes its
    /// recovery file and a restored file it still kept.
    fn end_session(&mut self, discard: bool) {
        let old = std::mem::replace(&mut self.session, new_session(&self.dir));
        if let Err(e) = old.close(discard) {
            log::warn!("closing the session: {e}");
        }
        if let Some((path, lock)) = self.restored.take() {
            if discard && let Err(e) = discard_files(&path) {
                log::warn!("{e}");
            }
            lock.release();
        }
    }

    /// Delete the restored recovery file once another file holds its state.
    fn drop_restored(&mut self) {
        if let Some((path, lock)) = self.restored.take() {
            if let Err(e) = discard_files(&path) {
                log::warn!("{e}");
            }
            lock.release();
        }
    }

    fn handle(&mut self, ticket: Ticket, req: Request) {
        let shared = self.shared.clone();
        let p = &shared.progress;
        match req {
            Request::Open { path } => {
                if p.is_cancelled() {
                    return self.failed(ticket, "open", IoError::Cancelled);
                }
                match load(&path, &self.load, &self.pool, p) {
                    Ok(mut loaded) => {
                        self.end_session(true);
                        // A v1 import is saved under a new name.
                        let main = (loaded.info.kind != FileKind::LegacyV1).then_some(path);
                        self.session.adopt(&mut loaded, main.clone());
                        self.emit(IoEvent::Loaded { ticket, path: main, loaded: Box::new(loaded) });
                    }
                    Err(e) => self.failed(ticket, "open", e),
                }
            }
            Request::Save { doc, ex, path, rev, overwrite_external } => {
                let o = SaveOptions::default();
                match self.session.save_main(&doc, &ex, &path, overwrite_external, &o, &self.pool, p) {
                    Ok(stats) => {
                        // The main file holds the restored state now.
                        self.drop_restored();
                        self.emit(IoEvent::Saved { ticket, path, rev, stats });
                    }
                    Err(e) => self.failed(ticket, "save", e),
                }
            }
            Request::Autosave { doc, ex, rev } => match self.session.autosave(&doc, &ex, rev, &self.pool, p) {
                Ok(stats) => {
                    // The new recovery file holds the restored state now.
                    self.drop_restored();
                    self.emit(IoEvent::Autosaved { ticket, rev, stats });
                }
                Err(e) => self.failed(ticket, "autosave", e),
            },
            Request::Restore { entry } => {
                let lock = match self.dir.claim(&entry) {
                    Ok(lock) => lock,
                    Err(e) => return self.failed(ticket, "restore", e),
                };
                let o = LoadOptions { fallback_to_previous: true, ..self.load.clone() };
                let r = if p.is_cancelled() { Err(IoError::Cancelled) } else { load(&entry.path, &o, &self.pool, p) };
                match r {
                    Ok(mut loaded) => {
                        self.end_session(true);
                        self.session.adopt_restored(&mut loaded, &entry.path, entry.src.clone());
                        self.restored = Some((entry.path, lock));
                        self.emit(IoEvent::Loaded { ticket, path: entry.src, loaded: Box::new(loaded) });
                    }
                    Err(e) => {
                        lock.release();
                        self.failed(ticket, "restore", e);
                    }
                }
            }
            Request::Discard { entry } => {
                if let Err(e) = self.dir.discard(&entry) {
                    self.failed(ticket, "discard the recovery file", e);
                }
            }
            Request::NewSession => self.end_session(true),
            Request::Trim => self.session.trim(),
            Request::CloseSession { discard_recovery } => self.end_session(discard_recovery),
            Request::ScanRecovery => {
                let found = self.dir.scan();
                self.emit(IoEvent::RecoveryFound(found));
            }
            Request::Shutdown => {}
            #[cfg(any(test, feature = "fault-injection"))]
            Request::Pause(rx) => {
                let _ = rx.recv();
            }
        }
    }
}

/// `ARTY_RAM_MB=<MiB>` (bench runs, plans/bench/B013): the memory size the
/// budgets are sized from, instead of the machine's, so a run on a big box
/// gets a small machine's budgets.
fn ram_override() -> Option<u64> {
    let s = std::env::var("ARTY_RAM_MB").ok()?;
    let v = parse_ram_mb(&s);
    if v.is_none() {
        log::warn!("ARTY_RAM_MB: ignoring invalid value {s:?}");
    }
    v
}

/// MiB, 256..=1 TiB, as bytes.
fn parse_ram_mb(s: &str) -> Option<u64> {
    s.trim().parse::<u64>().ok().filter(|mb| (256..=1 << 20).contains(mb)).map(|mb| mb << 20)
}

/// `ARTY_IO_THREADS=<n>` (bench runs, plans/bench/B013): io pool thread count override.
fn io_threads_override() -> Option<usize> {
    let s = std::env::var("ARTY_IO_THREADS").ok()?;
    let v = parse_io_threads(&s);
    if v.is_none() {
        log::warn!("ARTY_IO_THREADS: ignoring invalid value {s:?}");
    }
    v
}

/// Thread count, 1..=64.
fn parse_io_threads(s: &str) -> Option<usize> {
    s.trim().parse::<usize>().ok().filter(|v| (1..=64).contains(v))
}

/// The OS calls behind the io pool's priority and the load budget.
#[cfg(windows)]
#[allow(unsafe_code)]
mod sys {
    use windows_sys::Win32::System::JobObjects::{
        JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, QueryInformationJobObject,
    };
    use windows_sys::Win32::System::SystemInformation::{
        GlobalMemoryStatusEx, MEMORYSTATUSEX, GetLogicalProcessorInformationEx, RelationProcessorCore,
        SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX, GROUP_AFFINITY,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentThread, GetProcessAffinityMask, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
    };

    /// Lowers thread priority to below-normal so background IO and memory trimming do
    /// not preempt UI rendering.
    ///
    /// Note on `THREAD_MODE_BACKGROUND_BEGIN`: Windows background processing mode lowers
    /// both CPU priority (to 4/idle) and disk I/O priority to `VeryLow`, which throttles
    /// and defers file operations indefinitely while foreground activity is continuous.
    /// In ARTY, the UI thread paints at up to 240 Hz, which would starve background saves
    /// and autosave recovery writes, risking data loss or stalling the app on shutdown.
    /// Therefore, we use `THREAD_PRIORITY_BELOW_NORMAL` without background I/O mode.
    pub fn lower_thread_priority() {
        // SAFETY: GetCurrentThread returns a pseudo handle that is always
        // valid for the calling thread; SetThreadPriority only reads it.
        let ok = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) };
        if ok == 0 {
            log::debug!("could not lower the io thread priority");
        }
    }

    /// Usable physical cores and logical CPUs respecting process affinity.
    pub fn usable_cpus() -> super::UsableCpus {
        let fallback = || {
            let logical = std::thread::available_parallelism().map_or(1, |n| n.get());
            super::UsableCpus { physical: logical, logical }
        };

        let mut len = 0u32;
        // First call to determine required buffer size.
        // SAFETY: null buffer pointer returns 0 and sets `len`.
        let _ = unsafe { GetLogicalProcessorInformationEx(RelationProcessorCore, std::ptr::null_mut(), &mut len) };
        if len == 0 {
            return fallback();
        }

        let mut buf = vec![0u8; len as usize];
        // SAFETY: `buf` has `len` bytes and is writable.
        let ok = unsafe {
            GetLogicalProcessorInformationEx(
                RelationProcessorCore,
                buf.as_mut_ptr().cast(),
                &mut len,
            )
        };
        if ok == 0 {
            return fallback();
        }

        let mut process_mask = 0usize;
        let mut system_mask = 0usize;
        // SAFETY: GetCurrentProcess returns a valid pseudo-handle for the current process.
        let ok = unsafe { GetProcessAffinityMask(GetCurrentProcess(), &mut process_mask, &mut system_mask) };
        if ok == 0 || process_mask == 0 {
            process_mask = if system_mask != 0 { system_mask } else { !0usize };
        }

        let mut usable_physical = 0usize;
        let mut usable_logical = 0usize;

        let mut offset = 0usize;
        let len_usize = len as usize;
        // Each record starts with Relationship (i32) and Size (u32) = 8 bytes.
        let min_header_size = 8usize;

        while offset + min_header_size <= len_usize {
            // SAFETY: offset + min_header_size <= len_usize and record header is within bounds.
            let info = unsafe { &*buf.as_ptr().add(offset).cast::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>() };
            let record_size = info.Size as usize;
            if record_size < min_header_size || offset + record_size > len_usize {
                break;
            }

            if info.Relationship == RelationProcessorCore {
                // SAFETY: Relationship is RelationProcessorCore, so Processor union variant is active.
                let proc = unsafe { &info.Anonymous.Processor };
                let group_count = proc.GroupCount as usize;
                // Offset of GroupMask is 32 (8 header + 24 struct prefix).
                let min_needed = 32 + group_count * size_of::<GROUP_AFFINITY>();
                if record_size >= min_needed && group_count > 0 {
                    // SAFETY: record_size >= min_needed ensures all group_count GROUP_AFFINITY structs are within buf.
                    let groups = unsafe { std::slice::from_raw_parts(proc.GroupMask.as_ptr(), group_count) };
                    let mut core_has_usable = false;
                    for group_aff in groups {
                        if group_aff.Group == 0 {
                            let usable_in_group = group_aff.Mask & process_mask;
                            if usable_in_group != 0 {
                                core_has_usable = true;
                                usable_logical += usable_in_group.count_ones() as usize;
                            }
                        }
                    }
                    if core_has_usable {
                        usable_physical += 1;
                    }
                }
            }

            offset += record_size;
        }

        if usable_physical == 0 || usable_logical == 0 {
            fallback()
        } else {
            super::UsableCpus { physical: usable_physical, logical: usable_logical }
        }
    }

    /// `ARTY_RAM_MB`, else installed RAM capped by the job's memory limit,
    /// if the process runs in a job that has one (as .NET's GC does): the
    /// OS figure ignores the job.
    pub fn physical_memory() -> Option<u64> {
        if let Some(ram) = super::ram_override() {
            return Some(ram);
        }
        let mut s = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
        // SAFETY: `s` is a live, writable MEMORYSTATUSEX with dwLength set,
        // as the call requires.
        let ok = unsafe { GlobalMemoryStatusEx(&mut s) };
        (ok != 0 && s.ullTotalPhys > 0).then(|| job_memory_limit().map_or(s.ullTotalPhys, |cap| cap.min(s.ullTotalPhys)))
    }

    /// The smaller of the job and per-process commit limits of the job this
    /// process is in, if any.
    fn job_memory_limit() -> Option<u64> {
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        // SAFETY: a null handle means the calling process's job; `info` is a
        // live, writable struct of the size passed, and the return length
        // pointer may be null.
        let ok = unsafe {
            QueryInformationJobObject(
                std::ptr::null_mut(),
                JobObjectExtendedLimitInformation,
                (&raw mut info).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return None;
        }
        let flags = info.BasicLimitInformation.LimitFlags;
        let job = (flags & JOB_OBJECT_LIMIT_JOB_MEMORY != 0).then_some(info.JobMemoryLimit as u64);
        let process = (flags & JOB_OBJECT_LIMIT_PROCESS_MEMORY != 0).then_some(info.ProcessMemoryLimit as u64);
        job.into_iter().chain(process).filter(|&b| b > 0).min()
    }
}

#[cfg(not(windows))]
mod sys {
    pub fn lower_thread_priority() {}

    pub fn physical_memory() -> Option<u64> {
        super::ram_override()
    }

    pub fn usable_cpus() -> super::UsableCpus {
        let logical = std::thread::available_parallelism().map_or(1, |n| n.get());
        super::UsableCpus { physical: logical, logical }
    }
}

#[cfg(test)]
mod sys_tests {
    use super::*;

    #[test]
    fn pure_sizing_logic() {
        // I/O pool: physical / 2, clamp(1, 7)
        assert_eq!(default_io_threads(0), 1);
        assert_eq!(default_io_threads(1), 1);
        assert_eq!(default_io_threads(2), 1);
        assert_eq!(default_io_threads(3), 1);
        assert_eq!(default_io_threads(4), 2); // N100 (4 cores) gives 2
        assert_eq!(default_io_threads(8), 4);
        assert_eq!(default_io_threads(14), 7); // Dev box (14 cores) gives 7
        assert_eq!(default_io_threads(16), 7); // Capped at 7
        assert_eq!(default_io_threads(64), 7);

        // Rayon global pool: logical, clamp(1, 16)
        assert_eq!(default_rayon_threads(0), 1);
        assert_eq!(default_rayon_threads(1), 1);
        assert_eq!(default_rayon_threads(4), 4); // N100 or 4-core emu gives 4
        assert_eq!(default_rayon_threads(8), 8);
        assert_eq!(default_rayon_threads(16), 16);
        assert_eq!(default_rayon_threads(20), 16); // Dev box (20 logical) capped at 16
        assert_eq!(default_rayon_threads(64), 16);

        let sizes = compute_pool_sizes(UsableCpus { physical: 4, logical: 4 });
        assert_eq!(sizes, (2, 4));

        let sizes = compute_pool_sizes(UsableCpus { physical: 14, logical: 20 });
        assert_eq!(sizes, (7, 16));
    }

    #[test]
    fn ram_override_parses_mib() {
        assert_eq!(parse_ram_mb("8192"), Some(8 << 30));
        assert_eq!(parse_ram_mb(" 4096 "), Some(4 << 30));
        for bad in ["", "0", "255", "-1", "8G", "2000000"] {
            assert_eq!(parse_ram_mb(bad), None, "{bad}");
        }
    }

    #[test]
    fn io_threads_override_parses() {
        assert_eq!(parse_io_threads("2"), Some(2));
        assert_eq!(parse_io_threads(" 4 "), Some(4));
        for bad in ["", "0", "65", "-1", "two"] {
            assert_eq!(parse_io_threads(bad), None, "{bad}");
        }
    }

    #[test]
    fn physical_memory_is_plausible() {
        // Under a job memory cap (plans/bench/B013) this is the cap.
        if let Some(ram) = sys::physical_memory() {
            assert!(ram >= 256 << 20, "{ram}");
        }
    }

    #[test]
    fn usable_cpus_is_plausible() {
        let cpus = usable_cpus();
        assert!(cpus.physical >= 1, "physical: {}", cpus.physical);
        assert!(cpus.logical >= 1, "logical: {}", cpus.logical);
        assert!(cpus.physical <= cpus.logical, "physical <= logical: {cpus:?}");
    }
}
