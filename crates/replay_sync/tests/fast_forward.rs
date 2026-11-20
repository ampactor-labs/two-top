//! `sim::fast_forward` re-simulates a tape exactly as a session does:
//! the canonical match, row for row, against the session-driven run.

#[test]
fn fast_forward_matches_the_session_run_frame_for_frame() {
    let replay = replay_sync::canonical_replay();
    let live = replay_sync::compute_live_checksum_tsv(&replay);
    let fast = replay_sync::compute_fast_forward_checksum_tsv(&replay);
    assert_eq!(live.lines().count(), fast.lines().count());
    for (i, (a, b)) in live.lines().zip(fast.lines()).enumerate() {
        assert_eq!(a, b, "first difference at TSV line {i}");
    }
}
