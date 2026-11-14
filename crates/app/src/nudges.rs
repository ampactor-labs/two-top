//! In-match nudges: a word at the moment a player is visibly stuck.
//!
//! First Blood teaches the verbs once; these catch the three moments a
//! real match shows someone has not got one yet, and say one line about
//! it there and then:
//!
//! * out past the floor's edge (the void claims you in three seconds, and
//!   nothing on screen said so) — GET BACK, every time;
//! * DASH pressed twice with no direction held (a dash needs one; the press
//!   did nothing, twice) — at most three times ever;
//! * your fang lying loose for three seconds (you are unarmed, and may not
//!   know you can walk over it or call it back) — at most three times ever.
//!
//! [`pick`] is the pure decision; the rest reads the match and draws the
//! line. The counts live in `progress.json` with First Blood's.

use bevy::prelude::*;
use bevy_ggrs::LocalInputs;
use sim::{Boomerang, BoomerangMods, BoomerangState, GgrsCfg, OobTimer, Player, PlayerInput};

use crate::lesson::Device;
use crate::screen::AppScreen;

/// A nudge, once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nudge {
    GetBack,
    HoldADirection,
    LooseFang,
}

impl Nudge {
    /// Its key in `progress.json`.
    pub fn key(self) -> &'static str {
        match self {
            Nudge::GetBack => "get_back",
            Nudge::HoldADirection => "hold_a_direction",
            Nudge::LooseFang => "loose_fang",
        }
    }

    /// The line, for the way the player plays.
    pub fn line(self, device: Device) -> &'static str {
        match (self, device) {
            (Nudge::GetBack, _) => "GET BACK ON THE FLOOR",
            (Nudge::HoldADirection, Device::Touch) => "HOLD A DIRECTION, THEN DASH",
            (Nudge::HoldADirection, Device::Keys) => "HOLD WASD, THEN SHIFT",
            (Nudge::LooseFang, Device::Touch) => "PICK IT UP, OR TAP TO RECALL",
            (Nudge::LooseFang, Device::Keys) => "PICK IT UP, OR SPACE TO RECALL",
        }
    }

    /// How many times it is ever shown (`None`: every time it applies).
    fn limit(self) -> Option<u32> {
        match self {
            Nudge::GetBack => None,
            Nudge::HoldADirection | Nudge::LooseFang => Some(NUDGE_LIMIT),
        }
    }
}

/// The teaching nudges stop after this many showings.
pub const NUDGE_LIMIT: u32 = 3;
/// Directionless dash presses before the nudge.
pub const NEUTRAL_DASHES: u32 = 2;
/// Seconds a fang lies loose before the nudge.
pub const LOOSE_SECS: f32 = 3.0;
/// Seconds a teaching nudge stays up.
pub const SHOW_SECS: f32 = 2.5;

/// What the match shows about the player right now.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Watch {
    pub out_of_bounds: bool,
    pub neutral_dashes: u32,
    pub loose_secs: f32,
}

/// The nudge to show, if any. Pure for tests. `shown` is how many times
/// each has been shown before.
pub fn pick(watch: &Watch, shown: impl Fn(Nudge) -> u32) -> Option<Nudge> {
    let allowed = |n: Nudge| n.limit().is_none_or(|limit| shown(n) < limit);
    if watch.out_of_bounds {
        return Some(Nudge::GetBack);
    }
    if watch.neutral_dashes >= NEUTRAL_DASHES && allowed(Nudge::HoldADirection) {
        return Some(Nudge::HoldADirection);
    }
    if watch.loose_secs >= LOOSE_SECS && allowed(Nudge::LooseFang) {
        return Some(Nudge::LooseFang);
    }
    None
}

/// The handle whose nudges to show: the one player holding this device.
/// Couch has two, and a nudge could not say which one it meant.
fn local_handle(online: Option<usize>, practice: bool) -> Option<usize> {
    online.or(practice.then_some(0))
}

/// Directionless DASH presses, counted where the inputs are made.
#[derive(Resource, Default)]
pub struct DashPresses {
    neutral: u32,
    held: bool,
}

/// `ReadInputs`, after every input source: watch the local player's DASH
/// presses. A press with the stick at rest does nothing in the sim; two of
/// them in a row is someone who has not found out a dash needs a way to
/// go. A dash with a direction clears the count.
pub fn watch_dash_presses(
    local: Res<crate::netplay::LocalPlayerHandle>,
    practice: Res<crate::bot::PracticeMode>,
    inputs: Option<Res<LocalInputs<GgrsCfg>>>,
    mut presses: ResMut<DashPresses>,
) {
    let Some(handle) = local_handle(local.0, practice.0) else {
        return;
    };
    let Some(input) = inputs.and_then(|i| i.0.get(&handle).copied()) else {
        return;
    };
    let held = input.buttons & PlayerInput::DASH_DOWN != 0;
    if held && !presses.held {
        if sim::decode_stick(input).length() <= sim::DASH_MIN_STICK_MAG {
            presses.neutral += 1;
        } else {
            presses.neutral = 0;
        }
    }
    presses.held = held;
}

/// What is up now.
#[derive(Resource, Default)]
struct Showing {
    nudge: Option<Nudge>,
    until: f32,
    loose_for: f32,
    loose_nudged: bool,
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_nudges(
    screen: Res<State<AppScreen>>,
    state: Res<sim::MatchState>,
    time: Res<Time<Real>>,
    ctx: (
        Res<crate::netplay::LocalPlayerHandle>,
        Res<crate::bot::PracticeMode>,
        Res<crate::theater::TheaterMode>,
        Res<crate::first_blood::FirstBlood>,
        Res<crate::first_blood::InputDevice>,
    ),
    players: Query<(&Player, &OobTimer)>,
    fangs: Query<(&Boomerang, &BoomerangMods)>,
    mut presses: ResMut<DashPresses>,
    mut progress: ResMut<crate::first_blood::LessonProgress>,
    mut showing: ResMut<Showing>,
    mut callout: ResMut<crate::callout::Callout>,
) {
    let (local, practice, theater, first_blood, device) = ctx;
    let now = time.elapsed_secs();
    let handle = local_handle(local.0, practice.0);
    let live = *screen.get() == AppScreen::InMatch
        && state.is_in_round()
        && !theater.active()
        && !first_blood.active;
    let Some(handle) = handle.filter(|_| live) else {
        callout.nudge = None;
        *showing = Showing::default();
        presses.neutral = 0;
        return;
    };

    let out_of_bounds = players
        .iter()
        .any(|(p, oob)| p.handle == handle && oob.0 > 0);
    let loose = fangs.iter().any(|(b, m)| {
        b.owner_handle == handle && !m.is_secondary && matches!(b.state, BoomerangState::Loose)
    });
    if loose {
        showing.loose_for += time.delta_secs();
    } else {
        showing.loose_for = 0.0;
        showing.loose_nudged = false;
    }
    let watch = Watch {
        out_of_bounds,
        neutral_dashes: presses.neutral,
        // One nudge per dropped fang, not one per frame it stays down.
        loose_secs: if showing.loose_nudged {
            0.0
        } else {
            showing.loose_for
        },
    };
    let shown = |n: Nudge| progress.0.nudges.get(n.key()).copied().unwrap_or(0);
    if let Some(nudge) = pick(&watch, shown) {
        let fresh = showing.nudge != Some(nudge) || now >= showing.until;
        if fresh && nudge.limit().is_some() {
            *progress
                .0
                .nudges
                .entry(nudge.key().to_string())
                .or_default() += 1;
            crate::first_blood::save(&progress);
            showing.until = now + SHOW_SECS;
            match nudge {
                Nudge::HoldADirection => presses.neutral = 0,
                Nudge::LooseFang => showing.loose_nudged = true,
                Nudge::GetBack => {}
            }
        }
        if nudge == Nudge::GetBack {
            showing.until = now;
        }
        showing.nudge = Some(nudge);
    } else if now >= showing.until {
        showing.nudge = None;
    }

    callout.nudge = showing.nudge.map(|nudge| crate::callout::Line {
        text: nudge.line(device.0),
        color: if nudge == Nudge::GetBack {
            // The void is closing: pulse in ember.
            render::palette::EMBER.with_alpha(0.6 + 0.4 * (now * 8.0).sin().abs())
        } else {
            render::palette::HOT_BONE
        },
    });
}

pub struct NudgesPlugin;

impl Plugin for NudgesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DashPresses>()
            .init_resource::<Showing>()
            .add_systems(Update, update_nudges.in_set(crate::callout::CalloutSpeak));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_void_is_always_called_out_first() {
        let watch = Watch {
            out_of_bounds: true,
            neutral_dashes: 5,
            loose_secs: 9.0,
        };
        assert_eq!(pick(&watch, |_| 99), Some(Nudge::GetBack));
    }

    #[test]
    fn a_teaching_nudge_waits_for_its_moment_and_stops_after_three() {
        let quiet = Watch::default();
        assert_eq!(pick(&quiet, |_| 0), None);
        let one_press = Watch {
            neutral_dashes: 1,
            ..Watch::default()
        };
        assert_eq!(pick(&one_press, |_| 0), None);
        let two_presses = Watch {
            neutral_dashes: NEUTRAL_DASHES,
            ..Watch::default()
        };
        assert_eq!(pick(&two_presses, |_| 0), Some(Nudge::HoldADirection));
        assert_eq!(pick(&two_presses, |_| NUDGE_LIMIT), None);
        let loose = Watch {
            loose_secs: LOOSE_SECS,
            ..Watch::default()
        };
        assert_eq!(pick(&loose, |_| 2), Some(Nudge::LooseFang));
        assert_eq!(pick(&loose, |_| NUDGE_LIMIT), None);
    }

    #[test]
    fn couch_gets_no_nudges_and_practice_speaks_to_the_human() {
        assert_eq!(local_handle(None, false), None);
        assert_eq!(local_handle(None, true), Some(0));
        assert_eq!(local_handle(Some(1), false), Some(1));
    }

    #[test]
    fn every_line_fits_a_phone_unwrapped() {
        for n in [Nudge::GetBack, Nudge::HoldADirection, Nudge::LooseFang] {
            for d in [Device::Touch, Device::Keys] {
                assert!(n.line(d).len() <= 30, "{}", n.line(d));
            }
        }
    }
}
