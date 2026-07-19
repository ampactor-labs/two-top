//! The ritual before the round — the grudge intro card.
//!
//! The ledger has always known "4TH MEETING - you lead 2-1", and until now
//! it said so only on the way OUT, on the summary. The intro card moves the
//! stakes to the front: during round 1's countdown, over the table, both
//! names and the standing grudge — then it clears before GO and the duel
//! owns the screen. Practice states the gauntlet tier instead; couch says
//! the only true thing it can. Render-only; the sim never knows a card was
//! shown, and a tape in the theater shows its own match instead.

use bevy::prelude::*;
use sim::{MatchScore, MatchState};

use crate::anchor::ScreenAnchor;
use crate::netplay::NetplayConfig;
use crate::screen::{AppScreen, AwaitingPeer};

/// The two card lines: 0 = the names, 1 = the stakes under them.
#[derive(Component)]
struct IntroCardLine(usize);

/// Compose the card. Pure for tests. `rivalry` comes from
/// `CareerRecord::rivalry_line`, which counts the meeting being entered —
/// at countdown nothing is recorded yet, so a stranger reads FIRST MEETING.
pub fn intro_lines(
    online: bool,
    practice: bool,
    my_name: &str,
    peer_name: Option<&str>,
    gauntlet_tier: u32,
    rivalry: Option<String>,
) -> (String, String) {
    if practice {
        let stakes = if gauntlet_tier > 0 {
            format!("GAUNTLET TIER {gauntlet_tier}")
        } else {
            "SPARRING".to_string()
        };
        return (format!("{my_name} vs THE BOT"), stakes);
    }
    if online && let Some(peer) = peer_name {
        return (format!("{my_name} vs {peer}"), rivalry.unwrap_or_default());
    }
    ("CUR vs STAG".to_string(), String::new())
}

fn spawn_intro_card(mut commands: Commands) {
    for (line, fy, size) in [(0usize, 0.26, 56.0), (1usize, 0.325, 32.0)] {
        commands.spawn((
            IntroCardLine(line),
            Text2d::new(String::new()),
            TextFont {
                font_size: size,
                ..default()
            },
            TextColor(if line == 0 {
                render::palette::HOT_BONE
            } else {
                render::palette::BONE.with_alpha(0.85)
            }),
            TextLayout::new_with_justify(Justify::Center),
            ScreenAnchor::new(0.0, 1.0 - 2.0 * fy, 0.0, 0.0),
            Transform::from_xyz(0.0, 0.0, 205.0),
            Visibility::Hidden,
        ));
    }
}

/// Show during round 1's countdown only — a 0-0 score during `Countdown`
/// is the front door of a match (later rounds count down too, but their
/// score has moved; the ritual happens once). The card never shows over a
/// tape or an empty waiting table.
#[allow(clippy::too_many_arguments)]
fn update_intro_card(
    screen: Res<State<AppScreen>>,
    state: Res<MatchState>,
    score: Res<MatchScore>,
    awaiting: Res<AwaitingPeer>,
    theater: Res<crate::theater::TheaterMode>,
    netplay: Res<NetplayConfig>,
    practice: Res<crate::bot::PracticeMode>,
    career: Res<crate::grudge::CareerRecord>,
    profile: Res<crate::profile::LocalProfile>,
    peer: Res<net::PeerProfile>,
    mut q: Query<(&IntroCardLine, &mut Text2d, &mut Visibility)>,
) {
    let show = *screen.get() == AppScreen::InMatch
        && matches!(*state, MatchState::Countdown { .. })
        && score.p0 == 0
        && score.p1 == 0
        && !awaiting.0
        && !theater.active();
    if !show {
        for (_, _, mut vis) in &mut q {
            *vis = Visibility::Hidden;
        }
        return;
    }
    let online = netplay.room_url.is_some() && !practice.0;
    let peer_name = peer.0.map(|p| crate::profile::peer_name(Some(p)));
    let (names, stakes) = intro_lines(
        online,
        practice.0,
        &profile.name_string(),
        peer_name.as_deref(),
        career.gauntlet_tier,
        career.rivalry_line(peer.0),
    );
    for (line, mut text, mut vis) in &mut q {
        let content = if line.0 == 0 { &names } else { &stakes };
        text.0 = content.clone();
        *vis = if content.is_empty() {
            Visibility::Hidden
        } else {
            Visibility::Visible
        };
    }
}

pub struct IntroCardPlugin;

impl Plugin for IntroCardPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_intro_card)
            .add_systems(Update, update_intro_card);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn practice_card_states_the_tier() {
        let (names, stakes) = intro_lines(true, true, "CURS", None, 6, None);
        assert_eq!(names, "CURS vs THE BOT");
        assert_eq!(stakes, "GAUNTLET TIER 6");
        let (_, fresh) = intro_lines(false, true, "CURS", None, 0, None);
        assert_eq!(fresh, "SPARRING");
    }

    #[test]
    fn online_card_carries_the_grudge() {
        let (names, stakes) = intro_lines(
            true,
            false,
            "CURS",
            Some("TAGA"),
            0,
            Some("4TH MEETING with TAGA - you lead 2-1".into()),
        );
        assert_eq!(names, "CURS vs TAGA");
        assert!(stakes.contains("you lead 2-1"));
    }

    #[test]
    fn couch_card_says_the_only_true_thing() {
        let (names, stakes) = intro_lines(false, false, "CURS", None, 3, None);
        assert_eq!(names, "CUR vs STAG");
        assert!(stakes.is_empty(), "no stakes line to invent on the couch");
    }

    #[test]
    fn online_without_identity_falls_back_to_couch_read() {
        // A peer build with no profile message yet: no name to print, so
        // the card refuses to guess.
        let (names, _) = intro_lines(true, false, "CURS", None, 0, None);
        assert_eq!(names, "CUR vs STAG");
    }
}
