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
use fixed_math::{Fix, RectF, Vec2F};
use sim::{
    AnimState, BONE_PYRE_HALF_EXTENT_CM, BonePyre, Boomerang, BoomerangState, DefaultInputsPlugin,
    Empowered, GgrsCfg, MatchScore, MatchState, Player, PlayerInput, PositionF, PreviousPositionF,
    SimPlugin, SynthesizedInputs, VelocityF, respawn_position,
};

fn build() -> App {
    build_with(0, 2)
}

fn build_with(input_delay: usize, check_distance: usize) -> App {
    let mut sb = SessionBuilder::<GgrsCfg>::new()
        .with_num_players(2)
        .unwrap()
        .with_max_prediction_window(check_distance + 1)
        .with_check_distance(check_distance)
        .with_input_delay(input_delay);
    sb = sb.add_player(PlayerType::Local, 0).unwrap();
    sb = sb.add_player(PlayerType::Local, 1).unwrap();
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
    // SyncTest's first update is a no-op warmup (FrameCount stays 0).
    app.update();
    app
}

/// Force the app into `MatchOver` with a decided score, as if a kill just
/// crossed the threshold long enough ago that the rematch is open.
fn force_match_over(app: &mut App) {
    *app.world_mut().resource_mut::<MatchScore>() = MatchScore { p0: 5, p1: 0 };
    *app.world_mut().resource_mut::<MatchState>() = MatchState::MatchOver {
        rematch_at_frame: 0,
    };
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
        state(&app).is_match_over(),
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

/// Boot a match whose deciding kill has just landed: the next tick decides
/// it (5-0 while `InRound`), exactly as the sim itself would.
fn boot_on_the_deciding_tick(app: &mut App) {
    *app.world_mut().resource_mut::<MatchScore>() = MatchScore { p0: 5, p1: 0 };
    *app.world_mut().resource_mut::<MatchState>() = MatchState::InRound {
        expires_at_frame: 9_999,
    };
}

/// The rematch bug: a THROW pressed before the player could see the match
/// was decided lands inside `MatchOver` input-delay ticks later (and, online,
/// up to the prediction window later) and restarted the match at once — no
/// summary, no tape, no signed result. Held from the deciding tick through
/// 1.5 s, it must never restart the match.
fn a_throw_in_flight_never_restarts(input_delay: usize, check_distance: usize) {
    let mut app = build_with(input_delay, check_distance);
    boot_on_the_deciding_tick(&mut app);
    set_throw(&mut app, true);
    for tick in 0..90 {
        app.update();
        assert!(
            matches!(state(&app), MatchState::MatchOver { .. }),
            "tick {tick}: a THROW pressed before the decision restarted the match; got {:?}",
            state(&app),
        );
    }
}

#[test]
fn a_throw_in_flight_at_the_decision_never_restarts_the_match() {
    a_throw_in_flight_never_restarts(3, 2);
}

#[test]
fn a_throw_in_flight_never_restarts_the_match_under_deep_rollback() {
    a_throw_in_flight_never_restarts(3, 16);
}

/// Drive `ticks` ticks with THROW held on the ticks `throw_on` names (by the
/// frame each tick simulates; input delay 0, so a press lands on its tick).
fn drive(app: &mut App, ticks: u32, throw_on: impl Fn(u32) -> bool) {
    for _ in 0..ticks {
        let frame = app.world().resource::<sim::FrameCount>().0;
        set_throw(app, throw_on(frame));
        app.update();
    }
}

fn frame(app: &App) -> u32 {
    app.world().resource::<sim::FrameCount>().0
}

#[test]
fn the_rematch_opens_exactly_at_the_end_of_the_grace() {
    for (press_at, restarts) in [
        (sim::REMATCH_GRACE_FRAMES - 1, false),
        (sim::REMATCH_GRACE_FRAMES, true),
    ] {
        let mut app = build();
        boot_on_the_deciding_tick(&mut app);
        drive(&mut app, 1, |_| false);
        assert_eq!(
            state(&app),
            MatchState::decided(0),
            "decided on tick 0, the rematch opens {} ticks later",
            sim::REMATCH_GRACE_FRAMES
        );
        assert_eq!(state(&app).rematch_at(), Some(sim::REMATCH_GRACE_FRAMES));
        // A clean edge on `press_at`, then held.
        drive(&mut app, press_at + 5, |f| f >= press_at);
        assert_eq!(
            !state(&app).is_match_over(),
            restarts,
            "an edge on tick {press_at}: restarts={restarts}; got {:?}",
            state(&app)
        );
        if restarts {
            // It fired on the edge's own tick: the countdown is the opening
            // one, shifted to that tick.
            assert_eq!(state(&app), MatchState::opening(press_at));
        }
    }
}

#[test]
fn a_throw_held_through_the_grace_never_restarts_but_a_fresh_press_does() {
    let mut app = build();
    boot_on_the_deciding_tick(&mut app);
    // Pressed on the deciding tick, held well past the grace: no edge.
    drive(&mut app, 120, |_| true);
    assert!(state(&app).is_match_over(), "got {:?}", state(&app));
    // Released, pressed again: a deliberate press restarts.
    drive(&mut app, 1, |_| false);
    let at = frame(&app);
    drive(&mut app, 1, |_| true);
    assert_eq!(state(&app), MatchState::opening(at));
}

#[test]
fn an_abandoned_match_never_reopens() {
    let mut app = build();
    *app.world_mut().resource_mut::<MatchScore>() = MatchScore { p0: 3, p1: 1 };
    *app.world_mut().resource_mut::<MatchState>() = MatchState::abandoned();
    assert_eq!(MatchState::abandoned().rematch_at(), None);
    assert!(!MatchState::abandoned().rematch_open(u32::MAX));
    drive(&mut app, 200, |f| f % 4 == 0);
    assert_eq!(state(&app), MatchState::abandoned());
    assert_eq!(
        *app.world().resource::<MatchScore>(),
        MatchScore { p0: 3, p1: 1 }
    );
}

// ---- A rematch is a fresh match ----

/// A real table (spawn points, cover, pyres) on `arena`.
fn build_table(arena: sim::ArenaId) -> App {
    let mut sb = SessionBuilder::<GgrsCfg>::new()
        .with_num_players(2)
        .unwrap()
        .with_max_prediction_window(3)
        .with_check_distance(2)
        .with_input_delay(0);
    sb = sb.add_player(PlayerType::Local, 0).unwrap();
    sb = sb.add_player(PlayerType::Local, 1).unwrap();
    let session = sb.start_synctest_session().unwrap();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(TimeUpdateStrategy::ManualDuration(sim::tick_duration()));
    app.add_plugins(GgrsPlugin::<GgrsCfg>::default());
    app.add_plugins(SimPlugin);
    app.add_plugins(DefaultInputsPlugin);
    app.insert_resource(Session::SyncTest(session));
    app.insert_resource(sim::SelectedArena(arena));
    sim::spawn_match_world_now(app.world_mut(), arena);
    app.update();
    app
}

/// Everything a duelist carries, by handle.
type Duelist = (
    PositionF,
    PreviousPositionF,
    VelocityF,
    (
        sim::Dead,
        sim::DashState,
        sim::StunFrames,
        Empowered,
        sim::HeldModifier,
        AnimState,
    ),
    (
        sim::OobTimer,
        sim::ThrowCapacity,
        sim::ThrowCharge,
        sim::CatchStreak,
        sim::Taunt,
        sim::SpawnGuard,
    ),
);

fn duelists(app: &mut App) -> Vec<(usize, Duelist)> {
    let mut q = app.world_mut().query::<(
        &Player,
        &PositionF,
        &PreviousPositionF,
        &VelocityF,
        (
            &sim::Dead,
            &sim::DashState,
            &sim::StunFrames,
            &Empowered,
            &sim::HeldModifier,
            &AnimState,
        ),
        (
            &sim::OobTimer,
            &sim::ThrowCapacity,
            &sim::ThrowCharge,
            &sim::CatchStreak,
            &sim::Taunt,
            &sim::SpawnGuard,
        ),
    )>();
    let mut out: Vec<_> = q
        .iter(app.world())
        .map(|(p, pos, prev, vel, a, b)| {
            (
                p.handle,
                (
                    *pos,
                    *prev,
                    *vel,
                    (*a.0, *a.1, *a.2, *a.3, *a.4, *a.5),
                    (*b.0, *b.1, *b.2, *b.3, *b.4, *b.5),
                ),
            )
        })
        .collect();
    out.sort_by_key(|(h, _)| *h);
    out
}

/// `state` with every frame field moved `by` ticks earlier.
fn shifted_back(state: MatchState, by: u32) -> MatchState {
    match state {
        MatchState::Countdown {
            digit,
            expires_at_frame,
        } => MatchState::Countdown {
            digit,
            expires_at_frame: expires_at_frame - by,
        },
        MatchState::InRound { expires_at_frame } => MatchState::InRound {
            expires_at_frame: expires_at_frame - by,
        },
        MatchState::RoundOver { expires_at_frame } => MatchState::RoundOver {
            expires_at_frame: expires_at_frame - by,
        },
        MatchState::MatchOver { rematch_at_frame } => MatchState::MatchOver {
            rematch_at_frame: rematch_at_frame - by,
        },
    }
}

fn count<T: Component>(app: &mut App) -> usize {
    app.world_mut().query::<&T>().iter(app.world()).count()
}

const DIRTY_AT: u32 = 40;

/// What [`dirty_then_decide`] leaves on each duelist.
type DirtyDuelist = (
    &'static mut PositionF,
    &'static mut Empowered,
    &'static mut sim::HeldModifier,
    &'static mut sim::StunFrames,
    (
        &'static mut sim::OobTimer,
        &'static mut sim::ThrowCapacity,
        &'static mut sim::ThrowCharge,
        &'static mut sim::CatchStreak,
        &'static mut sim::Taunt,
        &'static mut sim::SpawnGuard,
    ),
);

/// On tick [`DIRTY_AT`]: leave on the duelists everything a match can (a
/// streak climbed, a pickup held, a throw charging, a pickup about to drop),
/// then decide the match with the rematch already open.
fn dirty_then_decide(
    frame: Res<sim::FrameCount>,
    mut state: ResMut<MatchState>,
    mut score: ResMut<MatchScore>,
    mut timer: ResMut<sim::PickupSpawnTimer>,
    mut q: Query<DirtyDuelist>,
) {
    if frame.0 != DIRTY_AT {
        return;
    }
    for (
        mut pos,
        mut emp,
        mut held,
        mut stun,
        (mut oob, mut cap, mut charge, mut streak, mut taunt, mut guard),
    ) in &mut q
    {
        pos.0 = Vec2F::from_cm(120, -80);
        emp.0 = true;
        held.0 = Some(sim::PickupKind::Fire);
        stun.0 = 4;
        oob.0 = 9;
        cap.0 = 2;
        charge.0 = 12;
        streak.0 = 3;
        taunt.0 = 5;
        guard.0 = 7;
    }
    timer.next_at_frame = 1;
    *score = MatchScore { p0: 5, p1: 4 };
    *state = MatchState::MatchOver {
        rematch_at_frame: DIRTY_AT,
    };
}

/// Whatever the last match left on the duelists (a streak climbed, a pickup
/// held, a throw charging, a timer about to drop a pickup), the rematch is
/// the fresh match shifted to the tick it fired on: tick for tick through
/// the countdown and into the round.
#[test]
fn a_rematch_is_a_fresh_match_shifted_in_time() {
    // The dirt goes in from inside the schedule: SyncTest rolls back and
    // re-simulates, which would undo a write from outside it.
    let mut played = build_table(sim::ArenaId::Anchor);
    played.add_systems(
        GgrsSchedule,
        dirty_then_decide.before(sim::snapshot_previous),
    );
    // Ticks 0..DIRTY_AT play the opening; DIRTY_AT dirties, decides with the
    // rematch already open, and a THROW edge on the same tick fires it.
    drive(&mut played, DIRTY_AT, |_| false);
    drive(&mut played, 1, |_| true);
    assert_eq!(
        state(&played),
        MatchState::opening(DIRTY_AT),
        "the rematch fired"
    );
    let at = DIRTY_AT;

    let mut fresh = build_table(sim::ArenaId::Anchor);
    drive(&mut fresh, 1, |_| false);

    // 180 ticks of countdown, then a second of the round.
    for tick in 0..240 {
        assert_eq!(
            duelists(&mut played),
            duelists(&mut fresh),
            "tick {tick}: duelists"
        );
        assert_eq!(
            shifted_back(state(&played), at),
            state(&fresh),
            "tick {tick}: state"
        );
        assert_eq!(
            played
                .world()
                .resource::<sim::PickupSpawnTimer>()
                .next_at_frame
                - at,
            fresh
                .world()
                .resource::<sim::PickupSpawnTimer>()
                .next_at_frame,
            "tick {tick}: the next pickup"
        );
        assert_eq!(
            *played.world().resource::<MatchScore>(),
            *fresh.world().resource::<MatchScore>()
        );
        assert_eq!(
            count::<Boomerang>(&mut played),
            count::<Boomerang>(&mut fresh)
        );
        assert_eq!(
            count::<sim::Pickup>(&mut played),
            count::<sim::Pickup>(&mut fresh)
        );
        drive(&mut played, 1, |_| false);
        drive(&mut fresh, 1, |_| false);
    }
    assert!(state(&fresh).is_in_round(), "the comparison reached GO");
}
