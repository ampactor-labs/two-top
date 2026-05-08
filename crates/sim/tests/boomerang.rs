//! Phase 10 cycle 1: Boomerang spawning + flight.
//!
//! Pure-function tests cover the throw-direction predicate. The
//! integration tests drive a real `SyncTestSession` through a
//! THROW_DOWN tap-release and assert a Boomerang entity gets spawned
//! with velocity in the stick direction at THROW_SPEED.

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy_ggrs::GgrsPlugin;
use bevy_ggrs::prelude::*;
use core::time::Duration;
use fixed_math::{Fix, Vec2F};
use sim::{
    ARENA_HALF_WIDTH_CM, Boomerang, BoomerangState, DefaultInputsPlugin, GgrsCfg,
    INPUT_HISTORY_LEN, Player, PlayerInput, PositionF, PreviousPositionF,
    RECALL_SPEED_CM_PER_TICK, SimPlugin, SynthesizedInputs, THROW_SPEED_CM_PER_TICK, VelocityF,
    arena_walls, boomerang_rect, player_rect, recall_velocity, reflect_velocity_for_push,
    try_throw_direction,
};

// ---- Pure helper unit tests ----

fn ring_with_release_at(release_pos: usize) -> [PlayerInput; INPUT_HISTORY_LEN] {
    // Held for the slots before `release_pos`, released from `release_pos` onward.
    let mut ring = [PlayerInput::default(); INPUT_HISTORY_LEN];
    for (i, slot) in ring.iter_mut().enumerate() {
        slot.buttons = if i < release_pos {
            PlayerInput::THROW_DOWN
        } else {
            0
        };
    }
    ring
}

fn input_with_stick(x: i8, y: i8) -> PlayerInput {
    PlayerInput {
        stick_x: x,
        stick_y: y,
        aim_angle: 0,
        buttons: 0,
    }
}

#[test]
fn throw_fires_on_recent_release_with_direction() {
    let ring = ring_with_release_at(INPUT_HISTORY_LEN - 2); // released 2 ticks ago
    let curr = input_with_stick(127, 0);
    let dir = try_throw_direction(&ring, curr, false);
    assert!(dir.is_some());
    let v = dir.unwrap();
    // Unit vector in +x direction.
    assert!((v.x - Fix::const_from_int(1)).abs() <= Fix::from_bits(2));
    assert!(v.y.abs() <= Fix::from_bits(2));
}

#[test]
fn throw_does_not_fire_when_owner_already_has_boomerang() {
    let ring = ring_with_release_at(INPUT_HISTORY_LEN - 2);
    let curr = input_with_stick(127, 0);
    assert!(try_throw_direction(&ring, curr, true).is_none());
}

#[test]
fn throw_does_not_fire_without_stick_direction() {
    let ring = ring_with_release_at(INPUT_HISTORY_LEN - 2);
    let curr = input_with_stick(0, 0);
    assert!(try_throw_direction(&ring, curr, false).is_none());
}

#[test]
fn throw_does_not_fire_when_release_is_outside_window() {
    // Held throughout the ring AND on the current tick — no release
    // transition has happened anywhere.
    let ring = [PlayerInput {
        stick_x: 0,
        stick_y: 0,
        aim_angle: 0,
        buttons: PlayerInput::THROW_DOWN,
    }; INPUT_HISTORY_LEN];
    let curr = PlayerInput {
        stick_x: 127,
        stick_y: 0,
        aim_angle: 0,
        buttons: PlayerInput::THROW_DOWN, // still held
    };
    assert!(try_throw_direction(&ring, curr, false).is_none());
}

#[test]
fn throw_direction_diagonal_normalized_to_unit() {
    let ring = ring_with_release_at(INPUT_HISTORY_LEN - 2);
    let curr = input_with_stick(127, 127);
    let dir = try_throw_direction(&ring, curr, false).expect("throw");
    let len_sq = dir.length_sq();
    // Unit vector squared length ≈ 1.0 within fixed-point slop.
    assert!(
        (len_sq - Fix::const_from_int(1)).abs() < Fix::from_bits(0x100),
        "diagonal throw length_sq {:?} should be ~1.0",
        len_sq,
    );
}

// ---- Integration: end-to-end throw via SimPlugin ----

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
    app.add_plugins(sim::InfiniteRoundPlugin);
    app.add_plugins(DefaultInputsPlugin);
    app.insert_resource(Session::SyncTest(session));

    app.world_mut().spawn((
        Player { handle: 0 },
        PositionF(Vec2F::ZERO),
        PreviousPositionF(Vec2F::ZERO),
        VelocityF(Vec2F::ZERO),
    ));
    app
}

fn count_boomerangs(app: &mut App) -> usize {
    let mut q = app.world_mut().query::<&Boomerang>();
    q.iter(app.world()).count()
}

fn first_boomerang(app: &mut App) -> Option<(Boomerang, Vec2F, Vec2F)> {
    let mut q = app
        .world_mut()
        .query::<(&Boomerang, &PositionF, &VelocityF)>();
    q.iter(app.world())
        .next()
        .map(|(b, p, v)| (*b, p.0, v.0))
}

#[test]
fn tap_release_spawns_one_boomerang_in_stick_direction() {
    let mut app = build_app();
    app.update(); // SyncTestSession warmup

    // Tick 1: hold THROW_DOWN, stick centered (so player doesn't drift —
    // makes the spawn position predictable for assertion).
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 0,
        stick_y: 0,
        aim_angle: 0,
        buttons: PlayerInput::THROW_DOWN,
    };
    app.update();
    assert_eq!(count_boomerangs(&mut app), 0, "no spawn while held");

    // Tick 2: release with stick aimed right. The release-from-held
    // transition fires this tick via the same-tick `just_released`
    // check; throw direction comes from the stick.
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 127,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    app.update();
    assert_eq!(count_boomerangs(&mut app), 1, "throw should have spawned a boomerang");

    let (boom, pos, vel) = first_boomerang(&mut app).unwrap();
    assert_eq!(boom.owner_handle, 0);
    assert!(matches!(boom.state, BoomerangState::Flying));
    // Player held stick at zero through tick 1 and only deflected for
    // tick 2 — by the time throw_boomerangs runs (after movement), the
    // player has moved one walk-tick in +x.
    let walk_step = Fix::const_from_int(sim::WALK_SPEED_CM_PER_TICK);
    assert!((pos.x - walk_step).abs() <= Fix::from_bits(2));
    assert!(pos.y.abs() <= Fix::from_bits(2));
    // Velocity in +x direction at THROW_SPEED.
    let speed = Fix::const_from_int(THROW_SPEED_CM_PER_TICK);
    assert!((vel.x - speed).abs() <= Fix::from_bits(2));
    assert!(vel.y.abs() <= Fix::from_bits(2));
}

#[test]
fn boomerang_advances_by_throw_speed_each_subsequent_tick() {
    let mut app = build_app();
    app.update(); // warmup
    // Hold (stick centered so player doesn't drift), release with
    // stick aimed +x to set the throw direction.
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 0,
        stick_y: 0,
        aim_angle: 0,
        buttons: PlayerInput::THROW_DOWN,
    };
    app.update();
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 127,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    app.update();
    let (_b1, pos_after_spawn, _) = first_boomerang(&mut app).unwrap();
    // Tick once more with stick centered so the player stops moving;
    // we're isolating boomerang dx from player dx.
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 0,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    app.update();
    let (_b2, pos_after_flight, _) = first_boomerang(&mut app).unwrap();
    let dx = pos_after_flight.x - pos_after_spawn.x;
    assert!(
        (dx - Fix::const_from_int(THROW_SPEED_CM_PER_TICK)).abs() <= Fix::from_bits(2),
        "boomerang dx {dx} should be ~{THROW_SPEED_CM_PER_TICK} cm/tick",
    );
}

// ---- Cycle 2: wall ricochet ----

#[test]
fn reflect_velocity_x_push_flips_vx_only() {
    let vel = Vec2F::new(Fix::const_from_int(50), Fix::const_from_int(20));
    let push = Vec2F::new(Fix::const_from_int(5), Fix::ZERO); // x push
    let r = reflect_velocity_for_push(vel, push);
    assert_eq!(r.x, Fix::const_from_int(-50));
    assert_eq!(r.y, Fix::const_from_int(20));
}

#[test]
fn reflect_velocity_y_push_flips_vy_only() {
    let vel = Vec2F::new(Fix::const_from_int(30), Fix::const_from_int(40));
    let push = Vec2F::new(Fix::ZERO, Fix::const_from_int(-3)); // y push
    let r = reflect_velocity_for_push(vel, push);
    assert_eq!(r.x, Fix::const_from_int(30));
    assert_eq!(r.y, Fix::const_from_int(-40));
}

#[test]
fn reflect_velocity_zero_push_returns_unchanged() {
    let vel = Vec2F::new(Fix::const_from_int(50), Fix::const_from_int(50));
    let r = reflect_velocity_for_push(vel, Vec2F::ZERO);
    assert_eq!(r, vel);
}

fn build_arena_app() -> App {
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
    app.add_plugins(sim::InfiniteRoundPlugin);
    app.add_plugins(DefaultInputsPlugin);
    app.insert_resource(Session::SyncTest(session));

    // Place the player just inside the east wall so a single throw
    // rightward will hit the wall on its first or second flight tick.
    app.world_mut().spawn((
        Player { handle: 0 },
        PositionF(Vec2F::from_cm(ARENA_HALF_WIDTH_CM - 50, 0)),
        PreviousPositionF(Vec2F::from_cm(ARENA_HALF_WIDTH_CM - 50, 0)),
        VelocityF(Vec2F::ZERO),
    ));
    for w in arena_walls() {
        app.world_mut().spawn(w);
    }
    app
}

#[test]
fn boomerang_ricochets_off_east_wall() {
    let mut app = build_arena_app();
    app.update(); // warmup
    // Throw east: hold (stick centered), release with stick +x.
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 0,
        stick_y: 0,
        aim_angle: 0,
        buttons: PlayerInput::THROW_DOWN,
    };
    app.update();
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 127,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    app.update();
    let (_b, _, vel_pre) = first_boomerang(&mut app).unwrap();
    assert!(vel_pre.x > Fix::ZERO, "boomerang should be flying east");

    // Idle stick for a few ticks — boomerang flies into wall and
    // reflects within ~2 ticks (50 cm/tick × 2 = 100 cm distance).
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 0,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    let mut reflected = false;
    for _ in 0..5 {
        app.update();
        if let Some((_, _, vel)) = first_boomerang(&mut app)
            && vel.x < Fix::ZERO
        {
            reflected = true;
            break;
        }
    }
    assert!(reflected, "boomerang did not reflect off east wall");
}

#[test]
fn ricochet_preserves_speed() {
    let mut app = build_arena_app();
    app.update();
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 0,
        stick_y: 0,
        aim_angle: 0,
        buttons: PlayerInput::THROW_DOWN,
    };
    app.update();
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 127,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    app.update();
    let (_, _, vel_before) = first_boomerang(&mut app).unwrap();
    let speed_before = vel_before.length();

    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 0,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    // Wait until reflected.
    for _ in 0..5 {
        app.update();
        if let Some((_, _, vel)) = first_boomerang(&mut app)
            && vel.x < Fix::ZERO
        {
            let speed_after = vel.length();
            let diff = (speed_after - speed_before).abs();
            assert!(
                diff <= Fix::from_bits(0x40),
                "ricochet changed speed: before={speed_before:?}, after={speed_after:?}",
            );
            return;
        }
    }
    panic!("boomerang did not reflect within window");
}

// ---- Cycle 3: recall trigger + homing ----

#[test]
fn recall_velocity_at_owner_returns_zero() {
    let p = Vec2F::from_cm(123, -45);
    let v = recall_velocity(p, p, Fix::const_from_int(RECALL_SPEED_CM_PER_TICK));
    assert_eq!(v, Vec2F::ZERO);
}

#[test]
fn recall_velocity_points_toward_owner_at_speed() {
    // Boomerang at +x of owner: velocity should point -x at full speed.
    let boom = Vec2F::from_cm(200, 0);
    let owner = Vec2F::ZERO;
    let speed = Fix::const_from_int(RECALL_SPEED_CM_PER_TICK);
    let v = recall_velocity(boom, owner, speed);
    assert!((v.x + speed).abs() <= Fix::from_bits(2), "vx {} should be -{speed}", v.x);
    assert!(v.y.abs() <= Fix::from_bits(2));
}

#[test]
fn recall_velocity_diagonal_normalized_then_scaled() {
    // Boomerang NE of owner at (300, 300): velocity SW, magnitude == speed.
    let boom = Vec2F::from_cm(300, 300);
    let owner = Vec2F::ZERO;
    let speed = Fix::const_from_int(RECALL_SPEED_CM_PER_TICK);
    let v = recall_velocity(boom, owner, speed);
    let mag = v.length();
    assert!(
        (mag - speed).abs() <= Fix::from_bits(0x80),
        "recall magnitude {mag} should be ~{speed}",
    );
    assert!(v.x < Fix::ZERO);
    assert!(v.y < Fix::ZERO);
    // Components should be roughly equal in magnitude (45° back at owner).
    assert!((v.x - v.y).abs() <= Fix::from_bits(0x80));
}

#[test]
fn recall_press_transitions_flying_to_returning() {
    let mut app = build_app();
    app.update(); // warmup

    // Throw east.
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 0,
        stick_y: 0,
        aim_angle: 0,
        buttons: PlayerInput::THROW_DOWN,
    };
    app.update();
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 127,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    app.update();
    assert_eq!(count_boomerangs(&mut app), 1);
    let (boom, _, _) = first_boomerang(&mut app).unwrap();
    assert!(matches!(boom.state, BoomerangState::Flying));

    // Let the boomerang fly out far enough that the first recall-tick
    // home-step won't already overlap the owner (cycle 4 catch would
    // despawn it before we observe the Returning state).
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 0,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    for _ in 0..10 {
        app.update();
    }

    // Press THROW_DOWN again — recall trigger.
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 0,
        stick_y: 0,
        aim_angle: 0,
        buttons: PlayerInput::THROW_DOWN,
    };
    app.update();
    let (boom, _, vel) = first_boomerang(&mut app).unwrap();
    assert!(
        matches!(boom.state, BoomerangState::Returning),
        "recall press should have flipped Flying → Returning",
    );
    // Velocity now points back toward owner (player at origin), so vx < 0.
    assert!(vel.x < Fix::ZERO, "recall velocity vx {} should be negative", vel.x);
}

#[test]
fn returning_boomerang_homes_each_tick_at_recall_speed() {
    let mut app = build_app();
    app.update();
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 0,
        stick_y: 0,
        aim_angle: 0,
        buttons: PlayerInput::THROW_DOWN,
    };
    app.update();
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 127,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    app.update();
    // Let the boomerang fly out a few hundred cm before recalling, so a
    // single homing tick can't overshoot the owner. Idle (also clears the
    // THROW_DOWN edge so the recall press is a clean rising edge).
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 0,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    for _ in 0..10 {
        app.update();
    }
    let (_, p_far, _) = first_boomerang(&mut app).unwrap();
    assert!(p_far.x > Fix::const_from_int(RECALL_SPEED_CM_PER_TICK * 2));

    // Recall: rising edge transitions to Returning and sets velocity.
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 0,
        stick_y: 0,
        aim_angle: 0,
        buttons: PlayerInput::THROW_DOWN,
    };
    app.update();
    let (_, p1, vel1) = first_boomerang(&mut app).unwrap();
    assert!(vel1.x < Fix::ZERO);

    // Hold further — homing continues. Since the boomerang is still well
    // outside RECALL_SPEED of the owner, dx stays approximately -recall.
    app.update();
    let (boom, p2, vel2) = first_boomerang(&mut app).unwrap();
    assert!(matches!(boom.state, BoomerangState::Returning));

    let dx = (p2 - p1).x;
    let recall_speed = Fix::const_from_int(RECALL_SPEED_CM_PER_TICK);
    assert!(
        (dx + recall_speed).abs() <= Fix::from_bits(0x80),
        "Returning dx {dx} should be ~-{recall_speed}",
    );
    assert!(vel2.x < Fix::ZERO);
    let mag = vel2.length();
    assert!(
        (mag - recall_speed).abs() <= Fix::from_bits(0x100),
        "Returning |vel| {mag} should be ~{recall_speed}",
    );
}

// ---- Cycle 4: catch on owner collision ----

#[test]
fn player_and_boomerang_rects_overlap_when_centers_close() {
    // Sanity: the AABB primitives we lean on for catch detection
    // overlap on tight centers and don't on far ones.
    let owner = player_rect(Vec2F::ZERO);
    let close = boomerang_rect(Vec2F::from_cm(5, 5));
    let far = boomerang_rect(Vec2F::from_cm(100, 0));
    assert!(owner.overlaps(close));
    assert!(!owner.overlaps(far));
}

#[test]
fn returning_boomerang_caught_when_overlapping_owner() {
    let mut app = build_app();
    app.update();

    // Throw east.
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 0,
        stick_y: 0,
        aim_angle: 0,
        buttons: PlayerInput::THROW_DOWN,
    };
    app.update();
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 127,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    app.update();
    assert_eq!(count_boomerangs(&mut app), 1);

    // Idle several ticks so the boomerang flies far.
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 0,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    for _ in 0..10 {
        app.update();
    }

    // Recall — and HOLD THROW_DOWN until catch fires. Throw-lock pins
    // the player at +13 (the spawn drift) so the homing target stays
    // stable; the boomerang reaches the player within ~10 ticks at
    // 55 cm/tick from ~500 cm out.
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 0,
        stick_y: 0,
        aim_angle: 0,
        buttons: PlayerInput::THROW_DOWN,
    };
    let mut caught = false;
    for _ in 0..30 {
        app.update();
        if count_boomerangs(&mut app) == 0 {
            caught = true;
            break;
        }
    }
    assert!(caught, "Returning boomerang should have been caught by owner");
}

#[test]
fn flying_boomerang_does_not_catch_on_overlap() {
    // catch_boomerangs only fires on Returning. We seed a Flying
    // boomerang directly on the player rect and assert it survives a
    // tick without despawning. Keeps the spawn-tick self-catch from
    // happening.
    let mut app = build_app();
    app.update();

    // Spawn a Flying boomerang manually at the owner's position.
    let owner_handle = 0;
    let owner_pos = Vec2F::ZERO;
    app.world_mut().spawn((
        Boomerang { owner_handle, state: BoomerangState::Flying },
        PositionF(owner_pos),
        PreviousPositionF(owner_pos),
        // Velocity zero so physics doesn't move it out of overlap.
        VelocityF(Vec2F::ZERO),
    ));

    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 0,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    app.update();
    assert_eq!(
        count_boomerangs(&mut app),
        1,
        "Flying boomerang must not be caught even when overlapping owner",
    );
    let (b, _, _) = first_boomerang(&mut app).unwrap();
    assert!(matches!(b.state, BoomerangState::Flying));
}

#[test]
fn catch_frees_owner_to_throw_again() {
    let mut app = build_app();
    app.update();

    // First throw.
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 0,
        stick_y: 0,
        aim_angle: 0,
        buttons: PlayerInput::THROW_DOWN,
    };
    app.update();
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 127,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    app.update();
    assert_eq!(count_boomerangs(&mut app), 1);

    // Idle, then recall + hold until catch.
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 0,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    for _ in 0..10 {
        app.update();
    }
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 0,
        stick_y: 0,
        aim_angle: 0,
        buttons: PlayerInput::THROW_DOWN,
    };
    let mut caught = false;
    for _ in 0..30 {
        app.update();
        if count_boomerangs(&mut app) == 0 {
            caught = true;
            break;
        }
    }
    assert!(caught, "boomerang should have been caught before re-throw test");

    // After catch, with THROW_DOWN still held: a release tick with stick
    // aimed should spawn a fresh Flying boomerang.
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 0,
        stick_y: -127,
        aim_angle: 0,
        buttons: 0,
    };
    app.update();
    assert_eq!(
        count_boomerangs(&mut app),
        1,
        "owner should be able to throw again after catch",
    );
    let (b, _, vel) = first_boomerang(&mut app).unwrap();
    assert!(matches!(b.state, BoomerangState::Flying));
    let speed = Fix::const_from_int(THROW_SPEED_CM_PER_TICK);
    // South throw — vy = -speed, vx ~ 0.
    assert!((vel.y + speed).abs() <= Fix::from_bits(2));
    assert!(vel.x.abs() <= Fix::from_bits(2));
}

#[test]
fn cannot_throw_again_while_boomerang_in_flight() {
    let mut app = build_app();
    app.update();
    // First throw: hold + release.
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 127,
        stick_y: 0,
        aim_angle: 0,
        buttons: PlayerInput::THROW_DOWN,
    };
    app.update();
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 127,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    app.update();
    assert_eq!(count_boomerangs(&mut app), 1);

    // Second hold + release with a fresh edge: should NOT spawn a
    // second boomerang because the player still owns the in-flight one.
    for _ in 0..10 {
        app.update(); // let the first throw clear the release window
    }
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 127,
        stick_y: 0,
        aim_angle: 0,
        buttons: PlayerInput::THROW_DOWN,
    };
    app.update();
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 127,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    app.update();
    assert_eq!(
        count_boomerangs(&mut app),
        1,
        "second throw spawned a duplicate boomerang"
    );
}
