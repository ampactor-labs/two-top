//! SIM 17: HOLD from both seats turns a live round into a countdown; from
//! one seat it does nothing.

use bevy::prelude::*;
use sim::{MatchState, PlayerInput, TickInputs};

fn world_in_round() -> World {
    let mut world = World::new();
    world.insert_resource(sim::FrameCount(500));
    world.insert_resource(MatchState::InRound {
        expires_at_frame: 900,
    });
    world.insert_resource(TickInputs::default());
    world
}

fn hold(world: &mut World, seats: [bool; 2]) {
    let mut inputs = TickInputs::default();
    for (seat, on) in seats.into_iter().enumerate() {
        if on {
            inputs.0[seat].buttons |= PlayerInput::HOLD;
        }
    }
    world.insert_resource(inputs);
    world.run_system_cached(sim::apply_resume_hold).unwrap();
}

#[test]
fn both_seats_holding_restart_the_round_with_a_countdown() {
    let mut world = world_in_round();
    hold(&mut world, [true, true]);
    assert_eq!(
        *world.resource::<MatchState>(),
        MatchState::Countdown {
            digit: sim::MID_MATCH_COUNTDOWN_DIGITS,
            expires_at_frame: 500 + sim::COUNTDOWN_DIGIT_FRAMES,
        }
    );
}

#[test]
fn one_seat_cannot_stall_the_match() {
    for seats in [[true, false], [false, true], [false, false]] {
        let mut world = world_in_round();
        hold(&mut world, seats);
        assert!(
            matches!(*world.resource::<MatchState>(), MatchState::InRound { .. }),
            "{seats:?}"
        );
    }
}

#[test]
fn hold_outside_a_live_round_is_ignored() {
    let mut world = world_in_round();
    let decided = MatchState::decided(400);
    world.insert_resource(decided);
    hold(&mut world, [true, true]);
    assert_eq!(*world.resource::<MatchState>(), decided);
}
