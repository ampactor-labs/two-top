//! The rematch bug over a real link.
//!
//! A player mashing THROW as the deciding fang arrives keeps pressing until
//! their own screen shows the result; those presses were made blind and land
//! after the decision (input delay, and online the prediction window, deep).
//! The sim used to read the first of them as RUN IT BACK and restart the
//! match unseen: no summary, no tape, no signed result.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use bevy::prelude::World;
use net_harness::{Duel, LinkConfig, Script, SessionConfig};
use sim::{MatchScore, MatchState, PlayerInput};

/// One kill from a decided match.
fn four_nil(world: &mut World) {
    *world.resource_mut::<MatchScore>() = MatchScore { p0: 4, p1: 0 };
}

/// Charge a fang through the opening and loose it at the other seat
/// (throws find their target from the spawn points).
fn the_killer() -> Script {
    Box::new(|f, _| PlayerInput {
        aim_angle: 64,
        buttons: if (200..240).contains(&f) {
            PlayerInput::THROW_DOWN | PlayerInput::AIM_ACTIVE
        } else {
            0
        },
        ..PlayerInput::default()
    })
}

/// Mash THROW from just before the fang lands until this screen shows the
/// result; `last` keeps the last tick a press was made on.
fn the_masher(last: Arc<AtomicU32>) -> Script {
    Box::new(move |f, state: &MatchState| {
        if state.is_match_over() || f < 236 || f % 2 == 0 {
            return PlayerInput::default();
        }
        last.store(f, Ordering::Relaxed);
        PlayerInput {
            buttons: PlayerInput::THROW_DOWN,
            ..PlayerInput::default()
        }
    })
}

#[test]
fn presses_made_before_a_peer_sees_the_decision_never_restart_it() {
    for latency_steps in [0, 8] {
        let session = SessionConfig {
            boot: four_nil,
            ..SessionConfig::default()
        };
        let last_press = Arc::new(AtomicU32::new(0));
        let mut duel = Duel::new(
            LinkConfig {
                latency_steps,
                drop_every: None,
            },
            session,
            [the_killer(), the_masher(last_press.clone())],
        );
        assert!(
            duel.step_until(4_000, |d| d.peers.iter().all(|p| p.state().is_match_over())),
            "latency {latency_steps}: the match was never decided"
        );
        let rematch_at = duel.peers[0]
            .state()
            .rematch_at()
            .expect("decided, not abandoned");
        let decided = rematch_at - sim::REMATCH_GRACE_FRAMES;
        // The test only means something if a blind press really landed
        // after the decision.
        let landed = last_press.load(Ordering::Relaxed) + session.input_delay as u32;
        assert!(
            landed > decided,
            "latency {latency_steps}: no press landed after the decision (last at {landed}, decided {decided})"
        );
        // Run on until both peers have confirmed well past the grace.
        let target = (rematch_at + 60) as i32;
        assert!(duel.step_until(8_000, |d| d.peers.iter().all(|p| p.confirmed() >= target)));
        for peer in &mut duel.peers {
            assert_eq!(
                peer.state(),
                MatchState::decided(decided),
                "latency {latency_steps}, seat {}: a blind press restarted the match",
                peer.seat
            );
            assert_eq!(peer.score(), MatchScore { p0: 5, p1: 0 });
            assert!(!peer.desynced(), "seat {} desynced", peer.seat);
        }
    }
}
