use bevy::prelude::*;
use bevy_ggrs::prelude::*;
use bevy_ggrs::{
    AdvanceWorld, AdvanceWorldSystems, GgrsConfig, LocalInputs, LocalPlayers, PlayerInputs,
    RollbackApp, SyncTestMismatch,
};
use bytemuck::{Pod, Zeroable};
use core::net::SocketAddr;
use fixed_math::{Fix, RectF, Vec2F};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ---- Wire input ----

/// Wire-format input. Exactly 4 bytes per player per frame.
///
/// Level signals only — edges (`just_pressed` etc.) are derived in sim
/// against a rolled-back `PreviousInputs` resource, never sent on the wire.
#[repr(C)]
#[derive(
    Clone, Copy, Default, PartialEq, Eq, Hash, Debug, Pod, Zeroable, Serialize, Deserialize,
)]
pub struct PlayerInput {
    pub stick_x: i8,
    pub stick_y: i8,
    pub aim_angle: u8,
    pub buttons: u8,
}

impl PlayerInput {
    pub const THROW_DOWN: u8 = 0b0000_0001;
    pub const AIM_ACTIVE: u8 = 0b0000_0010;
    pub const DASH_DOWN: u8 = 0b0000_0100;
    pub const TAUNT_DOWN: u8 = 0b0000_1000;
    // Bits 4-7 reserved.
}

// ---- ggrs config ----

pub type GgrsCfg = GgrsConfig<PlayerInput, SocketAddr>;

pub const TICK_HZ: usize = 60;
pub const TICK_DT: Fix = Fix::lit("0.01666666666");

/// Strict-match version stamped on `.bmrg` replays. `u32::MAX` is the dev
/// sentinel — every commit on `main` carries it. A release tag bumps this
/// to a real number so old replays are routed back to their tagged binary
/// rather than silently loaded into a binary with different sim semantics.
/// See `replay::decode_for_sim_version` for the gate.
pub const SIM_VERSION: u32 = u32::MAX;

// ---- Components ----

#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[require(Rollback)]
pub struct PositionF(pub Vec2F);

/// Snapshot of `PositionF` taken at the *start* of each `GgrsSchedule`
/// tick. Render-side `sync_transforms_from_sim` lerps between this and the
/// current `PositionF` using `LastSimTickTime` + tick rate. Maintaining
/// this lag is the contract that lets the visual layer run at any frame
/// rate while the sim stays at 60 Hz.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[require(Rollback)]
pub struct PreviousPositionF(pub Vec2F);

#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[require(Rollback)]
pub struct VelocityF(pub Vec2F);

#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[require(Rollback, DashState, StunFrames)]
pub struct Player {
    pub handle: usize,
}

/// Dash mechanic per Phase 9. Idle waiting for a DASH_DOWN edge;
/// Dashing for `DASH_DURATION_FRAMES` after a successful trigger,
/// applying a locked-direction high-speed velocity each tick;
/// Cooldown for `DASH_COOLDOWN_FRAMES` afterwards before the next
/// dash is allowed. The locked direction lives in the Dashing variant
/// so a mid-dash stick-direction change doesn't curve the dash.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[require(Rollback)]
pub enum DashState {
    #[default]
    Idle,
    Dashing {
        frames_remaining: u32,
        dir: Vec2F,
    },
    Cooldown {
        frames_remaining: u32,
    },
}

/// Invulnerability frames countdown. > 0 means the player ignores
/// incoming damage this tick. Set when a dash starts; decrements each
/// tick. Phase 11 reads this in `hit_boomerang_player` to gate hits.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[require(Rollback)]
pub struct StunFrames(pub u32);

/// Phase 11 — player has been killed and is awaiting respawn at
/// `respawn_at_frame`. While present, the existing player-input
/// systems (`player_movement`, `start_dash`, `throw_boomerangs`) and
/// hit detection (`hit_boomerang_player`) all filter the entity out
/// via `Without<Dead>`, so the dead player can't move, dash, throw,
/// or be re-killed. In-flight boomerangs owned by a dead player
/// continue to fly, ricochet, recall, and catch normally — the
/// corpse's position is still tracked as the recall target until
/// cycle 2's respawn snap.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[require(Rollback)]
pub struct Dead {
    pub respawn_at_frame: u32,
}

/// Frames a player stays Dead before respawn. 180 = 3 s @ 60 Hz —
/// long enough that a kill feels punitive without dragging the round
/// to a halt while still leaving room for the killer to reposition.
pub const RESPAWN_FRAMES: u32 = 180;

/// Phase 11 cycle 4: hit-stop. On a successful kill the killer's
/// `StunFrames` is bumped to at least this many ticks (~100 ms @ 60 Hz)
/// so the kill reads as a brief impact freeze (Phase 15 animation
/// will pick up the freeze cue) and the killer can't be instantly
/// countered by a buffered throw — the brief stun-window doubles as
/// short i-frame insurance. We `.max()` against the existing
/// `StunFrames` value so mid-dash killers keep their longer i-frames
/// rather than being truncated to `HIT_STOP_FRAMES`.
pub const HIT_STOP_FRAMES: u32 = 6;

/// Boomerang state machine. Phase 10 cycle 1 only exercises Flying;
/// Returning lands in cycle 3 (recall trigger). The state lives on a
/// rollback entity alongside `PositionF`/`VelocityF`/`PreviousPositionF`.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[require(Rollback)]
pub enum BoomerangState {
    #[default]
    Flying,
    Returning,
}

#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[require(Rollback)]
pub struct Boomerang {
    pub owner_handle: usize,
    pub state: BoomerangState,
}

/// Throw speed in cm/tick. ~3.8× walk speed: noticeably faster than
/// the player can move, so the throw reads as an attack rather than a
/// projectile drift. 50 × 60 = 3000 cm/sec.
pub const THROW_SPEED_CM_PER_TICK: i32 = 50;

/// Boomerang collision half-extent in cm. Smaller than the player's
/// 16 cm: ~10 cm gives a 20 cm catch/hit footprint that reads as a
/// chunky thrown weapon without making it cheese-easy to hit with.
pub const BOOMERANG_HALF_EXTENT_CM: i32 = 10;

/// Recall speed in cm/tick. A touch faster than `THROW_SPEED` so the
/// boomerang catches up to a player who's moved forward since the
/// throw — recall reads as "reeling in" rather than "drifting back".
pub const RECALL_SPEED_CM_PER_TICK: i32 = 55;

/// Distance from the world origin at which a boomerang is despawned.
/// Generously outside the arena (1000 cm half-extent of the visible
/// space; 4000 gives ~3 s of straight flight before despawn at
/// THROW_SPEED). Cycle 2's wall ricochet should keep boomerangs
/// bounded inside the arena under normal play; this radius is just a
/// safety net so a stuck-velocity boomerang can't run out the
/// `Fix` integer range (±32767) and panic on overflow.
pub const BOOMERANG_DESPAWN_RADIUS_CM: i32 = 4000;

pub fn boomerang_rect(pos: Vec2F) -> RectF {
    let half = Vec2F::from_cm(BOOMERANG_HALF_EXTENT_CM, BOOMERANG_HALF_EXTENT_CM);
    RectF::from_center_half_extents(pos, half)
}

/// Forgiveness window for THROW_DOWN edge detection. Same 6-frame
/// window as Phase 8's standard forgiveness — 100 ms at 60 Hz.
pub const THROW_FORGIVENESS_FRAMES: usize = 6;

/// Pure helper: should this player throw a boomerang this tick?
/// Returns the throw direction iff:
///   - they don't already own a boomerang in flight,
///   - THROW_DOWN was released this tick OR within the last
///     `THROW_FORGIVENESS_FRAMES` ticks of history,
///   - the stick has a usable direction.
///
/// The this-tick check (`just_released` against ring's last entry) is
/// what makes the throw feel snappy — fires the same tick as the
/// release, no 16 ms delay. The forgiveness window scan only catches
/// the tail (e.g. if a player tapped release and *then* nudged the
/// stick into a direction).
pub fn try_throw_direction(
    history_ring: &[PlayerInput; INPUT_HISTORY_LEN],
    current_input: PlayerInput,
    has_existing_boomerang: bool,
) -> Option<Vec2F> {
    if has_existing_boomerang {
        return None;
    }
    let just_released_this_tick = just_released(
        current_input,
        previous_input(history_ring),
        PlayerInput::THROW_DOWN,
    );
    let released_recently = released_within(
        history_ring,
        THROW_FORGIVENESS_FRAMES,
        PlayerInput::THROW_DOWN,
    );
    if !just_released_this_tick && !released_recently {
        return None;
    }
    let stick = decode_stick(current_input);
    if stick.length() <= DASH_MIN_STICK_MAG {
        return None;
    }
    Some(stick.normalize())
}

pub const DASH_DURATION_FRAMES: u32 = 10;
pub const DASH_COOLDOWN_FRAMES: u32 = 20;
/// Dash impulse speed in cm/tick. ~2.3× walk speed: makes dash feel
/// distinctly impulsive without crossing more than a fifth of the
/// arena per dash (10 ticks × 30 cm = 300 cm of travel; arena width is
/// 1000 cm).
pub const DASH_SPEED_CM_PER_TICK: i32 = 30;
/// Minimum stick magnitude required to start a dash. Without this, a
/// barely-deflected stick would commit to a near-random dash direction
/// after the deadzone-collapse rounding.
pub const DASH_MIN_STICK_MAG: Fix = Fix::lit("0.1");

/// Player collision half-extent in centimeters. ~16 cm gives a 32 cm
/// (≈12 in) square footprint — read at a glance from the camera-zoom
/// distance we expect for a portrait phone, and small enough that the
/// 1000×1500 cm arena gives plenty of room to dodge.
pub const PLAYER_HALF_EXTENT_CM: i32 = 16;

/// Compute the player's collision AABB centered on `pos`.
pub fn player_rect(pos: Vec2F) -> RectF {
    let half = Vec2F::from_cm(PLAYER_HALF_EXTENT_CM, PLAYER_HALF_EXTENT_CM);
    RectF::from_center_half_extents(pos, half)
}

/// Wall geometry kind. Solid v1 — boomerangs will bounce, players
/// can't pass through. Future kinds (one-way, breakable) extend
/// this enum.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum WallKind {
    Solid,
}

/// Static arena geometry. Not a `Rollback` requirement — walls don't
/// move, don't change kind, and aren't subject to resimulation. They
/// live in the world from app startup and are queried each tick by
/// the collision system.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Wall {
    pub kind: WallKind,
    pub rect: RectF,
}

/// Canonical arena dimensions. Centered on the world origin; the
/// interior playable space is ±500 cm × ±750 cm = 1000 × 1500 cm.
/// Walls (50 cm thick) ring the outside; corners are covered by the
/// vertical walls so all four corner cells have wall geometry.
pub const ARENA_HALF_WIDTH_CM: i32 = 500;
pub const ARENA_HALF_HEIGHT_CM: i32 = 750;
pub const WALL_THICKNESS_CM: i32 = 50;

/// The four boundary walls in fixed spawn order. Returned as a
/// const-friendly array so the app spawns them in the same order on
/// every host. Determinism depends on this ordering (entity ids end
/// up identical across hosts iff the spawn sequence is identical).
pub fn arena_walls() -> [Wall; 4] {
    let inner_x = ARENA_HALF_WIDTH_CM;
    let inner_y = ARENA_HALF_HEIGHT_CM;
    let t = WALL_THICKNESS_CM;
    [
        // North (top): full inner width, thickness above the arena.
        Wall {
            kind: WallKind::Solid,
            rect: RectF::from_min_max(
                Vec2F::from_cm(-inner_x, inner_y),
                Vec2F::from_cm(inner_x, inner_y + t),
            ),
        },
        // South (bottom): full inner width, thickness below the arena.
        Wall {
            kind: WallKind::Solid,
            rect: RectF::from_min_max(
                Vec2F::from_cm(-inner_x, -inner_y - t),
                Vec2F::from_cm(inner_x, -inner_y),
            ),
        },
        // West (left): full corner-to-corner height (covers top-left
        // and bottom-left corners), thickness to the left.
        Wall {
            kind: WallKind::Solid,
            rect: RectF::from_min_max(
                Vec2F::from_cm(-inner_x - t, -inner_y - t),
                Vec2F::from_cm(-inner_x, inner_y + t),
            ),
        },
        // East (right): mirror of west.
        Wall {
            kind: WallKind::Solid,
            rect: RectF::from_min_max(
                Vec2F::from_cm(inner_x, -inner_y - t),
                Vec2F::from_cm(inner_x + t, inner_y + t),
            ),
        },
    ]
}

/// Marker: render-side `sync_transforms_from_sim` skips interpolation for
/// entities carrying this component and uses `PositionF` directly. Useful
/// for entities whose sim-side position changes shouldn't smear on screen
/// (UI overlays, fixed-position decals, debug indicators). Rolled back so
/// the marker's presence/absence is consistent during resimulation.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[require(Rollback)]
pub struct NoInterpolate;

// ---- Resources ----

#[derive(Resource, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct FrameCount(pub u32);

/// Per-player round-win score. Increments on each kill via
/// `hit_boomerang_player`. Rolled back so a kill that gets undone by
/// rollback resimulation also undoes the score bump. Phase 11 cycle 6
/// reads this to detect first-to-N round-win → MatchOver transition.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct MatchScore {
    pub p0: u8,
    pub p1: u8,
}

/// Frames per countdown digit (3, 2, 1) — 60 = 1 s @ 60 Hz.
pub const COUNTDOWN_DIGIT_FRAMES: u32 = 60;

/// Active round duration. 1800 = 30 s @ 60 Hz; matches BUILD_PLAN
/// § Phase 11 and is short enough that a stalemate doesn't drag.
pub const ROUND_DURATION_FRAMES: u32 = 1800;

/// Beat between rounds. 60 = 1 s @ 60 Hz; long enough for an animated
/// "round won" flourish in Phase 15, short enough that the next round
/// drops in cleanly.
pub const ROUND_OVER_FRAMES: u32 = 60;

/// Total kills required to end the match (`InRound`/`RoundOver` →
/// `MatchOver`). Cycle 6's simplest scoring rule is "first to 5
/// kills"; the round timer still rotates state for input-gating and
/// future cleanup pulses, but doesn't independently end the match.
/// BUILD_PLAN's older "first to 5 round wins" framing is satisfied
/// in spirit: 5 kills is a reasonable proxy for round-level
/// dominance and avoids a second per-round kill counter.
pub const MATCH_WIN_THRESHOLD: u8 = 5;

/// Round/match state machine. Rolled back as a plain `Resource` enum
/// rather than via `bevy_roll_safe::init_ggrs_state` because
/// `bevy_roll_safe` 0.7.0 caps `bevy_ggrs` at `^0.20` and we're on
/// `=0.21`. See MORGAN_NOTES § "Why we cut bevy_roll_safe" for the
/// rationale. The lost capability is `OnEnter`/`OnExit` lifecycle
/// hooks — we don't use them; transitions are explicit pattern
/// matches inside the gameplay systems that drive them.
///
/// Round flow: `Countdown` ticks down to zero (3-2-1, 60 frames per
/// digit), then `InRound` until the round timer expires or a player
/// reaches the win threshold; then `RoundOver` for a brief beat
/// before the next `Countdown` (or `MatchOver` if the match is
/// decided). Frame-numbered fields name the tick the next transition
/// fires on so the system reading them is a single comparison
/// against `FrameCount`.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MatchState {
    /// Pre-round countdown. `digit` is 3, 2, or 1; `expires_at_frame`
    /// is the tick the digit changes (decrement, or transition to
    /// `InRound` when digit hits 0).
    Countdown { digit: u8, expires_at_frame: u32 },
    /// Active round. `expires_at_frame` is the tick the round timer
    /// runs out (transitions to `RoundOver`).
    InRound { expires_at_frame: u32 },
    /// Post-round beat before the next round starts.
    /// `expires_at_frame` is the tick the next `Countdown` begins.
    RoundOver { expires_at_frame: u32 },
    /// Match decided. Terminal state — no further transitions.
    MatchOver,
}

impl Default for MatchState {
    fn default() -> Self {
        // Match opens at the top of the countdown. `tick_match_state`
        // reads `FrameCount.0 >= expires_at_frame` to fire transitions;
        // FrameCount starts at 0. Setting the initial expiry to
        // `COUNTDOWN_DIGIT_FRAMES - 1` makes the transition fire on
        // the 60th tick (frame.0 == 59 at start of tick), so digit "3"
        // visibly shows for ticks 0..=59 (60 ticks = 1 s) and the
        // pattern composes cleanly with the post-init transitions
        // which always set `expires = frame.0 + COUNTDOWN_DIGIT_FRAMES`.
        MatchState::Countdown {
            digit: 3,
            expires_at_frame: COUNTDOWN_DIGIT_FRAMES - 1,
        }
    }
}

impl MatchState {
    /// True iff the round is active and gameplay inputs should be
    /// honored. `Countdown` / `RoundOver` / `MatchOver` lock the
    /// players in place.
    pub fn is_in_round(&self) -> bool {
        matches!(self, MatchState::InRound { .. })
    }

    /// Headless-test helper: an `InRound` state with a far-future
    /// expiry, suitable for sync_test / replay_sync / unit-test
    /// ceremonies where the countdown and round-end transitions are
    /// noise rather than the system under test. Pair with
    /// [`InfiniteRoundPlugin`] for the one-line plugin form.
    pub fn infinite_round() -> Self {
        MatchState::InRound {
            expires_at_frame: u32::MAX,
        }
    }
}

/// Wall-clock time of the most recent simulated tick. Render layer reads
/// this to interpolate `Transform` between sim frames.
///
/// Updated in the `AdvanceWorld` schedule under `AdvanceWorldSystems::Last`,
/// so it captures the moment the most recent tick (rolled-back or not)
/// finished advancing. Not itself rolled back — purely a render-side
/// timestamp.
#[derive(Resource, Default)]
pub struct LastSimTickTime(pub f64);

// ---- Systems ----

/// Read synthesized inputs each frame. The driver mutates this between
/// `app.update()` calls; the `read_local_inputs` system copies into
/// `LocalInputs<GgrsCfg>`.
#[derive(Resource, Default)]
pub struct SynthesizedInputs(pub PlayerInput);

/// Length of the per-player input ring. 8 ticks (~133ms at 60Hz) covers
/// the standard 100ms forgiveness window with headroom for sequence
/// detection (e.g. dash-cancel into throw).
pub const INPUT_HISTORY_LEN: usize = 8;

/// Per-handle ring buffer of the last `INPUT_HISTORY_LEN` ticks of
/// inputs. Index 0 is oldest, INPUT_HISTORY_LEN-1 is newest. Pushed
/// at the END of each `GgrsSchedule` tick by `advance_input_history`,
/// so during edge consumers in tick N the ring's last entry is tick
/// N-1's input — i.e. "previous" from the consumer's POV. Edges are
/// derived by comparing `PlayerInputs<GgrsCfg>` (= current tick) to
/// the ring's last entry.
///
/// Rolled back so resimulation reconstructs the same forgiveness
/// state as live play. `BTreeMap` (not `HashMap`) per CONVENTIONS to
/// keep iteration order portable across hosts.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct InputHistory(pub BTreeMap<usize, [PlayerInput; INPUT_HISTORY_LEN]>);

/// Push `curr` onto the end of `ring`, dropping the oldest entry. Pure
/// helper so cycle 6's logic is testable without a Bevy app.
pub fn push_history(ring: &mut [PlayerInput; INPUT_HISTORY_LEN], curr: PlayerInput) {
    ring.copy_within(1.., 0);
    ring[INPUT_HISTORY_LEN - 1] = curr;
}

/// "Previous tick" from the consumer's POV, given the convention that
/// `advance_input_history` runs at the end of `GgrsSchedule`.
pub fn previous_input(ring: &[PlayerInput; INPUT_HISTORY_LEN]) -> PlayerInput {
    ring[INPUT_HISTORY_LEN - 1]
}

/// Rising edge: bit was low last tick and is high this tick.
pub fn just_pressed(curr: PlayerInput, prev: PlayerInput, mask: u8) -> bool {
    (curr.buttons & mask != 0) && (prev.buttons & mask == 0)
}

/// Falling edge: bit was high last tick and is low this tick.
pub fn just_released(curr: PlayerInput, prev: PlayerInput, mask: u8) -> bool {
    (curr.buttons & mask == 0) && (prev.buttons & mask != 0)
}

/// Was a rising edge present in the last `n` adjacent-pair transitions
/// of the ring? `n=1` checks only the very last transition; larger n
/// values widen the forgiveness window.
pub fn pressed_within(ring: &[PlayerInput; INPUT_HISTORY_LEN], n: usize, mask: u8) -> bool {
    let n = n.min(INPUT_HISTORY_LEN - 1);
    for i in 0..n {
        let newer = INPUT_HISTORY_LEN - 1 - i;
        let older = newer - 1;
        if (ring[older].buttons & mask == 0) && (ring[newer].buttons & mask != 0) {
            return true;
        }
    }
    false
}

/// Was a falling edge present in the last `n` adjacent-pair transitions
/// of the ring? Mirrors `pressed_within`.
pub fn released_within(ring: &[PlayerInput; INPUT_HISTORY_LEN], n: usize, mask: u8) -> bool {
    let n = n.min(INPUT_HISTORY_LEN - 1);
    for i in 0..n {
        let newer = INPUT_HISTORY_LEN - 1 - i;
        let older = newer - 1;
        if (ring[older].buttons & mask != 0) && (ring[newer].buttons & mask == 0) {
            return true;
        }
    }
    false
}

pub fn read_local_inputs(
    mut commands: Commands,
    synthesized: Res<SynthesizedInputs>,
    local_players: Res<LocalPlayers>,
) {
    let mut map = bevy::platform::collections::HashMap::default();
    for handle in &local_players.0 {
        map.insert(*handle, synthesized.0);
    }
    commands.insert_resource(LocalInputs::<GgrsCfg>(map));
}

/// Walk speed in cm/tick. Sized so the arena's longest dimension
/// (2 × ARENA_HALF_HEIGHT_CM = 1500 cm) crosses in ~2 seconds at 60 Hz:
///
///     1500 cm / (13 cm/tick × 60 tick/s) ≈ 1.92 s
///
/// Phase 9 exit criterion was "cross arena in ~2 seconds"; 13 cm/tick
/// hits 1.92 s with integer-friendly arithmetic. Tuning this further is
/// a Phase 9 verify-time decision once the value is felt on a phone.
pub const WALK_SPEED_CM_PER_TICK: i32 = 13;

/// Decode the wire-format stick into a Fix-space vector with
/// magnitude clamped to ≤ 1. Independent-axis i8 quantization means a
/// full-diagonal stick (127, 127) has magnitude √2, which would naively
/// double-fast diagonal travel; clamping fixes that.
pub fn decode_stick(input: PlayerInput) -> Vec2F {
    let stick_max = Fix::const_from_int(127);
    let raw = Vec2F::new(
        Fix::const_from_int(input.stick_x as i32) / stick_max,
        Fix::const_from_int(input.stick_y as i32) / stick_max,
    );
    if raw.length() > Fix::const_from_int(1) {
        raw.normalize()
    } else {
        raw
    }
}

/// Pure transition for `try_start_dash`. Returns the new `DashState`,
/// plus whether a dash was committed (so the system can also set
/// `StunFrames`). Dash starts iff state == Idle, the DASH_DOWN edge
/// fired this tick, and the stick has a usable direction.
pub fn try_start_dash(
    state: DashState,
    stick: Vec2F,
    just_pressed_dash: bool,
) -> (DashState, bool) {
    if !matches!(state, DashState::Idle) || !just_pressed_dash {
        return (state, false);
    }
    if stick.length() <= DASH_MIN_STICK_MAG {
        return (state, false);
    }
    let new_state = DashState::Dashing {
        frames_remaining: DASH_DURATION_FRAMES,
        dir: stick.normalize(),
    };
    (new_state, true)
}

/// Pure transition for `DashState`'s end-of-tick countdown. Dashing
/// burns `frames_remaining`; when it hits 1 (consuming this tick's
/// dash) the next tick begins as Cooldown. Cooldown counts down the
/// same way back to Idle.
pub fn tick_dash_state(state: DashState) -> DashState {
    match state {
        DashState::Idle => state,
        DashState::Dashing {
            frames_remaining,
            dir,
        } => {
            if frames_remaining <= 1 {
                DashState::Cooldown {
                    frames_remaining: DASH_COOLDOWN_FRAMES,
                }
            } else {
                DashState::Dashing {
                    frames_remaining: frames_remaining - 1,
                    dir,
                }
            }
        }
        DashState::Cooldown { frames_remaining } => {
            if frames_remaining <= 1 {
                DashState::Idle
            } else {
                DashState::Cooldown {
                    frames_remaining: frames_remaining - 1,
                }
            }
        }
    }
}

/// `GgrsSchedule` system: detect DASH_DOWN edges and commit dash
/// starts. Runs after `snapshot_previous` and before `player_movement`
/// so this tick's movement system sees the new `DashState::Dashing`.
pub fn start_dash(
    match_state: Res<MatchState>,
    inputs: Res<PlayerInputs<GgrsCfg>>,
    history: Res<InputHistory>,
    mut q: Query<(&Player, &mut DashState, &mut StunFrames), Without<Dead>>,
) {
    if !match_state.is_in_round() {
        return;
    }
    for (player, mut dash, mut stun) in &mut q {
        let (curr, _status) = inputs[player.handle];
        let prev = history
            .0
            .get(&player.handle)
            .map(previous_input)
            .unwrap_or_default();
        let edge = just_pressed(curr, prev, PlayerInput::DASH_DOWN);
        let stick = decode_stick(curr);
        let (new_state, committed) = try_start_dash(*dash, stick, edge);
        *dash = new_state;
        if committed {
            *stun = StunFrames(DASH_DURATION_FRAMES);
        }
    }
}

/// Move players. Branches on `DashState`: while `Dashing`, velocity is
/// the locked dash direction × `DASH_SPEED_CM_PER_TICK`; otherwise
/// velocity comes from the (mag-clamped) stick × `WALK_SPEED_CM_PER_TICK`.
///
/// **Aim lock**: while `AIM_ACTIVE` is set, the stick is repurposed as
/// aim direction/power (input_touch's throw state machine engages
/// aim mode after a hold-and-drag threshold). The player is anchored
/// during aim so committing to a precise throw means committing
/// position — the risk dimension that makes aimed throws skill
/// expression rather than free-cost optimal play. A quick tap-throw
/// (THROW_DOWN held briefly without crossing the aim threshold) does
/// NOT lock movement, so running-and-throwing flows unbroken. Dash
/// overrides this — a dash committed before AIM_ACTIVE was set
/// continues through the aim windup.
pub fn player_movement(
    match_state: Res<MatchState>,
    inputs: Res<PlayerInputs<GgrsCfg>>,
    mut q: Query<(&Player, &mut PositionF, &mut VelocityF, &DashState), Without<Dead>>,
) {
    if !match_state.is_in_round() {
        return;
    }
    let walk_speed = Fix::const_from_int(WALK_SPEED_CM_PER_TICK);
    let dash_speed = Fix::const_from_int(DASH_SPEED_CM_PER_TICK);
    for (player, mut pos, mut vel, dash) in &mut q {
        let velocity = match *dash {
            DashState::Dashing { dir, .. } => Vec2F::new(dir.x * dash_speed, dir.y * dash_speed),
            _ => {
                let (input, _status) = inputs[player.handle];
                if input.buttons & PlayerInput::AIM_ACTIVE != 0 {
                    Vec2F::ZERO
                } else {
                    let stick = decode_stick(input);
                    Vec2F::new(stick.x * walk_speed, stick.y * walk_speed)
                }
            }
        };
        vel.0 = velocity;
        pos.0 = pos.0 + vel.0;
    }
}

/// `GgrsSchedule` system: countdown `DashState` and `StunFrames` at the
/// end of the tick (after movement and collision). Runs before
/// `advance_input_history` so any consumer of "just-finished dash"
/// edge detection in subsequent ticks sees a clean Idle/Cooldown state.
pub fn tick_player_timers(mut q: Query<(&mut DashState, &mut StunFrames)>) {
    for (mut dash, mut stun) in &mut q {
        *dash = tick_dash_state(*dash);
        if stun.0 > 0 {
            stun.0 -= 1;
        }
    }
}

/// `GgrsSchedule` system: a boomerang in `Flying` or `Returning` state
/// whose AABB overlaps a non-owner, non-Dead, non-stunned player kills
/// that player. Inserts the `Dead` component (with respawn frame
/// computed from `FrameCount`) and despawns the boomerang. Runs after
/// `boomerang_physics` + `boomerang_wall_collision` so the post-
/// resolution rect is what's tested; runs before `catch_boomerangs`
/// so a hit-and-catch on the same tick resolves as a kill (the
/// boomerang despawns via the kill path, never reaches catch).
///
/// **Owner immunity**: a boomerang cannot kill the player who threw
/// it. The owner_handle filter handles both the no-self-throw case
/// (matters mainly for the spawn tick when the boomerang spawns
/// inside the owner's rect) and the recalled-boomerang flying back
/// past the owner case.
///
/// **Stun immunity**: `StunFrames > 0` blocks the hit. Currently the
/// dash mechanic seeds `StunFrames` for `DASH_DURATION_FRAMES` ticks,
/// so a well-timed dash dodges incoming projectiles. Phase 11 cycle 4
/// will additionally use `StunFrames` for hit-stop on the killer.
pub fn hit_boomerang_player(
    mut commands: Commands,
    frame: Res<FrameCount>,
    mut score: ResMut<MatchScore>,
    boomerangs: Query<(Entity, &Boomerang, &PositionF)>,
    players: Query<(Entity, &Player, &PositionF, &StunFrames), Without<Dead>>,
) {
    for (boom_entity, boom, boom_pos) in &boomerangs {
        let bb = boomerang_rect(boom_pos.0);
        for (player_entity, player, player_pos, stun) in &players {
            if player.handle == boom.owner_handle {
                continue;
            }
            if stun.0 > 0 {
                continue;
            }
            if !player_rect(player_pos.0).overlaps(bb) {
                continue;
            }
            commands.entity(player_entity).insert(Dead {
                respawn_at_frame: frame.0 + RESPAWN_FRAMES,
            });
            commands.entity(boom_entity).despawn();
            // Award the kill to the boomerang's owner. saturating_add
            // is bookkeeping-safe; in practice MatchOver fires at 5
            // and resets, so the u8 ceiling is unreachable.
            match boom.owner_handle {
                0 => score.p0 = score.p0.saturating_add(1),
                1 => score.p1 = score.p1.saturating_add(1),
                _ => {}
            }
            // Hit-stop on the killer. Skipped if the killer is also
            // Dead (the players query is `Without<Dead>`, so the
            // .find() returns None). Otherwise insert a StunFrames
            // value at least HIT_STOP_FRAMES, preserving any longer
            // mid-dash i-frame window the killer already had.
            if let Some((killer_entity, existing_stun)) = players
                .iter()
                .find(|(_, p, _, _)| p.handle == boom.owner_handle)
                .map(|(e, _, _, s)| (e, s.0))
            {
                commands
                    .entity(killer_entity)
                    .insert(StunFrames(existing_stun.max(HIT_STOP_FRAMES)));
            }
            // One boomerang can only kill one player per tick — break
            // out of the inner loop now that the boomerang is gone, so
            // the next tick's iteration sees the despawned state.
            break;
        }
    }
}

/// `GgrsSchedule` system: catch a Returning boomerang the moment its
/// AABB overlaps the owner's. Despawns the boomerang — no health/score
/// effect yet (Phase 11 will read this). Runs after `boomerang_physics`
/// and `boomerang_wall_collision` (so the catch fires on the tick the
/// boomerang's post-physics rect overlaps the owner) and before
/// `throw_boomerangs` (so a same-tick catch frees up the throw query
/// and the player can re-throw without a one-tick latch). Bevy auto-
/// applies commands between chained systems, so the despawn flushes
/// before throw_boomerangs reads `Query<&Boomerang>`.
///
/// Flying boomerangs are not catchable — only Returning. Otherwise a
/// throw whose initial spawn position overlaps the owner would catch
/// itself on tick 1.
pub fn catch_boomerangs(
    mut commands: Commands,
    players: Query<(&Player, &PositionF)>,
    boomerangs: Query<(Entity, &Boomerang, &PositionF)>,
) {
    for (entity, boom, boom_pos) in &boomerangs {
        if !matches!(boom.state, BoomerangState::Returning) {
            continue;
        }
        let Some((_, owner_pos)) = players.iter().find(|(p, _)| p.handle == boom.owner_handle)
        else {
            continue;
        };
        if player_rect(owner_pos.0).overlaps(boomerang_rect(boom_pos.0)) {
            commands.entity(entity).despawn();
        }
    }
}

/// `GgrsSchedule` system: spawn boomerangs on THROW_DOWN release edges.
/// Runs after `wall_collision` so the spawn position is the post-
/// resolution player position, and after `boomerang_physics` so the
/// freshly-spawned boomerang doesn't take a phantom physics step on
/// its spawn frame.
pub fn throw_boomerangs(
    match_state: Res<MatchState>,
    mut commands: Commands,
    inputs: Res<PlayerInputs<GgrsCfg>>,
    history: Res<InputHistory>,
    players: Query<(&Player, &PositionF), Without<Dead>>,
    boomerangs: Query<&Boomerang>,
) {
    if !match_state.is_in_round() {
        return;
    }
    let throw_speed = Fix::const_from_int(THROW_SPEED_CM_PER_TICK);
    for (player, pos) in &players {
        let has_existing = boomerangs.iter().any(|b| b.owner_handle == player.handle);
        let Some(ring) = history.0.get(&player.handle) else {
            continue;
        };
        let (curr, _) = inputs[player.handle];
        let Some(unit_dir) = try_throw_direction(ring, curr, has_existing) else {
            continue;
        };
        let velocity = unit_dir * throw_speed;
        commands.spawn((
            Boomerang {
                owner_handle: player.handle,
                state: BoomerangState::Flying,
            },
            PositionF(pos.0),
            PreviousPositionF(pos.0),
            VelocityF(velocity),
        ));
    }
}

/// `GgrsSchedule` system: bounce boomerangs off arena walls. Runs
/// after `boomerang_physics` so the position update and OOB despawn
/// happen first; surviving boomerangs that ended up overlapping a
/// wall get pushed out and reflected. Iterates walls in Bevy's
/// deterministic query order, applying push + reflect per wall, so a
/// corner-hit resolves cleanly across two iterations.
///
/// Skips boomerangs in `Returning` state — recall is an uncanny pull
/// that phases through walls. Otherwise the per-tick recall_velocity
/// recompute would override any reflection on the next tick anyway.
pub fn boomerang_wall_collision(
    walls: Query<&Wall>,
    mut boomerangs: Query<(&Boomerang, &mut PositionF, &mut VelocityF)>,
) {
    for (boom, mut pos, mut vel) in &mut boomerangs {
        if matches!(boom.state, BoomerangState::Returning) {
            continue;
        }
        for wall in &walls {
            let bb = boomerang_rect(pos.0);
            if let Some(push) = resolve_collision(bb, wall.rect) {
                pos.0 = pos.0 + push;
                vel.0 = reflect_velocity_for_push(vel.0, push);
            }
        }
    }
}

/// Pure helper: velocity vector that homes a boomerang at `boom_pos`
/// toward `owner_pos` at the requested speed. Returns the zero vector
/// when the boomerang is already at the owner (caller is about to
/// catch it next tick).
pub fn recall_velocity(boom_pos: Vec2F, owner_pos: Vec2F, speed: Fix) -> Vec2F {
    let delta = owner_pos - boom_pos;
    if delta == Vec2F::ZERO {
        return Vec2F::ZERO;
    }
    delta.normalize() * speed
}

/// `GgrsSchedule` system: handle the recall trigger and Returning-state
/// homing. Runs before `boomerang_physics` so any state change or
/// velocity update applies on this tick's physics step.
///
/// Trigger: while a boomerang is in `Flying`, if its owner pressed
/// THROW_DOWN this tick (rising edge against `InputHistory`), the
/// boomerang transitions to `Returning` and gets a velocity toward
/// the owner.
///
/// Steering: in `Returning` state, velocity is recomputed every tick
/// to home toward the owner's current position — this is what lets
/// the boomerang track a player who's still moving during recall.
pub fn recall_boomerangs(
    match_state: Res<MatchState>,
    inputs: Res<PlayerInputs<GgrsCfg>>,
    history: Res<InputHistory>,
    players: Query<(&Player, &PositionF)>,
    mut boomerangs: Query<(&mut Boomerang, &PositionF, &mut VelocityF)>,
) {
    if !match_state.is_in_round() {
        return;
    }
    let recall_speed = Fix::const_from_int(RECALL_SPEED_CM_PER_TICK);
    for (mut boom, boom_pos, mut vel) in &mut boomerangs {
        let Some((_, owner_pos)) = players.iter().find(|(p, _)| p.handle == boom.owner_handle)
        else {
            continue;
        };
        match boom.state {
            BoomerangState::Flying => {
                let Some(ring) = history.0.get(&boom.owner_handle) else {
                    continue;
                };
                let (curr, _) = inputs[boom.owner_handle];
                let prev = previous_input(ring);
                if just_pressed(curr, prev, PlayerInput::THROW_DOWN) {
                    boom.state = BoomerangState::Returning;
                    vel.0 = recall_velocity(boom_pos.0, owner_pos.0, recall_speed);
                }
            }
            BoomerangState::Returning => {
                vel.0 = recall_velocity(boom_pos.0, owner_pos.0, recall_speed);
            }
        }
    }
}

/// `GgrsSchedule` system: advance flying/returning boomerangs by their
/// velocity, despawning any that wander outside `BOOMERANG_DESPAWN_RADIUS_CM`.
/// Cycle 1 has no ricochet — boomerangs fly forever in a straight line
/// until they hit the despawn radius. Cycle 2 adds wall reflection
/// (which keeps them in the arena under normal play); cycle 3+4 add
/// the recall + catch loop.
///
/// Position update uses saturating add so a boomerang that overshoots
/// `Fix::MAX` (~32767 cm) before the despawn check fires can't panic
/// on integer overflow — saturate to MAX, then despawn next pass.
pub fn boomerang_physics(
    mut commands: Commands,
    mut q: Query<(Entity, &mut PositionF, &VelocityF), With<Boomerang>>,
) {
    let max_r = Fix::const_from_int(BOOMERANG_DESPAWN_RADIUS_CM);
    for (entity, mut pos, vel) in &mut q {
        let new_x = pos.0.x.saturating_add(vel.0.x);
        let new_y = pos.0.y.saturating_add(vel.0.y);
        pos.0 = Vec2F::new(new_x, new_y);
        if pos.0.x.abs() > max_r || pos.0.y.abs() > max_r {
            commands.entity(entity).despawn();
        }
    }
}

/// Pure AABB collision resolution. If `player` overlaps `wall`, returns
/// the minimum-translation vector to push the player out along the
/// axis with the smaller overlap. `None` when there is no overlap.
///
/// Axis selection uses 2×centers so we don't pay a fixed-point division.
/// Tie-breaking (when both axes have equal overlap) picks the x axis.
/// Rationale: a thin boomerang flying horizontally into a thick wall
/// can produce equal overlaps on its first contact tick (overlap_x =
/// penetration depth, overlap_y = full boomerang height); reflecting
/// on x is the right answer there. For player vs walls, the smaller-
/// overlap axis is unambiguous (players are square and walls are
/// long), so the tie-break never bites.
pub fn resolve_collision(player: RectF, wall: RectF) -> Option<Vec2F> {
    if !player.overlaps(wall) {
        return None;
    }
    let overlap_x = core::cmp::min(player.max.x, wall.max.x)
        - core::cmp::max(player.min.x, wall.min.x);
    let overlap_y = core::cmp::min(player.max.y, wall.max.y)
        - core::cmp::max(player.min.y, wall.min.y);

    // 2× center comparisons — sign of (player_2cx - wall_2cx) tells us
    // which side of the wall the player center sits on.
    let player_2cx = player.min.x + player.max.x;
    let wall_2cx = wall.min.x + wall.max.x;
    let player_2cy = player.min.y + player.max.y;
    let wall_2cy = wall.min.y + wall.max.y;

    if overlap_x <= overlap_y {
        let push = if player_2cx < wall_2cx {
            -overlap_x
        } else {
            overlap_x
        };
        Some(Vec2F::new(push, Fix::ZERO))
    } else {
        let push = if player_2cy < wall_2cy {
            -overlap_y
        } else {
            overlap_y
        };
        Some(Vec2F::new(Fix::ZERO, push))
    }
}

/// Pure helper: reflect `vel` across the axis indicated by `push`.
/// `push` comes out of `resolve_collision` and is purely along one
/// axis (either x is zero or y is zero), so we just flip the
/// matching component of velocity. Zero push (no collision) returns
/// `vel` unchanged.
///
/// No damping by design — boomerangs ricochet at full energy. The
/// "feel awesome" loop is sharp clean reflection, not a mushy
/// energy-bleeding bounce. If players want the boomerang to slow
/// down, they recall it.
pub fn reflect_velocity_for_push(vel: Vec2F, push: Vec2F) -> Vec2F {
    if push.x != Fix::ZERO {
        Vec2F::new(-vel.x, vel.y)
    } else if push.y != Fix::ZERO {
        Vec2F::new(vel.x, -vel.y)
    } else {
        vel
    }
}

/// Resolve player-vs-walls each tick. Iterates walls; for each collision
/// applies the minimum-translation push to `PositionF`. Subsequent walls
/// see the updated player position so a corner overlap (player wedged
/// into a corner) resolves cleanly across two iterations rather than
/// over-correcting. Order-stability comes from Bevy's deterministic
/// query iteration over the wall entities (spawned in fixed order in
/// `app::setup`).
pub fn wall_collision(
    walls: Query<&Wall>,
    mut players: Query<&mut PositionF, With<Player>>,
) {
    for mut pos in &mut players {
        for wall in &walls {
            let player = player_rect(pos.0);
            if let Some(push) = resolve_collision(player, wall.rect) {
                pos.0 = pos.0 + push;
            }
        }
    }
}

pub fn advance_frame_count(mut frame: ResMut<FrameCount>) {
    frame.0 = frame.0.wrapping_add(1);
}

/// First system in `GgrsSchedule`: copy each entity's `PositionF` into
/// `PreviousPositionF` so subsequent systems' updates to `PositionF`
/// leave the snapshot intact for the render-side interpolator.
pub fn snapshot_previous(mut q: Query<(&PositionF, &mut PreviousPositionF)>) {
    for (pos, mut prev) in &mut q {
        prev.0 = pos.0;
    }
}

/// Teleport helper: collapses `prev` and `pos` to the same target so the
/// render-side lerp emits no motion. Use whenever the new sim position
/// isn't continuous with the old one (respawns, stage transitions, etc).
pub fn snap_position(pos: &mut PositionF, prev: &mut PreviousPositionF, new: Vec2F) {
    pos.0 = new;
    prev.0 = new;
}

/// Per-handle respawn point. Symmetric on the x axis so both players
/// re-enter the round on equal footing rather than spawning on top of
/// where they last died. Phase 16 will swap this for arena-specific
/// respawn slots.
pub fn respawn_position(handle: usize) -> Vec2F {
    match handle {
        0 => Vec2F::from_cm(-100, 0),
        _ => Vec2F::from_cm(100, 0),
    }
}

/// `GgrsSchedule` system: revive any player whose `Dead.respawn_at_frame`
/// has elapsed. Runs after `snapshot_previous` so we can use
/// `snap_position` to collapse `PositionF`/`PreviousPositionF` to the
/// new respawn point — the render-side interpolator then emits no
/// motion for the teleport tick (no streak across the arena from
/// corpse to spawn). Resets the player's `DashState`, `StunFrames`,
/// and `VelocityF` so they revive in a clean Idle baseline rather
/// than mid-dash or with stale velocity.
type DeadPlayerQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Player,
        &'static Dead,
        &'static mut PositionF,
        &'static mut PreviousPositionF,
        &'static mut VelocityF,
        &'static mut DashState,
        &'static mut StunFrames,
    ),
>;

pub fn tick_respawn(mut commands: Commands, frame: Res<FrameCount>, mut q: DeadPlayerQuery) {
    for (entity, player, dead, mut pos, mut prev, mut vel, mut dash, mut stun) in &mut q {
        if frame.0 < dead.respawn_at_frame {
            continue;
        }
        snap_position(&mut pos, &mut prev, respawn_position(player.handle));
        vel.0 = Vec2F::ZERO;
        *dash = DashState::default();
        *stun = StunFrames(0);
        commands.entity(entity).remove::<Dead>();
    }
}

/// `GgrsSchedule` system: drive the round/match state machine forward
/// based on `FrameCount` and `MatchScore`. Runs early in the tick (after
/// `tick_respawn`, before the input-gated gameplay systems) so a
/// transition to `InRound` enables movement on the same tick the
/// countdown finishes — "GO!" feel, no one-tick latch.
///
/// Transition rules:
///   - `Countdown { digit, .. }` at expiry: decrement digit, or flip
///     to `InRound` when digit was 1.
///   - `InRound` at expiry **OR** when either player crosses
///     `MATCH_WIN_THRESHOLD`: flip to `RoundOver` (timer-out) or
///     `MatchOver` (threshold reached).
///   - `RoundOver` at expiry: flip back to top of `Countdown` for the
///     next round (unless threshold was reached during the beat,
///     in which case `MatchOver`).
///   - `MatchOver`: terminal.
pub fn tick_match_state(
    frame: Res<FrameCount>,
    score: Res<MatchScore>,
    mut state: ResMut<MatchState>,
) {
    let match_won = score.p0 >= MATCH_WIN_THRESHOLD || score.p1 >= MATCH_WIN_THRESHOLD;
    *state = match *state {
        MatchState::Countdown {
            digit,
            expires_at_frame,
        } => {
            if frame.0 >= expires_at_frame {
                if digit > 1 {
                    MatchState::Countdown {
                        digit: digit - 1,
                        expires_at_frame: frame.0 + COUNTDOWN_DIGIT_FRAMES,
                    }
                } else {
                    MatchState::InRound {
                        expires_at_frame: frame.0 + ROUND_DURATION_FRAMES,
                    }
                }
            } else {
                *state
            }
        }
        MatchState::InRound { expires_at_frame } => {
            if match_won {
                MatchState::MatchOver
            } else if frame.0 >= expires_at_frame {
                MatchState::RoundOver {
                    expires_at_frame: frame.0 + ROUND_OVER_FRAMES,
                }
            } else {
                *state
            }
        }
        MatchState::RoundOver { expires_at_frame } => {
            if match_won {
                MatchState::MatchOver
            } else if frame.0 >= expires_at_frame {
                MatchState::Countdown {
                    digit: 3,
                    expires_at_frame: frame.0 + COUNTDOWN_DIGIT_FRAMES,
                }
            } else {
                *state
            }
        }
        MatchState::MatchOver => MatchState::MatchOver,
    };
}

pub fn record_last_tick_time(time: Res<Time<Real>>, mut last: ResMut<LastSimTickTime>) {
    last.0 = time.elapsed_secs_f64();
}

/// Last system in `GgrsSchedule`: pushes the current tick's inputs
/// onto each player's history ring. Must run AFTER all edge consumers
/// so they see history's last entry as "previous tick". Iterates
/// `Player` components rather than `LocalPlayers` so the ring is
/// populated for both local and remote players (relevant once
/// networking lands in Phase 11).
pub fn advance_input_history(
    inputs: Res<PlayerInputs<GgrsCfg>>,
    mut history: ResMut<InputHistory>,
    players: Query<&Player>,
) {
    for player in &players {
        let (current_input, _status) = inputs[player.handle];
        let entry = history
            .0
            .entry(player.handle)
            .or_insert([PlayerInput::default(); INPUT_HISTORY_LEN]);
        push_history(entry, current_input);
    }
}

// ---- Plugin ----

/// Adds the sim's rollback registrations, schedules, and gameplay systems.
/// **Does NOT** install an input source — pair with one of:
/// - [`DefaultInputsPlugin`] for synthesized inputs (sync_test, dev)
/// - `replay::ReplayPlaybackPlugin` for replay-driven playback
///
/// The `GgrsPlugin::<GgrsCfg>` itself must be added separately by the
/// caller before this plugin runs (so the `AdvanceWorld` schedule exists
/// when we register systems into it).
pub struct SimPlugin;

impl Plugin for SimPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FrameCount>()
            .init_resource::<LastSimTickTime>()
            .init_resource::<MatchScore>()
            .init_resource::<MatchState>()
            .init_resource::<InputHistory>()
            .insert_resource(RollbackFrameRate(TICK_HZ));

        // Rollback registrations
        app.rollback_component_with_copy::<PositionF>()
            .rollback_component_with_copy::<PreviousPositionF>()
            .rollback_component_with_copy::<VelocityF>()
            .rollback_component_with_copy::<Player>()
            .rollback_component_with_copy::<NoInterpolate>()
            .rollback_component_with_copy::<DashState>()
            .rollback_component_with_copy::<StunFrames>()
            .rollback_component_with_copy::<Dead>()
            .rollback_component_with_copy::<Boomerang>()
            .rollback_component_with_copy::<BoomerangState>()
            .rollback_resource_with_copy::<FrameCount>()
            .rollback_resource_with_copy::<MatchScore>()
            .rollback_resource_with_copy::<MatchState>()
            .rollback_resource_with_clone::<InputHistory>();

        // Checksums — required for SyncTest to detect divergence beyond
        // entity-count mismatches. PreviousPositionF participates because
        // a desync in the snapshot value would surface as a stuttering
        // visual even when the live position recovers.
        app.checksum_component_with_hash::<PositionF>()
            .checksum_component_with_hash::<PreviousPositionF>()
            .checksum_component_with_hash::<VelocityF>()
            .checksum_component_with_hash::<DashState>()
            .checksum_component_with_hash::<StunFrames>()
            .checksum_component_with_hash::<Dead>()
            .checksum_component_with_hash::<Boomerang>()
            .checksum_resource_with_hash::<FrameCount>()
            .checksum_resource_with_hash::<MatchScore>()
            .checksum_resource_with_hash::<MatchState>()
            .checksum_resource_with_hash::<InputHistory>();

        // Sim systems — explicitly ordered per CONVENTIONS.md.
        // snapshot_previous runs FIRST so the PositionF copy it captures
        // is the value at the start of this tick (== end of prior tick).
        // wall_collision runs immediately after player_movement so the
        // PositionF coming out of this tick is the post-resolution
        // position the render layer sees. advance_input_history runs
        // LAST so edge consumers see the ring's last entry as "previous
        // tick" until end-of-tick rolls it forward.
        app.add_systems(
            GgrsSchedule,
            (
                snapshot_previous,
                tick_respawn,
                tick_match_state,
                start_dash,
                player_movement,
                wall_collision,
                recall_boomerangs,
                boomerang_physics,
                boomerang_wall_collision,
                hit_boomerang_player,
                catch_boomerangs,
                throw_boomerangs,
                tick_player_timers,
                advance_frame_count,
                advance_input_history,
            )
                .chain(),
        );

        // Wall-clock timestamp captured after each tick.
        app.add_systems(
            AdvanceWorld,
            record_last_tick_time.in_set(AdvanceWorldSystems::Last),
        );

        // Hard panic on SyncTest divergence — the whole point of the harness.
        app.add_observer(|trigger: On<SyncTestMismatch>| {
            let event = trigger.event();
            panic!(
                "SyncTest desync at frame {}: mismatched frames {:?}",
                event.current_frame, event.mismatched_frames
            );
        });
    }
}

/// Default input source: writes `LocalInputs<GgrsCfg>` from the
/// `SynthesizedInputs` resource each tick. Caller mutates
/// `SynthesizedInputs` between `app.update()` calls.
pub struct DefaultInputsPlugin;

impl Plugin for DefaultInputsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SynthesizedInputs>()
            .add_systems(ReadInputs, read_local_inputs);
    }
}

/// Replaces the default `MatchState::Countdown` with
/// `MatchState::infinite_round()` so headless ceremonies (sync_test,
/// replay_sync, unit tests) can exercise gameplay systems without
/// burning 180 ticks waiting for the 3-2-1 countdown to finish. The
/// real game binary (`crates/app`) must NOT add this plugin — its
/// players need the countdown.
pub struct InfiniteRoundPlugin;

impl Plugin for InfiniteRoundPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(MatchState::infinite_round());
    }
}
