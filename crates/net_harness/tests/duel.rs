//! The harness itself: two peers over a slow link play in lockstep.

use net_harness::{Duel, LinkConfig, Script, SessionConfig};
use sim::PlayerInput;

/// Walk left and right, throwing now and then: enough to move state and
/// make the peers predict each other wrong.
fn restless(seed: u32) -> Script {
    Box::new(move |f| {
        let leg = (f + seed * 37) / 45;
        let stick_x = if leg.is_multiple_of(2) { 90 } else { -90 };
        let stick_y = if (leg / 3).is_multiple_of(2) { 40 } else { -40 };
        let buttons = if (f + seed * 11) % 97 < 6 {
            PlayerInput::THROW_DOWN
        } else {
            0
        };
        PlayerInput {
            stick_x,
            stick_y,
            aim_angle: ((f / 7) % 255) as u8,
            buttons,
        }
    })
}

#[test]
fn two_peers_over_a_slow_link_agree() {
    // A round trip past the prediction window (12 each way, 16 frames)
    // holds both peers every few frames; loss on top of it means waiting
    // on ggrs's wall-clock resend.
    for (latency_steps, drop_every) in [(0, Some(9)), (6, Some(9)), (12, None), (12, Some(25))] {
        let mut duel = Duel::new(
            LinkConfig {
                latency_steps,
                drop_every,
            },
            SessionConfig::default(),
            [restless(1), restless(2)],
        );
        // Both peers must run well past the countdown; track how far ahead
        // of the confirmed frame each ran on prediction.
        let mut lead = 0i64;
        let ran = duel.step_until(20_000, |d| {
            for p in &d.peers {
                lead = lead.max(i64::from(p.frame()) - i64::from(p.confirmed()));
            }
            d.peers.iter().all(|p| p.confirmed() >= 900)
        });
        if latency_steps >= 6 {
            // The point of the harness: a peer really does play on
            // predictions here, well past the input delay.
            assert!(
                lead > 6,
                "latency {latency_steps}: peers never ran ahead on prediction (lead {lead})"
            );
        }
        assert!(
            ran,
            "latency {latency_steps}: the duel never confirmed 900 frames (at {} / {})",
            duel.peers[0].confirmed(),
            duel.peers[1].confirmed(),
        );
        for peer in &mut duel.peers {
            let events: Vec<_> = match peer
                .app
                .world_mut()
                .resource_mut::<bevy_ggrs::Session<sim::GgrsCfg>>()
                .into_inner()
            {
                bevy_ggrs::Session::P2P(s) => s.events().collect(),
                _ => unreachable!(),
            };
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(e, bevy_ggrs::ggrs::GgrsEvent::DesyncDetected { .. })),
                "latency {latency_steps}: seat {} saw a desync",
                peer.seat,
            );
        }
    }
}
