//! The local input sources this file wires into `ReadInputs`: the touch
//! layer, the keyboard mirrored for the far seat, and the fullscreen key.

use super::*;

/// The touch source, on every platform: `TouchState` (fingers, and the
/// mouse as a finger) into `LocalInputs`, layered over the keyboard.
///
/// Overwrites ONLY while engaged ([`input_touch::is_engaged`], which
/// includes the one-frame sticky release of an aimed throw), so a keyboard
/// player is never stamped over with neutral. Online it drives this
/// device's handle; with no handle assigned (couch, practice, First Blood)
/// it drives the near seat, P0 — the bot takes P1 in practice, and in
/// couch versus P1 stays on the keyboard. It used to drive every local
/// handle at once, so one finger moved both couch players.
///
/// The flipped (P1) online client renders the world mirrored top-for-
/// bottom so its player sits at the near edge; its screen drags are
/// reflected into world space here, PRE-wire, or "down" walks the
/// character up. Runs after [`mirror_keyboard_for_flip`] so the keyboard's
/// reflection and this one are each applied exactly once. Both peers still
/// exchange plain world-space inputs, so determinism is untouched.
pub(crate) fn read_touch_inputs(
    touch_state: Res<input_touch::TouchState>,
    local_players: Res<bevy_ggrs::LocalPlayers>,
    local_handle: Res<netplay::LocalPlayerHandle>,
    flip: Res<render::PerspectiveFlip>,
    inputs: Option<ResMut<bevy_ggrs::LocalInputs<GgrsCfg>>>,
) {
    if !input_touch::is_engaged(&touch_state) {
        return;
    }
    let Some(mut inputs) = inputs else {
        return;
    };
    let mut input = input_touch::quantize_inputs(&touch_state);
    if flip.0 < 0.0 {
        input = input_touch::mirror_input_y(input);
    }
    if let Some(handle) = touch_handle(local_handle.0, &local_players.0) {
        inputs.0.insert(handle, input);
    }
}

/// Which handle a finger drives: this device's online handle, else the
/// lowest local one (P0). Pure for tests.
pub(crate) fn touch_handle(online: Option<usize>, local: &[usize]) -> Option<usize> {
    online.or_else(|| local.iter().copied().min())
}

/// The keyboard/gamepad's twin of the flip in [`read_touch_inputs`]: when
/// this window is the flipped (P1) client of an online session, reflect
/// the keyboard's world-Y before the wire so "up" on this screen moves the
/// character up on this screen. Couch and practice run with flip = 1.0, so
/// this is a no-op everywhere but an online P1 seat.
pub(crate) fn mirror_keyboard_for_flip(
    flip: Res<render::PerspectiveFlip>,
    inputs: Option<ResMut<bevy_ggrs::LocalInputs<GgrsCfg>>>,
) {
    if flip.0 >= 0.0 {
        return;
    }
    let Some(mut inputs) = inputs else {
        return;
    };
    for input in inputs.0.values_mut() {
        *input = input_touch::mirror_input_y(*input);
    }
}

/// Desktop: toggle borderless fullscreen on F11 — handy for showing a
/// couch match on the big screen. Android manages its own fullscreen, and
/// a browser's is the page's (the manifest asks for it on install), so it
/// runs only where `DeviceProfile::fullscreen_key` says so.
pub(crate) fn toggle_fullscreen(
    keys: Res<bevy::input::ButtonInput<bevy::input::keyboard::KeyCode>>,
    mut windows: Query<&mut Window>,
) {
    use bevy::input::keyboard::KeyCode;
    use bevy::window::{MonitorSelection, WindowMode};
    if !keys.just_pressed(KeyCode::F11) {
        return;
    }
    if let Ok(mut window) = windows.single_mut() {
        window.mode = match window.mode {
            WindowMode::Windowed => WindowMode::BorderlessFullscreen(MonitorSelection::Current),
            _ => WindowMode::Windowed,
        };
    }
}
