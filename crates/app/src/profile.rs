//! Local identity — the install-id and the dialed duelist name.
//!
//! matchbox `PeerId`s are ephemeral (fresh every connection), so anything
//! that should survive across matches — the per-opponent grudge ledger,
//! names on the summary — needs a durable identity. This is it: a random
//! u128 minted once per install plus a 4-glyph name from the CURSTAG
//! alphabet, persisted beside settings and exchanged over the reliable
//! side-channel right after the P2P swap (`NetMsg::Profile`).
//!
//! The name dials exactly like the room code: a tap row on the online
//! title, each tap cycling that slot through the 7-glyph alphabet. No
//! keyboard, no account. A fresh install gets a name derived from its
//! install-id, so every phone has one before its owner ever touches the
//! pad — you meet KIPS and TAGC in the wild, not UNNAMED.

use bevy::prelude::*;
use input_touch::WindowSize;
use net::{NAME_LEN, ProfileData};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::anchor::ScreenAnchor;
use crate::netplay::NetplayConfig;
use crate::room_code::CODE_ALPHABET;
use crate::screen::AppScreen;

/// Tap band (window-fraction, y-down). Online this row replaces the couch
/// arena picker's band (0.24–0.36) — online has no arena picker (the room
/// hash decides), so the slot is free there. The tagline sits just above
/// (anchored 0.46 y-up ≈ 0.27 y-down), so the row starts at 0.30.
const BAND_TOP: f32 = 0.30;
const BAND_BOTTOM: f32 = 0.36;
const BAND_LEFT: f32 = 0.05;
const CELL_W: f32 = 0.18;
const ROW_ANCHOR_Y: f32 = 1.0 - (BAND_TOP + BAND_BOTTOM);

#[derive(Resource, Serialize, Deserialize, Clone, Copy, Debug)]
#[serde(default)]
pub struct LocalProfile {
    pub install_id: u128,
    pub slots: [u8; NAME_LEN],
}

impl Default for LocalProfile {
    fn default() -> Self {
        Self {
            install_id: 0,
            slots: [0; NAME_LEN],
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

/// Render glyph slots into a name, clamping every index into the alphabet
/// (a peer's bytes are untrusted; the worst a wild value can do is show a
/// wrong letter).
pub fn name_from_slots(slots: &[u8; NAME_LEN]) -> String {
    slots
        .iter()
        .map(|&i| CODE_ALPHABET[i as usize % CODE_ALPHABET.len()])
        .collect()
}

/// A fresh install's default name: glyphs dealt from its install-id, so
/// two strangers almost never boot up with the same one.
fn default_slots(install_id: u128) -> [u8; NAME_LEN] {
    let mut slots = [0u8; NAME_LEN];
    for (i, slot) in slots.iter_mut().enumerate() {
        *slot = ((install_id >> (i * 8)) as u8) % CODE_ALPHABET.len() as u8;
    }
    slots
}

fn profile_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("two-top").join("profile.json"))
}

fn load_profile() -> LocalProfile {
    let mut profile: LocalProfile = profile_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    for slot in &mut profile.slots {
        *slot %= CODE_ALPHABET.len() as u8;
    }
    if profile.install_id == 0 {
        // First boot (or a hand-wiped file): mint the durable identity.
        profile.install_id = uuid::Uuid::new_v4().as_u128();
        profile.slots = default_slots(profile.install_id);
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

/// One cell of the name row. 0 = the "NAME" label, 1..=4 = glyph slots.
#[derive(Component)]
struct NameCell(usize);

fn spawn_name_pad(mut commands: Commands, netplay: Res<NetplayConfig>) {
    // Couch builds carry no identity to exchange; the row is online-only.
    if netplay.room_url.is_none() {
        return;
    }
    for cell in 0..=NAME_LEN {
        let fx = BAND_LEFT + (cell as f32 + 0.5) * CELL_W;
        let anchor_x = fx * 2.0 - 1.0;
        commands.spawn((
            NameCell(cell),
            Text2d::new(String::new()),
            TextFont {
                font_size: if cell == 0 { 36.0 } else { 54.0 },
                ..default()
            },
            TextColor(render::palette::BONE),
            ScreenAnchor::new(anchor_x, ROW_ANCHOR_Y, 0.0, 0.0),
            Transform::from_xyz(0.0, 0.0, 200.0),
            Visibility::Hidden,
        ));
    }
}

/// Title-screen taps/keys on the name row. Keys 5-8 cycle the slots (dev
/// convenience, clear of the room pad's 0-4).
fn name_pad_input(
    keys: Res<ButtonInput<KeyCode>>,
    touches: Res<Touches>,
    window: Res<WindowSize>,
    netplay: Res<NetplayConfig>,
    mut profile: ResMut<LocalProfile>,
) {
    if netplay.room_url.is_none() {
        return;
    }
    let mut touched: Option<usize> = None;

    let win = window.0;
    if win.x > 0.0 && win.y > 0.0 {
        for t in touches.iter_just_pressed() {
            let p = t.position();
            let (fx, fy) = (p.x / win.x, p.y / win.y);
            if (BAND_TOP..BAND_BOTTOM).contains(&fy) && fx >= BAND_LEFT {
                let cell = ((fx - BAND_LEFT) / CELL_W) as usize;
                if (1..=NAME_LEN).contains(&cell) {
                    touched = Some(cell);
                }
            }
        }
    }
    for (key, cell) in [
        (KeyCode::Digit5, 1usize),
        (KeyCode::Digit6, 2),
        (KeyCode::Digit7, 3),
        (KeyCode::Digit8, 4),
    ] {
        if keys.just_pressed(key) {
            touched = Some(cell);
        }
    }

    let Some(cell) = touched else {
        return;
    };
    let slot = &mut profile.slots[cell - 1];
    *slot = (*slot + 1) % CODE_ALPHABET.len() as u8;
    save_profile(&profile);
}

/// Show the row on the online title; render the dialed glyphs.
fn update_name_pad(
    screen: Res<State<AppScreen>>,
    netplay: Res<NetplayConfig>,
    profile: Res<LocalProfile>,
    mut cells: Query<(&NameCell, &mut Text2d, &mut TextColor, &mut Visibility)>,
) {
    let show = *screen.get() == AppScreen::Title && netplay.room_url.is_some();
    let name = profile.name_string();
    for (cell, mut text, mut color, mut vis) in &mut cells {
        *vis = if show {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        if !show {
            continue;
        }
        if cell.0 == 0 {
            text.0 = "NAME".to_string();
            color.0 = render::palette::BONE.with_alpha(0.45);
        } else {
            text.0 = name
                .chars()
                .nth(cell.0 - 1)
                .map(String::from)
                .unwrap_or_default();
            color.0 = render::palette::HOT_BONE;
        }
    }
}

pub struct ProfilePlugin;

impl Plugin for ProfilePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(load_profile())
            .add_systems(Startup, spawn_name_pad)
            .add_systems(
                Update,
                (
                    name_pad_input.run_if(in_state(AppScreen::Title)),
                    update_name_pad,
                ),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_render_from_slots_and_clamp_wild_bytes() {
        assert_eq!(name_from_slots(&[0, 1, 2, 3]), "CURS");
        assert_eq!(name_from_slots(&[4, 5, 6, 0]), "TAGC");
        // 7 wraps to 'C', 255 lands somewhere in the alphabet — never a panic.
        assert_eq!(name_from_slots(&[7, 7, 7, 7]), "CCCC");
        let wild = name_from_slots(&[255, 254, 253, 252]);
        assert_eq!(wild.len(), 4);
    }

    #[test]
    fn default_name_is_derived_from_the_install_id() {
        let a = default_slots(0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10);
        let b = default_slots(0xfeed_face_feed_face_feed_face_feed_face);
        for slot in a.iter().chain(b.iter()) {
            assert!((*slot as usize) < CODE_ALPHABET.len());
        }
        assert_ne!(a, b, "different installs deal different names");
    }
}
