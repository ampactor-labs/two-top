//! A peer that pairs and then never plays: a fixture for the online
//! paths no healthy phone can trigger on demand.
//!
//! It joins a matchbox room exactly as the app does (same two channels),
//! answers the table handshake so the app builds its ggrs session, and
//! then either says nothing on the ggrs channel (`silent`: the session
//! can never synchronize) or drops its connection (`leave`: the peer is
//! gone before the first frame). The app on the other end must give up on
//! it within `netplay::SYNC_BUDGET_SECS`, re-dial the room once, and on a
//! second such peer say COULDN'T REACH YOUR CHALLENGER, with nothing
//! recorded either time.
//!
//! Usage (one app already waiting in the room, e.g. via
//! `scripts/two_peer_smoke.sh`'s environment with one peer):
//!   cargo run -p app --example silent_peer -- ws://127.0.0.1:3536/two-top?next=2 silent
//!   cargo run -p app --example silent_peer -- ws://127.0.0.1:3536/two-top?next=2 leave

#[cfg(not(target_family = "wasm"))]
fn main() {
    use std::time::{Duration, Instant};

    use net::{ChannelConfig, NetMsg, PeerState, WebRtcSocketBuilder, encode_net_msg};

    let mut args = std::env::args().skip(1);
    let url = args
        .next()
        .unwrap_or_else(|| "ws://127.0.0.1:3536/two-top?next=2".to_string());
    let leave = args.next().as_deref() == Some("leave");
    let linger = Duration::from_secs(30);

    let (mut socket, message_loop) = WebRtcSocketBuilder::new(url.clone())
        .add_channel(ChannelConfig::unreliable())
        .add_channel(ChannelConfig::reliable())
        .build();
    std::thread::spawn(move || {
        let _ = bevy::tasks::block_on(message_loop);
    });
    eprintln!(
        "silent_peer: joined {url} ({})",
        if leave { "leave" } else { "silent" }
    );

    let mut paired_at = None;
    loop {
        for (peer, state) in socket.update_peers() {
            match state {
                PeerState::Connected => {
                    eprintln!("silent_peer: paired with {peer}; sending a table pick");
                    socket
                        .channel_mut(1)
                        .send(encode_net_msg(&NetMsg::Table { arena: 0 }), peer);
                    paired_at = Some(Instant::now());
                }
                PeerState::Disconnected => eprintln!("silent_peer: {peer} left"),
            }
        }
        // Read and drop everything: the app's sync requests go unanswered.
        let _ = socket.channel_mut(0).receive();
        let _ = socket.channel_mut(1).receive();
        if let Some(at) = paired_at {
            let waited = at.elapsed();
            if leave && waited > Duration::from_millis(500) {
                eprintln!("silent_peer: leaving");
                return; // dropping the socket closes the connection
            }
            if waited > linger {
                eprintln!("silent_peer: done");
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(16));
    }
}

#[cfg(target_family = "wasm")]
fn main() {}
