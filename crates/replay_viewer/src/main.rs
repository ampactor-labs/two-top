//! Phase 14 cycle 1 — standalone replay viewer.
//!
//! Loads a `.bmrg` file from disk and plays it back deterministically
//! in a windowed bevy app. The sim runs at the same TICK_HZ the live
//! game uses; the render layer interpolates between snapshots so the
//! window updates at vsync rate. Inputs come from
//! `replay::ReplayPlaybackPlugin` (replaces the live touch source).
//!
//! ## Usage
//!
//! ```sh
//! replay_viewer <path.bmrg>
//! ```
//!
//! The replay must match the running binary's `sim::SIM_VERSION`
//! exactly — replays from older builds need to be viewed via the
//! corresponding archived git-tagged binary (no migration code, per
//! ARCHITECTURE.md).
//!
//! ## Cycle 1 scope
//!
//! - Load `.bmrg`, deterministic forward playback, render with the
//!   same sprite stack as the live game.
//! - Frame counter HUD ("frame N / total").
//! - Auto-quits when the input stream is exhausted.
//!
//! Cycle 2 will add: snapshot system every 60 frames, scrub bar UI,
//! frame-step buttons, speed selector, hitbox/velocity overlay.

use std::path::PathBuf;
use std::process::ExitCode;

use bevy::prelude::*;
use bevy_ggrs::GgrsPlugin;
use bevy_ggrs::prelude::*;
use fixed_math::Vec2F;
use render::RenderSyncPlugin;
use replay::{ReplayPlayback, ReplayPlaybackPlugin, decode_for_sim_version};
use sim::{
    BOOMERANG_HALF_EXTENT_CM, Boomerang, GgrsCfg, PLAYER_HALF_EXTENT_CM, Player, PositionF,
    PreviousPositionF, SimPlugin, SimSnapshot, VelocityF, arena_walls,
};

/// Snapshot interval in sim ticks. Per BUILD_PLAN § Phase 14 Produces:
/// "Snapshots taken every 60 frames" (1 second at 60 Hz). Bigger
/// interval = less RAM, more replay-forward to scrub. 60 keeps the
/// worst-case scrub-forward to one second of sim, which `Time<Virtual>`
/// at the seek speed completes well under the 100 ms exit-criterion
/// budget.
const SNAPSHOT_INTERVAL: u32 = 60;
/// Multiplier applied to `Time<Virtual>` while a seek is in flight. At
/// 64x: one second of virtual playback = ~16 ms of wall-clock time, so
/// scrubbing across the worst-case 60-frame replay-forward window
/// completes in one or two Update cycles. Higher multipliers risk
/// bevy_ggrs's accumulator overflowing the per-Update budget; 64x is
/// the largest power-of-two that empirically lands in <100 ms across
/// the canonical demo's frame range.
const SEEK_SPEED: f32 = 64.0;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let path = match args.get(1) {
        Some(p) => PathBuf::from(p),
        None => {
            eprintln!("usage: replay_viewer <path.bmrg>");
            return ExitCode::from(2);
        }
    };

    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("replay_viewer: failed to read {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    let replay = match decode_for_sim_version(&bytes, sim::SIM_VERSION) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("replay_viewer: failed to decode {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };

    let total_frames = replay.header.frame_count;
    eprintln!(
        "replay_viewer: loaded {} (sim_version={}, frames={}, players={})",
        path.display(),
        replay.header.sim_version,
        total_frames,
        replay.header.num_players,
    );

    // Same SyncTest configuration replay_sync uses — a single-player
    // session that runs each frame deterministically without rollback
    // verification overhead. The check_distance value is irrelevant for
    // playback (no input divergence is possible), but keeping it at 2
    // matches the live app's session shape so the schedule executes
    // identically.
    let mut sb = SessionBuilder::<GgrsCfg>::new()
        .with_num_players(replay.header.num_players as usize)
        .expect("with_num_players")
        .with_check_distance(2)
        .with_input_delay(2);
    for i in 0..replay.header.num_players as usize {
        sb = sb.add_player(PlayerType::Local, i).expect("add_player");
    }
    let session = sb.start_synctest_session().expect("synctest");

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: format!("2-Top — replay viewer ({})", path.display()),
                resolution: (960u32, 720u32).into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(GgrsPlugin::<GgrsCfg>::default())
        .add_plugins(SimPlugin)
        .add_plugins(ReplayPlaybackPlugin)
        .add_plugins(RenderSyncPlugin)
        .insert_resource(Session::SyncTest(session))
        .insert_resource(ReplayPlayback::new(replay))
        .insert_resource(TotalFrames(total_frames))
        .insert_resource(SnapshotBuffer::default())
        .insert_resource(ViewerControls::default())
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                ensure_boomerang_visuals,
                handle_keyboard_controls,
                drive_seek,
                capture_snapshot,
                update_frame_counter,
                quit_when_replay_finished,
            )
                .chain(),
        )
        // The diagnostic overlay reads `Transform` written by
        // `render::sync_transforms_from_sim`, so it must run AFTER
        // the render-side sync. Bevy default ordering would give us
        // that, but `.after()` makes the dependency explicit.
        .add_systems(
            Update,
            draw_diagnostic_overlay.after(render::sync_transforms_from_sim),
        )
        .run();

    ExitCode::SUCCESS
}

#[derive(Resource, Clone, Copy)]
struct TotalFrames(u32);

#[derive(Component)]
struct FrameCounterText;

/// Ring of snapshots taken every [`SNAPSHOT_INTERVAL`] frames during
/// forward playback. Backward seeks restore from the latest snapshot
/// at-or-before the target frame and replay forward from there. Stays
/// in memory only — never persisted.
///
/// Memory budget: at 60-frame intervals across a 30 s round (1800
/// frames), that's 30 snapshots. Each snapshot is ~hundred-byte for the
/// player/boomerang bundles plus the InputHistory clone — call it 2 KB
/// per snapshot, 60 KB per round, well below any reasonable cap.
#[derive(Resource, Default)]
struct SnapshotBuffer {
    /// Snapshots ordered by ascending `frame`. Push-only during forward
    /// playback; truncated on backward seek so a future forward
    /// playthrough can re-populate from the seek-target onward.
    entries: Vec<SimSnapshot>,
}

impl SnapshotBuffer {
    /// Latest snapshot at or before `frame`, if any. Returns `None`
    /// when seeking before the very first snapshot (handled by a
    /// caller-side reset to frame 0).
    fn nearest_before(&self, frame: u32) -> Option<&SimSnapshot> {
        self.entries.iter().rev().find(|s| s.frame <= frame)
    }
}

/// Viewer playback state. Pause / single-step / seek are all driven
/// through this; the actual sim-tick advancement happens via
/// [`Time<Virtual>`] (paused → no accumulator fill → no ticks; sped-up
/// → many ticks per Update, used for seek scrubs).
///
/// Seeks are async by nature: setting `seek_target` records the
/// destination, and [`drive_seek`] fast-forwards the sim until the
/// frame counter catches up, then re-pauses (or returns to play).
#[derive(Resource, Clone, Copy, Default)]
struct ViewerControls {
    /// Player's stated paused/playing intent. Independent of
    /// `seek_target` — a seek may temporarily unpause `Time<Virtual>`
    /// even while `paused = true` so the seek can complete.
    paused: bool,
    /// Frame the user has requested. `None` means "no seek pending —
    /// just normal play (or pause)".
    seek_target: Option<u32>,
    /// Cycle 2b.1 hitbox / velocity overlay toggle. When `true` the
    /// `draw_diagnostic_overlay` system renders Charcoal-Line
    /// wireframes around every Player + Boomerang AABB plus a velocity
    /// vector from each entity's center.
    show_overlay: bool,
}

/// Spawns players + walls + camera. Spawn positions match
/// [`replay_sync::build_app`] (both at origin) because the only `.bmrg`
/// files that currently exist were recorded against that layout (the
/// canonical demo from `gen_canonical` and the fuzz-art demos). The
/// live `app` crate uses different spawn positions; reconciling that
/// (either by canonicalizing the spawn function in `sim` or by
/// encoding initial state in the `.bmrg` header) is out-of-scope for
/// Phase 14 cycle 1 — it surfaces the moment Phase 13's deferred
/// in-app match-recording lands and the format starts carrying real
/// match recordings.
fn setup(mut commands: Commands) {
    commands.spawn(Camera2d);

    commands.spawn((
        Player { handle: 0 },
        PositionF(Vec2F::ZERO),
        PreviousPositionF(Vec2F::ZERO),
        VelocityF(Vec2F::ZERO),
        Sprite {
            color: Color::srgb(0.95, 0.42, 0.65),
            custom_size: Some(Vec2::new(48.0, 48.0)),
            ..default()
        },
        Transform::default(),
    ));

    commands.spawn((
        Player { handle: 1 },
        PositionF(Vec2F::ZERO),
        PreviousPositionF(Vec2F::ZERO),
        VelocityF(Vec2F::ZERO),
        Sprite {
            color: Color::srgb(0.45, 0.78, 0.95),
            custom_size: Some(Vec2::new(48.0, 48.0)),
            ..default()
        },
        Transform::default(),
    ));

    for wall in arena_walls() {
        let size_cm = (
            (wall.rect.max.x - wall.rect.min.x).to_num::<f32>(),
            (wall.rect.max.y - wall.rect.min.y).to_num::<f32>(),
        );
        let center = (
            (wall.rect.min.x + wall.rect.max.x).to_num::<f32>() * 0.5,
            (wall.rect.min.y + wall.rect.max.y).to_num::<f32>() * 0.5,
        );
        commands.spawn((
            wall,
            Sprite {
                color: Color::srgb(0.18, 0.18, 0.22),
                custom_size: Some(Vec2::new(size_cm.0, size_cm.1)),
                ..default()
            },
            Transform::from_xyz(center.0, center.1, -1.0),
        ));
    }

    // Frame counter HUD pinned to the upper-left, world-space (matches
    // the existing debug overlay convention in the app crate). Cycle 2
    // will replace this with a real scrub bar.
    commands.spawn((
        Text2d::new(String::new()),
        TextFont {
            font_size: 14.0,
            ..default()
        },
        TextColor(Color::srgb(0.95, 0.85, 0.40)),
        Transform::from_xyz(-440.0, 340.0, 100.0),
        FrameCounterText,
    ));
}

/// Same boomerang-visual attach as the app crate's `ensure_boomerang_visuals`.
type NewBoomerangs<'w, 's> =
    Query<'w, 's, (Entity, &'static PositionF), (With<Boomerang>, Without<Sprite>)>;

fn ensure_boomerang_visuals(mut commands: Commands, q: NewBoomerangs) {
    let size_px = (BOOMERANG_HALF_EXTENT_CM * 2) as f32;
    for (entity, pos) in &q {
        let (x, y) = pos.0.to_f32();
        commands.entity(entity).insert((
            Sprite {
                color: Color::srgb(0.92, 0.85, 0.40),
                custom_size: Some(Vec2::new(size_px, size_px)),
                ..default()
            },
            Transform::from_xyz(x, y, 0.5),
        ));
    }
}

fn update_frame_counter(
    frame: Res<sim::FrameCount>,
    total: Res<TotalFrames>,
    controls: Res<ViewerControls>,
    snaps: Res<SnapshotBuffer>,
    mut q: Query<&mut Text2d, With<FrameCounterText>>,
) {
    let Ok(mut text) = q.single_mut() else {
        return;
    };
    let mode = match (controls.paused, controls.seek_target) {
        (_, Some(t)) => format!("seek→{t}"),
        (true, None) => "paused".into(),
        (false, None) => "play".into(),
    };
    text.0 = format!(
        "frame {} / {}  [{}]  snaps={}",
        frame.0,
        total.0,
        mode,
        snaps.entries.len(),
    );
}

/// Cycle-2a auto-quit policy: only exit when the replay is exhausted
/// AND the user is not paused. A paused viewer at the end of the
/// stream stays open so the operator can scrub backward; pressing
/// Space to resume from the last frame is the explicit quit.
fn quit_when_replay_finished(
    playback: Res<ReplayPlayback>,
    controls: Res<ViewerControls>,
    mut exit: MessageWriter<AppExit>,
) {
    if controls.paused || controls.seek_target.is_some() {
        return;
    }
    if playback.cursor >= playback.replay.inputs.len() {
        exit.write(AppExit::Success);
    }
}

// ---- Phase 14 cycle 2a: snapshot + scrub ----

/// `Update` system: snapshots the sim every `SNAPSHOT_INTERVAL` frames
/// during forward playback. Skips capture while a seek is in flight —
/// the snapshot we'd take mid-replay-forward would just duplicate one
/// already in the buffer (the seek itself was launched from one).
fn capture_snapshot(world: &mut World) {
    let frame = world.resource::<sim::FrameCount>().0;
    let controls = *world.resource::<ViewerControls>();
    if controls.seek_target.is_some() {
        return;
    }
    if frame == 0 || !frame.is_multiple_of(SNAPSHOT_INTERVAL) {
        return;
    }

    // Skip if we already captured this exact frame on a prior tick
    // (Update can run multiple times per sim tick at high refresh
    // rates — guard via the buffer's tail-frame).
    let already_captured = world
        .resource::<SnapshotBuffer>()
        .entries
        .last()
        .is_some_and(|s| s.frame == frame);
    if already_captured {
        return;
    }

    let snap = SimSnapshot::capture(world);
    world.resource_mut::<SnapshotBuffer>().entries.push(snap);
}

/// `Update` system: reads keyboard input and translates it into
/// [`ViewerControls`] changes. Bindings:
///
/// | Key       | Action                                                  |
/// |-----------|---------------------------------------------------------|
/// | Space     | Toggle pause / play                                     |
/// | →         | Step forward 1 frame (auto-pauses)                      |
/// | ←         | Step backward 1 frame (auto-pauses; uses snapshot)      |
/// | Home      | Seek to frame 0                                         |
/// | End       | Seek to last frame                                      |
fn handle_keyboard_controls(
    keys: Res<ButtonInput<KeyCode>>,
    frame: Res<sim::FrameCount>,
    total: Res<TotalFrames>,
    mut controls: ResMut<ViewerControls>,
) {
    let current = frame.0;
    if keys.just_pressed(KeyCode::Space) {
        controls.paused = !controls.paused;
        // Resuming play cancels any in-flight seek.
        if !controls.paused {
            controls.seek_target = None;
        }
    }
    if keys.just_pressed(KeyCode::ArrowRight) {
        controls.paused = true;
        controls.seek_target = Some(current.saturating_add(1).min(total.0.saturating_sub(1)));
    }
    if keys.just_pressed(KeyCode::ArrowLeft) {
        controls.paused = true;
        controls.seek_target = Some(current.saturating_sub(1));
    }
    if keys.just_pressed(KeyCode::Home) {
        controls.paused = true;
        controls.seek_target = Some(0);
    }
    if keys.just_pressed(KeyCode::End) {
        controls.paused = true;
        controls.seek_target = Some(total.0.saturating_sub(1));
    }
    if keys.just_pressed(KeyCode::KeyH) {
        controls.show_overlay = !controls.show_overlay;
    }
}

/// `Update` system: drives the seek state machine. Three phases:
///
/// 1. **Backward seek launch** (`target < current`): restore the
///    nearest-prior snapshot, reset the replay-cursor to that
///    snapshot's frame, truncate snapshots after the seek target so
///    forward play re-populates them. Then fall through to phase 2.
/// 2. **Fast-forward** (`current < target`): unpause `Time<Virtual>` at
///    [`SEEK_SPEED`]× so bevy_ggrs's accumulator chews through the
///    remaining ticks across the next Update or two.
/// 3. **Arrival** (`current == target`): clear `seek_target`, return
///    `Time<Virtual>` to its baseline (paused if the user paused,
///    1.0× otherwise).
///
/// Pause-without-seek is handled at the bottom — `Time<Virtual>` gets
/// paused so bevy_ggrs's accumulator stops filling.
fn drive_seek(world: &mut World) {
    let controls = *world.resource::<ViewerControls>();
    let current = world.resource::<sim::FrameCount>().0;

    if let Some(target) = controls.seek_target {
        // Phase 1: backward — restore from snapshot if needed.
        if target < current {
            // Truncate any snapshots at-or-after the seek target so a
            // later forward play re-populates them.
            world
                .resource_mut::<SnapshotBuffer>()
                .entries
                .retain(|s| s.frame < target.max(1));

            let nearest = world
                .resource::<SnapshotBuffer>()
                .nearest_before(target)
                .cloned();
            if let Some(snap) = nearest {
                snap.restore(world);
                world.resource_mut::<ReplayPlayback>().cursor = snap.frame as usize;
            } else {
                // No snapshot at-or-before target — must be a seek
                // toward frame 0 or 1. Cycle 2a doesn't snapshot
                // frame 0 (the initial state is what `setup` spawned),
                // so the only fallback is to ask the user to restart
                // the viewer. Surface a one-line warning instead of
                // silently doing nothing.
                bevy::log::warn!(
                    target: "two_top::replay_viewer",
                    target_frame = target,
                    current_frame = current,
                    "backward seek before first snapshot — restart viewer to scrub to frame 0",
                );
                world.resource_mut::<ViewerControls>().seek_target = None;
                return;
            }
        }

        let current_after_restore = world.resource::<sim::FrameCount>().0;

        if current_after_restore == target {
            // Arrival.
            world.resource_mut::<ViewerControls>().seek_target = None;
            apply_play_pause(world, controls.paused);
        } else {
            // Phase 2: fast-forward.
            let mut vt = world.resource_mut::<Time<Virtual>>();
            vt.unpause();
            vt.set_relative_speed(SEEK_SPEED);
        }
    } else {
        // No active seek — honor the play/pause intent.
        apply_play_pause(world, controls.paused);
    }
}

fn apply_play_pause(world: &mut World, paused: bool) {
    let mut vt = world.resource_mut::<Time<Virtual>>();
    if paused {
        vt.pause();
    } else {
        vt.unpause();
        vt.set_relative_speed(1.0);
    }
}

/// Charcoal Line from `assets/palettes/two_top_16.gpl` — palette index
/// 3, used as the wireframe stroke for the diagnostic overlay so the
/// hitbox/vector annotations sit visibly on top of the Bone Cathedral
/// rendering without competing with the gameplay sprites' brighter
/// hues.
const CHARCOAL_LINE: Color = Color::srgb(57.0 / 255.0, 52.0 / 255.0, 66.0 / 255.0);

/// Hot Bone (palette index 7) — used as the velocity-vector tip so
/// motion direction reads at a glance even at low speeds.
const HOT_BONE: Color = Color::srgb(1.0, 241.0 / 255.0, 194.0 / 255.0);

/// Velocity-vector visual scale: pixels of vector length per unit of
/// `cm/tick` velocity. Picked so a typical walk-speed velocity
/// (~13 cm/tick) renders as a ~52-pixel vector — visible at the
/// 960x720 viewer resolution without occluding the player sprite.
const VELOCITY_VECTOR_SCALE: f32 = 4.0;

/// `Update` system: render diagnostic overlay (player + boomerang
/// AABBs + velocity vectors) when the user has toggled it on. Uses
/// the `Time<Real>`-driven render Transform (set by
/// `render::sync_transforms_from_sim` earlier in Update) so the
/// wireframes track the rendered position even between sim ticks
/// during paused-but-still-rendering frames.
fn draw_diagnostic_overlay(
    controls: Res<ViewerControls>,
    mut gizmos: Gizmos,
    players: Query<(&Transform, &VelocityF), With<Player>>,
    boomerangs: Query<(&Transform, &VelocityF), With<Boomerang>>,
) {
    if !controls.show_overlay {
        return;
    }
    let player_size = Vec2::splat((PLAYER_HALF_EXTENT_CM * 2) as f32);
    let boomerang_size = Vec2::splat((BOOMERANG_HALF_EXTENT_CM * 2) as f32);

    for (xform, vel) in &players {
        let center = xform.translation.truncate();
        gizmos.rect_2d(
            Isometry2d::from_translation(center),
            player_size,
            CHARCOAL_LINE,
        );
        draw_velocity_vector(&mut gizmos, center, vel.0);
    }
    for (xform, vel) in &boomerangs {
        let center = xform.translation.truncate();
        gizmos.rect_2d(
            Isometry2d::from_translation(center),
            boomerang_size,
            CHARCOAL_LINE,
        );
        draw_velocity_vector(&mut gizmos, center, vel.0);
    }
}

fn draw_velocity_vector(gizmos: &mut Gizmos, center: Vec2, vel: fixed_math::Vec2F) {
    let (vx, vy) = vel.to_f32();
    if vx == 0.0 && vy == 0.0 {
        return;
    }
    let v = Vec2::new(vx, vy) * VELOCITY_VECTOR_SCALE;
    let tip = center + v;
    gizmos.line_2d(center, tip, CHARCOAL_LINE);
    // A small Hot-Bone arrowhead at the tip so the direction reads
    // even when the vector is short.
    let perp = Vec2::new(-v.y, v.x).normalize_or_zero() * 4.0;
    let back = tip - v.normalize_or_zero() * 6.0;
    gizmos.line_2d(tip, back + perp, HOT_BONE);
    gizmos.line_2d(tip, back - perp, HOT_BONE);
}
