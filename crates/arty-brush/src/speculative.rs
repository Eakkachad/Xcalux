//! Speculative stroke reshaping and replay (Zero-Wait Pen-Up).
//!
//! Renders the stable prefix of a shaped stroke concurrently on a background
//! worker thread running below normal priority into an isolated shadow grid.
//! At pen-up, only the unstable remainder (~40-50 dabs) is rendered, achieving
//! pen-up latency p99 < 8 ms with bit-identical layer output.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{JoinHandle, Thread};
use std::time::{Duration, Instant};

use arty_core::tile::new_tile_box;
use arty_core::{Selection, TILE_SIZE, TileCoord, TileGrid, TilePixels};
use hokusai::BrushState;

use crate::shape::{
    DabStats, MAX_LOGGED_SAMPLES, ShapeSample, correct_path, correction_sigma_px, correction_taps, seg_len,
    station_step, taper, unstable_reach,
};
use crate::surface::MaskCur;

#[cfg(windows)]
pub fn lower_thread_priority() {
    unsafe {
        let thread = windows_sys::Win32::System::Threading::GetCurrentThread();
        windows_sys::Win32::System::Threading::SetThreadPriority(
            thread,
            windows_sys::Win32::System::Threading::THREAD_PRIORITY_BELOW_NORMAL,
        );
    }
}

#[cfg(not(windows))]
pub fn lower_thread_priority() {}

/// Pre-allocated, lock-free single-producer single-consumer ring buffer for shape samples.
pub struct SampleRingBuffer {
    buffer: Box<[UnsafeCell<ShapeSample>]>,
    cap: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
}

unsafe impl Sync for SampleRingBuffer {}
unsafe impl Send for SampleRingBuffer {}

impl SampleRingBuffer {
    pub fn new(cap: usize) -> Self {
        let cap = cap.next_power_of_two().max(1024);
        let mut v = Vec::with_capacity(cap);
        for _ in 0..cap {
            v.push(UnsafeCell::new(ShapeSample::default()));
        }
        Self {
            buffer: v.into_boxed_slice(),
            cap,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Push one sample. Lock-free, allocation-free.
    #[inline]
    pub fn push(&self, sample: ShapeSample) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        if head.wrapping_sub(tail) >= self.cap {
            return false;
        }
        unsafe {
            *self.buffer[head & (self.cap - 1)].get() = sample;
        }
        self.head.store(head.wrapping_add(1), Ordering::Release);
        true
    }

    /// Pop one sample. Lock-free, allocation-free.
    #[inline]
    pub fn pop(&self) -> Option<ShapeSample> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        if tail == head {
            return None;
        }
        let sample = unsafe { *self.buffer[tail & (self.cap - 1)].get() };
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        Some(sample)
    }

    pub fn clear(&self) {
        let head = self.head.load(Ordering::Relaxed);
        self.tail.store(head, Ordering::Release);
    }
}

/// Surface for speculative rendering into an isolated shadow tile grid.
pub struct ShadowSurface<'a> {
    pub shadow_grid: &'a mut TileGrid,
    pub pre_stroke_grid: &'a TileGrid,
    pub discard: &'a mut TilePixels,
    pub tiles_wide: i32,
    pub tiles_high: i32,
    pub mask: Option<&'a Selection>,
    pub mask_cur: MaskCur<'a>,
    pub stats: &'a mut DabStats,
}

impl ShadowSurface<'_> {
    #[inline]
    fn in_page(&self, c: TileCoord) -> bool {
        c.x >= 0 && c.y >= 0 && c.x < self.tiles_wide && c.y < self.tiles_high
    }
}

impl hokusai::TiledSurface for ShadowSurface<'_> {
    fn tile_request_start(&mut self, tx: i32, ty: i32) -> &mut hokusai::TilePixels {
        let c = TileCoord::new(tx, ty);
        if !self.in_page(c) {
            self.mask_cur = MaskCur::All;
            return self.discard;
        }
        if let Some(sel) = self.mask {
            self.mask_cur = match sel.get(c) {
                arty_core::MaskView::Empty => {
                    self.mask_cur = MaskCur::None;
                    return self.discard;
                }
                arty_core::MaskView::Full => MaskCur::All,
                arty_core::MaskView::Partial(m) => {
                    let (ox, oy) = c.origin();
                    MaskCur::Tile(m, ox, oy)
                }
            };
        }
        if self.shadow_grid.get_ref(c).is_none()
            && let Some(t) = self.pre_stroke_grid.get_ref(c)
        {
            self.shadow_grid.insert(c, t.clone());
        }
        self.shadow_grid.get_mut_or_create(c)
    }

    fn tile_request_end(&mut self, _tx: i32, _ty: i32) {}

    fn draw_dab(&mut self, dab: &hokusai::Dab) -> bool {
        if let Some(sel) = self.mask {
            let r = dab.radius + 1.0;
            let t = |v: f32| (v.floor() as i32).div_euclid(TILE_SIZE as i32);
            let hit = sel.bounds().is_some_and(|b| {
                t(dab.x - r) < b.x1 && t(dab.x + r) >= b.x0 && t(dab.y - r) < b.y1 && t(dab.y + r) >= b.y0
            });
            if !hit {
                return false;
            }
        }
        self.stats.dabs += 1;
        let d = (2.0 * dab.radius + 3.0) as u64;
        self.stats.px += d * d;
        hokusai::brushmodes::draw_dab_default(self, dab)
    }

    fn tile_lookup(&self, tx: i32, ty: i32) -> Option<&hokusai::TilePixels> {
        let c = TileCoord::new(tx, ty);
        self.shadow_grid.get(c).or_else(|| self.pre_stroke_grid.get(c))
    }

    #[inline]
    fn get_pixel_mask(&self, px: f32, py: f32, _dab: &hokusai::Dab) -> f32 {
        match self.mask_cur {
            MaskCur::All => 1.0,
            MaskCur::None => 0.0,
            MaskCur::Tile(m, _ox, _oy) => {
                let (x, y) = (px as i32, py as i32);
                static MASK_SCALE: [f32; 256] = {
                    let mut t = [0.0f32; 256];
                    let mut i = 0;
                    while i < 256 {
                        t[i] = i as f32 / 255.0;
                        i += 1;
                    }
                    t
                };
                MASK_SCALE[usize::from(m[y as usize & (TILE_SIZE - 1)][x as usize & (TILE_SIZE - 1)])]
            }
        }
    }
}

/// Static configuration describing a stroke to the speculative worker.
#[derive(Clone)]
pub struct StrokeConfig {
    pub brush: hokusai::Brush,
    pub state0: BrushState,
    pub shape: (f32, f32, u8),
    pub view_zoom: f32,
    pub pre_stroke_grid: TileGrid,
    pub tiles_wide: i32,
    pub tiles_high: i32,
    pub mask: Option<Selection>,
}

/// Result produced by speculative replay when completed.
pub struct SpeculativeResult {
    pub shadow_grid: TileGrid,
    pub stats: DabStats,
    pub cpu_time_us: u64,
}

enum WorkerCommand {
    None,
    Start(Box<StrokeConfig>),
    End {
        final_log: Vec<ShapeSample>,
        final_total: f32,
    },
    Cancel,
    Shutdown,
}

struct WorkerState {
    command: WorkerCommand,
    result: Option<SpeculativeResult>,
    idle: bool,
}

/// A dedicated asynchronous background worker running below normal priority
/// that speculatively replays the stable prefix of a stroke into a private shadow grid.
pub struct SpeculativeWorker {
    ring: Arc<SampleRingBuffer>,
    state: Arc<(Mutex<WorkerState>, Condvar)>,
    worker_join: Mutex<Option<JoinHandle<()>>>,
    worker_thread: Thread,
    active: AtomicBool,
}

impl Default for SpeculativeWorker {
    fn default() -> Self {
        Self::new(MAX_LOGGED_SAMPLES)
    }
}

impl SpeculativeWorker {
    pub fn new(cap: usize) -> Self {
        let ring = Arc::new(SampleRingBuffer::new(cap));
        let state = Arc::new((
            Mutex::new(WorkerState {
                command: WorkerCommand::None,
                result: None,
                idle: true,
            }),
            Condvar::new(),
        ));

        let ring_clone = ring.clone();
        let state_clone = state.clone();

        let (init_tx, init_rx) = std::sync::mpsc::channel();

        let handle = std::thread::Builder::new()
            .name("arty-replay".into())
            .spawn(move || {
                lower_thread_priority();
                init_tx.send(std::thread::current()).unwrap();
                worker_loop(ring_clone, state_clone);
            })
            .expect("spawn replay worker thread");

        let current_thread = init_rx.recv().expect("receive worker thread handle");

        Self {
            ring,
            state,
            worker_join: Mutex::new(Some(handle)),
            worker_thread: current_thread,
            active: AtomicBool::new(false),
        }
    }

    /// Signal worker that a new stroke has begun.
    pub fn start_stroke(&self, config: StrokeConfig) {
        self.ring.clear();
        self.active.store(true, Ordering::Release);

        let (lock, cvar) = &*self.state;
        let mut s = lock.lock().unwrap();
        s.command = WorkerCommand::Start(Box::new(config));
        s.result = None;
        s.idle = false;
        cvar.notify_one();
        self.worker_thread.unpark();
    }

    /// Push one input sample to the worker. Allocation-free and lock-free!
    #[inline]
    pub fn push_sample(&self, sample: ShapeSample) -> bool {
        if !self.active.load(Ordering::Acquire) {
            return false;
        }
        let ok = self.ring.push(sample);
        self.worker_thread.unpark();
        ok
    }

    /// Finish the stroke and retrieve the replayed shadow grid.
    /// Waits up to timeout_ms for worker to finish remainder dabs.
    pub fn finish_stroke(&self, final_log: Vec<ShapeSample>, final_total: f32, timeout_ms: u64) -> Option<SpeculativeResult> {
        self.active.store(false, Ordering::Release);

        let (lock, cvar) = &*self.state;
        {
            let mut s = lock.lock().unwrap();
            s.command = WorkerCommand::End {
                final_log,
                final_total,
            };
            cvar.notify_one();
        }
        self.worker_thread.unpark();

        let mut s = lock.lock().unwrap();
        let timeout = Duration::from_millis(timeout_ms);
        let start = Instant::now();
        while s.result.is_none() && !s.idle {
            let elapsed = start.elapsed();
            if elapsed >= timeout {
                break;
            }
            let remaining = timeout - elapsed;
            let (new_s, timed_out) = cvar.wait_timeout(s, remaining).unwrap();
            s = new_s;
            if timed_out.timed_out() {
                break;
            }
        }
        s.result.take()
    }

    /// Cancel the stroke, discarding the worker's work.
    pub fn cancel(&self) {
        self.active.store(false, Ordering::Release);
        self.ring.clear();

        let (lock, cvar) = &*self.state;
        let mut s = lock.lock().unwrap();
        s.command = WorkerCommand::Cancel;
        s.result = None;
        cvar.notify_one();
        self.worker_thread.unpark();
    }
}

impl Drop for SpeculativeWorker {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
        {
            let (lock, cvar) = &*self.state;
            let mut s = lock.lock().unwrap();
            s.command = WorkerCommand::Shutdown;
            cvar.notify_one();
        }
        self.worker_thread.unpark();
        if let Ok(mut guard) = self.worker_join.lock()
            && let Some(th) = guard.take()
        {
            let _ = th.join();
        }
    }
}

fn worker_loop(ring: Arc<SampleRingBuffer>, state: Arc<(Mutex<WorkerState>, Condvar)>) {
    let mut discard = new_tile_box();
    let mut worker_log = Vec::with_capacity(MAX_LOGGED_SAMPLES);
    let mut worker_out = Vec::with_capacity(MAX_LOGGED_SAMPLES);
    let mut worker_scratch = Vec::with_capacity(MAX_LOGGED_SAMPLES);
    let mut shadow_grid = TileGrid::new();
    let mut stats = DabStats::default();

    let mut config: Option<StrokeConfig> = None;
    let mut worker_state = BrushState::default();
    let mut worker_arc = 0.0f32;
    let mut replayed_stations = 0usize;
    let mut replayed_samples = 0usize;
    let mut cpu_time_ns = 0u64;

    let (lock, cvar) = &*state;

    loop {
        // 1. Process command or wait
        let mut cmd = WorkerCommand::None;
        {
            let mut s = lock.lock().unwrap();
            if let WorkerCommand::None = s.command {
                if config.is_none() {
                    s.idle = true;
                    cvar.notify_all();
                    drop(s);
                    std::thread::park();
                    continue;
                }
            } else {
                cmd = std::mem::replace(&mut s.command, WorkerCommand::None);
            }
        }

        match cmd {
            WorkerCommand::Shutdown => break,
            WorkerCommand::Cancel => {
                config = None;
                worker_log.clear();
                worker_out.clear();
                worker_scratch.clear();
                shadow_grid = TileGrid::new();
                stats = DabStats::default();
                let mut s = lock.lock().unwrap();
                s.idle = true;
                cvar.notify_all();
                continue;
            }
            WorkerCommand::Start(cfg) => {
                worker_log.clear();
                worker_out.clear();
                worker_scratch.clear();
                shadow_grid = TileGrid::new();
                stats = DabStats::default();
                worker_arc = 0.0;
                replayed_stations = 0;
                replayed_samples = 0;
                cpu_time_ns = 0;
                worker_state = cfg.state0.clone();
                config = Some(*cfg);
            }
            WorkerCommand::End { final_log, final_total } => {
                if let Some(cfg) = config.take() {
                    // The prefix was rendered from the samples this thread popped. If one was
                    // dropped (full ring) or belongs to another stroke (a start racing the
                    // ring clear), the result would differ: let the engine replay instead.
                    if final_log.get(..worker_log.len()) != Some(&worker_log[..]) {
                        let mut s = lock.lock().unwrap();
                        s.result = None;
                        s.idle = true;
                        cvar.notify_all();
                        continue;
                    }
                    let t0 = Instant::now();
                    let (tin, tout, corr) = cfg.shape;
                    let sigma = if corr > 0 { correction_sigma_px(corr, cfg.view_zoom) } else { 0.0 };

                    let mut surface = ShadowSurface {
                        shadow_grid: &mut shadow_grid,
                        pre_stroke_grid: &cfg.pre_stroke_grid,
                        discard: &mut discard,
                        tiles_wide: cfg.tiles_wide,
                        tiles_high: cfg.tiles_high,
                        mask: cfg.mask.as_ref(),
                        mask_cur: MaskCur::default(),
                        stats: &mut stats,
                    };

                    if corr == 0 {
                        // Replay remainder samples with final exit taper:
                        let mut arc = 0.0f32;
                        for (i, s) in final_log.iter().enumerate() {
                            if i > 0 {
                                arc += seg_len(&final_log[i - 1], s);
                            }
                            if i >= replayed_samples {
                                let k = taper(arc, Some(final_total), tin, tout);
                                cfg.brush.stroke_to(
                                    &mut worker_state,
                                    &mut surface,
                                    s.x,
                                    s.y,
                                    s.pressure * k,
                                    s.tilt_x,
                                    s.tilt_y,
                                    s.dt.clamp(0.0005, 1.0),
                                );
                            }
                        }
                    } else {
                        // Resample & smooth full path:
                        correct_path(&final_log, sigma, &mut worker_out, &mut worker_scratch);
                        let total: f32 = worker_out.windows(2).fold(0.0, |t, w| t + seg_len(&w[0], &w[1]));
                        let mut arc = 0.0f32;
                        for (j, s) in worker_out.iter().enumerate() {
                            if j > 0 {
                                arc += seg_len(&worker_out[j - 1], s);
                            }
                            if j >= replayed_stations {
                                let k = taper(arc, Some(total), tin, tout);
                                cfg.brush.stroke_to(
                                    &mut worker_state,
                                    &mut surface,
                                    s.x,
                                    s.y,
                                    s.pressure * k,
                                    s.tilt_x,
                                    s.tilt_y,
                                    s.dt.clamp(0.0005, 1.0),
                                );
                            }
                        }
                    }
                    cfg.brush.finish_stroke(&mut worker_state, &mut surface);
                    cpu_time_ns += t0.elapsed().as_nanos() as u64;

                    let mut s = lock.lock().unwrap();
                    s.result = Some(SpeculativeResult {
                        shadow_grid: std::mem::take(&mut shadow_grid),
                        stats,
                        cpu_time_us: cpu_time_ns / 1000,
                    });
                    s.idle = true;
                    cvar.notify_all();
                }
                continue;
            }
            WorkerCommand::None => {}
        }

        // 2. Active stroke: drain samples from ring and replay stable prefix
        let Some(cfg) = config.as_ref() else {
            continue;
        };

        let mut got_samples = false;
        while let Some(sample) = ring.pop() {
            got_samples = true;
            if let Some(prev) = worker_log.last() {
                worker_arc += seg_len(prev, &sample);
            }
            worker_log.push(sample);
        }

        if !got_samples && replayed_stations > 0 {
            // Nothing new to process; sleep until unparked or timeout
            std::thread::park_timeout(Duration::from_millis(2));
            continue;
        }

        let t_start = Instant::now();
        let (tin, tout, corr) = cfg.shape;
        let sigma = if corr > 0 { correction_sigma_px(corr, cfg.view_zoom) } else { 0.0 };
        let unstable = unstable_reach(tout, sigma);
        let stable_arc = (worker_arc - unstable).max(0.0);

        let mut surface = ShadowSurface {
            shadow_grid: &mut shadow_grid,
            pre_stroke_grid: &cfg.pre_stroke_grid,
            discard: &mut discard,
            tiles_wide: cfg.tiles_wide,
            tiles_high: cfg.tiles_high,
            mask: cfg.mask.as_ref(),
            mask_cur: MaskCur::default(),
            stats: &mut stats,
        };

        if corr == 0 {
            // Replay stable raw samples (exit taper is 1.0 before stable_arc):
            let mut s_arc = 0.0f32;
            for i in 0..worker_log.len() {
                if i > 0 {
                    s_arc += seg_len(&worker_log[i - 1], &worker_log[i]);
                }
                if s_arc > stable_arc {
                    break;
                }
                if i >= replayed_samples {
                    let s = &worker_log[i];
                    let k = taper(s_arc, None, tin, tout);
                    cfg.brush.stroke_to(
                        &mut worker_state,
                        &mut surface,
                        s.x,
                        s.y,
                        s.pressure * k,
                        s.tilt_x,
                        s.tilt_y,
                        s.dt.clamp(0.0005, 1.0),
                    );
                    replayed_samples = i + 1;
                }
            }
        } else {
            // Post correction: stable stations whose support does not reach the live end
            let h = station_step(sigma);
            let taps = correction_taps(sigma, h);
            let corr_reach = taps as f32 * h;
            let unstable = tout.max(corr_reach) + 2.0 * h;
            let stable_arc = (worker_arc - unstable).max(0.0);
            let num_stations = (worker_arc / h).floor() as usize;
            let max_safe = num_stations.saturating_sub(taps + 2);
            let stable_limit = ((stable_arc / h).floor() as usize).min(max_safe);

            if stable_limit >= replayed_stations && worker_log.len() >= 3 {
                correct_path(&worker_log, sigma, &mut worker_out, &mut worker_scratch);
                let mut arc = 0.0f32;
                for (j, s) in worker_out.iter().enumerate() {
                    if j > 0 {
                        arc += seg_len(&worker_out[j - 1], s);
                    }
                    if j > stable_limit {
                        break;
                    }
                    if j >= replayed_stations {
                        let k = taper(arc, None, tin, tout);
                        cfg.brush.stroke_to(
                            &mut worker_state,
                            &mut surface,
                            s.x,
                            s.y,
                            s.pressure * k,
                            s.tilt_x,
                            s.tilt_y,
                            s.dt.clamp(0.0005, 1.0),
                        );
                    }
                }
                replayed_stations = stable_limit + 1;
            }
        }
        cpu_time_ns += t_start.elapsed().as_nanos() as u64;
    }
}
