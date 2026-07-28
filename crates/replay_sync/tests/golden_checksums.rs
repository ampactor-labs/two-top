//! The committed golden: `tests/demos/canonical/match_v1.checksums.tsv` is
//! the canonical demo's per-frame checksum table, byte-for-byte. The
//! cross-platform matrix diffs freshly computed TSVs against each other;
//! this test pins them to a committed file too, so a drift shows up in
//! `git diff` review and not only in a matrix mismatch. The wasm lane
//! (`wasm_checksums.rs`) asserts the same file from inside a browser —
//! same bytes on every platform is the whole religion.
//!
//! On a deliberate sim change: bump SIM_VERSION, regenerate the demo
//! (`gen_canonical`), regenerate this file (`replay_sync --demo ... --output`),
//! and commit all three together — exactly the existing canonical-demo
//! discipline, one file wider.

use replay::decode_for_sim_version;
use replay_sync::compute_checksum_tsv;

const GOLDEN: &str = include_str!("../../../tests/demos/canonical/match_v1.checksums.tsv");
const TAPE: &[u8] = include_bytes!("../../../tests/demos/canonical/match_v1.bmrg");

#[test]
fn the_canonical_demo_matches_its_committed_checksums() {
    let replay = decode_for_sim_version(TAPE, sim::SIM_VERSION).expect("canonical demo decodes");
    let tsv = compute_checksum_tsv(&replay);
    assert_eq!(
        tsv, GOLDEN,
        "canonical checksums drifted from the committed golden — \
         a sim change without the regenerate-and-commit ritual",
    );
}
