//! How you choose your opponent: a stranger, or the person next to you.
//!
//! Online, the title carries a two-state toggle — QUICK MATCH or PRIVATE —
//! sitting directly on top of the button it changes. QUICK MATCH is the
//! public room (the base `--room`/`MATCHBOX_ROOM`/`TWOTOP_ROOM` URL, where
//! strangers pair on `?next=2`) and the button below reads FIND OPPONENT.
//! PRIVATE unfolds a four-glyph dial and the button relabels itself to
//! `DUEL AT C-U-R-S`: both phones dial the same code, both press, and only
//! those two ever meet. 7⁴ = 2401 rooms without a keyboard, an account, or
//! a camera.
//!
//! The dial keeps the 7-glyph CURSTAG wheel (tap-cycling is cheap at seven
//! and a friend's code should be four quick taps away) — unlike the NAME,
//! which is A-Z on a grid because a name has to be worth reading.
//!
//! Desktop (dev): keys 1-4 cycle the slots, 0 back to QUICK MATCH.
//!
//! The chosen code persists (`room_code.json` beside settings) and is
//! applied by rewriting `NetplayConfig.room_url` — `start_matchbox` reads
//! that resource at match entry, so no connect-path changes are needed.

use bevy::prelude::*;
use input_touch::WindowSize;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::anchor::ScreenAnchor;
use crate::netplay::NetplayConfig;
use crate::screen::{
    AppScreen, BtnRole, DIAL_ANCHOR_Y, DIAL_RECT, MODE_TOGGLE_ANCHOR_Y, MODE_TOGGLE_RECT,
    spawn_button_part,
};

/// The glyph wheel each slot cycles through — the duelists' own letters.
pub const CODE_ALPHABET: [char; 7] = ['C', 'U', 'R', 'S', 'T', 'A', 'G'];
pub const CODE_LEN: usize = 4;

/// "CURS" → "C-U-R-S": the primary button's code reads as something you
/// dial, not as a word.
pub fn code_dashed(code: &str) -> String {
    code.chars().map(String::from).collect::<Vec<_>>().join("-")
}

/// The dial's four cells, centered under the toggle (window-x fractions).
const DIAL_LEFT: f32 = 0.18;
const CELL_W: f32 = 0.16;

#[derive(Resource, Serialize, Deserialize, Clone, Debug)]
pub struct RoomCode {
    pub slots: [u8; CODE_LEN],
    pub custom: bool,
    /// The quickmatch room URL the code decorates (captured at boot).
    #[serde(skip)]
    base_url: Option<String>,
}

impl Default for RoomCode {
    fn default() -> Self {
        Self {
            slots: [0; CODE_LEN],
            custom: false,
            base_url: None,
        }
    }
}

impl RoomCode {
    pub fn code_string(&self) -> String {
        self.slots
            .iter()
            .map(|&i| CODE_ALPHABET[i as usize % CODE_ALPHABET.len()])
            .collect()
    }

    /// The room URL this code selects: the base for QUICK, the code-suffixed
    /// room for a private match.
    pub fn room_url(&self) -> Option<String> {
        let base = self.base_url.as_ref()?;
        Some(if self.custom {
            room_url_with_code(base, &self.code_string())
        } else {
            base.clone()
        })
    }
}

/// Append the code to the room *name*, preserving any query string:
/// `ws://host/two-top?next=2` + `CURS` → `ws://host/two-top-CURS?next=2`.
/// Pure for testing.
pub fn room_url_with_code(base: &str, code: &str) -> String {
    match base.split_once('?') {
        Some((path, query)) => format!("{path}-{code}?{query}"),
        None => format!("{base}-{code}"),
    }
}

fn room_code_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("two-top").join("room_code.json"))
}

fn load_room_code() -> RoomCode {
    let Some(path) = room_code_path() else {
        return RoomCode::default();
    };
    let mut code: RoomCode = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    for slot in &mut code.slots {
        *slot %= CODE_ALPHABET.len() as u8;
    }
    code
}

fn save_room_code(code: &RoomCode) {
    let Some(path) = room_code_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(code) {
        let _ = std::fs::write(&path, json);
    }
}

/// One glyph slot of the private dial (0..CODE_LEN).
#[derive(Component)]
struct CodeCell(usize);

/// One piece of a mode pill. `private` picks which of the two.
#[derive(Component)]
struct ModePill {
    private: bool,
    role: BtnRole,
}

fn spawn_pad(mut commands: Commands, netplay: Res<NetplayConfig>) {
    // Couch builds have no room to dial.
    if netplay.room_url.is_none() {
        return;
    }
    // The toggle: two pills, selected one filled. Same bordered-box
    // language as every other button on the screen.
    // Anchors sized so the two boxes sit side by side with a real gap: a
    // pill is 480 wide plus its 22 of border on a ~1160-unit-wide screen,
    // so anything tighter than ±0.46 merges them into one box.
    for (private, anchor_x) in [(false, -0.47), (true, 0.47)] {
        for role in [BtnRole::Border, BtnRole::Fill, BtnRole::Label] {
            spawn_button_part(
                &mut commands,
                role,
                Vec2::new(anchor_x, MODE_TOGGLE_ANCHOR_Y),
                Vec2::new(480.0, 76.0),
                28.0,
                &mut |ec, role| {
                    ec.insert(ModePill { private, role });
                },
            );
        }
    }
    // The dial, hidden until PRIVATE is selected.
    for cell in 0..CODE_LEN {
        let fx = DIAL_LEFT + (cell as f32 + 0.5) * CELL_W;
        commands.spawn((
            CodeCell(cell),
            Text2d::new(String::new()),
            TextFont {
                font_size: 54.0,
                ..default()
            },
            TextColor(render::palette::P1_CYAN),
            ScreenAnchor::new(fx * 2.0 - 1.0, DIAL_ANCHOR_Y, 0.0, 0.0),
            Transform::from_xyz(0.0, 0.0, 200.0),
            Visibility::Hidden,
        ));
    }
}

/// Boot: capture the quickmatch base URL and apply any persisted code.
fn init_room_code(mut code: ResMut<RoomCode>, mut netplay: ResMut<NetplayConfig>) {
    code.base_url = netplay.room_url.clone();
    if let Some(url) = code.room_url() {
        netplay.room_url = Some(url);
    }
}

/// Title-screen taps/keys on the pad. Any change rewrites the live
/// `NetplayConfig.room_url` and persists the code.
fn room_code_input(
    keys: Res<ButtonInput<KeyCode>>,
    touches: Res<Touches>,
    window: Res<WindowSize>,
    mut code: ResMut<RoomCode>,
    mut netplay: ResMut<NetplayConfig>,
) {
    if code.base_url.is_none() {
        return;
    }
    let mut changed = false;

    let win = window.0;
    if win.x > 0.0 && win.y > 0.0 {
        for t in touches.iter_just_pressed() {
            let p = t.position();
            let (fx, fy) = (p.x / win.x, p.y / win.y);
            if (MODE_TOGGLE_RECT.0..MODE_TOGGLE_RECT.1).contains(&fy) {
                // Left pill quick, right pill private — the halves are the
                // pills, so a fat-fingered tap still lands on a real mode.
                code.custom = fx >= 0.5;
                changed = true;
            } else if code.custom
                && (DIAL_RECT.0..DIAL_RECT.1).contains(&fy)
                && fx >= DIAL_LEFT
            {
                let cell = ((fx - DIAL_LEFT) / CELL_W) as usize;
                if cell < CODE_LEN {
                    let slot = &mut code.slots[cell];
                    *slot = (*slot + 1) % CODE_ALPHABET.len() as u8;
                    changed = true;
                }
            }
        }
    }
    // Desktop dev path: 0 back to QUICK, 1-4 cycle a slot (and arm PRIVATE).
    if keys.just_pressed(KeyCode::Digit0) {
        code.custom = false;
        changed = true;
    }
    for (key, cell) in [
        (KeyCode::Digit1, 0usize),
        (KeyCode::Digit2, 1),
        (KeyCode::Digit3, 2),
        (KeyCode::Digit4, 3),
    ] {
        if keys.just_pressed(key) {
            code.custom = true;
            let slot = &mut code.slots[cell];
            *slot = (*slot + 1) % CODE_ALPHABET.len() as u8;
            changed = true;
        }
    }

    if !changed {
        return;
    }
    netplay.room_url = code.room_url();
    save_room_code(&code);
}

/// Show the toggle on the online title, and the dial only once PRIVATE is
/// selected: the row is simply empty in QUICK MATCH, so nothing on screen
/// asks a stranger to think about room codes.
#[allow(clippy::type_complexity)]
fn update_pad(
    screen: Res<State<AppScreen>>,
    netplay: Res<NetplayConfig>,
    code: Res<RoomCode>,
    mut cells: Query<(&CodeCell, &mut Text2d, &mut Visibility), Without<ModePill>>,
    mut pills: Query<
        (
            &ModePill,
            &mut Visibility,
            Option<&mut Sprite>,
            Option<&mut Text2d>,
            Option<&mut TextColor>,
        ),
        Without<CodeCell>,
    >,
) {
    let show = *screen.get() == AppScreen::Title && netplay.room_url.is_some();
    let glyphs = code.code_string();
    for (cell, mut text, mut vis) in &mut cells {
        *vis = if show && code.custom {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        text.0 = glyphs.chars().nth(cell.0).map(String::from).unwrap_or_default();
    }
    for (pill, mut vis, sprite, text, color) in &mut pills {
        *vis = if show {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        if !show {
            continue;
        }
        let selected = pill.private == code.custom;
        match pill.role {
            BtnRole::Border => {
                if let Some(mut s) = sprite {
                    s.color = render::palette::HOT_BONE
                        .with_alpha(if selected { 1.0 } else { 0.45 });
                }
            }
            BtnRole::Fill => {
                if let Some(mut s) = sprite {
                    s.color = if selected {
                        render::palette::HOT_BONE
                    } else {
                        render::palette::DEEP_ASH
                    };
                }
            }
            BtnRole::Label => {
                if let Some(mut t) = text {
                    t.0 = if pill.private {
                        "PRIVATE".to_string()
                    } else {
                        "QUICK MATCH".to_string()
                    };
                }
                if let Some(mut c) = color {
                    c.0 = if selected {
                        render::palette::VOID
                    } else {
                        render::palette::HOT_BONE.with_alpha(0.7)
                    };
                }
            }
        }
    }
}

pub struct RoomCodePlugin;

impl Plugin for RoomCodePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(load_room_code())
            .add_systems(Startup, (init_room_code, spawn_pad))
            .add_systems(
                Update,
                (
                    room_code_input.run_if(in_state(AppScreen::Title)),
                    update_pad,
                ),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_suffixes_the_room_name_not_the_query() {
        assert_eq!(
            room_url_with_code("ws://h:3536/two-top?next=2", "CURS"),
            "ws://h:3536/two-top-CURS?next=2"
        );
        assert_eq!(room_url_with_code("ws://h/two-top", "TAGS"), "ws://h/two-top-TAGS");
    }

    #[test]
    fn code_string_wraps_indices_into_the_alphabet() {
        let code = RoomCode {
            slots: [0, 1, 6, 7], // 7 wraps back to 'C'
            custom: true,
            base_url: None,
        };
        assert_eq!(code.code_string(), "CUGC");
    }

    #[test]
    fn quick_room_is_the_untouched_base() {
        let code = RoomCode {
            slots: [1, 2, 3, 4],
            custom: false,
            base_url: Some("ws://h/two-top?next=2".into()),
        };
        assert_eq!(code.room_url().as_deref(), Some("ws://h/two-top?next=2"));
    }

    #[test]
    fn custom_room_carries_the_code() {
        let code = RoomCode {
            slots: [0, 1, 2, 3],
            custom: true,
            base_url: Some("ws://h/two-top?next=2".into()),
        };
        assert_eq!(
            code.room_url().as_deref(),
            Some("ws://h/two-top-CURS?next=2")
        );
    }
}
