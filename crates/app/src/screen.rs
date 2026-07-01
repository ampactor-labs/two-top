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
//! matches. Couch (local SyncTest) drives the session here; online starts the
//! matchbox connection from `OnEnter(InMatch)` and inserts the P2P session on
//! connect.

use bevy::prelude::*;
use bevy::text::TextBounds;
use bevy_ggrs::prelude::*;
use fixed_math::Vec2F;
use render::{PLAYER_RENDER_SIZE, player_atlas_layout};
use sim::{
    Boomerang, GgrsCfg, MATCH_WIN_THRESHOLD, MatchScore, MatchState, Pickup, Player, PositionF,
    PreviousPositionF, SelectedArena, VelocityF, arena_obstacles_for, arena_walls,
};

use crate::netplay::NetplayConfig;
use crate::settings::Settings;
use input_touch::WindowSize;

/// Which screen the app is showing. Both local and online builds boot into
/// [`Title`](Self::Title); online starts the netplay lifecycle only after the
/// player enters [`InMatch`](Self::InMatch).
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AppScreen {
    #[default]
    Title,
    InMatch,
}

/// World units below a duelist's (centre-anchored) origin where its feet meet
/// the floor — the ground-contact point used for y-sorting and the drop shadow.
/// The 64-unit sprite carries the figure low in the cell, so the feet sit ~26
/// below centre.
const PLAYER_FOOT_OFFSET: f32 = 26.0;

/// World-unit height a cover block rises off the floor in the 3/4 view.
const OBSTACLE_RISE: f32 = 52.0;

/// Marker on app-spawned match entities (players, walls) so the whole match
/// can be torn down in one query on `OnExit(InMatch)`. Arena props carry
/// render's `ArenaProp` marker instead; play-spawned entities (boomerangs,
/// pickups, stains, effects) are despawned by their own component type.
#[derive(Component)]
struct MatchEntity;

/// Marker for the big static "2-TOP" banner (its own entity so the title can
/// stay bold while the dynamic body below shrinks to fit the portrait window).
#[derive(Component)]
struct TitleBanner;

/// Marker for the title-screen overlay text (the dynamic body: arena picker,
/// prompts, settings legend).
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
                    pick_arena.run_if(in_state(AppScreen::Title)),
                    start_match.run_if(in_state(AppScreen::Title)),
                    back_to_lobby.run_if(in_state(AppScreen::InMatch)),
                    update_title_overlay,
                    update_summary_overlay,
                )
                    .chain(),
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
    flip: Res<render::PerspectiveFlip>,
) {
    let layout = player_atlas_layout(&mut atlases);
    let sheets = [
        (
            0usize,
            "sprites/players/duelist_a_sheet.png",
            Vec2F::from_cm(0, -300),
        ),
        (
            1usize,
            "sprites/players/duelist_b_sheet.png",
            Vec2F::from_cm(0, 300),
        ),
    ];
    let shadow_img = asset_server.load("sprites/fx/shadow_blob.png");
    let charge_ring_img = asset_server.load("sprites/fx/charge_ring.png");
    for (handle, sheet, spawn) in sheets {
        let (sx, sy) = spawn.to_f32();
        let player = commands
            .spawn((
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
                // 2.5D: sort the duelist by its feet so a closer player draws
                // over a farther one (and gets occluded by raised cover behind).
                render::YSorted {
                    foot_offset: PLAYER_FOOT_OFFSET,
                },
            ))
            .id();
        // A drop shadow under the feet — the cheapest "stands on the floor" cue.
        render::spawn_ground_shadow(
            &mut commands,
            shadow_img.clone(),
            player,
            PLAYER_FOOT_OFFSET,
            PLAYER_RENDER_SIZE * 0.72,
            Vec2::new(sx, render::tilt_y(sy * flip.0)),
        );
        // A charge ring under the feet — hidden until they wind up a throw,
        // then it tightens + blooms toward full charge.
        let aura = render::spawn_charge_aura(
            &mut commands,
            charge_ring_img.clone(),
            player,
            PLAYER_FOOT_OFFSET,
        );
        commands.entity(aura).insert(MatchEntity);
    }

    // Arena boundary walls — fixed order for bit-identical entity ids.
    for wall in arena_walls() {
        commands.spawn((MatchEntity, wall));
    }

    // Inner cover (paintball-style Obstacle blocks): the invisible collision
    // body + a raised 2.5D composite so the cover has real height. Fixed order
    // (after the boundary, before props) keeps entity ids bit-identical across
    // hosts — the visual composite spawns no rollback components, so its
    // entities never enter the rollback id space.
    for wall in arena_obstacles_for(selected.0) {
        let (min_x, min_y_raw) = wall.rect.min.to_f32();
        let (max_x, max_y_raw) = wall.rect.max.to_f32();
        // Foreshorten the footprint Y into the tabletop tilt (visual only — the
        // collision `wall` keeps its true coords). The block's RISE is a screen-
        // vertical height and stays full, so cover still stands up.
        let y0 = render::tilt_y(min_y_raw * flip.0);
        let y1 = render::tilt_y(max_y_raw * flip.0);
        let (min_y, max_y) = if y0 < y1 { (y0, y1) } else { (y1, y0) };
        commands.spawn((MatchEntity, wall));
        spawn_obstacle_block(
            &mut commands,
            shadow_img.clone(),
            min_x,
            min_y,
            max_x,
            max_y,
        );
    }

    // Selected arena's props (pyres / chasm / doors) — tagged `ArenaProp`.
    render::spawn_arena_props(
        &mut commands,
        &asset_server,
        &mut atlases,
        &selected,
        flip.0,
    );

    // Couch: a fresh local session starts the sim from frame 0. Online: the
    // matchbox driver inserts the P2P session once the peer connects.
    if netplay.room_url.is_none() {
        commands.insert_resource(build_synctest_session());
    }
}

/// Spawn the raised-cover composite for one obstacle footprint: a cast shadow,
/// a void silhouette, a shaded front face, a contact/AO band, and the lit top
/// face. Flat-color quads — at this scale the dither/seams read as noise (this
/// matches the validated depth mock); the *height* + outline + shadow are what
/// sell the 2.5D read. Sorted by the front (nearest) base via
/// [`render::ground_z`] so a duelist behind the block is occluded and one in
/// front draws over it. All quads are tagged `MatchEntity` for teardown and
/// carry no rollback components.
fn spawn_obstacle_block(
    commands: &mut Commands,
    shadow_img: Handle<Image>,
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
) {
    let w = max_x - min_x;
    let d = max_y - min_y;
    let cx = (min_x + max_x) * 0.5;
    let rise = OBSTACLE_RISE;
    let total_h = rise + d;
    let z = render::ground_z(min_y);

    let mut quad = |color: Color, size: Vec2, center: Vec2, zz: f32| {
        commands.spawn((
            MatchEntity,
            Sprite {
                color,
                custom_size: Some(size),
                ..default()
            },
            Transform::from_xyz(center.x, center.y, zz),
        ));
    };
    // Lit top face (drawn first so the closures above it layer cleanly is moot —
    // explicit z does the ordering). Void silhouette → front → contact → top.
    quad(
        render::palette::VOID,
        Vec2::new(w + 6.0, total_h + 6.0),
        Vec2::new(cx, min_y + total_h * 0.5),
        z - 0.002,
    );
    quad(
        render::palette::CHARCOAL_LINE,
        Vec2::new(w, rise),
        Vec2::new(cx, min_y + rise * 0.5),
        z,
    );
    quad(
        render::palette::BRUISE_SHADOW,
        Vec2::new(w, 4.0),
        Vec2::new(cx, min_y + 2.0),
        z + 0.001,
    );
    quad(
        render::palette::COLD_STONE,
        Vec2::new(w, d),
        Vec2::new(cx, min_y + rise + d * 0.5),
        z + 0.001,
    );

    // Cast shadow on the floor, nudged toward the bottom-right (light top-left),
    // below the ground-actor band so a duelist in front steps over it.
    let mut shadow_color = Color::WHITE;
    shadow_color.set_alpha(render::GROUND_SHADOW_ALPHA);
    commands.spawn((
        MatchEntity,
        Sprite {
            image: shadow_img,
            color: shadow_color,
            custom_size: Some(Vec2::new(w * 1.15, d * 0.9)),
            ..default()
        },
        Transform::from_xyz(cx + 8.0, min_y + d * 0.15, -0.45),
    ));
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
            With<render::TrailGhost>,
            With<render::GroundShadow>,
        )>,
    >,
) {
    for entity in &match_entities {
        commands.entity(entity).despawn();
    }
    commands.remove_resource::<Session<GgrsCfg>>();
}

/// The widest title-body line at font 30 stays under this; long lines wrap
/// rather than clipping. Sized to the ~1160 world-units the `AutoMin` desktop
/// camera keeps visible across the 600×900 portrait window (`setup_world`).
const TITLE_BODY_WIDTH: f32 = 1080.0;

fn spawn_overlays(mut commands: Commands) {
    // Big static "2-TOP" banner — bold and unchanging, parked in the upper
    // playfield, shown only in Title. Its own entity so the body below can
    // shrink without dragging the title's impact down with it.
    commands.spawn((
        TitleBanner,
        Text2d::new("2-TOP"),
        TextFont {
            font_size: 96.0,
            ..default()
        },
        TextColor(render::palette::HOT_BONE),
        TextLayout::new_with_justify(Justify::Center),
        Transform::from_xyz(0.0, 520.0, 200.0),
        Visibility::Hidden,
    ));
    // Title body — arena picker + prompts + settings legend. Sized and
    // width-bounded so it fits the portrait window instead of bleeding off
    // both edges. Shown only in Title.
    commands.spawn((
        TitleOverlay,
        Text2d::new(String::new()),
        TextFont {
            font_size: 30.0,
            ..default()
        },
        TextColor(render::palette::BONE),
        TextLayout::new_with_justify(Justify::Center),
        TextBounds::new_horizontal(TITLE_BODY_WIDTH),
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

/// Next arena in the picker cycle (Anchor → Crossing → Reliquary → Anchor).
fn next_arena(id: sim::ArenaId) -> sim::ArenaId {
    match id {
        sim::ArenaId::Anchor => sim::ArenaId::Crossing,
        sim::ArenaId::Crossing => sim::ArenaId::Reliquary,
        sim::ArenaId::Reliquary => sim::ArenaId::Anchor,
    }
}

/// Title-screen arena picker. Desktop: 1/2/3 pick directly. Touch: a tap in the
/// *upper* half cycles to the next arena (the lower half is the start zone, so
/// the two gestures never collide). Mutates [`SelectedArena`] — safe here
/// because there's no session (the sim is idle) and `spawn_match` only reads it
/// on match start.
fn pick_arena(
    keys: Res<ButtonInput<KeyCode>>,
    touches: Res<Touches>,
    window: Res<WindowSize>,
    mut selected: ResMut<SelectedArena>,
) {
    if keys.just_pressed(KeyCode::Digit1) || keys.just_pressed(KeyCode::Numpad1) {
        selected.0 = sim::ArenaId::Anchor;
    } else if keys.just_pressed(KeyCode::Digit2) || keys.just_pressed(KeyCode::Numpad2) {
        selected.0 = sim::ArenaId::Crossing;
    } else if keys.just_pressed(KeyCode::Digit3) || keys.just_pressed(KeyCode::Numpad3) {
        selected.0 = sim::ArenaId::Reliquary;
    }
    let win = window.0;
    if win.y > 0.0
        && touches
            .iter_just_pressed()
            .any(|t| t.position().y < win.y * 0.5)
    {
        selected.0 = next_arena(selected.0);
    }
}

/// Title → InMatch on a start press: Space / Enter (desktop) or a tap in the
/// *lower* half of the screen (mobile — the upper half is the arena picker).
/// The sim input layer is dormant here (no session), so this reads raw Bevy
/// input directly — it's app-UI, not wire input.
fn start_match(
    keys: Res<ButtonInput<KeyCode>>,
    touches: Res<Touches>,
    window: Res<WindowSize>,
    mut next: ResMut<NextState<AppScreen>>,
) {
    let key_start = keys.just_pressed(KeyCode::Space)
        || keys.just_pressed(KeyCode::Enter)
        || keys.just_pressed(KeyCode::NumpadEnter);
    let win = window.0;
    let touch_start = win.y > 0.0
        && touches
            .iter_just_pressed()
            .any(|t| t.position().y >= win.y * 0.5);
    if key_start || touch_start {
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
    settings: Res<Settings>,
    netplay: Res<NetplayConfig>,
    mut q: Query<(&mut Text2d, &mut Visibility), With<TitleOverlay>>,
    mut banner: Query<&mut Visibility, (With<TitleBanner>, Without<TitleOverlay>)>,
) {
    let on_title = *screen.get() == AppScreen::Title;
    if let Ok(mut banner_vis) = banner.single_mut() {
        *banner_vis = if on_title {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
    let Ok((mut text, mut vis)) = q.single_mut() else {
        return;
    };
    if on_title {
        *vis = Visibility::Visible;
        let online = netplay.room_url.is_some();
        let options = [
            sim::ArenaId::Anchor,
            sim::ArenaId::Crossing,
            sim::ArenaId::Reliquary,
        ]
        .iter()
        .map(|&id| {
            if id == selected.0 {
                format!("> {} <", arena_name(id))
            } else {
                format!("  {}  ", arena_name(id))
            }
        })
        .collect::<Vec<_>>()
        .join("   ");
        if online {
            text.0 = format!("{options}\n\n\nTAP TO FIND OPPONENT",);
        } else {
            let haptics = if settings.haptics { "on" } else { "off" };
            text.0 = format!(
                "{options}\n\n1/2/3 or tap top to choose\n\npress START  (Space / tap bottom)\
                 \n\n— settings —\n\
                 [H] haptics {haptics}    [-/=] sfx {sfx:.0}%\n\
                 [ [ / ] ] music {music:.0}%    [ , / . ] deadzone {dz:.0}%",
                sfx = settings.sfx_volume * 100.0,
                music = settings.music_volume * 100.0,
                dz = settings.stick_deadzone * 100.0,
            );
        }
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
        text.0 = format!(
            "{winner}\n\n{}  —  {}\n\n{again}{lobby}",
            score.p0, score.p1
        );
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

    #[test]
    fn next_arena_cycles_through_all_three() {
        let mut id = sim::ArenaId::Anchor;
        id = next_arena(id);
        assert_eq!(id, sim::ArenaId::Crossing);
        id = next_arena(id);
        assert_eq!(id, sim::ArenaId::Reliquary);
        id = next_arena(id);
        assert_eq!(id, sim::ArenaId::Anchor, "wraps back to the start");
    }
}
