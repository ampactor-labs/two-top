//! An idle player against the sparring partner on every arena, from each
//! spawn, six seeds apiece: `cargo run -p bot_brain --example idle_probe
//! --release`. A newcomer who has not found the controls must still meet a
//! finished match on any table — this is where a stall on one arena's
//! geometry shows up (the Crossing's moat, the Gallery's fenced centre).
use bot_brain::bench::{Seat, play};
fn main() {
    for arena in sim::ALL_ARENAS {
        for swapped in [false, true] {
            let mut line = format!("{arena:?} idle-in-seat-{}:", swapped as u8);
            for seed in 0..6 {
                let seats = if swapped {
                    [Seat::Bot { tier: 0 }, Seat::Idle]
                } else {
                    [Seat::Idle, Seat::Bot { tier: 0 }]
                };
                let r = play(arena, seats, seed, 600);
                line.push_str(&format!(
                    " {}-{} {:.0}s/{}e",
                    r.score.0,
                    r.score.1,
                    r.seconds(),
                    r.empty_rounds
                ));
            }
            println!("{line}");
        }
    }
}
