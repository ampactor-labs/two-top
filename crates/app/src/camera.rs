//! Phase 9 cycle 5: camera follow with exponential damping.
//!
//! Render-side, runs in `Update`. Per CONVENTIONS the camera is purely
//! visual — it never feeds back into sim, so f32 math is fine and the
//! system doesn't need to be deterministic across frame rates. We
//! apply frame-rate-independent damping via `1 - exp(-rate · dt)` so
//! the time-to-target feel is the same on a 30 Hz phone and a 144 Hz
//! desktop.

use bevy::prelude::*;
use rand::Rng as _;
use render::{CosmeticRng, SHAKE_DECAY_PER_SEC, SHAKE_MAX_OFFSET, ScreenShake, shake_offset};
use sim::{Player, PositionF};

/// Marker for the camera that should track the players. The mobile build
/// adds it (a zoomed follow cam for a phone); the desktop build omits it
/// and frames the whole arena statically, so `camera_follow` no-ops there
/// with no platform `cfg` in this module.
#[derive(Component)]
pub struct FollowCam;

/// Damping rate in 1/sec. Higher = camera snaps to target faster.
/// 4.0 means ~250 ms time constant — responsive but not jarring.
pub const CAMERA_FOLLOW_RATE: f32 = 4.0;

/// Pure helper for frame-rate-independent exponential damping.
/// `(current → target)` advances toward `target` by a fraction
/// `1 - exp(-rate·dt)` of the remaining distance.
pub fn damped_step(current: f32, target: f32, rate: f32, dt: f32) -> f32 {
    let alpha = 1.0 - (-rate * dt).exp();
    current + (target - current) * alpha
}

/// Linear trauma decay (pure, for testing). Trauma bleeds off at
/// [`SHAKE_DECAY_PER_SEC`] per second, floored at zero.
pub fn decay_trauma(trauma: f32, dt: f32) -> f32 {
    (trauma - SHAKE_DECAY_PER_SEC * dt).max(0.0)
}

pub struct CameraFollowPlugin;

impl Plugin for CameraFollowPlugin {
    fn build(&self, app: &mut App) {
        // Screen shake straddles the base-positioning systems: `shake_settle`
        // removes last frame's offset (restoring the camera to its true base)
        // *before* `camera_follow` recomputes the base, then `shake_apply`
        // decays trauma, samples a fresh offset, and re-applies it *after*.
        // This keeps the kick from drifting the follow/kill-cam base. The
        // ScreenShake resource is provided by render's EffectsPlugin.
        app.add_systems(
            Update,
            (
                shake_settle.before(camera_follow),
                camera_follow,
                shake_apply.after(camera_follow),
            ),
        );
    }
}

/// Remove the previously-applied shake offset so the base-positioning
/// systems (camera_follow / the kill-cam) see the camera at its true base.
fn shake_settle(shake: Res<ScreenShake>, mut camera: Query<&mut Transform, With<Camera2d>>) {
    let Ok(mut xform) = camera.single_mut() else {
        return;
    };
    xform.translation.x -= shake.offset.x;
    xform.translation.y -= shake.offset.y;
}

/// Decay trauma, sample a fresh offset from the cosmetic RNG, and add it to
/// the camera's (freshly-positioned) base. The offset is stashed in the
/// resource so [`shake_settle`] can undo it next frame.
fn shake_apply(
    time: Res<Time<Real>>,
    mut shake: ResMut<ScreenShake>,
    mut rng: ResMut<CosmeticRng>,
    mut camera: Query<&mut Transform, With<Camera2d>>,
) {
    shake.trauma = decay_trauma(shake.trauma, time.delta_secs());
    let angle = rng.0.gen_range(0.0..std::f32::consts::TAU);
    let offset = shake_offset(shake.trauma, SHAKE_MAX_OFFSET, angle);
    shake.offset = offset;
    let Ok(mut xform) = camera.single_mut() else {
        return;
    };
    xform.translation.x += offset.x;
    xform.translation.y += offset.y;
}

fn camera_follow(
    time: Res<Time<Real>>,
    players: Query<&PositionF, With<Player>>,
    mut camera: Query<&mut Transform, With<FollowCam>>,
) {
    let mut sum = Vec2::ZERO;
    let mut count = 0u32;
    for p in &players {
        let (x, y) = p.0.to_f32();
        sum += Vec2::new(x, y);
        count += 1;
    }
    if count == 0 {
        return;
    }
    let target = sum / count as f32;
    let Ok(mut xform) = camera.single_mut() else {
        return;
    };
    let dt = time.delta_secs();
    xform.translation.x = damped_step(xform.translation.x, target.x, CAMERA_FOLLOW_RATE, dt);
    xform.translation.y = damped_step(xform.translation.y, target.y, CAMERA_FOLLOW_RATE, dt);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damped_step_zero_dt_returns_current() {
        let v = damped_step(10.0, 100.0, 4.0, 0.0);
        assert!((v - 10.0).abs() < 1e-6);
    }

    #[test]
    fn damped_step_advances_toward_target() {
        let v = damped_step(0.0, 100.0, 4.0, 1.0 / 60.0);
        // alpha = 1 - exp(-4/60) ≈ 0.0645; v ≈ 6.45
        assert!(v > 0.0 && v < 100.0);
        assert!((v - 6.45).abs() < 0.5);
    }

    #[test]
    fn damped_step_does_not_overshoot() {
        // Even a huge dt shouldn't push past the target.
        let v = damped_step(0.0, 100.0, 4.0, 100.0);
        assert!(v <= 100.0 + 1e-3);
        assert!((v - 100.0).abs() < 1e-3);
    }

    #[test]
    fn damped_step_is_symmetric_through_target() {
        // Approaching from above and below should converge at the
        // same rate (alpha is independent of direction).
        let from_below = damped_step(0.0, 100.0, 4.0, 0.1);
        let from_above = damped_step(200.0, 100.0, 4.0, 0.1);
        let dist_below = (from_below - 0.0).abs();
        let dist_above = (200.0 - from_above).abs();
        assert!((dist_below - dist_above).abs() < 1e-4);
    }

    #[test]
    fn damped_step_at_target_stays_at_target() {
        let v = damped_step(50.0, 50.0, 4.0, 0.1);
        assert!((v - 50.0).abs() < 1e-6);
    }

    #[test]
    fn decay_trauma_bleeds_off_over_time() {
        // A 0.1 s frame removes 0.1 × SHAKE_DECAY_PER_SEC of trauma.
        let t = decay_trauma(1.0, 0.1);
        assert!((t - (1.0 - 0.1 * SHAKE_DECAY_PER_SEC)).abs() < 1e-6);
    }

    #[test]
    fn decay_trauma_floors_at_zero() {
        // A long frame can't drive trauma negative.
        assert_eq!(decay_trauma(0.1, 10.0), 0.0);
    }

    #[test]
    fn decay_trauma_zero_dt_is_identity() {
        assert!((decay_trauma(0.42, 0.0) - 0.42).abs() < 1e-6);
    }
}
