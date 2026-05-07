//! Phase 4 integration: record a sim run, encode→decode the replay, then
//! play it back into a fresh sim and assert bit-identical end state.

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy_ggrs::prelude::*;
use bevy_ggrs::GgrsPlugin;
use core::time::Duration;
use fixed_math::Vec2F;
use replay::{
    decode, encode, RecordPlugin, RecordedInputs, Replay, ReplayHeader, ReplayPlayback,
    ReplayPlaybackPlugin, DEV_SIM_VERSION, FORMAT_VERSION, MAGIC,
};
use sim::{
    DefaultInputsPlugin, GgrsCfg, Player, PlayerInput, PositionF, SimPlugin, SynthesizedInputs,
    VelocityF,
};

const FRAMES: u32 = 60;

fn build_app() -> App {
    let mut sb = SessionBuilder::<GgrsCfg>::new()
        .with_num_players(2)
        .unwrap()
        .with_check_distance(2)
        .with_input_delay(2);
    for i in 0..2 {
        sb = sb.add_player(PlayerType::Local, i).unwrap();
    }
    let session = sb.start_synctest_session().unwrap();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
        1.0 / sim::TICK_HZ as f64,
    )));
    app.add_plugins(GgrsPlugin::<GgrsCfg>::default());
    app.add_plugins(SimPlugin);
    app.insert_resource(Session::SyncTest(session));

    app.world_mut().spawn((
        Player { handle: 0 },
        PositionF(Vec2F::ZERO),
        VelocityF(Vec2F::ZERO),
    ));
    app.world_mut().spawn((
        Player { handle: 1 },
        PositionF(Vec2F::ZERO),
        VelocityF(Vec2F::ZERO),
    ));
    app
}

fn final_positions(app: &mut App) -> Vec<(usize, i32, i32)> {
    let world = app.world_mut();
    let mut positions: Vec<(usize, i32, i32)> = world
        .query::<(&Player, &PositionF)>()
        .iter(world)
        .map(|(p, pos)| (p.handle, pos.0.x.to_bits(), pos.0.y.to_bits()))
        .collect();
    positions.sort_by_key(|(h, _, _)| *h);
    positions
}

#[test]
fn record_then_playback_reproduces_end_state() {
    // ---- Pass 1: record while running synthesized inputs ----
    let mut record_app = build_app();
    record_app.add_plugins(DefaultInputsPlugin);
    record_app.add_plugins(RecordPlugin);

    for f in 0..FRAMES {
        let dir = if (f / 10) % 2 == 0 { 80i8 } else { -80i8 };
        record_app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
            stick_x: dir,
            stick_y: 0,
            aim_angle: 0,
            buttons: 0,
        };
        record_app.update();
    }
    let original_positions = final_positions(&mut record_app);
    let recorded_frames = record_app
        .world()
        .resource::<RecordedInputs>()
        .frames
        .clone();
    assert!(
        !recorded_frames.is_empty(),
        "recording captured no frames"
    );

    let replay = Replay {
        header: ReplayHeader {
            magic: MAGIC,
            format_version: FORMAT_VERSION,
            sim_version: DEV_SIM_VERSION,
            seed: 0,
            num_players: 2,
            frame_rate: 60,
            frame_count: recorded_frames.len() as u32,
            recorded_at: 0,
            winner: None,
            player_handles: [None, None],
            arena_id: 0,
        },
        inputs: recorded_frames,
    };

    // Round-trip through the wire codec to exercise encode + decode end-to-end.
    let bytes = encode(&replay).expect("encode");
    let decoded = decode(&bytes).expect("decode");
    assert_eq!(decoded, replay);

    // ---- Pass 2: playback into a fresh sim ----
    let mut playback_app = build_app();
    playback_app.add_plugins(ReplayPlaybackPlugin);
    playback_app.insert_resource(ReplayPlayback::new(decoded));

    for _ in 0..FRAMES {
        playback_app.update();
    }
    let replay_positions = final_positions(&mut playback_app);

    assert_eq!(
        replay_positions, original_positions,
        "playback diverged: original={:?} playback={:?}",
        original_positions, replay_positions,
    );
}
