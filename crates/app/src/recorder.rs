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
use bevy_ggrs::{GgrsSchedule, PlayerInputs};
use replay::{FORMAT_VERSION, MAGIC, Replay, ReplayHeader, encode};
use sim::{
    FrameCount, GgrsCfg, MATCH_WIN_THRESHOLD, MatchScore, MatchState, PlayerInput, SelectedArena,
    TICK_HZ,
};
use std::path::PathBuf;

/// Render-frames to wait after `MatchOver` before writing the file. The
/// tape already holds the deciding kill the moment it lands; the delay
/// lets the last few PREDICTED online ticks get resim-corrected (rollback
/// overwrites their tape slots) before the bytes are frozen.
const SAVE_DELAY_FRAMES: u8 = 30;

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
    /// Countdown to the post-`MatchOver` write (None = not pending).
    save_in: Option<u8>,
    /// Latch so one MatchOver writes exactly one file.
    saved: bool,
}

impl MatchRecorder {
    fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Where the replay of the last decided match landed (the summary screen
/// surfaces it).
#[derive(Resource, Default)]
pub struct LastSavedReplay(pub Option<PathBuf>);

/// `GgrsSchedule`, after the whole sim chain: record the tick that was just
/// simulated. Runs for every tick INCLUDING resimulations — a resimulated
/// tick overwrites its slot with the corrected inputs, so the tape
/// converges to confirmed values by construction.
pub fn capture_tick_inputs(
    frame: Res<FrameCount>,
    inputs: Res<PlayerInputs<GgrsCfg>>,
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
    let pair = [inputs[0].0, inputs[1].0];
    match t.cmp(&rec.frames.len()) {
        std::cmp::Ordering::Less => rec.frames[t] = pair,
        std::cmp::Ordering::Equal => rec.frames.push(pair),
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

/// On the tick a match is decided, arm a short delay (to let the harvest
/// catch up past the deciding kill), then write the tape as a `.bmrg`.
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
    mut record: ResMut<crate::grudge::CareerRecord>,
    mut rec: ResMut<MatchRecorder>,
    mut last_saved: ResMut<LastSavedReplay>,
) {
    if !matches!(*state, MatchState::MatchOver) {
        // Leaving MatchOver (rematch / lobby): re-arm for the next decision.
        rec.save_in = None;
        rec.saved = false;
        return;
    }
    if rec.saved || rec.frames.is_empty() {
        return;
    }
    match rec.save_in {
        None => rec.save_in = Some(SAVE_DELAY_FRAMES),
        Some(0) => {
            rec.saved = true;
            rec.save_in = None;
            let winner = if score.p0 >= MATCH_WIN_THRESHOLD {
                0
            } else {
                1
            };
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
                    frame_count: rec.frames.len() as u32,
                    recorded_at,
                    winner: Some(winner),
                    player_handles: header_names(
                        &netplay,
                        practice.0,
                        shade.0.as_ref().map(|s| s.name.as_str()),
                        local.0,
                        &profile,
                        &peer,
                    ),
                    arena_id: selected.0.as_u8(),
                },
                inputs: rec.frames.clone(),
            };
            last_saved.0 = write_replay(&replay, recorded_at, winner);
            // A live duel's tape lands on the rival's ring too, so the
            // rivals screen can replay the recent conversation.
            if let (Some(path), Some(peer), false) = (&last_saved.0, peer.0, practice.0)
                && netplay.room_url.is_some()
                && let Some(name) = path.file_name().and_then(|n| n.to_str())
            {
                crate::grudge::note_rival_tape(&mut record, peer, name.to_string());
            }
        }
        Some(ref mut n) => *n -= 1,
    }
}

/// Encode + write the tape. Returns the path on success.
fn write_replay(replay: &Replay, recorded_at: u64, winner: u8) -> Option<PathBuf> {
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
        "match_{recorded_at}_{}wins.bmrg",
        if winner == 0 { "cur" } else { "stag" }
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
            .add_systems(
                GgrsSchedule,
                capture_tick_inputs.after(sim::advance_input_history),
            )
            .add_systems(Update, save_replay_on_match_over);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            save_in: Some(3),
            saved: true,
        };
        rec.reset();
        assert!(rec.frames.is_empty());
        assert!(!rec.saved && rec.save_in.is_none());
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
        app.add_systems(
            GgrsSchedule,
            capture_tick_inputs.after(sim::advance_input_history),
        );
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
