//! `sim::reset_match_world` leaves nothing of the last match behind: a
//! table played on and then reset re-runs the canonical match exactly like
//! a freshly built one.

use bevy::prelude::*;

/// Per tick: the replay checksum, plus what it does not cover and a leaky
/// reset would carry: the rolled-back RNG, the input history, the pickups.
fn run(app: &mut App, inputs: &[[sim::PlayerInput; 2]]) -> Vec<(u64, String)> {
    let mut rows = Vec::new();
    sim::fast_forward(app.world_mut(), inputs, |w| {
        let pickups = w
            .query::<&sim::Pickup>()
            .iter(w)
            .map(|p| format!("{p:?}"))
            .collect::<Vec<_>>();
        let extra = format!(
            "{:?} {:?} {pickups:?}",
            w.resource::<sim::SimRng>(),
            w.resource::<sim::InputHistory>(),
        );
        rows.push((replay_sync::world_state_checksum(w), extra));
    });
    rows
}

fn entities(app: &mut App) -> usize {
    app.world_mut()
        .query_filtered::<Entity, With<bevy_ggrs::Rollback>>()
        .iter(app.world())
        .count()
}

#[test]
fn a_reset_table_replays_like_a_fresh_one() {
    let replay = replay_sync::canonical_replay();
    let arena = sim::ArenaId::from_u8(replay.header.arena_id);
    let n = replay.inputs.len().min(replay.header.frame_count as usize);

    let mut fresh = replay_sync::fast_forward_world(&replay);
    let fresh_entities = entities(&mut fresh);
    let expected = run(&mut fresh, &replay.inputs[..n]);

    // Play most of the match (kills, pickups, fangs in flight), reset.
    let mut reused = replay_sync::fast_forward_world(&replay);
    run(&mut reused, &replay.inputs[..n * 2 / 3]);
    sim::reset_match_world(reused.world_mut(), arena);
    // Restart the frame clock as a new session would.
    reused
        .world_mut()
        .insert_resource(bevy_ggrs::RollbackFrameCount(0));
    assert_eq!(
        entities(&mut reused),
        fresh_entities,
        "the old table left entities"
    );
    let got = run(&mut reused, &replay.inputs[..n]);
    for (tick, (a, b)) in expected.iter().zip(&got).enumerate() {
        assert_eq!(a, b, "first difference at tick {tick}");
    }
}
