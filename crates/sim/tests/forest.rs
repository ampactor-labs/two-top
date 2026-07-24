//! The Forest arena — bone trees + spreading fire (2026-07-16 roster).
//!
//! Drives the tree systems directly through the standard sim-test ceremony
//! (MinimalPlugins + GgrsPlugin + SimPlugin, `RunSystemOnce`): chip-felling,
//! Heavy's one-hit plow, fire ignition + BFu-style proximity spread, burn
//! kills with the pyre credit rule, player blocking, and the snapshot
//! round-trip that keeps theater scrubbing honest.

use core::time::Duration;

use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy_ggrs::GgrsPlugin;
use bevy_ggrs::prelude::*;
use fixed_math::{Fix, Vec2F};
use sim::{
    ArenaId, BoneTree, Boomerang, BoomerangMods, BoomerangState, Dead, FrameCount, GgrsCfg,
    MatchScore, MatchState, PickupKind, Player, PositionF, PreviousPositionF, SimPlugin,
    SimSnapshot, TREE_BURN_FRAMES, TREE_HP, TREE_SPREAD_DELAY_FRAMES, VelocityF,
    arena_trees_for, boomerang_tree_collision, tree_burn_kills, tree_collision, tree_fire,
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

/// A standing tree centered at cm coords.
fn tree_at(app: &mut App, cx: i32, cy: i32) -> Entity {
    let trees = arena_trees_for(ArenaId::Forest);
    let template = trees[0];
    let half = (template.rect.max.x - template.rect.min.x) * Fix::lit("0.5");
    let center = Vec2F::from_cm(cx, cy);
    app.world_mut()
        .spawn(BoneTree::standing(fixed_math::RectF::from_center_half_extents(
            center,
            Vec2F::new(half, half),
        )))
        .id()
}

fn fang_at(app: &mut App, modifier: Option<PickupKind>, cx: i32, cy: i32, vx: i32) -> Entity {
    app.world_mut()
        .spawn((
            Boomerang {
                owner_handle: 0,
                state: BoomerangState::Flying,
            },
            BoomerangMods {
                modifier,
                is_secondary: false,
                despawn_at_frame: None,
                wall_bounces: 0,
            },
            PositionF(Vec2F::from_cm(cx, cy)),
            PreviousPositionF(Vec2F::from_cm(cx, cy)),
            VelocityF(Vec2F::from_cm(vx, 0)),
        ))
        .id()
}

fn tree(app: &mut App, e: Entity) -> BoneTree {
    *app.world().entity(e).get::<BoneTree>().unwrap()
}

#[test]
fn grove_is_point_symmetric() {
    let trees = arena_trees_for(ArenaId::Forest);
    assert!(trees.len() >= 10, "a grove, not a shrub");
    for t in &trees {
        let mirrored = trees.iter().any(|o| {
            o.rect.min.x + t.rect.max.x == Fix::ZERO && o.rect.min.y + t.rect.max.y == Fix::ZERO
        });
        assert!(mirrored, "every tree has its 180-degree twin: {:?}", t.rect);
    }
    // Only the Forest grows them.
    for arena in sim::ALL_ARENAS {
        if arena != ArenaId::Forest {
            assert!(arena_trees_for(arena).is_empty(), "{arena:?} has no trees");
        }
    }
}

#[test]
fn two_chips_fell_a_tree_and_a_stump_stops_blocking() {
    let mut app = bare_app();
    let t = tree_at(&mut app, 0, 0);
    for chip in 1..=TREE_HP {
        let f = fang_at(&mut app, None, -20, 0, 50);
        app.world_mut()
            .run_system_once(boomerang_tree_collision)
            .unwrap();
        let tr = tree(&mut app, t);
        assert_eq!(tr.hp, TREE_HP - chip, "chip {chip} lands");
        assert_eq!(tr.felled, chip == TREE_HP, "falls exactly on the last chip");
        // Each fresh fang banks its one free bounce off the trunk (still
        // Flying, still lethal); the chip lands either way. The drop on a
        // SECOND solid contact is pinned by the two-pillar boomerang test.
        let boom = *app.world().entity(f).get::<Boomerang>().unwrap();
        assert!(matches!(boom.state, BoomerangState::Flying));
        app.world_mut().despawn(f);
    }
    // A felled tree is open ground: a fresh fang flies straight through.
    let f = fang_at(&mut app, None, -20, 0, 50);
    app.world_mut()
        .run_system_once(boomerang_tree_collision)
        .unwrap();
    let boom = *app.world().entity(f).get::<Boomerang>().unwrap();
    assert!(matches!(boom.state, BoomerangState::Flying), "stump doesn't block");
}

#[test]
fn heavy_fells_in_one_and_plows_through() {
    let mut app = bare_app();
    let t = tree_at(&mut app, 0, 0);
    let f = fang_at(&mut app, Some(PickupKind::Heavy), -20, 0, 50);
    app.world_mut()
        .run_system_once(boomerang_tree_collision)
        .unwrap();
    assert!(tree(&mut app, t).felled, "one Heavy hit fells");
    let vel = app.world().entity(f).get::<VelocityF>().unwrap().0;
    assert_eq!(vel, Vec2F::from_cm(50, 0), "no ricochet — it plows");
}

#[test]
fn returning_and_phantom_phase_through() {
    let mut app = bare_app();
    let t = tree_at(&mut app, 0, 0);
    let ph = fang_at(&mut app, Some(PickupKind::Phantom), -20, 0, 50);
    let ret = fang_at(&mut app, None, 20, 0, 50);
    app.world_mut().entity_mut(ret).get_mut::<Boomerang>().unwrap().state =
        BoomerangState::Returning { since: 0 };
    app.world_mut()
        .run_system_once(boomerang_tree_collision)
        .unwrap();
    assert_eq!(tree(&mut app, t).hp, TREE_HP, "nobody chipped it");
    for f in [ph, ret] {
        let vel = app.world().entity(f).get::<VelocityF>().unwrap().0;
        assert_eq!(vel, Vec2F::from_cm(50, 0), "undeflected");
    }
}

#[test]
fn fire_fang_ignites_instead_of_chipping_and_burnout_fells() {
    let mut app = bare_app();
    let t = tree_at(&mut app, 0, 0);
    fang_at(&mut app, Some(PickupKind::Fire), -20, 0, 50);
    app.world_mut()
        .run_system_once(boomerang_tree_collision)
        .unwrap();
    let tr = tree(&mut app, t);
    assert_eq!(tr.hp, TREE_HP, "fire doesn't chip");
    assert_eq!(tr.lit_until_frame, Some(TREE_BURN_FRAMES), "lit from frame 0");
    assert_eq!(tr.lit_by, 0);
    assert!(tr.is_burning(1));

    // Burn-out: at lit_until the tree falls and stops being lethal.
    app.world_mut().resource_mut::<FrameCount>().0 = TREE_BURN_FRAMES;
    app.world_mut().run_system_once(tree_fire).unwrap();
    let tr = tree(&mut app, t);
    assert!(tr.felled, "burned down");
    assert!(!tr.is_burning(TREE_BURN_FRAMES));
}

#[test]
fn fire_spreads_to_neighbors_after_the_delay_within_radius_only() {
    let mut app = bare_app();
    let src = tree_at(&mut app, 0, 0);
    let near = tree_at(&mut app, 120, 0); // inside the 150 cm spread radius
    let far = tree_at(&mut app, 400, 0); // outside
    // Light the source at frame 0.
    app.world_mut()
        .entity_mut(src)
        .get_mut::<BoneTree>()
        .unwrap()
        .lit_until_frame = Some(TREE_BURN_FRAMES);

    // Before the spread delay: nothing catches.
    app.world_mut().resource_mut::<FrameCount>().0 = TREE_SPREAD_DELAY_FRAMES - 1;
    app.world_mut().run_system_once(tree_fire).unwrap();
    assert!(tree(&mut app, near).lit_until_frame.is_none(), "too early");

    // At the delay: the neighbor catches, the far tree never does.
    app.world_mut().resource_mut::<FrameCount>().0 = TREE_SPREAD_DELAY_FRAMES;
    app.world_mut().run_system_once(tree_fire).unwrap();
    assert!(tree(&mut app, near).lit_until_frame.is_some(), "neighbor catches");
    assert!(tree(&mut app, far).lit_until_frame.is_none(), "out of reach");
}

#[test]
fn burning_tree_kills_and_credits_like_a_pyre() {
    let mut app = bare_app();
    *app.world_mut().resource_mut::<MatchState>() = MatchState::InRound {
        expires_at_frame: 1_000_000,
    };
    let t = tree_at(&mut app, 0, 0);
    {
        let mut tr = app.world_mut().entity_mut(t);
        let mut tr = tr.get_mut::<BoneTree>().unwrap();
        tr.lit_until_frame = Some(TREE_BURN_FRAMES);
        tr.lit_by = 0;
    }
    // The igniter's OPPONENT touches the burning tree: igniter is credited.
    let victim = app
        .world_mut()
        .spawn((
            Player { handle: 1 },
            PositionF(Vec2F::from_cm(0, 0)),
            PreviousPositionF(Vec2F::from_cm(0, 0)),
        ))
        .id();
    app.world_mut().run_system_once(tree_burn_kills).unwrap();
    assert!(
        app.world().entity(victim).get::<Dead>().unwrap().is_dying(),
        "the fire bites"
    );
    assert_eq!(app.world().resource::<MatchScore>().p0, 1, "igniter credited");

    // Self-burn: the igniter walking into their own fire credits the OPPONENT.
    let own_goal = app
        .world_mut()
        .spawn((
            Player { handle: 0 },
            PositionF(Vec2F::from_cm(0, 0)),
            PreviousPositionF(Vec2F::from_cm(0, 0)),
        ))
        .id();
    app.world_mut().run_system_once(tree_burn_kills).unwrap();
    assert!(app.world().entity(own_goal).get::<Dead>().unwrap().is_dying());
    assert_eq!(
        app.world().resource::<MatchScore>().p1,
        1,
        "your own fire is never a free out"
    );
}

#[test]
fn standing_tree_blocks_a_player_felled_does_not() {
    let mut app = bare_app();
    let t = tree_at(&mut app, 0, 0);
    let p = app
        .world_mut()
        .spawn((
            Player { handle: 0 },
            PositionF(Vec2F::from_cm(10, 0)), // overlapping the trunk
            PreviousPositionF(Vec2F::from_cm(10, 0)),
        ))
        .id();
    app.world_mut().run_system_once(tree_collision).unwrap();
    let pushed = app.world().entity(p).get::<PositionF>().unwrap().0;
    assert_ne!(pushed, Vec2F::from_cm(10, 0), "standing trunk pushes out");

    app.world_mut().entity_mut(t).get_mut::<BoneTree>().unwrap().felled = true;
    let back = Vec2F::from_cm(10, 0);
    app.world_mut().entity_mut(p).get_mut::<PositionF>().unwrap().0 = back;
    app.world_mut().run_system_once(tree_collision).unwrap();
    assert_eq!(
        app.world().entity(p).get::<PositionF>().unwrap().0,
        back,
        "a stump is walkable"
    );
}

#[test]
fn snapshot_round_trips_tree_state() {
    let mut app = bare_app();
    let t = tree_at(&mut app, -100, 50);
    let pristine = SimSnapshot::capture(app.world_mut());
    assert_eq!(pristine.trees.len(), 1, "snapshot carries the tree");
    assert!(!pristine.trees[0].tree.felled);

    // Chip it, light it, fell it — then scrub back.
    {
        let mut e = app.world_mut().entity_mut(t);
        let mut tr = e.get_mut::<BoneTree>().unwrap();
        tr.hp = 0;
        tr.felled = true;
        tr.lit_until_frame = Some(999);
        tr.lit_by = 1;
    }
    pristine.restore(app.world_mut());
    let tr = tree(&mut app, t);
    assert_eq!(tr.hp, TREE_HP, "restore stands the tree back up");
    assert!(!tr.felled && tr.lit_until_frame.is_none());
    let mut q = app.world_mut().query::<&BoneTree>();
    assert_eq!(q.iter(app.world()).count(), 1, "no tree churn on restore");
}
