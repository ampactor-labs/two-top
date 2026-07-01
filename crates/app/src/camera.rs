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
use render::{
    CosmeticRng, LastKillPos, SHAKE_DECAY_PER_SEC, SHAKE_MAX_OFFSET, ScreenShake, shake_offset,
};
use sim::{
    ARENA_HALF_HEIGHT_CM, ARENA_HALF_WIDTH_CM, Boomerang, BoomerangState, MatchState, Player,
    PositionF, TICK_HZ, VelocityF,
};

/// Marker for the camera that should track the players. The mobile build
/// adds it (a zoomed follow cam for a phone); the desktop build omits it
/// and frames the whole arena statically, so `camera_follow` no-ops there
/// with no platform `cfg` in this module.
#[derive(Component)]
pub struct FollowCam;

/// Damping rate in 1/sec. Higher = camera snaps to target faster.
/// 4.0 means ~250 ms time constant — responsive but not jarring.
pub const CAMERA_FOLLOW_RATE: f32 = 4.0;

/// How much a live (Flying) boomerang pulls the follow centroid relative to a
/// player (weight 1.0). The camera leans toward the in-flight threat so the
/// thrown fang stays framed — the Boomerang-Fu "track the action" feel.
pub const BOOMERANG_CAMERA_WEIGHT: f32 = 0.5;

/// Seconds of player-velocity lookahead. The camera leads the duelists' motion
/// so what they're moving toward is already on screen. Kept short so a dash
/// burst doesn't fling the frame.
pub const CAMERA_LOOKAHEAD_SEC: f32 = 0.15;

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

// ===========================================================================
// Phase 18 Task 5.2 — the camera rig: base → kill-cam → shake.
//
// One system writes the camera Transform/Projection each frame, composed
// from three layers so they never fight over the translation:
//   1. base   — the follow target (mobile: damped player centroid; desktop:
//               static origin framing the whole arena).
//   2. kill-cam — on a round/match-ending kill, ease the camera onto the
//               kill position and zoom in ×KILL_CAM_ZOOM, hold through the
//               RoundOver/MatchOver beat, then ease back during the next
//               Countdown.
//   3. shake   — the cosmetic trauma offset (Task 5.1) added on top.
// Because `compose_camera` recomputes from `base` every frame, the shake
// and zoom can never accumulate drift into the base position.
// ===========================================================================

/// Zoom factor at the peak of the kill-cam beat (orthographic scale shrinks
/// to `1/KILL_CAM_ZOOM`, so the world appears 1.6× larger).
pub const KILL_CAM_ZOOM: f32 = 1.6;
/// Render frames to ease the kill-cam fully in (and fully back out). The
/// plan's 20-frame beat — counted in render frames, so it's ~0.33 s at 60 Hz
/// and proportionally snappier on a high-refresh display.
pub const KILL_CAM_EASE_FRAMES: f32 = 20.0;

/// The follow/static base position the camera tracks before kill-cam + shake.
/// Mobile damps it toward the player centroid; desktop leaves it at the
/// origin (whole-arena framing).
#[derive(Resource, Default)]
pub struct CameraBase(pub Vec2);

/// Kill-cam beat state. `blend` is the raw 0..1 ease progress (0 = base
/// framing, 1 = fully punched in on `target`); `zooming_in` is the ramp
/// direction (true while holding on the kill, false while easing back).
#[derive(Resource, Default)]
pub struct KillCam {
    pub blend: f32,
    pub zooming_in: bool,
    pub target: Vec2,
}

/// Smoothstep ease (pure, testable): `3t² − 2t³`, clamped. Gives the kill-cam
/// a soft start/stop instead of a linear ramp.
pub fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Orthographic scale at a given eased blend: 1.0 (neutral) → `1/KILL_CAM_ZOOM`
/// (zoomed in). Pure for testing.
pub fn kill_cam_scale(eased: f32) -> f32 {
    1.0 + (1.0 / KILL_CAM_ZOOM - 1.0) * eased
}

pub struct CameraFollowPlugin;

impl Plugin for CameraFollowPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraBase>()
            .init_resource::<KillCam>()
            .add_systems(
                Update,
                ((update_camera_base, update_kill_cam), compose_camera).chain(),
            );
    }
}

/// Layer 1: update the follow/static base. Mobile (a `FollowCam` exists) damps
/// toward a weighted centroid of the duelists PLUS any in-flight boomerangs, led
/// by player velocity (lookahead) and clamped to the playfield — so the camera
/// tracks the action without ever wandering off the arena. Desktop has no
/// `FollowCam`, so the base stays at the origin for static whole-arena framing.
///
/// Pure render-side: reads sim `PositionF`/`VelocityF`/`Boomerang`, writes only
/// the non-rollback `CameraBase`. Never feeds back into sim.
fn update_camera_base(
    time: Res<Time<Real>>,
    players: Query<(&PositionF, &VelocityF), With<Player>>,
    boomerangs: Query<(&PositionF, &Boomerang)>,
    follow: Query<(), With<FollowCam>>,
    persp: Res<render::PerspectiveFlip>,
    mut base: ResMut<CameraBase>,
) {
    if follow.is_empty() {
        return; // desktop: static origin framing
    }
    let mut sum = Vec2::ZERO;
    let mut vel_sum = Vec2::ZERO;
    let mut count = 0u32;
    for (p, v) in &players {
        let (x, y) = p.0.to_f32();
        sum += Vec2::new(x, y);
        let (vx, vy) = v.0.to_f32();
        vel_sum += Vec2::new(vx, vy);
        count += 1;
    }
    if count == 0 {
        return;
    }
    // Lean the centroid toward live (Flying) boomerangs so the thrown fang —
    // the thing both players are tracking — stays in frame.
    let mut weighted = sum;
    let mut weight = count as f32;
    for (p, b) in &boomerangs {
        if matches!(b.state, BoomerangState::Flying) {
            let (x, y) = p.0.to_f32();
            weighted += Vec2::new(x, y) * BOOMERANG_CAMERA_WEIGHT;
            weight += BOOMERANG_CAMERA_WEIGHT;
        }
    }
    // Velocity is cm/tick; ×TICK_HZ → cm/sec; ×lookahead seconds → a cm lead.
    let lead = (vel_sum / count as f32) * (TICK_HZ as f32 * CAMERA_LOOKAHEAD_SEC);
    let mut target = weighted / weight + lead;
    // The world renders Y-foreshortened into the tabletop tilt, so the camera —
    // which lives in that rendered space — must foreshorten its target Y (and
    // its playfield clamp) to stay centred on the action.
    target.y *= render::WORLD_TILT_Y * persp.0;
    // Soft clamp: never center past the playfield edge.
    let half_w = ARENA_HALF_WIDTH_CM as f32;
    let half_h = ARENA_HALF_HEIGHT_CM as f32 * render::WORLD_TILT_Y;
    target.x = target.x.clamp(-half_w, half_w);
    target.y = target.y.clamp(-half_h, half_h);

    let dt = time.delta_secs();
    base.0.x = damped_step(base.0.x, target.x, CAMERA_FOLLOW_RATE, dt);
    base.0.y = damped_step(base.0.y, target.y, CAMERA_FOLLOW_RATE, dt);
}

/// Layer 2: advance the kill-cam beat from `MatchState` transitions. A round
/// ends on the clock and the match ends on the threshold-crossing kill (sim's
/// model), so the beat triggers on entering `RoundOver` *or* `MatchOver` and
/// punches in on the most recent kill (`LastKillPos`). It eases back when the
/// next `Countdown` begins; `MatchOver` is terminal, so it simply holds (the
/// match-summary overlay lands in Task 5.5).
fn update_kill_cam(
    state: Res<MatchState>,
    last_kill: Res<LastKillPos>,
    mut kc: ResMut<KillCam>,
    mut prev: Local<Option<MatchState>>,
) {
    let now = *state;
    let in_beat =
        |s: &MatchState| matches!(s, MatchState::RoundOver { .. } | MatchState::MatchOver);
    let in_countdown = |s: &MatchState| matches!(s, MatchState::Countdown { .. });

    let entering_beat = in_beat(&now) && !prev.map(|p| in_beat(&p)).unwrap_or(false);
    let entering_countdown = in_countdown(&now) && !prev.map(|p| in_countdown(&p)).unwrap_or(false);

    if entering_beat {
        kc.target = last_kill.0;
        kc.zooming_in = true;
    } else if entering_countdown {
        kc.zooming_in = false;
    }

    let step = 1.0 / KILL_CAM_EASE_FRAMES;
    kc.blend = if kc.zooming_in {
        (kc.blend + step).min(1.0)
    } else {
        (kc.blend - step).max(0.0)
    };

    *prev = Some(now);
}

/// The single camera writer: composes base + kill-cam ease + shake into the
/// camera `Transform` and orthographic scale. Decays trauma and samples the
/// shake offset here (the cosmetic RNG never feeds `SimRng`).
fn compose_camera(
    time: Res<Time<Real>>,
    base: Res<CameraBase>,
    kc: Res<KillCam>,
    mut shake: ResMut<ScreenShake>,
    mut rng: ResMut<CosmeticRng>,
    mut camera: Query<(&mut Transform, &mut Projection), With<Camera2d>>,
) {
    shake.trauma = decay_trauma(shake.trauma, time.delta_secs());
    let angle = rng.0.gen_range(0.0..std::f32::consts::TAU);
    let offset = shake_offset(shake.trauma, SHAKE_MAX_OFFSET, angle);

    let eased = smoothstep(kc.blend);
    let pos = base.0.lerp(kc.target, eased);
    let scale = kill_cam_scale(eased);

    let Ok((mut xform, mut projection)) = camera.single_mut() else {
        return;
    };
    xform.translation.x = pos.x + offset.x;
    xform.translation.y = pos.y + offset.y;
    if let Projection::Orthographic(ortho) = projection.as_mut() {
        ortho.scale = scale;
    }
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

    #[test]
    fn smoothstep_endpoints_and_midpoint() {
        assert!((smoothstep(0.0) - 0.0).abs() < 1e-6);
        assert!((smoothstep(1.0) - 1.0).abs() < 1e-6);
        assert!((smoothstep(0.5) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn smoothstep_clamps_out_of_range() {
        assert_eq!(smoothstep(-1.0), 0.0);
        assert_eq!(smoothstep(2.0), 1.0);
    }

    #[test]
    fn smoothstep_is_flat_at_ends() {
        // Soft start: small input produces even smaller output (derivative 0).
        assert!(smoothstep(0.1) < 0.1);
        // Soft stop: near 1.0 the output is pulled above the linear line.
        assert!(smoothstep(0.9) > 0.9);
    }

    #[test]
    fn kill_cam_scale_neutral_at_zero_zoomed_at_one() {
        assert!((kill_cam_scale(0.0) - 1.0).abs() < 1e-6);
        assert!((kill_cam_scale(1.0) - 1.0 / KILL_CAM_ZOOM).abs() < 1e-6);
        // Smaller scale == zoomed in (world appears larger).
        assert!(kill_cam_scale(1.0) < kill_cam_scale(0.0));
    }
}
