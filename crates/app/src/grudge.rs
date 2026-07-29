//! Career record — the grudge ledger, v2.
//!
//! v1 persisted a single online W-L. v2 adds the two ladders that hang off
//! a durable identity:
//!
//!   * **Rivalry** — per-opponent records keyed by the peer's install-id
//!     (exchanged over the reliable side-channel as `NetMsg::Profile`).
//!     The summary can finally say "4TH MEETING — YOU LEAD 2-1".
//!   * **Gauntlet** — the practice ladder: beat the bot, the tier climbs
//!     and persists; lose once, it resets. Best tier is remembered. The
//!     bot's policy sharpens with the tier (`bot::drive_bot`).
//!
//! Forfeits are scored honestly: the survivor of a fled match records a
//! win (v1 wrongly gave them a LOSS — the score-threshold check assumed
//! every MatchOver was earned), and a player whose own phone went away
//! (suspend / focus loss, tracked in `netplay::RecentAbsence`) records
//! the loss they walked into.
//!
//! Couch matches still count for nothing; the theater records nothing.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use sim::{MATCH_WIN_THRESHOLD, MatchScore, MatchState};
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::netplay::{LocalPlayerHandle, NetplayConfig, RecentAbsence};

/// One opponent's ledger line. `name` is their latest dialed name — it can
/// change between meetings; the install-id is the identity.
#[derive(Serialize, Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct RivalRecord {
    pub name: String,
    pub wins: u32,
    pub losses: u32,
    /// Wins whose result carries a completed dual-signed attestation
    /// (NORTH N2). A subset of `wins`: unsigned wins still count — a
    /// legacy peer's build simply can't sign.
    pub attested_wins: u32,
    /// When this rivalry last met (unix seconds; 0 for pre-N4 rows).
    pub last_met_unix: u64,
    /// The current run: +n = our last n meetings were wins, -n = theirs.
    pub streak: i32,
    /// Filenames (not paths) of recent tapes against this rival, newest
    /// last, capped at [`RIVAL_TAPE_RING`]. The rivals screen plays them
    /// straight from `recorder::replays_dir()`.
    pub tapes: Vec<String>,
}

/// How many tapes a rivalry remembers. Older ones stay on disk for the
/// REPLAYS screen; the ledger keeps the recent conversation.
pub const RIVAL_TAPE_RING: usize = 4;

/// The next value of a win/loss streak. Pure for the tests: a streak
/// extends in its own direction and flips to ±1 on a reversal.
pub fn next_streak(streak: i32, won: bool) -> i32 {
    if won {
        if streak > 0 { streak + 1 } else { 1 }
    } else if streak < 0 {
        streak - 1
    } else {
        -1
    }
}

/// Meeting numbers the dark beyond celebrates (`dark_beyond` consumes
/// [`MilestoneFlareArmed`] on the next GO).
pub fn milestone_meeting(n: u32) -> bool {
    matches!(n, 10 | 50 | 100 | 500)
}

/// Armed when the CURRENT online match is a milestone meeting; the dark
/// beyond's every eye flares on the first GO, then this disarms.
#[derive(Resource, Default, Clone, Copy)]
pub struct MilestoneFlareArmed(pub bool);

/// Arm the milestone flare the moment the peer's identity lands during a
/// live match: meetings()+1 is the meeting now being played.
fn arm_milestone_flare(
    screen: Res<State<crate::screen::AppScreen>>,
    peer: Res<net::PeerProfile>,
    record: Res<CareerRecord>,
    mut armed: ResMut<MilestoneFlareArmed>,
    mut seen: Local<Option<u128>>,
) {
    if *screen.get() != crate::screen::AppScreen::InMatch {
        *seen = None;
        return;
    }
    let Some(peer) = peer.0 else {
        return;
    };
    if *seen == Some(peer.install_id) {
        return;
    }
    *seen = Some(peer.install_id);
    let n = record
        .rivals
        .get(&rival_key(peer.install_id))
        .map(|r| r.meetings())
        .unwrap_or(0)
        + 1;
    if milestone_meeting(n) {
        armed.0 = true;
        tracing::info!(target: "two_top::grudge", meeting = n, "milestone meeting — the dark beyond is watching");
    }
}

impl RivalRecord {
    pub fn meetings(&self) -> u32 {
        self.wins + self.losses
    }
}

/// Lifetime record. Loaded at boot, saved on every decided match.
/// `#[serde(default)]` keeps v1 career.json files (wins/losses only)
/// loading cleanly with the new fields defaulted.
#[derive(Resource, Serialize, Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct CareerRecord {
    pub wins: u32,
    pub losses: u32,
    /// Current practice-ladder tier (resets to 0 on a loss to the bot).
    pub gauntlet_tier: u32,
    /// Highest tier ever reached.
    pub gauntlet_best: u32,
    /// Per-opponent records, keyed by the peer install-id in lowercase hex.
    pub rivals: BTreeMap<String, RivalRecord>,
}

impl CareerRecord {
    pub fn total(&self) -> u32 {
        self.wins + self.losses
    }

    /// What to call a peer on screen.
    ///
    /// Names are not unique and cannot be: there is no server here to
    /// enforce it, and Riot's own postmortem is that hunting for an
    /// unclaimed name is where new players quit. So the ledger keys on the
    /// install-id (two MORGANs are already two rows, correctly) and the
    /// DISPLAY borrows the Riot ID shape — name plus a short tag — but only
    /// on the day it earns its keep: the tag appears when this ledger
    /// actually holds another identity wearing the same name. Meet one
    /// MORGAN and they are MORGAN forever; meet a second and they both
    /// become MORGAN#XYZ, at the moment the distinction starts to matter.
    pub fn display_name(&self, peer: net::ProfileData) -> String {
        let name = crate::profile::peer_name(Some(peer));
        let key = rival_key(peer.install_id);
        let collides = self.rivals.iter().any(|(k, r)| *k != key && r.name == name);
        if collides {
            format!("{name}#{}", crate::profile::identity_tag(peer.install_id))
        } else {
            name
        }
    }

    /// The rivalry line for the CURRENT match against `peer` — counting
    /// this meeting. `None` when no identity arrived (offline peer build,
    /// or the handshake hasn't landed yet).
    pub fn rivalry_line(&self, peer: Option<net::ProfileData>) -> Option<String> {
        let peer = peer?;
        let key = rival_key(peer.install_id);
        let name = self.display_name(peer);
        let Some(rival) = self.rivals.get(&key) else {
            return Some(format!("FIRST MEETING with {name}"));
        };
        let n = rival.meetings() + 1;
        let standing = match rival.wins.cmp(&rival.losses) {
            std::cmp::Ordering::Greater => {
                format!("you lead {}-{}", rival.wins, rival.losses)
            }
            std::cmp::Ordering::Less => {
                format!("{} leads {}-{}", name, rival.losses, rival.wins)
            }
            std::cmp::Ordering::Equal => format!("tied {}-{}", rival.wins, rival.losses),
        };
        Some(format!("{} MEETING with {name} - {standing}", ordinal(n)))
    }
}

/// Install-id → ledger key (lowercase hex, stable and greppable).
pub fn rival_key(install_id: u128) -> String {
    format!("{install_id:032x}")
}

/// Quitting a live online duel is a loss, recorded on the spot — the same
/// honesty the away-grace forfeit applies to a phone that wandered off.
/// Called by the in-match QUIT path right before the socket teardown
/// (`record_match_result` can't cover it: the quitter leaves the screen
/// before any `MatchOver` tick happens on their side).
pub fn record_abandoned_loss(record: &mut CareerRecord, peer: Option<net::ProfileData>) {
    record.losses += 1;
    if let Some(peer) = peer {
        let rival = record.rivals.entry(rival_key(peer.install_id)).or_default();
        rival.name = crate::profile::peer_name(Some(peer));
        rival.losses += 1;
    }
    save_career(record);
    tracing::info!(target: "two_top::grudge", "abandoned duel recorded as a loss");
}

/// An attestation completed for a match we won on score: the rival's line
/// gains a provable win (`crate::attest` calls this after the sidecar is
/// on disk). Separate from the win/loss tally on purpose — wins count
/// whether or not the peer's build could sign.
pub fn record_attested_win(record: &mut CareerRecord, peer: net::ProfileData) {
    let rival = record.rivals.entry(rival_key(peer.install_id)).or_default();
    rival.attested_wins += 1;
    save_career(record);
    tracing::info!(target: "two_top::grudge", "attested win recorded");
}

/// 1 → 1ST, 2 → 2ND, 3 → 3RD, 4 → 4TH, 11-13 → TH (the English trap).
pub fn ordinal(n: u32) -> String {
    let suffix = match (n % 10, n % 100) {
        (1, 11) | (2, 12) | (3, 13) => "TH",
        (1, _) => "ST",
        (2, _) => "ND",
        (3, _) => "RD",
        _ => "TH",
    };
    format!("{n}{suffix}")
}

fn career_path() -> Option<PathBuf> {
    crate::paths::config_file("career.json")
}

fn load_career() -> CareerRecord {
    let Some(path) = career_path() else {
        return CareerRecord::default();
    };
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_career(record: &CareerRecord) {
    let Some(path) = career_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(record)
        && let Err(e) = crate::paths::write_atomic(&path, json.as_bytes())
    {
        tracing::warn!(target: "two_top::grudge", error = %e, "failed to save career record");
    }
}

/// Did we win this decided match? Score settles it when someone actually
/// reached the threshold; a forfeit goes to whoever stayed at the table.
/// Pure for testing.
pub fn match_won(our_score: u8, their_score: u8, forfeited: bool, we_went_absent: bool) -> bool {
    if our_score >= MATCH_WIN_THRESHOLD {
        return true;
    }
    if their_score >= MATCH_WIN_THRESHOLD {
        return false;
    }
    // Nobody reached the threshold: a forfeit decided it. If our own phone
    // went away, the walk-out is ours to own; otherwise the field is ours.
    forfeited && !we_went_absent
}

/// Commit the result on the tick a match is decided. Online only; the local
/// handle decides which side of the score is "ours".
#[allow(clippy::too_many_arguments)]
fn record_match_result(
    state: Res<MatchState>,
    score: Res<MatchScore>,
    netplay: Res<NetplayConfig>,
    practice: Res<crate::bot::PracticeMode>,
    theater: Res<crate::theater::TheaterMode>,
    local: Res<LocalPlayerHandle>,
    lobby: Res<net::LobbyState>,
    peer: Res<net::PeerProfile>,
    absence: Res<RecentAbsence>,
    time: Res<Time<Real>>,
    mut record: ResMut<CareerRecord>,
    mut prev_over: Local<bool>,
) {
    let over = matches!(*state, MatchState::MatchOver);
    let entered = over && !*prev_over;
    *prev_over = over;
    // Only live duels count — beating the bot is the gauntlet's business,
    // and a watched tape is nobody's.
    if !entered || netplay.room_url.is_none() || practice.0 || theater.active() {
        return;
    }
    let Some(handle) = local.0 else {
        return;
    };
    let (ours, theirs) = if handle == 0 {
        (score.p0, score.p1)
    } else {
        (score.p1, score.p0)
    };
    let forfeited = matches!(*lobby, net::LobbyState::Forfeited { .. });
    let we_went_absent = absence.within(time.elapsed_secs(), RecentAbsence::FORFEIT_BLAME_SECS);
    let won = match_won(ours, theirs, forfeited, we_went_absent);

    if won {
        record.wins += 1;
    } else {
        record.losses += 1;
    }
    if let Some(peer) = peer.0 {
        let rival = record.rivals.entry(rival_key(peer.install_id)).or_default();
        rival.name = crate::profile::peer_name(Some(peer));
        if won {
            rival.wins += 1;
        } else {
            rival.losses += 1;
        }
        rival.streak = next_streak(rival.streak, won);
        rival.last_met_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
    }
    save_career(&record);
}

/// The recorder froze a tape for a live duel: remember it on the rival's
/// ring so the rivals screen can replay the recent conversation.
pub fn note_rival_tape(record: &mut CareerRecord, peer: net::ProfileData, filename: String) {
    let rival = record.rivals.entry(rival_key(peer.install_id)).or_default();
    rival.tapes.push(filename);
    while rival.tapes.len() > RIVAL_TAPE_RING {
        rival.tapes.remove(0);
    }
    save_career(record);
}

/// The practice ladder: a decided bot match moves the gauntlet. Win → the
/// tier climbs (and the best-ever remembers); lose → back to the bottom.
fn record_gauntlet_result(
    state: Res<MatchState>,
    score: Res<MatchScore>,
    practice: Res<crate::bot::PracticeMode>,
    shade: Res<crate::bot::ShadeStyle>,
    theater: Res<crate::theater::TheaterMode>,
    mut record: ResMut<CareerRecord>,
    mut prev_over: Local<bool>,
) {
    let over = matches!(*state, MatchState::MatchOver);
    let entered = over && !*prev_over;
    *prev_over = over;
    if !entered || !practice.0 || theater.active() {
        return;
    }
    if shade.0.is_some() {
        // Sparring a shade moves NOTHING: not the rivalry (practice
        // already guards that) and not the tier — the ladder is the
        // ladder, and a fitted caricature is neither a human nor a rung.
        return;
    }
    // The human is always handle 0 in practice.
    if score.p0 >= MATCH_WIN_THRESHOLD {
        record.gauntlet_tier += 1;
        record.gauntlet_best = record.gauntlet_best.max(record.gauntlet_tier);
        tracing::info!(
            target: "two_top::grudge",
            tier = record.gauntlet_tier,
            best = record.gauntlet_best,
            "gauntlet tier climbed",
        );
    } else {
        record.gauntlet_tier = 0;
    }
    save_career(&record);
}

pub struct GrudgePlugin;

impl Plugin for GrudgePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(load_career())
            .init_resource::<MilestoneFlareArmed>()
            .add_systems(
                Update,
                (
                    record_match_result,
                    record_gauntlet_result,
                    arm_milestone_flare,
                ),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinals_speak_english() {
        assert_eq!(ordinal(1), "1ST");
        assert_eq!(ordinal(2), "2ND");
        assert_eq!(ordinal(3), "3RD");
        assert_eq!(ordinal(4), "4TH");
        assert_eq!(ordinal(11), "11TH");
        assert_eq!(ordinal(12), "12TH");
        assert_eq!(ordinal(13), "13TH");
        assert_eq!(ordinal(21), "21ST");
        assert_eq!(ordinal(102), "102ND");
    }

    #[test]
    fn earned_scores_beat_forfeit_reasoning() {
        // Threshold reached: the score is the verdict, absence irrelevant.
        assert!(match_won(MATCH_WIN_THRESHOLD, 3, true, true));
        assert!(!match_won(2, MATCH_WIN_THRESHOLD, true, false));
    }

    #[test]
    fn forfeits_go_to_whoever_stayed() {
        // The v1 bug: the survivor of a fled match must record a WIN.
        assert!(match_won(2, 1, true, false));
        // The one whose phone went away owns the loss.
        assert!(!match_won(2, 1, true, true));
        // No forfeit and no threshold: not a win (shouldn't happen online).
        assert!(!match_won(2, 1, false, false));
    }

    #[test]
    fn streaks_extend_and_flip() {
        assert_eq!(next_streak(0, true), 1);
        assert_eq!(next_streak(3, true), 4);
        assert_eq!(next_streak(3, false), -1, "a reversal starts their run");
        assert_eq!(next_streak(-2, false), -3);
        assert_eq!(next_streak(-2, true), 1);
    }

    #[test]
    fn the_tape_ring_keeps_the_recent_conversation() {
        let mut record = CareerRecord::default();
        let peer = net::ProfileData {
            install_id: 0xabc,
            name: net::name_slots(&[0]),
        };
        for i in 0..6 {
            note_rival_tape(&mut record, peer, format!("t{i}.bmrg"));
        }
        let rival = &record.rivals[&rival_key(peer.install_id)];
        assert_eq!(rival.tapes.len(), RIVAL_TAPE_RING);
        assert_eq!(rival.tapes.first().unwrap(), "t2.bmrg", "oldest dropped");
        assert_eq!(rival.tapes.last().unwrap(), "t5.bmrg");
    }

    #[test]
    fn milestones_are_the_meetings_worth_a_flare() {
        assert!(milestone_meeting(10));
        assert!(milestone_meeting(100));
        assert!(!milestone_meeting(9));
        assert!(!milestone_meeting(11));
    }

    #[test]
    fn v1_career_files_still_load() {
        let v1 = r#"{ "wins": 7, "losses": 4 }"#;
        let career: CareerRecord = serde_json::from_str(v1).unwrap();
        assert_eq!(career.wins, 7);
        assert_eq!(career.losses, 4);
        assert_eq!(career.gauntlet_tier, 0);
        assert!(career.rivals.is_empty());
    }

    #[test]
    fn the_tag_appears_only_once_two_rivals_share_a_name() {
        let mut career = CareerRecord::default();
        let morgan_a = net::ProfileData {
            install_id: 0xa11,
            name: net::name_slots(&[12, 14, 17, 6, 0, 13]), // MORGAN
        };
        let morgan_b = net::ProfileData {
            install_id: 0xb22,
            name: net::name_slots(&[12, 14, 17, 6, 0, 13]), // MORGAN too
        };
        // One MORGAN in the ledger: they are just MORGAN.
        career.rivals.insert(
            rival_key(morgan_a.install_id),
            RivalRecord {
                name: "MORGAN".into(),
                wins: 1,
                losses: 0,
                ..Default::default()
            },
        );
        assert_eq!(career.display_name(morgan_a), "MORGAN");
        // A second, different identity wearing the same name: now both
        // carry the tag, and the two tags differ.
        career.rivals.insert(
            rival_key(morgan_b.install_id),
            RivalRecord {
                name: "MORGAN".into(),
                wins: 0,
                losses: 1,
                ..Default::default()
            },
        );
        let (a, b) = (career.display_name(morgan_a), career.display_name(morgan_b));
        assert!(
            a.starts_with("MORGAN#") && b.starts_with("MORGAN#"),
            "{a} / {b}"
        );
        assert_ne!(a, b, "the tag is what tells them apart");
        // An unrelated name is untouched by their collision.
        let suds = net::ProfileData {
            install_id: 0xc33,
            name: net::name_slots(&[18, 20, 3, 18]), // SUDS
        };
        assert_eq!(career.display_name(suds), "SUDS");
    }

    #[test]
    fn rivalry_line_counts_the_current_meeting() {
        let mut career = CareerRecord::default();
        let peer = net::ProfileData {
            install_id: 0xabc,
            name: net::name_slots(&[19, 0, 6, 2]), // TAGC
        };
        assert_eq!(
            career.rivalry_line(Some(peer)).unwrap(),
            "FIRST MEETING with TAGC"
        );
        career.rivals.insert(
            rival_key(0xabc),
            RivalRecord {
                name: "TAGC".into(),
                wins: 2,
                losses: 1,
                ..Default::default()
            },
        );
        assert_eq!(
            career.rivalry_line(Some(peer)).unwrap(),
            "4TH MEETING with TAGC - you lead 2-1"
        );
        assert_eq!(career.rivalry_line(None), None);
    }
}
