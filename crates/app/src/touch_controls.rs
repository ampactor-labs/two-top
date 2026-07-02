//! On-screen touch controls — the visible half of the input layer.
//!
//! The input mapping (input_touch) already turns touch into named actions:
//! the lower-left quadrant is the floating move stick, the lower-right is
//! hold-to-charge / drag-to-aim throw, and the upper-right is dash. But
//! none of it was *drawn*, so a new player couldn't find dash at all. This
//! module renders a labelled ring in each active quadrant, lit while its
//! action is held, so the controls teach themselves.
//!
//! Render-only, reads the local `TouchState` (never rolled back). Shown
//! only in a live match on touch builds (or with `TWOTOP_SHOW_TOUCH=1` for
//! desktop capture verification).

use bevy::prelude::*;
use input_touch::TouchState;

use crate::screen::{AppScreen, AwaitingPeer};

/// Which control a ring represents.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Control {
    Move,
    Throw,
    Dash,
}

#[derive(Component)]
struct TouchRing(Control);

#[derive(Component)]
struct TouchLabel(Control);

/// Whether the on-screen controls render at all (touch platform / forced).
#[derive(Resource)]
struct TouchControlsShown(bool);

const RING_SIZE: f32 = 240.0;
const DASH_RING_SIZE: f32 = 200.0;

pub struct TouchControlsPlugin;

impl Plugin for TouchControlsPlugin {
    fn build(&self, app: &mut App) {
        let shown = cfg!(target_os = "android")
            || std::env::var("TWOTOP_SHOW_TOUCH").is_ok_and(|v| v == "1");
        app.insert_resource(TouchControlsShown(shown))
            .add_systems(Startup, spawn_controls)
            .add_systems(Update, update_controls);
    }
}

/// Anchor (y-up frac) for each control's rendered center, matching the
/// `input_touch` touch zones: move = lower-left, throw = lower-right (lifted
/// clear of the dash corner), dash = the bottom-right corner button.
fn anchor_for(c: Control) -> (f32, f32) {
    match c {
        Control::Move => (-0.5, -0.5),
        Control::Throw => (0.42, -0.34),
        Control::Dash => (0.66, -0.80),
    }
}

fn label_text(c: Control) -> &'static str {
    match c {
        Control::Move => "MOVE",
        Control::Throw => "THROW\nhold + aim",
        Control::Dash => "DASH",
    }
}

fn spawn_controls(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    shown: Res<TouchControlsShown>,
) {
    if !shown.0 {
        return;
    }
    let ring_img = asset_server.load("sprites/fx/charge_ring.png");
    for c in [Control::Move, Control::Throw, Control::Dash] {
        let (fx, fy) = anchor_for(c);
        let size = if c == Control::Dash {
            DASH_RING_SIZE
        } else {
            RING_SIZE
        };
        commands.spawn((
            TouchRing(c),
            Sprite {
                image: ring_img.clone(),
                color: idle_color(c),
                custom_size: Some(Vec2::splat(size)),
                ..default()
            },
            crate::anchor::ScreenAnchor::new(fx, fy, 0.0, 0.0),
            // Above gameplay + HUD, below the menu scrim (z=100) so it hides
            // itself on the title with everything else.
            Transform::from_xyz(0.0, 0.0, 60.0),
            Visibility::Hidden,
        ));
        commands.spawn((
            TouchLabel(c),
            Text2d::new(label_text(c)),
            TextFont {
                font_size: if c == Control::Dash { 40.0 } else { 30.0 },
                ..default()
            },
            TextColor(idle_color(c)),
            TextLayout::new_with_justify(Justify::Center),
            crate::anchor::ScreenAnchor::new(fx, fy, 0.0, 0.0),
            Transform::from_xyz(0.0, 0.0, 61.0),
            Visibility::Hidden,
        ));
    }
}

/// Dash reads brightest (it's the one players can't find); move/throw are
/// quieter hints.
fn idle_color(c: Control) -> Color {
    match c {
        Control::Dash => render::palette::SPARK.with_alpha(0.55),
        Control::Move => render::palette::COLD_STONE.with_alpha(0.32),
        Control::Throw => render::palette::COLD_STONE.with_alpha(0.32),
    }
}

fn active_color(c: Control) -> Color {
    match c {
        Control::Dash => render::scale_color(render::palette::SPARK, 1.4).with_alpha(0.95),
        Control::Move => render::palette::HOT_BONE.with_alpha(0.8),
        Control::Throw => render::palette::EMBER.with_alpha(0.85),
    }
}

#[allow(clippy::type_complexity)]
fn update_controls(
    shown: Res<TouchControlsShown>,
    screen: Res<State<AppScreen>>,
    awaiting: Res<AwaitingPeer>,
    touch: Res<TouchState>,
    time: Res<Time<Real>>,
    mut rings: Query<
        (&TouchRing, &mut Sprite, &mut Visibility, &mut Transform),
        Without<TouchLabel>,
    >,
    mut labels: Query<(&TouchLabel, &mut TextColor, &mut Visibility), Without<TouchRing>>,
) {
    if !shown.0 {
        return;
    }
    // Only during live play (not the title or the pre-peer waiting room).
    let live = *screen.get() == AppScreen::InMatch && !awaiting.0;
    let active = |c: Control| match c {
        Control::Move => touch.stick_touch.is_some(),
        Control::Throw => touch.throw_held,
        Control::Dash => touch.dash_held,
    };
    // A slow breath on the dash ring so it draws the eye until first used.
    let pulse = 0.75 + 0.25 * (time.elapsed_secs() * 2.5).sin();

    for (ring, mut sprite, mut vis, mut tx) in &mut rings {
        *vis = if live {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        let on = active(ring.0);
        sprite.color = if on {
            active_color(ring.0)
        } else if ring.0 == Control::Dash {
            idle_color(ring.0).with_alpha(0.55 * pulse)
        } else {
            idle_color(ring.0)
        };
        // A tiny scale kick when held so the tap registers visually.
        let base = if ring.0 == Control::Dash {
            DASH_RING_SIZE
        } else {
            RING_SIZE
        };
        sprite.custom_size = Some(Vec2::splat(base * if on { 1.12 } else { 1.0 }));
        tx.rotation = Quat::from_rotation_z(time.elapsed_secs() * 0.6);
    }
    for (label, mut color, mut vis) in &mut labels {
        *vis = if live {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        color.0 = if active(label.0) {
            active_color(label.0)
        } else {
            idle_color(label.0)
        };
    }
}
