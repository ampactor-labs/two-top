//! Phase 12 cycle 5: lobby state debug overlay.
//!
//! Mirrors the `debug_overlay` pattern (Text2d pinned near a window
//! corner via `ScreenAnchor`).
//! Renders the current `LobbyState` as a single-line label so an
//! operator can see at a glance whether they're idle, connecting,
//! waiting, in-match, on the disconnection countdown, or forfeit.
//!
//! This is the minimal viable lobby UI — Phase 12 cycle 5's contract
//! is "operator can observe the netplay lifecycle." The Title screen owns the
//! Play gesture; polished room-code entry/matchmaking controls land later.
//! For dev sessions running the existing `Session::SyncTest`, the
//! overlay shows `Idle` since `NetPlugin` doesn't drive any state
//! transitions on its own — exactly the right "no networking active"
//! signal.

use bevy::prelude::*;
use net::LobbyState;

use crate::anchor::ScreenAnchor;
use crate::room_code::RoomCode;
use crate::screen::AppScreen;

#[derive(Component)]
struct LobbyOverlayText;

/// The player-facing matchmaking presence: a big centered status while the
/// P2P handshake runs (the corner label above stays as dev telemetry).
#[derive(Component)]
struct SummoningText;

pub struct LobbyOverlayPlugin;

impl Plugin for LobbyOverlayPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, (spawn_overlay, spawn_summoning))
            .add_systems(Update, (update_overlay, update_summoning));
    }
}

fn spawn_summoning(mut commands: Commands) {
    commands.spawn((
        SummoningText,
        Text2d::new(String::new()),
        TextFont {
            font_size: 54.0,
            ..default()
        },
        TextColor(render::palette::HOT_BONE),
        TextLayout::new_with_justify(Justify::Center),
        // Wide bounds: without them Bevy wraps at awkward widths on the
        // phone ("the other / phone" mid-phrase breaks).
        bevy::text::TextBounds::new_horizontal(1080.0),
        ScreenAnchor::new(0.0, 0.25, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, 210.0),
        Visibility::Hidden,
    ));
}

/// While in-match but pre-peer, the wait is a *ceremony*, not a debug
/// string: SUMMONING with a breathing ellipsis and the dialed room. The
/// bad states (reconnecting / forfeit) speak plainly and large.
fn update_summoning(
    time: Res<Time<Real>>,
    screen: Res<State<AppScreen>>,
    state: Res<LobbyState>,
    room: Res<RoomCode>,
    mut q: Query<(&mut Text2d, &mut Visibility), With<SummoningText>>,
) {
    let Ok((mut text, mut vis)) = q.single_mut() else {
        return;
    };
    if *screen.get() != AppScreen::InMatch {
        *vis = Visibility::Hidden;
        return;
    }
    let dots = [".", "..", "...", ".."][(time.elapsed_secs() * 2.0) as usize % 4];
    let room_line = if room.custom {
        format!("room {}", code_spaced(&room.code_string()))
    } else {
        "quick match".to_string()
    };
    let msg = match &*state {
        LobbyState::Connecting => Some(format!("SUMMONING{dots}\n\n{room_line}")),
        LobbyState::WaitingForPeer { .. } => Some(format!(
            "AWAITING A CHALLENGER{dots}\n\n{room_line}\ndial the same code over there"
        )),
        LobbyState::Disconnected { .. } => {
            Some(format!("OPPONENT LOST{dots}\nholding the field"))
        }
        LobbyState::Forfeited { .. } => Some("OPPONENT FLED\nthe field is yours".to_string()),
        LobbyState::Idle | LobbyState::Connected { .. } => None,
    };
    match msg {
        Some(m) => {
            text.0 = m;
            *vis = Visibility::Visible;
        }
        None => *vis = Visibility::Hidden,
    }
}

/// "CURS" → "C U R S" (the glyphs read as a dialed code, not a word).
fn code_spaced(code: &str) -> String {
    code.chars()
        .map(String::from)
        .collect::<Vec<_>>()
        .join(" ")
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
        // Top-right corner — keeps it out of the way of the top-left input
        // debug overlay. Pinned to the real screen corner on any aspect.
        ScreenAnchor::new(1.0, 1.0, -160.0, -30.0),
        Transform::from_xyz(0.0, 0.0, 100.0),
        LobbyOverlayText,
    ));
}

fn update_overlay(
    state: Res<LobbyState>,
    mut q: Query<(&mut Text2d, &mut Visibility), With<LobbyOverlayText>>,
) {
    let Ok((mut text, mut vis)) = q.single_mut() else {
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
