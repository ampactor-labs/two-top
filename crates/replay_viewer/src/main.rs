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

/// Onscreen width of the scrub bar in window pixels. Source asset is
/// 192 px wide, so 4x scale gives 768 px which fits the 960-wide
/// viewer with comfortable margins.
const SCRUB_BAR_WIDTH: f32 = 768.0;
const SCRUB_BAR_HEIGHT: f32 = 48.0;
/// Y-coordinate of the scrub bar relative to the camera. Negative
/// because the camera centers at (0,0) and the bar lives at the bottom
/// of the 720-tall window — `-720/2 + margin`.
const SCRUB_BAR_Y: f32 = -320.0;
const SCRUB_BAR_HANDLE_W: f32 = 16.0;
const SCRUB_BAR_HANDLE_H: f32 = 64.0;
/// Hot Bone color from the locked palette — used as the played-portion
/// fill that grows leftward-to-rightward across the track.
const PALETTE_HOT_BONE: Color = Color::srgb(1.0, 241.0 / 255.0, 194.0 / 255.0);

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
        .add_systems(Startup, (setup, spawn_scrub_bar, spawn_frame_step_and_speed))
        .add_systems(
            Update,
            (
                ensure_boomerang_visuals,
                handle_keyboard_controls,
                handle_scrub_bar_click,
                handle_frame_step_click,
                handle_speed_pip_click,
                drive_seek,
                capture_snapshot,
                update_frame_counter,
                update_scrub_bar,
                update_speed_pips,
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

/// Marker components for the scrub-bar entities. The viewer stores
/// the entity handles via these queries rather than a dedicated
/// resource so a future cycle that swaps the visuals (different
/// asset, different layout) only has to touch the system bodies.
#[derive(Component)]
struct ScrubBarTrack;

#[derive(Component)]
struct ScrubBarFill;

#[derive(Component)]
struct ScrubBarHandle;

/// Frame-step button — the `+/- 1 frame` value indicates which
/// direction the click should seek. Cycle 2b.3 spawns one of each
/// flanking the scrub bar.
#[derive(Component, Clone, Copy)]
struct FrameStepButton {
    delta: i32,
}

/// Speed-pip — its `idx` matches the index into [`SPEED_PRESETS`].
/// Click-tested by [`handle_speed_pip_click`]; rendered with a Hot-Bone
/// tint when `idx == ViewerControls.speed_idx`, Bone otherwise.
#[derive(Component, Clone, Copy)]
struct SpeedPip {
    idx: usize,
}

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
#[derive(Resource, Clone, Copy)]
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
    /// Index into [`SPEED_PRESETS`] selecting the active playback
    /// speed (0.25x, 0.5x, 1x, 2x, 4x). Drives
    /// [`Time<Virtual>::set_relative_speed`] when neither a seek nor
    /// pause is in flight.
    speed_idx: usize,
}

impl Default for ViewerControls {
    fn default() -> Self {
        Self {
            paused: false,
            seek_target: None,
            show_overlay: false,
            // Index 2 = 1.0x — natural realtime starting speed.
            speed_idx: 2,
        }
    }
}

/// Playback speed presets per BUILD_PLAN § Phase 14 Produces:
/// "Speed selector (0.25x, 0.5x, 1x, 2x, 4x)".
const SPEED_PRESETS: [f32; 5] = [0.25, 0.5, 1.0, 2.0, 4.0];

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
    // the existing debug overlay convention in the app crate).
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

/// Cycle 2b.2 — load the scrub-bar pixel art and spawn the
/// track + fill + handle entities. Loaded in a separate Startup
/// system so the AssetServer dependency stays out of the gameplay
/// `setup` (which is meant to mirror the live app's setup signature).
///
/// Layout (centered horizontally on the camera, pinned at
/// `SCRUB_BAR_Y` from camera center):
///
/// ```text
///       [-384 .. +384] ← track (768 px wide)
///       hot-bone fill grows from -384 rightward
///       handle hovers above the current-frame x position
/// ```
fn spawn_scrub_bar(mut commands: Commands, asset_server: Res<AssetServer>) {
    let track_image = asset_server.load("hud/scrub_bar_track.png");
    let handle_image = asset_server.load("hud/scrub_bar_handle.png");

    // Track: full-width, anchored at center.
    commands.spawn((
        Sprite {
            image: track_image,
            custom_size: Some(Vec2::new(SCRUB_BAR_WIDTH, SCRUB_BAR_HEIGHT)),
            ..default()
        },
        Transform::from_xyz(0.0, SCRUB_BAR_Y, 90.0),
        ScrubBarTrack,
    ));

    // Played-portion fill — Hot Bone color rectangle, anchored at the
    // LEFT edge so its width can scale with progress without the
    // sprite's center drifting. Z just above the track so the fill
    // overlays the dark interior cleanly. In bevy 0.18 the anchor is
    // a separate component required by Sprite (not a Sprite field).
    let track_left = -SCRUB_BAR_WIDTH / 2.0;
    commands.spawn((
        Sprite {
            color: PALETTE_HOT_BONE,
            custom_size: Some(Vec2::new(0.0, SCRUB_BAR_HEIGHT - 8.0)),
            ..default()
        },
        bevy::sprite::Anchor::CENTER_LEFT,
        Transform::from_xyz(track_left + 4.0, SCRUB_BAR_Y, 91.0),
        ScrubBarFill,
    ));

    // Handle: bone-fang vertical needle. Z above the fill so it always
    // reads as the cursor.
    commands.spawn((
        Sprite {
            image: handle_image,
            custom_size: Some(Vec2::new(SCRUB_BAR_HANDLE_W, SCRUB_BAR_HANDLE_H)),
            ..default()
        },
        Transform::from_xyz(track_left, SCRUB_BAR_Y, 92.0),
        ScrubBarHandle,
    ));
}

/// Cycle 2b.3 — frame-step buttons + speed pips. Frame-step buttons
/// are sliced from a single 32x16 strip via the Sprite::rect crop:
/// cell 0 (left half) = back, cell 1 (right half) = forward. Speed
/// pips render as five colored squares in code (no asset).
fn spawn_frame_step_and_speed(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
) {
    let buttons_image = asset_server.load("hud/frame_step_buttons.png");

    // Both buttons share the same source texture; rect crops select
    // the two 16x16 cells from the 32x16 strip.
    let button_y = SCRUB_BAR_Y;
    let button_size = Vec2::splat(48.0);
    let track_left = -SCRUB_BAR_WIDTH / 2.0;
    let track_right = SCRUB_BAR_WIDTH / 2.0;

    commands.spawn((
        Sprite {
            image: buttons_image.clone(),
            custom_size: Some(button_size),
            rect: Some(Rect::new(0.0, 0.0, 16.0, 16.0)),
            ..default()
        },
        Transform::from_xyz(track_left - 36.0, button_y, 92.0),
        FrameStepButton { delta: -1 },
    ));
    commands.spawn((
        Sprite {
            image: buttons_image,
            custom_size: Some(button_size),
            rect: Some(Rect::new(16.0, 0.0, 32.0, 16.0)),
            ..default()
        },
        Transform::from_xyz(track_right + 36.0, button_y, 92.0),
        FrameStepButton { delta: 1 },
    ));

    // Speed pips — clustered above the scrub bar, centered.
    // Each pip is a 16x16 colored square. Spaced 24 px apart
    // horizontally so the cluster spans 4*24 = 96 px.
    let pip_y = SCRUB_BAR_Y + 36.0;
    let pip_spacing = 24.0;
    let cluster_left = -((SPEED_PRESETS.len() as f32 - 1.0) * pip_spacing) / 2.0;
    for (i, _) in SPEED_PRESETS.iter().enumerate() {
        let x = cluster_left + i as f32 * pip_spacing;
        commands.spawn((
            Sprite {
                color: PALETTE_BONE,
                custom_size: Some(Vec2::splat(14.0)),
                ..default()
            },
            Transform::from_xyz(x, pip_y, 92.0),
            SpeedPip { idx: i },
        ));
    }
}

/// Bone color (palette idx 6) — used as the dim/inactive speed pip
/// fill so the active pip's Hot-Bone tint reads as the selection.
const PALETTE_BONE: Color = Color::srgb(203.0 / 255.0, 190.0 / 255.0, 148.0 / 255.0);

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
    let speed = SPEED_PRESETS[world.resource::<ViewerControls>().speed_idx];
    let mut vt = world.resource_mut::<Time<Virtual>>();
    if paused {
        vt.pause();
    } else {
        vt.unpause();
        vt.set_relative_speed(speed);
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

/// Cycle 2b.2 — update the scrub-bar fill width + handle x-position
/// each frame to reflect the current sim frame's progress through the
/// replay. Runs in `Update` after the sim has potentially advanced so
/// the visual is one tick fresh.
fn update_scrub_bar(
    frame: Res<sim::FrameCount>,
    total: Res<TotalFrames>,
    mut fills: Query<&mut Sprite, (With<ScrubBarFill>, Without<ScrubBarHandle>)>,
    mut handles: Query<&mut Transform, With<ScrubBarHandle>>,
) {
    let progress = if total.0 == 0 {
        0.0
    } else {
        (frame.0 as f32 / total.0 as f32).clamp(0.0, 1.0)
    };
    let inner_width = SCRUB_BAR_WIDTH - 8.0;
    let track_left = -SCRUB_BAR_WIDTH / 2.0;

    if let Ok(mut fill) = fills.single_mut() {
        let target_w = inner_width * progress;
        if let Some(size) = fill.custom_size.as_mut() {
            size.x = target_w;
        }
    }

    if let Ok(mut handle_xform) = handles.single_mut() {
        handle_xform.translation.x = track_left + 4.0 + inner_width * progress;
    }
}

/// Cycle 2b.3 — drive each frame-step button's hit-test. Clicking
/// the back/forward chevron sets the seek target one frame in that
/// direction (auto-pauses).
fn handle_frame_step_click(
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    step_buttons: Query<(&Transform, &FrameStepButton, &Sprite)>,
    frame: Res<sim::FrameCount>,
    total: Res<TotalFrames>,
    mut controls: ResMut<ViewerControls>,
) {
    if !buttons.just_pressed(MouseButton::Left) {
        return;
    }
    let Some(world_pos) = world_cursor(&windows, &cameras) else {
        return;
    };
    for (xform, button, sprite) in &step_buttons {
        let half = sprite
            .custom_size
            .unwrap_or(Vec2::splat(48.0))
            / 2.0;
        let center = xform.translation.truncate();
        if (world_pos.x - center.x).abs() <= half.x
            && (world_pos.y - center.y).abs() <= half.y
        {
            let next = (frame.0 as i64 + button.delta as i64)
                .clamp(0, total.0.saturating_sub(1) as i64) as u32;
            controls.paused = true;
            controls.seek_target = Some(next);
            return;
        }
    }
}

/// Cycle 2b.3 — drive each speed-pip's hit-test. Clicking sets the
/// active speed index; the actual `Time<Virtual>::set_relative_speed`
/// happens in [`drive_seek`]'s play branch on the next tick.
fn handle_speed_pip_click(
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    pips: Query<(&Transform, &SpeedPip, &Sprite)>,
    mut controls: ResMut<ViewerControls>,
) {
    if !buttons.just_pressed(MouseButton::Left) {
        return;
    }
    let Some(world_pos) = world_cursor(&windows, &cameras) else {
        return;
    };
    for (xform, pip, sprite) in &pips {
        let half = sprite
            .custom_size
            .unwrap_or(Vec2::splat(14.0))
            / 2.0;
        let center = xform.translation.truncate();
        if (world_pos.x - center.x).abs() <= half.x
            && (world_pos.y - center.y).abs() <= half.y
        {
            controls.speed_idx = pip.idx;
            return;
        }
    }
}

/// Cycle 2b.3 — update each speed pip's color tint each frame so the
/// active pip reads as Hot Bone and the others stay dim Bone. Also
/// applies the active speed to `Time<Virtual>` when the viewer is in
/// normal play (not paused, no seek). Pause / seek already drive
/// `Time<Virtual>` themselves and would conflict with a per-frame
/// speed reset here.
fn update_speed_pips(
    controls: Res<ViewerControls>,
    mut pips: Query<(&SpeedPip, &mut Sprite)>,
    mut vt: ResMut<Time<Virtual>>,
) {
    for (pip, mut sprite) in &mut pips {
        sprite.color = if pip.idx == controls.speed_idx {
            PALETTE_HOT_BONE
        } else {
            PALETTE_BONE
        };
    }
    if !controls.paused && controls.seek_target.is_none() {
        vt.set_relative_speed(SPEED_PRESETS[controls.speed_idx]);
    }
}

/// Shared cursor → world helper. Returns `None` when no window /
/// cursor / camera is currently available (e.g. during the first
/// frame before the window is fully initialized).
fn world_cursor(
    windows: &Query<&Window>,
    cameras: &Query<(&Camera, &GlobalTransform)>,
) -> Option<Vec2> {
    let window = windows.single().ok()?;
    let cursor = window.cursor_position()?;
    let (camera, cam_xform) = cameras.single().ok()?;
    camera.viewport_to_world_2d(cam_xform, cursor).ok()
}

/// Cycle 2b.2 — translate left-mouse clicks within the scrub bar's
/// bounding box into a [`ViewerControls::seek_target`] update. The
/// hit-test runs in window-space (cursor coordinates → world-space
/// via the primary camera).
///
/// Click anywhere on the track jumps to that frame; release after a
/// click also fires (no drag-tracking yet — cycle 2b.3 may add it
/// alongside the frame-step buttons).
fn handle_scrub_bar_click(
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    total: Res<TotalFrames>,
    mut controls: ResMut<ViewerControls>,
) {
    if !buttons.just_pressed(MouseButton::Left) {
        return;
    }
    let Ok(window) = windows.single() else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let Ok((camera, cam_xform)) = cameras.single() else {
        return;
    };
    let Ok(world_pos) = camera.viewport_to_world_2d(cam_xform, cursor) else {
        return;
    };

    let in_bar_x = world_pos.x >= -SCRUB_BAR_WIDTH / 2.0
        && world_pos.x <= SCRUB_BAR_WIDTH / 2.0;
    let in_bar_y = (world_pos.y - SCRUB_BAR_Y).abs() <= SCRUB_BAR_HEIGHT / 2.0;
    if !(in_bar_x && in_bar_y) {
        return;
    }

    let inner_width = SCRUB_BAR_WIDTH - 8.0;
    let track_left = -SCRUB_BAR_WIDTH / 2.0 + 4.0;
    let progress = ((world_pos.x - track_left) / inner_width).clamp(0.0, 1.0);
    let target = (progress * total.0 as f32).round() as u32;
    controls.paused = true;
    controls.seek_target = Some(target.min(total.0.saturating_sub(1)));
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
