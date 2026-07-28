//! The wasm determinism lane (NORTH N3, COMPLETION_PLAN P.6): the same
//! canonical tape, the same committed golden, asserted from inside a real
//! browser. Q16.16 fixed-point, BTree containers, and portable hashers
//! are why this can pass at all — floats would have made the web build a
//! fork of the game instead of the game.
//!
//! Run with: `wasm-pack test --headless --chrome crates/replay_sync -- --test wasm_checksums`
//! (the CI lane in .github/workflows/wasm.yml does exactly that).

#![cfg(target_arch = "wasm32")]

use replay::decode_for_sim_version;
use replay_sync::compute_checksum_tsv;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

const GOLDEN: &str = include_str!("../../../tests/demos/canonical/match_v1.checksums.tsv");
const TAPE: &[u8] = include_bytes!("../../../tests/demos/canonical/match_v1.bmrg");

/// Separates harness failures from sim failures: if this passes and the
/// checksum test doesn't, the wasm module and the browser plumbing are
/// fine and the sim itself is where to look.
#[wasm_bindgen_test]
fn the_harness_itself_boots() {
    assert_eq!(2 + 2, 4);
}

#[wasm_bindgen_test]
fn browser_checksums_equal_the_native_golden() {
    let replay = decode_for_sim_version(TAPE, sim::SIM_VERSION).expect("canonical demo decodes");
    let tsv = compute_checksum_tsv(&replay);
    assert_eq!(
        tsv, GOLDEN,
        "the browser's sim diverged from the native golden",
    );
}
