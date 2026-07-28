//! Local identity — the install-id, and the name you actually chose.
//!
//! matchbox `PeerId`s are ephemeral (fresh every connection), so anything
//! that should survive across matches — the per-opponent grudge ledger,
//! names on the summary — needs a durable identity. This is it: a random
//! u128 minted once per install, plus a name the wire carries as up to
//! [`NAME_MAX`] alphabet indices (`NetMsg::Profile`, exchanged right after
//! the P2P swap).
//!
//! Type it on a QWERTY keyboard, up to 12 characters of A-Z and 0-9, DONE.
//! It was four glyphs from the room code's CURSTAG wheel once, which is why
//! phones went out into the world named GSAA: nobody tap-cycles a wheel to
//! fix a letter, so nobody ever did. Twelve is the field's own number —
//! Xbox's gamertag ceiling, and the top of the 8-12 band that fits nearly
//! every platform.
//!
//! Names are NOT unique and cannot be (there is no server to enforce it).
//! The install-id carries identity; the name carries personality. Where the
//! two could be confused, `grudge::display_name` borrows the Riot ID shape
//! — see [`identity_tag`]. A fresh install still gets a name dealt from its
//! install-id so the wire is never empty, and first boot opens the keyboard
//! once so the first thing you do is claim one. (The ROOM CODE keeps the
//! 7-glyph wheel: a friend's code should be four quick cycles, and it is
//! never read as a word.)
//!
//! The identity is only durable if it is actually written down — see
//! `crate::paths`, which is where that went wrong for the whole life of
//! this module on Android.

use bevy::prelude::*;
use input_touch::WindowSize;
use net::{NAME_EMPTY, NAME_MAX, ProfileData};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::anchor::ScreenAnchor;
use crate::netplay::NetplayConfig;
use crate::screen::AppScreen;

/// The glyph set a name is built from: A-Z then 0-9. This is the WIRE
/// order (a name is a run of indices into it) — never reorder it, or every
/// peer's name renders wrong. The on-screen layout is a separate thing
/// entirely; see [`KEY_ROWS`].
pub const NAME_ALPHABET: [char; 36] = [
    'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R', 'S',
    'T', 'U', 'V', 'W', 'X', 'Y', 'Z', '0', '1', '2', '3', '4', '5', '6', '7', '8', '9',
];

/// The keyboard, laid out the way every phone on earth lays one out.
/// Alphabetical order is correct for a wire format and wrong for a thumb:
/// nobody has hunted for a letter in an A-B-C grid since the arcade, and
/// muscle memory for QWERTY is universal. Rows are ragged exactly like the
/// real thing, so the shape itself says "keyboard" before you read a key.
const KEY_ROWS: [&str; 4] = ["1234567890", "QWERTYUIOP", "ASDFGHJKL", "ZXCVBNM"];

// ---- Title: your name over your demon's head ----
/// Tap anywhere on your demon (or its name) to re-enter the keyboard.
const NAME_TAP_RECT: (f32, f32) = (0.30, 0.50);
const NAME_ANCHOR_Y: f32 = 1.0 - 2.0 * 0.335;

// ---- NameEntry screen (window-fraction, y-down) ----
/// The name you're typing, with its blinking cursor.
const ENTRY_ANCHOR_Y: f32 = 1.0 - 2.0 * 0.28;
/// The keyboard: four ragged rows low on the screen where a phone keyboard
/// lives, plus a DEL/DONE row under it.
const KEY_TOP: f32 = 0.44;
const KEY_ROW_H: f32 = 0.082;
/// A key's width, sized so the widest row (10 keys) spans the screen with
/// a margin — every row centers itself inside this pitch.
const KEY_W: f32 = 0.094;
/// The action row (DEL / DONE) sits one row below the letters.
const ACTION_ROW_Y: f32 = KEY_TOP + KEY_ROWS.len() as f32 * KEY_ROW_H + 0.04;
const ACTION_BAND: (f32, f32) = (ACTION_ROW_Y - 0.042, ACTION_ROW_Y + 0.042);

/// Where a key sits: its window-fraction center. Rows are centered, so a
/// 7-key row is inset from a 10-key row exactly like a real keyboard.
fn key_center(row: usize, col: usize) -> (f32, f32) {
    let len = KEY_ROWS[row].chars().count() as f32;
    let fx = 0.5 + (col as f32 - (len - 1.0) * 0.5) * KEY_W;
    let fy = KEY_TOP + (row as f32 + 0.5) * KEY_ROW_H;
    (fx, fy)
}

/// The glyph at a (row, col), as an index into [`NAME_ALPHABET`].
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

#[derive(Resource, Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct LocalProfile {
    pub install_id: u128,
    /// Your name, 1..=[`NAME_MAX`] glyphs. Stored as text so the file is
    /// greppable and hand-editable; `slots_from_name` is the total,
    /// clamping conversion to the wire form (unknown characters are simply
    /// dropped, so a fat-fingered edit degrades instead of exploding).
    pub name: String,
    /// Has the player actually claimed this name? False on a fresh install
    /// (the dealt name is a placeholder), which opens the keyboard once.
    pub named: bool,
}

impl LocalProfile {
    pub fn as_data(&self) -> ProfileData {
        ProfileData {
            install_id: self.install_id,
            name: slots_from_name(&self.name),
        }
    }

    pub fn name_string(&self) -> String {
        self.name.clone()
    }
}

/// Render wire glyph indices into text: stop at the first pad byte and
/// clamp every index (a peer's bytes are untrusted; the worst a wild value
/// does is show a wrong letter). Pure conversion — an empty name comes back
/// empty. What to SHOW for an empty one is [`peer_name`]'s business.
pub fn name_from_slots(slots: &[u8; NAME_MAX]) -> String {
    slots
        .iter()
        .take_while(|&&i| i != NAME_EMPTY)
        .map(|&i| NAME_ALPHABET[i as usize % NAME_ALPHABET.len()])
        .collect()
}

/// What to call an opponent before you know anything about them: a peer
/// whose handshake has not landed, or one that sent an empty name, is THE
/// CHALLENGER — never a blank space on the card.
pub fn peer_name(peer: Option<ProfileData>) -> String {
    let name = peer.map(|p| name_from_slots(&p.name)).unwrap_or_default();
    if name.is_empty() {
        "THE CHALLENGER".to_string()
    } else {
        name
    }
}

/// Text → the wire's padded index array. Characters outside the alphabet
/// are dropped; anything past [`NAME_MAX`] is cut.
pub fn slots_from_name(name: &str) -> [u8; NAME_MAX] {
    let mut slots = [NAME_EMPTY; NAME_MAX];
    let mut n = 0;
    for ch in name.chars().flat_map(|c| c.to_uppercase()) {
        if n == NAME_MAX {
            break;
        }
        if let Some(i) = NAME_ALPHABET.iter().position(|a| *a == ch) {
            slots[n] = i as u8;
            n += 1;
        }
    }
    slots
}

/// The identity tag: three glyphs dealt from the install-id, shown only
/// where two rivals would otherwise read as the same person (see
/// `grudge::display_name`).
///
/// This is the Riot ID lesson, adapted. Riot found players burning five
/// minutes and a dozen attempts hunting for an unclaimed name, so they
/// split identity into a free-choice name plus a short tagline that carries
/// the uniqueness. We need that split even more badly — with no server,
/// nothing CAN enforce a unique name — but we get the tag for free instead
/// of asking for it: the install-id is already the durable identity, so the
/// tag is just a readable window onto it. You type MORGAN and never think
/// about it again; the tag appears only on the day you meet a second one.
pub fn identity_tag(install_id: u128) -> String {
    (0..3)
        .map(|i| {
            let shift = i * 6;
            let idx = ((install_id >> shift) & 0x3F) as usize % NAME_ALPHABET.len();
            NAME_ALPHABET[idx]
        })
        .collect()
}

/// A fresh install's placeholder name: glyphs dealt from its install-id,
/// so two strangers almost never boot up with the same one.
fn default_name(install_id: u128) -> String {
    (0..4)
        .map(|i| {
            let idx = ((install_id >> (i * 8)) as u8) as usize % NAME_ALPHABET.len();
            NAME_ALPHABET[idx]
        })
        .collect()
}

fn profile_path() -> Option<PathBuf> {
    crate::paths::config_file("profile.json")
}

fn load_profile() -> LocalProfile {
    let mut profile = profile_path().map(|p| read_profile(&p)).unwrap_or_default();
    // Normalize whatever the file held (hand edits, an older build's
    // 4-glyph name) through the same clamp the wire uses.
    profile.name = name_from_slots(&slots_from_name(&profile.name));
    if !profile.name.is_empty() && profile.install_id != 0 {
        return profile;
    }
    if !profile.name.is_empty() {
        // A name with no identity behind it (hand-edited file): mint one.
        profile.install_id = uuid::Uuid::new_v4().as_u128();
        save_profile(&profile);
        return profile;
    }
    if profile.install_id == 0 {
        // First boot (or a hand-wiped file): mint the durable identity.
        profile.install_id = uuid::Uuid::new_v4().as_u128();
        profile.name = default_name(profile.install_id);
        profile.named = false;
        save_profile(&profile);
        tracing::info!(
            target: "two_top::profile",
            name = %profile.name_string(),
            "minted install identity",
        );
    } else {
        // Identity intact but no name (an older build's file, or a wiped
        // one): deal a placeholder and ask for a real one once.
        profile.name = default_name(profile.install_id);
        profile.named = false;
        save_profile(&profile);
    }
    profile
}

/// Read + parse the profile file. An absent file is a first boot. A file
/// that exists but will not parse is quarantined as a `.corrupt` sibling
/// (evidence a human can hand back) instead of being silently replaced;
/// the identity inside is unrecoverable either way, but the bytes say what
/// happened — and with `paths::write_atomic` on every save, the only way
/// to get here anymore is outside interference (a hand edit, a bad disk).
fn read_profile(path: &std::path::Path) -> LocalProfile {
    let Ok(text) = std::fs::read_to_string(path) else {
        return LocalProfile::default();
    };
    match serde_json::from_str(&text) {
        Ok(profile) => profile,
        Err(e) => {
            tracing::error!(
                target: "two_top::profile",
                error = %e,
                "profile.json is corrupt — quarantining it and reminting",
            );
            let mut name = path
                .file_name()
                .map(std::ffi::OsStr::to_os_string)
                .unwrap_or_default();
            name.push(".corrupt");
            let _ = std::fs::rename(path, path.with_file_name(name));
            LocalProfile::default()
        }
    }
}

fn save_profile(profile: &LocalProfile) {
    let Some(path) = profile_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(profile)
        && let Err(e) = crate::paths::write_atomic(&path, json.as_bytes())
    {
        tracing::warn!(target: "two_top::profile", error = %e, "failed to save profile");
    }
}

/// Your name over your demon on the title.
#[derive(Component)]
struct TitleName;

/// The name being typed, with its cursor.
#[derive(Component)]
struct EntryLine;

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
    commands.spawn((
        EntryLine,
        Text2d::new(String::new()),
        TextFont {
            // Sized so a full 12 glyphs still fit the width with margin.
            font_size: 64.0,
            ..default()
        },
        TextColor(render::palette::HOT_BONE),
        TextLayout {
            justify: Justify::Center,
            linebreak: bevy::text::LineBreak::NoWrap,
        },
        ScreenAnchor::new(0.0, ENTRY_ANCHOR_Y, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, 201.0),
        Visibility::Hidden,
    ));
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
    key(
        GridKey::Del,
        0.28,
        ACTION_ROW_Y,
        34.0,
        render::palette::COLD_STONE,
    );
    key(
        GridKey::Done,
        0.72,
        ACTION_ROW_Y,
        40.0,
        render::palette::SPARK,
    );
}

/// Entering the keyboard: an unclaimed name starts BLANK so you can just
/// type — the dealt placeholder exists so the wire is never empty, not so
/// you have to backspace it before your own name. Editing a name you
/// already chose keeps it, so a one-letter fix stays a one-letter fix.
fn clear_placeholder_on_entry(mut profile: ResMut<LocalProfile>) {
    if !profile.named {
        profile.name.clear();
    }
}

/// First boot opens the keyboard once: the first thing you do in 2-Top is
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

/// The keyboard: type to append, DEL to backspace, DONE to keep it —
/// exactly the three things a keyboard does. Escape keeps it too.
fn name_entry_input(
    keys: Res<ButtonInput<KeyCode>>,
    touches: Res<Touches>,
    window: Res<WindowSize>,
    mut profile: ResMut<LocalProfile>,
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
                hit = Some(if fx < 0.5 {
                    GridKey::Del
                } else {
                    GridKey::Done
                });
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
        if profile.name.chars().count() < NAME_MAX {
            let ch = NAME_ALPHABET[i as usize % NAME_ALPHABET.len()];
            profile.name.push(ch);
        }
        return;
    }
    match hit {
        Some(GridKey::Done) => {
            // An empty name is not a name: keep the dealt placeholder
            // rather than shipping a blank one to every opponent.
            if profile.name.is_empty() {
                profile.name = default_name(profile.install_id);
            }
            profile.named = true;
            save_profile(&profile);
            next.set(AppScreen::Title);
        }
        Some(GridKey::Del) => {
            profile.name.pop();
        }
        _ => {}
    }
}

/// Render the title name and the entry screen.
#[allow(clippy::type_complexity)]
fn update_name_ui(
    screen: Res<State<AppScreen>>,
    netplay: Res<NetplayConfig>,
    profile: Res<LocalProfile>,
    time: Res<Time<Real>>,
    mut title: Query<
        (&mut Text2d, &mut Visibility),
        (With<TitleName>, Without<EntryLine>, Without<GridKey>),
    >,
    mut entry: Query<
        (&mut Text2d, &mut Visibility),
        (With<EntryLine>, Without<TitleName>, Without<GridKey>),
    >,
    mut grid: Query<
        (&GridKey, &mut Text2d, &mut TextColor, &mut Visibility),
        (Without<TitleName>, Without<EntryLine>),
    >,
) {
    let online = netplay.room_url.is_some();
    let on_title = *screen.get() == AppScreen::Title && online;
    let entering = *screen.get() == AppScreen::NameEntry && online;

    if let Ok((mut text, mut vis)) = title.single_mut() {
        *vis = if on_title {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        text.0 = profile.name.clone();
    }
    if let Ok((mut text, mut vis)) = entry.single_mut() {
        *vis = if entering {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        if entering {
            // A blinking caret, the way a text field has blinked since
            // forever — it says "type here" without a word of instruction.
            let caret = if (time.elapsed_secs() * 2.5).fract() < 0.6 {
                "_"
            } else {
                " "
            };
            text.0 = format!("{}{caret}", profile.name);
        }
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
            .add_systems(Startup, spawn_name_ui)
            .add_systems(OnEnter(AppScreen::NameEntry), clear_placeholder_on_entry)
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
    fn names_round_trip_through_the_wire_and_clamp_wild_bytes() {
        assert_eq!(name_from_slots(&slots_from_name("MORGAN")), "MORGAN");
        assert_eq!(name_from_slots(&slots_from_name("SUDS99")), "SUDS99");
        // Lowercase is folded, unknown characters are dropped rather than
        // rejected: a hand-edited profile.json degrades, never explodes.
        assert_eq!(name_from_slots(&slots_from_name("mo rg-an!")), "MORGAN");
        // Past the ceiling is cut, not wrapped.
        assert_eq!(
            name_from_slots(&slots_from_name("ABCDEFGHIJKLMNOP")).len(),
            NAME_MAX
        );
        // A peer's bytes are untrusted: wild indices clamp to a letter,
        // and an all-pad name converts to nothing at all.
        assert_eq!(name_from_slots(&[255; NAME_MAX]), "");
        assert_eq!(name_from_slots(&net::name_slots(&[254, 253])).len(), 2);
        // The placeholder is a DISPLAY rule, not a conversion rule — that
        // conflation once stored the literal words as a player's name.
        assert_eq!(peer_name(None), "THE CHALLENGER");
        assert_eq!(
            peer_name(Some(net::ProfileData {
                install_id: 1,
                name: [NAME_EMPTY; NAME_MAX]
            })),
            "THE CHALLENGER"
        );
        assert_eq!(
            peer_name(Some(net::ProfileData {
                install_id: 1,
                name: slots_from_name("SUDS")
            })),
            "SUDS"
        );
    }

    #[test]
    fn a_short_name_pads_and_stops_at_the_pad() {
        let slots = slots_from_name("AB");
        assert_eq!(slots[0], 0);
        assert_eq!(slots[1], 1);
        assert_eq!(slots[2], NAME_EMPTY, "the tail is padded");
        assert_eq!(name_from_slots(&slots), "AB", "and reading stops there");
    }

    #[test]
    fn the_keyboard_covers_the_alphabet_exactly_once() {
        let mut seen: Vec<char> = KEY_ROWS.iter().flat_map(|r| r.chars()).collect();
        seen.sort_unstable();
        let mut alphabet = NAME_ALPHABET.to_vec();
        alphabet.sort_unstable();
        assert_eq!(seen, alphabet, "every glyph is typable, none twice");
    }

    #[test]
    fn every_key_hit_tests_back_to_its_own_glyph() {
        // The layout and the hit-test are separate pieces of math; this is
        // the round-trip that keeps them honest.
        for (row, keys) in KEY_ROWS.iter().enumerate() {
            for (col, ch) in keys.chars().enumerate() {
                let (fx, fy) = key_center(row, col);
                let hit = key_at(fx, fy).expect("a key's own center hits it");
                assert_eq!(
                    NAME_ALPHABET[hit as usize], ch,
                    "row {row} col {col} should hit {ch}"
                );
            }
        }
        // Above the keyboard is the typed line, not a key.
        assert!(key_at(0.5, KEY_TOP - 0.01).is_none());
    }

    #[test]
    fn default_name_is_derived_from_the_install_id() {
        let a = default_name(0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10);
        let b = default_name(0xfeed_face_feed_face_feed_face_feed_face);
        assert_eq!(a.chars().count(), 4);
        assert_ne!(a, b, "different installs deal different names");
    }

    #[test]
    fn the_tag_is_stable_per_install_and_differs_across_them() {
        let id = 0xfeed_face_feed_face_feed_face_feed_face;
        assert_eq!(identity_tag(id), identity_tag(id), "stable");
        assert_eq!(identity_tag(id).chars().count(), 3);
        assert_ne!(identity_tag(id), identity_tag(id ^ 0xFFFF));
    }

    #[test]
    fn a_garbage_tmp_sibling_never_touches_the_identity() {
        // The interrupted-save shape: the old profile intact on disk, a
        // truncated .tmp corpse beside it (the process died before the
        // rename). The identity must load exactly as saved.
        let dir = crate::paths::test_scratch("profile_tmp_corpse");
        let path = dir.join("profile.json");
        let saved = LocalProfile {
            install_id: 7,
            name: "SUDS".to_string(),
            named: true,
        };
        std::fs::write(&path, serde_json::to_string_pretty(&saved).unwrap()).unwrap();
        std::fs::write(dir.join("profile.json.tmp"), "{\"install_id\": 99").unwrap();
        let loaded = read_profile(&path);
        assert_eq!(loaded.install_id, 7);
        assert_eq!(loaded.name, "SUDS");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_profile_is_quarantined_not_silently_replaced() {
        let dir = crate::paths::test_scratch("profile_corrupt");
        let path = dir.join("profile.json");
        std::fs::write(&path, "{\"install_id\": 12345, \"na").unwrap();
        let loaded = read_profile(&path);
        assert_eq!(
            loaded.install_id, 0,
            "an unreadable identity falls to default; the caller mints"
        );
        assert!(!path.exists(), "the corrupt file is moved aside");
        assert!(
            dir.join("profile.json.corrupt").exists(),
            "the evidence survives for a human to hand back"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_fresh_profile_is_unclaimed_so_the_keyboard_opens_once() {
        let fresh = LocalProfile::default();
        assert!(!fresh.named);
        // A profile.json from an older build predates the flag: serde
        // defaults it to false, so those installs get the keyboard once
        // too. That is the right call — they are the phones named GSAA.
        let old: LocalProfile =
            serde_json::from_str(r#"{"install_id": 7, "name": "TAGC"}"#).unwrap();
        assert!(!old.named);
        assert_eq!(old.install_id, 7);
    }
}
