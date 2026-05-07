//! Phase 8 cycle 6: `InputHistory` + edge detection + forgiveness window.
//!
//! Two layers of test:
//!   1. Pure helper unit tests — `push_history`, `previous_input`,
//!      `just_pressed`/`just_released`, `pressed_within`/`released_within`.
//!   2. Integration: build a Bevy app with `SimPlugin` + `DefaultInputsPlugin`,
//!      drive `SynthesizedInputs` through several ticks, and assert the
//!      ring's contents and edge predicates.

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy_ggrs::GgrsPlugin;
use bevy_ggrs::prelude::*;
use core::time::Duration;
use fixed_math::Vec2F;
use sim::{
    DefaultInputsPlugin, GgrsCfg, INPUT_HISTORY_LEN, InputHistory, Player, PlayerInput, PositionF,
    PreviousPositionF, SimPlugin, SynthesizedInputs, VelocityF, just_pressed, just_released,
    pressed_within, previous_input, push_history, released_within,
};

// ---- Pure helper unit tests ----

fn input_with_buttons(buttons: u8) -> PlayerInput {
    PlayerInput {
        stick_x: 0,
        stick_y: 0,
        aim_angle: 0,
        buttons,
    }
}

#[test]
fn push_history_shifts_oldest_out_and_appends_newest() {
    let mut ring = [PlayerInput::default(); INPUT_HISTORY_LEN];
    for i in 0..(INPUT_HISTORY_LEN as u8) {
        push_history(&mut ring, input_with_buttons(i + 1));
    }
    // After N pushes the ring should hold 1..=N in order.
    for i in 0..INPUT_HISTORY_LEN {
        assert_eq!(ring[i].buttons, i as u8 + 1, "ring slot {i}");
    }
    // One more push: newest = N+1, oldest (was 1) is gone.
    push_history(&mut ring, input_with_buttons(99));
    assert_eq!(ring[0].buttons, 2);
    assert_eq!(ring[INPUT_HISTORY_LEN - 1].buttons, 99);
}

#[test]
fn previous_input_returns_last_entry() {
    let mut ring = [PlayerInput::default(); INPUT_HISTORY_LEN];
    push_history(&mut ring, input_with_buttons(7));
    assert_eq!(previous_input(&ring).buttons, 7);
}

#[test]
fn just_pressed_detects_rising_edge_only() {
    let down = input_with_buttons(PlayerInput::THROW_DOWN);
    let up = input_with_buttons(0);
    assert!(just_pressed(down, up, PlayerInput::THROW_DOWN));
    assert!(!just_pressed(down, down, PlayerInput::THROW_DOWN));
    assert!(!just_pressed(up, down, PlayerInput::THROW_DOWN));
    assert!(!just_pressed(up, up, PlayerInput::THROW_DOWN));
}

#[test]
fn just_released_detects_falling_edge_only() {
    let down = input_with_buttons(PlayerInput::THROW_DOWN);
    let up = input_with_buttons(0);
    assert!(just_released(up, down, PlayerInput::THROW_DOWN));
    assert!(!just_released(down, down, PlayerInput::THROW_DOWN));
    assert!(!just_released(down, up, PlayerInput::THROW_DOWN));
    assert!(!just_released(up, up, PlayerInput::THROW_DOWN));
}

#[test]
fn just_pressed_other_bits_dont_interfere() {
    // Mask only THROW_DOWN; AIM_ACTIVE rising shouldn't trigger.
    let curr = input_with_buttons(PlayerInput::AIM_ACTIVE);
    let prev = input_with_buttons(0);
    assert!(!just_pressed(curr, prev, PlayerInput::THROW_DOWN));
    assert!(just_pressed(curr, prev, PlayerInput::AIM_ACTIVE));
}

#[test]
fn pressed_within_finds_recent_rising_edge() {
    // Build a ring with a rising edge at index 5→6.
    let mut ring = [PlayerInput::default(); INPUT_HISTORY_LEN];
    ring[5] = input_with_buttons(0);
    ring[6] = input_with_buttons(PlayerInput::DASH_DOWN);
    ring[7] = input_with_buttons(PlayerInput::DASH_DOWN);
    // n=2 covers transitions (6→7) and (5→6) — the second one is a press.
    assert!(pressed_within(&ring, 2, PlayerInput::DASH_DOWN));
    // n=1 covers only (6→7) which is held, not a rising edge.
    assert!(!pressed_within(&ring, 1, PlayerInput::DASH_DOWN));
}

#[test]
fn pressed_within_clamps_n_to_history_size() {
    // Press at the very oldest transition (0→1).
    let mut ring = [PlayerInput::default(); INPUT_HISTORY_LEN];
    ring[1] = input_with_buttons(PlayerInput::DASH_DOWN);
    // n way past INPUT_HISTORY_LEN should clamp and still find it.
    assert!(pressed_within(&ring, 999, PlayerInput::DASH_DOWN));
}

#[test]
fn released_within_finds_recent_falling_edge() {
    let mut ring = [PlayerInput::default(); INPUT_HISTORY_LEN];
    for i in 0..6 {
        ring[i] = input_with_buttons(PlayerInput::THROW_DOWN);
    }
    ring[6] = input_with_buttons(0);
    ring[7] = input_with_buttons(0);
    // Release happened at 5→6 — covered by n>=2.
    assert!(released_within(&ring, 2, PlayerInput::THROW_DOWN));
    // n=1 only covers (6→7), which is held-low — not a release.
    assert!(!released_within(&ring, 1, PlayerInput::THROW_DOWN));
}

#[test]
fn pressed_within_returns_false_when_no_edge() {
    // Steady-low ring: no rising edges anywhere.
    let ring = [PlayerInput::default(); INPUT_HISTORY_LEN];
    assert!(!pressed_within(&ring, INPUT_HISTORY_LEN, PlayerInput::THROW_DOWN));
}

// ---- Integration: ring is populated through SimPlugin ----

fn build_app() -> App {
    // input_delay = 0 so the ring's contents are a 1:1 reflection of
    // the `SynthesizedInputs` we push each tick — the test's job is to
    // exercise the ring itself, not GGRS's input-delay semantics.
    let mut sb = SessionBuilder::<GgrsCfg>::new()
        .with_num_players(2)
        .unwrap()
        .with_check_distance(2)
        .with_input_delay(0);
    for i in 0..2 {
        sb = sb.add_player(PlayerType::Local, i).unwrap();
    }
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

    for handle in 0..2 {
        app.world_mut().spawn((
            Player { handle },
            PositionF(Vec2F::ZERO),
            PreviousPositionF(Vec2F::ZERO),
            VelocityF(Vec2F::ZERO),
        ));
    }
    app
}

/// Cycles the SyncTestSession past its first-update warmup. With a
/// SyncTestSession the very first `app.update()` initializes GGRS
/// state without running a tick — no `advance_input_history` push.
/// Every subsequent update consumes the current `SynthesizedInputs`
/// and produces exactly one ring push, so post-warmup test logic is
/// 1:1 with `app.update()` calls.
fn warmup(app: &mut App) {
    app.update();
}

#[test]
fn input_history_records_ticks_in_order() {
    let mut app = build_app();
    warmup(&mut app);
    // Drive distinct stick_x values so each tick's input is identifiable.
    let pattern = [10i8, 20, 30, 40, 50, 60, 70, 80];
    for &v in &pattern {
        app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
            stick_x: v,
            stick_y: 0,
            aim_angle: 0,
            buttons: 0,
        };
        app.update();
    }
    let history = app.world().resource::<InputHistory>();
    let ring = history.0.get(&0).expect("handle 0 ring");
    for (i, expected) in pattern.iter().enumerate() {
        assert_eq!(
            ring[i].stick_x, *expected,
            "tick {i}: ring slot stick_x mismatch (full ring: {:?})",
            ring.map(|p| p.stick_x)
        );
    }
}

#[test]
fn input_history_drops_oldest_when_ring_overflows() {
    let mut app = build_app();
    warmup(&mut app);
    // Push INPUT_HISTORY_LEN + 1 distinct values; oldest should fall off.
    for tick in 0..(INPUT_HISTORY_LEN as i8 + 1) {
        app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
            stick_x: tick + 1,
            stick_y: 0,
            aim_angle: 0,
            buttons: 0,
        };
        app.update();
    }
    let history = app.world().resource::<InputHistory>();
    let ring = history.0.get(&0).expect("handle 0 ring");
    // First tick (stick_x=1) was pushed out; ring now holds 2..=(N+1).
    assert_eq!(ring[0].stick_x, 2);
    assert_eq!(ring[INPUT_HISTORY_LEN - 1].stick_x, INPUT_HISTORY_LEN as i8 + 1);
}

#[test]
fn forgiveness_window_recognizes_recent_release() {
    let mut app = build_app();
    warmup(&mut app);
    // Hold throw for two ticks, then release. After the release tick,
    // `released_within(ring, n, THROW_DOWN)` should fire for n large
    // enough to cover the gap to the most recent push.
    let sequence = [
        PlayerInput::THROW_DOWN,
        PlayerInput::THROW_DOWN,
        0, // release transition lands here (held → 0)
        0,
        0,
    ];
    for buttons in sequence {
        app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
            stick_x: 0,
            stick_y: 0,
            aim_angle: 0,
            buttons,
        };
        app.update();
    }
    let history = app.world().resource::<InputHistory>();
    let ring = history.0.get(&0).expect("handle 0 ring");
    // The release was pushed 3 ticks ago; n=6 is wide enough to catch it.
    assert!(released_within(ring, 6, PlayerInput::THROW_DOWN));
    // n=2 only inspects the most recent two adjacent-pair transitions
    // (both 0 → 0), which contain no release edge.
    assert!(!released_within(ring, 2, PlayerInput::THROW_DOWN));
}
