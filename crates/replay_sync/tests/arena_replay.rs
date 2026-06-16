//! Phase 16 cycle 5: the replay header's `arena_id` must drive the
//! reproduced simulation. If playback ignored it, a Crossing match would
//! replay (and cross-platform-verify) as Anchor — a silent desync between
//! recording and playback. These tests pin that the arena is wired through.

use replay::{FrameInputs, Replay};
use replay_sync::compute_checksum_tsv;
use replay_sync::fuzz::fuzz_replay;
use sim::PlayerInput;

/// Player 0 walks toward arena centre (+x from its -100 spawn); player 1
/// holds still. On Crossing this marches P0 into the central chasm and kills
/// it; on Anchor P0 just walks to centre (players don't collide with pyres).
fn walk_into_centre(frames: u32) -> Vec<FrameInputs> {
    let p0 = PlayerInput {
        stick_x: 110,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
    let p1 = PlayerInput::default();
    (0..frames).map(|_| [p0, p1]).collect()
}

fn replay_on(arena_id: u8, inputs: Vec<FrameInputs>) -> Replay {
    // Borrow a valid header from the fuzzer (seed 0 → Anchor) and swap in
    // our inputs + target arena.
    let mut r = fuzz_replay(0);
    r.header.arena_id = arena_id;
    r.header.frame_count = inputs.len() as u32;
    r.inputs = inputs;
    r
}

#[test]
fn arena_id_changes_the_reproduced_simulation() {
    let frames = 600;
    let anchor = compute_checksum_tsv(&replay_on(0, walk_into_centre(frames)));
    let crossing = compute_checksum_tsv(&replay_on(1, walk_into_centre(frames)));
    assert_ne!(
        anchor, crossing,
        "identical inputs on different arenas must diverge — arena_id is wired into playback"
    );
}

#[test]
fn same_arena_replays_identically() {
    let frames = 600;
    let a = compute_checksum_tsv(&replay_on(1, walk_into_centre(frames)));
    let b = compute_checksum_tsv(&replay_on(1, walk_into_centre(frames)));
    assert_eq!(a, b, "same arena + same inputs is deterministic");
}

#[test]
fn every_arena_replays_without_panic() {
    // Each arena's geometry must survive a full fuzzed run (seeds chosen so
    // seed % 3 hits each arena: 0=Anchor, 1=Crossing, 2=Reliquary).
    for seed in [0u64, 1, 2] {
        let tsv = compute_checksum_tsv(&fuzz_replay(seed));
        assert!(!tsv.is_empty(), "seed {seed} produced a checksum stream");
    }
}
