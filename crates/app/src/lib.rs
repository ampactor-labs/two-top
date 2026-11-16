//! 2-Top — shared app entrypoint.
//!
//! `run()` builds the Bevy `App` (DefaultPlugins + GgrsPlugin + sim/render
//! plugins, two synchronized Players in a SyncTestSession). It's called
//! from two places:
//!   * `crates/app/src/main.rs` — desktop binary; calls `app::run()`.
//!   * the `android_main` export at the bottom of this file — looked up by
//!     the Android NativeActivity glue at app launch when the crate is
//!     compiled as a `cdylib` and packaged into an APK by cargo-apk. It
//!     hands straight to `platform::android::main`, which owns the
//!     process's lifetime (what `#[bevy_main]` generated here did not).
//!   * `platform::web::web_start` — the browser entry.

use bevy::prelude::*;
use bevy_ggrs::GgrsPlugin;
use fixed_math::Fix;
use input_touch::{InputTouchPlugin, WindowSize, update_touch_state};
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
mod callout;
mod camera;
mod capture;
mod config;
mod crash_report;
mod dark_beyond;
mod debug_overlay;
mod devour;
mod first_blood;
mod grudge;
mod haptics;
mod hud;
mod intro_card;
mod keys;
mod lesson;
mod lifecycle;
mod lobby_overlay;
mod logging;
mod netplay;
mod nudges;
mod paths;
mod platform;
mod pointer;
mod presentation;
mod profile;
mod recorder;
mod rivals;
mod room_code;
mod runes;
mod screen;
mod settings;
mod shade;
mod share;
mod state_log;
mod theater;
mod touch_controls;
mod tuning_reload;
mod ui;
use anchor::{FullScreenSprite, ScreenAnchorPlugin};
use attest::AttestPlugin;
use audio::GameAudioPlugin;
use bot::BotPlugin;
use camera::CameraFollowPlugin;
use capture::*;
use dark_beyond::DarkBeyondPlugin;
use debug_overlay::DebugInputOverlayPlugin;
use devour::DevourPlugin;
use grudge::GrudgePlugin;
use haptics::HapticsPlugin;
use hud::HudPlugin;
use keys::*;
use lobby_overlay::LobbyOverlayPlugin;
use netplay::{MatchboxPlugin, NetplayConfig};
use presentation::*;
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
    let parsed = crate::config::Knob::Window.value().and_then(|s| {
        let (w, h) = s.split_once(['x', 'X'])?;
        Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
    });
    parsed.unwrap_or((600, 900))
}

/// The local-input pipeline in `ReadInputs`. Every source (keyboard, touch,
/// the bot) writes this device's input first; then the pre-wire shaping
/// (the summary's rematch hold, the PLAY AGAIN tap, the online consent
/// gate, tape playback) edits what the sources wrote. A source added
/// outside `Sources`, or a mask outside `Shape`, could run on the wrong
/// side of the other and let a masked press reach the wire.
#[derive(bevy::ecs::schedule::SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum LocalInputSet {
    Sources,
    Shape,
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

    // Desktop: a second launch carrying a join link (a `twotop://` link the
    // browser opened) hands it to the game already running, and exits.
    #[cfg(not(any(target_os = "android", target_family = "wasm")))]
    {
        if platform::desktop::hand_off_to_running_instance(platform::launch_link().as_deref()) {
            return;
        }
        #[cfg(windows)]
        platform::desktop::register_scheme();
    }

    config::log_configuration();
    let netplay = NetplayConfig::from_env_and_args();
    let online = netplay.room_url.is_some();
    // What this device is and can do, asked once; every presentation
    // policy reads it (`platform::DeviceProfile`).
    let profile = platform::DeviceProfile::detect();
    tracing::info!(target: "two_top::app", ?profile, "device profile");

    let mut app = App::new();
    // First, so every plugin's `build` reads the same answer.
    app.insert_resource(profile);
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
                        // The web build renders into the page's own canvas
                        // (web/index.html) and follows its CSS size — the
                        // page owns layout, the game fills the element.
                        #[cfg(target_family = "wasm")]
                        canvas: Some("#twotop-canvas".into()),
                        #[cfg(target_family = "wasm")]
                        fit_canvas_to_parent: true,
                        ..default()
                    }),
                    ..default()
                })
                .disable::<bevy::log::LogPlugin>();
            // Desktop asset root: wherever this build's assets actually are
            // (`platform::desktop::asset_root` — beside a shipped binary,
            // in a macOS bundle, or the checkout under `cargo run`). It was
            // the checkout's absolute path, baked in at compile time, which
            // left a build on any other machine untextured and silent. No
            // `.meta` sidecars ship, so the probe for each is skipped (as in
            // the browser).
            // Android deliberately keeps the default ("assets"): cargo-apk
            // bundles `../../assets` (Cargo.toml `[package.metadata.android]
            // assets`) as the APK asset root, so the in-code load paths
            // ("audio/throw.wav", "sprites/...") already line up there.
            // Wasm keeps the default too: the browser fetches "assets/…"
            // relative to the page, and the Pages workflow serves the
            // workspace assets dir beside index.html.
            #[cfg(not(any(target_os = "android", target_family = "wasm")))]
            let plugins = plugins.set(bevy::asset::AssetPlugin {
                file_path: platform::desktop::asset_root()
                    .to_string_lossy()
                    .into_owned(),
                meta_check: bevy::asset::AssetMetaCheck::Never,
                ..default()
            });
            // The browser skips Bevy's `.meta` sidecar probe. By default every
            // asset load first asks for `<asset>.meta`; this repo ships none,
            // so on the web each one was a wasted round trip (a 404 in the
            // console, ~40 of them on boot) before the real fetch could start.
            #[cfg(target_family = "wasm")]
            let plugins = plugins.set(bevy::asset::AssetPlugin {
                meta_check: bevy::asset::AssetMetaCheck::Never,
                ..default()
            });
            // Phones request the GPU device with the core feature set only
            // (`DeviceProfile::core_gpu_only`). The browser keeps Bevy's own
            // WebGL2 settings untouched (the profile never asks it to).
            if profile.core_gpu_only {
                plugins.set(bevy::render::RenderPlugin {
                    render_creation: bevy::render::settings::WgpuSettings {
                        disabled_features: Some(bevy::render::settings::WgpuFeatures::all()),
                        ..default()
                    }
                    .into(),
                    ..default()
                })
            } else {
                plugins
            }
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
        // The mouse as a finger, ahead of every touch reader (`pointer`).
        .add_plugins(pointer::PointerPlugin)
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
        .add_plugins(share::SharePlugin)
        .add_plugins(rivals::RivalsPlugin)
        .add_plugins(ProfilePlugin)
        .add_plugins(MatchRecorderPlugin)
        .add_plugins(RoomCodePlugin)
        .add_plugins(TheaterPlugin)
        .add_plugins(arena_select::ArenaSelectPlugin)
        .add_plugins(intro_card::IntroCardPlugin)
        .add_plugins(runes::RunesPlugin)
        .add_plugins(TouchControlsPlugin)
        .add_plugins(BotPlugin)
        .add_plugins(callout::CalloutPlugin)
        .add_plugins(first_blood::FirstBloodPlugin)
        .add_plugins(nudges::NudgesPlugin)
        .add_plugins(state_log::StateLogPlugin)
        .add_plugins(tuning_reload::TuningReloadPlugin)
        .add_plugins(HudPlugin)
        .add_plugins(DebugInputOverlayPlugin)
        .add_plugins(net::NetPlugin)
        .add_plugins(LobbyOverlayPlugin)
        .add_plugins(ScreenPlugin)
        .add_plugins(SettingsPlugin)
        .add_plugins(lifecycle::LifecyclePlugin)
        .add_plugins(crash_report::CrashReportPlugin)
        .init_resource::<FangTextures>()
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
    app.add_plugins(netplay::NetplayStatePlugin);
    if online {
        app.add_plugins(MatchboxPlugin);
    }
    app.add_plugins(platform::PlatformPlugin);

    // One input chain, the same on every platform (level signals only).
    // The keyboard/gamepad source writes every local handle (WASD for P0,
    // arrows for P1, so two friends play couch versus on one keyboard);
    // the flip mirror reflects it for a P1 seat; then the touch source
    // overwrites the local handle, but only while a finger (or the mouse,
    // `pointer`) is actually down, so a keyboard is never stamped over;
    // then the bot patches handle 1 in practice. After that: the rematch
    // hold, the UI's injected THROW, the online rematch gate, and — when a
    // tape is loaded — the theater's playback source, which replaces the
    // whole map.
    //
    // This used to be three chains. Android took touch as its only source
    // (and wrote one finger to BOTH handles), native desktop never read
    // touch at all, and the browser layered the two. A touch layer fixed
    // on one platform stayed broken on another until someone noticed.
    app.add_plugins(input_desktop::DesktopInputsPlugin);
    app.configure_sets(
        bevy_ggrs::prelude::ReadInputs,
        LocalInputSet::Shape.after(LocalInputSet::Sources),
    );
    app.add_systems(
        bevy_ggrs::prelude::ReadInputs,
        (mirror_keyboard_for_flip, read_touch_inputs, bot::drive_bot)
            .chain()
            .after(input_desktop::read_local_desktop_inputs)
            .in_set(LocalInputSet::Sources),
    );
    app.add_systems(
        Update,
        toggle_fullscreen.run_if(|profile: Res<platform::DeviceProfile>| profile.fullscreen_key),
    );
    #[cfg(not(any(target_os = "android", target_family = "wasm")))]
    app.add_systems(Update, platform::desktop::set_window_icon);
    // The nudges watch the local player's DASH presses where inputs are
    // made (a directionless press does nothing the sim could show).
    app.add_systems(
        bevy_ggrs::prelude::ReadInputs,
        nudges::watch_dash_presses.after(bot::drive_bot),
    );
    app.add_systems(
        bevy_ggrs::prelude::ReadInputs,
        (
            screen::hold_rematch_until_settled,
            screen::inject_ui_throw,
            netplay::gate_rematch_inputs,
            replay::playback_inputs_system.run_if(resource_exists::<replay::ReplayPlayback>),
        )
            .chain()
            .in_set(LocalInputSet::Shape),
    );

    // Verification capture (opt-in via TWOTOP_CAPTURE): screenshot then exit.
    if let Some(cap) = capture_config_from_env() {
        app.insert_resource(cap).add_systems(Last, capture_frame);
    }

    // A panic that unwinds out of the whole app ended the run; mark it for
    // the next launch's crash notice and hand it on (on Android the
    // platform adapter turns it into an abort). Panics something below
    // caught — a detached task's — never get here and leave no mark.
    #[cfg(not(target_family = "wasm"))]
    if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.run())) {
        logging::mark_fatal_panic();
        std::panic::resume_unwind(panic);
    }
    #[cfg(target_family = "wasm")]
    app.run();

    tracing::info!(target: "two_top::app", "two-top exiting cleanly");
}

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

/// Phase 13: surface graceful-shutdown events into the diagnostic log.
/// Bevy's `AppExit` event fires when the window closes or
/// `app.send_event(AppExit::Success)` is invoked elsewhere. Reading it
/// here gives the log a clear "shutdown initiated" line right before
/// the `App::run()` loop unwinds — useful for distinguishing a clean
/// exit from a panic-driven termination.
fn log_app_exit(mut events: MessageReader<AppExit>) {
    for ev in events.read() {
        tracing::info!(
            target: "two_top::app",
            kind = ?ev,
            "AppExit received — shutdown initiated",
        );
    }
}

/// The symbol `android-activity`'s NativeActivity glue calls on its own
/// thread once the Java activity exists. Everything about that thread's
/// lifetime — and the process's — is `platform::android::main`'s business.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(android_app: bevy::android::android_activity::AndroidApp) {
    platform::android::main(android_app);
}

fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut atlases: ResMut<Assets<TextureAtlasLayout>>,
    selected: Res<SelectedArena>,
    profile: Res<platform::DeviceProfile>,
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
    // ONE camera, and the glow is a runtime decision (`DeviceProfile`).
    // This used to be two `cfg` blocks, which meant a phone browser took
    // the desktop one and paid ~65 ms/frame for an HDR + bloom chain it
    // could not afford, while the APK on the same handset ran lean at 60.
    // The cost tracks the GPU, not the target triple — so a phone browser
    // now gets what the APK gets, and a desktop browser keeps the full
    // HLD glow exactly like the native desktop build.
    {
        const VIEW_MARGIN_CM: f32 = 80.0;
        let min_width = (2 * sim::ARENA_HALF_WIDTH_CM) as f32 + 2.0 * VIEW_MARGIN_CM;
        // The arena renders Y-foreshortened, so frame the foreshortened height.
        let min_height =
            (2 * sim::ARENA_HALF_HEIGHT_CM) as f32 * render::WORLD_TILT_Y + 2.0 * VIEW_MARGIN_CM;
        let mut camera = commands.spawn((
            Camera2d,
            // MSAA does nothing for quad sprites; skip its fill cost.
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
        if profile.lean_render {
            // Overdriven accent colors clamp to white instead of
            // blooming — the trade the product platform already makes.
            tracing::info!(target: "two_top::app", "lean render: no HDR, no bloom");
        } else {
            camera.insert((
                bevy::render::view::Hdr,
                bevy::post_process::bloom::Bloom::OLD_SCHOOL,
            ));
        }
    }
    // The couch-play legend, wherever a keyboard is the likely input: the
    // desktop build and a laptop browser (which had no controls on screen
    // at all), never a touchscreen. World-space Text2d (the app has no
    // bevy_ui), pinned to the bottom of the *screen* so it stays clear of
    // the top-edge HUD (pips + round clock) on any window aspect. Bounded
    // so a narrow window wraps the long lines instead of clipping them.
    if profile.shows_keyboard_legend {
        commands.spawn((
            Text2d::new(keyboard_legend(profile.fullscreen_key)),
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

    let floor_img = asset_server.load(render::arena::info(selected.0).floor);
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

/// The couch keyboard legend. It used to end with "or controllers (build
/// --features gamepad)": a build instruction, shown to players.
fn keyboard_legend(fullscreen_key: bool) -> String {
    let mut legend = String::from(
        "P0: WASD  -  Space throw  -  LShift dash  -  T taunt\n\
         P1: Arrows  -  RShift throw  -  RCtrl dash  -  Enter taunt",
    );
    if fullscreen_key {
        legend.push_str("\nF11 fullscreen");
    }
    legend
}

/// Mirrors the primary `Window`'s logical size into the resource
/// `input_touch` reads. Lives in the app crate (not `input_touch`) because
/// only the app pulls in `bevy_window` — `input_touch` stays
/// headless-friendly for tests and CI. Ordered before
/// `input_touch::update_touch_state` so the same-frame touch sync sees
/// fresh metrics. (The mouse reaches `input_touch` as a finger, through
/// `pointer`, so no cursor position is mirrored any more.)
fn update_window_metrics(window: Query<&Window>, mut window_size: ResMut<WindowSize>) {
    let Ok(w) = window.single() else { return };
    window_size.0 = Vec2::new(w.width(), w.height());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_finger_drives_the_online_seat_or_else_p0_alone() {
        assert_eq!(touch_handle(Some(1), &[1]), Some(1));
        // Couch / practice / First Blood: both handles are local, and a
        // finger is the near seat's — never both players at once.
        assert_eq!(touch_handle(None, &[0, 1]), Some(0));
        assert_eq!(touch_handle(None, &[1, 0]), Some(0));
        assert_eq!(touch_handle(None, &[]), None);
    }

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
