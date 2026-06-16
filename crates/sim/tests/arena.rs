//! Phase 16 cycle 1 (re-land): arena infrastructure — BonePyre cover.
//!
//! Re-implements the reverted `58fd4ab` against current code. The snapshot
//! round-trip uses the post-`1204aa9` mutate-in-place pattern (NOT the
//! despawn/respawn the reverted diff used), so pyre rollback IDs don't churn.
//!
//! Mirrors the standard sim-test ceremony (MinimalPlugins + GgrsPlugin +
//! SimPlugin) so `#[require(Rollback)]` resolves, then drives the pyre
//! collision system + snapshot round-trip directly.

use core::time::Duration;

use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy_ggrs::GgrsPlugin;
use bevy_ggrs::prelude::*;
use fixed_math::{Fix, RectF, Vec2F};
use sim::{
    ArenaId, BONE_PYRE_HALF_EXTENT_CM, BRIDGE_DURATION_FRAMES, BonePyre, Boomerang, BoomerangState,
    BridgeState, CHAIN_IGNITION_DELAY_FRAMES, DOOR_COOLDOWN_FRAMES, Dead, DoorCooldown, FrameCount,
    GgrsCfg, MatchScore, MatchState, Player, PositionF, PreviousPositionF, RESPAWN_FRAMES,
    SelectedArena, SimPlugin, SimSnapshot, VelocityF, arena_pyres_for, boomerang_pyre_collision,
    boomerang_sigil_collision, chain_ignition, chasm_kills, crossing_sigils, reliquary_doors,
    sigil_door_teleport,
};

fn bare_app() -> App {
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
    app.insert_resource(Session::SyncTest(session));
    app
}

fn pyre_at(cx_cm: i32, cy_cm: i32) -> BonePyre {
    let h = Fix::const_from_int(BONE_PYRE_HALF_EXTENT_CM);
    BonePyre::intact(RectF::from_center_half_extents(
        Vec2F::from_cm(cx_cm, cy_cm),
        Vec2F::new(h, h),
    ))
}

#[test]
fn arena_pyres_for_anchor_is_mirror_symmetric() {
    let pyres = arena_pyres_for(ArenaId::Anchor);
    assert!(!pyres.is_empty(), "Anchor has at least one pyre");
    for p in &pyres {
        assert_eq!(
            p.rect.min.x + p.rect.max.x,
            Fix::const_from_int(0),
            "pyre must be mirror-symmetric about x=0"
        );
    }
}

#[test]
fn crossing_has_no_pyres() {
    // The chasm owns the Crossing centre.
    assert!(arena_pyres_for(ArenaId::Crossing).is_empty());
}

#[test]
fn reliquary_has_two_chain_linked_pyres() {
    let pyres = arena_pyres_for(ArenaId::Reliquary);
    assert_eq!(pyres.len(), 2);
    assert!(pyres.iter().all(|p| p.chain_group == 1), "both pyres share a chain group");
    assert!(pyres.iter().all(|p| !p.shattered && p.chain_delay.is_none()));
    // Mirror-symmetric about x=0.
    assert_eq!(
        pyres[0].rect.min.x + pyres[1].rect.max.x,
        Fix::const_from_int(0)
    );
}

#[test]
fn flying_boomerang_ricochets_and_shatters_pyre() {
    let mut app = bare_app();
    app.world_mut().spawn(pyre_at(0, 0));
    let bm = app
        .world_mut()
        .spawn((
            Boomerang {
                owner_handle: 0,
                state: BoomerangState::Flying,
            },
            PositionF(Vec2F::from_cm(20, 0)),
            PreviousPositionF(Vec2F::from_cm(20, 0)),
            VelocityF(Vec2F::from_cm(50, 0)),
        ))
        .id();
    app.world_mut()
        .run_system_once(boomerang_pyre_collision)
        .unwrap();

    let mut q = app.world_mut().query::<&BonePyre>();
    assert!(
        q.iter(app.world()).all(|p| p.shattered),
        "pyre shatters on impact"
    );
    let vx = app.world().entity(bm).get::<VelocityF>().unwrap().0.x;
    assert!(vx < Fix::const_from_int(0), "x velocity reflected to negative");
}

#[test]
fn shattered_pyre_does_not_block() {
    let mut app = bare_app();
    let mut pyre = pyre_at(0, 0);
    pyre.shattered = true;
    app.world_mut().spawn(pyre);
    let bm = app
        .world_mut()
        .spawn((
            Boomerang {
                owner_handle: 0,
                state: BoomerangState::Flying,
            },
            PositionF(Vec2F::from_cm(20, 0)),
            PreviousPositionF(Vec2F::from_cm(20, 0)),
            VelocityF(Vec2F::from_cm(50, 0)),
        ))
        .id();
    app.world_mut()
        .run_system_once(boomerang_pyre_collision)
        .unwrap();

    let vx = app.world().entity(bm).get::<VelocityF>().unwrap().0.x;
    assert_eq!(
        vx,
        Fix::const_from_int(50),
        "a shattered pyre must not deflect"
    );
}

#[test]
fn returning_boomerang_phases_through_pyre() {
    let mut app = bare_app();
    app.world_mut().spawn(pyre_at(0, 0));
    let bm = app
        .world_mut()
        .spawn((
            Boomerang {
                owner_handle: 0,
                state: BoomerangState::Returning,
            },
            PositionF(Vec2F::from_cm(20, 0)),
            PreviousPositionF(Vec2F::from_cm(20, 0)),
            VelocityF(Vec2F::from_cm(50, 0)),
        ))
        .id();
    app.world_mut()
        .run_system_once(boomerang_pyre_collision)
        .unwrap();

    let mut q = app.world_mut().query::<&BonePyre>();
    assert!(
        q.iter(app.world()).all(|p| !p.shattered),
        "a returning boomerang phases through and does not shatter"
    );
    let vx = app.world().entity(bm).get::<VelocityF>().unwrap().0.x;
    assert_eq!(vx, Fix::const_from_int(50), "returning boomerang not deflected");
}

#[test]
fn snapshot_round_trips_pyre_shatter() {
    let mut app = bare_app();
    app.world_mut().spawn(pyre_at(0, 0));

    let intact = SimSnapshot::capture(app.world_mut());
    assert_eq!(intact.pyres.len(), 1, "snapshot captures the pyre");
    assert!(!intact.pyres[0].pyre.shattered);

    // Shatter it, then restore the intact snapshot.
    {
        let mut q = app.world_mut().query::<&mut BonePyre>();
        for mut p in q.iter_mut(app.world_mut()) {
            p.shattered = true;
        }
    }
    intact.restore(app.world_mut());

    let mut q = app.world_mut().query::<&BonePyre>();
    assert!(
        q.iter(app.world()).all(|p| !p.shattered),
        "restoring an intact snapshot un-shatters the pyre"
    );
    assert_eq!(q.iter(app.world()).count(), 1, "no pyre churn on restore");
}

// ---- Crossing arena: blood chasm + altar-sigil bridge ----

fn crossing_app() -> App {
    let mut app = bare_app();
    *app.world_mut().resource_mut::<SelectedArena>() = SelectedArena(ArenaId::Crossing);
    *app.world_mut().resource_mut::<MatchState>() = MatchState::InRound {
        expires_at_frame: 1_000_000,
    };
    app
}

#[test]
fn crossing_chasm_kills_player_and_scores_opponent() {
    let mut app = crossing_app();
    *app.world_mut().resource_mut::<FrameCount>() = FrameCount(500);
    // P0 stands in the central chasm (x=0); P1 is safe on its side.
    app.world_mut()
        .spawn((Player { handle: 0 }, PositionF(Vec2F::from_cm(0, 0))));
    app.world_mut()
        .spawn((Player { handle: 1 }, PositionF(Vec2F::from_cm(100, 0))));

    app.world_mut().run_system_once(chasm_kills).unwrap();

    assert_eq!(app.world().resource::<MatchScore>().p1, 1, "opponent scores");
    assert_eq!(app.world().resource::<MatchScore>().p0, 0);
    let mut q = app.world_mut().query::<(&Player, &Dead)>();
    let (p0_dead, p1_dead) = {
        let mut d0 = None;
        let mut d1 = None;
        for (p, d) in q.iter(app.world()) {
            if p.handle == 0 {
                d0 = Some(d.respawn_at_frame);
            } else {
                d1 = Some(d.respawn_at_frame);
            }
        }
        (d0.unwrap(), d1.unwrap())
    };
    assert_eq!(p0_dead, Some(500 + RESPAWN_FRAMES), "chasm victim is dying");
    assert_eq!(p1_dead, None, "safe player unharmed");
}

#[test]
fn bridge_makes_chasm_safe() {
    let mut app = crossing_app();
    *app.world_mut().resource_mut::<FrameCount>() = FrameCount(500);
    *app.world_mut().resource_mut::<BridgeState>() = BridgeState {
        active_until_frame: 800,
    };
    app.world_mut()
        .spawn((Player { handle: 0 }, PositionF(Vec2F::from_cm(0, 0))));

    app.world_mut().run_system_once(chasm_kills).unwrap();

    assert_eq!(app.world().resource::<MatchScore>().p1, 0, "bridge prevents the kill");
    let mut q = app.world_mut().query::<&Dead>();
    assert!(q.iter(app.world()).all(|d| d.respawn_at_frame.is_none()));
}

#[test]
fn bridge_expires_at_exact_frame() {
    // active_until_frame is exclusive: active while frame < it.
    for (frame, expect_dead) in [(799u32, false), (800u32, true)] {
        let mut app = crossing_app();
        *app.world_mut().resource_mut::<FrameCount>() = FrameCount(frame);
        *app.world_mut().resource_mut::<BridgeState>() = BridgeState {
            active_until_frame: 800,
        };
        app.world_mut()
            .spawn((Player { handle: 0 }, PositionF(Vec2F::from_cm(0, 0))));
        app.world_mut().run_system_once(chasm_kills).unwrap();
        let died = app.world().resource::<MatchScore>().p1 == 1;
        assert_eq!(died, expect_dead, "frame {frame} bridge boundary");
    }
}

#[test]
fn sigil_hit_activates_bridge_and_ricochets() {
    let mut app = crossing_app();
    *app.world_mut().resource_mut::<FrameCount>() = FrameCount(300);
    // Approach the first altar sigil from below (clean y-axis overlap so
    // the reflection is unambiguous — a dead-centre hit ties and resolves
    // on x).
    let sigil = crossing_sigils()[0];
    let center = (sigil.min + sigil.max) * fixed_math::Fix::from_bits(1 << 15); // midpoint (×0.5)
    let pos = Vec2F::new(center.x, center.y - fixed_math::Fix::const_from_int(20));
    let bm = app
        .world_mut()
        .spawn((
            Boomerang {
                owner_handle: 0,
                state: BoomerangState::Flying,
            },
            PositionF(pos),
            PreviousPositionF(pos),
            VelocityF(Vec2F::from_cm(0, 50)),
        ))
        .id();

    app.world_mut()
        .run_system_once(boomerang_sigil_collision)
        .unwrap();

    let bridge = *app.world().resource::<BridgeState>();
    assert_eq!(
        bridge.active_until_frame,
        300 + BRIDGE_DURATION_FRAMES,
        "sigil hit raises the bridge"
    );
    let vy = app.world().entity(bm).get::<VelocityF>().unwrap().0.y;
    assert!(vy < fixed_math::Fix::const_from_int(0), "boomerang ricochets off the sigil");
}

// ---- Reliquary arena: sigil-door teleports + chain-linked pyres ----

fn reliquary_app() -> App {
    let mut app = bare_app();
    *app.world_mut().resource_mut::<SelectedArena>() = SelectedArena(ArenaId::Reliquary);
    *app.world_mut().resource_mut::<MatchState>() = MatchState::InRound {
        expires_at_frame: 1_000_000,
    };
    app
}

#[test]
fn door_teleports_player_to_paired_exit() {
    let mut app = reliquary_app();
    *app.world_mut().resource_mut::<FrameCount>() = FrameCount(100);
    let (door_a, exit_a) = reliquary_doors()[0];
    let center_a = (door_a.min + door_a.max) * fixed_math::Fix::from_bits(1 << 15);
    let e = app
        .world_mut()
        .spawn((
            Player { handle: 0 },
            PositionF(center_a),
            PreviousPositionF(center_a),
        ))
        .id();

    app.world_mut().run_system_once(sigil_door_teleport).unwrap();

    assert_eq!(
        app.world().entity(e).get::<PositionF>().unwrap().0,
        exit_a,
        "teleported to the paired exit"
    );
    assert_eq!(
        app.world().entity(e).get::<PreviousPositionF>().unwrap().0,
        exit_a,
        "prev snapped too — no interpolation streak across the teleport"
    );
    assert_eq!(
        app.world().resource::<DoorCooldown>().until_frame,
        100 + DOOR_COOLDOWN_FRAMES
    );
}

#[test]
fn door_cooldown_blocks_immediate_reentry() {
    let mut app = reliquary_app();
    *app.world_mut().resource_mut::<FrameCount>() = FrameCount(100);
    *app.world_mut().resource_mut::<DoorCooldown>() = DoorCooldown { until_frame: 200 };
    let (door_a, _) = reliquary_doors()[0];
    let center_a = (door_a.min + door_a.max) * fixed_math::Fix::from_bits(1 << 15);
    let e = app
        .world_mut()
        .spawn((
            Player { handle: 0 },
            PositionF(center_a),
            PreviousPositionF(center_a),
        ))
        .id();

    app.world_mut().run_system_once(sigil_door_teleport).unwrap();

    assert_eq!(
        app.world().entity(e).get::<PositionF>().unwrap().0,
        center_a,
        "cooldown blocks the teleport"
    );
}

#[test]
fn chain_ignition_fires_after_delay() {
    let mut app = reliquary_app();
    let mut shattered = pyre_at(-200, 0);
    shattered.chain_group = 1;
    shattered.shattered = true;
    app.world_mut().spawn(shattered);
    let mut linked = pyre_at(200, 0);
    linked.chain_group = 1;
    let e = app.world_mut().spawn(linked).id();

    // Frame 500 arms the fuse on the intact group-mate.
    *app.world_mut().resource_mut::<FrameCount>() = FrameCount(500);
    app.world_mut().run_system_once(chain_ignition).unwrap();
    let p = *app.world().entity(e).get::<BonePyre>().unwrap();
    assert_eq!(p.chain_delay, Some(500 + CHAIN_IGNITION_DELAY_FRAMES));
    assert!(!p.shattered, "armed but not yet ignited");

    // At the exact ignition frame it shatters.
    *app.world_mut().resource_mut::<FrameCount>() = FrameCount(500 + CHAIN_IGNITION_DELAY_FRAMES);
    app.world_mut().run_system_once(chain_ignition).unwrap();
    let p = *app.world().entity(e).get::<BonePyre>().unwrap();
    assert!(p.shattered, "chain fires at the delay");
    assert_eq!(p.chain_delay, None, "fuse consumed");
}

#[test]
fn chain_delay_survives_snapshot_restore() {
    let mut app = reliquary_app();
    let mut shattered = pyre_at(-200, 0);
    shattered.chain_group = 1;
    shattered.shattered = true;
    app.world_mut().spawn(shattered);
    let mut linked = pyre_at(200, 0);
    linked.chain_group = 1;
    let e = app.world_mut().spawn(linked).id();

    *app.world_mut().resource_mut::<FrameCount>() = FrameCount(500);
    app.world_mut().run_system_once(chain_ignition).unwrap();
    // Capture mid-fuse.
    let mid = SimSnapshot::capture(app.world_mut());

    // Force the fuse to fire, then restore the mid-fuse snapshot.
    *app.world_mut().resource_mut::<FrameCount>() = FrameCount(900);
    app.world_mut().run_system_once(chain_ignition).unwrap();
    assert!(app.world().entity(e).get::<BonePyre>().unwrap().shattered);
    mid.restore(app.world_mut());

    let mut q = app.world_mut().query::<&BonePyre>();
    let linked = q
        .iter(app.world())
        .find(|p| !p.shattered)
        .expect("the linked pyre is intact again after restore");
    assert_eq!(
        linked.chain_delay,
        Some(500 + CHAIN_IGNITION_DELAY_FRAMES),
        "restore brings back the armed fuse"
    );
}
