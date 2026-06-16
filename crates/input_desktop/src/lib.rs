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
//!   * **P0**: `WASD` move · `Space` throw · `LeftShift` dash
//!   * **P1**: arrow keys move · `RightShift` throw · `RightCtrl` dash
//!
//! In online play (a single local handle) that handle always uses the P0
//! (primary) scheme regardless of which network slot it occupies.
//!
//! Gamepad support is a planned follow-up (plan task P.1) — it needs the
//! `bevy_gilrs` feature, which isn't in the app's bevy build yet.

use core::f32::consts::PI;

use bevy::input::ButtonInput;
use bevy::input::keyboard::KeyCode;
use bevy::prelude::*;
use bevy_ggrs::prelude::ReadInputs;
use bevy_ggrs::{LocalInputs, LocalPlayers};
use sim::{GgrsCfg, PlayerInput};

/// The six keys that drive one player. All level signals.
#[derive(Clone, Copy, Debug)]
pub struct KeyBindings {
    pub up: KeyCode,
    pub down: KeyCode,
    pub left: KeyCode,
    pub right: KeyCode,
    pub throw: KeyCode,
    pub dash: KeyCode,
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
    };
    /// Player 1 — right hand on the arrow cluster.
    pub const P1: KeyBindings = KeyBindings {
        up: KeyCode::ArrowUp,
        down: KeyCode::ArrowDown,
        left: KeyCode::ArrowLeft,
        right: KeyCode::ArrowRight,
        throw: KeyCode::ShiftRight,
        dash: KeyCode::ControlRight,
    };
}

/// Pure: build the wire-format `PlayerInput` from six held booleans.
///
/// Game-space convention — `+y` is up, `+x` is right — synthesized
/// directly (no y-flip, unlike the touch path which inverts Bevy's
/// screen-down y). A held diagonal is normalized to unit length so it
/// isn't √2-fast (the sim clamps too, but keeping the wire honest avoids
/// surprises). When throw is held over a live direction, `AIM_ACTIVE` is
/// set and `aim_angle` tracks the heading so the render reticle reads.
pub fn input_from_dpad(
    up: bool,
    down: bool,
    left: bool,
    right: bool,
    throw: bool,
    dash: bool,
) -> PlayerInput {
    let x = (right as i32 - left as i32) as f32;
    let y = (up as i32 - down as i32) as f32;
    let (sx, sy) = if x != 0.0 && y != 0.0 {
        let inv = 1.0 / 2.0_f32.sqrt();
        (x * inv, y * inv)
    } else {
        (x, y)
    };
    let stick_x = (sx * 127.0).round().clamp(-127.0, 127.0) as i8;
    let stick_y = (sy * 127.0).round().clamp(-127.0, 127.0) as i8;

    let mut buttons = 0u8;
    if throw {
        buttons |= PlayerInput::THROW_DOWN;
    }
    if dash {
        buttons |= PlayerInput::DASH_DOWN;
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
    )
}

/// `ReadInputs` system: map the keyboard to a per-handle `PlayerInput`.
/// Two local handles (couch versus) → handle 0 uses P0 keys, handle 1 uses
/// P1 keys. One local handle (online) → that handle uses the P0 scheme.
pub fn read_local_desktop_inputs(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    local_players: Res<LocalPlayers>,
) {
    let p0 = read_bindings(&keys, KeyBindings::P0);
    let p1 = read_bindings(&keys, KeyBindings::P1);
    let solo = local_players.0.len() == 1;
    let mut map = bevy::platform::collections::HashMap::default();
    for &handle in &local_players.0 {
        let input = if solo || handle == 0 { p0 } else { p1 };
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
        let p = input_from_dpad(false, false, false, false, false, false);
        assert_eq!(p.stick_x, 0);
        assert_eq!(p.stick_y, 0);
        assert_eq!(p.aim_angle, 0);
        assert_eq!(p.buttons, 0);
    }

    #[test]
    fn cardinal_directions_are_game_space() {
        // +y is up, +x is right.
        assert_eq!(input_from_dpad(true, false, false, false, false, false).stick_y, 127);
        assert_eq!(input_from_dpad(false, true, false, false, false, false).stick_y, -127);
        assert_eq!(input_from_dpad(false, false, true, false, false, false).stick_x, -127);
        assert_eq!(input_from_dpad(false, false, false, true, false, false).stick_x, 127);
    }

    #[test]
    fn opposite_keys_cancel() {
        let p = input_from_dpad(true, true, true, true, false, false);
        assert_eq!((p.stick_x, p.stick_y), (0, 0));
    }

    #[test]
    fn diagonal_is_normalized_not_root_two_fast() {
        // up+right → each axis ~127/√2 ≈ 90, never the full 127.
        let p = input_from_dpad(true, false, false, true, false, false);
        assert!(p.stick_x > 80 && p.stick_x < 100, "stick_x={}", p.stick_x);
        assert!(p.stick_y > 80 && p.stick_y < 100, "stick_y={}", p.stick_y);
    }

    #[test]
    fn throw_and_dash_set_level_bits() {
        let p = input_from_dpad(false, false, false, false, true, true);
        assert_ne!(p.buttons & PlayerInput::THROW_DOWN, 0);
        assert_ne!(p.buttons & PlayerInput::DASH_DOWN, 0);
    }

    #[test]
    fn aim_active_only_with_a_direction() {
        // Throw with no direction → no AIM_ACTIVE, no angle.
        let still = input_from_dpad(false, false, false, false, true, false);
        assert_eq!(still.buttons & PlayerInput::AIM_ACTIVE, 0);
        assert_eq!(still.aim_angle, 0);
        // Throw aimed right → AIM_ACTIVE set, angle is the "right" bucket.
        let aimed = input_from_dpad(false, false, false, true, true, false);
        assert_ne!(aimed.buttons & PlayerInput::AIM_ACTIVE, 0);
        assert_eq!(aimed.aim_angle, 128, "atan2(0,1)=0 → mid bucket");
    }
}
