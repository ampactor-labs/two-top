//! Phase 7: `PreviousPositionF` and `snapshot_previous`.
//!
//! Property under test: at the end of frame N, `PreviousPositionF` equals
//! `PositionF` as it stood at the end of frame N-1. That's the contract
//! `sync_transforms_from_sim` relies on for interpolation — without this
//! single-frame lag the render layer can't lerp between the two and the
//! quad will jitter or skip.
//!
//! `snapshot_previous` must run *first* in `GgrsSchedule`, before any
//! system that mutates `PositionF`. We don't assert ordering directly
//! here (CONVENTIONS § Determinism Invariants requires it explicitly via
//! `.before()`/`.after()`), but the lag property would fail loudly if
//! the system were misordered.

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy_ggrs::GgrsPlugin;
use bevy_ggrs::prelude::*;
use fixed_math::Vec2F;
use sim::{
    DefaultInputsPlugin, GgrsCfg, Player, PlayerInput, PositionF, PreviousPositionF, SimPlugin,
    SynthesizedInputs, VelocityF,
};

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
    app.insert_resource(TimeUpdateStrategy::ManualDuration(sim::tick_duration()));
    app.add_plugins(GgrsPlugin::<GgrsCfg>::default());
    app.add_plugins(SimPlugin);
    app.add_plugins(DefaultInputsPlugin);
    app.insert_resource(Session::SyncTest(session));

    for handle in 0..2 {
        app.world_mut().spawn((
            Player { handle },
            PositionF(Vec2F::ZERO),
            PreviousPositionF(Vec2F::ZERO),
            VelocityF(Vec2F::ZERO),
        ));
    }
    app
}

fn read_pos(app: &mut App) -> (i32, i32) {
    let world = app.world_mut();
    let mut q = world.query::<(&Player, &PositionF)>();
    let (_, pos) = q
        .iter(world)
        .find(|(p, _)| p.handle == 0)
        .expect("player 0");
    (pos.0.x.to_bits(), pos.0.y.to_bits())
}

fn read_prev(app: &mut App) -> (i32, i32) {
    let world = app.world_mut();
    let mut q = world.query::<(&Player, &PreviousPositionF)>();
    let (_, prev) = q
        .iter(world)
        .find(|(p, _)| p.handle == 0)
        .expect("player 0 prev");
    (prev.0.x.to_bits(), prev.0.y.to_bits())
}

#[test]
fn previous_position_lags_position_by_one_frame() {
    let mut app = build_app();
    // Hold stick to a constant value so player_movement keeps writing new
    // positions; the lag property is observable only when PositionF is
    // actively changing.
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 100,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };

    let mut history: Vec<(i32, i32)> = Vec::new();
    for frame in 0..8u32 {
        app.update();
        let pos = read_pos(&mut app);
        let prev = read_prev(&mut app);

        let expected_prev = if frame == 0 {
            (0, 0) // PreviousPositionF was initialized to ZERO.
        } else {
            history[(frame - 1) as usize]
        };
        assert_eq!(
            prev, expected_prev,
            "frame {frame}: PreviousPositionF didn't match prior frame's PositionF",
        );
        history.push(pos);
    }
}
