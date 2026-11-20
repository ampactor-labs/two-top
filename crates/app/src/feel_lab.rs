//! The feel lab: practice with the game's insides showing.
//!
//! A tool for tuning how the game feels, on the device it is felt on. In a
//! practice match, a three-finger tap (F1 on a keyboard) turns it on:
//!
//! * every duelist and fang wears its sim hitbox, so "that should have
//!   hit" can be checked against what the sim actually tested;
//! * a readout says, per duelist, the charge, the dash and its i-frames,
//!   the stun, the catch streak, and the input on the wire this tick;
//! * a four-finger tap (F2) steps the speed through 1x, 0.5x, 0.25x and
//!   paused, through the same `Time<Virtual>` transport the theater uses.
//!
//! Hidden on purpose: nothing about it is for a player. Presentation and
//! input `Tuning` stay live-reloadable (`TWOTOP_TUNING`) while it runs.

use bevy::prelude::*;
use sim::{Boomerang, DashState, Player, PositionF, StunFrames, ThrowCharge, TickInputs};

use crate::anchor::ScreenAnchor;

/// The speeds the lab steps through; 0 is paused.
const LAB_SPEEDS: [f32; 4] = [1.0, 0.5, 0.25, 0.0];

#[derive(Resource, Default)]
pub struct FeelLab {
    pub on: bool,
    speed_idx: usize,
}

#[derive(Component)]
struct LabReadout;

#[derive(Component)]
struct LabHitbox;

/// The readout's text for one duelist. Pure.
pub fn duelist_line(
    handle: usize,
    charge: u32,
    dash: &DashState,
    stun: u32,
    input: sim::PlayerInput,
) -> String {
    let dash = match dash {
        DashState::Idle => "ready".to_string(),
        DashState::Dashing {
            frames_remaining, ..
        } => format!("DASH {frames_remaining} (i-frames)"),
        DashState::Cooldown { frames_remaining } => format!("cool {frames_remaining}"),
    };
    let bit = |b: u8, c: char| if input.buttons & b != 0 { c } else { '.' };
    format!(
        "P{handle} charge {charge:>2}  {dash:<18} stun {stun:>2}  in {}{}{}{}{} {:>4},{:>4}",
        bit(sim::PlayerInput::THROW_DOWN, 'T'),
        bit(sim::PlayerInput::AIM_ACTIVE, 'A'),
        bit(sim::PlayerInput::DASH_DOWN, 'D'),
        bit(sim::PlayerInput::TAUNT_DOWN, 'F'),
        bit(sim::PlayerInput::HOLD, 'H'),
        input.stick_x,
        input.stick_y,
    )
}

fn toggle_lab(
    keys: Res<ButtonInput<KeyCode>>,
    touches: Res<Touches>,
    practice: Res<crate::bot::PracticeMode>,
    mut lab: ResMut<FeelLab>,
) {
    let fingers = touches.iter().count();
    let fresh = touches.iter_just_pressed().count() > 0;
    if keys.just_pressed(KeyCode::F1) || (fresh && fingers == 3) {
        lab.on = !lab.on && practice.0;
        lab.speed_idx = 0;
    }
    if lab.on && (keys.just_pressed(KeyCode::F2) || (fresh && fingers == 4)) {
        lab.speed_idx = (lab.speed_idx + 1) % LAB_SPEEDS.len();
    }
    if !practice.0 {
        lab.on = false;
    }
}

fn apply_lab_speed(world: &mut World) {
    let (on, idx) = {
        let lab = world.resource::<FeelLab>();
        (lab.on, lab.speed_idx)
    };
    if !world.is_resource_changed::<FeelLab>() {
        return;
    }
    if !on {
        crate::transport::realtime(world);
        return;
    }
    let speed = LAB_SPEEDS[idx];
    crate::transport::apply(world, speed == 0.0, speed.max(0.05));
}

fn spawn_readout(mut commands: Commands) {
    commands.spawn((
        LabReadout,
        Text2d::new(String::new()),
        (
            TextFont {
                font_size: crate::ui::TextRole::Debug.font_size(),
                ..default()
            },
            crate::ui::TextRole::Debug,
        ),
        TextColor(render::palette::SPARK),
        bevy::sprite::Anchor::TOP_LEFT,
        ScreenAnchor::new(-0.96, 0.62, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, crate::ui::z::MENU_TEXT + 20.0),
        Visibility::Hidden,
    ));
}

#[allow(clippy::type_complexity)]
fn update_readout(
    lab: Res<FeelLab>,
    frame: Res<sim::FrameCount>,
    inputs: Res<TickInputs>,
    players: Query<(&Player, &ThrowCharge, &DashState, &StunFrames)>,
    mut q: Query<(&mut Text2d, &mut Visibility), With<LabReadout>>,
) {
    let Ok((mut text, mut vis)) = q.single_mut() else {
        return;
    };
    if !lab.on {
        *vis = Visibility::Hidden;
        return;
    }
    *vis = Visibility::Visible;
    let mut rows: Vec<_> = players.iter().collect();
    rows.sort_by_key(|(p, ..)| p.handle);
    let speed = LAB_SPEEDS[lab.speed_idx];
    let mut out = format!(
        "FEEL LAB  tick {}  speed {}\n",
        frame.0,
        if speed == 0.0 {
            "PAUSED".to_string()
        } else {
            format!("{speed}x")
        }
    );
    for (p, charge, dash, stun) in rows {
        out.push_str(&duelist_line(
            p.handle,
            charge.0,
            dash,
            stun.0,
            inputs.0[p.handle.min(1)],
        ));
        out.push('\n');
    }
    out.push_str("3 fingers: lab off   4 fingers: speed");
    text.0 = out;
}

/// One box per duelist and fang, the size the sim tests, every frame.
fn draw_hitboxes(
    mut commands: Commands,
    lab: Res<FeelLab>,
    old: Query<Entity, With<LabHitbox>>,
    players: Query<&PositionF, With<Player>>,
    fangs: Query<&PositionF, With<Boomerang>>,
    flip: Res<render::PerspectiveFlip>,
) {
    for e in &old {
        commands.entity(e).despawn();
    }
    if !lab.on {
        return;
    }
    let boxes = players
        .iter()
        .map(|p| (p, sim::PLAYER_HALF_EXTENT_CM, render::palette::P1_CYAN))
        .chain(
            fangs
                .iter()
                .map(|p| (p, sim::BOOMERANG_HALF_EXTENT_CM, render::palette::SPARK)),
        );
    for (pos, half, color) in boxes {
        let (x, y) = pos.0.to_f32();
        let half = half as f32;
        commands.spawn((
            LabHitbox,
            Sprite {
                color: color.with_alpha(0.35),
                custom_size: Some(Vec2::new(half * 2.0, half * 2.0 * render::WORLD_TILT_Y)),
                ..default()
            },
            Transform::from_xyz(x * flip.0, render::tilt_y(y) * flip.0, 50.0),
        ));
    }
}

pub struct FeelLabPlugin;

impl Plugin for FeelLabPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(FeelLab {
            on: crate::config::Knob::FeelLab.is_on(),
            speed_idx: 0,
        })
        .add_systems(Startup, spawn_readout)
        .add_systems(
            Update,
            (toggle_lab, apply_lab_speed, update_readout, draw_hitboxes).chain(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_readout_says_what_is_on_the_wire() {
        let input = sim::PlayerInput {
            stick_x: 12,
            stick_y: -3,
            aim_angle: 0,
            buttons: sim::PlayerInput::THROW_DOWN | sim::PlayerInput::HOLD,
        };
        let line = duelist_line(1, 7, &DashState::Idle, 0, input);
        assert!(line.starts_with("P1 charge  7"), "{line}");
        assert!(line.contains("T...H"), "{line}");
        assert!(line.contains("12,  -3"), "{line}");
    }
}
