//! The browser entry (NORTH N3 / COMPLETION_PLAN P.5).
//!
//! `web/index.html` loads the wasm-bindgen glue, optionally fetches a
//! shared tape from the drop (`#watch=<id>` in the page URL), and calls
//! [`web_start`]. A tape parks in a static until the app reaches the
//! Title screen, where [`watch_autoplay`] rolls it through the theater's
//! own projector — the same `start_playback` the REPLAYS screen uses, so
//! the browser shows the real presentation, not a port of it. No tape ⇒
//! the game boots to the Title exactly like every other platform.

use bevy::prelude::*;
use std::sync::OnceLock;
use wasm_bindgen::prelude::*;

use crate::screen::AppScreen;

static WATCH_TAPE: OnceLock<Vec<u8>> = OnceLock::new();

/// The page's entry point. `watch_tape` is the raw `.bmrg` bytes when the
/// URL named a shared match (index.html already fetched them from the
/// drop); `None` boots the game.
#[wasm_bindgen]
pub fn web_start(watch_tape: Option<Vec<u8>>) {
    console_error_panic_hook::set_once();
    if let Some(bytes) = watch_tape {
        let _ = WATCH_TAPE.set(bytes);
    }
    crate::run();
}

/// The determinism lane's probe (wasm.yml + web/check.html): recompute
/// the canonical demo's checksums inside this very browser build and
/// compare them to the committed golden, byte for byte. This replaced a
/// wasm-bindgen-test harness that hung identically on two machines
/// without ever running a test; a plain page in plain headless Chrome is
/// the machinery the theater screenshot already proved.
#[cfg(feature = "wasm-probe")]
#[wasm_bindgen]
pub fn checksum_golden_probe() -> String {
    const GOLDEN: &str = include_str!("../../../tests/demos/canonical/match_v1.checksums.tsv");
    const TAPE: &[u8] = include_bytes!("../../../tests/demos/canonical/match_v1.bmrg");
    let replay = match replay::decode_for_sim_version(TAPE, sim::SIM_VERSION) {
        Ok(r) => r,
        Err(e) => return format!("CHECKSUMS-FAIL decode: {e}"),
    };
    let tsv = replay_sync::compute_checksum_tsv(&replay);
    if tsv == GOLDEN {
        format!("CHECKSUMS-OK {} frames", replay.header.frame_count)
    } else {
        match tsv
            .lines()
            .zip(GOLDEN.lines())
            .enumerate()
            .find(|(_, (a, b))| a != b)
        {
            Some((i, (ours, golden))) => {
                format!("CHECKSUMS-FAIL line {i}\nbrowser: {ours}\ngolden:  {golden}")
            }
            None => "CHECKSUMS-FAIL length mismatch".to_string(),
        }
    }
}

/// Once, at the Title: if the page brought a tape, roll it. A tape that
/// won't decode (foreign sim version, truncation) logs and leaves the
/// game at the Title — the visitor still gets the game itself.
pub(crate) fn watch_autoplay(world: &mut World, mut done: Local<bool>) {
    if *done {
        return;
    }
    if *world.resource::<State<AppScreen>>().get() != AppScreen::Title {
        return;
    }
    *done = true;
    let Some(bytes) = WATCH_TAPE.get() else {
        return;
    };
    match replay::decode_for_sim_version(bytes, sim::SIM_VERSION) {
        Ok(replay) => {
            tracing::info!(
                target: "two_top::web",
                frames = replay.header.frame_count,
                "shared tape on the projector",
            );
            crate::theater::start_playback(world, replay);
        }
        Err(e) => {
            tracing::error!(target: "two_top::web", error = %e, "shared tape rejected");
        }
    }
}
