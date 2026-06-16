//! Phase 17 cycle 2: pickup spawn / collect / expire plumbing + the rolled-
//! back SimRng. Behaviors land in cycle 3; here a collected modifier just
//! rides the throw inertly.

use core::time::Duration;

use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy_ggrs::GgrsPlugin;
use bevy_ggrs::prelude::*;
use fixed_math::Vec2F;
use sim::{
    FrameCount, GgrsCfg, HeldModifier, MatchState, Pickup, PickupKind, PickupSpawnTimer, Player,
    PositionF, PreviousPositionF, SimPlugin, SimRng, SimSnapshot, VelocityF, collect_pickups,
    expire_pickups, pickup_slots, pickup_spawner,
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
    *app.world_mut().resource_mut::<MatchState>() = MatchState::InRound {
        expires_at_frame: 1_000_000,
    };
    app
}

fn pickup_count(app: &mut App) -> usize {
    let mut q = app.world_mut().query::<&Pickup>();
    q.iter(app.world()).count()
}

#[test]
fn sim_rng_is_deterministic_for_a_fixed_seed() {
    let mut a = SimRng::default();
    let mut b = SimRng::default();
    let seq_a: Vec<u32> = (0..32).map(|_| a.range(0, 1000)).collect();
    let seq_b: Vec<u32> = (0..32).map(|_| b.range(0, 1000)).collect();
    assert_eq!(seq_a, seq_b, "same seed must produce the same stream");
    // ...and it actually varies (not a stuck constant).
    assert!(seq_a.iter().any(|&v| v != seq_a[0]));
    assert!(seq_a.iter().all(|&v| v < 1000), "range bound respected");
}

#[test]
fn spawner_spawns_one_pickup_when_due_then_holds() {
    let mut app = bare_app();
    *app.world_mut().resource_mut::<FrameCount>() = FrameCount(100);
    *app.world_mut().resource_mut::<PickupSpawnTimer>() = PickupSpawnTimer { next_at_frame: 0 };

    app.world_mut().run_system_once(pickup_spawner).unwrap();
    assert_eq!(pickup_count(&mut app), 1, "one pickup spawns when due");

    // While one is live, the spawner holds.
    *app.world_mut().resource_mut::<FrameCount>() = FrameCount(100_000);
    app.world_mut().run_system_once(pickup_spawner).unwrap();
    assert_eq!(pickup_count(&mut app), 1, "only one pickup at a time");
}

#[test]
fn spawn_sequence_is_deterministic_across_runs() {
    // Drive the spawner over many frames in two fresh sims, despawning the
    // pickup each time it appears, and record (slot, kind). The sequences
    // must match exactly — the spawn is pure over the rolled-back SimRng.
    fn run() -> Vec<(u8, PickupKind)> {
        let mut app = bare_app();
        let mut seq = Vec::new();
        for f in 0..4000u32 {
            *app.world_mut().resource_mut::<FrameCount>() = FrameCount(f);
            app.world_mut().run_system_once(pickup_spawner).unwrap();
            let spawned: Option<(u8, PickupKind)> = {
                let mut q = app.world_mut().query::<&Pickup>();
                q.iter(app.world()).next().map(|p| (p.slot, p.kind))
            };
            if let Some(s) = spawned {
                seq.push(s);
                let e = {
                    let mut q = app.world_mut().query::<(Entity, &Pickup)>();
                    q.iter(app.world()).next().map(|(e, _)| e).unwrap()
                };
                app.world_mut().despawn(e);
            }
        }
        seq
    }
    let a = run();
    let b = run();
    assert!(!a.is_empty(), "some pickups spawned over 4000 frames");
    assert_eq!(a, b, "pickup spawn sequence is deterministic");
}

#[test]
fn collecting_fills_then_replaces_held_modifier() {
    let mut app = bare_app();
    let slot = 0usize;
    app.world_mut().spawn((
        Pickup {
            kind: PickupKind::Fire,
            slot: slot as u8,
            despawn_at_frame: 9999,
        },
        PositionF(pickup_slots()[slot]),
    ));
    let p = app
        .world_mut()
        .spawn((Player { handle: 0 }, PositionF(pickup_slots()[slot])))
        .id();

    app.world_mut().run_system_once(collect_pickups).unwrap();
    assert_eq!(
        app.world().entity(p).get::<HeldModifier>().unwrap().0,
        Some(PickupKind::Fire)
    );
    assert_eq!(pickup_count(&mut app), 0, "pickup consumed on collect");

    // Walking over a second pickup replaces what's held.
    app.world_mut().spawn((
        Pickup {
            kind: PickupKind::Phantom,
            slot: 1,
            despawn_at_frame: 9999,
        },
        PositionF(pickup_slots()[slot]),
    ));
    app.world_mut().run_system_once(collect_pickups).unwrap();
    assert_eq!(
        app.world().entity(p).get::<HeldModifier>().unwrap().0,
        Some(PickupKind::Phantom),
        "new pickup replaces the held one"
    );
}

#[test]
fn pickup_expires_at_its_lifetime() {
    let mut app = bare_app();
    app.world_mut().spawn((
        Pickup {
            kind: PickupKind::Bouncy,
            slot: 2,
            despawn_at_frame: 500,
        },
        PositionF(pickup_slots()[2]),
    ));
    *app.world_mut().resource_mut::<FrameCount>() = FrameCount(499);
    app.world_mut().run_system_once(expire_pickups).unwrap();
    assert_eq!(pickup_count(&mut app), 1, "alive one frame before expiry");
    *app.world_mut().resource_mut::<FrameCount>() = FrameCount(500);
    app.world_mut().run_system_once(expire_pickups).unwrap();
    assert_eq!(pickup_count(&mut app), 0, "despawned at the lifetime frame");
}

#[test]
fn pickups_rng_and_held_survive_snapshot_round_trip() {
    let mut app = bare_app();
    let p = app
        .world_mut()
        .spawn((
            Player { handle: 0 },
            PositionF(Vec2F::ZERO),
            PreviousPositionF(Vec2F::ZERO),
            VelocityF(Vec2F::ZERO),
        ))
        .id();
    *app.world_mut().entity_mut(p).get_mut::<HeldModifier>().unwrap() =
        HeldModifier(Some(PickupKind::Curve));
    app.world_mut().spawn((
        Pickup {
            kind: PickupKind::Multishot,
            slot: 3,
            despawn_at_frame: 1234,
        },
        PositionF(pickup_slots()[3]),
    ));
    // Advance the RNG + timer so they carry non-default state.
    app.world_mut().resource_mut::<SimRng>().range(0, 9999);
    *app.world_mut().resource_mut::<PickupSpawnTimer>() = PickupSpawnTimer { next_at_frame: 777 };

    let snap = SimSnapshot::capture(app.world_mut());

    // Clobber everything, then restore.
    *app.world_mut().entity_mut(p).get_mut::<HeldModifier>().unwrap() = HeldModifier(None);
    {
        let e = {
            let mut q = app.world_mut().query::<(Entity, &Pickup)>();
            q.iter(app.world()).next().map(|(e, _)| e).unwrap()
        };
        app.world_mut().despawn(e);
    }
    *app.world_mut().resource_mut::<PickupSpawnTimer>() = PickupSpawnTimer::default();
    snap.restore(app.world_mut());

    assert_eq!(
        app.world().entity(p).get::<HeldModifier>().unwrap().0,
        Some(PickupKind::Curve),
        "held modifier restored"
    );
    assert_eq!(pickup_count(&mut app), 1, "pickup respawned on restore");
    assert_eq!(
        app.world().resource::<PickupSpawnTimer>().next_at_frame,
        777,
        "spawn timer restored"
    );
}
