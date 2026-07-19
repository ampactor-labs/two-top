//! The arena roster — every map choosable, from a screen that shows them.
//!
//! Before this screen the arena picker was a text line that cycled blind on
//! tap, couch-only, keys 1-3 covering three of seven arenas. Now: tap the
//! arena line on the Title (couch OR online — the pick rides the room name,
//! see `room_code`) and the roster opens: seven rows, each with its real
//! floor as the thumbnail, its name, and one line of the rules that make it
//! that arena. Tap a row to pick it; the pick persists in the settings JSON
//! and (online) becomes part of the room you summon in.

use bevy::prelude::*;
use input_touch::WindowSize;
use sim::{ALL_ARENAS, ArenaId, SelectedArena};

use crate::anchor::ScreenAnchor;
use crate::screen::AppScreen;

/// Rows band (window-fraction, y-down).
const LIST_TOP: f32 = 0.14;
const LIST_PITCH: f32 = 0.085;
/// The BACK band at the bottom — same gesture as every menu.
const BACK_BAND: (f32, f32) = (0.86, 0.96);

pub fn arena_title(id: ArenaId) -> &'static str {
    match id {
        ArenaId::Anchor => "ANCHOR",
        ArenaId::Crossing => "CROSSING",
        ArenaId::Reliquary => "RELIQUARY",
        ArenaId::Pit => "THE PIT",
        ArenaId::Vigil => "THE VIGIL",
        ArenaId::Gallery => "THE GALLERY",
        ArenaId::Forest => "THE FOREST",
    }
}

/// One line of the rules that make the arena itself — the roster is where
/// a player learns the roster's verbs, so the tags say mechanics, not mood.
pub fn arena_rule(id: ArenaId) -> &'static str {
    match id {
        ArenaId::Anchor => "four crates - one pyre",
        ArenaId::Crossing => "a moat between you - ring a sigil to bridge it",
        ArenaId::Reliquary => "doors teleport - pyres chain",
        ArenaId::Pit => "walls ricochet - nothing falls",
        ArenaId::Vigil => "no storm - open ground",
        ArenaId::Gallery => "corridors of angles",
        ArenaId::Forest => "the grove burns",
    }
}

fn floor_asset(id: ArenaId) -> &'static str {
    match id {
        ArenaId::Anchor => "arenas/anchor_floor.png",
        ArenaId::Crossing => "arenas/crossing_floor.png",
        ArenaId::Reliquary => "arenas/reliquary_floor.png",
        ArenaId::Pit => "arenas/pit_floor.png",
        ArenaId::Vigil => "arenas/vigil_floor.png",
        ArenaId::Gallery => "arenas/gallery_floor.png",
        ArenaId::Forest => "arenas/forest_floor.png",
    }
}

/// Everything this screen spawns, for one-query teardown.
#[derive(Component)]
struct RosterUi;

/// A row's name text, tagged with its arena so selection can recolor it.
#[derive(Component)]
struct RosterRowName(ArenaId);

fn row_anchor_y(i: usize) -> f32 {
    let fy = LIST_TOP + (i as f32 + 0.5) * LIST_PITCH;
    1.0 - 2.0 * fy
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
        ScreenAnchor::new(0.0, 1.0 - 2.0 * 0.07, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, 210.0),
    ));
    for (i, &arena) in ALL_ARENAS.iter().enumerate() {
        let ay = row_anchor_y(i);
        // The floor itself is the thumbnail — no separate preview art to
        // drift out of date.
        commands.spawn((
            RosterUi,
            Sprite {
                image: asset_server.load(floor_asset(arena)),
                custom_size: Some(Vec2::new(58.0, 87.0)),
                ..default()
            },
            ScreenAnchor::new(-0.66, ay, 0.0, 0.0),
            Transform::from_xyz(0.0, 0.0, 210.0),
        ));
        commands.spawn((
            RosterUi,
            RosterRowName(arena),
            Text2d::new(arena_title(arena)),
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
            Text2d::new(arena_rule(arena)),
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
    let back_y = 1.0 - (BACK_BAND.0 + BACK_BAND.1);
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
            if in_band(fy, BACK_BAND) {
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

fn in_band(y_down: f32, band: (f32, f32)) -> bool {
    y_down >= band.0 && y_down < band.1
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
        ALL_ARENAS
            .iter()
            .copied()
            .find(|a| arena_title(*a).to_ascii_lowercase().contains(&v))
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
    fn every_arena_has_a_row_worth_of_copy() {
        for &a in sim::ALL_ARENAS.iter() {
            assert!(!arena_title(a).is_empty());
            assert!(
                arena_rule(a).len() < 48,
                "rule tag for {:?} must fit one phone row",
                a
            );
            assert!(
                arena_rule(a)
                    .chars()
                    .all(|c| c.is_ascii() && c != '\u{2014}'),
                "ASCII only — the bundled font renders tofu otherwise"
            );
        }
    }

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
