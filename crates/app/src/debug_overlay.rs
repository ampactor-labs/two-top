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
//! window. Uses the existing `WindowSize` resource (populated by the
//! app's `update_window_metrics`) so the overlay reflows when the
//! window resizes.

use bevy::prelude::*;
use input_touch::{TouchState, WindowSize, quantize_inputs};
use sim::{INPUT_HISTORY_LEN, InputHistory, PlayerInput};

#[derive(Component)]
struct DebugOverlayText;

pub struct DebugInputOverlayPlugin;

impl Plugin for DebugInputOverlayPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_overlay)
            .add_systems(Update, update_overlay);
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
        // Initial transform — re-positioned each frame from WindowSize.
        Transform::from_xyz(0.0, 0.0, 100.0),
        DebugOverlayText,
    ));
}

fn update_overlay(
    touch: Res<TouchState>,
    history: Res<InputHistory>,
    window: Res<WindowSize>,
    mut q: Query<(&mut Text2d, &mut Transform), With<DebugOverlayText>>,
) {
    let Ok((mut text, mut tx)) = q.single_mut() else {
        return;
    };

    // Pin the overlay to roughly the upper-left of the screen. Text2d
    // anchors at the text's bounding-box center by default, so we offset
    // inward by ~half the expected text size. With a ~13px font and ~5
    // visible lines, a rough offset of (200, -60) from the top-left of
    // world space puts the block visibly inside the window without
    // clipping.
    if window.0.length_squared() > 0.0 {
        tx.translation.x = -window.0.x * 0.5 + 200.0;
        tx.translation.y = window.0.y * 0.5 - 60.0;
    }

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
        stick.x, stick.y, touch.aim_angle_rad,
        touch.aim_active, touch.throw_held,
        p.stick_x, p.stick_y, p.aim_angle, p.buttons,
    );
    *text = Text2d::new(s);
}
