//! Phase 9 cycle 2: AABB resolve_collision + wall_collision integration.
//!
//! Pure-function tests cover the resolution math against synthetic rects.
//! The integration test drives a Bevy app with `SimPlugin` so the
//! `wall_collision` system actually runs against arena walls and resolves
//! a player driven into them by `player_movement`.

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy_ggrs::GgrsPlugin;
use bevy_ggrs::prelude::*;
use core::time::Duration;
use fixed_math::{Fix, RectF, Vec2F};
use sim::{
    ARENA_HALF_WIDTH_CM, DefaultInputsPlugin, GgrsCfg, PLAYER_HALF_EXTENT_CM, Player, PlayerInput,
    PositionF, PreviousPositionF, SimPlugin, SynthesizedInputs, VelocityF, arena_walls,
    resolve_collision,
};

fn rect(min_x: i32, min_y: i32, max_x: i32, max_y: i32) -> RectF {
    RectF::from_min_max(Vec2F::from_cm(min_x, min_y), Vec2F::from_cm(max_x, max_y))
}

#[test]
fn resolve_returns_none_when_disjoint() {
    let p = rect(0, 0, 30, 30);
    let w = rect(100, 100, 150, 150);
    assert_eq!(resolve_collision(p, w), None);
}

#[test]
fn resolve_returns_none_when_touching_edge() {
    // Strict overlap: edge contact does not resolve.
    let p = rect(0, 0, 30, 30);
    let w = rect(30, 0, 60, 30);
    assert_eq!(resolve_collision(p, w), None);
}

#[test]
fn resolve_pushes_left_when_player_is_left_of_wall_center_with_smaller_x_overlap() {
    // Player overlaps wall by 10 in x, 30 in y → push along x.
    // Player center is left of wall center → push negative x.
    let p = rect(0, 0, 50, 50);
    let w = rect(40, -100, 100, 100);
    let push = resolve_collision(p, w).expect("overlap");
    assert_eq!(push.y, Fix::ZERO);
    assert!(push.x < Fix::ZERO);
    assert_eq!(push.x, Fix::const_from_int(-10));
}

#[test]
fn resolve_pushes_right_when_player_is_right_of_wall_center() {
    let p = rect(50, 0, 100, 50);
    let w = rect(0, -100, 60, 100);
    let push = resolve_collision(p, w).expect("overlap");
    assert_eq!(push.y, Fix::ZERO);
    assert!(push.x > Fix::ZERO);
    assert_eq!(push.x, Fix::const_from_int(10));
}

#[test]
fn resolve_pushes_up_when_smaller_y_overlap_and_player_above_wall_center() {
    // Player overlaps wall by 30 in x, 10 in y → push along y.
    // Player center above (smaller y because Bevy y-up in sim, but the
    // resolution doesn't care about Bevy convention — it just pushes
    // the player away from the wall's center).
    let p = rect(0, 0, 100, 50);
    let w = rect(-100, 40, 100, 100);
    let push = resolve_collision(p, w).expect("overlap");
    assert_eq!(push.x, Fix::ZERO);
    assert_eq!(push.y, Fix::const_from_int(-10)); // player center y < wall center y
}

#[test]
fn resolve_minimum_translation_picks_smaller_axis() {
    // Player overlaps wall by 5 in x, 25 in y. Resolution must pick x
    // (smaller overlap → smaller translation = "push out the easy way").
    let p = rect(0, 0, 50, 50);
    let w = rect(45, 25, 100, 100);
    let push = resolve_collision(p, w).expect("overlap");
    assert_eq!(push.y, Fix::ZERO);
    assert_eq!(push.x.abs(), Fix::const_from_int(5));
}

// ---- integration test ----

fn build_app() -> App {
    let mut sb = SessionBuilder::<GgrsCfg>::new()
        .with_num_players(2)
        .unwrap()
        .with_check_distance(2)
        .with_input_delay(0);
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
    app.add_plugins(sim::InfiniteRoundPlugin);
    app.add_plugins(DefaultInputsPlugin);
    app.insert_resource(Session::SyncTest(session));

    // Spawn a single player near the right wall — will hit the east
    // wall when input drives it further right.
    app.world_mut().spawn((
        Player { handle: 0 },
        PositionF(Vec2F::from_cm(ARENA_HALF_WIDTH_CM - 30, 0)),
        PreviousPositionF(Vec2F::from_cm(ARENA_HALF_WIDTH_CM - 30, 0)),
        VelocityF(Vec2F::ZERO),
    ));
    app.world_mut().spawn((
        Player { handle: 1 },
        PositionF(Vec2F::from_cm(0, 0)),
        PreviousPositionF(Vec2F::from_cm(0, 0)),
        VelocityF(Vec2F::ZERO),
    ));

    // Spawn the arena walls in their canonical order.
    for w in arena_walls() {
        app.world_mut().spawn(w);
    }
    app
}

fn player_pos(app: &mut App, handle: usize) -> Vec2F {
    let mut q = app.world_mut().query::<(&Player, &PositionF)>();
    q.iter(app.world())
        .find(|(p, _)| p.handle == handle)
        .map(|(_, pos)| pos.0)
        .expect("player not found")
}

#[test]
fn player_cannot_pass_through_east_wall() {
    let mut app = build_app();
    // Warmup tick (SyncTestSession's first update is no-op for inputs).
    app.update();
    // Drive stick fully right for 30 ticks. The player starts ~30 cm
    // from the east wall and shouldn't be able to move past it.
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 127,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    for _ in 0..30 {
        app.update();
    }
    let pos = player_pos(&mut app, 0);
    let max_allowed_x = Fix::const_from_int(ARENA_HALF_WIDTH_CM - PLAYER_HALF_EXTENT_CM);
    assert!(
        pos.x <= max_allowed_x,
        "player x {} exceeded arena boundary {} (cm)",
        pos.x,
        max_allowed_x,
    );
    // Y shouldn't have drifted.
    assert_eq!(pos.y, Fix::ZERO);
}

#[test]
fn player_resting_against_wall_does_not_keep_drifting() {
    let mut app = build_app();
    app.update();
    // Drive into the wall, then idle. Position should stop changing
    // once the resolution converges.
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 127,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    for _ in 0..30 {
        app.update();
    }
    let p1 = player_pos(&mut app, 0);
    // Hold stick still for 5 more ticks.
    for _ in 0..5 {
        app.update();
    }
    let p2 = player_pos(&mut app, 0);
    assert_eq!(p1, p2, "player drifted while held against wall");
}

#[test]
fn player_can_slide_along_wall() {
    let mut app = build_app();
    app.update();
    // Hard against the east wall, then push north-east — y component
    // should still take effect even though x is blocked.
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 127,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    for _ in 0..30 {
        app.update();
    }
    let resting = player_pos(&mut app, 0);
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 127,
        stick_y: 127,
        aim_angle: 0,
        buttons: 0,
    };
    for _ in 0..10 {
        app.update();
    }
    let after = player_pos(&mut app, 0);
    assert_eq!(after.x, resting.x, "x drifted while sliding north along wall");
    assert!(after.y > resting.y, "y didn't increase while sliding");
}
