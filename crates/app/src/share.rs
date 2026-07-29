//! SHARE — a decided match becomes a link becomes a QR (NORTH N3).
//!
//! The tape is the broadcast: POST the `.bmrg` to the tape drop
//! (`crates/tape_drop`), compose `<watch-url>#watch=<id>`, and put that
//! link on screen as a QR the other person's camera can eat. The share
//! surfaces are the match summary (the tape the recorder just froze,
//! `LastSavedReplay`) and the theater (the tape currently playing,
//! [`SharedTapePath`]). Everything here is one self-contained module: its
//! own labels, its own tap bands, its own overlay — no surgery on the
//! summary card or the theater marquee.
//!
//! Native-only posting: the browser build is the *destination* of these
//! links, and `ureq` has no wasm analogue (a gloo-fetch share-from-web is
//! a follow-up nobody has asked for yet). Config rides the same
//! runtime-env-then-compile-time-bake convention as the ICE vendor:
//! `TWOTOP_DROP_URL` (the tape_drop service) and `TWOTOP_WATCH_URL` (the
//! static web theater). Both unset ⇒ the share surfaces never appear and
//! nothing else changes.

use bevy::prelude::*;
use input_touch::WindowSize;
use sim::MatchState;
use std::path::PathBuf;

use crate::anchor::ScreenAnchor;
use crate::screen::AppScreen;

/// Where the SHARE label sits on the summary (window-fraction, y-down),
/// and the tap band around it. Low corner, clear of the summary card and
/// the primary/quit strips.
const SUMMARY_ANCHOR: (f32, f32) = (0.80, 0.16);
const SUMMARY_BAND: ((f32, f32), (f32, f32)) = ((0.62, 0.98), (0.10, 0.22));
/// Theater placement: top-right, under the marquee line.
const THEATER_ANCHOR: (f32, f32) = (0.82, 0.12);
const THEATER_BAND: ((f32, f32), (f32, f32)) = ((0.66, 0.98), (0.06, 0.18));

/// QR module scale (screen pixels per module) is left to the sprite
/// transform; this is the quiet zone in modules, per the QR spec minimum.
const QUIET_ZONE: u32 = 4;

/// Share endpoints, resolved once at startup. `None` anywhere ⇒ that half
/// of the flow is off (no label, no band).
#[derive(Resource, Default, Clone, Debug)]
pub struct ShareConfig {
    pub drop_url: Option<String>,
    pub watch_url: Option<String>,
}

impl ShareConfig {
    fn from_env() -> Self {
        let get = |run: &str, bake: Option<&'static str>| {
            std::env::var(run)
                .ok()
                .filter(|s| !s.is_empty())
                .or_else(|| bake.filter(|s| !s.is_empty()).map(str::to_string))
        };
        Self {
            drop_url: get("TWOTOP_DROP_URL", option_env!("TWOTOP_DROP_URL")),
            watch_url: get("TWOTOP_WATCH_URL", option_env!("TWOTOP_WATCH_URL")),
        }
    }

    fn complete(&self) -> bool {
        self.drop_url.is_some() && self.watch_url.is_some()
    }
}

/// The tape the theater currently has loaded — set when a row starts
/// playing, cleared at theater teardown. The summary share uses
/// `LastSavedReplay` instead; the two never overlap (the recorder stands
/// down while a tape plays).
#[derive(Resource, Default)]
pub struct SharedTapePath(pub Option<PathBuf>);

/// The share flow's state. One in flight at a time; a new request while
/// posting is ignored (the button disappears anyway).
#[derive(Resource, Default)]
enum ShareState {
    #[default]
    Idle,
    /// Native-only by construction: the wasm arm never posts, so the
    /// variant is dead there and the lint knows it.
    #[cfg_attr(target_family = "wasm", allow(dead_code))]
    Posting {
        rx: std::sync::Mutex<std::sync::mpsc::Receiver<Result<String, String>>>,
    },
    /// The overlay is up.
    Showing,
}

/// Marker components for the module's own UI entities.
#[derive(Component)]
struct ShareLabel {
    /// Which surface this label serves.
    on_summary: bool,
}

#[derive(Component)]
struct ShareOverlayPiece;

/// `<watch-url>#watch=<id>`, tolerant of a trailing slash on the bake.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn compose_watch_url(watch_base: &str, id: &str) -> String {
    format!("{}#watch={id}", watch_base.trim_end_matches('/'))
}

/// Render a QR of `text` into a bevy `Image`: dark modules in Void on a
/// Hot Bone card, [`QUIET_ZONE`] modules of margin. The global
/// nearest-neighbor sampler keeps it crisp at any sprite scale.
pub(crate) fn qr_image(text: &str) -> Option<(Image, u32)> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let w = code.width() as u32;
    let modules = code.to_colors(); // row-major, w * w
    let side = w + QUIET_ZONE * 2;
    let dark = render::palette::VOID.to_srgba().to_u8_array();
    let light = render::palette::HOT_BONE.to_srgba().to_u8_array();
    let mut data = Vec::with_capacity((side * side * 4) as usize);
    for y in 0..side {
        for x in 0..side {
            let inside =
                x >= QUIET_ZONE && x < w + QUIET_ZONE && y >= QUIET_ZONE && y < w + QUIET_ZONE;
            let is_dark = inside
                && modules[((y - QUIET_ZONE) * w + (x - QUIET_ZONE)) as usize]
                    == qrcode::Color::Dark;
            data.extend_from_slice(if is_dark { &dark } else { &light });
        }
    }
    let image = Image::new(
        bevy::render::render_resource::Extent3d {
            width: side,
            height: side,
            depth_or_array_layers: 1,
        },
        bevy::render::render_resource::TextureDimension::D2,
        data,
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        bevy::asset::RenderAssetUsages::default(),
    );
    Some((image, side))
}

/// The drop's receipt for a stored tape.
#[cfg(not(target_family = "wasm"))]
#[derive(serde::Deserialize)]
struct DropReceipt {
    id: String,
}

/// Post `bytes` to the drop and compose the watch link. Runs on the IO
/// task pool; reports through the channel.
#[cfg(not(target_family = "wasm"))]
fn post_tape(drop_url: String, watch_url: String, bytes: Vec<u8>) -> Result<String, String> {
    let receipt: DropReceipt = ureq::post(&format!("{}/tape", drop_url.trim_end_matches('/')))
        .timeout(std::time::Duration::from_secs(5))
        .send_bytes(&bytes)
        .map_err(|e| format!("tape drop unreachable: {e}"))?
        .into_json()
        .map_err(|e| format!("tape drop answered strangely: {e}"))?;
    Ok(compose_watch_url(&watch_url, &receipt.id))
}

fn spawn_share_labels(mut commands: Commands, config: Res<ShareConfig>) {
    if !config.complete() {
        return;
    }
    for (on_summary, (fx, fy)) in [(true, SUMMARY_ANCHOR), (false, THEATER_ANCHOR)] {
        commands.spawn((
            ShareLabel { on_summary },
            Text2d::new("SHARE"),
            TextFont {
                font_size: 30.0,
                ..default()
            },
            TextColor(render::palette::SPARK),
            TextLayout::new_with_justify(Justify::Center),
            ScreenAnchor::new(fx * 2.0 - 1.0, 1.0 - 2.0 * fy, 0.0, 0.0),
            Transform::from_xyz(0.0, 0.0, 205.0),
            Visibility::Hidden,
        ));
    }
}

/// Which surface (if either) can share right now, and the tape it would
/// share. Pure, so the band logic and the label logic cannot drift apart.
fn shareable(
    screen: AppScreen,
    match_over: bool,
    theater_active: bool,
    saved: &Option<PathBuf>,
    theater_tape: &Option<PathBuf>,
) -> Option<(bool, PathBuf)> {
    if theater_active {
        return theater_tape.clone().map(|p| (false, p));
    }
    if screen == AppScreen::InMatch && match_over {
        return saved.clone().map(|p| (true, p));
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn drive_share(
    mut commands: Commands,
    config: Res<ShareConfig>,
    screen: Res<State<AppScreen>>,
    match_state: Res<MatchState>,
    theater: Res<crate::theater::TheaterMode>,
    saved: Res<crate::recorder::LastSavedReplay>,
    theater_tape: Res<SharedTapePath>,
    keys: Res<ButtonInput<KeyCode>>,
    touches: Res<Touches>,
    window: Res<WindowSize>,
    mut images: ResMut<Assets<Image>>,
    mut state: ResMut<ShareState>,
    mut labels: Query<(&ShareLabel, &mut Visibility)>,
    overlay: Query<Entity, With<ShareOverlayPiece>>,
) {
    if !config.complete() {
        return;
    }
    let can_share = shareable(
        *screen.get(),
        matches!(*match_state, MatchState::MatchOver),
        theater.active(),
        &saved.0,
        &theater_tape.0,
    );

    // Labels track the shareable surface; both hide while the flow is busy.
    let busy = !matches!(*state, ShareState::Idle);
    for (label, mut vis) in &mut labels {
        let on = !busy
            && can_share
                .as_ref()
                .is_some_and(|(on_summary, _)| *on_summary == label.on_summary);
        *vis = if on {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }

    let win = window.0;
    let tapped_at = |band: ((f32, f32), (f32, f32))| {
        win.x > 0.0
            && win.y > 0.0
            && touches.iter_just_pressed().any(|t| {
                let (fx, fy) = (t.position().x / win.x, t.position().y / win.y);
                (band.0.0..band.0.1).contains(&fx) && (band.1.0..band.1.1).contains(&fy)
            })
    };

    match &mut *state {
        ShareState::Idle => {
            let Some((on_summary, path)) = can_share else {
                return;
            };
            let band = if on_summary {
                SUMMARY_BAND
            } else {
                THEATER_BAND
            };
            if !(keys.just_pressed(KeyCode::KeyS) || tapped_at(band)) {
                return;
            }
            #[cfg(target_family = "wasm")]
            {
                let _ = path;
                tracing::warn!(
                    target: "two_top::share",
                    "sharing from the browser is a follow-up — the web build is the destination",
                );
            }
            #[cfg(not(target_family = "wasm"))]
            {
                let Ok(bytes) = std::fs::read(&path) else {
                    tracing::warn!(target: "two_top::share", path = %path.display(), "tape unreadable — not shared");
                    return;
                };
                let (drop_url, watch_url) = (
                    config.drop_url.clone().expect("checked complete"),
                    config.watch_url.clone().expect("checked complete"),
                );
                let (tx, rx) = std::sync::mpsc::channel();
                bevy::tasks::IoTaskPool::get()
                    .spawn(async move {
                        let _ = tx.send(post_tape(drop_url, watch_url, bytes));
                    })
                    .detach();
                *state = ShareState::Posting {
                    rx: std::sync::Mutex::new(rx),
                };
                tracing::info!(target: "two_top::share", "tape posting to the drop");
            }
        }
        ShareState::Posting { rx } => {
            let outcome = rx
                .get_mut()
                .expect("share task never panics holding it")
                .try_recv();
            match outcome {
                Ok(Ok(url)) => {
                    spawn_overlay(&mut commands, &mut images, &url);
                    *state = ShareState::Showing;
                }
                Ok(Err(why)) => {
                    tracing::warn!(target: "two_top::share", %why, "share failed");
                    *state = ShareState::Idle;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    *state = ShareState::Idle;
                }
            }
        }
        ShareState::Showing => {
            // Any tap or key closes; leaving the shareable surface closes.
            let any_tap = touches.iter_just_pressed().next().is_some();
            if any_tap
                || keys.just_pressed(KeyCode::KeyS)
                || keys.just_pressed(KeyCode::Escape)
                || can_share.is_none()
            {
                for e in &overlay {
                    commands.entity(e).despawn();
                }
                *state = ShareState::Idle;
            }
        }
    }
}

/// The QR card: code sprite center-screen, the link under it, the close
/// hint under that. All screen-anchored, z above every other overlay.
fn spawn_overlay(commands: &mut Commands, images: &mut Assets<Image>, url: &str) {
    let Some((image, side)) = qr_image(url) else {
        tracing::warn!(target: "two_top::share", "QR render failed — link is still in the log");
        tracing::info!(target: "two_top::share", %url, "watch link");
        return;
    };
    let handle = images.add(image);
    // custom_size, never Transform::scale — the screen-anchor system owns
    // anchored scale (see room_code's join QR for the incident report).
    commands.spawn((
        ShareOverlayPiece,
        Sprite {
            image: handle,
            custom_size: Some(Vec2::splat(190.0)),
            ..default()
        },
        ScreenAnchor::new(0.0, 0.12, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, 210.0),
    ));
    let _ = side;
    commands.spawn((
        ShareOverlayPiece,
        Text2d::new(url.to_string()),
        TextFont {
            font_size: 22.0,
            ..default()
        },
        TextColor(render::palette::BONE),
        TextLayout::new_with_justify(Justify::Center),
        ScreenAnchor::new(0.0, -0.62, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, 210.0),
    ));
    commands.spawn((
        ShareOverlayPiece,
        Text2d::new("TAP TO CLOSE"),
        TextFont {
            font_size: 20.0,
            ..default()
        },
        TextColor(render::palette::COLD_STONE),
        TextLayout::new_with_justify(Justify::Center),
        ScreenAnchor::new(0.0, -0.76, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, 210.0),
    ));
    tracing::info!(target: "two_top::share", %url, "watch link on screen");
}

pub struct SharePlugin;

impl Plugin for SharePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ShareConfig::from_env())
            .init_resource::<ShareState>()
            .init_resource::<SharedTapePath>()
            .add_systems(Startup, spawn_share_labels)
            .add_systems(Update, drive_share);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_urls_compose_with_and_without_trailing_slash() {
        assert_eq!(
            compose_watch_url("https://x.test/watch/", "1a7eaaef45d9"),
            "https://x.test/watch#watch=1a7eaaef45d9"
        );
        assert_eq!(
            compose_watch_url("https://x.test/watch", "abc"),
            "https://x.test/watch#watch=abc"
        );
    }

    #[test]
    fn qr_images_carry_the_finder_pattern_and_quiet_zone() {
        let (image, side) = qr_image("https://x.test/watch#watch=1a7eaaef45d9").unwrap();
        let data = image.data.as_ref().expect("cpu-side image data");
        assert_eq!(data.len() as u32, side * side * 4);
        let px = |x: u32, y: u32| {
            let i = ((y * side + x) * 4) as usize;
            (data[i], data[i + 1], data[i + 2])
        };
        let light = render::palette::HOT_BONE.to_srgba().to_u8_array();
        let dark = render::palette::VOID.to_srgba().to_u8_array();
        // The quiet zone is light all the way around.
        assert_eq!(px(0, 0), (light[0], light[1], light[2]));
        assert_eq!(px(side - 1, side - 1), (light[0], light[1], light[2]));
        // The finder pattern's top-left corner module is dark, just inside
        // the quiet zone.
        assert_eq!(
            px(QUIET_ZONE, QUIET_ZONE),
            (dark[0], dark[1], dark[2]),
            "finder pattern anchors the corner"
        );
    }

    #[test]
    fn shareable_picks_the_right_surface() {
        use AppScreen::*;
        let tape = Some(PathBuf::from("a.bmrg"));
        // Theater wins while active, whatever the screen says.
        assert_eq!(
            shareable(InMatch, true, true, &None, &tape),
            Some((false, PathBuf::from("a.bmrg")))
        );
        // Summary shares the recorder's tape only at MatchOver.
        assert_eq!(
            shareable(InMatch, true, false, &tape, &None),
            Some((true, PathBuf::from("a.bmrg")))
        );
        assert_eq!(shareable(InMatch, false, false, &tape, &None), None);
        assert_eq!(shareable(Title, false, false, &tape, &None), None);
        // No tape, no share.
        assert_eq!(shareable(InMatch, true, false, &None, &None), None);
    }
}
