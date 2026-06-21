//! Phase 18 Task 5.3 — synthesized-cue playback.
//!
//! Twelve one-shot SFX plus a looping ambient bed, fired off the same
//! sim-event edges the render crate's effect sprites detect (a player dying, a
//! boomerang thrown/caught/ricocheting, a pyre shattering, pickups, the match
//! clock). Everything here is cosmetic — `AudioPlayer` entities are never
//! rolled back, and the cues read *current* sim state through the same `Local`
//! prev-state edge pattern the effect spawners use (never `Added`/`Changed`,
//! which rollback re-simulation makes unreliable).
//!
//! **Why this lives in `app`, not `render`:** audio needs an output *device*,
//! exactly as windowing needs a display — so `bevy_audio`/`cpal` belong with
//! the windowing crate, not the determinism-core `render` crate that the
//! cross-platform matrix builds on every target. Keeping `cpal` out of
//! `render` keeps that matrix's build lean (audio is orthogonal to the
//! bit-identical-sim guarantee).
//!
//! The cues themselves are generated deterministically by
//! `scripts/generate_audio.py` into `assets/audio/*.wav`. This module only
//! decides *when* to play them. Volume routing through a `Settings` resource
//! lands in Task 5.5; for now the levels are the named constants below.

use bevy::audio::Volume;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use sim::{
    AnimState, BonePyre, Boomerang, BoomerangMods, BoomerangState, Dead, Empowered, HeldModifier,
    MatchState, Pickup, PickupKind, Player, VelocityF,
};

/// SFX bus level. The WAVs are already peak-normalized to −3 dBFS; this leaves
/// headroom for several stacking at once. Task 5.5 routes it through settings.
pub const SFX_VOLUME: f32 = 0.7;
/// Ambient-bed level. The loop asset is already a −18 dBFS bed; this keeps it
/// politely under the gameplay cues.
pub const MUSIC_VOLUME: f32 = 0.6;
/// GO-toll pitch: the countdown toll replayed up a major third (×1.25 speed)
/// so the "go" reads distinct from the three descending pre-beats.
pub const GO_TOLL_SPEED: f32 = 1.25;

/// A flying fang fires the ricochet cue when its heading turns harder than this
/// (dot product of consecutive unit velocities below cos 45°). Wall/pyre
/// bounces reverse a velocity component → well below the threshold; the Curve
/// modifier (≈1.5°/tick) and Bouncy (speed-only) stay above it, so they don't
/// false-trigger.
const RICOCHET_DOT_THRESHOLD: f32 = 0.707;

/// Pure predicate (testable): did a fang's heading turn hard enough this frame
/// to count as a bounce? `prev_dir`/`cur_dir` are unit velocity vectors. A wall
/// or pyre ricochet flips a component (dot ≤ 0); gentle Curve steering and
/// Bouncy speed-only changes keep the dot near 1. Split out so the threshold is
/// unit-tested without standing up a `World`.
fn is_ricochet_turn(prev_dir: Vec2, cur_dir: Vec2) -> bool {
    prev_dir.dot(cur_dir) < RICOCHET_DOT_THRESHOLD
}

/// Pre-loaded handles for every cue, so the per-event systems never hit the
/// asset server on the hot path.
#[derive(Resource, Clone)]
pub struct AudioAssets {
    pub throw: Handle<AudioSource>,
    pub throw_empowered: Handle<AudioSource>,
    pub ricochet: Handle<AudioSource>,
    pub shatter: Handle<AudioSource>,
    pub catch: Handle<AudioSource>,
    pub catch_perfect: Handle<AudioSource>,
    pub kill: Handle<AudioSource>,
    pub countdown_toll: Handle<AudioSource>,
    pub round_over_sting: Handle<AudioSource>,
    pub pickup_spawn: Handle<AudioSource>,
    pub pickup_collect: Handle<AudioSource>,
    pub ambient_loop: Handle<AudioSource>,
}

/// Spawn a one-shot cue at `volume`, set to despawn its entity when finished
/// (so cues never leak entities).
fn play(commands: &mut Commands, clip: &Handle<AudioSource>, volume: f32) {
    play_pitched(commands, clip, volume, 1.0);
}

/// One-shot cue with a playback-speed multiplier (used for the GO toll).
fn play_pitched(commands: &mut Commands, clip: &Handle<AudioSource>, volume: f32, speed: f32) {
    commands.spawn((
        AudioPlayer::new(clip.clone()),
        PlaybackSettings::DESPAWN
            .with_volume(Volume::Linear(volume))
            .with_speed(speed),
    ));
}

/// Startup: load every cue handle, insert [`AudioAssets`], and start the
/// looping ambient bed immediately.
fn load_audio_and_start_ambient(mut commands: Commands, asset_server: Res<AssetServer>) {
    let assets = AudioAssets {
        throw: asset_server.load("audio/throw.wav"),
        throw_empowered: asset_server.load("audio/throw_empowered.wav"),
        ricochet: asset_server.load("audio/ricochet.wav"),
        shatter: asset_server.load("audio/shatter.wav"),
        catch: asset_server.load("audio/catch.wav"),
        catch_perfect: asset_server.load("audio/catch_perfect.wav"),
        kill: asset_server.load("audio/kill.wav"),
        countdown_toll: asset_server.load("audio/countdown_toll.wav"),
        round_over_sting: asset_server.load("audio/round_over_sting.wav"),
        pickup_spawn: asset_server.load("audio/pickup_spawn.wav"),
        pickup_collect: asset_server.load("audio/pickup_collect.wav"),
        ambient_loop: asset_server.load("audio/ambient_loop.wav"),
    };
    commands.spawn((
        AudioPlayer::new(assets.ambient_loop.clone()),
        PlaybackSettings::LOOP.with_volume(Volume::Linear(MUSIC_VOLUME)),
    ));
    commands.insert_resource(assets);
}

/// Per-handle throw-edge tracking: whether each player's *primary* fang was in
/// flight last frame, and that player's empowered flag the frame before the
/// throw consumed it (so the empowered throw reads its brighter variant).
#[derive(Default)]
struct ThrowTrack {
    had_primary: HashMap<usize, bool>,
    prev_empowered: HashMap<usize, bool>,
}

/// Throw cue: a primary fang appearing for an owner that had none is a throw.
/// If that owner's empowered flag was up the prior frame (a perfect-catch
/// reward, consumed by this throw), play the empowered variant.
fn play_throw_sfx(
    mut commands: Commands,
    assets: Res<AudioAssets>,
    booms: Query<(&Boomerang, &BoomerangMods)>,
    players: Query<(&Player, &Empowered)>,
    mut track: Local<ThrowTrack>,
) {
    let mut has_primary: HashMap<usize, bool> = HashMap::default();
    for (boom, mods) in &booms {
        if !mods.is_secondary {
            has_primary.insert(boom.owner_handle, true);
        }
    }
    for (player, _) in &players {
        let h = player.handle;
        let now = has_primary.get(&h).copied().unwrap_or(false);
        let was = track.had_primary.get(&h).copied().unwrap_or(false);
        if now && !was {
            let empowered = track.prev_empowered.get(&h).copied().unwrap_or(false);
            let clip = if empowered {
                &assets.throw_empowered
            } else {
                &assets.throw
            };
            play(&mut commands, clip, SFX_VOLUME);
        }
        track.had_primary.insert(h, now);
    }
    // Record the post-throw empowered state for next frame's comparison.
    for (player, emp) in &players {
        track.prev_empowered.insert(player.handle, emp.0);
    }
}

/// Per-boomerang-entity previous unit velocity, for ricochet detection.
#[derive(Default)]
struct RicochetTrack(HashMap<Entity, Vec2>);

/// Ricochet cue: a flying fang whose heading turns harder than the threshold
/// (a wall or pyre bounce) plays the hard click. Recall (Flying→Returning) and
/// the homing return are excluded by gating on `Flying`.
fn play_ricochet_sfx(
    mut commands: Commands,
    assets: Res<AudioAssets>,
    booms: Query<(Entity, &Boomerang, &VelocityF)>,
    mut track: Local<RicochetTrack>,
) {
    for (entity, boom, vel) in &booms {
        let (vx, vy) = vel.0.to_f32();
        let cur = Vec2::new(vx, vy);
        let len = cur.length();
        if len < 1e-3 {
            continue;
        }
        let dir = cur / len;
        if matches!(boom.state, BoomerangState::Flying)
            && let Some(prev_dir) = track.0.get(&entity)
            && is_ricochet_turn(*prev_dir, dir)
        {
            play(&mut commands, &assets.ricochet, SFX_VOLUME);
        }
        track.0.insert(entity, dir);
    }
    // Drop tracking for despawned fangs so the map can't grow unbounded.
    track.0.retain(|entity, _| booms.get(*entity).is_ok());
}

/// Per-player previous `AnimState.anim_id`, for catch detection.
#[derive(Default)]
struct CatchTrack(HashMap<usize, u8>);

/// Catch cue: a player's anim entering CATCH is a catch. If their empowered
/// flag is up at that moment, it was a perfect catch (the catch system raises
/// `Empowered` and sets CATCH on the same tick) → the bell variant.
fn play_catch_sfx(
    mut commands: Commands,
    assets: Res<AudioAssets>,
    players: Query<(&Player, &AnimState, &Empowered)>,
    mut track: Local<CatchTrack>,
) {
    for (player, anim, emp) in &players {
        let was = track
            .0
            .get(&player.handle)
            .copied()
            .unwrap_or(AnimState::IDLE);
        if anim.anim_id == AnimState::CATCH && was != AnimState::CATCH {
            let clip = if emp.0 {
                &assets.catch_perfect
            } else {
                &assets.catch
            };
            play(&mut commands, clip, SFX_VOLUME);
        }
        track.0.insert(player.handle, anim.anim_id);
    }
}

/// Per-player previous `is_dying`, for the kill cue.
#[derive(Default)]
struct DyingTrack(HashMap<usize, bool>);

/// Kill cue: the tick a player transitions into dying (one-hit-kill — every
/// landed contact is a death). Mirrors `spawn_hit_and_death_bursts`'s edge.
fn play_kill_sfx(
    mut commands: Commands,
    assets: Res<AudioAssets>,
    players: Query<(&Player, &Dead)>,
    mut track: Local<DyingTrack>,
) {
    for (player, dead) in &players {
        let now = dead.is_dying();
        let was = track.0.get(&player.handle).copied().unwrap_or(false);
        if now && !was {
            play(&mut commands, &assets.kill, SFX_VOLUME);
        }
        track.0.insert(player.handle, now);
    }
}

/// Per-pyre previous `shattered`, for the shatter cue.
#[derive(Default)]
struct ShatterTrack(HashMap<Entity, bool>);

/// Shatter cue: the tick a `BonePyre` breaks. Same false→true edge as
/// `shake_on_pyre_shatter` (so a startup `Changed` spawn doesn't trip it).
fn play_shatter_sfx(
    mut commands: Commands,
    assets: Res<AudioAssets>,
    pyres: Query<(Entity, &BonePyre)>,
    mut track: Local<ShatterTrack>,
) {
    for (entity, pyre) in &pyres {
        let was = track.0.get(&entity).copied().unwrap_or(false);
        if pyre.shattered && !was {
            play(&mut commands, &assets.shatter, SFX_VOLUME);
        }
        track.0.insert(entity, pyre.shattered);
    }
}

/// Pickup-spawn cue: a floor pickup appearing where there was none. At most one
/// pickup exists at a time (the spawner gates on emptiness), so a simple
/// presence edge is sufficient.
fn play_pickup_spawn_sfx(
    mut commands: Commands,
    assets: Res<AudioAssets>,
    pickups: Query<(), With<Pickup>>,
    mut present: Local<bool>,
) {
    let now = !pickups.is_empty();
    if now && !*present {
        play(&mut commands, &assets.pickup_spawn, SFX_VOLUME);
    }
    *present = now;
}

/// Per-player previously-held modifier, for the collect cue.
#[derive(Default)]
struct HeldTrack(HashMap<usize, Option<PickupKind>>);

/// Pickup-collect cue: a player's held modifier becoming `Some` (a fresh
/// pickup) or changing to a different `Some` (walking over a new one while
/// already holding). A throw clearing the slot (`Some`→`None`) is not a
/// collect.
fn play_pickup_collect_sfx(
    mut commands: Commands,
    assets: Res<AudioAssets>,
    players: Query<(&Player, &HeldModifier)>,
    mut track: Local<HeldTrack>,
) {
    for (player, held) in &players {
        let now = held.0;
        let was = track.0.get(&player.handle).copied().unwrap_or(None);
        if now.is_some() && now != was {
            play(&mut commands, &assets.pickup_collect, SFX_VOLUME);
        }
        track.0.insert(player.handle, now);
    }
}

/// Match-clock cues: the three descending countdown tolls (3/2/1), the GO toll
/// (same toll a major third up), and the round/match-over sting. Driven off
/// `MatchState` transitions tracked in a `Local`.
fn play_match_state_sfx(
    mut commands: Commands,
    assets: Res<AudioAssets>,
    state: Res<MatchState>,
    mut prev: Local<Option<MatchState>>,
) {
    let now = *state;
    let prev_state = *prev;
    *prev = Some(now);

    let is_countdown = |s: MatchState| matches!(s, MatchState::Countdown { .. });
    let is_roundover = |s: MatchState| matches!(s, MatchState::RoundOver { .. });
    let is_matchover = |s: MatchState| matches!(s, MatchState::MatchOver);

    // First observation: the match boots straight into Countdown{3}; toll it.
    let Some(prev_state) = prev_state else {
        if is_countdown(now) {
            play(&mut commands, &assets.countdown_toll, SFX_VOLUME);
        }
        return;
    };

    // Entering a fresh countdown (a new round begins) → the first toll.
    if is_countdown(now) && !is_countdown(prev_state) {
        play(&mut commands, &assets.countdown_toll, SFX_VOLUME);
        return;
    }
    // Digit ticking down inside the countdown (3→2→1) → a toll per beat.
    if let (
        MatchState::Countdown { digit: pd, .. },
        MatchState::Countdown { digit: nd, .. },
    ) = (prev_state, now)
    {
        if nd != pd {
            play(&mut commands, &assets.countdown_toll, SFX_VOLUME);
        }
        return;
    }
    // Countdown → InRound: the GO toll, pitched up.
    if is_countdown(prev_state) && matches!(now, MatchState::InRound { .. }) {
        play_pitched(&mut commands, &assets.countdown_toll, SFX_VOLUME, GO_TOLL_SPEED);
        return;
    }
    // Entering RoundOver or MatchOver → the descending sting.
    if (is_roundover(now) && !is_roundover(prev_state))
        || (is_matchover(now) && !is_matchover(prev_state))
    {
        play(&mut commands, &assets.round_over_sting, SFX_VOLUME);
    }
}

/// Plugin: loads the cue handles + starts the ambient bed, then runs all the
/// edge-detector playback systems in `Update`. Added in `app::run` alongside
/// the other render/sim plugins; relies on `DefaultPlugins` supplying
/// bevy's `AudioPlugin` (the `bevy_audio` feature is on in this crate).
pub struct GameAudioPlugin;

impl Plugin for GameAudioPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, load_audio_and_start_ambient)
            .add_systems(
                Update,
                (
                    play_throw_sfx,
                    play_ricochet_sfx,
                    play_catch_sfx,
                    play_kill_sfx,
                    play_shatter_sfx,
                    play_pickup_spawn_sfx,
                    play_pickup_collect_sfx,
                    play_match_state_sfx,
                ),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(x: f32, y: f32) -> Vec2 {
        Vec2::new(x, y).normalize()
    }

    #[test]
    fn ricochet_ignores_straight_flight() {
        // Frame-to-frame heading unchanged → not a bounce.
        assert!(!is_ricochet_turn(unit(1.0, 0.0), unit(1.0, 0.0)));
    }

    #[test]
    fn ricochet_ignores_gentle_curve() {
        // Curve modifier steers ~1.5°/tick — far inside the threshold.
        let a = unit(1.0, 0.0);
        let theta = 1.5_f32.to_radians();
        let b = unit(theta.cos(), theta.sin());
        assert!(!is_ricochet_turn(a, b));
    }

    #[test]
    fn ricochet_ignores_bouncy_speed_only_change() {
        // Bouncy keeps heading, only gains speed; the *unit* vectors are equal.
        assert!(!is_ricochet_turn(unit(0.6, 0.8), unit(0.6, 0.8)));
    }

    #[test]
    fn ricochet_fires_on_wall_reflection() {
        // A vertical wall flips vx: a 90°+ turn → well past the threshold.
        assert!(is_ricochet_turn(unit(1.0, 0.2), unit(-1.0, 0.2)));
    }

    #[test]
    fn ricochet_fires_on_full_reversal() {
        assert!(is_ricochet_turn(unit(1.0, 0.0), unit(-1.0, 0.0)));
    }

    #[test]
    fn ricochet_threshold_is_at_45_degrees() {
        // Exactly 45° sits right at the boundary (dot ≈ 0.707, not < it) → no
        // fire; just past 45° fires. Documents where the line is drawn.
        let a = unit(1.0, 0.0);
        let just_under = 44.0_f32.to_radians();
        let just_over = 46.0_f32.to_radians();
        assert!(!is_ricochet_turn(a, unit(just_under.cos(), just_under.sin())));
        assert!(is_ricochet_turn(a, unit(just_over.cos(), just_over.sin())));
    }
}
