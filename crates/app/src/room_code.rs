//! Room-code entry pad — private rooms with zero text input.
//!
//! Online, the title screen grows a tap row: `QUICK  ·  C U R S`. QUICK is
//! the default public room (the base `--room`/`MATCHBOX_ROOM`/`TWOTOP_ROOM`
//! URL — strangers pair on `?next=2` as before). Tapping a glyph slot
//! switches to a PRIVATE room: each tap cycles that slot through the
//! 7-glyph alphabet (the letters of CUR + STAG), and the code is appended
//! to the room name — only phones dialed to the same four glyphs ever
//! meet. 7⁴ = 2401 rooms: plenty of privacy for "you and me, right now"
//! without a keyboard, an account, or a camera.
//!
//! Desktop (dev): keys 1-4 cycle the slots, 0 resets to QUICK.
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
use crate::screen::AppScreen;

/// The glyph wheel each slot cycles through — the duelists' own letters.
pub const CODE_ALPHABET: [char; 7] = ['C', 'U', 'R', 'S', 'T', 'A', 'G'];
pub const CODE_LEN: usize = 4;

/// Tap band, in window-fraction coordinates (y-down, like `Touches`):
/// a horizontal strip below the title body, above the start zone.
// Between the practice button (ends 0.655) and the PLAY button (0.80).
const BAND_TOP: f32 = 0.68;
const BAND_BOTTOM: f32 = 0.78;
/// The band's five cells (QUICK + four slots) span this horizontal window.
const BAND_LEFT: f32 = 0.05;
const CELL_W: f32 = 0.18;

/// The screen-anchor row the five cells render on (anchor frac, y-up):
/// centers of the tap cells above, converted to [-1, 1].
const ROW_ANCHOR_Y: f32 = 1.0 - (BAND_TOP + BAND_BOTTOM);

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

/// One cell of the pad row. 0 = QUICK, 1..=4 = glyph slots.
#[derive(Component)]
struct CodeCell(usize);

fn spawn_pad(mut commands: Commands, netplay: Res<NetplayConfig>) {
    // Couch builds have no room to dial.
    if netplay.room_url.is_none() {
        return;
    }
    for cell in 0..=CODE_LEN {
        // Cell centers in window fraction → anchor frac (y-up, [-1,1]).
        let fx = BAND_LEFT + (cell as f32 + 0.5) * CELL_W;
        let anchor_x = fx * 2.0 - 1.0;
        commands.spawn((
            CodeCell(cell),
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
    let mut touched: Option<usize> = None;

    let win = window.0;
    if win.x > 0.0 && win.y > 0.0 {
        for t in touches.iter_just_pressed() {
            let p = t.position();
            let (fx, fy) = (p.x / win.x, p.y / win.y);
            if (BAND_TOP..BAND_BOTTOM).contains(&fy) && fx >= BAND_LEFT {
                let cell = ((fx - BAND_LEFT) / CELL_W) as usize;
                if cell <= CODE_LEN {
                    touched = Some(cell);
                }
            }
        }
    }
    // Desktop dev path: 1-4 cycle the slots, 0 back to QUICK.
    for (key, cell) in [
        (KeyCode::Digit0, 0usize),
        (KeyCode::Digit1, 1),
        (KeyCode::Digit2, 2),
        (KeyCode::Digit3, 3),
        (KeyCode::Digit4, 4),
    ] {
        if keys.just_pressed(key) {
            touched = Some(cell);
        }
    }

    let Some(cell) = touched else {
        return;
    };
    if cell == 0 {
        code.custom = false;
    } else {
        code.custom = true;
        let slot = &mut code.slots[cell - 1];
        *slot = (*slot + 1) % CODE_ALPHABET.len() as u8;
    }
    netplay.room_url = code.room_url();
    save_room_code(&code);
}

/// Show the pad only on the online title; render the selection state.
fn update_pad(
    screen: Res<State<AppScreen>>,
    netplay: Res<NetplayConfig>,
    code: Res<RoomCode>,
    mut cells: Query<(&CodeCell, &mut Text2d, &mut TextColor, &mut Visibility)>,
) {
    let show = *screen.get() == AppScreen::Title && netplay.room_url.is_some();
    let glyphs = code.code_string();
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
            text.0 = if code.custom {
                "QUICK".to_string()
            } else {
                "> QUICK <".to_string()
            };
            color.0 = if code.custom {
                render::palette::BONE.with_alpha(0.45)
            } else {
                render::palette::HOT_BONE
            };
        } else {
            text.0 = glyphs
                .chars()
                .nth(cell.0 - 1)
                .map(String::from)
                .unwrap_or_default();
            color.0 = if code.custom {
                render::palette::P1_CYAN
            } else {
                render::palette::BONE.with_alpha(0.35)
            };
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
