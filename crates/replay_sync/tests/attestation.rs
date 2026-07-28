//! NORTH N2 — `verify_attestation`: a claim passes only when the
//! statement's facts match what the tape actually re-simulates to AND both
//! signatures verify. The keys here are fixed test seeds; the honest
//! statement is built FROM the re-simulated score, which is exactly how an
//! honest client builds one.
//!
//! The canonical demo ends 2-2 (it is a determinism probe, not a decided
//! match), so the decided-match cases ride the first fuzz seed whose
//! random flailing reaches the score threshold — deterministic per seed
//! and sim version, found by a bounded scan.

use net::{Attestation, MatchStatement, SeatStatement, pubkey_for, sig_to_hex, sign_statement};
use replay::{DEV_SIM_VERSION, FORMAT_VERSION, MAGIC, Replay, ReplayHeader};
use replay_sync::{build_app, canonical_replay, final_score, verify_attestation};
use sim::{MATCH_WIN_THRESHOLD, PlayerInput};
use std::sync::OnceLock;

const KEY_A: &[u8; 32] = &[31u8; 32];
const KEY_B: &[u8; 32] = &[32u8; 32];

/// The canonical demo's hand-authored choreography, looped. One pass lands
/// four kills in 1800 frames; every kill re-anchors both duelists to their
/// spawns through the round flow, so a repeated pass keeps re-firing the
/// same proven kill patterns from re-anchored state until the threshold
/// lands. (Blind input-mashing was tried first and managed 1-1 in 9000
/// frames — throws that connect need choreography, which the canonical
/// tape already is.)
fn looped_canonical_tape(loops: usize) -> Replay {
    let base = canonical_replay();
    let inputs: Vec<_> = std::iter::repeat_with(|| base.inputs.clone())
        .take(loops)
        .flatten()
        .collect();
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
            arena_id: base.header.arena_id,
        },
        inputs,
    }
}

/// A tape that re-simulates to a threshold-decided match, built the honest
/// way: probe-run the looped duel, find the tick the threshold is reached,
/// then truncate the tape just past it with a neutral tail. The tail is
/// load-bearing — `apply_rematch` restarts on a THROW edge from either
/// player during MatchOver, so a tape that kept pressing past the decision
/// would re-simulate straight into a fresh 0-0.
fn decided_replay() -> &'static Replay {
    static DECIDED: OnceLock<Replay> = OnceLock::new();
    DECIDED.get_or_init(|| {
        let probe = looped_canonical_tape(4);
        let mut app = build_app(probe.clone());
        let mut decided_at = None;
        let mut best = (0u8, 0u8);
        for f in 0..probe.header.frame_count {
            app.update();
            let score = *app.world().resource::<sim::MatchScore>();
            best = (best.0.max(score.p0), best.1.max(score.p1));
            if score.p0 >= MATCH_WIN_THRESHOLD || score.p1 >= MATCH_WIN_THRESHOLD {
                decided_at = Some(f);
                break;
            }
        }
        let decided_at = decided_at.unwrap_or_else(|| {
            panic!(
                "looped canonical duel never decided in {} frames (best score {best:?}) — raise the loop count",
                probe.header.frame_count,
            )
        });
        let mut inputs = probe.inputs[..=decided_at as usize].to_vec();
        inputs.extend(std::iter::repeat_n([PlayerInput::default(); 2], 60));
        let mut header = probe.header;
        header.frame_count = inputs.len() as u32;
        Replay { header, inputs }
    })
}

fn honest_attestation(replay: &replay::Replay) -> Attestation {
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
/// practice flailing. The canonical 2-2 demo is exactly that tape.
#[test]
fn an_undecided_tape_cannot_carry_a_result() {
    let replay = canonical_replay();
    let score = final_score(&replay);
    assert!(
        score.p0 < sim::MATCH_WIN_THRESHOLD && score.p1 < sim::MATCH_WIN_THRESHOLD,
        "precondition: the canonical demo stays undecided ({} - {})",
        score.p0,
        score.p1,
    );
    let err = verify_attestation(&replay, &honest_attestation(&replay)).unwrap_err();
    assert!(err.contains("threshold"), "{err}");
}
