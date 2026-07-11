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
use input_touch::WindowSize;
use replay::{Replay, ReplayPlayback, decode_for_sim_version};
use sim::SimSnapshot;
use std::path::PathBuf;

use crate::anchor::{ScreenAnchor, ScreenAnchorSet, ViewRect};
use crate::screen::AppScreen;

/// Sim ticks between scrub snapshots (1 s @ 60 Hz — the viewer's value).
const SNAPSHOT_INTERVAL: u32 = 60;
/// `Time<Virtual>` multiplier while a seek is in flight (viewer's value).
const SEEK_SPEED: f32 = 64.0;
/// Touch-facing playback speeds. 0.25x is a desktop-scrubbing speed; the
/// phone gets the four that matter.
const SPEED_PRESETS: [f32; 4] = [0.5, 1.0, 2.0, 4.0];
/// Most recent tapes listed. Older ones stay on disk (a Files app reaches
/// them); the list stays one thumb-screen tall.
const LIST_MAX: usize = 8;

// ---- Screen bands (window-fraction, y-down) ----
/// Replays list: rows.
const LIST_TOP: f32 = 0.20;
const LIST_PITCH: f32 = 0.07;
/// Replays list: the BACK band at the bottom.
const BACK_BAND: (f32, f32) = (0.86, 0.96);
/// Theater: tap the top strip to exit back to the list.
const EXIT_BAND: (f32, f32) = (0.02, 0.14);
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
    prev_arena: Option<sim::ArenaId>,
    /// Names carried in the tape header, for the marquee.
    names: [Option<String>; 2],
}

impl TheaterMode {
    pub fn active(&self) -> bool {
        self.active
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
}

impl Default for TheaterControls {
    fn default() -> Self {
        Self {
            paused: false,
            seek_target: None,
            speed_idx: 1, // 1.0x
        }
    }
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
struct TapeEntry {
    path: PathBuf,
    line: String,
}

/// The scanned list (refreshed on entering the Replays screen).
#[derive(Resource, Default)]
struct TapeList(Vec<TapeEntry>);

// ---- UI markers ----

#[derive(Component)]
struct ReplayScreenText;

#[derive(Component)]
struct TapeRow;

#[derive(Component)]
struct ScrubPart(ScrubRole);

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScrubRole {
    Track,
    Fill,
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

/// Scan the replay dir for loadable tapes, newest first. Only headers that
/// strict-decode against this binary's `SIM_VERSION` are listed — an old
/// tape from a previous sim version is honestly invisible here (view it
/// via the archived tagged binary, per the no-migrations law).
fn scan_tapes() -> Vec<TapeEntry> {
    let Some(dir) = replays_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut tapes: Vec<(u64, TapeEntry)> = entries
        .filter_map(|e| {
            let path = e.ok()?.path();
            if path.extension().and_then(|s| s.to_str()) != Some("bmrg") {
                return None;
            }
            let bytes = std::fs::read(&path).ok()?;
            let replay = decode_for_sim_version(&bytes, sim::SIM_VERSION).ok()?;
            let h = &replay.header;
            let winner_name = match h.winner {
                Some(w) => h.player_handles[w as usize % 2]
                    .clone()
                    .unwrap_or_else(|| if w == 0 { "CUR".into() } else { "STAG".into() }),
                None => "NOBODY".into(),
            };
            let secs = h.frame_count / h.frame_rate.max(1) as u32;
            let line = format!(
                "{} WINS - {} - {}:{:02} - {}",
                winner_name,
                arena_label(h.arena_id),
                secs / 60,
                secs % 60,
                date_label(h.recorded_at),
            );
            Some((h.recorded_at, TapeEntry { path, line }))
        })
        .collect();
    tapes.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    tapes.truncate(LIST_MAX);
    tapes.into_iter().map(|(_, t)| t).collect()
}

fn arena_label(id: u8) -> &'static str {
    match sim::ArenaId::from_u8(id) {
        sim::ArenaId::Anchor => "ANCHOR",
        sim::ArenaId::Crossing => "CROSSING",
        sim::ArenaId::Reliquary => "RELIQUARY",
    }
}

/// Unix seconds → "10 JUL" (UTC). The civil-from-days algorithm (Howard
/// Hinnant's) — no chrono dependency for one label.
fn date_label(unix_secs: u64) -> String {
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

fn enter_replays(mut commands: Commands, mut list: ResMut<TapeList>) {
    list.0 = scan_tapes();

    commands.spawn((
        ReplayScreenText,
        Text2d::new("REPLAYS"),
        TextFont {
            font_size: 84.0,
            ..default()
        },
        TextColor(render::palette::HOT_BONE),
        TextLayout::new_with_justify(Justify::Center),
        ScreenAnchor::new(0.0, 0.78, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, 210.0),
    ));
    let sub = if list.0.is_empty() {
        "no tapes yet - every decided match saves one".to_string()
    } else {
        "tap a tape to watch it".to_string()
    };
    commands.spawn((
        ReplayScreenText,
        Text2d::new(sub),
        TextFont {
            font_size: 32.0,
            ..default()
        },
        TextColor(render::palette::BONE.with_alpha(0.7)),
        TextLayout::new_with_justify(Justify::Center),
        ScreenAnchor::new(0.0, 0.66, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, 210.0),
    ));
    for (i, tape) in list.0.iter().enumerate() {
        let fy = LIST_TOP + (i as f32 + 0.5) * LIST_PITCH;
        commands.spawn((
            ReplayScreenText,
            TapeRow,
            Text2d::new(tape.line.clone()),
            TextFont {
                font_size: 34.0,
                ..default()
            },
            TextColor(render::palette::BONE),
            TextLayout::new_with_justify(Justify::Center),
            ScreenAnchor::new(0.0, 1.0 - 2.0 * fy, 0.0, 0.0),
            Transform::from_xyz(0.0, 0.0, 210.0),
        ));
    }
    commands.spawn((
        ReplayScreenText,
        Text2d::new("BACK"),
        TextFont {
            font_size: 48.0,
            ..default()
        },
        TextColor(render::palette::HOT_BONE),
        TextLayout::new_with_justify(Justify::Center),
        ScreenAnchor::new(0.0, 1.0 - (BACK_BAND.0 + BACK_BAND.1), 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, 210.0),
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
                } else if fy >= LIST_TOP {
                    let row = ((fy - LIST_TOP) / LIST_PITCH) as usize;
                    if fy < LIST_TOP + LIST_PITCH * LIST_MAX as f32 {
                        tapped_row = Some(row);
                    }
                }
            }
        }
        // Desktop dev path: 1-8 pick a row directly.
        let keys = world.resource::<ButtonInput<KeyCode>>();
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
    // TWOTOP_AUTOPLAY_TAPE=1: roll the newest tape on arrival, once per
    // process (headless capture verification of the theater, pairing with
    // TWOTOP_AUTOSTART=replays; the once-latch keeps exiting playback from
    // re-rolling forever).
    static AUTOPLAYED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if tapped_row.is_none()
        && std::env::var("TWOTOP_AUTOPLAY_TAPE").is_ok_and(|v| v == "1")
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
        entry.path.clone()
    };
    let Ok(bytes) = std::fs::read(&path) else {
        tracing::warn!(target: "two_top::theater", path = %path.display(), "tape unreadable");
        return;
    };
    let Ok(replay) = decode_for_sim_version(&bytes, sim::SIM_VERSION) else {
        tracing::warn!(target: "two_top::theater", path = %path.display(), "tape rejected (version/format)");
        return;
    };
    start_playback(world, replay);
}

/// Load a decoded tape and roll it: arena from the header, playback
/// resources in, then enter the match screen (whose spawn path sees the
/// theater flag and builds the playback session).
fn start_playback(world: &mut World, replay: Replay) {
    let header = replay.header.clone();
    let prev_arena = world.resource::<sim::SelectedArena>().0;
    world.resource_mut::<sim::SelectedArena>().0 = sim::ArenaId::from_u8(header.arena_id);
    // The theater is a spectator seat: no perspective flip, P0 near.
    world.resource_mut::<render::PerspectiveFlip>().0 = 1.0;

    {
        let mut theater = world.resource_mut::<TheaterMode>();
        theater.active = true;
        theater.total_frames = header.frame_count;
        theater.prev_arena = Some(prev_arena);
        theater.names = header.player_handles.clone();
    }
    world.insert_resource(ReplayPlayback::new(replay));
    world.insert_resource(TheaterControls::default());
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
    // Marquee: who's on the card.
    let a = theater.names[0].clone().unwrap_or_else(|| "CUR".into());
    let b = theater.names[1].clone().unwrap_or_else(|| "STAG".into());
    commands.spawn((
        TheaterMarquee,
        Text2d::new(format!("{a} vs {b} - tap top edge to exit")),
        TextFont {
            font_size: 30.0,
            ..default()
        },
        TextColor(render::palette::BONE.with_alpha(0.75)),
        TextLayout::new_with_justify(Justify::Center),
        ScreenAnchor::new(0.0, 1.0 - (EXIT_BAND.0 + EXIT_BAND.1), 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, 210.0),
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
        Transform::from_xyz(0.0, 0.0, 209.0),
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
        Transform::from_xyz(0.0, 0.0, 209.5),
    ));
    // Speed pips.
    for (i, speed) in SPEED_PRESETS.iter().enumerate() {
        let fx = 0.5 + (i as f32 - (SPEED_PRESETS.len() as f32 - 1.0) / 2.0) * 0.16;
        commands.spawn((
            TheaterMarquee,
            SpeedPip(i),
            Text2d::new(format!("{speed}x")),
            TextFont {
                font_size: 34.0,
                ..default()
            },
            TextColor(render::palette::BONE),
            TextLayout::new_with_justify(Justify::Center),
            ScreenAnchor::new(
                fx * 2.0 - 1.0,
                1.0 - (SPEED_BAND.0 + SPEED_BAND.1),
                0.0,
                0.0,
            ),
            Transform::from_xyz(0.0, 0.0, 210.0),
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
fn theater_input(
    theater: Res<TheaterMode>,
    keys: Res<ButtonInput<KeyCode>>,
    touches: Res<Touches>,
    window: Res<WindowSize>,
    frame: Res<sim::FrameCount>,
    controls: Option<ResMut<TheaterControls>>,
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
        controls.seek_target = Some(1);
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
        // Anywhere else: transport toggle.
        controls.paused = !controls.paused;
        if !controls.paused {
            controls.seek_target = None;
        }
    }
    // Press OR drag along the scrub strip seeks (coarse live scrub).
    for t in touches.iter() {
        let p = t.position();
        let fy = p.y / win.y;
        if (SCRUB_BAND.0..SCRUB_BAND.1).contains(&fy) {
            let progress = (p.x / win.x).clamp(0.0, 1.0);
            let target = ((progress * total as f32) as u32).clamp(1, total.saturating_sub(1));
            if controls.seek_target != Some(target) && target != current {
                controls.paused = true;
                controls.seek_target = Some(target);
            }
        }
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
    if frame == 0 || !frame.is_multiple_of(SNAPSHOT_INTERVAL) {
        return;
    }
    let already = world
        .resource::<SnapshotBuffer>()
        .entries
        .last()
        .is_some_and(|s| s.frame == frame);
    if already {
        return;
    }
    let snap = SimSnapshot::capture(world);
    world.resource_mut::<SnapshotBuffer>().entries.push(snap);
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
    // letting the sim idle forward on neutral inputs.
    if controls.seek_target.is_none() && !controls.paused {
        let cursor = world.resource::<ReplayPlayback>().cursor as u32;
        if cursor >= total {
            world.resource_mut::<TheaterControls>().paused = true;
        }
    }

    let controls = *world.resource::<TheaterControls>();
    if let Some(target) = controls.seek_target {
        if target < current {
            world
                .resource_mut::<SnapshotBuffer>()
                .entries
                .retain(|s| s.frame < target.max(1));
            let nearest = world
                .resource::<SnapshotBuffer>()
                .nearest_before(target)
                .cloned();
            if let Some(snap) = nearest {
                snap.restore(world);
                world.resource_mut::<ReplayPlayback>().cursor = snap.frame as usize;
            } else {
                // Seeking earlier than the first snapshot: restart the tape
                // is not available mid-session (world state at frame 0 is
                // the spawn state, which despawn/spawn cycles own). Snap to
                // the earliest snapshot instead — a one-second floor.
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
        let now = world.resource::<sim::FrameCount>().0;
        if now >= target {
            world.resource_mut::<TheaterControls>().seek_target = None;
            theater_apply_play_pause(world, controls.paused);
        } else {
            let mut vt = world.resource_mut::<Time<Virtual>>();
            vt.unpause();
            vt.set_relative_speed(SEEK_SPEED);
        }
    } else {
        theater_apply_play_pause(world, controls.paused);
    }
}

fn theater_apply_play_pause(world: &mut World, paused: bool) {
    let speed = SPEED_PRESETS[world.resource::<TheaterControls>().speed_idx];
    let mut vt = world.resource_mut::<Time<Virtual>>();
    if paused {
        vt.pause();
    } else {
        vt.unpause();
        vt.set_relative_speed(speed);
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
    let progress = (frame.0 as f32 / total as f32).clamp(0.0, 1.0);

    // Band center/height in world units through the view rect.
    let band_center_frac = 1.0 - (SCRUB_BAND.0 + SCRUB_BAND.1); // y-up anchor frac
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
    }
    world.remove_resource::<ReplayPlayback>();
    world.remove_resource::<TheaterControls>();
    world.remove_resource::<SnapshotBuffer>();
    if let Some(arena) = prev_arena {
        world.resource_mut::<sim::SelectedArena>().0 = arena;
    }
    let mut vt = world.resource_mut::<Time<Virtual>>();
    vt.unpause();
    vt.set_relative_speed(1.0);
}

pub struct TheaterPlugin;

impl Plugin for TheaterPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TheaterMode>()
            .init_resource::<TapeList>()
            .add_systems(OnEnter(AppScreen::Replays), enter_replays)
            .add_systems(OnExit(AppScreen::Replays), exit_replays)
            .add_systems(OnEnter(AppScreen::InMatch), spawn_theater_ui)
            .add_systems(OnExit(AppScreen::InMatch), (despawn_theater_ui, theater_teardown))
            .add_systems(
                Update,
                (
                    replays_input.run_if(in_state(AppScreen::Replays)),
                    theater_input.run_if(in_state(AppScreen::InMatch)),
                    theater_capture_snapshot.run_if(in_state(AppScreen::InMatch)),
                    theater_drive_seek
                        .run_if(in_state(AppScreen::InMatch))
                        .after(theater_capture_snapshot),
                    theater_update_ui.after(ScreenAnchorSet),
                ),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    }
}
