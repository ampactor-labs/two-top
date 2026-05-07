//! Unit tests for the pure interpolation helpers — no Bevy app, no sim,
//! just the math. The end-to-end `sync_transforms_from_sim` system is
//! exercised via the `app` binary's manual smoothness check.

use fixed_math::Vec2F;
use render::{interpolation_alpha, lerp_position};

#[test]
fn lerp_at_zero_returns_prev() {
    let prev = Vec2F::from_cm(10, 20);
    let curr = Vec2F::from_cm(30, 40);
    let v = lerp_position(prev, curr, 0.0);
    assert_eq!(v.x, 10.0);
    assert_eq!(v.y, 20.0);
}

#[test]
fn lerp_at_one_returns_curr() {
    let prev = Vec2F::from_cm(10, 20);
    let curr = Vec2F::from_cm(30, 40);
    let v = lerp_position(prev, curr, 1.0);
    assert_eq!(v.x, 30.0);
    assert_eq!(v.y, 40.0);
}

#[test]
fn lerp_at_half_returns_midpoint() {
    let prev = Vec2F::from_cm(10, 20);
    let curr = Vec2F::from_cm(30, 40);
    let v = lerp_position(prev, curr, 0.5);
    assert_eq!(v.x, 20.0);
    assert_eq!(v.y, 30.0);
}

#[test]
fn lerp_clamps_alpha_above_one() {
    let prev = Vec2F::from_cm(10, 20);
    let curr = Vec2F::from_cm(30, 40);
    let v = lerp_position(prev, curr, 5.0);
    assert_eq!(v.x, 30.0);
    assert_eq!(v.y, 40.0);
}

#[test]
fn lerp_clamps_alpha_below_zero() {
    let prev = Vec2F::from_cm(10, 20);
    let curr = Vec2F::from_cm(30, 40);
    let v = lerp_position(prev, curr, -1.0);
    assert_eq!(v.x, 10.0);
    assert_eq!(v.y, 20.0);
}

#[test]
fn alpha_is_zero_at_tick_boundary() {
    // now == last_tick → no time has elapsed since the last sim tick.
    assert_eq!(interpolation_alpha(1.0, 1.0, 60), 0.0);
}

#[test]
fn alpha_is_half_midway_through_tick() {
    // 60 Hz ticks → tick_dt ≈ 0.01666… s. Halfway = ~0.00833 s.
    let alpha = interpolation_alpha(1.008333, 1.0, 60);
    assert!((alpha - 0.5).abs() < 1e-3, "got alpha={alpha}");
}

#[test]
fn alpha_clamps_at_one_past_tick() {
    // Far past the next tick — render should hold at curr until the sim
    // catches up rather than extrapolate beyond it.
    assert_eq!(interpolation_alpha(2.0, 1.0, 60), 1.0);
}
