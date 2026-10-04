//! The IO thread: owns the editing [`Session`] and runs every load and save
//! in the background, one at a time, so the UI never waits on the disk.
//!
//! The UI sends [`Request`]s and polls [`IoEvent`]s each frame. Before each
//! job the thread drains its queue, and an autosave is dropped when a later
//! autosave or save is queued: only the newest state matters. Saves are
//! never dropped. Snapshots are dropped on this thread, so freeing tiles
//! never stalls a frame.
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

pub struct IoConfig {
    /// Threads of the io pool.
    pub threads: usize,
    pub recovery_dir: PathBuf,
    pub load: LoadOptions,
}

impl IoConfig {
    /// `cores - 1` io threads, and a pixel budget for loads of 75% of
    /// physical memory (at most 16 GiB).
    pub fn new(recovery_dir: PathBuf) -> Self {
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(1).max(1));
        let mut load = LoadOptions::default();
        if let Some(ram) = sys::physical_memory() {
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
    /// autosave of the new session.
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
    /// deleted after the first autosave.
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
            if !superseded {
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
                    Ok(stats) => self.emit(IoEvent::Saved { ticket, path, rev, stats }),
                    Err(e) => self.failed(ticket, "save", e),
                }
            }
            Request::Autosave { doc, ex, rev } => match self.session.autosave(&doc, &ex, rev, &self.pool, p) {
                Ok(stats) => {
                    // The new recovery file holds the restored state now.
                    if let Some((path, lock)) = self.restored.take() {
                        if let Err(e) = discard_files(&path) {
                            log::warn!("{e}");
                        }
                        lock.release();
                    }
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

/// The OS calls behind the io pool's priority and the load budget.
#[cfg(windows)]
#[allow(unsafe_code)]
mod sys {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};

    pub fn lower_thread_priority() {
        // SAFETY: GetCurrentThread returns a pseudo handle that is always
        // valid for the calling thread; SetThreadPriority only reads it.
        let ok = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) };
        if ok == 0 {
            log::debug!("could not lower the io thread priority");
        }
    }

    pub fn physical_memory() -> Option<u64> {
        let mut s = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
        // SAFETY: `s` is a live, writable MEMORYSTATUSEX with dwLength set,
        // as the call requires.
        let ok = unsafe { GlobalMemoryStatusEx(&mut s) };
        (ok != 0 && s.ullTotalPhys > 0).then_some(s.ullTotalPhys)
    }
}

#[cfg(not(windows))]
mod sys {
    pub fn lower_thread_priority() {}

    pub fn physical_memory() -> Option<u64> {
        None
    }
}
