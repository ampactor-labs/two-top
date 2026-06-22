//! Phase 12 cycle 5: lobby state debug overlay.
//!
//! Mirrors the `debug_overlay` pattern (Text2d pinned near a window
//! corner, world-space-positioned via the `WindowSize` resource).
//! Renders the current `LobbyState` as a single-line label so an
//! operator can see at a glance whether they're idle, connecting,
//! waiting, in-match, on the disconnection countdown, or forfeit.
//!
//! This is the minimal viable lobby UI — Phase 12 cycle 5's contract
//! is "operator can observe the netplay lifecycle." A polished
//! Find-Match button + room-code entry lands later, after the
//! signaling driver is operator-tested per SIGNALING.md (cycle 6).
//! For dev sessions running the existing `Session::SyncTest`, the
//! overlay shows `Idle` since `NetPlugin` doesn't drive any state
//! transitions on its own — exactly the right "no networking active"
//! signal.

use bevy::prelude::*;
use input_touch::WindowSize;
use net::LobbyState;

#[derive(Component)]
struct LobbyOverlayText;

pub struct LobbyOverlayPlugin;

impl Plugin for LobbyOverlayPlugin {
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
        // Yellow tint distinguishes lobby state from the cyan-ish
        // input debug overlay so they're trivially separable on
        // screen even before reading the actual text.
        TextColor(render::palette::SPARK),
        Transform::from_xyz(0.0, 0.0, 100.0),
        LobbyOverlayText,
    ));
}

fn update_overlay(
    state: Res<LobbyState>,
    window: Res<WindowSize>,
    mut q: Query<(&mut Text2d, &mut Transform, &mut Visibility), With<LobbyOverlayText>>,
) {
    let Ok((mut text, mut tx, mut vis)) = q.single_mut() else {
        return;
    };

    // Audit D-HUD-02: the lobby label is meaningful netplay feedback
    // (connecting / reconnecting / forfeit), but in the default couch build
    // `LobbyState` sits at `Idle` forever — so hide the label entirely while
    // idle and only surface it once a netplay lifecycle is actually underway.
    *vis = if matches!(*state, LobbyState::Idle) {
        Visibility::Hidden
    } else {
        Visibility::Visible
    };

    // Top-right corner — keeps it out of the way of the
    // top-left input debug overlay. Same world-space convention.
    if window.0.length_squared() > 0.0 {
        tx.translation.x = window.0.x * 0.5 - 150.0;
        tx.translation.y = window.0.y * 0.5 - 30.0;
    }

    let label = match &*state {
        LobbyState::Idle => "lobby: idle".to_string(),
        LobbyState::Connecting => "lobby: connecting…".to_string(),
        LobbyState::WaitingForPeer { our_id } => {
            format!("lobby: waiting peer (us={})", &our_id.0.to_string()[..8])
        }
        LobbyState::Connected { peer_id } => {
            format!("lobby: connected ({})", &peer_id.0.to_string()[..8])
        }
        LobbyState::Disconnected {
            peer_id,
            since_frame,
        } => {
            format!(
                "lobby: reconnecting… peer={} since=f{since_frame}",
                &peer_id.0.to_string()[..8]
            )
        }
        LobbyState::Forfeited { peer_id } => {
            format!("lobby: FORFEIT (peer={})", &peer_id.0.to_string()[..8])
        }
    };
    text.0 = label;
}
