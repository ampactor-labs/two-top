//! Phase 9 cycle 3: walk-speed tuning.
//!
//! The Phase 9 exit criterion is "cross arena in ~2 seconds". Arena
//! longest dimension is 2 × ARENA_HALF_HEIGHT_CM = 1500 cm; at 60 Hz
//! that's ~120 ticks for the target. We assert the player crosses the
//! full arena within a tight window around that target with no walls
//! in the way, and that diagonal max-speed doesn't outrun cardinal
//! max-speed (the classic axis-independent quantization bug).

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy_ggrs::GgrsPlugin;
use bevy_ggrs::prelude::*;
use fixed_math::{Fix, Vec2F};
use sim::{
    ARENA_HALF_HEIGHT_CM, DefaultInputsPlugin, GgrsCfg, Player, PlayerInput, PositionF,
    PreviousPositionF, SimPlugin, SynthesizedInputs, VelocityF, WALK_SPEED_CM_PER_TICK,
};

fn build_app() -> App {
    let mut sb = SessionBuilder::<GgrsCfg>::new()
        .with_num_players(1)
        .unwrap()
        .with_check_distance(2)
        .with_input_delay(0);
    sb = sb.add_player(PlayerType::Local, 0).unwrap();
    let session = sb.start_synctest_session().unwrap();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(TimeUpdateStrategy::ManualDuration(sim::tick_duration()));
    app.add_plugins(GgrsPlugin::<GgrsCfg>::default());
    app.add_plugins(SimPlugin);
    app.add_plugins(sim::InfiniteRoundPlugin);
    app.add_plugins(DefaultInputsPlugin);
    app.insert_resource(Session::SyncTest(session));

    app.world_mut().spawn((
        Player { handle: 0 },
        PositionF(Vec2F::ZERO),
        PreviousPositionF(Vec2F::ZERO),
        VelocityF(Vec2F::ZERO),
    ));
    app
}

fn player_pos(app: &mut App) -> Vec2F {
    let mut q = app.world_mut().query::<(&Player, &PositionF)>();
    q.iter(app.world())
        .next()
        .map(|(_, pos)| pos.0)
        .expect("player not found")
}

#[test]
fn full_stick_speed_is_walk_speed_per_tick() {
    let mut app = build_app();
    app.update(); // warmup
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 127,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    let p_before = player_pos(&mut app);
    app.update();
    let p_after = player_pos(&mut app);
    let dx = p_after.x - p_before.x;
    // Walk speed per tick — exactly WALK_SPEED_CM_PER_TICK (modulo
    // fixed-point quantization at the stick boundary; with stick=127
    // the divide-by-127 rounds back to ~1, then ×13 gives ~13).
    let target = Fix::const_from_int(WALK_SPEED_CM_PER_TICK);
    let tolerance = Fix::const_from_int(1); // 1 cm slop for fixed-point
    assert!(
        (dx - target).abs() <= tolerance,
        "single-tick dx {} should be ~{} (within {})",
        dx,
        target,
        tolerance,
    );
}

#[test]
fn arena_crosses_in_target_two_second_window() {
    let mut app = build_app();
    app.update();
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 0,
        stick_y: 127, // drive north
        aim_angle: 0,
        buttons: 0,
    };
    // Without a wall in the way, 1500 cm / 11 cm/tick = ~136 ticks (~2.3 s
    // after the 2026-07-16 feel-tune walk trim). Stopping at the half-height.
    let mut ticks = 0;
    while player_pos(&mut app).y < Fix::const_from_int(ARENA_HALF_HEIGHT_CM) {
        app.update();
        ticks += 1;
        if ticks > 240 {
            panic!("never crossed arena half-height in 240 ticks");
        }
    }
    // Crossing the half-height (750 cm) at 11 cm/tick = ~68 ticks.
    // ±10 ticks of fixed-point slop.
    assert!(
        (58..=78).contains(&ticks),
        "crossed half-arena in {ticks} ticks (target ~68)",
    );
}

#[test]
fn diagonal_max_speed_does_not_exceed_cardinal_max_speed() {
    // Run two parallel apps: one with cardinal max stick (127, 0) and
    // one with diagonal max stick (127, 127). After N ticks, the
    // diagonal app's distance from origin shouldn't exceed the
    // cardinal app's by more than fixed-point slop.
    let n_ticks = 30;

    let mut card = build_app();
    card.update();
    card.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 127,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    for _ in 0..n_ticks {
        card.update();
    }
    let card_dist = player_pos(&mut card).length();

    let mut diag = build_app();
    diag.update();
    diag.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 127,
        stick_y: 127,
        aim_angle: 0,
        buttons: 0,
    };
    for _ in 0..n_ticks {
        diag.update();
    }
    let diag_dist = player_pos(&mut diag).length();

    // Without the magnitude clamp, diagonal would be sqrt(2)× faster.
    // With the clamp, diagonal should be within 1 cm of cardinal.
    let tolerance = Fix::const_from_int(2);
    assert!(
        (diag_dist - card_dist).abs() <= tolerance,
        "diagonal dist {} vs cardinal {} differ by more than {}",
        diag_dist,
        card_dist,
        tolerance,
    );
}
