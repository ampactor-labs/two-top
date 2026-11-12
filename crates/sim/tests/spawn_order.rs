//! Same-frame spawns must not depend on query order.
//!
//! bevy_ggrs mixes each rollback entity's registration index into the
//! frame checksum, and registration follows spawn order. A system that
//! spawns while iterating a query spawns in table order — and a rollback
//! restore can reorder a table on one peer and not the other. Two fangs
//! thrown on the same frame then registered in opposite orders on the two
//! phones: identical states, different checksums, and the match ended as
//! a desync (found by the two-peer smoke: both bots threw on frame 203,
//! DESYNC at 210, every run that hit it).
//!
//! The test spawns the players in the same order (so they register the
//! same way) and then moves player 0 out of its table and back, which puts
//! it last in every query — what a restore can do — and asserts the full
//! ggrs checksum, registration order included, does not move.

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy_ggrs::prelude::*;
use bevy_ggrs::{Checksum, GgrsPlugin, Session};
use fixed_math::Vec2F;
use sim::{
    DefaultInputsPlugin, GgrsCfg, Player, PlayerInput, PositionF, PreviousPositionF, SimPlugin,
    SynthesizedInputs, VelocityF,
};

/// A component that exists only to move an entity between tables.
#[derive(Component)]
struct Shuffle;

fn build(reorder_table: bool) -> App {
    let mut sb = SessionBuilder::<GgrsCfg>::new()
        .with_num_players(2)
        .unwrap()
        .with_check_distance(2)
        .with_input_delay(0);
    for handle in 0..2 {
        sb = sb.add_player(PlayerType::Local, handle).unwrap();
    }
    let session = sb.start_synctest_session().unwrap();
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(TimeUpdateStrategy::ManualDuration(sim::tick_duration()));
    app.add_plugins(GgrsPlugin::<GgrsCfg>::default());
    app.add_plugins(SimPlugin);
    app.add_plugins(sim::InfiniteRoundPlugin);
    app.add_plugins(DefaultInputsPlugin);
    app.insert_resource(Session::SyncTest(session));
    let mut first = None;
    for handle in [0usize, 1] {
        let at = Vec2F::from_cm(0, if handle == 0 { -300 } else { 300 });
        let id = app
            .world_mut()
            .spawn((
                Player { handle },
                PositionF(at),
                PreviousPositionF(at),
                VelocityF(Vec2F::ZERO),
            ))
            .id();
        first.get_or_insert(id);
    }
    if reorder_table && let Some(p0) = first {
        // Out of the table and back in: same entity, same registration,
        // now behind player 1 in every query.
        app.world_mut().entity_mut(p0).insert(Shuffle);
        app.world_mut().entity_mut(p0).remove::<Shuffle>();
        let mut q = app.world_mut().query::<&Player>();
        let order: Vec<usize> = q.iter(app.world()).map(|p| p.handle).collect();
        assert_eq!(order, [1, 0], "the reorder took");
    }
    app
}

/// Both players wind up together and release together (the synthesized
/// input drives every handle), so their fangs spawn on the same frame.
fn throw_together(app: &mut App) -> Vec<u128> {
    let mut checksums = Vec::new();
    let mut step = |app: &mut App, input: PlayerInput| {
        app.world_mut().resource_mut::<SynthesizedInputs>().0 = input;
        app.update();
        checksums.push(app.world().resource::<Checksum>().0);
    };
    step(app, PlayerInput::default());
    for _ in 0..12 {
        step(
            app,
            PlayerInput {
                buttons: PlayerInput::THROW_DOWN,
                ..PlayerInput::default()
            },
        );
    }
    for _ in 0..12 {
        step(
            app,
            PlayerInput {
                stick_x: 100,
                ..PlayerInput::default()
            },
        );
    }
    checksums
}

#[test]
fn fangs_thrown_on_one_frame_register_in_the_same_order_whatever_the_table_order() {
    let mut forward = build(false);
    let mut reversed = build(true);
    let a = throw_together(&mut forward);
    let b = throw_together(&mut reversed);
    let fangs = |app: &mut App| {
        let mut q = app.world_mut().query::<&sim::Boomerang>();
        q.iter(app.world()).count()
    };
    assert_eq!(fangs(&mut forward), 2, "both players threw");
    assert_eq!(fangs(&mut reversed), 2, "both players threw");
    // Same inputs, same state: the checksum may not depend on which player
    // the query met first.
    for (frame, (x, y)) in a.iter().zip(&b).enumerate() {
        assert_eq!(x, y, "checksums part at step {frame}");
    }
}
