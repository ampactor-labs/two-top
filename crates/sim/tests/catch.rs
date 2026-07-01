//! Phase 17 cycle 1: perfect catch → empowered throw.
//!
//! Catching a returning boomerang within PERFECT_CATCH_WINDOW_FRAMES of the
//! recall starting empowers the catcher's next throw. The window detection
//! is driven directly through `catch_boomerangs`; the speed application is a
//! pure helper (`throw_speed_for`) so the bonus is covered without fragile
//! end-to-end recall orchestration.

use core::time::Duration;

use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy_ggrs::GgrsPlugin;
use bevy_ggrs::prelude::*;
use fixed_math::{Fix, Vec2F};
use sim::{
    Boomerang, BoomerangState, EMPOWERED_THROW_SPEED_CM_PER_TICK, Empowered, FrameCount, GgrsCfg,
    PERFECT_CATCH_WINDOW_FRAMES, Player, PositionF, PreviousPositionF, SimPlugin, SimSnapshot,
    THROW_SPEED_CM_PER_TICK, VelocityF, catch_boomerangs, throw_speed_for,
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

/// Spawn P0 at the origin and a returning boomerang it owns, overlapping it,
/// that began returning at `since`. Returns P0's entity.
fn setup_catch(app: &mut App, frame: u32, since: u32) -> Entity {
    *app.world_mut().resource_mut::<FrameCount>() = FrameCount(frame);
    let p0 = app
        .world_mut()
        .spawn((Player { handle: 0 }, PositionF(Vec2F::ZERO)))
        .id();
    app.world_mut().spawn((
        Boomerang {
            owner_handle: 0,
            state: BoomerangState::Returning { since },
        },
        PositionF(Vec2F::ZERO),
        PreviousPositionF(Vec2F::ZERO),
        VelocityF(Vec2F::ZERO),
    ));
    p0
}

#[test]
fn catch_within_window_empowers() {
    let mut app = bare_app();
    // since = frame - window  →  exactly on the edge, still perfect.
    let p0 = setup_catch(&mut app, 500, 500 - PERFECT_CATCH_WINDOW_FRAMES);
    app.world_mut().run_system_once(catch_boomerangs).unwrap();
    assert!(
        app.world().entity(p0).get::<Empowered>().unwrap().0,
        "catch on the last window frame is a perfect catch"
    );
    let mut q = app.world_mut().query::<&Boomerang>();
    assert_eq!(q.iter(app.world()).count(), 0, "boomerang was caught");
}

#[test]
fn catch_one_frame_late_does_not_empower() {
    let mut app = bare_app();
    let p0 = setup_catch(&mut app, 500, 500 - PERFECT_CATCH_WINDOW_FRAMES - 1);
    app.world_mut().run_system_once(catch_boomerangs).unwrap();
    assert!(
        !app.world().entity(p0).get::<Empowered>().unwrap().0,
        "one frame past the window is an ordinary catch"
    );
    let mut q = app.world_mut().query::<&Boomerang>();
    assert_eq!(
        q.iter(app.world()).count(),
        0,
        "still caught, just not perfect"
    );
}

#[test]
fn throw_speed_bonus_is_applied_and_is_a_real_increase() {
    assert_eq!(
        throw_speed_for(true),
        Fix::const_from_int(EMPOWERED_THROW_SPEED_CM_PER_TICK)
    );
    assert_eq!(
        throw_speed_for(false),
        Fix::const_from_int(THROW_SPEED_CM_PER_TICK)
    );
    assert!(
        throw_speed_for(true) > throw_speed_for(false),
        "empowered throw must be strictly faster"
    );
}

#[test]
fn empowered_flag_survives_snapshot_round_trip() {
    let mut app = bare_app();
    let p0 = app
        .world_mut()
        .spawn((
            Player { handle: 0 },
            PositionF(Vec2F::ZERO),
            PreviousPositionF(Vec2F::ZERO),
            VelocityF(Vec2F::ZERO),
        ))
        .id();
    *app.world_mut()
        .entity_mut(p0)
        .get_mut::<Empowered>()
        .unwrap() = Empowered(true);

    let snap = SimSnapshot::capture(app.world_mut());
    *app.world_mut()
        .entity_mut(p0)
        .get_mut::<Empowered>()
        .unwrap() = Empowered(false);
    snap.restore(app.world_mut());

    assert!(
        app.world().entity(p0).get::<Empowered>().unwrap().0,
        "restore brings back the empowered flag"
    );
}
