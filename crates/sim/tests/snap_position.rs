//! Phase 7: `snap_position` helper for teleports.
//!
//! Render-side `sync_transforms_from_sim` lerps between `PreviousPositionF`
//! and `PositionF`. On a teleport (respawn, stage transition, etc.) we
//! don't want a visible smear from the old location to the new one, so we
//! force `prev = new = current` and the lerp emits the new position
//! immediately at any alpha.

use fixed_math::Vec2F;
use sim::{snap_position, PositionF, PreviousPositionF};

#[test]
fn snap_position_collapses_prev_and_current_to_new() {
    let mut pos = PositionF(Vec2F::from_cm(10, 20));
    let mut prev = PreviousPositionF(Vec2F::from_cm(7, 9));
    let target = Vec2F::from_cm(500, -200);

    snap_position(&mut pos, &mut prev, target);

    assert_eq!(pos.0, target);
    assert_eq!(prev.0, target);
}

#[test]
fn snap_position_is_idempotent() {
    let mut pos = PositionF(Vec2F::from_cm(10, 20));
    let mut prev = PreviousPositionF(Vec2F::from_cm(10, 20));
    let target = Vec2F::from_cm(10, 20);

    snap_position(&mut pos, &mut prev, target);
    snap_position(&mut pos, &mut prev, target);

    assert_eq!(pos.0, target);
    assert_eq!(prev.0, target);
}
