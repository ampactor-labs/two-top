//! LIVE: a duel streamed as it is played, watched anywhere from a link.
//!
//! A broadcast is a tape the relay holds open (`crates/tape_drop`): the
//! phone opens one, every second appends the ticks that have become fact
//! (confirmed by both phones, so a viewer never sees a kill that is rolled
//! back), and ends it when the duel does. The stream is the replay crate's
//! live format: the header, then raw input pairs.
//!
//! * **Going live** is the BROADCAST row in Settings. A tap opens the tape
//!   at the relay and shows its watch link as a QR card (the same card
//!   SHARE shows); the next online duel goes out on it. Both phones wear
//!   the LIVE mark while it does ([`net::NetMsg::Live`]): nobody is
//!   broadcast without seeing it. Tapping the row's left half stops.
//! * **Watching** is the theater, on a tape that is still growing: the
//!   page (`web/index.html`) hands the first bytes and the tape's address
//!   to the game, [`LiveFeed`] keeps asking the relay for more (a long
//!   poll), and the theater plays what has arrived. Starved, it holds on
//!   the last tick it has; behind by more than a few seconds, it plays at
//!   1.25x until it has caught up. When the duel ends the tape is simply
//!   a tape.
//!
//! Broadcasting is the app's (a thread per upload); the browser build
//! watches, and its GO LIVE row says to broadcast from the app.

// The broadcasting half is compiled for the browser but never driven there.
#![cfg_attr(target_family = "wasm", allow(dead_code))]

use bevy::prelude::*;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};

use crate::screen::AppScreen;

/// How often the broadcaster appends what has been confirmed.
const APPEND_EVERY_SECS: f32 = 1.0;

/// The broadcast state on this phone.
#[derive(Resource, Default)]
pub struct Broadcast {
    state: BroadcastState,
}

#[derive(Default)]
enum BroadcastState {
    #[default]
    Off,
    /// Asking the relay for a tape.
    Opening(Mutex<Receiver<Result<OpenTape, String>>>),
    /// A tape is open and waiting for the next duel.
    Armed(OpenTape),
    /// The duel is going out.
    OnAir {
        tape: OpenTape,
        jobs: Sender<Job>,
        /// Ticks sent so far.
        sent: usize,
        seq: u32,
        since_append: f32,
        failed: Arc<std::sync::atomic::AtomicBool>,
    },
    /// The relay could not be reached, or refused.
    Failed(String),
}

#[derive(Clone)]
struct OpenTape {
    /// `<relay>/tape/<id>`.
    url: String,
    token: String,
    /// What a viewer opens.
    watch: String,
}

enum Job {
    Put { seq: u32, bytes: Vec<u8> },
    End,
}

impl Broadcast {
    /// The Settings row's text.
    pub fn row_label(&self) -> String {
        match &self.state {
            BroadcastState::Off => "go live".into(),
            BroadcastState::Opening(_) => "going live...".into(),
            BroadcastState::Armed(_) => "live: next duel  (link >)".into(),
            BroadcastState::OnAir { .. } => "ON AIR  (link >)".into(),
            BroadcastState::Failed(why) => format!("live off: {}", short(why)),
        }
    }

    pub fn on_air(&self) -> bool {
        matches!(self.state, BroadcastState::OnAir { .. })
    }
}

/// A failure's reason, short enough for a settings row.
fn short(why: &str) -> &str {
    let cut = why.char_indices().nth(22).map_or(why.len(), |(i, _)| i);
    &why[..cut]
}

/// Who is broadcasting the duel on screen: us, or the peer (who told us).
#[derive(Resource, Default)]
pub struct OnAir {
    pub peer: bool,
}

#[derive(Component, Clone)]
struct LiveCard;

#[derive(Component)]
struct LiveMark;

/// The relay's answer to an open.
#[derive(serde::Deserialize)]
struct Opened {
    id: String,
    token: String,
}

#[cfg(not(target_family = "wasm"))]
fn open_tape(drop: &str, watch: &str) -> Result<OpenTape, String> {
    let base = drop.trim_end_matches('/');
    let opened: Opened = ureq::post(&format!("{base}/tape?live=1"))
        .timeout(std::time::Duration::from_secs(6))
        .send_bytes(&[])
        .map_err(|e| format!("relay unreachable: {e}"))?
        .into_json()
        .map_err(|e| format!("relay answered strangely: {e}"))?;
    Ok(OpenTape {
        url: format!("{base}/tape/{}", opened.id),
        token: opened.token,
        watch: crate::share::compose_watch_url(watch, &opened.id),
    })
}

/// The uploader: one thread per broadcast, sending chunks strictly in
/// order. A chunk is retried; one that cannot be sent ends the broadcast
/// (a gap would make the rest unplayable).
#[cfg(not(target_family = "wasm"))]
fn uploader(tape: OpenTape, jobs: Receiver<Job>, failed: Arc<std::sync::atomic::AtomicBool>) {
    use std::time::Duration;
    for job in jobs {
        let (url, bytes) = match job {
            Job::Put { seq, bytes } => (format!("{}/{seq}", tape.url), Some(bytes)),
            Job::End => (format!("{}/end", tape.url), None),
        };
        let mut ok = false;
        for attempt in 0..5u32 {
            let request = match &bytes {
                Some(_) => ureq::put(&url),
                None => ureq::post(&url),
            }
            .timeout(Duration::from_secs(8))
            .set("X-Live-Token", &tape.token);
            match request.send_bytes(bytes.as_deref().unwrap_or(&[])) {
                Ok(_) => {
                    ok = true;
                    break;
                }
                // Refused outright (not ours, ended, too large): no retry
                // will change that.
                Err(ureq::Error::Status(code, _)) if code != 429 && code < 500 => break,
                Err(e) => {
                    tracing::debug!(target: "two_top::live", attempt, error = %e, "append retry");
                    std::thread::sleep(Duration::from_millis(400 << attempt));
                }
            }
        }
        if !ok {
            tracing::warn!(target: "two_top::live", %url, "the broadcast lost its relay — ending it");
            failed.store(true, std::sync::atomic::Ordering::Relaxed);
            return;
        }
        if bytes.is_none() {
            return;
        }
    }
}

/// Ask the relay for a live tape (off the main thread).
fn begin_open(live: &mut Broadcast, config: &crate::share::ShareConfig) {
    let (Some(drop), Some(watch)) = (config.drop_url.clone(), config.watch_url.clone()) else {
        live.state = BroadcastState::Failed("this build has no relay".into());
        return;
    };
    #[cfg(not(target_family = "wasm"))]
    {
        let (tx, rx) = channel();
        crate::platform::off_main_thread("twotop-live-open", move || {
            let _ = tx.send(open_tape(&drop, &watch));
        });
        live.state = BroadcastState::Opening(Mutex::new(rx));
    }
    #[cfg(target_family = "wasm")]
    {
        let _ = (drop, watch);
        live.state = BroadcastState::Failed("broadcast from the app".into());
    }
}

/// `TWOTOP_GO_LIVE=1`: arm a broadcast at boot, as the row does.
fn go_live_at_boot(config: Res<crate::share::ShareConfig>, mut live: ResMut<Broadcast>) {
    if crate::config::Knob::GoLive.is_on() {
        begin_open(&mut live, &config);
    }
}

/// The GO LIVE row: right half (or the link when armed) opens and shows,
/// left half stops. An open that lands shows its link if the player is
/// still on Settings.
#[allow(clippy::too_many_arguments)]
fn go_live_row(
    mut commands: Commands,
    screen: Res<State<AppScreen>>,
    touches: Res<Touches>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Res<input_touch::WindowSize>,
    config: Res<crate::share::ShareConfig>,
    mut images: ResMut<Assets<Image>>,
    mut live: ResMut<Broadcast>,
    cards: Query<Entity, With<LiveCard>>,
) {
    let on_settings = *screen.get() == AppScreen::Settings;
    if let BroadcastState::Opening(rx) = &mut live.state {
        let got = rx.get_mut().map(|rx| rx.try_recv());
        match got {
            Ok(Ok(Ok(tape))) => {
                // The smokes read the link from here.
                tracing::info!(target: "two_top::live", url = %tape.watch, "live tape open");
                if on_settings {
                    crate::share::spawn_link_card(
                        &mut commands,
                        &mut images,
                        &tape.watch,
                        LiveCard,
                    );
                }
                live.state = BroadcastState::Armed(tape);
            }
            Ok(Ok(Err(why))) => {
                tracing::warn!(target: "two_top::live", %why, "could not go live");
                live.state = BroadcastState::Failed(why);
            }
            Ok(Err(TryRecvError::Disconnected)) | Err(_) => {
                live.state = BroadcastState::Failed("no answer".into());
            }
            Ok(Err(TryRecvError::Empty)) => {}
        }
    }
    if !on_settings {
        for e in &cards {
            commands.entity(e).despawn();
        }
        return;
    }
    let win = window.0;
    let mut tap = None;
    if win.x > 0.0 && win.y > 0.0 {
        for t in touches.iter_just_pressed() {
            let p = t.position();
            if !cards.is_empty() {
                // Any tap closes the card.
                for e in &cards {
                    commands.entity(e).despawn();
                }
                return;
            }
            if crate::settings::row_at(p.y / win.y) == Some(crate::settings::LIVE_ROW) {
                tap = Some(p.x > win.x * 0.5);
            }
        }
    }
    if keys.just_pressed(KeyCode::KeyG) {
        tap = Some(true);
    }
    let Some(right) = tap else {
        return;
    };
    match (&live.state, right) {
        (BroadcastState::Armed(t), true) | (BroadcastState::OnAir { tape: t, .. }, true) => {
            let watch = t.watch.clone();
            crate::share::spawn_link_card(&mut commands, &mut images, &watch, LiveCard);
        }
        (BroadcastState::Off | BroadcastState::Failed(_), true) => begin_open(&mut live, &config),
        (_, false) => stop(&mut live),
        _ => {}
    }
}

/// End whatever is open.
fn stop(live: &mut Broadcast) {
    if let BroadcastState::OnAir { jobs, .. } = &live.state {
        let _ = jobs.send(Job::End);
    }
    live.state = BroadcastState::Off;
}

/// Take an armed tape on air when an online duel starts, append what has
/// been confirmed once a second, end it when the duel's screen closes.
#[allow(clippy::too_many_arguments)]
fn feed_broadcast(
    screen: Res<State<AppScreen>>,
    time: Res<Time<Real>>,
    clock: crate::sim_clock::SimClock,
    rec: Res<crate::recorder::MatchRecorder>,
    netplay: Res<crate::netplay::NetplayConfig>,
    local: Res<crate::netplay::LocalPlayerHandle>,
    theater: Res<crate::theater::TheaterMode>,
    (profile, peer, selected): (
        Res<crate::profile::LocalProfile>,
        Res<net::PeerProfile>,
        Res<sim::SelectedArena>,
    ),
    mut queue: ResMut<net::NetSendQueue>,
    mut live: ResMut<Broadcast>,
) {
    let in_duel = *screen.get() == AppScreen::InMatch
        && netplay.room_url.is_some()
        && local.0.is_some()
        && !theater.active();
    let confirmed = clock.confirmed_tick();
    match &mut live.state {
        BroadcastState::Armed(tape) if in_duel && confirmed >= 0 => {
            let header = replay::ReplayHeader {
                magic: replay::MAGIC,
                format_version: replay::FORMAT_VERSION,
                sim_version: sim::SIM_VERSION,
                seed: 0,
                num_players: 2,
                frame_rate: sim::TICK_HZ as u8,
                frame_count: 0,
                recorded_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs()),
                winner: None,
                player_handles: crate::recorder::header_names(
                    &netplay, false, None, local.0, &profile, &peer,
                ),
                arena_id: selected.0.as_u8(),
                start_tick: 0,
                kind: replay::TapeKind::Duel,
                kills: Vec::new(),
            };
            let Ok(head) = replay::encode_live_head(&header) else {
                return;
            };
            let tape = tape.clone();
            let (jobs, rx) = channel();
            let failed = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let _ = jobs.send(Job::Put {
                seq: 0,
                bytes: head,
            });
            #[cfg(not(target_family = "wasm"))]
            {
                let (tape, failed) = (tape.clone(), failed.clone());
                crate::platform::off_main_thread("twotop-live", move || uploader(tape, rx, failed));
            }
            #[cfg(target_family = "wasm")]
            drop(rx);
            queue.0.push(net::NetMsg::Live);
            tracing::info!(target: "two_top::live", "on air");
            live.state = BroadcastState::OnAir {
                tape,
                jobs,
                sent: 0,
                seq: 1,
                since_append: 0.0,
                failed,
            };
        }
        BroadcastState::OnAir {
            jobs,
            sent,
            seq,
            since_append,
            failed,
            ..
        } => {
            if failed.load(std::sync::atomic::Ordering::Relaxed) {
                live.state = BroadcastState::Failed("the relay dropped the broadcast".into());
                return;
            }
            *since_append += time.delta_secs();
            let leaving = !in_duel;
            if *since_append < APPEND_EVERY_SECS && !leaving {
                return;
            }
            *since_append = 0.0;
            // Confirmed ticks only: a viewer must never see a kill that a
            // rollback takes back.
            let fact = usize::try_from(confirmed + 1)
                .unwrap_or(0)
                .min(rec.inputs().len());
            if fact > *sent {
                let bytes = replay::encode_live_pairs(&rec.inputs()[*sent..fact]);
                let _ = jobs.send(Job::Put { seq: *seq, bytes });
                *seq += 1;
                *sent = fact;
            }
            if leaving {
                tracing::info!(target: "two_top::live", ticks = *sent, "off air");
                stop(&mut live);
            }
        }
        _ => {}
    }
}

fn clear_peer_mark(mut on_air: ResMut<OnAir>) {
    on_air.peer = false;
}

fn spawn_mark(mut commands: Commands) {
    commands.spawn((
        LiveMark,
        Text2d::new("● LIVE"),
        (
            TextFont {
                font_size: crate::ui::TextRole::ShareLabel.font_size(),
                ..default()
            },
            crate::ui::TextRole::ShareLabel,
        ),
        TextColor(render::palette::P0_BLOOD),
        crate::anchor::ScreenAnchor::new(-0.80, 0.80, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, crate::ui::z::MENU_TEXT - 5.0),
        Visibility::Hidden,
    ));
}

/// The LIVE mark: on both phones of a broadcast duel, and on a viewer's
/// screen while the tape is still growing.
fn show_mark(
    screen: Res<State<AppScreen>>,
    live: Res<Broadcast>,
    on_air: Res<OnAir>,
    theater: Res<crate::theater::TheaterMode>,
    mut q: Query<&mut Visibility, With<LiveMark>>,
) {
    let shown =
        *screen.get() == AppScreen::InMatch && (live.on_air() || on_air.peer || theater.live());
    for mut vis in &mut q {
        *vis = if shown {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}

// ---------------------------------------------------------------------------
// Watching.
// ---------------------------------------------------------------------------

/// One read's outcome: the bytes, and whether the tape is still open.
type Read = Result<(Vec<u8>, bool), String>;

/// A live tape being watched: everything received so far, and the next
/// read in flight.
#[derive(Resource)]
pub struct LiveFeed {
    /// `<relay>/tape/<id>`.
    url: String,
    bytes: Vec<u8>,
    /// Bytes of `bytes` already handed to the theater (the header and the
    /// whole pairs after it).
    used: usize,
    open: bool,
    rolling: bool,
    in_flight: bool,
    /// Consecutive failed reads; the feed gives up after a few.
    fails: u32,
    wait_until: f64,
    tx: Sender<Read>,
    rx: Mutex<Receiver<Read>>,
}

impl LiveFeed {
    /// A feed of the tape at `url`, starting from `first` (what the page
    /// already fetched).
    pub fn new(url: String, first: Vec<u8>, open: bool) -> Self {
        let (tx, rx) = channel();
        Self {
            url,
            bytes: first,
            used: 0,
            open,
            rolling: false,
            in_flight: false,
            fails: 0,
            wait_until: 0.0,
            tx,
            rx: Mutex::new(rx),
        }
    }
}

/// One read of the tape from `from` (a long poll at the relay).
#[cfg(not(target_family = "wasm"))]
fn read_from(url: String, from: usize, tx: Sender<Read>) {
    crate::platform::off_main_thread("twotop-live-read", move || {
        let got = ureq::get(&format!("{url}?from={from}"))
            .timeout(std::time::Duration::from_secs(30))
            .call()
            .map_err(|e| e.to_string())
            .and_then(|resp| {
                let open = resp.header("X-Live-Open") == Some("1");
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(&mut resp.into_reader(), &mut bytes)
                    .map_err(|e| e.to_string())?;
                Ok((bytes, open))
            });
        let _ = tx.send(got);
    });
}

#[cfg(target_family = "wasm")]
fn read_from(url: String, from: usize, tx: Sender<Read>) {
    wasm_bindgen_futures::spawn_local(async move {
        let _ = tx.send(read_from_web(format!("{url}?from={from}")).await);
    });
}

#[cfg(target_family = "wasm")]
async fn read_from_web(url: String) -> Result<(Vec<u8>, bool), String> {
    use wasm_bindgen::JsCast;
    let window = web_sys::window().ok_or("no window")?;
    let response = wasm_bindgen_futures::JsFuture::from(window.fetch_with_str(&url))
        .await
        .map_err(|_| "relay unreachable".to_string())?;
    let response: web_sys::Response = response
        .dyn_into()
        .map_err(|_| "relay answered strangely".to_string())?;
    if !response.ok() {
        return Err(format!("relay answered {}", response.status()));
    }
    let open = response
        .headers()
        .get("X-Live-Open")
        .ok()
        .flatten()
        .as_deref()
        == Some("1");
    let buffer = wasm_bindgen_futures::JsFuture::from(
        response.array_buffer().map_err(|_| "no body".to_string())?,
    )
    .await
    .map_err(|_| "no body".to_string())?;
    Ok((js_sys::Uint8Array::new(&buffer).to_vec(), open))
}

/// Feed the theater: roll the tape once its header has arrived, hand it
/// each new whole tick, and keep a read in flight while the tape is open.
pub(crate) fn pump_live_feed(world: &mut World) {
    let Some(mut feed) = world.remove_resource::<LiveFeed>() else {
        return;
    };
    let now = world.resource::<Time<Real>>().elapsed_secs_f64();
    loop {
        let got = feed.rx.get_mut().map(|rx| rx.try_recv());
        match got {
            Ok(Ok(Ok((bytes, open)))) => {
                feed.in_flight = false;
                feed.fails = 0;
                feed.bytes.extend_from_slice(&bytes);
                feed.open = open;
            }
            Ok(Ok(Err(why))) => {
                feed.in_flight = false;
                feed.fails += 1;
                feed.wait_until = now + 2.0 * feed.fails as f64;
                tracing::warn!(target: "two_top::live", %why, fails = feed.fails, "live read failed");
            }
            _ => break,
        }
    }
    if !feed.rolling {
        match replay::decode_live(&feed.bytes) {
            Ok(Some(tape)) if tape.header.sim_version == sim::SIM_VERSION => {
                let (_, head) = replay::decode_live_head(&feed.bytes)
                    .ok()
                    .flatten()
                    .expect("decoded above");
                feed.used = head + tape.inputs.len() * replay::LIVE_PAIR_BYTES;
                feed.rolling = true;
                let open = feed.open;
                crate::theater::start_playback(world, tape);
                crate::theater::set_live(world, open);
            }
            Ok(Some(tape)) => {
                tracing::error!(target: "two_top::live", got = tape.header.sim_version, "live tape is from another version of the game");
                return;
            }
            Ok(None) => {}
            Err(e) => {
                tracing::error!(target: "two_top::live", error = %e, "not a live tape");
                return;
            }
        }
    } else {
        let (pairs, used) = replay::decode_live_pairs(&feed.bytes[feed.used..]);
        feed.used += used;
        if !pairs.is_empty() || !feed.open {
            crate::theater::extend_live(world, &pairs, feed.open);
        }
        // The viewer left the tape: the feed goes with it.
        if !world.resource::<crate::theater::TheaterMode>().active() {
            return;
        }
    }
    if feed.open && !feed.in_flight && feed.fails < 8 && now >= feed.wait_until {
        feed.in_flight = true;
        read_from(feed.url.clone(), feed.bytes.len(), feed.tx.clone());
    }
    world.insert_resource(feed);
}

pub struct LivePlugin;

impl Plugin for LivePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Broadcast>()
            .init_resource::<OnAir>()
            .add_systems(Startup, (spawn_mark, go_live_at_boot))
            .add_systems(OnExit(AppScreen::InMatch), clear_peer_mark)
            .add_systems(
                Update,
                (go_live_row, feed_broadcast, show_mark, pump_live_feed).chain(),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_row_says_where_the_broadcast_is() {
        let mut live = Broadcast::default();
        assert_eq!(live.row_label(), "go live");
        live.state = BroadcastState::Armed(OpenTape {
            url: "u".into(),
            token: "t".into(),
            watch: "w".into(),
        });
        assert!(live.row_label().contains("next duel"));
        assert!(!live.on_air());
    }
}
