//! Career record — the grudge ledger, v1.
//!
//! Persists online wins/losses across sessions (same JSON-in-config-dir
//! scheme as `settings.rs`) and surfaces the record on the online title
//! screen. Couch matches don't count — the ledger is the story of duels
//! against *other phones*.
//!
//! v1 scope note: matchbox `PeerId`s are ephemeral (fresh per connection),
//! so a true per-opponent ledger ("12th meeting — Stag leads 7-4") needs a
//! persistent install-id exchanged over a reliable app-data channel the
//! netplay layer doesn't carry yet. The career total is the durable value
//! we can record honestly today; the rivalry breakdown is the follow-up
//! once the identity handshake exists.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use sim::{MATCH_WIN_THRESHOLD, MatchScore, MatchState};
use std::path::PathBuf;

use crate::netplay::{LocalPlayerHandle, NetplayConfig};

/// Lifetime online record. Loaded at boot, saved on every decided match.
#[derive(Resource, Serialize, Deserialize, Default, Clone, Copy, Debug)]
pub struct CareerRecord {
    pub wins: u32,
    pub losses: u32,
}

impl CareerRecord {
    pub fn total(&self) -> u32 {
        self.wins + self.losses
    }
}

fn career_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("two-top").join("career.json"))
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
        && let Err(e) = std::fs::write(&path, json)
    {
        tracing::warn!(target: "two_top::grudge", error = %e, "failed to save career record");
    }
}

/// Commit the result on the tick a match is decided. Online only; the local
/// handle decides which side of the score is "ours".
fn record_match_result(
    state: Res<MatchState>,
    score: Res<MatchScore>,
    netplay: Res<NetplayConfig>,
    practice: Res<crate::bot::PracticeMode>,
    local: Res<LocalPlayerHandle>,
    mut record: ResMut<CareerRecord>,
    mut prev_over: Local<bool>,
) {
    let over = matches!(*state, MatchState::MatchOver);
    let entered = over && !*prev_over;
    *prev_over = over;
    // Only live duels count — beating the bot is training, not a record.
    if !entered || netplay.room_url.is_none() || practice.0 {
        return;
    }
    let Some(handle) = local.0 else {
        return;
    };
    let ours = if handle == 0 { score.p0 } else { score.p1 };
    if ours >= MATCH_WIN_THRESHOLD {
        record.wins += 1;
    } else {
        record.losses += 1;
    }
    save_career(&record);
}

pub struct GrudgePlugin;

impl Plugin for GrudgePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(load_career())
            .add_systems(Update, record_match_result);
    }
}
