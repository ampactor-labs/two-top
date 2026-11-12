//! Live tuning on a dev build: `render::tuning::Tuning`'s numbers from a
//! file, reloaded whenever the file changes.
//!
//! The feel and visuals passes turn dozens of numbers and judge each by
//! eye. Rebuilding the game for every nudge made that a pass of minutes per
//! number; with `TWOTOP_TUNING=<file.json>` the running game picks up an
//! edit within half a second. The file holds any subset of the fields (the
//! rest stay at their defaults); `TWOTOP_TUNING_DUMP=<file.json>` writes the
//! full set of current defaults to start from. A file that does not parse
//! is reported and ignored, keeping what was running. The values that win
//! are copied back into `Tuning::default()`, which every build ships.

use bevy::prelude::*;
use render::tuning::Tuning;

/// Seconds between looks at the file.
const POLL_SECS: f32 = 0.5;

/// The watched file, and when it last changed.
#[derive(Resource)]
struct Watched {
    path: std::path::PathBuf,
    modified: Option<std::time::SystemTime>,
    next_look: f32,
}

fn modified(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

fn reload(time: Res<Time<Real>>, mut watched: ResMut<Watched>, mut tuning: ResMut<Tuning>) {
    let now = time.elapsed_secs();
    if now < watched.next_look {
        return;
    }
    watched.next_look = now + POLL_SECS;
    let stamp = modified(&watched.path);
    if stamp.is_none() || stamp == watched.modified {
        return;
    }
    watched.modified = stamp;
    let Ok(text) = std::fs::read_to_string(&watched.path) else {
        return;
    };
    match Tuning::from_overrides(&text) {
        Ok(tuned) => {
            if *tuning != tuned {
                *tuning = tuned;
                tracing::info!(target: "two_top::tuning", path = %watched.path.display(), "tuning reloaded");
            }
        }
        Err(e) => {
            tracing::warn!(target: "two_top::tuning", error = %e, "tuning file does not parse; keeping the running values");
        }
    }
}

pub struct TuningReloadPlugin;

impl Plugin for TuningReloadPlugin {
    fn build(&self, app: &mut App) {
        if let Some(path) = std::env::var_os("TWOTOP_TUNING_DUMP") {
            match serde_json::to_string_pretty(&Tuning::default()) {
                Ok(text) => match std::fs::write(&path, text) {
                    Ok(()) => {
                        tracing::info!(target: "two_top::tuning", path = ?path, "tuning defaults written")
                    }
                    Err(e) => {
                        tracing::warn!(target: "two_top::tuning", error = %e, "could not write the tuning dump")
                    }
                },
                Err(e) => {
                    tracing::warn!(target: "two_top::tuning", error = %e, "could not encode the tuning dump")
                }
            }
        }
        let Some(path) = std::env::var_os("TWOTOP_TUNING").map(std::path::PathBuf::from) else {
            return;
        };
        tracing::info!(target: "two_top::tuning", path = %path.display(), "watching the tuning file");
        app.insert_resource(Watched {
            path,
            modified: None,
            next_look: 0.0,
        })
        .add_systems(Update, reload);
    }
}
