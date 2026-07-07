//! On-screen touch controls — the visible half of the input layer.
//!
//! The match screen is split down the center (input_touch's zones). The
//! LEFT half is the floating move stick: wherever a touch lands, the stick
//! spawns there — base ring at the touch origin, knob riding the drag, gone
//! the moment the thumb lifts. The RIGHT half is the throw BUTTON: any
//! touch there charges a throw, and while it's held the LEFT stick aims
//! (same model as the desktop throw-key + d-pad). A ring appears under the
//! right thumb as the charge cue, and the move stick turns ember to say
//! "you're aiming with this now". DASH stays fixed in the bottom-right
//! corner (the dash zone), where the right thumb can find it blind.
//!
//! Render-only, reads the local `TouchState` (never rolled back). Shown
//! only in a live match on touch builds (or with `TWOTOP_SHOW_TOUCH=1` for
//! desktop capture verification).

use bevy::prelude::*;
use input_touch::{STICK_DEADZONE_SATURATION, STICK_MAX_RADIUS_PX, TouchState, WindowSize};

use crate::anchor::{ScreenAnchor, ScreenAnchorSet, ViewRect};
use crate::screen::{AppScreen, AwaitingPeer};

/// One drawable piece of the control layer.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum Part {
    MoveBase,
    MoveKnob,
    ThrowRing,
    Dash,
}

#[derive(Component)]
struct DashLabel;

/// One-shot onboarding labels for the center-split scheme. Each hint
/// disappears forever (this app run) the first time its control is used —
/// the controls teach themselves, then get out of the way.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum HintZone {
    Move,
    Throw,
    Taunt,
}

/// Which hints have been "graduated" by actually using the control.
#[derive(Resource, Default)]
struct HintsUsed {
    moved: bool,
    threw: bool,
    taunted: bool,
}

/// Whether the on-screen controls render at all (touch platform / forced).
#[derive(Resource)]
struct TouchControlsShown(bool);

/// Fixed dash ring diameter in world units.
const DASH_RING_SIZE: f32 = 150.0;

/// Knob diameter as a fraction of the move base ring's.
const KNOB_FRAC: f32 = 0.45;

/// Throw-button ring diameter as a fraction of the move base ring's.
const THROW_RING_FRAC: f32 = 0.62;

pub struct TouchControlsPlugin;

impl Plugin for TouchControlsPlugin {
    fn build(&self, app: &mut App) {
        let shown = cfg!(target_os = "android")
            || std::env::var("TWOTOP_SHOW_TOUCH").is_ok_and(|v| v == "1");
        app.insert_resource(TouchControlsShown(shown))
            .init_resource::<HintsUsed>()
            .add_systems(Startup, spawn_controls)
            // After the anchor pass: ViewRect is fresh (post camera rig), so
            // the screen-to-world conversion tracks shake/zoom with no lag.
            .add_systems(Update, update_controls.after(ScreenAnchorSet));
    }
}

fn spawn_controls(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    shown: Res<TouchControlsShown>,
) {
    if !shown.0 {
        return;
    }
    let ring_img = asset_server.load("sprites/fx/charge_ring.png");
    // Floating pieces: positions and sizes are set every frame while their
    // touch is live; hidden entities keep stale transforms.
    for (part, z) in [
        (Part::MoveBase, 60.0),
        (Part::MoveKnob, 61.0),
        (Part::ThrowRing, 60.0),
    ] {
        commands.spawn((
            part,
            Sprite {
                image: ring_img.clone(),
                ..default()
            },
            // Above gameplay + HUD, below the menu scrim (z=100) so the
            // controls hide themselves on the title with everything else.
            Transform::from_xyz(0.0, 0.0, z),
            Visibility::Hidden,
        ));
    }
    // The dash corner ring + label.
    commands.spawn((
        Part::Dash,
        Sprite {
            image: ring_img.clone(),
            color: dash_idle(),
            custom_size: Some(Vec2::splat(DASH_RING_SIZE)),
            ..default()
        },
        dash_anchor(),
        Transform::from_xyz(0.0, 0.0, 60.0),
        Visibility::Hidden,
    ));
    commands.spawn((
        DashLabel,
        Text2d::new("DASH"),
        TextFont {
            font_size: 30.0,
            ..default()
        },
        TextColor(dash_idle()),
        TextLayout::new_with_justify(Justify::Center),
        dash_anchor(),
        Transform::from_xyz(0.0, 0.0, 61.0),
        Visibility::Hidden,
    ));
    // Zone hints for first-time thumbs — quiet, mid-low on each half where
    // the eye lands but the thumb won't cover them. The taunt hint sits in
    // its own strip at the top (where the tap goes).
    for (zone, fx, fy, text) in [
        (HintZone::Move, -0.5, -0.38, "MOVE\ndrag anywhere here"),
        (HintZone::Throw, 0.5, -0.38, "THROW\nhold to charge, stick aims"),
        (HintZone::Taunt, 0.0, 0.56, "TAUNT\ntap up here: finish the flex, tier up"),
    ] {
        commands.spawn((
            zone,
            Text2d::new(text),
            TextFont {
                font_size: 26.0,
                ..default()
            },
            TextColor(render::palette::COLD_STONE.with_alpha(0.55)),
            TextLayout::new_with_justify(Justify::Center),
            ScreenAnchor::new(fx, fy, 0.0, 0.0),
            Transform::from_xyz(0.0, 0.0, 61.0),
            Visibility::Hidden,
        ));
    }
}

/// Center of the dash zone as a screen anchor. A zone spanning [F, 1] of
/// the window has its center at anchor frac F on x — and -F on y, since
/// the zone fraction counts y-down while anchors count y-up.
fn dash_anchor() -> ScreenAnchor {
    ScreenAnchor::new(
        input_touch::DASH_ZONE_X_FRAC,
        -input_touch::DASH_ZONE_Y_FRAC,
        0.0,
        0.0,
    )
}

/// Dash reads brightest — it's the one control that must be findable blind.
fn dash_idle() -> Color {
    render::palette::SPARK.with_alpha(0.55)
}

fn dash_active() -> Color {
    render::scale_color(render::palette::SPARK, 1.4).with_alpha(0.95)
}

/// Screen px (top-left origin, y-down) to world, through the same visible
/// rect the anchor pass computed this frame.
fn screen_to_world(p: Vec2, win: Vec2, rect: &ViewRect) -> Vec2 {
    let frac = Vec2::new(p.x / win.x * 2.0 - 1.0, 1.0 - p.y / win.y * 2.0);
    rect.center + rect.half * frac
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn update_controls(
    shown: Res<TouchControlsShown>,
    screen: Res<State<AppScreen>>,
    awaiting: Res<AwaitingPeer>,
    match_state: Res<sim::MatchState>,
    touch: Res<TouchState>,
    window: Res<WindowSize>,
    rect: Res<ViewRect>,
    time: Res<Time<Real>>,
    local_handle: Res<crate::netplay::LocalPlayerHandle>,
    sim_players: Query<(&sim::Player, &sim::ThrowCharge, &sim::DashState)>,
    booms: Query<(&sim::Boomerang, &sim::BoomerangMods)>,
    mut hints_used: ResMut<HintsUsed>,
    mut parts: Query<(&Part, &mut Sprite, &mut Visibility, &mut Transform)>,
    mut labels: Query<(&mut TextColor, &mut Visibility), (With<DashLabel>, Without<Part>)>,
    mut hints: Query<(&HintZone, &mut Visibility), (Without<Part>, Without<DashLabel>)>,
) {
    if !shown.0 {
        return;
    }
    // Only during live play — not the title, the pre-peer waiting room, or
    // the match-over summary (the controls were overlapping the win text).
    let over = matches!(*match_state, sim::MatchState::MatchOver);
    let live = *screen.get() == AppScreen::InMatch && !awaiting.0 && !over;
    let win = window.0;
    let ready = live && win.x > 0.0 && win.y > 0.0 && rect.half != Vec2::ZERO;

    // The hold is LIVE when it's doing something: a charge armed (winding
    // up) or a primary fang out (recall / steer). An inert hold — thumb kept
    // down through the catch — shows a dead-dim ring and no ember handoff,
    // matching what sim actually does with those inputs (nothing, until a
    // fresh press re-arms).
    let local = local_handle.0.unwrap_or(0);
    let local_sim = sim_players.iter().find(|(p, ..)| p.handle == local);
    let charge_armed = local_sim.is_some_and(|(_, c, _)| c.0 > 0);
    let dash_state = local_sim.map(|(_, _, d)| *d);
    let fang_out = booms
        .iter()
        .any(|(b, m)| b.owner_handle == local && !m.is_secondary);
    let live_hold = charge_armed || fang_out;

    // Graduate the onboarding hints the first time each control is used
    // in live play (title-screen taps don't count).
    if live {
        if touch.stick_touch.is_some() {
            hints_used.moved = true;
        }
        if touch.throw_held {
            hints_used.threw = true;
        }
        if touch.taunt_held {
            hints_used.taunted = true;
        }
    }

    // Move stick pose: (base, knob) world positions. The base ring is drawn
    // at the SATURATION circle — the knob rides its rim at full deflection,
    // so the picture tells the truth about the input.
    let sat_px = STICK_MAX_RADIUS_PX * STICK_DEADZONE_SATURATION;
    let move_pose = ready
        .then(|| touch.stick_touch.and_then(|id| touch.find(id)))
        .flatten()
        .map(|t| {
            let delta = t.current_pos - t.start_pos;
            let clamped = if delta.length() > sat_px {
                delta * (sat_px / delta.length())
            } else {
                delta
            };
            (
                screen_to_world(t.start_pos, win, &rect),
                screen_to_world(t.start_pos + clamped, win, &rect),
            )
        });
    // Throw button pose: a ring tracking the right thumb (no drag mechanics
    // — the aim lives on the left stick).
    let throw_pos = ready
        .then(|| touch.right_touch.and_then(|id| touch.find(id)))
        .flatten()
        .map(|t| screen_to_world(t.current_pos, win, &rect));

    // Logical px to world units, so the rings match the finger's real
    // travel on every device (and under the kill-cam zoom).
    let world_per_px = if win.x > 0.0 {
        rect.half.x * 2.0 / win.x
    } else {
        1.0
    };
    let base_size = 2.0 * sat_px * world_per_px;

    // A slow breath on the dash ring so it draws the eye until first used.
    let pulse = 0.75 + 0.25 * (time.elapsed_secs() * 2.5).sin();

    for (part, mut sprite, mut vis, mut tx) in &mut parts {
        match *part {
            Part::MoveBase | Part::MoveKnob => {
                let Some((base, knob)) = move_pose else {
                    *vis = Visibility::Hidden;
                    continue;
                };
                *vis = Visibility::Visible;
                let is_knob = *part == Part::MoveKnob;
                let p = if is_knob { knob } else { base };
                tx.translation.x = p.x;
                tx.translation.y = p.y;
                let d = if is_knob { base_size * KNOB_FRAC } else { base_size };
                sprite.custom_size = Some(Vec2::splat(d));
                // While a LIVE throw hold is down, this stick IS the aim —
                // it heats up ember so the handoff reads at a glance. An
                // inert hold leaves it in movement colors: the stick really
                // is just walking.
                let aiming_hand =
                    (touch.throw_held || touch.aim_release_sticky) && live_hold;
                sprite.color = match (is_knob, aiming_hand) {
                    (false, false) => render::palette::COLD_STONE.with_alpha(0.35),
                    (false, true) => render::palette::EMBER.with_alpha(0.55),
                    (true, false) => render::palette::HOT_BONE.with_alpha(0.85),
                    (true, true) => render::palette::EMBER.with_alpha(0.95),
                };
            }
            Part::ThrowRing => {
                let Some(p) = throw_pos else {
                    *vis = Visibility::Hidden;
                    continue;
                };
                *vis = Visibility::Visible;
                tx.translation.x = p.x;
                tx.translation.y = p.y;
                tx.rotation = Quat::from_rotation_z(time.elapsed_secs() * 1.2);
                sprite.custom_size = Some(Vec2::splat(base_size * THROW_RING_FRAC));
                // Bright while aiming a live hold, warm while charging or
                // recalling, near-dead when the hold is inert (lift to re-arm).
                sprite.color = if !live_hold {
                    render::palette::EMBER.with_alpha(0.22)
                } else if touch.aim_active {
                    render::palette::EMBER.with_alpha(0.9)
                } else {
                    render::palette::EMBER.with_alpha(0.5)
                };
            }
            Part::Dash => {
                *vis = if live {
                    Visibility::Visible
                } else {
                    Visibility::Hidden
                };
                tx.rotation = Quat::from_rotation_z(time.elapsed_secs() * 0.6);
                match dash_state {
                    // Refilling: dim and shrunken, growing back to full size
                    // as it recharges — the ring itself is the cooldown meter.
                    Some(sim::DashState::Cooldown { frames_remaining }) => {
                        let f = 1.0
                            - frames_remaining as f32 / sim::DASH_COOLDOWN_FRAMES as f32;
                        sprite.color = render::palette::COLD_STONE.with_alpha(0.25 + 0.2 * f);
                        sprite.custom_size =
                            Some(Vec2::splat(DASH_RING_SIZE * (0.7 + 0.3 * f)));
                    }
                    Some(sim::DashState::Dashing { .. }) => {
                        sprite.color = dash_active();
                        sprite.custom_size = Some(Vec2::splat(DASH_RING_SIZE * 1.12));
                    }
                    // Ready: the spark pulse; a tiny scale kick while held so
                    // the tap registers visually.
                    _ => {
                        let on = touch.dash_held;
                        sprite.color = if on {
                            dash_active()
                        } else {
                            dash_idle().with_alpha(0.55 * pulse)
                        };
                        sprite.custom_size =
                            Some(Vec2::splat(DASH_RING_SIZE * if on { 1.12 } else { 1.0 }));
                    }
                }
            }
        }
    }
    for (mut color, mut vis) in &mut labels {
        *vis = if live {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        color.0 = match dash_state {
            Some(sim::DashState::Cooldown { .. }) => {
                render::palette::COLD_STONE.with_alpha(0.35)
            }
            Some(sim::DashState::Dashing { .. }) => dash_active(),
            _ if touch.dash_held => dash_active(),
            _ => dash_idle(),
        };
    }
    for (zone, mut vis) in &mut hints {
        let used = match zone {
            HintZone::Move => hints_used.moved,
            HintZone::Throw => hints_used.threw,
            HintZone::Taunt => hints_used.taunted,
        };
        *vis = if live && !used {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}
