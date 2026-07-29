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
}

/// Measure `handle`'s seat across a tape's input stream.
pub fn extract(inputs: &[[PlayerInput; 2]], handle: usize) -> TapeStats {
    let mut throws = 0u32;
    let mut dashes = 0u32;
    let mut hold_frames = 0u32;
    let mut aim_in_hold = 0u32;
    let mut holds_total = 0u32;
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
    }
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
            },
            inputs: Vec::new(),
        }
    }
}
