//! The devouring — the match ends with the victor eating the fallen's fang.
//!
//! Pure render-side ceremony, sequenced with the kill-cam: on `MatchOver`
//! the kill-cam punches in on the final kill; after a short hold, a
//! cosmetic bone fang rises from the kill spot, arcs into the victor, and
//! vanishes in a bite-flash + shake kick. Cur and Stag are demons — the
//! win is consumption, not a trophy screen.
//!
//! No sim state, no rollback entities: the fang here is a throwaway sprite
//! keyed off `MatchState`/`MatchScore`/`LastKillPos`, torn down the moment
//! the match leaves `MatchOver` (rematch or back-to-lobby).

use bevy::prelude::*;
use render::{EffectAssets, LastKillPos, ScreenShake};
use sim::{MATCH_WIN_THRESHOLD, MatchScore, MatchState, Player};

/// Seconds the fang holds at the kill spot before flying (lets the kill-cam
/// finish its punch-in so the whole ceremony is on-camera).
const DEVOUR_HOLD: f32 = 0.7;
/// Seconds of flight from the kill spot into the victor.
const DEVOUR_FLIGHT: f32 = 0.9;
/// Spin during flight (rad/sec) — the fang tumbles home one last time.
const DEVOUR_SPIN: f32 = 9.0;
/// Shake kick when the jaws close.
const TRAUMA_DEVOUR: f32 = 0.45;

/// The ceremonial fang. `t` runs from `-DEVOUR_HOLD` (waiting) through
/// `0..DEVOUR_FLIGHT` (flying); the bite fires at the end.
#[derive(Component)]
struct DevourFang {
    t: f32,
    from: Vec2,
    winner: usize,
}

pub struct DevourPlugin;

impl Plugin for DevourPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, devouring_ceremony);
    }
}

#[allow(clippy::too_many_arguments)]
fn devouring_ceremony(
    mut commands: Commands,
    time: Res<Time<Real>>,
    state: Res<MatchState>,
    score: Res<MatchScore>,
    last_kill: Res<LastKillPos>,
    asset_server: Res<AssetServer>,
    assets: Option<Res<EffectAssets>>,
    mut shake: ResMut<ScreenShake>,
    players: Query<(&Player, &Transform), Without<DevourFang>>,
    mut fang: Query<(Entity, &mut DevourFang, &mut Transform, &mut Sprite), Without<Player>>,
    mut prev_over: Local<bool>,
) {
    let over = matches!(*state, MatchState::MatchOver);
    let entered = over && !*prev_over;
    *prev_over = over;

    if entered && fang.is_empty() {
        let winner = if score.p0 >= MATCH_WIN_THRESHOLD { 0 } else { 1 };
        commands.spawn((
            DevourFang {
                t: -DEVOUR_HOLD,
                from: last_kill.0,
                winner,
            },
            Sprite {
                image: asset_server.load("sprites/projectiles/bone_fang.png"),
                custom_size: Some(Vec2::splat(52.0)),
                ..default()
            },
            // Above the stage, below HUD/kill-flash — the ceremony is the shot.
            Transform::from_xyz(last_kill.0.x, last_kill.0.y, 60.0),
        ));
    }

    let dt = time.delta_secs();
    for (entity, mut df, mut tx, mut sprite) in &mut fang {
        // Rematch/lobby exit mid-ceremony: the fang vanishes with the beat.
        if !over {
            commands.entity(entity).despawn();
            continue;
        }
        df.t += dt;
        if df.t < 0.0 {
            continue; // holding at the kill spot while the kill-cam lands
        }
        let Some(target) = players
            .iter()
            .find(|(p, _)| p.handle == df.winner)
            .map(|(_, t)| t.translation.truncate())
        else {
            continue;
        };
        let k = (df.t / DEVOUR_FLIGHT).clamp(0.0, 1.0);
        let ease = k * k * (3.0 - 2.0 * k);
        let pos = df.from.lerp(target, ease);
        tx.translation.x = pos.x;
        tx.translation.y = pos.y;
        tx.rotate_z(DEVOUR_SPIN * dt);
        // The fang shrinks as the jaws take it.
        sprite.custom_size = Some(Vec2::splat(52.0 * (1.0 - 0.85 * ease)));
        if k >= 1.0 {
            if let Some(assets) = assets.as_ref() {
                render::spawn_effect(
                    &mut commands,
                    assets.hit_burst.0.clone(),
                    assets.hit_burst.1.clone(),
                    4,
                    0.05,
                    target,
                    56.0,
                    60.0,
                );
            }
            shake.add_trauma(TRAUMA_DEVOUR);
            commands.entity(entity).despawn();
        }
    }
}
