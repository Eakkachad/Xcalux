//! The recovery folder: each editing session autosaves to `{session}.arty`
//! and holds `{session}.lock` open (unshared on Windows) while it lives.
//!
//! At startup [`RecoveryDir::scan`] sorts what earlier sessions left:
//! - a locked session belongs to a running instance and is skipped;
//! - a file marked `clean` (its state was saved to the main file) is
//!   deleted;
//! - a file whose main file has the same `session` and a revision at least
//!   as new is obsolete (a crash between the save's rename and the clean
//!   commit) and is deleted;
//! - a file whose newest manifest is damaged is described by the commit
//!   Restore falls back to, and always offered;
//! - anything else is offered to the user;
//! - unlocked `.saving~` temp files and stale lock files are deleted.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use crate::error::IoError;
use crate::reader::read_info;

const DATA_EXT: &str = "arty";
const LOCK_EXT: &str = "lock";
const TEMP_SUFFIX: &str = ".saving~";

/// A recovery file left by a session that ended without saving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryEntry {
    /// The recovery file.
    pub path: PathBuf,
    /// Hex id of the session that wrote it.
    pub session: String,
    /// The main file the document came from or was last saved to.
    pub src: Option<PathBuf>,
    pub title: Option<String>,
    /// When the newest commit was written (Unix ms).
    pub saved_ms: u64,
    /// File size in bytes.
    pub size: u64,
    /// Document revision of the newest commit.
    pub rev: u64,
}

impl RecoveryEntry {
    /// A name for the prompt: the title, else the main file's name.
    pub fn display_name(&self) -> String {
        match (&self.title, &self.src) {
            (Some(t), _) if !t.is_empty() => t.clone(),
            (_, Some(src)) => src.file_name().map_or_else(|| src.display().to_string(), |n| n.to_string_lossy().into_owned()),
            _ => "Untitled".to_owned(),
        }
    }
}

/// `{session}.lock`, held open for as long as the session lives.
#[derive(Debug)]
pub(crate) struct SessionLock {
    file: Option<File>,
    path: PathBuf,
}

impl SessionLock {
    /// Close and delete the lock file.
    pub(crate) fn release(mut self) {
        self.file.take();
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(windows)]
pub(crate) fn is_sharing_violation(e: &io::Error) -> bool {
    // ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION
    matches!(e.raw_os_error(), Some(32 | 33))
}

#[cfg(not(windows))]
pub(crate) fn is_sharing_violation(_: &io::Error) -> bool {
    false
}

/// Take `{session}.lock` in `dir`. `Busy` when another live session holds
/// it.
pub(crate) fn acquire_lock(dir: &Path, session_hex: &str) -> Result<SessionLock, IoError> {
    fs::create_dir_all(dir).map_err(IoError::io("create the recovery folder"))?;
    let path = dir.join(format!("{session_hex}.{LOCK_EXT}"));
    #[cfg(not(windows))]
    if is_locked(&path) {
        return Err(IoError::Busy);
    }
    let mut o = OpenOptions::new();
    o.read(true).write(true).create(true).truncate(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        o.share_mode(0);
    }
    let file = o.open(&path).map_err(|e| {
        if is_sharing_violation(&e) { IoError::Busy } else { IoError::Io { op: "lock the session", source: e } }
    })?;
    #[cfg(not(windows))]
    {
        use std::io::Write;
        let _ = (&file).write_all(std::process::id().to_string().as_bytes());
    }
    Ok(SessionLock { file: Some(file), path })
}

/// True when a live session holds the lock file at `path`.
#[cfg(windows)]
fn is_locked(path: &Path) -> bool {
    // The holder opened it unshared, so any other open fails.
    OpenOptions::new().read(true).open(path).is_err_and(|e| is_sharing_violation(&e))
}

/// True when a live session holds the lock file at `path`. Best effort:
/// the file holds the owner's pid, checked through `/proc` where there is
/// one (elsewhere a lock file never counts as live).
#[cfg(not(windows))]
fn is_locked(path: &Path) -> bool {
    let Ok(pid) = fs::read_to_string(path) else { return false };
    let Ok(pid) = pid.trim().parse::<u32>() else { return false };
    let proc = Path::new("/proc");
    proc.is_dir() && proc.join(pid.to_string()).exists()
}

fn remove(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// `<path>.saving~`
fn temp_of(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(TEMP_SUFFIX);
    PathBuf::from(s)
}

/// What the scan decided for one recovery file.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    Live,
    Clean,
    Obsolete,
    Unreadable,
    Offer(RecoveryEntry),
}

/// The folder holding every session's recovery and lock files.
#[derive(Debug, Clone)]
pub struct RecoveryDir {
    path: PathBuf,
}

impl RecoveryDir {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// `%LOCALAPPDATA%\ARTY\recovery`, else `<temp>/ARTY/recovery`. Always
    /// a local folder: autosaves fsync often.
    pub fn default_path() -> PathBuf {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(std::env::temp_dir)
            .join("ARTY")
            .join("recovery")
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn lock_path(&self, session_hex: &str) -> PathBuf {
        self.path.join(format!("{session_hex}.{LOCK_EXT}"))
    }

    /// Sort the folder's files (see the module docs), deleting what is no
    /// longer needed, and return the files to offer, newest first. A
    /// missing folder has nothing to offer.
    pub fn scan(&self) -> Vec<RecoveryEntry> {
        let Ok(dir) = fs::read_dir(&self.path) else { return Vec::new() };
        let files: Vec<PathBuf> = dir.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_file()).collect();
        let mut offered = Vec::new();
        for path in files.iter().filter(|p| p.extension().is_some_and(|e| e == DATA_EXT)) {
            let Some(session) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else { continue };
            let verdict = self.judge(path, &session);
            log::debug!("recovery file {}: {verdict:?}", path.display());
            match verdict {
                Verdict::Clean | Verdict::Obsolete => {
                    if let Err(e) = remove(path) {
                        log::warn!("deleting {}: {e}", path.display());
                    }
                }
                Verdict::Offer(entry) => offered.push(entry),
                Verdict::Live | Verdict::Unreadable => {}
            }
        }
        // Temp files of sessions that are gone: a crash during a rewrite.
        for path in files.iter().filter(|p| p.to_string_lossy().ends_with(TEMP_SUFFIX)) {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let session = name.split('.').next().unwrap_or_default();
            if !is_locked(&self.lock_path(session)) {
                let _ = remove(path);
            }
        }
        // Lock files nobody holds whose recovery file is gone.
        for path in files.iter().filter(|p| p.extension().is_some_and(|e| e == LOCK_EXT)) {
            if !path.with_extension(DATA_EXT).exists() && !is_locked(path) {
                let _ = remove(path);
            }
        }
        offered.sort_by_key(|e| std::cmp::Reverse(e.saved_ms));
        offered
    }

    fn judge(&self, path: &Path, session: &str) -> Verdict {
        if is_locked(&self.lock_path(session)) {
            return Verdict::Live;
        }
        let info = match read_info(path) {
            Ok(info) => info,
            Err(e) => {
                // Left in place: nothing here can tell what it was.
                log::warn!("recovery file {} is unreadable: {e}", path.display());
                return Verdict::Unreadable;
            }
        };
        let meta = |key: &str| info.meta.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        // Described by an earlier commit than the damaged newest one, whose
        // META may say clean or be older than the main file while the
        // newest state was not: offered, never deleted.
        let fell_back = info.fell_back_from.is_some();
        if meta("clean") == Some("1") && !fell_back {
            return Verdict::Clean;
        }
        let rev = meta("rev").and_then(|r| r.parse::<u64>().ok()).unwrap_or(0);
        let src = meta("src").filter(|s| !s.is_empty()).map(PathBuf::from);
        // A crash after the save's rename and before the clean commit: the
        // main file holds this session's state or a newer one.
        if let Some(src) = &src
            && !fell_back
            && let Ok(main) = read_info(src)
        {
            let main_meta = |key: &str| main.meta.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
            let main_rev = main_meta("rev").and_then(|r| r.parse::<u64>().ok());
            if main_meta("session").is_some_and(|s| Some(s) == meta("session")) && main_rev.is_some_and(|r| r >= rev) {
                return Verdict::Obsolete;
            }
        }
        Verdict::Offer(RecoveryEntry {
            path: path.to_path_buf(),
            session: meta("session").unwrap_or(session).to_owned(),
            src,
            title: meta("title").map(str::to_owned),
            saved_ms: info.saved_ms,
            size: fs::metadata(path).map_or(0, |m| m.len()),
            rev,
        })
    }

    /// Take the lock of `entry`'s session, so no other instance offers or
    /// deletes it while this one restores it. `Busy` when a live session
    /// holds it.
    pub(crate) fn claim(&self, entry: &RecoveryEntry) -> Result<SessionLock, IoError> {
        let stem = entry.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        acquire_lock(&self.path, &stem)
    }

    /// Delete `entry`'s recovery file (and its temp and lock files).
    pub fn discard(&self, entry: &RecoveryEntry) -> Result<(), IoError> {
        let lock = self.claim(entry)?;
        let r = discard_files(&entry.path);
        lock.release();
        r
    }
}

/// Delete a recovery file and its temp file.
pub(crate) fn discard_files(path: &Path) -> Result<(), IoError> {
    let _ = remove(&temp_of(path));
    remove(path).map_err(IoError::io("delete the recovery file"))
}
