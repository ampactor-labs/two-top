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
use input_touch::{CursorPosition, TouchInputsPlugin, WindowSize, update_touch_state};
use render::{EffectsPlugin, RenderSyncPlugin};
use sim::{
    AnimState, BOOMERANG_HALF_EXTENT_CM, Boomerang, GgrsCfg, Player, PositionF, PreviousPositionF,
    SimLifecycleLogPlugin, SimPlugin, VelocityF, arena_walls,
};

mod camera;
mod debug_overlay;
mod lobby_overlay;
mod logging;
use camera::CameraFollowPlugin;
use debug_overlay::DebugInputOverlayPlugin;
use lobby_overlay::LobbyOverlayPlugin;

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

    let mut sb = SessionBuilder::<GgrsCfg>::new()
        .with_num_players(2)
        .expect("with_num_players")
        .with_check_distance(2)
        .with_input_delay(2);
    for i in 0..2 {
        sb = sb.add_player(PlayerType::Local, i).expect("add_player");
    }
    let session = sb.start_synctest_session().expect("synctest");
    tracing::info!(
        target: "two_top::app",
        kind = "synctest",
        num_players = 2,
        check_distance = 2,
        input_delay = 2,
        "ggrs session started",
    );

    App::new()
        // Disable bevy's LogPlugin: we own the subscriber installed
        // above. LogPlugin would try to call `set_global_default` again
        // and emit a noisy "global default subscriber set" complaint
        // (and skip its own filter setup). Owning the subscriber lets
        // the file appender + custom filter survive `App::run()`.
        .add_plugins(DefaultPlugins.build().disable::<bevy::log::LogPlugin>())
        .add_plugins(GgrsPlugin::<GgrsCfg>::default())
        .add_plugins(SimPlugin)
        // Phase 13: edge-detect MatchState/MatchScore transitions in
        // Update so the diagnostic log captures round flow without
        // duplicating events on each rollback resimulation. Headless
        // ceremonies (sync_test, replay_sync) intentionally don't add
        // this plugin.
        .add_plugins(SimLifecycleLogPlugin)
        .add_plugins(TouchInputsPlugin)
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
        )
        .run();

    tracing::info!(target: "two_top::app", "two-top exiting cleanly");
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
) {
    commands.spawn(Camera2d);

    // Phase 15: load the polished player sheets and slice them into
    // 22-frame atlases (24x24 source per cell). Onscreen each player
    // renders at 48x48 — same as the placeholder rectangle they
    // replace, so the camera-follow + hitbox visualization land at
    // the same scale.
    let layout = atlases.add(TextureAtlasLayout::from_grid(
        UVec2::splat(24),
        22,
        1,
        None,
        None,
    ));
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
            custom_size: Some(Vec2::new(48.0, 48.0)),
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
            custom_size: Some(Vec2::new(48.0, 48.0)),
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

    // Phase 15 cycle 3b: arena backdrop. training_floor.png is the
    // composed Bone-Cathedral arena (160x240 px source) — scaled to
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
