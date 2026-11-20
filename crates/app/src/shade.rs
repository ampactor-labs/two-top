//! The shade (NORTH N6) — a rival's tapes become a sparring partner.
//!
//! Tapes are measurable: throw cadence, charge holds, dash appetite, plant
//! discipline. `extract` reads one tape's input stream for one seat;
//! `fit` folds a ring of them onto the practice bot's existing knobs
//! (`bot::BotStyle`) — the shade is the same readable duelist the gauntlet
//! ships, with the rival's numbers in it. Input-stream stats only: no
//! resimulation, so fitting a ring is microseconds.
//!
//! Honesty rules, enforced in code: a shade match is `PracticeMode`, so
//! the grudge ledger never moves (the record is human-only), and
//! `grudge::record_gauntlet_result` skips while a shade is armed, so the
//! tier ladder doesn't move either. The framing is a sparring partner
//! with their habits — a fitted bot is a caricature, not the person, and
//! the UI copy says SHADE, never their bare name.

use replay::Replay;
use sim::{CHARGE_MAX_FRAMES, PlayerInput, TICK_HZ};

use crate::bot::BotStyle;

/// One tape's measured habits for one seat.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TapeStats {
    /// THROW rising edges per minute.
    pub throws_per_min: f32,
    /// Mean frames a THROW press was held (the charge habit).
    pub mean_hold_frames: f32,
    /// DASH rising edges per minute.
    pub dashes_per_min: f32,
    /// Of the frames inside a THROW hold, the fraction with AIM active —
    /// the plant discipline.
    pub aim_held_frac: f32,
    /// TAUNT rising edges per minute.
    pub taunts_per_min: f32,
    /// The tape's first move: a dash (`Some(true)`) or a throw.
    pub opener_dash: Option<bool>,
}

/// Measure `handle`'s seat across a tape's input stream.
pub fn extract(inputs: &[[PlayerInput; 2]], handle: usize) -> TapeStats {
    let mut throws = 0u32;
    let mut dashes = 0u32;
    let mut hold_frames = 0u32;
    let mut aim_in_hold = 0u32;
    let mut holds_total = 0u32;
    let mut taunts = 0u32;
    let mut opener_dash = None;
    let mut prev = PlayerInput::default();
    for frame in inputs {
        let cur = frame[handle % 2];
        let down = |b: u8| cur.buttons & b != 0;
        let was = |b: u8| prev.buttons & b != 0;
        if down(PlayerInput::THROW_DOWN) && !was(PlayerInput::THROW_DOWN) {
            throws += 1;
        }
        if down(PlayerInput::DASH_DOWN) && !was(PlayerInput::DASH_DOWN) {
            dashes += 1;
            opener_dash.get_or_insert(true);
        }
        if down(PlayerInput::THROW_DOWN) && !was(PlayerInput::THROW_DOWN) {
            opener_dash.get_or_insert(false);
        }
        if down(PlayerInput::TAUNT_DOWN) && !was(PlayerInput::TAUNT_DOWN) {
            taunts += 1;
        }
        if down(PlayerInput::THROW_DOWN) {
            holds_total += 1;
            hold_frames += 1;
            if down(PlayerInput::AIM_ACTIVE) {
                aim_in_hold += 1;
            }
        }
        prev = cur;
    }
    let minutes = (inputs.len().max(1) as f32) / (TICK_HZ as f32 * 60.0);
    TapeStats {
        throws_per_min: throws as f32 / minutes,
        mean_hold_frames: if throws > 0 {
            hold_frames as f32 / throws as f32
        } else {
            0.0
        },
        dashes_per_min: dashes as f32 / minutes,
        aim_held_frac: if holds_total > 0 {
            aim_in_hold as f32 / holds_total as f32
        } else {
            0.0
        },
        taunts_per_min: taunts as f32 / minutes,
        opener_dash,
    }
}

/// The one thing worth knowing before you meet them: the most distinctive
/// habit across their tapes, in a line. `None` when nothing stands out
/// (or there is nothing to read). Pure.
pub fn scouting_line(stats: &[TapeStats]) -> Option<String> {
    if stats.is_empty() {
        return None;
    }
    let n = stats.len() as f32;
    let avg = |f: fn(&TapeStats) -> f32| stats.iter().map(f).sum::<f32>() / n;
    let throws = avg(|s| s.throws_per_min);
    let dashes = avg(|s| s.dashes_per_min);
    let hold = avg(|s| s.mean_hold_frames) / CHARGE_MAX_FRAMES as f32;
    let aim = avg(|s| s.aim_held_frac);
    let taunts = avg(|s| s.taunts_per_min);
    let openers: Vec<bool> = stats.iter().filter_map(|s| s.opener_dash).collect();
    let dash_opens = openers.iter().filter(|d| **d).count() as f32;
    // Each candidate scores how far past "unremarkable" it sits; the
    // strongest speaks.
    let mut lines: Vec<(f32, String)> = vec![
        (
            dashes / throws.max(1.0) - 1.0,
            "THEY DASH MORE THAN THEY THROW".to_string(),
        ),
        (
            (hold - 0.6) * 4.0,
            "THEY CHARGE DEEP - PUNISH THE WIND-UP".to_string(),
        ),
        (
            (0.3 - aim) * 3.0 * (throws > 2.0) as u8 as f32,
            "THEY THROW FROM THE HIP - NO PLANT".to_string(),
        ),
        (
            (throws - 14.0) / 6.0,
            "THEY THROW EARLY AND OFTEN".to_string(),
        ),
        (
            (taunts - 2.0) / 2.0,
            format!("THEY TAUNT - {:.0} A MINUTE", taunts),
        ),
    ];
    if openers.len() >= 2 {
        let frac = dash_opens / openers.len() as f32;
        lines.push(((frac - 0.75) * 4.0, "THEY OPEN WITH A DASH".to_string()));
        lines.push(((0.25 - frac) * 4.0, "THEY OPEN WITH A THROW".to_string()));
    }
    lines
        .into_iter()
        .filter(|(score, _)| *score > 0.0)
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, line)| line)
}

/// Measure a rival's seat across their ring of tapes on this phone.
pub fn ring_stats(
    record: &crate::grudge::CareerRecord,
    key: &str,
    my_name: &str,
) -> Vec<TapeStats> {
    let Some(r) = record.rivals.get(key) else {
        return Vec::new();
    };
    let Some(dir) = crate::recorder::replays_dir() else {
        return Vec::new();
    };
    r.tapes
        .iter()
        .filter_map(|tape| crate::paths::read_bytes(&dir.join(tape)).ok())
        .filter_map(|bytes| replay::decode_for_sim_version(&bytes, sim::SIM_VERSION).ok())
        .filter_map(|replay| {
            let handle = rival_handle(&replay, &r.name, my_name)?;
            Some(extract(&replay.inputs, handle))
        })
        .collect()
}

/// The scouting line for the rival across the table, read once per
/// pairing (their tapes do not change mid-match).
#[derive(bevy::prelude::Resource, Default)]
pub struct Scouting(pub Option<String>);

fn scout_the_peer(
    peer: bevy::prelude::Res<net::PeerProfile>,
    record: bevy::prelude::Res<crate::grudge::CareerRecord>,
    profile: bevy::prelude::Res<crate::profile::LocalProfile>,
    mut scouting: bevy::prelude::ResMut<Scouting>,
) {
    if !bevy::prelude::DetectChanges::is_changed(&peer) {
        return;
    }
    scouting.0 = peer.0.and_then(|p| {
        let key = crate::grudge::rival_key(p.install_id);
        scouting_line(&ring_stats(&record, &key, &profile.name_string()))
    });
}

/// Which seat of a tape the rival played, from the header names: their
/// stored ledger name wins; failing that, the seat that isn't ours.
/// `None` when neither name matches — a renamed rival's old tape measures
/// nobody, and a wrong seat would fit OUR habits onto their shade.
pub fn rival_handle(replay: &Replay, rival_name: &str, my_name: &str) -> Option<usize> {
    let names = &replay.header.player_handles;
    if let Some(h) = names.iter().position(|n| n.as_deref() == Some(rival_name)) {
        return Some(h);
    }
    let mine = names.iter().position(|n| n.as_deref() == Some(my_name))?;
    Some(1 - mine)
}

/// Fold a ring of measurements onto the bot's knobs. Every mapping is
/// clamped into the range the tier ladder itself uses, so a degenerate
/// tape (all idle, all mash) still yields a playable duelist.
pub fn fit(stats: &[TapeStats]) -> BotStyle {
    let n = stats.len().max(1) as f32;
    let avg = |f: fn(&TapeStats) -> f32| stats.iter().map(f).sum::<f32>() / n;
    let throws = avg(|s| s.throws_per_min);
    let hold = avg(|s| s.mean_hold_frames);
    let dashes = avg(|s| s.dashes_per_min);
    let aim = avg(|s| s.aim_held_frac);
    BotStyle {
        commit_frac: (hold / CHARGE_MAX_FRAMES as f32).clamp(0.25, 0.85),
        dodge_radius: (dashes * 12.0).clamp(60.0, 300.0),
        wobble: (0.32 - aim * 0.26).clamp(0.06, 0.32),
        range: (520.0 - (throws / 14.0).clamp(0.0, 1.0) * 180.0).clamp(340.0, 520.0),
    }
}

/// How many of my newest duel tapes a sent shade is fitted from.
const MY_SHADE_TAPES: usize = 8;

/// My own seat across tapes, newest first: the measurements a shade I
/// send is fitted from. Pure over the decoded tapes.
pub fn my_stats(tapes: &[Replay], my_name: &str) -> Vec<TapeStats> {
    let mut duels: Vec<&Replay> = tapes
        .iter()
        .filter(|t| t.header.kind == replay::TapeKind::Duel)
        .collect();
    duels.sort_by_key(|t| std::cmp::Reverse(t.header.recorded_at));
    duels
        .into_iter()
        .filter_map(|t| {
            let seat = t
                .header
                .player_handles
                .iter()
                .position(|n| n.as_deref() == Some(my_name))?;
            Some(extract(&t.inputs, seat))
        })
        .take(MY_SHADE_TAPES)
        .collect()
}

#[derive(bevy::prelude::Component, Clone)]
struct ShadeCardPiece;

/// SEND MY SHADE (Settings): fit my habits from my newest duels and hand
/// the link to the share sheet, with its QR on screen for the phone
/// beside me. The link opens the web build on the spar (`#shade=`), so it
/// works for anyone, app or not.
#[allow(clippy::too_many_arguments)]
fn send_my_shade(
    mut commands: bevy::prelude::Commands,
    screen: bevy::prelude::Res<bevy::prelude::State<crate::screen::AppScreen>>,
    touches: bevy::prelude::Res<bevy::prelude::Touches>,
    keys: bevy::prelude::Res<bevy::prelude::ButtonInput<bevy::prelude::KeyCode>>,
    window: bevy::prelude::Res<input_touch::WindowSize>,
    profile: bevy::prelude::Res<crate::profile::LocalProfile>,
    config: bevy::prelude::Res<crate::share::ShareConfig>,
    mut images: bevy::prelude::ResMut<bevy::prelude::Assets<bevy::prelude::Image>>,
    cards: bevy::prelude::Query<bevy::prelude::Entity, bevy::prelude::With<ShadeCardPiece>>,
) {
    use bevy::prelude::*;
    if *screen.get() != crate::screen::AppScreen::Settings {
        for e in &cards {
            commands.entity(e).despawn();
        }
        return;
    }
    let win = window.0;
    let mut tapped = keys.just_pressed(KeyCode::KeyY);
    for t in touches.iter_just_pressed() {
        if !cards.is_empty() {
            for e in &cards {
                commands.entity(e).despawn();
            }
            return;
        }
        if win.y > 0.0
            && crate::settings::row_at(t.position().y / win.y) == Some(crate::settings::SHADE_ROW)
        {
            tapped = true;
        }
    }
    if !tapped {
        return;
    }
    let my_name = profile.name_string();
    let tapes: Vec<Replay> = crate::recorder::replays_dir()
        .map(|dir| crate::paths::list_dir(&dir, "bmrg"))
        .unwrap_or_default()
        .iter()
        .filter_map(|p| crate::paths::read_bytes(p).ok())
        .filter_map(|b| replay::decode_for_sim_version(&b, sim::SIM_VERSION).ok())
        .collect();
    let stats = my_stats(&tapes, &my_name);
    if stats.is_empty() {
        tracing::info!(target: "two_top::shade", "no duel of mine on this phone yet — nothing to fit");
        return;
    }
    let card = crate::launch::ShadeCard {
        name: my_name.clone(),
        install_id: profile.install_id,
        style: fit(&stats),
    };
    let link = match &config.watch_url {
        Some(watch) => format!("{}#shade={}", watch.trim_end_matches('/'), card.encode()),
        None => crate::launch::shade_link(&card),
    };
    tracing::info!(target: "two_top::shade", %link, tapes = stats.len(), "my shade, sent");
    crate::share::spawn_link_card(&mut commands, &mut images, &link, ShadeCardPiece);
    crate::platform::share_text("Spar my shade", &format!("Spar my shade in 2-Top: {link}"));
}

pub struct ShadePlugin;

impl bevy::prelude::Plugin for ShadePlugin {
    fn build(&self, app: &mut bevy::prelude::App) {
        app.init_resource::<Scouting>()
            .add_systems(bevy::prelude::Update, (send_my_shade, scout_the_peer));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tape(frames: u32, per: impl Fn(u32) -> PlayerInput) -> Vec<[PlayerInput; 2]> {
        (0..frames)
            .map(|f| [PlayerInput::default(), per(f)])
            .collect()
    }

    #[test]
    fn a_presser_and_a_turtle_measure_apart() {
        // The presser: a planted 30-frame charge every 90 frames, dashing
        // on a 120-frame cycle.
        let presser = tape(3600, |f| {
            let phase = f % 90;
            let buttons = if phase < 30 {
                PlayerInput::THROW_DOWN | PlayerInput::AIM_ACTIVE
            } else if f % 120 == 0 {
                PlayerInput::DASH_DOWN
            } else {
                0
            };
            PlayerInput {
                stick_x: 60,
                stick_y: 0,
                aim_angle: 0,
                buttons,
            }
        });
        // The turtle: one unplanted poke every 600 frames, never dashes.
        let turtle = tape(3600, |f| PlayerInput {
            stick_x: 0,
            stick_y: 0,
            aim_angle: 0,
            buttons: if f % 600 < 4 {
                PlayerInput::THROW_DOWN
            } else {
                0
            },
        });
        let p = extract(&presser, 1);
        let t = extract(&turtle, 1);
        assert!(p.throws_per_min > t.throws_per_min * 3.0);
        assert!(p.mean_hold_frames > t.mean_hold_frames);
        assert!(p.dashes_per_min > 0.0 && t.dashes_per_min == 0.0);
        assert!(p.aim_held_frac > 0.9 && t.aim_held_frac < 0.1);

        let ps = fit(&[p]);
        let ts = fit(&[t]);
        assert!(
            ps.commit_frac > ts.commit_frac,
            "the presser charges deeper"
        );
        assert!(
            ps.dodge_radius > ts.dodge_radius,
            "the dasher dodges sooner"
        );
        assert!(ps.wobble < ts.wobble, "the planter aims tighter");
        assert!(ps.range < ts.range, "the presser fights closer");
    }

    #[test]
    fn my_shade_is_fitted_from_my_seat_of_my_newest_duels() {
        let mut duel = replay_sync_free_replay();
        duel.header.kind = replay::TapeKind::Duel;
        duel.header.player_handles = [Some("ZED".into()), Some("ME".into())];
        duel.inputs = tape(600, |f| PlayerInput {
            buttons: if f % 60 < 20 {
                PlayerInput::THROW_DOWN
            } else {
                0
            },
            ..PlayerInput::default()
        });
        let mut practice = duel.clone();
        practice.header.kind = replay::TapeKind::Practice { tier: 1 };
        let stats = my_stats(&[duel.clone(), practice], "ME");
        assert_eq!(stats.len(), 1, "only duels speak for me");
        assert!(stats[0].throws_per_min > 0.0, "my seat, not theirs");
        assert!(my_stats(&[duel], "NOBODY").is_empty());
    }

    #[test]
    fn the_scouting_line_names_the_loudest_habit() {
        let base = TapeStats {
            throws_per_min: 8.0,
            mean_hold_frames: 10.0,
            dashes_per_min: 3.0,
            aim_held_frac: 0.6,
            taunts_per_min: 0.0,
            opener_dash: None,
        };
        assert_eq!(scouting_line(&[base, base]), None, "nothing stands out");
        let dasher = TapeStats {
            dashes_per_min: 20.0,
            ..base
        };
        assert_eq!(
            scouting_line(&[dasher]).as_deref(),
            Some("THEY DASH MORE THAN THEY THROW")
        );
        let opener = TapeStats {
            opener_dash: Some(true),
            ..base
        };
        assert_eq!(
            scouting_line(&[opener, opener, opener]).as_deref(),
            Some("THEY OPEN WITH A DASH")
        );
        assert_eq!(scouting_line(&[]), None);
    }

    #[test]
    fn fitting_nothing_still_yields_a_playable_duelist() {
        let idle = extract(&tape(1800, |_| PlayerInput::default()), 1);
        let style = fit(&[idle]);
        assert!(style.commit_frac >= 0.25);
        assert!(style.dodge_radius >= 60.0);
        assert!(style.wobble <= 0.32);
        assert!(style.range <= 520.0);
    }

    #[test]
    fn the_rivals_seat_is_found_by_name_or_by_elimination() {
        let mut replay = replay_sync_free_replay();
        replay.header.player_handles = [Some("SUDS".into()), Some("TAGC".into())];
        assert_eq!(rival_handle(&replay, "TAGC", "SUDS"), Some(1));
        assert_eq!(rival_handle(&replay, "SUDS", "TAGC"), Some(0));
        // Renamed rival: their old name is on the tape; ours still pins it.
        replay.header.player_handles = [Some("SUDS".into()), Some("OLDNAME".into())];
        assert_eq!(rival_handle(&replay, "TAGC", "SUDS"), Some(1));
        // Neither name matches: refuse rather than fit the wrong seat.
        replay.header.player_handles = [Some("A".into()), Some("B".into())];
        assert_eq!(rival_handle(&replay, "TAGC", "SUDS"), None);
    }

    fn replay_sync_free_replay() -> Replay {
        Replay {
            header: replay::ReplayHeader {
                magic: replay::MAGIC,
                format_version: replay::FORMAT_VERSION,
                sim_version: sim::SIM_VERSION,
                seed: 0,
                num_players: 2,
                frame_rate: 60,
                frame_count: 0,
                recorded_at: 0,
                winner: None,
                player_handles: [None, None],
                arena_id: 0,
                start_tick: 0,
                kind: replay::TapeKind::default(),
                kills: Vec::new(),
            },
            inputs: Vec::new(),
        }
    }
}
