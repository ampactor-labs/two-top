//! Phase 6 fuzzer. Given a `u64` seed, deterministically generates a
//! `Replay` whose inputs span the wire format's value space and whose
//! buttons toggle frequently enough to exercise sim transitions. Each seed
//! is a complete reproduction recipe — the seed alone is enough to
//! regenerate the `.bmrg` and re-run divergence diagnosis.
//!
//! `fuzz_one(seed)` is the workflow-side entry point: it generates the
//! replay, runs it through `compute_checksum_tsv` inside `catch_unwind` so
//! a SyncTest panic surfaces as `FuzzError::Panic` instead of taking the
//! whole process down. The CLI exits non-zero on `Err` so the bash loop in
//! `fuzz_soak.yml` can decide which seeds to upload.

use crate::compute_checksum_tsv;
use rand_xoshiro::Xoshiro256StarStar;
use rand_xoshiro::rand_core::{Rng, SeedableRng};
use replay::{
    DEV_SIM_VERSION, FORMAT_VERSION, FrameInputs, MAGIC, Replay, ReplayHeader,
};
use sim::PlayerInput;

/// Frames per fuzz run. 30 s @ 60 Hz, matching the canonical demo and the
/// Phase 5 cross-platform check.
pub const FUZZ_FRAMES: u32 = 1800;

/// Wire-format mask for valid `buttons` bits. Bits 4-7 are reserved per
/// ARCHITECTURE.md § Input Model and must never be set on the wire — even
/// random fuzzer-generated input keeps this contract.
pub const BUTTONS_VALID_MASK: u8 = 0x0F;

#[derive(Debug)]
pub enum FuzzError {
    Panic(String),
}

impl core::fmt::Display for FuzzError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FuzzError::Panic(msg) => write!(f, "sim panicked during fuzz run: {msg}"),
        }
    }
}

impl std::error::Error for FuzzError {}

/// Generate a `PlayerInput` from the next 4 bytes of `rng`.
fn random_input(rng: &mut impl Rng) -> PlayerInput {
    let mut buf = [0u8; 4];
    rng.fill_bytes(&mut buf);
    let stick_x = (buf[0] as i8).max(-127);
    let stick_y = (buf[1] as i8).max(-127);
    let aim_angle = buf[2];
    let buttons = buf[3] & BUTTONS_VALID_MASK;
    PlayerInput {
        stick_x,
        stick_y,
        aim_angle,
        buttons,
    }
}

/// Build a `Replay` deterministically from `seed`. Same seed always
/// produces identical bytes — that's what makes the seed a complete
/// reproduction recipe.
pub fn fuzz_replay(seed: u64) -> Replay {
    let mut rng = Xoshiro256StarStar::seed_from_u64(seed);
    let inputs: Vec<FrameInputs> = (0..FUZZ_FRAMES)
        .map(|_| {
            let p0 = random_input(&mut rng);
            let p1 = random_input(&mut rng);
            [p0, p1]
        })
        .collect();
    Replay {
        header: ReplayHeader {
            magic: MAGIC,
            format_version: FORMAT_VERSION,
            sim_version: DEV_SIM_VERSION,
            seed,
            num_players: 2,
            frame_rate: 60,
            frame_count: FUZZ_FRAMES,
            recorded_at: 0,
            winner: None,
            player_handles: [None, None],
            arena_id: 0,
        },
        inputs,
    }
}

/// Run a `Replay` through the checksum pipeline with panics caught.
/// Used by both the seeded fuzzer and the CLI's `--fuzz` path so SyncTest
/// panics (single-machine non-determinism) surface as `FuzzError::Panic`
/// instead of taking the process down. Other (non-sim) panics are caught
/// the same way; the workflow doesn't need to distinguish.
pub fn run_replay_caught(replay: &Replay) -> Result<String, FuzzError> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        compute_checksum_tsv(replay)
    }));
    match result {
        Ok(tsv) => Ok(tsv),
        Err(payload) => {
            let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic payload".to_string()
            };
            Err(FuzzError::Panic(msg))
        }
    }
}

/// Convenience: build the seeded replay and run it through the pipeline.
pub fn fuzz_one(seed: u64) -> Result<String, FuzzError> {
    run_replay_caught(&fuzz_replay(seed))
}
