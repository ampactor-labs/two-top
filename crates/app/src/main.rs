//! Phase 7 visual smoke test. Two pink/blue quads drift left and right
//! under synthesized stick input while the sim runs at 60 Hz under a
//! `SyncTestSession`. The render layer (`RenderSyncPlugin`) interpolates
//! transforms between sim ticks at whatever rate the display refreshes —
//! the visible smoothness is the Phase 7 exit criterion. SyncTest's
//! mismatch observer (in `SimPlugin`) panics on any single-machine
//! determinism violation, so this app doubles as a long-running soak.

use bevy::prelude::*;
use bevy_ggrs::GgrsPlugin;
use bevy_ggrs::prelude::*;
use fixed_math::Vec2F;
use render::RenderSyncPlugin;
use sim::{
    DefaultInputsPlugin, GgrsCfg, Player, PlayerInput, PositionF, PreviousPositionF, SimPlugin,
    SynthesizedInputs, VelocityF,
};

fn main() {
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
        .add_plugins(DefaultInputsPlugin)
        .add_plugins(RenderSyncPlugin)
        .insert_resource(Session::SyncTest(session))
        .add_systems(Startup, setup)
        .add_systems(Update, drive_inputs)
        .run();
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
}

fn drive_inputs(time: Res<Time<Real>>, mut synthesized: ResMut<SynthesizedInputs>) {
    // Sweep stick_x to ±100 every two seconds. Both players share the
    // input, which makes desyncs (if any) immediately visible as the two
    // sprites parting ways.
    let phase = (time.elapsed_secs_f64() % 2.0) / 2.0;
    let dir = if phase < 0.5 { 100i8 } else { -100i8 };
    synthesized.0 = PlayerInput {
        stick_x: dir,
        stick_y: 0,
        aim_angle: 0,
        buttons: 0,
    };
}
