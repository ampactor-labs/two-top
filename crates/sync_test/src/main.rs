use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy_ggrs::GgrsPlugin;
use bevy_ggrs::prelude::*;
use core::time::Duration;
use fixed_math::Vec2F;
use sim::{
    DefaultInputsPlugin, GgrsCfg, Player, PlayerInput, PositionF, SimPlugin, SynthesizedInputs,
    VelocityF,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let frames: u32 = arg_value(&args, "--frames").unwrap_or(600);
    let check_distance: usize = arg_value(&args, "--check-distance").unwrap_or(7);

    println!("sync_test: frames={frames} check_distance={check_distance}");

    let mut sb = SessionBuilder::<GgrsCfg>::new()
        .with_num_players(2)
        .expect("with_num_players(2) accepted")
        .with_check_distance(check_distance)
        .with_input_delay(2);
    for i in 0..2 {
        sb = sb
            .add_player(PlayerType::Local, i)
            .expect("add_player accepted");
    }
    let session = sb.start_synctest_session().expect("start_synctest_session");

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    // Force Time<Real> to advance by exactly one tick per `update()` so the
    // GGRS driver advances exactly one frame per call. Without this, frames
    // are gated on real wall-clock and 600 update() calls in a tight loop
    // produce zero ggrs frames.
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
        1.0 / sim::TICK_HZ as f64,
    )));
    app.add_plugins(GgrsPlugin::<GgrsCfg>::default());
    app.add_plugins(SimPlugin);
    app.add_plugins(sim::InfiniteRoundPlugin);
    app.add_plugins(DefaultInputsPlugin);
    app.insert_resource(Session::SyncTest(session));

    // Spawn one entity per player handle.
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

    for f in 0..frames {
        // Alternate stick direction every 60 frames so the entities move,
        // ensuring SyncTest checksums see real state changes.
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
    for (handle, xb, yb) in &positions {
        println!("sync_test: handle={handle} pos.x.bits={xb:#010x} pos.y.bits={yb:#010x}");
    }

    println!("sync_test: completed {frames} frames without panic");
}

fn arg_value<T: std::str::FromStr>(args: &[String], flag: &str) -> Option<T> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
}
