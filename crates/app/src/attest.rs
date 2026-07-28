//! Dual-signed results (NORTH N2) — the app half of `net::MatchStatement`.
//!
//! When an online match is decided ON SCORE, both phones observe the same
//! rollback-settled facts (sim version, arena, final scores, handle
//! assignment) and share the same session identity (the matchbox peer-id
//! pair plus a count of matches this session already decided). Each side
//! builds the identical canonical statement, signs it with the key minted
//! beside its install-id, and sends the signature over the reliable side
//! channel. Once the peer's signature verifies AND the recorder has frozen
//! the tape, the pair lands beside it as `<stem>.attest.json` — an
//! artifact `replay_sync --attest` can check end-to-end: re-run the tape,
//! rebuild the statement, verify both signatures.
//!
//! Forfeits stay ledger-only: each client's lobby FSM observes the
//! walk-away at its own frame, so there is no shared statement to sign.
//! Against a legacy build (no `Profile2`) nothing here fires and the
//! grudge ledger records the match exactly as before.

use bevy::prelude::*;
use net::{MatchStatement, NetMsg, NetSendQueue, PeerKeys, PeerProfile, PeerSig, SeatStatement};
use sim::{MATCH_WIN_THRESHOLD, MatchScore, MatchState};
use std::path::PathBuf;

use crate::netplay::{LocalPlayerHandle, NetplayConfig, SessionIds};
use crate::recorder::LastSavedReplay;

/// Per-session attestation state. `decided` survives RUN IT BACK (it is
/// what keeps each rematch's statement distinct); everything else is
/// per-match and clears when the sim leaves `MatchOver`.
#[derive(Resource, Default)]
pub struct AttestState {
    /// Matches this session already decided on score — the statement's
    /// `match_index`.
    decided: u32,
    statement: Option<MatchStatement>,
    ours: Option<[[u8; 32]; 2]>,
    theirs: Option<[[u8; 32]; 2]>,
    /// The tape path that existed BEFORE this match was decided — the
    /// writer must see `LastSavedReplay` move past it, or a failed save
    /// would pair this match's signatures with the previous match's tape.
    tape_before: Option<PathBuf>,
    written: bool,
}

impl AttestState {
    fn reset_match(&mut self) {
        self.statement = None;
        self.ours = None;
        self.theirs = None;
        self.tape_before = None;
        self.written = false;
    }

    /// Session teardown (`netplay::leave_online_match`).
    pub fn reset_session(&mut self) {
        *self = Self::default();
    }
}

/// On the tick a match is observed decided on score: build the canonical
/// statement, sign it, send the signature. Same guards as the grudge
/// ledger's recorder, plus the threshold and the Profile2 handshake.
#[allow(clippy::too_many_arguments)]
fn sign_decided_match(
    state: Res<MatchState>,
    score: Res<MatchScore>,
    selected: Res<sim::SelectedArena>,
    netplay: Res<NetplayConfig>,
    practice: Res<crate::bot::PracticeMode>,
    theater: Res<crate::theater::TheaterMode>,
    local: Res<LocalPlayerHandle>,
    session: Res<SessionIds>,
    profile: Res<crate::profile::LocalProfile>,
    peer: Res<PeerProfile>,
    peer_keys: Res<PeerKeys>,
    last_saved: Res<LastSavedReplay>,
    mut attest: ResMut<AttestState>,
    mut queue: ResMut<NetSendQueue>,
    mut prev_over: Local<bool>,
) {
    let over = matches!(*state, MatchState::MatchOver);
    let entered = over && !*prev_over;
    *prev_over = over;
    if !over {
        attest.reset_match();
        return;
    }
    if !entered {
        return;
    }
    if netplay.room_url.is_none() || practice.0 || theater.active() {
        return;
    }
    if score.p0 < MATCH_WIN_THRESHOLD && score.p1 < MATCH_WIN_THRESHOLD {
        // A forfeit, not a scored win: no shared deciding moment to sign.
        return;
    }
    let (Some(handle), Some(session), Some(peer), Some(peer_key), Some(our_key), Some(ours2)) = (
        local.0,
        session.0,
        peer.0,
        peer_keys.0,
        profile.signing_key_bytes(),
        profile.as_data2(),
    ) else {
        // Legacy peer (no Profile2) or a broken local key: the ledger
        // still records; the result just stays unsigned.
        return;
    };
    let (our_score, their_score) = if handle == 0 {
        (score.p0, score.p1)
    } else {
        (score.p1, score.p0)
    };
    let statement = MatchStatement::new(
        sim::SIM_VERSION,
        selected.0.as_u8(),
        session,
        attest.decided,
        [
            SeatStatement {
                install_id: ours2.install_id,
                pubkey: ours2.pubkey,
                handle: handle as u8,
                score: our_score,
            },
            SeatStatement {
                install_id: peer.install_id,
                pubkey: peer_key,
                handle: (1 - handle) as u8,
                score: their_score,
            },
        ],
    );
    let sig = net::sign_statement(&statement, &our_key);
    attest.decided += 1;
    attest.statement = Some(statement);
    attest.ours = Some(sig);
    attest.tape_before = last_saved.0.clone();
    queue.0.push(NetMsg::MatchSig { sig });
    tracing::info!(
        target: "two_top::attest",
        match_index = statement.match_index,
        "statement signed — signature sent",
    );
}

/// Consume the peer's `MatchSig` once our own statement exists. A
/// signature that arrives first (their Update simply ran earlier) waits in
/// `PeerSig`; one that fails to verify is dropped loudly and the result
/// stays unsigned — a wrong signature is a wrong statement, and re-running
/// the tape would say whose.
fn verify_peer_sig(
    state: Res<MatchState>,
    mut peer_sig: ResMut<PeerSig>,
    peer_keys: Res<PeerKeys>,
    mut attest: ResMut<AttestState>,
) {
    if !matches!(*state, MatchState::MatchOver) {
        // Anything still parked here is stale (a sig for a match that
        // ended some other way); never let it poison the next match.
        peer_sig.0 = None;
        return;
    }
    let Some(sig) = peer_sig.0 else {
        return;
    };
    let (Some(statement), Some(key)) = (attest.statement, peer_keys.0) else {
        return;
    };
    peer_sig.0 = None;
    if statement.verify(&key, &sig) {
        attest.theirs = Some(sig);
        tracing::info!(target: "two_top::attest", "peer signature verified");
    } else {
        tracing::warn!(
            target: "two_top::attest",
            "peer signature REJECTED — result stays unsigned",
        );
    }
}

/// Once both signatures exist AND the recorder has frozen this match's
/// tape, write `<stem>.attest.json` beside it and credit the attested win.
#[allow(clippy::too_many_arguments)]
fn write_attestation(
    last_saved: Res<LastSavedReplay>,
    profile: Res<crate::profile::LocalProfile>,
    peer: Res<PeerProfile>,
    local: Res<LocalPlayerHandle>,
    score: Res<MatchScore>,
    mut attest: ResMut<AttestState>,
    mut record: ResMut<crate::grudge::CareerRecord>,
) {
    if attest.written {
        return;
    }
    let (Some(statement), Some(ours), Some(theirs), Some(path)) = (
        attest.statement,
        attest.ours,
        attest.theirs,
        last_saved.0.clone(),
    ) else {
        return;
    };
    if attest.tape_before.as_ref() == Some(&path) {
        // The recorder hasn't frozen THIS match's tape yet (it saves a
        // beat after MatchOver); pairing these signatures with the
        // previous tape would be a lie.
        return;
    }
    let we_are_low = statement.seat_low.install_id == profile.install_id;
    let (sig_low, sig_high) = if we_are_low {
        (ours, theirs)
    } else {
        (theirs, ours)
    };
    let attestation = net::Attestation {
        statement,
        sig_low: net::sig_to_hex(&sig_low),
        sig_high: net::sig_to_hex(&sig_high),
    };
    let out = path.with_extension("attest.json");
    let json = match serde_json::to_string_pretty(&attestation) {
        Ok(json) => json,
        Err(e) => {
            tracing::warn!(target: "two_top::attest", error = %e, "attestation encode failed");
            attest.written = true;
            return;
        }
    };
    if let Err(e) = crate::paths::write_atomic(&out, json.as_bytes()) {
        tracing::warn!(target: "two_top::attest", error = %e, "attestation write failed");
        return;
    }
    attest.written = true;
    tracing::info!(
        target: "two_top::attest",
        path = %out.display(),
        "attestation written",
    );
    if let (Some(handle), Some(peer)) = (local.0, peer.0) {
        let ours_score = if handle == 0 { score.p0 } else { score.p1 };
        if ours_score >= MATCH_WIN_THRESHOLD {
            crate::grudge::record_attested_win(&mut record, peer);
        }
    }
}

pub struct AttestPlugin;

impl Plugin for AttestPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AttestState>().add_systems(
            Update,
            (sign_decided_match, verify_peer_sig, write_attestation).chain(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::netplay::{LocalPlayerHandle, NetplayConfig, SessionIds};
    use crate::profile::LocalProfile;

    /// Drive the whole state machine as one peer of a decided match, with
    /// the "opponent" simulated by signing the expected statement with
    /// their key: the sidecar lands beside the tape, verifies end-to-end,
    /// and the attested win reaches the ledger.
    #[test]
    fn a_decided_match_writes_a_verifying_attestation() {
        let dir = crate::paths::test_scratch("attest_e2e");
        let tape = dir.join("match_170_curwins.bmrg");
        std::fs::write(&tape, b"tape bytes irrelevant here").unwrap();

        let our_key = [21u8; 32];
        let their_key = [22u8; 32];
        let profile = LocalProfile {
            install_id: 0xaaa,
            name: "CUR".into(),
            named: true,
            signing_key: net::hex32(&our_key),
        };
        let peer_profile = net::ProfileData {
            install_id: 0xbbb,
            name: net::name_slots(&[18, 19, 0, 6]),
        };

        let mut app = App::new();
        app.add_plugins(AttestPlugin);
        app.insert_resource(MatchState::MatchOver);
        app.insert_resource(MatchScore { p0: 5, p1: 2 });
        app.insert_resource(sim::SelectedArena(sim::ArenaId::Pit));
        app.insert_resource(NetplayConfig {
            room_url: Some("ws://test".into()),
            ice_url: None,
            ice_key: None,
        });
        app.insert_resource(crate::bot::PracticeMode(false));
        app.init_resource::<crate::theater::TheaterMode>();
        app.insert_resource(LocalPlayerHandle(Some(0)));
        app.insert_resource(SessionIds(Some((7, 3))));
        app.insert_resource(profile);
        app.insert_resource(PeerProfile(Some(peer_profile)));
        app.insert_resource(PeerKeys(Some(net::pubkey_for(&their_key))));
        app.init_resource::<PeerSig>();
        app.init_resource::<NetSendQueue>();
        app.insert_resource(LastSavedReplay(None));
        app.init_resource::<crate::grudge::CareerRecord>();

        // Tick 1: MatchOver observed — we sign and queue our signature.
        app.update();
        let queued = &app.world().resource::<NetSendQueue>().0;
        assert!(
            matches!(queued.last(), Some(NetMsg::MatchSig { .. })),
            "our signature goes out on the side channel",
        );
        let expected = MatchStatement::new(
            sim::SIM_VERSION,
            sim::ArenaId::Pit.as_u8(),
            (7, 3),
            0,
            [
                SeatStatement {
                    install_id: 0xaaa,
                    pubkey: net::pubkey_for(&our_key),
                    handle: 0,
                    score: 5,
                },
                SeatStatement {
                    install_id: 0xbbb,
                    pubkey: net::pubkey_for(&their_key),
                    handle: 1,
                    score: 2,
                },
            ],
        );

        // Tick 2: the peer's signature over the SAME statement arrives,
        // and the recorder freezes the tape.
        app.world_mut().resource_mut::<PeerSig>().0 =
            Some(net::sign_statement(&expected, &their_key));
        app.world_mut().resource_mut::<LastSavedReplay>().0 = Some(tape.clone());
        app.update();

        let sidecar = tape.with_extension("attest.json");
        assert!(sidecar.exists(), "attestation lands beside the tape");
        let attestation: net::Attestation =
            serde_json::from_str(&std::fs::read_to_string(&sidecar).unwrap()).unwrap();
        assert_eq!(attestation.statement, expected);
        assert!(attestation.verify(), "both signatures check out");
        let record = app.world().resource::<crate::grudge::CareerRecord>();
        assert_eq!(
            record
                .rivals
                .get(&crate::grudge::rival_key(0xbbb))
                .unwrap()
                .attested_wins,
            1,
            "the win is now provable on the ledger",
        );

        // A garbage peer signature on a later match is refused, not written.
        let world = app.world_mut();
        *world.resource_mut::<MatchState>() = MatchState::Countdown {
            digit: 3,
            expires_at_frame: 0,
        };
        app.update(); // leaves MatchOver: per-match state clears
        let world = app.world_mut();
        *world.resource_mut::<MatchState>() = MatchState::MatchOver;
        world.resource_mut::<LastSavedReplay>().0 = Some(tape.clone());
        app.update(); // re-decided (match_index 1), our side signs
        app.world_mut().resource_mut::<PeerSig>().0 = Some([[0u8; 32], [0u8; 32]]);
        app.update();
        let state = app.world().resource::<AttestState>();
        assert!(state.theirs.is_none(), "a bad signature never lands");
        assert_eq!(state.decided, 2, "the rematch still counted a decision");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
