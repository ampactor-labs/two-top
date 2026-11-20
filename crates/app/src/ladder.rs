//! The ladder: signed results, posted where anyone can read them.
//!
//! Opt-in twice over. A player turns the ladder on in Settings; a phone
//! with it on says so at the start of every duel ([`net::NetMsg::Ladder`]);
//! and a result is posted only when both duelists said it. The low seat of
//! the signed statement posts (one copy per match, never two): its sealed
//! tape goes to the relay (`POST /tape`), then the tape's id to
//! `POST /results`, where the relay keeps the seal the tape carries.
//! `web/ladder.html` lists them and rates the players per season (one per
//! sim version: a new sim is a new game); each row opens the tape in the
//! web theater, which re-simulates it and checks both signatures itself.

use bevy::prelude::*;

/// The peer said it wants its results on the ladder this session.
#[derive(Resource, Default)]
pub struct PeerConsent(pub bool);

/// Tell the peer, once per session, that this side has the ladder on.
fn send_consent(
    settings: Res<crate::settings::Settings>,
    peer: Res<net::PeerProfile>,
    mut sent: Local<bool>,
    mut queue: ResMut<net::NetSendQueue>,
) {
    let in_session = peer.0.is_some();
    if !in_session {
        *sent = false;
        return;
    }
    if settings.ladder && !*sent {
        *sent = true;
        queue.0.push(net::NetMsg::Ladder);
        tracing::info!(target: "two_top::ladder", "ladder consent sent");
    }
}

fn clear_consent(mut consent: ResMut<PeerConsent>) {
    consent.0 = false;
}

/// Post `bytes` (a sealed tape), then name it as a result.
#[cfg(not(target_family = "wasm"))]
fn post_result(drop: &str, bytes: &[u8]) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    struct Receipt {
        id: String,
    }
    let base = drop.trim_end_matches('/');
    let receipt: Receipt = ureq::post(&format!("{base}/tape"))
        .timeout(std::time::Duration::from_secs(8))
        .send_bytes(bytes)
        .map_err(|e| format!("relay: {e}"))?
        .into_json()
        .map_err(|e| format!("relay answered strangely: {e}"))?;
    ureq::post(&format!("{base}/results"))
        .timeout(std::time::Duration::from_secs(8))
        .send_string(&format!(r#"{{"tape":"{}"}}"#, receipt.id))
        .map_err(|e| format!("relay refused the result: {e}"))?;
    Ok(receipt.id)
}

/// Whether this phone posts a sealed result: both consented, and ours is
/// the statement's low seat. Pure.
pub fn should_post(ours: bool, theirs: bool, our_install: u128, low_install: u128) -> bool {
    ours && theirs && our_install == low_install
}

fn post_sealed(
    settings: Res<crate::settings::Settings>,
    consent: Res<PeerConsent>,
    profile: Res<crate::profile::LocalProfile>,
    config: Res<crate::share::ShareConfig>,
    mut attest: ResMut<crate::attest::AttestState>,
) {
    let Some((path, statement)) = attest.sealed.take() else {
        return;
    };
    if !should_post(
        settings.ladder,
        consent.0,
        profile.install_id,
        statement.seat_low.install_id,
    ) {
        return;
    }
    let Some(drop) = config.drop_url.clone() else {
        return;
    };
    let Ok(bytes) = crate::paths::read_bytes(&path) else {
        return;
    };
    #[cfg(not(target_family = "wasm"))]
    crate::platform::off_main_thread("twotop-ladder", move || match post_result(&drop, &bytes) {
        Ok(id) => tracing::info!(target: "two_top::ladder", %id, "result posted to the ladder"),
        Err(why) => {
            tracing::warn!(target: "two_top::ladder", %why, "the ladder did not take the result")
        }
    });
    #[cfg(target_family = "wasm")]
    let _ = (drop, bytes);
}

pub struct LadderPlugin;

impl Plugin for LadderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PeerConsent>()
            .add_systems(OnExit(crate::screen::AppScreen::InMatch), clear_consent)
            .add_systems(Update, (send_consent, post_sealed));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_low_seat_posts_and_only_with_both_consents() {
        assert!(should_post(true, true, 1, 1));
        assert!(
            !should_post(true, true, 2, 1),
            "the high seat leaves it to the low"
        );
        assert!(!should_post(true, false, 1, 1), "the peer never agreed");
        assert!(!should_post(false, true, 1, 1), "we never agreed");
    }
}
