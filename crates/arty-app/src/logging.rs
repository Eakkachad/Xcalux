//! Log file and crash report (plans/bench/B028_preview_pack.md).
//!
//! Every start writes `arty-YYYYMMDD-HHMMSS.log` under `%LOCALAPPDATA%\ARTY\logs`
//! (the newest five are kept, 2 MiB at most each) next to stderr. Lines go through
//! a channel to a writer thread, so a slow disk never stalls the UI. A panic hook
//! writes `crash-YYYYMMDD-HHMMSS.txt` and lets the default behaviour go on; the next
//! start finds it and tells the user (about.rs).
//!
//! Bench runs write nothing unless `ARTY_LOG_DIR` is set: they must leave the
//! profile folders as they found them (plans/bench/B013_t1emu_lib.ps1).

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// Log and crash files kept (the newest).
pub const KEEP_FILES: usize = 5;
/// A log file stops taking lines at this size.
pub const MAX_LOG_BYTES: u64 = 2 << 20;
/// Log lines a crash report carries.
pub const REPORT_LOG_LINES: usize = 200;

/// Info for ARTY's own crates, warnings for the rest; `RUST_LOG` overrides.
const DEFAULT_FILTER: &str = "warn,arty=info,arty_core=info,arty_brush=info,arty_io=info,arty_pen=info,arty_render=info,arty_smart=info";
const LOG_PREFIX: &str = "arty-";
const LOG_EXT: &str = ".log";
const CRASH_PREFIX: &str = "crash-";
const CRASH_EXT: &str = ".txt";
const TRUNCATED: &str = "-- log truncated at 2 MiB; later lines are not saved --";
/// How long a flush waits for the writer thread.
const FLUSH_WAIT: Duration = Duration::from_millis(300);

/// Local date and time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub millis: u16,
}

impl Stamp {
    #[cfg(windows)]
    pub fn now() -> Self {
        use windows_sys::Win32::Foundation::SYSTEMTIME;
        use windows_sys::Win32::System::SystemInformation::GetLocalTime;
        let mut t = SYSTEMTIME { wYear: 0, wMonth: 0, wDayOfWeek: 0, wDay: 0, wHour: 0, wMinute: 0, wSecond: 0, wMilliseconds: 0 };
        // SAFETY: `t` is a valid SYSTEMTIME the call fills in.
        unsafe { GetLocalTime(&mut t) };
        Self { year: t.wYear, month: t.wMonth as u8, day: t.wDay as u8, hour: t.wHour as u8, minute: t.wMinute as u8, second: t.wSecond as u8, millis: t.wMilliseconds }
    }

    /// UTC where there is no local time call.
    #[cfg(not(windows))]
    pub fn now() -> Self {
        let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        Self::from_unix(d.as_secs(), d.subsec_millis() as u16)
    }

    /// Civil date from days since 1970-01-01 (Hinnant's algorithm).
    #[cfg(any(test, not(windows)))]
    fn from_unix(secs: u64, millis: u16) -> Self {
        let days = (secs / 86_400) as i64;
        let rem = secs % 86_400;
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
        let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u8;
        let year = (yoe + era * 400 + i64::from(month <= 2)) as u16;
        Self { year, month, day, hour: (rem / 3600) as u8, minute: (rem % 3600 / 60) as u8, second: (rem % 60) as u8, millis }
    }

    /// `20261006-143015`, the part of a file name that orders files by time.
    pub fn file_part(&self) -> String {
        format!("{:04}{:02}{:02}-{:02}{:02}{:02}", self.year, self.month, self.day, self.hour, self.minute, self.second)
    }

    /// `14:30:15.123`
    pub fn clock(&self) -> String {
        format!("{:02}:{:02}:{:02}.{:03}", self.hour, self.minute, self.second, self.millis)
    }

    /// `2026-10-06 14:30:15`
    pub fn date_time(&self) -> String {
        format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", self.year, self.month, self.day, self.hour, self.minute, self.second)
    }
}

/// The time in `prefix` + `20261006-143015` + `ext` as one number that sorts by time;
/// `None` for any other name.
fn stamp_of(name: &str, prefix: &str, ext: &str) -> Option<u64> {
    let part = name.strip_prefix(prefix)?.strip_suffix(ext)?;
    let (date, time) = part.split_once('-')?;
    let digits = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_digit());
    (digits(date, 8) && digits(time, 6)).then(|| format!("{date}{time}").parse().ok())?
}

/// The files to delete so that `keep` of the `prefix`/`ext` files remain (the
/// newest); names of another shape are never listed.
fn expired(names: &[String], prefix: &str, ext: &str, keep: usize) -> Vec<String> {
    let mut stamped: Vec<(u64, &String)> = names.iter().filter_map(|n| stamp_of(n, prefix, ext).map(|s| (s, n))).collect();
    stamped.sort();
    let old = stamped.len().saturating_sub(keep);
    stamped[..old].iter().map(|(_, n)| (*n).clone()).collect()
}

fn names_in(dir: &Path) -> Vec<String> {
    fs::read_dir(dir).map(|rd| rd.flatten().filter_map(|e| e.file_name().into_string().ok()).collect()).unwrap_or_default()
}

fn prune(dir: &Path, prefix: &str, ext: &str, keep: usize) {
    for name in expired(&names_in(dir), prefix, ext, keep) {
        let _ = fs::remove_file(dir.join(name));
    }
}

/// The newest crash file written since the last start: one not older than the
/// newest log (a log's name is its start time), or any when there is no log.
fn crash_since_last_start(names: &[String]) -> Option<String> {
    let last_start = names.iter().filter_map(|n| stamp_of(n, LOG_PREFIX, LOG_EXT)).max();
    names
        .iter()
        .filter_map(|n| stamp_of(n, CRASH_PREFIX, CRASH_EXT).map(|s| (s, n)))
        .filter(|(s, _)| last_start.is_none_or(|l| *s >= l))
        .max()
        .map(|(_, n)| n.clone())
}

/// Where logs and crash reports go: `ARTY_LOG_DIR`, else beside the recovery folder.
pub fn dir() -> PathBuf {
    std::env::var_os("ARTY_LOG_DIR").filter(|p| !p.is_empty()).map(PathBuf::from).unwrap_or_else(|| {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(std::env::temp_dir)
            .join("ARTY")
            .join("logs")
    })
}

/// Stops once `limit` bytes are written; the one line it adds says so.
struct CappedLog<W: Write> {
    out: W,
    written: u64,
    limit: u64,
    capped: bool,
}

impl<W: Write> CappedLog<W> {
    fn new(out: W, limit: u64) -> Self {
        Self { out, written: 0, limit, capped: false }
    }

    fn line(&mut self, line: &str) {
        if self.capped {
            return;
        }
        let bytes = line.len() as u64 + 1;
        if self.written + bytes > self.limit {
            self.capped = true;
            let _ = writeln!(self.out, "{TRUNCATED}");
            return;
        }
        self.written += bytes;
        let _ = writeln!(self.out, "{line}");
    }
}

enum Msg {
    Line(Arc<str>),
    /// Everything before this is on disk when the sender is answered.
    Flush(Sender<()>),
}

static SENDER: OnceLock<Sender<Msg>> = OnceLock::new();
static PREVIOUS_CRASH: OnceLock<Option<PathBuf>> = OnceLock::new();
/// The last log lines, for the crash report.
static RING: Mutex<VecDeque<Arc<str>>> = Mutex::new(VecDeque::new());
/// `Machine::log_line` as of the last change (machine.rs).
static MACHINE: Mutex<String> = Mutex::new(String::new());

/// Remembers the machine line for crash reports.
pub fn set_machine_line(line: &str) {
    if let Ok(mut m) = MACHINE.lock() {
        line.clone_into(&mut m);
    }
}

/// The crash file the previous run left, if there is one newer than that run's start.
pub fn previous_crash() -> Option<&'static Path> {
    PREVIOUS_CRASH.get().and_then(|p| p.as_deref())
}

/// The sink `env_logger` writes to: stderr, the ring and the writer thread.
struct Tee;

impl Write for Tee {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let _ = io::stderr().write_all(buf);
        let line: Arc<str> = String::from_utf8_lossy(buf).trim_end().into();
        if let Ok(mut ring) = RING.lock() {
            if ring.len() == REPORT_LOG_LINES {
                ring.pop_front();
            }
            ring.push_back(line.clone());
        }
        if let Some(tx) = SENDER.get() {
            let _ = tx.send(Msg::Line(line));
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        io::stderr().flush()
    }
}

/// The writer thread: lines in bursts, flushed when the channel runs dry.
fn write_logs(rx: std::sync::mpsc::Receiver<Msg>, file: File) {
    let mut log = CappedLog::new(BufWriter::new(file), MAX_LOG_BYTES);
    fn handle(msg: Msg, log: &mut CappedLog<BufWriter<File>>) {
        match msg {
            Msg::Line(l) => log.line(&l),
            Msg::Flush(ack) => {
                let _ = log.out.flush();
                let _ = ack.send(());
            }
        }
    }
    while let Ok(msg) = rx.recv() {
        handle(msg, &mut log);
        while let Ok(msg) = rx.try_recv() {
            handle(msg, &mut log);
        }
        let _ = log.out.flush();
    }
}

/// Waits (briefly) until the lines logged so far are in the file.
pub fn flush() {
    if let Some(tx) = SENDER.get() {
        let (ack, done) = channel();
        if tx.send(Msg::Flush(ack)).is_ok() {
            let _ = done.recv_timeout(FLUSH_WAIT);
        }
    }
}

/// Opens this run's log file and starts its writer; notes a crash left by the last run.
fn start_file(dir: &Path) -> Option<Sender<Msg>> {
    if let Err(e) = fs::create_dir_all(dir) {
        eprintln!("ARTY: no log folder {}: {e}", dir.display());
        return None;
    }
    let names = names_in(dir);
    let _ = PREVIOUS_CRASH.set(crash_since_last_start(&names).map(|n| dir.join(n)));
    prune(dir, LOG_PREFIX, LOG_EXT, KEEP_FILES - 1);
    let path = dir.join(format!("{LOG_PREFIX}{}{LOG_EXT}", Stamp::now().file_part()));
    let file = match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("ARTY: no log file {}: {e}", path.display());
            return None;
        }
    };
    // A BOM lets Notepad on any Windows 10 read the UTF-8.
    if file.metadata().is_ok_and(|m| m.len() == 0) {
        let _ = (&file).write_all("\u{feff}".as_bytes());
    }
    let (tx, rx) = channel();
    std::thread::Builder::new().name("arty-log".into()).spawn(move || write_logs(rx, file)).ok()?;
    Some(tx)
}

/// Starts logging to stderr and, unless this is a bench run, to a file; installs the
/// crash report hook. Call first thing in `main`.
pub fn init(bench: bool) {
    let to_file = !bench || std::env::var_os("ARTY_LOG_DIR").is_some();
    let dir = dir();
    if to_file && let Some(tx) = start_file(&dir) {
        let _ = SENDER.set(tx);
    }
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(DEFAULT_FILTER))
        .format(|buf, r| writeln!(buf, "{} {:<5} {} {}", Stamp::now().clock(), r.level(), r.target(), r.args()))
        .target(env_logger::Target::Pipe(Box::new(Tee)))
        .init();
    if to_file {
        install_panic_hook(dir);
    }
}

/// What a crash report says.
struct Report<'a> {
    version: &'a str,
    time: &'a str,
    windows: &'a str,
    machine: &'a str,
    image_base: usize,
    thread: &'a str,
    message: &'a str,
    location: &'a str,
    backtrace: &'a str,
    /// Return addresses of the panicking thread, as offsets in arty.exe.
    stack: &'a str,
    log: &'a [Arc<str>],
}

impl Report<'_> {
    fn text(&self) -> String {
        let machine = if self.machine.is_empty() { "machine: not detected yet" } else { self.machine };
        let mut s = format!(
            "ARTY crash report\n\
             version: {}\n\
             time:    {} (local)\n\
             windows: {}\n\
             {machine}\n\
             image base: {:#x} (backtrace address minus this = offset in arty.exe)\n\
             thread:  {}\n\
             panic:   {}\n\
             at:      {}\n\n\
             stack (offsets in arty.exe; arty.pdb of this build names them):\n{}\n\n\
             backtrace:\n{}\n\n",
            self.version, self.time, self.windows, self.image_base, self.thread, self.message, self.location, self.stack, self.backtrace
        );
        let first = self.log.len().saturating_sub(REPORT_LOG_LINES);
        s.push_str(&format!("last {} log lines:\n", self.log.len() - first));
        for line in &self.log[first..] {
            s.push_str(line);
            s.push('\n');
        }
        s
    }
}

fn install_panic_hook(dir: PathBuf) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // One report per run: a second panic only adds to the noise.
        static REPORTED: AtomicBool = AtomicBool::new(false);
        if !REPORTED.swap(true, Ordering::Relaxed)
            && let Err(e) = write_crash_file(&dir, info)
        {
            eprintln!("ARTY: could not write the crash report: {e}");
        }
        previous(info);
    }));
}

fn write_crash_file(dir: &Path, info: &std::panic::PanicHookInfo<'_>) -> io::Result<PathBuf> {
    flush();
    let payload = info.payload();
    let message = payload.downcast_ref::<&str>().map(|s| (*s).to_owned()).or_else(|| payload.downcast_ref::<String>().cloned());
    let location = info.location().map_or_else(|| "unknown".to_owned(), |l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
    let base = image_base();
    let stack = raw_stack(base);
    let backtrace = std::backtrace::Backtrace::force_capture().to_string();
    // A panic while the ring is locked would deadlock: skip the lines then.
    let log: Vec<Arc<str>> = RING.try_lock().map(|r| r.iter().cloned().collect()).unwrap_or_default();
    let machine = MACHINE.try_lock().map(|m| m.clone()).unwrap_or_default();
    let now = Stamp::now();
    let thread = std::thread::current();
    let text = Report {
        version: &crate::about::version_line(),
        time: &now.date_time(),
        windows: &windows_version(),
        machine: &machine,
        image_base: base,
        thread: thread.name().unwrap_or("unnamed"),
        message: message.as_deref().unwrap_or("(no message)"),
        location: &location,
        backtrace: &backtrace,
        stack: &stack,
        log: &log,
    }
    .text();
    fs::create_dir_all(dir)?;
    let path = dir.join(format!("{CRASH_PREFIX}{}{CRASH_EXT}", now.file_part()));
    // A BOM lets Notepad on any Windows 10 read the UTF-8 (the machine line has a middle dot).
    fs::write(&path, format!("\u{feff}{text}"))?;
    prune(dir, CRASH_PREFIX, CRASH_EXT, KEEP_FILES);
    Ok(path)
}

/// "Windows 11 (10.0.26200)"; Windows 11 still says major 10.
fn windows_name(major: u32, minor: u32, build: u32) -> String {
    let name = if major == 10 && build >= 22_000 { "Windows 11" } else if major == 10 { "Windows 10" } else { "Windows" };
    format!("{name} ({major}.{minor}.{build})")
}

#[cfg(windows)]
fn windows_version() -> String {
    use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
    use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;
    let mut v = OSVERSIONINFOW { dwOSVersionInfoSize: size_of::<OSVERSIONINFOW>() as u32, dwMajorVersion: 0, dwMinorVersion: 0, dwBuildNumber: 0, dwPlatformId: 0, szCSDVersion: [0; 128] };
    // SAFETY: `v` is a valid OSVERSIONINFOW with its size set.
    if unsafe { RtlGetVersion(&mut v) } == 0 {
        windows_name(v.dwMajorVersion, v.dwMinorVersion, v.dwBuildNumber)
    } else {
        "Windows (unknown version)".to_owned()
    }
}

#[cfg(not(windows))]
fn windows_version() -> String {
    "not Windows".to_owned()
}

#[cfg(windows)]
fn image_base() -> usize {
    // SAFETY: a null name asks for the module of the running exe.
    unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) as usize }
}

#[cfg(not(windows))]
fn image_base() -> usize {
    0
}

/// Return addresses of this thread: `+0x1a2b3c` inside the exe, else absolute.
#[cfg(windows)]
fn raw_stack(base: usize) -> String {
    use windows_sys::Win32::System::Diagnostics::Debug::RtlCaptureStackBackTrace;
    let mut frames = [std::ptr::null_mut::<std::ffi::c_void>(); 48];
    // SAFETY: the buffer holds 48 pointers; no hash is asked for.
    let n = unsafe { RtlCaptureStackBackTrace(0, 48, frames.as_mut_ptr(), std::ptr::null_mut()) } as usize;
    let shown: Vec<String> = frames[..n].iter().map(|&f| stack_entry(f as usize, base)).collect();
    shown.chunks(8).map(|c| c.join(" ")).collect::<Vec<_>>().join("\n")
}

#[cfg(not(windows))]
fn raw_stack(_base: usize) -> String {
    "not available".to_owned()
}

/// An address as an offset in the exe when it lies within it (64 MiB from its base).
#[cfg(any(windows, test))]
fn stack_entry(addr: usize, base: usize) -> String {
    match addr.checked_sub(base) {
        Some(off) if base != 0 && off < 0x400_0000 => format!("+{off:#x}"),
        _ => format!("{addr:#x}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("arty-logging-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn stamps_from_unix_seconds() {
        let s = Stamp::from_unix(0, 0);
        assert_eq!((s.year, s.month, s.day, s.hour), (1970, 1, 1, 0));
        // 2026-10-06 14:30:15 UTC
        let s = Stamp::from_unix(1_791_297_015, 123);
        assert_eq!(s.date_time(), "2026-10-06 14:30:15");
        assert_eq!(s.file_part(), "20261006-143015");
        assert_eq!(s.clock(), "14:30:15.123");
        // A leap day.
        assert_eq!(Stamp::from_unix(1_709_164_800, 0).date_time(), "2024-02-29 00:00:00");
    }

    #[test]
    fn file_names_order_by_time() {
        assert_eq!(stamp_of("arty-20261006-143015.log", LOG_PREFIX, LOG_EXT), Some(20_261_006_143_015));
        assert_eq!(stamp_of("crash-20261006-143015.txt", CRASH_PREFIX, CRASH_EXT), Some(20_261_006_143_015));
        for bad in ["arty-2026100-143015.log", "arty-20261006-14301.log", "arty-20261006-143015.txt", "arty-latest.log", "notes.log", "arty-2026100a-143015.log"] {
            assert_eq!(stamp_of(bad, LOG_PREFIX, LOG_EXT), None, "{bad}");
        }
    }

    #[test]
    fn rotation_keeps_the_newest() {
        let list = names(&[
            "arty-20261003-100000.log",
            "arty-20261001-100000.log",
            "arty-20261005-100000.log",
            "arty-20261002-100000.log",
            "arty-20261004-100000.log",
            "arty-20261006-100000.log",
            "arty-20261007-100000.log",
            "crash-20261001-100000.txt",
            "readme.txt",
        ]);
        assert_eq!(expired(&list, LOG_PREFIX, LOG_EXT, 5), ["arty-20261001-100000.log", "arty-20261002-100000.log"]);
        assert!(expired(&list, LOG_PREFIX, LOG_EXT, 7).is_empty());
        assert!(expired(&list, LOG_PREFIX, LOG_EXT, 9).is_empty());
        assert_eq!(expired(&list, CRASH_PREFIX, CRASH_EXT, 0), ["crash-20261001-100000.txt"]);
    }

    #[test]
    fn prune_deletes_only_expired_log_files() {
        let dir = temp_dir("prune");
        for day in 1..=8 {
            fs::write(dir.join(format!("arty-202610{day:02}-120000.log")), "x").unwrap();
        }
        fs::write(dir.join("crash-20261001-120000.txt"), "x").unwrap();
        fs::write(dir.join("keep.txt"), "x").unwrap();
        prune(&dir, LOG_PREFIX, LOG_EXT, KEEP_FILES - 1);
        let mut left = names_in(&dir);
        left.sort();
        assert_eq!(
            left,
            ["arty-20261005-120000.log", "arty-20261006-120000.log", "arty-20261007-120000.log", "arty-20261008-120000.log", "crash-20261001-120000.txt", "keep.txt"]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_log_stops_at_its_size_cap() {
        let mut log = CappedLog::new(Vec::new(), 40);
        log.line("0123456789"); // 11 bytes
        log.line("0123456789"); // 22
        log.line("0123456789"); // 33
        log.line("0123456789"); // would be 44: refused, one notice
        log.line("short");
        log.line("short");
        let text = String::from_utf8(log.out).unwrap();
        assert_eq!(text, format!("{}\n{}\n{}\n{TRUNCATED}\n", "0123456789", "0123456789", "0123456789"));
        // A line that fits exactly is kept.
        let mut log = CappedLog::new(Vec::new(), 11);
        log.line("0123456789");
        assert_eq!(log.out.len(), 11);
        assert!(!log.capped);
    }

    #[test]
    fn a_crash_is_news_only_when_newer_than_the_last_start() {
        // Run A started at 10:00 and crashed at 10:05; this is run B's start.
        let list = names(&["arty-20261006-100000.log", "crash-20261006-100500.txt"]);
        assert_eq!(crash_since_last_start(&list).as_deref(), Some("crash-20261006-100500.txt"));
        // Run B (10:10) ran fine: at run C's start the crash is old news.
        let list = names(&["arty-20261006-100000.log", "crash-20261006-100500.txt", "arty-20261006-101000.log"]);
        assert_eq!(crash_since_last_start(&list), None);
        // Crash in the same second as the start still counts.
        let list = names(&["arty-20261006-100000.log", "crash-20261006-100000.txt"]);
        assert!(crash_since_last_start(&list).is_some());
        // No log left at all: any crash file is news; the newest is named.
        let list = names(&["crash-20261005-090000.txt", "crash-20261006-100500.txt"]);
        assert_eq!(crash_since_last_start(&list).as_deref(), Some("crash-20261006-100500.txt"));
        assert_eq!(crash_since_last_start(&names(&["arty-20261006-100000.log"])), None);
        assert_eq!(crash_since_last_start(&[]), None);
    }

    #[test]
    fn stack_entries_are_offsets_in_the_exe() {
        assert_eq!(stack_entry(0x7ff6_0001_2345, 0x7ff6_0000_0000), "+0x12345");
        assert_eq!(stack_entry(0x7ffa_1234_0000, 0x7ff6_0000_0000), "0x7ffa12340000");
        assert_eq!(stack_entry(0x1000, 0x7ff6_0000_0000), "0x1000");
    }

    #[test]
    fn windows_names() {
        assert_eq!(windows_name(10, 0, 19045), "Windows 10 (10.0.19045)");
        assert_eq!(windows_name(10, 0, 26200), "Windows 11 (10.0.26200)");
        assert_eq!(windows_name(6, 1, 7601), "Windows (6.1.7601)");
    }

    #[test]
    fn the_report_carries_the_last_log_lines() {
        let log: Vec<Arc<str>> = (0..250).map(|i| Arc::from(format!("line {i}"))).collect();
        let text = Report {
            version: "0.2.0 (abc12345) Preview",
            time: "2026-10-06 14:30:15",
            windows: "Windows 11 (10.0.26200)",
            machine: "machine: RAM 7.8 GiB",
            image_base: 0x7ff6_0000_0000,
            thread: "main",
            message: "boom",
            location: "src/app.rs:1:2",
            backtrace: "   0: frame",
            stack: "+0x10 +0x20",
            log: &log,
        }
        .text();
        for needle in ["ARTY crash report", "version: 0.2.0 (abc12345) Preview", "windows: Windows 11", "machine: RAM 7.8 GiB", "panic:   boom", "at:      src/app.rs:1:2", "0: frame", "+0x10 +0x20", "last 200 log lines:", "line 249\n", "line 50\n"] {
            assert!(text.contains(needle), "{needle:?} missing:\n{text}");
        }
        assert!(!text.contains("line 49\n"), "only the last 200 lines");
    }
}
