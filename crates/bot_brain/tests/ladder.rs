//! The ladder's shape, as a CI gate. The full measurement is the
//! `bot_bench` binary (`docs/BOT_LADDER.md`); these are the parts of it
//! that must never regress, sized to run in seconds.

use bot_brain::bench::{Seat, play, series};

/// A player who does nothing still sees a match end against the sparring
/// partner, on every table and from either spawn. The old tier 0 never
/// threw, and such a match looped 30-second rounds at 0-0 forever; the
/// brain then walked into the Crossing's chasm and stalled behind the
/// Gallery's bars.
#[test]
fn an_idle_newcomer_meets_a_finished_match_on_every_table() {
    for arena in sim::ALL_ARENAS {
        for bot_seat in [0usize, 1] {
            let mut seats = [Seat::Idle, Seat::Idle];
            seats[bot_seat] = Seat::Bot { tier: 0 };
            let report = play(arena, seats, 1, 300);
            assert!(
                report.finished,
                "{arena:?}, bot in seat {bot_seat}: unfinished after 300 s: {report:?}"
            );
            assert_eq!(report.winner(), Some(bot_seat), "{arena:?}: {report:?}");
            assert!(
                report.seconds() < 180.0,
                "{arena:?}, bot in seat {bot_seat}: took {:.0} s",
                report.seconds()
            );
        }
    }
}

/// The top of the ladder beats the bottom, clearly. Measured 19-1 over
/// these 20 matches when this gate was written; adjacent tiers are close
/// by design, but ten of them are not.
#[test]
fn the_top_of_the_ladder_beats_the_bottom() {
    let (low, high, unfinished, _) = series(Seat::Bot { tier: 1 }, Seat::Bot { tier: 10 }, 20, 600);
    assert_eq!(unfinished, 0);
    assert!(high >= 15, "tier 10 won {high}, tier 1 won {low}");
}

/// Bot against bot, every round ends in a kill: no tier can stall a round
/// out at 0-0 (the top tiers once did, a third of the time).
#[test]
fn the_top_tier_does_not_stall_against_itself() {
    let (_, _, unfinished, reports) =
        series(Seat::Bot { tier: 10 }, Seat::Bot { tier: 10 }, 7, 600);
    assert_eq!(unfinished, 0);
    let rounds: u32 = reports.iter().map(|r| r.rounds).sum();
    let empty: u32 = reports.iter().map(|r| r.empty_rounds).sum();
    assert!(
        empty * 20 <= rounds,
        "{empty} of {rounds} rounds ended with no kill"
    );
}
