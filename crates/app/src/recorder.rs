//! Match recorder — every match becomes a shareable `.bmrg` replay.
//!
//! The whole architecture is already a replay machine: the sim is
//! bit-deterministic and driven only by the input tape, so a complete match
//! is a few KB that reproduces perfectly on any device (and scrubs in
//! `replay_viewer`). This module writes that tape to a plain file the
//! player can share however they like.
//!
//! ## Why harvest from `InputHistory` (not `LocalInputs`)
//!
//! `replay::RecordPlugin` captures `LocalInputs` in `ReadInputs` — complete
//! for couch (both players local), but an ONLINE recording would miss the
//! peer's half entirely. The one place both players' inputs exist in their
//! final form is the sim's own rolled-back [`sim::InputHistory`] ring: it
//! converges to the confirmed values, and the sim itself derives gameplay
//! edges from it — so an input that has aged out of the rollback window in
//! that ring is exactly as "confirmed" as the game state itself. We harvest
//! each tick's inputs once it is `INPUT_HISTORY_LEN` ticks old (the ring's
//! full depth ≥ the max rollback the game tolerates by construction).
//!
//! A frame hitch that advances the sim more than the ring depth in one
//! render frame would lose ticks from the ring; the recorder detects the
//! gap and poisons the tape (logged) rather than writing a corrupt replay.

use bevy::prelude::*;
use replay::{FORMAT_VERSION, MAGIC, Replay, ReplayHeader, encode};
use sim::{
    FrameCount, INPUT_HISTORY_LEN, InputHistory, MATCH_WIN_THRESHOLD, MatchScore, MatchState,
    PlayerInput, SelectedArena, TICK_HZ,
};
use std::path::PathBuf;

/// Render-frames to keep harvesting after `MatchOver` before writing the
/// file, so the tape safely covers the deciding kill (the harvest trails
/// the sim by `INPUT_HISTORY_LEN` ticks; the sim keeps ticking through the
/// summary screen, so this catches up within a few frames).
const SAVE_DELAY_FRAMES: u8 = 30;

/// The in-progress tape plus harvest bookkeeping.
#[derive(Resource, Default)]
pub struct MatchRecorder {
    frames: Vec<replay::FrameInputs>,
    /// Next tick index to harvest.
    next: u32,
    /// A gap ate part of the tape — don't write a corrupt replay.
    poisoned: bool,
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

/// Harvest confirmed ticks out of the rolled-back input ring. Runs in
/// `Update` (post-rollback), catching up on every tick that has aged past
/// the ring's rollback depth since the last render frame.
fn harvest_confirmed_inputs(
    frame: Res<FrameCount>,
    history: Res<InputHistory>,
    mut rec: ResMut<MatchRecorder>,
) {
    let f = frame.0;
    let len = INPUT_HISTORY_LEN as u32;
    if f < rec.next {
        // The rollback frame counter restarted: a fresh session (new match
        // from the title / a reconnect). Start a fresh tape.
        rec.reset();
    }
    while rec.next + len <= f {
        let t = rec.next;
        let back = (f - 1 - t) as usize;
        if back >= INPUT_HISTORY_LEN {
            if !rec.poisoned {
                tracing::warn!(
                    target: "two_top::recorder",
                    tick = t,
                    frame = f,
                    "input ring gap (frame hitch) — replay tape poisoned",
                );
            }
            rec.poisoned = true;
            // Skip to what's still recoverable so bookkeeping stays sane
            // (the tape is already marked unusable).
            rec.next = f - len;
            break;
        }
        let idx = INPUT_HISTORY_LEN - 1 - back;
        let mut inputs = [PlayerInput::default(); 2];
        for (handle, slot) in inputs.iter_mut().enumerate() {
            if let Some(ring) = history.0.get(&handle) {
                *slot = ring[idx];
            }
        }
        rec.frames.push(inputs);
        rec.next += 1;
    }
}

/// On the tick a match is decided, arm a short delay (to let the harvest
/// catch up past the deciding kill), then write the tape as a `.bmrg`.
fn save_replay_on_match_over(
    state: Res<MatchState>,
    score: Res<MatchScore>,
    selected: Res<SelectedArena>,
    mut rec: ResMut<MatchRecorder>,
    mut last_saved: ResMut<LastSavedReplay>,
) {
    if !matches!(*state, MatchState::MatchOver) {
        // Leaving MatchOver (rematch / lobby): re-arm for the next decision.
        rec.save_in = None;
        rec.saved = false;
        return;
    }
    if rec.saved || rec.poisoned || rec.frames.is_empty() {
        return;
    }
    match rec.save_in {
        None => rec.save_in = Some(SAVE_DELAY_FRAMES),
        Some(0) => {
            rec.saved = true;
            rec.save_in = None;
            let winner = if score.p0 >= MATCH_WIN_THRESHOLD { 0 } else { 1 };
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
                    player_handles: [None, None],
                    arena_id: selected.0.as_u8(),
                },
                inputs: rec.frames.clone(),
            };
            last_saved.0 = write_replay(&replay, recorded_at, winner);
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
    match std::fs::write(&path, &bytes) {
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
fn replays_dir() -> Option<PathBuf> {
    #[cfg(target_os = "android")]
    {
        bevy::android::ANDROID_APP
            .get()
            .and_then(|app| app.external_data_path())
            .map(|p| p.join("replays"))
    }
    #[cfg(not(target_os = "android"))]
    {
        dirs::download_dir()
            .or_else(dirs::data_dir)
            .map(|d| d.join("two-top").join("replays"))
    }
}

pub struct MatchRecorderPlugin;

impl Plugin for MatchRecorderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MatchRecorder>()
            .init_resource::<LastSavedReplay>()
            .add_systems(
                Update,
                (harvest_confirmed_inputs, save_replay_on_match_over).chain(),
            );
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
            next: 5,
            poisoned: true,
            save_in: Some(3),
            saved: true,
        };
        rec.reset();
        assert!(rec.frames.is_empty());
        assert_eq!(rec.next, 0);
        assert!(!rec.poisoned && !rec.saved && rec.save_in.is_none());
    }

    /// Drive a real SyncTest session (rollback active, check_distance 2)
    /// through a constant-then-flipped input schedule and assert the
    /// harvested tape reproduces it: one clean transition, correct values,
    /// both handles, no gaps, trailing the sim by the ring depth.
    #[test]
    fn recorder_harvests_a_faithful_tape() {
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
        app.add_systems(Update, harvest_confirmed_inputs);
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
        for tick in 0..120u32 {
            app.world_mut().resource_mut::<SynthesizedInputs>().0 =
                if tick < 60 { a } else { b };
            app.update();
        }

        let rec = app.world().resource::<MatchRecorder>();
        assert!(!rec.poisoned, "no ring gaps at one tick per update");
        // The tape trails the sim by the ring depth: the newest harvested
        // tick is `frame - LEN`, so the next to harvest is one past it.
        let frame = app.world().resource::<sim::FrameCount>().0;
        assert_eq!(rec.next, frame - (INPUT_HISTORY_LEN as u32 - 1));
        assert!(rec.frames.len() as u32 == rec.next);
        // Both handles recorded identically (couch: shared SynthesizedInputs),
        // and the stream is A* then B* with exactly one transition.
        let mut transitions = 0;
        for pair in rec.frames.windows(2) {
            assert_eq!(pair[0][0], pair[0][1], "both handles share the tape");
            if pair[0][0] != pair[1][0] {
                transitions += 1;
            }
        }
        assert_eq!(transitions, 1, "one clean A→B flip, no glitch frames");
        assert_eq!(rec.frames.first().unwrap()[0], a);
        assert_eq!(rec.frames.last().unwrap()[0], b);
    }
}
