use serde::{Deserialize, Serialize};

/// A customizable pressure response curve for stylus input.
///
/// The curve is defined by a series of control points in the [0, 1] x [0, 1] plane.
/// `calibrate()` interpolates between the sorted points to map raw pressure to
/// a calibrated output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PressureCurve {
    pub points: Vec<(f32, f32)>,
}

impl Default for PressureCurve {
    fn default() -> Self {
        Self::new_linear()
    }
}

impl PressureCurve {
    pub fn new_linear() -> Self {
        Self {
            points: vec![(0.0, 0.0), (0.5, 0.5), (1.0, 1.0)],
        }
    }

    pub fn new_steep() -> Self {
        Self {
            points: vec![(0.0, 0.0), (0.2, 0.7), (1.0, 1.0)],
        }
    }

    pub fn new_ease_in() -> Self {
        Self {
            points: vec![(0.0, 0.0), (0.8, 0.3), (1.0, 1.0)],
        }
    }

    pub fn calibrate(&self, raw_pressure: f32) -> f32 {
        let p = raw_pressure.clamp(0.0, 1.0);

        let mut sorted = self.points.clone();
        sorted.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        if sorted.is_empty() {
            return p;
        }
        if p <= sorted[0].0 {
            return sorted[0].1;
        }
        if p >= sorted.last().unwrap().0 {
            return sorted.last().unwrap().1;
        }

        for window in sorted.windows(2) {
            let (x0, y0) = window[0];
            let (x1, y1) = window[1];
            if p >= x0 && p <= x1 {
                let t = if (x1 - x0).abs() < f32::EPSILON {
                    0.0
                } else {
                    (p - x0) / (x1 - x0)
                };
                return y0 + (y1 - y0) * t;
            }
        }

        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_linear_curve_endpoints() {
        let curve = PressureCurve::new_linear();
        assert!((curve.calibrate(0.0) - 0.0).abs() < 1e-6);
        assert!((curve.calibrate(1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_linear_curve_midpoint() {
        let curve = PressureCurve::new_linear();
        let mid = curve.calibrate(0.5);
        assert!((mid - 0.5).abs() < 1e-6, "Expected 0.5, got {}", mid);
    }

    #[test]
    fn test_steep_curve() {
        let curve = PressureCurve::new_steep();
        let low = curve.calibrate(0.1);
        let high = curve.calibrate(0.9);
        assert!(
            low > 0.0,
            "Low pressure should map to nonzero output, got {}",
            low
        );
        assert!(high > low, "Steep curve: high pressure should exceed low");
        assert!(
            (high - 1.0).abs() < 0.2,
            "High pressure should approach 1.0"
        );
    }

    #[test]
    fn test_ease_in_curve() {
        let curve = PressureCurve::new_ease_in();
        let low = curve.calibrate(0.3);
        assert!(
            low < 0.3,
            "Ease-in: low pressure should be even lower, got {}",
            low
        );
    }

    #[test]
    fn test_clamping() {
        let curve = PressureCurve::new_linear();
        assert!((curve.calibrate(-0.5) - 0.0).abs() < 1e-6);
        assert!((curve.calibrate(1.5) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_custom_curve_interpolation() {
        let curve = PressureCurve {
            points: vec![(0.0, 0.0), (0.5, 0.8), (1.0, 1.0)],
        };
        let val = curve.calibrate(0.25);
        assert!((val - 0.4).abs() < 1e-5, "Expected 0.4, got {}", val);
    }

    #[test]
    fn test_single_point_curve() {
        let curve = PressureCurve {
            points: vec![(0.5, 0.7)],
        };
        assert!((curve.calibrate(0.3) - 0.7).abs() < 1e-6);
        assert!((curve.calibrate(0.5) - 0.7).abs() < 1e-6);
        assert!((curve.calibrate(0.8) - 0.7).abs() < 1e-6);
    }
}
