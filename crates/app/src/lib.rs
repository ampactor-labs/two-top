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

mod audio;
mod camera;
mod debug_overlay;
mod haptics;
mod hud;
mod lobby_overlay;
mod logging;
mod netplay;
mod screen;
mod settings;
use audio::GameAudioPlugin;
use camera::CameraFollowPlugin;
use debug_overlay::DebugInputOverlayPlugin;
use haptics::HapticsPlugin;
use hud::HudPlugin;
use lobby_overlay::LobbyOverlayPlugin;
use netplay::{MatchboxPlugin, NetplayConfig};
use screen::{AppScreen, ScreenPlugin};
use settings::SettingsPlugin;

/// Pick the arena from the `TWOTOP_ARENA` env var (desktop testing handle
/// until the lobby arena-picker lands). Defaults to the tournament Anchor.
fn arena_from_env() -> SelectedArena {
    let id = match std::env::var("TWOTOP_ARENA").as_deref() {
        Ok("crossing") => sim::ArenaId::Crossing,
        Ok("reliquary") => sim::ArenaId::Reliquary,
        _ => sim::ArenaId::Anchor,
    };
    SelectedArena(id)
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
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "2-Top".to_string(),
                        resolution: (600u32, 900u32).into(),
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
        // Arena selection. TWOTOP_ARENA=anchor|crossing|reliquary can seed
        // desktop automation; the Title picker may overwrite it before
        // `screen::spawn_match` reads the resource. Must be inserted AFTER
        // SimPlugin (which defaults it to Anchor).
        .insert_resource(arena_from_env())
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
        .add_plugins(HudPlugin)
        .add_plugins(DebugInputOverlayPlugin)
        .add_plugins(net::NetPlugin)
        .add_plugins(LobbyOverlayPlugin)
        .add_plugins(ScreenPlugin)
        .add_plugins(SettingsPlugin)
        .init_resource::<netplay::LocalPlayerHandle>()
        .init_resource::<render::PerspectiveFlip>()
        .insert_resource(netplay.clone())
        .add_systems(Startup, setup)
        .add_systems(PreUpdate, update_window_metrics.before(update_touch_state))
        .add_systems(
            Update,
            (
                ensure_boomerang_visuals,
                sync_sprite_atlas_from_anim,
                frame_time_watch,
                log_app_exit,
                update_arena_floor,
            ),
        );

    app.insert_state(AppScreen::Title);
    if online {
        app.add_plugins(MatchboxPlugin);
    }

    // Platform input source (level signals only; exactly one source).
    // Android: touch. Everything else (PC): keyboard — WASD for P0, arrows
    // for P1, so two friends play couch versus on one keyboard.
    #[cfg(target_os = "android")]
    app.add_systems(
        bevy_ggrs::prelude::ReadInputs,
        input_touch::read_local_touch_inputs,
    );
    #[cfg(not(target_os = "android"))]
    {
        app.add_plugins(input_desktop::DesktopInputsPlugin);
        app.add_systems(Update, toggle_fullscreen);
    }

    // Verification capture (opt-in via TWOTOP_CAPTURE): screenshot then exit.
    if let Some(cap) = capture_config_from_env() {
        app.insert_resource(cap).add_systems(Last, capture_frame);
    }

    app.run();

    tracing::info!(target: "two_top::app", "two-top exiting cleanly");
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

/// Verification capture mode. With `TWOTOP_CAPTURE=<path.png>` set, the app
/// renders for a settle window (so assets/atlases/visibility resolve), grabs a
/// PNG of the primary window's framebuffer, then exits — letting a render
/// change be *seen* (or screenshot-diffed) without a human watching the window.
/// Unset → the resource is never inserted and the system never registered, so a
/// normal `cargo run -p app` pays nothing. `TWOTOP_CAPTURE_FRAMES` overrides the
/// settle window (default 90 ≈ 1.5 s at 60 Hz).
#[derive(Resource)]
struct CaptureConfig {
    path: String,
    settle_frames: u32,
}

fn capture_config_from_env() -> Option<CaptureConfig> {
    let path = std::env::var("TWOTOP_CAPTURE")
        .ok()
        .filter(|p| !p.is_empty())?;
    let settle_frames = std::env::var("TWOTOP_CAPTURE_FRAMES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(90);
    Some(CaptureConfig {
        path,
        settle_frames,
    })
}

/// `Last`-schedule capture driver: count frames, fire one screenshot at the
/// settle mark, then quit a few frames later so the async GPU readback + file
/// write have flushed. The `save_to_disk` observer owns the actual encode.
fn capture_frame(
    mut commands: Commands,
    cfg: Res<CaptureConfig>,
    mut frame: Local<u32>,
    mut fired: Local<bool>,
    mut exit: MessageWriter<AppExit>,
) {
    use bevy::render::view::screenshot::{Screenshot, save_to_disk};
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

fn setup(mut commands: Commands, asset_server: Res<AssetServer>, selected: Res<SelectedArena>) {
    // Camera. Mobile: a bare Camera2d (1:1 pixels) with the follow cam in
    // `CameraFollowPlugin` keeps it zoomed in for a phone. Desktop: frame
    // the WHOLE arena at once (couch versus — both players always visible)
    // via AutoMin scaling, centered at the arena origin, no follow. AutoMin
    // guarantees the full arena is shown on any window aspect, pillarboxing
    // a wide monitor rather than cropping the portrait playfield.
    // HDR + thresholded bloom give the dark stage its HLD glow: only the
    // brightest accents (eye-slits, boomerang highlights, hit/kill flashes,
    // pickup auras) bloom, while `Tonemapping::None` keeps every other pixel
    // exactly on the locked 16-color palette. `Bloom::OLD_SCHOOL` carries a
    // high threshold so the matte cloaks and floor never wash out.
    #[cfg(target_os = "android")]
    commands.spawn((
        Camera2d,
        bevy::render::view::Hdr,
        bevy::core_pipeline::tonemapping::Tonemapping::None,
        bevy::post_process::bloom::Bloom::OLD_SCHOOL,
        camera::FollowCam,
    ));
    #[cfg(not(target_os = "android"))]
    {
        const VIEW_MARGIN_CM: f32 = 80.0;
        let min_width = (2 * sim::ARENA_HALF_WIDTH_CM) as f32 + 2.0 * VIEW_MARGIN_CM;
        // The arena renders Y-foreshortened, so frame the foreshortened height.
        let min_height =
            (2 * sim::ARENA_HALF_HEIGHT_CM) as f32 * render::WORLD_TILT_Y + 2.0 * VIEW_MARGIN_CM;
        commands.spawn((
            Camera2d,
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
        // Screen vignette (desktop only — the static whole-arena cam sits at the
        // origin, so a sprite sized to the AutoMin min view lands its dithered
        // dark frame at the screen edges). Frames the couch view + unifies the
        // palette (HLD cohesion). Above gameplay, below the HUD legend + kill
        // flash. On mobile the follow-cam moves, so the floor's own edge
        // vignette carries the framing there instead.
        let mut vig_color = Color::WHITE;
        vig_color.set_alpha(0.7);
        commands.spawn((
            Sprite {
                image: asset_server.load("sprites/fx/vignette.png"),
                color: vig_color,
                custom_size: Some(Vec2::new(min_width, min_height)),
                ..default()
            },
            Transform::from_xyz(0.0, 0.0, 45.0),
        ));
        // Couch-play control legend (desktop only — touch needs none).
        // World-space Text2d (the app has no bevy_ui); the static
        // whole-arena camera parks it at the top of the playfield without
        // a per-frame reposition.
        commands.spawn((
            Text2d::new(
                "P0: WASD  ·  Space throw  ·  LShift dash\n\
                 P1: Arrows  ·  RShift throw  ·  RCtrl dash\n\
                 or controllers (build --features gamepad)  ·  F11 fullscreen",
            ),
            TextFont {
                font_size: 26.0,
                ..default()
            },
            TextColor(render::palette::BONE.with_alpha(0.55)),
            Transform::from_xyz(0.0, sim::ARENA_HALF_HEIGHT_CM as f32 - 44.0, 100.0),
        ));
    }

    // Arena backdrop: the composed moody Bone-Cathedral floor (320x480 px
    // source) for the selected arena, sized to EXACTLY the safe playfield
    // (2×ARENA_HALF = 1000×1500 cm) so the floor's lit ledge lip lands on the
    // out-of-bounds death line — step off the lit island and you're over the
    // void (the Boomerang-Fu open-field read). On the static desktop cam the
    // void rings the island; the mobile follow-cam stays inside it. Z below
    // players, stains, and effects so everything sits ON the floor. Tagged
    // `ArenaFloor` so the lobby arena picker can retexture it live.
    commands.spawn((
        ArenaFloor,
        Sprite {
            image: asset_server.load(arena_floor_asset(selected.0)),
            custom_size: Some(Vec2::new(
                (sim::ARENA_HALF_WIDTH_CM * 2) as f32,
                // Y foreshortened into the tabletop tilt (matches every world Y).
                (sim::ARENA_HALF_HEIGHT_CM * 2) as f32 * render::WORLD_TILT_Y,
            )),
            ..default()
        },
        Transform::from_xyz(0.0, 0.0, -1.0),
    ));
}

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
    for (player, anim, vel, mut sprite) in &mut q {
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
