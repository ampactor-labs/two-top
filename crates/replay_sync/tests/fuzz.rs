//! Phase 6 fuzzer: seeded random input streams that exercise the sim
//! through `compute_checksum_tsv` (and therefore SyncTest's check_distance
//! gate). Tests here verify the *generator* contract — same seed → same
//! replay, different seeds → different replays, inputs respect the wire
//! format constraints. The actual divergence-catching property is what the
//! nightly workflow exercises end-to-end.

use replay_sync::fuzz::{fuzz_one, fuzz_replay, FUZZ_FRAMES};
use sim::PlayerInput;

#[test]
fn same_seed_produces_identical_replay() {
    let a = fuzz_replay(0x1234_5678_9abc_def0);
    let b = fuzz_replay(0x1234_5678_9abc_def0);
    assert_eq!(a, b);
}

#[test]
fn different_seeds_produce_different_replays() {
    let a = fuzz_replay(0);
    let b = fuzz_replay(1);
    assert_ne!(a, b);
}

#[test]
fn fuzz_replay_has_canonical_frame_count() {
    let r = fuzz_replay(0);
    assert_eq!(r.header.frame_count, FUZZ_FRAMES);
    assert_eq!(r.inputs.len() as u32, FUZZ_FRAMES);
}

#[test]
fn fuzz_replay_respects_wire_constraints() {
    // Seed chosen arbitrarily; any seed should satisfy the invariants.
    let r = fuzz_replay(42);
    for (i, [p0, p1]) in r.inputs.iter().enumerate() {
        for p in [p0, p1] {
            // i8 is naturally in -128..=127; we explicitly clamp to
            // -127..=127 per ARCHITECTURE.md § Input Model.
            assert!(p.stick_x >= -127, "frame {i} stick_x out of range: {}", p.stick_x);
            assert!(p.stick_y >= -127, "frame {i} stick_y out of range: {}", p.stick_y);
            // Bits 4-7 of `buttons` are reserved per ARCHITECTURE.md.
            assert_eq!(
                p.buttons & 0xF0,
                0,
                "frame {i} reserved button bits set: {:#010b}",
                p.buttons
            );
        }
    }
    // Sanity: at least *some* button bits should fire across the run, or
    // the fuzzer is degenerate.
    let any_button = r
        .inputs
        .iter()
        .any(|[p0, p1]| p0.buttons != 0 || p1.buttons != 0);
    assert!(any_button, "fuzzer never set any buttons across {FUZZ_FRAMES} frames");
}

#[test]
fn fuzz_one_known_good_seed_returns_ok() {
    // Most seeds should pass — this is the gate the nightly job exercises.
    // If this seed ever flakes, replace with another and document the
    // why in MORGAN_NOTES.md.
    let result = fuzz_one(0xC0DE_BEEF);
    assert!(result.is_ok(), "fuzz_one(0xC0DE_BEEF) failed: {:?}", result.err());
}

#[test]
fn player_input_alignment_is_preserved() {
    // Regression guard: PlayerInput layout should still be exactly 4 bytes.
    // The fuzzer doesn't probe this directly but a layout drift would
    // change checksum hashes silently.
    assert_eq!(core::mem::size_of::<PlayerInput>(), 4);
}
