//! Phase 17 cycle 3: the five flight-modifying pickup behaviors driven
//! directly through their systems (Fire/Heavy throw speed, Phantom phase,
//! Heavy plow, Bouncy acceleration, Curve bend). Multishot + Fire-trail land
//! in their own cycles.

use core::time::Duration;

use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy_ggrs::GgrsPlugin;
use bevy_ggrs::prelude::*;
use fixed_math::{Fix, RectF, Vec2F};
use sim::{
    BONE_PYRE_HALF_EXTENT_CM, BOUNCY_MAX_SPEED_CM_PER_TICK, BonePyre, Boomerang, BoomerangMods,
    BoomerangState, GgrsCfg, PickupKind, PositionF, PreviousPositionF, SimPlugin,
    THROW_SPEED_CM_PER_TICK, VelocityF, Wall, WallKind, boomerang_pyre_collision,
    boomerang_wall_collision, curve_boomerangs, modified_throw_speed,
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
        Fix::const_from_int(65)
    );
    assert_eq!(
        modified_throw_speed(false, Some(PickupKind::Heavy)),
        Fix::const_from_int(40)
    );
    // Modifiers compose with the perfect-catch empowerment.
    assert_eq!(
        modified_throw_speed(true, Some(PickupKind::Fire)),
        Fix::const_from_int(80)
    );
}

#[test]
fn phantom_phases_through_walls() {
    let mut app = bare_app();
    app.world_mut().spawn(Wall {
        kind: WallKind::Solid,
        rect: square_rect(Vec2F::ZERO, 50),
    });
    let bm = spawn_mod(&mut app, PickupKind::Phantom, Vec2F::ZERO, Vec2F::from_cm(50, 0));
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
    app.world_mut()
        .spawn(BonePyre::intact(square_rect(Vec2F::ZERO, BONE_PYRE_HALF_EXTENT_CM)));
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
        kind: WallKind::Solid,
        rect: RectF::from_center_half_extents(
            Vec2F::from_cm(30, 0),
            Vec2F::new(Fix::const_from_int(10), Fix::const_from_int(100)),
        ),
    });
    let bm = spawn_mod(
        &mut app,
        PickupKind::Bouncy,
        Vec2F::from_cm(25, 0),
        Vec2F::from_cm(50, 0),
    );
    app.world_mut()
        .run_system_once(boomerang_wall_collision)
        .unwrap();
    let speed = vel_of(&app, bm).length();
    // 50 * 1.1 = 55 (within fixed-point slop), and faster than before.
    assert!(
        speed > Fix::const_from_int(53) && speed < Fix::const_from_int(57),
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
    let bm = spawn_mod(&mut app, PickupKind::Curve, Vec2F::ZERO, Vec2F::from_cm(50, 0));
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
    assert!((sa - sb).abs() <= Fix::from_bits(0x400), "speed is preserved");
}
