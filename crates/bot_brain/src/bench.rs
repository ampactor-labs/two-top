//! The tuning bench: whole matches, bot against bot, headless.
//!
//! "The practice bot feels right" is a claim about numbers — how often a
//! tier beats the one below it, how long a match against the sparring
//! partner lasts, whether a match can stall at 0-0 — so the ladder is tuned
//! against those numbers. This runs the real sim (the same plugins, arena
//! geometry and spawn points as a live match) with each seat driven by a
//! [`Brain`] at a fixed tier, or left idle, and reports how it went.
//!
//! No rendering, no window, no network: one sim tick per `App::update`.

use std::time::Duration;

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy_ggrs::prelude::*;
use bevy_ggrs::{GgrsPlugin, LocalInputs, Session};
use fixed_math::Vec2F;
use sim::{
    ArenaId, GgrsCfg, MatchScore, MatchState, Player, PlayerInput, PositionF, PreviousPositionF,
    SimPlugin, VelocityF,
};

use crate::{Brain, view_for};

/// Who sits in a seat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seat {
    /// Sends nothing, ever (a newcomer who has not found the controls).
    Idle,
    /// The bot at a fixed tier (no rubber band: the bench measures tiers).
    Bot { tier: u32 },
}

/// How one match went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatchReport {
    /// Final score, seat 0 then seat 1.
    pub score: (u8, u8),
    /// Sim ticks played (60 per second).
    pub ticks: u32,
    /// Whether it reached `MatchOver` before the cap.
    pub finished: bool,
    /// Rounds that ended with no kill in them.
    pub empty_rounds: u32,
    /// Rounds played.
    pub rounds: u32,
}

impl MatchReport {
    /// Seat 0 won, seat 1 won, or neither (the cap).
    pub fn winner(&self) -> Option<usize> {
        match self.score {
            _ if !self.finished => None,
            (a, b) if a > b => Some(0),
            (a, b) if b > a => Some(1),
            _ => None,
        }
    }

    /// Match length in seconds.
    pub fn seconds(&self) -> f32 {
        self.ticks as f32 / sim::TICK_HZ as f32
    }
}

#[derive(Resource)]
struct Seats {
    brains: [Option<(Brain, u32)>; 2],
    seed: u32,
}

/// `ReadInputs`: both seats' inputs for this tick.
fn seat_inputs(world: &mut World) {
    world.resource_scope::<Seats, _>(|world, mut seats| {
        let seed = seats.seed;
        let mut map = bevy::platform::collections::HashMap::default();
        for (handle, seat) in seats.brains.iter_mut().enumerate() {
            let input = match seat {
                Some((brain, tier)) => {
                    match view_for(world, handle, *tier, None, seed.wrapping_add(handle as u32)) {
                        Some(view) => brain.decide(view),
                        None => {
                            brain.forget();
                            PlayerInput::default()
                        }
                    }
                }
                None => PlayerInput::default(),
            };
            map.insert(handle, input);
        }
        world.insert_resource(LocalInputs::<GgrsCfg>(map));
    });
}

/// A headless match on `arena`, ready to step: the live spawn points, the
/// arena's walls, cover, pyres and trees, and the real round flow
/// (countdown, 30 s rounds, first to five).
fn build(arena: ArenaId, seats: [Seat; 2], seed: u32) -> App {
    let mut sb = SessionBuilder::<GgrsCfg>::new()
        .with_num_players(2)
        .expect("two players")
        .with_check_distance(0)
        .with_input_delay(0);
    for handle in 0..2 {
        sb = sb
            .add_player(PlayerType::Local, handle)
            .expect("add player");
    }
    let session = sb.start_synctest_session().expect("synctest session");

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
        1.0 / sim::TICK_HZ as f64,
    )));
    app.add_plugins(GgrsPlugin::<GgrsCfg>::default());
    app.add_plugins(SimPlugin);
    app.insert_resource(Session::SyncTest(session));
    app.insert_resource(sim::SelectedArena(arena));
    app.insert_resource(Seats {
        brains: seats.map(|seat| match seat {
            Seat::Idle => None,
            Seat::Bot { tier } => Some((Brain::default(), tier)),
        }),
        seed,
    });
    app.add_systems(ReadInputs, seat_inputs);

    for (handle, y) in [(0usize, -300), (1, 300)] {
        let spawn = Vec2F::from_cm(0, y);
        app.world_mut().spawn((
            Player { handle },
            PositionF(spawn),
            PreviousPositionF(spawn),
            VelocityF(Vec2F::ZERO),
        ));
    }
    for wall in sim::arena_walls() {
        app.world_mut().spawn(wall);
    }
    for wall in sim::arena_obstacles_for(arena) {
        app.world_mut().spawn(wall);
    }
    for pyre in sim::arena_pyres_for(arena) {
        app.world_mut().spawn(pyre);
    }
    for tree in sim::arena_trees_for(arena) {
        app.world_mut().spawn(tree);
    }
    app
}

/// Play one match to `MatchOver`, or until `max_seconds` of sim time.
pub fn play(arena: ArenaId, seats: [Seat; 2], seed: u32, max_seconds: u32) -> MatchReport {
    play_watched(arena, seats, seed, max_seconds, |_, _| {})
}

/// [`play`], calling `watch(world, tick)` after every tick — for tracing a
/// match that went wrong (where the bot stood, what it saw) without a
/// window.
pub fn play_watched(
    arena: ArenaId,
    seats: [Seat; 2],
    seed: u32,
    max_seconds: u32,
    mut watch: impl FnMut(&mut World, u32),
) -> MatchReport {
    let mut app = build(arena, seats, seed);
    let cap = max_seconds * sim::TICK_HZ as u32;
    let mut ticks = 0;
    let mut rounds = 0;
    let mut empty_rounds = 0;
    let mut kills_at_round_start = 0u32;
    let mut was_in_round = false;
    loop {
        app.update();
        ticks += 1;
        watch(app.world_mut(), ticks);
        let state = *app.world().resource::<MatchState>();
        let score = *app.world().resource::<MatchScore>();
        let kills = u32::from(score.p0) + u32::from(score.p1);
        let in_round = state.is_in_round();
        if in_round && !was_in_round {
            kills_at_round_start = kills;
        }
        if was_in_round && !in_round {
            rounds += 1;
            if kills == kills_at_round_start {
                empty_rounds += 1;
            }
        }
        was_in_round = in_round;
        let finished = matches!(state, MatchState::MatchOver);
        if finished || ticks >= cap {
            return MatchReport {
                score: (score.p0, score.p1),
                ticks,
                finished,
                empty_rounds,
                rounds,
            };
        }
    }
}

/// Seat `a` against seat `b` over `matches` matches: arenas cycled, seats
/// swapped every other match (so neither spawn is favoured), one seed each.
/// Returns (a's wins, b's wins, unfinished, reports).
pub fn series(
    a: Seat,
    b: Seat,
    matches: u32,
    max_seconds: u32,
) -> (u32, u32, u32, Vec<MatchReport>) {
    let (mut a_wins, mut b_wins, mut unfinished) = (0, 0, 0);
    let mut reports = Vec::new();
    for i in 0..matches {
        let arena = sim::ALL_ARENAS[i as usize % sim::ALL_ARENAS.len()];
        let swapped = i % 2 == 1;
        let seats = if swapped { [b, a] } else { [a, b] };
        let report = play(arena, seats, 0x5EED_0000 + i, max_seconds);
        match report.winner() {
            Some(seat) if (seat == 0) != swapped => a_wins += 1,
            Some(_) => b_wins += 1,
            None => unfinished += 1,
        }
        reports.push(report);
    }
    (a_wins, b_wins, unfinished, reports)
}
