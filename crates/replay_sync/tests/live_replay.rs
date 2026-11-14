//! A recorded match must re-simulate to its own result.
//!
//! `replay_sync --attest` re-simulates a tape to check a signed result's
//! claim. It used to rebuild every tape with the determinism probe's setup
//! (both duelists at the origin, rounds that never end, input delay 2): a
//! different match from the one played, so no genuine signed result could
//! verify. The bot bench plays real matches under the live rules, from the
//! one `sim::spawn_match_world` the game lays its table with; recorded as
//! tapes, they must come back to the score they were played to.

use bot_brain::bench::{Seat, play_recorded};
use replay::{DEV_SIM_VERSION, FORMAT_VERSION, MAGIC, Replay, ReplayHeader};
use replay_sync::{compute_live_checksum_tsv, final_score};
use sim::ArenaId;

fn tape(arena: ArenaId, inputs: Vec<[sim::PlayerInput; 2]>) -> Replay {
    Replay {
        header: ReplayHeader {
            magic: MAGIC,
            format_version: FORMAT_VERSION,
            sim_version: DEV_SIM_VERSION,
            seed: 0,
            num_players: 2,
            frame_rate: 60,
            frame_count: inputs.len() as u32,
            recorded_at: 0,
            winner: None,
            player_handles: [None, None],
            arena_id: arena.as_u8(),
        },
        inputs,
    }
}

#[test]
fn a_recorded_match_re_simulates_to_its_own_score() {
    // Arenas with cover, pyres and trees, so every kind of entity the
    // table carries is in play.
    for (arena, seed) in [
        (ArenaId::Anchor, 11),
        (ArenaId::Gallery, 12),
        (ArenaId::Forest, 13),
    ] {
        let (report, inputs) = play_recorded(
            arena,
            [Seat::Bot { tier: 8 }, Seat::Bot { tier: 3 }],
            seed,
            300,
        );
        assert!(
            report.finished,
            "{arena:?}: the bench match must be decided"
        );
        let score = final_score(&tape(arena, inputs));
        assert_eq!(
            (score.p0, score.p1),
            report.score,
            "{arena:?}: the tape re-simulates to a different match than the one played",
        );
    }
}

#[test]
fn a_live_re_simulation_is_deterministic() {
    let (_, inputs) = play_recorded(
        ArenaId::Crossing,
        [Seat::Bot { tier: 6 }, Seat::Bot { tier: 4 }],
        21,
        60,
    );
    let tape = tape(ArenaId::Crossing, inputs);
    assert_eq!(
        compute_live_checksum_tsv(&tape),
        compute_live_checksum_tsv(&tape)
    );
}
