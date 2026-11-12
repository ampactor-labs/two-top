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
use crate::ui;
use crate::ui::in_band;
use input_touch::WindowSize;

// The screen's larger pieces, each in its own file. They see this
// module's private items, and everything they export reads as
// `crate::screen::…` as it always did.
mod bot_offer;
mod match_world;
mod quit;
mod summary;

use bot_offer::*;
pub(crate) use match_world::rebuild_match;
use match_world::*;
pub use quit::*;
pub use summary::*;

/// True while the player is sitting in an online room with no opponent yet
/// (connecting or waiting for a peer). The whole "you wait alone at your
/// table, then the challenger materializes" beat keys off this: the far
/// seat's duelist is hidden, and the round HUD (countdown, pips, timer)
/// stays dark until a real match is running. Couch and practice are never
/// "awaiting" — their opponent (local or bot) exists from frame one.
#[derive(Resource, Default)]
pub struct AwaitingPeer(pub bool);

/// Seconds a summons must hang before PLAY THE BOT is offered. The title
/// PLAY and the waiting room's PLAY THE BOT share one rect, and the offer
/// used to appear on the first `InMatch` frame — so a mashed double-tap
/// on FIND OPPONENT landed the second tap on the bot, leaving the other
/// phone waiting alone. A summons that has hung this long is the case
/// the button's own doc describes.
pub const BOT_OFFER_DELAY_SECS: f32 = 5.0;

/// Whether the bot may be offered to a player still waiting for a peer.
/// A summons that failed outright (`SummonFailed`, `PeerUnreachable`) is
/// offered at once —
/// there is nothing left to wait for.
pub fn bot_offer_armed(waited_secs: f32, summons_failed: bool) -> bool {
    summons_failed || waited_secs >= BOT_OFFER_DELAY_SECS
}

/// PLAY THE BOT's arming state while awaiting a peer: when the wait began
/// and whether the offer is live. Derived every frame from [`AwaitingPeer`]
/// by `update_bot_offer`; the fled-opponent summary offers the bot on its
/// own, immediately, and does not go through this.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct BotOffer {
    pub since: Option<f32>,
    pub armed: bool,
}

/// Armed when the player converts an online state into a bot match (the
/// hanging summons, or a fled opponent's summary). `bot_fallback_input`
/// tears the online attempt down and bounces through Title — the
/// sessionless frame that lets bevy_ggrs and [`RollbackClockPlugin`] zero
/// the rollback clock, plus the full despawn/spawn — and this flag
/// re-enters InMatch on arrival with practice armed.
#[derive(Resource, Default)]
pub struct PendingBotMatch(pub bool);

/// Set by FIND OPPONENT on the summary after First Blood: the Title passes
/// it straight on into an online match, the way [`PendingBotMatch`] passes
/// a bot match (practice has been disarmed on the way out).
#[derive(Resource, Default)]
pub struct PendingFindOpponent(pub bool);

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
    /// The knobs, moved off the title. Haptics and deadzone get touched
    /// once a month; they were eating the middle of the first screen a
    /// stranger ever sees.
    Settings,
    /// Arcade initials. Four letters, a 26-key grid, one DONE.
    NameEntry,
    /// The arena roster: seven tables, each shown with its floor and its
    /// rules, tap to pick (`crate::arena_select`).
    ArenaSelect,
    /// The rivalry home: every opponent you have history with, the ledger
    /// as a place (`crate::rivals`, NORTH N4).
    Rivals,
}

impl AppScreen {
    /// True for the screens that are menus rather than the live table —
    /// the scrim, and anything else that dims the arena, keys off this.
    pub fn is_menu(self) -> bool {
        !matches!(self, AppScreen::InMatch)
    }
}

/// World units below a duelist's (centre-anchored) origin where its feet meet
/// the floor — the ground-contact point used for y-sorting and the drop shadow.
/// The 64-unit sprite carries the figure low in the cell, so the feet sit ~26
/// below centre.
const PLAYER_FOOT_OFFSET: f32 = 32.0;

/// World-unit height a cover block rises off the floor in the 3/4 view.
const OBSTACLE_RISE: f32 = 52.0;

/// Marker on app-spawned match entities (players, walls) so the whole match
/// can be torn down in one query on `OnExit(InMatch)`. Arena props carry
/// render's `ArenaProp` marker instead; play-spawned entities (boomerangs,
/// pickups, stains, effects) are despawned by their own component type.
#[derive(Component)]
pub(crate) struct MatchEntity;

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
            .init_resource::<BotOffer>()
            .init_resource::<VictoryPose>()
            .init_resource::<PendingUiThrow>()
            .init_resource::<PendingBotMatch>()
            .init_resource::<PendingFindOpponent>()
            .init_resource::<QuitArm>()
            .init_resource::<MatchOverSince>()
            .add_systems(Startup, spawn_overlays)
            .add_plugins(RollbackClockPlugin)
            .add_systems(OnEnter(AppScreen::InMatch), spawn_match)
            .add_systems(
                OnExit(AppScreen::InMatch),
                (despawn_match, disarm_practice_on_exit),
            )
            .add_systems(
                Update,
                quit_button_input.run_if(in_state(AppScreen::InMatch)),
            )
            .add_systems(
                Update,
                (
                    track_match_over,
                    update_awaiting_peer,
                    update_bot_offer.after(update_awaiting_peer),
                    pick_arena.run_if(in_state(AppScreen::Title)),
                    title_buttons_input.run_if(in_state(AppScreen::Title)),
                    back_to_lobby.run_if(in_state(AppScreen::InMatch)),
                    online_leave_input.run_if(in_state(AppScreen::InMatch)),
                    summary_buttons_input.run_if(in_state(AppScreen::InMatch)),
                    // After summary_buttons_input so the same tap can't
                    // double-fire: the summary sees the pre-practice state
                    // and stays inert on a fled opponent.
                    bot_fallback_input.run_if(in_state(AppScreen::InMatch)),
                    update_victory_pose,
                    hide_absent_challenger,
                    update_title_overlay,
                    update_title_buttons,
                    update_summary_overlay,
                    update_summary_buttons,
                    update_quit_button,
                    update_bot_offer_button,
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

/// `OnExit(InMatch)`: practice is a per-entry choice, not a sticky mode.
/// Leaving it armed made the home page depend on how you arrived: a fresh
/// boot showed FIND OPPONENT with PRACTICE at rest, while the title after
/// a bot match showed PLAY with PRACTICE inverted — and the big button
/// silently started another bot match instead of matchmaking. Disarming on
/// the way out gives one home page regardless of the path; tapping
/// PRACTICE re-arms it in one gesture, and the gauntlet ladder lives in
/// the persisted `CareerRecord`, not in this flag. The one exception is
/// the bot-fallback bounce ([`PendingBotMatch`]), which passes through
/// Title with practice deliberately armed.
fn disarm_practice_on_exit(
    pending: Res<PendingBotMatch>,
    mut practice: ResMut<crate::bot::PracticeMode>,
) {
    if !pending.0 && practice.0 {
        practice.0 = false;
    }
}

/// Zero the rollback clock on every tick with no session, which is exactly
/// when bevy_ggrs zeroes the frame count that clock is derived from.
///
/// `Time<GgrsTime>` is advanced by `GgrsTimePlugin::update` to
/// `RollbackFrameCount / RollbackFrameRate` via `Time::advance_to`, which
/// asserts the new elapsed is not earlier than the old one and aborts the
/// process if it is. bevy_ggrs resets `RollbackFrameCount` to 0 on any tick
/// where no `Session` exists, but never resets the clock to match. So the
/// first tick of the *second* session in a process advances the clock to
/// frame 0 while it still reads ~50s from the match before, and the assert
/// kills the app.
///
/// That is the replay crash: the phone died on a tape only because watching
/// one is the common way to start a second session, and a fresh launch into
/// a tape (which is how it was tested) never reproduces it. It is not about
/// replays at all; a second *match* takes the same path.
///
/// The condition here mirrors bevy_ggrs's own reset condition rather than
/// hooking the teardown sites, so the clock and the frame count cannot
/// disagree no matter which path drops the session: quitting a match,
/// `netplay`'s peer drop, or a forfeit. Ordered before `RunGgrsSystems` so
/// the reset lands on the same tick as theirs, not one behind.
fn reset_rollback_clock_between_sessions(
    session: Option<Res<Session<GgrsCfg>>>,
    mut time: ResMut<Time<bevy_ggrs::GgrsTime>>,
) {
    if session.is_none() && time.elapsed() != core::time::Duration::ZERO {
        *time = Time::new_with(bevy_ggrs::GgrsTime);
    }
}

/// Carries [`reset_rollback_clock_between_sessions`]. Split out of
/// `ScreenPlugin` because the fix belongs to the ggrs session lifecycle
/// rather than to screens; a headless harness that swaps sessions needs it
/// and has no screens to speak of.
pub struct RollbackClockPlugin;

impl Plugin for RollbackClockPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PreUpdate,
            reset_rollback_clock_between_sessions.before(bevy_ggrs::RunGgrsSystems),
        );
    }
}

/// `OnExit(InMatch)`: tear the whole match down — every app-spawned match
/// entity, every arena prop, and every play-spawned entity (boomerangs,
/// pickups, stains, effect sprites) — drop the session, and reset the match
/// resources to a clean slate.
///
/// The reset is the load-bearing half. Dropping the `Session` stops the sim
/// dead, so whatever `MatchState` held at the moment you left is what it
/// holds forever — and leaving a decided match means that value is
/// `MatchOver`. Everything keyed off it then lied on the title screen: the
/// summary card stayed up over the menu, and the kill-cam (which punches in
/// on `MatchOver` with `hold_frames = INFINITY` and only releases on the
/// next `Countdown`) held the camera zoomed 1.6x and parked on the last
/// kill spot — which shrank the anchored UI's view rect until the buttons
/// overlapped each other and the text ran off both screen edges. The next
/// match then started at `MatchOver` too, dead until a stray THROW tripped
/// `apply_rematch`.
///
/// Resetting rolled-back resources out of band is normally forbidden
/// (CONVENTIONS § Determinism), but that rule protects transitions *inside*
/// a session. Here the session is already gone and the next `spawn_match`
/// builds a fresh one, so this is the same clean-slate the entity teardown
/// below has always been.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn despawn_match(
    mut commands: Commands,
    mut state: ResMut<MatchState>,
    mut score: ResMut<MatchScore>,
    mut frame: ResMut<sim::FrameCount>,
    mut rng: ResMut<sim::SimRng>,
    mut history: ResMut<sim::InputHistory>,
    mut bridge: ResMut<sim::BridgeState>,
    mut door: ResMut<sim::DoorCooldown>,
    mut pickup_timer: ResMut<sim::PickupSpawnTimer>,
    mut kill_cam: ResMut<crate::camera::KillCam>,
    mut last_kill: ResMut<render::LastKillPos>,
    mut shake: ResMut<render::ScreenShake>,
    match_entities: Query<
        Entity,
        Or<(
            With<MatchEntity>,
            With<render::ArenaProp>,
            With<Boomerang>,
            With<Pickup>,
            With<sim::FireTrailCell>,
            With<render::FloorStain>,
            With<render::EffectSprite>,
            With<render::TrailGhost>,
            With<render::GroundShadow>,
            With<render::PickupAura>,
        )>,
    >,
) {
    for entity in &match_entities {
        commands.entity(entity).despawn();
    }
    commands.remove_resource::<Session<GgrsCfg>>();
    *state = MatchState::default();
    *score = MatchScore::default();
    *frame = sim::FrameCount::default();
    // The WHOLE rolled-back resource set resets, not just the three the
    // summary reads. Anything skipped here seeds the next session with the
    // previous match's tail: a carried `SimRng` made every second match's
    // pickup schedule differ from a fresh boot (and two re-paired online
    // peers with different histories would desync on it at the first
    // spawn), a carried `InputHistory` made frame-1 edges read against the
    // last match's final buttons, and a live `FireTrailCell` (no despawn
    // system runs without a session) stayed lethal into the next match.
    *rng = sim::SimRng::default();
    *history = sim::InputHistory::default();
    *bridge = sim::BridgeState::default();
    *door = sim::DoorCooldown::default();
    *pickup_timer = sim::PickupSpawnTimer::default();
    *kill_cam = crate::camera::KillCam::default();
    *last_kill = render::LastKillPos::default();
    *shake = render::ScreenShake::default();
}

/// The widest title-body line at font 30 stays under this; long lines wrap
/// rather than clipping. Sized to the ~1160 world-units the `AutoMin` desktop
/// camera keeps visible across the 600×900 portrait window (`setup_world`).
const TITLE_BODY_WIDTH: f32 = 1080.0;

// ---- Title layout (window-fraction, y-down for taps) ----
// The table is the pitch, so it owns the middle of the screen: banner,
// then the diorama (your demon at the near seat, the far seat empty —
// `lib.rs`), then one column of actions with the primary last and biggest.
// A stranger reads the picture and the buttons say the rest; the screen
// carries no instructional prose at all.
//
// Bands, y-down, never overlapping (room_code.rs shares this budget):
// diorama 0.18–0.50 · subline 0.52–0.58 · secondary row 0.59–0.66 ·
// mode toggle 0.68–0.74 · private dial 0.75–0.81 · primary 0.82–0.93,
// which stops clear of the phone's nav-bar / gesture strip (~last 6%).

// The bands themselves live in `ui::title` (one table, overlap-tested);
// these names keep the call sites short.
const SUBLINE_RECT: ui::Band = ui::title::SUBLINE;
const SUBLINE_ANCHOR_Y: f32 = ui::band_anchor_y(SUBLINE_RECT);
const SECONDARY_ROW_RECT: ui::Band = ui::title::SECONDARY_ROW;
const SECONDARY_ANCHOR_Y: f32 = ui::band_anchor_y(SECONDARY_ROW_RECT);
/// PRACTICE · RIVALS · REPLAYS · SETTINGS split the secondary row on
/// window-x at these fractions.
const SECONDARY_PRACTICE_X: f32 = 0.26;
const SECONDARY_RIVALS_X: f32 = 0.50;
const SECONDARY_REPLAYS_X: f32 = 0.74;
pub const MODE_TOGGLE_RECT: ui::Band = ui::title::MODE_TOGGLE;
pub const MODE_TOGGLE_ANCHOR_Y: f32 = ui::band_anchor_y(MODE_TOGGLE_RECT);
pub const DIAL_RECT: ui::Band = ui::title::DIAL;
pub const DIAL_ANCHOR_Y: f32 = ui::band_anchor_y(DIAL_RECT);
const PLAY_BTN_RECT: ui::Band = ui::title::PLAY;
const PLAY_ANCHOR_Y: f32 = ui::band_anchor_y(PLAY_BTN_RECT);
/// The app-wide BACK strip (`ui::TOP_EXIT`).
pub const TOP_EXIT_BAND: ui::Band = ui::TOP_EXIT;
/// Screen-anchor Y for whatever labels the top-exit strip.
pub const TOP_EXIT_ANCHOR_Y: f32 = ui::band_anchor_y(TOP_EXIT_BAND);

/// Which title action a button fires.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TitleAction {
    Play,
    Practice,
    Rivals,
    Replays,
    Settings,
}

/// One piece of a title button. Border/fill are quads, label is text; all
/// three share the button's screen anchor so they move as a unit.
#[derive(Component)]
struct TitleButton {
    action: TitleAction,
    role: BtnRole,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BtnRole {
    Border,
    Fill,
    Label,
}

/// Spawn one piece of a bordered box button. `insert_marker` stamps the
/// screen-specific marker (title vs summary vs the room pad) so each
/// screen's update system finds only its own buttons.
pub fn spawn_button_part(
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
    // The banner sits at the top and stays out of the way: the diorama
    // below it is what actually says what this is.
    commands.spawn((
        TitleBanner,
        Text2d::new("2-TOP"),
        TextFont {
            font_size: 110.0,
            ..default()
        },
        TextColor(render::palette::HOT_BONE),
        TextLayout::new_with_justify(Justify::Center),
        ScreenAnchor::new(0.0, 0.80, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, 200.0),
        Visibility::Hidden,
    ));
    // The line under the table: the tappable arena name on couch, the
    // career record online. Never an instruction.
    commands.spawn((
        TitleOverlay,
        Text2d::new(String::new()),
        TextFont {
            font_size: 34.0,
            ..default()
        },
        TextColor(render::palette::BONE),
        TextLayout::new_with_justify(Justify::Center),
        TextBounds::new_horizontal(TITLE_BODY_WIDTH),
        ScreenAnchor::new(0.0, SUBLINE_ANCHOR_Y, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, 200.0),
        Visibility::Hidden,
    ));
    // The primary: bottom, biggest, and it always names what tapping it
    // does right now (PLAY / FIND OPPONENT / DUEL AT C-U-R-S).
    spawn_title_button(
        &mut commands,
        TitleAction::Play,
        Vec2::new(0.0, PLAY_ANCHOR_Y),
        Vec2::new(760.0, 150.0),
        48.0,
    );
    // The secondary row: everything you reach for occasionally, in one
    // line, out of the way of the table and the primary.
    spawn_title_button(
        &mut commands,
        TitleAction::Practice,
        Vec2::new(-0.72, SECONDARY_ANCHOR_Y),
        Vec2::new(250.0, 92.0),
        22.0,
    );
    spawn_title_button(
        &mut commands,
        TitleAction::Rivals,
        Vec2::new(-0.24, SECONDARY_ANCHOR_Y),
        Vec2::new(230.0, 92.0),
        22.0,
    );
    spawn_title_button(
        &mut commands,
        TitleAction::Replays,
        Vec2::new(0.24, SECONDARY_ANCHOR_Y),
        Vec2::new(230.0, 92.0),
        22.0,
    );
    spawn_title_button(
        &mut commands,
        TitleAction::Settings,
        Vec2::new(0.72, SECONDARY_ANCHOR_Y),
        Vec2::new(240.0, 92.0),
        22.0,
    );
    // Summary overlay — screen-centered (the kill-cam holds zoomed-in on
    // MatchOver, so a world-parked summary would sit off-center), shown only
    // on MatchOver. Sits above the primary button's band, which reuses the
    // PLAY slot below.
    commands.spawn((
        SummaryOverlay,
        Text2d::new(String::new()),
        TextFont {
            // 56, not 72: the card carries full sentences (rivalry line,
            // replay note) and they must fit the bounds below unwrapped.
            font_size: 56.0,
            ..default()
        },
        TextColor(render::palette::HOT_BONE),
        TextLayout::new_with_justify(Justify::Center),
        // Wide explicit bounds — without them Bevy wraps Text2d at awkward
        // widths ("STAG WINS" became STAG/WINS, "2 - 5" split mid-score).
        // Same fix the summoning overlay needed (lobby_overlay.rs).
        TextBounds::new_horizontal(TITLE_BODY_WIDTH),
        ScreenAnchor::new(0.0, 0.12, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, 200.0),
        Visibility::Hidden,
    ));
    spawn_summary_buttons(&mut commands);
    spawn_quit_button(&mut commands);
    spawn_bot_offer_button(&mut commands);
}

/// Title-screen arena entry. Desktop: 1-7 pick directly. Touch: a tap on the
/// arena line opens the roster screen (`crate::arena_select`) — couch AND
/// online, since the pair agrees on a table after pairing
/// (`netplay::agreed_table`). Mutating [`SelectedArena`] is safe here because
/// there's no session (the sim is idle) and `spawn_match` only reads it on
/// match start.
fn pick_arena(
    keys: Res<ButtonInput<KeyCode>>,
    touches: Res<Touches>,
    window: Res<WindowSize>,
    netplay: Res<NetplayConfig>,
    mut selected: ResMut<SelectedArena>,
    mut settings: ResMut<crate::settings::Settings>,
    mut next: ResMut<NextState<AppScreen>>,
) {
    // Couch only for the digit shortcuts — the ONLINE title's digits belong
    // to the room pad (1-4) and the name dial (5-8). Online desktop opens
    // the roster with A instead.
    if netplay.room_url.is_none() {
        const DIGITS: [(KeyCode, KeyCode); 7] = [
            (KeyCode::Digit1, KeyCode::Numpad1),
            (KeyCode::Digit2, KeyCode::Numpad2),
            (KeyCode::Digit3, KeyCode::Numpad3),
            (KeyCode::Digit4, KeyCode::Numpad4),
            (KeyCode::Digit5, KeyCode::Numpad5),
            (KeyCode::Digit6, KeyCode::Numpad6),
            (KeyCode::Digit7, KeyCode::Numpad7),
        ];
        for (i, (a, b)) in DIGITS.into_iter().enumerate() {
            if keys.just_pressed(a) || keys.just_pressed(b) {
                selected.0 = sim::ALL_ARENAS[i];
                settings.arena = selected.0.as_u8();
                crate::settings::persist(&settings);
            }
        }
    }
    if keys.just_pressed(KeyCode::KeyA) {
        next.set(AppScreen::ArenaSelect);
        return;
    }
    if ui::tapped(
        touches.iter_just_pressed().map(|t| t.position()),
        SUBLINE_RECT,
        window.0,
    ) {
        next.set(AppScreen::ArenaSelect);
    }
}

/// The single title input handler: keyboard shortcuts plus taps dispatched to
/// the visible button bands. No more invisible screen-half zones — a touch
/// only does something when it lands on the PLAY or PRACTICE button.
#[allow(clippy::too_many_arguments)]
fn title_buttons_input(
    keys: Res<ButtonInput<KeyCode>>,
    touches: Res<Touches>,
    window: Res<WindowSize>,
    mut next: ResMut<NextState<AppScreen>>,
    mut practice: ResMut<crate::bot::PracticeMode>,
    mut pending_bot: ResMut<PendingBotMatch>,
    mut pending_find: ResMut<PendingFindOpponent>,
    mut autostart: Local<Option<Option<AppScreen>>>,
) {
    // A bot fallback tapped mid-wait lands on the Title for one frame:
    // go straight back in with practice armed. FIND OPPONENT after the
    // lesson lands here the same way, practice off.
    if pending_bot.0 || pending_find.0 {
        pending_bot.0 = false;
        pending_find.0 = false;
        next.set(AppScreen::InMatch);
        return;
    }
    // TWOTOP_AUTOSTART=1 skips the gesture; =replays / =settings / =name
    // boot straight into those screens (headless capture verification).
    let auto = *autostart.get_or_insert_with(|| {
        std::env::var("TWOTOP_AUTOSTART")
            .map(|v| match v.as_str() {
                "1" => Some(AppScreen::InMatch),
                "replays" => Some(AppScreen::Replays),
                "settings" => Some(AppScreen::Settings),
                "rivals" | "rivalsdetail" => Some(AppScreen::Rivals),
                "name" => Some(AppScreen::NameEntry),
                "arenas" => Some(AppScreen::ArenaSelect),
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
    if keys.just_pressed(KeyCode::KeyR) {
        next.set(AppScreen::Rivals);
        return;
    }
    if keys.just_pressed(KeyCode::KeyO) {
        next.set(AppScreen::Settings);
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
        if in_band(yd, SECONDARY_ROW_RECT) {
            if xd < SECONDARY_PRACTICE_X {
                practice.0 = !practice.0;
            } else if xd < SECONDARY_RIVALS_X {
                next.set(AppScreen::Rivals);
                return;
            } else if xd < SECONDARY_REPLAYS_X {
                next.set(AppScreen::Replays);
                return;
            } else {
                next.set(AppScreen::Settings);
                return;
            }
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
    room: Res<crate::room_code::RoomCode>,
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
                        // The primary always names what tapping it does NOW:
                        // practice armed → PLAY the bot; private room dialed
                        // → DUEL AT that code (the button narrating its own
                        // mode is what makes the dial above it explain
                        // itself); else online → FIND OPPONENT; else PLAY.
                        TitleAction::Play => {
                            if !online || practice.0 {
                                "PLAY".to_string()
                            } else if room.custom {
                                format!(
                                    "DUEL AT {}",
                                    crate::room_code::code_dashed(&room.code_string())
                                )
                            } else {
                                "FIND OPPONENT".to_string()
                            }
                        }
                        // The label carries the gauntlet climb once one is
                        // underway; the filled/inverted box stays the on/off
                        // state either way.
                        TitleAction::Practice => {
                            if crate::grudge::gauntlet_mastered(career.gauntlet_tier) {
                                "GAUNTLET MASTERED".to_string()
                            } else if career.gauntlet_tier > 0 {
                                format!("GAUNTLET {}", career.gauntlet_tier)
                            } else {
                                "PRACTICE".to_string()
                            }
                        }
                        TitleAction::Rivals => "RIVALS".to_string(),
                        TitleAction::Replays => "REPLAYS".to_string(),
                        TitleAction::Settings => "SETTINGS".to_string(),
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
    let tapped = ui::tapped(
        touches.iter_just_pressed().map(|t| t.position()),
        TOP_EXIT_BAND,
        window.0,
    );
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
    sync: Res<crate::netplay::SyncWait>,
    mut awaiting: ResMut<AwaitingPeer>,
) {
    // Paired but not yet synchronized is still waiting: the challenger
    // has not arrived until their first frame has.
    awaiting.0 = *screen.get() == AppScreen::InMatch
        && netplay.room_url.is_some()
        && !practice.0
        && !theater.active()
        && (!lobby.is_in_match() || sync.waiting());
}

/// Arm PLAY THE BOT only once the summons has actually hung (or failed).
fn update_bot_offer(
    time: Res<Time<Real>>,
    awaiting: Res<AwaitingPeer>,
    lobby: Res<net::LobbyState>,
    mut offer: ResMut<BotOffer>,
) {
    if !awaiting.0 {
        *offer = BotOffer::default();
        return;
    }
    let now = time.elapsed_secs();
    let since = *offer.since.get_or_insert(now);
    let failed = matches!(
        *lobby,
        net::LobbyState::SummonFailed | net::LobbyState::PeerUnreachable
    );
    offer.armed = bot_offer_armed(now - since, failed);
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
        // The tappable arena line shows on BOTH modes now — online it is
        // this side's offer in the table handshake. The career
        // record joins it once there is one.
        let online = netplay.room_url.is_some();
        let arena_line = format!("< {} >", render::arena::info(selected.0).name);
        text.0 = if online && career.total() > 0 {
            format!("{arena_line}\ncareer {}W-{}L", career.wins, career.losses)
        } else {
            arena_line
        };
    } else {
        *vis = Visibility::Hidden;
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

    fn online_summary(gone: bool) -> String {
        let peer = net::ProfileData {
            install_id: 7,
            name: net::name_slots(&[19, 0, 6, 2]), // TAGC
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
            false, // we_fled
            false, // desynced
            false, // unfinished
            true,
        )
    }

    /// A mid-match forfeit: nobody reached the threshold; blame decides.
    fn forfeit_summary(we_fled: bool) -> String {
        let peer = net::ProfileData {
            install_id: 7,
            name: net::name_slots(&[19, 0, 6, 2]), // TAGC
        };
        summary_text(
            MatchScore { p0: 2, p1: 1 },
            true,
            false,
            None,
            Some(0),
            "CURS",
            Some(peer),
            None,
            true, // opponent_gone
            we_fled,
            false, // desynced
            false, // unfinished
            false,
        )
    }

    #[test]
    fn forfeit_winner_is_whoever_stayed() {
        // The peer fled at 2-1: WE win, whatever the raw score says.
        let stayed = forfeit_summary(false);
        assert!(stayed.contains("CURS WINS"), "{stayed}");
        assert!(stayed.contains("the field is yours"), "{stayed}");
        // We walked out: the peer takes it and the card owns the blame.
        let fled = forfeit_summary(true);
        assert!(fled.contains("TAGC WINS"), "{fled}");
        assert!(fled.contains("you left the duel"), "{fled}");
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
        // A fled opponent doesn't kill the slot — it offers the bot.
        assert_eq!(
            primary_label(true, false, c(false, false), "TAGC", true).unwrap(),
            "PLAY THE BOT"
        );
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
    fn quit_label_walks_its_states() {
        assert_eq!(quit_label(false, false, false), "QUIT");
        assert_eq!(quit_label(false, false, true), "SURE?");
        assert_eq!(quit_label(false, true, false), "SKIP");
        // The summoning wait cancels in one tap — no arming, no SURE?.
        assert_eq!(quit_label(true, false, false), "CANCEL");
        assert_eq!(quit_label(true, false, true), "CANCEL");
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
            false, // desynced
            false, // unfinished
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
            false,
            false, // desynced
            false, // unfinished
            true,
        );
        assert!(text.contains("CUR WINS"), "{text}");
        assert!(text.contains("replay saved"), "{text}");
    }
}

#[cfg(test)]
mod practice_summary_tests {
    use super::*;

    #[test]
    fn the_lesson_ends_ready_either_way() {
        let won = MatchScore {
            p0: MATCH_WIN_THRESHOLD,
            p1: 1,
        };
        let text = first_blood_summary(won, Some((0, 1)), false);
        assert!(text.starts_with("YOU'RE READY"), "{text}");
        assert!(text.contains("FIRST BLOOD"), "{text}");
        assert!(text.contains("UP TO GAUNTLET 1"), "{text}");
        let lost = MatchScore {
            p0: 3,
            p1: MATCH_WIN_THRESHOLD,
        };
        let text = first_blood_summary(lost, Some((0, 0)), false);
        assert!(text.starts_with("YOU'RE READY"), "{text}");
        assert!(!text.contains("GAUNTLET"), "{text}");
    }

    #[test]
    fn the_practice_card_speaks_to_the_player_and_names_the_rung() {
        let won = MatchScore {
            p0: MATCH_WIN_THRESHOLD,
            p1: 2,
        };
        let lost = MatchScore {
            p0: 1,
            p1: MATCH_WIN_THRESHOLD,
        };
        let text = practice_summary(won, None, Some((3, 4)), true);
        assert!(text.starts_with("YOU WIN"), "{text}");
        assert!(text.contains("UP TO GAUNTLET 4"), "{text}");
        assert!(text.contains("replay saved"), "{text}");
        assert!(!text.contains("CUR"), "{text}");
        let text = practice_summary(lost, None, Some((5, 4)), false);
        assert!(text.starts_with("THE BOT WINS"), "{text}");
        assert!(text.contains("DOWN TO GAUNTLET 4"), "{text}");
        // A shade moves no rung and is named as what it is.
        let text = practice_summary(lost, Some("CURS"), None, false);
        assert!(text.starts_with("CURS SHADE WINS"), "{text}");
        assert!(!text.contains("GAUNTLET"), "{text}");
    }
}

#[cfg(test)]
mod bot_offer_tests {
    use super::*;

    #[test]
    fn the_bot_is_offered_only_once_the_summons_has_hung() {
        // A mashed double-tap lands ~150 ms after the first: no offer.
        assert!(!bot_offer_armed(0.0, false));
        assert!(!bot_offer_armed(0.15, false));
        assert!(!bot_offer_armed(BOT_OFFER_DELAY_SECS - 0.01, false));
        assert!(bot_offer_armed(BOT_OFFER_DELAY_SECS, false));
        // A summons that failed outright has nothing left to wait for.
        assert!(bot_offer_armed(0.0, true));
    }
}

#[cfg(test)]
mod unfinished_summary_tests {
    use super::*;

    /// An earned score outranks a dropped link, on the card exactly as in
    /// the ledger — the two must never disagree about the same match.
    #[test]
    fn a_decided_score_still_crowns_the_winner_even_if_the_link_then_dies() {
        let peer = net::ProfileData {
            install_id: 7,
            name: net::name_slots(&[19, 0, 6, 2]), // TAGC
        };
        let text = summary_text(
            MatchScore {
                p0: MATCH_WIN_THRESHOLD,
                p1: 2,
            },
            true,
            false,
            None,
            Some(0),
            "CURS",
            Some(peer),
            None,
            true,  // opponent_gone
            false, // we_fled
            false, // desynced
            true,  // unfinished — but the score was reached
            false,
        );
        assert!(text.contains("CURS WINS"), "{text}");
        assert!(!text.contains("CONNECTION LOST"), "{text}");
        // And the ledger agrees on the same inputs.
        assert_eq!(
            crate::grudge::match_outcome(
                MATCH_WIN_THRESHOLD,
                2,
                Some(crate::grudge::ForfeitKind::Silent),
                false
            ),
            crate::grudge::Outcome::Won
        );
    }

    #[test]
    fn a_silent_drop_crowns_nobody() {
        let peer = net::ProfileData {
            install_id: 7,
            name: net::name_slots(&[19, 0, 6, 2]), // TAGC
        };
        let text = summary_text(
            MatchScore { p0: 2, p1: 1 },
            true,
            false,
            None,
            Some(0),
            "CURS",
            Some(peer),
            Some("3RD MEETING with TAGC - tied 1-1".into()),
            true,  // opponent_gone
            false, // we_fled
            false, // desynced
            true,  // unfinished
            false,
        );
        assert!(text.contains("CONNECTION LOST"), "{text}");
        assert!(text.contains("nobody's win"), "{text}");
        assert!(!text.contains("WINS"), "{text}");
        assert!(text.contains("3RD MEETING"), "{text}");
    }
}

#[cfg(test)]
mod rematch_lockout_tests {
    use super::*;

    #[test]
    fn the_decided_match_is_not_settled_until_the_lockout_has_run() {
        assert!(!MatchOverSince(None).settled(100.0));
        let since = MatchOverSince(Some(10.0));
        assert!(!since.settled(10.0));
        assert!(!since.settled(10.0 + REMATCH_LOCKOUT_SECS - 0.01));
        assert!(since.settled(10.0 + REMATCH_LOCKOUT_SECS));
    }

    #[test]
    fn a_thumb_held_through_the_lockout_must_lift_before_it_counts() {
        // Mashing through the kill: nothing arms during the lockout.
        assert!(!rematch_armed(false, false, true));
        assert!(!rematch_armed(false, false, false));
        // Lockout over, but THROW still down from the fight: not yet.
        assert!(!rematch_armed(false, true, true));
        // The thumb comes up: armed, and the next press is a real one.
        assert!(rematch_armed(false, true, false));
        // Armed stays armed while the press it was waiting for arrives.
        assert!(rematch_armed(true, true, true));
    }
}
