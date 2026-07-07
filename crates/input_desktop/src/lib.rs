//! Desktop keyboard input source — maps the keyboard to wire-format
//! `PlayerInput` for PC play (local couch versus now; PC online once the
//! Matchbox driver lands).
//!
//! Mirrors `input_touch`'s boundary exactly: a pure mapping fn (Bevy-free,
//! unit-tested) plus a thin `ReadInputs` system that writes per-handle
//! `LocalInputs<GgrsCfg>`. Per CONVENTIONS the wire carries LEVEL signals
//! only (held bits) — the sim derives press/release edges by diffing
//! against `InputHistory`, so this never emits a "just pressed" bit.
//!
//! Couch layout — two players share one keyboard, no controllers needed:
//!   * **P0**: `WASD` move · `Space` throw · `LeftShift` dash · `T` taunt
//!   * **P1**: arrow keys move · `RightShift` throw · `RightCtrl` dash ·
//!     `Enter` taunt
//!
//! Or grab controllers: the first connected gamepad drives P0, the second
//! drives P1 (left stick moves; South/RightTrigger throw; East/LeftTrigger
//! dash; North taunt). A gamepad only takes over its handle while it's
//! active, so the keyboard stays a live fallback.
//!
//! In online play (a single local handle) that handle always uses the P0
//! (primary) scheme regardless of which network slot it occupies.

use core::f32::consts::PI;

use bevy::input::ButtonInput;
use bevy::input::gamepad::{Gamepad, GamepadButton};
use bevy::input::keyboard::KeyCode;
use bevy::prelude::*;
use bevy_ggrs::prelude::ReadInputs;
use bevy_ggrs::{LocalInputs, LocalPlayers};
use sim::{GgrsCfg, PlayerInput};

/// The seven keys that drive one player. All level signals.
#[derive(Clone, Copy, Debug)]
pub struct KeyBindings {
    pub up: KeyCode,
    pub down: KeyCode,
    pub left: KeyCode,
    pub right: KeyCode,
    pub throw: KeyCode,
    pub dash: KeyCode,
    pub taunt: KeyCode,
}

impl KeyBindings {
    /// Player 0 — left hand on WASD.
    pub const P0: KeyBindings = KeyBindings {
        up: KeyCode::KeyW,
        down: KeyCode::KeyS,
        left: KeyCode::KeyA,
        right: KeyCode::KeyD,
        throw: KeyCode::Space,
        dash: KeyCode::ShiftLeft,
        taunt: KeyCode::KeyT,
    };
    /// Player 1 — right hand on the arrow cluster.
    pub const P1: KeyBindings = KeyBindings {
        up: KeyCode::ArrowUp,
        down: KeyCode::ArrowDown,
        left: KeyCode::ArrowLeft,
        right: KeyCode::ArrowRight,
        throw: KeyCode::ShiftRight,
        dash: KeyCode::ControlRight,
        taunt: KeyCode::Enter,
    };
}

/// Pure: build the wire-format `PlayerInput` from seven held booleans.
///
/// Game-space convention — `+y` is up, `+x` is right — synthesized
/// directly (no y-flip, unlike the touch path which inverts Bevy's
/// screen-down y). A held diagonal is normalized to unit length so it
/// isn't √2-fast (the sim clamps too, but keeping the wire honest avoids
/// surprises). When throw is held over a live direction, `AIM_ACTIVE` is
/// set and `aim_angle` tracks the heading so the render reticle reads.
#[allow(clippy::fn_params_excessive_bools)] // the seven level signals, verbatim
pub fn input_from_dpad(
    up: bool,
    down: bool,
    left: bool,
    right: bool,
    throw: bool,
    dash: bool,
    taunt: bool,
) -> PlayerInput {
    let x = (right as i32 - left as i32) as f32;
    let y = (up as i32 - down as i32) as f32;
    let (sx, sy) = if x != 0.0 && y != 0.0 {
        let inv = 1.0 / 2.0_f32.sqrt();
        (x * inv, y * inv)
    } else {
        (x, y)
    };
    wire_input(sx, sy, throw, dash, taunt)
}

/// Pure core shared by the keyboard (d-pad) and gamepad (analog) paths:
/// quantize a game-space stick `(sx, sy)` in `[-1, 1]` plus the three held
/// buttons into the 4-byte wire format, setting `AIM_ACTIVE` + `aim_angle`
/// when throwing over a live direction.
fn wire_input(sx: f32, sy: f32, throw: bool, dash: bool, taunt: bool) -> PlayerInput {
    let stick_x = (sx * 127.0).round().clamp(-127.0, 127.0) as i8;
    let stick_y = (sy * 127.0).round().clamp(-127.0, 127.0) as i8;

    let mut buttons = 0u8;
    if throw {
        buttons |= PlayerInput::THROW_DOWN;
    }
    if dash {
        buttons |= PlayerInput::DASH_DOWN;
    }
    if taunt {
        buttons |= PlayerInput::TAUNT_DOWN;
    }
    let aim_active = throw && (stick_x != 0 || stick_y != 0);
    let aim_angle = if aim_active {
        buttons |= PlayerInput::AIM_ACTIVE;
        quantize_angle(sy.atan2(sx))
    } else {
        0
    };

    PlayerInput {
        stick_x,
        stick_y,
        aim_angle,
        buttons,
    }
}

/// Left-stick deadzone — a resting/drifting stick reads as neutral.
const GAMEPAD_DEADZONE: f32 = 0.18;

/// Pure: collapse a raw analog stick to a deadzoned game-space vector,
/// rescaling past the deadzone edge so motion ramps from zero (not from a
/// jump to 0.18). Inside the deadzone → exactly zero.
pub fn apply_deadzone(raw: Vec2, deadzone: f32) -> Vec2 {
    let mag = raw.length();
    if mag <= deadzone {
        return Vec2::ZERO;
    }
    let scaled = ((mag - deadzone) / (1.0 - deadzone)).min(1.0);
    (raw / mag) * scaled
}

/// Quantize a heading in radians `[-π, π]` to the wire's u8 angle, matching
/// `input_touch`'s convention so render interprets it identically.
fn quantize_angle(rad: f32) -> u8 {
    let normalized = (rad + PI) / (2.0 * PI);
    (normalized * 256.0).floor().clamp(0.0, 255.0) as u8
}

/// Read one binding set off the live keyboard.
fn read_bindings(keys: &ButtonInput<KeyCode>, b: KeyBindings) -> PlayerInput {
    input_from_dpad(
        keys.pressed(b.up),
        keys.pressed(b.down),
        keys.pressed(b.left),
        keys.pressed(b.right),
        keys.pressed(b.throw),
        keys.pressed(b.dash),
        keys.pressed(b.taunt),
    )
}

/// Read a connected gamepad into a `PlayerInput`, or `None` when it's idle
/// so the keyboard fallback for that handle keeps control. Left stick
/// moves; South or RightTrigger throws; East or LeftTrigger dashes;
/// North taunts (the flex button earns the top of the diamond).
fn read_gamepad(gp: &Gamepad) -> Option<PlayerInput> {
    let stick = apply_deadzone(gp.left_stick(), GAMEPAD_DEADZONE);
    let throw = gp.pressed(GamepadButton::South) || gp.pressed(GamepadButton::RightTrigger);
    let dash = gp.pressed(GamepadButton::East) || gp.pressed(GamepadButton::LeftTrigger);
    let taunt = gp.pressed(GamepadButton::North);
    let active = stick != Vec2::ZERO || throw || dash || taunt;
    active.then(|| wire_input(stick.x, stick.y, throw, dash, taunt))
}

/// `ReadInputs` system: map keyboard + gamepads to a per-handle
/// `PlayerInput`. Two local handles (couch versus) → scheme 0 (WASD / pad
/// 0) drives handle 0, scheme 1 (arrows / pad 1) drives handle 1. One
/// local handle (online) → that handle uses scheme 0. A gamepad assigned
/// to a scheme overrides the keyboard for it while the pad is active.
pub fn read_local_desktop_inputs(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    gamepads: Query<(Entity, &Gamepad)>,
    local_players: Res<LocalPlayers>,
) {
    let kb = [
        read_bindings(&keys, KeyBindings::P0),
        read_bindings(&keys, KeyBindings::P1),
    ];
    // Stable scheme→gamepad assignment by entity order: first pad → P0.
    let mut pads: Vec<(Entity, &Gamepad)> = gamepads.iter().collect();
    pads.sort_by_key(|(e, _)| *e);

    let solo = local_players.0.len() == 1;
    let mut map = bevy::platform::collections::HashMap::default();
    for &handle in &local_players.0 {
        let scheme = if solo { 0 } else { handle.min(1) };
        let input = pads
            .get(scheme)
            .and_then(|(_, gp)| read_gamepad(gp))
            .unwrap_or(kb[scheme]);
        map.insert(handle, input);
    }
    commands.insert_resource(LocalInputs::<GgrsCfg>(map));
}

/// Installs the desktop keyboard `ReadInputs` source. Add this OR a touch
/// source, never both — two sources would race over `LocalInputs<GgrsCfg>`.
pub struct DesktopInputsPlugin;

impl Plugin for DesktopInputsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(ReadInputs, read_local_desktop_inputs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_is_all_zero() {
        let p = input_from_dpad(false, false, false, false, false, false, false);
        assert_eq!(p.stick_x, 0);
        assert_eq!(p.stick_y, 0);
        assert_eq!(p.aim_angle, 0);
        assert_eq!(p.buttons, 0);
    }

    #[test]
    fn cardinal_directions_are_game_space() {
        // +y is up, +x is right.
        assert_eq!(
            input_from_dpad(true, false, false, false, false, false, false).stick_y,
            127
        );
        assert_eq!(
            input_from_dpad(false, true, false, false, false, false, false).stick_y,
            -127
        );
        assert_eq!(
            input_from_dpad(false, false, true, false, false, false, false).stick_x,
            -127
        );
        assert_eq!(
            input_from_dpad(false, false, false, true, false, false, false).stick_x,
            127
        );
    }

    #[test]
    fn opposite_keys_cancel() {
        let p = input_from_dpad(true, true, true, true, false, false, false);
        assert_eq!((p.stick_x, p.stick_y), (0, 0));
    }

    #[test]
    fn diagonal_is_normalized_not_root_two_fast() {
        // up+right → each axis ~127/√2 ≈ 90, never the full 127.
        let p = input_from_dpad(true, false, false, true, false, false, false);
        assert!(p.stick_x > 80 && p.stick_x < 100, "stick_x={}", p.stick_x);
        assert!(p.stick_y > 80 && p.stick_y < 100, "stick_y={}", p.stick_y);
    }

    #[test]
    fn throw_and_dash_set_level_bits() {
        let p = input_from_dpad(false, false, false, false, true, true, false);
        assert_ne!(p.buttons & PlayerInput::THROW_DOWN, 0);
        assert_ne!(p.buttons & PlayerInput::DASH_DOWN, 0);
    }

    #[test]
    fn taunt_sets_only_its_level_bit() {
        let p = input_from_dpad(false, false, false, false, false, false, true);
        assert_eq!(p.buttons, PlayerInput::TAUNT_DOWN);
    }

    #[test]
    fn deadzone_zeroes_small_input_and_passes_full_tilt() {
        // Inside the deadzone collapses to exactly zero.
        assert_eq!(apply_deadzone(Vec2::new(0.1, 0.0), 0.18), Vec2::ZERO);
        assert_eq!(apply_deadzone(Vec2::new(0.0, -0.15), 0.18), Vec2::ZERO);
        // Full tilt survives at ~unit length (rescaled from the edge).
        let full = apply_deadzone(Vec2::new(1.0, 0.0), 0.18);
        assert!((full.x - 1.0).abs() < 1e-6 && full.y.abs() < 1e-6);
        // Just past the deadzone ramps up from ~0, not a jump to 0.18.
        let near = apply_deadzone(Vec2::new(0.2, 0.0), 0.18);
        assert!(near.x > 0.0 && near.x < 0.1, "ramps from zero: {}", near.x);
    }

    #[test]
    fn analog_stick_quantizes_through_wire_input() {
        // Half-tilt right → ~half of 127.
        let p = wire_input(0.5, 0.0, false, false, false);
        assert!((p.stick_x as i32 - 64).abs() <= 1, "stick_x={}", p.stick_x);
        assert_eq!(p.stick_y, 0);
        // Analog throw aimed up sets AIM_ACTIVE.
        let up = wire_input(0.0, 0.9, true, false, false);
        assert_ne!(up.buttons & PlayerInput::AIM_ACTIVE, 0);
        assert!(up.stick_y > 100);
    }

    #[test]
    fn aim_active_only_with_a_direction() {
        // Throw with no direction → no AIM_ACTIVE, no angle.
        let still = input_from_dpad(false, false, false, false, true, false, false);
        assert_eq!(still.buttons & PlayerInput::AIM_ACTIVE, 0);
        assert_eq!(still.aim_angle, 0);
        // Throw aimed right → AIM_ACTIVE set, angle is the "right" bucket.
        let aimed = input_from_dpad(false, false, false, true, true, false, false);
        assert_ne!(aimed.buttons & PlayerInput::AIM_ACTIVE, 0);
        assert_eq!(aimed.aim_angle, 128, "atan2(0,1)=0 → mid bucket");
    }
}
