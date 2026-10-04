use octotablet::axis::Pose;
use octotablet::events::{Event, TabletEvent, ToolEvent};
use octotablet::Builder;
use octotablet::Manager;

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct StylusEvent {
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
    pub tilt_x: f32,
    pub tilt_y: f32,
}

/// Holds the latest native tablet axis data extracted from octotablet event pumping.
#[derive(Debug, Clone, Copy, Default)]
pub struct TabletAxisState {
    /// Normalized pressure [0.0, 1.0]. 0.0 = no contact, 1.0 = full pressure.
    pub pressure: f32,
    /// Absolute tilt from perpendicular in radians (X axis). None if the hardware doesn't report it.
    pub tilt_x: Option<f32>,
    /// Absolute tilt from perpendicular in radians (Y axis). None if the hardware doesn't report it.
    pub tilt_y: Option<f32>,
    /// Whether the pen tip is currently in contact / pressed down.
    pub tip_down: bool,
    /// Whether the pen is in proximity of the tablet surface.
    pub in_proximity: bool,
}

/// Manages the connection to the Windows Ink RealTimeStylus API via octotablet.
///
/// This struct owns the octotablet `Manager` and provides a clean interface for
/// pumping tablet events and extracting the latest axis data (pressure, tilt, etc.).
pub struct InputManager {
    manager: Manager,
    axis_state: TabletAxisState,
}

impl InputManager {
    /// Build an `InputManager` from an eframe `CreationContext`.
    ///
    /// # Safety
    /// The `CreationContext` contains window handles that must outlive the returned `InputManager`.
    /// Since the manager is stored within `PaintApp` which has the same lifetime as the window,
    /// this is safe.
    pub unsafe fn new(
        context: &eframe::CreationContext<'_>,
    ) -> Result<Self, octotablet::builder::BuildError> {
        let builder = Builder::new()
            // Keep mouse input on egui's normal pointer path. Emulating it through
            // Windows Ink can stall the native message loop on some drivers.
            .emulate_tool_from_mouse(false);

        let manager = unsafe { builder.build_raw(context) }?;

        let backend_name = match manager.backed() {
            octotablet::Backend::WindowsInkRealTimeStylus => "Windows Ink RealTimeStylus",
            _ => "Unknown backend",
        };
        log::info!("[InputManager] Connected: {}", backend_name);

        // Log detected tablets
        for tablet in manager.tablets() {
            log::info!(
                "[InputManager] Tablet detected: {} (USB VID:{:04X} PID:{:04X})",
                tablet.name.as_deref().unwrap_or("Unknown"),
                tablet.usb_id.map(|u| u.vid).unwrap_or(0),
                tablet.usb_id.map(|u| u.pid).unwrap_or(0),
            );
        }

        Ok(Self {
            manager,
            axis_state: TabletAxisState::default(),
        })
    }

    /// Pump pending tablet events and update the internal axis state.
    /// This should be called once per frame in the update loop.
    ///
    /// Returns a reference to the updated axis state so callers can read the latest pressure/tilt.
    ///
    /// NOTE: We process events into a local accumulator first to avoid borrow conflicts
    /// with the iterator (which holds a reference to the inner Manager).
    pub fn pump(&mut self) -> (TabletAxisState, bool) {
        // Start from the previous state. Tablet backends often report only the
        // axes that changed in this frame, so resetting here loses pressure.
        let mut local = self.axis_state;
        let mut has_events = false;

        let Ok(events) = self.manager.pump();
        for event in events {
            has_events = true;
            match event {
                Event::Tool { tool: _, event } => match event {
                    ToolEvent::Down => {
                        local.tip_down = true;
                    }
                    ToolEvent::Up => {
                        local.tip_down = false;
                        local.pressure = 0.0;
                    }
                    ToolEvent::In { tablet: _ } => {
                        local.in_proximity = true;
                    }
                    ToolEvent::Out => {
                        local.in_proximity = false;
                        local.tip_down = false;
                        local.pressure = 0.0;
                    }
                    ToolEvent::Pose(pose) => {
                        extract_pose_data(&pose, &mut local);
                    }
                    _ => {}
                },
                Event::Tablet { tablet: _, event } => match event {
                    TabletEvent::Added => {
                        log::info!("[InputManager] Tablet added");
                    }
                    TabletEvent::Removed => {
                        log::info!("[InputManager] Tablet removed");
                    }
                },
                Event::Pad { pad: _, event: _ } => {
                    // Pad events (buttons, rings, strips) are not currently used for drawing.
                }
            }
        }

        // Copy local state into self after events iterator is consumed
        self.axis_state = local;

        (self.axis_state, has_events)
    }

    /// Get a reference to the current tablet axis state (read-only).
    #[allow(dead_code)]
    pub fn axis_state(&self) -> &TabletAxisState {
        &self.axis_state
    }

    /// Check if a real pen tablet (not emulated mouse) is connected.
    #[allow(dead_code)]
    pub fn has_tablet(&self) -> bool {
        !self.manager.tablets().is_empty()
    }
}

/// Free function to extract pose data into TabletAxisState.
/// This avoids borrow conflicts with the events iterator.
#[inline(always)]
fn extract_pose_data(pose: &Pose, state: &mut TabletAxisState) {
    // Pressure: NicheF32 is a niche-optimized Option<f32> where NaN = None.
    // We use .get() to extract Option<f32> and default to 0.0.
    let raw_pressure: f32 = pose.pressure.get().unwrap_or(0.0);
    state.pressure = raw_pressure.clamp(0.0, 1.0);

    // Tilt: reported as absolute radians from perpendicular in X and Y directions.
    // octotablet reports tilt as Option<[f32; 2]> where:
    //   [0] = X tilt (positive = rightward)
    //   [1] = Y tilt (positive = toward user)
    if let Some([tx, ty]) = pose.tilt {
        state.tilt_x = Some(tx);
        state.tilt_y = Some(ty);
    }
}

// =========================================================================
// Dual-Stage Stroke Stabilizer (original, unchanged)
// =========================================================================

// =========================================================================
// Advanced Physics-Based & Circular Stroke Stabilizer (Zero-Allocation)
// =========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StabilizerMode {
    Ema,
    SpringMassDamper,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StabilizerLevel {
    Off,
    Level(usize),  // 1..=15
    SLevel(usize), // 1..=5
}

impl Default for StabilizerLevel {
    fn default() -> Self {
        Self::Level(5)
    }
}

pub struct StrokeStabilizer {
    pub mode: StabilizerMode,
    pub level: StabilizerLevel,

    // Circular ring buffers (zero allocation, capacity 128)
    pos_x_buf: [f32; 128],
    pos_y_buf: [f32; 128],
    pos_start: usize,
    pos_len: usize,

    pressure_buf: [f32; 128],
    pressure_start: usize,
    pressure_len: usize,

    tilt_x_buf: [f32; 128],
    tilt_x_start: usize,
    tilt_x_len: usize,

    tilt_y_buf: [f32; 128],
    tilt_y_start: usize,
    tilt_y_len: usize,

    // EMA state
    last_smoothed_x: Option<f32>,
    last_smoothed_y: Option<f32>,
    pub last_smoothed_pressure: Option<f32>,
    pub last_smoothed_tilt_x: Option<f32>,
    pub last_smoothed_tilt_y: Option<f32>,

    // Spring-Mass-Damper physics state
    tip_x: f32,
    tip_y: f32,
    vel_x: f32,
    vel_y: f32,

    pub is_drawing: bool,
}

#[inline(always)]
fn push_ring(data: &mut [f32; 128], start: &mut usize, len: &mut usize, val: f32) {
    let cap = 128;
    if *len < cap {
        let idx = (*start + *len) % cap;
        data[idx] = val;
        *len += 1;
    } else {
        data[*start] = val;
        *start = (*start + 1) % cap;
    }
}

#[inline(always)]
fn avg_ring(data: &[f32; 128], start: usize, len: usize) -> f32 {
    if len == 0 {
        return 0.0;
    }
    let mut sum = 0.0;
    for i in 0..len {
        sum += data[(start + i) % 128];
    }
    sum / len as f32
}

impl StrokeStabilizer {
    pub fn new(level_val: usize) -> Self {
        Self {
            mode: StabilizerMode::SpringMassDamper,
            level: StabilizerLevel::Level(level_val.clamp(1, 15)),
            pos_x_buf: [0.0; 128],
            pos_y_buf: [0.0; 128],
            pos_start: 0,
            pos_len: 0,
            pressure_buf: [0.0; 128],
            pressure_start: 0,
            pressure_len: 0,
            tilt_x_buf: [0.0; 128],
            tilt_x_start: 0,
            tilt_x_len: 0,
            tilt_y_buf: [0.0; 128],
            tilt_y_start: 0,
            tilt_y_len: 0,
            last_smoothed_x: None,
            last_smoothed_y: None,
            last_smoothed_pressure: None,
            last_smoothed_tilt_x: None,
            last_smoothed_tilt_y: None,
            tip_x: 0.0,
            tip_y: 0.0,
            vel_x: 0.0,
            vel_y: 0.0,
            is_drawing: false,
        }
    }

    pub fn reset(&mut self) {
        self.pos_start = 0;
        self.pos_len = 0;
        self.pressure_start = 0;
        self.pressure_len = 0;
        self.tilt_x_start = 0;
        self.tilt_x_len = 0;
        self.tilt_y_start = 0;
        self.tilt_y_len = 0;
        self.last_smoothed_x = None;
        self.last_smoothed_y = None;
        self.last_smoothed_pressure = None;
        self.last_smoothed_tilt_x = None;
        self.last_smoothed_tilt_y = None;
        self.tip_x = 0.0;
        self.tip_y = 0.0;
        self.vel_x = 0.0;
        self.vel_y = 0.0;
        self.is_drawing = false;
    }

    pub fn set_level(&mut self, level: StabilizerLevel) {
        self.level = level;
    }

    /// Process a new raw pointer event through the physical/EMA stabilizer.
    /// Incorporates moving average filtering to remove jitter, followed by either
    /// Exponential Moving Average or Spring-Mass-Damper physics integration.
    pub fn process(
        &mut self,
        raw_x: f32,
        raw_y: f32,
        raw_pressure: f32,
        raw_tilt_x: f32,
        raw_tilt_y: f32,
        dt: f32,
    ) -> (f32, f32, f32, f32, f32) {
        if matches!(self.level, StabilizerLevel::Off) {
            self.last_smoothed_x = Some(raw_x);
            self.last_smoothed_y = Some(raw_y);
            self.last_smoothed_pressure = Some(raw_pressure);
            self.last_smoothed_tilt_x = Some(raw_tilt_x);
            self.last_smoothed_tilt_y = Some(raw_tilt_y);
            return (raw_x, raw_y, raw_pressure, raw_tilt_x, raw_tilt_y);
        }

        let window_size = match self.level {
            StabilizerLevel::Off => 1,
            StabilizerLevel::Level(val) => val.clamp(1, 15),
            StabilizerLevel::SLevel(val) => (val * 12).clamp(1, 128),
        };

        // 1. Queue raw coordinates into circular ring buffers
        let cap = 128;
        if self.pos_len < cap {
            let idx = (self.pos_start + self.pos_len) % cap;
            self.pos_x_buf[idx] = raw_x;
            self.pos_y_buf[idx] = raw_y;
            self.pos_len += 1;
        } else {
            let idx = self.pos_start;
            self.pos_x_buf[idx] = raw_x;
            self.pos_y_buf[idx] = raw_y;
            self.pos_start = (self.pos_start + 1) % cap;
        }

        // Adjust pos length to match current window size
        while self.pos_len > window_size {
            self.pos_start = (self.pos_start + 1) % 128;
            self.pos_len -= 1;
        }

        // Calculate moving average
        let avg_x = avg_ring(&self.pos_x_buf, self.pos_start, self.pos_len);
        let avg_y = avg_ring(&self.pos_y_buf, self.pos_start, self.pos_len);

        // 2. Queue pressure and tilt values
        let p_window = (window_size / 2).max(1);
        push_ring(
            &mut self.pressure_buf,
            &mut self.pressure_start,
            &mut self.pressure_len,
            raw_pressure,
        );
        while self.pressure_len > p_window {
            self.pressure_start = (self.pressure_start + 1) % 128;
            self.pressure_len -= 1;
        }
        let avg_p = avg_ring(&self.pressure_buf, self.pressure_start, self.pressure_len);

        push_ring(
            &mut self.tilt_x_buf,
            &mut self.tilt_x_start,
            &mut self.tilt_x_len,
            raw_tilt_x,
        );
        while self.tilt_x_len > p_window {
            self.tilt_x_start = (self.tilt_x_start + 1) % 128;
            self.tilt_x_len -= 1;
        }
        let avg_tx = avg_ring(&self.tilt_x_buf, self.tilt_x_start, self.tilt_x_len);

        push_ring(
            &mut self.tilt_y_buf,
            &mut self.tilt_y_start,
            &mut self.tilt_y_len,
            raw_tilt_y,
        );
        while self.tilt_y_len > p_window {
            self.tilt_y_start = (self.tilt_y_start + 1) % 128;
            self.tilt_y_len -= 1;
        }
        let avg_ty = avg_ring(&self.tilt_y_buf, self.tilt_y_start, self.tilt_y_len);

        // 3. Smooth pressure and tilt with standard EMA
        let pressure_alpha = 1.0 / (window_size as f32 * 0.2 + 1.0);
        let smoothed_p = match self.last_smoothed_pressure {
            Some(prev_p) => pressure_alpha * avg_p + (1.0 - pressure_alpha) * prev_p,
            None => avg_p,
        };
        self.last_smoothed_pressure = Some(smoothed_p);

        let smoothed_tx = match self.last_smoothed_tilt_x {
            Some(prev_tx) => pressure_alpha * avg_tx + (1.0 - pressure_alpha) * prev_tx,
            None => avg_tx,
        };
        self.last_smoothed_tilt_x = Some(smoothed_tx);

        let smoothed_ty = match self.last_smoothed_tilt_y {
            Some(prev_ty) => pressure_alpha * avg_ty + (1.0 - pressure_alpha) * prev_ty,
            None => avg_ty,
        };
        self.last_smoothed_tilt_y = Some(smoothed_ty);

        // 4. Smooth position using selected stabilizer mode
        let (smoothed_x, smoothed_y) = match self.mode {
            StabilizerMode::Ema => {
                let position_alpha = 1.0 / (window_size as f32 * 0.4 + 1.0);
                let (sx, sy) = match self.last_smoothed_x {
                    Some(prev_x) => {
                        let prev_y = self.last_smoothed_y.unwrap_or(avg_y);
                        let sx = position_alpha * avg_x + (1.0 - position_alpha) * prev_x;
                        let sy = position_alpha * avg_y + (1.0 - position_alpha) * prev_y;
                        (sx, sy)
                    }
                    None => (avg_x, avg_y),
                };
                self.last_smoothed_x = Some(sx);
                self.last_smoothed_y = Some(sy);
                (sx, sy)
            }
            StabilizerMode::SpringMassDamper => {
                if self.last_smoothed_x.is_none() {
                    self.tip_x = avg_x;
                    self.tip_y = avg_y;
                    self.vel_x = 0.0;
                    self.vel_y = 0.0;
                    self.last_smoothed_x = Some(avg_x);
                    self.last_smoothed_y = Some(avg_y);
                }

                // Map level to Spring-Mass-Damper parameters
                let (k, c, mass) = match self.level {
                    StabilizerLevel::Off => (1000.0, 0.0, 1.0),
                    StabilizerLevel::Level(val) => {
                        let k = 300.0 / (val as f32).powf(1.1);
                        let c = 12.0 + (val as f32) * 0.6;
                        let mass = 1.0 + (val as f32) * 0.08;
                        (k, c, mass)
                    }
                    StabilizerLevel::SLevel(val) => {
                        let k = 15.0 / (val as f32);
                        let c = 20.0 + (val as f32) * 3.0;
                        let mass = 2.5 + (val as f32) * 0.8;
                        (k, c, mass)
                    }
                };

                // Run sub-stepping Euler integration for numerical stability
                let dt_clamped = dt.clamp(0.001, 0.1);
                let sub_steps = 16;
                let sub_dt = dt_clamped / sub_steps as f32;

                for _ in 0..sub_steps {
                    let f_spring_x = k * (avg_x - self.tip_x);
                    let f_spring_y = k * (avg_y - self.tip_y);

                    let f_damping_x = -c * self.vel_x;
                    let f_damping_y = -c * self.vel_y;

                    let accel_x = (f_spring_x + f_damping_x) / mass;
                    let accel_y = (f_spring_y + f_damping_y) / mass;

                    self.vel_x += accel_x * sub_dt;
                    self.vel_y += accel_y * sub_dt;

                    self.tip_x += self.vel_x * sub_dt;
                    self.tip_y += self.vel_y * sub_dt;
                }

                self.last_smoothed_x = Some(self.tip_x);
                self.last_smoothed_y = Some(self.tip_y);
                (self.tip_x, self.tip_y)
            }
        };

        (smoothed_x, smoothed_y, smoothed_p, smoothed_tx, smoothed_ty)
    }

    #[allow(dead_code)]
    pub fn has_pending_points(&self) -> bool {
        self.is_drawing && self.last_smoothed_x.is_some()
    }
}
