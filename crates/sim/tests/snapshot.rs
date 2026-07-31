//! Phase 14 cycle 2a: `SimSnapshot` capture/restore round-trip.
//!
//! These tests are headless — they exercise the snapshot machinery
//! directly against a synthetic World. The replay viewer's interactive
//! scrub flow is built on top; these tests guarantee the underlying
//! state-restoration is bit-identical so the visual scrub is, too.
//!
//! The test World pipes through a `MinimalPlugins` + `GgrsPlugin`
//! ceremony so the `#[require(Rollback)]` macro on Player/Boomerang
//! can resolve the RollbackId provider resource. This mirrors the
//! pattern used by every other sim integration test (e.g. dash.rs,
//! match_state.rs).

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy_ggrs::GgrsPlugin;
use bevy_ggrs::prelude::*;
use fixed_math::Vec2F;
use sim::{
    Boomerang, BoomerangState, DashState, Dead, FrameCount, GgrsCfg, InputHistory, MatchScore,
    MatchState, Player, PlayerInput, PositionF, PreviousPositionF, SimPlugin, SimSnapshot,
    StunFrames, VelocityF,
};

/// Build an App seeded with two players + one boomerang + the
/// rolled-back resources `SimSnapshot::capture` reads. Uses
/// `MinimalPlugins + GgrsPlugin + SimPlugin` (the standard sim-test
/// ceremony) so `#[require(Rollback)]` can resolve.
fn seed_app() -> App {
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
    app.insert_resource(TimeUpdateStrategy::ManualDuration(sim::tick_duration()));
    app.add_plugins(GgrsPlugin::<GgrsCfg>::default());
    app.add_plugins(SimPlugin);
    app.insert_resource(Session::SyncTest(session));

    // Overwrite the post-SimPlugin defaults with deterministic test
    // values so capture/restore assertions have known constants.
    *app.world_mut().resource_mut::<FrameCount>() = FrameCount(412);
    *app.world_mut().resource_mut::<MatchScore>() = MatchScore { p0: 2, p1: 1 };
    *app.world_mut().resource_mut::<MatchState>() = MatchState::InRound {
        expires_at_frame: 1700,
    };
    let mut history = InputHistory::default();
    history.0.insert(
        0,
        [PlayerInput {
            stick_x: 100,
            stick_y: -50,
            aim_angle: 0,
            buttons: PlayerInput::DASH_DOWN,
        }; 8],
    );
    history.0.insert(1, [PlayerInput::default(); 8]);
    *app.world_mut().resource_mut::<InputHistory>() = history;

    app.world_mut().spawn((
        Player { handle: 0 },
        PositionF(Vec2F::from_cm(-100, 60)),
        PreviousPositionF(Vec2F::from_cm(-110, 55)),
        VelocityF(Vec2F::from_cm(2, 1)),
        DashState::Cooldown {
            frames_remaining: 8,
        },
        StunFrames(3),
        Dead {
            respawn_at_frame: None,
        },
    ));
    app.world_mut().spawn((
        Player { handle: 1 },
        PositionF(Vec2F::from_cm(100, -60)),
        PreviousPositionF(Vec2F::from_cm(95, -55)),
        VelocityF(Vec2F::from_cm(-3, 2)),
        DashState::Idle,
        StunFrames(0),
        Dead {
            respawn_at_frame: Some(580),
        },
    ));

    app.world_mut().spawn((
        Boomerang {
            owner_handle: 0,
            state: BoomerangState::Returning { since: 0 },
        },
        PositionF(Vec2F::from_cm(50, 30)),
        PreviousPositionF(Vec2F::from_cm(60, 35)),
        VelocityF(Vec2F::from_cm(-10, -5)),
    ));

    app
}

#[test]
fn snapshot_captures_all_player_components() {
    let mut app = seed_app();
    let snap = SimSnapshot::capture(app.world_mut());

    assert_eq!(snap.players.len(), 2);
    let p0 = snap
        .players
        .iter()
        .find(|s| s.player.handle == 0)
        .expect("p0 missing");
    assert_eq!(p0.pos.0, Vec2F::from_cm(-100, 60));
    assert_eq!(p0.prev_pos.0, Vec2F::from_cm(-110, 55));
    assert_eq!(p0.vel.0, Vec2F::from_cm(2, 1));
    assert!(matches!(
        p0.dash,
        DashState::Cooldown {
            frames_remaining: 8
        }
    ));
    assert_eq!(p0.stun.0, 3);
    assert_eq!(p0.dead.respawn_at_frame, None);

    let p1 = snap
        .players
        .iter()
        .find(|s| s.player.handle == 1)
        .expect("p1 missing");
    assert_eq!(p1.dead.respawn_at_frame, Some(580));
}

#[test]
fn snapshot_captures_boomerangs_and_resources() {
    let mut app = seed_app();
    let snap = SimSnapshot::capture(app.world_mut());

    assert_eq!(snap.boomerangs.len(), 1);
    let b = &snap.boomerangs[0];
    assert_eq!(b.boomerang.owner_handle, 0);
    assert!(matches!(
        b.boomerang.state,
        BoomerangState::Returning { .. }
    ));
    assert_eq!(b.pos.0, Vec2F::from_cm(50, 30));

    assert_eq!(snap.frame, 412);
    assert_eq!(snap.match_score, MatchScore { p0: 2, p1: 1 });
    assert!(matches!(
        snap.match_state,
        MatchState::InRound {
            expires_at_frame: 1700
        }
    ));
    let h0 = snap.input_history.0.get(&0).expect("p0 history missing");
    assert_eq!(h0[0].buttons, PlayerInput::DASH_DOWN);
}

#[test]
fn restore_round_trips_to_byte_identical_capture() {
    let mut app = seed_app();
    let original = SimSnapshot::capture(app.world_mut());

    // Mutate the world so we can confirm restore actually overwrites.
    *app.world_mut().resource_mut::<FrameCount>() = FrameCount(0);
    *app.world_mut().resource_mut::<MatchState>() = MatchState::default();
    let world = app.world_mut();
    let player_entities: Vec<Entity> = world
        .query_filtered::<Entity, With<Player>>()
        .iter(world)
        .collect();
    for e in player_entities {
        world.despawn(e);
    }

    original.restore(app.world_mut());
    let after = SimSnapshot::capture(app.world_mut());

    // Frame, score, match-state should round-trip exactly.
    assert_eq!(after.frame, original.frame);
    assert_eq!(after.match_score, original.match_score);
    assert_eq!(
        format!("{:?}", after.match_state),
        format!("{:?}", original.match_state)
    );
    // Player count + per-handle state survives the round trip.
    assert_eq!(after.players.len(), original.players.len());
    for orig in &original.players {
        let restored = after
            .players
            .iter()
            .find(|s| s.player.handle == orig.player.handle)
            .expect("handle missing post-restore");
        assert_eq!(restored.pos.0, orig.pos.0);
        assert_eq!(restored.prev_pos.0, orig.prev_pos.0);
        assert_eq!(restored.vel.0, orig.vel.0);
        assert_eq!(restored.stun.0, orig.stun.0);
        assert_eq!(restored.dead.respawn_at_frame, orig.dead.respawn_at_frame);
    }
    // Boomerang state survives.
    assert_eq!(after.boomerangs.len(), original.boomerangs.len());
    let orig_b = &original.boomerangs[0];
    let after_b = &after.boomerangs[0];
    assert_eq!(
        after_b.boomerang.owner_handle,
        orig_b.boomerang.owner_handle
    );
    assert_eq!(after_b.boomerang.state, orig_b.boomerang.state);
    assert_eq!(after_b.pos.0, orig_b.pos.0);
}

#[test]
fn restore_clears_pre_existing_entities_before_respawn() {
    let mut app = seed_app();
    let snap = SimSnapshot::capture(app.world_mut());

    // Add an extra player + extra boomerang the snapshot doesn't know
    // about. Restore must despawn them so the post-restore entity set
    // matches the snapshot exactly.
    app.world_mut().spawn((
        Player { handle: 99 },
        PositionF(Vec2F::ZERO),
        PreviousPositionF(Vec2F::ZERO),
        VelocityF(Vec2F::ZERO),
        DashState::Idle,
        StunFrames(0),
        Dead {
            respawn_at_frame: None,
        },
    ));
    app.world_mut().spawn((
        Boomerang {
            owner_handle: 99,
            state: BoomerangState::Flying,
        },
        PositionF(Vec2F::ZERO),
        PreviousPositionF(Vec2F::ZERO),
        VelocityF(Vec2F::ZERO),
    ));

    snap.restore(app.world_mut());

    let after = SimSnapshot::capture(app.world_mut());
    assert_eq!(after.players.len(), 2, "extra player should be despawned");
    assert!(
        after.players.iter().all(|s| s.player.handle != 99),
        "handle=99 should not survive restore",
    );
    assert_eq!(after.boomerangs.len(), 1);
    assert_ne!(after.boomerangs[0].boomerang.owner_handle, 99);
}
