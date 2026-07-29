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
//!      `Box<[u8]>` packet and forwarding to `WebRtcChannel::send`.
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
use matchbox_socket::{Packet, PeerId, WebRtcChannel};
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
}

impl MatchboxBridge {
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
        Self { channel }
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
        .into_boxed_slice()
}

/// Decode a packet back into `(NetAddr, ggrs::Message)`, or `None` for
/// bytes that don't parse. Peers that send malformed bincode are either
/// lying or running an incompatible build; the matchbox reference impl
/// panics on them, but this app has a better answer than aborting
/// mid-duel — drop the packet and let the silence FSM score the abuser
/// as the walk-away ([`DISCONNECT_AFTER_FRAMES`] →
/// [`FORFEIT_AFTER_FRAMES`]). The public room pairs strangers, and no
/// stranger's bytes get to crash the table. (ggrs itself still `expect`s
/// on per-player input bytes inside a well-formed `Message` — an
/// upstream issue; this closes the cheap half.) The inbound `PeerId` is
/// mapped to the neutral `sim::NetAddr` ggrs expects (see module docs).
pub fn decode_packet(message: (PeerId, Packet)) -> Option<(NetAddr, Message)> {
    let (peer, bytes) = message;
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
    fn send_to(&mut self, msg: &Message, addr: &NetAddr) {
        self.channel.send(encode_message(msg), addr_to_peer(*addr));
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
    /// Identity handshake, sent once right after the P2P session swap.
    Profile(ProfileData),
    /// "I want to run it back." The receiving client surfaces it on the
    /// summary; when both sides have consented, each client emits its own
    /// THROW input and the in-sim `apply_rematch` restarts the match the
    /// same rollback-correct way it always has.
    RematchWant,
    /// Clean goodbye: the peer is leaving on purpose. The receiver forfeits
    /// immediately instead of waiting out the disconnect grace.
    Bye,
    /// Identity plus result-signing key, sent alongside [`NetMsg::Profile`].
    /// New builds read this; old builds fail to decode the unknown variant,
    /// ignore the message, and still get the legacy `Profile` — a mixed
    /// pairing degrades to unsigned results instead of breaking.
    Profile2(ProfileData2),
    /// Our ed25519 signature over [`MatchStatement::encode`] for the match
    /// that just reached the score threshold. Split halves keep the enum
    /// `Copy` (serde's array impls stop at 32).
    MatchSig { sig: [[u8; 32]; 2] },
}

/// Encode a side-channel message. Infallible for these types (postcard on
/// plain enums/structs cannot fail without allocation failure).
pub fn encode_net_msg(msg: &NetMsg) -> Packet {
    postcard::to_allocvec(msg)
        .expect("NetMsg postcard encoding cannot fail")
        .into_boxed_slice()
}

/// Decode a side-channel message. Unlike the ggrs channel (where malformed
/// bytes mean an incompatible build and panicking is honest), the side
/// channel tolerates strangers: a peer running a newer protocol just has
/// its unknown messages ignored.
pub fn decode_net_msg(bytes: &[u8]) -> Option<NetMsg> {
    postcard::from_bytes(bytes).ok()
}

// ---- Signed results (NORTH N2) --------------------------------------------
//
// The install-id travels in the open on the side channel, so it identifies
// but cannot prove. An ed25519 keypair minted beside it can: pubkeys ride
// `Profile2`, and when a match is decided ON SCORE both peers sign one
// canonical statement and swap signatures. The result becomes an artifact
// anyone can check — re-run the tape, rebuild the statement, verify both
// signatures (`replay_sync --attest`). Forfeits stay ledger-only: each
// client's lobby FSM observes the walk-away at its own frame, so there is
// no shared statement to sign.

/// A peer's shareable identity plus its result-signing pubkey.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ProfileData2 {
    pub install_id: u128,
    pub name: [u8; NAME_MAX],
    /// ed25519 verifying key. The matching signing key never leaves the
    /// device that minted it.
    pub pubkey: [u8; 32],
}

impl ProfileData2 {
    /// The identity half, for everything that already speaks `ProfileData`.
    pub fn profile(&self) -> ProfileData {
        ProfileData {
            install_id: self.install_id,
            name: self.name,
        }
    }
}

/// A hasher wrapper that makes fingerprints width-portable: every
/// fixed-width write forwards untouched, while `usize`/`isize` — which
/// std's derived `Hash` uses for enum discriminants, slice length
/// prefixes, and handle fields — widen to 64-bit. `Hasher::write_usize`
/// emits 4 bytes on wasm32 and 8 on the 64-bit natives, which skewed
/// every enum-bearing column of the browser determinism lane's first
/// run from frame 0. On 64-bit targets the widening writes the bytes
/// `usize` already wrote, so native fingerprints are unchanged — the
/// committed golden proved that before the browser did. Used by
/// `replay_sync`'s fingerprint columns and by the app's online desync
/// checksums, so a future browser-vs-phone duel compares like with like.
pub struct PortableHasher<H: core::hash::Hasher>(pub H);

impl<H: core::hash::Hasher> core::hash::Hasher for PortableHasher<H> {
    fn finish(&self) -> u64 {
        self.0.finish()
    }
    fn write(&mut self, bytes: &[u8]) {
        self.0.write(bytes)
    }
    fn write_u8(&mut self, i: u8) {
        self.0.write_u8(i)
    }
    fn write_u16(&mut self, i: u16) {
        self.0.write_u16(i)
    }
    fn write_u32(&mut self, i: u32) {
        self.0.write_u32(i)
    }
    fn write_u64(&mut self, i: u64) {
        self.0.write_u64(i)
    }
    fn write_u128(&mut self, i: u128) {
        self.0.write_u128(i)
    }
    fn write_i8(&mut self, i: i8) {
        self.0.write_i8(i)
    }
    fn write_i16(&mut self, i: i16) {
        self.0.write_i16(i)
    }
    fn write_i32(&mut self, i: i32) {
        self.0.write_i32(i)
    }
    fn write_i64(&mut self, i: i64) {
        self.0.write_i64(i)
    }
    fn write_i128(&mut self, i: i128) {
        self.0.write_i128(i)
    }
    fn write_usize(&mut self, i: usize) {
        self.0.write_u64(i as u64)
    }
    fn write_isize(&mut self, i: isize) {
        self.0.write_i64(i as i64)
    }
}

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

/// The verifying key for a raw signing key — what `Profile2` carries.
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

/// The connected peer's result-signing pubkey, once its `Profile2`
/// arrives. `None` against a legacy build — results then stay unsigned.
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
/// rollback. The `Forfeited` terminal state hands off to
/// `sim::MatchState::MatchOver` (which IS rolled back), so the
/// in-sim view of "match has ended" is the rolled-back source of
/// truth and the Lobby state is just the trigger that sets it.
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
    /// Disconnection persisted past the forfeit threshold. The match
    /// is over; the next `tick_match_state` round will read
    /// `MatchScore` and crown the surviving peer.
    Forfeited { peer_id: PeerId },
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

/// Frames of peer silence that trigger `Connected -> Disconnected`.
/// 60 = 1 s @ 60 Hz. Short enough that the UI can surface a
/// reconnect spinner immediately on a real disconnect; long enough
/// that ordinary packet jitter doesn't cause spurious flickers.
pub const DISCONNECT_AFTER_FRAMES: u32 = 60;

/// Frames of peer silence that trigger `Disconnected -> Forfeited`.
/// 600 = 10 s @ 60 Hz — the grace window that lets a phone survive a
/// notification-shade peek or a short call screen without losing the
/// match. ggrs's own disconnect (9 s, `app::netplay::DISCONNECT_TIMEOUT`)
/// is the authoritative trigger and fires just before this; the silence
/// FSM here is the fallback for the pre-swap window and for anything
/// ggrs misses. The `Forfeited` transition also sets
/// `sim::MatchState::MatchOver`, ending the round in the sim layer.
pub const FORFEIT_AFTER_FRAMES: u32 = 600;

/// Pure helper: given the current lobby state, the current frame,
/// and the last-message frame, return the next lobby state if a
/// transition fires this tick — or `None` if nothing changes.
///
/// Transitions:
///   - `Connected` after `DISCONNECT_AFTER_FRAMES` of silence ->
///     `Disconnected`.
///   - `Disconnected` after `FORFEIT_AFTER_FRAMES` of silence ->
///     `Forfeited` (terminal).
///   - `Disconnected` whose elapsed silence dropped below the
///     disconnect threshold -> `Connected` (reconnect happened).
///   - All other states pass through unchanged.
pub fn next_lobby_state_for_silence(
    curr: &LobbyState,
    frame: u32,
    last_msg: u32,
) -> Option<LobbyState> {
    let elapsed = frame.saturating_sub(last_msg);
    match curr {
        LobbyState::Connected { peer_id } if elapsed >= DISCONNECT_AFTER_FRAMES => {
            Some(LobbyState::Disconnected {
                peer_id: *peer_id,
                since_frame: last_msg,
            })
        }
        LobbyState::Disconnected { peer_id, .. } if elapsed >= FORFEIT_AFTER_FRAMES => {
            Some(LobbyState::Forfeited { peer_id: *peer_id })
        }
        LobbyState::Disconnected { peer_id, .. } if elapsed < DISCONNECT_AFTER_FRAMES => {
            Some(LobbyState::Connected { peer_id: *peer_id })
        }
        _ => None,
    }
}

/// `Update`-schedule system: drives the disconnection-grace timer.
/// Reads `FrameCount` (post-tick value, since Update runs after
/// `GgrsSchedule`) + `LastPeerMessageFrame`. Writes the next
/// `LobbyState` if a threshold was crossed, and pulses
/// `sim::MatchState::MatchOver` on a `Forfeited` transition so the
/// in-sim round flow ends.
pub fn tick_disconnection_grace(
    frame: Res<sim::FrameCount>,
    last_msg: Res<LastPeerMessageFrame>,
    mut lobby: ResMut<LobbyState>,
    mut match_state: ResMut<sim::MatchState>,
) {
    if let Some(next) = next_lobby_state_for_silence(&lobby, frame.0, last_msg.0) {
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
            tracing::error!(
                target: "two_top::net::peer",
                frame = frame.0,
                "peer silence exceeded forfeit threshold — match forfeited",
            );
            *match_state = sim::MatchState::MatchOver;
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
            .init_resource::<PeerProfile>()
            .init_resource::<PeerKeys>()
            .init_resource::<PeerSig>()
            .init_resource::<RematchConsent>()
            .init_resource::<NetSendQueue>()
            .add_systems(
                Update,
                (detect_peer_connection_edge, tick_disconnection_grace),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let garbage: Packet = vec![0xff; 7].into_boxed_slice();
        assert!(decode_packet((peer, garbage)).is_none());
        let empty: Packet = Vec::new().into_boxed_slice();
        assert!(decode_packet((peer, empty)).is_none());
    }

    // ---- Side-channel codec ----

    #[test]
    fn net_msgs_round_trip_through_postcard() {
        let msgs = [
            NetMsg::Profile(ProfileData {
                install_id: 0xdead_beef_cafe_f00d_1122_3344_5566_7788,
                name: crate::name_slots(&[2, 6, 0, 4]),
            }),
            NetMsg::RematchWant,
            NetMsg::Bye,
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
    fn profile2_and_matchsig_ride_the_side_channel() {
        let key = [10u8; 32];
        let msgs = [
            NetMsg::Profile2(ProfileData2 {
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
        assert!(!LobbyState::Forfeited { peer_id: peer }.is_connected());
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
        assert!(!LobbyState::Forfeited { peer_id: peer }.is_in_match());
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
                &LobbyState::Forfeited { peer_id: peer }
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
            next_lobby_state_for_silence(&LobbyState::Connected { peer_id: peer }, 50, 49),
            None,
        );
    }

    #[test]
    fn connected_to_disconnected_at_threshold() {
        let peer = dummy_peer();
        let last_msg = 100;
        let now = last_msg + DISCONNECT_AFTER_FRAMES;
        assert_eq!(
            next_lobby_state_for_silence(&LobbyState::Connected { peer_id: peer }, now, last_msg),
            Some(LobbyState::Disconnected {
                peer_id: peer,
                since_frame: last_msg,
            }),
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
                last_msg
            ),
            Some(LobbyState::Forfeited { peer_id: peer }),
        );
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
                last_msg
            ),
            Some(LobbyState::Connected { peer_id: peer }),
        );
    }

    #[test]
    fn forfeited_is_terminal_in_silence_grace() {
        let peer = dummy_peer();
        // Even with very long silence, a Forfeited state stays Forfeited.
        assert_eq!(
            next_lobby_state_for_silence(&LobbyState::Forfeited { peer_id: peer }, 10_000, 0),
            None,
        );
    }

    #[test]
    fn forfeit_transition_triggers_match_over_in_sim() {
        let mut app = App::new();
        app.add_plugins(NetPlugin);
        let peer = dummy_peer();

        // Pre-load the lobby into Disconnected with stale last-msg.
        *app.world_mut().resource_mut::<LobbyState>() = LobbyState::Disconnected {
            peer_id: peer,
            since_frame: 0,
        };
        app.world_mut().resource_mut::<sim::FrameCount>().0 = FORFEIT_AFTER_FRAMES + 10;
        app.world_mut().resource_mut::<LastPeerMessageFrame>().0 = 0;

        app.update();

        assert!(
            matches!(
                *app.world().resource::<LobbyState>(),
                LobbyState::Forfeited { .. }
            ),
            "lobby should have forfeited",
        );
        assert_eq!(
            *app.world().resource::<sim::MatchState>(),
            sim::MatchState::MatchOver,
            "sim::MatchState should be MatchOver after forfeit",
        );
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
