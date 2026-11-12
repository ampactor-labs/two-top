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
    /// Never throws, never stops walking: a fresh heading every half to
    /// one and a half seconds, steered off the edge, cover and the chasm.
    /// A stand-in for the part of a human the bot has to beat — a target
    /// that keeps moving — so "how long does tier n take to land five" is
    /// measured on something a tight aim at the wrong spot misses.
    Strafer,
}

/// How one match went.
#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// Frames whose re-simulation under rollback did not match the first
    /// run (always empty at check distance 0). Any entry is a desync.
    pub mismatched_frames: Vec<i32>,
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

/// What drives one seat during a match.
enum Driver {
    Idle,
    Bot(Brain, u32),
    Strafer,
}

/// Frames a rollback re-simulation disagreed on (`SyncTestMismatch`).
#[derive(Resource, Default)]
struct Mismatches(Vec<i32>);

#[derive(Resource)]
struct Seats {
    drivers: [Driver; 2],
    seed: u32,
}

/// The strafer's input this tick: a heading hashed from the seed and the
/// current leg (legs of 30-90 ticks), through the brain's own steering.
fn strafe(view: &crate::BotView, seed: u32) -> PlayerInput {
    let hash = |x: u32| {
        let mut h = x.wrapping_mul(0x9E37_79B9) ^ seed;
        h ^= h >> 16;
        h = h.wrapping_mul(0x7FEB_352D);
        h ^ (h >> 15)
    };
    // Walk the legs forward to the one this frame falls in.
    let (mut start, mut leg) = (0u32, 0u32);
    loop {
        let len = 30 + hash(leg) % 61;
        if view.frame < start + len {
            break;
        }
        start += len;
        leg += 1;
        if leg > 100_000 {
            break;
        }
    }
    let angle = (hash(leg ^ 0xA5A5_5A5A) % 360) as f32 * std::f32::consts::PI / 180.0;
    let dir = crate::steer(view, Vec2::from_angle(angle));
    crate::input_from(dir, 0)
}

/// `ReadInputs`: both seats' inputs for this tick.
fn seat_inputs(world: &mut World) {
    world.resource_scope::<Seats, _>(|world, mut seats| {
        let seed = seats.seed;
        let mut map = bevy::platform::collections::HashMap::default();
        for (handle, seat) in seats.drivers.iter_mut().enumerate() {
            let seat_seed = seed.wrapping_add(handle as u32);
            let input = match seat {
                Driver::Bot(brain, tier) => match view_for(world, handle, *tier, None, seat_seed) {
                    Some(view) => brain.decide(view),
                    None => {
                        brain.forget();
                        PlayerInput::default()
                    }
                },
                Driver::Strafer => view_for(world, handle, 0, None, seat_seed)
                    .map(|view| strafe(&view, seat_seed))
                    .unwrap_or_default(),
                Driver::Idle => PlayerInput::default(),
            };
            map.insert(handle, input);
        }
        world.insert_resource(LocalInputs::<GgrsCfg>(map));
    });
}

/// A headless match on `arena`, ready to step: the live spawn points, the
/// arena's walls, cover, pyres and trees, and the real round flow
/// (countdown, 30 s rounds, first to five).
fn build(arena: ArenaId, seats: [Seat; 2], seed: u32, check_distance: usize) -> App {
    let mut sb = SessionBuilder::<GgrsCfg>::new()
        .with_num_players(2)
        .expect("two players")
        // ggrs requires `check_dist < max_prediction` strictly.
        .with_max_prediction_window((check_distance + 1).max(8))
        .with_check_distance(check_distance)
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
    // bevy_ggrs only warns (to a log this app has no plugin for) and
    // triggers this; catch it, or a mismatch passes in silence.
    app.init_resource::<Mismatches>();
    app.add_observer(|t: On<SyncTestMismatch>, mut seen: ResMut<Mismatches>| {
        seen.0.extend(t.mismatched_frames.iter().copied());
    });
    app.insert_resource(sim::SelectedArena(arena));
    app.insert_resource(Seats {
        drivers: seats.map(|seat| match seat {
            Seat::Idle => Driver::Idle,
            Seat::Bot { tier } => Driver::Bot(Brain::default(), tier),
            Seat::Strafer => Driver::Strafer,
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
    watch: impl FnMut(&mut World, u32),
) -> MatchReport {
    play_inner(arena, seats, seed, max_seconds, 0, watch)
}

/// [`play`] under rollback: every frame is re-simulated `check_distance`
/// frames back and its checksum compared, the way a live session rolls
/// back on a late input. A whole match of throws, catches, dashes, kills
/// and round resets, where the `sync_test` binary only walks two players
/// left and right. A mismatch is a desync waiting for two phones.
pub fn play_rolled_back(
    arena: ArenaId,
    seats: [Seat; 2],
    seed: u32,
    max_seconds: u32,
    check_distance: usize,
) -> MatchReport {
    play_inner(arena, seats, seed, max_seconds, check_distance, |_, _| {})
}

fn play_inner(
    arena: ArenaId,
    seats: [Seat; 2],
    seed: u32,
    max_seconds: u32,
    check_distance: usize,
    mut watch: impl FnMut(&mut World, u32),
) -> MatchReport {
    let mut app = build(arena, seats, seed, check_distance);
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
            let mut mismatched_frames = app.world().resource::<Mismatches>().0.clone();
            mismatched_frames.sort_unstable();
            mismatched_frames.dedup();
            return MatchReport {
                score: (score.p0, score.p1),
                ticks,
                finished,
                empty_rounds,
                rounds,
                mismatched_frames,
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
