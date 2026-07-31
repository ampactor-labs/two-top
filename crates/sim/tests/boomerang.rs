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
use fixed_math::{Fix, RectF, Vec2F};
use sim::{
    ARENA_HALF_HEIGHT_CM, ARENA_HALF_WIDTH_CM, Boomerang, BoomerangState, Dead,
    DefaultInputsPlugin, EMPOWERED_THROW_SPEED_CM_PER_TICK, GgrsCfg, INPUT_HISTORY_LEN,
    OOB_GRACE_FRAMES, Player, PlayerInput, PositionF, PreviousPositionF, RECALL_SPEED_CM_PER_TICK,
    SimPlugin, SynthesizedInputs, THROW_SPEED_CM_PER_TICK, VelocityF, Wall, WallKind, arena_walls,
    boomerang_rect, player_rect, recall_velocity, reflect_velocity_for_push, swept_wall_contact,
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
fn throw_fires_on_exact_release_with_direction() {
    // THROW held all through the ring (incl. the previous tick), released on the
    // current tick → the exact release edge fires (forgiveness window dropped).
    let ring = ring_with_release_at(INPUT_HISTORY_LEN);
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
    let ring = ring_with_release_at(INPUT_HISTORY_LEN); // held until this tick's release
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
    app.insert_resource(TimeUpdateStrategy::ManualDuration(sim::tick_duration()));
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

/// Hold THROW (stick centered, no drift) to FULL charge so the next release
/// launches at the full `THROW_SPEED` / max reach — the charge pass made throw
/// speed depend on hold time, so integration tests that want a full-power fang
/// wind up first. Leaves THROW still held; the caller releases with a direction.
fn hold_full_charge(app: &mut App) {
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 0,
        stick_y: 0,
        aim_angle: 0,
        buttons: PlayerInput::THROW_DOWN,
    };
    // A few extra ticks past the cap absorb the 1-tick SynthesizedInputs→sim
    // latency, so charge is guaranteed saturated at CHARGE_MAX (full power).
    for _ in 0..sim::CHARGE_MAX_FRAMES + 4 {
        app.update();
    }
}

fn first_boomerang(app: &mut App) -> Option<(Boomerang, Vec2F, Vec2F)> {
    let mut q = app
        .world_mut()
        .query::<(&Boomerang, &PositionF, &VelocityF)>();
    q.iter(app.world()).next().map(|(b, p, v)| (*b, p.0, v.0))
}

#[test]
fn tap_release_spawns_one_boomerang_in_stick_direction() {
    let mut app = build_app();
    app.update(); // SyncTestSession warmup

    // Hold THROW_DOWN to full charge, stick centered (so player doesn't drift —
    // makes the spawn position predictable, and gives a full-power throw).
    hold_full_charge(&mut app);
    assert_eq!(count_boomerangs(&mut app), 0, "no spawn while held");

    // Release with stick aimed right. The release-from-held transition fires
    // this tick via the same-tick `just_released` check; throw direction comes
    // from the stick, speed/reach from the accumulated charge (now full).
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
        "throw should have spawned a boomerang"
    );

    let (boom, pos, vel) = first_boomerang(&mut app).unwrap();
    assert_eq!(boom.owner_handle, 0);
    assert!(matches!(boom.state, BoomerangState::Flying));
    // The player is ROOTED while charging (the release tick still carries
    // charge>0 when player_movement runs), so the fang spawns at the origin
    // — no strafe-while-winding-up.
    assert!(pos.x.abs() <= Fix::from_bits(2));
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
    // Hold to full charge (stick centered so player doesn't drift), release
    // with stick aimed +x to set the throw direction at full THROW_SPEED.
    hold_full_charge(&mut app);
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
    // Grow-slow: a Flying fang bleeds FLY_DECAY per tick, so the first full
    // flight tick advances THROW_SPEED × FLY_DECAY (24 × 0.99 ≈ 23.76).
    let expected = Fix::const_from_int(THROW_SPEED_CM_PER_TICK) * sim::FLY_DECAY;
    assert!(
        (dx - expected).abs() <= Fix::from_bits(2),
        "boomerang dx {dx} should be ~{expected} cm/tick (throw speed × fly decay)",
    );
}

// ---- Cycle 2: wall ricochet ----

#[test]
fn reflect_velocity_x_push_flips_vx_only() {
    // A fang moving INTO the wall (-x) with an outward +x push reflects to +x.
    let vel = Vec2F::new(Fix::const_from_int(-50), Fix::const_from_int(20));
    let push = Vec2F::new(Fix::const_from_int(5), Fix::ZERO); // x push (outward +x)
    let r = reflect_velocity_for_push(vel, push);
    assert_eq!(r.x, Fix::const_from_int(50));
    assert_eq!(r.y, Fix::const_from_int(20));
}

#[test]
fn reflect_does_not_double_flip_a_fang_already_leaving() {
    // Regression for the deafening in/out wall oscillation: a fang already
    // moving OUT (+x) with an outward +x push must NOT be flipped back in.
    let vel = Vec2F::new(Fix::const_from_int(50), Fix::const_from_int(20));
    let push = Vec2F::new(Fix::const_from_int(5), Fix::ZERO);
    let r = reflect_velocity_for_push(vel, push);
    assert_eq!(r, vel, "an already-leaving fang must keep its velocity");
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

fn build_arena_app_at(x_cm: i32, y_cm: i32) -> App {
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
    app.add_plugins(sim::InfiniteRoundPlugin);
    app.add_plugins(DefaultInputsPlugin);
    app.insert_resource(Session::SyncTest(session));

    app.world_mut().spawn((
        Player { handle: 0 },
        PositionF(Vec2F::from_cm(x_cm, y_cm)),
        PreviousPositionF(Vec2F::from_cm(x_cm, y_cm)),
        VelocityF(Vec2F::ZERO),
    ));
    for w in arena_walls() {
        app.world_mut().spawn(w);
    }
    app
}

fn build_arena_app() -> App {
    // Place the player just inside the east wall so a single throw
    // rightward will hit the wall on its first or second flight tick.
    build_arena_app_at(ARENA_HALF_WIDTH_CM - 50, 0)
}

/// Throw +x and idle; returns `(saw_returning, despawned)` over `ticks`.
fn throw_east_and_watch(app: &mut App, ticks: usize) -> (bool, bool, bool) {
    app.update(); // warmup
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
    assert!(
        matches!(
            first_boomerang(app).unwrap().0.state,
            BoomerangState::Flying
        ),
        "fang should launch Flying"
    );
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput::default();
    let half_w = Fix::const_from_int(ARENA_HALF_WIDTH_CM);
    let (mut exited, mut returning, mut despawned) = (false, false, false);
    for _ in 0..ticks {
        app.update();
        match first_boomerang(app) {
            Some((b, pos, _)) => {
                if matches!(b.state, BoomerangState::Flying) && pos.x.abs() > half_w {
                    exited = true; // flew OUT past the boundary while still flying
                }
                if matches!(b.state, BoomerangState::Returning { .. }) {
                    returning = true;
                    break;
                }
            }
            None => {
                despawned = true;
                break;
            }
        }
    }
    (exited, returning, despawned)
}

#[test]
fn boomerang_exits_boundary_then_returns_via_cap() {
    // The outer ring is PERMEABLE to fangs: a thrown primary flies straight OUT
    // past the boundary (open field — the same way a player can now leave the
    // field), then auto-returns via the throw-distance cap. It must NOT turn
    // around at the wall, and must never escape (the cap brings it home).
    let mut app = build_arena_app(); // player just inside the east boundary
    // ~40 ticks to travel the 1000 cm cap at the halved 25 cm/tick — give the
    // watcher generous headroom to see the auto-return after the fang exits.
    let (exited, returning, despawned) = throw_east_and_watch(&mut app, 90);
    assert!(
        !despawned,
        "the distance cap should return the fang, not let it despawn"
    );
    assert!(
        exited,
        "fang should fly OUT past the boundary, not turn at the wall"
    );
    assert!(returning, "fang should auto-return via the distance cap");
}

#[test]
fn boomerang_banks_off_inner_obstacle_and_stays_flying() {
    // Inner cover (an Obstacle wall) ricochets a fang — and the FIRST solid
    // contact is a free bank: the x-velocity flips negative (clean
    // reflection off the surface normal) while the fang stays Flying (and
    // therefore lethal — the deliberate carom into a kill).
    let mut app = build_arena_app_at(0, 0);
    app.world_mut().spawn(Wall {
        kind: WallKind::Obstacle,
        rect: RectF::from_min_max(Vec2F::from_cm(150, -100), Vec2F::from_cm(200, 100)),
    });
    app.update();
    hold_full_charge(&mut app);
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 127,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    app.update();
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput::default();
    let mut reflected = false;
    for _ in 0..8 {
        app.update();
        if let Some((b, _, v)) = first_boomerang(&mut app)
            && v.x < Fix::ZERO
        {
            assert!(
                matches!(b.state, BoomerangState::Flying),
                "the first cover contact is a free bank — the fang keeps flying, got {:?}",
                b.state
            );
            reflected = true;
            break;
        }
    }
    assert!(reflected, "fang should ricochet off the inner obstacle");
}

/// Centre player throws east between two facing pillars: the east pillar
/// banks the fang (first free bounce, still Flying), the west pillar is the
/// SECOND solid contact that knocks it Loose. Returns once the fang is
/// Loose, with the stick idle.
fn app_with_loose_fang() -> App {
    let mut app = build_arena_app_at(0, 0);
    app.world_mut().spawn(Wall {
        kind: WallKind::Obstacle,
        rect: RectF::from_min_max(Vec2F::from_cm(150, -100), Vec2F::from_cm(200, 100)),
    });
    app.world_mut().spawn(Wall {
        kind: WallKind::Obstacle,
        rect: RectF::from_min_max(Vec2F::from_cm(-200, -100), Vec2F::from_cm(-150, 100)),
    });
    app.update();
    hold_full_charge(&mut app);
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 127,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    app.update();
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput::default();
    for _ in 0..40 {
        app.update();
        if matches!(
            first_boomerang(&mut app).map(|(b, _, _)| b.state),
            Some(BoomerangState::Loose)
        ) {
            break;
        }
    }
    app
}

#[test]
fn second_solid_contact_knocks_the_fang_loose() {
    // The bank budget is exactly one: pillar one reflects a still-Flying
    // fang, pillar two drops it. `app_with_loose_fang` drives that exact
    // ping-pong; reaching Loose (asserted by the fixture's consumers below)
    // plus the bank test above pins both halves of the rule. Here: the
    // fang really did spend a bounce before dropping.
    let mut app = app_with_loose_fang();
    let mut q = app
        .world_mut()
        .query::<(&sim::Boomerang, &sim::BoomerangMods)>();
    let (boom, mods) = q.iter(app.world()).next().expect("fang exists");
    assert!(matches!(boom.state, BoomerangState::Loose));
    assert!(
        mods.wall_bounces > sim::MAX_FREE_WALL_BOUNCES,
        "the drop must come from spending the budget, got {} bounces",
        mods.wall_bounces
    );
}

#[test]
fn fang_goes_loose_and_bleeds_speed_after_cover_bounce() {
    let mut app = app_with_loose_fang();
    let (b, _, v0) = first_boomerang(&mut app).expect("fang exists after the bounce");
    assert!(
        matches!(b.state, BoomerangState::Loose),
        "a cover bounce knocks the fang loose, not pinballing"
    );
    let speed0 = v0.length();
    app.update();
    if let Some((b, _, v1)) = first_boomerang(&mut app) {
        assert!(matches!(b.state, BoomerangState::Loose));
        assert!(
            v1.length() < speed0,
            "a loose fang must bleed speed (drag): {speed0:?} -> {:?}",
            v1.length()
        );
    }
}

#[test]
fn loose_fang_is_hold_recalled() {
    let mut app = app_with_loose_fang();
    assert!(matches!(
        first_boomerang(&mut app).unwrap().0.state,
        BoomerangState::Loose
    ));
    // A THROW_DOWN edge hold-recalls the dropped fang back to the owner.
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 0,
        stick_y: 0,
        aim_angle: 0,
        buttons: PlayerInput::THROW_DOWN,
    };
    app.update();
    // Returning now (or already caught as it homed in — both = "came home").
    let came_home = first_boomerang(&mut app)
        .map(|(b, _, _)| matches!(b.state, BoomerangState::Returning { .. }))
        .unwrap_or(true);
    assert!(came_home, "hold-recall should summon the loose fang back");
}

#[test]
fn loose_fang_is_picked_up_on_owner_overlap() {
    // The loose fang drifts back toward the idle owner and is picked up on
    // overlap (walk-over retrieval), freeing the owner to throw again.
    let mut app = app_with_loose_fang();
    let mut picked_up = false;
    for _ in 0..40 {
        app.update();
        if first_boomerang(&mut app).is_none() {
            picked_up = true;
            break;
        }
    }
    assert!(
        picked_up,
        "owner should pick up the loose fang it walks over"
    );
}

#[test]
fn player_left_out_of_bounds_dies_after_grace() {
    // The outer ring no longer contains players: parked past the east edge,
    // the void claims them once they've been out longer than the grace window.
    let mut app = build_arena_app_at(ARENA_HALF_WIDTH_CM + 100, 0);
    app.update(); // warmup
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput::default();
    let mut died = false;
    for _ in 0..(OOB_GRACE_FRAMES as usize + 5) {
        app.update();
        let mut q = app.world_mut().query::<(&Player, &Dead)>();
        if q.iter(app.world())
            .any(|(p, d)| p.handle == 0 && d.is_dying())
        {
            died = true;
            break;
        }
    }
    assert!(
        died,
        "a player out of bounds past the grace window should die"
    );
}

#[test]
fn swept_contact_catches_fast_tunnel_through_north_wall() {
    // A fang at ~85 cm/tick (Bouncy / Fire+empowered) phased so NEITHER
    // endpoint AABB overlaps the 50 cm north wall: with the v12 half-extent
    // of 13, centers in [737, 813] overlap the wall, so prev y=735 sits
    // just short of the band and cur y=820 just past it. A plain point
    // check returns None at both ends and the fang escapes; the swept check
    // must reflect it at the entry face.
    let north = RectF::from_min_max(Vec2F::from_cm(-500, 750), Vec2F::from_cm(500, 800));
    let prev = Vec2F::from_cm(0, 735);
    let cur = Vec2F::from_cm(0, 820);
    // Sanity: both endpoints are genuinely clear (so this is a true tunnel).
    assert!(sim::resolve_collision(boomerang_rect(prev), north).is_none());
    assert!(sim::resolve_collision(boomerang_rect(cur), north).is_none());
    let (_contact, push) =
        swept_wall_contact(prev, cur, north).expect("fast tunnel must be caught");
    assert_eq!(push.x, Fix::ZERO, "north tunnel must reflect on Y, not X");
    assert!(
        push.y < Fix::ZERO,
        "should reflect back down into the arena"
    );
}

#[test]
fn charge_scales_throw_speed_by_hold_time() {
    // The throw's speed scales with how long THROW is held (the CHARGE): a short
    // hold lobs a slow fang, a full hold hurls a fast one at full THROW_SPEED.
    // Direction always comes from the release-frame stick (+x here).
    fn throw_after_holding(hold_ticks: u32) -> Vec2F {
        let mut app = build_arena_app_at(0, 0);
        app.update(); // warmup
        app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
            stick_x: 0,
            stick_y: 0,
            aim_angle: 0,
            buttons: PlayerInput::THROW_DOWN,
        };
        for _ in 0..hold_ticks {
            app.update();
        }
        // Release +x → throw fires with the accumulated charge.
        app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
            stick_x: 127,
            stick_y: 0,
            aim_angle: 0,
            buttons: 0,
        };
        app.update();
        first_boomerang(&mut app).expect("boomerang spawned").2
    }

    let full = throw_after_holding(sim::CHARGE_MAX_FRAMES);
    let short = throw_after_holding(2);

    // Both fly +x (the stick direction), neither drifts in y.
    assert!(full.x > Fix::ZERO && short.x > Fix::ZERO, "both throw +x");
    assert!(full.y.abs() <= Fix::from_bits(4) && short.y.abs() <= Fix::from_bits(4));
    // Full charge launches at THROW_SPEED.
    assert!(
        (full.x - Fix::const_from_int(THROW_SPEED_CM_PER_TICK)).abs() <= Fix::const_from_int(1),
        "full charge should launch at THROW_SPEED, got {full:?}"
    );
    // A short hold is clearly slower.
    assert!(
        short.x < full.x - Fix::const_from_int(6),
        "a short charge should be clearly slower: short={short:?} full={full:?}"
    );
}

#[test]
fn boomerang_auto_recalls_at_max_throw_distance() {
    // Parked near the south wall and throwing straight up: the cap
    // (1000 cm) is reached at y≈+300, well short of the north wall
    // (~1450 cm away), so the fang must turn itself around with NO recall
    // press and without ever touching a wall.
    let mut app = build_arena_app_at(0, -(ARENA_HALF_HEIGHT_CM - 50));
    app.update(); // warmup
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 0,
        stick_y: 0,
        aim_angle: 0,
        buttons: PlayerInput::THROW_DOWN,
    };
    app.update();
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 0,
        stick_y: 127,
        aim_angle: 0,
        buttons: 0,
    };
    app.update();
    let (b, _, v) = first_boomerang(&mut app).expect("boomerang spawned");
    assert!(
        matches!(b.state, BoomerangState::Flying),
        "should start Flying"
    );
    assert!(v.y > Fix::ZERO, "should launch upward");

    // Idle the stick — never press recall — and watch for the auto-recall.
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput::default();
    let mut auto_recalled = false;
    // ~40 ticks to reach the 1000 cm cap at the halved 25 cm/tick; budget extra.
    for _ in 0..90 {
        app.update();
        if let Some((b, pos, _)) = first_boomerang(&mut app)
            && matches!(b.state, BoomerangState::Returning { .. })
        {
            // Turned around short of the north wall (never hit it).
            assert!(
                pos.y < Fix::const_from_int(ARENA_HALF_HEIGHT_CM),
                "auto-recall should fire before the north wall, y={pos:?}"
            );
            auto_recalled = true;
            break;
        }
    }
    assert!(
        auto_recalled,
        "boomerang did not auto-recall at max throw distance"
    );
}

// (Ricochet speed-preservation is covered by the `reflect_velocity_*` unit
// tests and `boomerang_ricochets_off_inner_obstacle`; the old test bounced off
// the arena edge, which now auto-returns at recall speed by design.)

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
    assert!(
        (v.x + speed).abs() <= Fix::from_bits(2),
        "vx {} should be -{speed}",
        v.x
    );
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
        matches!(boom.state, BoomerangState::Returning { .. }),
        "recall press should have flipped Flying → Returning",
    );
    // Velocity now points back toward owner (player at origin), so vx < 0.
    assert!(
        vel.x < Fix::ZERO,
        "recall velocity vx {} should be negative",
        vel.x
    );
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
    assert!(matches!(boom.state, BoomerangState::Returning { .. }));

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
    assert!(
        caught,
        "Returning boomerang should have been caught by owner"
    );
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
        Boomerang {
            owner_handle,
            state: BoomerangState::Flying,
        },
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
    assert!(
        caught,
        "boomerang should have been caught before re-throw test"
    );

    // The still-held THROW is now INERT — the hold outlived its purpose
    // (it was a recall press, and the fang is home). Releasing it must NOT
    // lob a surprise throw.
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 0,
        stick_y: -127,
        aim_angle: 0,
        buttons: 0,
    };
    app.update();
    assert_eq!(
        count_boomerangs(&mut app),
        0,
        "releasing a leftover recall hold must not throw",
    );

    // A FRESH press + hold + release: the catch really did free the slot.
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 0,
        stick_y: 0,
        aim_angle: 0,
        buttons: PlayerInput::THROW_DOWN,
    };
    for _ in 0..5 {
        app.update();
    }
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
    // South throw — vy negative, vx ~ 0. Speed depends on the short fresh
    // hold's CHARGE, and the catch may also empower it; here we only assert
    // re-throw freedom + aim, so accept any plausible charged speed from the
    // min-charge floor up to the empowered ceiling. Exact speed selection is
    // pinned in catch.rs.
    let speed = vel.y.abs();
    assert!(
        vel.y < Fix::ZERO
            && speed >= Fix::const_from_int(7)
            && speed <= Fix::const_from_int(EMPOWERED_THROW_SPEED_CM_PER_TICK + 1),
        "re-throw should be south at a plausible charged speed (vy={:?})",
        vel.y,
    );
    assert!(vel.x.abs() <= Fix::from_bits(2));
}

#[test]
fn cannot_throw_again_while_boomerang_in_flight() {
    let mut app = build_app();
    app.update();
    // First throw: FULL charge + release, so the fang's reach is the whole
    // board and it is genuinely still in flight when the second press
    // lands. (A tap throw's tiny reach had it turn around and get caught
    // within the test window, which made this assert time-sensitive to
    // speed tuning instead of testing the duplicate block.)
    hold_full_charge(&mut app);
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

// ---- SIM_VERSION 8: fresh-press throw arming ----

fn player_x(app: &mut App) -> Fix {
    let mut q = app.world_mut().query_filtered::<&PositionF, With<Player>>();
    q.iter(app.world()).next().unwrap().0.x
}

fn charge_of(app: &mut App) -> u32 {
    let mut q = app.world_mut().query::<&sim::ThrowCharge>();
    q.iter(app.world()).next().unwrap().0
}

#[test]
fn leftover_recall_hold_is_inert_walks_free_and_never_charges() {
    let mut app = build_app();
    app.update();

    // Throw at FULL charge (a tap lob's reach is short enough that the
    // duelist catches it before the recall press below, which would make
    // that press a legitimate fresh arm and test nothing), idle a beat,
    // then recall-press and hold to the catch.
    hold_full_charge(&mut app);
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 127,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    app.update();
    assert_eq!(count_boomerangs(&mut app), 1);
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput::default();
    for _ in 0..6 {
        app.update();
    }
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 0,
        stick_y: 0,
        aim_angle: 0,
        buttons: PlayerInput::THROW_DOWN,
    };
    let mut caught = false;
    for _ in 0..40 {
        app.update();
        if count_boomerangs(&mut app) == 0 {
            caught = true;
            break;
        }
    }
    assert!(caught, "recall should have brought the fang home");

    // Keep the dead hold down with the stick deflected and the AIM bit set
    // (exactly what the touch layer sends): no wind-up may arm, and the
    // player must WALK — the aim lock only binds a LIVE hold.
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = PlayerInput {
        stick_x: 127,
        stick_y: 0,
        aim_angle: 128,
        buttons: PlayerInput::THROW_DOWN | PlayerInput::AIM_ACTIVE,
    };
    let x_before = player_x(&mut app);
    for _ in 0..10 {
        app.update();
    }
    assert_eq!(
        charge_of(&mut app),
        0,
        "a hold kept down through the catch must not re-arm the charge"
    );
    let moved = player_x(&mut app) - x_before;
    assert!(
        moved > Fix::const_from_int(50),
        "inert hold must not root the player (moved {moved:?})"
    );
    assert_eq!(count_boomerangs(&mut app), 0, "and nothing was thrown");
}
