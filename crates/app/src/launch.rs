//! One launch link, parsed once.
//!
//! Every way 2-Top is opened with a purpose — the Android intent, the
//! join page's clipboard hand-off, a desktop argument or a second launch's
//! hand-off, the page's `#` fragment — arrives as a string, and
//! [`LaunchLink::parse`] says what it asks for:
//!
//! * **Join** a dialed table: `twotop://join/CURS-pit`, `…/join.html#CURS-pit`
//!   or the bare `CURS-pit` (`room_code::parse_join`).
//! * **Watch** a tape, sealed or live: `twotop://watch/<id>` or
//!   `…#watch=<id>`. The relay says which; a live tape plays as it grows.
//! * **Spar a shade** someone sent: `twotop://shade/<card>` or
//!   `…#shade=<card>`, where the card is their name, install id and the
//!   four habits the practice bot plays them with ([`ShadeCard`]).

use bevy::prelude::*;

use crate::screen::AppScreen;

/// What a launch link asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum LaunchLink {
    Join {
        slots: [u8; crate::room_code::CODE_LEN],
        arena: sim::ArenaId,
    },
    Watch {
        id: String,
    },
    Shade(ShadeCard),
}

/// A shade, as a link carries it: who, and how they play.
#[derive(Clone, Debug, PartialEq)]
pub struct ShadeCard {
    pub name: String,
    pub install_id: u128,
    pub style: crate::bot::BotStyle,
}

impl ShadeCard {
    /// `NAME.<32 hex>.<commit %>.<dodge cm>.<wobble mrad>.<range cm>`:
    /// plain ASCII a URL carries untouched.
    pub fn encode(&self) -> String {
        let s = &self.style;
        format!(
            "{}.{:032x}.{}.{}.{}.{}",
            self.name,
            self.install_id,
            (s.commit_frac * 100.0).round() as u32,
            s.dodge_radius.round() as u32,
            (s.wobble * 1000.0).round() as u32,
            s.range.round() as u32,
        )
    }

    /// The card back; `None` for anything malformed. The habits are
    /// clamped into the range the fit itself produces, so a hand-edited
    /// link cannot build an unbeatable or broken duelist.
    pub fn decode(text: &str) -> Option<Self> {
        let parts: Vec<&str> = text.split('.').collect();
        let [name, id, commit, dodge, wobble, range] = parts.as_slice() else {
            return None;
        };
        let name_ok =
            (1..=8).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphabetic());
        if !name_ok || id.len() != 32 {
            return None;
        }
        let num = |s: &str| s.parse::<u32>().ok().map(|v| v as f32);
        Some(Self {
            name: name.to_ascii_uppercase(),
            install_id: u128::from_str_radix(id, 16).ok()?,
            style: crate::bot::BotStyle {
                commit_frac: (num(commit)? / 100.0).clamp(0.25, 0.85),
                dodge_radius: num(dodge)?.clamp(60.0, 300.0),
                wobble: (num(wobble)? / 1000.0).clamp(0.06, 0.32),
                range: num(range)?.clamp(340.0, 520.0),
            },
        })
    }
}

impl LaunchLink {
    pub fn parse(uri: &str) -> Option<Self> {
        let uri = uri.trim();
        let tagged = |scheme: &str, frag: &str| {
            uri.strip_prefix(scheme)
                .or_else(|| uri.split_once(frag).map(|(_, rest)| rest))
        };
        if let Some(id) = tagged("twotop://watch/", "#watch=") {
            let id = id.trim_end_matches('/');
            let ok = id.len() == 12 && id.bytes().all(|b| b.is_ascii_hexdigit());
            return ok.then(|| Self::Watch {
                id: id.to_ascii_lowercase(),
            });
        }
        if let Some(card) = tagged("twotop://shade/", "#shade=") {
            return ShadeCard::decode(card.trim_end_matches('/')).map(Self::Shade);
        }
        crate::room_code::parse_join(uri).map(|(slots, arena)| Self::Join { slots, arena })
    }
}

/// `twotop://shade/<card>`: the link a player sends so a friend can spar
/// their shade.
pub fn shade_link(card: &ShadeCard) -> String {
    format!("twotop://shade/{}", card.encode())
}

/// A tape the launch link asked to watch, on its way from the relay.
#[derive(Resource)]
struct PendingFetch(std::sync::Mutex<std::sync::mpsc::Receiver<Result<Fetched, String>>>);

struct Fetched {
    url: String,
    bytes: Vec<u8>,
    /// `Some(open)` for a live tape.
    live: Option<bool>,
}

#[cfg(not(target_family = "wasm"))]
fn fetch_tape(url: String) -> Result<Fetched, String> {
    let resp = ureq::get(&url)
        .timeout(std::time::Duration::from_secs(30))
        .call()
        .map_err(|e| format!("relay: {e}"))?;
    let live = resp.header("X-Live-Open").map(|v| v == "1");
    let mut bytes = Vec::new();
    use std::io::Read as _;
    resp.into_reader()
        .take(4 << 20)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    Ok(Fetched { url, bytes, live })
}

/// Act on a launch link: dial a table, fetch a tape, or summon a shade.
pub fn dispatch(world: &mut World, uri: &str) {
    let Some(link) = LaunchLink::parse(uri) else {
        tracing::info!(target: "two_top::launch", "launch link not understood");
        return;
    };
    match link {
        LaunchLink::Join { slots, arena } => crate::room_code::dial(world, slots, arena),
        LaunchLink::Watch { id } => {
            let Some(drop) = crate::config::Endpoint::DropUrl.get() else {
                tracing::warn!(target: "two_top::launch", "a watch link, but this build has no relay");
                return;
            };
            let url = format!("{}/tape/{id}", drop.trim_end_matches('/'));
            tracing::info!(target: "two_top::launch", %id, "fetching a shared tape");
            #[cfg(not(target_family = "wasm"))]
            {
                let (tx, rx) = std::sync::mpsc::channel();
                crate::platform::off_main_thread("twotop-fetch", move || {
                    let _ = tx.send(fetch_tape(url));
                });
                world.insert_resource(PendingFetch(std::sync::Mutex::new(rx)));
            }
            // The page fetched it already (`web/index.html`).
            #[cfg(target_family = "wasm")]
            let _ = url;
        }
        LaunchLink::Shade(card) => {
            tracing::info!(target: "two_top::launch", name = %card.name, "a shade was sent — stepping onto the table");
            world.resource_mut::<crate::bot::ShadeStyle>().0 = Some(crate::bot::ShadeSpec {
                style: card.style,
                install_id: card.install_id,
                name: card.name,
            });
            world.resource_mut::<crate::bot::PracticeMode>().0 = true;
            world
                .resource_mut::<NextState<AppScreen>>()
                .set(AppScreen::InMatch);
        }
    }
}

/// On the Title: a fetched tape rolls, live or sealed.
fn roll_fetched(world: &mut World) {
    let got = {
        let Some(mut pending) = world.get_resource_mut::<PendingFetch>() else {
            return;
        };
        match pending.0.get_mut().map(|rx| rx.try_recv()) {
            Ok(Ok(got)) => got,
            Ok(Err(std::sync::mpsc::TryRecvError::Empty)) => return,
            _ => Err("the fetch died".into()),
        }
    };
    world.remove_resource::<PendingFetch>();
    match got {
        Ok(Fetched {
            url,
            bytes,
            live: Some(open),
        }) => {
            world.insert_resource(crate::live::LiveFeed::new(url, bytes, open));
        }
        Ok(Fetched { bytes, .. }) => match replay::decode_for_sim_version(&bytes, sim::SIM_VERSION)
        {
            Ok(tape) => {
                crate::theater::start_playback(world, tape);
                crate::verify::note(world, &bytes);
            }
            Err(e) => tracing::warn!(target: "two_top::launch", error = %e, "shared tape rejected"),
        },
        Err(why) => tracing::warn!(target: "two_top::launch", %why, "shared tape unreachable"),
    }
}

/// The link this launch was opened with, once everything it may touch
/// has loaded.
fn dispatch_launch_link(world: &mut World) {
    if let Some(uri) = crate::platform::launch_link() {
        dispatch(world, &uri);
    }
}

/// A link left for the running game, waiting to be acted on.
#[derive(Resource, Default)]
struct QueuedLink(Option<String>);

/// A warm start: claim a link left for the running game when 2-Top
/// regains focus on the Title (`platform::pending_join_link`: the join
/// page's clipboard on Android, a second launch's hand-off on desktop).
fn claim_pending_link(
    mut focus: MessageReader<bevy::window::WindowFocused>,
    screen: Res<State<AppScreen>>,
    mut queued: ResMut<QueuedLink>,
) {
    // Drain every focus message this frame (not `any`, which would stop
    // early and leave the rest to re-trigger next frame).
    let mut gained = false;
    for f in focus.read() {
        gained |= f.focused;
    }
    if gained && *screen.get() == AppScreen::Title {
        queued.0 = crate::platform::pending_join_link();
    }
}

fn dispatch_queued(world: &mut World) {
    if let Some(uri) = world.resource_mut::<QueuedLink>().0.take() {
        dispatch(world, &uri);
    }
}

pub struct LaunchPlugin;

impl Plugin for LaunchPlugin {
    fn build(&self, app: &mut App) {
        // After the persisted picks restore, so the link's table wins the
        // boot it arrived on.
        app.init_resource::<QueuedLink>()
            .add_systems(PostStartup, dispatch_launch_link)
            .add_systems(
                Update,
                (
                    (claim_pending_link, dispatch_queued).chain(),
                    roll_fetched.run_if(in_state(AppScreen::Title)),
                ),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card() -> ShadeCard {
        ShadeCard {
            name: "TAGC".into(),
            install_id: 0x0123_4567_89ab_cdef_0011_2233_4455_6677,
            style: crate::bot::BotStyle {
                commit_frac: 0.55,
                dodge_radius: 180.0,
                wobble: 0.12,
                range: 420.0,
            },
        }
    }

    #[test]
    fn every_link_kind_parses_from_every_place_it_arrives() {
        assert_eq!(
            LaunchLink::parse("twotop://watch/0123456789ab"),
            Some(LaunchLink::Watch {
                id: "0123456789ab".into()
            })
        );
        assert_eq!(
            LaunchLink::parse("https://x.github.io/two-top/#watch=0123456789AB"),
            Some(LaunchLink::Watch {
                id: "0123456789ab".into()
            })
        );
        assert_eq!(LaunchLink::parse("twotop://watch/../../etc"), None);
        let shade = shade_link(&card());
        assert_eq!(LaunchLink::parse(&shade), Some(LaunchLink::Shade(card())));
        let page = format!("https://x/#shade={}", card().encode());
        assert_eq!(LaunchLink::parse(&page), Some(LaunchLink::Shade(card())));
        assert!(matches!(
            LaunchLink::parse("twotop://join/CURS-pit"),
            Some(LaunchLink::Join { .. })
        ));
        assert_eq!(LaunchLink::parse("nonsense"), None);
    }

    #[test]
    fn a_hand_edited_shade_is_clamped_to_a_fair_duelist() {
        let text = format!("TAGC.{:032x}.100.9999.0.1", 7u128);
        let card = ShadeCard::decode(&text).unwrap();
        assert_eq!(card.style.commit_frac, 0.85);
        assert_eq!(card.style.dodge_radius, 300.0);
        assert_eq!(card.style.wobble, 0.06);
        assert_eq!(card.style.range, 340.0);
        assert!(ShadeCard::decode("TAGC.short.1.2.3.4").is_none());
        assert!(ShadeCard::decode(&format!("<script>.{:032x}.1.2.3.4", 7u128)).is_none());
    }
}
