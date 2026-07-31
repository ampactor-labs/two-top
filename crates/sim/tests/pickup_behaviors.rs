//! Phase 17 cycle 3: the five flight-modifying pickup behaviors driven
//! directly through their systems (Fire/Heavy throw speed, Phantom phase,
//! Heavy plow, Bouncy acceleration, Curve bend). Multishot + Fire-trail land
//! in their own cycles.

use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy_ggrs::GgrsPlugin;
use bevy_ggrs::prelude::*;
use fixed_math::{Fix, RectF, Vec2F};
use sim::{
    BONE_PYRE_HALF_EXTENT_CM, BOUNCY_MAX_SPEED_CM_PER_TICK, BonePyre, Boomerang, BoomerangMods,
    BoomerangState, Dead, DefaultInputsPlugin, FIRE_TRAIL_LIFETIME_FRAMES, FireTrailCell,
    FrameCount, GgrsCfg, HeldModifier, InfiniteRoundPlugin, MULTISHOT_SECONDARY_LIFETIME_FRAMES,
    MatchScore, PickupKind, Player, PlayerInput, PositionF, PreviousPositionF, SimPlugin,
    SimSnapshot, StunFrames, SynthesizedInputs, THROW_SPEED_CM_PER_TICK, VelocityF, Wall, WallKind,
    boomerang_pyre_collision, boomerang_wall_collision, curve_boomerangs, drop_fire_trail,
    expire_fire_trail, expire_secondary_boomerangs, fire_trail_kills, modified_throw_speed,
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
    app.insert_resource(TimeUpdateStrategy::ManualDuration(sim::tick_duration()));
    app.add_plugins(GgrsPlugin::<GgrsCfg>::default());
    app.add_plugins(SimPlugin);
    app.insert_resource(Session::SyncTest(session));
    app
}

fn spawn_mod(app: &mut App, modifier: PickupKind, pos: Vec2F, vel: Vec2F) -> Entity {
    app.world_mut()
        .spawn((
            Boomerang {
                owner_handle: 0,
                state: BoomerangState::Flying,
            },
            BoomerangMods {
                modifier: Some(modifier),
                is_secondary: false,
                despawn_at_frame: None,
                wall_bounces: 0,
            },
            PositionF(pos),
            PreviousPositionF(pos),
            VelocityF(vel),
        ))
        .id()
}

fn vel_of(app: &App, e: Entity) -> Vec2F {
    app.world().entity(e).get::<VelocityF>().unwrap().0
}

fn square_rect(center: Vec2F, half_cm: i32) -> RectF {
    let h = Fix::const_from_int(half_cm);
    RectF::from_center_half_extents(center, Vec2F::new(h, h))
}

#[test]
fn fire_throws_faster_heavy_slower() {
    assert_eq!(
        modified_throw_speed(false, None),
        Fix::const_from_int(THROW_SPEED_CM_PER_TICK)
    );
    assert_eq!(
        modified_throw_speed(false, Some(PickupKind::Fire)),
        Fix::const_from_int(40) // base 32 + 8
    );
    assert_eq!(
        modified_throw_speed(false, Some(PickupKind::Heavy)),
        Fix::const_from_int(25) // base 32 × 4/5
    );
    // Modifiers compose with the perfect-catch empowerment.
    assert_eq!(
        modified_throw_speed(true, Some(PickupKind::Fire)),
        Fix::const_from_int(49) // empowered 41 + 8
    );
}

#[test]
fn phantom_phases_through_walls() {
    let mut app = bare_app();
    app.world_mut().spawn(Wall {
        kind: WallKind::Obstacle,
        rect: square_rect(Vec2F::ZERO, 50),
    });
    let bm = spawn_mod(
        &mut app,
        PickupKind::Phantom,
        Vec2F::ZERO,
        Vec2F::from_cm(50, 0),
    );
    app.world_mut()
        .run_system_once(boomerang_wall_collision)
        .unwrap();
    assert_eq!(
        vel_of(&app, bm),
        Vec2F::from_cm(50, 0),
        "phantom velocity unchanged by an overlapping wall"
    );
}

#[test]
fn heavy_plows_pyre_without_ricochet() {
    let mut app = bare_app();
    app.world_mut().spawn(BonePyre::intact(square_rect(
        Vec2F::ZERO,
        BONE_PYRE_HALF_EXTENT_CM,
    )));
    let bm = spawn_mod(
        &mut app,
        PickupKind::Heavy,
        Vec2F::from_cm(20, 0),
        Vec2F::from_cm(50, 0),
    );
    app.world_mut()
        .run_system_once(boomerang_pyre_collision)
        .unwrap();

    let mut q = app.world_mut().query::<&BonePyre>();
    assert!(
        q.iter(app.world()).all(|p| p.shattered),
        "heavy still shatters the pyre"
    );
    assert_eq!(
        vel_of(&app, bm),
        Vec2F::from_cm(50, 0),
        "heavy plows straight through — no ricochet"
    );
}

#[test]
fn bouncy_gains_speed_on_ricochet_and_caps() {
    // Wall on the +x side; a +x boomerang ricochets off it.
    let mut app = bare_app();
    app.world_mut().spawn(Wall {
        kind: WallKind::Obstacle,
        rect: RectF::from_center_half_extents(
            Vec2F::from_cm(30, 0),
            Vec2F::new(Fix::const_from_int(10), Fix::const_from_int(100)),
        ),
    });
    let bm = spawn_mod(
        &mut app,
        PickupKind::Bouncy,
        Vec2F::from_cm(25, 0),
        Vec2F::from_cm(25, 0), // launch at the (halved) base throw speed
    );
    app.world_mut()
        .run_system_once(boomerang_wall_collision)
        .unwrap();
    let speed = vel_of(&app, bm).length();
    // 25 * 1.1 = 27.5 (within fixed-point slop), well under the 40 cap.
    assert!(
        speed > Fix::const_from_int(26) && speed < Fix::const_from_int(29),
        "bouncy speeds up ~10% per ricochet (got {speed:?})"
    );
    assert!(
        speed <= Fix::const_from_int(BOUNCY_MAX_SPEED_CM_PER_TICK),
        "never exceeds the cap"
    );
}

#[test]
fn curve_bends_velocity_preserving_speed() {
    let mut app = bare_app();
    let bm = spawn_mod(
        &mut app,
        PickupKind::Curve,
        Vec2F::ZERO,
        Vec2F::from_cm(50, 0),
    );
    let before = vel_of(&app, bm);
    app.world_mut().run_system_once(curve_boomerangs).unwrap();
    let after = vel_of(&app, bm);
    assert_ne!(after, before, "curve rotates the heading");
    assert!(
        after.y.abs() > Fix::from_bits(0x2000),
        "rotation bends it off the x-axis (y={:?})",
        after.y
    );
    let (sb, sa) = (before.length(), after.length());
    assert!(
        (sa - sb).abs() <= Fix::from_bits(0x400),
        "speed is preserved"
    );
}

// ---- Multishot: 3-fan throw, secondary lifecycle, recall/snapshot ----

/// A full SimPlugin app driven by synthesized inputs (1 player), so a
/// tap-release actually runs `throw_boomerangs`. Returns the player entity
/// so a held modifier can be planted before the throw.
fn full_app() -> (App, Entity) {
    let mut sb = SessionBuilder::<GgrsCfg>::new()
        .with_num_players(1)
        .unwrap()
        .with_check_distance(2)
        .with_input_delay(0);
    sb = sb.add_player(PlayerType::Local, 0).unwrap();
    let session = sb.start_synctest_session().unwrap();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(TimeUpdateStrategy::ManualDuration(sim::tick_duration()));
    app.add_plugins(GgrsPlugin::<GgrsCfg>::default());
    app.add_plugins(SimPlugin);
    app.add_plugins(InfiniteRoundPlugin);
    app.add_plugins(DefaultInputsPlugin);
    app.insert_resource(Session::SyncTest(session));

    let p = app
        .world_mut()
        .spawn((
            Player { handle: 0 },
            PositionF(Vec2F::ZERO),
            PreviousPositionF(Vec2F::ZERO),
            VelocityF(Vec2F::ZERO),
        ))
        .id();
    (app, p)
}

fn set_input(app: &mut App, stick_x: i8, buttons: u8) {
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x,
        stick_y: 0,
        aim_angle: 0,
        buttons,
    };
}

fn boom_count(app: &mut App) -> usize {
    let mut q = app.world_mut().query::<&Boomerang>();
    q.iter(app.world()).count()
}

/// Spawn one Multishot fang directly (no throw). Secondaries carry a
/// lifetime backstop the way `throw_boomerangs` sets it.
fn spawn_fang(app: &mut App, is_secondary: bool, pos: Vec2F) -> Entity {
    app.world_mut()
        .spawn((
            Boomerang {
                owner_handle: 0,
                state: BoomerangState::Flying,
            },
            BoomerangMods {
                modifier: Some(PickupKind::Multishot),
                is_secondary,
                despawn_at_frame: if is_secondary { Some(120) } else { None },
                wall_bounces: 0,
            },
            PositionF(pos),
            PreviousPositionF(pos),
            VelocityF(Vec2F::from_cm(50, 0)),
        ))
        .id()
}

fn despawn_all_booms(app: &mut App) {
    let es: Vec<Entity> = {
        let mut q = app.world_mut().query::<(Entity, &Boomerang)>();
        q.iter(app.world()).map(|(e, _)| e).collect()
    };
    for e in es {
        app.world_mut().despawn(e);
    }
}

#[test]
fn multishot_throw_spawns_three_fanned_fangs() {
    let (mut app, p) = full_app();
    app.update(); // SyncTestSession warmup
    *app.world_mut()
        .entity_mut(p)
        .get_mut::<HeldModifier>()
        .unwrap() = HeldModifier(Some(PickupKind::Multishot));
    // Hold (centered stick), then release aimed +x.
    set_input(&mut app, 0, PlayerInput::THROW_DOWN);
    app.update();
    set_input(&mut app, 127, 0);
    app.update();

    let mut q = app.world_mut().query::<(&BoomerangMods, &VelocityF)>();
    let fangs: Vec<(BoomerangMods, Vec2F)> = q.iter(app.world()).map(|(m, v)| (*m, v.0)).collect();
    assert_eq!(fangs.len(), 3, "multishot throws three fangs");

    let primaries: Vec<&(BoomerangMods, Vec2F)> =
        fangs.iter().filter(|(m, _)| !m.is_secondary).collect();
    let secondaries: Vec<&(BoomerangMods, Vec2F)> =
        fangs.iter().filter(|(m, _)| m.is_secondary).collect();
    assert_eq!(primaries.len(), 1, "exactly one recallable primary");
    assert_eq!(secondaries.len(), 2, "two fire-and-forget side-fangs");

    let primary_v = primaries[0].1;
    let pspeed = primary_v.length();
    assert!(
        primaries[0].0.despawn_at_frame.is_none(),
        "primary has no timeout"
    );
    for (m, v) in &fangs {
        assert!(
            (v.length() - pspeed).abs() <= Fix::from_bits(0x2000),
            "all three fangs share the throw speed",
        );
        if m.is_secondary {
            assert!(
                m.despawn_at_frame.is_some(),
                "side-fang has a lifetime backstop"
            );
        }
    }
    // The two side-fangs fan to OPPOSITE sides of the primary heading:
    // the cross product (primary × fang) flips sign between them.
    let cross = |a: Vec2F, b: Vec2F| a.x * b.y - a.y * b.x;
    let c0 = cross(primary_v, secondaries[0].1);
    let c1 = cross(primary_v, secondaries[1].1);
    assert!(
        c0 != Fix::ZERO && c1 != Fix::ZERO,
        "side-fangs are rotated off the primary"
    );
    assert!(
        (c0 > Fix::ZERO) != (c1 > Fix::ZERO),
        "side-fangs fan symmetrically to opposite sides",
    );
}

#[test]
fn multishot_recall_returns_only_the_primary() {
    let (mut app, p) = full_app();
    app.update();
    *app.world_mut()
        .entity_mut(p)
        .get_mut::<HeldModifier>()
        .unwrap() = HeldModifier(Some(PickupKind::Multishot));
    // Hold to full charge so the primary launches FAST — otherwise a slow fang
    // stays near the owner and gets caught on the recall tick before we can
    // observe the Returning state.
    set_input(&mut app, 0, PlayerInput::THROW_DOWN);
    for _ in 0..sim::CHARGE_MAX_FRAMES + 4 {
        app.update();
    }
    set_input(&mut app, 127, 0);
    app.update();
    assert_eq!(boom_count(&mut app), 3, "three fangs in flight");

    // Let the fangs fly out a few ticks so the recalled primary's first
    // home-step doesn't overlap the owner (catch would despawn it before
    // we observe Returning). Stick centered so the player holds at origin.
    set_input(&mut app, 0, 0);
    for _ in 0..6 {
        app.update();
    }
    // Press THROW_DOWN again — a rising edge triggers recall.
    set_input(&mut app, 0, PlayerInput::THROW_DOWN);
    app.update();

    let mut q = app.world_mut().query::<(&Boomerang, &BoomerangMods)>();
    let states: Vec<(BoomerangState, bool)> = q
        .iter(app.world())
        .map(|(b, m)| (b.state, m.is_secondary))
        .collect();
    assert!(
        states
            .iter()
            .any(|(s, sec)| !sec && matches!(s, BoomerangState::Returning { .. })),
        "recall flips the primary to Returning",
    );
    assert!(
        states
            .iter()
            .filter(|(_, sec)| *sec)
            .all(|(s, _)| matches!(s, BoomerangState::Flying)),
        "side-fangs ignore recall and keep flying",
    );
}

#[test]
fn multishot_secondary_dies_on_first_wall_primary_ricochets() {
    let mut app = bare_app();
    app.world_mut().spawn(Wall {
        kind: WallKind::Obstacle,
        rect: RectF::from_center_half_extents(
            Vec2F::from_cm(30, 0),
            Vec2F::new(Fix::const_from_int(10), Fix::const_from_int(100)),
        ),
    });
    spawn_fang(&mut app, true, Vec2F::from_cm(25, 0));
    spawn_fang(&mut app, false, Vec2F::from_cm(25, 0));
    app.world_mut()
        .run_system_once(boomerang_wall_collision)
        .unwrap();

    let mut q = app.world_mut().query::<(&BoomerangMods, &VelocityF)>();
    let survivors: Vec<(bool, Vec2F)> = q
        .iter(app.world())
        .map(|(m, v)| (m.is_secondary, v.0))
        .collect();
    assert_eq!(survivors.len(), 1, "only the primary survives the wall");
    assert!(
        !survivors[0].0,
        "the survivor is the primary, not a side-fang"
    );
    assert!(
        survivors[0].1.x < Fix::ZERO,
        "the primary ricocheted (vx flipped negative)",
    );
}

#[test]
fn multishot_secondary_expires_at_backstop_but_primary_persists() {
    let mut app = bare_app();
    app.world_mut().spawn((
        Boomerang {
            owner_handle: 0,
            state: BoomerangState::Flying,
        },
        BoomerangMods {
            modifier: Some(PickupKind::Multishot),
            is_secondary: true,
            despawn_at_frame: Some(200),
            wall_bounces: 0,
        },
        PositionF(Vec2F::ZERO),
        PreviousPositionF(Vec2F::ZERO),
        VelocityF(Vec2F::from_cm(50, 0)),
    ));
    spawn_fang(&mut app, false, Vec2F::from_cm(40, 0));

    *app.world_mut().resource_mut::<FrameCount>() = FrameCount(199);
    app.world_mut()
        .run_system_once(expire_secondary_boomerangs)
        .unwrap();
    assert_eq!(
        boom_count(&mut app),
        2,
        "alive one frame before the backstop"
    );

    *app.world_mut().resource_mut::<FrameCount>() = FrameCount(200);
    app.world_mut()
        .run_system_once(expire_secondary_boomerangs)
        .unwrap();
    let mut q = app.world_mut().query::<&BoomerangMods>();
    let left: Vec<bool> = q.iter(app.world()).map(|m| m.is_secondary).collect();
    assert_eq!(left.len(), 1, "secondary despawned at its backstop frame");
    assert!(!left[0], "the no-timeout primary persists past frame 200");
    // sanity: the constant the throw uses is the one we're modeling.
    assert_eq!(MULTISHOT_SECONDARY_LIFETIME_FRAMES, 120);
}

#[test]
fn three_same_owner_fangs_survive_snapshot_round_trip() {
    let mut app = bare_app();
    spawn_fang(&mut app, false, Vec2F::from_cm(0, 0));
    spawn_fang(&mut app, true, Vec2F::from_cm(10, 0));
    spawn_fang(&mut app, true, Vec2F::from_cm(20, 0));

    let snap = SimSnapshot::capture(app.world_mut());
    despawn_all_booms(&mut app);
    assert_eq!(boom_count(&mut app), 0);
    snap.restore(app.world_mut());

    let mut q = app.world_mut().query::<(&BoomerangMods, &PositionF)>();
    let mut primaries = 0;
    let mut secondaries = 0;
    let mut xs: Vec<i32> = Vec::new();
    for (m, pos) in q.iter(app.world()) {
        if m.is_secondary {
            secondaries += 1;
        } else {
            primaries += 1;
        }
        xs.push(pos.0.x.to_bits());
    }
    assert_eq!(primaries, 1, "one primary restored");
    assert_eq!(
        secondaries, 2,
        "two side-fangs restored — NOT collapsed onto one entity by owner_handle",
    );
    xs.sort();
    assert_eq!(
        xs,
        vec![
            Vec2F::from_cm(0, 0).x.to_bits(),
            Vec2F::from_cm(10, 0).x.to_bits(),
            Vec2F::from_cm(20, 0).x.to_bits(),
        ],
        "every fang's position survives the round trip",
    );
}

#[test]
fn restore_shrinks_an_overpopulated_boomerang_set() {
    let mut app = bare_app();
    spawn_fang(&mut app, false, Vec2F::from_cm(0, 0));
    let snap = SimSnapshot::capture(app.world_mut()); // captures exactly one
    // Over-populate the world the way a Multishot throw would.
    spawn_fang(&mut app, true, Vec2F::from_cm(10, 0));
    spawn_fang(&mut app, true, Vec2F::from_cm(20, 0));
    assert_eq!(boom_count(&mut app), 3);

    snap.restore(app.world_mut());
    assert_eq!(
        boom_count(&mut app),
        1,
        "restore despawns the surplus fangs"
    );
    let mut q = app.world_mut().query::<&BoomerangMods>();
    assert!(
        !q.iter(app.world()).next().unwrap().is_secondary,
        "the lone survivor is the snapshot's primary",
    );
}

// ---- Fire trail: drop cadence, kills, expiry, snapshot ----

fn cell_count(app: &mut App) -> usize {
    let mut q = app.world_mut().query::<&FireTrailCell>();
    q.iter(app.world()).count()
}

#[test]
fn fire_boomerang_drops_cells_on_the_interval_only() {
    let mut app = bare_app();
    // A flying Fire boomerang...
    app.world_mut().spawn((
        Boomerang {
            owner_handle: 0,
            state: BoomerangState::Flying,
        },
        BoomerangMods {
            modifier: Some(PickupKind::Fire),
            is_secondary: false,
            despawn_at_frame: None,
            wall_bounces: 0,
        },
        PositionF(Vec2F::from_cm(100, 50)),
        PreviousPositionF(Vec2F::from_cm(100, 50)),
        VelocityF(Vec2F::from_cm(50, 0)),
    ));
    // ...and a plain boomerang that must never leave fire.
    app.world_mut().spawn((
        Boomerang {
            owner_handle: 1,
            state: BoomerangState::Flying,
        },
        BoomerangMods::default(),
        PositionF(Vec2F::from_cm(-100, 0)),
        PreviousPositionF(Vec2F::from_cm(-100, 0)),
        VelocityF(Vec2F::from_cm(-50, 0)),
    ));

    // On a 6-tick boundary → exactly one cell, at the Fire boomerang.
    *app.world_mut().resource_mut::<FrameCount>() = FrameCount(12);
    app.world_mut().run_system_once(drop_fire_trail).unwrap();
    let cells: Vec<(FireTrailCell, Vec2F)> = {
        let mut q = app.world_mut().query::<(&FireTrailCell, &PositionF)>();
        q.iter(app.world()).map(|(c, p)| (*c, p.0)).collect()
    };
    assert_eq!(
        cells.len(),
        1,
        "one Fire boomerang drops one cell on cadence"
    );
    assert_eq!(cells[0].0.owner_handle, 0, "cell inherits the thrower");
    assert_eq!(
        cells[0].0.expires_at_frame,
        12 + FIRE_TRAIL_LIFETIME_FRAMES,
        "cell burns out a lifetime later",
    );
    assert_eq!(
        cells[0].1,
        Vec2F::from_cm(100, 50),
        "cell drops at the boomerang's position",
    );

    // Off-cadence → nothing new.
    *app.world_mut().resource_mut::<FrameCount>() = FrameCount(13);
    app.world_mut().run_system_once(drop_fire_trail).unwrap();
    assert_eq!(cell_count(&mut app), 1, "no drop between intervals");

    // Next boundary → another cell.
    *app.world_mut().resource_mut::<FrameCount>() = FrameCount(18);
    app.world_mut().run_system_once(drop_fire_trail).unwrap();
    assert_eq!(cell_count(&mut app), 2, "drops again on the next interval");
}

#[test]
fn fire_cell_kills_opponent_credits_owner_spares_owner() {
    let mut app = bare_app();
    app.world_mut().spawn((
        FireTrailCell {
            owner_handle: 0,
            expires_at_frame: 9999,
        },
        PositionF(Vec2F::ZERO),
    ));
    // Owner standing in their own fire — immune.
    let p0 = app
        .world_mut()
        .spawn((Player { handle: 0 }, PositionF(Vec2F::ZERO)))
        .id();
    // Opponent standing in it — burns.
    let p1 = app
        .world_mut()
        .spawn((Player { handle: 1 }, PositionF(Vec2F::ZERO)))
        .id();

    app.world_mut().run_system_once(fire_trail_kills).unwrap();

    assert!(
        !app.world().entity(p0).get::<Dead>().unwrap().is_dying(),
        "the owner is immune to their own fire",
    );
    assert!(
        app.world().entity(p1).get::<Dead>().unwrap().is_dying(),
        "the opponent burns",
    );
    assert_eq!(
        app.world().resource::<MatchScore>().p0,
        1,
        "the kill is credited to the fire's owner",
    );
}

#[test]
fn fire_cell_does_not_burn_through_iframes() {
    let mut app = bare_app();
    app.world_mut().spawn((
        FireTrailCell {
            owner_handle: 0,
            expires_at_frame: 9999,
        },
        PositionF(Vec2F::ZERO),
    ));
    let p1 = app
        .world_mut()
        .spawn((Player { handle: 1 }, PositionF(Vec2F::ZERO)))
        .id();
    *app.world_mut()
        .entity_mut(p1)
        .get_mut::<StunFrames>()
        .unwrap() = StunFrames(5);

    app.world_mut().run_system_once(fire_trail_kills).unwrap();
    assert!(
        !app.world().entity(p1).get::<Dead>().unwrap().is_dying(),
        "dash i-frames carry a player through the fire unharmed",
    );
}

#[test]
fn fire_cell_expires_at_its_lifetime() {
    let mut app = bare_app();
    app.world_mut().spawn((
        FireTrailCell {
            owner_handle: 0,
            expires_at_frame: 300,
        },
        PositionF(Vec2F::ZERO),
    ));
    *app.world_mut().resource_mut::<FrameCount>() = FrameCount(299);
    app.world_mut().run_system_once(expire_fire_trail).unwrap();
    assert_eq!(cell_count(&mut app), 1, "alive one frame before burnout");
    *app.world_mut().resource_mut::<FrameCount>() = FrameCount(300);
    app.world_mut().run_system_once(expire_fire_trail).unwrap();
    assert_eq!(cell_count(&mut app), 0, "despawned at the burnout frame");
}

#[test]
fn fire_cells_survive_snapshot_round_trip() {
    let mut app = bare_app();
    app.world_mut().spawn((
        FireTrailCell {
            owner_handle: 0,
            expires_at_frame: 100,
        },
        PositionF(Vec2F::from_cm(10, 0)),
    ));
    app.world_mut().spawn((
        FireTrailCell {
            owner_handle: 0,
            expires_at_frame: 200,
        },
        PositionF(Vec2F::from_cm(20, 0)),
    ));
    app.world_mut().spawn((
        FireTrailCell {
            owner_handle: 1,
            expires_at_frame: 150,
        },
        PositionF(Vec2F::from_cm(-10, 0)),
    ));

    let snap = SimSnapshot::capture(app.world_mut());
    let es: Vec<Entity> = {
        let mut q = app.world_mut().query::<(Entity, &FireTrailCell)>();
        q.iter(app.world()).map(|(e, _)| e).collect()
    };
    for e in es {
        app.world_mut().despawn(e);
    }
    assert_eq!(cell_count(&mut app), 0);
    snap.restore(app.world_mut());

    let mut got: Vec<(usize, u32, i32)> = {
        let mut q = app.world_mut().query::<(&FireTrailCell, &PositionF)>();
        q.iter(app.world())
            .map(|(c, p)| (c.owner_handle, c.expires_at_frame, p.0.x.to_bits()))
            .collect()
    };
    got.sort();
    let mut want = vec![
        (0usize, 100u32, Vec2F::from_cm(10, 0).x.to_bits()),
        (0, 200, Vec2F::from_cm(20, 0).x.to_bits()),
        (1, 150, Vec2F::from_cm(-10, 0).x.to_bits()),
    ];
    want.sort();
    assert_eq!(
        got, want,
        "every fire cell restored with owner/expiry/position"
    );
}
