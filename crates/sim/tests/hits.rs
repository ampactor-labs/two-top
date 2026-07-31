//! Phase 11 cycle 1: hit detection + Dead component.
//!
//! Coverage:
//!   * Owner immunity — a Flying or Returning boomerang in the owner's
//!     own rect does NOT kill the owner.
//!   * Cross-player kill — a Flying boomerang owned by p0 in p1's rect
//!     inserts `Dead` on p1 and despawns the boomerang.
//!   * Stun immunity — `StunFrames > 0` blocks the hit.
//!   * Returning state also kills (recall traveling through an enemy).
//!   * `respawn_at_frame` is computed from `FrameCount` + `RESPAWN_FRAMES`.

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy_ggrs::GgrsPlugin;
use bevy_ggrs::prelude::*;
use fixed_math::Vec2F;
use sim::{
    Boomerang, BoomerangState, DashState, Dead, DefaultInputsPlugin, FrameCount, GgrsCfg,
    HIT_STOP_FRAMES, MatchScore, Player, PositionF, PreviousPositionF, RESPAWN_FRAMES, SimPlugin,
    StunFrames, ThrowCapacity, VelocityF,
};

fn build_two_player_app() -> App {
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
    app.add_plugins(sim::InfiniteRoundPlugin);
    app.add_plugins(DefaultInputsPlugin);
    app.insert_resource(Session::SyncTest(session));

    // p0 at origin, p1 100 cm east.
    app.world_mut().spawn((
        Player { handle: 0 },
        PositionF(Vec2F::ZERO),
        PreviousPositionF(Vec2F::ZERO),
        VelocityF(Vec2F::ZERO),
    ));
    app.world_mut().spawn((
        Player { handle: 1 },
        PositionF(Vec2F::from_cm(100, 0)),
        PreviousPositionF(Vec2F::from_cm(100, 0)),
        VelocityF(Vec2F::ZERO),
    ));
    app
}

fn count_boomerangs(app: &mut App) -> usize {
    let mut q = app.world_mut().query::<&Boomerang>();
    q.iter(app.world()).count()
}

fn dead_handles(app: &mut App) -> Vec<usize> {
    let mut q = app.world_mut().query::<(&Player, &Dead)>();
    let mut h: Vec<usize> = q
        .iter(app.world())
        .filter(|(_, d)| d.is_dying())
        .map(|(p, _)| p.handle)
        .collect();
    h.sort();
    h
}

fn dead_for(app: &mut App, handle: usize) -> Option<Dead> {
    let mut q = app.world_mut().query::<(&Player, &Dead)>();
    q.iter(app.world())
        .find(|(p, d)| p.handle == handle && d.is_dying())
        .map(|(_, d)| *d)
}

// ---- Owner immunity ----

#[test]
fn flying_boomerang_does_not_kill_owner() {
    let mut app = build_two_player_app();
    app.update(); // warmup

    // Spawn a Flying boomerang owned by p0 directly on top of p0.
    app.world_mut().spawn((
        Boomerang {
            owner_handle: 0,
            state: BoomerangState::Flying,
        },
        PositionF(Vec2F::ZERO),
        PreviousPositionF(Vec2F::ZERO),
        VelocityF(Vec2F::ZERO),
    ));

    app.update();

    assert_eq!(
        count_boomerangs(&mut app),
        1,
        "owner-immune boomerang despawned"
    );
    assert!(
        dead_handles(&mut app).is_empty(),
        "owner should not die from own boomerang",
    );
}

// ---- Cross-player kill ----

#[test]
fn flying_boomerang_kills_non_owner_on_overlap() {
    let mut app = build_two_player_app();
    app.update();

    // Spawn a Flying boomerang owned by p0 inside p1's rect (p1 at (100, 0)).
    app.world_mut().spawn((
        Boomerang {
            owner_handle: 0,
            state: BoomerangState::Flying,
        },
        PositionF(Vec2F::from_cm(100, 0)),
        PreviousPositionF(Vec2F::from_cm(100, 0)),
        VelocityF(Vec2F::ZERO),
    ));

    app.update();

    assert_eq!(
        count_boomerangs(&mut app),
        0,
        "boomerang should despawn on kill"
    );
    assert_eq!(dead_handles(&mut app), vec![1], "p1 should be Dead");
}

#[test]
fn returning_boomerang_also_kills_on_overlap() {
    let mut app = build_two_player_app();
    app.update();

    // Place the Returning boomerang east of p1 so that this tick's
    // recall_boomerangs (homing toward p0 at origin) + boomerang_physics
    // step lands the boom *on* p1's rect rather than overshooting past
    // it. Recall speed is 28 cm/tick and p0/p1 are 100 cm apart on the
    // x axis, so starting at (128, 0) puts the post-physics position at
    // (100, 0) — center of p1.
    app.world_mut().spawn((
        Boomerang {
            owner_handle: 0,
            state: BoomerangState::Returning { since: 0 },
        },
        PositionF(Vec2F::from_cm(128, 0)),
        PreviousPositionF(Vec2F::from_cm(128, 0)),
        VelocityF(Vec2F::ZERO),
    ));

    app.update();

    assert_eq!(count_boomerangs(&mut app), 0);
    assert_eq!(dead_handles(&mut app), vec![1]);
}

#[test]
fn boomerang_far_from_players_does_not_kill_anyone() {
    let mut app = build_two_player_app();
    app.update();

    // Boomerang in dead space — nowhere near either player.
    app.world_mut().spawn((
        Boomerang {
            owner_handle: 0,
            state: BoomerangState::Flying,
        },
        PositionF(Vec2F::from_cm(300, 300)),
        PreviousPositionF(Vec2F::from_cm(300, 300)),
        VelocityF(Vec2F::ZERO),
    ));

    app.update();

    assert_eq!(count_boomerangs(&mut app), 1);
    assert!(dead_handles(&mut app).is_empty());
}

// ---- Stun immunity ----

#[test]
fn stunned_player_is_immune_to_hits() {
    let mut app = build_two_player_app();
    app.update();

    // Bump p1's StunFrames so the inbound hit ought to be ignored.
    let mut q = app.world_mut().query::<(&Player, &mut StunFrames)>();
    for (p, mut stun) in q.iter_mut(app.world_mut()) {
        if p.handle == 1 {
            *stun = StunFrames(5);
        }
    }

    app.world_mut().spawn((
        Boomerang {
            owner_handle: 0,
            state: BoomerangState::Flying,
        },
        PositionF(Vec2F::from_cm(100, 0)),
        PreviousPositionF(Vec2F::from_cm(100, 0)),
        VelocityF(Vec2F::ZERO),
    ));

    app.update();

    assert_eq!(count_boomerangs(&mut app), 1, "stun should block the hit");
    assert!(
        dead_handles(&mut app).is_empty(),
        "stunned player should not die",
    );
}

// ---- Already-Dead immunity (no double-kill) ----

#[test]
fn already_dead_player_is_not_re_hit() {
    let mut app = build_two_player_app();
    app.update();

    // Pre-mark p1 as Dead. The Without<Dead> filter on the players query
    // in hit_boomerang_player should skip them entirely.
    let p1_entity = {
        let mut q = app.world_mut().query::<(Entity, &Player)>();
        q.iter(app.world())
            .find(|(_, p)| p.handle == 1)
            .map(|(e, _)| e)
            .expect("p1 entity")
    };
    app.world_mut().entity_mut(p1_entity).insert(Dead {
        respawn_at_frame: Some(5000),
    });

    app.world_mut().spawn((
        Boomerang {
            owner_handle: 0,
            state: BoomerangState::Flying,
        },
        PositionF(Vec2F::from_cm(100, 0)),
        PreviousPositionF(Vec2F::from_cm(100, 0)),
        VelocityF(Vec2F::ZERO),
    ));

    app.update();

    // Boomerang did NOT despawn (the dead player is filtered out before
    // the overlap check ever runs).
    assert_eq!(count_boomerangs(&mut app), 1);
    // p1 still has its original Dead with respawn_at_frame == 5000.
    let d = dead_for(&mut app, 1).expect("p1 still Dead");
    assert_eq!(d.respawn_at_frame, Some(5000));
}

// ---- respawn_at_frame computed from FrameCount ----

#[test]
fn respawn_at_frame_is_current_frame_plus_respawn_window() {
    let mut app = build_two_player_app();
    app.update(); // warmup tick — FrameCount advances to 1.

    app.world_mut().spawn((
        Boomerang {
            owner_handle: 0,
            state: BoomerangState::Flying,
        },
        PositionF(Vec2F::from_cm(100, 0)),
        PreviousPositionF(Vec2F::from_cm(100, 0)),
        VelocityF(Vec2F::ZERO),
    ));

    app.update();

    let fc_after = app.world().resource::<FrameCount>().0;
    let d = dead_for(&mut app, 1).expect("p1 Dead");
    // hit_boomerang_player ran when FrameCount.0 was fc_after - 1 (the
    // very last system in the tick is advance_frame_count). So the
    // recorded respawn_at_frame is (fc_after - 1) + RESPAWN_FRAMES.
    assert_eq!(d.respawn_at_frame, Some(fc_after - 1 + RESPAWN_FRAMES));
}

// ---- MatchScore increment on kill (cycle 3) ----

#[test]
fn match_score_starts_at_zero() {
    let mut app = build_two_player_app();
    app.update();
    let score = *app.world().resource::<MatchScore>();
    assert_eq!(score, MatchScore { p0: 0, p1: 0 });
}

#[test]
fn p0_kill_increments_p0_score_only() {
    let mut app = build_two_player_app();
    app.update();

    // p0 throws — boomerang owned by handle 0 lands a kill on p1.
    app.world_mut().spawn((
        Boomerang {
            owner_handle: 0,
            state: BoomerangState::Flying,
        },
        PositionF(Vec2F::from_cm(100, 0)),
        PreviousPositionF(Vec2F::from_cm(100, 0)),
        VelocityF(Vec2F::ZERO),
    ));

    app.update();
    let score = *app.world().resource::<MatchScore>();
    assert_eq!(score, MatchScore { p0: 1, p1: 0 });
}

#[test]
fn p1_kill_increments_p1_score_only() {
    let mut app = build_two_player_app();
    app.update();

    // p1's boomerang on top of p0.
    app.world_mut().spawn((
        Boomerang {
            owner_handle: 1,
            state: BoomerangState::Flying,
        },
        PositionF(Vec2F::ZERO),
        PreviousPositionF(Vec2F::ZERO),
        VelocityF(Vec2F::ZERO),
    ));

    app.update();
    let score = *app.world().resource::<MatchScore>();
    assert_eq!(score, MatchScore { p0: 0, p1: 1 });
}

#[test]
fn missed_throw_does_not_change_score() {
    let mut app = build_two_player_app();
    app.update();

    // Boomerang in dead space — no overlap with anyone.
    app.world_mut().spawn((
        Boomerang {
            owner_handle: 0,
            state: BoomerangState::Flying,
        },
        PositionF(Vec2F::from_cm(300, 300)),
        PreviousPositionF(Vec2F::from_cm(300, 300)),
        VelocityF(Vec2F::ZERO),
    ));

    app.update();
    let score = *app.world().resource::<MatchScore>();
    assert_eq!(score, MatchScore { p0: 0, p1: 0 });
}

// ---- Hit-stop on killer (cycle 4) ----

fn stun_for(app: &mut App, handle: usize) -> u32 {
    let mut q = app.world_mut().query::<(&Player, &StunFrames)>();
    q.iter(app.world())
        .find(|(p, _)| p.handle == handle)
        .map(|(_, s)| s.0)
        .expect("player entity")
}

#[test]
fn successful_kill_bumps_killer_stun_to_hit_stop_frames() {
    let mut app = build_two_player_app();
    app.update();

    // Sanity: nobody is stunned to start.
    assert_eq!(stun_for(&mut app, 0), 0);
    assert_eq!(stun_for(&mut app, 1), 0);

    // p0 throws, lands the kill on p1.
    app.world_mut().spawn((
        Boomerang {
            owner_handle: 0,
            state: BoomerangState::Flying,
        },
        PositionF(Vec2F::from_cm(100, 0)),
        PreviousPositionF(Vec2F::from_cm(100, 0)),
        VelocityF(Vec2F::ZERO),
    ));
    app.update();

    // tick_player_timers runs after hit_boomerang_player and decrements
    // StunFrames by 1, so the post-update value is HIT_STOP_FRAMES - 1.
    assert_eq!(
        stun_for(&mut app, 0),
        HIT_STOP_FRAMES - 1,
        "killer should be hit-stopped",
    );
}

#[test]
fn hit_stop_does_not_truncate_longer_existing_stun() {
    let mut app = build_two_player_app();
    app.update();

    // Pre-stun p0 with a longer window than HIT_STOP_FRAMES (mimics
    // mid-dash i-frames). The kill must NOT shorten this — `.max()`.
    let p0_entity = {
        let mut q = app.world_mut().query::<(Entity, &Player)>();
        q.iter(app.world())
            .find(|(_, p)| p.handle == 0)
            .map(|(e, _)| e)
            .expect("p0 entity")
    };
    let big_stun = HIT_STOP_FRAMES + 5;
    app.world_mut()
        .entity_mut(p0_entity)
        .insert(StunFrames(big_stun));

    app.world_mut().spawn((
        Boomerang {
            owner_handle: 0,
            state: BoomerangState::Flying,
        },
        PositionF(Vec2F::from_cm(100, 0)),
        PreviousPositionF(Vec2F::from_cm(100, 0)),
        VelocityF(Vec2F::ZERO),
    ));
    app.update();

    // Existing stun was big_stun, decremented once by tick_player_timers.
    assert_eq!(stun_for(&mut app, 0), big_stun - 1);
}

#[test]
fn missed_throw_does_not_apply_hit_stop() {
    let mut app = build_two_player_app();
    app.update();

    // Boomerang in dead space. No kill, no hit-stop.
    app.world_mut().spawn((
        Boomerang {
            owner_handle: 0,
            state: BoomerangState::Flying,
        },
        PositionF(Vec2F::from_cm(300, 300)),
        PreviousPositionF(Vec2F::from_cm(300, 300)),
        VelocityF(Vec2F::ZERO),
    ));
    app.update();
    assert_eq!(stun_for(&mut app, 0), 0);
}

// ---- Dead-player gating: dead players don't move/dash/throw ----

#[test]
fn dead_player_does_not_move_with_stick_input() {
    use sim::{PlayerInput, SynthesizedInputs};

    let mut app = build_two_player_app();
    app.update();

    // Mark p1 as Dead.
    let p1_entity = {
        let mut q = app.world_mut().query::<(Entity, &Player)>();
        q.iter(app.world())
            .find(|(_, p)| p.handle == 1)
            .map(|(e, _)| e)
            .expect("p1 entity")
    };
    app.world_mut().entity_mut(p1_entity).insert(Dead {
        respawn_at_frame: Some(5000),
    });

    // Drive full-east stick. p1's position must not change tick over tick.
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 127,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    let pos_before = app
        .world_mut()
        .query::<(&Player, &PositionF)>()
        .iter(app.world())
        .find(|(p, _)| p.handle == 1)
        .map(|(_, pos)| pos.0)
        .expect("p1 pos");

    app.update();
    app.update();

    let pos_after = app
        .world_mut()
        .query::<(&Player, &PositionF)>()
        .iter(app.world())
        .find(|(p, _)| p.handle == 1)
        .map(|(_, pos)| pos.0)
        .expect("p1 pos");

    assert_eq!(pos_before, pos_after, "Dead player must not move");
}

// ---- Dash-as-melee (2026-06-30 charge pass) ----

/// Query helper: the entity of a given player handle.
fn player_entity(app: &mut App, handle: usize) -> Entity {
    let mut q = app.world_mut().query::<(Entity, &Player)>();
    q.iter(app.world())
        .find(|(_, p)| p.handle == handle)
        .map(|(e, _)| e)
        .expect("player entity")
}

#[test]
fn dash_into_opponent_is_a_melee_kill() {
    let mut app = build_two_player_app();
    app.update();
    let p0e = player_entity(&mut app, 0);
    let p1e = player_entity(&mut app, 1);
    // p0 is mid-dash east, positioned so the dash step (DASH_SPEED=46) lands
    // it overlapping p1 (at 100): 70 + 46 = 116, within the 32 cm hitbox.
    app.world_mut().entity_mut(p0e).insert((
        PositionF(Vec2F::from_cm(70, 0)),
        DashState::Dashing {
            frames_remaining: 5,
            dir: Vec2F::from_cm(1, 0),
        },
        StunFrames(5), // the dasher's own i-frames
    ));
    app.world_mut()
        .entity_mut(p1e)
        .insert((PositionF(Vec2F::from_cm(100, 0)), StunFrames(0)));
    app.update();
    assert_eq!(
        dead_handles(&mut app),
        vec![1],
        "a dash into a non-i-frame opponent is a melee kill",
    );
}

#[test]
fn dash_versus_dash_clashes_no_kill() {
    let mut app = build_two_player_app();
    app.update();
    let p0e = player_entity(&mut app, 0);
    let p1e = player_entity(&mut app, 1);
    // Both dashing toward each other, overlapping — both carry dash i-frames, so
    // the melee clashes and neither dies.
    app.world_mut().entity_mut(p0e).insert((
        PositionF(Vec2F::from_cm(95, 0)),
        DashState::Dashing {
            frames_remaining: 5,
            dir: Vec2F::from_cm(1, 0),
        },
        StunFrames(5),
    ));
    app.world_mut().entity_mut(p1e).insert((
        PositionF(Vec2F::from_cm(105, 0)),
        DashState::Dashing {
            frames_remaining: 5,
            dir: Vec2F::from_cm(-1, 0),
        },
        StunFrames(5),
    ));
    app.update();
    assert!(
        dead_handles(&mut app).is_empty(),
        "two dashers clash — both are invincible, neither dies",
    );
}

// ---- Loose-fang persistence + opponent steal ----

fn capacity_of(app: &mut App, handle: usize) -> u32 {
    let e = player_entity(app, handle);
    app.world().entity(e).get::<ThrowCapacity>().unwrap().0
}

#[test]
fn opponent_steals_a_loose_fang_as_a_second_boomerang() {
    let mut app = build_two_player_app();
    app.update();
    let p1e = player_entity(&mut app, 1);
    // Park p1 on top of a loose fang owned by p0, far from p0 (origin).
    app.world_mut()
        .entity_mut(p1e)
        .insert(PositionF(Vec2F::from_cm(300, 0)));
    app.world_mut().spawn((
        Boomerang {
            owner_handle: 0,
            state: BoomerangState::Loose,
        },
        PositionF(Vec2F::from_cm(300, 0)),
        PreviousPositionF(Vec2F::from_cm(300, 0)),
        VelocityF(Vec2F::ZERO),
    ));
    assert_eq!(capacity_of(&mut app, 1), 1, "p1 starts with one boomerang");
    app.update();
    assert_eq!(
        count_boomerangs(&mut app),
        0,
        "the loose fang is picked up (stolen) on walk-over",
    );
    assert_eq!(
        capacity_of(&mut app, 1),
        2,
        "the opponent gains it as a SECOND boomerang (+1 capacity)",
    );
    assert!(
        dead_handles(&mut app).is_empty(),
        "walking over a loose fang steals it — it does NOT kill",
    );
}

#[test]
fn loose_fang_persists_until_claimed() {
    let mut app = build_two_player_app();
    app.update();
    // A loose fang lying in an empty corner, owned by p0, nobody near it.
    app.world_mut().spawn((
        Boomerang {
            owner_handle: 0,
            state: BoomerangState::Loose,
        },
        PositionF(Vec2F::from_cm(400, 400)),
        PreviousPositionF(Vec2F::from_cm(400, 400)),
        VelocityF(Vec2F::ZERO),
    ));
    // The old model despawned a loose fang after 180 frames; now it must stay.
    for _ in 0..300 {
        app.update();
    }
    assert_eq!(
        count_boomerangs(&mut app),
        1,
        "a loose fang lies on the ground indefinitely until recalled/caught/stolen",
    );
}
