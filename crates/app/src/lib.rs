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
use render::RenderSyncPlugin;
use sim::{GgrsCfg, Player, PositionF, PreviousPositionF, SimPlugin, VelocityF, arena_walls};

mod camera;
mod debug_overlay;
use camera::CameraFollowPlugin;
use debug_overlay::DebugInputOverlayPlugin;

pub fn run() {
    let mut sb = SessionBuilder::<GgrsCfg>::new()
        .with_num_players(2)
        .expect("with_num_players")
        .with_check_distance(2)
        .with_input_delay(2);
    for i in 0..2 {
        sb = sb.add_player(PlayerType::Local, i).expect("add_player");
    }
    let session = sb.start_synctest_session().expect("synctest");

    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(GgrsPlugin::<GgrsCfg>::default())
        .add_plugins(SimPlugin)
        .add_plugins(TouchInputsPlugin)
        .add_plugins(RenderSyncPlugin)
        .add_plugins(CameraFollowPlugin)
        .add_plugins(DebugInputOverlayPlugin)
        .insert_resource(Session::SyncTest(session))
        .add_systems(Startup, setup)
        .add_systems(PreUpdate, update_window_metrics.before(update_touch_state))
        .run();
}

#[bevy_main]
fn main() {
    run();
}

fn setup(mut commands: Commands) {
    commands.spawn(Camera2d);

    commands.spawn((
        Player { handle: 0 },
        PositionF(Vec2F::from_cm(-100, 60)),
        PreviousPositionF(Vec2F::from_cm(-100, 60)),
        VelocityF(Vec2F::ZERO),
        Sprite {
            color: Color::srgb(0.95, 0.42, 0.65),
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
            color: Color::srgb(0.45, 0.78, 0.95),
            custom_size: Some(Vec2::new(48.0, 48.0)),
            ..default()
        },
        Transform::default(),
    ));

    // Arena boundary walls. Spawned in the fixed order returned by
    // `arena_walls()` so entity ids are bit-identical across hosts
    // (rollback determinism depends on this).
    for wall in arena_walls() {
        let size_cm = (
            (wall.rect.max.x - wall.rect.min.x).to_num::<f32>(),
            (wall.rect.max.y - wall.rect.min.y).to_num::<f32>(),
        );
        let center = (
            (wall.rect.min.x + wall.rect.max.x).to_num::<f32>() * 0.5,
            (wall.rect.min.y + wall.rect.max.y).to_num::<f32>() * 0.5,
        );
        commands.spawn((
            wall,
            Sprite {
                color: Color::srgb(0.18, 0.18, 0.22),
                custom_size: Some(Vec2::new(size_cm.0, size_cm.1)),
                ..default()
            },
            Transform::from_xyz(center.0, center.1, -1.0),
        ));
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
