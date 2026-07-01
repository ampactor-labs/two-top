//! Phase 12 / M4: live Matchbox WebRTC netplay driver.
//!
//! Online play is opt-in: pass `--room <url>` on the desktop binary (or
//! set `MATCHBOX_ROOM`). The Android APK, launched from a tapped icon with
//! no argv and no settable process env, instead reads a **compile-time**
//! `TWOTOP_ROOM` baked at build time (`TWOTOP_ROOM=<url> cargo apk run ...`).
//! Absent all three ⇒ the local `SyncTestSession` couch-versus build is
//! unchanged. When a room URL is present this module:
//!
//!   1. builds a `WebRtcSocket` with a single **unreliable** channel and
//!      drives its `MessageLoopFuture` on Bevy's `IoTaskPool` (ggrs has
//!      its own reliability layer — an unreliable+unordered channel is the
//!      recommended config);
//!   2. polls `socket.update_peers()` each frame, driving the existing
//!      `net::LobbyState` FSM (`Connecting → WaitingForPeer → Connected`);
//!   3. on the rising edge into `Connected` (surfaced as
//!      `net::PendingP2PSwap`), takes the channel into a `MatchboxBridge`,
//!      builds a 2-player `P2PSession` (local handle assigned by peer-id
//!      ordering — **lower `PeerId` = handle 0**, agreed by both peers),
//!      and inserts it as the live `Session`. The sim is already at frame 0
//!      (Startup spawned the players; no session ran pre-connect, so no
//!      reset is needed);
//!   4. post-swap, drains `P2PSession` events: `DesyncDetected` logs a loud
//!      `error!` (the gate that matters), `Disconnected` forfeits the match.
//!
//! Both peers spawn an identical world from `setup`, so the only
//! peer-specific value is which handle reads local input — the deterministic
//! sim guarantees the rest.

use std::time::Duration;

use bevy::prelude::*;
use bevy::tasks::IoTaskPool;
use bevy_ggrs::Session;
use bevy_ggrs::ggrs::{DesyncDetection, GgrsEvent, PlayerType, SessionBuilder};
// `net` re-exports matchbox's `PeerId` as `MatchboxPeerId`; alias it back
// to the short name and reuse net's `PeerId`↔`NetAddr` bijection helpers so
// app needs no direct matchbox/uuid dependency.
use net::{
    ChannelConfig, LastPeerMessageFrame, LobbyState, MatchboxBridge, MatchboxPeerId as PeerId,
    PeerState, PendingP2PSwap, WebRtcSocket, WebRtcSocketBuilder, addr_to_peer, peer_to_addr,
};
use sim::GgrsCfg;

/// Online input delay (frames). 2 hides a couple frames of network jitter
/// behind local prediction without a perceptible feel cost — the couch
/// build runs 0 because there's no network to hide.
const ONLINE_INPUT_DELAY: usize = 2;

/// ggrs desync-detection cadence: exchange a state checksum every N frames.
/// 30 @ 60 Hz = twice a second — frequent enough to catch a divergence
/// within a few hundred ms, cheap enough to be free on the wire.
const DESYNC_CHECK_INTERVAL: u32 = 30;

/// Drop the peer (and forfeit) after this much silence. Matches the
/// SIGNALING.md long-disconnect gate (~3 s).
const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// Resolved at startup from `--room`/`MATCHBOX_ROOM`. `None` ⇒ local
/// SyncTest mode (the driver systems no-op / aren't added).
#[derive(Resource, Clone, Debug, Default)]
pub struct NetplayConfig {
    pub room_url: Option<String>,
}

/// Which player handle is *this* device's local player, once a P2P session is
/// established. `None` until then — and in couch/SyncTest mode it stays `None`
/// because both players share the one device. Read by render/app concerns that
/// want "you" vs "them" (Phase 18 haptics; the match-summary banner later).
/// Init'd in `app::run` so it always exists; set in [`perform_swap`].
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct LocalPlayerHandle(pub Option<usize>);

impl NetplayConfig {
    /// Read the room URL, in precedence order: the `--room <url>` CLI flag,
    /// then the `MATCHBOX_ROOM` runtime env var, then a `TWOTOP_ROOM` value
    /// baked in at **compile time**. Returns an all-`None` config (local
    /// couch-versus) when none are set.
    ///
    /// The compile-time fallback exists for Android: an APK is launched from
    /// a tapped launcher icon, so it has no argv and no way to set a process
    /// env var. Baking the room URL into the build is the only way the phone
    /// can learn it without an in-app lobby text field (a tracked follow-up).
    /// Build the APK pointed at a room with:
    ///
    /// ```text
    /// TWOTOP_ROOM=ws://<host>:3536/two-top?next=2 cargo apk run -p app \
    ///   --target aarch64-linux-android
    /// ```
    ///
    /// Desktop builds left without `TWOTOP_ROOM` at compile time and without
    /// `--room`/`MATCHBOX_ROOM` at runtime behave exactly as before.
    pub fn from_env_and_args() -> Self {
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            if arg == "--room" {
                return Self {
                    room_url: args.next(),
                };
            }
            if let Some(url) = arg.strip_prefix("--room=") {
                return Self {
                    room_url: Some(url.to_string()),
                };
            }
        }
        // Runtime env var first, then the compile-time bake. Empty strings
        // are treated as unset so a stray `TWOTOP_ROOM=` can't force a
        // bogus online boot with a blank URL.
        let room_url = std::env::var("MATCHBOX_ROOM")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                option_env!("TWOTOP_ROOM")
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            });
        Self { room_url }
    }
}

/// Holds the live socket between lobby ticks. Kept as a **non-send**
/// resource: the socket's async channels are `Send` on native but we never
/// need it off the main schedule thread, and non-send sidesteps any `Sync`
/// question. `channel_taken` flips once the unreliable channel has been
/// handed to the ggrs bridge — after that the socket is only polled for
/// peer-state (disconnect) updates.
struct MatchboxDriver {
    socket: WebRtcSocket,
    channel_taken: bool,
}

/// Phase 12 driver plugin. Added only when a room URL is present. The matchbox
/// socket opens when the player enters the match (OnEnter InMatch), NOT at app
/// boot — so the Title screen is a clean staging area and the signaling server
/// connection only starts on "Play." The per-frame lobby/swap/event-drain driver
/// runs in Update once connected.
pub struct MatchboxPlugin;

impl Plugin for MatchboxPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(crate::screen::AppScreen::InMatch), start_matchbox)
            .add_systems(Update, drive_netplay);
    }
}

/// Startup: build the socket + spawn its message loop. Runs after Bevy's
/// `TaskPoolPlugin` has initialized `IoTaskPool`, so the spawn is safe here
/// (it would not be in `run()` before `App::run`).
fn start_matchbox(world: &mut World) {
    let Some(url) = world.resource::<NetplayConfig>().room_url.clone() else {
        return;
    };

    let (socket, message_loop) = WebRtcSocketBuilder::new(url.clone())
        .add_channel(ChannelConfig::unreliable())
        .build();

    // Drive the WebRTC message loop on the IO pool for the app's lifetime.
    // `detach` lets it run independently; it ends when the socket closes.
    IoTaskPool::get()
        .spawn(async move {
            if let Err(e) = message_loop.await {
                tracing::error!(target: "two_top::net", error = %e, "matchbox message loop ended with error");
            }
        })
        .detach();

    world.insert_non_send_resource(MatchboxDriver {
        socket,
        channel_taken: false,
    });
    *world.resource_mut::<LobbyState>() = LobbyState::Connecting;

    tracing::info!(target: "two_top::net", room = %url, "matchbox socket built — connecting");
}

/// Per-frame driver. Exclusive-world so it can touch the non-send socket,
/// the lobby resources, and the ggrs `Session` without borrow gymnastics.
fn drive_netplay(world: &mut World) {
    let Some(channel_taken) = world
        .get_non_send_resource::<MatchboxDriver>()
        .map(|d| d.channel_taken)
    else {
        return;
    };

    if channel_taken {
        // Post-swap: ggrs owns the channel. Drain session events and keep
        // the silence timer fresh (ggrs's own disconnect path drives
        // forfeit, so net's grace timer must not also trip).
        drain_session_events(world);
        return;
    }

    // --- Pre-swap: drive the lobby from socket peer-state ---
    let (our_id, peer_updates) = {
        let mut driver = world.non_send_resource_mut::<MatchboxDriver>();
        let our_id = driver.socket.id();
        let updates = driver.socket.update_peers();
        (our_id, updates)
    };

    {
        let mut lobby = world.resource_mut::<LobbyState>();
        if let Some(our_id) = our_id
            && matches!(*lobby, LobbyState::Connecting)
        {
            *lobby = LobbyState::WaitingForPeer { our_id };
        }
        for (peer, state) in &peer_updates {
            match state {
                PeerState::Connected => {
                    tracing::info!(target: "two_top::net", peer = %peer, "peer connected");
                    *lobby = LobbyState::Connected { peer_id: *peer };
                }
                PeerState::Disconnected => {
                    tracing::warn!(target: "two_top::net", peer = %peer, "peer left before match start");
                    if let Some(our_id) = our_id {
                        *lobby = LobbyState::WaitingForPeer { our_id };
                    }
                }
            }
        }
    }

    // `net::detect_peer_connection_edge` (also in Update) turns the rising
    // edge into `Connected` into a `PendingP2PSwap`; consume it here.
    if let Some(peer_id) = world.resource::<PendingP2PSwap>().0 {
        perform_swap(world, peer_id);
    }
}

/// Build the `P2PSession` and install it as the live `Session`, replacing
/// the (absent) pre-connect session. Idempotent-ish: bails if our own
/// `PeerId` isn't assigned yet (retried next frame).
fn perform_swap(world: &mut World, peer_id: PeerId) {
    let Some(our_id) = world.non_send_resource_mut::<MatchboxDriver>().socket.id() else {
        return; // id not assigned yet; retry next tick
    };

    // Deterministic, peer-agreed handle assignment: lower PeerId = handle 0.
    let local_handle = if our_id < peer_id { 0 } else { 1 };
    let remote_handle = 1 - local_handle;
    let peer_addr = peer_to_addr(peer_id);

    let bridge = {
        let mut driver = world.non_send_resource_mut::<MatchboxDriver>();
        let channel = driver
            .socket
            .take_channel(0)
            .expect("unreliable channel 0 is present until taken exactly once");
        driver.channel_taken = true;
        MatchboxBridge::new(channel)
    };

    let mut sb = SessionBuilder::<GgrsCfg>::new()
        .with_num_players(2)
        .expect("2 players")
        .with_input_delay(ONLINE_INPUT_DELAY)
        .with_disconnect_timeout(DISCONNECT_TIMEOUT)
        .with_desync_detection_mode(DesyncDetection::On {
            interval: DESYNC_CHECK_INTERVAL,
        });
    sb = sb
        .add_player(PlayerType::Local, local_handle)
        .expect("add local player");
    sb = sb
        .add_player(PlayerType::Remote(peer_addr), remote_handle)
        .expect("add remote player");
    let session = sb.start_p2p_session(bridge).expect("start p2p session");

    world.insert_resource(Session::P2P(session));
    world.resource_mut::<PendingP2PSwap>().0 = None;
    // Record which player is us, so "your-action" feedback (haptics, summary
    // banner) can distinguish local from remote.
    world.resource_mut::<LocalPlayerHandle>().0 = Some(local_handle);
    world.resource_mut::<render::PerspectiveFlip>().0 = if local_handle == 1 { -1.0 } else { 1.0 };

    // Pin the silence timer to "now" so net's grace timer never spuriously
    // forfeits a healthy session — real disconnects come via ggrs events.
    let frame = world.resource::<sim::FrameCount>().0;
    world.resource_mut::<LastPeerMessageFrame>().0 = frame;

    tracing::info!(
        target: "two_top::net",
        local_handle,
        remote_handle,
        our_id = %our_id,
        peer_id = %peer_id,
        input_delay = ONLINE_INPUT_DELAY,
        desync_interval = DESYNC_CHECK_INTERVAL,
        "swapped to P2P session",
    );
}

/// Post-swap: drain ggrs session events. `DesyncDetected` is the
/// load-bearing one — a loud `error!` is the verification signal. A
/// `Disconnected` event forfeits the match (sets `MatchState::MatchOver`,
/// mirroring `net::tick_disconnection_grace`'s forfeit handoff).
fn drain_session_events(world: &mut World) {
    let mut forfeited_peer = None;
    {
        let mut session = world.resource_mut::<Session<GgrsCfg>>();
        if let Session::P2P(s) = &mut *session {
            for event in s.events() {
                match event {
                    GgrsEvent::Synchronizing { addr, total, count } => {
                        tracing::info!(target: "two_top::net", ?addr, total, count, "synchronizing");
                    }
                    GgrsEvent::Synchronized { addr } => {
                        tracing::info!(target: "two_top::net", ?addr, "peer synchronized");
                    }
                    GgrsEvent::NetworkInterrupted {
                        addr,
                        disconnect_timeout,
                    } => {
                        tracing::warn!(target: "two_top::net", ?addr, disconnect_timeout, "network interrupted");
                    }
                    GgrsEvent::NetworkResumed { addr } => {
                        tracing::info!(target: "two_top::net", ?addr, "network resumed");
                    }
                    GgrsEvent::WaitRecommendation { skip_frames } => {
                        tracing::debug!(target: "two_top::net", skip_frames, "wait recommendation");
                    }
                    GgrsEvent::Disconnected { addr } => {
                        tracing::error!(target: "two_top::net", ?addr, "peer disconnected — forfeiting match");
                        forfeited_peer = Some(addr);
                    }
                    GgrsEvent::DesyncDetected {
                        frame,
                        local_checksum,
                        remote_checksum,
                        addr,
                    } => {
                        tracing::error!(
                            target: "two_top::net",
                            frame,
                            local_checksum,
                            remote_checksum,
                            ?addr,
                            "DESYNC DETECTED — local and remote state diverged",
                        );
                    }
                }
            }
        }
    }

    // Healthy session: keep silence ~0 so net's grace timer stays quiet.
    let frame = world.resource::<sim::FrameCount>().0;
    world.resource_mut::<LastPeerMessageFrame>().0 = frame;

    if let Some(addr) = forfeited_peer {
        *world.resource_mut::<LobbyState>() = LobbyState::Forfeited {
            peer_id: addr_to_peer(addr),
        };
        *world.resource_mut::<sim::MatchState>() = sim::MatchState::MatchOver;
    }
}
