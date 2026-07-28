//! Phase 5/6 CLI. Two input sources:
//!   * `--demo <path.bmrg>` — load a saved replay from disk.
//!   * `--fuzz <seed>` — generate a deterministic replay from a u64 seed
//!     (Phase 6 fuzzer).
//!
//! Output modes (apply to both input sources):
//!   * `--output <path.tsv>` — write the per-frame checksum TSV.
//!   * `--dump-state-at <frame>` — print sim state at the given frame.
//!   * `--emit-bmrg <path>` — write the (loaded or generated) `.bmrg` to
//!     disk before running. Used by the fuzz_soak workflow to preserve a
//!     reproducible artifact for any seed that produces a divergence.
//!
//! With no output flag the TSV is streamed to stdout. The exit code is
//! `0` on success, non-zero on parse/load/encode/decode/sim error.

use replay::{decode_for_sim_version, encode};
use replay_sync::fuzz::{fuzz_replay, run_replay_caught};
use replay_sync::{compute_checksum_tsv, dump_state_at};
use std::fs;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();

    // Resolve the input replay from --demo or --fuzz. Mutually exclusive;
    // neither set is a usage error.
    let replay_result = match (
        arg_value::<String>(&args, "--demo"),
        arg_value::<u64>(&args, "--fuzz"),
    ) {
        (Some(_), Some(_)) => {
            eprintln!("replay_sync: --demo and --fuzz are mutually exclusive");
            return ExitCode::from(2);
        }
        (Some(path), None) => load_demo(&PathBuf::from(path)),
        (None, Some(seed)) => Ok(fuzz_replay(seed)),
        (None, None) => {
            eprintln!(
                "usage: replay_sync (--demo <path.bmrg> | --fuzz <seed>) \
                 [--output <path.tsv>] [--dump-state-at <frame>] \
                 [--emit-bmrg <path>]"
            );
            return ExitCode::from(2);
        }
    };

    let replay = match replay_result {
        Ok(r) => r,
        Err(code) => return code,
    };

    if let Some(out) = arg_value::<String>(&args, "--emit-bmrg") {
        let bytes = match encode(&replay) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("replay_sync: failed to encode bmrg: {e}");
                return ExitCode::FAILURE;
            }
        };
        if let Err(e) = fs::write(&out, bytes) {
            eprintln!("replay_sync: failed to write {out}: {e}");
            return ExitCode::FAILURE;
        }
    }

    if let Some(frame) = arg_value::<u32>(&args, "--dump-state-at") {
        let dump = dump_state_at(&replay, frame);
        print!("{dump}");
        return ExitCode::SUCCESS;
    }

    // NORTH N2: verify a dual-signed result against this tape. The tape
    // re-simulates (that's the whole point), the statement's facts must
    // match it, and both signatures must check out.
    if let Some(attest_path) = arg_value::<String>(&args, "--attest") {
        let attestation: net::Attestation = match fs::read_to_string(&attest_path)
            .map_err(|e| e.to_string())
            .and_then(|text| serde_json::from_str(&text).map_err(|e| e.to_string()))
        {
            Ok(a) => a,
            Err(e) => {
                eprintln!("replay_sync: failed to read attestation {attest_path}: {e}");
                return ExitCode::FAILURE;
            }
        };
        return match replay_sync::verify_attestation(&replay, &attestation) {
            Ok(()) => {
                let s = &attestation.statement;
                println!(
                    "ATTESTED: {:032x} {} - {} {:032x} on arena {} (sim v{}, match {})",
                    s.seat_low.install_id,
                    s.seat_low.score,
                    s.seat_high.score,
                    s.seat_high.install_id,
                    s.arena_id,
                    s.sim_version,
                    s.match_index,
                );
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("replay_sync: attestation REFUSED — {e}");
                ExitCode::FAILURE
            }
        };
    }

    // For --fuzz, catch SyncTest panics via run_replay_caught so the
    // workflow's bash loop sees a clean non-zero exit instead of a stack
    // trace. For --demo, run compute_checksum_tsv directly — a failing
    // canonical demo *should* panic loudly and immediately.
    let tsv = if arg_value::<u64>(&args, "--fuzz").is_some() {
        match run_replay_caught(&replay) {
            Ok(tsv) => tsv,
            Err(e) => {
                eprintln!("replay_sync: fuzz divergence — {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        compute_checksum_tsv(&replay)
    };

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

fn load_demo(path: &PathBuf) -> Result<replay::Replay, ExitCode> {
    let bytes = fs::read(path).map_err(|e| {
        eprintln!("replay_sync: failed to read {}: {}", path.display(), e);
        ExitCode::FAILURE
    })?;
    decode_for_sim_version(&bytes, sim::SIM_VERSION).map_err(|e| {
        eprintln!("replay_sync: failed to decode: {e}");
        ExitCode::FAILURE
    })
}

fn arg_value<T: std::str::FromStr>(args: &[String], flag: &str) -> Option<T> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
}
