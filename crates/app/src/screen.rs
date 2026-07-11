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
/// player enters [`InMatch`](Self::InMatch). [`Replays`](Self::Replays) is
/// the tape list — picking one re-enters `InMatch` with the theater flag up
/// (`crate::theater`), so playback wears the whole live presentation.
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AppScreen {
    #[default]
    Title,
    InMatch,
    Replays,
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
            .init_resource::<VictoryPose>()
            .init_resource::<PendingUiThrow>()
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
                    online_leave_input.run_if(in_state(AppScreen::InMatch)),
                    summary_buttons_input.run_if(in_state(AppScreen::InMatch)),
                    update_victory_pose,
                    hide_absent_challenger,
                    update_title_overlay,
                    update_title_buttons,
                    update_summary_overlay,
                    update_summary_buttons,
                )
                    .chain(),
            );
    }
}

/// The decided match's winner, held through the summary. The app's atlas
/// picker reads this and pins the winner's sprite on the CHARGE pose — a
/// victory statue over the devouring. Render-only; the sim knows nothing.
#[derive(Resource, Default)]
pub struct VictoryPose(pub Option<usize>);

fn update_victory_pose(
    state: Res<MatchState>,
    score: Res<MatchScore>,
    mut pose: ResMut<VictoryPose>,
) {
    pose.0 = matches!(*state, MatchState::MatchOver).then(|| {
        if score.p0 >= MATCH_WIN_THRESHOLD {
            0
        } else {
            1
        }
    });
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
#[allow(clippy::too_many_arguments)]
fn spawn_match(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut atlases: ResMut<Assets<TextureAtlasLayout>>,
    selected: Res<SelectedArena>,
    netplay: Res<NetplayConfig>,
    practice: Res<crate::bot::PracticeMode>,
    theater: Res<crate::theater::TheaterMode>,
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
        // vertical height scaled by its base row's depth, so far cover stands
        // shorter than near cover — the same perspective the duelists obey.
        // The footprint's WIDTH stays collision-true (the table's X axis is
        // not projected), so a fang that visually clears a corner really does.
        let y0 = render::tilt_y(min_y_raw * flip.0);
        let y1 = render::tilt_y(max_y_raw * flip.0);
        let (min_y, max_y) = if y0 < y1 { (y0, y1) } else { (y1, y0) };
        let near_row = (min_y_raw * flip.0).min(max_y_raw * flip.0);
        let rise = OBSTACLE_RISE * render::depth_scale(near_row);
        commands.spawn((MatchEntity, wall));
        spawn_obstacle_block(
            &mut commands,
            shadow_img.clone(),
            min_x,
            min_y,
            max_x,
            max_y,
            rise,
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

    // Theater: the tape drives a playback session (check_distance 0 +
    // input_delay 0 — the snapshot-scrub requirements). Couch + practice:
    // a fresh local session starts the sim from frame 0 (practice forces
    // local even on an online build — the bot supplies handle 1). Online:
    // the matchbox driver inserts the P2P session once the peer connects.
    if theater.active() {
        commands.insert_resource(crate::theater::build_playback_session());
    } else if netplay.room_url.is_none() || practice.0 {
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
    rise: f32,
) {
    let w = max_x - min_x;
    let d = max_y - min_y;
    let cx = (min_x + max_x) * 0.5;
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
// the thumb zone. Bands never overlap (settings.rs, room_code.rs and
// profile.rs share this budget): name/arena 0.24–0.36 · settings 0.40–0.57
// · practice + replays (side by side) 0.575–0.655 · room 0.68–0.78 · play
// 0.80–0.92.
const PRACTICE_BTN_RECT: (f32, f32) = (0.575, 0.655);
// PLAY sits clear of the bottom nav-bar/gesture strip (~last 6% of a phone).
const PLAY_BTN_RECT: (f32, f32) = (0.80, 0.92);
/// PRACTICE and REPLAYS share their band, split at this window-x fraction:
/// practice left of it, replays right. Keeping them one row returns the
/// vertical space to the settings rows (whose tap pitch the device pass
/// sized for thumbs — see settings.rs).
const PRACTICE_REPLAYS_SPLIT_X: f32 = 0.64;
/// Couch-only: tapping the arena-name band cycles the arena.
const ARENA_TAP_RECT: (f32, f32) = (0.24, 0.36);
/// Screen-anchor Y (y-up, [-1,1]) for each button's center = 1 − 2·y_down.
const PLAY_ANCHOR_Y: f32 = 1.0 - 2.0 * 0.855;
const PRACTICE_ANCHOR_Y: f32 = 1.0 - 2.0 * 0.615;

/// One top strip, one meaning, every screen: tapping it goes BACK — leave
/// the online match, back to the couch lobby, exit the tape. Starts below
/// any camera cutout/status bar (the v9 device pass moved every target off
/// the notch; keep it that way).
pub const TOP_EXIT_BAND: (f32, f32) = (0.05, 0.16);
/// Screen-anchor Y for whatever labels the top-exit strip.
pub const TOP_EXIT_ANCHOR_Y: f32 = 1.0 - (TOP_EXIT_BAND.0 + TOP_EXIT_BAND.1);

/// Which title action a button fires.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TitleAction {
    Play,
    Practice,
    Replays,
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

/// Spawn one piece of a bordered box button. `insert_marker` stamps the
/// screen-specific marker (title vs summary) so each screen's update
/// system finds only its own buttons.
fn spawn_button_part(
    commands: &mut Commands,
    role: BtnRole,
    anchor: Vec2,
    fill: Vec2,
    font: f32,
    insert_marker: &mut impl FnMut(&mut bevy::ecs::system::EntityCommands, BtnRole),
) {
    let mut ec = match role {
        BtnRole::Border => commands.spawn((
            Sprite {
                color: render::palette::HOT_BONE,
                custom_size: Some(fill + Vec2::splat(22.0)),
                ..default()
            },
            ScreenAnchor::new(anchor.x, anchor.y, 0.0, 0.0),
            Transform::from_xyz(0.0, 0.0, 199.0),
            Visibility::Hidden,
        )),
        BtnRole::Fill => commands.spawn((
            Sprite {
                color: render::palette::DEEP_ASH,
                custom_size: Some(fill),
                ..default()
            },
            ScreenAnchor::new(anchor.x, anchor.y, 0.0, 0.0),
            Transform::from_xyz(0.0, 0.0, 199.5),
            Visibility::Hidden,
        )),
        BtnRole::Label => commands.spawn((
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
            ScreenAnchor::new(anchor.x, anchor.y, 0.0, 0.0),
            Transform::from_xyz(0.0, 0.0, 200.0),
            Visibility::Hidden,
        )),
    };
    insert_marker(&mut ec, role);
}

/// Spawn a bordered button (three anchored entities) centered at `anchor`.
fn spawn_title_button(
    commands: &mut Commands,
    action: TitleAction,
    anchor: Vec2,
    fill: Vec2,
    font: f32,
) {
    for role in [BtnRole::Border, BtnRole::Fill, BtnRole::Label] {
        spawn_button_part(commands, role, anchor, fill, font, &mut |ec, role| {
            ec.insert(TitleButton { action, role });
        });
    }
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
        // 0.46 y-up ≈ 0.27 y-down: above the online NAME row (0.30–0.36)
        // and still inside the couch arena-tap band (0.24–0.36, where this
        // text IS the tappable arena name).
        ScreenAnchor::new(0.0, 0.46, 0.0, 0.0),
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
        Vec2::new(0.0, PLAY_ANCHOR_Y),
        Vec2::new(760.0, 172.0),
        60.0,
    );
    // PRACTICE and REPLAYS share one row (split at PRACTICE_REPLAYS_SPLIT_X)
    // so the settings rows above keep their thumb-sized tap pitch.
    spawn_title_button(
        &mut commands,
        TitleAction::Practice,
        Vec2::new(-0.34, PRACTICE_ANCHOR_Y),
        Vec2::new(490.0, 104.0),
        30.0,
    );
    spawn_title_button(
        &mut commands,
        TitleAction::Replays,
        Vec2::new(0.60, PRACTICE_ANCHOR_Y),
        Vec2::new(280.0, 104.0),
        30.0,
    );
    // Summary overlay — screen-centered (the kill-cam holds zoomed-in on
    // MatchOver, so a world-parked summary would sit off-center), shown only
    // on MatchOver. Sits above the primary button's band, which reuses the
    // PLAY slot below.
    commands.spawn((
        SummaryOverlay,
        Text2d::new(String::new()),
        TextFont {
            font_size: 72.0,
            ..default()
        },
        TextColor(render::palette::HOT_BONE),
        TextLayout::new_with_justify(Justify::Center),
        ScreenAnchor::new(0.0, 0.12, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, 200.0),
        Visibility::Hidden,
    ));
    spawn_summary_buttons(&mut commands);
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
    mut autostart: Local<Option<Option<AppScreen>>>,
) {
    // TWOTOP_AUTOSTART=1 skips the gesture; =replays boots into the tape
    // list (both for headless capture verification).
    let auto = *autostart.get_or_insert_with(|| {
        std::env::var("TWOTOP_AUTOSTART")
            .map(|v| match v.as_str() {
                "1" => Some(AppScreen::InMatch),
                "replays" => Some(AppScreen::Replays),
                _ => None,
            })
            .unwrap_or(None)
    });
    if let Some(target) = auto {
        next.set(target);
        return;
    }

    if keys.just_pressed(KeyCode::KeyP) {
        practice.0 = !practice.0;
    }
    if keys.just_pressed(KeyCode::KeyV) {
        next.set(AppScreen::Replays);
        return;
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
        let xd = t.position().x / win.x;
        if in_band(yd, PLAY_BTN_RECT) {
            next.set(AppScreen::InMatch);
            return;
        }
        if in_band(yd, PRACTICE_BTN_RECT) {
            if xd >= PRACTICE_REPLAYS_SPLIT_X {
                next.set(AppScreen::Replays);
                return;
            }
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
    career: Res<crate::grudge::CareerRecord>,
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
                        // The label carries the gauntlet climb once one is
                        // underway; the filled/inverted box stays the on/off
                        // state either way.
                        TitleAction::Practice => {
                            if career.gauntlet_tier > 0 {
                                format!("GAUNTLET TIER {}", career.gauntlet_tier)
                            } else {
                                "PRACTICE VS BOT".to_string()
                            }
                        }
                        TitleAction::Replays => "REPLAYS".to_string(),
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
    // Escape (desktop) or the labeled LOBBY strip — the one universal
    // top-exit band every screen shares.
    let win = window.0;
    let tapped = win.y > 0.0
        && touches
            .iter_just_pressed()
            .any(|t| in_band(t.position().y / win.y, TOP_EXIT_BAND));
    if keys.just_pressed(KeyCode::Escape) || tapped {
        next.set(AppScreen::Title);
    }
}

/// Recompute [`AwaitingPeer`] each frame (runs before every consumer).
fn update_awaiting_peer(
    screen: Res<State<AppScreen>>,
    netplay: Res<NetplayConfig>,
    practice: Res<crate::bot::PracticeMode>,
    theater: Res<crate::theater::TheaterMode>,
    lobby: Res<net::LobbyState>,
    mut awaiting: ResMut<AwaitingPeer>,
) {
    awaiting.0 = *screen.get() == AppScreen::InMatch
        && netplay.room_url.is_some()
        && !practice.0
        && !theater.active()
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

/// Compose the MatchOver summary CARD — the facts only (winner, score,
/// rivalry, the fled note, the saved-tape note). The ACTIONS live on real
/// buttons now (`update_summary_buttons`): the card never again instructs
/// a phone player to "press ESC" or tap an invisible zone. Pure for tests.
#[allow(clippy::too_many_arguments)]
fn summary_text(
    score: MatchScore,
    online: bool,
    practice: bool,
    theater_names: Option<[Option<String>; 2]>,
    local_handle: Option<usize>,
    local_name: &str,
    peer: Option<net::ProfileData>,
    rivalry: Option<String>,
    opponent_gone: bool,
    saved: bool,
) -> String {
    let p0_won = score.p0 >= MATCH_WIN_THRESHOLD;

    // Theater: the tape ran out — say who won it; the marquee up top is
    // already the way out.
    if let Some(names) = theater_names {
        let idx = if p0_won { 0 } else { 1 };
        let fallback = if p0_won { "CUR" } else { "STAG" };
        let who = names[idx].clone().unwrap_or_else(|| fallback.to_string());
        return format!(
            "{who} WINS\n\n{}  -  {}\n\nTAPE ENDS",
            score.p0, score.p1
        );
    }

    if !online || practice {
        let winner = winner_label(score);
        let replay_line = if saved {
            "\n\nmatch replay saved - share the .bmrg"
        } else {
            ""
        };
        return format!("{winner}\n\n{}  -  {}{replay_line}", score.p0, score.p1);
    }

    // Online: name the winner by who they ARE when we know it.
    let me = local_handle.unwrap_or(0);
    let peer_name = peer
        .map(|p| crate::profile::name_from_slots(&p.name))
        .unwrap_or_else(|| "THE CHALLENGER".to_string());
    let i_won = if me == 0 { p0_won } else { !p0_won };
    let winner = if i_won {
        format!("{local_name} WINS")
    } else {
        format!("{peer_name} WINS")
    };
    let rivalry_line = rivalry.map(|r| format!("\n{r}")).unwrap_or_default();
    let gone_line = if opponent_gone {
        "\n\nthe field is yours"
    } else {
        ""
    };
    let replay_line = if saved {
        "\nmatch replay saved - share the .bmrg"
    } else {
        ""
    };
    format!(
        "{winner}\n\n{}  -  {}{rivalry_line}{gone_line}{replay_line}",
        score.p0, score.p1
    )
}

/// The primary summary button's label — the RUN IT BACK handshake as a
/// state machine the thumb can read. `None` hides the button (opponent
/// gone, or a tape playing). Pure for tests.
pub fn primary_label(
    online: bool,
    practice: bool,
    consent: net::RematchConsent,
    peer_name: &str,
    opponent_gone: bool,
) -> Option<String> {
    if !online || practice {
        return Some("PLAY AGAIN".to_string());
    }
    if opponent_gone {
        return None;
    }
    Some(match (consent.local, consent.peer) {
        (false, false) => "RUN IT BACK".to_string(),
        (true, false) => format!("WAITING ON {peer_name}..."),
        (false, true) => format!("{peer_name} IS IN - RUN IT BACK"),
        (true, true) => "RUNNING IT BACK...".to_string(),
    })
}

#[allow(clippy::too_many_arguments)]
fn update_summary_overlay(
    state: Res<MatchState>,
    score: Res<MatchScore>,
    netplay: Res<NetplayConfig>,
    practice: Res<crate::bot::PracticeMode>,
    theater: Res<crate::theater::TheaterMode>,
    local: Res<crate::netplay::LocalPlayerHandle>,
    profile: Res<crate::profile::LocalProfile>,
    peer: Res<net::PeerProfile>,
    career: Res<crate::grudge::CareerRecord>,
    lobby: Res<net::LobbyState>,
    saved: Res<crate::recorder::LastSavedReplay>,
    mut q: Query<(&mut Text2d, &mut Visibility), With<SummaryOverlay>>,
) {
    let Ok((mut text, mut vis)) = q.single_mut() else {
        return;
    };
    if matches!(*state, MatchState::MatchOver) {
        *vis = Visibility::Visible;
        let opponent_gone = matches!(*lobby, net::LobbyState::Forfeited { .. });
        text.0 = summary_text(
            *score,
            netplay.room_url.is_some(),
            practice.0,
            theater.active().then(|| theater.header_names()),
            local.0,
            &profile.name_string(),
            peer.0,
            career.rivalry_line(peer.0),
            opponent_gone,
            saved.0.is_some(),
        );
    } else {
        *vis = Visibility::Hidden;
    }
}

// ---- Summary action buttons ----
// The endgame's actions are BUTTONS, not instructions: the touch controls
// hide at MatchOver, so "tap THROW" pointed at an invisible zone, and
// "press ESC" pointed at a key the phone doesn't have. The primary button
// carries the whole RUN IT BACK handshake state; the top strip is the one
// universal BACK gesture, labeled.

#[derive(Clone, Copy, PartialEq, Eq)]
enum SummaryAction {
    /// PLAY AGAIN (couch/practice) / the RUN IT BACK handshake (online).
    Primary,
    /// The labeled top-exit strip: LOBBY (couch/practice) / LEAVE (online).
    Exit,
}

#[derive(Component)]
struct SummaryButton {
    action: SummaryAction,
    role: BtnRole,
}

/// A tap on PLAY AGAIN must become a real THROW input so the in-sim
/// `apply_rematch` restarts the match the rollback-correct way. The tap
/// arms this counter; `inject_ui_throw` (ReadInputs) ORs THROW_DOWN into
/// the local player's input while it runs down — a held press with a clean
/// rising edge, exactly as if the thumb had pressed the throw zone.
#[derive(Resource, Default)]
pub struct PendingUiThrow(pub u8);

/// Frames the injected press holds. A few ticks survive the forgiveness
/// window and any same-frame ordering; release follows for a clean edge
/// next time.
const UI_THROW_FRAMES: u8 = 4;

pub fn inject_ui_throw(
    mut pending: ResMut<PendingUiThrow>,
    local_players: Res<bevy_ggrs::LocalPlayers>,
    inputs: Option<ResMut<bevy_ggrs::LocalInputs<sim::GgrsCfg>>>,
) {
    if pending.0 == 0 {
        return;
    }
    let Some(mut inputs) = inputs else {
        return;
    };
    pending.0 -= 1;
    // Couch has two local handles; P0's press restarts for everyone
    // (apply_rematch fires on either player's edge).
    if let Some(&handle) = local_players.0.first()
        && let Some(input) = inputs.0.get_mut(&handle)
    {
        input.buttons |= sim::PlayerInput::THROW_DOWN;
    }
}

fn spawn_summary_buttons(commands: &mut Commands) {
    // Primary sits in the PLAY slot — the same thumb position that started
    // the match offers to restart it.
    for role in [BtnRole::Border, BtnRole::Fill, BtnRole::Label] {
        spawn_button_part(
            commands,
            role,
            Vec2::new(0.0, PLAY_ANCHOR_Y),
            Vec2::new(760.0, 150.0),
            44.0,
            &mut |ec, role| {
                ec.insert(SummaryButton {
                    action: SummaryAction::Primary,
                    role,
                });
            },
        );
    }
    for role in [BtnRole::Border, BtnRole::Fill, BtnRole::Label] {
        spawn_button_part(
            commands,
            role,
            Vec2::new(0.0, TOP_EXIT_ANCHOR_Y),
            Vec2::new(340.0, 76.0),
            30.0,
            &mut |ec, role| {
                ec.insert(SummaryButton {
                    action: SummaryAction::Exit,
                    role,
                });
            },
        );
    }
}

type SummaryButtonQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static SummaryButton,
        &'static mut Visibility,
        Option<&'static mut Sprite>,
        Option<&'static mut Text2d>,
        Option<&'static mut TextColor>,
    ),
>;

#[allow(clippy::too_many_arguments)]
fn update_summary_buttons(
    screen: Res<State<AppScreen>>,
    state: Res<MatchState>,
    netplay: Res<NetplayConfig>,
    practice: Res<crate::bot::PracticeMode>,
    theater: Res<crate::theater::TheaterMode>,
    peer: Res<net::PeerProfile>,
    consent: Res<net::RematchConsent>,
    lobby: Res<net::LobbyState>,
    mut q: SummaryButtonQuery,
) {
    let over = *screen.get() == AppScreen::InMatch
        && matches!(*state, MatchState::MatchOver)
        // The theater's marquee owns the top strip and a tape needs no
        // rematch button — the summary card alone says TAPE ENDS.
        && !theater.active();
    let online = netplay.room_url.is_some();
    let opponent_gone = matches!(*lobby, net::LobbyState::Forfeited { .. });
    let peer_name = peer
        .0
        .map(|p| crate::profile::name_from_slots(&p.name))
        .unwrap_or_else(|| "THE CHALLENGER".to_string());
    let primary = primary_label(online, practice.0, *consent, &peer_name, opponent_gone);
    // Local consent shows as a pressed/armed button: inverted fill.
    let armed = online && !practice.0 && consent.local && !consent.peer;

    for (btn, mut vis, sprite, text, color) in &mut q {
        let shown = over
            && match btn.action {
                SummaryAction::Primary => primary.is_some(),
                SummaryAction::Exit => true,
            };
        *vis = if shown {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        if !shown {
            continue;
        }
        let inverted = btn.action == SummaryAction::Primary && armed;
        match btn.role {
            BtnRole::Border => {
                if let Some(mut s) = sprite {
                    s.color = render::palette::HOT_BONE;
                }
            }
            BtnRole::Fill => {
                if let Some(mut s) = sprite {
                    s.color = if inverted {
                        render::palette::HOT_BONE
                    } else {
                        render::palette::DEEP_ASH
                    };
                }
            }
            BtnRole::Label => {
                if let Some(mut t) = text {
                    t.0 = match btn.action {
                        SummaryAction::Primary => primary.clone().unwrap_or_default(),
                        SummaryAction::Exit => if online && !practice.0 {
                            "LEAVE"
                        } else {
                            "LOBBY"
                        }
                        .to_string(),
                    };
                }
                if let Some(mut c) = color {
                    c.0 = if inverted {
                        render::palette::VOID
                    } else {
                        render::palette::HOT_BONE
                    };
                }
            }
        }
    }
}

/// Taps on the summary's PRIMARY button. Couch/practice: arm the injected
/// THROW (the input-driven rematch). Online: give consent — the same thing
/// a raw THROW press means at MatchOver (`netplay::gate_rematch_inputs`
/// converts those), so both paths converge on one handshake.
#[allow(clippy::too_many_arguments)]
fn summary_buttons_input(
    screen: Res<State<AppScreen>>,
    state: Res<MatchState>,
    netplay: Res<NetplayConfig>,
    practice: Res<crate::bot::PracticeMode>,
    theater: Res<crate::theater::TheaterMode>,
    touches: Res<Touches>,
    window: Res<WindowSize>,
    lobby: Res<net::LobbyState>,
    mut consent: ResMut<net::RematchConsent>,
    mut queue: ResMut<net::NetSendQueue>,
    mut pending: ResMut<PendingUiThrow>,
) {
    if *screen.get() != AppScreen::InMatch
        || !matches!(*state, MatchState::MatchOver)
        || theater.active()
    {
        return;
    }
    let win = window.0;
    if win.y <= 0.0 {
        return;
    }
    let tapped_primary = touches
        .iter_just_pressed()
        .any(|t| in_band(t.position().y / win.y, PLAY_BTN_RECT));
    if !tapped_primary {
        return;
    }
    let online = netplay.room_url.is_some() && !practice.0;
    if !online {
        pending.0 = UI_THROW_FRAMES;
        return;
    }
    if matches!(*lobby, net::LobbyState::Forfeited { .. }) {
        return; // nobody left to run it back with
    }
    if !consent.local {
        consent.local = true;
        queue.0.push(net::NetMsg::RematchWant);
    }
}

/// Online summary exit: a top-band tap or Escape LEAVES the match cleanly —
/// goodbye on the side-channel, socket + session torn down, back to Title.
/// Exclusive-world because the teardown touches the non-send socket.
fn online_leave_input(world: &mut World) {
    let online = world.resource::<NetplayConfig>().room_url.is_some();
    let practice = world.resource::<crate::bot::PracticeMode>().0;
    let theater = world.resource::<crate::theater::TheaterMode>().active();
    if !online || practice || theater {
        return;
    }
    if !matches!(*world.resource::<MatchState>(), MatchState::MatchOver) {
        return;
    }
    let esc = world
        .resource::<ButtonInput<KeyCode>>()
        .just_pressed(KeyCode::Escape);
    let win = world.resource::<WindowSize>().0;
    let tapped = win.y > 0.0
        && world
            .resource::<Touches>()
            .iter_just_pressed()
            .any(|t| in_band(t.position().y / win.y, TOP_EXIT_BAND));
    if !(esc || tapped) {
        return;
    }
    crate::netplay::leave_online_match(world);
    world
        .resource_mut::<NextState<AppScreen>>()
        .set(AppScreen::Title);
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

    // ---- The summary card + the RUN IT BACK button's state machine ----

    fn online_summary(gone: bool) -> String {
        let peer = net::ProfileData {
            install_id: 7,
            name: [4, 5, 6, 0], // TAGC
        };
        summary_text(
            MatchScore {
                p0: MATCH_WIN_THRESHOLD,
                p1: 2,
            },
            true,  // online
            false, // practice
            None,  // theater
            Some(0),
            "CURS",
            Some(peer),
            Some("2ND MEETING with TAGC - tied 1-1".into()),
            gone,
            true,
        )
    }

    #[test]
    fn summary_card_carries_facts_not_instructions() {
        let card = online_summary(false);
        assert!(card.contains("CURS WINS"), "{card}");
        assert!(card.contains("2ND MEETING"), "{card}");
        assert!(card.contains("replay saved"), "{card}");
        // Actions live on buttons now — the card never instructs.
        assert!(!card.contains("THROW"), "{card}");
        assert!(!card.contains("ESC"), "{card}");
        assert!(!card.contains("tap top edge"), "{card}");
    }

    #[test]
    fn summary_with_a_fled_opponent_says_so() {
        let fled = online_summary(true);
        assert!(fled.contains("the field is yours"), "{fled}");
    }

    #[test]
    fn primary_button_walks_the_handshake_states() {
        let c = |local, peer| net::RematchConsent { local, peer };
        assert_eq!(
            primary_label(true, false, c(false, false), "TAGC", false).unwrap(),
            "RUN IT BACK"
        );
        assert_eq!(
            primary_label(true, false, c(true, false), "TAGC", false).unwrap(),
            "WAITING ON TAGC..."
        );
        assert_eq!(
            primary_label(true, false, c(false, true), "TAGC", false).unwrap(),
            "TAGC IS IN - RUN IT BACK"
        );
        assert_eq!(
            primary_label(true, false, c(true, true), "TAGC", false).unwrap(),
            "RUNNING IT BACK..."
        );
        // Nobody left to run it back with: the button hides.
        assert_eq!(primary_label(true, false, c(false, false), "TAGC", true), None);
        // Couch and practice keep the plain restart.
        assert_eq!(
            primary_label(false, false, c(false, false), "", false).unwrap(),
            "PLAY AGAIN"
        );
        assert_eq!(
            primary_label(true, true, c(false, false), "", false).unwrap(),
            "PLAY AGAIN"
        );
    }

    #[test]
    fn theater_summary_says_tape_ends() {
        let text = summary_text(
            MatchScore {
                p0: 1,
                p1: MATCH_WIN_THRESHOLD,
            },
            false,
            false,
            Some([Some("CURS".into()), Some("TAGC".into())]),
            None,
            "",
            None,
            None,
            false,
            false,
        );
        assert!(text.contains("TAGC WINS"), "{text}");
        assert!(text.contains("TAPE ENDS"), "{text}");
        assert!(!text.contains("THROW"), "{text}");
    }

    #[test]
    fn couch_summary_keeps_the_classic_card() {
        let text = summary_text(
            MatchScore {
                p0: MATCH_WIN_THRESHOLD,
                p1: 4,
            },
            false,
            false,
            None,
            None,
            "",
            None,
            None,
            false,
            true,
        );
        assert!(text.contains("CUR WINS"), "{text}");
        assert!(text.contains("replay saved"), "{text}");
    }
}
