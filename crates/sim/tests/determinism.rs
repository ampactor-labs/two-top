//! Phase 3 cross-platform determinism gate.
//!
//! Runs a 600-frame headless `SyncTestSession` with two players, alternating
//! synthesized stick direction every second, and asserts the final
//! `PositionF` state on every entity is bit-identical to a hand-locked
//! baseline. Captured from linux-x64 and frozen — any matrix target whose
//! sim produces different bits is non-deterministic and would desync in
//! networked play.
//!
//! Pairs with `fixed_math::determinism_locked_1000_rotations` (Phase 2):
//! that one verifies fixed-point trig deterministically; this one verifies
//! the full Bevy + bevy_ggrs + sim stack does too.

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy_ggrs::prelude::*;
use bevy_ggrs::GgrsPlugin;
use core::time::Duration;
use fixed_math::Vec2F;
use sim::{GgrsCfg, Player, PlayerInput, PositionF, SimPlugin, SynthesizedInputs, VelocityF};

fn build_app(check_distance: usize) -> App {
    let mut sb = SessionBuilder::<GgrsCfg>::new()
        .with_num_players(2)
        .unwrap()
        .with_check_distance(check_distance)
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

#[test]
fn determinism_locked_600_frame_synctest() {
    let mut app = build_app(7);
    for f in 0..600u32 {
        let dir = if (f / 60) % 2 == 0 { 100i8 } else { -100i8 };
        app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
            stick_x: dir,
            stick_y: 0,
            aim_angle: 0,
            buttons: 0,
        };
        app.update();
    }

    let world = app.world_mut();
    let mut positions: Vec<(usize, i32, i32)> = world
        .query::<(&Player, &PositionF)>()
        .iter(world)
        .map(|(p, pos)| (p.handle, pos.0.x.to_bits(), pos.0.y.to_bits()))
        .collect();
    positions.sort_by_key(|(h, _, _)| *h);

    // Locked baseline (linux-x64). Same bits expected on every supported
    // matrix target. Recapture and update only when the sim deliberately
    // changes determinism-affecting behavior.
    assert_eq!(
        positions,
        vec![(0, 0x0003efdf, 0x00000000), (1, 0x0003efdf, 0x00000000)],
        "matrix target produced non-baseline bits"
    );
}
