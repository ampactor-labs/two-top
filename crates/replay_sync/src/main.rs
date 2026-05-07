//! Phase 4 skeleton. Loads a `.bmrg` replay, runs the sim under playback,
//! and dumps final entity state. Phase 5 extends this with per-frame
//! per-component checksum TSV output for the cross-platform diff harness.

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy_ggrs::prelude::*;
use bevy_ggrs::GgrsPlugin;
use core::time::Duration;
use fixed_math::Vec2F;
use replay::{decode, ReplayPlayback, ReplayPlaybackPlugin};
use sim::{GgrsCfg, Player, PositionF, SimPlugin, VelocityF};
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let Some(demo_path) = arg_value::<String>(&args, "--demo") else {
        eprintln!("usage: replay_sync --demo <path.bmrg> [--frames N]");
        return ExitCode::from(2);
    };
    let demo_path = PathBuf::from(demo_path);

    let bytes = match fs::read(&demo_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("replay_sync: failed to read {}: {}", demo_path.display(), e);
            return ExitCode::FAILURE;
        }
    };

    let replay = match decode(&bytes) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("replay_sync: failed to decode: {e}");
            return ExitCode::FAILURE;
        }
    };

    let frames: u32 = arg_value(&args, "--frames").unwrap_or(replay.header.frame_count);
    println!(
        "replay_sync: demo={} frames={frames} format_v{} sim_v{}",
        demo_path.display(),
        replay.header.format_version,
        replay.header.sim_version,
    );

    // Build SyncTest session matching the replay's player count.
    let mut sb = SessionBuilder::<GgrsCfg>::new()
        .with_num_players(replay.header.num_players as usize)
        .expect("with_num_players")
        .with_check_distance(2)
        .with_input_delay(2);
    for i in 0..replay.header.num_players as usize {
        sb = sb.add_player(PlayerType::Local, i).expect("add_player");
    }
    let session = sb.start_synctest_session().expect("synctest");

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
        1.0 / sim::TICK_HZ as f64,
    )));
    app.add_plugins(GgrsPlugin::<GgrsCfg>::default());
    app.add_plugins(SimPlugin);
    app.add_plugins(ReplayPlaybackPlugin);
    app.insert_resource(Session::SyncTest(session));
    app.insert_resource(ReplayPlayback::new(replay));

    for handle in 0..2usize {
        app.world_mut().spawn((
            Player { handle },
            PositionF(Vec2F::ZERO),
            VelocityF(Vec2F::ZERO),
        ));
    }

    for _ in 0..frames {
        app.update();
    }

    let world = app.world_mut();
    let mut positions: Vec<(usize, i32, i32)> = world
        .query::<(&Player, &PositionF)>()
        .iter(world)
        .map(|(p, pos)| (p.handle, pos.0.x.to_bits(), pos.0.y.to_bits()))
        .collect();
    positions.sort_by_key(|(h, _, _)| *h);
    for (handle, xb, yb) in &positions {
        println!("replay_sync: handle={handle} pos.x.bits={xb:#010x} pos.y.bits={yb:#010x}");
    }

    println!("replay_sync: finished {frames} frames");
    ExitCode::SUCCESS
}

fn arg_value<T: std::str::FromStr>(args: &[String], flag: &str) -> Option<T> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
}
