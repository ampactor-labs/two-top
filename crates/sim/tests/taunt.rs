//! SIM_VERSION 9 sweat batch: TAUNT (rooted flex → streak tier) and
//! SpawnGuard (anti-spawn-camp respawn protection that breaks on the
//! first offensive act). Pure-input tests run at check_distance 2 so
//! the mechanics also prove they resimulate cleanly; the guard tests
//! that pre-arrange world state out-of-band run at cd 0 (SyncTest
//! snapshot restore despawns out-of-band entities — see feel_batch.rs).

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy_ggrs::GgrsPlugin;
use bevy_ggrs::prelude::*;
use core::time::Duration;
use fixed_math::Vec2F;
use sim::{
    Boomerang, BoomerangState, CatchStreak, Dead, DefaultInputsPlugin, GgrsCfg, Player,
    PlayerInput, PositionF, PreviousPositionF, RESPAWN_FRAMES, SPAWN_GUARD_FRAMES, SimPlugin,
    SpawnGuard, SynthesizedInputs, TAUNT_FRAMES, Taunt, VelocityF,
};

fn build_two_player_app_cd(check_distance: usize) -> App {
    let mut sb = SessionBuilder::<GgrsCfg>::new()
        .with_num_players(2)
        .unwrap()
        .with_check_distance(check_distance)
        .with_input_delay(0);
    sb = sb.add_player(PlayerType::Local, 0).unwrap();
    sb = sb.add_player(PlayerType::Local, 1).unwrap();
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

    app.world_mut().spawn((
        Player { handle: 0 },
        PositionF(Vec2F::ZERO),
        PreviousPositionF(Vec2F::ZERO),
        VelocityF(Vec2F::ZERO),
    ));
    app.world_mut().spawn((
        Player { handle: 1 },
        PositionF(Vec2F::from_cm(0, 400)),
        PreviousPositionF(Vec2F::from_cm(0, 400)),
        VelocityF(Vec2F::ZERO),
    ));
    app
}

fn set_inputs(app: &mut App, input: PlayerInput) {
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = input;
}

fn player_pos(app: &mut App, handle: usize) -> Vec2F {
    let mut q = app.world_mut().query::<(&Player, &PositionF)>();
    q.iter(app.world())
        .find(|(p, _)| p.handle == handle)
        .map(|(_, pos)| pos.0)
        .unwrap()
}

fn taunt_of(app: &mut App, handle: usize) -> u32 {
    let mut q = app.world_mut().query::<(&Player, &Taunt)>();
    q.iter(app.world())
        .find(|(p, _)| p.handle == handle)
        .map(|(_, t)| t.0)
        .unwrap()
}

fn streak_of(app: &mut App, handle: usize) -> u32 {
    let mut q = app.world_mut().query::<(&Player, &CatchStreak)>();
    q.iter(app.world())
        .find(|(p, _)| p.handle == handle)
        .map(|(_, s)| s.0)
        .unwrap()
}

fn guard_of(app: &mut App, handle: usize) -> u32 {
    let mut q = app.world_mut().query::<(&Player, &SpawnGuard)>();
    q.iter(app.world())
        .find(|(p, _)| p.handle == handle)
        .map(|(_, g)| g.0)
        .unwrap()
}

fn is_dying(app: &mut App, handle: usize) -> bool {
    let mut q = app.world_mut().query::<(&Player, &Dead)>();
    q.iter(app.world())
        .find(|(p, _)| p.handle == handle)
        .map(|(_, d)| d.is_dying())
        .unwrap()
}

const STICK_FULL: i8 = 127;

// ---- Taunt ----

#[test]
fn taunt_roots_then_completion_feeds_streak_once() {
    let mut app = build_two_player_app_cd(2);
    app.update();

    // Hold TAUNT with a full deflected stick: the flex must root the
    // player even though the stick says "walk".
    set_inputs(
        &mut app,
        PlayerInput {
            stick_x: STICK_FULL,
            stick_y: 0,
            aim_angle: 0,
            buttons: PlayerInput::TAUNT_DOWN,
        },
    );
    // Two ticks: SynthesizedInputs reach the sim with a 1-tick latency.
    app.update();
    app.update();
    assert!(taunt_of(&mut app, 0) > 0, "taunt should have armed");
    let planted = player_pos(&mut app, 0);
    for _ in 0..10 {
        app.update();
    }
    assert_eq!(
        player_pos(&mut app, 0),
        planted,
        "a taunting player is rooted; the stick must not walk them"
    );

    // Let the flex complete (keep TAUNT held — a level signal can't
    // re-trigger without a fresh edge).
    for _ in 0..(TAUNT_FRAMES as usize + 4) {
        app.update();
    }
    assert_eq!(taunt_of(&mut app, 0), 0, "taunt should have completed");
    assert_eq!(
        streak_of(&mut app, 0),
        1,
        "a completed taunt feeds the streak exactly one tier"
    );
    // With the taunt over and the stick still deflected, walking resumes.
    let after = player_pos(&mut app, 0);
    for _ in 0..5 {
        app.update();
    }
    assert_ne!(
        player_pos(&mut app, 0),
        after,
        "movement must resume once the flex ends"
    );
}

#[test]
fn dash_cancels_taunt_without_reward() {
    let mut app = build_two_player_app_cd(2);
    app.update();

    set_inputs(
        &mut app,
        PlayerInput {
            stick_x: STICK_FULL,
            stick_y: 0,
            aim_angle: 0,
            buttons: PlayerInput::TAUNT_DOWN,
        },
    );
    app.update();
    app.update();
    assert!(taunt_of(&mut app, 0) > 0);

    // Mid-flex dash: the escape valve. The taunt dies with no reward.
    set_inputs(
        &mut app,
        PlayerInput {
            stick_x: STICK_FULL,
            stick_y: 0,
            aim_angle: 0,
            buttons: PlayerInput::TAUNT_DOWN | PlayerInput::DASH_DOWN,
        },
    );
    app.update();
    app.update();
    assert_eq!(taunt_of(&mut app, 0), 0, "dash must cancel the taunt");
    for _ in 0..(TAUNT_FRAMES as usize + 4) {
        app.update();
    }
    assert_eq!(
        streak_of(&mut app, 0),
        0,
        "a canceled taunt must not pay out"
    );
}

#[test]
fn throw_press_cancels_taunt_without_reward() {
    let mut app = build_two_player_app_cd(2);
    app.update();

    set_inputs(
        &mut app,
        PlayerInput {
            stick_x: 0,
            stick_y: 0,
            aim_angle: 0,
            buttons: PlayerInput::TAUNT_DOWN,
        },
    );
    app.update();
    app.update();
    assert!(taunt_of(&mut app, 0) > 0);

    // Arming a wind-up mid-flex converts the plant into a real charge.
    set_inputs(
        &mut app,
        PlayerInput {
            stick_x: 0,
            stick_y: 0,
            aim_angle: 0,
            buttons: PlayerInput::TAUNT_DOWN | PlayerInput::THROW_DOWN,
        },
    );
    app.update();
    app.update();
    assert_eq!(
        taunt_of(&mut app, 0),
        0,
        "an armed wind-up must cancel the taunt"
    );
    for _ in 0..(TAUNT_FRAMES as usize + 4) {
        app.update();
    }
    assert_eq!(streak_of(&mut app, 0), 0);
}

// ---- Spawn guard ----

/// Out-of-band setup (cd 0): a guarded player standing in a lethal fang
/// survives until the guard runs out, then dies to the same fang.
#[test]
fn spawn_guard_blocks_the_camped_fang_until_it_expires() {
    let mut app = build_two_player_app_cd(0);
    app.update();

    // Camp p1's exact position with an enemy fang parked at zero velocity.
    let p1 = Vec2F::from_cm(0, 400);
    app.world_mut().spawn((
        Boomerang {
            owner_handle: 0,
            state: BoomerangState::Flying,
        },
        PositionF(p1),
        PreviousPositionF(p1),
        VelocityF(Vec2F::ZERO),
    ));
    // Hand-grant a short guard window (the respawn path is covered by
    // `respawn_grants_guard_that_breaks_on_offense`).
    {
        let mut q = app.world_mut().query::<(&Player, &mut SpawnGuard)>();
        for (p, mut g) in q.iter_mut(app.world_mut()) {
            if p.handle == 1 {
                g.0 = 10;
            }
        }
    }

    for _ in 0..8 {
        app.update();
        assert!(
            !is_dying(&mut app, 1),
            "the guard must hold off the camped fang"
        );
    }
    for _ in 0..6 {
        app.update();
    }
    assert!(
        is_dying(&mut app, 1),
        "once the guard expires the camped fang kills normally"
    );
}

#[test]
fn respawn_grants_guard_that_breaks_on_offense() {
    let mut app = build_two_player_app_cd(0);
    app.update();

    // Kill p1 with a passing enemy fang.
    let p1 = Vec2F::from_cm(0, 400);
    app.world_mut().spawn((
        Boomerang {
            owner_handle: 0,
            state: BoomerangState::Flying,
        },
        PositionF(p1),
        PreviousPositionF(p1),
        VelocityF(Vec2F::ZERO),
    ));
    app.update();
    app.update();
    assert!(is_dying(&mut app, 1), "setup: p1 should be dying");
    assert_eq!(guard_of(&mut app, 1), 0, "no guard while dead");

    // Ride out the respawn window; the revive must come up guarded.
    for _ in 0..(RESPAWN_FRAMES as usize + 3) {
        app.update();
    }
    assert!(!is_dying(&mut app, 1), "p1 should have respawned");
    let g = guard_of(&mut app, 1);
    assert!(
        g > 0 && g <= SPAWN_GUARD_FRAMES,
        "revive grants the spawn guard (got {g})"
    );

    // A fresh THROW press is offense: the guard breaks immediately
    // (both players share SynthesizedInputs; p0's press is harmless).
    set_inputs(
        &mut app,
        PlayerInput {
            stick_x: 0,
            stick_y: 0,
            aim_angle: 0,
            buttons: PlayerInput::THROW_DOWN,
        },
    );
    app.update();
    app.update();
    assert_eq!(
        guard_of(&mut app, 1),
        0,
        "throwing must forfeit the spawn guard"
    );
}
