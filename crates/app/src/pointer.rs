//! The mouse, as a finger.
//!
//! Every menu in the game reads Bevy's `Touches`: the title's buttons, the
//! summary, settings, the roster, rivals, replays, the theater's scrub bar,
//! the crash notice. A mouse never reaches `Touches` (winit's web backend
//! hands a mouse click over as a mouse event, and desktop has no touch at
//! all), so on a laptop — a browser or the desktop build — not one button
//! could be clicked; only the unlabelled keyboard shortcuts worked.
//!
//! So the left mouse button is turned into a touch here, before Bevy's
//! touch system runs, and everything downstream sees a finger: the taps,
//! the drags, and the gameplay touch layer (`input_touch`), which used to
//! carry its own separate mouse fallback for the same purpose. One
//! translation, at the edge, for every reader and every platform.
//!
//! A real finger wins: while one is down (a touchscreen laptop), the mouse
//! does not start a second touch, so a platform that also synthesizes mouse
//! events from touches cannot register one tap twice.

use bevy::input::ButtonState;
use bevy::input::InputSystems;
use bevy::input::mouse::MouseButtonInput;
use bevy::input::touch::{TouchInput, TouchPhase, Touches};
use bevy::prelude::*;
use input_touch::MOUSE_TOUCH_ID;

/// The synthetic finger, while the button is held: its window and where
/// it was last reported.
#[derive(Default)]
struct MouseFinger(Option<(Entity, Vec2)>);

/// What one frame of mouse activity becomes, as touch phases in order.
/// Pure, so the translation is tested without a window.
///
/// `held` is the finger carried over from last frame; `cursor` is where
/// the cursor is now (`None` when it is outside the window); `presses` are
/// this frame's left-button transitions in arrival order; `real_finger`
/// says a real touch is down.
fn translate(
    held: Option<Vec2>,
    cursor: Option<Vec2>,
    presses: &[ButtonState],
    real_finger: bool,
) -> (Vec<(TouchPhase, Vec2)>, Option<Vec2>) {
    let mut out = Vec::new();
    let mut down = held;
    // Movement first, for a finger that was already down.
    if let Some(last) = down {
        match cursor {
            Some(pos) if pos != last => {
                out.push((TouchPhase::Moved, pos));
                down = Some(pos);
            }
            // Dragged out of the window: the finger is gone.
            None => {
                out.push((TouchPhase::Canceled, last));
                down = None;
            }
            _ => {}
        }
    }
    for state in presses {
        match state {
            ButtonState::Pressed => {
                if down.is_none()
                    && !real_finger
                    && let Some(pos) = cursor
                {
                    out.push((TouchPhase::Started, pos));
                    down = Some(pos);
                }
            }
            ButtonState::Released => {
                if let Some(last) = down.take() {
                    out.push((TouchPhase::Ended, cursor.unwrap_or(last)));
                }
            }
        }
    }
    (out, down)
}

/// `PreUpdate`, before Bevy's input systems: write the left mouse button
/// into the touch stream as finger [`MOUSE_TOUCH_ID`].
fn mouse_as_touch(
    mut buttons: MessageReader<MouseButtonInput>,
    touches: Res<Touches>,
    windows: Query<&Window>,
    mut finger: Local<MouseFinger>,
    mut out: MessageWriter<TouchInput>,
) {
    let mut presses = Vec::new();
    let mut window = finger.0.map(|(window, _)| window);
    for event in buttons.read() {
        if event.button == MouseButton::Left {
            presses.push(event.state);
            window.get_or_insert(event.window);
        }
    }
    let Some(window) = window else {
        return;
    };
    let cursor = windows.get(window).ok().and_then(Window::cursor_position);
    let real_finger = touches.iter().any(|touch| touch.id() != MOUSE_TOUCH_ID);
    let (phases, down) = translate(finger.0.map(|(_, pos)| pos), cursor, &presses, real_finger);
    for (phase, position) in phases {
        out.write(TouchInput {
            phase,
            position,
            window,
            force: None,
            id: MOUSE_TOUCH_ID,
        });
    }
    finger.0 = down.map(|pos| (window, pos));
}

/// The mouse reaches every touch reader.
pub struct PointerPlugin;

impl Plugin for PointerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, mouse_as_touch.before(InputSystems));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Vec2 = Vec2::new(10.0, 20.0);
    const B: Vec2 = Vec2::new(30.0, 40.0);

    fn phases(out: &[(TouchPhase, Vec2)]) -> Vec<TouchPhase> {
        out.iter().map(|(phase, _)| *phase).collect()
    }

    #[test]
    fn a_click_is_a_tap() {
        let (out, down) = translate(None, Some(A), &[ButtonState::Pressed], false);
        assert_eq!(out, vec![(TouchPhase::Started, A)]);
        assert_eq!(down, Some(A));
        let (out, down) = translate(down, Some(A), &[ButtonState::Released], false);
        assert_eq!(out, vec![(TouchPhase::Ended, A)]);
        assert_eq!(down, None);
    }

    #[test]
    fn a_click_inside_one_frame_starts_and_ends() {
        let (out, down) = translate(
            None,
            Some(A),
            &[ButtonState::Pressed, ButtonState::Released],
            false,
        );
        assert_eq!(phases(&out), vec![TouchPhase::Started, TouchPhase::Ended]);
        assert_eq!(down, None);
    }

    #[test]
    fn a_drag_moves_the_finger() {
        let (out, down) = translate(Some(A), Some(B), &[], false);
        assert_eq!(out, vec![(TouchPhase::Moved, B)]);
        assert_eq!(down, Some(B));
        // Holding still says nothing.
        let (out, _) = translate(down, Some(B), &[], false);
        assert!(out.is_empty());
    }

    #[test]
    fn leaving_the_window_cancels_the_finger() {
        let (out, down) = translate(Some(A), None, &[], false);
        assert_eq!(out, vec![(TouchPhase::Canceled, A)]);
        assert_eq!(down, None);
    }

    #[test]
    fn a_real_finger_wins() {
        let (out, down) = translate(None, Some(A), &[ButtonState::Pressed], true);
        assert!(out.is_empty());
        assert_eq!(down, None);
    }

    #[test]
    fn a_press_outside_the_window_is_ignored() {
        let (out, down) = translate(None, None, &[ButtonState::Pressed], false);
        assert!(out.is_empty());
        assert_eq!(down, None);
    }
}
