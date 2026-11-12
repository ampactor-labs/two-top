//! The arena roster — every map choosable, from a screen that shows them.
//!
//! Before this screen the arena picker was a text line that cycled blind on
//! tap, couch-only, keys 1-3 covering three of seven arenas. Now: tap the
//! arena line on the Title (couch OR online — online, the pair agrees on a
//! table after pairing, see `netplay::agreed_table`) and the roster opens: seven rows, each with its real
//! floor as the thumbnail, its name, and one line of the rules that make it
//! that arena. Tap a row to pick it; the pick persists in the settings JSON
//! and (online) is what you offer the table handshake.

use bevy::prelude::*;
use input_touch::WindowSize;
use sim::{ALL_ARENAS, ArenaId, SelectedArena};

use crate::anchor::ScreenAnchor;
use crate::screen::AppScreen;

/// Rows band (window-fraction, y-down).
const LIST_TOP: f32 = 0.14;
const LIST_PITCH: f32 = 0.085;
/// The BACK band at the bottom — same gesture as every menu.
const BACK_BAND: crate::ui::Band = crate::ui::BACK;

/// Everything this screen spawns, for one-query teardown.
#[derive(Component)]
struct RosterUi;

/// A row's name text, tagged with its arena so selection can recolor it.
#[derive(Component)]
struct RosterRowName(ArenaId);

fn row_anchor_y(i: usize) -> f32 {
    let fy = LIST_TOP + (i as f32 + 0.5) * LIST_PITCH;
    crate::ui::anchor_y(fy)
}

fn enter_roster(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.spawn((
        RosterUi,
        Text2d::new("THE TABLES"),
        TextFont {
            font_size: 72.0,
            ..default()
        },
        TextColor(render::palette::HOT_BONE),
        TextLayout::new_with_justify(Justify::Center),
        ScreenAnchor::new(0.0, crate::ui::anchor_y(0.07), 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, 210.0),
    ));
    for (i, &arena) in ALL_ARENAS.iter().enumerate() {
        let ay = row_anchor_y(i);
        // The floor itself is the thumbnail — no separate preview art to
        // drift out of date.
        commands.spawn((
            RosterUi,
            Sprite {
                image: asset_server.load(render::arena::info(arena).floor),
                custom_size: Some(Vec2::new(58.0, 87.0)),
                ..default()
            },
            ScreenAnchor::new(-0.66, ay, 0.0, 0.0),
            Transform::from_xyz(0.0, 0.0, 210.0),
        ));
        commands.spawn((
            RosterUi,
            RosterRowName(arena),
            Text2d::new(render::arena::info(arena).title),
            TextFont {
                font_size: 44.0,
                ..default()
            },
            TextColor(render::palette::BONE),
            TextLayout::new_with_justify(Justify::Center),
            ScreenAnchor::new(0.10, ay + 0.030, 0.0, 0.0),
            Transform::from_xyz(0.0, 0.0, 210.0),
        ));
        commands.spawn((
            RosterUi,
            Text2d::new(render::arena::info(arena).rule),
            TextFont {
                font_size: 26.0,
                ..default()
            },
            TextColor(render::palette::BONE.with_alpha(0.6)),
            TextLayout::new_with_justify(Justify::Center),
            ScreenAnchor::new(0.10, ay - 0.032, 0.0, 0.0),
            Transform::from_xyz(0.0, 0.0, 210.0),
        ));
    }
    // BACK, in the shared bordered-box language.
    let back_y = crate::ui::band_anchor_y(BACK_BAND);
    for (role, size, z) in [
        (0, Vec2::new(362.0, 98.0), 209.0),
        (1, Vec2::new(340.0, 76.0), 209.5),
    ] {
        commands.spawn((
            RosterUi,
            Sprite {
                color: if role == 0 {
                    render::palette::HOT_BONE
                } else {
                    render::palette::DEEP_ASH
                },
                custom_size: Some(size),
                ..default()
            },
            ScreenAnchor::new(0.0, back_y, 0.0, 0.0),
            Transform::from_xyz(0.0, 0.0, z),
        ));
    }
    commands.spawn((
        RosterUi,
        Text2d::new("BACK"),
        TextFont {
            font_size: 40.0,
            ..default()
        },
        TextColor(render::palette::HOT_BONE),
        TextLayout::new_with_justify(Justify::Center),
        ScreenAnchor::new(0.0, back_y, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, 210.0),
    ));
}

fn exit_roster(mut commands: Commands, q: Query<Entity, With<RosterUi>>) {
    for e in &q {
        commands.entity(e).despawn();
    }
}

/// The selected row reads hot; the rest stay bone.
fn update_roster_selection(
    selected: Res<SelectedArena>,
    mut names: Query<(&RosterRowName, &mut TextColor)>,
) {
    for (row, mut color) in &mut names {
        color.0 = if row.0 == selected.0 {
            render::palette::HOT_BONE
        } else {
            render::palette::BONE
        };
    }
}

/// Tap a row (or 1-7) to pick; BACK / Escape leaves. A pick persists to the
/// settings JSON and returns to the Title — where PLAY (couch, practice, or
/// the summons) uses it.
fn roster_input(
    keys: Res<ButtonInput<KeyCode>>,
    touches: Res<Touches>,
    window: Res<WindowSize>,
    mut selected: ResMut<SelectedArena>,
    mut settings: ResMut<crate::settings::Settings>,
    mut next: ResMut<NextState<AppScreen>>,
) {
    let mut pick: Option<ArenaId> = None;
    for (i, key) in [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
    ]
    .into_iter()
    .enumerate()
    {
        if keys.just_pressed(key) {
            pick = Some(ALL_ARENAS[i]);
        }
    }
    let win = window.0;
    if win.y > 0.0 {
        for t in touches.iter_just_pressed() {
            let fy = t.position().y / win.y;
            if crate::ui::in_band(fy, BACK_BAND) {
                next.set(AppScreen::Title);
                return;
            }
            if fy >= LIST_TOP {
                let row = ((fy - LIST_TOP) / LIST_PITCH) as usize;
                if row < ALL_ARENAS.len() {
                    pick = Some(ALL_ARENAS[row]);
                }
            }
        }
    }
    if keys.just_pressed(KeyCode::Escape) {
        next.set(AppScreen::Title);
        return;
    }
    if let Some(arena) = pick {
        selected.0 = arena;
        settings.arena = arena.as_u8();
        crate::settings::persist(&settings);
        next.set(AppScreen::Title);
    }
}

/// Startup: restore the persisted pick (TWOTOP_ARENA overrides for capture
/// verification — a name or a wire id).
fn restore_arena_pick(
    settings: Res<crate::settings::Settings>,
    mut selected: ResMut<SelectedArena>,
) {
    let from_env = std::env::var("TWOTOP_ARENA").ok().and_then(|v| {
        let v = v.trim().to_ascii_lowercase();
        if let Ok(n) = v.parse::<u8>() {
            return Some(ArenaId::from_u8(n));
        }
        ALL_ARENAS.iter().copied().find(|a| {
            render::arena::info(*a)
                .title
                .to_ascii_lowercase()
                .contains(&v)
        })
    });
    selected.0 = from_env.unwrap_or_else(|| ArenaId::from_u8(settings.arena));
}

pub struct ArenaSelectPlugin;

impl Plugin for ArenaSelectPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, restore_arena_pick)
            .add_systems(OnEnter(AppScreen::ArenaSelect), enter_roster)
            .add_systems(OnExit(AppScreen::ArenaSelect), exit_roster)
            .add_systems(
                Update,
                (roster_input, update_roster_selection).run_if(in_state(AppScreen::ArenaSelect)),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_map_taps_back_to_arenas() {
        // The tap decode in roster_input mirrors this arithmetic; pin the
        // band math so a pitch tweak can't silently misalign row hits.
        for i in 0..ALL_ARENAS.len() {
            let fy = LIST_TOP + (i as f32 + 0.5) * LIST_PITCH;
            let row = ((fy - LIST_TOP) / LIST_PITCH) as usize;
            assert_eq!(row, i);
            assert!(fy < BACK_BAND.0, "row {i} must not reach the BACK band");
        }
    }
}
