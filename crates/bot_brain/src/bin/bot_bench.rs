//! The gauntlet ladder, measured: `cargo run -p bot_brain --bin bot_bench
//! --release -- [matches-per-pair]`.
//!
//! Prints, as a markdown table: an idle player against the sparring
//! partner, each tier against the next, and the top tier against itself —
//! wins, unfinished matches, mean match length, kills per round and rounds
//! with no kill at all. The targets the ladder is tuned to are in
//! `docs/BOT_LADDER.md`.

use bot_brain::bench::{Seat, series};

fn main() {
    let matches: u32 = std::env::args()
        .nth(1)
        .and_then(|n| n.parse().ok())
        .unwrap_or(14);
    let cap_seconds = 600;
    let mut pairs = vec![(Seat::Idle, Seat::Bot { tier: 0 })];
    for tier in 0..10 {
        pairs.push((Seat::Bot { tier }, Seat::Bot { tier: tier + 1 }));
    }
    pairs.push((Seat::Bot { tier: 10 }, Seat::Bot { tier: 10 }));

    println!(
        "| A | B | A wins | B wins | unfinished | mean match s | kills / round | empty rounds |"
    );
    println!("|---|---|---:|---:|---:|---:|---:|---:|");
    for (a, b) in pairs {
        let (a_wins, b_wins, unfinished, reports) = series(a, b, matches, cap_seconds);
        let seconds: f32 = reports.iter().map(|r| r.seconds()).sum::<f32>() / reports.len() as f32;
        let rounds: u32 = reports.iter().map(|r| r.rounds).sum();
        let kills: u32 = reports
            .iter()
            .map(|r| u32::from(r.score.0) + u32::from(r.score.1))
            .sum();
        let empty: u32 = reports.iter().map(|r| r.empty_rounds).sum();
        println!(
            "| {} | {} | {a_wins} | {b_wins} | {unfinished} | {seconds:.0} | {:.2} | {empty}/{rounds} |",
            name(a),
            name(b),
            kills as f32 / rounds.max(1) as f32,
        );
    }
}

fn name(seat: Seat) -> String {
    match seat {
        Seat::Idle => "idle".into(),
        Seat::Bot { tier } => format!("tier {tier}"),
    }
}
