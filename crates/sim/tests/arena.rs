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
    ArenaId, BONE_PYRE_HALF_EXTENT_CM, BonePyre, Boomerang, BoomerangState, GgrsCfg, PositionF,
    PreviousPositionF, SimPlugin, SimSnapshot, VelocityF, arena_pyres_for, boomerang_pyre_collision,
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
    BonePyre {
        rect: RectF::from_center_half_extents(Vec2F::from_cm(cx_cm, cy_cm), Vec2F::new(h, h)),
        shattered: false,
    }
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
fn other_arenas_have_no_pyres_yet() {
    assert!(arena_pyres_for(ArenaId::Crossing).is_empty());
    assert!(arena_pyres_for(ArenaId::Reliquary).is_empty());
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
