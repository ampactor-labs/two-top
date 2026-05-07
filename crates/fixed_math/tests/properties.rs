use fixed_math::{atan2, cos, sin, sin_cos, sqrt, Fix, FixWide, Vec2F, HALF_PI, PI, TWO_PI};
use proptest::prelude::*;

fn bounded_fix() -> impl Strategy<Value = Fix> {
    // Bound storage to ±10000 << 16 so add/sub stay in Fix's integer range
    // (I16F16 integer range is ±32767).
    let max_bits: i32 = 10_000 * 65_536;
    (-max_bits..=max_bits).prop_map(Fix::from_bits)
}

fn bounded_vec() -> impl Strategy<Value = Vec2F> {
    (bounded_fix(), bounded_fix()).prop_map(|(x, y)| Vec2F::new(x, y))
}

proptest! {
    #[test]
    fn add_sub_roundtrip(a in bounded_vec(), b in bounded_vec()) {
        prop_assert_eq!((a + b) - b, a);
        prop_assert_eq!((a - b) + b, a);
    }

    #[test]
    fn sqrt_of_square_roundtrip(bits in 0_i32..=(100 * 65_536)) {
        let x = Fix::from_bits(bits);
        let xx = x * x;
        let r = sqrt(xx);
        let tolerance = Fix::from_bits(100); // ~1.5e-3
        let diff = (r - x).abs();
        prop_assert!(diff <= tolerance, "sqrt({}*{}) = {}, expected {}, diff {}", x, x, r, x, diff);
    }
}

#[test]
fn length_sq_of_3_4_is_25() {
    let v = Vec2F::from_cm(3, 4);
    assert_eq!(v.length_sq(), Fix::const_from_int(25));
}

#[test]
fn length_sq_of_zero_is_zero() {
    assert_eq!(Vec2F::ZERO.length_sq(), Fix::ZERO);
}

#[test]
fn length_sq_saturates_for_large_vectors() {
    // 1000 cm * 1000 cm = 1_000_000, far above Fix::MAX (~32767).
    // length_sq saturates; length_sq_wide does not.
    let v = Vec2F::from_cm(1000, 0);
    assert_eq!(v.length_sq(), Fix::MAX);
    let wide_expected: fixed_math::FixWide = fixed_math::FixWide::const_from_int(1_000_000);
    assert_eq!(v.length_sq_wide(), wide_expected);
}

#[test]
fn length_sq_wide_arena_diagonal() {
    // Arena-scale ranking case: ~3000 cm vector.
    let v = Vec2F::from_cm(3000, 0);
    let expected = fixed_math::FixWide::const_from_int(9_000_000);
    assert_eq!(v.length_sq_wide(), expected);
}

#[test]
fn length_of_3_4_is_5() {
    let v = Vec2F::from_cm(3, 4);
    let len = v.length();
    let expected = Fix::const_from_int(5);
    let diff = (len - expected).abs();
    assert!(diff < Fix::from_bits(10), "len={} expected={} diff={}", len, expected, diff);
}

#[test]
fn length_of_zero_is_zero() {
    assert_eq!(Vec2F::ZERO.length(), Fix::ZERO);
}

#[test]
fn dot_self_equals_length_sq() {
    let v = Vec2F::from_cm(3, 4);
    assert_eq!(v.dot(v), v.length_sq());
}

#[test]
fn dot_perpendicular_is_zero() {
    let a = Vec2F::from_cm(1, 0);
    let b = Vec2F::from_cm(0, 1);
    assert_eq!(a.dot(b), Fix::ZERO);
}

#[test]
fn cross_of_parallel_is_zero() {
    let a = Vec2F::from_cm(2, 0);
    let b = Vec2F::from_cm(5, 0);
    assert_eq!(a.cross(b), Fix::ZERO);
}

#[test]
fn cross_right_angle_unit() {
    let a = Vec2F::from_cm(1, 0);
    let b = Vec2F::from_cm(0, 1);
    assert_eq!(a.cross(b), Fix::const_from_int(1));
}

#[test]
fn normalize_zero_stays_zero() {
    assert_eq!(Vec2F::ZERO.normalize(), Vec2F::ZERO);
}

#[test]
fn length_no_overflow_near_fix_max() {
    // Regression: cordic::sqrt overflows internally on Fix inputs near Fix::MAX
    // even when the result fits. length() must route through FixWide.
    let v = Vec2F::new(Fix::from_bits(-5292519), Fix::from_bits(-6508302));
    let len = v.length();
    let expected = Fix::const_from_int(128);
    let diff = (len - expected).abs();
    assert!(diff < Fix::from_bits(100), "len={} expected≈{} diff={}", len, expected, diff);
}

#[test]
fn pi_constants_relate() {
    // Within 1 ULP of the arithmetic relationships — Fix::lit rounds each
    // constant independently, so 2*PI may differ from TWO_PI by a single bit.
    let two_pi_diff = (TWO_PI - PI * Fix::const_from_int(2)).abs();
    assert!(two_pi_diff <= Fix::from_bits(2), "TWO_PI - 2*PI = {}", two_pi_diff);
    let half_pi_diff = (HALF_PI - PI / Fix::const_from_int(2)).abs();
    assert!(half_pi_diff <= Fix::from_bits(2), "HALF_PI - PI/2 = {}", half_pi_diff);
}

#[test]
fn pi_value_close_to_real() {
    // Q16.16 representation of π should be within one ULP of the float value.
    let f_pi: Fix = Fix::lit("3.1415926");
    let diff = (PI - f_pi).abs();
    assert!(diff < Fix::from_bits(2), "PI={} ref={} diff={}", PI, f_pi, diff);
}

#[test]
fn sin_cos_at_zero() {
    let s = sin(Fix::ZERO);
    assert!(s.abs() < Fix::from_bits(10), "sin(0)={}", s);
    let one = Fix::const_from_int(1);
    let c = cos(Fix::ZERO);
    let diff = (c - one).abs();
    assert!(diff < Fix::from_bits(10), "cos(0)={} expected 1 diff={}", c, diff);
}

#[test]
fn sin_cos_at_half_pi() {
    let one = Fix::const_from_int(1);
    let s = sin(HALF_PI);
    let c = cos(HALF_PI);
    assert!((s - one).abs() < Fix::from_bits(50), "sin(π/2)={} expected 1", s);
    assert!(c.abs() < Fix::from_bits(50), "cos(π/2)={} expected 0", c);
}

#[test]
fn sin_cos_consistent_with_separate_calls() {
    let angle = Fix::lit("0.7");
    let (s, c) = sin_cos(angle);
    assert_eq!(s, sin(angle));
    assert_eq!(c, cos(angle));
}

#[test]
fn rotate_zero_is_identity() {
    // Tolerance: cordic::sin(0) ≈ 3e-5, scaled by component magnitude (~4).
    let v = Vec2F::from_cm(3, 4);
    let r = v.rotate(Fix::ZERO);
    assert!((r.x - v.x).abs() < Fix::from_bits(50), "dx={}", (r.x - v.x).abs());
    assert!((r.y - v.y).abs() < Fix::from_bits(50), "dy={}", (r.y - v.y).abs());
}

#[test]
fn rotate_unit_x_by_half_pi_is_unit_y() {
    let v = Vec2F::new(Fix::const_from_int(1), Fix::ZERO);
    let r = v.rotate(HALF_PI);
    let zero = Fix::ZERO;
    let one = Fix::const_from_int(1);
    assert!(r.x.abs() < Fix::from_bits(50), "x={}", r.x);
    assert!((r.y - one).abs() < Fix::from_bits(50), "y={}", r.y);
    let _ = zero;
}

#[test]
fn angle_of_unit_x_is_zero() {
    let v = Vec2F::new(Fix::const_from_int(1), Fix::ZERO);
    assert!(v.angle().abs() < Fix::from_bits(10), "angle={}", v.angle());
}

#[test]
fn angle_of_unit_y_is_half_pi() {
    let v = Vec2F::new(Fix::ZERO, Fix::const_from_int(1));
    let diff = (v.angle() - HALF_PI).abs();
    assert!(diff < Fix::from_bits(10), "angle={} expected π/2", v.angle());
}

/// Phase 2 cross-platform determinism gate.
///
/// 1000 rotations of (100cm, 0) by 0.01 radians, accumulating Q16.16 error.
/// The resulting bit pattern must be identical on every supported target.
/// If this assertion ever differs across linux-x64, linux-aarch64, macos
/// aarch64, or android aarch64, the simulation is non-deterministic and
/// rollback netcode will desync.
#[test]
fn determinism_locked_1000_rotations() {
    let step = Fix::lit("0.01");
    let mut v = Vec2F::from_cm(100, 0);
    for _ in 0..1000 {
        v = v.rotate(step);
    }
    // Locked bit values — captured from linux-x64 and frozen as the
    // canonical post-1000-step state. Any platform deviation is a bug.
    assert_eq!(v.x.to_bits(), 0xffad8d3c_u32 as i32, "x bits diverged: {:#x}", v.x.to_bits() as u32);
    assert_eq!(v.y.to_bits(), 0xffc95ba0_u32 as i32, "y bits diverged: {:#x}", v.y.to_bits() as u32);
}

#[test]
#[allow(clippy::disallowed_types)]
fn to_f32_round_trips_integer_components() {
    let v = Vec2F::from_cm(3, 4);
    let (x, y) = v.to_f32();
    assert!((x - 3.0_f32).abs() < 1e-4);
    assert!((y - 4.0_f32).abs() < 1e-4);
}

#[test]
fn atan2_axes() {
    let zero = Fix::ZERO;
    let one = Fix::const_from_int(1);
    let half_pi_diff = (atan2(one, zero) - HALF_PI).abs();
    assert!(half_pi_diff < Fix::from_bits(10), "atan2(1,0)={} expected π/2", atan2(one, zero));
    assert_eq!(atan2(zero, one), Fix::ZERO);
}

proptest! {
    #[test]
    fn sin_squared_plus_cos_squared_is_one(bits in -(8 * 65_536_i32)..=(8 * 65_536)) {
        // Domain ±8 radians (well past one full revolution either way),
        // checking that angle normalization makes the identity hold.
        let angle = Fix::from_bits(bits);
        let (s, c) = sin_cos(angle);
        let sum = s.wide_mul(s) + c.wide_mul(c);
        let one = FixWide::from_num(1);
        let diff = (FixWide::from_num(sum) - one).abs();
        prop_assert!(diff < FixWide::from_bits(1 << 24), "angle={} sin²+cos²={} diff={}", angle, sum, diff);
    }

    #[test]
    fn rotate_by_two_pi_is_identity(
        bits_x in -100i32 * 65_536..=100 * 65_536,
        bits_y in -100i32 * 65_536..=100 * 65_536,
    ) {
        // Q16.16 cordic trig has ~5e-5 absolute precision; multiplied by
        // input magnitude up to 100 gives a worst-case error around 5e-3
        // per component. Tolerance set above that to keep the property
        // robust without becoming meaningless.
        let v = Vec2F::new(Fix::from_bits(bits_x), Fix::from_bits(bits_y));
        let r = v.rotate(TWO_PI);
        let dx = (r.x - v.x).abs();
        let dy = (r.y - v.y).abs();
        let tol = Fix::from_bits(500); // ~7.6e-3
        prop_assert!(dx <= tol && dy <= tol, "v={:?} rotated={:?} dx={} dy={}", v, r, dx, dy);
    }

    #[test]
    fn normalize_produces_unit_length(bits_x in -100i32 * 65_536..=100 * 65_536, bits_y in -100i32 * 65_536..=100 * 65_536) {
        let v = Vec2F::new(Fix::from_bits(bits_x), Fix::from_bits(bits_y));
        if v.length() < Fix::from_bits(1024) {
            // Skip near-zero vectors — division blows up precision
            return Ok(());
        }
        let n = v.normalize();
        let len = n.length();
        let one = Fix::const_from_int(1);
        let diff = (len - one).abs();
        prop_assert!(diff <= Fix::from_bits(200), "normalize len = {}, expected 1, diff {}", len, diff);
    }
}

#[test]
fn vec2f_zero_is_origin() {
    let z = Vec2F::ZERO;
    assert_eq!(z.x, Fix::ZERO);
    assert_eq!(z.y, Fix::ZERO);
}

#[test]
fn vec2f_new_constructs_from_fix() {
    let v = Vec2F::new(Fix::const_from_int(3), Fix::const_from_int(4));
    assert_eq!(v.x, Fix::const_from_int(3));
    assert_eq!(v.y, Fix::const_from_int(4));
}

#[test]
fn vec2f_from_cm_centimeter_units() {
    let v = Vec2F::from_cm(100, 200);
    assert_eq!(v.x, Fix::const_from_int(100));
    assert_eq!(v.y, Fix::const_from_int(200));
}
