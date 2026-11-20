//! The replay theater — watch saved tapes on the device that played them.
//!
//! The recorder writes every decided match to a `.bmrg` beside the app
//! (`recorder::replays_dir`), and until now the phone had no way to play
//! one — `replay_viewer` is a desktop crate. This module closes the loop:
//! a REPLAYS screen lists the saved tapes, and tapping one replays it
//! through the *live game's own presentation* — the deterministic sim is
//! driven by the tape's inputs, so the HUD, kill-cam, audio, particles,
//! and the devouring all fire exactly as they did in the match. Watching
//! a replay looks like the match because it IS the match.
//!
//! Playback mechanics are ported from `replay_viewer`: a SyncTest session
//! with `check_distance: 0` + `input_delay: 0` (both required for
//! snapshot-scrub determinism — see the viewer's rationale), inputs from
//! `replay::playback_inputs_system`, pause/speed via `Time<Virtual>`, and
//! backward seeks via a `SimSnapshot` ring captured every second.
//!
//! While a tape plays, everything interactive stands down: the matchbox
//! driver never starts, the recorder doesn't re-record the tape, the
//! rematch gate is idle, and the touch layer's zones are ignored (the
//! playback source overwrites `LocalInputs` after every platform source).

use bevy::prelude::*;
use bevy::text::TextBounds;
use input_touch::WindowSize;
use replay::{Replay, ReplayPlayback, decode_for_sim_version};
use sim::SimSnapshot;
use std::path::PathBuf;

use crate::anchor::{ScreenAnchor, ScreenAnchorSet, ViewRect};
use crate::screen::AppScreen;

/// Sim ticks between scrub snapshots (1 s @ 60 Hz — the viewer's value).
const SNAPSHOT_INTERVAL: u32 = 60;
use crate::transport::SPEED_PRESETS;
/// Most recent tapes listed. Older ones stay on disk (a Files app reaches
/// them); the list stays one thumb-screen tall.
const LIST_MAX: usize = 8;

// ---- Screen bands (window-fraction, y-down) ----
/// Replays list: rows.
const LIST_TOP: f32 = 0.20;
const LIST_PITCH: f32 = 0.07;
/// Replays list: the BACK band at the bottom.
const BACK_BAND: crate::ui::Band = crate::ui::BACK;
/// Theater: the app-wide top-exit strip (`screen::TOP_EXIT_BAND`) leaves
/// the tape — one gesture, one meaning, every screen.
const EXIT_BAND: (f32, f32) = crate::screen::TOP_EXIT_BAND;
/// Theater: speed pips row.
const SPEED_BAND: (f32, f32) = (0.76, 0.84);
/// Theater: scrub strip along the bottom.
const SCRUB_BAND: (f32, f32) = (0.86, 0.97);

/// Theater state. `active` while a tape is loaded and `InMatch` is playing
/// it back; `prev_arena` restores the player's arena pick afterwards (the
/// tape stomps `SelectedArena` so the right props spawn).
#[derive(Resource, Default)]
pub struct TheaterMode {
    active: bool,
    total_frames: u32,
    /// The tick this tape's match starts on (`ReplayHeader::start_tick`):
    /// the scrub strip spans `first_frame..total_frames`.
    first_frame: u32,
    /// The match's kills (tick, victim seat), marked on the scrub strip.
    kills: Vec<(u32, u8)>,
    prev_arena: Option<sim::ArenaId>,
    /// Names carried in the tape header, for the marquee.
    names: [Option<String>; 2],
    /// The tape is still growing (a live broadcast, `live.rs`): the end of
    /// what has arrived is a wait, not the end.
    live: bool,
}

impl TheaterMode {
    pub fn active(&self) -> bool {
        self.active
    }

    /// Is the tape still growing?
    pub fn live(&self) -> bool {
        self.active && self.live
    }

    /// The tape's duelist names (header order = handle order).
    pub fn header_names(&self) -> [Option<String>; 2] {
        self.names.clone()
    }
}

/// Playback transport state — the viewer's `ViewerControls`, touch-sized.
#[derive(Resource, Clone, Copy)]
pub struct TheaterControls {
    paused: bool,
    seek_target: Option<u32>,
    speed_idx: usize,
    /// Window-fraction x of the last drag-scrub seek, so a held finger
    /// issues one seek per deliberate move. The first cut of this anchor
    /// stored the exact target FRAME, which survives a unit test and dies
    /// on a phone: a held finger jitters a pixel or two every report, and
    /// on any real tape one pixel is several frames, so every report was a
    /// "new" target — the seek re-fired every frame, each backward jitter
    /// restored a snapshot and re-chased at 64x, and the playhead thrashed
    /// forever. The scrub loop, again. Screen-space with a dead zone
    /// ([`SCRUB_DEADZONE`]) is jitter-proof by construction. Cleared when
    /// no finger is on the strip. See [`scrub_debounce`].
    scrub_anchor: Option<f32>,
}

impl Default for TheaterControls {
    fn default() -> Self {
        Self {
            paused: false,
            seek_target: None,
            speed_idx: 1, // 1.0x
            scrub_anchor: None,
        }
    }
}

/// Fraction of the window width the scrubbing finger must move from the
/// spot that issued the last seek before a new one fires. A held finger's
/// touch jitter is a few device pixels — well under half a percent of the
/// width — while a deliberate drag crosses this in one report. ~80 stops
/// across the strip is finer than a thumb can aim anyway.
const SCRUB_DEADZONE: f32 = 0.012;

/// Decide whether a drag-scrub touch at window-fraction `finger_fx` should
/// issue a new seek. Fires on the first touch of a scrub and again each
/// time the finger moves [`SCRUB_DEADZONE`] past the spot that last fired;
/// anything closer is touch jitter and must stay silent — the playhead can
/// sit overshot past its target (the 64x chase bursts a chunk of ticks per
/// frame), and any re-fire while the finger holds still re-targets it into
/// the scrub loop. The anchor moves only when a seek fires, so jitter can
/// never random-walk it across the threshold.
fn scrub_debounce(finger_fx: f32, anchor: &mut Option<f32>) -> bool {
    match *anchor {
        Some(a) if (finger_fx - a).abs() < SCRUB_DEADZONE => false,
        _ => {
            *anchor = Some(finger_fx);
            true
        }
    }
}

/// Whether forward playback should bank a scrub snapshot on `frame`. Every
/// `SNAPSHOT_INTERVAL` (so a backward seek restores from at most a second
/// back), plus frame 1 so scrubbing can always reach the start of the tape —
/// without it the earliest snapshot is a full second in and the first second
/// is unreachable. Frame 0 is the spawn state the despawn/spawn cycle owns,
/// never snapshotted mid-session.
fn should_snapshot(frame: u32) -> bool {
    frame == 1 || (frame != 0 && frame.is_multiple_of(SNAPSHOT_INTERVAL))
}

/// Scrub snapshot ring (the viewer's `SnapshotBuffer`).
#[derive(Resource, Default)]
struct SnapshotBuffer {
    entries: Vec<SimSnapshot>,
}

impl SnapshotBuffer {
    fn nearest_before(&self, frame: u32) -> Option<&SimSnapshot> {
        self.entries.iter().rev().find(|s| s.frame <= frame)
    }
}

/// One listed tape: the path plus the header facts the row shows.
/// `foreign_version` is `Some(v)` for a tape from another sim version —
/// listed honestly (dimmed, labeled, inert) instead of silently hidden:
/// the no-migrations law should read as a law, not as data loss. Viewing
/// one still means the archived tagged binary, per that law.
struct TapeEntry {
    path: PathBuf,
    line: String,
    foreign_version: Option<u32>,
}

/// The scanned list (refreshed on entering the Replays screen).
#[derive(Resource, Default)]
struct TapeList(Vec<TapeEntry>);

// ---- UI markers ----

#[derive(Component)]
struct ReplayScreenText;

#[derive(Component)]
struct TapeRow(usize);

/// The list's subline ("tap a tape to watch it") — stands down while a
/// refusal notice speaks in its place.
#[derive(Component)]
struct ReplaySubline;

/// The refusal notice's text entity (same slot as the subline).
#[derive(Component)]
struct TapeNoticeText;

/// A live tap-refusal notice: the message and the moment it was raised.
/// A tap on a foreign-version tape used to answer with a tracing log and
/// nothing else — on a phone that is indistinguishable from a dead button.
/// The no-migrations law stands (the tape still doesn't play); the REFUSAL
/// just becomes something the thumb can see.
#[derive(Resource, Default)]
struct TapeNoticeState(Option<(String, f32)>);

/// Seconds a refusal notice holds the subline slot before the hint returns.
const TAPE_NOTICE_SECS: f32 = 4.0;

#[derive(Component)]
struct ScrubPart(ScrubRole);

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScrubRole {
    Track,
    Fill,
    /// The `n`th kill's mark.
    Kill(usize),
}

/// Most kill marks drawn on the strip (a match has at most nine kills).
const KILL_MARKS: usize = 9;

/// A scrub landing within this fraction of a kill mark snaps to just
/// before the kill: the marks are what a thumb aims at.
const KILL_SNAP: f32 = 0.03;

/// Ticks of run-up shown before a kill a scrub snapped to (2 s).
const KILL_RUN_UP: u32 = 120;

/// The fraction of the strip at `frame`, for a match spanning
/// `first..total`. Pure.
fn strip_frac(frame: u32, first: u32, total: u32) -> f32 {
    let span = total.saturating_sub(first).max(1);
    (frame.saturating_sub(first) as f32 / span as f32).clamp(0.0, 1.0)
}

/// Where a scrub at strip fraction `fx` seeks: just before a kill when it
/// lands on one's mark, else the frame under the finger. Pure.
fn scrub_target(fx: f32, first: u32, total: u32, kills: &[(u32, u8)]) -> u32 {
    let lo = first.max(1);
    let hi = total.saturating_sub(1).max(lo);
    if let Some((tick, _)) = kills
        .iter()
        .find(|(tick, _)| (strip_frac(*tick, first, total) - fx).abs() < KILL_SNAP)
    {
        return tick.saturating_sub(KILL_RUN_UP).clamp(lo, hi);
    }
    let span = total.saturating_sub(first) as f32;
    (first + (fx * span) as u32).clamp(lo, hi)
}

#[derive(Component)]
struct SpeedPip(usize);

#[derive(Component)]
struct TheaterMarquee;

/// Where saved tapes live — the recorder's dir, shared so the two modules
/// can never drift apart.
fn replays_dir() -> Option<PathBuf> {
    crate::recorder::replays_dir()
}

/// Scan the replay dir for tapes, newest first. Tapes from another sim
/// version are listed too — dimmed and labeled with their version — so a
/// version bump never reads as your replays vanishing. Only same-version
/// tapes are playable here; the rest wait for the archived tagged binary
/// (the no-migrations law). A tape whose FORMAT no longer decodes at all
/// stays invisible; there is nothing honest to say about bytes we can't
/// read.
fn scan_tapes() -> Vec<TapeEntry> {
    let Some(dir) = replays_dir() else {
        return Vec::new();
    };
    let mut tapes: Vec<(u64, TapeEntry)> = crate::paths::list_dir(&dir, "bmrg")
        .into_iter()
        .filter_map(|path| {
            let bytes = crate::paths::read_bytes(&path).ok()?;
            let h = &replay::decode_header(&bytes).ok()?;
            let foreign_version = (h.sim_version != sim::SIM_VERSION).then_some(h.sim_version);
            let winner_name = match h.winner {
                Some(w) => h.player_handles[w as usize % 2]
                    .clone()
                    .unwrap_or_else(|| if w == 0 { "CUR".into() } else { "STAG".into() }),
                None => "NOBODY".into(),
            };
            let secs = h.frame_count.saturating_sub(h.start_tick) / h.frame_rate.max(1) as u32;
            let mut line = format!(
                "{} {} WINS - {} - {}:{:02} - {}",
                kind_badge(h.kind),
                winner_name,
                arena_label(h.arena_id),
                secs / 60,
                secs % 60,
                date_label(h.recorded_at),
            );
            if let Some(v) = foreign_version {
                line.push_str(&format!(" - SIM v{v}"));
            }
            Some((
                h.recorded_at,
                TapeEntry {
                    path,
                    line,
                    foreign_version,
                },
            ))
        })
        .collect();
    tapes.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    tapes.into_iter().map(|(_, t)| t).collect()
}

/// The list's word for what a tape is. Pure.
fn kind_badge(kind: replay::TapeKind) -> String {
    match kind {
        replay::TapeKind::Duel => "DUEL".into(),
        replay::TapeKind::Practice { tier } => format!("BOT T{tier}"),
        replay::TapeKind::Shade => "SHADE".into(),
        replay::TapeKind::Lesson => "LESSON".into(),
        replay::TapeKind::Couch => "COUCH".into(),
    }
}

/// Which page of the list is showing ([`LIST_MAX`] tapes a page).
#[derive(Resource, Default)]
struct TapePage(usize);

fn page_count(tapes: usize) -> usize {
    tapes.div_ceil(LIST_MAX).max(1)
}

/// The subline: what to do, and the page when there is more than one.
/// Pure.
fn list_subline(tapes: usize, page: usize) -> String {
    match (tapes, page_count(tapes)) {
        (0, _) => "no tapes yet - every decided match saves one".into(),
        (_, 1) => "tap a tape to watch it".into(),
        (_, pages) => format!(
            "tap a tape to watch it\npage {}/{pages} - tap here for more",
            page + 1
        ),
    }
}

/// The subline's band (window-fraction, y-down): tapping it turns the page.
const SUBLINE_BAND: (f32, f32) = (0.13, 0.195);

/// Fill the row slots from the current page.
#[allow(clippy::type_complexity)]
fn update_tape_rows(
    list: Res<TapeList>,
    page: Res<TapePage>,
    mut rows: Query<(&TapeRow, &mut Text2d, &mut TextColor)>,
    mut sub: Query<&mut Text2d, (With<ReplaySubline>, Without<TapeRow>)>,
) {
    if !(list.is_changed() || page.is_changed()) {
        return;
    }
    for (row, mut text, mut color) in &mut rows {
        let tape = list.0.get(page.0 * LIST_MAX + row.0);
        text.0 = tape.map(|t| t.line.clone()).unwrap_or_default();
        // A foreign-version tape reads as an artifact, not an option.
        color.0 = if tape.is_some_and(|t| t.foreign_version.is_some()) {
            render::palette::BONE.with_alpha(0.35)
        } else {
            render::palette::BONE
        };
    }
    for mut text in &mut sub {
        text.0 = list_subline(list.0.len(), page.0);
    }
}

/// A tape header's arena byte, as the replay list prints it. An id this
/// build does not know reads as the first arena (`ArenaId::from_u8`).
fn arena_label(id: u8) -> &'static str {
    render::arena::info(sim::ArenaId::from_u8(id)).label
}

/// Unix seconds → "10 JUL" (UTC). The civil-from-days algorithm (Howard
/// Hinnant's) — no chrono dependency for one label.
pub(crate) fn date_label(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let (y, m, d) = civil_from_days(days);
    const MONTHS: [&str; 12] = [
        "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
    ];
    let _ = y;
    format!("{} {}", d, MONTHS[(m as usize - 1) % 12])
}

/// Days since 1970-01-01 → (year, month 1-12, day 1-31). Pure.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ---------------------------------------------------------------------------
// Replays list screen.
// ---------------------------------------------------------------------------

fn enter_replays(
    mut commands: Commands,
    mut list: ResMut<TapeList>,
    mut notice: ResMut<TapeNoticeState>,
    mut page: ResMut<TapePage>,
) {
    list.0 = scan_tapes();
    notice.0 = None;

    commands.spawn((
        ReplayScreenText,
        Text2d::new("REPLAYS"),
        (
            TextFont {
                font_size: crate::ui::TextRole::ScreenTitle.font_size(),
                ..default()
            },
            crate::ui::TextRole::ScreenTitle,
        ),
        TextColor(render::palette::HOT_BONE),
        TextLayout::justify(Justify::Center),
        ScreenAnchor::new(0.0, 0.78, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, crate::ui::z::MENU_TEXT),
    ));
    page.0 = 0;
    let sub = list_subline(list.0.len(), 0);
    commands.spawn((
        ReplayScreenText,
        ReplaySubline,
        Text2d::new(sub),
        (
            TextFont {
                font_size: crate::ui::TextRole::ScreenSubtitle.font_size(),
                ..default()
            },
            crate::ui::TextRole::ScreenSubtitle,
        ),
        TextColor(render::palette::BONE.with_alpha(0.7)),
        TextLayout::justify(Justify::Center),
        ScreenAnchor::new(0.0, 0.66, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, crate::ui::z::MENU_TEXT),
    ));
    // The refusal notice shares the subline's slot; `update_tape_notice`
    // swaps which of the two speaks.
    commands.spawn((
        ReplayScreenText,
        TapeNoticeText,
        Text2d::new(String::new()),
        (
            TextFont {
                font_size: crate::ui::TextRole::Notice.font_size(),
                ..default()
            },
            crate::ui::TextRole::Notice,
        ),
        TextColor(render::palette::EMBER),
        TextLayout::justify(Justify::Center),
        TextBounds::new_horizontal(1080.0),
        ScreenAnchor::new(0.0, 0.66, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, crate::ui::z::MENU_TEXT + 0.5),
        Visibility::Hidden,
    ));
    for i in 0..LIST_MAX {
        let fy = LIST_TOP + (i as f32 + 0.5) * LIST_PITCH;
        let tape = list.0.get(i);
        let color = if tape.is_some_and(|t| t.foreign_version.is_some()) {
            render::palette::BONE.with_alpha(0.35)
        } else {
            render::palette::BONE
        };
        commands.spawn((
            ReplayScreenText,
            TapeRow(i),
            Text2d::new(tape.map(|t| t.line.clone()).unwrap_or_default()),
            (
                TextFont {
                    font_size: crate::ui::TextRole::ListRow.font_size(),
                    ..default()
                },
                crate::ui::TextRole::ListRow,
            ),
            TextColor(color),
            TextLayout::justify(Justify::Center),
            ScreenAnchor::new(0.0, crate::ui::anchor_y(fy), 0.0, 0.0),
            Transform::from_xyz(0.0, 0.0, crate::ui::z::MENU_TEXT),
        ));
    }
    // Tape trading, stated where the tapes are: a match tape is a small
    // file, and the deterministic sim means a friend's tape plays HERE
    // exactly as it played there. The dir line makes the folder findable
    // from a Files app in both directions (copy out to send, drop in to
    // watch). One-tap sharing wants a FileProvider manifest entry that
    // cargo-apk can't express today — the folder IS the feature for now.
    if let Some(dir) = replays_dir() {
        commands.spawn((
            ReplayScreenText,
            Text2d::new("tapes are files - swap them with a friend, they play here"),
            (
                TextFont {
                    font_size: crate::ui::TextRole::ListDetail.font_size(),
                    ..default()
                },
                crate::ui::TextRole::ListDetail,
            ),
            TextColor(render::palette::BONE.with_alpha(0.55)),
            TextLayout::justify(Justify::Center),
            ScreenAnchor::new(0.0, crate::ui::anchor_y(0.79), 0.0, 0.0),
            Transform::from_xyz(0.0, 0.0, crate::ui::z::MENU_TEXT),
        ));
        commands.spawn((
            ReplayScreenText,
            Text2d::new(dir.display().to_string()),
            (
                TextFont {
                    font_size: crate::ui::TextRole::ListFine.font_size(),
                    ..default()
                },
                crate::ui::TextRole::ListFine,
            ),
            TextColor(render::palette::BONE.with_alpha(0.4)),
            TextLayout::justify(Justify::Center),
            TextBounds::new_horizontal(1000.0),
            ScreenAnchor::new(0.0, crate::ui::anchor_y(0.825), 0.0, 0.0),
            Transform::from_xyz(0.0, 0.0, crate::ui::z::MENU_TEXT),
        ));
    }
    // BACK wears the same bordered-box language as every other button.
    let back_anchor_y = crate::ui::band_anchor_y(BACK_BAND);
    commands.spawn((
        ReplayScreenText,
        Sprite {
            color: render::palette::HOT_BONE,
            custom_size: Some(Vec2::new(362.0, 98.0)),
            ..default()
        },
        ScreenAnchor::new(0.0, back_anchor_y, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, crate::ui::z::MENU_TEXT - 1.0),
    ));
    commands.spawn((
        ReplayScreenText,
        Sprite {
            color: render::palette::DEEP_ASH,
            custom_size: Some(Vec2::new(340.0, 76.0)),
            ..default()
        },
        ScreenAnchor::new(0.0, back_anchor_y, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, crate::ui::z::MENU_TEXT - 0.5),
    ));
    commands.spawn((
        ReplayScreenText,
        Text2d::new("BACK"),
        (
            TextFont {
                font_size: crate::ui::TextRole::Back.font_size(),
                ..default()
            },
            crate::ui::TextRole::Back,
        ),
        TextColor(render::palette::HOT_BONE),
        TextLayout::justify(Justify::Center),
        ScreenAnchor::new(0.0, back_anchor_y, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, crate::ui::z::MENU_TEXT),
    ));
}

fn exit_replays(mut commands: Commands, q: Query<Entity, With<ReplayScreenText>>) {
    for e in &q {
        commands.entity(e).despawn();
    }
}

/// List-screen input: tap a row to load its tape, BACK / Escape to leave.
fn replays_input(world: &mut World) {
    let win = world.resource::<WindowSize>().0;
    let mut tapped_row: Option<usize> = None;
    let mut next_page = false;
    let mut back = world
        .resource::<ButtonInput<KeyCode>>()
        .just_pressed(KeyCode::Escape);

    {
        let touches = world.resource::<Touches>();
        if win.y > 0.0 {
            for t in touches.iter_just_pressed() {
                let fy = t.position().y / win.y;
                if (BACK_BAND.0..BACK_BAND.1).contains(&fy) {
                    back = true;
                } else if (SUBLINE_BAND.0..SUBLINE_BAND.1).contains(&fy) {
                    next_page = true;
                } else if fy >= LIST_TOP {
                    let row = ((fy - LIST_TOP) / LIST_PITCH) as usize;
                    if fy < LIST_TOP + LIST_PITCH * LIST_MAX as f32 {
                        tapped_row = Some(row);
                    }
                }
            }
        }
        // Desktop dev path: 1-8 pick a row directly, N turns the page.
        let keys = world.resource::<ButtonInput<KeyCode>>();
        if keys.just_pressed(KeyCode::KeyN) || keys.just_pressed(KeyCode::PageDown) {
            next_page = true;
        }
        for (key, row) in [
            (KeyCode::Digit1, 0usize),
            (KeyCode::Digit2, 1),
            (KeyCode::Digit3, 2),
            (KeyCode::Digit4, 3),
            (KeyCode::Digit5, 4),
            (KeyCode::Digit6, 5),
            (KeyCode::Digit7, 6),
            (KeyCode::Digit8, 7),
        ] {
            if keys.just_pressed(key) {
                tapped_row = Some(row);
            }
        }
    }

    if back {
        world
            .resource_mut::<NextState<AppScreen>>()
            .set(AppScreen::Title);
        return;
    }
    if next_page {
        let pages = page_count(world.resource::<TapeList>().0.len());
        let mut page = world.resource_mut::<TapePage>();
        page.0 = (page.0 + 1) % pages;
        return;
    }
    // A row is a slot on the current page.
    let mut tapped_row = tapped_row.map(|row| world.resource::<TapePage>().0 * LIST_MAX + row);
    // TWOTOP_AUTOPLAY_TAPE=1: roll the newest tape on arrival, once per
    // process (headless capture verification of the theater, pairing with
    // TWOTOP_AUTOSTART=replays; the once-latch keeps exiting playback from
    // re-rolling forever).
    static AUTOPLAYED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if tapped_row.is_none()
        && crate::config::Knob::AutoplayTape.is_on()
        && !world.resource::<TapeList>().0.is_empty()
        && !AUTOPLAYED.swap(true, std::sync::atomic::Ordering::Relaxed)
    {
        tapped_row = Some(0);
    }
    let Some(row) = tapped_row else {
        return;
    };
    let path = {
        let list = world.resource::<TapeList>();
        let Some(entry) = list.0.get(row) else {
            return;
        };
        if let Some(v) = entry.foreign_version {
            tracing::info!(
                target: "two_top::theater",
                tape_version = v,
                "tape speaks sim v{v}; this build speaks v{} — archived binaries own it",
                sim::SIM_VERSION,
            );
            raise_tape_notice(
                world,
                format!(
                    "that tape speaks SIM v{v} - this build speaks v{}\nold tapes play on the build that recorded them",
                    sim::SIM_VERSION,
                ),
            );
            return;
        }
        entry.path.clone()
    };
    let Ok(bytes) = crate::paths::read_bytes(&path) else {
        tracing::warn!(target: "two_top::theater", path = %path.display(), "tape unreadable");
        raise_tape_notice(world, "that tape would not read from disk".to_string());
        return;
    };
    let Ok(replay) = decode_for_sim_version(&bytes, sim::SIM_VERSION) else {
        tracing::warn!(target: "two_top::theater", path = %path.display(), "tape rejected (version/format)");
        raise_tape_notice(
            world,
            "that tape would not decode - not a playable recording".to_string(),
        );
        return;
    };
    // The share surface needs to know which tape is on the projector.
    world.resource_mut::<crate::share::SharedTapePath>().0 = Some(path.clone());
    start_playback(world, replay);
    crate::verify::note(world, &bytes);
}

/// Every refusal a tape tap can hit answers on screen through here — the
/// log line is for the operator, this is for the thumb.
fn raise_tape_notice(world: &mut World, msg: String) {
    let now = world.resource::<Time<Real>>().elapsed_secs();
    world.resource_mut::<TapeNoticeState>().0 = Some((msg, now));
}

/// Swap the subline for the live refusal notice, and put it back once the
/// notice has said its piece.
#[allow(clippy::type_complexity)]
fn update_tape_notice(
    time: Res<Time<Real>>,
    mut state: ResMut<TapeNoticeState>,
    mut notice: Query<
        (&mut Text2d, &mut Visibility),
        (With<TapeNoticeText>, Without<ReplaySubline>),
    >,
    mut subline: Query<&mut Visibility, (With<ReplaySubline>, Without<TapeNoticeText>)>,
) {
    let live = match &state.0 {
        Some((_, at)) if time.elapsed_secs() - at > TAPE_NOTICE_SECS => {
            state.0 = None;
            false
        }
        Some(_) => true,
        None => false,
    };
    if let Ok((mut text, mut vis)) = notice.single_mut() {
        if live {
            if let Some((msg, _)) = &state.0 {
                text.0.clone_from(msg);
            }
            *vis = Visibility::Visible;
        } else {
            *vis = Visibility::Hidden;
        }
    }
    if let Ok(mut vis) = subline.single_mut() {
        *vis = if live {
            Visibility::Hidden
        } else {
            Visibility::Visible
        };
    }
}

/// A tape to roll at its last kill, asked for by WATCH THE KILL on a
/// summary; the Title hands it to the theater ([`roll_pending_watch`]).
#[derive(Resource, Default)]
pub struct PendingWatch(pub Option<PathBuf>);

/// On the Title with a pending WATCH THE KILL: roll the tape from two
/// seconds before its last kill, at half speed.
fn roll_pending_watch(world: &mut World) {
    let Some(path) = world.resource_mut::<PendingWatch>().0.take() else {
        return;
    };
    let Ok(bytes) = crate::paths::read_bytes(&path) else {
        return;
    };
    let Ok(replay) = decode_for_sim_version(&bytes, sim::SIM_VERSION) else {
        return;
    };
    let at = replay
        .header
        .kills
        .last()
        .map(|(tick, _)| tick.saturating_sub(KILL_RUN_UP));
    world.resource_mut::<crate::share::SharedTapePath>().0 = Some(path);
    start_playback(world, replay);
    crate::verify::note(world, &bytes);
    let mut controls = world.resource_mut::<TheaterControls>();
    if let Some(at) = at {
        controls.seek_target = Some(at.max(controls.seek_target.unwrap_or(0)));
    }
    controls.speed_idx = 0; // half speed: the kill is the point
}

/// Is the projector still chasing a seek?
pub(crate) fn seeking(world: &World) -> bool {
    world
        .get_resource::<TheaterControls>()
        .is_some_and(|c| c.seek_target.is_some())
}

/// Roll the tape on the projector to just before its last kill, at full
/// speed (CLIP's moment); from the top when it has no kill.
pub(crate) fn roll_to_last_kill(world: &mut World) {
    let (first, at) = {
        let theater = world.resource::<TheaterMode>();
        (
            theater.first_frame,
            theater
                .kills
                .last()
                .map(|(tick, _)| tick.saturating_sub(KILL_RUN_UP)),
        )
    };
    let mut controls = world.resource_mut::<TheaterControls>();
    controls.seek_target = Some(at.unwrap_or(first).max(first).max(1));
    controls.paused = false;
    controls.speed_idx = 1;
}

/// Load a decoded tape and roll it: arena from the header, playback
/// resources in, then enter the match screen (whose spawn path sees the
/// theater flag and builds the playback session).
pub(crate) fn start_playback(world: &mut World, replay: Replay) {
    let header = replay.header.clone();
    let prev_arena = world.resource::<sim::SelectedArena>().0;
    world.resource_mut::<sim::SelectedArena>().0 = sim::ArenaId::from_u8(header.arena_id);
    // The theater is a spectator seat: no perspective flip, P0 near.
    world.resource_mut::<render::PerspectiveFlip>().0 = 1.0;

    {
        let mut theater = world.resource_mut::<TheaterMode>();
        theater.active = true;
        theater.total_frames = header.frame_count;
        theater.first_frame = header.start_tick.min(header.frame_count);
        theater.kills = header.kills.clone();
        theater.prev_arena = Some(prev_arena);
        theater.names = header.player_handles.clone();
        theater.live = false;
    }
    world.insert_resource(ReplayPlayback::new(replay));
    // A tape with a seal says so through `verify::note`, after this.
    world.insert_resource(crate::verify::SealCheck::Unsealed);
    // A later match of a session tape: fast-forward to where it starts.
    world.insert_resource(TheaterControls {
        seek_target: (header.start_tick > 0).then_some(header.start_tick),
        ..TheaterControls::default()
    });
    world.insert_resource(SnapshotBuffer::default());
    world
        .resource_mut::<NextState<AppScreen>>()
        .set(AppScreen::InMatch);
    tracing::info!(
        target: "two_top::theater",
        frames = header.frame_count,
        arena = header.arena_id,
        "tape rolling",
    );
}

/// Build the playback session — the viewer's exact config. `check_distance`
/// 0 and `input_delay` 0 are load-bearing for snapshot scrubbing (bevy_ggrs's
/// verification ring and delay buffer both desync a restored world).
pub fn build_playback_session() -> bevy_ggrs::Session<sim::GgrsCfg> {
    use bevy_ggrs::ggrs::{PlayerType, SessionBuilder};
    let mut sb = SessionBuilder::<sim::GgrsCfg>::new()
        .with_num_players(2)
        .expect("with_num_players")
        .with_check_distance(0)
        .with_input_delay(0);
    for i in 0..2 {
        sb = sb.add_player(PlayerType::Local, i).expect("add_player");
    }
    bevy_ggrs::Session::SyncTest(sb.start_synctest_session().expect("synctest"))
}

// ---------------------------------------------------------------------------
// Theater playback (InMatch with the theater flag up).
// ---------------------------------------------------------------------------

fn spawn_theater_ui(mut commands: Commands, theater: Res<TheaterMode>) {
    if !theater.active {
        return;
    }
    tracing::info!(target: "two_top::theater", "step: spawn_theater_ui");
    // The exit. Every menu screen puts a bordered BACK box at the tap target;
    // the theater used to make do with a thin line of text at the top, easy
    // to miss when the bottom (where a menu's BACK lives) is the scrub strip.
    // Give it the same button chrome so it reads as the exit it is. The whole
    // top band taps to leave (`EXIT_BAND` in `theater_input`); the box just
    // shows where. Border, fill, label, all tagged for one-shot teardown.
    let exit_y = crate::screen::TOP_EXIT_ANCHOR_Y;
    commands.spawn((
        TheaterMarquee,
        Sprite {
            color: render::palette::HOT_BONE,
            custom_size: Some(Vec2::new(322.0, 98.0)),
            ..default()
        },
        ScreenAnchor::new(0.0, exit_y, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, crate::ui::z::MENU_TEXT),
    ));
    commands.spawn((
        TheaterMarquee,
        Sprite {
            color: render::palette::DEEP_ASH,
            custom_size: Some(Vec2::new(300.0, 76.0)),
            ..default()
        },
        ScreenAnchor::new(0.0, exit_y, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, crate::ui::z::MENU_TEXT + 0.5),
    ));
    commands.spawn((
        TheaterMarquee,
        Text2d::new("BACK"),
        (
            TextFont {
                font_size: crate::ui::TextRole::Back.font_size(),
                ..default()
            },
            crate::ui::TextRole::Back,
        ),
        TextColor(render::palette::HOT_BONE),
        TextLayout::justify(Justify::Center),
        ScreenAnchor::new(0.0, exit_y, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, crate::ui::z::MENU_TEXT + 1.0),
    ));
    // Scrub track + fill (sized per-frame against the live view rect).
    commands.spawn((
        TheaterMarquee,
        ScrubPart(ScrubRole::Track),
        Sprite {
            color: render::palette::COLD_STONE.with_alpha(0.45),
            custom_size: Some(Vec2::ZERO),
            ..default()
        },
        Transform::from_xyz(0.0, 0.0, crate::ui::z::MENU_TEXT - 1.0),
    ));
    commands.spawn((
        TheaterMarquee,
        ScrubPart(ScrubRole::Fill),
        Sprite {
            color: render::palette::HOT_BONE,
            custom_size: Some(Vec2::ZERO),
            ..default()
        },
        bevy::sprite::Anchor::CENTER_LEFT,
        Transform::from_xyz(0.0, 0.0, crate::ui::z::MENU_TEXT - 0.5),
    ));
    // The kill marks: one tick on the strip per kill, in the victim's
    // stain color, so the match's story reads at a glance.
    for (n, (_, victim)) in theater.kills.iter().take(KILL_MARKS).enumerate() {
        commands.spawn((
            TheaterMarquee,
            ScrubPart(ScrubRole::Kill(n)),
            Sprite {
                color: if *victim == 0 {
                    render::palette::P0_BLOOD
                } else {
                    render::palette::P1_CYAN
                },
                custom_size: Some(Vec2::ZERO),
                ..default()
            },
            Transform::from_xyz(0.0, 0.0, crate::ui::z::MENU_TEXT),
        ));
    }
    // Speed pips.
    for (i, speed) in SPEED_PRESETS.iter().enumerate() {
        let fx = 0.5 + (i as f32 - (SPEED_PRESETS.len() as f32 - 1.0) / 2.0) * 0.16;
        commands.spawn((
            TheaterMarquee,
            SpeedPip(i),
            Text2d::new(format!("{speed}x")),
            (
                TextFont {
                    font_size: crate::ui::TextRole::Marquee.font_size(),
                    ..default()
                },
                crate::ui::TextRole::Marquee,
            ),
            TextColor(render::palette::BONE),
            TextLayout::justify(Justify::Center),
            ScreenAnchor::new(
                crate::ui::anchor_x(fx),
                crate::ui::band_anchor_y(SPEED_BAND),
                0.0,
                0.0,
            ),
            Transform::from_xyz(0.0, 0.0, crate::ui::z::MENU_TEXT),
        ));
    }
}

fn despawn_theater_ui(mut commands: Commands, q: Query<Entity, With<TheaterMarquee>>) {
    for e in &q {
        commands.entity(e).despawn();
    }
}

/// Theater input: top strip exits, scrub band seeks, speed pips switch,
/// anywhere else toggles pause. Desktop: Space / arrows / Home / End / Esc.
#[allow(clippy::too_many_arguments)]
fn theater_input(
    theater: Res<TheaterMode>,
    keys: Res<ButtonInput<KeyCode>>,
    touches: Res<Touches>,
    window: Res<WindowSize>,
    frame: Res<sim::FrameCount>,
    controls: Option<ResMut<TheaterControls>>,
    share: Res<crate::share::ShareTapBand>,
    mut next: ResMut<NextState<AppScreen>>,
) {
    // The transport only exists while a tape rolls — a live match reaches
    // this system too (same screen state), so the absence is normal.
    let Some(mut controls) = controls else {
        return;
    };
    if !theater.active {
        return;
    }
    let total = theater.total_frames;
    let current = frame.0;

    if keys.just_pressed(KeyCode::Escape) {
        next.set(AppScreen::Replays);
        return;
    }
    if keys.just_pressed(KeyCode::Space) {
        controls.paused = !controls.paused;
        if !controls.paused {
            controls.seek_target = None;
        }
    }
    if keys.just_pressed(KeyCode::ArrowRight) {
        controls.paused = true;
        controls.seek_target = Some(current.saturating_add(1).min(total.saturating_sub(1)));
    }
    if keys.just_pressed(KeyCode::ArrowLeft) {
        controls.paused = true;
        controls.seek_target = Some(current.saturating_sub(1));
    }
    if keys.just_pressed(KeyCode::Home) {
        controls.paused = true;
        controls.seek_target = Some(theater.first_frame.max(1));
    }
    if keys.just_pressed(KeyCode::End) {
        controls.paused = true;
        controls.seek_target = Some(total.saturating_sub(1));
    }

    let win = window.0;
    if win.x <= 0.0 || win.y <= 0.0 {
        return;
    }
    for t in touches.iter_just_pressed() {
        let p = t.position();
        let (fx, fy) = (p.x / win.x, p.y / win.y);
        if (EXIT_BAND.0..EXIT_BAND.1).contains(&fy) {
            next.set(AppScreen::Replays);
            return;
        }
        if (SCRUB_BAND.0..SCRUB_BAND.1).contains(&fy) || (SPEED_BAND.0..SPEED_BAND.1).contains(&fy)
        {
            if (SPEED_BAND.0..SPEED_BAND.1).contains(&fy) {
                let idx = (fx * SPEED_PRESETS.len() as f32) as usize;
                controls.speed_idx = idx.min(SPEED_PRESETS.len() - 1);
            }
            continue; // scrub presses are handled with drags below
        }
        // The SHARE label's own tap is SHARE's, not a pause; CLIP's too.
        if share.hit(p, win) || crate::clip::hit(p, win, theater.active) {
            continue;
        }
        // Anywhere else: transport toggle.
        controls.paused = !controls.paused;
        if !controls.paused {
            controls.seek_target = None;
        }
    }
    // Press OR drag along the scrub strip seeks. ONE finger owns the scrub
    // (lowest touch id = the finger that landed first): two touches in the
    // band alternating positions frame-to-frame would defeat any debounce.
    // A seek fires only when that finger has moved a real distance since
    // the last one (`scrub_debounce`); the anchor clears the moment no
    // finger is on the strip, so the next scrub starts fresh. This is what
    // keeps a landed (possibly overshot) playhead from being re-targeted
    // into an oscillation.
    let scrub_touch = touches
        .iter()
        .filter(|t| (SCRUB_BAND.0..SCRUB_BAND.1).contains(&(t.position().y / win.y)))
        .min_by_key(|t| t.id());
    if let Some(t) = scrub_touch {
        let fx = (t.position().x / win.x).clamp(0.0, 1.0);
        // A tape under two frames has no interior to scrub to — and
        // `clamp(1, 0)` is a release-mode panic (`min > max`). The codec
        // now refuses a header that lies about its count, but a 0- or
        // 1-frame tape is a legitimate encoding, so the guard stays.
        if total >= 2 && scrub_debounce(fx, &mut controls.scrub_anchor) {
            let target = scrub_target(fx, theater.first_frame, total, &theater.kills);
            controls.paused = true;
            controls.seek_target = Some(target);
        }
    } else {
        controls.scrub_anchor = None;
    }
}

/// Snapshot capture during forward playback (viewer's `capture_snapshot`).
fn theater_capture_snapshot(world: &mut World) {
    if !world.resource::<TheaterMode>().active {
        return;
    }
    let frame = world.resource::<sim::FrameCount>().0;
    if world.resource::<TheaterControls>().seek_target.is_some() {
        return;
    }
    if !should_snapshot(frame) {
        return;
    }
    // Idempotent by frame: after a backward seek we replay frames that were
    // already banked on the first pass, and a duplicate would bloat the ring
    // (and, since the ring is no longer pruned, accumulate without bound).
    let already = world
        .resource::<SnapshotBuffer>()
        .entries
        .iter()
        .any(|s| s.frame == frame);
    if already {
        return;
    }
    let snap = SimSnapshot::capture(world);
    let mut buf = world.resource_mut::<SnapshotBuffer>();
    buf.entries.push(snap);
    tracing::info!(
        target: "two_top::theater",
        frame,
        snapshots = buf.entries.len(),
        "step: snapshot captured",
    );
}

/// The seek state machine (viewer's `drive_seek`), plus end-of-tape pause.
fn theater_drive_seek(world: &mut World) {
    if !world.resource::<TheaterMode>().active {
        return;
    }
    let controls = *world.resource::<TheaterControls>();
    let current = world.resource::<sim::FrameCount>().0;
    let total = world.resource::<TheaterMode>().total_frames;

    // Tape exhausted in normal play: hold on the last frame instead of
    // letting the sim idle forward on neutral inputs. A live tape that has
    // run dry only waits (a few ticks short of the edge, so one frame's
    // burst of ticks never runs past what has arrived); it plays on by
    // itself when more arrives.
    if controls.seek_target.is_none() && !controls.paused {
        let cursor = world.resource::<ReplayPlayback>().cursor as u32;
        if world.resource::<TheaterMode>().live {
            if cursor + LIVE_EDGE >= total {
                crate::transport::pause(world);
                return;
            }
        } else if cursor >= total {
            world.resource_mut::<TheaterControls>().paused = true;
        }
    }

    let controls = *world.resource::<TheaterControls>();
    if let Some(target) = controls.seek_target {
        tracing::info!(target: "two_top::theater", seek = target, current, "step: seek");
        if target < current {
            // The snapshot ring is a monotonic read cache built during
            // forward play; NEVER prune it on a seek. An earlier revision
            // ran `retain(|s| s.frame < target)` here, which gutted the ring
            // a little more on every backward scrub until nearest_before
            // returned nothing and seeking silently died. A frame's snapshot
            // is deterministic, so a later forward seek can still reuse it.
            let nearest = world
                .resource::<SnapshotBuffer>()
                .nearest_before(target)
                .cloned();
            if let Some(snap) = nearest {
                snap.restore(world);
                world.resource_mut::<ReplayPlayback>().cursor = snap.frame as usize;
            } else {
                // Seeking earlier than the first snapshot (frame 1). Snap to
                // it — the earliest playable state we hold — rather than the
                // spawn state at frame 0, which the despawn/spawn cycle owns.
                let earliest = world.resource::<SnapshotBuffer>().entries.first().cloned();
                if let Some(snap) = earliest {
                    snap.restore(world);
                    world.resource_mut::<ReplayPlayback>().cursor = snap.frame as usize;
                } else {
                    world.resource_mut::<TheaterControls>().seek_target = None;
                    return;
                }
            }
        }
        // Forward: re-simulate straight to the target from the tape's own
        // inputs (`sim::fast_forward`), a chunk per frame, banking the
        // scrub snapshots on the way. Instant for a scrub; a few frames
        // for a whole match on a slow phone.
        fast_seek(world, target);
        let now = world.resource::<sim::FrameCount>().0;
        if now >= target {
            world.resource_mut::<TheaterControls>().seek_target = None;
            theater_apply_play_pause(world, controls.paused);
        } else {
            crate::transport::pause(world);
        }
    } else {
        theater_apply_play_pause(world, controls.paused);
    }
}

/// Ticks a forward seek re-simulates per frame.
const SEEK_CHUNK: usize = 1_500;

/// Run the tape forward toward `target` (at most [`SEEK_CHUNK`] ticks this
/// frame) outside the playback session, advancing its cursor to match.
fn fast_seek(world: &mut World, target: u32) {
    let now = world.resource::<sim::FrameCount>().0;
    if now >= target {
        return;
    }
    let inputs = {
        let playback = world.resource::<ReplayPlayback>();
        let from = playback.cursor.min(playback.replay.inputs.len());
        let to = (from + (target - now) as usize)
            .min(from + SEEK_CHUNK)
            .min(playback.replay.inputs.len());
        playback.replay.inputs[from..to].to_vec()
    };
    if inputs.is_empty() {
        return;
    }
    let n = inputs.len();
    sim::fast_forward(world, &inputs, |world| {
        let frame = world.resource::<sim::FrameCount>().0;
        if should_snapshot(frame)
            && !world
                .resource::<SnapshotBuffer>()
                .entries
                .iter()
                .any(|s| s.frame == frame)
        {
            let snap = SimSnapshot::capture(world);
            world.resource_mut::<SnapshotBuffer>().entries.push(snap);
        }
    });
    world.resource_mut::<ReplayPlayback>().cursor += n;
}

fn theater_apply_play_pause(world: &mut World, paused: bool) {
    let mut speed = SPEED_PRESETS[world.resource::<TheaterControls>().speed_idx];
    let theater = world.resource::<TheaterMode>();
    if theater.live {
        let cursor = world.resource::<ReplayPlayback>().cursor as u32;
        speed *= live_speed(theater.total_frames.saturating_sub(cursor));
    }
    crate::transport::apply(world, paused, speed);
}

/// Ticks a live viewer keeps between the playhead and the newest tick
/// that has arrived.
const LIVE_EDGE: u32 = 8;
/// Behind by more than this, a live viewer plays fast until caught up.
const LIVE_BEHIND: u32 = 3 * sim::TICK_HZ as u32;

/// The live viewer's pace multiplier, `ahead` ticks from the newest. Pure.
fn live_speed(ahead: u32) -> f32 {
    if ahead > LIVE_BEHIND { 1.25 } else { 1.0 }
}

/// Mark the tape just rolled as live (`open`) or finished.
pub(crate) fn set_live(world: &mut World, open: bool) {
    world.resource_mut::<TheaterMode>().live = open;
}

/// Ticks that have just arrived for the live tape on the projector.
pub(crate) fn extend_live(world: &mut World, pairs: &[replay::FrameInputs], open: bool) {
    if !world.resource::<TheaterMode>().active {
        return;
    }
    let (had, cursor) = {
        let mut playback = world.resource_mut::<ReplayPlayback>();
        let had = playback.replay.inputs.len();
        playback.replay.inputs.extend_from_slice(pairs);
        let total = playback.replay.inputs.len() as u32;
        playback.replay.header.frame_count = total;
        (had, playback.cursor)
    };
    {
        let mut theater = world.resource_mut::<TheaterMode>();
        theater.total_frames += pairs.len() as u32;
        theater.live = open;
    }
    // A frame hitch ran the sim past what had arrived, on neutral input:
    // go back to the last good tick and play the real ones.
    if cursor > had && !pairs.is_empty() {
        tracing::warn!(target: "two_top::live", cursor, had, "ran past the live edge — re-seeking");
        world.resource_mut::<TheaterControls>().seek_target = Some(had as u32);
    }
}

/// Scrub bar + speed pip rendering. Sized against the live view rect so the
/// bar spans the real screen on any aspect (and under the kill-cam zoom).
#[allow(clippy::type_complexity)]
fn theater_update_ui(
    theater: Res<TheaterMode>,
    controls: Option<Res<TheaterControls>>,
    frame: Res<sim::FrameCount>,
    rect: Res<ViewRect>,
    mut scrub: Query<(&ScrubPart, &mut Sprite, &mut Transform)>,
    mut pips: Query<(&SpeedPip, &mut TextColor), Without<ScrubPart>>,
) {
    if !theater.active || rect.half == Vec2::ZERO {
        return;
    }
    let Some(controls) = controls else {
        return;
    };
    let total = theater.total_frames.max(1);
    let progress = strip_frac(frame.0, theater.first_frame, total);

    // Band center/height in world units through the view rect.
    let band_center_frac = crate::ui::band_anchor_y(SCRUB_BAND); // y-up anchor frac
    let center_y = rect.center.y + rect.half.y * band_center_frac;
    let width = rect.half.x * 2.0 * 0.92;
    let height = rect.half.y * 2.0 * 0.012;
    for (part, mut sprite, mut tx) in &mut scrub {
        match part.0 {
            ScrubRole::Track => {
                sprite.custom_size = Some(Vec2::new(width, height));
                tx.translation.x = rect.center.x;
                tx.translation.y = center_y;
            }
            ScrubRole::Fill => {
                sprite.custom_size = Some(Vec2::new(width * progress, height * 1.9));
                tx.translation.x = rect.center.x - width * 0.5;
                tx.translation.y = center_y;
            }
            ScrubRole::Kill(n) => {
                let Some((tick, _)) = theater.kills.get(n) else {
                    continue;
                };
                let f = strip_frac(*tick, theater.first_frame, total);
                sprite.custom_size = Some(Vec2::new(height * 0.9, height * 3.4));
                tx.translation.x = rect.center.x - width * 0.5 + width * f;
                tx.translation.y = center_y;
            }
        }
    }
    for (pip, mut color) in &mut pips {
        color.0 = if pip.0 == controls.speed_idx {
            render::palette::HOT_BONE
        } else {
            render::palette::BONE.with_alpha(0.5)
        };
    }
}

/// OnExit(InMatch): if the theater was rolling, put the room back the way
/// it was — playback resources out, virtual time back to realtime, the
/// player's arena pick restored.
fn theater_teardown(world: &mut World) {
    tracing::info!(target: "two_top::theater", "step: teardown");
    world.resource_mut::<crate::share::SharedTapePath>().0 = None;
    let (was_active, prev_arena) = {
        let theater = world.resource::<TheaterMode>();
        (theater.active, theater.prev_arena)
    };
    if !was_active {
        return;
    }
    {
        let mut theater = world.resource_mut::<TheaterMode>();
        theater.active = false;
        theater.prev_arena = None;
        theater.names = [None, None];
        theater.live = false;
    }
    world.remove_resource::<crate::live::LiveFeed>();
    world.remove_resource::<ReplayPlayback>();
    world.remove_resource::<TheaterControls>();
    world.remove_resource::<SnapshotBuffer>();
    if let Some(arena) = prev_arena {
        world.resource_mut::<sim::SelectedArena>().0 = arena;
    }
    crate::transport::realtime(world);
}

pub struct TheaterPlugin;

impl Plugin for TheaterPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TheaterMode>()
            .init_resource::<TapeList>()
            .init_resource::<TapeNoticeState>()
            .init_resource::<TapePage>()
            .init_resource::<PendingWatch>()
            .add_systems(
                Update,
                roll_pending_watch.run_if(in_state(AppScreen::Title)),
            )
            .add_systems(OnEnter(AppScreen::Replays), enter_replays)
            .add_systems(OnExit(AppScreen::Replays), exit_replays)
            .add_systems(OnEnter(AppScreen::InMatch), spawn_theater_ui)
            .add_systems(
                OnExit(AppScreen::InMatch),
                (despawn_theater_ui, theater_teardown),
            )
            .add_systems(
                Update,
                (
                    (replays_input, update_tape_rows, update_tape_notice)
                        .chain()
                        .run_if(in_state(AppScreen::Replays)),
                    // Ordered: the seek `theater_input` issues this frame must
                    // be consumed by `theater_drive_seek` this frame, not next,
                    // or the transport lags a frame behind every tap.
                    (theater_input, theater_capture_snapshot, theater_drive_seek)
                        .chain()
                        .run_if(in_state(AppScreen::InMatch)),
                    theater_update_ui.after(ScreenAnchorSet),
                ),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_live_viewer_hurries_only_when_well_behind() {
        assert_eq!(live_speed(0), 1.0);
        assert_eq!(live_speed(LIVE_BEHIND), 1.0);
        assert_eq!(live_speed(LIVE_BEHIND + 1), 1.25);
    }

    #[test]
    fn arriving_ticks_grow_the_tape_and_repair_an_overrun() {
        let mut world = World::new();
        let tape = replay::Replay {
            header: replay::ReplayHeader {
                magic: replay::MAGIC,
                format_version: replay::FORMAT_VERSION,
                sim_version: sim::SIM_VERSION,
                seed: 0,
                num_players: 2,
                frame_rate: sim::TICK_HZ as u8,
                frame_count: 10,
                recorded_at: 0,
                winner: None,
                player_handles: [None, None],
                arena_id: 0,
                start_tick: 0,
                kind: replay::TapeKind::Duel,
                kills: Vec::new(),
            },
            inputs: vec![[sim::PlayerInput::default(); 2]; 10],
        };
        world.insert_resource(TheaterMode {
            active: true,
            total_frames: 10,
            live: true,
            ..default()
        });
        world.insert_resource(TheaterControls::default());
        world.insert_resource(ReplayPlayback::new(tape));
        // In step: the playhead behind the edge.
        world.resource_mut::<ReplayPlayback>().cursor = 8;
        extend_live(&mut world, &[[sim::PlayerInput::default(); 2]; 5], true);
        assert_eq!(world.resource::<TheaterMode>().total_frames, 15);
        assert_eq!(
            world.resource::<ReplayPlayback>().replay.header.frame_count,
            15
        );
        assert_eq!(world.resource::<TheaterControls>().seek_target, None);
        // A hitch ran past the edge on made-up input: back to the edge.
        world.resource_mut::<ReplayPlayback>().cursor = 17;
        extend_live(&mut world, &[[sim::PlayerInput::default(); 2]; 5], false);
        assert_eq!(world.resource::<TheaterControls>().seek_target, Some(15));
        assert!(
            !world.resource::<TheaterMode>().live(),
            "the tape has ended"
        );
    }

    /// Playback's tap targets: leave, speed, scrub. None may overlap.
    #[test]
    fn the_list_pages_and_says_so() {
        assert_eq!(page_count(0), 1);
        assert_eq!(page_count(LIST_MAX), 1);
        assert_eq!(page_count(LIST_MAX + 1), 2);
        assert!(list_subline(0, 0).starts_with("no tapes"));
        assert_eq!(list_subline(3, 0), "tap a tape to watch it");
        assert!(list_subline(LIST_MAX * 2 + 1, 1).contains("page 2/3"));
    }

    #[test]
    fn the_strip_spans_the_tapes_own_match() {
        assert_eq!(strip_frac(1_000, 1_000, 3_000), 0.0);
        assert_eq!(strip_frac(2_000, 1_000, 3_000), 0.5);
        assert_eq!(
            strip_frac(500, 1_000, 3_000),
            0.0,
            "before the match clamps"
        );
        // A scrub maps into the match, never before it.
        assert_eq!(scrub_target(0.0, 1_000, 3_000, &[]), 1_000);
        assert_eq!(scrub_target(0.5, 1_000, 3_000, &[]), 2_000);
        assert_eq!(scrub_target(1.0, 1_000, 3_000, &[]), 2_999);
    }

    #[test]
    fn a_scrub_on_a_kill_mark_lands_just_before_the_kill() {
        let kills = [(2_000, 1)];
        assert_eq!(
            scrub_target(0.51, 1_000, 3_000, &kills),
            2_000 - KILL_RUN_UP
        );
        assert_eq!(scrub_target(0.80, 1_000, 3_000, &kills), 2_600);
    }

    #[test]
    fn the_theaters_tap_targets_never_overlap() {
        let bands = [EXIT_BAND, SPEED_BAND, SCRUB_BAND];
        for (i, a) in bands.iter().enumerate() {
            assert!(a.0 < a.1, "{a:?}");
            for b in &bands[i + 1..] {
                assert!(!crate::ui::overlaps(*a, *b), "{a:?} overlaps {b:?}");
            }
        }
        // The list's BACK and its top exit, likewise; and the page-turn
        // band stays clear of the first row.
        assert!(!crate::ui::overlaps(BACK_BAND, EXIT_BAND));
        assert!(SUBLINE_BAND.1 <= LIST_TOP);
    }

    /// The sequence from the crash reports, which a fresh launch never
    /// reproduces: play a match, leave it, then roll a tape — all in ONE
    /// process. Booting straight into a tape works (verified on the phone),
    /// so whatever breaks belongs to the *second* session, not the first.
    ///
    /// This drives the real ggrs session swap the screen path performs:
    /// couch session (check_distance 2) → session removed on match exit →
    /// menu ticks → playback session (check_distance 0) + `ReplayPlayback`.
    /// No render, no assets: a panic in a system reproduces here anyway,
    /// which is the failure mode a phone crash actually is.
    #[test]
    fn a_tape_rolls_after_a_match_in_the_same_process() {
        let mut app = harness();
        play_a_session(&mut app, 300); // the couch match
        leave_the_match(&mut app);
        roll_a_tape(&mut app);
        assert!(
            app.world().resource::<sim::FrameCount>().0 > 0,
            "the tape must advance the sim in the same process as a prior match",
        );
    }

    /// A held finger issues exactly one seek, INCLUDING under touch jitter.
    /// This is the scrub-loop fix, second cut: the first debounce keyed on
    /// the exact target frame, and a phone finger never reports the same
    /// position twice — on any real tape one pixel is several frames, so
    /// sub-pixel wander re-fired the seek every report and the playhead
    /// thrashed. Positions here are window-fraction x.
    #[test]
    fn a_jittering_held_finger_seeks_once() {
        let mut anchor = None;
        // Finger lands mid-strip: one seek.
        assert!(scrub_debounce(0.500, &mut anchor));
        // Held "still" — which on a touchscreen means wandering a couple of
        // device pixels every report: all silent.
        assert!(!scrub_debounce(0.502, &mut anchor));
        assert!(!scrub_debounce(0.498, &mut anchor));
        assert!(!scrub_debounce(0.500, &mut anchor));
        // A slow deliberate drag: sub-threshold steps stay silent but keep
        // measuring from the spot that last FIRED (no anchor creep), so the
        // step that crosses the zone fires.
        assert!(!scrub_debounce(0.508, &mut anchor));
        assert!(scrub_debounce(0.516, &mut anchor));
        // A fast drag fires immediately.
        assert!(scrub_debounce(0.60, &mut anchor));
        // Finger lifts (theater_input clears the anchor); re-pressing the
        // same spot must seek again, not be swallowed.
        anchor = None;
        assert!(scrub_debounce(0.60, &mut anchor));
    }

    /// Six hundred frames of a finger held on the strip with realistic
    /// sensor noise (±3 px around a fixed point on a 1080-wide screen):
    /// exactly one seek total. This is the soak the first cut would have
    /// failed — it fired on nearly every one of the 600 reports.
    #[test]
    fn six_hundred_jittery_reports_fire_one_seek() {
        let mut anchor = None;
        let mut fires = 0;
        let mut lcg: u32 = 0x2b99_2ddf;
        for _ in 0..600 {
            lcg = lcg.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = ((lcg >> 16) % 7) as f32 - 3.0; // -3..=3 px
            let x = 540.0 + noise;
            if scrub_debounce(x / 1080.0, &mut anchor) {
                fires += 1;
            }
        }
        assert_eq!(
            fires, 1,
            "a held finger's jitter must never re-fire the seek"
        );
    }

    /// Scrub snapshots land every second AND on frame 1, so a backward seek
    /// can always reach the start of the tape. Frame 0 is never banked.
    #[test]
    fn snapshots_cover_the_whole_tape() {
        assert!(
            !should_snapshot(0),
            "frame 0 is the spawn state, never banked"
        );
        assert!(
            should_snapshot(1),
            "frame 1 anchors the start so scrub reaches it"
        );
        assert!(!should_snapshot(2));
        assert!(!should_snapshot(59));
        assert!(should_snapshot(60), "one-second cadence");
        assert!(should_snapshot(600));
        assert!(!should_snapshot(601));
    }

    /// The control: a fresh launch straight into a tape. This is the path
    /// that always worked on the phone, so it must keep working — and it
    /// pins the failure above to the session *swap*, not to playback.
    #[test]
    fn a_tape_rolls_on_a_fresh_launch() {
        let mut app = harness();
        roll_a_tape(&mut app);
        assert!(app.world().resource::<sim::FrameCount>().0 > 0);
    }

    /// Same swap, no theater: quit a match and start another. The ggrs clock
    /// doesn't know what a replay is, so if the tape case breaks, this one
    /// breaks identically — the blast radius is every second match, and the
    /// replay was only where it got noticed.
    #[test]
    fn a_second_match_starts_after_the_first() {
        let mut app = harness();
        play_a_session(&mut app, 300);
        leave_the_match(&mut app);
        play_a_session(&mut app, 120);
        assert!(app.world().resource::<sim::FrameCount>().0 > 0);
    }

    fn harness() -> App {
        use bevy::time::TimeUpdateStrategy;
        use bevy_ggrs::GgrsPlugin;
        use bevy_ggrs::prelude::*;
        use core::time::Duration;
        use fixed_math::Vec2F;
        use sim::{
            DefaultInputsPlugin, GgrsCfg, Player, PositionF, PreviousPositionF, SimPlugin,
            VelocityF,
        };

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / sim::TICK_HZ as f64,
        )));
        app.add_plugins(GgrsPlugin::<GgrsCfg>::default());
        app.add_plugins(SimPlugin);
        app.add_plugins(sim::InfiniteRoundPlugin);
        app.add_plugins(DefaultInputsPlugin);
        // The real app carries this via `ScreenPlugin`; without it every
        // assertion below is about the crash, not about the theater.
        app.add_plugins(crate::screen::RollbackClockPlugin);
        // The app's real ReadInputs tail: the playback source overwrites the
        // platform source's map, and only while a tape is loaded.
        app.add_systems(
            ReadInputs,
            replay::playback_inputs_system
                .run_if(resource_exists::<replay::ReplayPlayback>)
                .after(sim::read_local_inputs),
        );
        for handle in 0..2usize {
            app.world_mut().spawn((
                Player { handle },
                PositionF(Vec2F::ZERO),
                PreviousPositionF(Vec2F::ZERO),
                VelocityF(Vec2F::ZERO),
            ));
        }
        app
    }

    /// A couch match: `screen::build_synctest_session` + `ticks` of play.
    fn play_a_session(app: &mut App, ticks: usize) {
        use bevy_ggrs::prelude::*;
        use sim::GgrsCfg;

        let mut sb = SessionBuilder::<GgrsCfg>::new()
            .with_num_players(2)
            .unwrap()
            .with_check_distance(2)
            .with_input_delay(0);
        for i in 0..2 {
            sb = sb.add_player(PlayerType::Local, i).unwrap();
        }
        app.insert_resource(Session::SyncTest(sb.start_synctest_session().unwrap()));
        for _ in 0..ticks {
            app.update();
        }
        assert!(
            app.world().resource::<sim::FrameCount>().0 > 0,
            "the session must actually advance",
        );
    }

    /// `screen::despawn_match`, then the menu the player browses through.
    /// Those menu ticks are load-bearing: bevy_ggrs resets its frame count
    /// only on a tick with no session at all.
    fn leave_the_match(app: &mut App) {
        use bevy_ggrs::prelude::*;
        use sim::GgrsCfg;

        app.world_mut().remove_resource::<Session<GgrsCfg>>();
        *app.world_mut().resource_mut::<sim::FrameCount>() = sim::FrameCount::default();
        for _ in 0..5 {
            app.update();
        }
    }

    /// `theater::start_playback` + the playback session `spawn_match` builds.
    fn roll_a_tape(app: &mut App) {
        use replay::{Replay, ReplayHeader};
        use sim::PlayerInput;

        let inputs = vec![[PlayerInput::default(); 2]; 240];
        let replay = Replay {
            header: ReplayHeader {
                magic: replay::MAGIC,
                format_version: replay::FORMAT_VERSION,
                sim_version: sim::SIM_VERSION,
                seed: 0,
                num_players: 2,
                frame_rate: sim::TICK_HZ as u8,
                frame_count: inputs.len() as u32,
                recorded_at: 0,
                winner: Some(0),
                player_handles: [None, None],
                arena_id: 0,
                start_tick: 0,
                kind: replay::TapeKind::default(),
                kills: Vec::new(),
            },
            inputs,
        };
        app.insert_resource(replay::ReplayPlayback::new(replay));
        app.insert_resource(build_playback_session());
        for _ in 0..240 {
            app.update();
        }
    }

    #[test]
    fn civil_dates_are_correct_around_epochs() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        // 2026-07-10 is day 20_644 since the epoch.
        assert_eq!(civil_from_days(20_644), (2026, 7, 10));
    }

    #[test]
    fn date_label_reads_like_a_marquee() {
        // 2026-07-10 00:00:01 UTC.
        assert_eq!(date_label(20_644 * 86_400 + 1), "10 JUL");
    }

    #[test]
    fn arena_labels_cover_every_id() {
        assert_eq!(arena_label(0), "ANCHOR");
        assert_eq!(arena_label(1), "CROSSING");
        assert_eq!(arena_label(2), "RELIQUARY");
        assert_eq!(arena_label(3), "PIT");
        assert_eq!(arena_label(4), "VIGIL");
        assert_eq!(arena_label(5), "GALLERY");
        assert_eq!(arena_label(6), "FOREST");
        assert_eq!(arena_label(99), "ANCHOR", "unknown ids fall back");
    }
}
