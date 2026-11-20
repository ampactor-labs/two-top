//! The one reader of bevy_ggrs's frame counters.
//!
//! bevy_ggrs numbers frames per session; the sim counts ticks per match
//! (`sim::FrameCount`). Today they coincide. A resumed session (a match
//! continued after a dropped link) starts ggrs's numbering again at 0 on
//! a world already `base` ticks in, so every "is this tick confirmed?"
//! question goes through [`SimClock`], which adds the base. A test fails
//! if any other file in this crate reads the counters directly.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy_ggrs::{ConfirmedFrameCount, RollbackFrameCount};

/// Sim ticks already simulated when the current ggrs session's frame 0
/// began. 0 for every session that starts a match from its first tick.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockBase(pub i32);

/// bevy_ggrs's frame counters, in sim ticks.
#[derive(SystemParam)]
pub struct SimClock<'w> {
    confirmed: Option<Res<'w, ConfirmedFrameCount>>,
    rollback: Option<Res<'w, RollbackFrameCount>>,
    base: Option<Res<'w, ClockBase>>,
}

impl SimClock<'_> {
    fn base(&self) -> i32 {
        self.base.as_ref().map_or(0, |b| b.0)
    }

    /// The last tick both peers' inputs are known for; -1 for none.
    pub fn confirmed_tick(&self) -> i32 {
        to_tick(self.confirmed.as_ref().map(|c| c.0), self.base())
    }

    /// The tick a rollback is loading (valid inside `LoadWorld`).
    pub fn rollback_tick(&self) -> i32 {
        self.rollback.as_ref().map_or(0, |r| r.0) + self.base()
    }
}

/// bevy_ggrs's counters as a fresh session expects them: frame 0, nothing
/// confirmed (after ticks were run outside a session).
pub fn reset_counters(world: &mut World) {
    world.insert_resource(RollbackFrameCount(0));
    world.insert_resource(ConfirmedFrameCount(-1));
}

/// Put the confirmed frame back at sim tick `tick` (a resume that gives
/// up books the match where its link died).
pub fn set_confirmed_tick(world: &mut World, tick: i32) {
    let base = world.get_resource::<ClockBase>().map_or(0, |b| b.0);
    world.insert_resource(ConfirmedFrameCount(tick - base));
}

/// [`SimClock::confirmed_tick`] for exclusive systems.
pub fn confirmed_tick(world: &World) -> i32 {
    to_tick(
        world.get_resource::<ConfirmedFrameCount>().map(|c| c.0),
        world.get_resource::<ClockBase>().map_or(0, |b| b.0),
    )
}

/// A ggrs frame as a sim tick; nothing confirmed stays -1. Pure.
fn to_tick(frame: Option<i32>, base: i32) -> i32 {
    match frame {
        Some(f) if f >= 0 => f + base,
        _ => -1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_map_to_ticks_and_nothing_stays_nothing() {
        assert_eq!(to_tick(None, 0), -1);
        assert_eq!(to_tick(Some(-1), 500), -1);
        assert_eq!(to_tick(Some(0), 0), 0);
        assert_eq!(to_tick(Some(12), 500), 512);
    }

    /// Every read of the counters goes through this file, so a resumed
    /// session's offset cannot be forgotten at one call site.
    #[test]
    fn nothing_else_reads_the_ggrs_frame_counters() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stray = Vec::new();
        let mut stack = vec![src];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") || path.ends_with("sim_clock.rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                for (i, line) in text.lines().enumerate() {
                    let code = line.split("//").next().unwrap_or("");
                    if code.contains("ConfirmedFrameCount") || code.contains("RollbackFrameCount") {
                        stray.push(format!("{}:{}: {}", path.display(), i + 1, line.trim()));
                    }
                }
            }
        }
        assert!(
            stray.is_empty(),
            "read the frame counters through sim_clock::SimClock:\n{}",
            stray.join("\n")
        );
    }
}
