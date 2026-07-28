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
    NetMsg, NetSendQueue, PeerProfile, PeerState, PendingP2PSwap, RematchConsent,
    RtcIceServerConfig, WebRtcSocket, WebRtcSocketBuilder, addr_to_peer, decode_net_msg,
    encode_net_msg, peer_to_addr,
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

/// Drop the peer (and forfeit) after this much silence. 9 s — long enough
/// that a phone surviving a notification-shade peek or a short call banner
/// comes back into a live match (ggrs replays the missed ticks), short
/// enough that a genuinely gone opponent doesn't hold the field hostage.
/// The OPPONENT AWAY overlay covers the wait (net's silence FSM flips the
/// lobby to `Disconnected` after ~1 s once the driver stops pinning the
/// silence timer). net::FORFEIT_AFTER_FRAMES (10 s) is the fallback gate
/// just behind this.
const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(9);

/// The matchbox channel ggrs owns (unreliable + unordered).
const GGRS_CHANNEL: usize = 0;
/// The reliable side-channel (identity, rematch consent, goodbye).
const SIDE_CHANNEL: usize = 1;

/// Resolved at startup from `--room`/`MATCHBOX_ROOM`. `None` ⇒ local
/// SyncTest mode (the driver systems no-op / aren't added).
#[derive(Resource, Clone, Debug, Default)]
pub struct NetplayConfig {
    pub room_url: Option<String>,
    /// The ephemeral-ICE credential vendor (`ice_vendor`'s `GET /ice`).
    /// When set, match entry fetches short-lived TURN credentials from it
    /// instead of using anything compiled into the binary — the public APK
    /// carries a URL, never a secret. `None` ⇒ the baked/STUN-only
    /// [`ice_server_config`] path, unchanged.
    pub ice_url: Option<String>,
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
        // Runtime env first, then the compile-time bake (the APK path —
        // note it is a URL, never a credential; see `ice_vendor`).
        let ice_url = std::env::var("TWOTOP_ICE_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                option_env!("TWOTOP_ICE_URL")
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            });
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            if arg == "--room" {
                return Self {
                    room_url: args.next(),
                    ice_url,
                };
            }
            if let Some(url) = arg.strip_prefix("--room=") {
                return Self {
                    room_url: Some(url.to_string()),
                    ice_url,
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
        Self { room_url, ice_url }
    }
}

/// TURN relay config, resolved like the room URL: runtime env first
/// (`TWOTOP_TURN_URL` / `TWOTOP_TURN_USER` / `TWOTOP_TURN_PASS`), then the
/// compile-time bake. Returns the matchbox default (Google STUN only)
/// when unset — same behavior as before this existed.
///
/// This is the LOCAL/testing path and the fallback when the ephemeral
/// vendor is unreachable. Public builds must never bake `TWOTOP_TURN_*`
/// (extractable from any distributed binary) — they carry `TWOTOP_ICE_URL`
/// instead and fetch throwaway credentials at match entry.
///
/// Why TURN matters: STUN-only traversal fails for phone pairs behind
/// carrier-grade NAT (very common on cellular). A TURN relay is the
/// fallback path that makes "two strangers on two networks" reliable.
/// The STUN urls stay in the list either way; WebRTC only applies the
/// credentials to the `turn:` entries.
fn ice_server_config() -> RtcIceServerConfig {
    let get = |run: &str, bake: Option<&'static str>| {
        std::env::var(run)
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| bake.filter(|s| !s.is_empty()).map(str::to_string))
    };
    let turn_url = get("TWOTOP_TURN_URL", option_env!("TWOTOP_TURN_URL"));
    let mut config = RtcIceServerConfig::default();
    let Some(url) = turn_url else {
        return config;
    };
    config.urls.push(url);
    config.username = get("TWOTOP_TURN_USER", option_env!("TWOTOP_TURN_USER"));
    config.credential = get("TWOTOP_TURN_PASS", option_env!("TWOTOP_TURN_PASS"));
    config
}

/// Holds the live socket between lobby ticks. Kept as a **non-send**
/// resource: the socket's async channels are `Send` on native but we never
/// need it off the main schedule thread, and non-send sidesteps any `Sync`
/// question. `channel_taken` flips once the unreliable channel has been
/// handed to the ggrs bridge — after that the socket is polled for
/// peer-state updates and pumps the reliable side-channel.
struct MatchboxDriver {
    socket: WebRtcSocket,
    channel_taken: bool,
    /// ggrs reported `NetworkInterrupted` and no `NetworkResumed` yet.
    /// While true the driver stops pinning `LastPeerMessageFrame`, so
    /// net's silence FSM ages honestly into `Disconnected` (the
    /// OPPONENT AWAY overlay) and recovers when traffic resumes.
    interrupted: bool,
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
            .add_systems(
                OnExit(crate::screen::AppScreen::InMatch),
                |world: &mut World| {
                    // A quit/cancel during a pending credential fetch must
                    // not open a socket after we've left the screen.
                    world.remove_resource::<PendingIce>();
                },
            )
            .add_systems(
                Update,
                (
                    finish_ice_fetch,
                    drive_netplay,
                    track_absence,
                    reset_rematch_consent,
                ),
            );
    }
}

/// Seconds the credential fetch may hold up the connection before we fall
/// back to the baked/STUN-only config. The fetch rides the SUMMONING wait,
/// so its only visible cost is when the vendor is down — and then it's this.
const ICE_FETCH_TIMEOUT_SECS: f32 = 2.5;

/// An in-flight ephemeral-credential fetch (`NetplayConfig::ice_url`).
/// The task thread reports through the channel; `finish_ice_fetch` opens
/// the socket with whatever arrives — or with the fallback on
/// timeout/error. `Mutex` because `mpsc::Receiver` is `Send` but not
/// `Sync` and resources must be both.
#[derive(Resource)]
struct PendingIce {
    rx: std::sync::Mutex<std::sync::mpsc::Receiver<Option<RtcIceServerConfig>>>,
    started_at: f32,
}

/// The vendor's `GET /ice` contract — field names in lockstep with
/// `ice_vendor::IceResponse`.
#[derive(serde::Deserialize)]
struct IceResponse {
    urls: Vec<String>,
    username: Option<String>,
    credential: Option<String>,
}

/// Parse a vendor response body into matchbox's ICE config. `None` for
/// anything malformed or empty — the caller falls back to the baked path.
/// Pure for tests.
pub(crate) fn parse_ice_response(body: &str) -> Option<RtcIceServerConfig> {
    let parsed: IceResponse = serde_json::from_str(body).ok()?;
    if parsed.urls.is_empty() {
        return None;
    }
    Some(RtcIceServerConfig {
        urls: parsed.urls,
        username: parsed.username,
        credential: parsed.credential,
    })
}

/// Blocking fetch of the vendor's ICE config (runs on the IO task pool —
/// never the main thread). Tight timeout: the fallback exists.
fn fetch_ice(url: &str) -> Option<RtcIceServerConfig> {
    let response = ureq::get(url)
        .timeout(Duration::from_secs(2))
        .call()
        .map_err(|e| {
            tracing::warn!(target: "two_top::net", error = %e, "ice vendor unreachable");
            e
        })
        .ok()?;
    let body = response.into_string().ok()?;
    let config = parse_ice_response(&body);
    if config.is_none() {
        tracing::warn!(target: "two_top::net", "ice vendor response unusable — falling back");
    }
    config
}

/// Startup: build the socket + spawn its message loop. Runs after Bevy's
/// `TaskPoolPlugin` has initialized `IoTaskPool`, so the spawn is safe here
/// (it would not be in `run()` before `App::run`).
///
/// With an ice vendor configured, the socket build waits (bounded) for the
/// ephemeral-credential fetch — `finish_ice_fetch` completes it. Without
/// one, the socket opens immediately on the baked/STUN-only config.
fn start_matchbox(world: &mut World) {
    // Practice mode runs a local session against the bot — no socket, even
    // on an online build. The replay theater replays a tape the same way.
    if world.resource::<crate::bot::PracticeMode>().0
        || world.resource::<crate::theater::TheaterMode>().active()
    {
        return;
    }
    let config = world.resource::<NetplayConfig>().clone();
    let Some(url) = config.room_url else {
        return;
    };

    if let Some(ice_url) = config.ice_url {
        let (tx, rx) = std::sync::mpsc::channel();
        bevy::tasks::IoTaskPool::get()
            .spawn(async move {
                let _ = tx.send(fetch_ice(&ice_url));
            })
            .detach();
        let started_at = world.resource::<Time<Real>>().elapsed_secs();
        world.insert_resource(PendingIce {
            rx: std::sync::Mutex::new(rx),
            started_at,
        });
        // SUMMONING goes up now; the socket follows within the timeout.
        *world.resource_mut::<LobbyState>() = LobbyState::Connecting;
        tracing::info!(target: "two_top::net", "fetching ephemeral ICE credentials");
        return;
    }
    open_socket(world, url, ice_server_config());
}

/// Complete a pending credential fetch: open the socket with the fetched
/// config the moment it lands, or with the baked/STUN-only fallback on
/// error or timeout. Exclusive-world (socket is non-send).
fn finish_ice_fetch(world: &mut World) {
    if world.get_resource::<PendingIce>().is_none() {
        return;
    }
    let now = world.resource::<Time<Real>>().elapsed_secs();
    let outcome = {
        let pending = world.resource::<PendingIce>();
        match pending
            .rx
            .lock()
            .expect("fetch thread never panics holding it")
            .try_recv()
        {
            Ok(result) => Some(result),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(None),
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                if now - pending.started_at > ICE_FETCH_TIMEOUT_SECS {
                    tracing::warn!(target: "two_top::net", "ice vendor timed out — falling back");
                    Some(None)
                } else {
                    None
                }
            }
        }
    };
    let Some(result) = outcome else {
        return; // still in flight, within budget
    };
    world.remove_resource::<PendingIce>();
    let Some(url) = world.resource::<NetplayConfig>().room_url.clone() else {
        return;
    };
    let fetched = result.is_some();
    let ice = result.unwrap_or_else(ice_server_config);
    tracing::info!(target: "two_top::net", fetched, "ice config resolved");
    open_socket(world, url, ice);
}

/// Build the matchbox socket with the given ICE config and hand its
/// message loop to the IO pool — the tail of the old `start_matchbox`,
/// shared by the immediate and fetched paths.
fn open_socket(world: &mut World, url: String, ice: RtcIceServerConfig) {
    let has_turn = ice.urls.iter().any(|u| u.starts_with("turn"));
    let (socket, message_loop) = WebRtcSocketBuilder::new(url.clone())
        .ice_server(ice)
        // Channel 0: ggrs (unreliable — it has its own reliability layer).
        .add_channel(ChannelConfig::unreliable())
        // Channel 1: the reliable side-channel (identity, rematch, goodbye).
        .add_channel(ChannelConfig::reliable())
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
        interrupted: false,
    });
    *world.resource_mut::<LobbyState>() = LobbyState::Connecting;

    tracing::info!(
        target: "two_top::net",
        room = %url,
        turn_relay = has_turn,
        "matchbox socket built — connecting",
    );
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
        // Post-swap: ggrs owns channel 0. Drain session events, pump the
        // reliable side-channel, and keep the silence timer fresh only
        // while ggrs reports a healthy link — during an interruption the
        // timer ages honestly and net's silence FSM raises OPPONENT AWAY.
        drain_session_events(world);
        pump_side_channel(world);
        return;
    }

    // --- Pre-swap: drive the lobby from socket peer-state ---
    // `try_`, not `update_peers`: once the signaling loop dies (server
    // unreachable after matchbox's built-in retries), the plain call
    // unwraps the closed channel and ABORTS the app — a phone with no
    // network crashed ~6 s after tapping FIND OPPONENT. A dead socket
    // reads as a refusal instead: drop the driver, park the lobby at
    // Idle. The SUMMONING overlay's stall diagnosis has already told the
    // player to check the connection; QUIT and PLAY THE BOT stay one tap
    // away.
    let polled = {
        let mut driver = world.non_send_resource_mut::<MatchboxDriver>();
        let our_id = driver.socket.id();
        driver.socket.try_update_peers().map(|u| (our_id, u))
    };
    let (our_id, peer_updates) = match polled {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(
                target: "two_top::net",
                error = %e,
                "signaling connection lost before pairing — abandoning the summons",
            );
            world.remove_non_send_resource::<MatchboxDriver>();
            *world.resource_mut::<LobbyState>() = LobbyState::Idle;
            return;
        }
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
            .take_channel(GGRS_CHANNEL)
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

    // Open the duel with the identity handshake: install-id + name on the
    // reliable channel. The peer's grudge ledger files this match under it.
    let profile = world.resource::<crate::profile::LocalProfile>().as_data();
    world
        .resource_mut::<NetSendQueue>()
        .0
        .push(NetMsg::Profile(profile));

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
    let mut interruption_edge: Option<bool> = None;
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
                        tracing::warn!(target: "two_top::net", ?addr, disconnect_timeout, "network interrupted — opponent away");
                        interruption_edge = Some(true);
                    }
                    GgrsEvent::NetworkResumed { addr } => {
                        tracing::info!(target: "two_top::net", ?addr, "network resumed — opponent back");
                        interruption_edge = Some(false);
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

    if let Some(interrupted) = interruption_edge {
        world.non_send_resource_mut::<MatchboxDriver>().interrupted = interrupted;
    }

    // Healthy session: keep silence ~0 so net's grace timer stays quiet.
    // During an interruption the timer ages honestly — net's silence FSM
    // flips the lobby to `Disconnected` (the OPPONENT AWAY overlay) after
    // ~1 s and recovers it the moment pinning resumes.
    if !world.non_send_resource::<MatchboxDriver>().interrupted {
        let frame = world.resource::<sim::FrameCount>().0;
        world.resource_mut::<LastPeerMessageFrame>().0 = frame;
    }

    if let Some(addr) = forfeited_peer {
        *world.resource_mut::<LobbyState>() = LobbyState::Forfeited {
            peer_id: addr_to_peer(addr),
        };
        *world.resource_mut::<sim::MatchState>() = sim::MatchState::MatchOver;
    }
}

/// Pump the reliable side-channel: drain the app's outbound queue to the
/// peer, then handle inbound identity / rematch / goodbye messages.
fn pump_side_channel(world: &mut World) {
    // The peer to address: whichever the lobby currently holds.
    let peer = match world.resource::<LobbyState>() {
        LobbyState::Connected { peer_id } => Some(*peer_id),
        LobbyState::Disconnected { peer_id, .. } => Some(*peer_id),
        _ => None,
    };

    let outbound: Vec<NetMsg> = std::mem::take(&mut world.resource_mut::<NetSendQueue>().0);
    let inbound: Vec<(PeerId, Box<[u8]>)> = {
        let mut driver = world.non_send_resource_mut::<MatchboxDriver>();
        if let Some(peer) = peer {
            for msg in &outbound {
                driver
                    .socket
                    .channel_mut(SIDE_CHANNEL)
                    .send(encode_net_msg(msg), peer);
            }
        } else if !outbound.is_empty() {
            tracing::debug!(
                target: "two_top::net",
                dropped = outbound.len(),
                "side-channel messages dropped — no peer to address",
            );
        }
        driver.socket.channel_mut(SIDE_CHANNEL).receive()
    };

    for (from, bytes) in inbound {
        let Some(msg) = decode_net_msg(&bytes) else {
            tracing::warn!(target: "two_top::net", ?from, len = bytes.len(), "unreadable side-channel message ignored");
            continue;
        };
        match msg {
            NetMsg::Profile(profile) => {
                tracing::info!(
                    target: "two_top::net",
                    install_id = format_args!("{:032x}", profile.install_id),
                    "peer profile received",
                );
                world.resource_mut::<PeerProfile>().0 = Some(profile);
            }
            NetMsg::RematchWant => {
                world.resource_mut::<RematchConsent>().peer = true;
            }
            NetMsg::Bye => {
                tracing::info!(target: "two_top::net", ?from, "peer said goodbye — forfeit without the grace wait");
                *world.resource_mut::<LobbyState>() = LobbyState::Forfeited { peer_id: from };
                *world.resource_mut::<sim::MatchState>() = sim::MatchState::MatchOver;
            }
        }
    }
}

/// When did this app last go away — a suspend, a focus loss, or a frozen
/// main loop (Android holds the whole process while backgrounded, so a
/// giant `Time<Real>` delta IS the suspension telling on itself). Stored
/// as `Time<Real>` elapsed seconds. Consumed by the grudge ledger to
/// decide who owns a forfeit: if we went absent just before the match
/// forfeited, the walk-out (and the loss) is ours.
#[derive(Resource, Default, Clone, Copy, Debug)]
pub struct RecentAbsence(pub Option<f32>);

impl RecentAbsence {
    /// How close (seconds) an absence must be to a forfeit to take the
    /// blame for it. Comfortably wider than the 9 s disconnect grace.
    pub const FORFEIT_BLAME_SECS: f32 = 20.0;

    pub fn within(&self, now: f32, window: f32) -> bool {
        self.0.is_some_and(|at| now - at <= window)
    }
}

/// A frozen frame this long means the OS held the process (or the window
/// manager starved us) — either way, the peer watched us vanish.
const ABSENCE_FREEZE_SECS: f32 = 2.0;

/// Track absences: window focus loss and main-loop freezes.
pub fn track_absence(
    time: Res<Time<Real>>,
    mut focus_events: MessageReader<bevy::window::WindowFocused>,
    mut absence: ResMut<RecentAbsence>,
) {
    let now = time.elapsed_secs();
    if time.delta_secs() > ABSENCE_FREEZE_SECS {
        absence.0 = Some(now);
    }
    for ev in focus_events.read() {
        if !ev.focused {
            absence.0 = Some(now);
        }
    }
}

/// The ONLINE rematch gate (`ReadInputs`, after every input source): during
/// `MatchOver` the local THROW press becomes rematch CONSENT instead of an
/// instant restart. The press is masked off the wire and `RematchWant` goes
/// to the peer on the reliable channel; once both sides have consented the
/// gate emits the real THROW input and `sim::apply_rematch` restarts the
/// match exactly as it always has — input-driven, rollback-correct, no
/// out-of-band state writes (CONVENTIONS § match transitions).
///
/// This is pre-wire input shaping, the legal kind: it changes what WE
/// choose to press, never what a received input means. Couch and practice
/// keep the classic instant rematch; the theater plays tapes and needs no
/// gate at all.
#[allow(clippy::too_many_arguments)]
pub fn gate_rematch_inputs(
    netplay: Res<NetplayConfig>,
    practice: Res<crate::bot::PracticeMode>,
    theater: Res<crate::theater::TheaterMode>,
    state: Res<sim::MatchState>,
    local: Res<LocalPlayerHandle>,
    mut consent: ResMut<RematchConsent>,
    mut queue: ResMut<NetSendQueue>,
    inputs: Option<ResMut<bevy_ggrs::LocalInputs<GgrsCfg>>>,
) {
    if netplay.room_url.is_none() || practice.0 || theater.active() {
        return;
    }
    if !matches!(*state, sim::MatchState::MatchOver) {
        return;
    }
    let Some(mut inputs) = inputs else {
        return;
    };
    let Some(handle) = local.0 else {
        return;
    };
    let Some(input) = inputs.0.get_mut(&handle) else {
        return;
    };
    let pressed = input.buttons & sim::PlayerInput::THROW_DOWN != 0;
    if pressed && !consent.local {
        consent.local = true;
        queue.0.push(NetMsg::RematchWant);
        tracing::info!(target: "two_top::net", "rematch consent given — telling the peer");
    }
    if consent.local && consent.peer {
        // Both in: emit the real input. The preceding masked frames
        // guarantee sim sees a rising edge.
        input.buttons |= sim::PlayerInput::THROW_DOWN;
    } else {
        input.buttons &= !sim::PlayerInput::THROW_DOWN;
    }
}

/// Clear rematch consent whenever the sim is not sitting on a summary —
/// covers the restart itself (MatchOver → Countdown) and every other path
/// out of the screen.
pub fn reset_rematch_consent(state: Res<sim::MatchState>, mut consent: ResMut<RematchConsent>) {
    if !matches!(*state, sim::MatchState::MatchOver) && *consent != RematchConsent::default() {
        *consent = RematchConsent::default();
    }
}

/// Tear down the online session cleanly: tell the peer goodbye (so their
/// screen flips to OPPONENT LEFT immediately instead of waiting out the
/// grace), drop the socket + session, and reset every per-match netplay
/// resource. The caller flips `AppScreen` back to Title.
pub fn leave_online_match(world: &mut World) {
    // Best-effort goodbye straight into the channel — the send queue won't
    // get another pump after the driver is removed.
    let peer = match world.resource::<LobbyState>() {
        LobbyState::Connected { peer_id } => Some(*peer_id),
        LobbyState::Disconnected { peer_id, .. } => Some(*peer_id),
        _ => None,
    };
    // The removal is unconditional (dropping the socket ends the message
    // loop); the goodbye rides out only when a peer is still addressable.
    let mut driver = world.remove_non_send_resource::<MatchboxDriver>();
    if let Some(driver) = driver.as_mut()
        && let Some(peer) = peer
        && driver.channel_taken
    {
        driver
            .socket
            .channel_mut(SIDE_CHANNEL)
            .send(encode_net_msg(&NetMsg::Bye), peer);
    }
    drop(driver);
    world.remove_resource::<Session<GgrsCfg>>();
    *world.resource_mut::<LobbyState>() = LobbyState::Idle;
    world.resource_mut::<PendingP2PSwap>().0 = None;
    world.resource_mut::<PeerProfile>().0 = None;
    *world.resource_mut::<RematchConsent>() = RematchConsent::default();
    world.resource_mut::<NetSendQueue>().0.clear();
    world.resource_mut::<LocalPlayerHandle>().0 = None;
    world.resource_mut::<render::PerspectiveFlip>().0 = 1.0;
    tracing::info!(target: "two_top::net", "left the online match — lobby reset");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ice_response_parses_into_matchbox_config() {
        let body = r#"{"urls":["stun:stun.l.google.com:19302","turn:relay.example.com:3478?transport=udp"],"username":"1700000000:twotop","credential":"5qrcx5XkYi6dvbeiSMJF7UgDbag=","ttl_secs":14400}"#;
        let config = parse_ice_response(body).expect("valid vendor response parses");
        assert_eq!(config.urls.len(), 2);
        assert!(config.urls[1].starts_with("turn:"));
        assert_eq!(config.username.as_deref(), Some("1700000000:twotop"));
        assert!(config.credential.is_some());
    }

    #[test]
    fn stun_only_vendor_answers_parse_without_credentials() {
        let body = r#"{"urls":["stun:stun.l.google.com:19302"],"username":null,"credential":null,"ttl_secs":60}"#;
        let config = parse_ice_response(body).expect("stun-only is a valid answer");
        assert!(config.username.is_none() && config.credential.is_none());
    }

    #[test]
    fn garbage_and_empty_responses_fall_back() {
        // Malformed JSON, wrong shape, and an empty url list all read as
        // "vendor unusable" — the caller takes the baked/STUN-only path.
        assert!(parse_ice_response("not json").is_none());
        assert!(parse_ice_response(r#"{"nope":true}"#).is_none());
        assert!(
            parse_ice_response(r#"{"urls":[],"username":null,"credential":null,"ttl_secs":9}"#)
                .is_none()
        );
    }
}
