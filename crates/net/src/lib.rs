//! Phase 12 networking — matchbox WebRTC + ggrs bridge.
//!
//! `matchbox_socket` 0.14 ships an optional `ggrs` cargo feature that
//! glues `WebRtcChannel` into ggrs's `NonBlockingSocket` trait, but it
//! pins `ggrs ^0.11`. Our `bevy_ggrs = "=0.21"` pulls `ggrs ^0.12`,
//! which is incompatible per cargo semver (0.x bumps are breaking).
//!
//! Rather than fork matchbox_socket or downgrade bevy_ggrs (which
//! would re-validate Phases 8/9/10/11 against an older API and almost
//! certainly drift the determinism baseline), we own the ~50 LOC of
//! bridge code here. This is the same playbook we used to cut
//! `bevy_roll_safe` in Phase 11. See MORGAN_NOTES § "Why we cut
//! bevy_roll_safe" for the broader rationale on owning small adapters.
//!
//! The bridge is a transparent wrapper around `WebRtcChannel` that
//! impls `NonBlockingSocket<sim::NetAddr>` by:
//!   1. bincode-serializing each outbound `ggrs::Message` to a
//!      `Bytes` packet and forwarding to `WebRtcChannel::send`.
//!   2. draining inbound `(PeerId, Packet)` pairs and
//!      bincode-deserializing each packet back to `ggrs::Message`.
//!
//! This matches matchbox's own reference impl byte-for-byte at the
//! wire level, just bound to ggrs 0.12 instead of 0.11.
//!
//! ## Address type: `sim::NetAddr`, not `PeerId`
//!
//! ggrs's `Config::Address` is `sim::NetAddr(u128)`, a neutral handle
//! that keeps the `sim` crate free of any networking dependency
//! (CONVENTIONS: the determinism core is headless). Matchbox identifies
//! peers with `PeerId(Uuid)`; the bridge converts at the ggrs boundary
//! via a trivial bijection — `PeerId(Uuid::from_u128(addr.0))` outbound
//! and `NetAddr(peer.0.as_u128())` inbound. A `Uuid` is exactly 128 bits,
//! so the round-trip is lossless and total.

use bevy::prelude::*;
use ggrs::{Message, NonBlockingSocket};
pub use matchbox_socket::Packet;
use matchbox_socket::{PeerId, WebRtcChannel};
use serde::{Deserialize, Serialize};
use sim::NetAddr;
use uuid::Uuid;

// Re-export the matchbox types the app crate's live driver needs, so the
// driver can build/poll a socket without declaring its own
// `matchbox_socket` dependency (which would risk feature-set drift from
// ours). The session-swap itself lives in `app` because it constructs a
// `bevy_ggrs::Session`, and `net` deliberately depends only on raw `ggrs`
// (no bevy_ggrs) to stay headless.
pub use matchbox_socket::{
    ChannelConfig, MessageLoopFuture, PeerId as MatchboxPeerId, PeerState, RtcIceServerConfig,
    WebRtcSocket, WebRtcSocketBuilder,
};

/// Convert a matchbox `PeerId` to the neutral `sim::NetAddr` used at the
/// ggrs `Config::Address` boundary. A `Uuid` is 128 bits, so this is
/// lossless.
#[inline]
pub fn peer_to_addr(peer: PeerId) -> NetAddr {
    NetAddr(peer.0.as_u128())
}

/// Inverse of [`peer_to_addr`]: rebuild the matchbox `PeerId` from a
/// `sim::NetAddr` so ggrs's per-peer routing can be handed back to
/// `WebRtcChannel::send`.
#[inline]
pub fn addr_to_peer(addr: NetAddr) -> PeerId {
    PeerId(Uuid::from_u128(addr.0))
}

/// Newtype wrapper that owns a `WebRtcChannel` and exposes ggrs's
/// `NonBlockingSocket` interface. Construct one per ggrs session,
/// per channel — typically the unreliable channel. ggrs implements
/// its own retransmission and out-of-order tolerance, so reliable
/// channels are wasteful (matchbox warns about this in their
/// reference impl; we mirror the warning for parity).
pub struct MatchboxBridge {
    channel: WebRtcChannel,
    /// Bytes handed to the channel so far, shared with whoever wants the
    /// link's bandwidth ([`MatchboxBridge::bytes_sent`]).
    bytes_sent: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// The socket under the channel has ended; logged once, then quiet.
    link_lost_logged: bool,
    /// Debug builds only (`TWOTOP_FAULT`): drop this side's outgoing
    /// checksum reports, so only the OTHER side's ggrs can detect a desync
    /// this side causes. A one-sided desync on demand, to prove the peer
    /// that did not detect it still hears of it.
    #[cfg(debug_assertions)]
    mute_checksum_reports: bool,
}

/// The `MessageBody` variant index of a ggrs `ChecksumReport` (declaration
/// order in ggrs's `network/messages.rs`).
#[cfg(debug_assertions)]
const GGRS_CHECKSUM_REPORT: u8 = 6;

/// Is this encoded ggrs message a checksum report? The body is private to
/// ggrs, so read the wire: bincode's standard config writes the header's
/// `u16` magic as a varint (one byte below 251, else `251` and two bytes)
/// and then the body's variant index (one byte). Debug builds only.
#[cfg(debug_assertions)]
pub fn is_checksum_report(bytes: &[u8]) -> bool {
    let tag_at = match bytes.first() {
        Some(&b) if b < 251 => 1,
        Some(&251) => 3,
        _ => return false,
    };
    bytes.get(tag_at) == Some(&GGRS_CHECKSUM_REPORT)
}

impl MatchboxBridge {
    /// A live count of the bytes this bridge has sent: the session owns the
    /// bridge, so the counter is how the app still reads the link's rate.
    pub fn bytes_sent(&self) -> std::sync::Arc<std::sync::atomic::AtomicU64> {
        self.bytes_sent.clone()
    }

    /// Construct a bridge over an already-connected `WebRtcChannel`.
    /// The caller is responsible for spinning up the matchbox socket
    /// and signaling exchange — this wrapper only handles the
    /// frame-by-frame send/receive once the channel is open.
    pub fn new(channel: WebRtcChannel) -> Self {
        if channel.config().max_retransmits != Some(0) || channel.config().ordered {
            tracing::warn!(
                target: "two_top::net",
                config = ?channel.config(),
                "matchbox channel is reliable or ordered; ggrs has its own \
                 reliability layer and performs better on an unreliable+\
                 unordered channel. See SIGNALING.md for the recommended \
                 channel config.",
            );
        }
        Self {
            channel,
            bytes_sent: Default::default(),
            link_lost_logged: false,
            #[cfg(debug_assertions)]
            mute_checksum_reports: false,
        }
    }

    /// Debug builds only: stop sending checksum reports
    /// ([`MatchboxBridge::mute_checksum_reports`]).
    #[cfg(debug_assertions)]
    pub fn mute_checksum_reports(mut self) -> Self {
        self.mute_checksum_reports = true;
        self
    }

    /// Surrender the wrapped channel — useful for tests or for
    /// shutdown paths that need to drain the socket directly.
    pub fn into_inner(self) -> WebRtcChannel {
        self.channel
    }
}

/// Encode a `ggrs::Message` to a wire packet via bincode + serde.
/// Mirrors matchbox's reference encoder so the on-the-wire format is
/// identical to what a peer running matchbox's `ggrs` feature would
/// produce. Compatibility with matchbox 0.14 is incidental but worth
/// preserving.
pub fn encode_message(msg: &Message) -> Packet {
    bincode::serde::encode_to_vec(msg, bincode::config::standard())
        .expect("ggrs Message serialization must not fail — Message is Serialize-by-derive")
        .into()
}

// ---- Wire-format guard for ggrs channel 0 ----
//
// ggrs 0.12 trusts its own wire. `to_player_inputs` (network/protocol.rs)
// `expect`s the bincode decode of every per-player slice, and the delta
// decoder (network/compression.rs) sizes each frame from a two-byte length
// prefix that the REMOTE wrote. So a peer whose `Input` stream decodes to
// a three-byte frame panics this process inside `poll_remote_clients`, on
// the main thread — from the public quick-match room, where the peer is
// whoever queued next. `MessageBody` is `pub(crate)` in ggrs, so nothing
// can inspect a decoded `Message`; the guard instead decodes a private
// mirror of the same layout and refuses the packet before ggrs sees it.
// The mirror's layout is pinned by `input_stream_tests`: bytes the mirror
// emits must decode as a real `ggrs::Message`, or the guard is inert.
//
// The RLE layer underneath is hostile-input-unsafe too, and ggrs calls it
// on every Input it receives: `bitfield_rle` (via `varinteger`) reads
// `buf[off]` past the end of a stream that ends on a varint continuation
// byte, and sizes its output allocation straight from the remote's repeat
// count (`vec![0; 2^30]` from six bytes). `rle_decoded_len` vets the
// stream with checked arithmetic and hard bounds BEFORE the crate's
// decoder runs, so the guard is the first — and only — caller that ever
// hands it something it cannot survive.
//
// Not closable at this boundary: protocol.rs also `assert!`ed
// `last_recv_frame + 1 >= body.start_frame` on the wire's own `start_frame`,
// which needs ggrs's private receive bookkeeping to check. That one is
// fixed in ggrs itself: the vendored copy (vendor/README.md) drops such a
// packet with a warning (`an_input_that_skips_ahead_is_dropped_not_fatal`).

/// One player's input on the wire, as ggrs sizes it: the bincode-1 fixint
/// encoding of `sim::PlayerInput`, four one-byte fields. `size_of` is
/// exact here because the type is `repr(C)` + `Pod`; the equivalence is
/// pinned by `input_stream_tests::one_input_on_the_wire_is_four_bytes`.
const INPUT_BYTES_PER_PLAYER: usize = std::mem::size_of::<sim::PlayerInput>();

/// Players each remote endpoint speaks for. 2-Top seats one player per
/// phone, and ggrs sizes a frame as `bytes_per_player × handles.len()` on
/// both ends, so this is the one number the guard must agree with it on.
const PLAYERS_PER_PEER: usize = 1;

/// Players in a session: what ggrs sizes its connection-status table by,
/// and the length it indexes a remote Input's list up to. 2-Top is a duel.
const SESSION_PLAYERS: usize = 2;

/// The exact frame width an honest peer sends, every frame.
const INPUT_FRAME_BYTES: usize = INPUT_BYTES_PER_PLAYER * PLAYERS_PER_PEER;

#[allow(dead_code)] // consumed positionally by serde; only `bytes` is read by hand
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct WireHeader {
    magic: u16,
}

#[allow(dead_code)] // consumed positionally by serde; only `bytes` is read by hand
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct WireConnStatus {
    disconnected: bool,
    last_frame: i32,
}

#[allow(dead_code)] // consumed positionally by serde; only `bytes` is read by hand
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct WireSyncRequest {
    random_request: u32,
}

#[allow(dead_code)] // consumed positionally by serde; only `bytes` is read by hand
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct WireSyncReply {
    random_reply: u32,
}

#[allow(dead_code)] // consumed positionally by serde; only `bytes` is read by hand
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct WireInput {
    peer_connect_status: Vec<WireConnStatus>,
    disconnect_requested: bool,
    start_frame: i32,
    ack_frame: i32,
    bytes: Vec<u8>,
}

#[allow(dead_code)] // consumed positionally by serde; only `bytes` is read by hand
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct WireInputAck {
    ack_frame: i32,
}

#[allow(dead_code)] // consumed positionally by serde; only `bytes` is read by hand
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct WireQualityReport {
    frame_advantage: i16,
    ping: u128,
}

#[allow(dead_code)] // consumed positionally by serde; only `bytes` is read by hand
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct WireQualityReply {
    pong: u128,
}

#[allow(dead_code)] // consumed positionally by serde; only `bytes` is read by hand
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct WireChecksumReport {
    checksum: u128,
    frame: i32,
}

/// `ggrs::network::messages::MessageBody`, variant for variant, in the
/// same order: bincode encodes the discriminant by index, so the order IS
/// the format.
#[allow(dead_code)] // consumed positionally by serde; only `bytes` is read by hand
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
enum WireBody {
    SyncRequest(WireSyncRequest),
    SyncReply(WireSyncReply),
    Input(WireInput),
    InputAck(WireInputAck),
    QualityReport(WireQualityReport),
    QualityReply(WireQualityReply),
    ChecksumReport(WireChecksumReport),
    KeepAlive,
}

/// `ggrs::Message`, field for field.
#[allow(dead_code)] // consumed positionally by serde; only `bytes` is read by hand
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct WireMessage {
    header: WireHeader,
    body: WireBody,
}

/// Bound for the RLE-decoded delta stream. An honest packet is six bytes
/// per pending frame and ggrs's pending window is tens of frames; this is
/// over a thousand frames of slack — far above honest, far below harm.
const MAX_DELTA_STREAM_BYTES: usize = 8 * 1024;

/// Longest varint the vet accepts, in bytes. Four bytes is 2^28: every
/// run length an honest ≤8 KiB stream can carry fits in two, and staying
/// well under `varinteger`'s ten-byte u64 limit keeps both decoders in
/// the region where they agree exactly and nothing overflows.
const MAX_RLE_VARINT_BYTES: u32 = 4;

/// Vet an RLE stream the way `bitfield_rle::decode_len_with_offset` will
/// walk it — varint header, low bit = repeat run, else a literal run of
/// that many bytes follows — with every read bounds-checked and every
/// sum checked. Returns the decoded length, which is ≤ the cap. Once this
/// passes, the crate's own decoder cannot index past the buffer, overflow
/// a shift, or allocate more than the cap.
fn rle_decoded_len(buf: &[u8]) -> Result<usize, &'static str> {
    let mut offset = 0usize;
    let mut total = 0usize;
    while offset < buf.len() {
        let mut value = 0u64;
        let mut shift = 0u32;
        loop {
            let Some(&byte) = buf.get(offset) else {
                return Err("truncated varint");
            };
            offset += 1;
            if shift >= 7 * MAX_RLE_VARINT_BYTES {
                return Err("varint too long");
            }
            value |= u64::from(byte & 0x7f) << shift;
            shift += 7;
            if (byte & 0x80) == 0 {
                break;
            }
        }
        let repeat = (value & 1) == 1;
        let run = if repeat { value >> 2 } else { value >> 1 };
        let run = usize::try_from(run).map_err(|_| "run too long")?;
        total = total.checked_add(run).ok_or("run too long")?;
        if total > MAX_DELTA_STREAM_BYTES {
            return Err("delta stream too long");
        }
        if !repeat {
            offset = offset
                .checked_add(run)
                .ok_or("literal run overruns stream")?;
            if offset > buf.len() {
                return Err("literal run overruns stream");
            }
        }
    }
    Ok(total)
}

/// Walk an `Input` body's stream exactly the way ggrs's decoder will —
/// RLE first, then a `u16` little-endian length prefix per frame — and
/// refuse it unless every frame is exactly [`INPUT_FRAME_BYTES`] wide.
/// Returns the frame count on success, for the log.
fn input_stream_frames(input: &WireInput) -> Result<usize, &'static str> {
    rle_decoded_len(&input.bytes)?;
    let raw = bitfield_rle::decode(&input.bytes).map_err(|_| "rle stream undecodable")?;
    let mut pos = 0;
    let mut frames = 0;
    while pos < raw.len() {
        let Some(prefix) = raw.get(pos..pos + 2) else {
            return Err("truncated length prefix");
        };
        let len = usize::from(u16::from_le_bytes([prefix[0], prefix[1]]));
        pos += 2;
        if len != INPUT_FRAME_BYTES {
            return Err("frame is not one input wide");
        }
        if raw.get(pos..pos + len).is_none() {
            return Err("truncated input data");
        }
        pos += len;
        frames += 1;
    }
    Ok(frames)
}

/// `Some(why)` if handing this packet to ggrs would panic it. Bytes the
/// mirror cannot decode pass through as `None`: ggrs's own decode of the
/// same layout fails next and drops them with the existing log. The guard
/// refuses only what is well-formed AND lethal — it is never a second
/// parser that could take netplay down if it drifted.
fn refuse_reason(bytes: &[u8]) -> Option<&'static str> {
    let decoded =
        bincode::serde::decode_from_slice::<WireMessage, _>(bytes, bincode::config::standard());
    let Ok((wire, _)) = decoded else {
        return None;
    };
    match wire.body {
        // ggrs reads one connection status per player out of the body
        // (`body.peer_connect_status[i]`, unchecked) unless the peer is
        // asking to disconnect: a shorter list was an index panic.
        WireBody::Input(input)
            if !input.disconnect_requested && input.peer_connect_status.len() < SESSION_PLAYERS =>
        {
            Some("connection status list shorter than the player count")
        }
        WireBody::Input(input) => input_stream_frames(&input).err(),
        _ => None,
    }
}

/// Decode a packet back into `(NetAddr, ggrs::Message)`, or `None` for
/// bytes that don't parse. Peers that send malformed bincode are either
/// lying or running an incompatible build; the matchbox reference impl
/// panics on them, but this app has a better answer than aborting
/// mid-duel — drop the packet and let the silence FSM score the abuser
/// as the walk-away ([`DISCONNECT_AFTER_FRAMES`] →
/// [`FORFEIT_AFTER_FRAMES`]). The public room pairs strangers, and no
/// stranger's bytes get to crash the table. A well-formed `Message` whose
/// `Input` stream would panic ggrs's own decoder is refused the same way
/// by the wire-format guard above (see that section for the one upstream
/// assert it cannot reach). The inbound `PeerId` is mapped to the neutral
/// `sim::NetAddr` ggrs expects (see module docs).
pub fn decode_packet(message: (PeerId, Packet)) -> Option<(NetAddr, Message)> {
    let (peer, bytes) = message;
    if let Some(reason) = refuse_reason(&bytes) {
        tracing::warn!(
            target: "two_top::net",
            peer = %peer.0,
            len = bytes.len(),
            reason,
            "ggrs input packet that would panic the session dropped",
        );
        return None;
    }
    match bincode::serde::decode_from_slice(&bytes, bincode::config::standard()) {
        Ok((msg, _)) => Some((peer_to_addr(peer), msg)),
        Err(e) => {
            tracing::warn!(
                target: "two_top::net",
                peer = %peer.0,
                len = bytes.len(),
                error = %e,
                "undecodable ggrs packet dropped",
            );
            None
        }
    }
}

impl NonBlockingSocket<NetAddr> for MatchboxBridge {
    /// Never panics. matchbox's `WebRtcChannel::send` does (`"Send
    /// failed"`) once the socket's future has ended — and it ends for the
    /// whole socket, data channels included, the moment the signaling
    /// websocket closes after pairing: the room server redeploying, or a
    /// phone moving from Wi-Fi to cellular. ggrs sends every frame, so
    /// every match in progress used to crash on the next frame. A failed
    /// send is a packet lost on a dead link; the app sees the closed
    /// socket and ends the match (`netplay`'s link-lost check).
    fn send_to(&mut self, msg: &Message, addr: &NetAddr) {
        let packet = encode_message(msg);
        #[cfg(debug_assertions)]
        if self.mute_checksum_reports && is_checksum_report(&packet) {
            return;
        }
        let len = packet.len() as u64;
        if self.channel.try_send(packet, addr_to_peer(*addr)).is_ok() {
            self.bytes_sent
                .fetch_add(len, std::sync::atomic::Ordering::Relaxed);
        } else if !self.link_lost_logged {
            self.link_lost_logged = true;
            tracing::warn!(target: "two_top::net", "ggrs channel closed under the session — link lost");
        }
    }

    fn receive_all_messages(&mut self) -> Vec<(NetAddr, Message)> {
        self.channel
            .receive()
            .into_iter()
            .filter_map(decode_packet)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// The reliable side-channel: everything the duel needs that is NOT sim input.
//
// ggrs owns matchbox channel 0 (unreliable — it has its own reliability
// layer). Channel 1 is a RELIABLE matchbox channel carrying small postcard
// messages: the identity handshake (install-id + name, the thing the grudge
// ledger's per-opponent rivalry has been parked on), rematch consent, and a
// clean goodbye so the peer never has to wait out the silence grace to learn
// we left. None of this ever enters the sim — the messages change what a
// client SHOWS and when it chooses to emit its own inputs, never how a tick
// resolves.
// ---------------------------------------------------------------------------

/// Longest name the wire carries. 12 is the field's own answer: it is
/// Xbox's gamertag ceiling, and 8-12 is the band that fits nearly every
/// platform's limit — long enough to be a name people actually own,
/// short enough to render on a phone HUD without wrapping.
pub const NAME_MAX: usize = 12;

/// The wire generation of everything `net` sends that the sim does not
/// see (the side channel, the handshake). Bump it on any such change; it
/// rides the room name beside `sim::SIM_VERSION`, so two builds that
/// cannot talk never meet. 2: one keyed `Profile` (the unkeyed one and
/// `Profile2` are gone).
pub const NET_PROTOCOL: u32 = 2;

/// Tail padding for names shorter than [`NAME_MAX`]. A fixed array keeps
/// `ProfileData` `Copy`, allocation-free, and bounded BY CONSTRUCTION — a
/// hostile peer cannot hand us a 10 MB name, because there is nowhere to
/// put one.
pub const NAME_EMPTY: u8 = 0xFF;

/// Build a padded wire name from a run of glyph indices — the shape
/// `ProfileData` wants, without spelling out the tail at every call site.
pub fn name_slots(indices: &[u8]) -> [u8; NAME_MAX] {
    let mut slots = [NAME_EMPTY; NAME_MAX];
    for (slot, i) in slots.iter_mut().zip(indices) {
        *slot = *i;
    }
    slots
}

/// A peer's shareable identity. `install_id` is a random u128 minted once
/// per install and persisted — matchbox `PeerId`s are ephemeral per
/// connection, so this is the durable key the rivalry ledger files a peer
/// under, and the only thing here that is ever trusted. `name` is opaque
/// glyph indices padded with [`NAME_EMPTY`]; the app clamps them into its
/// alphabet at render time, so a malicious value can at worst display a
/// wrong letter.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ProfileData {
    pub install_id: u128,
    pub name: [u8; NAME_MAX],
}

/// Messages on the reliable side-channel. postcard-encoded (the project's
/// serialization convention; the bincode above exists only to mirror
/// matchbox's ggrs reference codec byte-for-byte).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum NetMsg {
    /// Identity handshake, sent once right after the P2P session swap:
    /// install-id, name and the result-signing pubkey.
    Profile(KeyedProfile),
    /// "I want to run it back." The receiving client surfaces it on the
    /// summary; when both sides have consented, each client emits its own
    /// THROW input and the in-sim `apply_rematch` restarts the match the
    /// same rollback-correct way it always has.
    RematchWant,
    /// "I quit a live duel": a concession, sent only by QUIT mid-match once
    /// the duel has begun (the quitter files the loss first). The receiver
    /// books the win at once instead of waiting out the disconnect grace.
    /// Every other way of leaving says [`NetMsg::Ended`].
    Bye,
    /// Our ed25519 signature over [`MatchStatement::encode`] for the match
    /// that just reached the score threshold. Split halves keep the enum
    /// `Copy` (serde's array impls stop at 32).
    MatchSig {
        sig: [[u8; 32]; 2],
    },
    /// The table this side would play on (`sim::ArenaId::as_u8`). Sent
    /// once, BEFORE the ggrs session exists, the moment the peer connects:
    /// the table no longer rides the room name, so the two sides agree on
    /// it here instead — both adopt the lower peer id's pick, then start
    /// the session. Reliable + ordered, so it always lands ahead of the
    /// peer's `Profile`.
    Table {
        arena: u8,
    },
    /// "This phone is going away for a moment" — the app is being
    /// backgrounded (a notification-shade peek, a call, a switch to the
    /// chat app) or its tab hidden. Sent on the last update before it
    /// sleeps. The receiver gives it [`PAUSED_FORFEIT_AFTER_FRAMES`] of
    /// silence instead of [`FORFEIT_AFTER_FRAMES`] before forfeiting, and
    /// says so on screen. An old build cannot decode it and ignores it,
    /// which is the old behaviour.
    Pausing,
    /// "This session is over for me", for every ending that is not a
    /// concession: leaving a decided match's summary, cancelling before
    /// the duel began, forfeiting a silent peer, or a desync this side
    /// detected (so both phones void it, not only the one whose checksum
    /// check fired). Honoured only from the session's peer, and only
    /// before the receiver's own result has settled; it can only close a
    /// match unfinished or void it, which silence or a forged checksum
    /// already could.
    Ended {
        reason: EndReason,
    },
    /// Round-trip probe in the table handshake; answered with
    /// [`NetMsg::Pong`] carrying the same `id`.
    Ping {
        id: u8,
    },
    Pong {
        id: u8,
    },
    /// The input delay this side proposes ([`delay::delay_for_rtt`] of its
    /// median round trip); both sides play [`delay::agreed`].
    Delay {
        frames: u8,
    },
}

/// Why a session ended, in a [`NetMsg::Ended`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum EndReason {
    /// The sender's ggrs found the two sims disagreeing at `frame`.
    Desynced { frame: u32 },
    /// The sender left or gave up on the session.
    Closed,
}

/// Encode a side-channel message. Infallible for these types (postcard on
/// plain enums/structs cannot fail without allocation failure).
pub fn encode_net_msg(msg: &NetMsg) -> Packet {
    postcard::to_allocvec(msg)
        .expect("NetMsg postcard encoding cannot fail")
        .into()
}

/// Decode a side-channel message. Unlike the ggrs channel (where malformed
/// bytes mean an incompatible build and panicking is honest), the side
/// channel tolerates strangers: a peer running a newer protocol just has
/// its unknown messages ignored.
pub fn decode_net_msg(bytes: &[u8]) -> Option<NetMsg> {
    postcard::from_bytes(bytes).ok()
}

/// Input delay for an online session, from the link it will run over.
///
/// A peer's input delay is what spares the OTHER screen rollbacks of that
/// peer's duelist: an input filed `delay` frames ahead arrives before the
/// frame it belongs to whenever the one-way trip is shorter than the delay.
/// Each side measures the round trip in the table handshake
/// ([`NetMsg::Ping`]), proposes [`delay_for_rtt`], and both play the larger
/// of the two proposals ([`NetMsg::Delay`]). ggrs fixes the delay when the
/// session is built and cannot change it after.
pub mod delay {
    /// The least delay ever used: the same Wi-Fi.
    pub const MIN: usize = 2;
    /// What a session uses when the round trip could not be measured.
    pub const DEFAULT: usize = 3;
    /// The most: past it the delay is felt in the hands, and rollback takes
    /// over the rest.
    pub const MAX: usize = 4;

    /// The delay a round trip of `rtt_ms` asks for: the one-way trip plus
    /// 8 ms of jitter, in 60 Hz frames, rounded up and clamped
    /// (LAN → 2, ~60 ms → 3, ~100 ms → 4).
    pub fn delay_for_rtt(rtt_ms: u32) -> usize {
        // ceil((rtt / 2 + 8) / (1000 / 60)) in integers.
        let frames = (u64::from(rtt_ms) * 30 + 480).div_ceil(1000) as usize;
        frames.clamp(MIN, MAX)
    }

    /// The median of the measured round trips (`None`: none came back).
    pub fn median_rtt(samples: &[u32]) -> Option<u32> {
        let mut sorted = samples.to_vec();
        sorted.sort_unstable();
        sorted.get(sorted.len() / 2).copied()
    }

    /// The delay both sides play: the larger proposal, so neither side's
    /// link is under-covered. Clamped, so a hostile proposal cannot ask for
    /// a sluggish match.
    pub fn agreed(ours: usize, theirs: usize) -> usize {
        ours.max(theirs).clamp(MIN, MAX)
    }
}

// ---- Signed results (NORTH N2) --------------------------------------------
//
// The install-id travels in the open on the side channel, so it identifies
// but cannot prove. An ed25519 keypair minted beside it can: pubkeys ride
// `NetMsg::Profile`, and when a match is decided ON SCORE both peers sign one
// canonical statement and swap signatures. The result becomes an artifact
// anyone can check — re-run the tape, rebuild the statement, verify both
// signatures (`replay_sync --attest`). Forfeits stay ledger-only: each
// client's lobby FSM observes the walk-away at its own frame, so there is
// no shared statement to sign.

/// A peer's shareable identity plus its result-signing pubkey.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct KeyedProfile {
    pub install_id: u128,
    pub name: [u8; NAME_MAX],
    /// ed25519 verifying key. The matching signing key never leaves the
    /// device that minted it.
    pub pubkey: [u8; 32],
}

impl KeyedProfile {
    /// The identity half, for everything that already speaks `ProfileData`.
    pub fn profile(&self) -> ProfileData {
        ProfileData {
            install_id: self.install_id,
            name: self.name,
        }
    }
}

// ---- Resume (a match continued after a dropped link) ----------------------
//
// When the transport under a live duel dies (a Wi-Fi to mobile handoff, the
// room server restarting, a long freeze), both phones dial a room named
// after the session that broke ([`resume_room`]) and rebuild the match from
// the inputs both have confirmed. These messages ride the reliable side
// channel of the NEW connection, before any session exists, behind a tag
// byte no `NetMsg` starts with, so a stray one can never be read as the
// other kind.

/// First byte of every encoded [`ResumeMsg`]. `NetMsg`'s postcard variant
/// index is its first byte and stays far below this.
pub const RESUME_TAG: u8 = 0xEE;

/// The resume handshake.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum ResumeMsg {
    /// "I was in the session `(lo, hi)` (its two original peer ids, sorted)
    /// and hold confirmed inputs for its first `confirmed` ticks."
    Hello { lo: u128, hi: u128, confirmed: u32 },
    /// Sent once both hellos are in: the inputs from tick `from` (the
    /// lower of the two confirmed counts) to the end of what this side
    /// holds, and [`inputs_hash`] of everything before `from`, which both
    /// must share.
    Tail {
        from: u32,
        prefix: u64,
        inputs: Vec<[sim::PlayerInput; 2]>,
    },
}

pub fn encode_resume(msg: &ResumeMsg) -> Packet {
    let mut bytes = vec![RESUME_TAG];
    bytes.extend(postcard::to_allocvec(msg).expect("ResumeMsg postcard encoding cannot fail"));
    bytes.into()
}

/// `None` for anything that is not a resume message. Never panics on peer
/// bytes.
pub fn decode_resume(bytes: &[u8]) -> Option<ResumeMsg> {
    match bytes.split_first() {
        Some((&RESUME_TAG, rest)) => postcard::from_bytes(rest).ok(),
        _ => None,
    }
}

/// A width-portable hash of an input stream (both phones compute it on
/// the prefix they must share).
pub fn inputs_hash(inputs: &[[sim::PlayerInput; 2]]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = portable_checksum_hasher();
    inputs.len().hash(&mut h);
    for pair in inputs {
        pair.hash(&mut h);
    }
    h.finish()
}

/// The room a broken session's two phones meet in again: named from the
/// session's sorted original peer ids, so only those two land there.
pub fn resume_room(lo: u128, hi: u128) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = portable_checksum_hasher();
    (lo, hi).hash(&mut h);
    format!("re{:016x}", h.finish())
}

/// Both sides' agreed input stream: `ours` (what this side holds) merged
/// with the peer's [`ResumeMsg::Tail`]. The shared prefix must hash the
/// same on both sides; the longer side's tail extends it. Pure.
pub fn merge_resume(
    ours: &[[sim::PlayerInput; 2]],
    from: u32,
    their_prefix: u64,
    their_tail: &[[sim::PlayerInput; 2]],
) -> Result<Vec<[sim::PlayerInput; 2]>, &'static str> {
    let from = from as usize;
    if from > ours.len() {
        return Err("the peer's prefix is longer than what we hold");
    }
    if inputs_hash(&ours[..from]) != their_prefix {
        return Err("the two records disagree before the break");
    }
    let our_tail = &ours[from..];
    // Where both hold a tick, they must hold the same one.
    let overlap = our_tail.len().min(their_tail.len());
    if our_tail[..overlap] != their_tail[..overlap] {
        return Err("the two records disagree after the break");
    }
    let longer = if their_tail.len() > our_tail.len() {
        their_tail
    } else {
        our_tail
    };
    let mut merged = ours[..from].to_vec();
    merged.extend_from_slice(longer);
    Ok(merged)
}

/// The width-portable hasher wrapper now lives in `sim`, beside the
/// checksum registrations that use it; re-exported here for
/// `replay_sync` and existing callers.
pub use sim::PortableHasher;

/// A fresh width-portable SeaHasher — the same hasher family
/// `bevy_ggrs::checksum_hasher` hands out, wrapped.
pub fn portable_checksum_hasher() -> PortableHasher<seahash::SeaHasher> {
    PortableHasher(seahash::SeaHasher::new())
}

/// One-shot width-portable hash of any `Hash` value. The `fn`-pointer
/// shape bevy_ggrs's `checksum_component`/`checksum_resource` want.
pub fn portable_hash64<T: core::hash::Hash>(value: &T) -> u64 {
    use core::hash::Hasher as _;
    let mut h = portable_checksum_hasher();
    value.hash(&mut h);
    h.finish()
}

pub const STATEMENT_MAGIC: [u8; 4] = *b"2TRS";
pub const STATEMENT_VERSION: u16 = 1;

/// One duelist's seat in a [`MatchStatement`]. `handle` records which ggrs
/// seat this identity played (both peers agree on it: lower matchbox
/// peer-id is handle 0), which is what lets a verifier map the statement's
/// scores onto a replayed tape's per-handle scores.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SeatStatement {
    pub install_id: u128,
    pub pubkey: [u8; 32],
    pub handle: u8,
    pub score: u8,
}

/// The canonical, dual-signable statement of a score-decided match. Both
/// peers must serialize identical bytes, so seats sort by install-id
/// (never by handle) and every field is either rollback-deterministic
/// (sim_version, arena, scores) or session-shared: the sorted matchbox
/// peer-id pair is the session nonce (an old signature cannot be replayed
/// for a new pairing), and `match_index` counts the matches this session
/// already decided (RUN IT BACK keeps the session; the index keeps each
/// rematch's statement distinct).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct MatchStatement {
    pub magic: [u8; 4],
    pub version: u16,
    pub sim_version: u32,
    pub arena_id: u8,
    pub session_low: u128,
    pub session_high: u128,
    pub match_index: u32,
    pub seat_low: SeatStatement,
    pub seat_high: SeatStatement,
}

impl MatchStatement {
    /// Build with seats and session ids in canonical order, whatever order
    /// the caller holds them in.
    pub fn new(
        sim_version: u32,
        arena_id: u8,
        session: (u128, u128),
        match_index: u32,
        seats: [SeatStatement; 2],
    ) -> Self {
        let (session_low, session_high) = if session.0 <= session.1 {
            (session.0, session.1)
        } else {
            (session.1, session.0)
        };
        let [a, b] = seats;
        let (seat_low, seat_high) = if a.install_id <= b.install_id {
            (a, b)
        } else {
            (b, a)
        };
        Self {
            magic: STATEMENT_MAGIC,
            version: STATEMENT_VERSION,
            sim_version,
            arena_id,
            session_low,
            session_high,
            match_index,
            seat_low,
            seat_high,
        }
    }

    /// The bytes both peers sign. Postcard is deterministic for a fixed
    /// struct layout, so equal statements encode to equal bytes.
    pub fn encode(&self) -> Vec<u8> {
        postcard::to_allocvec(self).expect("MatchStatement postcard encoding cannot fail")
    }

    /// Verify a split signature over this statement against `pubkey`.
    pub fn verify(&self, pubkey: &[u8; 32], sig: &[[u8; 32]; 2]) -> bool {
        use ed25519_dalek::{Signature, Verifier, VerifyingKey};
        let Ok(key) = VerifyingKey::from_bytes(pubkey) else {
            return false;
        };
        let mut bytes = [0u8; 64];
        bytes[..32].copy_from_slice(&sig[0]);
        bytes[32..].copy_from_slice(&sig[1]);
        key.verify(&self.encode(), &Signature::from_bytes(&bytes))
            .is_ok()
    }

    /// The seat that played ggrs handle `handle`, if the statement has one.
    pub fn seat_for_handle(&self, handle: u8) -> Option<&SeatStatement> {
        [&self.seat_low, &self.seat_high]
            .into_iter()
            .find(|s| s.handle == handle)
    }
}

/// Sign a statement with a raw 32-byte ed25519 signing key, split for the
/// `Copy` wire form.
pub fn sign_statement(stmt: &MatchStatement, signing_key: &[u8; 32]) -> [[u8; 32]; 2] {
    use ed25519_dalek::{Signer, SigningKey};
    let sig = SigningKey::from_bytes(signing_key)
        .sign(&stmt.encode())
        .to_bytes();
    let mut halves = [[0u8; 32]; 2];
    halves[0].copy_from_slice(&sig[..32]);
    halves[1].copy_from_slice(&sig[32..]);
    halves
}

/// The verifying key for a raw signing key — what `NetMsg::Profile` carries.
pub fn pubkey_for(signing_key: &[u8; 32]) -> [u8; 32] {
    use ed25519_dalek::SigningKey;
    SigningKey::from_bytes(signing_key)
        .verifying_key()
        .to_bytes()
}

/// 32 bytes as lowercase hex — the same greppable convention as the grudge
/// ledger's install-id keys.
pub fn hex32(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Inverse of [`hex32`].
pub fn from_hex32(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        out[i] = u8::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?;
    }
    Some(out)
}

/// A split signature as 128 hex chars, and back.
pub fn sig_to_hex(sig: &[[u8; 32]; 2]) -> String {
    format!("{}{}", hex32(&sig[0]), hex32(&sig[1]))
}

pub fn sig_from_hex(hex: &str) -> Option<[[u8; 32]; 2]> {
    if hex.len() != 128 {
        return None;
    }
    Some([from_hex32(&hex[..64])?, from_hex32(&hex[64..])?])
}

/// The on-disk attestation written beside a tape (`<stem>.attest.json`):
/// the statement plus both seats' signatures, hex-encoded so the file is
/// greppable and hand-checkable. `replay_sync --attest` is the reader.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Attestation {
    pub statement: MatchStatement,
    /// `seat_low`'s signature over `statement.encode()`, 128 hex chars.
    pub sig_low: String,
    /// `seat_high`'s signature, 128 hex chars.
    pub sig_high: String,
}

impl Attestation {
    /// Verify both signatures against their seats' own pubkeys.
    pub fn verify(&self) -> bool {
        let (Some(low), Some(high)) = (sig_from_hex(&self.sig_low), sig_from_hex(&self.sig_high))
        else {
            return false;
        };
        self.statement.verify(&self.statement.seat_low.pubkey, &low)
            && self
                .statement
                .verify(&self.statement.seat_high.pubkey, &high)
    }
}

/// The connected peer's result-signing pubkey, once its `NetMsg::Profile`
/// arrives.
/// Cleared on session teardown.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerKeys(pub Option<[u8; 32]>);

/// The peer's signature for the current decided match, once its
/// `MatchSig` arrives. Consumed by the app's attestation writer; cleared
/// when the sim leaves `MatchOver` and on session teardown.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerSig(pub Option<[[u8; 32]; 2]>);

/// The connected peer's identity, once its `Profile` message arrives.
/// Cleared on session teardown.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerProfile(pub Option<ProfileData>);

/// Rematch consent state for the current summary screen. `local` flips when
/// this player asks to run it back (their THROW press is converted into
/// consent while online), `peer` when the side-channel says the opponent
/// did. Both true → each client emits the real THROW input and the sim
/// restarts. Reset whenever the sim leaves `MatchOver`.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct RematchConsent {
    pub local: bool,
    pub peer: bool,
}

/// Outbound side-channel queue. App systems push; the matchbox driver
/// drains it into channel 1 each frame (it owns the non-send socket).
#[derive(Resource, Default, Debug)]
pub struct NetSendQueue(pub Vec<NetMsg>);

/// Phase 12 cycle 2: lobby state machine.
///
/// Drives the pre-match → in-match → post-disconnect lifecycle. The
/// state values name the operations a UI / signaling system would
/// observe; transitions happen elsewhere (cycle 3 wires
/// `Connecting → WaitingForPeer → Connected` against matchbox events;
/// cycle 4 wires `Connected → Disconnected → Forfeited` against the
/// 3-second grace timer).
///
/// Not rolled back. Lobby state is an out-of-band coordination layer
/// that lives outside the deterministic sim — it doesn't replay or
/// rollback. A terminal state (`Forfeited`, `Desynced`) is the trigger:
/// the app's `netplay::conclude_session` ends the match in the sim,
/// booking it as it stood on the last confirmed frame.
///
/// `since_frame` / `forfeit_at_frame` use the rolled-back
/// `FrameCount` from the sim crate so the disconnection countdown
/// composes with sim time rather than wall-clock — important for
/// reconnect logic that must agree across peers.
#[derive(Resource, Clone, PartialEq, Eq, Debug, Default)]
pub enum LobbyState {
    /// Pre-match. The "Find Match" button hasn't been clicked.
    #[default]
    Idle,
    /// `Find Match` clicked. WebSocket handshake to the signaling
    /// server is in flight.
    Connecting,
    /// Signaling server reachable, our `PeerId` is assigned, waiting
    /// for a remote peer to be paired with us. The `our_id` field is
    /// useful for HUD display + room-code coordination later.
    WaitingForPeer { our_id: PeerId },
    /// Paired with a peer. WebRTC datachannel is open.
    /// `peer_id` identifies the remote; the bridge's `MatchboxBridge`
    /// owns the actual `WebRtcChannel`.
    Connected { peer_id: PeerId },
    /// Datachannel went silent past the grace threshold during a
    /// match. `since_frame` records the last sim tick we heard from
    /// the peer — the disconnection countdown is
    /// `frame.0 - since_frame >= GRACE_FRAMES`.
    Disconnected { peer_id: PeerId, since_frame: u32 },
    /// The transport under a live duel died and both phones are dialing
    /// the session's resume room to continue it (`peer_id`: the original
    /// peer). Not terminal and not "in match": no session runs meanwhile.
    Reconnecting { peer_id: PeerId },
    /// The match ended without the sim deciding it. `conceded` is the
    /// fact the ledger can bank: true when the peer said goodbye (a
    /// deliberate quit — their app filed the loss before sending it),
    /// false when ggrs timed them out or the silence FSM fired, which
    /// proves nothing about who left. Both phones used to record a WIN
    /// for the latter; `grudge::match_outcome` records it as unfinished.
    Forfeited { peer_id: PeerId, conceded: bool },
    /// The two sims stopped agreeing (ggrs `DesyncDetected` at `frame`).
    /// Terminal like `Forfeited`, but nothing about this match is a
    /// shared fact any more: the app records no result, signs nothing,
    /// writes no tape, and refuses a rematch on the corrupt session.
    Desynced { peer_id: PeerId, frame: u32 },
    /// The signaling socket died before a peer was paired (a phone with
    /// no network, ~6 s after FIND OPPONENT). Nothing to recover: the
    /// player is told, and CANCEL / PLAY THE BOT are the exits. Replaces
    /// a fall-back to `Idle`, which rendered nothing at all.
    SummonFailed,
    /// Paired, but the session never synchronized: the peer vanished
    /// between pairing and the first frame, or packets never got through,
    /// twice (the app re-dials once on its own first). Nothing was at
    /// stake and nothing is recorded; the player is told, and CANCEL /
    /// PLAY THE BOT are the exits, as for [`LobbyState::SummonFailed`].
    PeerUnreachable,
}

impl LobbyState {
    /// True iff a peer has been paired and the datachannel is
    /// presumed live. Used by sim/render to gate "in-match" UI and
    /// to decide when the P2PSession transition (cycle 3) should
    /// fire.
    pub fn is_connected(&self) -> bool {
        matches!(self, LobbyState::Connected { .. })
    }

    /// True iff the lobby is in any state where a P2P session would
    /// be active or recoverable — i.e. not Idle/Connecting/
    /// WaitingForPeer (pre-match) and not Forfeited (terminal).
    /// `Disconnected` counts as "in match" for the purpose of
    /// keeping the sim ticking through the grace period.
    pub fn is_in_match(&self) -> bool {
        matches!(
            self,
            LobbyState::Connected { .. } | LobbyState::Disconnected { .. }
        )
    }

    /// True iff the match ended without the sim deciding it — a forfeit
    /// or a desync. The app drops the ggrs `Session` on these so the
    /// out-of-band `MatchOver` write cannot be rolled back
    /// (`netplay::freeze_terminal_session`).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            LobbyState::Forfeited { .. } | LobbyState::Desynced { .. }
        )
    }
}

/// Phase 12 cycle 3: signal that the lobby just rose-edged into
/// `Connected`. Set by [`detect_peer_connection_edge`] on the tick
/// the transition is observed; consumed and cleared by the app
/// crate's session-swap system (cycle 5 wires the real matchbox
/// bridge construction).
///
/// We use a simple `Option<PeerId>` resource rather than Bevy's
/// `Event` channel because the sim/net crates compile against a
/// minimal Bevy feature set (no `bevy_app::App::add_event` extension
/// trait). Same observable semantics, fewer features pulled in.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingP2PSwap(pub Option<PeerId>);

/// Pure transition detector. Returns `Some(peer_id)` iff the lobby
/// just transitioned **into** `Connected` from any other state — the
/// rising edge that should trigger the P2P session swap. Returns
/// `None` when already `Connected` (no double-swap), and `None` for
/// every transition that doesn't end at `Connected`.
///
/// Note: a transition `Disconnected -> Connected` is technically a
/// reconnect after a brief blip; we treat it as a fresh connection
/// edge for now (cycle 4 may refine the semantics if reconnects
/// need to keep the existing `P2PSession` alive instead of swapping
/// in a new one).
pub fn should_swap_to_p2p(prev: &LobbyState, curr: &LobbyState) -> Option<PeerId> {
    match (prev, curr) {
        (LobbyState::Connected { .. }, LobbyState::Connected { .. }) => None,
        (_, LobbyState::Connected { peer_id }) => Some(*peer_id),
        _ => None,
    }
}

/// `Update`-schedule system: detects rising-edge transitions into
/// `Connected` and writes the peer id into [`PendingP2PSwap`] for
/// the app crate's swap-system to consume. Uses a system-local
/// `Option<LobbyState>` to remember the previous-tick state.
///
/// This system does NOT clear `PendingP2PSwap` — the consumer is
/// expected to take ownership of the value and reset it to `None`
/// after performing the session swap. That keeps the cycle
/// consume-once even if the consumer happens to run before the
/// detector on a given tick.
pub fn detect_peer_connection_edge(
    state: Res<LobbyState>,
    mut prev: Local<Option<LobbyState>>,
    mut pending: ResMut<PendingP2PSwap>,
) {
    let previous = prev.clone().unwrap_or(LobbyState::Idle);
    if previous != *state {
        tracing::info!(
            target: "two_top::net::lobby",
            prev = ?previous,
            next = ?*state,
            "lobby-state transition",
        );
    }
    if let Some(peer_id) = should_swap_to_p2p(&previous, &state) {
        pending.0 = Some(peer_id);
        tracing::info!(
            target: "two_top::net::peer",
            peer_id = %peer_id.0,
            "rising-edge to Connected — pending P2P session swap",
        );
    }
    *prev = Some(state.clone());
}

/// Phase 12 cycle 4: frames since the last received-from-peer
/// message. The matchbox driver writes this whenever
/// `WebRtcChannel::receive` returns a non-empty packet vector; the
/// disconnection-grace system reads it to compute "frames of
/// silence" and cross thresholds. Initialized to 0 — the very
/// first tick after connection sets it to the current frame.
///
/// Wall-clock time would be simpler, but using `FrameCount` means
/// both peers agree on the silence threshold even during clock
/// drift — the same property that motivates the rest of the
/// determinism stack. (Disconnection is observed locally, but the
/// agreement matters when Phase 14 replays a forfeit and needs the
/// reconstruction to match the live behavior.)
#[derive(Resource, Default, Clone, Copy, Debug)]
pub struct LastPeerMessageFrame(pub u32);

/// The clock the silence FSM ages on: real time since boot, counted in
/// 60 Hz ticks, so every `*_AFTER_FRAMES` threshold keeps its meaning.
/// [`LastPeerMessageFrame`] is stamped from it.
///
/// It was the sim's `FrameCount`, and that clock stops exactly when it is
/// needed: a P2P session stops advancing frames once the peer's inputs stop
/// arriving (ggrs holds at its prediction window), so silence never aged
/// past the 16 frames of that window, the lobby never reached
/// `Disconnected`, OPPONENT AWAY could never show, and the healthy side
/// stared at a frozen board until ggrs's own timeout. Disconnection is
/// observed locally on each phone either way; nothing here is simulated.
/// Monotonic: it only ever moves forward (a test can set it ahead).
#[derive(Resource, Default, Clone, Copy, Debug)]
pub struct SilenceClock(pub u32);

/// `Update`, before the grace timer: advance [`SilenceClock`] to now.
/// No time plugin (a bare test app): the clock holds still.
pub fn advance_silence_clock(time: Option<Res<Time<Real>>>, mut clock: ResMut<SilenceClock>) {
    let Some(time) = time else {
        return;
    };
    let now = (time.elapsed_secs_f64() * f64::from(sim::TICK_HZ as u32)) as u32;
    clock.0 = clock.0.max(now);
}

/// Frames of peer silence that trigger `Connected -> Disconnected`.
/// 60 = 1 s @ 60 Hz. Short enough that the UI can surface a
/// reconnect spinner immediately on a real disconnect; long enough
/// that ordinary packet jitter doesn't cause spurious flickers.
pub const DISCONNECT_AFTER_FRAMES: u32 = 60;

/// Frames of peer silence that trigger `Disconnected -> Forfeited`, for a
/// peer that went quiet without a word. 540 = 9 s @ 60 Hz. This FSM is
/// the authority: ggrs's own disconnect timeout
/// (`app::netplay::GGRS_DISCONNECT_TIMEOUT`) is set at the paused grace
/// below, a ceiling it only reaches if this one somehow does not fire.
/// The app ends the match in the sim on the `Forfeited` transition.
pub const FORFEIT_AFTER_FRAMES: u32 = 540;

/// The same for a peer that said it was going away ([`NetMsg::Pausing`]):
/// 1200 = 20 s, long enough for a notification-shade peek, a short call
/// or a trip to the chat app to send the room code, without losing the
/// match.
pub const PAUSED_FORFEIT_AFTER_FRAMES: u32 = 1200;

/// Silence the FSM forfeits a peer at.
pub fn forfeit_after(peer_paused: bool) -> u32 {
    if peer_paused {
        PAUSED_FORFEIT_AFTER_FRAMES
    } else {
        FORFEIT_AFTER_FRAMES
    }
}

/// The peer said it was going away ([`NetMsg::Pausing`]) and has not been
/// heard from since. Cleared when its traffic resumes and when a session
/// starts or ends.
#[derive(Resource, Default, Clone, Copy, Debug)]
pub struct PeerPaused(pub bool);

/// Pure helper: given the current lobby state, the current frame,
/// and the last-message frame, return the next lobby state if a
/// transition fires this tick — or `None` if nothing changes.
///
/// Transitions:
///   - `Connected` after `DISCONNECT_AFTER_FRAMES` of silence ->
///     `Disconnected`.
///   - `Disconnected` after `forfeit_after` of silence ->
///     `Forfeited` (terminal).
///   - `Disconnected` whose elapsed silence dropped below the
///     disconnect threshold -> `Connected` (reconnect happened).
///   - All other states pass through unchanged.
pub fn next_lobby_state_for_silence(
    curr: &LobbyState,
    frame: u32,
    last_msg: u32,
    forfeit_after: u32,
) -> Option<LobbyState> {
    let elapsed = frame.saturating_sub(last_msg);
    match curr {
        LobbyState::Connected { peer_id } if elapsed >= DISCONNECT_AFTER_FRAMES => {
            Some(LobbyState::Disconnected {
                peer_id: *peer_id,
                since_frame: last_msg,
            })
        }
        LobbyState::Disconnected { peer_id, .. } if elapsed >= forfeit_after => {
            Some(LobbyState::Forfeited {
                peer_id: *peer_id,
                conceded: false,
            })
        }
        LobbyState::Disconnected { peer_id, .. } if elapsed < DISCONNECT_AFTER_FRAMES => {
            Some(LobbyState::Connected { peer_id: *peer_id })
        }
        _ => None,
    }
}

/// `Update`-schedule system: drives the disconnection-grace timer.
/// Reads [`SilenceClock`] + `LastPeerMessageFrame`. Writes the next
/// `LobbyState` if a threshold was crossed. The sim's state is not
/// touched: ending the match is `netplay::conclude_session`'s.
pub fn tick_disconnection_grace(
    frame: Res<SilenceClock>,
    last_msg: Res<LastPeerMessageFrame>,
    paused: Res<PeerPaused>,
    mut lobby: ResMut<LobbyState>,
) {
    let forfeit_after = forfeit_after(paused.0);
    if let Some(next) = next_lobby_state_for_silence(&lobby, frame.0, last_msg.0, forfeit_after) {
        let became_forfeit = matches!(next, LobbyState::Forfeited { .. });
        tracing::warn!(
            target: "two_top::net::peer",
            frame = frame.0,
            silence_frames = frame.0.saturating_sub(last_msg.0),
            prev = ?*lobby,
            next = ?next,
            "disconnection-grace transition",
        );
        *lobby = next;
        if became_forfeit {
            // The app's `conclude_session` ends the match: it books it as
            // it stood on the last confirmed frame.
            tracing::error!(
                target: "two_top::net::peer",
                frame = frame.0,
                "peer silence exceeded forfeit threshold — match forfeited",
            );
        }
    }
}

/// Phase 12 plugin: registers the [`LobbyState`], [`PendingP2PSwap`],
/// and [`LastPeerMessageFrame`] resources and the rising-edge +
/// disconnection-grace systems. Adding this plugin to `app` is the
/// entry point for the networking lifecycle. Cycle 5 (app crate)
/// consumes `PendingP2PSwap` to swap from `Session::SyncTest` to
/// `Session::P2P` and writes `LastPeerMessageFrame` from the
/// matchbox driver loop.
#[derive(Default)]
pub struct NetPlugin;

impl Plugin for NetPlugin {
    fn build(&self, app: &mut App) {
        // Initialize sim's `FrameCount` + `MatchState` here too so a
        // headless test or signaling-only ceremony that adds
        // `NetPlugin` without `SimPlugin` still has the resources
        // the disconnection-grace system reads. `init_resource` is
        // idempotent — if `SimPlugin` is also added, its
        // `init_resource::<FrameCount>` etc. become no-ops.
        app.init_resource::<sim::FrameCount>()
            .init_resource::<sim::MatchState>()
            .init_resource::<LobbyState>()
            .init_resource::<PendingP2PSwap>()
            .init_resource::<LastPeerMessageFrame>()
            .init_resource::<SilenceClock>()
            .init_resource::<PeerPaused>()
            .init_resource::<PeerProfile>()
            .init_resource::<PeerKeys>()
            .init_resource::<PeerSig>()
            .init_resource::<RematchConsent>()
            .init_resource::<NetSendQueue>()
            .add_systems(
                Update,
                (
                    detect_peer_connection_edge,
                    (advance_silence_clock, tick_disconnection_grace).chain(),
                ),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(debug_assertions)]
    #[test]
    fn a_checksum_report_is_known_by_its_wire_tag() {
        // A one-byte magic, then the variant.
        assert!(is_checksum_report(&[42, 6, 0, 0]));
        assert!(!is_checksum_report(&[42, 2, 0, 0]), "an Input");
        // A large magic: varint marker 251 and two bytes, then the variant.
        assert!(is_checksum_report(&[251, 0x34, 0x12, 6, 9]));
        assert!(!is_checksum_report(&[251, 0x34, 0x12, 7]), "a KeepAlive");
        assert!(!is_checksum_report(&[]));
    }

    #[test]
    fn the_delay_follows_the_round_trip() {
        use delay::*;
        assert_eq!(delay_for_rtt(1), MIN, "the same Wi-Fi");
        assert_eq!(delay_for_rtt(40), 2);
        assert_eq!(delay_for_rtt(60), 3);
        assert_eq!(delay_for_rtt(100), 4);
        assert_eq!(delay_for_rtt(400), MAX, "never past the cap");
        assert_eq!(median_rtt(&[90, 20, 40]), Some(40));
        assert_eq!(median_rtt(&[]), None);
        assert_eq!(agreed(2, 4), 4);
        assert_eq!(agreed(3, 200), MAX, "a hostile proposal is clamped");
    }

    /// The side channel's wire bytes are pinned: changing a variant's
    /// postcard index means bumping `NET_PROTOCOL` (which splits rooms)
    /// and updating this table in the same commit.
    #[test]
    fn the_side_channel_keeps_its_wire_indices() {
        assert_eq!(
            NET_PROTOCOL, 2,
            "re-pin the indices below for the new protocol"
        );
        assert_eq!(&*encode_net_msg(&NetMsg::Bye), &[2u8][..]);
        assert_eq!(&*encode_net_msg(&NetMsg::Pausing), &[5u8][..]);
        assert_eq!(
            &*encode_net_msg(&NetMsg::Ended {
                reason: EndReason::Closed
            }),
            &[6u8, 1][..]
        );
        assert_eq!(&*encode_net_msg(&NetMsg::Ping { id: 2 }), &[7u8, 2][..]);
        assert_eq!(&*encode_net_msg(&NetMsg::Pong { id: 2 }), &[8u8, 2][..]);
        assert_eq!(
            &*encode_net_msg(&NetMsg::Delay { frames: 3 }),
            &[9u8, 3][..]
        );
        for reason in [EndReason::Closed, EndReason::Desynced { frame: 4321 }] {
            let msg = NetMsg::Ended { reason };
            assert_eq!(decode_net_msg(&encode_net_msg(&msg)), Some(msg));
        }
    }

    /// Compile-time check: `MatchboxBridge` must impl
    /// `NonBlockingSocket<sim::NetAddr>` so it can be plugged into a
    /// `ggrs::SessionBuilder<GgrsCfg>::start_p2p_session(socket)`
    /// call — `GgrsCfg::Address` is `sim::NetAddr`, so the socket's
    /// address type must match exactly. ggrs 0.12's `Message` has
    /// private fields (only the struct itself is `pub`), so we can't
    /// construct one in a unit test — the wire-format round-trip is
    /// implicitly covered the moment two real peers exchange a sync
    /// handshake at runtime. This compile-fence is the strongest
    /// static guarantee we can author from outside the ggrs crate.
    #[test]
    fn matchbox_bridge_impls_non_blocking_socket() {
        fn assert_impl<T: NonBlockingSocket<NetAddr>>() {}
        assert_impl::<MatchboxBridge>();
    }

    /// The link-lost crash: once the socket's future is gone (the room
    /// server restarted, the phone changed networks), matchbox's `send`
    /// panics. The bridge must keep accepting sends as lost packets, and
    /// the closure must be visible to the app through `any_channel_closed`
    /// on what remains of the socket.
    // Not on Android: it builds a real matchbox socket, which links
    // webrtc's interface enumeration (`getifaddrs`, API 24+), and the
    // determinism lane links Android test binaries against API 21. The
    // app itself ships at a level that has it; the bridge logic is the same
    // on every target and runs here on the others.
    // platform-gate-ok: a native test; the browser never runs net's tests.
    #[cfg(not(target_os = "android"))]
    #[test]
    fn a_bridge_over_a_dead_socket_drops_packets_instead_of_panicking() {
        let (mut socket, message_loop) =
            matchbox_socket::WebRtcSocketBuilder::new("ws://127.0.0.1:1/room")
                .add_channel(ChannelConfig::unreliable())
                .add_channel(ChannelConfig::reliable())
                .build();
        let mut channel = socket.take_channel(0).expect("channel 0");
        assert!(!socket.any_channel_closed(), "a fresh socket is open");
        // The socket's future ending is what drops its receivers.
        drop(message_loop);
        assert!(
            socket.any_channel_closed(),
            "the side channel reports the loss"
        );
        // A ggrs `Message` cannot be built outside ggrs, so the bridge's
        // send path is exercised through the call it now makes: `try_send`
        // answers Err where matchbox's `send` panicked with "Send failed".
        assert!(
            channel
                .try_send(vec![1u8, 2, 3].into(), addr_to_peer(NetAddr(7)))
                .is_err()
        );
        let mut bridge = MatchboxBridge::new(channel);
        assert!(bridge.receive_all_messages().is_empty());
    }

    /// The `PeerId` ↔ `NetAddr` bijection must round-trip losslessly
    /// in both directions — ggrs routes by `NetAddr` while matchbox
    /// routes by `PeerId`, so a lossy conversion would misroute
    /// packets between peers and desync the session.
    #[test]
    fn peer_addr_bijection_round_trips() {
        let peer = PeerId(Uuid::from_u128(0x1234_5678_9abc_def0_1122_3344_5566_7788));
        assert_eq!(addr_to_peer(peer_to_addr(peer)), peer);

        let addr = NetAddr(0xdead_beef_cafe_f00d);
        assert_eq!(peer_to_addr(addr_to_peer(addr)), addr);
    }

    /// Malformed ggrs-channel bytes must be dropped, not fatal. The old
    /// contract panicked here (mirroring matchbox's reference impl),
    /// which handed any stranger in the public room a one-packet abort
    /// of the app; now the packet is discarded and the silence FSM
    /// scores a peer that only ever sends garbage as the walk-away.
    /// A well-formed `Message` can't be constructed from outside ggrs
    /// (private fields), so the decode-success path is covered the
    /// moment two real peers exchange a sync handshake; the refusal
    /// path is what's testable, and it's the one that matters.
    #[test]
    fn malformed_ggrs_packets_are_dropped_not_fatal() {
        let peer = PeerId(Uuid::from_u128(0xbad));
        let garbage: Packet = vec![0xffu8; 7].into();
        assert!(decode_packet((peer, garbage)).is_none());
        let empty: Packet = Vec::<u8>::new().into();
        assert!(decode_packet((peer, empty)).is_none());
    }

    // ---- Side-channel codec ----

    #[test]
    fn net_msgs_round_trip_through_postcard() {
        let msgs = [
            NetMsg::Profile(KeyedProfile {
                install_id: 0xdead_beef_cafe_f00d_1122_3344_5566_7788,
                name: crate::name_slots(&[2, 6, 0, 4]),
                pubkey: [7; 32],
            }),
            NetMsg::RematchWant,
            NetMsg::Bye,
            NetMsg::Table { arena: 6 },
        ];
        for msg in msgs {
            let bytes = encode_net_msg(&msg);
            assert_eq!(decode_net_msg(&bytes), Some(msg));
        }
    }

    #[test]
    fn malformed_side_channel_bytes_are_ignored_not_fatal() {
        assert_eq!(decode_net_msg(&[]), None);
        assert_eq!(decode_net_msg(&[0xff, 0xff, 0xff, 0xff]), None);
    }

    // ---- Signed results ----

    fn seat(install_id: u128, key: &[u8; 32], handle: u8, score: u8) -> SeatStatement {
        SeatStatement {
            install_id,
            pubkey: pubkey_for(key),
            handle,
            score,
        }
    }

    /// Both peers hold the same facts in opposite order (each lists itself
    /// first) — the canonical form must serialize to identical bytes, or
    /// the two signatures cover different messages and nothing verifies.
    #[test]
    fn statements_are_byte_identical_across_handle_orderings() {
        let (ka, kb) = (&[1u8; 32], &[2u8; 32]);
        let a = seat(0xaaa, ka, 0, 5);
        let b = seat(0xbbb, kb, 1, 3);
        let ours = MatchStatement::new(14, 3, (77, 11), 2, [a, b]);
        let theirs = MatchStatement::new(14, 3, (11, 77), 2, [b, a]);
        assert_eq!(ours, theirs);
        assert_eq!(ours.encode(), theirs.encode());
        assert_eq!(ours.seat_low.install_id, 0xaaa, "sorted by install-id");
        assert_eq!(ours.session_low, 11, "session pair sorted too");
        assert_eq!(ours.seat_for_handle(1).unwrap().install_id, 0xbbb);
    }

    #[test]
    fn signatures_round_trip_and_tampering_breaks_them() {
        let (ka, kb) = (&[3u8; 32], &[4u8; 32]);
        let stmt = MatchStatement::new(
            14,
            0,
            (1, 2),
            0,
            [seat(0xaaa, ka, 0, 5), seat(0xbbb, kb, 1, 2)],
        );
        let sig_a = sign_statement(&stmt, ka);
        assert!(stmt.verify(&pubkey_for(ka), &sig_a));
        assert!(
            !stmt.verify(&pubkey_for(kb), &sig_a),
            "the other seat's key must not accept it"
        );
        let mut flipped = stmt;
        flipped.seat_low.score = 4;
        assert!(
            !flipped.verify(&pubkey_for(ka), &sig_a),
            "a changed score invalidates the signature"
        );
    }

    #[test]
    fn attestations_verify_both_seats_or_fail() {
        let (ka, kb) = (&[5u8; 32], &[6u8; 32]);
        let stmt = MatchStatement::new(
            14,
            6,
            (9, 8),
            1,
            [seat(0x111, ka, 1, 3), seat(0x222, kb, 0, 5)],
        );
        let good = Attestation {
            statement: stmt,
            sig_low: sig_to_hex(&sign_statement(&stmt, ka)),
            sig_high: sig_to_hex(&sign_statement(&stmt, kb)),
        };
        assert!(good.verify());
        let swapped = Attestation {
            statement: stmt,
            sig_low: good.sig_high.clone(),
            sig_high: good.sig_low.clone(),
        };
        assert!(
            !swapped.verify(),
            "seats' signatures are not interchangeable"
        );
        let garbled = Attestation {
            sig_low: "zz".repeat(64),
            ..good.clone()
        };
        assert!(
            !garbled.verify(),
            "unparseable hex is a failure, not a panic"
        );
    }

    #[test]
    fn hex_helpers_round_trip() {
        let bytes = pubkey_for(&[7u8; 32]);
        assert_eq!(from_hex32(&hex32(&bytes)), Some(bytes));
        assert_eq!(from_hex32("short"), None);
        let sig = sign_statement(
            &MatchStatement::new(
                1,
                0,
                (0, 1),
                0,
                [seat(1, &[8u8; 32], 0, 5), seat(2, &[9u8; 32], 1, 0)],
            ),
            &[8u8; 32],
        );
        assert_eq!(sig_from_hex(&sig_to_hex(&sig)), Some(sig));
        assert_eq!(sig_from_hex(&"0".repeat(127)), None);
    }

    #[test]
    fn resume_messages_round_trip_and_never_read_as_net_msgs() {
        let tail = ResumeMsg::Tail {
            from: 7,
            prefix: 99,
            inputs: vec![[sim::PlayerInput::default(); 2]; 3],
        };
        for msg in [
            ResumeMsg::Hello {
                lo: 1,
                hi: 2,
                confirmed: 400,
            },
            tail,
        ] {
            let bytes = encode_resume(&msg);
            assert_eq!(decode_resume(&bytes), Some(msg));
            assert_eq!(decode_net_msg(&bytes), None);
        }
        assert_eq!(decode_resume(&encode_net_msg(&NetMsg::Bye)), None);
        assert_eq!(decode_resume(&[]), None);
        assert_eq!(decode_resume(&[RESUME_TAG, 0xff, 0xff]), None);
    }

    #[test]
    fn a_resume_merges_to_the_longer_record_and_refuses_a_disagreement() {
        let tick = |b: u8| {
            let mut p = [sim::PlayerInput::default(); 2];
            p[0].buttons = b;
            p
        };
        let long: Vec<_> = (0..10).map(tick).collect();
        let short = &long[..6];
        // The short side merges the long side's tail...
        let tail = &long[6..];
        assert_eq!(
            merge_resume(short, 6, inputs_hash(&long[..6]), tail).unwrap(),
            long
        );
        // ...and the long side keeps its own, the short side's tail empty.
        assert_eq!(
            merge_resume(&long, 6, inputs_hash(short), &[]).unwrap(),
            long
        );
        // A prefix that differs is refused.
        let mut other = long.clone();
        other[2] = tick(9);
        assert!(merge_resume(&other, 6, inputs_hash(short), &[]).is_err());
        // So is a tail that contradicts what we hold.
        assert!(merge_resume(&long, 6, inputs_hash(short), &[tick(77)]).is_err());
    }

    #[test]
    fn a_resume_room_is_the_same_for_both_sides_and_distinct_per_session() {
        assert_eq!(resume_room(3, 9), resume_room(3, 9));
        assert_ne!(resume_room(3, 9), resume_room(3, 10));
    }

    #[test]
    fn keyed_profile_and_matchsig_ride_the_side_channel() {
        let key = [10u8; 32];
        let msgs = [
            NetMsg::Profile(KeyedProfile {
                install_id: 0xfeed,
                name: crate::name_slots(&[2, 6, 0, 4]),
                pubkey: pubkey_for(&key),
            }),
            NetMsg::MatchSig {
                sig: [[0xab; 32], [0xcd; 32]],
            },
        ];
        for msg in msgs {
            let bytes = encode_net_msg(&msg);
            assert_eq!(decode_net_msg(&bytes), Some(msg));
        }
    }

    // ---- Cycle 2: LobbyState ----

    fn dummy_peer() -> PeerId {
        PeerId(uuid::Uuid::from_u128(0xc0ffee))
    }

    #[test]
    fn default_lobby_state_is_idle() {
        assert_eq!(LobbyState::default(), LobbyState::Idle);
    }

    #[test]
    fn is_connected_only_true_for_connected_variant() {
        let peer = dummy_peer();
        assert!(!LobbyState::Idle.is_connected());
        assert!(!LobbyState::Connecting.is_connected());
        assert!(!LobbyState::WaitingForPeer { our_id: peer }.is_connected());
        assert!(LobbyState::Connected { peer_id: peer }.is_connected());
        assert!(
            !LobbyState::Disconnected {
                peer_id: peer,
                since_frame: 100
            }
            .is_connected()
        );
        assert!(
            !LobbyState::Forfeited {
                peer_id: peer,
                conceded: false,
            }
            .is_connected()
        );
    }

    #[test]
    fn is_in_match_covers_connected_and_disconnected_only() {
        let peer = dummy_peer();
        assert!(!LobbyState::Idle.is_in_match());
        assert!(!LobbyState::Connecting.is_in_match());
        assert!(!LobbyState::WaitingForPeer { our_id: peer }.is_in_match());
        assert!(LobbyState::Connected { peer_id: peer }.is_in_match());
        assert!(
            LobbyState::Disconnected {
                peer_id: peer,
                since_frame: 100
            }
            .is_in_match()
        );
        assert!(
            !LobbyState::Forfeited {
                peer_id: peer,
                conceded: false,
            }
            .is_in_match()
        );
        assert!(
            !LobbyState::Desynced {
                peer_id: peer,
                frame: 7
            }
            .is_in_match()
        );
        assert!(!LobbyState::SummonFailed.is_in_match());
        assert!(!LobbyState::PeerUnreachable.is_in_match());
    }

    #[test]
    fn terminal_is_forfeit_or_desync_only() {
        let peer = dummy_peer();
        assert!(
            LobbyState::Forfeited {
                peer_id: peer,
                conceded: false,
            }
            .is_terminal()
        );
        assert!(
            LobbyState::Desynced {
                peer_id: peer,
                frame: 7
            }
            .is_terminal()
        );
        for s in [
            LobbyState::Idle,
            LobbyState::Connecting,
            LobbyState::SummonFailed,
            LobbyState::PeerUnreachable,
            LobbyState::WaitingForPeer { our_id: peer },
            LobbyState::Connected { peer_id: peer },
            LobbyState::Disconnected {
                peer_id: peer,
                since_frame: 1,
            },
        ] {
            assert!(!s.is_terminal(), "{s:?}");
        }
    }

    #[test]
    fn net_plugin_registers_lobby_state_as_idle() {
        let mut app = App::new();
        app.add_plugins(NetPlugin);
        assert_eq!(*app.world().resource::<LobbyState>(), LobbyState::Idle);
    }

    // ---- Cycle 3: PeerConnected edge detection ----

    #[test]
    fn should_swap_fires_on_rising_edge_to_connected() {
        let peer = dummy_peer();
        assert_eq!(
            should_swap_to_p2p(&LobbyState::Idle, &LobbyState::Connected { peer_id: peer }),
            Some(peer),
        );
        assert_eq!(
            should_swap_to_p2p(
                &LobbyState::WaitingForPeer { our_id: peer },
                &LobbyState::Connected { peer_id: peer }
            ),
            Some(peer),
        );
    }

    #[test]
    fn should_swap_returns_none_when_already_connected() {
        let peer = dummy_peer();
        assert_eq!(
            should_swap_to_p2p(
                &LobbyState::Connected { peer_id: peer },
                &LobbyState::Connected { peer_id: peer }
            ),
            None,
        );
    }

    #[test]
    fn should_swap_returns_none_for_non_connected_targets() {
        let peer = dummy_peer();
        assert_eq!(
            should_swap_to_p2p(&LobbyState::Idle, &LobbyState::Connecting),
            None,
        );
        assert_eq!(
            should_swap_to_p2p(
                &LobbyState::Connecting,
                &LobbyState::WaitingForPeer { our_id: peer }
            ),
            None,
        );
        assert_eq!(
            should_swap_to_p2p(
                &LobbyState::Connected { peer_id: peer },
                &LobbyState::Disconnected {
                    peer_id: peer,
                    since_frame: 100
                }
            ),
            None,
        );
        assert_eq!(
            should_swap_to_p2p(
                &LobbyState::Disconnected {
                    peer_id: peer,
                    since_frame: 100
                },
                &LobbyState::Forfeited {
                    peer_id: peer,
                    conceded: false,
                }
            ),
            None,
        );
    }

    /// Reconnect after a transient blip currently treats
    /// Disconnected -> Connected as a fresh edge — verify that
    /// behavior so a future cycle that wants to suppress the
    /// re-swap notices the test breaking.
    #[test]
    fn reconnect_after_disconnect_fires_a_fresh_edge() {
        let peer = dummy_peer();
        assert_eq!(
            should_swap_to_p2p(
                &LobbyState::Disconnected {
                    peer_id: peer,
                    since_frame: 100
                },
                &LobbyState::Connected { peer_id: peer }
            ),
            Some(peer),
        );
    }

    // ---- Cycle 4: disconnection grace ----

    #[test]
    fn no_transition_when_messages_fresh() {
        let peer = dummy_peer();
        // Frame 50, last msg at 49 — only 1 frame of silence.
        assert_eq!(
            next_lobby_state_for_silence(
                &LobbyState::Connected { peer_id: peer },
                50,
                49,
                FORFEIT_AFTER_FRAMES
            ),
            None,
        );
    }

    #[test]
    fn connected_to_disconnected_at_threshold() {
        let peer = dummy_peer();
        let last_msg = 100;
        let now = last_msg + DISCONNECT_AFTER_FRAMES;
        assert_eq!(
            next_lobby_state_for_silence(
                &LobbyState::Connected { peer_id: peer },
                now,
                last_msg,
                FORFEIT_AFTER_FRAMES
            ),
            Some(LobbyState::Disconnected {
                peer_id: peer,
                since_frame: last_msg,
            }),
        );
    }

    /// The AWAY overlay could never show: the grace timer aged on the sim's
    /// frame clock, which a P2P session stops advancing once the peer's
    /// inputs stop arriving. On the silence clock, a peer silent for a
    /// second of real time reads as away, whatever the frame count does.
    #[test]
    fn silence_ages_while_the_sim_frame_stands_still() {
        let mut app = App::new();
        app.add_plugins(NetPlugin);
        let peer = dummy_peer();
        *app.world_mut().resource_mut::<LobbyState>() = LobbyState::Connected { peer_id: peer };
        // The sim froze a few frames past the last message (its prediction
        // window), so a frame clock reads no silence; real time moves on.
        app.world_mut().resource_mut::<sim::FrameCount>().0 = 10;
        app.world_mut().resource_mut::<LastPeerMessageFrame>().0 = 0;
        app.world_mut().resource_mut::<SilenceClock>().0 = DISCONNECT_AFTER_FRAMES + 1;
        app.update();
        assert!(
            matches!(
                *app.world().resource::<LobbyState>(),
                LobbyState::Disconnected { .. }
            ),
            "a second of real silence is away"
        );
    }

    #[test]
    fn disconnected_to_forfeited_at_threshold() {
        let peer = dummy_peer();
        let last_msg = 100;
        let now = last_msg + FORFEIT_AFTER_FRAMES;
        assert_eq!(
            next_lobby_state_for_silence(
                &LobbyState::Disconnected {
                    peer_id: peer,
                    since_frame: last_msg
                },
                now,
                last_msg,
                FORFEIT_AFTER_FRAMES,
            ),
            Some(LobbyState::Forfeited {
                peer_id: peer,
                conceded: false,
            }),
        );
    }

    /// A peer that said it was going away is not forfeited at the quiet
    /// threshold; it is at the paused one.
    #[test]
    fn a_paused_peer_gets_the_longer_grace() {
        let peer = dummy_peer();
        let away = LobbyState::Disconnected {
            peer_id: peer,
            since_frame: 0,
        };
        let quiet = forfeit_after(false);
        let paused = forfeit_after(true);
        assert!(paused > quiet);
        assert_eq!(next_lobby_state_for_silence(&away, quiet, 0, paused), None);
        assert_eq!(
            next_lobby_state_for_silence(&away, paused, 0, paused),
            Some(LobbyState::Forfeited {
                peer_id: peer,
                conceded: false,
            }),
        );

        let mut app = App::new();
        app.add_plugins(NetPlugin);
        *app.world_mut().resource_mut::<LobbyState>() = away.clone();
        app.world_mut().resource_mut::<PeerPaused>().0 = true;
        app.world_mut().resource_mut::<SilenceClock>().0 = FORFEIT_AFTER_FRAMES + 10;
        app.update();
        assert_eq!(*app.world().resource::<LobbyState>(), away, "still waiting");
        app.world_mut().resource_mut::<SilenceClock>().0 = PAUSED_FORFEIT_AFTER_FRAMES;
        app.update();
        assert!(matches!(
            *app.world().resource::<LobbyState>(),
            LobbyState::Forfeited { .. }
        ));
    }

    #[test]
    fn disconnected_recovers_to_connected_when_message_arrives() {
        let peer = dummy_peer();
        // A new message bumps last_msg to ~now; elapsed silence is small.
        let now = 1000;
        let last_msg = 999;
        assert_eq!(
            next_lobby_state_for_silence(
                &LobbyState::Disconnected {
                    peer_id: peer,
                    since_frame: 800
                },
                now,
                last_msg,
                FORFEIT_AFTER_FRAMES,
            ),
            Some(LobbyState::Connected { peer_id: peer }),
        );
    }

    #[test]
    fn forfeited_is_terminal_in_silence_grace() {
        let peer = dummy_peer();
        // Even with very long silence, a Forfeited state stays Forfeited.
        assert_eq!(
            next_lobby_state_for_silence(
                &LobbyState::Forfeited {
                    peer_id: peer,
                    conceded: false,
                },
                10_000,
                0,
                FORFEIT_AFTER_FRAMES,
            ),
            None,
        );
    }

    /// The silence FSM ends the lobby; the sim's state is the app's to
    /// book (`netplay::conclude_session`, on the last confirmed frame), so
    /// net no longer writes it.
    #[test]
    fn a_silent_forfeit_ends_the_lobby_and_leaves_the_sim_alone() {
        let mut app = App::new();
        app.add_plugins(NetPlugin);
        let peer = dummy_peer();

        // Pre-load the lobby into Disconnected with stale last-msg.
        *app.world_mut().resource_mut::<LobbyState>() = LobbyState::Disconnected {
            peer_id: peer,
            since_frame: 0,
        };
        let before = *app.world().resource::<sim::MatchState>();
        app.world_mut().resource_mut::<SilenceClock>().0 = FORFEIT_AFTER_FRAMES + 10;
        app.world_mut().resource_mut::<LastPeerMessageFrame>().0 = 0;

        app.update();

        assert_eq!(
            *app.world().resource::<LobbyState>(),
            LobbyState::Forfeited {
                peer_id: peer,
                conceded: false
            },
        );
        assert_eq!(*app.world().resource::<sim::MatchState>(), before);
    }

    /// End-to-end: drive the system through a state transition and
    /// confirm `PendingP2PSwap` flips exactly once on the rising
    /// edge, with subsequent same-state ticks NOT re-firing.
    #[test]
    fn detect_system_sets_pending_swap_exactly_once() {
        let mut app = App::new();
        app.add_plugins(NetPlugin);
        let peer = dummy_peer();

        // First tick — still Idle. No pending swap.
        app.update();
        assert_eq!(
            *app.world().resource::<PendingP2PSwap>(),
            PendingP2PSwap(None)
        );

        // Flip to Connected; the next update should set the swap.
        *app.world_mut().resource_mut::<LobbyState>() = LobbyState::Connected { peer_id: peer };
        app.update();
        assert_eq!(
            *app.world().resource::<PendingP2PSwap>(),
            PendingP2PSwap(Some(peer)),
            "rising edge to Connected should populate the swap"
        );

        // Simulate the consumer clearing the swap (cycle 5's job).
        app.world_mut().resource_mut::<PendingP2PSwap>().0 = None;
        app.update();
        // Same state next tick — detector should NOT re-fire.
        assert_eq!(
            *app.world().resource::<PendingP2PSwap>(),
            PendingP2PSwap(None),
            "no re-fire when state unchanged"
        );
    }
}

#[cfg(test)]
mod input_stream_tests {
    use super::*;
    use matchbox_socket::PeerId;
    use uuid::Uuid;

    fn packet(msg: &WireMessage) -> Packet {
        bincode::serde::encode_to_vec(msg, bincode::config::standard())
            .unwrap()
            .into()
    }

    /// The exact stream ggrs's delta encoder emits: a `u16` LE length per
    /// frame, then the frame's (XORed) bytes, then RLE over the lot. The
    /// XOR content is irrelevant to the guard — only widths are checked.
    fn input_message(frames: &[Vec<u8>]) -> WireMessage {
        let mut delta = Vec::new();
        for f in frames {
            delta.extend_from_slice(&(f.len() as u16).to_le_bytes());
            delta.extend_from_slice(f);
        }
        WireMessage {
            header: WireHeader { magic: 0x1234 },
            body: WireBody::Input(WireInput {
                peer_connect_status: vec![
                    WireConnStatus {
                        disconnected: false,
                        last_frame: -1,
                    };
                    2
                ],
                disconnect_requested: false,
                start_frame: 0,
                ack_frame: -1,
                bytes: bitfield_rle::encode(delta),
            }),
        }
    }

    /// A real `P2PSession` fed one honest Input and then one whose start
    /// frame skips past what it has received. ggrs 0.12 `assert!`ed on
    /// that (`protocol.rs`, `on_input`) — one packet from a stranger in the
    /// public room took the whole app down. The vendored ggrs drops it.
    #[test]
    fn an_input_that_skips_ahead_is_dropped_not_fatal() {
        use ggrs::{NonBlockingSocket, PlayerType, SessionBuilder};
        struct Inject(Vec<(sim::NetAddr, Message)>);
        impl NonBlockingSocket<sim::NetAddr> for Inject {
            fn send_to(&mut self, _: &Message, _: &sim::NetAddr) {}
            fn receive_all_messages(&mut self) -> Vec<(sim::NetAddr, Message)> {
                std::mem::take(&mut self.0)
            }
        }
        let peer = PeerId(Uuid::from_u128(1));
        let honest = input_message(&[vec![0; 4]]);
        let mut skips_ahead = input_message(&[vec![0; 4]]);
        if let WireBody::Input(input) = &mut skips_ahead.body {
            input.start_frame = 100;
        }
        let inbound = [honest, skips_ahead]
            .iter()
            .map(|m| decode_packet((peer, packet(m))).expect("well-formed, so the guard passes it"))
            .collect();
        let mut session = SessionBuilder::<sim::GgrsCfg>::new()
            .with_num_players(2)
            .unwrap()
            .add_player(PlayerType::Local, 0)
            .unwrap()
            .add_player(PlayerType::Remote(peer_to_addr(peer)), 1)
            .unwrap()
            .start_p2p_session(Inject(inbound))
            .unwrap();
        session.poll_remote_clients();
    }

    /// ggrs indexes the remote's connection-status list once per player,
    /// unchecked: a short one was an index panic, so the guard refuses it.
    #[test]
    fn a_short_connection_status_list_is_refused() {
        let peer = PeerId(Uuid::from_u128(1));
        let mut short = input_message(&[vec![0; 4]]);
        if let WireBody::Input(input) = &mut short.body {
            input.peer_connect_status.truncate(1);
        }
        assert!(decode_packet((peer, packet(&short))).is_none());
        // Asking to disconnect, ggrs reads no list: nothing to refuse.
        if let WireBody::Input(input) = &mut short.body {
            input.disconnect_requested = true;
        }
        assert!(decode_packet((peer, packet(&short))).is_some());
        assert!(decode_packet((peer, packet(&input_message(&[vec![0; 4]])))).is_some());
    }

    #[test]
    fn one_input_on_the_wire_is_four_bytes() {
        // Four one-byte fields, repr(C), Pod — the width ggrs computes via
        // bincode-1 fixint `serialized_size(&PlayerInput::default())`.
        assert_eq!(INPUT_BYTES_PER_PLAYER, 4);
        assert_eq!(INPUT_FRAME_BYTES, 4);
    }

    #[test]
    fn mirror_matches_ggrs_layout() {
        // Bytes the mirror emits must be a real `ggrs::Message`, or the
        // guard is reading a format nobody sends and is silently inert.
        // `decode_packet` runs the guard AND ggrs's own `Deserialize`, so
        // `Some` here means both agreed on every variant exercised.
        let peer = PeerId(Uuid::from_u128(1));
        let honest = [
            input_message(&[vec![0; 4], vec![1, 2, 3, 4], vec![0xff; 4]]),
            input_message(&[]),
            WireMessage {
                header: WireHeader { magic: 7 },
                body: WireBody::KeepAlive,
            },
            WireMessage {
                header: WireHeader { magic: 7 },
                body: WireBody::SyncRequest(WireSyncRequest { random_request: 9 }),
            },
            WireMessage {
                header: WireHeader { magic: 7 },
                body: WireBody::SyncReply(WireSyncReply { random_reply: 9 }),
            },
            WireMessage {
                header: WireHeader { magic: 7 },
                body: WireBody::InputAck(WireInputAck { ack_frame: 3 }),
            },
            WireMessage {
                header: WireHeader { magic: 7 },
                body: WireBody::QualityReport(WireQualityReport {
                    frame_advantage: -2,
                    ping: 12,
                }),
            },
            WireMessage {
                header: WireHeader { magic: 7 },
                body: WireBody::QualityReply(WireQualityReply { pong: 12 }),
            },
            WireMessage {
                header: WireHeader { magic: 7 },
                body: WireBody::ChecksumReport(WireChecksumReport {
                    checksum: 99,
                    frame: 60,
                }),
            },
        ];
        for msg in honest {
            assert!(
                decode_packet((peer, packet(&msg))).is_some(),
                "honest {msg:?} must reach ggrs"
            );
        }
    }

    #[test]
    fn an_input_frame_of_the_wrong_width_is_refused_before_ggrs_can_expect_on_it() {
        let peer = PeerId(Uuid::from_u128(0xbad));
        let lethal: [Vec<Vec<u8>>; 5] = [
            vec![vec![0; 3]],             // bincode-1 EOF inside PlayerInput
            vec![vec![0; 4], vec![0; 5]], // one good frame, then a bad one
            vec![vec![]],                 // zero-width frame
            vec![vec![0; 8]],             // two players' worth from a one-seat peer
            vec![vec![0; 1]],
        ];
        for frames in lethal {
            assert!(
                decode_packet((peer, packet(&input_message(&frames)))).is_none(),
                "{frames:?} must not reach ggrs"
            );
        }
    }

    /// `varinteger`'s encoding: 7-bit little-endian groups, high bit set
    /// on every byte but the last.
    fn varint_bytes(mut v: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(byte);
                return out;
            }
            out.push(byte | 0x80);
        }
    }

    /// An Input body carrying `rle` verbatim as its stream — the shapes
    /// the real encoder would never produce.
    fn input_with_stream(rle: Vec<u8>) -> WireMessage {
        let mut msg = input_message(&[]);
        let WireBody::Input(input) = &mut msg.body else {
            unreachable!()
        };
        input.bytes = rle;
        msg
    }

    fn refused(rle: Vec<u8>) -> bool {
        let peer = PeerId(Uuid::from_u128(0xbad));
        decode_packet((peer, packet(&input_with_stream(rle)))).is_none()
    }

    #[test]
    fn hostile_rle_streams_are_refused_before_the_decoder_sees_them() {
        // The shapes that panic or abort `bitfield_rle::decode` itself —
        // which ggrs runs on every Input it receives.
        // A varint that never terminates: the crate reads buf[off] off the end.
        assert!(refused(vec![0x80, 0x80, 0x80]));
        // A repeat run of 2^30 zero bytes: a 1 GiB allocation from five bytes.
        assert!(refused(varint_bytes(((1u64 << 30) << 2) | 1)));
        // A literal run promising fifty bytes over a two-byte body.
        assert!(refused(vec![50 << 1, 0xaa, 0xbb]));
        // Eleven continuation bytes: past u64, where the crate's `fac <<= 7`
        // wraps; refused long before that.
        assert!(refused(vec![0x81; 11]));
        // The vet agrees with the crate on honest shapes, including runs.
        for delta in [
            vec![4u8, 0, 1, 2, 3, 4],
            vec![0u8; 300],
            vec![0xffu8; 300],
            Vec::new(),
        ] {
            let enc = bitfield_rle::encode(&delta);
            assert_eq!(rle_decoded_len(&enc), Ok(delta.len()), "{delta:?}");
            assert_eq!(bitfield_rle::decode(&enc).unwrap(), delta);
        }
    }

    #[test]
    fn truncated_streams_are_refused() {
        // A length prefix promising four bytes over a two-byte body.
        assert!(refused(bitfield_rle::encode([4u8, 0, 0xaa, 0xbb])));
        // A lone trailing byte where a two-byte prefix should be.
        assert!(refused(bitfield_rle::encode([4u8, 0, 0, 0, 0, 0, 0x01])));
    }
}
