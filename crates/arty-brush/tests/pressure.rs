use arty_brush::pressure::{MIN_GAP, PressureCurve};

#[global_allocator]
static ALLOC: arty_testkit::CountingAllocator = arty_testkit::CountingAllocator;

#[test]
fn eval_is_allocation_free() {
    let curves = [
        PressureCurve::linear(),
        PressureCurve::from_gamma(1.8),
        PressureCurve::from_points(&[[0.0, 0.0], [0.5, 0.0], [0.5 + MIN_GAP, 1.0], [1.0, 1.0]]),
    ];
    let mut sum = 0.0f32;
    let allocs = arty_testkit::count_allocs(|| {
        for c in &curves {
            for i in 0..=10_000 {
                sum += c.eval(i as f32 / 10_000.0);
            }
            sum += c.eval(f32::NAN) + c.eval(-1.0) + c.eval(2.0);
        }
    });
    assert_eq!(allocs, 0, "PressureCurve::eval allocated");
    assert!(sum.is_finite() && sum > 0.0);
}
