//! Phase 5: per-frame, per-component checksum TSV is the cross-platform
//! determinism gate. Two invariants exercised here:
//!   1. Output is deterministic across runs of the same replay (RED gate).
//!   2. Format is exactly `frame\ttotal\tpositionf_part\tvelocityf_part\n`,
//!      one header row + one row per simulated frame.
//!
//! Invariant #7 in CONVENTIONS: bevy_ggrs::checksum_hasher only — never
//! std::hash::DefaultHasher (random, non-portable).

use replay::{
    DEV_SIM_VERSION, FORMAT_VERSION, FrameInputs, MAGIC, Replay, ReplayHeader,
};
use replay_sync::{compute_checksum_tsv, dump_state_at};
use sim::PlayerInput;

fn tiny_replay(frames: u32) -> Replay {
    let mut inputs: Vec<FrameInputs> = Vec::with_capacity(frames as usize);
    for f in 0..frames {
        let dir = if (f / 5) % 2 == 0 { 80i8 } else { -80i8 };
        let p = PlayerInput {
            stick_x: dir,
            stick_y: 0,
            aim_angle: 0,
            buttons: 0,
        };
        inputs.push([p, p]);
    }
    Replay {
        header: ReplayHeader {
            magic: MAGIC,
            format_version: FORMAT_VERSION,
            sim_version: DEV_SIM_VERSION,
            seed: 0,
            num_players: 2,
            frame_rate: 60,
            frame_count: frames,
            recorded_at: 0,
            winner: None,
            player_handles: [None, None],
            arena_id: 0,
        },
        inputs,
    }
}

#[test]
fn tsv_header_and_row_count() {
    let replay = tiny_replay(30);
    let tsv = compute_checksum_tsv(&replay);
    let lines: Vec<&str> = tsv.lines().collect();

    assert_eq!(
        lines[0],
        "frame\ttotal_checksum\tpositionf_part\tvelocityf_part\tdashstate_part\tstunframes_part",
        "header mismatch"
    );
    assert_eq!(
        lines.len() as u32,
        replay.header.frame_count + 1,
        "expected header + one row per frame, got {} lines",
        lines.len()
    );
    for (i, line) in lines.iter().skip(1).enumerate() {
        let cols: Vec<&str> = line.split('\t').collect();
        assert_eq!(cols.len(), 6, "row {i} has {} columns: {line:?}", cols.len());
        assert_eq!(cols[0].parse::<u32>().unwrap(), i as u32, "frame col");
    }
}

#[test]
fn tsv_is_deterministic_across_runs() {
    let replay = tiny_replay(60);
    let a = compute_checksum_tsv(&replay);
    let b = compute_checksum_tsv(&replay);
    assert_eq!(a, b, "two runs of the same replay produced different TSVs");
}

#[test]
fn dump_state_at_format_and_determinism() {
    let replay = tiny_replay(20);
    let dump = dump_state_at(&replay, 15);

    // Header line carries the frame, then one body line per entity.
    let mut lines = dump.lines();
    let header = lines.next().expect("header");
    assert_eq!(header, "# replay_sync state dump @ frame 15");

    let body: Vec<&str> = lines.collect();
    assert_eq!(body.len(), 2, "expected 2 entity lines, got {body:?}");

    for (i, line) in body.iter().enumerate() {
        assert!(
            line.starts_with(&format!("handle={i}")),
            "row {i}: {line:?}"
        );
        assert!(line.contains("pos="), "missing pos= column: {line:?}");
        assert!(line.contains("vel="), "missing vel= column: {line:?}");
    }

    let again = dump_state_at(&replay, 15);
    assert_eq!(dump, again, "dump_state_at not deterministic");
}

#[test]
fn tsv_changes_when_input_changes() {
    let mut a_replay = tiny_replay(30);
    let b_replay = tiny_replay(30);
    // Mutate one frame's input on the A side — checksums must diverge from
    // that frame onward.
    a_replay.inputs[10][0].stick_x = -80;
    let a = compute_checksum_tsv(&a_replay);
    let b = compute_checksum_tsv(&b_replay);
    assert_ne!(a, b, "checksum failed to detect input divergence");
}
