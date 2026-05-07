//! Generates `tests/demos/canonical/match_v1.bmrg`. Default mode is a
//! dry-run that diffs the generator output against the committed file —
//! pass `--write` to overwrite. The companion test
//! `committed_canonical_matches_generator` runs the dry-run automatically
//! in CI and fails if the generator drifts from the committed snapshot.

use replay::encode;
use replay_sync::{CANONICAL_FRAMES, canonical_path, canonical_replay};
use std::process::ExitCode;

fn main() -> ExitCode {
    let write = std::env::args().any(|a| a == "--write");
    let bytes = encode(&canonical_replay()).expect("encode canonical");
    let path = canonical_path();

    if write {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create dir");
        }
        std::fs::write(&path, &bytes).expect("write canonical");
        eprintln!(
            "gen_canonical: wrote {} bytes ({} frames) to {}",
            bytes.len(),
            CANONICAL_FRAMES,
            path.display()
        );
        return ExitCode::SUCCESS;
    }

    match std::fs::read(&path) {
        Ok(committed) if committed == bytes => {
            println!("gen_canonical: {} matches generator", path.display());
            ExitCode::SUCCESS
        }
        Ok(_) => {
            eprintln!(
                "gen_canonical: {} differs from generator. Re-run with --write to update.",
                path.display()
            );
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!(
                "gen_canonical: failed to read {}: {}. Run with --write to create it.",
                path.display(),
                e
            );
            ExitCode::FAILURE
        }
    }
}
