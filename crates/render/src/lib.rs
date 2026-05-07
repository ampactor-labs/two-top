//! Phase 7: simulation/render boundary.
//!
//! The sim writes `PositionF` (and friends) deterministically at 60 Hz.
//! The render layer's job is to produce a `Transform` for each rendered
//! entity at whatever rate the display refreshes — 30, 60, 120, 144 Hz
//! and everything in between. We do that by interpolating between the
//! last two simulated positions: at the start of each tick `sim` copies
//! `PositionF` into `PreviousPositionF` (`snapshot_previous`), and the
//! render system below lerps between them weighted by how far into the
//! current sim tick we are.
//!
//! The lerp is f32-based — we are over the sim/render boundary and floats
//! are explicitly allowed here. `Vec2F::to_f32` is the sanctioned bridge;
//! see CONVENTIONS § Render Layer Rules.

use bevy::prelude::*;
use fixed_math::Vec2F;
use sim::{LastSimTickTime, NoInterpolate, PositionF, PreviousPositionF, TICK_HZ};

/// Lerp between two sim positions, returning a `Vec2` ready to drop into
/// `Transform.translation`. `alpha` is clamped to `[0.0, 1.0]`.
pub fn lerp_position(prev: Vec2F, curr: Vec2F, alpha: f32) -> Vec2 {
    let alpha = alpha.clamp(0.0, 1.0);
    let (px, py) = prev.to_f32();
    let (cx, cy) = curr.to_f32();
    Vec2::new(px + (cx - px) * alpha, py + (cy - py) * alpha)
}

/// Compute the interpolation alpha for the current frame. `now` is the
/// real-time clock's elapsed seconds; `last_tick` is when the most recent
/// sim tick finished. The result is `0.0` immediately after a sim tick
/// and approaches `1.0` as the next tick draws near.
pub fn interpolation_alpha(now_secs: f32, last_tick_secs: f32, tick_hz: usize) -> f32 {
    let tick_dt = 1.0 / tick_hz as f32;
    ((now_secs - last_tick_secs) / tick_dt).clamp(0.0, 1.0)
}

/// `Update`-schedule system: writes `Transform.translation.{x,y}` for any
/// entity that has both a sim position pair and a `Transform`. Entities
/// carrying `NoInterpolate` get the raw current position (no lerp); all
/// others lerp between previous and current sim positions weighted by
/// `interpolation_alpha`.
pub fn sync_transforms_from_sim(
    time: Res<Time<Real>>,
    last_tick: Res<LastSimTickTime>,
    mut q: Query<(
        &PositionF,
        &PreviousPositionF,
        Option<&NoInterpolate>,
        &mut Transform,
    )>,
) {
    let alpha = interpolation_alpha(
        time.elapsed_secs_f64() as f32,
        last_tick.0 as f32,
        TICK_HZ,
    );
    for (pos, prev, no_interp, mut xform) in &mut q {
        let v = if no_interp.is_some() {
            let (x, y) = pos.0.to_f32();
            Vec2::new(x, y)
        } else {
            lerp_position(prev.0, pos.0, alpha)
        };
        xform.translation.x = v.x;
        xform.translation.y = v.y;
    }
}

pub struct RenderSyncPlugin;

impl Plugin for RenderSyncPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, sync_transforms_from_sim);
    }
}
