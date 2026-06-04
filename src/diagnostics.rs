//! Diagnostics: Performance HUD and Tablet Diagnostics data structures.
//!
//! The `PerformanceHud` tracks frame time, GPU cache usage, and brush dab rate.
//! The `TabletDiagnostics` records raw tablet input values plus a rolling pressure history.

const FRAME_HISTORY_LEN: usize = 60;
const PRESSURE_HISTORY_LEN: usize = 100;
const PACKET_RATE_WINDOW_SEC: f32 = 1.0;

/// Monotonic seconds elapsed since the program started, for diagnostics timestamps.
pub fn now_secs() -> f32 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    let start = START.get_or_init(Instant::now);
    start.elapsed().as_secs_f32()
}

/// Rolling performance metrics for the canvas surface.
#[derive(Debug, Clone)]
pub struct PerformanceHud {
    pub enabled: bool,
    pub frame_times: Vec<f32>,
    pub dirty_uploads_this_frame: u32,
    pub last_dab_count: u64,
    pub last_dab_sample_time: f32,
    pub dab_rate: f32,
}

impl Default for PerformanceHud {
    fn default() -> Self {
        Self {
            enabled: false,
            frame_times: Vec::with_capacity(FRAME_HISTORY_LEN),
            dirty_uploads_this_frame: 0,
            last_dab_count: 0,
            last_dab_sample_time: 0.0,
            dab_rate: 0.0,
        }
    }
}

impl PerformanceHud {
    /// Record a new frame's delta time (seconds).
    pub fn record_frame(&mut self, dt: f32) {
        if self.frame_times.len() >= FRAME_HISTORY_LEN {
            self.frame_times.remove(0);
        }
        self.frame_times.push(dt);
    }

    /// Average frame time across the rolling buffer.
    pub fn avg_frame_time(&self) -> f32 {
        if self.frame_times.is_empty() {
            return 0.0;
        }
        let sum: f32 = self.frame_times.iter().sum();
        sum / self.frame_times.len() as f32
    }

    /// Current frames per second based on the most recent frame time.
    pub fn current_fps(&self) -> f32 {
        match self.frame_times.last() {
            Some(&dt) if dt > 0.0 => 1.0 / dt,
            _ => 0.0,
        }
    }

    /// Reset the per-frame upload counter (called at the start of each frame).
    pub fn begin_frame(&mut self) {
        self.dirty_uploads_this_frame = 0;
    }

    /// Increment the per-frame upload counter.
    pub fn note_upload(&mut self) {
        self.dirty_uploads_this_frame = self.dirty_uploads_this_frame.saturating_add(1);
    }

    /// Increment the total stroke-point counter and update the rate.
    pub fn note_stroke_point(&mut self, current_time: f32) {
        self.last_dab_count = self.last_dab_count.saturating_add(1);
        let dt = current_time - self.last_dab_sample_time;
        if dt > 0.0 && dt < 5.0 {
            // 1 unit in dt seconds -> rate
            self.dab_rate = 1.0 / dt;
        }
        self.last_dab_sample_time = current_time;
    }
}

/// Identifies the input device that produced the latest tablet event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeviceType {
    #[default]
    None,
    #[allow(dead_code)]
    Mouse,
    Pen,
    #[allow(dead_code)]
    Touch,
}

/// Snapshot of the most recent tablet state plus a rolling pressure buffer.
#[derive(Debug, Clone)]
pub struct TabletDiagnostics {
    pub enabled: bool,
    pub device_type: DeviceType,
    pub raw_x: f32,
    pub raw_y: f32,
    pub pressure: f32,
    pub tilt_x_deg: f32,
    pub tilt_y_deg: f32,
    pub tip_down: bool,
    pub in_proximity: bool,
    pub pressure_history: Vec<f32>,
    pub last_packet_time: f32,
    pub packets_in_window: u32,
    pub packet_rate: f32,
}

impl Default for TabletDiagnostics {
    fn default() -> Self {
        Self {
            enabled: false,
            device_type: DeviceType::None,
            raw_x: 0.0,
            raw_y: 0.0,
            pressure: 0.0,
            tilt_x_deg: 0.0,
            tilt_y_deg: 0.0,
            tip_down: false,
            in_proximity: false,
            pressure_history: Vec::with_capacity(PRESSURE_HISTORY_LEN),
            last_packet_time: 0.0,
            packets_in_window: 0,
            packet_rate: 0.0,
        }
    }
}

impl TabletDiagnostics {
    /// Push a new pressure sample into the rolling history (keeps at most `PRESSURE_HISTORY_LEN`).
    pub fn record_pressure(&mut self, pressure: f32) {
        if self.pressure_history.len() >= PRESSURE_HISTORY_LEN {
            self.pressure_history.remove(0);
        }
        self.pressure_history.push(pressure.clamp(0.0, 1.0));
    }

    /// Update device state and packet-rate statistics from a new event.
    #[allow(clippy::too_many_arguments)]
    pub fn record_event(
        &mut self,
        device: DeviceType,
        x: f32,
        y: f32,
        pressure: f32,
        tilt_x_rad: Option<f32>,
        tilt_y_rad: Option<f32>,
        tip_down: bool,
        in_proximity: bool,
        current_time: f32,
    ) {
        self.device_type = device;
        self.raw_x = x;
        self.raw_y = y;
        self.pressure = pressure.clamp(0.0, 1.0);
        self.tip_down = tip_down;
        self.in_proximity = in_proximity;
        if let Some(tx) = tilt_x_rad {
            self.tilt_x_deg = tx.to_degrees();
        }
        if let Some(ty) = tilt_y_rad {
            self.tilt_y_deg = ty.to_degrees();
        }

        // Update packet rate using a rolling 1-second window
        if current_time - self.last_packet_time < PACKET_RATE_WINDOW_SEC {
            self.packets_in_window = self.packets_in_window.saturating_add(1);
        } else {
            self.packet_rate = self.packets_in_window as f32;
            self.packets_in_window = 1;
            self.last_packet_time = current_time;
        }

        self.record_pressure(pressure);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frame_time_buffer_capped() {
        let mut hud = PerformanceHud::default();
        for _ in 0..200 {
            hud.record_frame(0.016);
            assert!(hud.frame_times.len() <= FRAME_HISTORY_LEN);
        }
        assert_eq!(hud.frame_times.len(), FRAME_HISTORY_LEN);
    }

    #[test]
    fn test_avg_frame_time() {
        let mut hud = PerformanceHud::default();
        hud.record_frame(0.010);
        hud.record_frame(0.020);
        hud.record_frame(0.030);
        let avg = hud.avg_frame_time();
        assert!((avg - 0.020).abs() < 1e-6, "avg was {}", avg);
    }

    #[test]
    fn test_current_fps() {
        let mut hud = PerformanceHud::default();
        hud.record_frame(0.005);
        let fps = hud.current_fps();
        assert!(fps > 195.0 && fps < 205.0, "fps was {}", fps);
    }

    #[test]
    fn test_pressure_history_capacity_ceiling() {
        let mut diag = TabletDiagnostics::default();
        for i in 0..250 {
            diag.record_pressure(i as f32 / 250.0);
        }
        assert_eq!(diag.pressure_history.len(), PRESSURE_HISTORY_LEN);
    }

    #[test]
    fn test_pressure_clamped_to_unit() {
        let mut diag = TabletDiagnostics::default();
        diag.record_pressure(2.5);
        assert_eq!(diag.pressure_history.last(), Some(&1.0));
        diag.record_pressure(-0.5);
        assert_eq!(diag.pressure_history.last(), Some(&0.0));
    }

    #[test]
    fn test_record_event_updates_state() {
        let mut diag = TabletDiagnostics::default();
        diag.record_event(
            DeviceType::Pen,
            100.0,
            200.0,
            0.75,
            Some(0.1),
            Some(-0.2),
            true,
            true,
            1.0,
        );
        assert_eq!(diag.device_type, DeviceType::Pen);
        assert_eq!(diag.raw_x, 100.0);
        assert_eq!(diag.raw_y, 200.0);
        assert!((diag.pressure - 0.75).abs() < 1e-6);
        assert!(diag.tip_down);
        assert!(diag.in_proximity);
        assert!(diag.tilt_x_deg > 5.0 && diag.tilt_x_deg < 6.5);
        assert!(diag.tilt_y_deg < -11.0 && diag.tilt_y_deg > -12.0);
        assert_eq!(diag.pressure_history.len(), 1);
    }

    #[test]
    fn test_dab_rate_calculation() {
        let mut hud = PerformanceHud::default();
        // First dab at time 0
        hud.note_stroke_point(0.0);
        // Second dab 0.001s later -> rate = 1000
        hud.note_stroke_point(0.001);
        assert!(hud.dab_rate > 500.0, "rate was {}", hud.dab_rate);
    }
}
