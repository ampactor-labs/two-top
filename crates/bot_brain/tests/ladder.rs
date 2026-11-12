//! The ladder's floor, as a CI gate: a player who does nothing still sees
//! a match end against the sparring partner. The old tier 0 never threw,
//! and such a match looped 30-second rounds at 0-0 forever.

use bot_brain::bench::{Seat, play};

#[test]
fn an_idle_newcomer_still_meets_a_finished_match() {
    let report = play(
        sim::ArenaId::Anchor,
        [Seat::Idle, Seat::Bot { tier: 0 }],
        1,
        300,
    );
    assert!(report.finished, "unfinished after 300 s: {report:?}");
    assert_eq!(report.winner(), Some(1));
    assert!(
        report.seconds() < 180.0,
        "took {:.0} s: {report:?}",
        report.seconds()
    );
}
