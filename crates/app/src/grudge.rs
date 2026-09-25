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
    /// Meetings a silent drop ended — nobody's win, nobody's loss, and
    /// the streak untouched. Counted so the rivalry still remembers it
    /// happened.
    pub unfinished: u32,
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
        self.wins + self.losses + self.unfinished
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
    /// Online matches a silent drop ended (see [`Outcome::Unfinished`]).
    pub unfinished: u32,
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
    career_path().map_or_else(CareerRecord::default, |p| read_career(&p))
}

/// Read + parse the ledger. An absent file is a fresh career. A file that
/// exists but will not parse is quarantined as a `.corrupt` sibling — the
/// same treatment `profile.json` gets, and for a stronger reason: this
/// file is the gauntlet tier, every rivalry and every tape ring, and the
/// old path handed back a default that the next decided match then wrote
/// over the only evidence of what happened.
fn read_career(path: &std::path::Path) -> CareerRecord {
    let Ok(text) = crate::paths::read_document(path) else {
        return CareerRecord::default();
    };
    match serde_json::from_str(&text) {
        Ok(record) => record,
        Err(e) => {
            tracing::error!(
                target: "two_top::grudge",
                error = %e,
                "career.json is corrupt — quarantining it and starting a fresh ledger",
            );
            crate::paths::quarantine_corrupt(path);
            CareerRecord::default()
        }
    }
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

/// How a forfeit reached us. Read off `LobbyState::Forfeited.conceded`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForfeitKind {
    /// The peer said goodbye: a deliberate quit, and their app filed the
    /// loss before sending it. The stayer may bank the win.
    Conceded,
    /// ggrs timed the peer out, or the silence FSM fired. A tunnel, a
    /// Wi-Fi→cellular handoff, a pulled cable — or airplane mode from a
    /// player down 1-4. Nothing here says which.
    Silent,
}

/// The forfeit kind the lobby is reporting, if any.
pub fn forfeit_kind(lobby: &net::LobbyState) -> Option<ForfeitKind> {
    match lobby {
        net::LobbyState::Forfeited { conceded: true, .. } => Some(ForfeitKind::Conceded),
        net::LobbyState::Forfeited {
            conceded: false, ..
        } => Some(ForfeitKind::Silent),
        _ => None,
    }
}

/// How a decided online match lands on the ledger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Won,
    Lost,
    /// Nobody reached the threshold and nobody conceded. Counts as a
    /// meeting; moves neither W/L nor the streak.
    Unfinished,
}

/// Score settles it when someone actually reached the threshold. Otherwise
/// a forfeit decided it, and only two facts can honestly be banked: a
/// goodbye is a concession, and our own freeze long enough to time us out
/// on the other phone is our loss. A silent drop proves nothing about who
/// left — both phones used to record a WIN for it, so every network drop
/// was double-credited and airplane mode was a free win. Pure for testing.
pub fn match_outcome(
    our_score: u8,
    their_score: u8,
    forfeit: Option<ForfeitKind>,
    we_went_absent: bool,
) -> Outcome {
    if our_score >= MATCH_WIN_THRESHOLD {
        return Outcome::Won;
    }
    if their_score >= MATCH_WIN_THRESHOLD {
        return Outcome::Lost;
    }
    match forfeit {
        Some(ForfeitKind::Conceded) => Outcome::Won,
        _ if we_went_absent => Outcome::Lost,
        Some(ForfeitKind::Silent) | None => Outcome::Unfinished,
    }
}

/// Commit the result on the tick a match is decided. Online only; the local
/// handle decides which side of the score is "ours".
#[allow(clippy::too_many_arguments)]
fn record_match_result(
    settled: Res<crate::attest::MatchOverSettled>,
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
    // On the SETTLED edge, not the raw MatchOver one: a predicted deciding
    // kill stands for up to the prediction window before a rollback
    // un-ends it, and this used to write the ledger twice for one match.
    let over = settled.settled;
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
    if matches!(*lobby, net::LobbyState::Desynced { .. }) {
        // Two phones that stopped agreeing played two different matches;
        // neither result is a fact the other phone shares. Record nothing.
        tracing::warn!(target: "two_top::grudge", "match desynced — no result recorded");
        return;
    }
    let (ours, theirs) = if handle == 0 {
        (score.p0, score.p1)
    } else {
        (score.p1, score.p0)
    };
    let forfeit = forfeit_kind(&lobby);
    let we_went_absent = absence.within(time.elapsed_secs(), RecentAbsence::FORFEIT_BLAME_SECS);
    let outcome = match_outcome(ours, theirs, forfeit, we_went_absent);

    match outcome {
        Outcome::Won => record.wins += 1,
        Outcome::Lost => record.losses += 1,
        Outcome::Unfinished => record.unfinished += 1,
    }
    if let Some(peer) = peer.0 {
        let rival = record.rivals.entry(rival_key(peer.install_id)).or_default();
        rival.name = crate::profile::peer_name(Some(peer));
        match outcome {
            Outcome::Won => {
                rival.wins += 1;
                rival.streak = next_streak(rival.streak, true);
            }
            Outcome::Lost => {
                rival.losses += 1;
                rival.streak = next_streak(rival.streak, false);
            }
            Outcome::Unfinished => rival.unfinished += 1,
        }
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

/// The last rung that changes the opponent. `bot.rs`'s knobs saturate at
/// difficulty 11 (`difficulty = tier + in_match_ramp`, the ramp being one
/// late notch), so a tier-10 bot ends its match fully sharpened and every
/// tier past it would be the same duelist under a bigger number. The
/// counter stops here and says so.
pub const GAUNTLET_MAX_TIER: u32 = 10;

/// The tier a win climbs to.
pub fn next_tier_after_win(tier: u32) -> u32 {
    (tier + 1).min(GAUNTLET_MAX_TIER)
}

/// The tier a loss (or a quit from a live match) falls to: one rung under
/// the tier just lost at, never back to the passive dummy once it has been
/// cleared (the dummy is worth exactly one visit).
///
/// This used to be "two rungs under the BEST ever reached", which made the
/// floor permanent: a player who once climbed to tier 8 could lose forever
/// and never face anything softer than tier 6. Practice has to stay
/// practice — keep losing and the bot keeps easing off until you can beat
/// it again.
pub fn loss_tier(tier: u32) -> u32 {
    tier.saturating_sub(1).max(1).min(tier)
}

/// At the cap the label says so, instead of promising a next rung.
pub fn gauntlet_mastered(tier: u32) -> bool {
    tier >= GAUNTLET_MAX_TIER
}

/// Quitting a live gauntlet match is a loss, the same as quitting a live
/// duel — the ladder's only downward pressure used to be choosing to sit
/// through a loss. Called by the in-match QUIT path; a match still in its
/// countdown, or a shade spar, stakes nothing and never reaches here.
pub fn record_gauntlet_quit(record: &mut CareerRecord) {
    if record.gauntlet_tier == 0 {
        return;
    }
    record.gauntlet_tier = loss_tier(record.gauntlet_tier);
    tracing::info!(
        target: "two_top::grudge",
        tier = record.gauntlet_tier,
        "gauntlet match quit — tier falls",
    );
    save_career(record);
}

/// The practice ladder: a decided bot match moves the gauntlet. Win → the
/// tier climbs to the cap (and the best-ever remembers); lose → one rung
/// down, never back to the dummy.
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
        record.gauntlet_tier = next_tier_after_win(record.gauntlet_tier);
        record.gauntlet_best = record.gauntlet_best.max(record.gauntlet_tier);
        tracing::info!(
            target: "two_top::grudge",
            tier = record.gauntlet_tier,
            best = record.gauntlet_best,
            "gauntlet tier climbed",
        );
    } else {
        record.gauntlet_tier = loss_tier(record.gauntlet_tier);
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
                    record_match_result.after(crate::attest::settle_match_over),
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
        use ForfeitKind::*;
        // Threshold reached: the score is the verdict, absence irrelevant.
        assert_eq!(
            match_outcome(MATCH_WIN_THRESHOLD, 3, Some(Silent), true),
            Outcome::Won
        );
        assert_eq!(
            match_outcome(2, MATCH_WIN_THRESHOLD, Some(Conceded), false),
            Outcome::Lost
        );
    }

    #[test]
    fn only_a_goodbye_concedes_and_only_our_own_freeze_loses() {
        use ForfeitKind::*;
        // A goodbye is a deliberate quit: the stayer banks the win.
        assert_eq!(match_outcome(2, 1, Some(Conceded), false), Outcome::Won);
        // Our own phone froze long enough to be timed out: the loss is ours.
        assert_eq!(match_outcome(2, 1, Some(Silent), true), Outcome::Lost);
        assert_eq!(match_outcome(2, 1, None, true), Outcome::Lost);
        // A silent drop with no fact behind it: BOTH phones used to bank a
        // win here, and airplane mode at 1-4 was a free one. Unfinished.
        assert_eq!(
            match_outcome(2, 1, Some(Silent), false),
            Outcome::Unfinished
        );
        assert_eq!(
            match_outcome(1, 4, Some(Silent), false),
            Outcome::Unfinished
        );
        assert_eq!(match_outcome(2, 1, None, false), Outcome::Unfinished);
    }

    #[test]
    fn an_unfinished_meeting_still_counts_as_a_meeting() {
        let r = RivalRecord {
            wins: 2,
            losses: 1,
            unfinished: 3,
            ..Default::default()
        };
        assert_eq!(r.meetings(), 6);
        // And a v1 row without the field reads as zero.
        let v1: RivalRecord = serde_json::from_str(r#"{ "wins": 1, "losses": 1 }"#).unwrap();
        assert_eq!(v1.unfinished, 0);
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
    fn a_corrupt_career_file_is_quarantined_not_overwritten() {
        let dir = crate::paths::test_scratch("career_corrupt");
        let path = dir.join("career.json");
        std::fs::write(&path, b"{ this is not json").unwrap();
        let career = read_career(&path);
        assert_eq!(career.wins, 0);
        assert_eq!(career.gauntlet_tier, 0);
        assert!(career.rivals.is_empty());
        assert!(!path.exists(), "the corrupt file is moved aside");
        assert_eq!(
            std::fs::read(dir.join("career.json.corrupt")).unwrap(),
            b"{ this is not json",
            "the evidence survives for a human to hand back"
        );
        // And an absent file is simply a fresh career, no quarantine.
        let fresh = read_career(&dir.join("never_written.json"));
        assert_eq!(fresh.wins, 0);
        assert!(!dir.join("never_written.json.corrupt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_ladder_caps_where_the_bot_stops_changing() {
        assert_eq!(next_tier_after_win(0), 1);
        assert_eq!(next_tier_after_win(9), GAUNTLET_MAX_TIER);
        assert_eq!(next_tier_after_win(GAUNTLET_MAX_TIER), GAUNTLET_MAX_TIER);
        assert!(gauntlet_mastered(GAUNTLET_MAX_TIER));
        assert!(!gauntlet_mastered(GAUNTLET_MAX_TIER - 1));
    }

    #[test]
    fn a_loss_falls_one_rung_and_never_back_to_the_dummy() {
        assert_eq!(loss_tier(0), 0, "never cleared the dummy: it waits");
        assert_eq!(loss_tier(1), 1);
        assert_eq!(loss_tier(2), 1);
        assert_eq!(loss_tier(3), 2);
        assert_eq!(loss_tier(8), 7);
        assert_eq!(loss_tier(GAUNTLET_MAX_TIER), GAUNTLET_MAX_TIER - 1);
    }

    /// The best-ever tier is a trophy, not a floor: a player who once
    /// reached the top can lose their way all the way back to tier 1.
    #[test]
    fn repeated_losses_keep_easing_the_bot() {
        let mut tier = GAUNTLET_MAX_TIER;
        for _ in 0..GAUNTLET_MAX_TIER {
            tier = loss_tier(tier);
        }
        assert_eq!(tier, 1);
    }

    #[test]
    fn quitting_a_live_gauntlet_match_costs_the_same_as_losing_it() {
        let mut record = CareerRecord {
            gauntlet_tier: 5,
            gauntlet_best: 7,
            ..Default::default()
        };
        record_gauntlet_quit(&mut record);
        assert_eq!(record.gauntlet_tier, loss_tier(5));
        assert_eq!(
            record.gauntlet_tier, 4,
            "falls from where it stood, not from the best"
        );
        let mut fresh = CareerRecord::default();
        record_gauntlet_quit(&mut fresh);
        assert_eq!(fresh.gauntlet_tier, 0, "tier 0 has nothing to lose");
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
