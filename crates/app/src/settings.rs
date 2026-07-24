//! Phase 18 Task 5.5c: persisted player settings.
//!
//! A small [`Settings`] resource — stick deadzone, haptics on/off, and the
//! SFX/music volumes — loaded from (and saved to) a JSON file in the platform
//! config dir. The audio cues and haptics read it live; the deadzone is pushed
//! into `input_touch`'s [`StickDeadzone`] so it shapes the virtual stick
//! *before* quantization to the wire format (a legal pre-wire input change —
//! never post-wire). Adjusted from the title screen.
//!
//! Persistence is best-effort: if the config dir can't be resolved (e.g. some
//! Android setups) settings simply stay in-memory for the session.

use bevy::prelude::*;
use input_touch::{Southpaw, StickDeadzone, WindowSize};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::screen::AppScreen;

/// Default SFX bus level. The slider maps through `audio::bus_gain`'s square
/// perceptual taper; the cue files are peak-normalized to −3 dBFS.
pub const SFX_VOLUME_DEFAULT: f32 = 0.7;
/// Default music bus level (same square taper; the music loops master at
/// −12 dBFS so the beds sit under gameplay cues at these defaults).
pub const MUSIC_VOLUME_DEFAULT: f32 = 0.6;
/// Default virtual-stick inner deadzone (matches `input_touch`'s baseline).
pub const DEADZONE_DEFAULT: f32 = 0.12;
/// Upper bound on the configurable deadzone — past this the stick is unusable.
pub const DEADZONE_MAX: f32 = 0.40;

#[derive(Resource, Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub stick_deadzone: f32,
    pub haptics: bool,
    pub sfx_volume: f32,
    pub music_volume: f32,
    /// Mirror the touch zones left-for-right (move on the right, throw on
    /// the left, dash bottom-LEFT). A touch fighter owes lefties this.
    pub southpaw: bool,
    /// The picked arena (`sim::ArenaId` wire id). The roster screen writes
    /// it; the pick survives relaunch like every other preference.
    pub arena: u8,
    /// Screen-shake intensity, 0..1 scaling the trauma offset. The feel
    /// layer hits hard by design; this is the kindness knob for anyone it
    /// hits too hard. Kill flash and haptics are separate.
    pub shake: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            stick_deadzone: DEADZONE_DEFAULT,
            haptics: true,
            sfx_volume: SFX_VOLUME_DEFAULT,
            music_volume: MUSIC_VOLUME_DEFAULT,
            southpaw: false,
            arena: 0,
            shake: 1.0,
        }
    }
}

/// Persist the settings JSON — the arena roster (and anything else outside
/// this module) saves through here.
pub fn persist(settings: &Settings) {
    save_settings(settings);
}

impl Settings {
    /// Clamp every field into its valid range (guards against a hand-edited or
    /// corrupt settings file feeding NaN / out-of-range values into audio and
    /// input).
    pub fn clamped(mut self) -> Self {
        self.stick_deadzone =
            clamp_finite(self.stick_deadzone, 0.0, DEADZONE_MAX, DEADZONE_DEFAULT);
        self.sfx_volume = clamp_finite(self.sfx_volume, 0.0, 1.0, SFX_VOLUME_DEFAULT);
        self.music_volume = clamp_finite(self.music_volume, 0.0, 1.0, MUSIC_VOLUME_DEFAULT);
        self.shake = clamp_finite(self.shake, 0.0, 1.0, 1.0);
        self
    }
}

/// Clamp into `[lo, hi]`, falling back to `default` for non-finite input.
fn clamp_finite(v: f32, lo: f32, hi: f32, default: f32) -> f32 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        default
    }
}

fn settings_path() -> Option<PathBuf> {
    crate::paths::config_file("settings.json")
}

fn load_settings() -> Settings {
    let Some(path) = settings_path() else {
        return Settings::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<Settings>(&text)
            .map(Settings::clamped)
            .unwrap_or_default(),
        Err(_) => Settings::default(),
    }
}

fn save_settings(settings: &Settings) {
    let Some(path) = settings_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(settings)
        && let Err(e) = std::fs::write(&path, json)
    {
        tracing::warn!(target: "two_top::settings", error = %e, "failed to save settings");
    }
}

pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(load_settings())
            .add_systems(Startup, (push_deadzone, spawn_setting_rows))
            .add_systems(
                Update,
                (
                    adjust_settings.run_if(in_state(AppScreen::Settings)),
                    settings_back.run_if(in_state(AppScreen::Settings)),
                    update_setting_rows,
                ),
            );
    }
}

/// BACK out of the settings screen: the bottom band, or Escape.
fn settings_back(
    keys: Res<ButtonInput<KeyCode>>,
    touches: Res<Touches>,
    window: Res<WindowSize>,
    mut next: ResMut<NextState<AppScreen>>,
) {
    let win = window.0;
    let tapped = win.y > 0.0
        && touches
            .iter_just_pressed()
            .any(|t| (BACK_BAND.0..BACK_BAND.1).contains(&(t.position().y / win.y)));
    if tapped || keys.just_pressed(KeyCode::Escape) {
        next.set(AppScreen::Title);
    }
}

/// Mirror the input-shaping settings into `input_touch`'s resources
/// (boot + on edit).
fn push_deadzone(
    settings: Res<Settings>,
    mut deadzone: ResMut<StickDeadzone>,
    mut southpaw: ResMut<Southpaw>,
) {
    deadzone.0 = settings.stick_deadzone;
    southpaw.0 = settings.southpaw;
}

/// Title-screen settings adjustment. Keys chosen to avoid the arena picker
/// (1/2/3) and start (Space/Enter): H toggles haptics, −/= the SFX volume,
/// `[`/`]` the music volume, `,`/`.` the deadzone, L the southpaw layout,
/// `;`/`'` the screen shake.
/// Any change re-clamps, pushes the input mirrors to `input_touch`, and
/// saves to disk.
// ---- Layout (window-fraction, y-down) ----
// The knobs read as three small families, not one undifferentiated stack:
// FEEL (haptics, shake), AUDIO (sfx, music), CONTROL (deadzone, southpaw).
// One walk of `layout()` produces every band, and the SAME walk serves the
// spawner and the tap hit-test — the rows you see and the rows you hit can
// never drift apart. Everything fits between the heading and BACK with
// air to spare, so the screen never needs to scroll.
const GROUPS_TOP: f32 = 0.21;
const GROUP_HEADER_H: f32 = 0.045;
const ROW_PITCH: f32 = 0.075;
const GROUP_GAP: f32 = 0.012;
const HEADING_FY: f32 = 0.12;
const ROW_COUNT: usize = 6;
/// The groups, naming rows by their settled indices (0 haptics, 1 sfx,
/// 2 music, 3 deadzone, 4 southpaw, 5 shake — `adjust_settings`' keys).
const GROUPS: [(&str, [usize; 2]); 3] = [
    ("FEEL", [0, 5]),
    ("AUDIO", [1, 2]),
    ("CONTROL", [3, 4]),
];
/// BACK, matching the replays screen's band exactly.
const BACK_BAND: (f32, f32) = (0.86, 0.96);

/// The computed screen layout: a y-down band per setting row (indexed by
/// the row's settled id) and a center fy per group header.
struct SettingsLayout {
    row_bands: [(f32, f32); ROW_COUNT],
    header_centers: [f32; GROUPS.len()],
}

fn layout() -> SettingsLayout {
    let mut row_bands = [(0.0, 0.0); ROW_COUNT];
    let mut header_centers = [0.0; GROUPS.len()];
    let mut cursor = GROUPS_TOP;
    for (g, (_, rows)) in GROUPS.iter().enumerate() {
        header_centers[g] = cursor + GROUP_HEADER_H * 0.5;
        cursor += GROUP_HEADER_H;
        for &row in rows {
            row_bands[row] = (cursor, cursor + ROW_PITCH);
            cursor += ROW_PITCH;
        }
        cursor += GROUP_GAP;
    }
    SettingsLayout {
        row_bands,
        header_centers,
    }
}

/// The setting row whose band contains a y-down window fraction.
fn row_at(fy: f32) -> Option<usize> {
    layout()
        .row_bands
        .iter()
        .position(|(top, bottom)| (*top..*bottom).contains(&fy))
}

/// One piece of the settings screen. `Setting(i)` is a tappable row,
/// `GroupHeader` a family label (text fixed at spawn), `Heading` the
/// screen title — one component, so one system shows and hides the whole
/// screen.
#[derive(Component)]
enum SettingsPiece {
    Setting(usize),
    GroupHeader,
    Heading,
}

/// The bordered BACK button's parts (the same button chrome every other
/// screen's BACK wears — this one was a bare floating word).
#[derive(Component)]
struct SettingsBack(crate::screen::BtnRole);

fn spawn_setting_rows(mut commands: Commands) {
    let l = layout();
    commands.spawn((
        SettingsPiece::Heading,
        Text2d::new("SETTINGS"),
        TextFont {
            font_size: 84.0,
            ..default()
        },
        TextColor(render::palette::HOT_BONE),
        TextLayout::new_with_justify(Justify::Center),
        crate::anchor::ScreenAnchor::new(0.0, 1.0 - 2.0 * HEADING_FY, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, 210.0),
        Visibility::Hidden,
    ));
    for (g, (name, _)) in GROUPS.iter().enumerate() {
        commands.spawn((
            SettingsPiece::GroupHeader,
            Text2d::new(name.to_string()),
            TextFont {
                font_size: 24.0,
                ..default()
            },
            TextColor(render::palette::COLD_STONE),
            TextLayout::new_with_justify(Justify::Center),
            crate::anchor::ScreenAnchor::new(0.0, 1.0 - 2.0 * l.header_centers[g], 0.0, 0.0),
            Transform::from_xyz(0.0, 0.0, 210.0),
            Visibility::Hidden,
        ));
    }
    for (row, (top, bottom)) in l.row_bands.iter().enumerate() {
        let fy = (top + bottom) * 0.5;
        commands.spawn((
            SettingsPiece::Setting(row),
            Text2d::new(String::new()),
            TextFont {
                font_size: 40.0,
                ..default()
            },
            TextColor(render::palette::BONE.with_alpha(0.85)),
            TextLayout::new_with_justify(Justify::Center),
            crate::anchor::ScreenAnchor::new(0.0, 1.0 - 2.0 * fy, 0.0, 0.0),
            Transform::from_xyz(0.0, 0.0, 210.0),
            Visibility::Hidden,
        ));
    }
    // BACK in the shared bordered-box language, at the shared band.
    let back_anchor_y = 1.0 - (BACK_BAND.0 + BACK_BAND.1);
    for role in [
        crate::screen::BtnRole::Border,
        crate::screen::BtnRole::Fill,
        crate::screen::BtnRole::Label,
    ] {
        crate::screen::spawn_button_part(
            &mut commands,
            role,
            Vec2::new(0.0, back_anchor_y),
            Vec2::new(340.0, 76.0),
            40.0,
            &mut |ec, role| {
                ec.insert(SettingsBack(role));
            },
        );
    }
}

/// Render each piece; the whole screen shows together. Rows carry their
/// tap arrows; the BACK chrome colors like every other bordered button.
#[allow(clippy::type_complexity)]
fn update_setting_rows(
    screen: Res<State<crate::screen::AppScreen>>,
    settings: Res<Settings>,
    mut pieces: Query<(&SettingsPiece, &mut Text2d, &mut Visibility), Without<SettingsBack>>,
    mut back: Query<
        (&SettingsBack, &mut Visibility, Option<&mut Text2d>),
        Without<SettingsPiece>,
    >,
) {
    let open = *screen.get() == crate::screen::AppScreen::Settings;
    for (piece, mut text, mut vis) in &mut pieces {
        *vis = if open {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        if !open {
            continue;
        }
        if let SettingsPiece::Setting(row) = piece {
            text.0 = match row {
                0 => format!(
                    "<  haptics {}  >",
                    if settings.haptics { "on" } else { "off" }
                ),
                1 => format!("<  sfx {:.0}%  >", settings.sfx_volume * 100.0),
                2 => format!("<  music {:.0}%  >", settings.music_volume * 100.0),
                3 => format!("<  deadzone {:.0}%  >", settings.stick_deadzone * 100.0),
                4 => format!(
                    "<  southpaw {}  >",
                    if settings.southpaw { "on" } else { "off" }
                ),
                _ => format!("<  shake {:.0}%  >", settings.shake * 100.0),
            };
        }
    }
    for (part, mut vis, text) in &mut back {
        *vis = if open {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        if open
            && part.0 == crate::screen::BtnRole::Label
            && let Some(mut t) = text
        {
            t.0 = "BACK".to_string();
        }
    }
}

/// Keyboard + touch settings edits, one owner for clamp/mirror/save.
/// Touch: a tap on a row's left half decreases (or toggles), right half
/// increases — the phone finally gets the same knobs the keyboard had.
fn adjust_settings(
    keys: Res<ButtonInput<KeyCode>>,
    touches: Res<Touches>,
    window: Res<WindowSize>,
    mut settings: ResMut<Settings>,
    mut deadzone: ResMut<StickDeadzone>,
    mut southpaw: ResMut<Southpaw>,
) {
    let before = *settings;
    let mut s = *settings;

    if keys.just_pressed(KeyCode::KeyH) {
        s.haptics = !s.haptics;
    }
    if keys.just_pressed(KeyCode::KeyL) {
        s.southpaw = !s.southpaw;
    }
    if keys.just_pressed(KeyCode::Minus) {
        s.sfx_volume -= 0.1;
    }
    if keys.just_pressed(KeyCode::Equal) {
        s.sfx_volume += 0.1;
    }
    if keys.just_pressed(KeyCode::BracketLeft) {
        s.music_volume -= 0.1;
    }
    if keys.just_pressed(KeyCode::BracketRight) {
        s.music_volume += 0.1;
    }
    if keys.just_pressed(KeyCode::Comma) {
        s.stick_deadzone -= 0.02;
    }
    if keys.just_pressed(KeyCode::Period) {
        s.stick_deadzone += 0.02;
    }
    if keys.just_pressed(KeyCode::Semicolon) {
        s.shake -= 0.25;
    }
    if keys.just_pressed(KeyCode::Quote) {
        s.shake += 0.25;
    }

    let win = window.0;
    if win.x > 0.0 && win.y > 0.0 {
        for t in touches.iter_just_pressed() {
            // The same layout walk that placed the rows resolves the tap.
            let Some(row) = row_at(t.position().y / win.y) else {
                continue;
            };
            let up = t.position().x > win.x * 0.5;
            let sign = if up { 1.0 } else { -1.0 };
            match row {
                0 => s.haptics = !s.haptics,
                1 => s.sfx_volume += 0.1 * sign,
                2 => s.music_volume += 0.1 * sign,
                3 => s.stick_deadzone += 0.02 * sign,
                4 => s.southpaw = !s.southpaw,
                _ => s.shake += 0.25 * sign,
            }
        }
    }

    let s = s.clamped();
    if s != before {
        *settings = s;
        deadzone.0 = s.stick_deadzone;
        southpaw.0 = s.southpaw;
        save_settings(&s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_in_range_and_round_trip_through_json() {
        let s = Settings::default();
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
        assert!(s.haptics);
    }

    #[test]
    fn clamping_pins_out_of_range_and_non_finite() {
        let wild = Settings {
            stick_deadzone: 9.0,
            haptics: false,
            sfx_volume: -3.0,
            music_volume: f32::NAN,
            southpaw: true,
            arena: 3,
            shake: -2.0,
        }
        .clamped();
        assert_eq!(wild.stick_deadzone, DEADZONE_MAX);
        assert_eq!(wild.sfx_volume, 0.0);
        assert_eq!(wild.shake, 0.0, "negative shake pins to zero");
        assert_eq!(
            wild.music_volume, MUSIC_VOLUME_DEFAULT,
            "NaN falls back to default"
        );
        assert!(!wild.haptics);
    }

    #[test]
    fn layout_bands_are_disjoint_and_clear_of_back() {
        let l = layout();
        // Every row got a real band.
        for (row, (top, bottom)) in l.row_bands.iter().enumerate() {
            assert!(bottom > top, "row {row} has a band");
            assert!(*top >= GROUPS_TOP, "row {row} below the heading");
            assert!(*bottom < BACK_BAND.0, "row {row} clear of BACK");
        }
        // No two bands overlap (grouping must never create a double-hit).
        for a in 0..ROW_COUNT {
            for b in (a + 1)..ROW_COUNT {
                let (at, ab) = l.row_bands[a];
                let (bt, bb) = l.row_bands[b];
                assert!(ab <= bt || bb <= at, "rows {a}/{b} overlap");
            }
        }
    }

    #[test]
    fn taps_resolve_to_the_row_they_land_on() {
        let l = layout();
        for (row, (top, bottom)) in l.row_bands.iter().enumerate() {
            assert_eq!(row_at((top + bottom) * 0.5), Some(row));
        }
        // A header tap adjusts nothing.
        assert_eq!(row_at(l.header_centers[0]), None);
        // Neither does the BACK band (settings_back owns it).
        assert_eq!(row_at((BACK_BAND.0 + BACK_BAND.1) * 0.5), None);
    }

    #[test]
    fn partial_json_fills_missing_fields_from_defaults() {
        // `#[serde(default)]` — a file written by an older build missing a
        // field still loads, with the new field defaulted.
        let s: Settings = serde_json::from_str(r#"{ "haptics": false }"#).unwrap();
        assert!(!s.haptics);
        assert_eq!(s.sfx_volume, SFX_VOLUME_DEFAULT);
        assert_eq!(s.stick_deadzone, DEADZONE_DEFAULT);
    }
}
