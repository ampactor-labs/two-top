//! Two peers in one process, over a link you control.
//!
//! Each [`Peer`] is a headless sim (`SimPlugin` on the table
//! `sim::spawn_match_world` lays) driving a real ggrs `P2PSession` whose
//! socket is one end of a [`Link`]. The link delivers each packet a fixed
//! number of steps after it was sent and can drop every n-th one, so a
//! test can put a match exactly where localhost never goes: a peer
//! predicting the other for most of its prediction window, rollbacks that
//! rewrite a decided tick, a settle racing a correction.
//!
//! Packets cross the link in their wire bytes, through `net`'s own codec
//! and guard (`encode_message` / `decode_packet`), the path a real match
//! takes. Both peers run on a manual clock: one [`Duel::step`] is one
//! `App::update` on each side (one sim tick, unless ggrs holds the side
//! that is too far ahead) and then one tick of link time.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy_ggrs::ggrs::{Message, NonBlockingSocket, PlayerType, SessionBuilder};
use bevy_ggrs::prelude::*;
use bevy_ggrs::{GgrsPlugin, LocalInputs, Session};
use sim::{ArenaId, GgrsCfg, NetAddr, PlayerInput, SimPlugin};

/// The two directions of an in-memory wire, on one shared clock.
#[derive(Default)]
struct Wire {
    now: u64,
    /// Packets in flight to peer 0 and to peer 1: (deliver at, bytes).
    inbound: [VecDeque<(u64, Box<[u8]>)>; 2],
    /// Packets sent so far in each direction (for drop_every).
    sent: [u64; 2],
}

/// How the link behaves.
#[derive(Clone, Copy, Debug, Default)]
pub struct LinkConfig {
    /// Steps between a packet being sent and arriving (one way).
    pub latency_steps: u64,
    /// Drop every n-th packet in each direction (`None`: lose nothing).
    pub drop_every: Option<u64>,
}

/// The shared link both peers' sockets sit on.
#[derive(Clone, Default)]
pub struct Link {
    wire: Arc<Mutex<Wire>>,
    config: Arc<Mutex<LinkConfig>>,
}

impl Link {
    pub fn new(config: LinkConfig) -> Self {
        Self {
            wire: Arc::default(),
            config: Arc::new(Mutex::new(config)),
        }
    }

    /// Change the link mid-match (a stall, a lossy patch).
    pub fn set(&self, config: LinkConfig) {
        *self.config.lock().unwrap() = config;
    }

    /// Advance link time one step.
    fn tick(&self) {
        self.wire.lock().unwrap().now += 1;
    }

    fn end(&self, me: usize) -> LinkEnd {
        LinkEnd {
            link: self.clone(),
            me,
        }
    }
}

/// The address each seat goes by on the link.
pub fn seat_addr(seat: usize) -> NetAddr {
    NetAddr(0x2_70b + seat as u128)
}

fn seat_peer(seat: usize) -> net::MatchboxPeerId {
    net::addr_to_peer(NetAddr(0x2_70b + seat as u128))
}

/// One peer's end of the link: a ggrs socket.
struct LinkEnd {
    link: Link,
    me: usize,
}

impl NonBlockingSocket<NetAddr> for LinkEnd {
    fn send_to(&mut self, msg: &Message, _addr: &NetAddr) {
        let config = *self.link.config.lock().unwrap();
        let mut wire = self.link.wire.lock().unwrap();
        let to = 1 - self.me;
        wire.sent[to] += 1;
        if config
            .drop_every
            .is_some_and(|n| n > 0 && wire.sent[to].is_multiple_of(n))
        {
            return;
        }
        let at = wire.now + config.latency_steps;
        wire.inbound[to].push_back((at, net::encode_message(msg)));
    }

    fn receive_all_messages(&mut self) -> Vec<(NetAddr, Message)> {
        let mut wire = self.link.wire.lock().unwrap();
        let now = wire.now;
        let from = seat_peer(1 - self.me);
        let mut out = Vec::new();
        while wire.inbound[self.me]
            .front()
            .is_some_and(|(at, _)| *at <= now)
        {
            let (_, bytes) = wire.inbound[self.me].pop_front().unwrap();
            if let Some(decoded) = net::decode_packet((from, bytes)) {
                out.push(decoded);
            }
        }
        out
    }
}

/// What a peer presses on each of its own ticks: `(frame) -> input`.
pub type Script = Box<dyn FnMut(u32) -> PlayerInput + Send + Sync>;

#[derive(Resource)]
struct Scripted {
    handle: usize,
    script: Script,
    frame: u32,
}

fn scripted_input(mut scripted: ResMut<Scripted>, mut commands: Commands) {
    let frame = scripted.frame;
    let input = (scripted.script)(frame);
    scripted.frame += 1;
    let mut map = bevy::platform::collections::HashMap::default();
    map.insert(scripted.handle, input);
    commands.insert_resource(LocalInputs::<GgrsCfg>(map));
}

/// Session settings both peers use.
#[derive(Clone, Copy, Debug)]
pub struct SessionConfig {
    pub input_delay: usize,
    pub max_prediction: usize,
    pub arena: ArenaId,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            input_delay: 3,
            max_prediction: 16,
            arena: ArenaId::Vigil,
        }
    }
}

/// One side of the duel: its own app, its own session, its own seat.
pub struct Peer {
    pub app: App,
    pub seat: usize,
}

impl Peer {
    fn new(link: &Link, seat: usize, config: SessionConfig, script: Script) -> Self {
        let mut sb = SessionBuilder::<GgrsCfg>::new()
            .with_num_players(2)
            .expect("two players")
            .with_input_delay(config.input_delay)
            .with_max_prediction_window(config.max_prediction)
            .with_desync_detection_mode(bevy_ggrs::ggrs::DesyncDetection::On { interval: 30 });
        sb = sb.add_player(PlayerType::Local, seat).expect("local seat");
        sb = sb
            .add_player(PlayerType::Remote(seat_addr(1 - seat)), 1 - seat)
            .expect("remote seat");
        let session = sb.start_p2p_session(link.end(seat)).expect("p2p session");

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / sim::TICK_HZ as f64,
        )));
        app.add_plugins(GgrsPlugin::<GgrsCfg>::default());
        app.add_plugins(SimPlugin);
        app.insert_resource(sim::SelectedArena(config.arena));
        sim::spawn_match_world_now(app.world_mut(), config.arena);
        app.insert_resource(Scripted {
            handle: seat,
            script,
            frame: 0,
        });
        app.add_systems(ReadInputs, scripted_input);
        app.insert_resource(Session::P2P(session));
        Self { app, seat }
    }

    /// The live session (panics if a test removed it).
    pub fn session(&self) -> &bevy_ggrs::ggrs::P2PSession<GgrsCfg> {
        match self.app.world().resource::<Session<GgrsCfg>>() {
            Session::P2P(s) => s,
            _ => panic!("not a P2P session"),
        }
    }

    /// The sim frame this peer has simulated up to (predictions included).
    pub fn frame(&self) -> u32 {
        self.app.world().resource::<sim::FrameCount>().0
    }

    pub fn state(&self) -> sim::MatchState {
        *self.app.world().resource::<sim::MatchState>()
    }

    pub fn score(&self) -> sim::MatchScore {
        *self.app.world().resource::<sim::MatchScore>()
    }

    /// The last frame both peers' inputs are known for, as bevy_ggrs last
    /// applied it to the world.
    pub fn confirmed(&self) -> i32 {
        self.app
            .world()
            .resource::<bevy_ggrs::ConfirmedFrameCount>()
            .0
    }
}

/// Two peers over one link.
pub struct Duel {
    pub link: Link,
    pub peers: [Peer; 2],
}

impl Duel {
    /// Two peers, synchronized, with the link set to `link` from then on.
    ///
    /// The ggrs sync handshake runs first on a lossless link of the same
    /// latency: ggrs retries a lost sync packet on a wall-clock timer
    /// (200 ms), which a manual-clock harness never reaches, so a packet
    /// dropped during sync would stall it forever. Losses start once both
    /// peers are running.
    pub fn new(link: LinkConfig, session: SessionConfig, scripts: [Script; 2]) -> Self {
        let lossless = LinkConfig {
            drop_every: None,
            ..link
        };
        let wire = Link::new(lossless);
        let [s0, s1] = scripts;
        let peers = [
            Peer::new(&wire, 0, session, s0),
            Peer::new(&wire, 1, session, s1),
        ];
        let mut duel = Self { link: wire, peers };
        let synced = duel.step_until(2_000, |d| {
            d.peers
                .iter()
                .all(|p| p.session().current_state() == bevy_ggrs::ggrs::SessionState::Running)
        });
        assert!(synced, "the two peers never synchronized");
        duel.link.set(link);
        duel
    }

    /// One step: each app updates once, then the link moves one tick.
    ///
    /// When neither peer advanced (both held at the prediction window,
    /// waiting on a lost packet), the step also lets a millisecond of real
    /// time pass: ggrs resends unacknowledged inputs on a wall-clock timer,
    /// and a manual clock that never lets real time pass would wait forever.
    pub fn step(&mut self) {
        let before = [self.peers[0].frame(), self.peers[1].frame()];
        for peer in &mut self.peers {
            peer.app.update();
        }
        self.link.tick();
        if before == [self.peers[0].frame(), self.peers[1].frame()] {
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Step until `done` holds or `max` steps pass. Returns whether it held.
    pub fn step_until(&mut self, max: u32, mut done: impl FnMut(&Duel) -> bool) -> bool {
        for _ in 0..max {
            if done(self) {
                return true;
            }
            self.step();
        }
        done(self)
    }
}

/// A script that presses nothing.
pub fn idle() -> Script {
    Box::new(|_| PlayerInput::default())
}
