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
    BOOMERANG_HALF_EXTENT_CM, Boomerang, GgrsCfg, Player, PositionF, PreviousPositionF, SimPlugin,
    VelocityF, arena_walls,
};

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
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                ensure_boomerang_visuals,
                update_frame_counter,
                quit_when_replay_finished,
            ),
        )
        .run();

    ExitCode::SUCCESS
}

#[derive(Resource, Clone, Copy)]
struct TotalFrames(u32);

#[derive(Component)]
struct FrameCounterText;

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
    mut q: Query<&mut Text2d, With<FrameCounterText>>,
) {
    let Ok(mut text) = q.single_mut() else {
        return;
    };
    text.0 = format!("frame {} / {}", frame.0, total.0);
}

/// Auto-quit when the replay's input stream is exhausted. Cycle 2 will
/// replace this with a "loop / pause at end" toggle, but cycle 1's
/// goal is identical-playback verification — finishing cleanly at the
/// last frame is the simplest way to make that easy to compare against
/// `replay_sync --dump-state-at <last_frame>`.
fn quit_when_replay_finished(
    playback: Res<ReplayPlayback>,
    mut exit: MessageWriter<AppExit>,
) {
    if playback.cursor >= playback.replay.inputs.len() {
        exit.write(AppExit::Success);
    }
}
