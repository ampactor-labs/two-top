//! Integration test for `scripts/diagnose_desync.sh`. Plants a known
//! divergence between two TSVs and asserts the script identifies the first
//! divergent frame and column. Exit code conveys the result so CI can
//! gate on it directly.

use std::path::PathBuf;
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonicalize workspace root")
}

fn script_path() -> PathBuf {
    workspace_root().join("scripts/diagnose_desync.sh")
}

fn write_tsv(path: &std::path::Path, rows: &[(&str, &str, &str, &str, &str, &str)]) {
    let mut s = String::from(
        "frame\ttotal_checksum\tpositionf_part\tvelocityf_part\tdashstate_part\tstunframes_part\n",
    );
    for (frame, total, pos, vel, dash, stun) in rows {
        s.push_str(&format!("{frame}\t{total}\t{pos}\t{vel}\t{dash}\t{stun}\n"));
    }
    std::fs::write(path, s).expect("write tsv");
}

#[test]
fn identical_tsvs_succeed() {
    let dir = tempdir();
    let a = dir.join("a.tsv");
    let b = dir.join("b.tsv");
    let rows = &[
        ("0", "aa", "bb", "cc", "dd", "ee"),
        ("1", "ff", "11", "22", "33", "44"),
        ("2", "55", "66", "77", "88", "99"),
    ];
    write_tsv(&a, rows);
    write_tsv(&b, rows);

    let out = Command::new("bash")
        .arg(script_path())
        .arg(&a)
        .arg(&b)
        .output()
        .expect("run script");

    assert!(
        out.status.success(),
        "expected success on identical TSVs; stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

#[test]
fn first_divergence_is_reported() {
    let dir = tempdir();
    let a = dir.join("a.tsv");
    let b = dir.join("b.tsv");
    let rows_a = &[
        ("0", "aa", "bb", "cc", "dd", "ee"),
        ("1", "ff", "11", "22", "33", "44"),
        ("2", "55", "66", "77", "88", "99"),
    ];
    let rows_b = &[
        ("0", "aa", "bb", "cc", "dd", "ee"),
        ("1", "ff", "XX", "22", "33", "44"), // diverges at frame 1, positionf_part
        ("2", "55", "66", "77", "88", "99"),
    ];
    write_tsv(&a, rows_a);
    write_tsv(&b, rows_b);

    let out = Command::new("bash")
        .arg(script_path())
        .arg(&a)
        .arg(&b)
        .output()
        .expect("run script");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = format!("{stdout}{stderr}");

    assert!(
        !out.status.success(),
        "expected failure on divergence; stdout={stdout} stderr={stderr}"
    );
    assert!(
        combined.contains("frame 1"),
        "missing 'frame 1' in output: {combined}"
    );
    assert!(
        combined.contains("positionf_part"),
        "missing 'positionf_part' in output: {combined}"
    );
}

fn tempdir() -> PathBuf {
    // Per-test unique dir under the cargo target tmp.
    let base = std::env::temp_dir().join(format!(
        "two_top_diagnose_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&base).expect("create tempdir");
    base
}
