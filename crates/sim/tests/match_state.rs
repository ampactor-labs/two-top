//! Phase 11 cycle 5: MatchState scaffolding.
//!
//! Cycle 5 is enum + initial state + rollback registration only.
//! Cycle 6 wires the actual frame-counted transitions on top.
//!
//! Coverage:
//!   * Default `MatchState` is `Countdown { digit: 3, .. }`.
//!   * The resource is initialized when `SimPlugin` builds.
//!   * The four arms (`Countdown` / `InRound` / `RoundOver` /
//!     `MatchOver`) are exhaustively pattern-matchable — guards
//!     against accidental enum-variant drift breaking cycle 6.

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy_ggrs::GgrsPlugin;
use bevy_ggrs::prelude::*;
use core::time::Duration;
use fixed_math::Vec2F;
use sim::{
    Boomerang, BoomerangState, COUNTDOWN_DIGIT_FRAMES, DefaultInputsPlugin, FrameCount, GgrsCfg,
    MATCH_WIN_THRESHOLD, MatchScore, MatchState, Player, PlayerInput, PositionF,
    PreviousPositionF, ROUND_DURATION_FRAMES, ROUND_OVER_FRAMES, SimPlugin, SynthesizedInputs,
    VelocityF,
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
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
        1.0 / sim::TICK_HZ as f64,
    )));
    app.add_plugins(GgrsPlugin::<GgrsCfg>::default());
    app.add_plugins(SimPlugin);
    app.add_plugins(DefaultInputsPlugin);
    app.insert_resource(Session::SyncTest(session));
    app
}

#[test]
fn default_match_state_is_countdown_digit_three() {
    let state = MatchState::default();
    match state {
        MatchState::Countdown { digit, .. } => {
            assert_eq!(digit, 3, "match must open at the top of the countdown");
        }
        _ => panic!("default MatchState should be Countdown, got {state:?}"),
    }
}

#[test]
fn match_state_resource_is_present_after_sim_plugin_builds() {
    let app = build_app();
    // `init_resource` ran during plugin build; the resource is
    // accessible without an `app.update()`.
    let state = *app.world().resource::<MatchState>();
    assert!(matches!(state, MatchState::Countdown { digit: 3, .. }));
}

// ---- Cycle 6: round timer + transitions ----

/// Build a single-player app WITHOUT `InfiniteRoundPlugin` so the
/// real countdown ticks. Tests that need the gate enabled use this
/// builder; tests that just want gameplay running use the
/// `InfiniteRoundPlugin`-augmented builders elsewhere.
fn build_app_with_real_countdown() -> App {
    let mut sb = SessionBuilder::<GgrsCfg>::new()
        .with_num_players(1)
        .unwrap()
        .with_check_distance(2)
        .with_input_delay(0);
    sb = sb.add_player(PlayerType::Local, 0).unwrap();
    let session = sb.start_synctest_session().unwrap();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
        1.0 / sim::TICK_HZ as f64,
    )));
    app.add_plugins(GgrsPlugin::<GgrsCfg>::default());
    app.add_plugins(SimPlugin);
    app.add_plugins(DefaultInputsPlugin);
    app.insert_resource(Session::SyncTest(session));

    app.world_mut().spawn((
        Player { handle: 0 },
        PositionF(Vec2F::ZERO),
        PreviousPositionF(Vec2F::ZERO),
        VelocityF(Vec2F::ZERO),
    ));

    // SyncTestSession's first `app.update()` is a no-op warmup tick
    // (the schedule does not run; FrameCount stays at 0). Bake the
    // warmup in here so tests downstream of build_app can count
    // `app.update()` calls as 1:1 sim ticks against `FrameCount`.
    app.update();
    app
}

fn match_state(app: &App) -> MatchState {
    *app.world().resource::<MatchState>()
}

#[test]
fn countdown_steps_three_two_one_then_in_round() {
    let mut app = build_app_with_real_countdown();

    // Tick 0..59: digit 3 (initial). Tick 60: transition to digit 2.
    // Tick 120: transition to digit 1. Tick 180: transition to InRound.
    for _ in 0..COUNTDOWN_DIGIT_FRAMES {
        app.update();
    }
    let fc = app.world().resource::<FrameCount>().0;
    assert!(
        matches!(match_state(&app), MatchState::Countdown { digit: 2, .. }),
        "after {} ticks (FrameCount={fc}), expect digit 2; got {:?}",
        COUNTDOWN_DIGIT_FRAMES,
        match_state(&app),
    );

    for _ in 0..COUNTDOWN_DIGIT_FRAMES {
        app.update();
    }
    assert!(
        matches!(match_state(&app), MatchState::Countdown { digit: 1, .. }),
        "after second 60 ticks, expect digit 1; got {:?}",
        match_state(&app),
    );

    for _ in 0..COUNTDOWN_DIGIT_FRAMES {
        app.update();
    }
    assert!(
        matches!(match_state(&app), MatchState::InRound { .. }),
        "after third 60 ticks, expect InRound; got {:?}",
        match_state(&app),
    );
}

#[test]
fn in_round_expires_to_round_over_after_round_duration() {
    let mut app = build_app_with_real_countdown();
    // Skip the countdown by ticking 3 × COUNTDOWN_DIGIT_FRAMES.
    for _ in 0..(3 * COUNTDOWN_DIGIT_FRAMES) {
        app.update();
    }
    assert!(matches!(match_state(&app), MatchState::InRound { .. }));

    // Tick the full round duration.
    for _ in 0..ROUND_DURATION_FRAMES {
        app.update();
    }
    assert!(
        matches!(match_state(&app), MatchState::RoundOver { .. }),
        "after round duration, expect RoundOver; got {:?}",
        match_state(&app),
    );
}

#[test]
fn round_over_loops_back_to_countdown_for_next_round() {
    let mut app = build_app_with_real_countdown();
    // Tick to RoundOver.
    for _ in 0..(3 * COUNTDOWN_DIGIT_FRAMES + ROUND_DURATION_FRAMES) {
        app.update();
    }
    assert!(matches!(match_state(&app), MatchState::RoundOver { .. }));

    // Tick the round-over beat.
    for _ in 0..ROUND_OVER_FRAMES {
        app.update();
    }
    assert!(
        matches!(match_state(&app), MatchState::Countdown { digit: 3, .. }),
        "after RoundOver expires, expect a fresh digit-3 Countdown; got {:?}",
        match_state(&app),
    );
}

#[test]
fn reaching_match_win_threshold_in_round_jumps_to_match_over() {
    // Pure-ish unit test that exercises `tick_match_state`'s logic
    // outside the bevy_ggrs rollback machinery. A `SyncTestSession`
    // restores rolled-back resources from snapshots between ticks,
    // so an external mutation to `MatchScore` doesn't survive into
    // the next schedule run. We skip the session entirely here and
    // run the system on a minimal world.
    use sim::tick_match_state;

    let mut app = App::new();
    app.insert_resource(FrameCount(500)); // arbitrary mid-round frame
    app.insert_resource(MatchScore {
        p0: MATCH_WIN_THRESHOLD,
        p1: 0,
    });
    app.insert_resource(MatchState::InRound {
        expires_at_frame: 1979,
    });
    app.add_systems(Update, tick_match_state);
    app.update();

    assert_eq!(
        *app.world().resource::<MatchState>(),
        MatchState::MatchOver,
        "p0 at threshold should end the match",
    );
}

#[test]
fn match_over_is_terminal() {
    let mut app = build_app_with_real_countdown();
    *app.world_mut().resource_mut::<MatchState>() = MatchState::MatchOver;
    for _ in 0..10 {
        app.update();
    }
    assert_eq!(
        match_state(&app),
        MatchState::MatchOver,
        "MatchOver must not transition out",
    );
}

// ---- Input gating ----

#[test]
fn player_does_not_move_during_countdown() {
    let mut app = build_app_with_real_countdown();

    // Drive the stick full east during countdown. Player must not move.
    let pos_before = app
        .world_mut()
        .query::<(&Player, &PositionF)>()
        .iter(app.world())
        .next()
        .map(|(_, p)| p.0)
        .expect("p0 entity");
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 127,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    for _ in 0..30 {
        app.update();
    }
    assert!(
        matches!(match_state(&app), MatchState::Countdown { .. }),
        "still in Countdown",
    );
    let pos_after = app
        .world_mut()
        .query::<(&Player, &PositionF)>()
        .iter(app.world())
        .next()
        .map(|(_, p)| p.0)
        .expect("p0 entity");
    assert_eq!(pos_before, pos_after, "no movement during Countdown");
}

#[test]
fn player_throw_is_blocked_during_round_over() {
    let mut app = build_app_with_real_countdown();
    // Tick into InRound, then to RoundOver.
    for _ in 0..(3 * COUNTDOWN_DIGIT_FRAMES + ROUND_DURATION_FRAMES) {
        app.update();
    }
    assert!(matches!(match_state(&app), MatchState::RoundOver { .. }));

    // Try a tap-release throw during RoundOver.
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 0,
        stick_y: 0,
        aim_angle: 0,
        buttons: PlayerInput::THROW_DOWN,
    };
    app.update();
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 127,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    app.update();

    let count = app.world_mut().query::<&Boomerang>().iter(app.world()).count();
    assert_eq!(count, 0, "throw must not spawn a boomerang during RoundOver");
}

// Suppress warnings on unused imports — Boomerang/BoomerangState are
// referenced in cycle 7+ tests that share this file, and FrameCount
// is used implicitly through assertions in this file's other tests.
#[allow(dead_code)]
fn _unused_imports_keepalive() {
    let _ = (
        BoomerangState::Flying,
        Boomerang {
            owner_handle: 0,
            state: BoomerangState::Flying,
        },
        FrameCount(0),
    );
}

#[test]
fn match_state_variants_are_exhaustively_patternable() {
    // Compile-time check: this match would fail to compile if any
    // variant got renamed/removed without updating cycle 6 — early
    // tripwire for refactor accidents.
    fn label(s: MatchState) -> &'static str {
        match s {
            MatchState::Countdown { .. } => "countdown",
            MatchState::InRound { .. } => "in_round",
            MatchState::RoundOver { .. } => "round_over",
            MatchState::MatchOver => "match_over",
        }
    }

    assert_eq!(
        label(MatchState::Countdown {
            digit: 3,
            expires_at_frame: 0
        }),
        "countdown"
    );
    assert_eq!(
        label(MatchState::InRound { expires_at_frame: 100 }),
        "in_round"
    );
    assert_eq!(
        label(MatchState::RoundOver { expires_at_frame: 200 }),
        "round_over"
    );
    assert_eq!(label(MatchState::MatchOver), "match_over");
}
