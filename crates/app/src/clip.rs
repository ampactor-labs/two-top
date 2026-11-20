//! CLIP (the web theater): the moment worth keeping, as a video file.
//!
//! In the browser, a tape on the projector wears a CLIP label opposite
//! SHARE. A tap rolls the tape to just before its last kill at full speed
//! and asks the page to record the canvas and the game's sound for a few
//! seconds (`twotopClip` in `web/index.html`, MediaRecorder); the page
//! then offers the file to the share sheet. Native builds have no CLIP:
//! a phone already records its own screen better than a game can.

use bevy::prelude::*;

/// Where CLIP sits (window fraction, y-down): SHARE's row, the other side.
const CLIP_ANCHOR: (f32, f32) = (0.18, 0.215);
const CLIP_BAND: ((f32, f32), (f32, f32)) = ((0.02, 0.34), (0.17, 0.26));
/// How long a clip runs: the run-up and the kill.
const CLIP_SECS: f64 = 5.0;

/// Is the window-pixel point `p` on CLIP (while it shows)? The theater's
/// tap-anywhere pause leaves it alone.
pub fn hit(p: Vec2, window: Vec2, theater_active: bool) -> bool {
    cfg!(target_family = "wasm") && theater_active && window.x > 0.0 && window.y > 0.0 && {
        let f = p / window;
        (CLIP_BAND.0.0..CLIP_BAND.0.1).contains(&f.x) && crate::ui::in_band(f.y, CLIP_BAND.1)
    }
}

#[derive(Component)]
struct ClipLabel;

fn spawn_label(mut commands: Commands) {
    if !cfg!(target_family = "wasm") {
        return;
    }
    commands.spawn((
        ClipLabel,
        Text2d::new("CLIP"),
        (
            TextFont {
                font_size: crate::ui::TextRole::ShareLabel.font_size(),
                ..default()
            },
            crate::ui::TextRole::ShareLabel,
        ),
        TextColor(render::palette::SPARK),
        TextLayout::justify(Justify::Center),
        crate::anchor::ScreenAnchor::at(CLIP_ANCHOR.0, CLIP_ANCHOR.1),
        Transform::from_xyz(0.0, 0.0, crate::ui::z::MENU_TEXT - 5.0),
        Visibility::Hidden,
    ));
}

/// Ask the page to record (`false` where it cannot).
#[cfg(target_family = "wasm")]
fn record(seconds: f64) -> bool {
    use wasm_bindgen::{JsCast, JsValue};
    let Some(window) = web_sys::window() else {
        return false;
    };
    let Ok(hook) = js_sys::Reflect::get(&window, &JsValue::from_str("twotopClip")) else {
        return false;
    };
    hook.dyn_ref::<js_sys::Function>()
        .and_then(|f| f.call1(&window, &JsValue::from_f64(seconds)).ok())
        .is_some_and(|v| v.as_bool() == Some(true))
}

#[cfg(not(target_family = "wasm"))]
fn record(_seconds: f64) -> bool {
    false
}

fn clip_input(world: &mut World) {
    let active = world.resource::<crate::theater::TheaterMode>().active();
    let mut q = world.query_filtered::<&mut Visibility, With<ClipLabel>>();
    for mut vis in q.iter_mut(world) {
        *vis = if active {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
    if !active {
        world.resource_mut::<ClipPending>().0 = false;
        return;
    }
    // The tape has reached the moment: roll the camera.
    if world.resource::<ClipPending>().0 && !crate::theater::seeking(world) {
        world.resource_mut::<ClipPending>().0 = false;
        let started = record(CLIP_SECS);
        tracing::info!(target: "two_top::clip", started, "clip recording");
    }
    let win = world.resource::<input_touch::WindowSize>().0;
    let tapped = world
        .resource::<Touches>()
        .iter_just_pressed()
        .any(|t| hit(t.position(), win, active))
        || world
            .resource::<ButtonInput<KeyCode>>()
            .just_pressed(KeyCode::KeyC);
    if !tapped {
        return;
    }
    crate::theater::roll_to_last_kill(world);
    world.resource_mut::<ClipPending>().0 = true;
}

/// A clip asked for, waiting for the tape to reach its moment.
#[derive(Resource, Default)]
struct ClipPending(bool);

pub struct ClipPlugin;

impl Plugin for ClipPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClipPending>()
            .add_systems(Startup, spawn_label)
            .add_systems(
                Update,
                clip_input.run_if(in_state(crate::screen::AppScreen::InMatch)),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_never_shares_a_band_with_share_or_the_exit() {
        assert!(!crate::ui::overlaps(CLIP_BAND.1, crate::ui::TOP_EXIT));
        // SHARE's theater band starts at x 0.66; CLIP ends at 0.34.
        assert!(CLIP_BAND.0.1 < 0.66);
    }
}
