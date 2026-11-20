//! Match recorder — every match becomes a shareable `.bmrg` replay.
//!
//! The whole architecture is already a replay machine: the sim is
//! bit-deterministic and driven only by the input tape, so a complete match
//! is a few KB that reproduces perfectly on any device (and scrubs in
//! `replay_viewer`). This module writes that tape to a plain file the
//! player can share however they like.
//!
//! ## Why capture per tick inside `GgrsSchedule` (not `LocalInputs`)
//!
//! `replay::RecordPlugin` captures `LocalInputs` in `ReadInputs` — complete
//! for couch (both players local), but an ONLINE recording would miss the
//! peer's half entirely. Inside the rollback schedule, `PlayerInputs`
//! carries BOTH players' inputs for the tick being simulated — including
//! every re-simulated tick, whose corrected inputs simply overwrite their
//! slot in the tape. Last write wins, so the tape converges to the
//! confirmed inputs exactly as the game state itself does, and because the
//! schedule runs once per tick there is no window a frame hitch can gap.
//! (The previous design harvested from the 8-tick `InputHistory` ring once
//! per render frame and had to poison the tape on any >8-tick hitch — which
//! match-start shader compiles hit almost every time, so tapes essentially
//! never saved. That surrender path is gone, not tuned.)
//!
//! The capture only READS rollback state and writes this render-side tape;
//! nothing feeds back into sim, so determinism is untouched.

use bevy::prelude::*;
use bevy_ggrs::GgrsSchedule;
use replay::{FORMAT_VERSION, MAGIC, Replay, ReplayHeader, encode};
use sim::{FrameCount, MatchScore, MatchState, PlayerInput, SelectedArena, TICK_HZ};
use std::path::PathBuf;

/// A tick-0 write against a tape longer than this is a fresh session (new
/// match from the title / an online re-pair), never a rollback — ggrs can
/// only rewind a handful of ticks (prediction window ≤ 8, SyncTest
/// check-distance ≤ 7), so a genuine resim of tick 0 can only happen while
/// the tape is still shorter than this.
const SESSION_RESTART_SLACK: usize = 32;

/// The in-progress tape plus save bookkeeping.
#[derive(Resource, Default)]
pub struct MatchRecorder {
    frames: Vec<replay::FrameInputs>,
    /// The score after each tick, beside its inputs: the same last-write-
    /// wins convergence, so the slot of a confirmed tick holds the
    /// confirmed score (what a session that ends books,
    /// `netplay::conclude_session`).
    scores: Vec<MatchScore>,
    /// Latch so one MatchOver writes exactly one file.
    saved: bool,
    /// Every confirmed kill of the session: (tick, victim seat).
    kills: Vec<(u32, u8)>,
}

impl MatchRecorder {
    fn reset(&mut self) {
        *self = Self::default();
    }

    /// The tick the match that ends at `end` (exclusive) began on: the last
    /// rematch in the score trail (the score falling back to 0-0), or 0.
    pub fn match_start(&self, end: usize) -> u32 {
        let scores = &self.scores[..end.min(self.scores.len())];
        scores
            .windows(2)
            .rposition(|w| w[0] != MatchScore::default() && w[1] == MatchScore::default())
            .map_or(0, |i| i as u32 + 1)
    }

    /// The recorded inputs of the first `ticks` ticks (fewer if fewer were
    /// recorded): what a resume rebuilds the match from.
    pub fn inputs_through(&self, ticks: usize) -> Vec<replay::FrameInputs> {
        self.frames[..ticks.min(self.frames.len())].to_vec()
    }

    /// The score after tick `tick` (`-1`, or a tick not recorded: 0-0).
    pub fn score_after(&self, tick: i32) -> MatchScore {
        usize::try_from(tick)
            .ok()
            .and_then(|t| self.scores.get(t).copied())
            .unwrap_or_default()
    }
}

/// How many ticks of the tape are fact, and so go in the file. A decided
/// match: through its deciding tick (both phones' tapes then hold the same
/// ticks, byte for byte). A session that ended otherwise: through the last
/// confirmed tick. A desync: none. A local match (couch, practice) that
/// was neither: all of it. Pure.
pub fn tape_ticks(
    state: MatchState,
    ending: Option<crate::netplay::Ending>,
    desynced: bool,
    recorded: usize,
) -> usize {
    if desynced {
        return 0;
    }
    let ticks = if let Some(decided) = state.decided_at() {
        decided as usize + 1
    } else if let Some(ending) = ending {
        usize::try_from(ending.confirmed + 1).unwrap_or(0)
    } else {
        recorded
    };
    ticks.min(recorded)
}

/// Where the replay of the last decided match landed (the summary screen
/// surfaces it).
#[derive(Resource, Default)]
pub struct LastSavedReplay(pub Option<PathBuf>);

/// `GgrsSchedule`, after the whole sim chain: record the tick that was just
/// simulated. Runs for every tick INCLUDING resimulations — a resimulated
/// tick overwrites its slot with the corrected inputs, so the tape
/// converges to confirmed values by construction.
pub fn record_tick(
    frame: Res<FrameCount>,
    inputs: Res<sim::TickInputs>,
    score: Res<MatchScore>,
    theater: Res<crate::theater::TheaterMode>,
    mut rec: ResMut<MatchRecorder>,
) {
    // A replay being WATCHED must not re-record itself into a copy tape.
    if theater.active() {
        if !rec.frames.is_empty() {
            rec.reset();
        }
        return;
    }
    // Ordered after `advance_frame_count`, so the tick just simulated with
    // these inputs is `frame - 1`.
    let Some(t) = frame.0.checked_sub(1) else {
        return;
    };
    let t = t as usize;
    if t == 0 && rec.frames.len() > SESSION_RESTART_SLACK {
        // A fresh session always restarts at tick 0; start a fresh tape.
        rec.reset();
    }
    let pair = inputs.0;
    let score = *score;
    match t.cmp(&rec.frames.len()) {
        std::cmp::Ordering::Less => {
            rec.frames[t] = pair;
            rec.scores[t] = score;
        }
        std::cmp::Ordering::Equal => {
            rec.frames.push(pair);
            rec.scores.push(score);
        }
        std::cmp::Ordering::Greater => {
            // Unreachable by construction (the schedule runs every tick from
            // 0) — but a tape must never be silently holey, so pad loudly.
            tracing::warn!(
                target: "two_top::recorder",
                tick = t,
                have = rec.frames.len(),
                "tick capture gap — padding with neutral inputs",
            );
            rec.frames.resize(t, [PlayerInput::default(); 2]);
            rec.frames.push(pair);
            let last = rec.scores.last().copied().unwrap_or_default();
            rec.scores.resize(t, last);
            rec.scores.push(score);
        }
    }
}

/// The two duelist names for the tape header, by handle. Online: the local
/// profile fills our seat and the peer's `Profile` handshake fills theirs.
/// Practice names the bot honestly. Couch leaves both `None` (two humans,
/// one device — the viewer falls back to CUR/STAG).
fn header_names(
    netplay: &crate::netplay::NetplayConfig,
    practice: bool,
    shade: Option<&str>,
    local: Option<usize>,
    profile: &crate::profile::LocalProfile,
    peer: &net::PeerProfile,
) -> [Option<String>; 2] {
    if practice {
        let far = match shade {
            Some(name) => format!("{name} SHADE"),
            None => "BOT".to_string(),
        };
        return [Some(profile.name_string()), Some(far)];
    }
    if netplay.room_url.is_none() {
        return [None, None];
    }
    let mut names = [None, None];
    let me = local.unwrap_or(0);
    names[me] = Some(profile.name_string());
    if let Some(peer) = peer.0 {
        names[1 - me] = Some(crate::profile::name_from_slots(&peer.name));
    }
    names
}

/// Collect the session's confirmed kills for the tape's header.
fn note_confirmed_kills(
    mut kills: MessageReader<render::facts::KillConfirmed>,
    mut rec: ResMut<MatchRecorder>,
) {
    for kill in kills.read() {
        let entry = (kill.tick, kill.victim as u8);
        if !rec.kills.contains(&entry) {
            rec.kills.push(entry);
        }
    }
}

/// What this tape is a recording of. Pure.
pub fn tape_kind(
    online: bool,
    practice: bool,
    shade: bool,
    lesson: bool,
    tier: u32,
) -> replay::TapeKind {
    match (online, practice, shade, lesson) {
        (_, true, true, _) => replay::TapeKind::Shade,
        (_, true, _, true) => replay::TapeKind::Lesson,
        (_, true, _, _) => replay::TapeKind::Practice {
            tier: tier.min(u8::MAX as u32) as u8,
        },
        (true, false, _, _) => replay::TapeKind::Duel,
        _ => replay::TapeKind::Couch,
    }
}

/// Once the match's ending can no longer roll back
/// (`attest::MatchOverSettled`), write the ticks that are fact
/// ([`tape_ticks`]) as a `.bmrg`.
#[allow(clippy::too_many_arguments)]
fn save_replay_on_match_over(
    state: Res<MatchState>,
    score: Res<MatchScore>,
    selected: Res<SelectedArena>,
    netplay: Res<crate::netplay::NetplayConfig>,
    practice: Res<crate::bot::PracticeMode>,
    local: Res<crate::netplay::LocalPlayerHandle>,
    profile: Res<crate::profile::LocalProfile>,
    peer: Res<net::PeerProfile>,
    shade: Res<crate::bot::ShadeStyle>,
    lobby: Res<net::LobbyState>,
    absence: Res<crate::netplay::RecentAbsence>,
    time: Res<Time<Real>>,
    (settled, ending, first_blood): (
        Res<crate::attest::MatchOverSettled>,
        Res<crate::netplay::SessionEnding>,
        Res<crate::first_blood::FirstBlood>,
    ),
    mut record: ResMut<crate::grudge::CareerRecord>,
    mut rec: ResMut<MatchRecorder>,
    mut last_saved: ResMut<LastSavedReplay>,
) {
    if !state.is_match_over() {
        // Leaving MatchOver (rematch / lobby): re-arm for the next decision.
        rec.saved = false;
        return;
    }
    if rec.saved || !settled.settled {
        return;
    }
    let desynced = matches!(*lobby, net::LobbyState::Desynced { .. });
    let ticks = tape_ticks(*state, ending.0, desynced, rec.frames.len());
    if ticks == 0 {
        // A desync (two phones that stopped agreeing recorded two
        // different matches: a tape of one is a record of nothing both
        // players saw), or a session that ended before a single tick was
        // confirmed. No tape, no share, no rival ring.
        rec.saved = true;
        tracing::warn!(target: "two_top::recorder", desynced, "nothing confirmed to keep — no tape written");
        return;
    }
    {
        {
            rec.saved = true;
            // Threshold first; on a forfeit (nobody reached it) the tape
            // is the SURVIVOR's recording, so the walkover goes to
            // whichever seat stayed — the same honesty the grudge ledger
            // applies. The first cut stamped `else 1`, which named the
            // Stag winner of a match the Cur won by walkover (caught by
            // the automated Rung 2 smoke's tape filename).
            // A silent drop crowns nobody (`winner: None`) — the same
            // verdict the ledger reaches in `grudge::match_outcome`.
            let winner: Option<u8> = if let Some(seat) = score.winner() {
                Some(seat)
            } else {
                let forfeit = crate::grudge::forfeit_kind(&lobby);
                let local_handle = local.0.unwrap_or(0);
                let we_went_absent = absence.within(
                    time.elapsed_secs(),
                    crate::netplay::RecentAbsence::FORFEIT_BLAME_SECS,
                );
                let (ours, theirs) = if local_handle == 0 {
                    (score.p0, score.p1)
                } else {
                    (score.p1, score.p0)
                };
                match crate::grudge::match_outcome(ours, theirs, forfeit, we_went_absent) {
                    crate::grudge::Outcome::Won => Some(local_handle as u8),
                    crate::grudge::Outcome::Lost => Some(1 - local_handle as u8),
                    crate::grudge::Outcome::Unfinished => None,
                }
            };
            let start_tick = rec.match_start(ticks);
            let recorded_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let replay = Replay {
                header: ReplayHeader {
                    magic: MAGIC,
                    format_version: FORMAT_VERSION,
                    sim_version: sim::SIM_VERSION,
                    seed: 0,
                    num_players: 2,
                    frame_rate: TICK_HZ as u8,
                    frame_count: ticks as u32,
                    recorded_at,
                    winner,
                    player_handles: header_names(
                        &netplay,
                        practice.0,
                        shade.0.as_ref().map(|s| s.name.as_str()),
                        local.0,
                        &profile,
                        &peer,
                    ),
                    arena_id: selected.0.as_u8(),
                    start_tick,
                    kind: tape_kind(
                        netplay.room_url.is_some(),
                        practice.0,
                        shade.0.is_some(),
                        first_blood.active,
                        record.gauntlet_tier,
                    ),
                    kills: rec
                        .kills
                        .iter()
                        .copied()
                        .filter(|(tick, _)| (start_tick..ticks as u32).contains(tick))
                        .collect(),
                },
                inputs: rec.frames[..ticks].to_vec(),
            };
            last_saved.0 = write_replay(&replay, recorded_at, winner);
            if let Some(dir) = replays_dir() {
                prune_tapes(&dir, &record);
            }
            // A live duel's tape lands on the rival's ring too, so the
            // rivals screen can replay the recent conversation.
            if let (Some(path), Some(peer), false) = (&last_saved.0, peer.0, practice.0)
                && netplay.room_url.is_some()
                && let Some(name) = path.file_name().and_then(|n| n.to_str())
            {
                crate::grudge::note_rival_tape(&mut record, peer, name.to_string());
            }
        }
    }
}

/// Encode + write the tape. Returns the path on success.
/// Tapes kept per kind: duels are the record of a rivalry; practice,
/// couch and the rest are kept for a while.
pub const KEEP_DUELS: usize = 200;
pub const KEEP_OTHERS: usize = 40;

/// Which of `tapes` (file name, recorded-at, is a duel) to delete: past the
/// newest [`KEEP_DUELS`] duels and [`KEEP_OTHERS`] others, never one that
/// is `protected` (on a rival's ring, or carrying a signed result). Pure.
pub fn tapes_to_prune(
    tapes: &[(String, u64, bool)],
    protected: &std::collections::BTreeSet<String>,
) -> Vec<String> {
    let mut sorted: Vec<&(String, u64, bool)> = tapes.iter().collect();
    sorted.sort_by_key(|(_, at, _)| std::cmp::Reverse(*at));
    let (mut duels, mut others) = (0usize, 0usize);
    let mut out = Vec::new();
    for (name, _, duel) in sorted {
        let kept = if *duel { &mut duels } else { &mut others };
        *kept += 1;
        let limit = if *duel { KEEP_DUELS } else { KEEP_OTHERS };
        if *kept > limit && !protected.contains(name) {
            out.push(name.clone());
        }
    }
    out
}

/// After a save: delete the tapes past the keep limits. A rival's ring and
/// every signed result are never touched.
fn prune_tapes(dir: &std::path::Path, record: &crate::grudge::CareerRecord) {
    let mut protected: std::collections::BTreeSet<String> = record
        .rivals
        .values()
        .flat_map(|r| r.tapes.iter().cloned())
        .collect();
    let mut tapes = Vec::new();
    for path in crate::paths::list_dir(dir, "bmrg") {
        let Some(name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        if path.with_extension("attest.json").exists() {
            protected.insert(name.clone());
        }
        let Ok(bytes) = crate::paths::read_bytes(&path) else {
            continue;
        };
        let Ok(header) = replay::decode_header(&bytes) else {
            continue; // not ours to judge
        };
        tapes.push((
            name,
            header.recorded_at,
            header.kind == replay::TapeKind::Duel,
        ));
    }
    for name in tapes_to_prune(&tapes, &protected) {
        let _ = std::fs::remove_file(dir.join(&name));
        tracing::info!(target: "two_top::recorder", tape = name, "pruned an old tape");
    }
}

fn write_replay(replay: &Replay, recorded_at: u64, winner: Option<u8>) -> Option<PathBuf> {
    let bytes = match encode(replay) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(target: "two_top::recorder", error = %e, "replay encode failed");
            return None;
        }
    };
    let Some(dir) = replays_dir() else {
        tracing::warn!(target: "two_top::recorder", "no writable replay dir on this platform");
        return None;
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!(target: "two_top::recorder", error = %e, "cannot create replay dir");
        return None;
    }
    let name = format!(
        "match_{recorded_at}_{}.bmrg",
        match winner {
            Some(0) => "curwins",
            Some(_) => "stagwins",
            None => "unfinished",
        }
    );
    let path = dir.join(name);
    match crate::paths::write_atomic(&path, &bytes) {
        Ok(()) => {
            tracing::info!(
                target: "two_top::recorder",
                path = %path.display(),
                bytes = bytes.len(),
                "match replay saved",
            );
            Some(path)
        }
        Err(e) => {
            tracing::warn!(target: "two_top::recorder", error = %e, "replay write failed");
            None
        }
    }
}

/// The replay landing spot — a plain folder the player can reach and share
/// from. Desktop: `~/Downloads/two-top/replays` (data-dir fallback).
/// Android: the app's external-files dir
/// (`Android/data/co.<...>.twotop/files/replays`), browsable with any
/// Files app and shareable from there — no storage permission needed.
pub fn replays_dir() -> Option<PathBuf> {
    crate::paths::shared_dir().map(|d| d.join("replays"))
}

pub struct MatchRecorderPlugin;

impl Plugin for MatchRecorderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MatchRecorder>()
            .init_resource::<LastSavedReplay>()
            // After the ENTIRE sim chain (CONVENTIONS: explicit ordering in
            // GgrsSchedule) — the frame counter and input history have both
            // advanced, so the tick just simulated is `FrameCount - 1`.
            .add_systems(GgrsSchedule, record_tick.after(sim::advance_input_history))
            .add_systems(Update, note_confirmed_kills.after(render::facts::KillFacts))
            .add_systems(
                Update,
                save_replay_on_match_over.after(crate::attest::settle_match_over),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pruning_keeps_the_newest_and_never_a_protected_tape() {
        let mut tapes = Vec::new();
        for i in 0..(KEEP_OTHERS as u64 + 5) {
            tapes.push((format!("p{i}"), i, false));
        }
        tapes.push(("duel".to_string(), 0, true));
        let protected = ["p0".to_string()].into_iter().collect();
        let pruned = tapes_to_prune(&tapes, &protected);
        // The five oldest practice tapes go, except the protected one;
        // the lone duel stays.
        assert_eq!(pruned, vec!["p4", "p3", "p2", "p1"]);
    }

    #[test]
    fn a_match_starts_at_the_last_rematch() {
        let mut rec = MatchRecorder::default();
        let s = |p0, p1| MatchScore { p0, p1 };
        rec.scores = vec![
            s(0, 0),
            s(1, 0),
            s(5, 0),
            s(5, 0),
            s(0, 0),
            s(0, 1),
            s(0, 2),
        ];
        assert_eq!(rec.match_start(7), 4, "the rematch tick");
        assert_eq!(rec.match_start(3), 0, "the first match starts at 0");
    }

    #[test]
    fn a_tape_knows_what_it_recorded() {
        use replay::TapeKind;
        assert_eq!(tape_kind(true, false, false, false, 0), TapeKind::Duel);
        assert_eq!(tape_kind(false, false, false, false, 0), TapeKind::Couch);
        assert_eq!(
            tape_kind(true, true, false, false, 7),
            TapeKind::Practice { tier: 7 }
        );
        assert_eq!(tape_kind(false, true, true, false, 7), TapeKind::Shade);
        assert_eq!(tape_kind(false, true, false, true, 0), TapeKind::Lesson);
    }
    use bevy::time::TimeUpdateStrategy;
    use bevy_ggrs::GgrsPlugin;
    use bevy_ggrs::prelude::*;
    use core::time::Duration;
    use fixed_math::Vec2F;
    use sim::{
        DefaultInputsPlugin, GgrsCfg, Player, PositionF, PreviousPositionF, SimPlugin,
        SynthesizedInputs, VelocityF,
    };

    #[test]
    fn recorder_reset_clears_everything() {
        let mut rec = MatchRecorder {
            frames: vec![[PlayerInput::default(); 2]],
            scores: vec![MatchScore { p0: 1, p1: 0 }],
            saved: true,
            kills: vec![(3, 1)],
        };
        rec.reset();
        assert!(rec.frames.is_empty() && rec.scores.is_empty() && rec.kills.is_empty());
        assert!(!rec.saved);
    }

    #[test]
    fn the_score_trail_answers_for_confirmed_ticks_only() {
        let rec = MatchRecorder {
            frames: vec![[PlayerInput::default(); 2]; 3],
            scores: vec![
                MatchScore::default(),
                MatchScore { p0: 1, p1: 0 },
                MatchScore { p0: 2, p1: 0 },
            ],
            saved: false,
            kills: Vec::new(),
        };
        assert_eq!(
            rec.score_after(-1),
            MatchScore::default(),
            "nothing confirmed"
        );
        assert_eq!(rec.score_after(1), MatchScore { p0: 1, p1: 0 });
        assert_eq!(rec.score_after(9), MatchScore::default(), "never recorded");
    }

    /// The tape holds what is fact: through the deciding tick, through the
    /// last confirmed tick of a session that ended, nothing of a desync.
    #[test]
    fn a_tape_keeps_only_the_ticks_that_are_fact() {
        use crate::netplay::Ending;
        let ended = |confirmed| {
            Some(Ending {
                confirmed,
                score: MatchScore::default(),
            })
        };
        // Decided on tick 200, recorded 216 (16 predicted past it).
        assert_eq!(tape_ticks(MatchState::decided(200), None, false, 216), 201);
        // The session ended later; the decision still stands.
        assert_eq!(
            tape_ticks(MatchState::decided(200), ended(210), false, 216),
            201
        );
        // Ended undecided: through the confirmed tick.
        assert_eq!(
            tape_ticks(MatchState::abandoned(), ended(150), false, 166),
            151
        );
        assert_eq!(tape_ticks(MatchState::abandoned(), ended(-1), false, 10), 0);
        // A desync keeps nothing.
        assert_eq!(tape_ticks(MatchState::decided(200), None, true, 216), 0);
        // Local, never ended out of band: everything recorded.
        assert_eq!(tape_ticks(MatchState::abandoned(), None, false, 99), 99);
    }

    /// Drive a real SyncTest session (rollback ACTIVE — check_distance 2
    /// re-simulates the last two ticks every frame, so the overwrite path
    /// runs constantly) through a constant-then-flipped input schedule and
    /// assert the captured tape reproduces it exactly: no trailing lag, one
    /// clean transition at the exact tick, both handles, no gaps.
    #[test]
    fn recorder_captures_a_faithful_tape() {
        let mut sb = SessionBuilder::<GgrsCfg>::new()
            .with_num_players(2)
            .unwrap()
            .with_check_distance(2)
            .with_input_delay(0);
        sb = sb.add_player(PlayerType::Local, 0).unwrap();
        sb = sb.add_player(PlayerType::Local, 1).unwrap();
        let session = sb.start_synctest_session().unwrap();

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / sim::TICK_HZ as f64,
        )));
        app.add_plugins(GgrsPlugin::<GgrsCfg>::default());
        app.add_plugins(SimPlugin);
        app.add_plugins(sim::InfiniteRoundPlugin);
        app.add_plugins(DefaultInputsPlugin);
        app.insert_resource(Session::SyncTest(session));
        app.init_resource::<MatchRecorder>();
        app.init_resource::<crate::theater::TheaterMode>();
        app.add_systems(GgrsSchedule, record_tick.after(sim::advance_input_history));
        for handle in 0..2usize {
            app.world_mut().spawn((
                Player { handle },
                PositionF(Vec2F::ZERO),
                PreviousPositionF(Vec2F::ZERO),
                VelocityF(Vec2F::ZERO),
            ));
        }

        let a = PlayerInput {
            stick_x: 50,
            stick_y: 0,
            aim_angle: 0,
            buttons: 0,
        };
        let b = PlayerInput {
            stick_x: -50,
            stick_y: 0,
            aim_angle: 0,
            buttons: PlayerInput::DASH_DOWN,
        };
        // Prime once: the first update warms the session without simulating
        // a tick (same as every sim test harness), so tick k below is fed
        // exactly at iteration k.
        app.update();
        for tick in 0..120u32 {
            app.world_mut().resource_mut::<SynthesizedInputs>().0 = if tick < 60 { a } else { b };
            app.update();
        }

        let rec = app.world().resource::<MatchRecorder>();
        // No trailing lag: every simulated tick is on the tape already.
        let frame = app.world().resource::<sim::FrameCount>().0;
        assert_eq!(rec.frames.len() as u32, frame);
        // Both handles recorded identically (couch: shared SynthesizedInputs),
        // and the flip lands on the exact tick it was fed.
        for pair in &rec.frames {
            assert_eq!(pair[0], pair[1], "both handles share the tape");
        }
        assert_eq!(rec.frames[59][0], a, "tick 59 is the last A");
        assert_eq!(rec.frames[60][0], b, "tick 60 is the first B");
        let transitions = rec.frames.windows(2).filter(|w| w[0][0] != w[1][0]).count();
        assert_eq!(transitions, 1, "one clean A→B flip, no glitch frames");
    }
}
