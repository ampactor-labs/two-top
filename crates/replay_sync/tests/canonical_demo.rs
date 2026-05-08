//! The committed canonical demo at `tests/demos/canonical/match_v1.bmrg`
//! is what the cross-platform CI matrix runs through `replay_sync` to
//! verify byte-identical TSV output. If the generator and committed file
//! drift apart, a green CI run no longer proves what it claims to. This
//! test fails fast on drift and tells you to re-run with `--write`.

use replay::{decode, encode};
use replay_sync::{CANONICAL_FRAMES, canonical_path, canonical_replay, compute_checksum_tsv};

#[test]
fn committed_canonical_matches_generator() {
    let bytes = encode(&canonical_replay()).expect("encode canonical");
    let path = canonical_path();
    let committed = std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "missing canonical demo at {}: {e}. \
             Run: cargo run -p replay_sync --bin gen_canonical -- --write",
            path.display()
        )
    });
    assert_eq!(
        committed, bytes,
        "canonical demo at {} drifted from generator. \
         Re-run: cargo run -p replay_sync --bin gen_canonical -- --write",
        path.display()
    );
}

#[test]
fn canonical_replay_runs_through_checksum_pipeline() {
    // Sanity gate: load the committed file, decode, run it through the
    // checksum TSV, and assert the row count matches the header. This is
    // the same path CI takes — failing here means the matrix would also
    // fail.
    let bytes = std::fs::read(canonical_path()).expect("read canonical");
    let replay = decode(&bytes).expect("decode canonical");
    assert_eq!(replay.header.frame_count, CANONICAL_FRAMES);

    let tsv = compute_checksum_tsv(&replay);
    let lines: Vec<&str> = tsv.lines().collect();
    assert_eq!(lines.len() as u32, CANONICAL_FRAMES + 1);
}

/// Phase 10 cycle 6: the canonical demo's throw-cycle windows must
/// actually spawn boomerangs (otherwise the matrix gate would pass
/// trivially when no boomerang code ever runs). We assert the
/// `boomerang_part` column changes value across the demo — a sufficient
/// signal that boomerangs entered + exited the world during the run.
#[test]
fn canonical_demo_exercises_boomerang_state() {
    let bytes = std::fs::read(canonical_path()).expect("read canonical");
    let replay = decode(&bytes).expect("decode canonical");
    let tsv = compute_checksum_tsv(&replay);

    // Column index 6 (zero-based): boomerang_part. Collect the unique
    // values across every frame.
    let mut uniq: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for line in tsv.lines().skip(1) {
        let cols: Vec<&str> = line.split('\t').collect();
        uniq.insert(cols[6]);
    }
    assert!(
        uniq.len() >= 2,
        "canonical demo's boomerang_part should change as boomerangs spawn/move/despawn; \
         saw only {} unique values: {uniq:?}",
        uniq.len(),
    );
}
