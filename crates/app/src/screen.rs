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

use crate::anchor::ScreenAnchor;
use crate::netplay::NetplayConfig;
use input_touch::WindowSize;

/// True while the player is sitting in an online room with no opponent yet
/// (connecting or waiting for a peer). The whole "you wait alone at your
/// table, then the challenger materializes" beat keys off this: the far
/// seat's duelist is hidden, and the round HUD (countdown, pips, timer)
/// stays dark until a real match is running. Couch and practice are never
/// "awaiting" — their opponent (local or bot) exists from frame one.
#[derive(Resource, Default)]
pub struct AwaitingPeer(pub bool);

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
        app.init_resource::<AwaitingPeer>()
            .add_systems(Startup, spawn_overlays)
            .add_systems(OnEnter(AppScreen::InMatch), spawn_match)
            .add_systems(OnExit(AppScreen::InMatch), despawn_match)
            .add_systems(
                Update,
                (
                    update_awaiting_peer,
                    pick_arena.run_if(in_state(AppScreen::Title)),
                    title_buttons_input.run_if(in_state(AppScreen::Title)),
                    back_to_lobby.run_if(in_state(AppScreen::InMatch)),
                    hide_absent_challenger,
                    update_title_overlay,
                    update_title_buttons,
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
    practice: Res<crate::bot::PracticeMode>,
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
        // The aim telegraph — the public plant both duelists can read.
        let beam = render::spawn_aim_telegraph(&mut commands, player, handle);
        commands.entity(beam).insert(MatchEntity);
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

    // Couch + practice: a fresh local session starts the sim from frame 0
    // (practice forces local even on an online build — the bot supplies
    // handle 1). Online: the matchbox driver inserts the P2P session once
    // the peer connects.
    if netplay.room_url.is_none() || practice.0 {
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

// ---- Title menu layout (window-fraction, y-down for taps) ----
// Every interactive band lives below the notch/status-bar strip and inside
// the thumb zone. Bands never overlap (settings.rs and room_code.rs share
// this budget): settings 0.42–0.56 · practice 0.575–0.655 · room 0.68–0.78
// · play 0.80–0.95.
const PRACTICE_BTN_RECT: (f32, f32) = (0.575, 0.655);
// PLAY sits clear of the bottom nav-bar/gesture strip (~last 6% of a phone).
const PLAY_BTN_RECT: (f32, f32) = (0.80, 0.92);
/// Couch-only: tapping the arena-name band cycles the arena.
const ARENA_TAP_RECT: (f32, f32) = (0.24, 0.36);
/// Screen-anchor Y (y-up, [-1,1]) for each button's center = 1 − 2·y_down.
const PLAY_ANCHOR_Y: f32 = 1.0 - 2.0 * 0.855;
const PRACTICE_ANCHOR_Y: f32 = 1.0 - 2.0 * 0.615;

/// Which title action a button fires.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TitleAction {
    Play,
    Practice,
}

/// One piece of a title button. Border/fill are quads, label is text; all
/// three share the button's screen anchor so they move as a unit.
#[derive(Component)]
struct TitleButton {
    action: TitleAction,
    role: BtnRole,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BtnRole {
    Border,
    Fill,
    Label,
}

/// True if a touch at window-fraction `y_down` fell in a button band.
fn in_band(y_down: f32, band: (f32, f32)) -> bool {
    y_down >= band.0 && y_down < band.1
}

/// Spawn a bordered button (three anchored entities) centered at `anchor_y`.
fn spawn_title_button(
    commands: &mut Commands,
    action: TitleAction,
    anchor_y: f32,
    fill: Vec2,
    font: f32,
) {
    let border = fill + Vec2::splat(22.0);
    commands.spawn((
        TitleButton {
            action,
            role: BtnRole::Border,
        },
        Sprite {
            color: render::palette::HOT_BONE,
            custom_size: Some(border),
            ..default()
        },
        ScreenAnchor::new(0.0, anchor_y, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, 199.0),
        Visibility::Hidden,
    ));
    commands.spawn((
        TitleButton {
            action,
            role: BtnRole::Fill,
        },
        Sprite {
            color: render::palette::DEEP_ASH,
            custom_size: Some(fill),
            ..default()
        },
        ScreenAnchor::new(0.0, anchor_y, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, 199.5),
        Visibility::Hidden,
    ));
    commands.spawn((
        TitleButton {
            action,
            role: BtnRole::Label,
        },
        Text2d::new(String::new()),
        TextFont {
            font_size: font,
            ..default()
        },
        TextColor(render::palette::HOT_BONE),
        // Never wrap a button label onto two lines — keep it on the box.
        TextLayout {
            justify: Justify::Center,
            linebreak: bevy::text::LineBreak::NoWrap,
        },
        ScreenAnchor::new(0.0, anchor_y, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, 200.0),
        Visibility::Hidden,
    ));
}

fn spawn_overlays(mut commands: Commands) {
    // Big static "2-TOP" banner in the upper third — clear of the notch, and
    // high enough that the whole interactive menu lives in the bottom half
    // where a thumb reaches. Its own entity so the tagline can move freely.
    commands.spawn((
        TitleBanner,
        Text2d::new("2-TOP"),
        TextFont {
            font_size: 150.0,
            ..default()
        },
        TextColor(render::palette::HOT_BONE),
        TextLayout::new_with_justify(Justify::Center),
        ScreenAnchor::new(0.0, 0.62, 0.0, 0.0),
        Transform::from_xyz(0.0, 520.0, 200.0),
        Visibility::Hidden,
    ));
    // One short tagline (arena name for couch, room mode for online) — the
    // menu's *actions* now live on visible buttons below, so this is a hint,
    // not an instruction wall.
    commands.spawn((
        TitleOverlay,
        Text2d::new(String::new()),
        TextFont {
            font_size: 40.0,
            ..default()
        },
        TextColor(render::palette::BONE),
        TextLayout::new_with_justify(Justify::Center),
        TextBounds::new_horizontal(TITLE_BODY_WIDTH),
        ScreenAnchor::new(0.0, 0.40, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, 200.0),
        Visibility::Hidden,
    ));
    // The two primary buttons: FIND OPPONENT / PLAY (bottom, biggest) and
    // PRACTICE VS BOT (above it). Each is a border quad + fill quad + label,
    // sharing a screen anchor. Visible affordances replace the old invisible
    // tap-zones + instruction text (and keep every touch target off the notch).
    spawn_title_button(
        &mut commands,
        TitleAction::Play,
        PLAY_ANCHOR_Y,
        Vec2::new(760.0, 172.0),
        60.0,
    );
    spawn_title_button(
        &mut commands,
        TitleAction::Practice,
        PRACTICE_ANCHOR_Y,
        Vec2::new(640.0, 104.0),
        34.0,
    );
    // Summary overlay — screen-centered (the kill-cam holds zoomed-in on
    // MatchOver, so a world-parked summary would sit off-center), shown only
    // on MatchOver.
    commands.spawn((
        SummaryOverlay,
        Text2d::new(String::new()),
        TextFont {
            font_size: 72.0,
            ..default()
        },
        TextColor(render::palette::HOT_BONE),
        TextLayout::new_with_justify(Justify::Center),
        ScreenAnchor::new(0.0, 0.0, 0.0, 0.0),
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
    netplay: Res<NetplayConfig>,
    mut selected: ResMut<SelectedArena>,
) {
    if netplay.room_url.is_some() {
        return;
    }
    if keys.just_pressed(KeyCode::Digit1) || keys.just_pressed(KeyCode::Numpad1) {
        selected.0 = sim::ArenaId::Anchor;
    } else if keys.just_pressed(KeyCode::Digit2) || keys.just_pressed(KeyCode::Numpad2) {
        selected.0 = sim::ArenaId::Crossing;
    } else if keys.just_pressed(KeyCode::Digit3) || keys.just_pressed(KeyCode::Numpad3) {
        selected.0 = sim::ArenaId::Reliquary;
    }
    let win = window.0;
    // Couch: tap the arena-name band to cycle (online has no picker — the
    // room hash decides the arena so both peers agree).
    if win.y > 0.0
        && touches
            .iter_just_pressed()
            .any(|t| in_band(t.position().y / win.y, ARENA_TAP_RECT))
    {
        selected.0 = next_arena(selected.0);
    }
}

/// The single title input handler: keyboard shortcuts plus taps dispatched to
/// the visible button bands. No more invisible screen-half zones — a touch
/// only does something when it lands on the PLAY or PRACTICE button.
fn title_buttons_input(
    keys: Res<ButtonInput<KeyCode>>,
    touches: Res<Touches>,
    window: Res<WindowSize>,
    mut next: ResMut<NextState<AppScreen>>,
    mut practice: ResMut<crate::bot::PracticeMode>,
    mut autostart: Local<Option<bool>>,
) {
    // TWOTOP_AUTOSTART=1 skips the gesture (headless capture verification).
    let auto = *autostart
        .get_or_insert_with(|| std::env::var("TWOTOP_AUTOSTART").is_ok_and(|v| v == "1"));
    if auto {
        next.set(AppScreen::InMatch);
        return;
    }

    if keys.just_pressed(KeyCode::KeyP) {
        practice.0 = !practice.0;
    }
    let key_start = keys.just_pressed(KeyCode::Space)
        || keys.just_pressed(KeyCode::Enter)
        || keys.just_pressed(KeyCode::NumpadEnter);
    if key_start {
        next.set(AppScreen::InMatch);
        return;
    }

    let win = window.0;
    if win.y <= 0.0 {
        return;
    }
    for t in touches.iter_just_pressed() {
        let yd = t.position().y / win.y;
        if in_band(yd, PLAY_BTN_RECT) {
            next.set(AppScreen::InMatch);
            return;
        }
        if in_band(yd, PRACTICE_BTN_RECT) {
            practice.0 = !practice.0;
        }
    }
}

/// Show + label the title buttons (Title screen only). PLAY reads FIND
/// OPPONENT online / PLAY on couch; PRACTICE inverts (bone fill, void text)
/// while active so its state is unmistakable.
type TitleButtonQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static TitleButton,
        &'static mut Visibility,
        Option<&'static mut Sprite>,
        Option<&'static mut Text2d>,
        Option<&'static mut TextColor>,
    ),
>;

fn update_title_buttons(
    screen: Res<State<AppScreen>>,
    netplay: Res<NetplayConfig>,
    practice: Res<crate::bot::PracticeMode>,
    mut q: TitleButtonQuery,
) {
    let on_title = *screen.get() == AppScreen::Title;
    let online = netplay.room_url.is_some();
    for (btn, mut vis, sprite, text, color) in &mut q {
        *vis = if on_title {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        if !on_title {
            continue;
        }
        let active = btn.action == TitleAction::Practice && practice.0;
        match btn.role {
            BtnRole::Border => {
                if let Some(mut s) = sprite {
                    s.color = render::palette::HOT_BONE;
                }
            }
            BtnRole::Fill => {
                if let Some(mut s) = sprite {
                    s.color = if active {
                        render::palette::HOT_BONE
                    } else {
                        render::palette::DEEP_ASH
                    };
                }
            }
            BtnRole::Label => {
                if let Some(mut t) = text {
                    t.0 = match btn.action {
                        // The primary button says what tapping it does NOW:
                        // practice armed → PLAY the bot match; else online →
                        // FIND OPPONENT (matchmaking); else couch → PLAY.
                        TitleAction::Play => {
                            if online && !practice.0 {
                                "FIND OPPONENT".to_string()
                            } else {
                                "PLAY".to_string()
                            }
                        }
                        // The label is constant; the filled/inverted box is
                        // the on/off state (a standard toggle affordance).
                        TitleAction::Practice => "PRACTICE VS BOT".to_string(),
                    };
                }
                if let Some(mut c) = color {
                    c.0 = if active {
                        render::palette::VOID
                    } else {
                        render::palette::HOT_BONE
                    };
                }
            }
        }
    }
}

/// InMatch → Title on Escape, but only once the match is decided (MatchOver),
/// and only in couch mode (online's teardown is the netplay FSM's job). The
/// in-sim rematch (throw) is the other MatchOver exit and stays InMatch.
fn back_to_lobby(
    keys: Res<ButtonInput<KeyCode>>,
    touches: Res<Touches>,
    window: Res<WindowSize>,
    state: Res<MatchState>,
    netplay: Res<NetplayConfig>,
    practice: Res<crate::bot::PracticeMode>,
    mut next: ResMut<NextState<AppScreen>>,
) {
    if netplay.room_url.is_some() && !practice.0 {
        return; // online: the lobby FSM owns teardown
    }
    if !matches!(*state, MatchState::MatchOver) {
        return;
    }
    // Escape (desktop) or a tap in the upper area, clear of the notch, is the
    // no-keyboard way home from a decided practice/couch match.
    let win = window.0;
    let tapped = win.y > 0.0
        && touches
            .iter_just_pressed()
            .any(|t| in_band(t.position().y / win.y, (0.12, 0.30)));
    if keys.just_pressed(KeyCode::Escape) || tapped {
        next.set(AppScreen::Title);
    }
}

/// Recompute [`AwaitingPeer`] each frame (runs before every consumer).
fn update_awaiting_peer(
    screen: Res<State<AppScreen>>,
    netplay: Res<NetplayConfig>,
    practice: Res<crate::bot::PracticeMode>,
    lobby: Res<net::LobbyState>,
    mut awaiting: ResMut<AwaitingPeer>,
) {
    awaiting.0 = *screen.get() == AppScreen::InMatch
        && netplay.room_url.is_some()
        && !practice.0
        && !lobby.is_in_match();
}

/// While online and still unpaired, the far seat stays EMPTY: you wait at
/// your table edge alone (the back-facing duelist you control), and the
/// challenger's body only materializes when a peer actually connects.
/// Before the handshake resolves handles, "you" defaults to the near seat.
#[allow(clippy::type_complexity)]
fn hide_absent_challenger(
    awaiting: Res<AwaitingPeer>,
    local: Res<crate::netplay::LocalPlayerHandle>,
    mut players: Query<(Entity, &Player, &mut Visibility)>,
    mut shadows: Query<
        (&render::GroundShadow, &mut Visibility),
        (Without<Player>, With<render::GroundShadow>),
    >,
) {
    let me = local.0.unwrap_or(0);
    let mut hidden: Option<Entity> = None;
    for (entity, player, mut vis) in &mut players {
        let hide = awaiting.0 && player.handle != me;
        *vis = if hide {
            hidden = Some(entity);
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
    }
    for (shadow, mut vis) in &mut shadows {
        *vis = if Some(shadow.target) == hidden {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
    }
}

fn update_title_overlay(
    screen: Res<State<AppScreen>>,
    selected: Res<SelectedArena>,
    netplay: Res<NetplayConfig>,
    career: Res<crate::grudge::CareerRecord>,
    practice: Res<crate::bot::PracticeMode>,
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
    let _ = &practice; // practice state now reads on its own button
    if on_title {
        *vis = Visibility::Visible;
        let online = netplay.room_url.is_some();
        if online {
            // One tagline: the private-room dialer sits on its own labelled
            // pad below; the career line rides here when there's a record.
            let career_line = if career.total() > 0 {
                format!("     career {}W-{}L", career.wins, career.losses)
            } else {
                String::new()
            };
            text.0 = format!("dial a code below for a private duel{career_line}");
        } else {
            // Couch: the tappable arena name (cycles on tap / 1-2-3).
            text.0 = format!("< {} >", arena_name(selected.0));
        }
    } else {
        *vis = Visibility::Hidden;
    }
}

fn update_summary_overlay(
    state: Res<MatchState>,
    score: Res<MatchScore>,
    netplay: Res<NetplayConfig>,
    saved: Res<crate::recorder::LastSavedReplay>,
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
        // The whole match is a shareable file (deterministic sim = the input
        // tape IS the recording). Point at it once the recorder lands it.
        let replay_line = if saved.0.is_some() {
            "\n\nmatch replay saved - share the .bmrg"
        } else {
            ""
        };
        text.0 = format!(
            "{winner}\n\n{}  -  {}\n\n{again}{lobby}{replay_line}",
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
