//! Phase 8 cycle 7: debug input overlay.
//!
//! Lives in the app crate because that's the only crate in the workspace
//! that pulls in text/render features — `input_touch` stays headless so
//! its tests can run without a windowed Bevy app.
//!
//! Reads:
//!   * `TouchState` — local raw signals (stick vector, aim radians, throw/aim flags)
//!   * `quantize_inputs(touch)` — the 4-byte wire-format `PlayerInput` we'd send
//!   * `InputHistory` — per-handle ring of the last `INPUT_HISTORY_LEN` ticks
//!
//! Writes a single multi-line `Text2d` pinned near the top-left of the
//! screen via `ScreenAnchor`, so it reflows with the window and stays put
//! under the follow-cam/kill-cam.

use bevy::prelude::*;
use input_touch::{TouchState, quantize_inputs};
use sim::{INPUT_HISTORY_LEN, InputHistory, PlayerInput};

use crate::anchor::ScreenAnchor;

#[derive(Component)]
struct DebugOverlayText;

pub struct DebugInputOverlayPlugin;

impl Plugin for DebugInputOverlayPlugin {
    fn build(&self, app: &mut App) {
        // Audit D-HUD-02: this is raw wire-byte input telemetry — a dev tool.
        // Gate it behind `debug_assertions` so it never renders over live
        // gameplay in a release build (the systems compile but never run).
        // It now spawns HIDDEN and toggles with F3, so a normal `cargo run`
        // shows a clean stage — the telemetry is one keypress away when wanted.
        app.add_systems(Startup, spawn_overlay.run_if(|| cfg!(debug_assertions)))
            .add_systems(
                Update,
                (toggle_overlay, update_overlay).run_if(|| cfg!(debug_assertions)),
            );
    }
}

fn spawn_overlay(mut commands: Commands) {
    commands.spawn((
        Text2d::new(String::new()),
        TextFont {
            font_size: 13.0,
            ..default()
        },
        TextColor(render::palette::HIT_WHITE),
        // Text2d anchors at its bounding-box center, so offset inward by
        // ~half the expected block size from the top-left screen corner.
        ScreenAnchor::new(-1.0, 1.0, 200.0, -60.0),
        Transform::from_xyz(0.0, 0.0, 100.0),
        // Off by default; F3 reveals it. Keeps the live stage uncluttered.
        Visibility::Hidden,
        DebugOverlayText,
    ));
}

/// F3 toggles the input-telemetry overlay's visibility (debug builds only).
fn toggle_overlay(
    keys: Res<ButtonInput<KeyCode>>,
    mut q: Query<&mut Visibility, With<DebugOverlayText>>,
) {
    if keys.just_pressed(KeyCode::F3) {
        for mut vis in &mut q {
            *vis = match *vis {
                Visibility::Hidden => Visibility::Visible,
                _ => Visibility::Hidden,
            };
        }
    }
}

fn update_overlay(
    touch: Res<TouchState>,
    history: Res<InputHistory>,
    mut q: Query<&mut Text2d, With<DebugOverlayText>>,
) {
    let Ok(mut text) = q.single_mut() else {
        return;
    };

    let p = quantize_inputs(&touch);
    let stick = touch.stick.unwrap_or(Vec2::ZERO);
    let ring0 = history
        .0
        .get(&0)
        .copied()
        .unwrap_or([PlayerInput::default(); INPUT_HISTORY_LEN]);

    let history_str = ring0
        .iter()
        .map(|e| format!("{:02x}", e.buttons))
        .collect::<Vec<_>>()
        .join(" ");

    let s = format!(
        "TouchState\n  stick=({:+.2}, {:+.2}) aim_rad={:+.2}\n  aim_active={} throw_held={}\nWire (P0)\n  x={:>+4} y={:>+4} aim={:>3} btn=0x{:02x}\nHistory[P0] btns:\n  {history_str}",
        stick.x,
        stick.y,
        touch.aim_angle_rad,
        touch.aim_active,
        touch.throw_held,
        p.stick_x,
        p.stick_y,
        p.aim_angle,
        p.buttons,
    );
    *text = Text2d::new(s);
}
