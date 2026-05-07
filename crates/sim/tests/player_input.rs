use core::mem;
use sim::PlayerInput;

#[test]
fn player_input_is_exactly_four_bytes() {
    assert_eq!(mem::size_of::<PlayerInput>(), 4);
}

#[test]
fn player_input_align_is_one() {
    // Required for safe wire-protocol byte casting.
    assert_eq!(mem::align_of::<PlayerInput>(), 1);
}

#[test]
fn player_input_round_trips_through_bytes() {
    let original = PlayerInput {
        stick_x: 42,
        stick_y: -42,
        aim_angle: 200,
        buttons: PlayerInput::THROW_DOWN | PlayerInput::DASH_DOWN,
    };
    let bytes: &[u8] = bytemuck::bytes_of(&original);
    assert_eq!(bytes.len(), 4);
    let recovered: PlayerInput = bytemuck::pod_read_unaligned(bytes);
    assert_eq!(recovered, original);
}

#[test]
fn button_bits_are_unique() {
    let bits = [
        PlayerInput::THROW_DOWN,
        PlayerInput::AIM_ACTIVE,
        PlayerInput::DASH_DOWN,
        PlayerInput::TAUNT_DOWN,
    ];
    // Each is a single bit
    for b in bits {
        assert_eq!(b.count_ones(), 1, "{:#010b} is not single-bit", b);
    }
    // No bit overlaps
    let combined: u8 = bits.iter().copied().fold(0u8, |a, b| a | b);
    assert_eq!(combined.count_ones() as usize, bits.len(), "button bits overlap");
}

#[test]
fn button_bits_in_low_nibble_only() {
    // ARCHITECTURE.md: bits 4-7 reserved.
    let combined = PlayerInput::THROW_DOWN
        | PlayerInput::AIM_ACTIVE
        | PlayerInput::DASH_DOWN
        | PlayerInput::TAUNT_DOWN;
    assert_eq!(combined & 0xF0, 0, "named button bits leak into reserved nibble");
}

#[test]
fn default_is_neutral_input() {
    let p = PlayerInput::default();
    assert_eq!(p.stick_x, 0);
    assert_eq!(p.stick_y, 0);
    assert_eq!(p.aim_angle, 0);
    assert_eq!(p.buttons, 0);
}
