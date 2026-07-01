//! Phase 11 cycle 2: respawn timer + snap_position teleport.
//!
//! Coverage:
//!   * Dead persists for exactly RESPAWN_FRAMES ticks before tick_respawn
//!     fires.
//!   * On revive: PositionF + PreviousPositionF both equal
//!     `respawn_position(handle)` (no lerp streak), VelocityF is zero,
//!     DashState is Idle, StunFrames is 0, Dead component is removed.
//!   * Per-handle respawn points are symmetric on the x axis.

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy_ggrs::GgrsPlugin;
use bevy_ggrs::prelude::*;
use core::time::Duration;
use fixed_math::Vec2F;
use sim::{
    Boomerang, BoomerangState, DashState, Dead, DefaultInputsPlugin, GgrsCfg, Player, PositionF,
    PreviousPositionF, RESPAWN_FRAMES, SimPlugin, StunFrames, VelocityF, respawn_position,
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
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
        1.0 / sim::TICK_HZ as f64,
    )));
    app.add_plugins(GgrsPlugin::<GgrsCfg>::default());
    app.add_plugins(SimPlugin);
    app.add_plugins(sim::InfiniteRoundPlugin);
    app.add_plugins(DefaultInputsPlugin);
    app.insert_resource(Session::SyncTest(session));

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

fn dead_count(app: &mut App) -> usize {
    app.world_mut()
        .query::<&Dead>()
        .iter(app.world())
        .filter(|d| d.is_dying())
        .count()
}

fn p1_dead(app: &mut App) -> bool {
    let mut q = app.world_mut().query::<(&Player, &Dead)>();
    q.iter(app.world())
        .find(|(p, _)| p.handle == 1)
        .map(|(_, d)| d.is_dying())
        .unwrap_or(false)
}

fn p1_position(app: &mut App) -> (Vec2F, Vec2F) {
    let mut q = app
        .world_mut()
        .query::<(&Player, &PositionF, &PreviousPositionF)>();
    q.iter(app.world())
        .find(|(p, _, _)| p.handle == 1)
        .map(|(_, pos, prev)| (pos.0, prev.0))
        .expect("p1 entity")
}

fn kill_p1(app: &mut App) {
    // Spawn a Flying boomerang owned by p0 right on top of p1 so the
    // very next tick's hit_boomerang_player marks p1 Dead.
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
    assert!(p1_dead(app), "kill_p1 helper failed to land the kill");
}

#[test]
fn dead_persists_until_respawn_window_elapses() {
    let mut app = build_two_player_app();
    app.update(); // warmup
    kill_p1(&mut app);

    // p1 should remain Dead for the next RESPAWN_FRAMES - 1 ticks.
    // (kill_p1 already called update() once for the hit; that's the
    // first frame of the dead window.)
    for tick in 1..RESPAWN_FRAMES {
        app.update();
        assert!(
            p1_dead(&mut app),
            "p1 should still be Dead after {tick} post-kill ticks",
        );
    }

    // The next tick lands at frame.0 == respawn_at_frame, revive fires.
    app.update();
    assert!(!p1_dead(&mut app), "p1 should respawn after window");
    assert_eq!(dead_count(&mut app), 0);
}

#[test]
fn revive_snaps_to_respawn_point_with_no_lerp_streak() {
    let mut app = build_two_player_app();
    app.update();
    kill_p1(&mut app);

    // Tick through the whole respawn window.
    for _ in 0..RESPAWN_FRAMES {
        app.update();
    }

    let (pos, prev) = p1_position(&mut app);
    assert_eq!(
        pos,
        respawn_position(1),
        "p1 should be at respawn point after revive",
    );
    assert_eq!(
        prev, pos,
        "PreviousPositionF must equal PositionF after snap (no lerp streak)",
    );
}

#[test]
fn revive_resets_velocity_dash_and_stun() {
    let mut app = build_two_player_app();
    app.update();

    // Land the kill first (with default zero state so the immunity gates
    // don't fire), THEN scribble non-zero VelocityF/DashState/StunFrames
    // onto the corpse so we can verify revive resets them. Scribbling
    // before the kill would have made StunFrames > 0 and the kill would
    // never have landed.
    kill_p1(&mut app);

    let p1_entity = {
        let mut q = app.world_mut().query::<(Entity, &Player)>();
        q.iter(app.world())
            .find(|(_, p)| p.handle == 1)
            .map(|(e, _)| e)
            .expect("p1 entity")
    };
    {
        let mut e = app.world_mut().entity_mut(p1_entity);
        e.insert(VelocityF(Vec2F::from_cm(13, 13)));
        e.insert(DashState::Cooldown {
            frames_remaining: 5,
        });
        e.insert(StunFrames(7));
    }

    for _ in 0..RESPAWN_FRAMES {
        app.update();
    }

    let mut q = app
        .world_mut()
        .query::<(&Player, &VelocityF, &DashState, &StunFrames)>();
    let (_, vel, dash, stun) = q
        .iter(app.world())
        .find(|(p, _, _, _)| p.handle == 1)
        .expect("p1 entity");

    assert_eq!(vel.0, Vec2F::ZERO, "velocity reset on revive");
    assert!(
        matches!(dash, DashState::Idle),
        "dash reset to Idle: {dash:?}"
    );
    assert_eq!(stun.0, 0, "stun reset to 0");
}

#[test]
fn respawn_position_is_symmetric_on_y_axis() {
    let p0 = respawn_position(0);
    let p1 = respawn_position(1);
    assert_eq!(p0.x, p1.x, "respawn points share x");
    assert_eq!(p0.y, -p1.y, "respawn points should mirror on y");
}
