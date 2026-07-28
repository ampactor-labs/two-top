//! Feel batch (SIM_VERSION 6): grow-slow, fang clash, graze empower,
//! perfect-catch streak escalation, steered recall, Swap trade-places,
//! fire-lit pyres, sudden-death crumble, and the tap-throw opponent
//! fallback. Pure-helper tests + SyncTest integration per mechanic.

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy_ggrs::GgrsPlugin;
use bevy_ggrs::prelude::*;
use core::time::Duration;
use fixed_math::{Fix, RectF, Vec2F};
use sim::{
    BONE_PYRE_HALF_EXTENT_CM, BOOMERANG_HALF_EXTENT_CM, BonePyre, Boomerang, BoomerangMods,
    BoomerangState, CatchStreak, Dead, DefaultInputsPlugin, Empowered, FrameCount, GROW_MAX_FACTOR,
    GgrsCfg, LastClashFrame, MatchScore, PYRE_BURN_FRAMES, PickupKind, Player, PlayerInput,
    PositionF, PreviousPositionF, REACH_MAX_CM, SUDDEN_DEATH_FRAMES, SUDDEN_DEATH_MIN_FACTOR,
    SimPlugin, SynthesizedInputs, ThrowReach, VelocityF, grown_half_extent, streak_speed_factor,
    sudden_death_factor,
};

// ---- Pure helpers ----

#[test]
fn grown_half_extent_swells_linearly_to_max() {
    let base = Fix::const_from_int(BOOMERANG_HALF_EXTENT_CM);
    let reach = Fix::const_from_int(1000);
    // At the hand: base size.
    assert_eq!(grown_half_extent(Fix::ZERO, reach), base);
    // At full reach: GROW_MAX_FACTOR × base.
    let far = grown_half_extent(reach, reach);
    assert!((far - base * GROW_MAX_FACTOR).abs() <= Fix::from_bits(4));
    // Halfway: strictly between.
    let mid = grown_half_extent(Fix::const_from_int(500), reach);
    assert!(mid > base && mid < far);
    // Past reach: clamped, never beyond max.
    let past = grown_half_extent(Fix::const_from_int(2000), reach);
    assert!((past - far).abs() <= Fix::from_bits(4));
    // Degenerate zero reach: base (no divide blowup).
    assert_eq!(grown_half_extent(Fix::const_from_int(1), Fix::ZERO), base);
}

#[test]
fn streak_speed_factor_tiers() {
    assert_eq!(streak_speed_factor(0), Fix::const_from_int(1));
    assert_eq!(streak_speed_factor(1), Fix::const_from_int(1));
    assert!(streak_speed_factor(2) > Fix::const_from_int(1));
    assert!(streak_speed_factor(3) > streak_speed_factor(2));
    // Tier 3 is the cap — a longer storm doesn't keep compounding.
    assert_eq!(streak_speed_factor(9), streak_speed_factor(3));
}

#[test]
fn sudden_death_factor_shrinks_linearly_to_min() {
    let one = Fix::const_from_int(1);
    // Outside the window: full island.
    assert_eq!(sudden_death_factor(SUDDEN_DEATH_FRAMES), one);
    assert_eq!(sudden_death_factor(u32::MAX), one);
    // At the buzzer: the minimum island.
    assert_eq!(sudden_death_factor(0), SUDDEN_DEATH_MIN_FACTOR);
    // Mid-window: strictly between, monotonic.
    let mid = sudden_death_factor(SUDDEN_DEATH_FRAMES / 2);
    assert!(mid > SUDDEN_DEATH_MIN_FACTOR && mid < one);
    assert!(sudden_death_factor(SUDDEN_DEATH_FRAMES / 4) < mid);
}

// ---- Integration harness (mirrors hits.rs) ----

/// Default harness: `check_distance = 0` (no per-frame resimulation).
/// Most tests here spawn fangs OUT-OF-BAND (directly into the world), and
/// SyncTest resimulation despawns entities absent from restored snapshots —
/// the mechanics under test would vanish mid-assert. Rollback-safety of
/// these systems is covered by the organic-input sync_test 600f + fuzz +
/// replay-matrix gates; these tests verify the mechanics themselves.
fn build_two_player_app() -> App {
    build_two_player_app_cd(0)
}

/// Pure-input tests (no out-of-band spawns) use the full `check_distance`
/// so they ALSO prove the mechanic resimulates cleanly.
fn build_two_player_app_cd(check_distance: usize) -> App {
    let mut sb = SessionBuilder::<GgrsCfg>::new()
        .with_num_players(2)
        .unwrap()
        .with_check_distance(check_distance)
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

    // p0 at origin, p1 400 cm north (the depth-duel axis).
    app.world_mut().spawn((
        Player { handle: 0 },
        PositionF(Vec2F::ZERO),
        PreviousPositionF(Vec2F::ZERO),
        VelocityF(Vec2F::ZERO),
    ));
    app.world_mut().spawn((
        Player { handle: 1 },
        PositionF(Vec2F::from_cm(0, 400)),
        PreviousPositionF(Vec2F::from_cm(0, 400)),
        VelocityF(Vec2F::ZERO),
    ));
    app
}

fn set_inputs(app: &mut App, input: PlayerInput) {
    app.world_mut().resource_mut::<SynthesizedInputs>().0 = input;
}

fn spawn_fang(
    app: &mut App,
    owner: usize,
    pos: Vec2F,
    vel: Vec2F,
    state: BoomerangState,
) -> Entity {
    app.world_mut()
        .spawn((
            Boomerang {
                owner_handle: owner,
                state,
            },
            PositionF(pos),
            PreviousPositionF(pos),
            VelocityF(vel),
        ))
        .id()
}

fn player_pos(app: &mut App, handle: usize) -> Vec2F {
    let mut q = app.world_mut().query::<(&Player, &PositionF)>();
    q.iter(app.world())
        .find(|(p, _)| p.handle == handle)
        .map(|(_, pos)| pos.0)
        .unwrap()
}

// ---- Tap-throw fallback: a neutral-stick release throws at the enemy ----

#[test]
fn neutral_stick_release_throws_at_the_opponent() {
    let mut app = build_two_player_app_cd(2);
    app.update();
    // Hold THROW with a centered stick (both players — shared inputs)…
    set_inputs(
        &mut app,
        PlayerInput {
            stick_x: 0,
            stick_y: 0,
            aim_angle: 0,
            buttons: PlayerInput::THROW_DOWN,
        },
    );
    for _ in 0..10 {
        app.update();
    }
    // …then release with the stick STILL centered. Old behavior: silent dud.
    set_inputs(&mut app, PlayerInput::default());
    app.update();
    let mut q = app.world_mut().query::<(&Boomerang, &VelocityF)>();
    let fangs: Vec<(usize, Vec2F)> = q
        .iter(app.world())
        .map(|(b, v)| (b.owner_handle, v.0))
        .collect();
    assert_eq!(fangs.len(), 2, "both duelists' taps must still throw");
    for (owner, vel) in fangs {
        // p0 throws north at p1; p1 throws south at p0.
        if owner == 0 {
            assert!(vel.y > Fix::ZERO, "p0 fang must fly toward p1 (+y)");
        } else {
            assert!(vel.y < Fix::ZERO, "p1 fang must fly toward p0 (-y)");
        }
        assert!(
            vel.x.abs() <= Fix::from_bits(2),
            "duel axis throw is straight"
        );
    }
}

// ---- Fang clash ----

#[test]
fn enemy_fangs_clash_deflect_and_mark_the_frame() {
    let mut app = build_two_player_app();
    // Warm past frame 0 so a recorded clash frame is distinguishable from
    // the LastClashFrame default (0 = never).
    for _ in 0..4 {
        app.update();
    }
    // Two enemy fangs meeting head-on far from both players.
    let east = Vec2F::from_cm(10, 0);
    let a = spawn_fang(
        &mut app,
        0,
        Vec2F::from_cm(-8, 200),
        east,
        BoomerangState::Flying,
    );
    let b = spawn_fang(
        &mut app,
        1,
        Vec2F::from_cm(8, 200),
        Vec2F::ZERO - east,
        BoomerangState::Flying,
    );
    app.update();
    let world = app.world_mut();
    let av = world.get::<VelocityF>(a).unwrap().0;
    let bv = world.get::<VelocityF>(b).unwrap().0;
    assert!(av.x < Fix::ZERO, "fang A deflected back (-x), got {av:?}");
    assert!(bv.x > Fix::ZERO, "fang B deflected back (+x), got {bv:?}");
    assert!(world.get::<LastClashFrame>(a).unwrap().0 > 0);
    assert!(world.get::<LastClashFrame>(b).unwrap().0 > 0);
}

#[test]
fn same_owner_fangs_do_not_clash() {
    let mut app = build_two_player_app();
    app.update();
    let east = Vec2F::from_cm(10, 0);
    let a = spawn_fang(
        &mut app,
        0,
        Vec2F::from_cm(-8, 200),
        east,
        BoomerangState::Flying,
    );
    let b = spawn_fang(
        &mut app,
        0,
        Vec2F::from_cm(8, 200),
        Vec2F::ZERO - east,
        BoomerangState::Flying,
    );
    app.update();
    let world = app.world_mut();
    assert_eq!(world.get::<LastClashFrame>(a).unwrap().0, 0);
    assert_eq!(world.get::<LastClashFrame>(b).unwrap().0, 0);
}

// ---- Graze empower ----

#[test]
fn dashing_through_an_enemy_fang_empowers() {
    let mut app = build_two_player_app();
    app.update();
    // A slow enemy fang parked east of p0; p0 dashes east straight through
    // it. Dash i-frames (StunFrames) make the graze survivable — and the
    // closest call is the most rewarded.
    spawn_fang(
        &mut app,
        1,
        Vec2F::from_cm(60, 0),
        Vec2F::from_cm(-1, 0),
        BoomerangState::Flying,
    );
    // Real dash via inputs (manual DashState would fight rollback restore):
    // stick east + DASH edge.
    set_inputs(
        &mut app,
        PlayerInput {
            stick_x: 127,
            stick_y: 0,
            aim_angle: 0,
            buttons: PlayerInput::DASH_DOWN,
        },
    );
    // Dash covers 30 cm/tick for 10 ticks — plenty to cross the fang at 60.
    let mut empowered_seen = false;
    let mut died = false;
    for _ in 0..8 {
        app.update();
        let world = app.world_mut();
        let mut q = world.query::<(&Player, &Empowered, &Dead)>();
        if let Some((_, e, d)) = q.iter(world).find(|(p, _, _)| p.handle == 0) {
            empowered_seen |= e.0;
            died |= d.is_dying();
        }
    }
    assert!(!died, "dash i-frames keep the graze survivable");
    assert!(
        empowered_seen,
        "dashing through the enemy fang must empower"
    );
}

// ---- Perfect-catch streak escalation ----

#[test]
fn three_perfect_catches_unlock_the_lightning_reach() {
    let mut app = build_two_player_app();
    app.update();
    // Three perfect catches: a Returning fang whose recall began THIS frame,
    // already overlapping its owner, is caught inside the perfect window.
    for expected_streak in 1..=3u32 {
        let now = app.world().resource::<FrameCount>().0;
        spawn_fang(
            &mut app,
            0,
            Vec2F::ZERO,
            Vec2F::ZERO,
            BoomerangState::Returning { since: now },
        );
        app.update();
        let world = app.world_mut();
        let mut q = world.query::<(&Player, &CatchStreak)>();
        let streak = q
            .iter(world)
            .find(|(p, _)| p.handle == 0)
            .map(|(_, s)| s.0)
            .unwrap();
        assert_eq!(streak, expected_streak);
    }
    // A quick tap now throws with FULL board reach — the storm breaks.
    set_inputs(
        &mut app,
        PlayerInput {
            stick_x: 0,
            stick_y: 0,
            aim_angle: 0,
            buttons: PlayerInput::THROW_DOWN,
        },
    );
    for _ in 0..3 {
        app.update();
    }
    set_inputs(&mut app, PlayerInput::default());
    app.update();
    let world = app.world_mut();
    let mut q = world.query::<(&Boomerang, &ThrowReach)>();
    let reach = q
        .iter(world)
        .find(|(b, _)| b.owner_handle == 0)
        .map(|(_, r)| r.0)
        .expect("p0 threw a fang");
    assert_eq!(
        reach,
        Fix::const_from_int(REACH_MAX_CM),
        "lightning throw reaches the whole board at any charge"
    );
}

// ---- Steered recall ----

#[test]
fn aim_stick_bends_the_return_arc() {
    let mut app = build_two_player_app();
    app.update();
    let now = app.world().resource::<FrameCount>().0;
    let fang = spawn_fang(
        &mut app,
        0,
        Vec2F::from_cm(0, 300),
        Vec2F::ZERO,
        BoomerangState::Returning { since: now },
    );
    // Owner holds AIM with the stick hard east: the wire stick carries the
    // aim vector during AIM_ACTIVE, and the recall recompute adds the bend.
    set_inputs(
        &mut app,
        PlayerInput {
            stick_x: 127,
            stick_y: 0,
            aim_angle: 0,
            buttons: PlayerInput::AIM_ACTIVE,
        },
    );
    app.update();
    let vel = app.world().get::<VelocityF>(fang).unwrap().0;
    assert!(
        vel.y < Fix::ZERO,
        "the pull home still dominates (fang above owner flies -y)"
    );
    assert!(
        vel.x > Fix::ZERO,
        "the aim stick bends the arc east, got {vel:?}"
    );
}

// ---- Fire-lit pyres ----

fn spawn_pyre(app: &mut App, cx: i32, cy: i32) -> Entity {
    let half = Fix::const_from_int(BONE_PYRE_HALF_EXTENT_CM);
    app.world_mut()
        .spawn(BonePyre::intact(RectF::from_center_half_extents(
            Vec2F::from_cm(cx, cy),
            Vec2F::new(half, half),
        )))
        .id()
}

#[test]
fn fire_fang_lights_the_pyre_it_shatters() {
    let mut app = build_two_player_app();
    app.update();
    let pyre = spawn_pyre(&mut app, 300, 200);
    let fang = spawn_fang(
        &mut app,
        0,
        Vec2F::from_cm(300, 160),
        Vec2F::from_cm(0, 20),
        BoomerangState::Flying,
    );
    app.world_mut().entity_mut(fang).insert(BoomerangMods {
        modifier: Some(PickupKind::Fire),
        is_secondary: false,
        despawn_at_frame: None,
        wall_bounces: 0,
    });
    for _ in 0..6 {
        app.update();
    }
    let world = app.world_mut();
    let p = world.get::<BonePyre>(pyre).unwrap();
    assert!(p.shattered, "the impact still shatters");
    let now = world.resource::<FrameCount>().0;
    assert!(p.is_burning(now), "a FIRE fang lights the bones");
    assert_eq!(p.lit_by, 0, "credit tracks the igniter");
    assert!(p.lit_until_frame.unwrap() <= now + PYRE_BURN_FRAMES);
}

#[test]
fn burning_pyre_kills_and_credits_the_igniter() {
    let mut app = build_two_player_app();
    app.update();
    let pyre = spawn_pyre(&mut app, 300, 200);
    let now = app.world().resource::<FrameCount>().0;
    {
        let mut p = app.world_mut().get_mut::<BonePyre>(pyre).unwrap();
        p.shattered = true;
        p.lit_until_frame = Some(now + PYRE_BURN_FRAMES);
        p.lit_by = 0;
    }
    // Walk p1 onto the burning pyre.
    {
        let world = app.world_mut();
        let mut q = world.query::<(&Player, &mut PositionF, &mut PreviousPositionF)>();
        for (p, mut pos, mut prev) in q.iter_mut(world) {
            if p.handle == 1 {
                pos.0 = Vec2F::from_cm(300, 200);
                prev.0 = pos.0;
            }
        }
    }
    app.update();
    let world = app.world_mut();
    let mut q = world.query::<(&Player, &Dead)>();
    let dying = q
        .iter(world)
        .find(|(p, _)| p.handle == 1)
        .map(|(_, d)| d.is_dying())
        .unwrap();
    assert!(dying, "a burning pyre is lethal");
    assert_eq!(
        world.resource::<MatchScore>().p0,
        1,
        "kill credits the igniter"
    );
}

// ---- Swap trade-places ----

#[test]
fn swap_fang_trades_places_on_the_recall_press() {
    let mut app = build_two_player_app();
    app.update();
    let fang_at = Vec2F::from_cm(200, 250);
    let fang = spawn_fang(&mut app, 0, fang_at, Vec2F::ZERO, BoomerangState::Flying);
    app.world_mut().entity_mut(fang).insert(BoomerangMods {
        modifier: Some(PickupKind::Swap),
        is_secondary: false,
        despawn_at_frame: None,
        wall_bounces: 0,
    });
    let p0_before = player_pos(&mut app, 0);
    // The recall press (THROW down with every slot out) trades places.
    // Two ticks: SynthesizedInputs reach the sim with a 1-tick latency.
    set_inputs(
        &mut app,
        PlayerInput {
            stick_x: 0,
            stick_y: 0,
            aim_angle: 0,
            buttons: PlayerInput::THROW_DOWN,
        },
    );
    app.update();
    app.update();
    let p0_after = player_pos(&mut app, 0);
    assert_eq!(p0_after, fang_at, "owner teleports to the fang");
    let world = app.world_mut();
    let boom = world.get::<Boomerang>(fang).unwrap();
    assert!(
        matches!(boom.state, BoomerangState::Loose),
        "the fang drops Loose where the owner stood"
    );
    assert_eq!(
        world.get::<PositionF>(fang).unwrap().0,
        p0_before,
        "the fang lands at the owner's old spot"
    );
}
