use replay::{decode, encode, Replay, ReplayError, ReplayHeader, DEV_SIM_VERSION, FORMAT_VERSION, MAGIC};
use sim::PlayerInput;

fn sample_replay() -> Replay {
    Replay {
        header: ReplayHeader {
            magic: MAGIC,
            format_version: FORMAT_VERSION,
            sim_version: DEV_SIM_VERSION,
            seed: 0xDEAD_BEEF_DEAD_BEEF,
            num_players: 2,
            frame_rate: 60,
            frame_count: 3,
            recorded_at: 1_715_000_000,
            winner: Some(1),
            player_handles: [
                Some("alice".to_string()),
                Some("bob".to_string()),
            ],
            arena_id: 0,
        },
        inputs: vec![
            [
                PlayerInput { stick_x: 100, stick_y: 0, aim_angle: 0, buttons: 0 },
                PlayerInput { stick_x: -100, stick_y: 0, aim_angle: 128, buttons: PlayerInput::THROW_DOWN },
            ],
            [
                PlayerInput { stick_x: 0, stick_y: 50, aim_angle: 64, buttons: PlayerInput::DASH_DOWN },
                PlayerInput { stick_x: 0, stick_y: -50, aim_angle: 192, buttons: 0 },
            ],
            [
                PlayerInput::default(),
                PlayerInput::default(),
            ],
        ],
    }
}

#[test]
fn roundtrip_preserves_replay() {
    let original = sample_replay();
    let bytes = encode(&original).expect("encode succeeds");
    let recovered = decode(&bytes).expect("decode succeeds");
    assert_eq!(recovered, original);
}

#[test]
fn decode_rejects_bad_magic() {
    let mut replay = sample_replay();
    replay.header.magic = *b"XXXX";
    let bytes = encode(&replay).unwrap();
    match decode(&bytes) {
        Err(ReplayError::InvalidMagic(got)) => assert_eq!(got, *b"XXXX"),
        other => panic!("expected InvalidMagic, got {other:?}"),
    }
}

#[test]
fn decode_rejects_unsupported_format_version() {
    let mut replay = sample_replay();
    replay.header.format_version = 0xFFFF;
    let bytes = encode(&replay).unwrap();
    match decode(&bytes) {
        Err(ReplayError::UnsupportedFormatVersion(v)) => assert_eq!(v, 0xFFFF),
        other => panic!("expected UnsupportedFormatVersion, got {other:?}"),
    }
}

#[test]
fn decode_rejects_truncated_bytes() {
    let replay = sample_replay();
    let bytes = encode(&replay).unwrap();
    let truncated = &bytes[..bytes.len() / 2];
    assert!(decode(truncated).is_err(), "truncated input should fail decode");
}

#[test]
fn empty_inputs_roundtrip() {
    let mut replay = sample_replay();
    replay.inputs.clear();
    replay.header.frame_count = 0;
    let bytes = encode(&replay).unwrap();
    let recovered = decode(&bytes).unwrap();
    assert_eq!(recovered, replay);
}

#[test]
fn magic_constant_is_bmrg() {
    assert_eq!(&MAGIC, b"BMRG");
}
