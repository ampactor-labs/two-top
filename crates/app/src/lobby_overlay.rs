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

/// Seconds in one waiting state before the overlay starts diagnosing out
/// loud. STUN-only pairs behind carrier NAT fail ICE *silently* — the
/// only observable is "we can see each other on signaling and never
/// connect" — so past this threshold the wait names its likely cause
/// instead of breathing dots forever.
const STALL_DIAGNOSIS_SECS: f32 = 15.0;

/// While in-match but pre-peer, the wait is a *ceremony*, not a debug
/// string: SUMMONING with a breathing ellipsis and the dialed room. The
/// bad states (away / forfeit) speak plainly and large, and a stalled
/// connection eventually says what is actually wrong.
#[allow(clippy::too_many_arguments)]
fn update_summoning(
    time: Res<Time<Real>>,
    screen: Res<State<AppScreen>>,
    state: Res<LobbyState>,
    match_state: Res<sim::MatchState>,
    room: Res<RoomCode>,
    peer_profile: Res<net::PeerProfile>,
    absence: Res<crate::netplay::RecentAbsence>,
    mut stall: Local<Option<(std::mem::Discriminant<LobbyState>, f32)>>,
    mut q: Query<(&mut Text2d, &mut Visibility), With<SummoningText>>,
) {
    let Ok((mut text, mut vis)) = q.single_mut() else {
        return;
    };
    // At MatchOver the summary card owns the screen — it carries the fled /
    // abandoned facts itself, and this text was stacking straight into it.
    if *screen.get() != AppScreen::InMatch
        || matches!(*match_state, sim::MatchState::MatchOver)
    {
        *vis = Visibility::Hidden;
        *stall = None;
        return;
    }
    let now = time.elapsed_secs();
    // Time-in-state: reset the stall clock whenever the state changes.
    let disc = std::mem::discriminant(&*state);
    let since = match *stall {
        Some((d, at)) if d == disc => now - at,
        _ => {
            *stall = Some((disc, now));
            0.0
        }
    };

    let dots = [".", "..", "...", ".."][(now * 2.0) as usize % 4];
    let room_line = if room.custom {
        format!("room {}", code_spaced(&room.code_string()))
    } else {
        "quick match".to_string()
    };
    let challenger = peer_profile
        .0
        .map(|p| crate::profile::name_from_slots(&p.name))
        .unwrap_or_else(|| "OPPONENT".to_string());
    let msg = match &*state {
        LobbyState::Connecting => {
            let mut m = format!("SUMMONING{dots}\n\n{room_line}");
            if since > STALL_DIAGNOSIS_SECS {
                m.push_str("\n\nstill reaching the room server\ncheck this phone's connection");
            }
            Some(m)
        }
        LobbyState::WaitingForPeer { .. } => {
            let mut m = format!(
                "AWAITING A CHALLENGER{dots}\n\n{room_line}\ndial the same code over there"
            );
            if since > STALL_DIAGNOSIS_SECS {
                m.push_str(
                    "\n\nif the other phone shows this too,\nthe networks may need the relay (TURN)",
                );
            }
            Some(m)
        }
        LobbyState::Disconnected { .. } => Some(format!(
            "{challenger} AWAY{dots}\nhold the field - forfeit soon"
        )),
        LobbyState::Forfeited { .. } => {
            // If OUR phone went away just before the forfeit, we are the
            // one who fled — say so instead of gaslighting the player.
            if absence.within(now, crate::netplay::RecentAbsence::FORFEIT_BLAME_SECS) {
                Some("MATCH ABANDONED\nyou left the duel".to_string())
            } else {
                Some(format!("{challenger} FLED\nthe field is yours"))
            }
        }
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
