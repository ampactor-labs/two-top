//! Phase 18 Task 5.5a: deterministic rematch ("play again").
//!
//! `MatchOver` is terminal in `tick_match_state`; `apply_rematch` is the one
//! escape — a THROW rising edge from either player while `MatchOver` restarts
//! the match deterministically (score 0-0, top of the countdown, arena wiped
//! clean). These tests pin the behavior and the clean-slate guarantees.

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy_ggrs::GgrsPlugin;
use bevy_ggrs::prelude::*;
use core::time::Duration;
use fixed_math::{Fix, RectF, Vec2F};
use sim::{
    AnimState, BONE_PYRE_HALF_EXTENT_CM, BonePyre, Boomerang, BoomerangState, DefaultInputsPlugin,
    Empowered, GgrsCfg, MatchScore, MatchState, Player, PlayerInput, PositionF, PreviousPositionF,
    SimPlugin, SynthesizedInputs, VelocityF, respawn_position,
};

fn build() -> App {
    let mut sb = SessionBuilder::<GgrsCfg>::new()
        .with_num_players(2)
        .unwrap()
        .with_check_distance(2)
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
    // SyncTest's first update is a no-op warmup (FrameCount stays 0).
    app.update();
    app
}

/// Force the app into `MatchOver` with a decided score, as if a kill just
/// crossed the threshold.
fn force_match_over(app: &mut App) {
    *app.world_mut().resource_mut::<MatchScore>() = MatchScore { p0: 5, p1: 0 };
    *app.world_mut().resource_mut::<MatchState>() = MatchState::MatchOver;
}

fn set_throw(app: &mut App, down: bool) {
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 0,
        stick_y: 0,
        aim_angle: 0,
        buttons: if down { PlayerInput::THROW_DOWN } else { 0 },
    };
}

/// Drive a THROW rising edge: one tick released (seeds history low), then one
/// tick pressed (the edge `apply_rematch` keys on).
fn throw_edge(app: &mut App) {
    set_throw(app, false);
    app.update();
    set_throw(app, true);
    app.update();
}

fn state(app: &App) -> MatchState {
    *app.world().resource::<MatchState>()
}

#[test]
fn throw_during_match_over_restarts_to_countdown_and_resets_score() {
    let mut app = build();
    force_match_over(&mut app);
    throw_edge(&mut app);

    assert!(
        matches!(state(&app), MatchState::Countdown { digit: 3, .. }),
        "a throw during MatchOver restarts at the top of the countdown; got {:?}",
        state(&app),
    );
    assert_eq!(
        *app.world().resource::<MatchScore>(),
        MatchScore::default(),
        "the new match opens 0-0",
    );
}

#[test]
fn no_throw_during_match_over_stays_terminal() {
    let mut app = build();
    force_match_over(&mut app);

    // Tick repeatedly with no throw — MatchOver must hold.
    set_throw(&mut app, false);
    for _ in 0..5 {
        app.update();
    }
    assert!(
        matches!(state(&app), MatchState::MatchOver),
        "without a throw, MatchOver is terminal; got {:?}",
        state(&app),
    );
}

#[test]
fn rematch_despawns_in_flight_boomerang() {
    let mut app = build();
    app.world_mut().spawn((
        Boomerang {
            owner_handle: 0,
            state: BoomerangState::Flying,
        },
        PositionF(Vec2F::from_cm(20, 0)),
        PreviousPositionF(Vec2F::from_cm(20, 0)),
        VelocityF(Vec2F::from_cm(50, 0)),
    ));
    force_match_over(&mut app);
    throw_edge(&mut app);

    let remaining = app
        .world_mut()
        .query::<&Boomerang>()
        .iter(app.world())
        .count();
    assert_eq!(remaining, 0, "rematch wipes in-flight boomerangs");
}

#[test]
fn rematch_unshatters_pyre() {
    let mut app = build();
    let h = Fix::const_from_int(BONE_PYRE_HALF_EXTENT_CM);
    let mut pyre = BonePyre::intact(RectF::from_center_half_extents(
        Vec2F::from_cm(0, 0),
        Vec2F::new(h, h),
    ));
    pyre.shattered = true;
    pyre.chain_delay = Some(99);
    app.world_mut().spawn(pyre);

    force_match_over(&mut app);
    throw_edge(&mut app);

    let mut q = app.world_mut().query::<&BonePyre>();
    let p = q.iter(app.world()).next().expect("pyre still present");
    assert!(!p.shattered, "rematch un-shatters pyres for a clean arena");
    assert!(
        p.chain_delay.is_none(),
        "and clears any pending chain ignition"
    );
}

#[test]
fn rematch_respawns_players_and_clears_empowered() {
    let mut app = build();
    // Empower a player and check the flag is cleared on rematch.
    {
        let mut q = app.world_mut().query::<(&Player, &mut Empowered)>();
        for (_player, mut emp) in q.iter_mut(app.world_mut()) {
            emp.0 = true;
        }
    }
    force_match_over(&mut app);
    throw_edge(&mut app);

    let mut q = app
        .world_mut()
        .query::<(&Player, &PositionF, &Empowered, &AnimState)>();
    let mut seen = 0;
    for (player, pos, emp, anim) in q.iter(app.world()) {
        assert_eq!(
            pos.0,
            respawn_position(player.handle),
            "player {} snaps to its spawn on rematch",
            player.handle,
        );
        assert!(!emp.0, "empowered flag cleared on rematch");
        assert_eq!(anim.anim_id, AnimState::IDLE, "anim reset to idle");
        seen += 1;
    }
    assert_eq!(seen, 2, "both players reset");
}

/// `apply_rematch` is inert outside `MatchOver` — a throw during a live round
/// must not reset score or state. Driven through the real schedule (the system
/// reads ggrs's `PlayerInputs`, which only exists inside `GgrsSchedule`).
#[test]
fn rematch_is_inert_during_a_live_round() {
    let mut app = build();
    *app.world_mut().resource_mut::<MatchScore>() = MatchScore { p0: 2, p1: 1 };
    *app.world_mut().resource_mut::<MatchState>() = MatchState::InRound {
        expires_at_frame: 9_999,
    };
    throw_edge(&mut app);

    assert!(
        matches!(state(&app), MatchState::InRound { .. }),
        "a throw mid-round never restarts the match; got {:?}",
        state(&app),
    );
    assert_eq!(
        *app.world().resource::<MatchScore>(),
        MatchScore { p0: 2, p1: 1 },
        "a throw mid-round never resets the score",
    );
}
