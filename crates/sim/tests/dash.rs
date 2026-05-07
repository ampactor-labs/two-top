//! Phase 9 cycle 4: dash mechanic + i-frames.
//!
//! Pure-helper unit tests cover the state-machine transitions; the
//! integration test drives a real `SyncTestSession` through a full
//! dash → cooldown → idle cycle and asserts the player's position,
//! `DashState`, and `StunFrames` advance as designed.

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy_ggrs::GgrsPlugin;
use bevy_ggrs::prelude::*;
use core::time::Duration;
use fixed_math::{Fix, Vec2F};
use sim::{
    DASH_COOLDOWN_FRAMES, DASH_DURATION_FRAMES, DASH_SPEED_CM_PER_TICK, DashState,
    DefaultInputsPlugin, GgrsCfg, Player, PlayerInput, PositionF, PreviousPositionF, SimPlugin,
    StunFrames, SynthesizedInputs, VelocityF, tick_dash_state, try_start_dash,
};

// ---- Pure helper unit tests ----

fn unit_right() -> Vec2F {
    Vec2F::new(Fix::const_from_int(1), Fix::const_from_int(0))
}

#[test]
fn try_start_dash_idle_with_pressed_and_stick_commits() {
    let (new, committed) = try_start_dash(DashState::Idle, unit_right(), true);
    assert!(committed);
    assert!(matches!(
        new,
        DashState::Dashing {
            frames_remaining: f,
            ..
        } if f == DASH_DURATION_FRAMES
    ));
}

#[test]
fn try_start_dash_no_press_stays_idle() {
    let (new, committed) = try_start_dash(DashState::Idle, unit_right(), false);
    assert!(!committed);
    assert!(matches!(new, DashState::Idle));
}

#[test]
fn try_start_dash_centered_stick_no_op() {
    let (new, committed) = try_start_dash(DashState::Idle, Vec2F::ZERO, true);
    assert!(!committed);
    assert!(matches!(new, DashState::Idle));
}

#[test]
fn try_start_dash_during_cooldown_no_op() {
    let cooldown = DashState::Cooldown {
        frames_remaining: 5,
    };
    let (new, committed) = try_start_dash(cooldown, unit_right(), true);
    assert!(!committed);
    assert_eq!(new, cooldown);
}

#[test]
fn try_start_dash_during_dashing_no_op() {
    let dashing = DashState::Dashing {
        frames_remaining: 5,
        dir: unit_right(),
    };
    let (new, committed) = try_start_dash(dashing, unit_right(), true);
    assert!(!committed);
    assert_eq!(new, dashing);
}

#[test]
fn tick_dash_idle_stays_idle() {
    assert!(matches!(tick_dash_state(DashState::Idle), DashState::Idle));
}

#[test]
fn tick_dash_dashing_decrements_until_transition() {
    let mut s = DashState::Dashing {
        frames_remaining: 3,
        dir: unit_right(),
    };
    s = tick_dash_state(s); // 3 → 2
    assert!(matches!(
        s,
        DashState::Dashing {
            frames_remaining: 2,
            ..
        }
    ));
    s = tick_dash_state(s); // 2 → 1
    assert!(matches!(
        s,
        DashState::Dashing {
            frames_remaining: 1,
            ..
        }
    ));
    s = tick_dash_state(s); // 1 → Cooldown(N)
    assert!(matches!(
        s,
        DashState::Cooldown {
            frames_remaining: f
        } if f == DASH_COOLDOWN_FRAMES
    ));
}

#[test]
fn tick_dash_cooldown_transitions_to_idle() {
    let mut s = DashState::Cooldown {
        frames_remaining: 2,
    };
    s = tick_dash_state(s);
    assert!(matches!(
        s,
        DashState::Cooldown {
            frames_remaining: 1
        }
    ));
    s = tick_dash_state(s);
    assert!(matches!(s, DashState::Idle));
}

// ---- Integration test ----

fn build_app() -> App {
    let mut sb = SessionBuilder::<GgrsCfg>::new()
        .with_num_players(1)
        .unwrap()
        .with_check_distance(2)
        .with_input_delay(0);
    sb = sb.add_player(PlayerType::Local, 0).unwrap();
    let session = sb.start_synctest_session().unwrap();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
        1.0 / sim::TICK_HZ as f64,
    )));
    app.add_plugins(GgrsPlugin::<GgrsCfg>::default());
    app.add_plugins(SimPlugin);
    app.add_plugins(DefaultInputsPlugin);
    app.insert_resource(Session::SyncTest(session));

    // Spawn at origin with a fresh DashState/StunFrames (auto-required
    // by Player). No PreviousPositionF — snapshot_previous skips it.
    app.world_mut().spawn((
        Player { handle: 0 },
        PositionF(Vec2F::ZERO),
        PreviousPositionF(Vec2F::ZERO),
        VelocityF(Vec2F::ZERO),
    ));
    app
}

fn read_dash(app: &mut App) -> DashState {
    let mut q = app.world_mut().query::<&DashState>();
    *q.iter(app.world()).next().expect("DashState")
}

fn read_stun(app: &mut App) -> u32 {
    let mut q = app.world_mut().query::<&StunFrames>();
    q.iter(app.world()).next().expect("StunFrames").0
}

fn read_pos(app: &mut App) -> Vec2F {
    let mut q = app.world_mut().query::<&PositionF>();
    q.iter(app.world()).next().expect("PositionF").0
}

#[test]
fn dash_full_cycle_advances_position_then_cools_down() {
    let mut app = build_app();
    app.update(); // SyncTestSession warmup

    // Tick 1: hold stick right, no dash button → walk, no dash.
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 127,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    app.update();
    assert!(matches!(read_dash(&mut app), DashState::Idle));
    assert_eq!(read_stun(&mut app), 0);
    let pos_walk = read_pos(&mut app);

    // Tick 2: press DASH_DOWN with stick still right → dash starts.
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 127,
        stick_y: 0,
        aim_angle: 0,
        buttons: PlayerInput::DASH_DOWN,
    };
    app.update();
    // Mid-dash: state is Dashing with remaining frames decremented once.
    let after_start = read_dash(&mut app);
    assert!(
        matches!(after_start, DashState::Dashing { frames_remaining: f, .. } if f == DASH_DURATION_FRAMES - 1),
        "expected Dashing with {} frames remaining, got {:?}",
        DASH_DURATION_FRAMES - 1,
        after_start,
    );
    assert_eq!(read_stun(&mut app), DASH_DURATION_FRAMES - 1);
    let pos_after_first_dash_tick = read_pos(&mut app);
    let dash_dx = pos_after_first_dash_tick.x - pos_walk.x;
    assert_eq!(dash_dx, Fix::const_from_int(DASH_SPEED_CM_PER_TICK));

    // Hold dash button + stick for the rest of the dash window.
    for _ in 1..DASH_DURATION_FRAMES {
        app.update();
    }
    // After DASH_DURATION_FRAMES total ticks of dashing, state should
    // be Cooldown.
    assert!(
        matches!(read_dash(&mut app), DashState::Cooldown { .. }),
        "expected Cooldown after dash window, got {:?}",
        read_dash(&mut app)
    );

    // Hold stick, but pressing DASH_DOWN during cooldown is a no-op.
    for _ in 0..DASH_COOLDOWN_FRAMES {
        app.update();
    }
    assert!(matches!(read_dash(&mut app), DashState::Idle));
    assert_eq!(read_stun(&mut app), 0);
}

#[test]
fn dash_cannot_restart_during_cooldown() {
    let mut app = build_app();
    app.update();
    // Start dash
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 127,
        stick_y: 0,
        aim_angle: 0,
        buttons: PlayerInput::DASH_DOWN,
    };
    app.update();
    // Run through dash + into cooldown
    for _ in 0..(DASH_DURATION_FRAMES + 1) {
        app.update();
    }
    assert!(matches!(read_dash(&mut app), DashState::Cooldown { .. }));
    // Release dash; next press inside cooldown should still no-op.
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 127,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    app.update();
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 127,
        stick_y: 0,
        aim_angle: 0,
        buttons: PlayerInput::DASH_DOWN,
    };
    app.update();
    assert!(
        matches!(read_dash(&mut app), DashState::Cooldown { .. }),
        "dash spuriously restarted during cooldown: {:?}",
        read_dash(&mut app)
    );
}
