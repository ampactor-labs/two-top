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
use replay::{FORMAT_VERSION, MAGIC, Replay, ReplayHeader};
use replay_sync::{compute_live_checksum_tsv, final_score};
use sim::ArenaId;

fn tape(arena: ArenaId, inputs: Vec<[sim::PlayerInput; 2]>) -> Replay {
    Replay {
        header: ReplayHeader {
            magic: MAGIC,
            format_version: FORMAT_VERSION,
            sim_version: sim::SIM_VERSION,
            seed: 0,
            num_players: 2,
            frame_rate: 60,
            frame_count: inputs.len() as u32,
            recorded_at: 0,
            winner: None,
            player_handles: [None, None],
            arena_id: arena.as_u8(),
            start_tick: 0,
            kind: replay::TapeKind::default(),
            kills: Vec::new(),
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

/// The rematch bug, seen from a tape: the loser's THROW presses in flight at
/// the decision (input delay + prediction window deep) used to restart the
/// match inside the tape, so it re-simulated to 0-0 and the signed result
/// beside it could never verify. Inside the sim's grace they are nothing.
#[test]
fn presses_in_flight_after_the_decision_leave_the_result_standing() {
    let (report, mut inputs) = play_recorded(
        ArenaId::Anchor,
        [Seat::Bot { tier: 8 }, Seat::Bot { tier: 3 }],
        7,
        300,
    );
    assert!(report.finished, "the bench match must be decided");
    // The longest a press can trail the decision online (input delay 4 +
    // prediction window 16), with THROW toggling on every tick of it.
    for tick in 0..20u32 {
        let mut late = [sim::PlayerInput::default(); 2];
        for input in &mut late {
            if tick % 2 == 1 {
                input.buttons = sim::PlayerInput::THROW_DOWN;
            }
        }
        inputs.push(late);
    }
    let score = final_score(&tape(ArenaId::Anchor, inputs));
    assert_eq!((score.p0, score.p1), report.score);
}
