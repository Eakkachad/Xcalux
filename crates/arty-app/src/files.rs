//! Open, Save, Save As, autosave, recovery and closing: the app side of
//! `arty-io`. Loads and saves run on the IO thread ([`IoService`]); this
//! controller sends requests, polls events each frame, and owns the
//! dialogs around them (unsaved changes, progress, warnings, recovery).
//!
//! Dirty state is `doc.revision() != saved_rev`. Autosave captures
//! `revision + view_revision` and follows [`autosave_due`].

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;

use arty_core::Document;
use arty_io::{AppSection, FileKind, IoError, IoEvent, IoService, LayerExt, Loaded, RecoveryEntry, Request, SaveExtras, Ticket};
use arty_io::selm::SelectionSave;
use arty_render::View;
use arty_render::view::{MAX_ZOOM, MIN_ZOOM};
use egui::{RichText, ViewportCommand};
use serde::{Deserialize, Serialize};

use crate::shell::{FileRequest, Shell};
use crate::studio::Studio;

/// Seconds without input before a due autosave runs.
const IDLE_SECS: f64 = 2.0;
/// Shortest autosave interval the settings allow.
pub const MIN_INTERVAL_SECS: u32 = 10;
/// Seconds between trims of the IO thread's tile cache.
const TRIM_SECS: f64 = 60.0;
/// `VIEW` section layout version.
const VIEW_V1: u8 = 1;
const VIEW_LEN: usize = 18;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutosaveSettings {
    pub enabled: bool,
    pub interval_secs: u32,
}

impl Default for AutosaveSettings {
    fn default() -> Self {
        Self { enabled: true, interval_secs: 60 }
    }
}

/// What the autosave policy decided this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Due {
    No,
    Now,
    /// Not yet; check again after this many seconds.
    After(f64),
}

/// The state the autosave policy looks at.
#[derive(Debug, Clone, Copy)]
pub struct AutosaveState {
    pub now: f64,
    /// `revision + view_revision` now, and at the last autosave (or at
    /// load; `None` when the document must be autosaved once anyway).
    pub key: u64,
    pub captured: Option<u64>,
    /// When the last autosave was sent (or the document loaded).
    pub since: f64,
    pub last_input: f64,
    pub stroking: bool,
    pub in_flight: bool,
    /// The window lost focus this frame.
    pub focus_lost: bool,
}

/// An autosave is due when it is enabled, the document changed since the
/// last one, no stroke or autosave is in progress, the interval has
/// passed, and the user is idle for 2 s, or 3 intervals have passed, or the
/// window just lost focus.
pub fn autosave_due(settings: &AutosaveSettings, s: &AutosaveState) -> Due {
    if !settings.enabled || s.captured == Some(s.key) || s.stroking || s.in_flight {
        return Due::No;
    }
    let interval = f64::from(settings.interval_secs.max(MIN_INTERVAL_SECS));
    let elapsed = s.now - s.since;
    if elapsed < interval {
        return Due::After(interval - elapsed);
    }
    let idle = s.now - s.last_input;
    if idle >= IDLE_SECS || elapsed >= 3.0 * interval || s.focus_lost {
        return Due::Now;
    }
    Due::After((IDLE_SECS - idle).min(3.0 * interval - elapsed))
}

/// `VIEW` v1: `u8 ver, f32 zoom, f32 rotation, u8 flip_x, f32 cx, f32 cy`.
pub fn encode_view(v: &View) -> Vec<u8> {
    let mut b = Vec::with_capacity(VIEW_LEN);
    b.push(VIEW_V1);
    b.extend_from_slice(&v.zoom.to_le_bytes());
    b.extend_from_slice(&v.rotation.to_le_bytes());
    b.push(u8::from(v.flip_x));
    b.extend_from_slice(&v.center[0].to_le_bytes());
    b.extend_from_slice(&v.center[1].to_le_bytes());
    b
}

/// The view saved by [`encode_view`]; `None` for other versions or
/// unusable values (the page is then fit to the window).
pub fn decode_view(b: &[u8]) -> Option<View> {
    if b.len() < VIEW_LEN || b[0] != VIEW_V1 {
        return None;
    }
    let f = |at: usize| f32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    let (zoom, rotation, cx, cy) = (f(1), f(5), f(10), f(14));
    if ![zoom, rotation, cx, cy].iter().all(|v| v.is_finite()) || zoom <= 0.0 {
        return None;
    }
    Some(View { center: [cx, cy], zoom: zoom.clamp(MIN_ZOOM, MAX_ZOOM), rotation, flip_x: b[9] != 0 })
}

/// Native file pickers (a trait so tests can answer them).
pub trait FileDialogs {
    fn open_path(&mut self) -> Option<PathBuf>;
    /// Where to save; `name` is the suggested file name.
    fn save_path(&mut self, name: &str, dir: Option<&Path>) -> Option<PathBuf>;
}

pub struct NativeDialogs;

impl FileDialogs for NativeDialogs {
    fn open_path(&mut self) -> Option<PathBuf> {
        rfd::FileDialog::new().add_filter("ARTY document", &["arty"]).pick_file()
    }

    fn save_path(&mut self, name: &str, dir: Option<&Path>) -> Option<PathBuf> {
        let mut d = rfd::FileDialog::new().add_filter("ARTY document", &["arty"]).set_file_name(name);
        if let Some(dir) = dir {
            d = d.set_directory(dir);
        }
        let path = d.save_file()?;
        Some(if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("arty")) { path } else { path.with_extension("arty") })
    }
}

/// What to do once unsaved changes are saved or dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Then {
    New,
    Open,
    Quit,
    /// Restore `FileController::restore`.
    Restore,
}

/// The load or save in flight (at most one; autosaves are tracked apart).
enum Job {
    Save { ticket: Ticket, key: u64, then: Option<Then> },
    /// Open or restore; `path` is what was asked for (names imports).
    Load { ticket: Ticket, path: Option<PathBuf>, restore: bool },
}

impl Job {
    fn ticket(&self) -> Ticket {
        match self {
            Job::Save { ticket, .. } | Job::Load { ticket, .. } => *ticket,
        }
    }
}

enum Modal {
    /// Save / Don't Save / Cancel.
    Unsaved(Then),
    /// Plain Save of a lossy document: Save As… / Cancel.
    Lossy(Option<Then>),
    /// The file changed on disk: Overwrite / Save As… / Cancel.
    External { path: PathBuf, then: Option<Then> },
    /// Open or restore in progress, with Cancel.
    Loading(Ticket),
    Message { title: &'static str, lines: Vec<String> },
    Recovery(Vec<RecoveryEntry>),
    /// Waiting for a save to finish before closing.
    Closing,
}

/// One frame's input to [`FileController::update`].
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameInput {
    pub now: f64,
    pub focused: bool,
    /// Any user input this frame.
    pub input: bool,
    pub close_requested: bool,
}

/// What [`FileController::update`] asks of the window.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameOutput {
    pub cancel_close: bool,
    pub close: bool,
    pub repaint_after: Option<f64>,
}

pub struct FileController {
    io: Option<IoService>,
    /// Why there is no IO thread, shown when a file action is attempted.
    io_error: Option<String>,
    dialogs: Box<dyn FileDialogs>,
    path: Option<PathBuf>,
    name: String,
    saved_rev: Option<u64>,
    epoch: u64,
    read_only_reason: Option<String>,
    extra_sections: Vec<AppSection>,
    layer_ext: Vec<LayerExt>,
    job: Option<Job>,
    autosave: Option<Ticket>,
    captured: Option<u64>,
    captured_at: f64,
    last_input: f64,
    last_trim: f64,
    focused: bool,
    modal: Option<Modal>,
    /// Recovery files found while another dialog was open.
    found: Vec<RecoveryEntry>,
    /// The recovery file chosen in the prompt, restored by `Then::Restore`.
    restore: Option<RecoveryEntry>,
    allow_close: bool,
    closing: bool,
    now: f64,
    title: String,
    title_key: Option<(bool, u64)>,
    title_changed: bool,
    /// Bumped when `name` changes (title key).
    name_gen: u64,
}

impl FileController {
    /// `io` is `Err` when the IO thread could not start; file actions then
    /// report why. Asks the IO thread for recovery files right away.
    pub fn new(io: Result<IoService, IoError>, dialogs: Box<dyn FileDialogs>, studio: &Studio) -> Self {
        let (io, io_error) = match io {
            Ok(io) => {
                io.send(Request::ScanRecovery);
                (Some(io), None)
            }
            Err(e) => {
                log::error!("io thread: {e}");
                (None, Some(e.to_string()))
            }
        };
        Self {
            io,
            io_error,
            dialogs,
            path: None,
            name: "Untitled".to_owned(),
            saved_rev: Some(studio.doc.revision()),
            epoch: studio.doc_epoch,
            read_only_reason: None,
            extra_sections: Vec::new(),
            layer_ext: Vec::new(),
            job: None,
            autosave: None,
            captured: Some(change_key(&studio.doc)),
            captured_at: 0.0,
            last_input: 0.0,
            last_trim: 0.0,
            focused: true,
            modal: None,
            found: Vec::new(),
            restore: None,
            allow_close: false,
            closing: false,
            now: 0.0,
            title: String::new(),
            title_key: None,
            title_changed: false,
            name_gen: 0,
        }
    }

    pub fn is_dirty(&self, doc: &Document) -> bool {
        self.saved_rev != Some(doc.revision())
    }

    /// A dialog of this controller is open (document shortcuts are off).
    pub fn has_modal(&self) -> bool {
        self.modal.is_some()
    }

    #[cfg(test)]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Run once per frame: IO events, File requests, close interception,
    /// autosave and the window title. Draw the dialogs with [`Self::ui`].
    pub fn tick(&mut self, ctx: &egui::Context, studio: &mut Studio, shell: &mut Shell) {
        let input = ctx.input(|i| FrameInput {
            now: i.time,
            focused: i.focused,
            input: !i.events.is_empty(),
            close_requested: i.viewport().close_requested(),
        });
        let out = self.update(&input, studio, shell);
        if out.cancel_close {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
        }
        if out.close {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        if let Some(secs) = out.repaint_after {
            ctx.request_repaint_after(Duration::from_secs_f64(secs.max(0.0)));
        }
        if self.title_changed {
            self.title_changed = false;
            ctx.send_viewport_cmd(ViewportCommand::Title(self.title.clone()));
        }
    }

    /// [`Self::tick`] without a window. Allocation-free when nothing
    /// happens.
    pub fn update(&mut self, f: &FrameInput, studio: &mut Studio, shell: &mut Shell) -> FrameOutput {
        let mut out = FrameOutput::default();
        self.now = f.now;
        if f.input {
            self.last_input = f.now;
        }
        let focus_lost = self.focused && !f.focused;
        self.focused = f.focused;

        // A document made by File > New.
        if studio.doc_epoch != self.epoch {
            self.start_untitled(studio);
        }
        while let Some(e) = self.io.as_ref().and_then(IoService::try_recv) {
            self.on_event(e, studio, shell);
        }
        if self.modal.is_none() && !self.found.is_empty() {
            self.modal = Some(Modal::Recovery(std::mem::take(&mut self.found)));
        }

        if f.close_requested {
            self.on_close_requested(studio, &mut out);
        }
        if self.closing && !self.busy() {
            self.closing = false;
            if matches!(self.modal, Some(Modal::Closing)) {
                self.modal = None;
            }
            // A load may have finished while waiting (a restore is dirty).
            // `allow_close` stays unset, so a load whose event is still
            // queued is asked about when the close comes back.
            if !self.allow_close && self.is_dirty(&studio.doc) {
                self.modal = Some(Modal::Unsaved(Then::Quit));
            } else {
                out.close = true;
            }
        }

        // Requests wait for dialogs, loads and saves in flight, and (to
        // snapshot a whole stroke) the end of a stroke.
        if let Some(req) = shell.file_request
            && self.modal.is_none()
            && self.job.is_none()
            && !studio.engine.is_stroking()
        {
            shell.file_request = None;
            // Save, Save As, New and Open take the transformed pixels.
            studio.commit_transform();
            self.on_request(req, studio, shell);
        }

        let state = AutosaveState {
            now: f.now,
            key: change_key(&studio.doc),
            captured: self.captured,
            since: self.captured_at,
            last_input: self.last_input,
            // A transform session's preview is not a state to recover.
            stroking: studio.engine.is_stroking() || studio.transform.is_some(),
            in_flight: self.autosave.is_some() || self.job.is_some(),
            focus_lost,
        };
        match autosave_due(&shell.autosave, &state) {
            Due::Now => self.send_autosave(studio),
            Due::After(secs) => out.repaint_after = Some(secs),
            Due::No => {}
        }
        if self.job.is_some() || self.autosave.is_some() || self.closing {
            // Progress in the status bar.
            out.repaint_after = Some(out.repaint_after.map_or(0.1, |s| s.min(0.1)));
        }
        if f.now - self.last_trim >= TRIM_SECS {
            self.last_trim = f.now;
            if let Some(io) = &self.io {
                io.send(Request::Trim);
            }
        }
        self.update_title(&studio.doc);
        out
    }

    fn busy(&self) -> bool {
        self.io.as_ref().is_some_and(IoService::busy)
    }

    fn update_title(&mut self, doc: &Document) {
        let key = (self.is_dirty(doc), self.name_gen);
        if self.title_key != Some(key) {
            self.title_key = Some(key);
            self.title = format!("{}{} — ARTY", self.name, if key.0 { "*" } else { "" });
            self.title_changed = true;
        }
    }

    fn set_name(&mut self, name: String) {
        self.name = name;
        self.name_gen += 1;
    }

    /// The document in `studio` is new and untitled.
    fn start_untitled(&mut self, studio: &Studio) {
        self.epoch = studio.doc_epoch;
        self.path = None;
        self.set_name("Untitled".to_owned());
        self.saved_rev = Some(studio.doc.revision());
        self.read_only_reason = None;
        self.extra_sections.clear();
        self.layer_ext.clear();
        self.captured = Some(change_key(&studio.doc));
        self.captured_at = self.now;
        if let Some(io) = &self.io {
            io.send(Request::NewSession);
        }
    }

    fn extras(&self, view: &View) -> SaveExtras {
        SaveExtras {
            view: Some(encode_view(view)),
            sections: self.extra_sections.clone(),
            layer_ext: self.layer_ext.clone(),
            title: self.name.clone(),
        }
    }

    fn io_or_report(&mut self) -> Option<&IoService> {
        if self.io.is_none() {
            let why = self.io_error.clone().unwrap_or_else(|| "the file thread has stopped".to_owned());
            self.modal = Some(Modal::Message { title: "Files are unavailable", lines: vec![why] });
        }
        self.io.as_ref()
    }

    fn on_request(&mut self, req: FileRequest, studio: &mut Studio, shell: &mut Shell) {
        match req {
            FileRequest::New => self.guard_unsaved(Then::New, studio, shell),
            FileRequest::Open => self.guard_unsaved(Then::Open, studio, shell),
            FileRequest::Save => self.save(None, studio),
            FileRequest::SaveAs => self.save_as(None, studio),
        }
    }

    /// Run `then` now, or after asking to save unsaved changes.
    fn guard_unsaved(&mut self, then: Then, studio: &mut Studio, shell: &mut Shell) {
        if self.is_dirty(&studio.doc) {
            self.modal = Some(Modal::Unsaved(then));
        } else {
            self.proceed(then, shell);
        }
    }

    fn proceed(&mut self, then: Then, shell: &mut Shell) {
        match then {
            Then::New => shell.new_doc_open = true,
            Then::Open => self.open(),
            Then::Quit => {
                self.allow_close = true;
                shell.quit_requested = true;
            }
            Then::Restore => self.start_restore(),
        }
    }

    /// Restore `entry` (Restore in the recovery prompt), after asking about
    /// unsaved changes. False, with nothing done, while a load or save runs.
    fn choose_restore(&mut self, entry: RecoveryEntry, dirty: bool) -> bool {
        if self.job.is_some() {
            return false;
        }
        self.restore = Some(entry);
        if dirty {
            self.modal = Some(Modal::Unsaved(Then::Restore));
        } else {
            self.start_restore();
        }
        true
    }

    fn start_restore(&mut self) {
        let Some(entry) = self.restore.take() else { return };
        let Some(io) = self.io_or_report() else { return };
        let ticket = io.send(Request::Restore { entry: entry.clone() });
        self.job = Some(Job::Load { ticket, path: entry.src, restore: true });
        // The other files are offered again next time.
        self.modal = Some(Modal::Loading(ticket));
    }

    fn open(&mut self) {
        if self.io_or_report().is_none() {
            return;
        }
        let Some(path) = self.dialogs.open_path() else { return };
        let Some(io) = &self.io else { return };
        let ticket = io.send(Request::Open { path: path.clone() });
        self.job = Some(Job::Load { ticket, path: Some(path), restore: false });
        self.modal = Some(Modal::Loading(ticket));
    }

    /// Save to the current file; without one, or when saving over it would
    /// lose data, this becomes Save As.
    fn save(&mut self, then: Option<Then>, studio: &Studio) {
        match &self.path {
            _ if self.read_only_reason.is_some() => self.modal = Some(Modal::Lossy(then)),
            Some(path) => {
                let path = path.clone();
                self.send_save(path, false, then, studio);
            }
            None => self.save_as(then, studio),
        }
    }

    fn save_as(&mut self, then: Option<Then>, studio: &Studio) {
        if self.io_or_report().is_none() {
            return;
        }
        let stem = self.name.strip_suffix(".arty").unwrap_or(&self.name);
        let name = format!("{}.arty", stem.strip_suffix(" (imported v1)").unwrap_or(stem));
        let dir = self.path.as_deref().and_then(Path::parent);
        if let Some(path) = self.dialogs.save_path(&name, dir) {
            self.send_save(path, false, then, studio);
        }
    }

    fn send_save(&mut self, path: PathBuf, overwrite_external: bool, then: Option<Then>, studio: &Studio) {
        let ex = self.extras(&studio.view);
        let Some(io) = self.io_or_report() else { return };
        let doc = &studio.doc;
        let (rev, key) = (doc.revision(), change_key(doc));
        let ticket = io.send(Request::Save { doc: Box::new(doc.snapshot()), ex, path, rev, overwrite_external });
        self.job = Some(Job::Save { ticket, key, then });
    }

    fn send_autosave(&mut self, studio: &Studio) {
        let ex = self.extras(&studio.view);
        let Some(io) = &self.io else { return };
        let doc = &studio.doc;
        let rev = doc.revision();
        self.autosave = Some(io.send(Request::Autosave { doc: Box::new(doc.snapshot()), ex, rev }));
        self.captured = Some(change_key(doc));
        self.captured_at = self.now;
    }

    fn on_close_requested(&mut self, studio: &Studio, out: &mut FrameOutput) {
        if !self.allow_close && self.is_dirty(&studio.doc) {
            out.cancel_close = true;
            // A load keeps its dialog (with Cancel): saving now would take
            // the place of its job. Closing again afterwards asks.
            if !matches!(self.modal, Some(Modal::Unsaved(_) | Modal::Loading(_))) {
                self.modal = Some(Modal::Unsaved(Then::Quit));
            }
        } else if self.busy() {
            out.cancel_close = true;
            self.closing = true;
            self.modal = Some(Modal::Closing);
        } else if let Some(io) = self.io.take() {
            // Closing for good: the work is saved or was dropped on purpose.
            io.send(Request::CloseSession { discard_recovery: true });
            io.shutdown();
        }
    }

    fn on_event(&mut self, e: IoEvent, studio: &mut Studio, shell: &mut Shell) {
        match e {
            IoEvent::Saved { ticket, path, rev, stats } => {
                let Some(Job::Save { ticket: t, key, then, .. }) = &self.job else { return };
                if *t != ticket {
                    return;
                }
                let (key, then) = (*key, *then);
                self.job = None;
                self.saved_rev = Some(rev);
                self.read_only_reason = None;
                // The recovery file got a clean commit of this state.
                self.captured = Some(key);
                self.captured_at = self.now;
                self.set_name(file_name(&path));
                studio.notice = Some(match stats.selection_saved {
                    SelectionSave::Binarized => "Saved; the selection was too detailed and its soft edges were made hard".into(),
                    SelectionSave::Dropped => "Saved without the selection, which was too large to store".into(),
                    SelectionSave::None | SelectionSave::Exact => format!("Saved {}", path.display()),
                });
                self.path = Some(path);
                if let Some(then) = then {
                    self.proceed(then, shell);
                }
            }
            IoEvent::Autosaved { ticket, .. } => {
                if self.autosave == Some(ticket) {
                    self.autosave = None;
                }
            }
            IoEvent::Loaded { ticket, path, loaded } => {
                // Any other job in flight is left alone.
                if !matches!(&self.job, Some(Job::Load { ticket: t, .. }) if *t == ticket) {
                    return;
                }
                let Some(Job::Load { path: asked, restore, .. }) = self.job.take() else { return };
                self.apply_loaded(*loaded, path, asked, restore, studio, shell);
            }
            IoEvent::RecoveryFound(entries) => {
                if entries.is_empty() {
                    return;
                }
                if self.modal.is_none() {
                    self.modal = Some(Modal::Recovery(entries));
                } else {
                    self.found = entries;
                }
            }
            IoEvent::Failed { ticket, op, error } => self.on_failed(ticket, op, error),
        }
    }

    fn on_failed(&mut self, ticket: Ticket, op: &'static str, error: IoError) {
        if self.autosave == Some(ticket) {
            self.autosave = None;
            // Not captured: retried after the next interval, even without
            // further edits. Cancelled: a queued save took its place.
            self.captured = None;
            self.captured_at = self.now;
            if !matches!(error, IoError::Cancelled) {
                log::warn!("autosave: {error}");
            }
            return;
        }
        let then = match self.job.take() {
            Some(job) if job.ticket() == ticket => match job {
                Job::Save { then, .. } => then,
                Job::Load { .. } => None,
            },
            other => {
                self.job = other;
                None
            }
        };
        if matches!(self.modal, Some(Modal::Loading(t)) if t == ticket) {
            self.modal = None;
        }
        match error {
            IoError::Cancelled => {}
            IoError::ExternallyModified => {
                if let Some(path) = self.path.clone() {
                    self.modal = Some(Modal::External { path, then });
                }
            }
            e => {
                let title = match op {
                    "save" => "Could not save",
                    "open" => "Could not open the file",
                    "restore" => "Could not restore the document",
                    _ => "File error",
                };
                self.modal = Some(Modal::Message { title, lines: vec![e.to_string()] });
            }
        }
    }

    fn apply_loaded(
        &mut self,
        loaded: Loaded,
        path: Option<PathBuf>,
        asked: Option<PathBuf>,
        restore: bool,
        studio: &mut Studio,
        shell: &mut Shell,
    ) {
        let Loaded { doc, extra_sections, layer_ext, view, warnings, read_only_reason, info, .. } = loaded;
        let legacy = info.kind == FileKind::LegacyV1;
        let title = info.meta.iter().find(|(k, _)| k == "title").map(|(_, v)| v.clone()).filter(|t| !t.is_empty());
        studio.replace_document(doc);
        if let Some(v) = view.as_deref().and_then(decode_view) {
            studio.view = v;
            studio.fit_pending = false;
        }
        shell.renaming = None;
        self.epoch = studio.doc_epoch;
        let name = match (&path, &asked) {
            (Some(p), _) => file_name(p),
            (None, Some(p)) if legacy => format!("{} (imported v1)", p.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned())),
            _ => title.unwrap_or_else(|| "Untitled".to_owned()),
        };
        self.set_name(name);
        self.path = path;
        self.read_only_reason = read_only_reason;
        self.extra_sections = extra_sections;
        self.layer_ext = layer_ext;
        let rev = studio.doc.revision();
        // A restored document is unsaved; autosave it to this session soon.
        self.saved_rev = (!restore).then_some(rev);
        self.captured = (!restore).then(|| change_key(&studio.doc));
        self.captured_at = self.now;
        self.modal = (!warnings.is_empty()).then(|| Modal::Message {
            title: "Opened with warnings",
            lines: warnings.iter().map(ToString::to_string).collect(),
        });
    }

    // ----- dialogs -----------------------------------------------------------

    /// Status bar text while a save or load runs.
    pub fn status(&self) -> Option<String> {
        let io = self.io.as_ref()?;
        let p = io.progress();
        let pct = || {
            let (done, total) = (p.done.load(Ordering::Relaxed), p.total.load(Ordering::Relaxed));
            (done * 100).checked_div(total).map_or_else(String::new, |p| format!(" {p}%"))
        };
        match (&self.job, self.autosave) {
            (Some(Job::Save { .. }), _) => Some(format!("Saving…{}", pct())),
            (Some(Job::Load { .. }), _) => Some(format!("Opening…{}", pct())),
            (None, Some(_)) => Some("Autosaving…".to_owned()),
            (None, None) => None,
        }
    }

    /// Draw the open dialog, if any, and act on its buttons.
    pub fn ui(&mut self, ctx: &egui::Context, studio: &mut Studio, shell: &mut Shell) {
        let Some(modal) = self.modal.take() else { return };
        let dirty = self.is_dirty(&studio.doc);
        let mut keep = true;
        let response = egui::Modal::new(egui::Id::new("file-dialog")).show(ctx, |ui| {
            ui.set_max_width(460.0);
            match &modal {
                Modal::Unsaved(then) => {
                    ui.heading("Save changes?");
                    ui.label(format!("{} has unsaved changes.", self.name));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new("Save").strong()).clicked() {
                            keep = false;
                            self.save(Some(*then), studio);
                        }
                        if ui.button("Don't Save").clicked() {
                            keep = false;
                            self.proceed(*then, shell);
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                            if *then == Then::Restore {
                                self.restore = None;
                            }
                        }
                    });
                }
                Modal::Lossy(then) => {
                    ui.heading("Save as a new file?");
                    ui.label(self.read_only_reason.as_deref().unwrap_or_default());
                    ui.label("Saving over the original would lose that data. Save a copy instead.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new("Save As…").strong()).clicked() {
                            keep = false;
                            self.save_as(*then, studio);
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
                Modal::External { path, then } => {
                    ui.heading("File changed on disk");
                    ui.label(format!("{} was changed by another program since it was opened or saved.", path.display()));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Overwrite").clicked() {
                            keep = false;
                            self.send_save(path.clone(), true, *then, studio);
                        }
                        if ui.button(RichText::new("Save As…").strong()).clicked() {
                            keep = false;
                            self.save_as(*then, studio);
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
                Modal::Loading(ticket) => {
                    ui.heading("Opening…");
                    if let Some(io) = &self.io {
                        let p = io.progress();
                        let (done, total) = (p.done.load(Ordering::Relaxed), p.total.load(Ordering::Relaxed));
                        let frac = if total > 0 { done as f32 / total as f32 } else { 0.0 };
                        ui.add(egui::ProgressBar::new(frac).show_percentage());
                        if ui.button("Cancel").clicked() {
                            io.cancel(*ticket);
                        }
                    }
                }
                Modal::Message { title, lines } => {
                    ui.heading(*title);
                    for l in lines {
                        ui.label(l.as_str());
                    }
                    ui.add_space(8.0);
                    if ui.button("OK").clicked() {
                        keep = false;
                    }
                }
                Modal::Recovery(entries) => {
                    keep = self.recovery_ui(ui, entries, dirty);
                }
                Modal::Closing => {
                    ui.heading("Finishing save…");
                    ui.spinner();
                }
            }
        });
        let dismissable = matches!(modal, Modal::Unsaved(_) | Modal::Lossy(_) | Modal::External { .. } | Modal::Message { .. });
        if dismissable && response.should_close() {
            keep = false;
            if matches!(modal, Modal::Unsaved(Then::Restore)) {
                self.restore = None;
            }
        }
        // A button may have opened the next dialog.
        if keep && self.modal.is_none() {
            self.modal = Some(modal);
        }
    }

    /// The recovery prompt; false once it should close. `dirty`: the open
    /// document has unsaved changes (Restore asks about them first).
    fn recovery_ui(&mut self, ui: &mut egui::Ui, entries: &[RecoveryEntry], dirty: bool) -> bool {
        ui.heading("Recover unsaved work?");
        ui.label("ARTY closed without saving these documents.");
        ui.add_space(6.0);
        let mut restore = None;
        let mut discard = None;
        for (i, e) in entries.iter().enumerate() {
            ui.separator();
            ui.label(RichText::new(e.display_name()).strong());
            if let Some(src) = &e.src {
                ui.weak(src.display().to_string());
            }
            ui.weak(format!("{} · {:.1} MB", ago(e.saved_ms), e.size as f64 / (1024.0 * 1024.0)));
            ui.horizontal(|ui| {
                // Not while a load or save runs: it would replace its job.
                let idle = self.job.is_none();
                if ui.add_enabled(idle, egui::Button::new(RichText::new("Restore").strong())).clicked() {
                    restore = Some(i);
                }
                if ui.button("Discard").clicked() {
                    discard = Some(i);
                }
            });
        }
        ui.separator();
        let later = ui.button("Later").on_hover_text("Keep these files and ask again next time").clicked();
        if let Some(i) = restore
            && self.choose_restore(entries[i].clone(), dirty)
        {
            return false;
        }
        if let (Some(i), Some(io)) = (discard, &self.io) {
            io.send(Request::Discard { entry: entries[i].clone() });
            let rest: Vec<RecoveryEntry> = entries.iter().enumerate().filter(|(k, _)| *k != i).map(|(_, e)| e.clone()).collect();
            if !rest.is_empty() {
                self.modal = Some(Modal::Recovery(rest));
            }
            return false;
        }
        !later
    }
}

/// Changes whenever the content or the view state (active layer, folder
/// expansion) changes; both revisions only grow.
fn change_key(doc: &Document) -> u64 {
    doc.revision().wrapping_add(doc.view_revision())
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// "5 min ago" for a Unix ms time.
fn ago(ms: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    let mins = now.saturating_sub(ms) / 60_000;
    match mins {
        0 => "just now".to_owned(),
        1..60 => format!("{mins} min ago"),
        60..2880 => format!("{} h ago", mins / 60),
        _ => format!("{} days ago", mins / 1440),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::mpsc::channel;
    use std::time::Instant;

    use arty_brush::InputSample;
    use arty_core::TileCoord;
    use arty_io::{IoConfig, LoadOptions};

    use super::*;
    use crate::theme::ThemeKind;

    /// Answers for the file pickers, and how often each was shown.
    #[derive(Default)]
    struct Answers {
        open: Option<PathBuf>,
        save: Vec<PathBuf>,
        asked_open: u32,
        asked_save: u32,
    }

    struct Fake(Rc<RefCell<Answers>>);

    impl FileDialogs for Fake {
        fn open_path(&mut self) -> Option<PathBuf> {
            let mut a = self.0.borrow_mut();
            a.asked_open += 1;
            a.open.clone()
        }

        fn save_path(&mut self, _: &str, _: Option<&Path>) -> Option<PathBuf> {
            let mut a = self.0.borrow_mut();
            a.asked_save += 1;
            (!a.save.is_empty()).then(|| a.save.remove(0))
        }
    }

    struct Rig {
        fc: FileController,
        studio: Studio,
        shell: Shell,
        answers: Rc<RefCell<Answers>>,
        dir: PathBuf,
    }

    impl Rig {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("arty-app-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let cfg = IoConfig { threads: 2, recovery_dir: dir.join("recovery"), load: LoadOptions::default() };
            let io = IoService::spawn(cfg, || {});
            let studio = Studio::new(Document::new(256, 256, 72));
            let answers = Rc::new(RefCell::new(Answers::default()));
            let fc = FileController::new(io, Box::new(Fake(answers.clone())), &studio);
            let mut rig = Self { fc, studio, shell: Shell::new(ThemeKind::Dark), answers, dir };
            rig.settle(0.0);
            rig
        }

        fn frame(&mut self, f: FrameInput) -> FrameOutput {
            self.fc.update(&f, &mut self.studio, &mut self.shell)
        }

        /// An idle, focused frame at `now`.
        fn at(&mut self, now: f64) -> FrameOutput {
            self.frame(FrameInput { now, focused: true, ..Default::default() })
        }

        /// Frames at `now` until the IO thread is idle and its events are
        /// handled.
        fn settle(&mut self, now: f64) -> FrameOutput {
            let deadline = Instant::now() + Duration::from_secs(60);
            loop {
                let out = self.at(now);
                if !self.fc.busy() && self.fc.job.is_none() && self.fc.autosave.is_none() {
                    return out;
                }
                assert!(Instant::now() < deadline, "the io thread did not finish");
                std::thread::sleep(Duration::from_millis(2));
            }
        }

        fn paint(&mut self) {
            let id = self.studio.doc.active();
            let (g, _) = self.studio.doc.paint_target(id).unwrap();
            let t = g.get_mut_or_create(TileCoord::new(0, 0));
            t[0][0][3] = (t[0][0][3] + 1) & 0x7fff;
        }

        fn recovery_files(&self) -> usize {
            std::fs::read_dir(self.dir.join("recovery"))
                .map_or(0, |d| d.filter(|e| e.as_ref().is_ok_and(|e| e.path().extension().is_some_and(|x| x == "arty"))).count())
        }
    }

    fn state(now: f64, last_input: f64) -> AutosaveState {
        AutosaveState { now, key: 1, captured: Some(0), since: 0.0, last_input, stroking: false, in_flight: false, focus_lost: false }
    }

    #[test]
    fn autosave_policy_follows_the_clock() {
        let on = AutosaveSettings::default();
        assert_eq!(autosave_due(&on, &state(30.0, 30.0)), Due::After(30.0), "before the interval");
        assert_eq!(autosave_due(&on, &state(61.0, 59.0)), Due::Now, "idle for 2 s");
        assert_eq!(autosave_due(&on, &state(61.0, 60.5)), Due::After(1.5), "waits for idle");
        assert_eq!(autosave_due(&on, &AutosaveState { focus_lost: true, ..state(61.0, 61.0) }), Due::Now, "focus loss");
        assert_eq!(autosave_due(&on, &state(180.0, 180.0)), Due::Now, "3 intervals without a pause");
        assert_eq!(autosave_due(&on, &state(179.0, 179.0)), Due::After(1.0));
        assert_eq!(autosave_due(&on, &AutosaveState { stroking: true, ..state(200.0, 0.0) }), Due::No, "mid-stroke");
        assert_eq!(autosave_due(&on, &AutosaveState { in_flight: true, ..state(200.0, 0.0) }), Due::No, "one at a time");
        assert_eq!(autosave_due(&on, &AutosaveState { captured: Some(1), ..state(200.0, 0.0) }), Due::No, "unchanged");
        assert_eq!(autosave_due(&on, &AutosaveState { captured: None, ..state(200.0, 0.0) }), Due::Now, "never captured");
        let off = AutosaveSettings { enabled: false, ..on };
        assert_eq!(autosave_due(&off, &state(200.0, 0.0)), Due::No, "disabled");
        let short = AutosaveSettings { interval_secs: 1, ..on };
        assert_eq!(autosave_due(&short, &state(5.0, 0.0)), Due::After(5.0), "the interval has a floor");
    }

    #[test]
    fn autosave_runs_on_idle_and_on_focus_loss() {
        let mut r = Rig::new("autosave");
        r.paint();
        let input = |now| FrameInput { now, focused: true, input: true, close_requested: false };
        let out = r.frame(input(1.0));
        assert!(r.fc.autosave.is_none());
        assert_eq!(out.repaint_after, Some(59.0));
        // The interval passed, but the user is still drawing.
        let out = r.frame(input(61.0));
        assert!(r.fc.autosave.is_none());
        assert_eq!(out.repaint_after, Some(IDLE_SECS));
        r.at(63.0);
        assert!(r.fc.autosave.is_some(), "idle for 2 s");
        r.settle(63.0);
        assert_eq!(r.recovery_files(), 1);
        // Nothing changed since: nothing is due.
        assert_eq!(r.at(500.0).repaint_after, None);

        r.paint();
        r.frame(input(523.5));
        r.frame(FrameInput { now: 523.6, focused: false, ..Default::default() });
        assert!(r.fc.autosave.is_some(), "the window lost focus");
        r.settle(524.0);

        // A stroke in progress holds it back.
        r.paint();
        let s = InputSample { x: 10.0, y: 10.0, pressure: 1.0, time: 0.0, ..Default::default() };
        assert!(r.studio.begin_stroke(s));
        r.at(800.0);
        assert!(r.fc.autosave.is_none());
        r.studio.end_stroke();
        r.at(800.0);
        assert!(r.fc.autosave.is_some());
        r.settle(800.0);
    }

    /// M3 §10.2 test 6: a transform session's preview is never autosaved.
    #[test]
    fn m3_autosave_waits_for_a_transform_session() {
        let mut r = Rig::new("autosave-transform");
        r.paint();
        assert!(r.studio.begin_transform(false));
        let p = r.studio.transform.as_ref().unwrap().session.params();
        r.studio.transform.as_mut().unwrap().request(arty_core::transform::XfParams { t: [3.0, 0.0], ..p });
        for now in [100.0, 400.0, 1000.0] {
            r.frame(FrameInput { now, focused: false, ..Default::default() });
            assert!(r.fc.autosave.is_none(), "at {now} s, focus lost");
        }
        r.studio.commit_transform();
        r.at(1000.0);
        assert!(r.fc.autosave.is_some(), "due once the session ends");
        r.settle(1000.0);
    }

    #[test]
    fn save_without_a_path_or_of_a_lossy_document_is_save_as() {
        let mut r = Rig::new("save-as");
        let (a, b) = (r.dir.join("a.arty"), r.dir.join("b.arty"));
        r.answers.borrow_mut().save = vec![a.clone(), b.clone()];
        r.paint();

        // Deferred while a stroke is in progress.
        let s = InputSample { x: 10.0, y: 10.0, pressure: 1.0, time: 0.0, ..Default::default() };
        assert!(r.studio.begin_stroke(s));
        r.shell.file_request = Some(FileRequest::Save);
        r.at(1.0);
        assert_eq!(r.shell.file_request, Some(FileRequest::Save));
        r.studio.end_stroke();

        // No path yet: Save asks where.
        r.at(1.0);
        assert_eq!(r.shell.file_request, None);
        assert_eq!(r.answers.borrow().asked_save, 1);
        r.settle(1.0);
        assert_eq!(r.fc.path.as_deref(), Some(a.as_path()));
        assert!(!r.fc.is_dirty(&r.studio.doc));
        assert!(a.exists());

        // With a path: saved in place, no dialog.
        r.paint();
        r.shell.file_request = Some(FileRequest::Save);
        r.settle(2.0);
        assert_eq!(r.answers.borrow().asked_save, 1);
        assert!(!r.fc.is_dirty(&r.studio.doc));

        // A lossy document warns first, then saves as a new file.
        r.fc.read_only_reason = Some("Saving would lose data: test.".into());
        r.paint();
        r.shell.file_request = Some(FileRequest::Save);
        r.at(3.0);
        assert!(matches!(r.fc.modal, Some(Modal::Lossy(None))));
        assert_eq!(r.answers.borrow().asked_save, 1);
        r.fc.modal = None;
        r.fc.save_as(None, &r.studio); // the dialog's Save As… button
        assert_eq!(r.answers.borrow().asked_save, 2);
        r.settle(3.0);
        assert_eq!(r.fc.path.as_deref(), Some(b.as_path()));
        assert_eq!(r.fc.read_only_reason, None);
        assert!(b.exists());
    }

    #[test]
    fn open_asks_about_changes_and_restores_the_saved_view() {
        let mut r = Rig::new("open");
        let a = r.dir.join("a.arty");
        r.answers.borrow_mut().save = vec![a.clone()];
        r.paint();
        let view = View { center: [12.5, -3.0], zoom: 2.5, rotation: 0.25, flip_x: true };
        r.studio.view = view;
        r.shell.file_request = Some(FileRequest::SaveAs);
        r.settle(1.0);
        let saved = r.studio.doc.snapshot();

        r.paint();
        r.studio.view = View::default();
        r.answers.borrow_mut().open = Some(a.clone());
        r.shell.file_request = Some(FileRequest::Open);
        r.at(2.0);
        assert!(matches!(r.fc.modal, Some(Modal::Unsaved(Then::Open))), "unsaved changes are not dropped silently");
        assert_eq!(r.answers.borrow().asked_open, 0);
        r.fc.modal = None;
        r.fc.proceed(Then::Open, &mut r.shell); // Don't Save
        assert_eq!(r.answers.borrow().asked_open, 1);
        assert!(matches!(r.fc.modal, Some(Modal::Loading(_))));
        r.settle(2.0);

        assert!(r.fc.modal.is_none());
        let id = saved.active();
        let tile = |d: &Document| d.layer(id).unwrap().raster().unwrap().get_ref(TileCoord::new(0, 0)).cloned().unwrap();
        assert!(*tile(&saved) == *tile(&r.studio.doc));
        assert_eq!(r.studio.view, view);
        assert!(!r.studio.fit_pending);
        assert!(!r.studio.history.can_undo());
        assert!(!r.fc.is_dirty(&r.studio.doc));
        assert_eq!(r.fc.path.as_deref(), Some(a.as_path()));
        assert_eq!(r.fc.title(), "a.arty — ARTY");
    }

    #[test]
    fn close_asks_about_changes_and_waits_for_saves() {
        let close = FrameInput { now: 1.0, focused: true, close_requested: true, ..Default::default() };
        // Clean and idle: closes right away.
        let mut r = Rig::new("close-clean");
        assert!(!r.frame(close).cancel_close);
        assert!(r.fc.io.is_none(), "the io thread was shut down");

        // Dirty: the close is cancelled and the user asked.
        let mut r = Rig::new("close-dirty");
        r.paint();
        assert!(r.frame(close).cancel_close);
        assert!(matches!(r.fc.modal, Some(Modal::Unsaved(Then::Quit))));
        r.fc.modal = None;
        r.fc.proceed(Then::Quit, &mut r.shell); // Don't Save
        assert!(r.shell.quit_requested);
        assert!(!r.frame(close).cancel_close);

        // Busy: waits for the IO thread, then closes.
        let mut r = Rig::new("close-busy");
        let (go, rx) = channel();
        r.fc.io.as_ref().unwrap().send(Request::Pause(rx));
        assert!(r.frame(close).cancel_close);
        assert!(matches!(r.fc.modal, Some(Modal::Closing)));
        assert!(!r.at(1.5).close);
        drop(go);
        let deadline = Instant::now() + Duration::from_secs(60);
        while r.fc.busy() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(r.at(2.0).close);
        assert!(r.fc.modal.is_none());
        assert!(!r.frame(close).cancel_close);
    }

    /// A recovery file left by an earlier session, as the scan offers it.
    fn crashed_session(r: &Rig) -> RecoveryEntry {
        let dir = r.dir.join("recovery");
        let mut old = arty_io::Session::new(arty_io::SessionId([0x42; 16]), Some(&dir));
        let mut doc = Document::new(64, 64, 72);
        let id = doc.active();
        doc.paint_target(id).unwrap().0.get_mut_or_create(TileCoord::new(0, 0))[0][0] = [1, 2, 3, 4];
        let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
        old.autosave(&doc, &SaveExtras::default(), 7, &pool, &arty_io::Progress::default()).unwrap();
        old.close(false).unwrap();
        arty_io::RecoveryDir::new(&dir).scan().pop().unwrap()
    }

    /// Wait for the IO thread without handling its events.
    fn wait_idle(r: &Rig) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while r.fc.busy() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn closing_during_a_restore_asks_and_keeps_the_recovery_file() {
        let close = FrameInput { now: 1.0, focused: true, close_requested: true, ..Default::default() };
        let mut r = Rig::new("close-restore");
        let entry = crashed_session(&r);
        let (go, rx) = channel();
        r.fc.io.as_ref().unwrap().send(Request::Pause(rx));
        assert!(r.fc.choose_restore(entry.clone(), false), "clean: no question");
        // Clean and busy: the close waits for the IO thread.
        assert!(r.frame(close).cancel_close);
        assert!(matches!(r.fc.modal, Some(Modal::Closing)));
        drop(go);
        wait_idle(&r);
        let out = r.at(2.0);
        assert!(!out.close, "the restored document is unsaved");
        assert!(r.fc.is_dirty(&r.studio.doc));
        assert!(matches!(r.fc.modal, Some(Modal::Unsaved(Then::Quit))));
        assert!(entry.path.exists(), "the restored work is still on disk");
    }

    #[test]
    fn restore_asks_about_changes_and_waits_for_jobs() {
        let mut r = Rig::new("restore-guard");
        let entry = crashed_session(&r);
        r.paint();
        r.at(1.0);
        let dirty = r.fc.is_dirty(&r.studio.doc);
        assert!(r.fc.choose_restore(entry.clone(), dirty));
        assert!(matches!(r.fc.modal, Some(Modal::Unsaved(Then::Restore))), "unsaved changes are not dropped silently");
        assert!(r.fc.job.is_none(), "nothing sent yet");

        // Save first: the restore waits for the save, then runs.
        let page = r.dir.join("page.arty");
        r.answers.borrow_mut().save = vec![page.clone()];
        r.fc.modal = None;
        r.fc.save(Some(Then::Restore), &r.studio); // the dialog's Save
        assert!(!r.fc.choose_restore(entry.clone(), true), "refused while the save runs");
        r.settle(2.0);
        assert!(page.exists());
        let id = r.studio.doc.active();
        let px = r.studio.doc.layer(id).unwrap().raster().unwrap().get(TileCoord::new(0, 0)).unwrap()[0][0];
        assert_eq!(px, [1, 2, 3, 4], "restored");
        assert!(r.fc.is_dirty(&r.studio.doc));
    }

    #[test]
    fn closing_during_an_open_keeps_the_load() {
        let mut r = Rig::new("close-open");
        let a = r.dir.join("a.arty");
        r.answers.borrow_mut().save = vec![a.clone()];
        r.shell.file_request = Some(FileRequest::SaveAs);
        r.settle(1.0);
        r.paint();
        r.answers.borrow_mut().open = Some(a.clone());
        let (go, rx) = channel();
        r.fc.io.as_ref().unwrap().send(Request::Pause(rx));
        r.fc.proceed(Then::Open, &mut r.shell); // Don't Save
        assert!(matches!(r.fc.modal, Some(Modal::Loading(_))));
        let close = FrameInput { now: 2.0, focused: true, close_requested: true, ..Default::default() };
        assert!(r.frame(close).cancel_close);
        assert!(matches!(r.fc.modal, Some(Modal::Loading(_))), "no Save that would replace the load");
        assert!(matches!(r.fc.job, Some(Job::Load { .. })));
        drop(go);
        r.settle(2.0);
        assert_eq!(r.fc.path.as_deref(), Some(a.as_path()));
        assert!(!r.fc.is_dirty(&r.studio.doc), "the opened file replaced the edits");
    }

    #[test]
    fn a_dropped_autosave_does_not_stop_autosaving() {
        let mut r = Rig::new("autosave-dropped");
        r.answers.borrow_mut().save = vec![r.dir.join("s.arty")];
        r.paint();
        let (go, rx) = channel();
        r.fc.io.as_ref().unwrap().send(Request::Pause(rx));
        r.at(63.0);
        assert!(r.fc.autosave.is_some());
        // A save queued behind it: the IO thread drops the autosave.
        r.shell.file_request = Some(FileRequest::Save);
        r.at(63.1);
        assert!(r.fc.job.is_some());
        drop(go);
        r.settle(63.2);
        assert_eq!(r.recovery_files(), 0, "dropped");
        r.paint();
        r.at(130.0);
        assert!(r.fc.autosave.is_some(), "autosave runs again");
        r.settle(130.0);
        assert_eq!(r.recovery_files(), 1);
    }

    #[test]
    fn a_failed_autosave_is_retried_after_the_interval() {
        let mut r = Rig::new("autosave-retry");
        // A file where the recovery folder should be: autosaves fail.
        let rec = r.dir.join("recovery");
        let _ = std::fs::remove_dir_all(&rec);
        std::fs::write(&rec, b"").unwrap();
        r.paint();
        r.at(63.0);
        assert!(r.fc.autosave.is_some());
        r.settle(63.0);
        assert_eq!(r.fc.captured, None, "nothing was captured");
        assert_eq!(r.at(100.0).repaint_after, Some(23.0), "retried one interval later, without edits");

        std::fs::remove_file(&rec).unwrap();
        r.at(123.0);
        assert!(r.fc.autosave.is_some());
        r.settle(123.0);
        assert_eq!(r.recovery_files(), 1);
        assert_eq!(r.at(500.0).repaint_after, None, "captured now");
    }

    #[test]
    fn title_asterisk_follows_the_revision() {
        let mut r = Rig::new("title");
        assert_eq!(r.fc.title(), "Untitled — ARTY");
        r.paint();
        r.at(0.2);
        assert_eq!(r.fc.title(), "Untitled* — ARTY");
        r.answers.borrow_mut().save = vec![r.dir.join("page.arty")];
        r.shell.file_request = Some(FileRequest::Save);
        r.settle(0.3);
        assert_eq!(r.fc.title(), "page.arty — ARTY");
        let id = r.studio.doc.active();
        let mut p = r.studio.doc.layer(id).unwrap().props.clone();
        p.opacity = 0.5;
        r.studio.set_layer_props(id, p, false);
        r.at(0.4);
        assert_eq!(r.fc.title(), "page.arty* — ARTY");
        // Dragging a layer in the panel is an edit too.
        r.studio.edit_structure(|d| {
            d.add_raster_layer().unwrap();
            true
        });
        r.shell.file_request = Some(FileRequest::Save);
        r.settle(0.42);
        assert_eq!(r.fc.title(), "page.arty — ARTY");
        let bottom = r.studio.doc.root()[0];
        crate::commands::execute(crate::commands::Command::MoveLayer { layer: bottom, parent: None, index: 2 }, &mut r.studio, &mut r.shell);
        assert_eq!(r.studio.doc.root()[1], bottom);
        r.at(0.44);
        assert_eq!(r.fc.title(), "page.arty* — ARTY");
        // File > New starts an untitled, clean document.
        r.studio.new_document(64, 64, 72);
        r.at(0.5);
        assert_eq!(r.fc.title(), "Untitled — ARTY");
        assert_eq!(r.fc.path, None);
    }

    #[test]
    fn idle_frames_do_not_allocate() {
        // The counting allocator is installed (main.rs).
        assert!(arty_testkit::count_allocs(|| drop(std::hint::black_box(vec![0u8; 8]))) > 0);
        let mut r = Rig::new("idle-alloc");
        r.paint();
        r.at(1.0);
        let n = arty_testkit::count_allocs(|| {
            for i in 0..100 {
                r.at(1.0 + f64::from(i) * 0.01);
            }
        });
        assert_eq!(n, 0, "FileController::update allocated {n} times");
    }

    #[test]
    fn view_bytes_round_trip() {
        let v = View { center: [1.5, -2.0], zoom: 0.125, rotation: -1.0, flip_x: true };
        let b = encode_view(&v);
        assert_eq!(b.len(), VIEW_LEN);
        assert_eq!(decode_view(&b), Some(v));
        let mut longer = b.clone();
        longer.extend_from_slice(&[9; 4]);
        assert_eq!(decode_view(&longer), Some(v), "trailing bytes are ignored");
        assert_eq!(decode_view(&b[..VIEW_LEN - 1]), None);
        let mut other = b.clone();
        other[0] = 2;
        assert_eq!(decode_view(&other), None);
        let mut nan = b;
        nan[1..5].copy_from_slice(&f32::NAN.to_le_bytes());
        assert_eq!(decode_view(&nan), None);
    }
}
