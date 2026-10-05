//! Bench hooks, each off unless its environment variable is set (plans/bench
//! B005, B013). None of them sends OS input: the stroke bench pushes samples
//! into ARTY's own pen queue, where the Windows Ink hook puts real ones.
//! Reports go to stderr, one line each, starting with the variable's name.
//!
//! - `ARTY_BENCH_PAN=<secs>` (B005): rotate the view 0.5° per frame (after
//!   the open, if any), report frame intervals, then quit.
//! - `ARTY_BENCH_OPEN=<file.arty>`: open the file at start-up through
//!   File > Open (the file dialog answers with it once) and report when the
//!   document is in.
//! - `ARTY_BENCH_STROKE=<secs>`: after start-up (and the open, if any) draw a
//!   looping path with a 240 Hz pen on the active layer, lifting the pen for
//!   0.1 s every 2 s, report frame times and the frames that ended a stroke,
//!   then quit without saving.
//! - `ARTY_BENCH_WINDOW=<W>x<H>`: resize the window's client area to W×H
//!   physical px on the first frames (eframe restores the saved geometry
//!   first) with its top-left at the primary monitor's (a capture shows
//!   nothing of what lies off screen), fit the page again as a start at that
//!   size would, and report the size reached; the geometry is then not saved.
//! - `ARTY_BENCH_ZOOM=<factor>`: egui zoom factor (UI scale on top of the OS scale).
//! - `ARTY_IO_THREADS=<n>`: io pool size instead of `IoConfig::new`'s, which
//!   counts the machine's CPUs, not the process affinity.
//!
//! While any of these is set nothing is saved to app.ron (window, egui
//! memory, settings), so a bench run does not change the next start.

use std::f64::consts::TAU;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use arty_pen::{PenEnd, PenPhase, PenQueue, PenSample};

use crate::files::{FileController, FileDialogs, NativeDialogs};
use crate::studio::{DisplaySync, Studio};

/// Synthetic pen rate (a typical Windows Ink pen: ~4 samples per 60 Hz frame).
pub const STROKE_HZ: f64 = 240.0;
/// Samples per contact (2 s at 240 Hz).
const CONTACT: u64 = 480;
/// Samples with the pen up between contacts (0.1 s).
const GAP: u64 = 24;
/// Samples over which the pressure ramps in and out of a contact.
const RAMP: u64 = 12;
/// Seconds per loop of the path; not a divisor of a contact, so contacts start at different places.
const LOOP_SECS: f64 = 1.7;
/// Pointer id of the synthetic pen.
const POINTER: u32 = 0xBE7C;
/// Frames before the stroke bench starts (start-up work, first uploads).
const WARMUP_FRAMES: u32 = 30;
/// Most samples pushed per frame (the native queue holds 1024).
const MAX_PER_FRAME: u64 = 256;
/// The path radius as a share of the window's smaller side.
const RADIUS: f32 = 0.18;

/// What a path sample does with the pen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Contact {
    Down,
    Move,
    Up,
    Hover,
}

/// One sample of the bench path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PathPoint {
    /// Offset from the canvas centre in path radii (|offset| < 1.5).
    pub offset: [f32; 2],
    /// Pen pressure, 0..=1.
    pub pressure: f32,
    pub contact: Contact,
}

/// Sample `k` of the deterministic bench path: a wobbling loop drifting
/// slowly over the canvas, in contacts of [`CONTACT`] samples with
/// [`GAP`] samples of hover between them. Depends only on `k`.
pub fn path_point(k: u64) -> PathPoint {
    let t = k as f64 / STROKE_HZ;
    let theta = TAU * t / LOOP_SECS;
    let r = 0.8 + 0.2 * (3.0 * theta + 0.7 * t).sin();
    let drift = [0.3 * (0.21 * t).sin(), 0.3 * (0.17 * t).cos()];
    let offset = [(r * theta.cos() + drift[0]) as f32, (0.8 * r * theta.sin() + drift[1]) as f32];
    let c = k % (CONTACT + GAP);
    let contact = match c {
        0 => Contact::Down,
        c if c < CONTACT - 1 => Contact::Move,
        c if c == CONTACT - 1 => Contact::Up,
        _ => Contact::Hover,
    };
    let ramp = ((c + 1).min(CONTACT.saturating_sub(c)) as f64 / RAMP as f64).min(1.0);
    let pressure = match contact {
        Contact::Hover => 0.0,
        _ => ((0.55 + 0.35 * (TAU * 0.9 * t).sin()) * ramp).clamp(0.02, 1.0) as f32,
    };
    PathPoint { offset, pressure, contact }
}

/// Seconds, > 0.
fn parse_secs(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0)
}

/// `<W>x<H>` (`x`, `X` or `×`), each side 320..=16384.
fn parse_size(s: &str) -> Option<[f32; 2]> {
    let (w, h) = s.trim().split_once(['x', 'X', '×'])?;
    let side = |v: &str| v.trim().parse::<u32>().ok().filter(|v| (320..=16384).contains(v)).map(|v| v as f32);
    Some([side(w)?, side(h)?])
}

/// egui zoom factor, 0.25..=4.
fn parse_zoom(s: &str) -> Option<f32> {
    s.trim().parse::<f32>().ok().filter(|v| (0.25..=4.0).contains(v))
}

/// Thread count, 1..=64.
fn parse_threads(s: &str) -> Option<usize> {
    s.trim().parse::<usize>().ok().filter(|v| (1..=64).contains(v))
}

/// The variable parsed, or `None` (with a note on stderr when it is set but invalid).
fn env<T>(name: &str, parse: fn(&str) -> Option<T>) -> Option<T> {
    let s = std::env::var(name).ok()?;
    let v = parse(&s);
    if v.is_none() {
        eprintln!("{name}: ignoring invalid value {s:?}");
    }
    v
}

/// Any bench variable is set (see the module docs).
pub fn active() -> bool {
    std::env::vars_os().any(|(k, _)| k.to_str().is_some_and(|k| k.starts_with("ARTY_BENCH_") || k == "ARTY_IO_THREADS"))
}

pub fn window_size() -> Option<[f32; 2]> {
    env("ARTY_BENCH_WINDOW", parse_size)
}

pub fn zoom() -> Option<f32> {
    env("ARTY_BENCH_ZOOM", parse_zoom)
}

pub fn io_threads() -> Option<usize> {
    env("ARTY_IO_THREADS", parse_threads)
}

pub fn open_path() -> Option<PathBuf> {
    std::env::var_os("ARTY_BENCH_OPEN").filter(|p| !p.is_empty()).map(PathBuf::from)
}

pub fn stroke_secs() -> Option<f64> {
    env("ARTY_BENCH_STROKE", parse_secs)
}

/// p-quantile of sorted values (nearest rank), 0 when empty.
fn quantile(sorted: &[f32], p: f32) -> f32 {
    sorted.get(((sorted.len() as f32 - 1.0) * p).round() as usize).copied().unwrap_or(0.0)
}

/// File > Open answers with the bench file once, then asks as usual.
pub struct BenchDialogs(pub Option<PathBuf>);

impl FileDialogs for BenchDialogs {
    fn open_path(&mut self) -> Option<PathBuf> {
        self.0.take().or_else(|| NativeDialogs.open_path())
    }

    fn save_path(&mut self, name: &str, dir: Option<&Path>) -> Option<PathBuf> {
        NativeDialogs.save_path(name, dir)
    }
}

pub struct Pan {
    secs: f64,
    start: Option<f64>,
    /// Frame intervals, ms.
    dts: Vec<f32>,
}

pub struct Open {
    path: PathBuf,
    /// `Studio::doc_epoch` before the load.
    epoch: u64,
    first_frame: Option<Instant>,
    warned: bool,
}

pub struct Stroke {
    secs: f64,
    queue: Rc<PenQueue>,
    frames: u32,
    /// `arty_pen::now_secs` of sample 0.
    t0: Option<f64>,
    /// Next path sample to push.
    next: u64,
    /// Pen position of the newest pushed sample, client physical px.
    pos: Option<[f32; 2]>,
    /// This frame pushed a pen-up.
    up_now: bool,
    strokes: u32,
    /// Frame intervals and `ArtyApp::ui` times, ms.
    dts: Vec<f32>,
    ui_ms: Vec<f32>,
    /// `ArtyApp::ui` time of the frames that ended a stroke, ms.
    up_ms: Vec<f32>,
}

/// Client size asked by `ARTY_BENCH_WINDOW`, physical px, the frames left
/// to reach it, and the frames still repainted once it is reached.
pub struct Window {
    px: [f32; 2],
    tries: u32,
    after: Option<u32>,
}

/// The bench hooks this run asked for.
pub struct Bench {
    /// `ArtyApp::new` ran (window and GPU are up).
    created: Instant,
    window: Option<Window>,
    pan: Option<Pan>,
    pub open: Option<Open>,
    stroke: Option<Stroke>,
}

impl Bench {
    /// The hooks set in the environment, or `None`. `doc_epoch` is the
    /// start-up document's; `queue` the native pen queue, if installed.
    pub fn from_env(doc_epoch: u64, queue: Option<&Rc<PenQueue>>) -> Option<Self> {
        let pan = env("ARTY_BENCH_PAN", parse_secs).map(|secs| Pan { secs, start: None, dts: Vec::with_capacity(4096) });
        let open = open_path().map(|path| Open { path, epoch: doc_epoch, first_frame: None, warned: false });
        let stroke = stroke_secs().map(|secs| {
            // Without the Windows Ink hook the canvas reads no queue; this one is then unused.
            let queue = queue.cloned().unwrap_or_else(|| Rc::new(PenQueue::new(1024)));
            let frames = (secs * 500.0) as usize + 64;
            Stroke {
                secs,
                queue,
                frames: 0,
                t0: None,
                next: 0,
                pos: None,
                up_now: false,
                strokes: 0,
                dts: Vec::with_capacity(frames),
                ui_ms: Vec::with_capacity(frames),
                up_ms: Vec::with_capacity((secs / 2.0) as usize + 8),
            }
        });
        let window = window_size().map(|px| Window { px, tries: 20, after: None });
        (pan.is_some() || open.is_some() || stroke.is_some() || window.is_some())
            .then(|| Bench { created: Instant::now(), window, pan, open, stroke })
    }

    /// Before the frame: push this frame's pen samples, and move egui's
    /// pointer with the pen so the canvas sees it hovered (as with a real pen).
    pub fn raw_input(&mut self, raw: &mut egui::RawInput, ppp: f32, canvas_center_px: [f32; 2], studio: &Studio) {
        let Some(s) = &mut self.stroke else { return };
        s.up_now = false;
        s.frames += 1;
        if s.frames < WARMUP_FRAMES || self.open.is_some() {
            return;
        }
        if !studio.input.native_pen {
            eprintln!("ARTY_BENCH_STROKE: native pen input is off in Settings; nothing to measure");
            self.stroke = None;
            return;
        }
        let now = arty_pen::now_secs();
        let t0 = *s.t0.get_or_insert(now);
        let due = ((now - t0) * STROKE_HZ) as u64 + 1;
        let side = raw.screen_rect.map_or(800.0, |r| r.width().min(r.height())) * ppp;
        let radius = RADIUS * side;
        for k in s.next..due.min(s.next + MAX_PER_FRAME) {
            let p = path_point(k);
            let pos = [canvas_center_px[0] + radius * p.offset[0], canvas_center_px[1] + radius * p.offset[1]];
            let phase = match p.contact {
                Contact::Down => PenPhase::Down,
                Contact::Move => PenPhase::Move,
                Contact::Up => PenPhase::Up,
                Contact::Hover => PenPhase::Hover,
            };
            if p.contact == Contact::Up {
                s.up_now = true;
                s.strokes += 1;
            }
            let time = t0 + k as f64 / STROKE_HZ;
            s.queue.push(PenSample { pointer: POINTER, phase, end: PenEnd::Tip, barrel: false, pos, pressure: Some(p.pressure), tilt: [0.0; 2], time });
            s.pos = Some(pos);
            s.next = k + 1;
        }
        if let Some([x, y]) = s.pos {
            raw.events.push(egui::Event::PointerMoved(egui::pos2(x / ppp, y / ppp)));
        }
    }

    /// Start of `ArtyApp::ui`: the pan bench turns the view.
    pub fn frame(&mut self, ctx: &egui::Context, studio: &mut Studio, sync: DisplaySync) {
        let (now, dt) = ctx.input(|i| (i.time, i.unstable_dt));
        if let Some(w) = &mut self.window {
            // InnerSize is in points at the current pixels_per_point (OS scale × zoom).
            let ppp = ctx.pixels_per_point();
            let inner = ctx.input(|i| i.viewport().inner_rect).map(|r| [r.width() * ppp, r.height() * ppp]);
            let reached = inner.is_some_and(|[w0, h0]| (w0 - w.px[0]).abs() < 1.5 && (h0 - w.px[1]).abs() < 1.5);
            if let Some(after) = &mut w.after {
                // A few more frames, so the one on screen has the final size.
                *after -= 1;
                if *after == 0 {
                    self.window = None;
                } else {
                    ctx.request_repaint_after(Duration::from_millis(100));
                }
            } else if reached || w.tries == 0 {
                let [iw, ih] = inner.unwrap_or_default();
                eprintln!("ARTY_BENCH_WINDOW asked {}×{} px · client {iw:.0}×{ih:.0} px · {ppp:.2} px per point", w.px[0], w.px[1]);
                studio.fit_pending = true;
                w.after = Some(10);
                ctx.request_repaint();
            } else {
                w.tries -= 1;
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(w.px[0] / ppp, w.px[1] / ppp)));
                if let Some((outer, inner)) = ctx.input(|i| i.viewport().outer_rect.zip(i.viewport().inner_rect)) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition((outer.min - inner.min).to_pos2()));
                }
                ctx.request_repaint_after(Duration::from_millis(50));
            }
        }
        if let Some(o) = &mut self.open {
            o.first_frame.get_or_insert_with(Instant::now);
        }
        if let Some(b) = &mut self.pan
            && self.open.is_none()
        {
            let start = *b.start.get_or_insert(now);
            b.dts.push(dt * 1000.0);
            let view = &mut studio.view;
            view.rotate_at([0.0; 2], [0.0; 2], view.rotation + 0.5f32.to_radians());
            ctx.request_repaint();
            if now - start >= b.secs {
                // The first frames include start-up work.
                let mut dts = b.dts.split_off(b.dts.len().min(30));
                dts.sort_by(f32::total_cmp);
                eprintln!(
                    "ARTY_BENCH_PAN {:.0} s · {} ({:?}) · {} frames · frame ms p50 {:.2} p95 {:.2} max {:.2}",
                    b.secs,
                    sync.label(),
                    sync.surface_config(studio.fast_vsync_ok),
                    dts.len(),
                    quantile(&dts, 0.5),
                    quantile(&dts, 0.95),
                    quantile(&dts, 1.0)
                );
                self.pan = None;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        if let Some(s) = &mut self.stroke {
            ctx.request_repaint();
            if s.t0.is_some() {
                s.dts.push(dt * 1000.0);
            }
        }
    }

    /// End of `ArtyApp::ui`, which took `ui`: the open bench reports a
    /// finished load (the file controller swaps the document in during the
    /// frame); the stroke bench records the frame, and reports and quits
    /// (dropping the strokes) once done.
    pub fn frame_done(&mut self, ctx: &egui::Context, ui: Duration, studio: &Studio, files: &mut FileController, sync: DisplaySync) {
        if let Some(o) = &mut self.open
            && let Some(first) = o.first_frame
        {
            if studio.doc_epoch != o.epoch {
                let ms = |since: Instant| since.elapsed().as_secs_f64() * 1000.0;
                let d = &studio.doc;
                eprintln!(
                    "ARTY_BENCH_OPEN {} · loaded {:.0} ms after the first frame ({:.0} ms after start-up) · {}×{} px · {} layers · {:.1} MB",
                    o.path.display(),
                    ms(first),
                    ms(self.created),
                    d.width(),
                    d.height(),
                    d.layer_count(),
                    d.pixel_bytes() as f64 / (1024.0 * 1024.0)
                );
                self.open = None;
            } else if !o.warned && first.elapsed() > Duration::from_secs(60) {
                eprintln!("ARTY_BENCH_OPEN {}: not loaded after 60 s (is a dialog open?)", o.path.display());
                o.warned = true;
            }
        }
        let Some(s) = &mut self.stroke else { return };
        let Some(t0) = s.t0 else { return };
        let ms = ui.as_secs_f32() * 1000.0;
        s.ui_ms.push(ms);
        if s.up_now {
            s.up_ms.push(ms);
        }
        let pen_up = path_point(s.next.saturating_sub(1)).contact == Contact::Hover;
        if arty_pen::now_secs() - t0 < s.secs || !pen_up || s.strokes == 0 {
            return;
        }
        for v in [&mut s.dts, &mut s.ui_ms, &mut s.up_ms] {
            v.sort_by(f32::total_cmp);
        }
        let p = studio.preset();
        let d = &studio.doc;
        eprintln!(
            "ARTY_BENCH_STROKE {:.0} s · {} · {} frames · frame ms p50 {:.2} p95 {:.2} p99 {:.2} max {:.2} · \
             ui ms p50 {:.2} p95 {:.2} p99 {:.2} max {:.2} · pen-up frames {} ui ms p50 {:.2} max {:.2} · \
             {} samples at {:.0} Hz · dropped {} · {} {:.1}px · {}×{} px · {} layers · {:.1} MB",
            s.secs,
            sync.label(),
            s.dts.len(),
            quantile(&s.dts, 0.5),
            quantile(&s.dts, 0.95),
            quantile(&s.dts, 0.99),
            quantile(&s.dts, 1.0),
            quantile(&s.ui_ms, 0.5),
            quantile(&s.ui_ms, 0.95),
            quantile(&s.ui_ms, 0.99),
            quantile(&s.ui_ms, 1.0),
            s.up_ms.len(),
            quantile(&s.up_ms, 0.5),
            quantile(&s.up_ms, 1.0),
            s.next,
            STROKE_HZ,
            s.queue.dropped(),
            p.name,
            p.size,
            d.width(),
            d.height(),
            d.layer_count(),
            d.pixel_bytes() as f64 / (1024.0 * 1024.0)
        );
        self.stroke = None;
        files.discard_on_close();
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_values_parse() {
        assert_eq!(parse_secs("20"), Some(20.0));
        assert_eq!(parse_secs(" 2.5 "), Some(2.5));
        for bad in ["0", "-1", "nan", "inf", "", "x"] {
            assert_eq!(parse_secs(bad), None, "{bad}");
        }
        assert_eq!(parse_size("1366x768"), Some([1366.0, 768.0]));
        assert_eq!(parse_size("1920X1080"), Some([1920.0, 1080.0]));
        assert_eq!(parse_size("1920×1080"), Some([1920.0, 1080.0]));
        for bad in ["1366", "1366x", "x768", "100x768", "1366x99999", "axb", "1366,768"] {
            assert_eq!(parse_size(bad), None, "{bad}");
        }
        assert_eq!(parse_zoom("1.5"), Some(1.5));
        for bad in ["0", "0.1", "5", "nan", "big"] {
            assert_eq!(parse_zoom(bad), None, "{bad}");
        }
        assert_eq!(parse_threads("2"), Some(2));
        for bad in ["0", "65", "-2", "two"] {
            assert_eq!(parse_threads(bad), None, "{bad}");
        }
    }

    #[test]
    fn stroke_path_is_deterministic_and_well_formed() {
        let cycle = CONTACT + GAP;
        let pts: Vec<PathPoint> = (0..3 * cycle).map(path_point).collect();
        assert!(pts.iter().zip(0..).all(|(p, k)| *p == path_point(k)), "same k, same sample");
        for (i, c) in pts.chunks(cycle as usize).enumerate() {
            let count = |want: Contact| c.iter().filter(|p| p.contact == want).count() as u64;
            assert_eq!((count(Contact::Down), count(Contact::Up)), (1, 1), "contact {i}");
            assert_eq!(count(Contact::Hover), GAP, "contact {i}");
            assert_eq!(c[0].contact, Contact::Down);
            assert_eq!(c[CONTACT as usize - 1].contact, Contact::Up);
        }
        for p in &pts {
            assert!(p.offset[0].hypot(p.offset[1]) < 1.5, "{p:?}");
            match p.contact {
                Contact::Hover => assert_eq!(p.pressure, 0.0),
                _ => assert!(p.pressure > 0.0 && p.pressure <= 1.0, "{p:?}"),
            }
        }
        // Pressure ramps in and out; contacts start at different places.
        assert!(pts[0].pressure < pts[RAMP as usize].pressure);
        assert!(pts[CONTACT as usize - 1].pressure < pts[(CONTACT - RAMP) as usize - 1].pressure);
        assert_ne!(pts[0].offset, pts[cycle as usize].offset);
        // About 2.5 px per sample at a 200 px radius: a realistic pen speed.
        let step = (pts[1].offset[0] - pts[0].offset[0]).hypot(pts[1].offset[1] - pts[0].offset[1]) * 200.0;
        assert!((1.0..6.0).contains(&step), "{step}");
    }

    #[test]
    fn quantiles() {
        let v = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!((quantile(&v, 0.5), quantile(&v, 1.0), quantile(&v, 0.0)), (3.0, 5.0, 1.0));
        assert_eq!(quantile(&[], 0.5), 0.0);
    }
}
