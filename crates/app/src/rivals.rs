//! The rivalry home (NORTH N4) — the ledger as a place, not a summary line.
//!
//! RIVALS on the Title opens the tables you have history with: one row per
//! opponent, sorted by meetings, carrying their name (the collision tag
//! appears exactly when `grudge::display_name` says it must), the lifetime
//! score, and when you last met. Tap a rival for the detail: the standing,
//! the current run, how many results are sealed (dual-signed, N2), and the
//! recent tapes of this exact rivalry — tap one and it rolls through the
//! theater's projector. The screen renders straight from `CareerRecord`;
//! there is no separate rivals state to drift out of sync.
//!
//! Layout, tap bands, and teardown mirror `arena_select` — the roster and
//! this screen are the same species of list, and a player who learned one
//! has learned the other.

use bevy::prelude::*;
use input_touch::WindowSize;

use crate::anchor::ScreenAnchor;
use crate::grudge::{CareerRecord, RivalRecord};
use crate::screen::AppScreen;

/// Rows band (window-fraction, y-down) — the roster's grammar.
const LIST_TOP: f32 = 0.16;
const LIST_PITCH: f32 = 0.088;
/// Most rivals listed; the ledger keeps everyone, the screen keeps a
/// thumb's worth (the REPLAYS ceiling, same reasoning).
const LIST_MAX: usize = 8;
/// The BACK band at the bottom — same gesture as every menu.
const BACK_BAND: (f32, f32) = (0.86, 0.96);
/// Detail view: the tape rows start here.
const DETAIL_TAPES_TOP: f32 = 0.52;
const DETAIL_TAPES_PITCH: f32 = 0.075;
/// The SPAR THEIR SHADE band, between the facts and the tapes.
const SHADE_BAND: (f32, f32) = (0.43, 0.50);
/// Ring tapes needed before a shade can be fitted — fewer reads one
/// match's mood, not a habit.
const SHADE_MIN_TAPES: usize = 3;

/// Which face the screen is showing: the list, or one rivalry's detail
/// (keyed by the ledger's install-id hex key).
#[derive(Resource, Default, Clone, PartialEq, Eq)]
enum RivalsView {
    #[default]
    List,
    Detail(String),
}

/// Everything this screen spawns, for one-query teardown on any change.
#[derive(Component)]
struct RivalsUi;

/// The rivals to show, newest history first: meetings desc, then most
/// recent meeting. Pure so the row math is testable.
fn ranked(record: &CareerRecord) -> Vec<(String, RivalRecord)> {
    let mut rows: Vec<(String, RivalRecord)> = record
        .rivals
        .iter()
        .map(|(k, r)| (k.clone(), r.clone()))
        .collect();
    rows.sort_by(|(_, a), (_, b)| {
        b.meetings()
            .cmp(&a.meetings())
            .then(b.last_met_unix.cmp(&a.last_met_unix))
    });
    rows.truncate(LIST_MAX);
    rows
}

/// The standing, from our side of the ledger: "YOU LEAD 12-9",
/// "SUDS LEADS 9-12", "TIED 4-4".
fn standing_line(name: &str, r: &RivalRecord) -> String {
    match r.wins.cmp(&r.losses) {
        std::cmp::Ordering::Greater => format!("YOU LEAD {}-{}", r.wins, r.losses),
        std::cmp::Ordering::Less => format!("{} LEADS {}-{}", name, r.losses, r.wins),
        std::cmp::Ordering::Equal => format!("TIED {}-{}", r.wins, r.losses),
    }
}

/// The current run as a row-sized token: "W3" / "L2" / "".
fn streak_token(streak: i32) -> String {
    match streak {
        s if s > 1 => format!("W{s}"),
        s if s < -1 => format!("L{}", -s),
        _ => String::new(),
    }
}

/// Display name straight off the ledger row (the live `display_name`
/// needs a ProfileData; rows render from the stored name plus the tag
/// only when another stored row wears the same name).
fn row_name(record: &CareerRecord, key: &str, r: &RivalRecord) -> String {
    let collides = record
        .rivals
        .iter()
        .any(|(k, other)| k != key && other.name == r.name);
    if collides {
        // The stored key IS the install-id in hex; recover the tag from it.
        let id = u128::from_str_radix(key, 16).unwrap_or(0);
        format!("{}#{}", r.name, crate::profile::identity_tag(id))
    } else {
        r.name.clone()
    }
}

fn spawn_text(
    commands: &mut Commands,
    text: String,
    size: f32,
    color: Color,
    anchor: (f32, f32),
    z: f32,
) {
    commands.spawn((
        RivalsUi,
        Text2d::new(text),
        TextFont {
            font_size: size,
            ..default()
        },
        TextColor(color),
        TextLayout::new_with_justify(Justify::Center),
        ScreenAnchor::new(anchor.0, anchor.1, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, z),
    ));
}

fn row_anchor_y(i: usize, top: f32, pitch: f32) -> f32 {
    1.0 - 2.0 * (top + (i as f32 + 0.5) * pitch)
}

/// (Re)build the screen for the current view. Runs on enter and on any
/// view change; teardown-first keeps it one honest render of the ledger.
fn build_rivals_ui(world: &mut World) {
    let entities: Vec<Entity> = world
        .query_filtered::<Entity, With<RivalsUi>>()
        .iter(world)
        .collect();
    for e in entities {
        world.entity_mut(e).despawn();
    }
    let view = world.resource::<RivalsView>().clone();
    let record = world.resource::<CareerRecord>().clone();
    let mut commands = world.commands();

    match view {
        RivalsView::List => {
            spawn_text(
                &mut commands,
                "THE RIVALS".to_string(),
                72.0,
                render::palette::HOT_BONE,
                (0.0, 1.0 - 2.0 * 0.08),
                210.0,
            );
            let rows = ranked(&record);
            if rows.is_empty() {
                spawn_text(
                    &mut commands,
                    "NO RIVALS YET\nFIND AN OPPONENT AND MAKE ONE".to_string(),
                    34.0,
                    render::palette::BONE.with_alpha(0.7),
                    (0.0, 0.0),
                    210.0,
                );
            }
            for (i, (key, r)) in rows.iter().enumerate() {
                let ay = row_anchor_y(i, LIST_TOP, LIST_PITCH);
                spawn_text(
                    &mut commands,
                    row_name(&record, key, r),
                    40.0,
                    render::palette::BONE,
                    (-0.30, ay + 0.026),
                    210.0,
                );
                let met = if r.last_met_unix > 0 {
                    crate::theater::date_label(r.last_met_unix)
                } else {
                    "LONG AGO".to_string()
                };
                spawn_text(
                    &mut commands,
                    format!("{}   {}", standing_line(&r.name, r), met),
                    24.0,
                    render::palette::BONE.with_alpha(0.6),
                    (-0.30, ay - 0.030),
                    210.0,
                );
                let token = streak_token(r.streak);
                if !token.is_empty() {
                    spawn_text(
                        &mut commands,
                        token,
                        34.0,
                        if r.streak > 0 {
                            render::palette::SPARK
                        } else {
                            render::palette::EMBER
                        },
                        (0.72, ay),
                        210.0,
                    );
                }
            }
        }
        RivalsView::Detail(key) => {
            let Some(r) = record.rivals.get(&key) else {
                return;
            };
            spawn_text(
                &mut commands,
                row_name(&record, &key, r),
                64.0,
                render::palette::HOT_BONE,
                (0.0, 1.0 - 2.0 * 0.10),
                210.0,
            );
            spawn_text(
                &mut commands,
                format!("{} MEETINGS - {}", r.meetings(), standing_line(&r.name, r)),
                36.0,
                render::palette::BONE,
                (0.0, 1.0 - 2.0 * 0.20),
                210.0,
            );
            let mut facts: Vec<String> = Vec::new();
            let token = streak_token(r.streak);
            if !token.is_empty() {
                facts.push(match r.streak {
                    s if s > 0 => format!("YOU HAVE TAKEN THE LAST {s}"),
                    s => format!("THEY HAVE TAKEN THE LAST {}", -s),
                });
            }
            if r.attested_wins > 0 {
                facts.push(format!("{} OF YOUR WINS ARE SEALED", r.attested_wins));
            }
            if r.last_met_unix > 0 {
                facts.push(format!(
                    "LAST MET {}",
                    crate::theater::date_label(r.last_met_unix)
                ));
            }
            for (i, fact) in facts.iter().enumerate() {
                spawn_text(
                    &mut commands,
                    fact.clone(),
                    28.0,
                    render::palette::BONE.with_alpha(0.75),
                    (0.0, 1.0 - 2.0 * (0.27 + i as f32 * 0.050)),
                    210.0,
                );
            }
            if r.tapes.len() >= SHADE_MIN_TAPES {
                spawn_text(
                    &mut commands,
                    "SPAR THEIR SHADE".to_string(),
                    32.0,
                    render::palette::EMBER,
                    (0.0, 1.0 - (SHADE_BAND.0 + SHADE_BAND.1)),
                    210.0,
                );
            }
            if r.tapes.is_empty() {
                spawn_text(
                    &mut commands,
                    "NO TAPES OF THIS RIVALRY YET".to_string(),
                    26.0,
                    render::palette::BONE.with_alpha(0.5),
                    (0.0, row_anchor_y(0, DETAIL_TAPES_TOP, DETAIL_TAPES_PITCH)),
                    210.0,
                );
            }
            for (i, tape) in r.tapes.iter().rev().enumerate() {
                spawn_text(
                    &mut commands,
                    format!("ROLL {}", tape.trim_end_matches(".bmrg")),
                    28.0,
                    render::palette::SPARK,
                    (0.0, row_anchor_y(i, DETAIL_TAPES_TOP, DETAIL_TAPES_PITCH)),
                    210.0,
                );
            }
        }
    }

    // BACK, in the shared language.
    let back_y = 1.0 - (BACK_BAND.0 + BACK_BAND.1);
    spawn_text(
        &mut commands,
        "BACK".to_string(),
        40.0,
        render::palette::HOT_BONE,
        (0.0, back_y),
        210.0,
    );
}

fn enter_rivals(world: &mut World) {
    // Capture-verification reach (the TWOTOP_AUTOSTART family): land
    // straight on the deepest rivalry's detail so the harness can see
    // the facts stack, the shade band, and the tape rows without input.
    let detail = std::env::var("TWOTOP_AUTOSTART").is_ok_and(|v| v == "rivalsdetail");
    *world.resource_mut::<RivalsView>() = if detail {
        ranked(world.resource::<CareerRecord>())
            .first()
            .map(|(key, _)| RivalsView::Detail(key.clone()))
            .unwrap_or(RivalsView::List)
    } else {
        RivalsView::List
    };
    build_rivals_ui(world);
}

fn exit_rivals(world: &mut World) {
    let entities: Vec<Entity> = world
        .query_filtered::<Entity, With<RivalsUi>>()
        .iter(world)
        .collect();
    for e in entities {
        world.entity_mut(e).despawn();
    }
}

/// Taps: a list row opens the detail; a detail tape row rolls it through
/// the theater; BACK walks detail → list → Title. Keys mirror: 1-8 rows,
/// Escape back.
fn rivals_input(world: &mut World) {
    let win = world.resource::<WindowSize>().0;
    let mut tapped_at: Option<f32> = None;
    let mut tapped_back = false;
    {
        let touches = world.resource::<Touches>();
        for t in touches.iter_just_pressed() {
            if win.y <= 0.0 {
                continue;
            }
            let fy = t.position().y / win.y;
            if fy >= BACK_BAND.0 && fy < BACK_BAND.1 {
                tapped_back = true;
            } else {
                tapped_at = Some(fy);
            }
        }
    }
    let keys = world.resource::<ButtonInput<KeyCode>>();
    if keys.just_pressed(KeyCode::Escape) {
        tapped_back = true;
    }
    let digit_row = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
    ]
    .into_iter()
    .position(|k| keys.just_pressed(k));

    let view = world.resource::<RivalsView>().clone();
    match view {
        RivalsView::List => {
            if tapped_back {
                world
                    .resource_mut::<NextState<AppScreen>>()
                    .set(AppScreen::Title);
                return;
            }
            let row = digit_row.or_else(|| {
                tapped_at.and_then(|fy| {
                    (fy >= LIST_TOP).then(|| ((fy - LIST_TOP) / LIST_PITCH) as usize)
                })
            });
            if let Some(row) = row {
                let rows = ranked(world.resource::<CareerRecord>());
                if let Some((key, _)) = rows.get(row) {
                    *world.resource_mut::<RivalsView>() = RivalsView::Detail(key.clone());
                    build_rivals_ui(world);
                }
            }
        }
        RivalsView::Detail(key) => {
            if tapped_back {
                *world.resource_mut::<RivalsView>() = RivalsView::List;
                build_rivals_ui(world);
                return;
            }
            let shade_tapped = tapped_at.is_some_and(|fy| fy >= SHADE_BAND.0 && fy < SHADE_BAND.1)
                || world
                    .resource::<ButtonInput<KeyCode>>()
                    .just_pressed(KeyCode::KeyS);
            if shade_tapped {
                summon_shade(world, &key);
                return;
            }
            let row = digit_row.or_else(|| {
                tapped_at.and_then(|fy| {
                    (fy >= DETAIL_TAPES_TOP)
                        .then(|| ((fy - DETAIL_TAPES_TOP) / DETAIL_TAPES_PITCH) as usize)
                })
            });
            let Some(row) = row else {
                return;
            };
            let tape = {
                let record = world.resource::<CareerRecord>();
                record
                    .rivals
                    .get(&key)
                    .and_then(|r| r.tapes.iter().rev().nth(row).cloned())
            };
            let Some(tape) = tape else {
                return;
            };
            roll_tape(world, &tape);
        }
    }
}

/// Load a rivalry tape from the replays dir and roll it through the
/// theater's projector — the same decode + playback path the REPLAYS
/// screen uses, minus its list ceremony.
fn roll_tape(world: &mut World, filename: &str) {
    let Some(path) = crate::recorder::replays_dir().map(|d| d.join(filename)) else {
        return;
    };
    let Ok(bytes) = std::fs::read(&path) else {
        tracing::warn!(target: "two_top::rivals", path = %path.display(), "rivalry tape unreadable");
        return;
    };
    match replay::decode_for_sim_version(&bytes, sim::SIM_VERSION) {
        Ok(replay) => {
            world.resource_mut::<crate::share::SharedTapePath>().0 = Some(path);
            crate::theater::start_playback(world, replay);
        }
        Err(e) => {
            tracing::warn!(target: "two_top::rivals", error = %e, "rivalry tape rejected");
        }
    }
}

/// Fit a shade from the rival's ring and step onto the table with it:
/// arm `bot::ShadeStyle`, flip practice on, enter the match. Refuses
/// quietly (log only) when the ring is thin or no tape names the rival's
/// seat — a wrong seat would fit OUR habits onto their ghost.
fn summon_shade(world: &mut World, key: &str) {
    let (rival_name, tapes) = {
        let record = world.resource::<CareerRecord>();
        let Some(r) = record.rivals.get(key) else {
            return;
        };
        (r.name.clone(), r.tapes.clone())
    };
    if tapes.len() < SHADE_MIN_TAPES {
        return;
    }
    let my_name = world
        .resource::<crate::profile::LocalProfile>()
        .name_string();
    let Some(dir) = crate::recorder::replays_dir() else {
        return;
    };
    let mut stats = Vec::new();
    for tape in &tapes {
        let Ok(bytes) = std::fs::read(dir.join(tape)) else {
            continue;
        };
        let Ok(replay) = replay::decode_for_sim_version(&bytes, sim::SIM_VERSION) else {
            continue;
        };
        let Some(handle) = crate::shade::rival_handle(&replay, &rival_name, &my_name) else {
            continue;
        };
        stats.push(crate::shade::extract(&replay.inputs, handle));
    }
    if stats.is_empty() {
        tracing::warn!(target: "two_top::rivals", "no readable tape names the rival's seat — no shade");
        return;
    }
    let style = crate::shade::fit(&stats);
    let install_id = u128::from_str_radix(key, 16).unwrap_or(0);
    tracing::info!(
        target: "two_top::rivals",
        tapes = stats.len(),
        ?style,
        "shade fitted — stepping onto the table",
    );
    world.resource_mut::<crate::bot::ShadeStyle>().0 = Some(crate::bot::ShadeSpec {
        style,
        install_id,
        name: rival_name,
    });
    world.resource_mut::<crate::bot::PracticeMode>().0 = true;
    world
        .resource_mut::<NextState<AppScreen>>()
        .set(AppScreen::InMatch);
}

pub struct RivalsPlugin;

impl Plugin for RivalsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RivalsView>()
            .add_systems(OnEnter(AppScreen::Rivals), enter_rivals)
            .add_systems(OnExit(AppScreen::Rivals), exit_rivals)
            .add_systems(Update, rivals_input.run_if(in_state(AppScreen::Rivals)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grudge::rival_key;

    fn rival(name: &str, wins: u32, losses: u32, last_met: u64) -> RivalRecord {
        RivalRecord {
            name: name.into(),
            wins,
            losses,
            last_met_unix: last_met,
            ..Default::default()
        }
    }

    #[test]
    fn ranking_puts_the_deepest_history_first() {
        let mut record = CareerRecord::default();
        record.rivals.insert(rival_key(1), rival("OLD", 1, 0, 5));
        record.rivals.insert(rival_key(2), rival("DEEP", 6, 6, 1));
        record.rivals.insert(rival_key(3), rival("FRESH", 1, 0, 9));
        let rows = ranked(&record);
        assert_eq!(rows[0].1.name, "DEEP", "meetings outrank recency");
        assert_eq!(rows[1].1.name, "FRESH", "recency breaks the tie");
        assert_eq!(rows[2].1.name, "OLD");
    }

    #[test]
    fn standings_and_streaks_speak_the_summary_language() {
        assert_eq!(
            standing_line("SUDS", &rival("SUDS", 12, 9, 0)),
            "YOU LEAD 12-9"
        );
        assert_eq!(
            standing_line("SUDS", &rival("SUDS", 2, 5, 0)),
            "SUDS LEADS 5-2"
        );
        assert_eq!(standing_line("SUDS", &rival("SUDS", 4, 4, 0)), "TIED 4-4");
        assert_eq!(streak_token(3), "W3");
        assert_eq!(streak_token(-2), "L2");
        assert_eq!(streak_token(1), "", "a single win is not yet a run");
    }

    #[test]
    fn name_collisions_grow_the_tag_exactly_like_the_summary() {
        let mut record = CareerRecord::default();
        record
            .rivals
            .insert(rival_key(0xa11), rival("MORGAN", 1, 0, 0));
        record
            .rivals
            .insert(rival_key(0xb22), rival("MORGAN", 0, 1, 0));
        record
            .rivals
            .insert(rival_key(0xc33), rival("SUDS", 2, 2, 0));
        let a = row_name(
            &record,
            &rival_key(0xa11),
            &record.rivals[&rival_key(0xa11)],
        );
        let b = row_name(
            &record,
            &rival_key(0xb22),
            &record.rivals[&rival_key(0xb22)],
        );
        assert!(a.starts_with("MORGAN#") && b.starts_with("MORGAN#"));
        assert_ne!(a, b);
        assert_eq!(
            row_name(
                &record,
                &rival_key(0xc33),
                &record.rivals[&rival_key(0xc33)]
            ),
            "SUDS"
        );
    }

    #[test]
    fn the_detail_bands_never_overlap() {
        // Facts stack from fy 0.27 at 0.055 pitch (three rows max); the
        // SHADE band and the tape rows must clear them and each other —
        // pixels for the list were eyeballed via the capture harness, and
        // this pins the detail view's arithmetic the same way the roster
        // pins its rows.
        let facts_bottom = 0.27 + 3.0 * 0.050;
        assert!(
            facts_bottom <= SHADE_BAND.0,
            "facts stop above the shade band"
        );
        assert!(
            SHADE_BAND.1 <= DETAIL_TAPES_TOP,
            "shade band clears the tapes"
        );
        let tapes_bottom =
            DETAIL_TAPES_TOP + crate::grudge::RIVAL_TAPE_RING as f32 * DETAIL_TAPES_PITCH;
        assert!(tapes_bottom <= BACK_BAND.0, "a full ring clears BACK");
    }

    #[test]
    fn list_rows_map_taps_back_to_rivals() {
        for i in 0..LIST_MAX {
            let fy = LIST_TOP + (i as f32 + 0.5) * LIST_PITCH;
            assert_eq!(((fy - LIST_TOP) / LIST_PITCH) as usize, i);
            assert!(fy < BACK_BAND.0, "row {i} must not reach the BACK band");
        }
    }
}
