//! The feel numbers, in one place.
//!
//! Every value the game-feel and visuals passes turn — how hard a kill
//! shakes the camera, how long the kill-cam holds, how far the camera
//! leans, how a fang's trail fades, how long a phone buzzes — lives in
//! [`Tuning`], one resource every build reads (Android, the browser,
//! desktop). Its `Default` is the source of truth: change a number there
//! and it changes everywhere.
//!
//! On a dev build `TWOTOP_TUNING=<file.json>` overrides any subset of it and
//! reloads the file whenever it changes, so a feel pass iterates without
//! rebuilding (`crates/app/src/tuning_reload.rs`); the numbers that win are
//! then copied back into `Default`. `TWOTOP_TUNING_DUMP=<file.json>` writes
//! the current defaults out to start from.
//!
//! Not here: anything the sim decides (`sim::HIT_STOP_FRAMES` and the like).
//! Those change the simulation, which is a `SIM_VERSION` bump, not a tune.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Every tunable feel number, grouped by what it shapes.
#[derive(Resource, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tuning {
    pub shake: Shake,
    pub flash: Flash,
    pub trail: Trail,
    pub camera: Camera,
    pub haptics: Haptics,
}

/// Screen shake: trauma in, camera kick out (`ScreenShake`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shake {
    /// Trauma lost per real-time second (linear decay).
    pub decay_per_sec: f32,
    /// Camera offset in world units at full trauma, before the player's own
    /// SETTINGS → shake scale (the source sprites are 32 px at 2x world
    /// scale, so 6 world units is about 3 source texels of kick).
    pub max_offset: f32,
    /// Kill shake. A one-hit-kill game: every landed contact is a death, so
    /// the kill edge gets the strongest kick.
    pub kill: f32,
    /// Pyre-shatter shake (arena geometry breaking).
    pub shatter: f32,
    /// Perfect-catch shake (the signature skill beat).
    pub perfect_catch: f32,
    /// A mid-air fang clash: a felt clang, well under a kill.
    pub clash: f32,
    /// The match-winning fang's jaws closing on the victor (the devour).
    pub devour: f32,
}

impl Default for Shake {
    fn default() -> Self {
        Self {
            decay_per_sec: 1.8,
            max_offset: 6.0,
            kill: 0.7,
            shatter: 0.3,
            perfect_catch: 0.25,
            clash: 0.22,
            devour: 0.45,
        }
    }
}

/// The kill flash.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Flash {
    /// Peak alpha of the full-screen flash on a kill (a contact accent:
    /// feedback, not information).
    pub kill_alpha: f32,
}

impl Default for Flash {
    fn default() -> Self {
        Self { kill_alpha: 0.6 }
    }
}

/// A flying fang's afterimage trail.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Trail {
    /// World units the fang travels between ghosts (about half a fang).
    pub stamp_spacing: f32,
    /// Real seconds a ghost lives: short, so the trail never walls off the
    /// arena.
    pub ghost_ttl: f32,
    /// A fresh ghost's alpha (the wake stays quieter than the fang).
    pub ghost_alpha: f32,
}

impl Default for Trail {
    fn default() -> Self {
        Self {
            stamp_spacing: 22.0,
            ghost_ttl: 0.18,
            ghost_alpha: 0.55,
        }
    }
}

/// The camera rig and the kill-cam (the app's `camera.rs` reads these).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Camera {
    /// How fast the camera closes on its target (per second, exponential).
    pub follow_rate: f32,
    /// How far toward the action the camera leans, as a fraction of it.
    pub lean_frac: f32,
    /// The lean's cap, in world units.
    pub lean_max: f32,
    /// How much a fang in flight pulls the camera, against the duelists.
    pub fang_weight: f32,
    /// Seconds of velocity the camera looks ahead by.
    pub lookahead_sec: f32,
    /// The kill-cam's zoom at the match-deciding kill.
    pub kill_cam_zoom: f32,
    /// Frames the kill-cam eases in over.
    pub kill_cam_ease_frames: f32,
    /// The micro kill-cam's strength on an ordinary kill (0..1 of the full).
    pub kill_cam_micro_strength: f32,
    /// Frames the micro kill-cam holds.
    pub kill_cam_micro_hold_frames: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            follow_rate: 4.0,
            lean_frac: 0.12,
            lean_max: 48.0,
            fang_weight: 0.5,
            lookahead_sec: 0.15,
            kill_cam_zoom: 1.6,
            kill_cam_ease_frames: 20.0,
            kill_cam_micro_strength: 0.45,
            kill_cam_micro_hold_frames: 26.0,
        }
    }
}

/// Phone buzzes, in milliseconds (the app's `haptics.rs` reads these).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Haptics {
    pub throw_ms: i64,
    pub kill_ms: i64,
    pub perfect_catch_ms: i64,
}

impl Default for Haptics {
    fn default() -> Self {
        Self {
            throw_ms: 10,
            kill_ms: 60,
            perfect_catch_ms: 15,
        }
    }
}

impl Tuning {
    /// `text` (JSON, any subset of the fields) laid over the defaults.
    pub fn from_overrides(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_override_file_changes_only_what_it_names() {
        let tuned = Tuning::from_overrides(r#"{ "shake": { "kill": 0.9 } }"#).unwrap();
        assert_eq!(tuned.shake.kill, 0.9);
        let defaults = Tuning::default();
        assert_eq!(tuned.shake.max_offset, defaults.shake.max_offset);
        assert_eq!(tuned.camera, defaults.camera);
    }

    #[test]
    fn the_defaults_round_trip_through_a_dump() {
        let text = serde_json::to_string_pretty(&Tuning::default()).unwrap();
        assert_eq!(Tuning::from_overrides(&text).unwrap(), Tuning::default());
    }

    #[test]
    fn a_bad_file_is_an_error_not_a_reset() {
        assert!(Tuning::from_overrides("{ shake: oops").is_err());
    }
}
