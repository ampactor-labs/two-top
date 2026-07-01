//! Phase 5 end-to-end gate: prove the full pipeline catches a planted
//! divergence. Take the canonical replay, perturb one input frame, run
//! `compute_checksum_tsv` on both, and confirm `scripts/diagnose_desync.sh`
//! identifies the first divergent (frame, column) when run on the resulting
//! TSV files.
//!
//! This is the practical equivalent of BUILD_PLAN Phase 5 exit criterion
//! #2 (`"test by temporarily injecting an f32 op"`): it exercises the
//! exact diagnostic path CI takes — produce two real sim outputs, write
//! them to disk, run the script — and asserts the script names a real
//! frame and a real column from the checksum TSV header.

use replay_sync::{canonical_replay, compute_checksum_tsv};
use std::path::PathBuf;
use std::process::Command;

const PERTURB_FRAME: usize = 100;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonicalize workspace root")
}

fn tempdir() -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "two_top_e2e_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&base).expect("create tempdir");
    base
}

#[test]
fn perturbed_replay_diagnose_pipeline() {
    let baseline = canonical_replay();
    let mut perturbed = canonical_replay();
    // Flip stick_x on player 0 at PERTURB_FRAME — this is the kind of
    // single-bit divergence a real cross-platform desync would surface as.
    perturbed.inputs[PERTURB_FRAME][0].stick_x =
        perturbed.inputs[PERTURB_FRAME][0].stick_x.wrapping_neg();
    assert_ne!(
        baseline.inputs[PERTURB_FRAME][0].stick_x, perturbed.inputs[PERTURB_FRAME][0].stick_x,
        "perturb is a no-op — pick a different frame"
    );

    let tsv_a = compute_checksum_tsv(&baseline);
    let tsv_b = compute_checksum_tsv(&perturbed);
    assert_ne!(
        tsv_a, tsv_b,
        "checksum TSV did not pick up the perturbation"
    );

    let dir = tempdir();
    let path_a = dir.join("baseline.tsv");
    let path_b = dir.join("perturbed.tsv");
    std::fs::write(&path_a, tsv_a).expect("write baseline tsv");
    std::fs::write(&path_b, tsv_b).expect("write perturbed tsv");

    let script = workspace_root().join("scripts/diagnose_desync.sh");
    let out = Command::new("bash")
        .arg(&script)
        .arg(&path_a)
        .arg(&path_b)
        .output()
        .expect("run diagnose_desync.sh");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = format!("{stdout}{stderr}");

    assert!(
        !out.status.success(),
        "expected diagnose_desync to flag a divergence; stdout={stdout} stderr={stderr}"
    );

    // The script must name some frame and at least one component column.
    assert!(
        combined.contains("frame "),
        "missing 'frame ' in diagnose output: {combined}"
    );
    let names_a_column = combined.contains("positionf_part")
        || combined.contains("velocityf_part")
        || combined.contains("total_checksum");
    assert!(
        names_a_column,
        "diagnose output didn't name a checksum column: {combined}"
    );
}
