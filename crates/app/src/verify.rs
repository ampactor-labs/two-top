//! VERIFIED: the theater checks a sealed tape's signed result itself.
//!
//! A tape whose match was decided online carries both duelists' signatures
//! over the result (`attest.rs`, `replay::seal`). The theater does not take
//! the seal's word for it: the signatures must verify against the seats'
//! own keys and the statement must be about these very inputs (their hash)
//! — that much is known the moment the tape loads (SIGNED) — and when the
//! playback reaches the tape's end, the score the theater itself simulated
//! must be the score both signed (VERIFIED). Anything else says REFUSED.
//! The ladder's rows open here, so this is the ladder's whole trust story:
//! the relay is never believed.

use bevy::prelude::*;

/// What the theater knows about the tape's seal.
#[derive(Resource, Default, Clone, Debug, PartialEq)]
pub enum SealCheck {
    /// No seal: an unsigned tape (practice, couch, a forfeit).
    #[default]
    Unsealed,
    /// Signatures and input hash good; the score is checked at the end.
    Signed(net::MatchStatement),
    Verified,
    Refused(String),
}

/// Judge a seal against its tape's header and inputs. Pure.
pub fn judge(seal_json: &str, replay: &replay::Replay) -> SealCheck {
    let Ok(att) = serde_json::from_str::<net::Attestation>(seal_json) else {
        return SealCheck::Refused("unreadable seal".into());
    };
    let s = att.statement;
    if s.magic != net::STATEMENT_MAGIC || s.version != net::STATEMENT_VERSION {
        return SealCheck::Refused("unknown statement".into());
    }
    if s.sim_version != replay.header.sim_version || s.arena_id != replay.header.arena_id {
        return SealCheck::Refused("about another match".into());
    }
    if !att.verify() {
        return SealCheck::Refused("bad signature".into());
    }
    if s.tape_hash != replay::body_hash(&replay.inputs) {
        return SealCheck::Refused("about other inputs".into());
    }
    SealCheck::Signed(s)
}

/// At the tape's end: does the simulated score match the signed one? Pure.
pub fn conclude(statement: &net::MatchStatement, score: sim::MatchScore) -> SealCheck {
    for (handle, got) in [(0u8, score.p0), (1u8, score.p1)] {
        match statement.seat_for_handle(handle) {
            Some(seat) if seat.score == got => {}
            _ => return SealCheck::Refused("the score does not replay".into()),
        }
    }
    SealCheck::Verified
}

/// The marquee's words for a check.
pub fn label(check: &SealCheck) -> &'static str {
    match check {
        SealCheck::Unsealed => "",
        SealCheck::Signed(_) => "SIGNED - checking",
        SealCheck::Verified => "VERIFIED",
        SealCheck::Refused(_) => "SEAL REFUSED",
    }
}

/// Note the seal of the tape that just started rolling (`bytes` is the
/// file it came from).
pub fn note(world: &mut World, bytes: &[u8]) {
    let check = match replay::read_seal(bytes) {
        None => SealCheck::Unsealed,
        Some(json) => {
            let playback = world.resource::<replay::ReplayPlayback>();
            judge(&json, &playback.replay)
        }
    };
    if let SealCheck::Refused(why) = &check {
        tracing::warn!(target: "two_top::verify", %why, "the tape's seal is refused");
    }
    world.insert_resource(check);
}

#[derive(Component)]
struct SealText;

fn spawn_seal_text(mut commands: Commands) {
    commands.spawn((
        SealText,
        Text2d::new(String::new()),
        (
            TextFont {
                font_size: crate::ui::TextRole::Marquee.font_size(),
                ..default()
            },
            crate::ui::TextRole::Marquee,
        ),
        TextColor(render::palette::SPARK),
        crate::anchor::ScreenAnchor::new(0.0, 0.66, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, crate::ui::z::MENU_TEXT),
        Visibility::Hidden,
    ));
}

/// Conclude at the tape's end, and keep the marquee honest.
fn update_seal(
    theater: Res<crate::theater::TheaterMode>,
    playback: Option<Res<replay::ReplayPlayback>>,
    score: Res<sim::MatchScore>,
    mut check: ResMut<SealCheck>,
    mut q: Query<(&mut Text2d, &mut TextColor, &mut Visibility), With<SealText>>,
) {
    if let (SealCheck::Signed(statement), Some(playback)) = (&*check, playback)
        && theater.active()
        && !theater.live()
        && playback.cursor >= playback.replay.inputs.len()
    {
        *check = conclude(statement, *score);
        tracing::info!(target: "two_top::verify", result = label(&check), "the tape's seal, judged");
    }
    let Ok((mut text, mut color, mut vis)) = q.single_mut() else {
        return;
    };
    let words = label(&check);
    *vis = if theater.active() && !words.is_empty() {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
    text.0 = words.to_string();
    color.0 = match *check {
        SealCheck::Verified => render::palette::P1_CYAN,
        SealCheck::Refused(_) => render::palette::P0_BLOOD,
        _ => render::palette::SPARK,
    };
}

pub struct VerifyPlugin;

impl Plugin for VerifyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SealCheck>()
            .add_systems(Startup, spawn_seal_text)
            .add_systems(Update, update_seal);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use net::{Attestation, MatchStatement, SeatStatement, pubkey_for, sig_to_hex, sign_statement};

    const KA: &[u8; 32] = &[1; 32];
    const KB: &[u8; 32] = &[2; 32];

    fn tape() -> replay::Replay {
        replay::Replay {
            header: replay::ReplayHeader {
                magic: replay::MAGIC,
                format_version: replay::FORMAT_VERSION,
                sim_version: sim::SIM_VERSION,
                seed: 0,
                num_players: 2,
                frame_rate: 60,
                frame_count: 2,
                recorded_at: 0,
                winner: Some(0),
                player_handles: [None, None],
                arena_id: 1,
                start_tick: 0,
                kind: replay::TapeKind::Duel,
                kills: Vec::new(),
            },
            inputs: vec![[sim::PlayerInput::default(); 2]; 2],
        }
    }

    fn seal_for(t: &replay::Replay, p0: u8, p1: u8) -> String {
        let seat = |id, key, handle, score| SeatStatement {
            install_id: id,
            pubkey: pubkey_for(key),
            handle,
            score,
        };
        let s = MatchStatement::new(
            sim::SIM_VERSION,
            1,
            (1, 2),
            0,
            [seat(10, KA, 0, p0), seat(20, KB, 1, p1)],
        )
        .with_tape_hash(replay::body_hash(&t.inputs));
        serde_json::to_string(&Attestation {
            statement: s,
            sig_low: sig_to_hex(&sign_statement(&s, KA)),
            sig_high: sig_to_hex(&sign_statement(&s, KB)),
        })
        .unwrap()
    }

    #[test]
    fn a_true_seal_is_signed_then_verified_by_the_replayed_score() {
        let t = tape();
        let SealCheck::Signed(s) = judge(&seal_for(&t, 5, 2), &t) else {
            panic!("an honest seal is signed");
        };
        assert_eq!(
            conclude(&s, sim::MatchScore { p0: 5, p1: 2 }),
            SealCheck::Verified
        );
        assert!(matches!(
            conclude(&s, sim::MatchScore { p0: 4, p1: 5 }),
            SealCheck::Refused(_)
        ));
    }

    #[test]
    fn a_seal_on_other_inputs_or_garbage_is_refused() {
        let t = tape();
        let seal = seal_for(&t, 5, 2);
        let mut other = t.clone();
        other.inputs[1][1].buttons = sim::PlayerInput::THROW_DOWN;
        assert!(matches!(judge(&seal, &other), SealCheck::Refused(_)));
        assert!(matches!(judge("{}", &t), SealCheck::Refused(_)));
    }
}
