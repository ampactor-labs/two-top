//! Phase 18 Task 5.5b: the app screen state machine — Title ↔ InMatch.
//!
//! A `MatchOver` summary plus a "back to lobby" path needs somewhere to go
//! back *to*. This adds a minimal title/lobby screen and models the match
//! lifecycle explicitly:
//!
//!   * [`AppScreen::Title`] — no ggrs `Session` exists, so bevy_ggrs idles the
//!     sim at frame 0 (it resets `RollbackFrameCount`/snapshots every frame
//!     while session-less — the same state the online build sits in before a
//!     peer connects). The title overlay shows; pressing start begins a match.
//!   * [`AppScreen::InMatch`] — match entities exist and a `Session` drives
//!     the rollback sim. On `MatchOver` the summary overlay shows: throw to
//!     play again (the in-sim rematch from Task 5.5a), or back to the lobby.
//!
//! Match entities (players, walls, arena props) are spawned on entering
//! InMatch and despawned on leaving, so a fresh match always starts from a
//! clean slate — and (Task 5.5b-ii) the chosen arena can change between
//! matches. Couch (local SyncTest) drives the session here; online keeps its
//! existing `perform_swap` path untouched (it boots straight into InMatch and
//! the matchbox driver inserts the P2P session on connect).

use bevy::prelude::*;
use bevy_ggrs::prelude::*;
use fixed_math::Vec2F;
use render::{player_atlas_layout, PLAYER_RENDER_SIZE};
use sim::{
    arena_walls, Boomerang, GgrsCfg, MatchScore, MatchState, Pickup, Player, PositionF,
    PreviousPositionF, SelectedArena, VelocityF, MATCH_WIN_THRESHOLD,
};

use crate::netplay::NetplayConfig;

/// Which screen the app is showing. Couch boots into [`Title`](Self::Title);
/// online boots straight into [`InMatch`](Self::InMatch) (its lobby lifecycle
/// is the netplay FSM, not this menu).
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AppScreen {
    #[default]
    Title,
    InMatch,
}

/// Marker on app-spawned match entities (players, walls) so the whole match
/// can be torn down in one query on `OnExit(InMatch)`. Arena props carry
/// render's `ArenaProp` marker instead; play-spawned entities (boomerangs,
/// pickups, stains, effects) are despawned by their own component type.
#[derive(Component)]
struct MatchEntity;

/// Marker for the title-screen overlay text.
#[derive(Component)]
struct TitleOverlay;

/// Marker for the match-summary overlay text.
#[derive(Component)]
struct SummaryOverlay;

pub struct ScreenPlugin;

impl Plugin for ScreenPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_overlays)
            .add_systems(OnEnter(AppScreen::InMatch), spawn_match)
            .add_systems(OnExit(AppScreen::InMatch), despawn_match)
            .add_systems(
                Update,
                (
                    start_match.run_if(in_state(AppScreen::Title)),
                    back_to_lobby.run_if(in_state(AppScreen::InMatch)),
                    update_title_overlay,
                    update_summary_overlay,
                ),
            );
    }
}

/// Build a fresh local SyncTest session (2 local players, distinct per-handle
/// inputs, input-delay 0) — the couch-versus / dev session. Inserted on match
/// start; a fresh one each time so the rollback frame count restarts at 0.
fn build_synctest_session() -> Session<GgrsCfg> {
    let mut sb = SessionBuilder::<GgrsCfg>::new()
        .with_num_players(2)
        .expect("with_num_players")
        .with_check_distance(2)
        .with_input_delay(0);
    for i in 0..2 {
        sb = sb.add_player(PlayerType::Local, i).expect("add_player");
    }
    Session::SyncTest(sb.start_synctest_session().expect("synctest"))
}

/// `OnEnter(InMatch)`: spawn the two duelists, the arena walls, and the
/// selected arena's props, then (couch only) install a fresh SyncTest session.
/// Spawn order is fixed so rollback entity ids are bit-identical across hosts
/// (CONVENTIONS § Determinism). Online leaves the session to `perform_swap`.
fn spawn_match(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut atlases: ResMut<Assets<TextureAtlasLayout>>,
    selected: Res<SelectedArena>,
    netplay: Res<NetplayConfig>,
) {
    let layout = player_atlas_layout(&mut atlases);
    let sheets = [
        (0usize, "sprites/players/duelist_a_sheet.png", Vec2F::from_cm(-100, 60)),
        (1usize, "sprites/players/duelist_b_sheet.png", Vec2F::from_cm(100, -60)),
    ];
    for (handle, sheet, spawn) in sheets {
        commands.spawn((
            MatchEntity,
            Player { handle },
            PositionF(spawn),
            PreviousPositionF(spawn),
            VelocityF(Vec2F::ZERO),
            Sprite {
                image: asset_server.load(sheet),
                texture_atlas: Some(TextureAtlas {
                    layout: layout.clone(),
                    index: 0,
                }),
                custom_size: Some(Vec2::splat(PLAYER_RENDER_SIZE)),
                ..default()
            },
            Transform::default(),
        ));
    }

    // Arena boundary walls — fixed order for bit-identical entity ids.
    for wall in arena_walls() {
        commands.spawn((MatchEntity, wall));
    }

    // Selected arena's props (pyres / chasm / doors) — tagged `ArenaProp`.
    render::spawn_arena_props(&mut commands, &asset_server, &mut atlases, &selected);

    // Couch: a fresh local session starts the sim from frame 0. Online: the
    // matchbox driver inserts the P2P session once the peer connects.
    if netplay.room_url.is_none() {
        commands.insert_resource(build_synctest_session());
    }
}

/// `OnExit(InMatch)`: tear the whole match down — every app-spawned match
/// entity, every arena prop, and every play-spawned entity (boomerangs,
/// pickups, stains, effect sprites) — and drop the session so bevy_ggrs idles
/// the sim back at frame 0.
#[allow(clippy::type_complexity)]
fn despawn_match(
    mut commands: Commands,
    match_entities: Query<
        Entity,
        Or<(
            With<MatchEntity>,
            With<render::ArenaProp>,
            With<Boomerang>,
            With<Pickup>,
            With<render::FloorStain>,
            With<render::EffectSprite>,
        )>,
    >,
) {
    for entity in &match_entities {
        commands.entity(entity).despawn();
    }
    commands.remove_resource::<Session<GgrsCfg>>();
}

fn spawn_overlays(mut commands: Commands) {
    // Title overlay — centered, large, shown only in Title.
    commands.spawn((
        TitleOverlay,
        Text2d::new(String::new()),
        TextFont {
            font_size: 64.0,
            ..default()
        },
        TextColor(render::palette::BONE),
        TextLayout::new_with_justify(Justify::Center),
        Transform::from_xyz(0.0, 0.0, 200.0),
        Visibility::Hidden,
    ));
    // Summary overlay — centered, shown only on MatchOver.
    commands.spawn((
        SummaryOverlay,
        Text2d::new(String::new()),
        TextFont {
            font_size: 56.0,
            ..default()
        },
        TextColor(render::palette::HOT_BONE),
        TextLayout::new_with_justify(Justify::Center),
        Transform::from_xyz(0.0, 0.0, 200.0),
        Visibility::Hidden,
    ));
}

/// Title → InMatch on a start press: Space / Enter (desktop) or any touch
/// (mobile). The sim input layer is dormant here (no session), so this reads
/// raw Bevy input directly — it's app-UI, not wire input.
fn start_match(
    keys: Res<ButtonInput<KeyCode>>,
    touches: Res<Touches>,
    mut next: ResMut<NextState<AppScreen>>,
) {
    let pressed = keys.just_pressed(KeyCode::Space)
        || keys.just_pressed(KeyCode::Enter)
        || keys.just_pressed(KeyCode::NumpadEnter)
        || touches.any_just_pressed();
    if pressed {
        next.set(AppScreen::InMatch);
    }
}

/// InMatch → Title on Escape, but only once the match is decided (MatchOver),
/// and only in couch mode (online's teardown is the netplay FSM's job). The
/// in-sim rematch (throw) is the other MatchOver exit and stays InMatch.
fn back_to_lobby(
    keys: Res<ButtonInput<KeyCode>>,
    state: Res<MatchState>,
    netplay: Res<NetplayConfig>,
    mut next: ResMut<NextState<AppScreen>>,
) {
    if netplay.room_url.is_some() {
        return; // online: the lobby FSM owns teardown
    }
    if matches!(*state, MatchState::MatchOver) && keys.just_pressed(KeyCode::Escape) {
        next.set(AppScreen::Title);
    }
}

fn update_title_overlay(
    screen: Res<State<AppScreen>>,
    selected: Res<SelectedArena>,
    mut q: Query<(&mut Text2d, &mut Visibility), With<TitleOverlay>>,
) {
    let Ok((mut text, mut vis)) = q.single_mut() else {
        return;
    };
    if *screen.get() == AppScreen::Title {
        *vis = Visibility::Visible;
        text.0 = format!(
            "2-TOP\n\narena: {}\n\npress START to begin",
            arena_name(selected.0),
        );
    } else {
        *vis = Visibility::Hidden;
    }
}

fn update_summary_overlay(
    state: Res<MatchState>,
    score: Res<MatchScore>,
    netplay: Res<NetplayConfig>,
    mut q: Query<(&mut Text2d, &mut Visibility), With<SummaryOverlay>>,
) {
    let Ok((mut text, mut vis)) = q.single_mut() else {
        return;
    };
    if matches!(*state, MatchState::MatchOver) {
        *vis = Visibility::Visible;
        let winner = winner_label(*score);
        let again = "press THROW to play again";
        let lobby = if netplay.room_url.is_none() {
            "\npress ESC for lobby"
        } else {
            ""
        };
        text.0 = format!("{winner}\n\n{}  —  {}\n\n{again}{lobby}", score.p0, score.p1);
    } else {
        *vis = Visibility::Hidden;
    }
}

fn arena_name(id: sim::ArenaId) -> &'static str {
    match id {
        sim::ArenaId::Anchor => "Anchor",
        sim::ArenaId::Crossing => "Crossing",
        sim::ArenaId::Reliquary => "Reliquary",
    }
}

/// The match decided when a player's kills reached `MATCH_WIN_THRESHOLD`. P0 is
/// the Cur, P1 the Stag (ART_DIRECTION v2). Pure for testing; the deciding
/// kill always belongs to exactly one side (it's a one-hit-kill game).
fn winner_label(score: MatchScore) -> &'static str {
    if score.p0 >= MATCH_WIN_THRESHOLD {
        "CUR WINS"
    } else {
        "STAG WINS"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_screen_defaults_to_title() {
        assert_eq!(AppScreen::default(), AppScreen::Title);
    }

    #[test]
    fn winner_is_whoever_hit_the_threshold() {
        assert_eq!(
            winner_label(MatchScore {
                p0: MATCH_WIN_THRESHOLD,
                p1: 3
            }),
            "CUR WINS"
        );
        assert_eq!(
            winner_label(MatchScore {
                p0: 2,
                p1: MATCH_WIN_THRESHOLD
            }),
            "STAG WINS"
        );
    }

    #[test]
    fn arena_names_cover_every_variant() {
        assert_eq!(arena_name(sim::ArenaId::Anchor), "Anchor");
        assert_eq!(arena_name(sim::ArenaId::Crossing), "Crossing");
        assert_eq!(arena_name(sim::ArenaId::Reliquary), "Reliquary");
    }
}
