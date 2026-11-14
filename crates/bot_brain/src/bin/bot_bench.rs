//! The gauntlet ladder, measured: `cargo run -p bot_brain --bin bot_bench
//! --release -- [matches-per-row]`.
//!
//! Prints two markdown tables. The first is the ladder against a target:
//! an idle player against the sparring partner, then every tier against a
//! strafer (a dummy that never throws and never stops walking) — how long
//! each tier takes to land five on something that keeps moving. The second
//! is the ladder against itself: each tier against the next, and the top
//! tier against itself. The targets the ladder is tuned to, and the last
//! measured run, are in `docs/BOT_LADDER.md`.

use bot_brain::bench::{MatchReport, Seat, series};

fn main() {
    let matches: u32 = std::env::args()
        .nth(1)
        .and_then(|n| n.parse().ok())
        .unwrap_or(14);
    let cap_seconds = 600;

    println!(
        "| target | bot | bot wins | target wins | unfinished | mean match s | empty rounds |"
    );
    println!("|---|---|---:|---:|---:|---:|---:|");
    let mut targets = vec![(Seat::Idle, Seat::Bot { tier: 0 })];
    for tier in 0..=10 {
        targets.push((Seat::Strafer, Seat::Bot { tier }));
    }
    for (target, bot) in targets {
        let (target_wins, bot_wins, unfinished, reports) =
            series(target, bot, matches, cap_seconds);
        let (seconds, _, empty, rounds) = totals(&reports);
        println!(
            "| {} | {} | {bot_wins} | {target_wins} | {unfinished} | {seconds:.0} | {empty}/{rounds} |",
            name(target),
            name(bot),
        );
    }

    println!();
    println!(
        "| A | B | A wins | B wins | unfinished | mean match s | kills / round | empty rounds |"
    );
    println!("|---|---|---:|---:|---:|---:|---:|---:|");
    let mut pairs = Vec::new();
    for tier in 0..10 {
        pairs.push((Seat::Bot { tier }, Seat::Bot { tier: tier + 1 }));
    }
    pairs.push((Seat::Bot { tier: 10 }, Seat::Bot { tier: 10 }));
    for (a, b) in pairs {
        let (a_wins, b_wins, unfinished, reports) = series(a, b, matches, cap_seconds);
        let (seconds, kills, empty, rounds) = totals(&reports);
        println!(
            "| {} | {} | {a_wins} | {b_wins} | {unfinished} | {seconds:.0} | {:.2} | {empty}/{rounds} |",
            name(a),
            name(b),
            kills as f32 / rounds.max(1) as f32,
        );
    }
}

/// Mean match seconds, total kills, empty rounds and rounds over a series.
fn totals(reports: &[MatchReport]) -> (f32, u32, u32, u32) {
    let seconds = reports.iter().map(|r| r.seconds()).sum::<f32>() / reports.len().max(1) as f32;
    let kills = reports
        .iter()
        .map(|r| u32::from(r.score.0) + u32::from(r.score.1))
        .sum();
    let empty = reports.iter().map(|r| r.empty_rounds).sum();
    let rounds = reports.iter().map(|r| r.rounds).sum();
    (seconds, kills, empty, rounds)
}

fn name(seat: Seat) -> String {
    match seat {
        Seat::Idle => "idle".into(),
        Seat::Strafer => "strafer".into(),
        Seat::Bot { tier } => format!("tier {tier}"),
    }
}
