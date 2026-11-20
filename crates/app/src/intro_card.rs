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
    shade_name: Option<&str>,
    gauntlet_tier: u32,
    rivalry: Option<String>,
) -> (String, String) {
    // A shade moves neither the ladder nor the rivalry: it is sparring,
    // against a fitted likeness — named as one.
    if practice && let Some(shade) = shade_name {
        return (
            format!("{my_name} vs {shade} SHADE"),
            "SPARRING".to_string(),
        );
    }
    if practice {
        let stakes = if crate::grudge::gauntlet_mastered(gauntlet_tier) {
            format!("GAUNTLET TIER {gauntlet_tier} - MASTERED")
        } else if gauntlet_tier > 0 {
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
    for (line, fy, size) in [
        (0usize, 0.26, crate::ui::TextRole::IntroTitle),
        (1usize, 0.325, crate::ui::TextRole::IntroSub),
    ] {
        commands.spawn((
            IntroCardLine(line),
            Text2d::new(String::new()),
            (
                TextFont {
                    font_size: size.font_size(),
                    ..default()
                },
                size,
            ),
            TextColor(if line == 0 {
                render::palette::HOT_BONE
            } else {
                render::palette::BONE.with_alpha(0.85)
            }),
            TextLayout::justify(Justify::Center),
            ScreenAnchor::new(0.0, crate::ui::anchor_y(fy), 0.0, 0.0),
            Transform::from_xyz(0.0, 0.0, crate::ui::z::MENU_TEXT - 5.0),
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
    selected: Res<sim::SelectedArena>,
    career: Res<crate::grudge::CareerRecord>,
    profile: Res<crate::profile::LocalProfile>,
    peer: Res<net::PeerProfile>,
    first_blood: Res<crate::first_blood::FirstBlood>,
    (shade, scouting): (Res<crate::bot::ShadeStyle>, Res<crate::shade::Scouting>),
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
    let (names, mut stakes) = intro_lines(
        online,
        practice.0,
        &profile.name_string(),
        peer_name.as_deref(),
        shade.0.as_ref().map(|s| s.name.as_str()),
        career.gauntlet_tier,
        career.rivalry_line(peer.0),
    );
    if first_blood.active {
        stakes = "FIRST BLOOD".to_string();
    }
    // A rival with tapes on this phone: the one habit worth knowing.
    if online && let Some(line) = &scouting.0 {
        stakes = if stakes.is_empty() {
            line.clone()
        } else {
            format!("{stakes}\n{line}")
        };
    }
    // Online the table was agreed at pairing and may not be the one this
    // player picked — say where they are sitting.
    if online {
        let table = format!("AT {}", render::arena::info(selected.0).title);
        stakes = if stakes.is_empty() {
            table
        } else {
            format!("{stakes}\n{table}")
        };
    }
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
        let (names, stakes) = intro_lines(true, true, "CURS", None, None, 6, None);
        assert_eq!(names, "CURS vs THE BOT");
        assert_eq!(stakes, "GAUNTLET TIER 6");
        let (_, fresh) = intro_lines(false, true, "CURS", None, None, 0, None);
        assert_eq!(fresh, "SPARRING");
    }

    #[test]
    fn online_card_carries_the_grudge() {
        let (names, stakes) = intro_lines(
            true,
            false,
            "CURS",
            Some("TAGA"),
            None,
            0,
            Some("4TH MEETING with TAGA - you lead 2-1".into()),
        );
        assert_eq!(names, "CURS vs TAGA");
        assert!(stakes.contains("you lead 2-1"));
    }

    #[test]
    fn a_shade_is_named_as_one() {
        let (names, stakes) = intro_lines(true, true, "CURS", None, Some("TAGA"), 6, None);
        assert_eq!(names, "CURS vs TAGA SHADE");
        assert_eq!(stakes, "SPARRING");
    }

    #[test]
    fn couch_card_says_the_only_true_thing() {
        let (names, stakes) = intro_lines(false, false, "CURS", None, None, 3, None);
        assert_eq!(names, "CUR vs STAG");
        assert!(stakes.is_empty(), "no stakes line to invent on the couch");
    }

    #[test]
    fn online_without_identity_falls_back_to_couch_read() {
        // A peer build with no profile message yet: no name to print, so
        // the card refuses to guess.
        let (names, _) = intro_lines(true, false, "CURS", None, None, 0, None);
        assert_eq!(names, "CUR vs STAG");
    }
}
