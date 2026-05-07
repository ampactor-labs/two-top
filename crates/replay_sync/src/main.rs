//! Phase 5 CLI. Loads a `.bmrg` replay and either:
//!   * `--output <path.tsv>` — writes per-frame, per-component checksum TSV
//!     for the cross-platform diff job to consume, or
//!   * `--dump-state-at <frame>` — pretty-prints the full sim state at the
//!     requested frame, used by `scripts/diagnose_desync.sh`.
//!
//! With no `--output` flag the TSV is streamed to stdout.

use replay::decode_for_sim_version;
use replay_sync::{compute_checksum_tsv, dump_state_at};
use std::fs;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let Some(demo_path) = arg_value::<String>(&args, "--demo") else {
        eprintln!(
            "usage: replay_sync --demo <path.bmrg> [--output <path.tsv> | --dump-state-at <frame>]"
        );
        return ExitCode::from(2);
    };
    let demo_path = PathBuf::from(demo_path);

    let bytes = match fs::read(&demo_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("replay_sync: failed to read {}: {}", demo_path.display(), e);
            return ExitCode::FAILURE;
        }
    };

    let replay = match decode_for_sim_version(&bytes, sim::SIM_VERSION) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("replay_sync: failed to decode: {e}");
            return ExitCode::FAILURE;
        }
    };

    if let Some(frame) = arg_value::<u32>(&args, "--dump-state-at") {
        let dump = dump_state_at(&replay, frame);
        print!("{dump}");
        return ExitCode::SUCCESS;
    }

    let tsv = compute_checksum_tsv(&replay);

    if let Some(out_path) = arg_value::<String>(&args, "--output") {
        match fs::File::create(&out_path).and_then(|mut f| f.write_all(tsv.as_bytes())) {
            Ok(()) => {
                eprintln!(
                    "replay_sync: wrote {} frames to {}",
                    replay.header.frame_count, out_path
                );
            }
            Err(e) => {
                eprintln!("replay_sync: failed to write {out_path}: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        print!("{tsv}");
    }
    ExitCode::SUCCESS
}

fn arg_value<T: std::str::FromStr>(args: &[String], flag: &str) -> Option<T> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
}
