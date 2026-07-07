//! Synthesized-cue playback + the two-bed music system.
//!
//! Twenty-odd one-shot SFX, two music loops (title theme / match groove), and
//! the match-point heartbeat, all fired off the same sim-event edges the
//! render crate's effect sprites detect. Everything here is cosmetic —
//! `AudioPlayer` entities are never rolled back, and the cues read *current*
//! sim state through the same `Local` prev-state edge pattern the effect
//! spawners use (never `Added`/`Changed`, which rollback re-simulation makes
//! unreliable).
//!
//! Mixing model (audio-design discipline): two buses — SFX and Music — whose
//! 0..1 settings sliders map through a SQUARE perceptual taper ([`bus_gain`]),
//! not raw linear amplitude. Both music beds loop from boot and are
//! crossfaded by screen state; kills sidechain-DUCK the music (fast attack,
//! slow release) so the impact owns the room for a beat; the match-point
//! ritual ducks the bed nearly out entirely under the heartbeat.
//!
//! **Why this lives in `app`, not `render`:** audio needs an output *device*,
//! exactly as windowing needs a display — so `bevy_audio`/`cpal` belong with
//! the windowing crate, not the determinism-core `render` crate that the
//! cross-platform matrix builds on every target.
//!
//! The cues themselves are generated deterministically by
//! `scripts/generate_audio.py` into `assets/audio/*.wav` — an 80s analog
//! synth palette (detuned saws, resonant sweeps, gated drums, echoes). This
//! module only decides *when* to play them and how loud.

use bevy::audio::Volume;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use rand::Rng as _;
use render::CosmeticRng;
use sim::{
    AnimState, BonePyre, Boomerang, BoomerangMods, BoomerangState, DashState, Dead, Empowered,
    FrameCount, HeldModifier, MatchState, Pickup, PickupKind, Player, SelectedArena, Taunt,
    ThrowCharge, VelocityF,
};

use crate::screen::AppScreen;
use crate::settings::Settings;

/// GO-toll pitch: the countdown toll replayed up a major third (×1.25 speed)
/// so the "go" reads distinct from the three descending pre-beats.
pub const GO_TOLL_SPEED: f32 = 1.25;

/// Menu-confirm pitch: the tap blip slowed a touch so "start match" lands
/// lower and heavier than the arena-cycle tick.
const CONFIRM_TAP_SPEED: f32 = 0.85;

/// A flying fang fires the ricochet cue when its heading turns harder than this
/// (dot product of consecutive unit velocities below cos 45°). Wall/pyre
/// bounces reverse a velocity component → well below the threshold; the Curve
/// modifier (≈1.5°/tick) and Bouncy (speed-only) stay above it, so they don't
/// false-trigger.
const RICOCHET_DOT_THRESHOLD: f32 = 0.707;

// ---- Per-cue trims (multiplied into the SFX bus gain). The files are all
// peak-normalized to -3 dBFS; these place each cue in the mix. UI ticks sit
// far down; kills own the room. ----
const TRIM_DASH: f32 = 0.7;
const TRIM_DASH_READY: f32 = 0.3;
const TRIM_CHARGE_RISER: f32 = 0.55;
const TRIM_RESPAWN: f32 = 0.6;
const TRIM_MENU_TAP: f32 = 0.4;
const TRIM_SUDDEN_DEATH: f32 = 0.9;
const TRIM_TAUNT: f32 = 0.75;

/// Kill-moment sidechain: the music bus dips to this level instantly, then
/// recovers with [`DUCK_RELEASE_TAU`] — audible dip, no pumping.
const KILL_DUCK: f32 = 0.3;
const DUCK_RELEASE_TAU: f32 = 0.6;

/// Title↔match crossfade time constant. Slow enough to feel like a scene
/// change, fast enough that the groove is in before the countdown ends.
const XFADE_TAU: f32 = 1.2;

/// Match-point ritual: the bed drops nearly out so the heartbeat owns the
/// room (the ritual is a designed near-silence).
const RITUAL_DUCK: f32 = 0.15;

/// Map a 0..1 volume slider to linear gain through a square taper — the
/// perceptual approximation (equal slider steps ≈ more-equal loudness steps
/// than raw linear, which does almost nothing until the very bottom).
pub fn bus_gain(slider: f32) -> f32 {
    let v = slider.clamp(0.0, 1.0);
    v * v
}

/// Exponential approach toward a target — the smoothing primitive behind the
/// crossfade and the duck release. Frame-rate independent via `dt/tau`.
pub fn approach(current: f32, target: f32, dt: f32, tau: f32) -> f32 {
    if tau <= 0.0 {
        return target;
    }
    current + (target - current) * (1.0 - (-dt / tau).exp())
}

/// Pure predicate (testable): did a fang's heading turn hard enough this frame
/// to count as a bounce? `prev_dir`/`cur_dir` are unit velocity vectors.
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
    pub match_win_sting: Handle<AudioSource>,
    pub pickup_spawn: Handle<AudioSource>,
    pub pickup_collect: Handle<AudioSource>,
    pub dash: Handle<AudioSource>,
    pub dash_ready: Handle<AudioSource>,
    pub charge_riser: Handle<AudioSource>,
    pub respawn: Handle<AudioSource>,
    pub sudden_death: Handle<AudioSource>,
    pub menu_tap: Handle<AudioSource>,
    pub taunt: Handle<AudioSource>,
    pub title_loop: Handle<AudioSource>,
    pub match_loop: Handle<AudioSource>,
    pub heartbeat_loop: Handle<AudioSource>,
}

/// Spawn a one-shot cue at `volume`, set to despawn its entity when finished
/// (so cues never leak entities).
fn play(commands: &mut Commands, clip: &Handle<AudioSource>, volume: f32) {
    play_pitched(commands, clip, volume, 1.0);
}

/// One-shot cue with a playback-speed multiplier (GO toll, confirm tap).
fn play_pitched(commands: &mut Commands, clip: &Handle<AudioSource>, volume: f32, speed: f32) {
    commands.spawn((
        AudioPlayer::new(clip.clone()),
        PlaybackSettings::DESPAWN
            .with_volume(Volume::Linear(volume))
            .with_speed(speed),
    ));
}

/// The SFX bus gain for this frame.
fn sfx_gain(settings: &Settings) -> f32 {
    bus_gain(settings.sfx_volume)
}

// ---------------------------------------------------------------------------
// Music: two beds, crossfaded by screen, ducked by kills and the ritual.
// ---------------------------------------------------------------------------

/// Marker on the looping title-theme entity.
#[derive(Component)]
struct TitleBed;

/// Marker on the looping match-groove entity.
#[derive(Component)]
struct MatchBed;

/// Marker on the looping heartbeat entity (match-point ritual bed).
#[derive(Component)]
struct HeartbeatLoop;

/// Live music-mix state: the two bed levels chase their screen-state targets
/// ([`XFADE_TAU`]); `duck` snaps down on kills and releases back to 1.
#[derive(Resource)]
pub struct MusicMix {
    title: f32,
    match_: f32,
    duck: f32,
}

impl Default for MusicMix {
    fn default() -> Self {
        Self {
            title: 1.0,
            match_: 0.0,
            duck: 1.0,
        }
    }
}

/// Startup: load every cue handle, insert [`AudioAssets`], and start BOTH
/// music beds looping — the title theme audible, the match groove parked at
/// zero until the crossfade brings it in.
fn load_audio_and_start_music(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    settings: Res<Settings>,
) {
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
        match_win_sting: asset_server.load("audio/match_win_sting.wav"),
        pickup_spawn: asset_server.load("audio/pickup_spawn.wav"),
        pickup_collect: asset_server.load("audio/pickup_collect.wav"),
        dash: asset_server.load("audio/dash.wav"),
        dash_ready: asset_server.load("audio/dash_ready.wav"),
        charge_riser: asset_server.load("audio/charge_riser.wav"),
        respawn: asset_server.load("audio/respawn.wav"),
        sudden_death: asset_server.load("audio/sudden_death.wav"),
        menu_tap: asset_server.load("audio/menu_tap.wav"),
        taunt: asset_server.load("audio/taunt.wav"),
        title_loop: asset_server.load("audio/title_loop.wav"),
        match_loop: asset_server.load("audio/match_loop.wav"),
        heartbeat_loop: asset_server.load("audio/heartbeat_loop.wav"),
    };
    commands.spawn((
        TitleBed,
        AudioPlayer::new(assets.title_loop.clone()),
        PlaybackSettings::LOOP.with_volume(Volume::Linear(bus_gain(settings.music_volume))),
    ));
    commands.spawn((
        MatchBed,
        AudioPlayer::new(assets.match_loop.clone()),
        PlaybackSettings::LOOP.with_volume(Volume::Linear(0.0)),
    ));
    commands.insert_resource(assets);
}

/// Per-frame music mixer: chase the screen-state crossfade targets, release
/// the kill duck, apply the ritual duck, and write the resulting bus volumes
/// into both bed sinks.
#[allow(clippy::type_complexity)]
fn mix_music(
    time: Res<Time>,
    settings: Res<Settings>,
    screen: Res<State<AppScreen>>,
    ritual: Res<render::MatchPointRitual>,
    mut mix: ResMut<MusicMix>,
    mut title: Query<&mut AudioSink, (With<TitleBed>, Without<MatchBed>)>,
    mut match_: Query<&mut AudioSink, (With<MatchBed>, Without<TitleBed>)>,
) {
    let dt = time.delta_secs();
    let on_title = *screen.get() == AppScreen::Title;
    mix.title = approach(mix.title, if on_title { 1.0 } else { 0.0 }, dt, XFADE_TAU);
    mix.match_ = approach(mix.match_, if on_title { 0.0 } else { 1.0 }, dt, XFADE_TAU);
    mix.duck = approach(mix.duck, 1.0, dt, DUCK_RELEASE_TAU);

    let bus = bus_gain(settings.music_volume) * mix.duck;
    let ritual_duck = if ritual.0 { RITUAL_DUCK } else { 1.0 };
    for mut sink in &mut title {
        sink.set_volume(Volume::Linear(bus * mix.title));
    }
    for mut sink in &mut match_ {
        sink.set_volume(Volume::Linear(bus * mix.match_ * ritual_duck));
    }
}

/// Start/stop the 40 BPM heartbeat with the match-point ritual. The loop
/// entity exists only while the ritual holds; its volume tracks the music
/// bus live (a Title-screen slider change must reach it too).
fn match_point_heartbeat(
    mut commands: Commands,
    ritual: Res<render::MatchPointRitual>,
    assets: Option<Res<AudioAssets>>,
    settings: Res<Settings>,
    mut running: Query<(Entity, &mut AudioSink), With<HeartbeatLoop>>,
) {
    let Some(assets) = assets else {
        return;
    };
    let have = !running.is_empty();
    if ritual.0 && !have {
        commands.spawn((
            HeartbeatLoop,
            AudioPlayer::new(assets.heartbeat_loop.clone()),
            PlaybackSettings::LOOP.with_volume(Volume::Linear(bus_gain(settings.music_volume))),
        ));
    } else if !ritual.0 && have {
        for (entity, _) in &running {
            commands.entity(entity).despawn();
        }
    } else if have && settings.is_changed() {
        for (_, mut sink) in &mut running {
            sink.set_volume(Volume::Linear(bus_gain(settings.music_volume)));
        }
    }
}

// ---------------------------------------------------------------------------
// Event-edge SFX systems.
// ---------------------------------------------------------------------------

/// Per-handle throw-edge tracking: whether each player's *primary* fang was in
/// flight last frame, and that player's empowered flag the frame before the
/// throw consumed it (so the empowered throw reads its brighter variant).
#[derive(Default)]
struct ThrowTrack {
    had_primary: HashMap<usize, bool>,
    prev_empowered: HashMap<usize, bool>,
}

/// Throw cue: a primary fang appearing for an owner that had none is a throw.
fn play_throw_sfx(
    mut commands: Commands,
    assets: Res<AudioAssets>,
    settings: Res<Settings>,
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
            play(&mut commands, clip, sfx_gain(&settings));
        }
        track.had_primary.insert(h, now);
    }
    for (player, emp) in &players {
        track.prev_empowered.insert(player.handle, emp.0);
    }
}

/// Per-boomerang-entity previous unit velocity, for ricochet detection.
#[derive(Default)]
struct RicochetTrack(HashMap<Entity, Vec2>);

/// Ricochet cue: a flying fang whose heading turns harder than the threshold
/// (a wall or pyre bounce) rings the tuned-bone filter ping.
fn play_ricochet_sfx(
    mut commands: Commands,
    assets: Res<AudioAssets>,
    settings: Res<Settings>,
    mut rng: ResMut<CosmeticRng>,
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
            // The most-repeated cue in the game: a small pitch scatter keeps
            // a triple-bounce from sounding like a sampler pad (SFX-variation
            // 101). Cosmetic RNG only — never SimRng.
            let speed = rng.0.gen_range(0.92..1.08);
            play_pitched(&mut commands, &assets.ricochet, sfx_gain(&settings), speed);
        }
        track.0.insert(entity, dir);
    }
    track.0.retain(|entity, _| booms.get(*entity).is_ok());
}

/// Per-player previous `AnimState.anim_id`, for catch detection.
#[derive(Default)]
struct CatchTrack(HashMap<usize, u8>);

/// Catch cue: a player's anim entering CATCH. Empowered at that moment means
/// it was a perfect catch → the glitter-arp variant.
fn play_catch_sfx(
    mut commands: Commands,
    assets: Res<AudioAssets>,
    settings: Res<Settings>,
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
            play(&mut commands, clip, sfx_gain(&settings));
        }
        track.0.insert(player.handle, anim.anim_id);
    }
}

/// Per-player previous `is_dying`, for the kill and respawn cues.
#[derive(Default)]
struct DyingTrack(HashMap<usize, bool>);

/// Kill cue + music duck: the tick a player transitions into dying, the 80s
/// action hit lands and the music bus dips out of its way (sidechain feel:
/// instant attack here, slow release in [`mix_music`]).
fn play_kill_sfx(
    mut commands: Commands,
    assets: Res<AudioAssets>,
    settings: Res<Settings>,
    mut mix: ResMut<MusicMix>,
    mut rng: ResMut<CosmeticRng>,
    players: Query<(&Player, &Dead)>,
    mut track: Local<DyingTrack>,
) {
    for (player, dead) in &players {
        let now = dead.is_dying();
        let was = track.0.get(&player.handle).copied().unwrap_or(false);
        if now && !was {
            // A hair of pitch scatter so a double-kill round doesn't fire
            // two identical hits back to back. Tight range — the kill's
            // identity must stay recognizable.
            let speed = rng.0.gen_range(0.96..1.05);
            play_pitched(&mut commands, &assets.kill, sfx_gain(&settings), speed);
            mix.duck = mix.duck.min(KILL_DUCK);
        }
        if !now && was {
            play(
                &mut commands,
                &assets.respawn,
                sfx_gain(&settings) * TRIM_RESPAWN,
            );
        }
        track.0.insert(player.handle, now);
    }
}

/// Per-player previous `Taunt` counter, for the start and completion edges.
#[derive(Default)]
struct TauntTrack(HashMap<usize, u32>);

/// Taunt cues: the cocky two-stab horn the tick a player's flex starts,
/// and the perfect-catch arp when it COMPLETES (counter walking 1 → 0 —
/// a cancel drops from higher, and the same streak tier-up deserves the
/// same glitter). Both players' taunts are audible — the whole point of
/// the mechanic is that the disrespect (and the punish window) is public.
fn play_taunt_sfx(
    mut commands: Commands,
    assets: Res<AudioAssets>,
    settings: Res<Settings>,
    players: Query<(&Player, &Taunt)>,
    mut track: Local<TauntTrack>,
) {
    for (player, taunt) in &players {
        let was = track.0.get(&player.handle).copied().unwrap_or(0);
        if taunt.0 > 0 && was == 0 {
            play(
                &mut commands,
                &assets.taunt,
                sfx_gain(&settings) * TRIM_TAUNT,
            );
        }
        if taunt.0 == 0 && was == 1 {
            play(&mut commands, &assets.catch_perfect, sfx_gain(&settings));
        }
        track.0.insert(player.handle, taunt.0);
    }
}

/// Per-player previous dashing flag.
#[derive(Default)]
struct DashTrack(HashMap<usize, bool>);

/// Dash cue: any player entering `Dashing` fires the zip — the opponent's
/// dash is a tell worth hearing.
fn play_dash_sfx(
    mut commands: Commands,
    assets: Res<AudioAssets>,
    settings: Res<Settings>,
    players: Query<(&Player, &DashState)>,
    mut track: Local<DashTrack>,
) {
    for (player, dash) in &players {
        let now = matches!(dash, DashState::Dashing { .. });
        let was = track.0.get(&player.handle).copied().unwrap_or(false);
        if now && !was {
            play(&mut commands, &assets.dash, sfx_gain(&settings) * TRIM_DASH);
        }
        track.0.insert(player.handle, now);
    }
}

/// Dash-ready tick: the LOCAL player's cooldown ending — pure UI feedback,
/// closing the loop with the dash ring's refill sweep. Local only; the
/// opponent's cooldown is their business.
fn play_dash_ready_sfx(
    mut commands: Commands,
    assets: Res<AudioAssets>,
    settings: Res<Settings>,
    local_handle: Res<crate::netplay::LocalPlayerHandle>,
    players: Query<(&Player, &DashState)>,
    mut was_cooling: Local<bool>,
) {
    let local = local_handle.0.unwrap_or(0);
    let cooling = players
        .iter()
        .find(|(p, _)| p.handle == local)
        .is_some_and(|(_, d)| matches!(d, DashState::Cooldown { .. }));
    if *was_cooling && !cooling {
        play(
            &mut commands,
            &assets.dash_ready,
            sfx_gain(&settings) * TRIM_DASH_READY,
        );
    }
    *was_cooling = cooling;
}

/// Per-player live charge-riser entity, so the riser can be cut the instant
/// the wind-up ends (throw released, dash, death).
#[derive(Default)]
struct RiserTrack(HashMap<usize, Entity>);

/// Charge riser: a resonant sweep that climbs with the wind-up — both
/// players' plants are public telegraphs, so both are audible. The file is
/// exactly `CHARGE_MAX_FRAMES` long; an early release just cuts it (the
/// throw zap takes over).
fn play_charge_riser_sfx(
    mut commands: Commands,
    assets: Res<AudioAssets>,
    settings: Res<Settings>,
    players: Query<(&Player, &ThrowCharge)>,
    mut track: Local<RiserTrack>,
) {
    for (player, charge) in &players {
        let h = player.handle;
        let winding = charge.0 > 0;
        let live = track.0.contains_key(&h);
        if winding && !live {
            let entity = commands
                .spawn((
                    AudioPlayer::new(assets.charge_riser.clone()),
                    PlaybackSettings::DESPAWN.with_volume(Volume::Linear(
                        sfx_gain(&settings) * TRIM_CHARGE_RISER,
                    )),
                ))
                .id();
            track.0.insert(h, entity);
        } else if !winding
            && live
            && let Some(entity) = track.0.remove(&h)
            && let Ok(mut e) = commands.get_entity(entity)
        {
            // If the riser already finished, DESPAWN removed it and
            // get_entity fails — nothing to cut.
            e.despawn();
        }
    }
}

/// Per-pyre previous `shattered`, for the shatter cue.
#[derive(Default)]
struct ShatterTrack(HashMap<Entity, bool>);

/// Shatter cue: the tick a `BonePyre` breaks.
fn play_shatter_sfx(
    mut commands: Commands,
    assets: Res<AudioAssets>,
    settings: Res<Settings>,
    pyres: Query<(Entity, &BonePyre)>,
    mut track: Local<ShatterTrack>,
) {
    for (entity, pyre) in &pyres {
        let was = track.0.get(&entity).copied().unwrap_or(false);
        if pyre.shattered && !was {
            play(&mut commands, &assets.shatter, sfx_gain(&settings));
        }
        track.0.insert(entity, pyre.shattered);
    }
}

/// Pickup-spawn cue: a floor pickup appearing where there was none.
fn play_pickup_spawn_sfx(
    mut commands: Commands,
    assets: Res<AudioAssets>,
    settings: Res<Settings>,
    pickups: Query<(), With<Pickup>>,
    mut present: Local<bool>,
) {
    let now = !pickups.is_empty();
    if now && !*present {
        play(&mut commands, &assets.pickup_spawn, sfx_gain(&settings));
    }
    *present = now;
}

/// Per-player previously-held modifier, for the collect cue.
#[derive(Default)]
struct HeldTrack(HashMap<usize, Option<PickupKind>>);

/// Pickup-collect cue: a player's held modifier becoming `Some` (or changing
/// to a different `Some`). A throw clearing the slot is not a collect.
fn play_pickup_collect_sfx(
    mut commands: Commands,
    assets: Res<AudioAssets>,
    settings: Res<Settings>,
    players: Query<(&Player, &HeldModifier)>,
    mut track: Local<HeldTrack>,
) {
    for (player, held) in &players {
        let now = held.0;
        let was = track.0.get(&player.handle).copied().unwrap_or(None);
        if now.is_some() && now != was {
            play(&mut commands, &assets.pickup_collect, sfx_gain(&settings));
        }
        track.0.insert(player.handle, now);
    }
}

/// Sudden-death cue: the frame the round clock pushes the floor into its
/// crumble (`sudden_death_factor` dropping below 1), the dread drone lands.
/// Resets automatically when the round ends (the factor reads 1 again).
fn play_sudden_death_sfx(
    mut commands: Commands,
    assets: Res<AudioAssets>,
    settings: Res<Settings>,
    state: Res<MatchState>,
    frame: Res<FrameCount>,
    mut was_crumbling: Local<bool>,
) {
    let crumbling = match *state {
        MatchState::InRound { expires_at_frame } => {
            let remaining = expires_at_frame.saturating_sub(frame.0);
            sim::sudden_death_factor(remaining).to_num::<f32>() < 0.9995
        }
        _ => false,
    };
    if crumbling && !*was_crumbling {
        play(
            &mut commands,
            &assets.sudden_death,
            sfx_gain(&settings) * TRIM_SUDDEN_DEATH,
        );
    }
    *was_crumbling = crumbling;
}

/// Title-menu taps: cycling the arena picker blips; committing to a match
/// plays the same blip slowed into a confirm. Edge-driven off the
/// `SelectedArena` resource and the screen transition.
fn play_menu_tap_sfx(
    mut commands: Commands,
    assets: Res<AudioAssets>,
    settings: Res<Settings>,
    screen: Res<State<AppScreen>>,
    selected: Res<SelectedArena>,
    mut prev_arena: Local<Option<sim::ArenaId>>,
    mut prev_screen: Local<Option<AppScreen>>,
) {
    let on_title = *screen.get() == AppScreen::Title;
    if on_title
        && let Some(prev) = *prev_arena
        && prev != selected.0
    {
        play(
            &mut commands,
            &assets.menu_tap,
            sfx_gain(&settings) * TRIM_MENU_TAP,
        );
    }
    *prev_arena = Some(selected.0);

    let now = *screen.get();
    if *prev_screen == Some(AppScreen::Title) && now == AppScreen::InMatch {
        play_pitched(
            &mut commands,
            &assets.menu_tap,
            sfx_gain(&settings) * TRIM_MENU_TAP,
            CONFIRM_TAP_SPEED,
        );
    }
    *prev_screen = Some(now);
}

/// Match-clock cues: the three descending countdown tolls (3/2/1), the GO
/// toll (same toll a major third up), the round-over sting, and the bigger
/// match-won stinger. Driven off `MatchState` transitions in a `Local`.
fn play_match_state_sfx(
    mut commands: Commands,
    assets: Res<AudioAssets>,
    settings: Res<Settings>,
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
            play(&mut commands, &assets.countdown_toll, sfx_gain(&settings));
        }
        return;
    };

    // Entering a fresh countdown (a new round begins) → the first toll.
    if is_countdown(now) && !is_countdown(prev_state) {
        play(&mut commands, &assets.countdown_toll, sfx_gain(&settings));
        return;
    }
    // Digit ticking down inside the countdown (3→2→1) → a toll per beat.
    if let (MatchState::Countdown { digit: pd, .. }, MatchState::Countdown { digit: nd, .. }) =
        (prev_state, now)
    {
        if nd != pd {
            play(&mut commands, &assets.countdown_toll, sfx_gain(&settings));
        }
        return;
    }
    // Countdown → InRound: the GO toll, pitched up.
    if is_countdown(prev_state) && matches!(now, MatchState::InRound { .. }) {
        play_pitched(
            &mut commands,
            &assets.countdown_toll,
            sfx_gain(&settings),
            GO_TOLL_SPEED,
        );
        return;
    }
    // Round over → the mournful sting; the whole match won → the big one.
    if is_matchover(now) && !is_matchover(prev_state) {
        play(&mut commands, &assets.match_win_sting, sfx_gain(&settings));
    } else if is_roundover(now) && !is_roundover(prev_state) {
        play(&mut commands, &assets.round_over_sting, sfx_gain(&settings));
    }
}

/// Plugin: loads the cue handles + starts both music beds, then runs the
/// edge-detector playback systems and the per-frame music mixer in `Update`.
pub struct GameAudioPlugin;

impl Plugin for GameAudioPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MusicMix>()
            .add_systems(Startup, load_audio_and_start_music)
            .add_systems(
                Update,
                (
                    play_throw_sfx,
                    play_ricochet_sfx,
                    play_catch_sfx,
                    play_kill_sfx,
                    play_dash_sfx,
                    play_dash_ready_sfx,
                    play_charge_riser_sfx,
                    play_shatter_sfx,
                    play_pickup_spawn_sfx,
                    play_pickup_collect_sfx,
                    play_sudden_death_sfx,
                    play_menu_tap_sfx,
                    play_taunt_sfx,
                    play_match_state_sfx,
                    // The mixer runs after the kill system so a kill's duck
                    // lands the same frame the hit does.
                    mix_music.after(play_kill_sfx),
                    match_point_heartbeat,
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
        assert!(!is_ricochet_turn(unit(0.6, 0.8), unit(0.6, 0.8)));
    }

    #[test]
    fn ricochet_fires_on_wall_reflection() {
        assert!(is_ricochet_turn(unit(1.0, 0.2), unit(-1.0, 0.2)));
    }

    #[test]
    fn ricochet_fires_on_full_reversal() {
        assert!(is_ricochet_turn(unit(1.0, 0.0), unit(-1.0, 0.0)));
    }

    #[test]
    fn ricochet_threshold_is_at_45_degrees() {
        let a = unit(1.0, 0.0);
        let just_under = 44.0_f32.to_radians();
        let just_over = 46.0_f32.to_radians();
        assert!(!is_ricochet_turn(
            a,
            unit(just_under.cos(), just_under.sin())
        ));
        assert!(is_ricochet_turn(a, unit(just_over.cos(), just_over.sin())));
    }

    // ---- Mixing math ----

    #[test]
    fn bus_gain_is_a_perceptual_taper() {
        // Square taper: end-points exact, midpoint well below linear, and
        // monotonic throughout — the slider does something everywhere.
        assert_eq!(bus_gain(0.0), 0.0);
        assert_eq!(bus_gain(1.0), 1.0);
        assert!((bus_gain(0.5) - 0.25).abs() < 1e-6);
        let mut prev = -1.0;
        for i in 0..=20 {
            let g = bus_gain(i as f32 / 20.0);
            assert!(g >= prev);
            prev = g;
        }
        // Out-of-range inputs clamp instead of exploding.
        assert_eq!(bus_gain(-1.0), 0.0);
        assert_eq!(bus_gain(2.0), 1.0);
    }

    #[test]
    fn approach_converges_and_is_stable() {
        // A few steps get most of the way; it never overshoots the target.
        let mut v = 0.0;
        for _ in 0..60 {
            v = approach(v, 1.0, 1.0 / 60.0, 0.2);
            assert!(v <= 1.0);
        }
        assert!(v > 0.95, "60 frames at tau=0.2 should be converged: {v}");
        // Zero tau snaps.
        assert_eq!(approach(0.3, 1.0, 0.016, 0.0), 1.0);
    }

    #[test]
    fn kill_duck_recovers_slower_than_it_attacks() {
        // The attack is instant (min-assignment); one release step at 60 Hz
        // must recover only a fraction of the dip — no pumping.
        let ducked = KILL_DUCK;
        let after_one_frame = approach(ducked, 1.0, 1.0 / 60.0, DUCK_RELEASE_TAU);
        assert!(after_one_frame < ducked + 0.05);
    }
}
