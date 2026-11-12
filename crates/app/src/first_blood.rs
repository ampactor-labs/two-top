//! First Blood, wired to the match: the adapter for [`crate::lesson`].
//!
//! On a fresh install the first thing after naming yourself is a practice
//! match that teaches the game one verb at a time (the stages and their
//! rules are `lesson.rs`, pure). This module decides when it runs, reads
//! the match into the lesson's [`Events`] each frame, puts the stage's one
//! line on screen, and remembers that it has been done. The bot's part is
//! `bot::drive_bot`, which asks [`FirstBlood::bot_mode`].
//!
//! When it runs on its own: a fresh install (no match ever played, the
//! lesson never finished or skipped), and never for a player who arrived
//! by a friend's join link — their friend is waiting at the table. SKIP
//! (the quit chip, one tap) ends it for good. SETTINGS → HOW TO PLAY runs
//! it again any time.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use sim::{
    AnimState, ArenaId, Boomerang, BoomerangMods, DashState, Empowered, MatchScore, MatchState,
    Player, PositionF,
};

use crate::anchor::ScreenAnchor;
use crate::lesson::{BotMode, Device, Events, Lesson, Stage};
use crate::screen::AppScreen;

/// The table the lesson is taught at: a void to learn the edge by, a floor
/// that never crumbles under someone still finding the stick (the Vigil
/// has no sudden-death storm), and no cover in the way of a first throw.
pub const LESSON_ARENA: ArenaId = ArenaId::Vigil;

/// What the install has been through, persisted (`progress.json`).
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq, Eq)]
pub struct Progress {
    /// First Blood finished or skipped: it never starts on its own again.
    #[serde(default)]
    pub first_blood_done: bool,
}

/// The persisted progress, as a resource.
#[derive(Resource, Default, Debug)]
pub struct LessonProgress(pub Progress);

const PROGRESS_FILE: &str = "progress.json";

fn load_progress() -> Progress {
    let Some(path) = crate::paths::config_file(PROGRESS_FILE) else {
        return Progress::default();
    };
    let Ok(text) = crate::paths::read_document(&path) else {
        return Progress::default();
    };
    serde_json::from_str(&text).unwrap_or_else(|e| {
        tracing::warn!(target: "two_top::first_blood", error = %e, "progress.json is corrupt; starting fresh");
        crate::paths::quarantine_corrupt(&path);
        Progress::default()
    })
}

fn save_progress(progress: &Progress) {
    let Some(path) = crate::paths::config_file(PROGRESS_FILE) else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(text) = serde_json::to_string_pretty(progress) else {
        return;
    };
    if let Err(e) = crate::paths::write_atomic(&path, text.as_bytes()) {
        tracing::warn!(target: "two_top::first_blood", error = %e, "could not save progress");
    }
}

/// The lesson, while a match is it.
#[derive(Resource, Default, Debug)]
pub struct FirstBlood {
    /// The match running (or about to) is the lesson.
    pub active: bool,
    pub lesson: Lesson,
    /// The table the player had picked, given back when the lesson ends.
    prev_arena: Option<ArenaId>,
}

impl FirstBlood {
    /// The lesson ran to its end (the summary is up).
    pub fn finished(&self) -> bool {
        self.active && self.lesson.stage == Stage::Done
    }

    /// What the bot does right now, if the lesson is driving it.
    pub fn bot_mode(&self) -> Option<BotMode> {
        self.active
            .then(|| crate::lesson::bot_mode(self.lesson.stage))
    }
}

/// The way the player is playing, for the wording: the last input device
/// they actually used (the device's primary pointer until they touch one).
#[derive(Resource, Debug)]
pub struct InputDevice(pub Device);

/// Should the lesson start on its own? Pure for tests.
///
/// * not if it has been done (finished or skipped);
/// * not for an install that has played a match (an update reaching an
///   existing player is not a first launch);
/// * not for a player who arrived by a join link (someone is waiting);
/// * `TWOTOP_FIRST_BLOOD` forces it (`1`) or keeps it away (`0`), and a
///   scripted run (capture, autostart) keeps it away unless forced.
pub fn should_auto_run(
    progress: &Progress,
    career: &crate::grudge::CareerRecord,
    arrived_by_link: bool,
    env_override: Option<bool>,
    scripted: bool,
) -> bool {
    if let Some(forced) = env_override {
        return forced;
    }
    !progress.first_blood_done
        && career.total() == 0
        && career.gauntlet_best == 0
        && !arrived_by_link
        && !scripted
}

/// Begin the lesson: a practice match at [`LESSON_ARENA`].
pub fn begin(
    fb: &mut FirstBlood,
    practice: &mut crate::bot::PracticeMode,
    selected: &mut sim::SelectedArena,
    next: &mut NextState<AppScreen>,
) {
    fb.active = true;
    fb.lesson = Lesson {
        stage: stage_from_env().unwrap_or_default(),
        ..Lesson::default()
    };
    fb.prev_arena = Some(selected.0);
    selected.0 = LESSON_ARENA;
    practice.0 = true;
    next.set(AppScreen::InMatch);
    tracing::info!(target: "two_top::first_blood", "first blood begins");
}

/// `TWOTOP_LESSON_STAGE=dodge` (any stage, lower case) starts the lesson
/// there: a dev and capture knob, for checking one stage without playing
/// through the ones before it.
fn stage_from_env() -> Option<Stage> {
    let name = std::env::var("TWOTOP_LESSON_STAGE").ok()?;
    Some(match name.as_str() {
        "move" => Stage::Move,
        "throw" => Stage::Throw,
        "recall" => Stage::Recall,
        "kill" => Stage::Kill,
        "dodge" => Stage::Dodge,
        "perfect" => Stage::Perfect,
        "edge" => Stage::Edge,
        "finale" => Stage::Finale,
        _ => return None,
    })
}

/// End the lesson for good (SKIP, or a QUIT out of it). The match itself
/// is torn down by the screen change.
pub fn skip(world: &mut World) {
    if !world.resource::<FirstBlood>().active {
        return;
    }
    mark_done(&mut world.resource_mut::<LessonProgress>());
    tracing::info!(target: "two_top::first_blood", stage = ?world.resource::<FirstBlood>().lesson.stage, "first blood skipped");
}

fn mark_done(progress: &mut LessonProgress) {
    if !progress.0.first_blood_done {
        progress.0.first_blood_done = true;
        save_progress(&progress.0);
    }
}

/// `Update` on the title: start the lesson on a fresh install, once.
/// Waits for the name (the keyboard opens first on a fresh online
/// install), then decides.
#[allow(clippy::too_many_arguments)]
fn auto_begin(
    profile: Res<crate::profile::LocalProfile>,
    netplay: Res<crate::netplay::NetplayConfig>,
    progress: Res<LessonProgress>,
    career: Res<crate::grudge::CareerRecord>,
    mut fb: ResMut<FirstBlood>,
    mut practice: ResMut<crate::bot::PracticeMode>,
    mut selected: ResMut<sim::SelectedArena>,
    mut next: ResMut<NextState<AppScreen>>,
    mut decided: Local<bool>,
) {
    if *decided || (!profile.named && netplay.room_url.is_some()) {
        return;
    }
    *decided = true;
    let env_override = std::env::var("TWOTOP_FIRST_BLOOD")
        .ok()
        .and_then(|v| match v.as_str() {
            "1" => Some(true),
            "0" => Some(false),
            _ => None,
        });
    let scripted = std::env::var_os("TWOTOP_CAPTURE").is_some()
        || std::env::var_os("TWOTOP_AUTOSTART").is_some();
    let arrived_by_link = crate::platform::launch_link()
        .as_deref()
        .and_then(crate::room_code::parse_join)
        .is_some();
    if should_auto_run(
        &progress.0,
        &career,
        arrived_by_link,
        env_override,
        scripted,
    ) {
        begin(&mut fb, &mut practice, &mut selected, &mut next);
    }
}

/// `OnExit(InMatch)`: the lesson is over, however it ended. The table the
/// player had picked comes back.
fn end_on_exit(mut fb: ResMut<FirstBlood>, mut selected: ResMut<sim::SelectedArena>) {
    if !fb.active {
        return;
    }
    if let Some(arena) = fb.prev_arena.take() {
        selected.0 = arena;
    }
    fb.active = false;
    fb.lesson = Lesson::default();
}

/// What the lesson's reader remembers from the last frame.
#[derive(Default)]
struct Seen {
    pos: Option<Vec2>,
    fang_out: bool,
    catching: bool,
    dashing: bool,
    empowered: bool,
    kills: u8,
}

/// `Update` while the lesson runs: read this frame's match into the
/// lesson's events and step it. Reads the sim the way the haptics and the
/// audio do — edges on the player's own components.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn read_lesson(
    time: Res<Time>,
    state: Res<MatchState>,
    score: Res<MatchScore>,
    players: Query<(&Player, &PositionF, &DashState, &Empowered, &AnimState)>,
    fangs: Query<(&Boomerang, &BoomerangMods)>,
    mut fb: ResMut<FirstBlood>,
    mut progress: ResMut<LessonProgress>,
    mut seen: Local<Seen>,
) {
    if !fb.active || fb.lesson.stage == Stage::Done {
        *seen = Seen::default();
        return;
    }
    let me = crate::bot::BOT_HANDLE ^ 1;
    let mut ev = Events {
        dt: time.delta_secs(),
        match_over: matches!(*state, MatchState::MatchOver),
        ..Events::default()
    };
    let live = state.is_in_round();
    if let Some((_, pos, dash, emp, anim)) = players.iter().find(|(p, ..)| p.handle == me) {
        let (x, y) = pos.0.to_f32();
        let here = Vec2::new(x, y);
        // Walking only: a respawn teleport is not a step.
        if live && let Some(was) = seen.pos {
            let step = here.distance(was);
            if step < 60.0 {
                ev.moved_cm = step;
            }
        }
        seen.pos = Some(here);
        let dashing = matches!(dash, DashState::Dashing { .. });
        ev.dashed = live && dashing && !seen.dashing;
        seen.dashing = dashing;
        ev.empowered = emp.0 && !seen.empowered;
        seen.empowered = emp.0;
        let catching = anim.anim_id == AnimState::CATCH;
        ev.caught = catching && !seen.catching;
        seen.catching = catching;
    }
    let fang_out = fangs
        .iter()
        .any(|(b, m)| b.owner_handle == me && !m.is_secondary);
    ev.threw = fang_out && !seen.fang_out;
    seen.fang_out = fang_out;
    let kills = if me == 0 { score.p0 } else { score.p1 };
    ev.killed = kills > seen.kills;
    seen.kills = kills;

    let before = fb.lesson.stage;
    fb.lesson = fb.lesson.step(&ev);
    if fb.lesson.stage != before {
        tracing::info!(target: "two_top::first_blood", from = ?before, to = ?fb.lesson.stage, "lesson stage");
        if fb.lesson.stage == Stage::Done {
            mark_done(&mut progress);
        }
    }
}

/// Track the last input device the player used.
fn track_device(
    keys: Res<ButtonInput<KeyCode>>,
    touches: Res<Touches>,
    mut device: ResMut<InputDevice>,
) {
    if keys.get_just_pressed().next().is_some() {
        device.0 = Device::Keys;
    }
    if touches
        .iter_just_pressed()
        .any(|t| t.id() != input_touch::MOUSE_TOUCH_ID)
    {
        device.0 = Device::Touch;
    }
}

/// The lesson's one line.
#[derive(Component)]
struct LessonLine;

/// The dark plate behind it: the line is read over a live, busy floor.
#[derive(Component)]
struct LessonPlate;

/// The plate's height per line of text, and its padding (world units).
const PLATE_LINE_H: f32 = 52.0;
const PLATE_PAD: f32 = 28.0;

/// Where the line sits: between the intro card's stakes (0.325) and the
/// countdown glyph at the centre, over the far half of the floor.
const LINE_FY: f32 = 0.395;

fn spawn_line(mut commands: Commands) {
    commands.spawn((
        LessonPlate,
        Sprite {
            color: render::palette::DEEP_ASH.with_alpha(0.72),
            custom_size: Some(Vec2::new(4000.0, PLATE_LINE_H + PLATE_PAD)),
            ..default()
        },
        ScreenAnchor::new(0.0, crate::ui::anchor_y(LINE_FY), 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, 205.5),
        Visibility::Hidden,
    ));
    commands.spawn((
        LessonLine,
        Text2d::new(String::new()),
        TextFont {
            font_size: 40.0,
            ..default()
        },
        TextColor(render::palette::HOT_BONE),
        // Lines break where `lesson::line` breaks them, never mid-phrase.
        TextLayout {
            justify: Justify::Center,
            linebreak: bevy::text::LineBreak::NoWrap,
        },
        ScreenAnchor::new(0.0, crate::ui::anchor_y(LINE_FY), 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, 206.0),
        Visibility::Hidden,
    ));
}

/// Show the stage's line while the lesson runs, breathing gently so it
/// reads as alive rather than as a caption.
#[allow(clippy::type_complexity)]
fn update_line(
    screen: Res<State<AppScreen>>,
    state: Res<MatchState>,
    fb: Res<FirstBlood>,
    device: Res<InputDevice>,
    time: Res<Time<Real>>,
    mut q: Query<(&mut Text2d, &mut TextColor, &mut Visibility), With<LessonLine>>,
    mut plate: Query<(&mut Sprite, &mut Visibility), (With<LessonPlate>, Without<LessonLine>)>,
) {
    let Ok((mut text, mut color, mut vis)) = q.single_mut() else {
        return;
    };
    let Ok((mut plate, mut plate_vis)) = plate.single_mut() else {
        return;
    };
    let line = (fb.active
        && *screen.get() == AppScreen::InMatch
        && !matches!(*state, MatchState::MatchOver))
    .then(|| crate::lesson::line(fb.lesson.stage, device.0))
    .flatten();
    let Some(line) = line else {
        *vis = Visibility::Hidden;
        *plate_vis = Visibility::Hidden;
        return;
    };
    *vis = Visibility::Visible;
    *plate_vis = Visibility::Visible;
    if text.0 != line {
        text.0 = line.to_string();
        let lines = line.lines().count() as f32;
        plate.custom_size = Some(Vec2::new(4000.0, PLATE_LINE_H * lines + PLATE_PAD));
    }
    let breath = 0.82 + 0.18 * (time.elapsed_secs() * 3.0).sin();
    color.0 = render::palette::HOT_BONE.with_alpha(breath);
}

pub struct FirstBloodPlugin;

impl Plugin for FirstBloodPlugin {
    fn build(&self, app: &mut App) {
        let touch = app
            .world()
            .get_resource::<crate::platform::DeviceProfile>()
            .is_none_or(|p| p.touch_primary);
        app.insert_resource(LessonProgress(load_progress()))
            .insert_resource(InputDevice(if touch {
                Device::Touch
            } else {
                Device::Keys
            }))
            .init_resource::<FirstBlood>()
            .add_systems(Startup, spawn_line)
            .add_systems(OnExit(AppScreen::InMatch), end_on_exit)
            .add_systems(
                Update,
                (
                    auto_begin.run_if(in_state(AppScreen::Title)),
                    track_device,
                    read_lesson.run_if(in_state(AppScreen::InMatch)),
                    update_line,
                ),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grudge::CareerRecord;

    #[test]
    fn it_runs_on_its_own_only_for_a_newcomer() {
        let fresh = Progress::default();
        let career = CareerRecord::default();
        assert!(should_auto_run(&fresh, &career, false, None, false));
        // Done (finished or skipped): never again by itself.
        let done = Progress {
            first_blood_done: true,
        };
        assert!(!should_auto_run(&done, &career, false, None, false));
        // An existing player who updated: not a first launch.
        let veteran = CareerRecord {
            wins: 3,
            ..CareerRecord::default()
        };
        assert!(!should_auto_run(&fresh, &veteran, false, None, false));
        let climber = CareerRecord {
            gauntlet_best: 2,
            ..CareerRecord::default()
        };
        assert!(!should_auto_run(&fresh, &climber, false, None, false));
        // A friend is waiting at the table.
        assert!(!should_auto_run(&fresh, &career, true, None, false));
        // A scripted run keeps away; the override wins either way.
        assert!(!should_auto_run(&fresh, &career, false, None, true));
        assert!(should_auto_run(&done, &career, false, Some(true), true));
        assert!(!should_auto_run(&fresh, &career, false, Some(false), false));
    }

    #[test]
    fn progress_reads_back_and_tolerates_a_missing_field() {
        let p = Progress {
            first_blood_done: true,
        };
        let text = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<Progress>(&text).unwrap(), p);
        assert_eq!(
            serde_json::from_str::<Progress>("{}").unwrap(),
            Progress::default()
        );
    }

    #[test]
    fn the_line_sits_clear_of_the_intro_card_and_the_exit_strip() {
        const { assert!(LINE_FY > 0.325 + 0.04) };
        assert!(!crate::ui::in_band(LINE_FY, crate::ui::TOP_EXIT));
    }
}
