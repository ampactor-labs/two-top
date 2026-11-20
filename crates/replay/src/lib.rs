use bevy::prelude::*;
use bevy_ggrs::prelude::*;
use bevy_ggrs::{LocalInputs, LocalPlayers};
use serde::{Deserialize, Serialize};
use sim::{GgrsCfg, PlayerInput};

pub const MAGIC: [u8; 4] = *b"BMRG";
/// 2: the header names which match of the session the tape shows
/// (`start_tick`), what kind of tape it is, and its kills.
pub const FORMAT_VERSION: u16 = 2;

/// What a tape is a recording of.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum TapeKind {
    /// Two people on one device (or a test).
    #[default]
    Couch,
    /// An online duel between two phones.
    Duel,
    /// Practice against the bot, at a gauntlet tier.
    Practice { tier: u8 },
    /// A spar against a rival's shade.
    Shade,
    /// The newcomer lesson.
    Lesson,
}

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
    /// The tick this tape's match starts on. A session that ran it back
    /// records every match from tick 0; the theater fast-forwards to here
    /// and plays this one.
    pub start_tick: u32,
    pub kind: TapeKind,
    /// Every confirmed kill in this match: (tick, victim seat). The scrub
    /// strip marks them; WATCH THE KILL jumps to the last.
    pub kills: Vec<(u32, u8)>,
}

#[derive(Debug)]
pub enum ReplayError {
    Postcard(postcard::Error),
    InvalidMagic([u8; 4]),
    UnsupportedFormatVersion(u16),
    SimVersionMismatch {
        expected: u32,
        got: u32,
    },
    /// The header's `frame_count` disagrees with the number of input
    /// frames actually in the tape. The count drives the theater's
    /// playhead and scrub clamp, so a tape that lies about it is either
    /// crafted or truncated — refused at the codec boundary.
    FrameCountMismatch {
        declared: u32,
        actual: usize,
    },
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
            ReplayError::FrameCountMismatch { declared, actual } => {
                write!(
                    f,
                    "frame_count mismatch: header declares {declared} frames, tape holds {actual}"
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
    if replay.header.frame_count as usize != replay.inputs.len() {
        return Err(ReplayError::FrameCountMismatch {
            declared: replay.header.frame_count,
            actual: replay.inputs.len(),
        });
    }
    Ok(replay)
}

/// The header alone, without decoding the inputs behind it: what a list of
/// tapes needs, at a fraction of the cost of [`decode`].
pub fn decode_header(bytes: &[u8]) -> Result<ReplayHeader, ReplayError> {
    let (header, _): (ReplayHeader, _) = postcard::take_from_bytes(bytes)?;
    if header.magic != MAGIC {
        return Err(ReplayError::InvalidMagic(header.magic));
    }
    if header.format_version != FORMAT_VERSION {
        return Err(ReplayError::UnsupportedFormatVersion(header.format_version));
    }
    Ok(header)
}

/// Decode and additionally enforce strict `sim_version` match against the
/// running binary's version. Per ARCHITECTURE.md § Replay Format and
/// CONVENTIONS § Replay and Logging: replays only load if `sim_version`
/// matches binary. Old replays are viewed via archived git-tagged binaries —
/// no migration code lives in the codec.
///
/// Both sides normally pass `sim::SIM_VERSION`.
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

// ---- Seal ----
//
// A decided online match's dual-signed result rides at the tape's end, so
// a tape is one file to keep, prune, upload and verify. The footer is
// `SEAL` then the attestation as a postcard string (its JSON, written and
// read by the app and replay_sync: the codec only carries it). Bytes after
// a tape that are not a seal are ignored, as they always were.

pub const SEAL_MAGIC: [u8; 4] = *b"SEAL";

/// The tape's encoded length, and so where a seal would start.
fn body_len(bytes: &[u8]) -> Result<usize, ReplayError> {
    let (_, rest): (Replay, _) = postcard::take_from_bytes(bytes)?;
    Ok(bytes.len() - rest.len())
}

/// `bytes` (a tape, sealed or not) with `attestation` as its seal.
pub fn seal(bytes: &[u8], attestation: &str) -> Result<Vec<u8>, ReplayError> {
    let mut out = bytes[..body_len(bytes)?].to_vec();
    out.extend_from_slice(&SEAL_MAGIC);
    out.extend(postcard::to_allocvec(attestation)?);
    Ok(out)
}

/// The tape's seal, if it carries one.
pub fn read_seal(bytes: &[u8]) -> Option<String> {
    let rest = bytes.get(body_len(bytes).ok()?..)?;
    let body = rest.strip_prefix(&SEAL_MAGIC)?;
    postcard::from_bytes(body).ok()
}

/// What a signed statement binds: SHA-256 over the match's inputs, eight
/// bytes a tick (the live stream's encoding). A seal moved onto another
/// tape, or a tape whose inputs were edited, no longer matches it.
pub fn body_hash(inputs: &[FrameInputs]) -> [u8; 32] {
    use sha2::Digest as _;
    sha2::Sha256::digest(encode_live_pairs(inputs)).into()
}

// ---- Live stream ----
//
// A tape being played, streamed as it is played: the postcard header (its
// `frame_count` 0, its winner and kills unknown yet), then each tick's
// input pair as eight raw bytes, appended forever. Nothing in it is
// length-prefixed, so any prefix of the stream is a valid stream, and a
// reader can start from any byte offset it has already consumed.

/// Bytes per tick in a live stream: two four-byte `PlayerInput`s.
pub const LIVE_PAIR_BYTES: usize = 8;

/// The stream's opening: the header alone.
pub fn encode_live_head(header: &ReplayHeader) -> Result<Vec<u8>, ReplayError> {
    Ok(postcard::to_allocvec(header)?)
}

/// Ticks appended to a live stream.
pub fn encode_live_pairs(pairs: &[FrameInputs]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pairs.len() * LIVE_PAIR_BYTES);
    for pair in pairs {
        for p in pair {
            out.extend_from_slice(&[p.stick_x as u8, p.stick_y as u8, p.aim_angle, p.buttons]);
        }
    }
    out
}

/// Whole ticks in `bytes` (a trailing partial tick is left for the next
/// read). Returns the pairs and how many bytes they used.
pub fn decode_live_pairs(bytes: &[u8]) -> (Vec<FrameInputs>, usize) {
    let pairs: Vec<FrameInputs> = bytes
        .chunks_exact(LIVE_PAIR_BYTES)
        .map(|c| {
            let one = |o: usize| PlayerInput {
                stick_x: c[o] as i8,
                stick_y: c[o + 1] as i8,
                aim_angle: c[o + 2],
                buttons: c[o + 3],
            };
            [one(0), one(4)]
        })
        .collect();
    let used = pairs.len() * LIVE_PAIR_BYTES;
    (pairs, used)
}

/// The header at the front of a live stream and how many bytes it took,
/// or `None` while not all of it has arrived. A header that has arrived
/// and is wrong (another game, another sim) is an error.
pub fn decode_live_head(bytes: &[u8]) -> Result<Option<(ReplayHeader, usize)>, ReplayError> {
    match postcard::take_from_bytes::<ReplayHeader>(bytes) {
        Ok((header, rest)) => {
            if header.magic != MAGIC {
                return Err(ReplayError::InvalidMagic(header.magic));
            }
            if header.format_version != FORMAT_VERSION {
                return Err(ReplayError::UnsupportedFormatVersion(header.format_version));
            }
            Ok(Some((header, bytes.len() - rest.len())))
        }
        Err(postcard::Error::DeserializeUnexpectedEnd) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// A whole live stream as a tape: the header, with its count set to the
/// ticks that have arrived.
pub fn decode_live(bytes: &[u8]) -> Result<Option<Replay>, ReplayError> {
    let Some((mut header, used)) = decode_live_head(bytes)? else {
        return Ok(None);
    };
    let (inputs, _) = decode_live_pairs(&bytes[used..]);
    header.frame_count = inputs.len() as u32;
    Ok(Some(Replay { header, inputs }))
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

#[cfg(test)]
mod live_tests {
    use super::*;

    fn header() -> ReplayHeader {
        ReplayHeader {
            magic: MAGIC,
            format_version: FORMAT_VERSION,
            sim_version: 1,
            seed: 9,
            num_players: 2,
            frame_rate: 60,
            frame_count: 0,
            recorded_at: 0,
            winner: None,
            player_handles: [Some("CUR".into()), None],
            arena_id: 2,
            start_tick: 0,
            kind: TapeKind::Duel,
            kills: Vec::new(),
        }
    }

    #[test]
    fn a_live_stream_reads_back_at_every_prefix() {
        let pairs: Vec<FrameInputs> = (0..5)
            .map(|i| {
                let p = PlayerInput {
                    stick_x: -(i as i8),
                    stick_y: i as i8,
                    aim_angle: 200,
                    buttons: i as u8,
                };
                [p, PlayerInput::default()]
            })
            .collect();
        let mut stream = encode_live_head(&header()).unwrap();
        let head_len = stream.len();
        stream.extend(encode_live_pairs(&pairs));
        for cut in 0..stream.len() {
            let got = decode_live(&stream[..cut]).unwrap();
            if cut < head_len {
                assert!(got.is_none(), "cut {cut}");
            } else {
                let tape = got.unwrap();
                let whole = (cut - head_len) / LIVE_PAIR_BYTES;
                assert_eq!(tape.inputs, pairs[..whole], "cut {cut}");
                assert_eq!(tape.header.frame_count as usize, whole);
            }
        }
        let tape = decode_live(&stream).unwrap().unwrap();
        // A whole stream is a tape the codec takes.
        assert_eq!(decode(&encode(&tape).unwrap()).unwrap(), tape);
    }

    #[test]
    fn a_seal_rides_at_the_end_and_replaces_an_old_one() {
        let tape = Replay {
            header: header(),
            inputs: Vec::new(),
        };
        let bytes = encode(&tape).unwrap();
        assert_eq!(read_seal(&bytes), None);
        let sealed = seal(&bytes, "{\"a\":1}").unwrap();
        assert_eq!(
            decode(&sealed).unwrap(),
            tape,
            "a sealed tape is still the tape"
        );
        assert_eq!(read_seal(&sealed).as_deref(), Some("{\"a\":1}"));
        let resealed = seal(&sealed, "{}").unwrap();
        assert_eq!(read_seal(&resealed).as_deref(), Some("{}"));
        assert_eq!(resealed.len(), bytes.len() + 4 + 1 + 2);
        assert_ne!(body_hash(&[]), body_hash(&[[PlayerInput::default(); 2]]));
    }

    #[test]
    fn a_stream_from_elsewhere_is_refused() {
        assert!(decode_live(b"GIF89a-not-a-tape-at-all").is_err());
    }
}
