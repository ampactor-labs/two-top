//! Local identity — the install-id and your four-letter name.
//!
//! matchbox `PeerId`s are ephemeral (fresh every connection), so anything
//! that should survive across matches — the per-opponent grudge ledger,
//! names on the summary — needs a durable identity. This is it: a random
//! u128 minted once per install, plus a name the wire carries as four
//! alphabet indices (`NetMsg::Profile`, exchanged right after the P2P
//! swap).
//!
//! The name is four letters on a QWERTY keyboard: type it, DONE. It used
//! to tap-cycle the room code's 7-glyph wheel, which is why phones went
//! out into the world named GSAA — nobody dials to fix a letter, so nobody
//! did. A fresh install still gets a name dealt from its install-id so the
//! wire is never empty, and first boot opens the keyboard once so the first
//! thing you do is claim it. (The ROOM CODE keeps the 7-glyph wheel: a
//! friend's code should be four quick cycles, and it is never read as a
//! word.)
//!
//! The identity is only durable if it is actually written down — see
//! `crate::paths`, which is where that went wrong for the whole life of
//! this module on Android.

use bevy::prelude::*;
use input_touch::WindowSize;
use net::{NAME_LEN, ProfileData};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::anchor::ScreenAnchor;
use crate::netplay::NetplayConfig;
use crate::screen::AppScreen;

/// The name wheel: the whole alphabet, because a name is meant to be read.
/// This is the WIRE order (a name is four indices into it) — never reorder
/// it, or every peer's name renders wrong. The on-screen layout is a
/// separate thing entirely; see [`KEY_ROWS`].
pub const NAME_ALPHABET: [char; 26] = [
    'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R',
    'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z',
];

/// The keyboard, laid out the way every phone on earth lays one out.
/// Alphabetical order is correct for a wire format and wrong for a thumb:
/// nobody has hunted for a letter in an A-B-C grid since the arcade, and
/// muscle memory for QWERTY is universal. Rows are ragged (10/9/7) exactly
/// like the real thing, so the shape itself says "keyboard".
const KEY_ROWS: [&str; 3] = ["QWERTYUIOP", "ASDFGHJKL", "ZXCVBNM"];

// ---- Title: your name over your demon's head ----
/// Tap anywhere on your demon (or its name) to re-enter the grid.
const NAME_TAP_RECT: (f32, f32) = (0.30, 0.50);
const NAME_ANCHOR_Y: f32 = 1.0 - 2.0 * 0.335;

// ---- NameEntry screen (window-fraction, y-down) ----
/// The four big letters you're editing.
const ENTRY_ANCHOR_Y: f32 = 1.0 - 2.0 * 0.30;
const ENTRY_CELL_DX: f32 = 0.11;
/// The keyboard: three ragged rows low on the screen where a phone
/// keyboard lives, plus a DEL/DONE row under it.
const KEY_TOP: f32 = 0.50;
const KEY_ROW_H: f32 = 0.085;
/// A key's width, sized so the widest row (10 keys) spans the screen with
/// a margin — every row centers itself inside this pitch.
const KEY_W: f32 = 0.094;
/// The action row (DEL / DONE) sits one row below the letters.
const ACTION_ROW_Y: f32 = KEY_TOP + 3.0 * KEY_ROW_H + 0.045;
const ACTION_BAND: (f32, f32) = (ACTION_ROW_Y - 0.045, ACTION_ROW_Y + 0.045);

/// Where a key sits: its window-fraction center. Rows are centered, so a
/// 7-key row is inset from a 10-key row exactly like a real keyboard.
fn key_center(row: usize, col: usize) -> (f32, f32) {
    let len = KEY_ROWS[row].chars().count() as f32;
    let fx = 0.5 + (col as f32 - (len - 1.0) * 0.5) * KEY_W;
    let fy = KEY_TOP + (row as f32 + 0.5) * KEY_ROW_H;
    (fx, fy)
}

/// The letter at a (row, col), as an index into [`NAME_ALPHABET`].
fn key_letter_index(row: usize, col: usize) -> Option<u8> {
    let ch = KEY_ROWS.get(row)?.chars().nth(col)?;
    NAME_ALPHABET.iter().position(|c| *c == ch).map(|i| i as u8)
}

/// Hit-test a tap against the keyboard: `Some(alphabet index)`.
fn key_at(fx: f32, fy: f32) -> Option<u8> {
    if fy < KEY_TOP {
        return None;
    }
    let row = ((fy - KEY_TOP) / KEY_ROW_H) as usize;
    let keys = KEY_ROWS.get(row)?;
    let len = keys.chars().count() as f32;
    // Invert `key_center`: the row's leftmost key starts here.
    let left = 0.5 - len * 0.5 * KEY_W;
    if fx < left {
        return None;
    }
    let col = ((fx - left) / KEY_W) as usize;
    key_letter_index(row, col)
}

#[derive(Resource, Serialize, Deserialize, Clone, Copy, Debug)]
#[serde(default)]
pub struct LocalProfile {
    pub install_id: u128,
    pub slots: [u8; NAME_LEN],
    /// Has the player actually claimed this name? False on a fresh install
    /// (the dealt name is a placeholder), which opens the grid once.
    pub named: bool,
}

impl Default for LocalProfile {
    fn default() -> Self {
        Self {
            install_id: 0,
            slots: [0; NAME_LEN],
            named: false,
        }
    }
}

impl LocalProfile {
    pub fn as_data(&self) -> ProfileData {
        ProfileData {
            install_id: self.install_id,
            name: self.slots,
        }
    }

    pub fn name_string(&self) -> String {
        name_from_slots(&self.slots)
    }
}

/// Render alphabet indices into a name, clamping every index (a peer's
/// bytes are untrusted; the worst a wild value does is show a wrong
/// letter).
pub fn name_from_slots(slots: &[u8; NAME_LEN]) -> String {
    slots
        .iter()
        .map(|&i| NAME_ALPHABET[i as usize % NAME_ALPHABET.len()])
        .collect()
}

/// A fresh install's placeholder name: letters dealt from its install-id,
/// so two strangers almost never boot up with the same one.
fn default_slots(install_id: u128) -> [u8; NAME_LEN] {
    let mut slots = [0u8; NAME_LEN];
    for (i, slot) in slots.iter_mut().enumerate() {
        *slot = ((install_id >> (i * 8)) as u8) % NAME_ALPHABET.len() as u8;
    }
    slots
}

fn profile_path() -> Option<PathBuf> {
    crate::paths::config_file("profile.json")
}

fn load_profile() -> LocalProfile {
    let mut profile: LocalProfile = profile_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    for slot in &mut profile.slots {
        *slot %= NAME_ALPHABET.len() as u8;
    }
    if profile.install_id == 0 {
        // First boot (or a hand-wiped file): mint the durable identity.
        profile.install_id = uuid::Uuid::new_v4().as_u128();
        profile.slots = default_slots(profile.install_id);
        profile.named = false;
        save_profile(&profile);
        tracing::info!(
            target: "two_top::profile",
            name = %profile.name_string(),
            "minted install identity",
        );
    }
    profile
}

fn save_profile(profile: &LocalProfile) {
    let Some(path) = profile_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(profile) {
        let _ = std::fs::write(&path, json);
    }
}

/// Which of the four letters the grid is filling.
#[derive(Resource, Default)]
struct NameCursor(usize);

/// Your name over your demon on the title.
#[derive(Component)]
struct TitleName;

/// One of the four big letters on the entry screen.
#[derive(Component)]
struct EntryCell(usize);

/// One key of the keyboard: a letter at (row, col), or an action.
#[derive(Component, Clone, Copy, PartialEq)]
enum GridKey {
    Letter { row: usize, col: usize },
    Del,
    Done,
}

fn spawn_name_ui(mut commands: Commands, netplay: Res<NetplayConfig>) {
    // Couch builds carry no identity to exchange; the name is online-only.
    if netplay.room_url.is_none() {
        return;
    }
    commands.spawn((
        TitleName,
        Text2d::new(String::new()),
        TextFont {
            font_size: 44.0,
            ..default()
        },
        TextColor(render::palette::HOT_BONE),
        TextLayout::new_with_justify(Justify::Center),
        ScreenAnchor::new(0.0, NAME_ANCHOR_Y, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, 201.0),
        Visibility::Hidden,
    ));
    for i in 0..NAME_LEN {
        let fx = 0.5 + (i as f32 - (NAME_LEN as f32 - 1.0) * 0.5) * ENTRY_CELL_DX;
        commands.spawn((
            EntryCell(i),
            Text2d::new(String::new()),
            TextFont {
                font_size: 96.0,
                ..default()
            },
            TextColor(render::palette::HOT_BONE),
            TextLayout::new_with_justify(Justify::Center),
            ScreenAnchor::new(fx * 2.0 - 1.0, ENTRY_ANCHOR_Y, 0.0, 0.0),
            Transform::from_xyz(0.0, 0.0, 201.0),
            Visibility::Hidden,
        ));
    }
    let mut key = |k: GridKey, fx: f32, fy: f32, size: f32, color: Color| {
        commands.spawn((
            k,
            Text2d::new(String::new()),
            TextFont {
                font_size: size,
                ..default()
            },
            TextColor(color),
            TextLayout::new_with_justify(Justify::Center),
            ScreenAnchor::new(fx * 2.0 - 1.0, 1.0 - 2.0 * fy, 0.0, 0.0),
            Transform::from_xyz(0.0, 0.0, 201.0),
            Visibility::Hidden,
        ));
    };
    for (row, keys) in KEY_ROWS.iter().enumerate() {
        for col in 0..keys.chars().count() {
            let (fx, fy) = key_center(row, col);
            key(
                GridKey::Letter { row, col },
                fx,
                fy,
                46.0,
                render::palette::BONE,
            );
        }
    }
    key(GridKey::Del, 0.28, ACTION_ROW_Y, 34.0, render::palette::COLD_STONE);
    key(GridKey::Done, 0.72, ACTION_ROW_Y, 40.0, render::palette::SPARK);
}

/// First boot opens the grid once: the first thing you do in 2-Top is
/// claim a name, exactly like putting your initials in.
fn open_name_entry_on_first_boot(
    netplay: Res<NetplayConfig>,
    profile: Res<LocalProfile>,
    screen: Res<State<AppScreen>>,
    mut next: ResMut<NextState<AppScreen>>,
    mut done: Local<bool>,
) {
    if *done || profile.named || netplay.room_url.is_none() {
        return;
    }
    if *screen.get() != AppScreen::Title {
        return;
    }
    *done = true;
    next.set(AppScreen::NameEntry);
}

/// Tapping your demon (or its name) on the title reopens the grid.
fn title_name_input(
    keys: Res<ButtonInput<KeyCode>>,
    touches: Res<Touches>,
    window: Res<WindowSize>,
    netplay: Res<NetplayConfig>,
    mut next: ResMut<NextState<AppScreen>>,
) {
    if netplay.room_url.is_none() {
        return;
    }
    let win = window.0;
    let tapped = win.y > 0.0
        && touches.iter_just_pressed().any(|t| {
            let fy = t.position().y / win.y;
            (NAME_TAP_RECT.0..NAME_TAP_RECT.1).contains(&fy)
        });
    if tapped || keys.just_pressed(KeyCode::KeyN) {
        next.set(AppScreen::NameEntry);
    }
}

/// The grid: tap a letter to set the cursor's slot and advance, DEL to
/// step back, DONE to keep it. Escape leaves it as-is.
fn name_entry_input(
    keys: Res<ButtonInput<KeyCode>>,
    touches: Res<Touches>,
    window: Res<WindowSize>,
    mut profile: ResMut<LocalProfile>,
    mut cursor: ResMut<NameCursor>,
    mut next: ResMut<NextState<AppScreen>>,
) {
    let mut hit: Option<GridKey> = None;
    let mut letter: Option<u8> = None;
    let win = window.0;
    if win.x > 0.0 && win.y > 0.0 {
        for t in touches.iter_just_pressed() {
            let p = t.position();
            let (fx, fy) = (p.x / win.x, p.y / win.y);
            if (ACTION_BAND.0..ACTION_BAND.1).contains(&fy) {
                hit = Some(if fx < 0.5 { GridKey::Del } else { GridKey::Done });
            } else if let Some(i) = key_at(fx, fy) {
                letter = Some(i);
            }
        }
    }
    // Desktop: type the name, backspace, enter.
    for (i, ch) in NAME_ALPHABET.iter().enumerate() {
        let key = match ch {
            'A' => KeyCode::KeyA,
            'B' => KeyCode::KeyB,
            'C' => KeyCode::KeyC,
            'D' => KeyCode::KeyD,
            'E' => KeyCode::KeyE,
            'F' => KeyCode::KeyF,
            'G' => KeyCode::KeyG,
            'H' => KeyCode::KeyH,
            'I' => KeyCode::KeyI,
            'J' => KeyCode::KeyJ,
            'K' => KeyCode::KeyK,
            'L' => KeyCode::KeyL,
            'M' => KeyCode::KeyM,
            'N' => KeyCode::KeyN,
            'O' => KeyCode::KeyO,
            'P' => KeyCode::KeyP,
            'Q' => KeyCode::KeyQ,
            'R' => KeyCode::KeyR,
            'S' => KeyCode::KeyS,
            'T' => KeyCode::KeyT,
            'U' => KeyCode::KeyU,
            'V' => KeyCode::KeyV,
            'W' => KeyCode::KeyW,
            'X' => KeyCode::KeyX,
            'Y' => KeyCode::KeyY,
            _ => KeyCode::KeyZ,
        };
        if keys.just_pressed(key) {
            letter = Some(i as u8);
        }
    }
    if keys.just_pressed(KeyCode::Backspace) {
        hit = Some(GridKey::Del);
    }
    if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::Escape) {
        hit = Some(GridKey::Done);
    }

    if let Some(i) = letter {
        let slot = cursor.0;
        profile.slots[slot] = i;
        cursor.0 = (slot + 1) % NAME_LEN;
        return;
    }
    match hit {
        Some(GridKey::Done) => {
            profile.named = true;
            save_profile(&profile);
            cursor.0 = 0;
            next.set(AppScreen::Title);
        }
        Some(GridKey::Del) => cursor.0 = (cursor.0 + NAME_LEN - 1) % NAME_LEN,
        _ => {}
    }
}

/// Render the title name and the entry screen.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn update_name_ui(
    screen: Res<State<AppScreen>>,
    netplay: Res<NetplayConfig>,
    profile: Res<LocalProfile>,
    cursor: Res<NameCursor>,
    time: Res<Time<Real>>,
    mut title: Query<
        (&mut Text2d, &mut Visibility),
        (With<TitleName>, Without<EntryCell>, Without<GridKey>),
    >,
    mut entry: Query<
        (&EntryCell, &mut Text2d, &mut TextColor, &mut Visibility),
        (Without<TitleName>, Without<GridKey>),
    >,
    mut grid: Query<
        (&GridKey, &mut Text2d, &mut TextColor, &mut Visibility),
        (Without<TitleName>, Without<EntryCell>),
    >,
) {
    let online = netplay.room_url.is_some();
    let on_title = *screen.get() == AppScreen::Title && online;
    let entering = *screen.get() == AppScreen::NameEntry && online;
    let name = profile.name_string();

    if let Ok((mut text, mut vis)) = title.single_mut() {
        *vis = if on_title {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        text.0 = name.clone();
    }
    // The cursor slot blinks, the way initials entry has always blinked.
    let blink = (time.elapsed_secs() * 3.0).fract() < 0.6;
    for (cell, mut text, mut color, mut vis) in &mut entry {
        *vis = if entering {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        if !entering {
            continue;
        }
        let at_cursor = cell.0 == cursor.0;
        text.0 = name.chars().nth(cell.0).map(String::from).unwrap_or_default();
        color.0 = if at_cursor && blink {
            render::palette::SPARK
        } else if at_cursor {
            render::palette::HOT_BONE.with_alpha(0.35)
        } else {
            render::palette::HOT_BONE
        };
    }
    for (key, mut text, mut color, mut vis) in &mut grid {
        *vis = if entering {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        if !entering {
            continue;
        }
        match *key {
            GridKey::Del => {
                text.0 = "DEL".to_string();
                color.0 = render::palette::COLD_STONE;
            }
            GridKey::Done => {
                text.0 = "DONE".to_string();
                color.0 = render::palette::SPARK;
            }
            GridKey::Letter { row, col } => {
                text.0 = KEY_ROWS[row]
                    .chars()
                    .nth(col)
                    .map(String::from)
                    .unwrap_or_default();
                color.0 = render::palette::BONE;
            }
        }
    }
}

pub struct ProfilePlugin;

impl Plugin for ProfilePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(load_profile())
            .init_resource::<NameCursor>()
            .add_systems(Startup, spawn_name_ui)
            .add_systems(
                Update,
                (
                    open_name_entry_on_first_boot.run_if(in_state(AppScreen::Title)),
                    title_name_input.run_if(in_state(AppScreen::Title)),
                    name_entry_input.run_if(in_state(AppScreen::NameEntry)),
                    update_name_ui,
                ),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_render_from_slots_and_clamp_wild_bytes() {
        assert_eq!(name_from_slots(&[0, 1, 2, 3]), "ABCD");
        assert_eq!(name_from_slots(&[12, 14, 17, 6]), "MORG");
        // 26 wraps to 'A', 255 lands somewhere in the alphabet — never a panic.
        assert_eq!(name_from_slots(&[26, 26, 26, 26]), "AAAA");
        assert_eq!(name_from_slots(&[255, 254, 253, 252]).len(), 4);
    }

    #[test]
    fn default_name_is_derived_from_the_install_id() {
        let a = default_slots(0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10);
        let b = default_slots(0xfeed_face_feed_face_feed_face_feed_face);
        for slot in a.iter().chain(b.iter()) {
            assert!((*slot as usize) < NAME_ALPHABET.len());
        }
        assert_ne!(a, b, "different installs deal different names");
    }

    #[test]
    fn a_fresh_profile_is_unclaimed_so_the_grid_opens_once() {
        let fresh = LocalProfile::default();
        assert!(!fresh.named);
        // An older profile.json predates the flag: serde defaults it to
        // false, so those installs get the grid once too. That's the right
        // call — they're the phones named GSAA.
        let old: LocalProfile =
            serde_json::from_str(r#"{"install_id": 7, "slots": [1,2,3,4]}"#).unwrap();
        assert!(!old.named);
        assert_eq!(old.install_id, 7);
    }
}
