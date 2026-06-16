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
use bevy_ggrs::prelude::*;
use fixed_math::Vec2F;
use input_touch::{CursorPosition, InputTouchPlugin, WindowSize, update_touch_state};
use render::{EffectsPlugin, RenderSyncPlugin, player_atlas_layout, PLAYER_RENDER_SIZE};
use sim::{
    AnimState, BOOMERANG_HALF_EXTENT_CM, Boomerang, GgrsCfg, Player, PositionF, PreviousPositionF,
    SelectedArena, SimLifecycleLogPlugin, SimPlugin, VelocityF, arena_walls,
};

mod camera;
mod debug_overlay;
mod lobby_overlay;
mod logging;
use camera::CameraFollowPlugin;
use debug_overlay::DebugInputOverlayPlugin;
use lobby_overlay::LobbyOverlayPlugin;

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

    // Local play (PC couch versus + the touch dev build) runs a SyncTest
    // session with both players LOCAL: the input source feeds DISTINCT
    // per-handle inputs (keyboard P0/P1 on desktop), so it's a real versus
    // that also self-verifies determinism every frame. Input delay is 0 —
    // there's no network latency to hide locally, so inputs apply at once
    // for a snappy feel. (M4 swaps in a P2PSession with its own delay for
    // online play.)
    let mut sb = SessionBuilder::<GgrsCfg>::new()
        .with_num_players(2)
        .expect("with_num_players")
        .with_check_distance(2)
        .with_input_delay(0);
    for i in 0..2 {
        sb = sb.add_player(PlayerType::Local, i).expect("add_player");
    }
    let session = sb.start_synctest_session().expect("synctest");
    tracing::info!(
        target: "two_top::app",
        kind = "synctest",
        num_players = 2,
        check_distance = 2,
        input_delay = 0,
        "ggrs session started",
    );

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
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "2-Top".to_string(),
                        resolution: (600u32, 900u32).into(),
                        ..default()
                    }),
                    ..default()
                })
                .disable::<bevy::log::LogPlugin>(),
        )
        .add_plugins(GgrsPlugin::<GgrsCfg>::default())
        .add_plugins(SimPlugin)
        // Phase 16: arena selection. Until the lobby arena-picker UI lands
        // (polish), choose via TWOTOP_ARENA=anchor|crossing|reliquary. Must
        // be inserted AFTER SimPlugin (which defaults it to Anchor).
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
        .add_plugins(CameraFollowPlugin)
        .add_plugins(DebugInputOverlayPlugin)
        .add_plugins(net::NetPlugin)
        .add_plugins(LobbyOverlayPlugin)
        .insert_resource(Session::SyncTest(session))
        .add_systems(Startup, setup)
        .add_systems(PreUpdate, update_window_metrics.before(update_touch_state))
        .add_systems(
            Update,
            (
                ensure_boomerang_visuals,
                sync_sprite_atlas_from_anim,
                log_app_exit,
            ),
        );

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

fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut atlases: ResMut<Assets<TextureAtlasLayout>>,
    selected: Res<SelectedArena>,
) {
    // Camera. Mobile: a bare Camera2d (1:1 pixels) with the follow cam in
    // `CameraFollowPlugin` keeps it zoomed in for a phone. Desktop: frame
    // the WHOLE arena at once (couch versus — both players always visible)
    // via AutoMin scaling, centered at the arena origin, no follow. AutoMin
    // guarantees the full arena is shown on any window aspect, pillarboxing
    // a wide monitor rather than cropping the portrait playfield.
    #[cfg(target_os = "android")]
    commands.spawn((Camera2d, camera::FollowCam));
    #[cfg(not(target_os = "android"))]
    {
        const VIEW_MARGIN_CM: f32 = 80.0;
        let min_width = (2 * sim::ARENA_HALF_WIDTH_CM) as f32 + 2.0 * VIEW_MARGIN_CM;
        let min_height = (2 * sim::ARENA_HALF_HEIGHT_CM) as f32 + 2.0 * VIEW_MARGIN_CM;
        commands.spawn((
            Camera2d,
            Projection::from(OrthographicProjection {
                scaling_mode: bevy::camera::ScalingMode::AutoMin {
                    min_width,
                    min_height,
                },
                ..OrthographicProjection::default_2d()
            }),
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
            TextColor(Color::srgba(0.82, 0.82, 0.78, 0.55)),
            Transform::from_xyz(0.0, sim::ARENA_HALF_HEIGHT_CM as f32 - 44.0, 100.0),
        ));
    }

    // ART_DIRECTION.md v2: 41-frame atlases (32x32 source per cell).
    // Render at PLAYER_RENDER_SIZE (64x64 world units).
    let layout = player_atlas_layout(&mut atlases);
    let p0_image = asset_server.load("sprites/players/duelist_a_sheet.png");
    let p1_image = asset_server.load("sprites/players/duelist_b_sheet.png");

    commands.spawn((
        Player { handle: 0 },
        PositionF(Vec2F::from_cm(-100, 60)),
        PreviousPositionF(Vec2F::from_cm(-100, 60)),
        VelocityF(Vec2F::ZERO),
        Sprite {
            image: p0_image,
            texture_atlas: Some(TextureAtlas {
                layout: layout.clone(),
                index: 0,
            }),
            custom_size: Some(Vec2::splat(PLAYER_RENDER_SIZE)),
            ..default()
        },
        Transform::default(),
    ));

    commands.spawn((
        Player { handle: 1 },
        PositionF(Vec2F::from_cm(100, -60)),
        PreviousPositionF(Vec2F::from_cm(100, -60)),
        VelocityF(Vec2F::ZERO),
        Sprite {
            image: p1_image,
            texture_atlas: Some(TextureAtlas {
                layout,
                index: 0,
            }),
            custom_size: Some(Vec2::splat(PLAYER_RENDER_SIZE)),
            ..default()
        },
        Transform::default(),
    ));

    // Arena boundary walls. Spawned in the fixed order returned by
    // `arena_walls()` so entity ids are bit-identical across hosts
    // (rollback determinism depends on this). Phase 15 cycle 3b:
    // walls no longer spawn with their own placeholder Sprite; the
    // training_floor backdrop below already integrates the wall
    // pattern visually, so an extra dark rectangle would just sit
    // on top of the polished art and clip it.
    for wall in arena_walls() {
        commands.spawn(wall);
    }

    // Phase 15 cycle 3b / A2: arena backdrop. training_floor.png is the
    // composed moody Bone-Cathedral floor (320x480 px source) — scaled to
    // cover roughly the arena's playable area + walls (1100x1600 cm).
    // Z below players, stains, and effect sprites so the camera
    // composition reads as "everything sits ON the cathedral floor".
    commands.spawn((
        Sprite {
            image: asset_server.load("arenas/training_floor.png"),
            custom_size: Some(Vec2::new(1100.0, 1600.0)),
            ..default()
        },
        Transform::from_xyz(0.0, 0.0, -1.0),
    ));

    // Phase 16: spawn the selected arena's bone-pyre cover (shared helper
    // so app + replay_viewer render arenas identically).
    render::spawn_arena_props(&mut commands, &asset_server, &mut atlases, &selected);
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
    q: NewBoomerangs,
) {
    // Phase 15: render the bone fang as the polished 12x12 sprite,
    // upscaled 2x to match the placeholder's original 20-px footprint
    // (BOOMERANG_HALF_EXTENT_CM * 2 = 20). Marked variants will land
    // when per-round blood accumulation is wired in cycle 3.
    let size_px = ((BOOMERANG_HALF_EXTENT_CM * 2) as f32) * 2.0;
    let image = asset_server.load("sprites/projectiles/bone_fang.png");
    for (entity, pos) in &q {
        let (x, y) = pos.0.to_f32();
        commands.entity(entity).insert((
            Sprite {
                image: image.clone(),
                custom_size: Some(Vec2::new(size_px, size_px)),
                ..default()
            },
            Transform::from_xyz(x, y, 0.5),
        ));
    }
}

/// Phase 15 cycle 1: drive each player's TextureAtlas index from the
/// rolled-back AnimState. Runs in `Update` after sim has advanced;
/// `display_index()` snaps to a single atlas frame per tick (no
/// interpolation per CONVENTIONS § Render Layer Rules).
fn sync_sprite_atlas_from_anim(mut q: Query<(&AnimState, &mut Sprite), With<Player>>) {
    for (anim, mut sprite) in &mut q {
        if let Some(atlas) = sprite.texture_atlas.as_mut() {
            atlas.index = anim.display_index() as usize;
        }
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
