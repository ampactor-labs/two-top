use bevy::prelude::*;
use bevy_ggrs::prelude::*;
use bevy_ggrs::{LocalInputs, LocalPlayers};
use serde::{Deserialize, Serialize};
use sim::{GgrsCfg, PlayerInput};

pub const MAGIC: [u8; 4] = *b"BMRG";
pub const FORMAT_VERSION: u16 = 1;
/// Sentinel sim_version for non-release builds. ARCHITECTURE.md: dev replays
/// surface as "🚧 dev replay" in the viewer; release replays carry a real
/// sim_version that must match the executing binary's version exactly.
pub const DEV_SIM_VERSION: u32 = u32::MAX;

/// A frame's worth of inputs for a 1v1 match.
pub type FrameInputs = [PlayerInput; 2];

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Replay {
    pub header: ReplayHeader,
    pub inputs: Vec<FrameInputs>,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ReplayHeader {
    pub magic: [u8; 4],
    pub format_version: u16,
    pub sim_version: u32,
    pub seed: u64,
    pub num_players: u8,
    pub frame_rate: u8,
    pub frame_count: u32,
    pub recorded_at: u64,
    pub winner: Option<u8>,
    pub player_handles: [Option<String>; 2],
    pub arena_id: u8,
}

#[derive(Debug)]
pub enum ReplayError {
    Postcard(postcard::Error),
    InvalidMagic([u8; 4]),
    UnsupportedFormatVersion(u16),
    SimVersionMismatch { expected: u32, got: u32 },
}

impl core::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ReplayError::Postcard(e) => write!(f, "postcard error: {e}"),
            ReplayError::InvalidMagic(got) => {
                write!(f, "invalid magic: expected {:?}, got {:?}", MAGIC, got)
            }
            ReplayError::UnsupportedFormatVersion(v) => {
                write!(f, "unsupported replay format version: {v}")
            }
            ReplayError::SimVersionMismatch { expected, got } => {
                write!(
                    f,
                    "sim_version mismatch: replay was recorded with {got}, this binary is {expected}"
                )
            }
        }
    }
}

impl std::error::Error for ReplayError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ReplayError::Postcard(e) => Some(e),
            _ => None,
        }
    }
}

impl From<postcard::Error> for ReplayError {
    fn from(e: postcard::Error) -> Self {
        ReplayError::Postcard(e)
    }
}

pub fn encode(replay: &Replay) -> Result<Vec<u8>, ReplayError> {
    Ok(postcard::to_allocvec(replay)?)
}

pub fn decode(bytes: &[u8]) -> Result<Replay, ReplayError> {
    let replay: Replay = postcard::from_bytes(bytes)?;
    if replay.header.magic != MAGIC {
        return Err(ReplayError::InvalidMagic(replay.header.magic));
    }
    if replay.header.format_version != FORMAT_VERSION {
        return Err(ReplayError::UnsupportedFormatVersion(
            replay.header.format_version,
        ));
    }
    Ok(replay)
}

/// Decode and additionally enforce strict `sim_version` match against the
/// running binary's version. Per ARCHITECTURE.md § Replay Format and
/// CONVENTIONS § Replay and Logging: replays only load if `sim_version`
/// matches binary. Old replays are viewed via archived git-tagged binaries —
/// no migration code lives in the codec.
///
/// Both sides will normally pass `sim::SIM_VERSION`. A binary built from
/// main carries `u32::MAX` (the dev sentinel); release tags carry a real
/// version. A dev replay loaded into a release binary, or vice versa, is
/// rejected here.
pub fn decode_for_sim_version(bytes: &[u8], expected: u32) -> Result<Replay, ReplayError> {
    let replay = decode(bytes)?;
    if replay.header.sim_version != expected {
        return Err(ReplayError::SimVersionMismatch {
            expected,
            got: replay.header.sim_version,
        });
    }
    Ok(replay)
}

// ---- Recording ----

/// Buffered recording target. The `record_inputs_system` appends one
/// `FrameInputs` per ReadInputs tick. After the run, build a `Replay` from
/// `inputs.clone()` plus a hand-filled header.
#[derive(Resource, Default, Debug)]
pub struct RecordedInputs {
    pub frames: Vec<FrameInputs>,
}

/// Captures the LocalInputs map into the buffered recording each tick.
/// Runs in `ReadInputs` after the input-source system has already populated
/// `LocalInputs<GgrsCfg>`.
pub fn record_inputs_system(
    local_inputs: Option<Res<LocalInputs<GgrsCfg>>>,
    local_players: Res<LocalPlayers>,
    mut recorded: ResMut<RecordedInputs>,
) {
    let Some(local_inputs) = local_inputs else {
        return;
    };
    let mut frame: FrameInputs = [PlayerInput::default(); 2];
    for &handle in &local_players.0 {
        if let Some(input) = local_inputs.0.get(&handle)
            && handle < frame.len()
        {
            frame[handle] = *input;
        }
    }
    recorded.frames.push(frame);
}

/// Plugin: enables recording. Add alongside the input-source plugin (e.g.
/// `sim::DefaultInputsPlugin`) so `LocalInputs` is already populated when
/// `record_inputs_system` runs.
pub struct RecordPlugin;

impl Plugin for RecordPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RecordedInputs>().add_systems(
            ReadInputs,
            record_inputs_system.after(sim::read_local_inputs),
        );
    }
}

// ---- Playback ----

/// A loaded `Replay` with a mutable read cursor. The playback system reads
/// `frames[cursor]` and increments. Past the end, it falls back to default
/// (neutral) input — the caller is expected to stop driving frames before
/// that point.
#[derive(Resource, Debug)]
pub struct ReplayPlayback {
    pub replay: Replay,
    pub cursor: usize,
}

impl ReplayPlayback {
    pub fn new(replay: Replay) -> Self {
        Self { replay, cursor: 0 }
    }
}

pub fn playback_inputs_system(
    mut commands: Commands,
    local_players: Res<LocalPlayers>,
    mut playback: ResMut<ReplayPlayback>,
) {
    let frame_inputs = playback
        .replay
        .inputs
        .get(playback.cursor)
        .copied()
        .unwrap_or([PlayerInput::default(); 2]);
    playback.cursor += 1;

    let mut map = bevy::platform::collections::HashMap::default();
    for &handle in &local_players.0 {
        if handle < frame_inputs.len() {
            map.insert(handle, frame_inputs[handle]);
        }
    }
    commands.insert_resource(LocalInputs::<GgrsCfg>(map));
}

/// Plugin: replaces the synthesized input source with a Replay-driven one.
/// **Do not** add `sim::DefaultInputsPlugin` alongside this — they will
/// fight over `LocalInputs<GgrsCfg>` and produce undefined behavior.
///
/// Caller must `insert_resource(ReplayPlayback::new(replay))` before
/// running any frames.
pub struct ReplayPlaybackPlugin;

impl Plugin for ReplayPlaybackPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(ReadInputs, playback_inputs_system);
    }
}
