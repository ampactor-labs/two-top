//! NORTH N2 — `verify_attestation`: a claim passes only when the
//! statement's facts match what the tape actually re-simulates to AND both
//! signatures verify. The keys here are fixed test seeds; the honest
//! statement is built FROM the re-simulated score, which is exactly how an
//! honest client builds one.
//!
//! The decided-match cases ride a real match: the bot bench plays one
//! headless under the live rules and records the inputs every tick
//! consumed, exactly what the game's own tape holds. (They used to ride the
//! canonical choreography looped under the determinism probe's setup —
//! duelists at the origin, rounds that never end — which no live match is,
//! so a genuine signed result could never verify. `live_replay.rs` pins
//! that.)

use bot_brain::bench::{Seat, play_recorded};
use net::{Attestation, MatchStatement, SeatStatement, pubkey_for, sig_to_hex, sign_statement};
use replay::{DEV_SIM_VERSION, FORMAT_VERSION, MAGIC, Replay, ReplayHeader};
use replay_sync::{final_score, verify_attestation};
use sim::{ArenaId, MATCH_WIN_THRESHOLD, PlayerInput};
use std::sync::OnceLock;

const KEY_A: &[u8; 32] = &[31u8; 32];
const KEY_B: &[u8; 32] = &[32u8; 32];

fn tape(arena: ArenaId, inputs: Vec<[PlayerInput; 2]>) -> Replay {
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

/// A real match, played to its decision and recorded tick by tick.
fn played() -> &'static (bot_brain::bench::MatchReport, Vec<[PlayerInput; 2]>) {
    static PLAYED: OnceLock<(bot_brain::bench::MatchReport, Vec<[PlayerInput; 2]>)> =
        OnceLock::new();
    PLAYED.get_or_init(|| {
        let played = play_recorded(
            ArenaId::Anchor,
            [Seat::Bot { tier: 8 }, Seat::Bot { tier: 3 }],
            7,
            300,
        );
        assert!(played.0.finished, "the bench match must be decided");
        played
    })
}

/// The decided match's tape, with a second of neutral inputs after the
/// deciding tick (the summary a phone keeps recording through).
fn decided_replay() -> &'static Replay {
    static DECIDED: OnceLock<Replay> = OnceLock::new();
    DECIDED.get_or_init(|| {
        let mut inputs = played().1.clone();
        inputs.extend(std::iter::repeat_n([PlayerInput::default(); 2], 60));
        tape(ArenaId::Anchor, inputs)
    })
}

fn honest_attestation(replay: &Replay) -> Attestation {
    let score = final_score(replay);
    let statement = MatchStatement::new(
        replay.header.sim_version,
        replay.header.arena_id,
        (0xC0FFEE, 0xF00D),
        0,
        [
            SeatStatement {
                install_id: 0xaaa,
                pubkey: pubkey_for(KEY_A),
                handle: 0,
                score: score.p0,
            },
            SeatStatement {
                install_id: 0xbbb,
                pubkey: pubkey_for(KEY_B),
                handle: 1,
                score: score.p1,
            },
        ],
    );
    Attestation {
        statement,
        sig_low: sig_to_hex(&sign_statement(&statement, KEY_A)),
        sig_high: sig_to_hex(&sign_statement(&statement, KEY_B)),
    }
}

#[test]
fn an_honest_attestation_verifies_against_its_tape() {
    let replay = decided_replay();
    verify_attestation(replay, &honest_attestation(replay)).expect("honest claim re-simulates");
}

#[test]
fn a_lying_score_is_refused_even_with_valid_signatures() {
    let replay = decided_replay();
    // Both parties conspire to sign a false score: the signatures verify,
    // the tape does not.
    let mut statement = honest_attestation(replay).statement;
    if statement.seat_low.score > 0 {
        statement.seat_low.score -= 1;
    } else {
        statement.seat_high.score -= 1;
    }
    let conspiracy = Attestation {
        statement,
        sig_low: sig_to_hex(&sign_statement(&statement, KEY_A)),
        sig_high: sig_to_hex(&sign_statement(&statement, KEY_B)),
    };
    let err = verify_attestation(replay, &conspiracy).unwrap_err();
    assert!(
        err.contains("re-simulates"),
        "refused for the right reason: {err}"
    );
}

#[test]
fn a_tampered_signature_is_refused() {
    let replay = decided_replay();
    let mut attestation = honest_attestation(replay);
    attestation.sig_high = attestation.sig_low.clone();
    let err = verify_attestation(replay, &attestation).unwrap_err();
    assert!(err.contains("signature"), "{err}");
}

#[test]
fn a_wrong_arena_claim_is_refused_before_resimulation() {
    let replay = decided_replay();
    let mut attestation = honest_attestation(replay);
    attestation.statement.arena_id = attestation.statement.arena_id.wrapping_add(1);
    let err = verify_attestation(replay, &attestation).unwrap_err();
    assert!(err.contains("arena"), "{err}");
}

/// The app only signs threshold-decided matches; a verifier must refuse an
/// attestation over a tape that never got there, even if every signature
/// and score checks out — otherwise a client could mint "results" out of
/// practice flailing. The first half of a real match is exactly that tape.
#[test]
fn an_undecided_tape_cannot_carry_a_result() {
    let (report, inputs) = played();
    let half = tape(ArenaId::Anchor, inputs[..inputs.len() / 2].to_vec());
    let score = final_score(&half);
    assert!(
        score.p0 < MATCH_WIN_THRESHOLD && score.p1 < MATCH_WIN_THRESHOLD,
        "precondition: half of a {:?} match stays undecided ({} - {})",
        report.score,
        score.p0,
        score.p1,
    );
    let err = verify_attestation(&half, &honest_attestation(&half)).unwrap_err();
    assert!(err.contains("threshold"), "{err}");
}

/// The honest claim really is the played result, not just whatever the
/// verifier re-simulates.
#[test]
fn the_verified_score_is_the_one_played() {
    let (report, _) = played();
    let score = final_score(decided_replay());
    assert_eq!((score.p0, score.p1), report.score);
}
