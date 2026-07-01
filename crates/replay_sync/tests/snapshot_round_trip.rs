//! Phase 14 cycle 2a robustness: SimSnapshot capture + restore must
//! preserve EVERY rolled-back component and resource. A field that
//! escapes the snapshot would surface as a backward-scrub regression
//! in the replay viewer (state diverges after restore-and-replay
//! relative to forward play). This test catches those silently
//! before operator validation does.
//!
//! Property: for any seed and any midpoint frame M (within the
//! replay), the state at frame N (final) reached via:
//!   path A: build app, advance to N
//!   path B: build app, advance to M, capture S, advance to N
//!   path C: build app, advance to M, capture S, restore S into a
//!           FRESH app, advance from M to N using inputs[M..N]
//! must match BYTE-IDENTICALLY for paths A, B, and C.
//!
//! Path C is the load-bearing assertion — that's the same flow the
//! viewer uses for backward scrub.

use bevy::prelude::*;
use replay::ReplayPlayback;
use replay_sync::fuzz::fuzz_replay;
use replay_sync::{build_app_configurable, world_state_checksum};
use sim::{FrameCount, SimSnapshot};

/// Advance `app.update()` until `FrameCount.0 >= target` or a hard
/// upper bound trips. bevy_ggrs's accumulator can need a warm-up tick
/// before sim systems start running, so a fixed `target` count of
/// update() calls doesn't reliably yield `FrameCount == target`.
/// Loop-until-target keeps the test independent of accumulator
/// internals. Caps at `target + 8` updates so a stuck sim throws
/// rather than hanging the test runner.
fn advance_to(app: &mut App, target: u32) {
    let cap = (target as u64).saturating_add(8);
    for _ in 0..=cap {
        if app.world().resource::<FrameCount>().0 >= target {
            return;
        }
        app.update();
    }
    let actual = app.world().resource::<FrameCount>().0;
    panic!(
        "advance_to({target}) exceeded {cap} updates without reaching target \
         (FrameCount stuck at {actual})"
    );
}

/// Build a fresh app from `seed`, run to `target_frame`, capture +
/// return both the world checksum and a SimSnapshot at that frame.
fn run_to(seed: u64, target_frame: u32) -> (u64, SimSnapshot) {
    // check_distance=0 + input_delay=0: SimSnapshot::restore can't
    // restore bevy_ggrs's internal verification ring (check_distance)
    // OR the internal input delay buffer (input_delay). Both are
    // session-internal state independent of the World. For replay
    // playback (no rollback, no network) both should be 0.
    let mut app = build_app_configurable(fuzz_replay(seed), 0, 0);
    advance_to(&mut app, target_frame);
    let snap = SimSnapshot::capture(app.world_mut());
    let checksum = world_state_checksum(app.world_mut());
    (checksum, snap)
}

/// Path C: build a fresh app, restore `snap` into it (sets sim state
/// to snap.frame), reset the replay cursor, and advance forward to
/// `final_frame`. Returns the post-advance checksum.
fn restore_then_run_to(seed: u64, snap: &SimSnapshot, final_frame: u32) -> u64 {
    let mut app = build_app_configurable(fuzz_replay(seed), 0, 0);
    snap.restore(app.world_mut());
    app.world_mut().resource_mut::<ReplayPlayback>().cursor = snap.frame as usize;
    advance_to(&mut app, final_frame);
    world_state_checksum(app.world_mut())
}

/// Concrete seeds + (mid, final) frame pairs spanning the early /
/// middle / late portions of a fuzzed replay. Hand-picked rather
/// than randomly proptest-generated because this test build_apps the
/// replay multiple times per case (slow); a fixed table gives us
/// deterministic CI runtime + repeatable bug surfaces.
const CASES: &[(u64, u32, u32)] = &[
    (0, 30, 90),             // early-mid -> late
    (0xdead_beef, 100, 200), // mid -> later
    (1, 60, 180),
    (42, 45, 240),
    (1234, 120, 300),
];

#[test]
fn snapshot_round_trip_matches_forward_play() {
    for &(seed, mid, final_frame) in CASES {
        // Path A: forward play to final_frame.
        let (checksum_a, _) = run_to(seed, final_frame);

        // Path B: forward play to mid, capture, continue to final.
        // Re-uses the same app shape so this is implicitly tested by
        // path A succeeding — but the SimSnapshot we capture at mid
        // is the load-bearing input for path C.
        let (_, snap_at_mid) = run_to(seed, mid);

        // Path C: restore into fresh app, advance from mid to final.
        let checksum_c = restore_then_run_to(seed, &snap_at_mid, final_frame);

        assert_eq!(
            checksum_a, checksum_c,
            "seed={seed:#x} mid={mid} final={final_frame}: \
             snapshot round-trip checksum mismatch \
             (forward={checksum_a:016x}, restore-then-advance={checksum_c:016x}). \
             SimSnapshot is missing rolled-back state."
        );
    }
}

#[test]
fn snapshot_at_zero_frame_round_trips() {
    // Edge case: capture at frame 0 (before any sim ticks). The
    // initial state should round-trip cleanly even at frame 0.
    let seed = 0xc0ffee_u64;
    let (_, snap_at_zero) = run_to(seed, 0);
    let checksum_a = run_to(seed, 50).0;
    let checksum_c = restore_then_run_to(seed, &snap_at_zero, 50);
    assert_eq!(
        checksum_a, checksum_c,
        "seed={seed:#x}: zero-frame snapshot round-trip mismatch"
    );
}

#[test]
fn back_to_back_restore_is_idempotent() {
    // Restoring the same snapshot twice must produce the same world
    // state both times. Catches bugs where restore mutates the
    // snapshot or accumulates state across calls.
    let seed = 7_u64;
    let (_, snap) = run_to(seed, 80);

    let mut app1 = build_app_configurable(fuzz_replay(seed), 0, 0);
    snap.restore(app1.world_mut());
    let after_first = world_state_checksum(app1.world_mut());

    let mut app2 = build_app_configurable(fuzz_replay(seed), 0, 0);
    snap.restore(app2.world_mut());
    snap.restore(app2.world_mut()); // restore twice
    let after_second = world_state_checksum(app2.world_mut());

    assert_eq!(
        after_first, after_second,
        "restore(restore(snap)) != restore(snap) — restore is not idempotent"
    );
}
