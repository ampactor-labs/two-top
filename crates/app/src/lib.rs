//! 2-Top — shared app entrypoint.
//!
//! `run()` builds the Bevy `App` (DefaultPlugins + GgrsPlugin + sim/render
//! plugins, two synchronized Players in a SyncTestSession). It's called
//! from two places:
//!   * `crates/app/src/main.rs` — desktop binary; calls `app::run()`.
//!   * the `android_main` extern generated below by `#[bevy_main]` —
//!     loaded by the Android NativeActivity at app launch when the crate
//!     is compiled as a `cdylib` and packaged into an APK by cargo-apk.
//!
//! The `#[bevy_main]` macro is a no-op on every non-android target, so
//! the `fn main()` it wraps is just an unreachable private function in
//! desktop builds — the real desktop entry is `src/main.rs`.

use bevy::prelude::*;
use bevy_ggrs::GgrsPlugin;
use fixed_math::Fix;
use input_touch::{CursorPosition, InputTouchPlugin, WindowSize, update_touch_state};
use render::{EffectsPlugin, RenderSyncPlugin};
use sim::{
    AnimState, BOOMERANG_HALF_EXTENT_CM, Boomerang, GgrsCfg, Player, PositionF, SelectedArena,
    SimLifecycleLogPlugin, SimPlugin, VelocityF,
};
use std::collections::HashMap;

mod anchor;
mod arena_select;
mod attest;
mod audio;
mod bot;
mod camera;
mod dark_beyond;
mod debug_overlay;
mod devour;
mod grudge;
mod haptics;
mod hud;
mod intro_card;
mod lobby_overlay;
mod logging;
mod netplay;
mod paths;
mod profile;
mod recorder;
mod room_code;
mod runes;
mod screen;
mod settings;
mod theater;
mod touch_controls;
use anchor::{FullScreenSprite, ScreenAnchorPlugin};
use attest::AttestPlugin;
use audio::GameAudioPlugin;
use bot::BotPlugin;
use camera::CameraFollowPlugin;
use dark_beyond::DarkBeyondPlugin;
use debug_overlay::DebugInputOverlayPlugin;
use devour::DevourPlugin;
use grudge::GrudgePlugin;
use haptics::HapticsPlugin;
use hud::HudPlugin;
use lobby_overlay::LobbyOverlayPlugin;
use netplay::{MatchboxPlugin, NetplayConfig};
use profile::ProfilePlugin;
use recorder::MatchRecorderPlugin;
use room_code::RoomCodePlugin;
use screen::{AppScreen, ScreenPlugin};
use settings::SettingsPlugin;
use theater::TheaterPlugin;
use touch_controls::TouchControlsPlugin;

/// Desktop window size, overridable via `TWOTOP_WINDOW=WxH` (e.g. `540x1200`
/// to preview a tall-phone aspect). Defaults to the 2:3 portrait that frames
/// the 1000×1500 cm arena. Android ignores this entirely.
fn window_resolution_from_env() -> (u32, u32) {
    let parsed = std::env::var("TWOTOP_WINDOW").ok().and_then(|s| {
        let (w, h) = s.split_once(['x', 'X'])?;
        Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
    });
    parsed.unwrap_or((600, 900))
}

pub fn run() {
    // Phase 13: install the tracing subscriber FIRST. The guard owns
    // the non-blocking appender's worker thread in release; binding it
    // to a `let` keeps it alive for the entire `App::run()` scope so
    // pending log writes flush on graceful shutdown.
    let _log_guard = logging::init_logging();
    tracing::info!(
        target: "two_top::app",
        version = env!("CARGO_PKG_VERSION"),
        "two-top starting",
    );

    let netplay = NetplayConfig::from_env_and_args();
    let online = netplay.room_url.is_some();

    let mut app = App::new();
    app
        // Disable bevy's LogPlugin: we own the subscriber installed
        // above. LogPlugin would try to call `set_global_default` again
        // and emit a noisy "global default subscriber set" complaint
        // (and skip its own filter setup). Owning the subscriber lets
        // the file appender + custom filter survive `App::run()`. The
        // window opens portrait (2:3, matching the 1000×1500 cm arena) so
        // the desktop build frames the playfield with no letterboxing;
        // android ignores the resolution and fills the device screen.
        .add_plugins({
            let plugins = DefaultPlugins
                // Chunky pixels stay chunky: linear filtering smears any
                // sprite drawn above its source size (the countdown glyphs
                // upscale ~14x and were visibly soft on device).
                .set(bevy::image::ImagePlugin::default_nearest())
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "2-Top".to_string(),
                        resolution: window_resolution_from_env().into(),
                        ..default()
                    }),
                    ..default()
                })
                .disable::<bevy::log::LogPlugin>();
            // Desktop asset root. Bevy's file asset reader resolves its base
            // from CARGO_MANIFEST_DIR under `cargo run`, which points at
            // `crates/app/assets` — a dir that does not exist. The runtime
            // assets (sprites, arena floors, HUD atlases, the 12 audio cues)
            // live at the workspace root (`assets/`, two levels up), so aim
            // the reader there; the build-time path makes it cwd-independent.
            // Android deliberately keeps the default ("assets"): cargo-apk
            // bundles `../../assets` (Cargo.toml `[package.metadata.android]
            // assets`) as the APK asset root, so the in-code load paths
            // ("audio/throw.wav", "sprites/...") already line up there.
            #[cfg(not(target_os = "android"))]
            let plugins = plugins.set(bevy::asset::AssetPlugin {
                file_path: concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets").to_string(),
                ..default()
            });
            plugins
        })
        .add_plugins(GgrsPlugin::<GgrsCfg>::default())
        .add_plugins(SimPlugin)
        // Arena selection lives in `arena_select`: the persisted pick (or
        // TWOTOP_ARENA for automation) restores at Startup over SimPlugin's
        // Anchor default, and the roster screen owns changes from there.
        // Phase 13: edge-detect MatchState/MatchScore transitions in
        // Update so the diagnostic log captures round flow without
        // duplicating events on each rollback resimulation. Headless
        // ceremonies (sync_test, replay_sync) intentionally don't add
        // this plugin.
        .add_plugins(SimLifecycleLogPlugin)
        // Touch *state* tracking + window/cursor metrics on every platform
        // (harmless on desktop — no touches arrive). The ReadInputs *source*
        // that turns local state into wire-format `PlayerInput` is wired
        // per-platform below; the two must never both be installed or
        // they'd race over `LocalInputs<GgrsCfg>`.
        .add_plugins(InputTouchPlugin)
        .add_plugins(RenderSyncPlugin)
        .add_plugins(EffectsPlugin)
        .add_plugins(GameAudioPlugin)
        .add_plugins(HapticsPlugin)
        .add_plugins(CameraFollowPlugin)
        .add_plugins(ScreenAnchorPlugin)
        .add_plugins(DarkBeyondPlugin)
        .add_plugins(DevourPlugin)
        .add_plugins(GrudgePlugin)
        .add_plugins(AttestPlugin)
        .add_plugins(ProfilePlugin)
        .add_plugins(MatchRecorderPlugin)
        .add_plugins(RoomCodePlugin)
        .add_plugins(TheaterPlugin)
        .add_plugins(arena_select::ArenaSelectPlugin)
        .add_plugins(intro_card::IntroCardPlugin)
        .add_plugins(runes::RunesPlugin)
        .add_plugins(TouchControlsPlugin)
        .add_plugins(BotPlugin)
        .add_plugins(HudPlugin)
        .add_plugins(DebugInputOverlayPlugin)
        .add_plugins(net::NetPlugin)
        .add_plugins(LobbyOverlayPlugin)
        .add_plugins(ScreenPlugin)
        .add_plugins(SettingsPlugin)
        .init_resource::<netplay::LocalPlayerHandle>()
        .init_resource::<netplay::SessionIds>()
        .init_resource::<netplay::RecentAbsence>()
        .init_resource::<render::PerspectiveFlip>()
        // Full-bleed void: whatever the aspect ratio shows beyond the arena
        // island is composed palette darkness, never engine-default gray.
        // Fairness note: the overscan can only ever reveal cosmetic void —
        // every gameplay entity lives inside the AutoMin-guaranteed view.
        .insert_resource(ClearColor(render::palette::VOID))
        .insert_resource(netplay.clone())
        .add_systems(Startup, setup)
        .add_systems(
            PreUpdate,
            (
                update_window_metrics.before(update_touch_state),
                publish_depth_projection.after(update_window_metrics),
            ),
        )
        .add_systems(
            Update,
            (
                ensure_boomerang_visuals,
                grow_boomerang_sprites.after(ensure_boomerang_visuals),
                sync_sprite_atlas_from_anim,
                frame_time_watch,
                log_app_exit,
                update_arena_floor,
                crumble_arena_floor.after(update_arena_floor),
                scale_actors_by_depth,
                blink_spawn_guard,
                animate_ritual_wash,
                update_menu_scrim,
                update_title_diorama,
            ),
        );

    app.insert_state(AppScreen::Title);
    if online {
        app.add_plugins(MatchboxPlugin);
    }

    // Platform input source (level signals only; exactly one source).
    // Android: touch. Everything else (PC): keyboard — WASD for P0, arrows
    // for P1, so two friends play couch versus on one keyboard. After the
    // source (and the bot's patch): the online rematch gate converts a
    // MatchOver THROW press into handshake consent, and — when a tape is
    // loaded — the theater's playback source replaces the whole map.
    #[cfg(target_os = "android")]
    app.add_systems(
        bevy_ggrs::prelude::ReadInputs,
        // The bot patches handle 1 after the touch source's insert applies.
        (read_android_touch_inputs, bot::drive_bot).chain(),
    );
    #[cfg(not(target_os = "android"))]
    {
        app.add_plugins(input_desktop::DesktopInputsPlugin);
        // Mirror-for-flip runs between the keyboard source and the bot: the
        // P1 window of a desktop loopback session needs the same world-Y
        // reflection as the P1 phone, and the bot's world-space inputs must
        // never be mirrored (it patches handle 1 afterwards).
        app.add_systems(
            bevy_ggrs::prelude::ReadInputs,
            (mirror_desktop_inputs_for_flip, bot::drive_bot)
                .chain()
                .after(input_desktop::read_local_desktop_inputs),
        );
        app.add_systems(Update, toggle_fullscreen);
    }
    app.add_systems(
        bevy_ggrs::prelude::ReadInputs,
        (
            screen::inject_ui_throw,
            netplay::gate_rematch_inputs,
            replay::playback_inputs_system.run_if(resource_exists::<replay::ReplayPlayback>),
        )
            .chain()
            .after(bot::drive_bot),
    );

    // Verification capture (opt-in via TWOTOP_CAPTURE): screenshot then exit.
    if let Some(cap) = capture_config_from_env() {
        app.insert_resource(cap).add_systems(Last, capture_frame);
    }

    app.run();

    tracing::info!(target: "two_top::app", "two-top exiting cleanly");
}

#[cfg(target_os = "android")]
fn read_android_touch_inputs(
    mut commands: Commands,
    touch_state: Res<input_touch::TouchState>,
    local_players: Res<bevy_ggrs::LocalPlayers>,
    local_handle: Res<netplay::LocalPlayerHandle>,
    flip: Res<render::PerspectiveFlip>,
) {
    let mut input = input_touch::quantize_inputs(&touch_state);
    // The flipped client (P1's phone) renders the world mirrored top-for-
    // bottom so its player sits at the near edge; its screen drags must be
    // reflected into world space PRE-wire or "down" walks the character up
    // (and aim inverts with it). Both peers still exchange plain world-space
    // inputs, so determinism is untouched.
    if flip.0 < 0.0 {
        input = input_touch::mirror_input_y(input);
    }
    let mut map = bevy::platform::collections::HashMap::default();
    if let Some(handle) = local_handle.0 {
        map.insert(handle, input);
    } else {
        for handle in &local_players.0 {
            map.insert(*handle, input);
        }
    }
    commands.insert_resource(bevy_ggrs::LocalInputs::<GgrsCfg>(map));
}

/// Desktop twin of the flip mirror inside `read_android_touch_inputs`: when
/// this window is the flipped (P1) client of an online session, reflect the
/// keyboard's world-Y before the wire so "up" on this screen moves the
/// character up on this screen. Couch and practice run with flip = 1.0, so
/// this is a no-op everywhere but a desktop loopback P1 window.
#[cfg(not(target_os = "android"))]
fn mirror_desktop_inputs_for_flip(
    flip: Res<render::PerspectiveFlip>,
    inputs: Option<ResMut<bevy_ggrs::LocalInputs<GgrsCfg>>>,
) {
    if flip.0 >= 0.0 {
        return;
    }
    let Some(mut inputs) = inputs else {
        return;
    };
    for input in inputs.0.values_mut() {
        *input = input_touch::mirror_input_y(*input);
    }
}

/// Desktop: toggle borderless fullscreen on F11 — handy for showing a
/// couch match on the big screen. Android manages its own fullscreen.
#[cfg(not(target_os = "android"))]
fn toggle_fullscreen(
    keys: Res<bevy::input::ButtonInput<bevy::input::keyboard::KeyCode>>,
    mut windows: Query<&mut Window>,
) {
    use bevy::input::keyboard::KeyCode;
    use bevy::window::{MonitorSelection, WindowMode};
    if !keys.just_pressed(KeyCode::F11) {
        return;
    }
    if let Ok(mut window) = windows.single_mut() {
        window.mode = match window.mode {
            WindowMode::Windowed => WindowMode::BorderlessFullscreen(MonitorSelection::Current),
            _ => WindowMode::Windowed,
        };
    }
}

/// Phase 13: surface graceful-shutdown events into the diagnostic log.
/// Bevy's `AppExit` event fires when the window closes or
/// `app.send_event(AppExit::Success)` is invoked elsewhere. Reading it
/// here gives the log a clear "shutdown initiated" line right before
/// the `App::run()` loop unwinds — useful for distinguishing a clean
/// exit from a panic-driven termination.
/// Phase 18 Task 5.6 — frame-time instrumentation. Accumulates per-frame
/// `Time<Real>` deltas and emits a periodic `two_top::perf` summary (avg / max
/// / count over the 60 fps budget) so a profiling session surfaces slow
/// windows in the log without per-frame spam. `info!` level so it survives the
/// release `release_max_level_info` filter (the operator's device session runs
/// release). The 5-minute session + on-device 60 fps verification are the
/// operator's batch (M6); this is the lens they read it through.
#[derive(Default)]
struct FrameStats {
    window_start: f32,
    frames: u32,
    over_budget: u32,
    max_ms: f32,
}

fn frame_time_watch(time: Res<Time<Real>>, mut stats: Local<FrameStats>) {
    const BUDGET_MS: f32 = 1000.0 / 60.0;
    const REPORT_SECS: f32 = 5.0;
    let dt_ms = time.delta_secs() * 1000.0;
    stats.frames += 1;
    stats.max_ms = stats.max_ms.max(dt_ms);
    if dt_ms > BUDGET_MS {
        stats.over_budget += 1;
    }
    let now = time.elapsed_secs();
    if stats.window_start == 0.0 {
        stats.window_start = now;
    }
    let elapsed = now - stats.window_start;
    if elapsed >= REPORT_SECS && stats.frames > 0 {
        tracing::info!(
            target: "two_top::perf",
            window_s = elapsed,
            frames = stats.frames,
            avg_ms = elapsed * 1000.0 / stats.frames as f32,
            max_ms = stats.max_ms,
            over_budget = stats.over_budget,
            "frame-time window",
        );
        *stats = FrameStats {
            window_start: now,
            ..Default::default()
        };
    }
}

fn log_app_exit(mut events: MessageReader<AppExit>) {
    for ev in events.read() {
        tracing::info!(
            target: "two_top::app",
            kind = ?ev,
            "AppExit received — shutdown initiated",
        );
    }
}

#[bevy_main]
fn main() {
    run();
}

/// `Startup`: spawn the *persistent* scene — camera, the desktop control
/// legend, and the floor backdrop. Match entities (players, walls, arena
/// props) are spawned per-match by [`screen::spawn_match`] on entering
/// `AppScreen::InMatch`, so the title screen sits over an empty floor.
/// Marker on the persistent arena-floor backdrop so [`update_arena_floor`] can
/// swap its texture when the lobby arena picker changes [`SelectedArena`].
#[derive(Component)]
struct ArenaFloor;

/// Composed floor backdrop per arena (DESIGN_DIRECTION § 4 — one quiet shared
/// composition retinted per arena; props carry the rest of the identity).
fn arena_floor_asset(id: sim::ArenaId) -> &'static str {
    match id {
        sim::ArenaId::Anchor => "arenas/anchor_floor.png",
        sim::ArenaId::Crossing => "arenas/crossing_floor.png",
        sim::ArenaId::Reliquary => "arenas/reliquary_floor.png",
        sim::ArenaId::Pit => "arenas/pit_floor.png",
        sim::ArenaId::Vigil => "arenas/vigil_floor.png",
        sim::ArenaId::Gallery => "arenas/gallery_floor.png",
        sim::ArenaId::Forest => "arenas/forest_floor.png",
    }
}

/// Swap the backdrop texture when the lobby arena picker changes the selection,
/// so the Title screen previews the chosen arena's floor and the match starts on
/// the right one. Render-only; never touches sim.
fn update_arena_floor(
    selected: Res<SelectedArena>,
    asset_server: Res<AssetServer>,
    mut q: Query<&mut Sprite, With<ArenaFloor>>,
) {
    if !selected.is_changed() {
        return;
    }
    let path = arena_floor_asset(selected.0);
    for mut sprite in &mut q {
        sprite.image = asset_server.load(path);
    }
}

/// Publish the perspective-table projection for this frame: the island maps
/// onto the device's fitted view height (minus a small UI reserve), so every
/// pixel row of every phone is table. Uses the AutoMin math at ortho scale 1
/// — the kill-cam zoom then magnifies the projected scene as intended
/// instead of the projection re-fitting the zoomed view.
fn publish_depth_projection(window: Res<WindowSize>) {
    let win = window.0;
    if win.x <= 0.0 || win.y <= 0.0 {
        return;
    }
    const VIEW_MARGIN_CM: f32 = 80.0;
    let min_width = (2 * sim::ARENA_HALF_WIDTH_CM) as f32 + 2.0 * VIEW_MARGIN_CM;
    let min_height =
        (2 * sim::ARENA_HALF_HEIGHT_CM) as f32 * render::WORLD_TILT_Y + 2.0 * VIEW_MARGIN_CM;
    let wpp = (min_width / win.x).max(min_height / win.y);
    // 8% reserve keeps the table clear of the pip row and the bottom hint.
    let span = win.y * wpp * 0.92;
    render::publish_depth_projection(span, render::DEPTH_FOCAL_DEFAULT);
}

/// Squash/stretch multipliers (x, y) for a duelist — the exaggeration layer
/// over the flat atlas poses. Cartoon rules: stretch INTO the motion axis
/// with a counter-squash on the other (roughly area-preserving), a hard
/// overstretch on dash, a coiled-wide anticipation stance while winding up a
/// throw or a taunt, and a flattened pop during hit-stop. `gait` is a free
/// phase for the run bounce. Pure for tests; render-only.
fn actor_pose(
    speed_frac: f32,
    horizontal: bool,
    dashing: bool,
    winding_up: bool,
    stunned: bool,
    gait: f32,
) -> Vec2 {
    if stunned {
        return Vec2::new(1.14, 0.88);
    }
    if dashing {
        return if horizontal {
            Vec2::new(1.22, 0.85)
        } else {
            Vec2::new(0.88, 1.18)
        };
    }
    if winding_up {
        return Vec2::new(1.08, 0.93);
    }
    let run = speed_frac.min(1.0);
    if run > 0.05 {
        // Stretch along the motion + a gait-synced hop that scales with speed.
        let k = 0.09 * run;
        let bounce = 1.0 + 0.05 * run * gait.sin().abs();
        return if horizontal {
            Vec2::new(1.0 + k, (1.0 - 0.6 * k) * bounce)
        } else {
            Vec2::new(1.0 - 0.6 * k, (1.0 + k) * bounce)
        };
    }
    Vec2::ONE
}

/// Scale ground actors by their table depth so the perspective read is
/// carried by the bodies, not just the row spacing: your duelist looms,
/// the far one recedes. Fangs get theirs inside `grow_boomerang_sprites`.
/// The duelists also wear [`actor_pose`]'s squash/stretch on top, so the
/// bodies telegraph what they're doing (run, dash, wind-up, hit-stop) —
/// the exaggeration composes with (never replaces) the depth scale.
#[allow(clippy::type_complexity)]
fn scale_actors_by_depth(
    time: Res<Time<Real>>,
    flip: Res<render::PerspectiveFlip>,
    mut players: Query<
        (
            &Player,
            &PositionF,
            &VelocityF,
            &sim::DashState,
            &sim::ThrowCharge,
            &sim::Taunt,
            &sim::StunFrames,
            &mut Sprite,
        ),
        Without<sim::Pickup>,
    >,
    mut pickups: Query<(&PositionF, &mut Sprite), (With<sim::Pickup>, Without<Player>)>,
) {
    for (player, pos, vel, dash, charge, taunt, stun, mut sprite) in &mut players {
        let (_, y) = pos.0.to_f32();
        let s = render::depth_scale(y * flip.0);
        let (vx, vy) = vel.0.to_f32();
        let speed_frac = (vx.abs().max(vy.abs())) / sim::WALK_SPEED_CM_PER_TICK as f32;
        // Per-player gait phase offset so the two never hop in sync.
        let gait = time.elapsed_secs() * 11.0 + player.handle as f32 * 1.7;
        let pose = actor_pose(
            speed_frac,
            vx.abs() >= vy.abs(),
            matches!(dash, sim::DashState::Dashing { .. }),
            charge.0 > 0 || taunt.0 > 0,
            stun.0 > 0,
            gait,
        );
        sprite.custom_size = Some(Vec2::new(
            render::PLAYER_RENDER_SIZE * s * pose.x,
            render::PLAYER_RENDER_SIZE * s * pose.y,
        ));
    }
    for (pos, mut sprite) in &mut pickups {
        let (_, y) = pos.0.to_f32();
        let s = render::depth_scale(y * flip.0);
        sprite.custom_size = Some(Vec2::splat(render::pickup_render_size() * s));
    }
}

/// Flicker a freshly-respawned duelist while its `SpawnGuard` holds — the
/// arcade "can't touch me yet" read. Both guards are public information
/// (the killer needs to know the camp won't pay), so both players blink.
/// This system owns the player sprite's alpha; nothing else writes it.
fn blink_spawn_guard(
    time: Res<Time<Real>>,
    mut q: Query<(&sim::SpawnGuard, &mut Sprite), With<Player>>,
) {
    for (guard, mut sprite) in &mut q {
        let alpha = if guard.0 > 0 {
            // ~7 Hz square-wave flicker: unmistakably "protected", never
            // strobe-fast (the window is only 0.75 s).
            if (time.elapsed_secs() * 7.0).fract() < 0.5 {
                0.35
            } else {
                0.9
            }
        } else {
            1.0
        };
        sprite.color.set_alpha(alpha);
    }
}

/// Marker for the match-point ritual's full-screen darkness wash.
#[derive(Component)]
struct RitualWash;

/// Marker for the menu scrim (darkens the arena behind title / waiting text).
#[derive(Component)]
struct MenuScrim;

/// Show the menu scrim on every menu screen and while awaiting a peer;
/// hide it the instant a real match is being played so gameplay is never
/// dimmed. The TITLE gets a lighter one: that screen is a diorama of the
/// table, and you have to be able to see the table. The text screens
/// (replays, settings, name entry) keep the full-contrast wash.
fn update_menu_scrim(
    screen: Res<State<AppScreen>>,
    awaiting: Res<screen::AwaitingPeer>,
    mut q: Query<(&mut Visibility, &mut Sprite), With<MenuScrim>>,
) {
    let show = screen.get().is_menu() || awaiting.0;
    let alpha = if *screen.get() == AppScreen::Title {
        0.55
    } else {
        0.85
    };
    for (mut vis, mut sprite) in &mut q {
        *vis = if show {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        sprite.color = render::palette::VOID.with_alpha(alpha);
    }
}

/// The title's table. Rather than describe the game in prose, the title
/// shows it: your demon stands at the near seat with your name over its
/// head, and the far seat is empty — which is exactly what the button
/// underneath offers to fix. Couch fills both seats instead (the second
/// player is sitting next to you).
///
/// Cosmetic sprites, screen-anchored into the diorama band: they are not
/// rollback entities, they never touch sim, and they exist only while the
/// title is up. The arena floor behind them (visible through the title's
/// lighter scrim) is the tabletop.
#[derive(Component, Clone, Copy, PartialEq)]
enum DioramaPart {
    NearDemon,
    FarDemon,
    FarSeat,
}

/// The idle atlas frame, facing the camera — a portrait pose, not the
/// back-of-the-head the near seat wears in a real match.
fn diorama_atlas_index() -> usize {
    let idle = AnimState {
        anim_id: AnimState::IDLE,
        ticks: 0,
    }
    .display_index() as usize;
    (render::FACING_FRONT as usize) * AnimState::TOTAL_ATLAS_FRAMES as usize + idle
}

fn spawn_title_diorama(
    commands: &mut Commands,
    asset_server: &AssetServer,
    atlases: &mut Assets<TextureAtlasLayout>,
) {
    let layout = render::player_atlas_layout(atlases);
    let index = diorama_atlas_index();
    // Near seat: bigger and lower, far seat smaller and higher — the same
    // depth read the perspective table gives in play.
    for (part, sheet, anchor_y, size) in [
        (
            DioramaPart::NearDemon,
            "sprites/players/duelist_a_sheet.png",
            1.0 - 2.0 * 0.42,
            render::PLAYER_RENDER_SIZE * 3.1,
        ),
        (
            DioramaPart::FarDemon,
            "sprites/players/duelist_b_sheet.png",
            1.0 - 2.0 * 0.26,
            render::PLAYER_RENDER_SIZE * 2.1,
        ),
    ] {
        commands.spawn((
            part,
            Sprite {
                image: asset_server.load(sheet),
                texture_atlas: Some(TextureAtlas {
                    layout: layout.clone(),
                    index,
                }),
                custom_size: Some(Vec2::splat(size)),
                ..default()
            },
            anchor::ScreenAnchor::new(0.0, anchor_y, 0.0, 0.0),
            // Above the menu scrim (z=100): the demons stay lit while the
            // floor behind them drops a register.
            Transform::from_xyz(0.0, 0.0, 101.0),
            Visibility::Hidden,
        ));
    }
    // The empty seat: a slow-breathing ring where the challenger will
    // stand. Nobody has to be told what an empty chair means.
    commands.spawn((
        DioramaPart::FarSeat,
        Sprite {
            image: asset_server.load("sprites/fx/charge_ring.png"),
            color: render::palette::COLD_STONE.with_alpha(0.3),
            custom_size: Some(Vec2::splat(render::PLAYER_RENDER_SIZE * 1.9)),
            ..default()
        },
        anchor::ScreenAnchor::new(0.0, 1.0 - 2.0 * 0.26, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, 101.0),
        Visibility::Hidden,
    ));
}

/// Show the right seats for the mode, and keep the near demon breathing —
/// a still portrait reads as a screenshot; a bobbing one reads as someone
/// waiting for you to press the button.
fn update_title_diorama(
    screen: Res<State<AppScreen>>,
    netplay: Res<NetplayConfig>,
    time: Res<Time<Real>>,
    mut q: Query<(
        &DioramaPart,
        &mut Visibility,
        &mut Sprite,
        &mut anchor::ScreenAnchor,
    )>,
) {
    let on_title = *screen.get() == AppScreen::Title;
    let online = netplay.room_url.is_some();
    let t = time.elapsed_secs();
    for (part, mut vis, mut sprite, mut anchor) in &mut q {
        let show = on_title
            && match part {
                DioramaPart::NearDemon => true,
                DioramaPart::FarDemon => !online,
                DioramaPart::FarSeat => online,
            };
        *vis = if show {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        if !show {
            continue;
        }
        match part {
            DioramaPart::NearDemon => anchor.offset.y = 5.0 * (t * 1.6).sin(),
            DioramaPart::FarDemon => anchor.offset.y = 3.0 * (t * 1.6 + 1.1).sin(),
            DioramaPart::FarSeat => {
                let pulse = 0.22 + 0.14 * (t * 1.8).sin().abs();
                sprite.color = render::palette::COLD_STONE.with_alpha(pulse);
                anchor.offset.y = 0.0;
            }
        }
    }
}

/// Ease the ritual wash in/out with `render::MatchPointRitual`. The eased
/// alpha (not a hard cut) makes the room feel like it *dims* — lights going
/// down for the last rite — rather than a state toggle.
fn animate_ritual_wash(
    time: Res<Time<Real>>,
    ritual: Res<render::MatchPointRitual>,
    mut q: Query<(&mut Sprite, &mut Visibility), With<RitualWash>>,
) {
    const WASH_MAX_ALPHA: f32 = 0.38;
    let dt = time.delta_secs();
    for (mut sprite, mut vis) in &mut q {
        let current = sprite.color.alpha();
        let target = if ritual.0 { WASH_MAX_ALPHA } else { 0.0 };
        let next = camera::damped_step(current, target, 3.0, dt);
        sprite.color = render::palette::VOID.with_alpha(next);
        // A fully-faded wash still rasterizes as a full-screen transparent
        // quad — real fill cost on tile GPUs. Cull it outside the ritual.
        *vis = if next < 0.01 {
            Visibility::Hidden
        } else {
            Visibility::Visible
        };
    }
}

/// Render mirror of the sim's SUDDEN-DEATH CRUMBLE: over the round's final
/// seconds the island's safe bounds shrink (`sim::sudden_death_factor`), so
/// the floor art shrinks with them — the world visibly falling away into the
/// void is the wordless "get to the centre" cue. Render-only; the lethal
/// bounds live in `sim::oob_death`.
fn crumble_arena_floor(
    state: Res<sim::MatchState>,
    frame: Res<sim::FrameCount>,
    selected: Res<SelectedArena>,
    mut q: Query<(&FloorStrip, &mut Sprite, &mut Transform), With<ArenaFloor>>,
) {
    // No-storm arenas (Pit / Vigil) never shrink — mirror the sim's rule
    // exactly or the art would lie about the lethal bounds.
    let remaining = match *state {
        sim::MatchState::InRound { expires_at_frame } if selected.0.crumbles() => {
            expires_at_frame.saturating_sub(frame.0)
        }
        _ => u32::MAX,
    };
    // Sudden-death crumble scales the island in WORLD space; the projection
    // then bends the shrunken island like everything else. The island is
    // centre-symmetric, so PerspectiveFlip never changes it.
    let factor = sim::sudden_death_factor(remaining).to_num::<f32>();
    let half_w = sim::ARENA_HALF_WIDTH_CM as f32 * factor;
    let half_h = sim::ARENA_HALF_HEIGHT_CM as f32 * factor;
    for (strip, mut sprite, mut tx) in &mut q {
        // Source row 0 is the art's top = the far (+y) court edge.
        let f0 = strip.0 as f32 / FLOOR_STRIPS as f32;
        let f1 = (strip.0 + 1) as f32 / FLOOR_STRIPS as f32;
        let (y0, y1) = (half_h - 2.0 * half_h * f0, half_h - 2.0 * half_h * f1);
        let e0 = render::tilt_y(y0);
        let e1 = render::tilt_y(y1);
        // A hair of overlap between bands hides projection seams.
        let h = (e0 - e1).abs() + 1.0;
        sprite.custom_size = Some(Vec2::new(half_w * 2.0, h));
        tx.translation.x = 0.0;
        tx.translation.y = (e0 + e1) * 0.5;
    }
}

/// Render mirror of GROW-SLOW: the fang sprite swells with flight progress in
/// lockstep with its lethal rect (`sim::grown_half_extent`), so what you see
/// is what kills. Render-only, f32 (reads sim state, never writes).
#[allow(clippy::type_complexity)]
fn grow_boomerang_sprites(
    flip: Res<render::PerspectiveFlip>,
    mut q: Query<
        (
            &PositionF,
            Option<&sim::ThrowOrigin>,
            Option<&sim::ThrowReach>,
            &mut Sprite,
        ),
        With<Boomerang>,
    >,
) {
    let base_px = ((BOOMERANG_HALF_EXTENT_CM * 2) as f32) * 2.6;
    for (pos, origin, reach, mut sprite) in &mut q {
        let factor = match (origin, reach) {
            (Some(o), Some(r)) if r.0 > Fix::ZERO => {
                let half = sim::grown_half_extent((pos.0 - o.0).length(), r.0);
                half.to_num::<f32>() / BOOMERANG_HALF_EXTENT_CM as f32
            }
            _ => 1.0,
        };
        let (_, y) = pos.0.to_f32();
        let depth = render::depth_scale(y * flip.0);
        sprite.custom_size = Some(Vec2::splat(base_px * factor * depth));
    }
}

/// Verification capture mode. With `TWOTOP_CAPTURE=<path.png>` set, the app
/// renders for a settle window (so assets/atlases/visibility resolve), grabs a
/// PNG of the primary window's framebuffer, then exits — letting a render
/// change be *seen* (or screenshot-diffed) without a human watching the window.
/// Unset → the resource is never inserted and the system never registered, so a
/// normal `cargo run -p app` pays nothing. `TWOTOP_CAPTURE_FRAMES` overrides the
/// settle window (default 90 ≈ 1.5 s at 60 Hz).
///
/// `TWOTOP_CAPTURE_ON=matchover` gates the settle countdown on the sim
/// reaching `MatchState::MatchOver` first — the fps-independent way to grab
/// an endgame screen (the summary card + buttons) under a software renderer,
/// where a fixed render-frame target maps to an unknowable wall-clock time.
#[derive(Resource)]
struct CaptureConfig {
    path: String,
    settle_frames: u32,
    wait_matchover: bool,
}

fn capture_config_from_env() -> Option<CaptureConfig> {
    let path = std::env::var("TWOTOP_CAPTURE")
        .ok()
        .filter(|p| !p.is_empty())?;
    let settle_frames = std::env::var("TWOTOP_CAPTURE_FRAMES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(90);
    let wait_matchover = std::env::var("TWOTOP_CAPTURE_ON").is_ok_and(|v| v == "matchover");
    Some(CaptureConfig {
        path,
        settle_frames,
        wait_matchover,
    })
}

/// `Last`-schedule capture driver: (optionally wait for a gate, then) count
/// settle frames, fire one screenshot at the mark, and quit a few frames
/// later so the async GPU readback + file write have flushed. The
/// `save_to_disk` observer owns the actual encode.
fn capture_frame(
    mut commands: Commands,
    cfg: Res<CaptureConfig>,
    match_state: Res<sim::MatchState>,
    mut frame: Local<u32>,
    mut fired: Local<bool>,
    mut exit: MessageWriter<AppExit>,
) {
    use bevy::render::view::screenshot::{Screenshot, save_to_disk};
    // Hold the settle counter at zero until the gate opens (if any), so
    // `settle_frames` is a delay measured from MatchOver, not app start.
    if cfg.wait_matchover && *frame == 0 && !matches!(*match_state, sim::MatchState::MatchOver) {
        return;
    }
    *frame += 1;
    if !*fired {
        if *frame >= cfg.settle_frames {
            let path = cfg.path.clone();
            tracing::warn!(target: "two_top::capture", path = %path, frame = *frame, "capturing frame");
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk(path));
            *fired = true;
        }
        return;
    }
    if *frame >= cfg.settle_frames + 20 {
        exit.write(AppExit::Success);
    }
}

fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut atlases: ResMut<Assets<TextureAtlasLayout>>,
    selected: Res<SelectedArena>,
) {
    // Camera. Mobile and desktop both frame the whole playable arena with
    // stable AutoMin constraints. That keeps competitive information fair
    // across different phone sizes: a taller/narrower screen may show more
    // void/background, but it never reveals extra playable space.
    // HDR + thresholded bloom give the dark stage its HLD glow: only the
    // brightest accents (eye-slits, boomerang highlights, hit/kill flashes,
    // pickup auras) bloom, while `Tonemapping::None` keeps every other pixel
    // exactly on the locked 16-color palette. `Bloom::OLD_SCHOOL` carries a
    // high threshold so the matte cloaks and floor never wash out.
    #[cfg(target_os = "android")]
    {
        const VIEW_MARGIN_CM: f32 = 80.0;
        let min_width = (2 * sim::ARENA_HALF_WIDTH_CM) as f32 + 2.0 * VIEW_MARGIN_CM;
        let min_height =
            (2 * sim::ARENA_HALF_HEIGHT_CM) as f32 * render::WORLD_TILT_Y + 2.0 * VIEW_MARGIN_CM;
        // Android renders LDR, no bloom, no MSAA — measured on a Galaxy A16
        // (Mali class): the HDR+bloom chain cost ~65 ms/frame and default
        // 4× MSAA + spare full-screen quads another ~17 ms, pinning the
        // phone at 10 fps. Without them it locks to 60 (16.7 ms avg).
        // Overdriven accent colors clamp to white instead of blooming —
        // an acceptable trade on the product platform; desktop keeps the
        // full HLD glow. MSAA buys nothing for a quad-sprite game anyway.
        commands.spawn((
            Camera2d,
            Msaa::Off,
            bevy::core_pipeline::tonemapping::Tonemapping::None,
            Projection::from(OrthographicProjection {
                scaling_mode: bevy::camera::ScalingMode::AutoMin {
                    min_width,
                    min_height,
                },
                ..OrthographicProjection::default_2d()
            }),
        ));
    }
    #[cfg(not(target_os = "android"))]
    {
        const VIEW_MARGIN_CM: f32 = 80.0;
        let min_width = (2 * sim::ARENA_HALF_WIDTH_CM) as f32 + 2.0 * VIEW_MARGIN_CM;
        // The arena renders Y-foreshortened, so frame the foreshortened height.
        let min_height =
            (2 * sim::ARENA_HALF_HEIGHT_CM) as f32 * render::WORLD_TILT_Y + 2.0 * VIEW_MARGIN_CM;
        commands.spawn((
            Camera2d,
            // MSAA does nothing for quad sprites; skip its fill cost.
            Msaa::Off,
            bevy::render::view::Hdr,
            bevy::core_pipeline::tonemapping::Tonemapping::None,
            bevy::post_process::bloom::Bloom::OLD_SCHOOL,
            Projection::from(OrthographicProjection {
                scaling_mode: bevy::camera::ScalingMode::AutoMin {
                    min_width,
                    min_height,
                },
                ..OrthographicProjection::default_2d()
            }),
        ));
        // Couch-play control legend (desktop only — touch needs none).
        // World-space Text2d (the app has no bevy_ui), pinned to the bottom
        // of the *screen* so it stays clear of the top-edge HUD (pips +
        // round clock) on any window aspect. Bounded so a narrow window
        // wraps the long lines instead of clipping them off both edges.
        commands.spawn((
            Text2d::new(
                "P0: WASD  -  Space throw  -  LShift dash  -  T taunt\n\
                 P1: Arrows  -  RShift throw  -  RCtrl dash  -  Enter taunt\n\
                 or controllers (build --features gamepad)  -  F11 fullscreen",
            ),
            TextFont {
                font_size: 26.0,
                ..default()
            },
            TextColor(render::palette::BONE.with_alpha(0.55)),
            bevy::text::TextBounds::new_horizontal(1080.0),
            anchor::ScreenAnchor::new(0.0, -1.0, 0.0, 90.0),
            Transform::from_xyz(0.0, -(sim::ARENA_HALF_HEIGHT_CM as f32) + 44.0, 100.0),
        ));
    }

    // Two vignettes (every platform):
    //   * island vignette — world-fixed, sized to the AutoMin min view, so
    //     the lit island keeps its dark dithered rim (the pre-overscan look).
    //   * screen vignette — stretched to the live view rect each frame, so
    //     the tall-phone overscan reads as composed darkness closing in on
    //     the island instead of dead margin.
    // Together: island rimmed in dark, then the dark beyond out to the true
    // screen edges (HLD cohesion). Above gameplay, below the HUD + kill flash.
    // One vignette only: screen-fitted. (A second, world-fixed "island rim"
    // vignette used to live here sized to the classic 2:3 island; under the
    // perspective table the island outgrew it and its dithered ring landed
    // mid-table over prop bases — the "floor drawn over the walls" bug.)
    let vig_img = asset_server.load("sprites/fx/vignette.png");
    let mut vig_color = Color::WHITE;
    vig_color.set_alpha(0.7);
    commands.spawn((
        Sprite {
            image: vig_img,
            color: vig_color,
            ..default()
        },
        FullScreenSprite { cover: 1.02 },
        Transform::from_xyz(0.0, 0.0, 45.0),
    ));
    // Match-point ritual wash: a full-screen darkness that eases in when
    // both duelists are one kill from victory (render::MatchPointRitual) —
    // the palette drops a register for the final ceremony. Above gameplay
    // and vignettes, below HUD/kill flash.
    commands.spawn((
        RitualWash,
        Sprite {
            color: render::palette::VOID.with_alpha(0.0),
            ..default()
        },
        FullScreenSprite { cover: 1.02 },
        Transform::from_xyz(0.0, 0.0, 46.0),
    ));
    // Menu scrim: a dark wash over the busy arena while the title menu or the
    // "awaiting a challenger" room is up, so the text reads with real
    // contrast instead of fighting the dithered floor and the center sigil.
    // Above the arena + HUD, below the menu text (z=200). 0.85 — 0.72 still
    // let the center sigil ghost through behind the title copy.
    commands.spawn((
        MenuScrim,
        Sprite {
            color: render::palette::VOID.with_alpha(0.85),
            ..default()
        },
        FullScreenSprite { cover: 1.02 },
        Transform::from_xyz(0.0, 0.0, 100.0),
    ));

    // Arena backdrop: the composed moody Bone-Cathedral floor (320x480 px
    // source) for the selected arena, sized to EXACTLY the safe playfield
    // (2×ARENA_HALF = 1000×1500 cm) so the floor's lit ledge lip lands on the
    // out-of-bounds death line — step off the lit island and you're over the
    // void (the Boomerang-Fu open-field read). On the static desktop cam the
    // void rings the island; the mobile follow-cam stays inside it. Z below
    // players, stains, and effects so everything sits ON the floor. Tagged
    // `ArenaFloor` so the lobby arena picker can retexture it live.
    // The floor renders as horizontal STRIPS so it can bend through the
    // perspective table: each strip samples its band of the same floor
    // texture (Sprite::rect) and is positioned/sized every frame by
    // `crumble_arena_floor` through the live projection — near bands tall,
    // far bands squeezed, exactly like the actors standing on them.
    // The title's table (shown only on the title; see `update_title_diorama`).
    spawn_title_diorama(&mut commands, &asset_server, &mut atlases);

    let floor_img = asset_server.load(arena_floor_asset(selected.0));
    for i in 0..FLOOR_STRIPS {
        let v0 = FLOOR_SRC_H * i as f32 / FLOOR_STRIPS as f32;
        let v1 = FLOOR_SRC_H * (i + 1) as f32 / FLOOR_STRIPS as f32;
        commands.spawn((
            ArenaFloor,
            FloorStrip(i),
            Sprite {
                image: floor_img.clone(),
                rect: Some(Rect::new(0.0, v0, FLOOR_SRC_W, v1)),
                custom_size: Some(Vec2::ZERO),
                ..default()
            },
            Transform::from_xyz(0.0, 0.0, -1.0),
        ));
    }
}

/// Horizontal floor bands (source texture rows per strip: 480 / 20 = 24 px).
const FLOOR_STRIPS: u32 = 20;
/// The composed floor source art is 320×480.
const FLOOR_SRC_W: f32 = 320.0;
const FLOOR_SRC_H: f32 = 480.0;

/// Which band of the floor a strip draws (0 = the TOP source row = the far
/// court's +y edge in world space; source v grows downward while world y
/// grows upward).
#[derive(Component)]
struct FloorStrip(u32);

/// `Update`-schedule system: attaches a placeholder sprite + transform
/// to any `Boomerang` entity that doesn't already have a `Sprite`. The
/// initial `Transform` is seeded from the boomerang's `PositionF` so
/// the first rendered frame appears at the spawn position rather than
/// blinking through origin before `render::sync_transforms_from_sim`
/// catches up. Subsequent frames are driven by the render-side
/// interpolator.
///
/// Z-order: 0.5 — above the arena walls (z=-1.0) and above the players
/// (z=0.0 by default), so a recalled boomerang reads cleanly when it
/// passes over the player on its way home.
///
/// Lives in the `app` crate (not `render`) because `render` is built
/// with a minimal Bevy feature set (no `bevy_sprite`); only `app`
/// pulls in `DefaultPlugins`.
type NewBoomerangs<'w, 's> =
    Query<'w, 's, (Entity, &'static PositionF), (With<Boomerang>, Without<Sprite>)>;

fn ensure_boomerang_visuals(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    flip: Res<render::PerspectiveFlip>,
    q: NewBoomerangs,
) {
    // Phase 15: render the bone fang as the polished 12x12 sprite,
    // upscaled 2x to match the placeholder's original 20-px footprint
    // (BOOMERANG_HALF_EXTENT_CM * 2 = 20). Marked variants will land
    // when per-round blood accumulation is wired in cycle 3.
    // Larger weapon sprite (2.6× the 20 cm fang) — the fang reads as a bigger,
    // more present threat per the portrait-fighter scale-up.
    let size_px = ((BOOMERANG_HALF_EXTENT_CM * 2) as f32) * 2.6;
    let image = asset_server.load("sprites/projectiles/bone_fang.png");
    let shadow_img = asset_server.load("sprites/fx/shadow_blob.png");
    for (entity, pos) in &q {
        let (x, y) = pos.0.to_f32();
        let ty = render::tilt_y(y * flip.0);
        commands.entity(entity).insert((
            Sprite {
                image: image.clone(),
                custom_size: Some(Vec2::new(size_px, size_px)),
                ..default()
            },
            Transform::from_xyz(x, ty, 0.5),
        ));
        // A ground shadow so the fang reads as flying *over* the floor; it
        // self-cleans when the boomerang despawns (render::sync_ground_shadows).
        render::spawn_ground_shadow(
            &mut commands,
            shadow_img.clone(),
            entity,
            0.0,
            size_px * 0.7,
            Vec2::new(x, ty),
        );
    }
}

/// Phase 15 cycle 1: drive each player's TextureAtlas index from the
/// Selects the atlas frame from the rolled-back AnimState AND the facing
/// direction (side/back/front row) from the player's velocity. The 3-row
/// atlas layout: row 0 = side, row 1 = back (away), row 2 = front (toward).
///
/// Direction logic: if |vy| > |vx| the character is moving vertically —
/// positive Y = back (walking away from camera), negative Y = front (toward).
/// Otherwise side-facing with flip_x for left/right. Idle defaults: P0
/// (near/bottom) shows back (facing away toward the opponent), P1 (far/top)
/// shows front (facing toward the camera/opponent).
fn sync_sprite_atlas_from_anim(
    mut q: Query<(&Player, &AnimState, &VelocityF, &mut Sprite)>,
    persp: Res<render::PerspectiveFlip>,
    victory: Res<screen::VictoryPose>,
    mut facing: Local<HashMap<usize, (bool, u16)>>,
) {
    let deadzone = Fix::const_from_int(3);
    let frames_per_row = AnimState::TOTAL_ATLAS_FRAMES as usize;
    let flipped = persp.0 < 0.0;
    let (away, toward) = if flipped {
        (render::FACING_FRONT, render::FACING_BACK)
    } else {
        (render::FACING_BACK, render::FACING_FRONT)
    };
    // The decided match's winner stands as a statue: pinned on the CHARGE
    // pose, facing the camera, through the whole summary. Render-only.
    let victory_frame = AnimState {
        anim_id: AnimState::CHARGE,
        ticks: 0,
    }
    .display_index();
    for (player, anim, vel, mut sprite) in &mut q {
        if victory.0 == Some(player.handle) {
            if let Some(atlas) = sprite.texture_atlas.as_mut() {
                atlas.index =
                    (render::FACING_FRONT as usize) * frames_per_row + victory_frame as usize;
            }
            sprite.flip_x = false;
            continue;
        }
        let (flip, dir) = facing.entry(player.handle).or_insert_with(|| {
            let default_dir = if (player.handle == 0) ^ flipped {
                render::FACING_BACK
            } else {
                render::FACING_FRONT
            };
            (false, default_dir)
        });

        let ax = vel.0.x.abs();
        let ay = vel.0.y.abs();
        if ax > deadzone || ay > deadzone {
            if ay > ax {
                if vel.0.y > Fix::ZERO {
                    *dir = away;
                } else {
                    *dir = toward;
                }
                *flip = false;
            } else {
                *dir = render::FACING_SIDE;
                if vel.0.x > deadzone {
                    *flip = false;
                } else if vel.0.x < -deadzone {
                    *flip = true;
                }
            }
        }

        if let Some(atlas) = sprite.texture_atlas.as_mut() {
            atlas.index = (*dir as usize) * frames_per_row + anim.display_index() as usize;
        }
        sprite.flip_x = *flip;
    }
}

/// Mirrors the primary `Window`'s logical size and cursor position into
/// the resources `input_touch` reads. Lives in the app crate (not
/// `input_touch`) because only the app pulls in `bevy_window` —
/// `input_touch` stays headless-friendly for tests and CI.
/// Ordered before `input_touch::update_touch_state` so the same-frame
/// touch sync sees fresh metrics.
fn update_window_metrics(
    window: Query<&Window>,
    mut window_size: ResMut<WindowSize>,
    mut cursor_pos: ResMut<CursorPosition>,
) {
    let Ok(w) = window.single() else { return };
    window_size.0 = Vec2::new(w.width(), w.height());
    if let Some(pos) = w.cursor_position() {
        cursor_pos.0 = pos;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actor_pose_idle_is_neutral() {
        assert_eq!(actor_pose(0.0, true, false, false, false, 0.0), Vec2::ONE);
    }

    #[test]
    fn actor_pose_stretches_into_the_motion_axis() {
        let h = actor_pose(1.0, true, false, false, false, 0.0);
        assert!(h.x > 1.0 && h.y < 1.0, "horizontal run stretches x: {h:?}");
        let v = actor_pose(1.0, false, false, false, false, 0.0);
        assert!(v.y > 1.0 && v.x < 1.0, "vertical run stretches y: {v:?}");
    }

    #[test]
    fn actor_pose_states_rank_by_intensity() {
        let dash = actor_pose(2.0, true, true, false, false, 0.0);
        let run = actor_pose(1.0, true, false, false, false, 0.0);
        assert!(dash.x > run.x, "dash overstretches past a full run");
        // Wind-up coils wide; hit-stop pops flat; both beat the run states.
        let coil = actor_pose(0.0, true, false, true, false, 0.0);
        assert!(coil.x > 1.0 && coil.y < 1.0);
        let pop = actor_pose(0.0, true, false, false, true, 0.0);
        assert!(pop.x > 1.1 && pop.y < 0.9);
    }
}
