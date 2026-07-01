use bevy::prelude::*;
use bevy_ggrs::prelude::*;
use bevy_ggrs::{
    AdvanceWorld, AdvanceWorldSystems, GgrsConfig, LocalInputs, LocalPlayers, PlayerInputs,
    RollbackApp, SyncTestMismatch,
};
use bytemuck::{Pod, Zeroable};
use fixed_math::{Fix, RectF, Vec2F};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

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

/// Neutral peer-address type for the ggrs `Config::Address` slot.
///
/// `ggrs` requires `Address: Clone + PartialEq + Eq + Hash + Send + Sync +
/// Debug`. The transport (Matchbox) identifies peers with a `PeerId`
/// (a `Uuid` newtype), but `sim` must stay free of any networking crate
/// (CONVENTIONS: the determinism core is headless). `NetAddr` is the
/// neutral handle the bridge maps to/from a `PeerId` via a trivial u128
/// bijection (`PeerId(Uuid::from_u128(n))` ↔ `peer.0.as_u128()`), so the
/// address only ever appears at the ggrs boundary and never in sim logic.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct NetAddr(pub u128);

pub type GgrsCfg = GgrsConfig<PlayerInput, NetAddr>;

pub const TICK_HZ: usize = 60;
pub const TICK_DT: Fix = Fix::lit("0.01666666666");

/// Strict-match version stamped on `.bmrg` replays. `1` was the
/// `v1.0.0-rc1` release (Milestone 6); pre-release `main` carried the
/// `u32::MAX` dev sentinel. Bumped to `2` for the boomerang-feel pass:
/// the wall-ricochet tie-break/swept fix, the throw-distance auto-recall
/// cap, two-thumb aim (throw direction + power from the aim), and the
/// aim-gated animation read all change sim semantics. Any future
/// sim-affecting change must bump this so old replays are routed back to
/// their tagged binary rather than silently loaded into a binary with
/// different sim semantics — and the committed canonical demo (which
/// stamps this value, see `replay_sync::canonical_replay`) must be
/// regenerated (`gen_canonical --write`). See
/// `replay::decode_for_sim_version` for the gate.
pub const SIM_VERSION: u32 = 5;

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
#[require(
    Rollback,
    DashState,
    StunFrames,
    Dead,
    AnimState,
    Empowered,
    HeldModifier,
    OobTimer,
    ThrowCapacity,
    ThrowCharge
)]
pub struct Player {
    pub handle: usize,
}

/// Phase 17: set true by a *perfect catch* (catching a returning boomerang
/// within `PERFECT_CATCH_WINDOW_FRAMES` of recall). The player's next throw
/// consumes the flag and launches faster + deadlier — the signature skill-
/// expression reward. Rolled back so the empowered state survives
/// resimulation.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[require(Rollback)]
pub struct Empowered(pub bool);

/// Catch window (frames after recall begins) that counts as a *perfect
/// catch*. ~167 ms at 60 Hz — tight enough to demand a read, forgiving
/// enough to land with practice.
pub const PERFECT_CATCH_WINDOW_FRAMES: u32 = 10;

/// Full-charge throw speed of an empowered (perfect-catch) throw. 31 vs the
/// base 24 (~1.3×): a clearly faster, harder-to-react-to fang. Charge scales
/// this down for a partial draw just like the base (see [`aimed_throw_speed`]).
pub const EMPOWERED_THROW_SPEED_CM_PER_TICK: i32 = 31;

/// Throw speed (cm/tick) for a throw, empowered or not. The single place
/// the perfect-catch speed bonus is applied.
pub fn throw_speed_for(empowered: bool) -> Fix {
    Fix::const_from_int(if empowered {
        EMPOWERED_THROW_SPEED_CM_PER_TICK
    } else {
        THROW_SPEED_CM_PER_TICK
    })
}

/// Throw speed (cm/tick) accounting for empowerment AND a pickup modifier:
/// Fire launches faster, Heavy slower, everything else at the base/empowered
/// speed. The single source of truth for launch speed.
pub fn modified_throw_speed(empowered: bool, modifier: Option<PickupKind>) -> Fix {
    let base: i32 = if empowered {
        EMPOWERED_THROW_SPEED_CM_PER_TICK
    } else {
        THROW_SPEED_CM_PER_TICK
    };
    let cm = match modifier {
        // +8 (halved with the boomerang-speed cut, 2026-06-30) keeps Fire ~1.3×
        // the base; Heavy's ×4/5 is relative and auto-scales.
        Some(PickupKind::Fire) => base + 8,
        Some(PickupKind::Heavy) => base * 4 / 5,
        _ => base,
    };
    Fix::const_from_int(cm)
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

/// Phase 11 — player liveness state. `respawn_at_frame.is_some()`
/// means the player is dying; `None` means alive. Stored as a
/// VALUE component (always present on every Player) rather than
/// add/remove on death so kills don't trigger Bevy archetype
/// transitions on the player entity. Archetype transitions during
/// rollback resimulation interact subtly with bevy_ggrs's snapshot
/// restoration in ways that surface as fuzzed-input desyncs (the
/// nightly Fuzz Soak caught this on Phase 11 seeds 29 and 61).
/// Keeping `Dead` always-present collapses the kill into a value
/// update on the existing component — no archetype mutation, no
/// resimulation drift.
///
/// While `respawn_at_frame.is_some()`, the player-input systems
/// (`player_movement`, `start_dash`, `throw_boomerangs`) and hit
/// detection (`hit_boomerang_player`) skip the entity by reading
/// `Dead` and checking the value. In-flight boomerangs owned by a
/// dead player continue to fly, ricochet, recall, and catch
/// normally — the corpse's position is still tracked as the recall
/// target until `tick_respawn` snaps it to the spawn point.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[require(Rollback)]
pub struct Dead {
    pub respawn_at_frame: Option<u32>,
}

impl Dead {
    /// True iff the player is currently dying (respawn pending).
    pub fn is_dying(&self) -> bool {
        self.respawn_at_frame.is_some()
    }
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
    /// Homing back to the owner. `since` is the frame the recall began —
    /// `catch_boomerangs` compares it to the catch frame for the
    /// perfect-catch window (the data only exists while it's meaningful).
    Returning { since: u32 },
    /// Knocked LOOSE by inner cover: the fang ricocheted off an Obstacle and is
    /// now decelerating to rest on the ground (Boomerang-Fu drop). It PERSISTS
    /// there until claimed (no lifetime timer) — the owner can hold-recall it
    /// (→ Returning) or walk over to re-arm, and an OPPONENT who walks over it
    /// (while it isn't being recalled) STEALS it as an extra boomerang
    /// (+1 `ThrowCapacity`). Cleared on round reset. Still ricochets off cover.
    Loose,
}

#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[require(Rollback, BoomerangMods)]
pub struct Boomerang {
    pub owner_handle: usize,
    pub state: BoomerangState,
}

/// Per-boomerang pickup modifier + multishot role. Carried as a separate
/// `#[require]`d component (default = primary, unmodified) so every boomerang
/// — including the dozens spawned bare in tests — auto-gets a sane value
/// without touching every `Boomerang { .. }` literal. `throw_boomerangs`
/// overrides it when the thrower holds a pickup.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[require(Rollback)]
pub struct BoomerangMods {
    /// Active pickup modifier, if any (Phase 17 cycle 3 gives each behavior).
    pub modifier: Option<PickupKind>,
    /// Multishot side-fang: despawns on first wall contact and is ignored by
    /// recall/catch. Default false = the primary fang.
    pub is_secondary: bool,
    /// Frame at which a Multishot side-fang self-despawns if it hasn't already
    /// hit a wall (a backstop for fangs that fly through a gap). `None` for
    /// primaries and non-multishot fangs — they live until recalled/caught.
    pub despawn_at_frame: Option<u32>,
}

/// Throw origin of a recallable primary boomerang — the thrower's
/// position at launch. A primary auto-recalls once it has travelled
/// [`BOOMERANG_MAX_THROW_DISTANCE_CM`] straight-line from here, so a
/// throw can't sail more than ~2/3 of the arena's long axis before
/// turning back. Only primaries carry it (Multishot side-fangs are
/// throw-and-forget); bare-spawned test boomerangs omit it and are
/// simply uncapped. Straight-line-from-origin is the right "reach"
/// metric: it bounds how far the fang can get from the thrower
/// regardless of speed or ricochets.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[require(Rollback)]
pub struct ThrowOrigin(pub Vec2F);

/// Per-throw reach (straight-line distance from [`ThrowOrigin`]) at which a
/// primary auto-recalls. Scales with the throw's CHARGE — a soft tap barely
/// reaches, a full charge threatens most of the board (see [`charged_reach`]).
/// Only charged primaries carry it; bare-spawned test fangs omit it and fall
/// back to the uncapped default.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[require(Rollback)]
pub struct ThrowReach(pub Fix);

/// How long (frames) the owner has held THROW without a fang out — the throw
/// CHARGE. Longer hold → faster, farther, more vicious fang ([`charge_power`]).
/// Rolled back + checksummed; accumulated and consumed in `throw_boomerangs`.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[require(Rollback)]
pub struct ThrowCharge(pub u32);

/// How many boomerangs a player may have live at once. Starts at 1; stealing an
/// opponent's loose fang (walking over it) grants +1 — you now duel with a
/// "second boomerang" (the Boomerang-Fu pickup). Reset to 1 on respawn. Rolled
/// back + checksummed.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[require(Rollback)]
pub struct ThrowCapacity(pub u32);

impl Default for ThrowCapacity {
    fn default() -> Self {
        Self(1)
    }
}

/// Frames of THROW-hold to reach full charge (max speed + reach). 45 ≈ 0.75 s:
/// chargeable under pressure without feeling sluggish. A release before this
/// throws proportionally weaker (never a dud — even an instant tap lobs).
pub const CHARGE_MAX_FRAMES: u32 = 45;

/// Per-tick velocity retention for a loose (tumbling-to-rest) fang. 0.86 bleeds
/// ~half the speed every ~4-5 ticks, so a bounced fang slows and settles in
/// well under a second instead of pinballing at constant velocity.
pub const LOOSE_DRAG: Fix = Fix::lit("0.86");

/// At or below this speed (cm/tick) a loose fang snaps to rest (velocity zero),
/// so it actually stops instead of asymptotically creeping.
pub const LOOSE_REST_SPEED_CM: i32 = 4;

/// Maximum straight-line distance (cm) a primary boomerang travels from
/// its throw origin before it auto-recalls. ~2/3 of the arena's 1500 cm
/// long axis: a full-power throw threatens most of the board but always
/// turns back, so the opponent keeps a "safe distance" to play around —
/// the cat-and-mouse spacing that makes a 1v1 boomerang duel tick.
/// Tunable; 750 (= 1/2 longways) is the tighter alternative.
pub const BOOMERANG_MAX_THROW_DISTANCE_CM: i32 = 1000;

/// Full-charge throw speed in cm/tick — the fastest a fang launches (at max
/// [`ThrowCharge`]). Lowered to 24 (2026-06-30 charge pass) so the whole game
/// runs slower/floatier and the DASH gains value by contrast. A partial charge
/// scales this down toward `THROW_SPEED × MIN_THROW_POWER_FRAC` (see
/// [`aimed_throw_speed`]). 24 × 60 = 1440 cm/sec at full draw.
pub const THROW_SPEED_CM_PER_TICK: i32 = 24;

/// Floor of the charge→speed ramp: a zero-charge (instant tap) throw launches
/// at `base × MIN_THROW_POWER_FRAC`, a full charge at `base × 1.0`. 0.35 makes a
/// tap a weak lob and a full charge ~3× as fast — the charge is worth holding.
/// Direction comes from the aim/stick; charge scales only magnitude.
pub const MIN_THROW_POWER_FRAC: Fix = Fix::lit("0.35");

/// Charge-scaled launch speed (cm/tick) for a unit direction: scales `base`
/// (the full-charge speed, incl. empowered/modifier) by `power` (clamped [0,1])
/// across `[MIN_THROW_POWER_FRAC, 1.0]`. Pure so it's matrix-deterministic.
/// Historically named for the two-thumb aim drag; `power` is now the throw
/// CHARGE ([`charge_power`]).
pub fn aimed_throw_speed(base: Fix, power: Fix) -> Fix {
    let p = power.clamp(Fix::ZERO, Fix::const_from_int(1));
    let frac = MIN_THROW_POWER_FRAC + (Fix::const_from_int(1) - MIN_THROW_POWER_FRAC) * p;
    base * frac
}

/// Charge fraction [0,1] from held frames: `frames / CHARGE_MAX_FRAMES`, capped.
pub fn charge_power(frames: u32) -> Fix {
    Fix::const_from_int(frames.min(CHARGE_MAX_FRAMES) as i32)
        / Fix::const_from_int(CHARGE_MAX_FRAMES as i32)
}

/// Charge→reach endpoints (cm). Min: a tap lands close. Max: ~2/3 of the
/// 1500 cm long axis, so even a full charge always turns back — the cat-and-
/// mouse spacing that makes a 1v1 boomerang duel tick.
pub const REACH_MIN_CM: i32 = 300;
pub const REACH_MAX_CM: i32 = 1100;

/// Per-throw auto-recall reach (cm) from the charge: interpolates
/// `[REACH_MIN_CM, REACH_MAX_CM]` by `power`. Straight-line from the throw
/// origin (see [`ThrowReach`]). Pure for matrix determinism.
pub fn charged_reach(power: Fix) -> Fix {
    let p = power.clamp(Fix::ZERO, Fix::const_from_int(1));
    let min = Fix::const_from_int(REACH_MIN_CM);
    let max = Fix::const_from_int(REACH_MAX_CM);
    min + (max - min) * p
}

/// Boomerang collision half-extent in cm. Smaller than the player's
/// 16 cm: ~10 cm gives a 20 cm catch/hit footprint that reads as a
/// chunky thrown weapon without making it cheese-easy to hit with.
pub const BOOMERANG_HALF_EXTENT_CM: i32 = 10;

/// Recall speed in cm/tick. A touch faster than `THROW_SPEED` so the
/// boomerang catches up to a player who's moved forward since the
/// throw — recall reads as "reeling in" rather than "drifting back".
/// A touch above the full-charge throw speed (26 vs 24) so recall still reels in.
pub const RECALL_SPEED_CM_PER_TICK: i32 = 26;

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
    cannot_throw: bool,
) -> Option<Vec2F> {
    if cannot_throw {
        return None;
    }
    // Fire ONLY on the exact release tick. The charge model makes the release
    // deliberate (you hold to charge, then let go), so the old forgiveness-
    // window scan is dropped — with `ThrowCapacity > 1` it would re-fire the
    // same release across the window and throw several fangs from one press.
    if !just_released(
        current_input,
        previous_input(history_ring),
        PlayerInput::THROW_DOWN,
    ) {
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
    /// The outer arena ring. Blocks PLAYERS (containment) but is PERMEABLE to
    /// boomerangs — a thrown fang flies out over the edge and auto-returns to
    /// the thrower (the Boomerang-Fu open-field model) instead of pinballing off
    /// a hard border forever. (Out-of-bounds death for players is a later pass.)
    Boundary,
    /// Inner-layout cover (paintball-style crates/pads/hedges). Blocks players
    /// AND ricochets boomerangs — and a fang that clips one then loses momentum
    /// and settles to rest on the ground (see [`BoomerangState::Loose`]).
    Obstacle,
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

/// Frames a player must be continuously OUT OF BOUNDS (past the floor edge)
/// before the void claims them — ~3 s at 60 Hz (the Boomerang-Fu "outside the
/// map bites you" grace). Resets the instant they're back inside.
pub const OOB_GRACE_FRAMES: u32 = 180;

/// Per-player out-of-bounds frame counter (rolled back). 0 while inside the
/// play area; at [`OOB_GRACE_FRAMES`] the player dies and the opponent scores.
/// `#[require]`d by `Player` so every duelist carries it without touching spawns.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[require(Rollback)]
pub struct OobTimer(pub u32);

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
            kind: WallKind::Boundary,
            rect: RectF::from_min_max(
                Vec2F::from_cm(-inner_x, inner_y),
                Vec2F::from_cm(inner_x, inner_y + t),
            ),
        },
        // South (bottom): full inner width, thickness below the arena.
        Wall {
            kind: WallKind::Boundary,
            rect: RectF::from_min_max(
                Vec2F::from_cm(-inner_x, -inner_y - t),
                Vec2F::from_cm(inner_x, -inner_y),
            ),
        },
        // West (left): full corner-to-corner height (covers top-left
        // and bottom-left corners), thickness to the left.
        Wall {
            kind: WallKind::Boundary,
            rect: RectF::from_min_max(
                Vec2F::from_cm(-inner_x - t, -inner_y - t),
                Vec2F::from_cm(-inner_x, inner_y + t),
            ),
        },
        // East (right): mirror of west.
        Wall {
            kind: WallKind::Boundary,
            rect: RectF::from_min_max(
                Vec2F::from_cm(inner_x, -inner_y - t),
                Vec2F::from_cm(inner_x + t, inner_y + t),
            ),
        },
    ]
}

/// Inner `Obstacle` cover per arena — paintball-style crates/pillars that block
/// players AND ricochet boomerangs (a fang that clips one is knocked Loose).
/// Symmetric about both axes, clear of the player spawns (±100, 0/±60) and the
/// per-arena props. Spawned in a fixed order so rollback entity ids stay
/// bit-identical across hosts (same discipline as `arena_walls`).
pub fn arena_obstacles_for(arena: ArenaId) -> [Wall; 4] {
    let block = |cx: i32, cy: i32, hw: i32, hh: i32| Wall {
        kind: WallKind::Obstacle,
        rect: RectF::from_center_half_extents(Vec2F::from_cm(cx, cy), Vec2F::from_cm(hw, hh)),
    };
    match arena {
        // Anchor: four crates boxing the central pyre — cover to juke around.
        ArenaId::Anchor => [
            block(-280, 300, 38, 38),
            block(280, 300, 38, 38),
            block(-280, -300, 38, 38),
            block(280, -300, 38, 38),
        ],
        // Crossing: tall pillars flanking the central chasm.
        ArenaId::Crossing => [
            block(-300, 210, 30, 72),
            block(300, 210, 30, 72),
            block(-300, -210, 30, 72),
            block(300, -210, 30, 72),
        ],
        // Reliquary: bars guarding the chain pyres + sigil doors.
        ArenaId::Reliquary => [
            block(0, 330, 64, 30),
            block(0, -330, 64, 30),
            block(-330, 0, 30, 64),
            block(330, 0, 30, 64),
        ],
    }
}

/// Marker: render-side `sync_transforms_from_sim` skips interpolation for
/// entities carrying this component and uses `PositionF` directly. Useful
/// for entities whose sim-side position changes shouldn't smear on screen
/// (UI overlays, fixed-position decals, debug indicators). Rolled back so
/// the marker's presence/absence is consistent during resimulation.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[require(Rollback)]
pub struct NoInterpolate;

/// Per-entity animation state (v2). `anim_id` selects which
/// animation is playing (Idle/Run/Throw/Dash/Hit/Catch/Death);
/// `ticks` counts sim ticks since the animation started. Render
/// layer divides `ticks` by the per-animation frame-divider to get
/// the display frame (animation runs slower than sim).
///
/// Rolled back so animation state is consistent across resimulation
/// (a respawn-during-rollback would otherwise drift the animation
/// vs the post-rollback authoritative state). Per CONVENTIONS:
/// "Animation does not interpolate. Pixel art frames snap." The
/// render layer reads `display_index()` and picks exactly one
/// atlas index per render frame; no fractional blending.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[require(Rollback)]
pub struct AnimState {
    /// Selector into the per-character sprite sheet. See
    /// [`AnimState::IDLE`]/[`THROW`]/etc. constants for the encoding.
    pub anim_id: u8,
    /// Sim ticks since this animation started. Reset to 0 on
    /// transition. Range checked at u16::MAX would mean the player
    /// idled for 18+ minutes, which is fine — even if it overflows,
    /// `display_index` modulo's it down for looping anims and caps
    /// it for one-shots.
    pub ticks: u16,
}

impl AnimState {
    /// Looping idle bob.
    pub const IDLE: u8 = 0;
    /// Looping run cycle (stick magnitude > DASH_MIN_STICK_MAG).
    pub const RUN: u8 = 1;
    /// One-shot throw: wind-up → cock → release → fly-out → recovery → settle.
    pub const THROW: u8 = 2;
    /// Looping dash blur (only while `DashState::Dashing`).
    pub const DASH: u8 = 3;
    /// One-shot hit reaction (white-flash silhouette → return).
    pub const HIT: u8 = 4;
    /// One-shot catch: arm snap up, spark flash at the hand.
    pub const CATCH: u8 = 5;
    /// One-shot death: stagger → bow → buckle → disperse → corpse mark.
    pub const DEATH: u8 = 6;
    /// Looping throw CHARGE: crouched wind-up that tightens as the throw builds
    /// (held while `ThrowCharge > 0`). Reads as coiled potential energy.
    pub const CHARGE: u8 = 7;

    /// Per ART_DIRECTION.md v2 animation table. 41-frame atlas strip
    /// (32×32 source cells). `frame_count` returns the source-frame
    /// span; `atlas_offset` returns the strip index where this anim
    /// starts (so render can compute final atlas index = offset +
    /// display_frame).
    pub const fn frame_count(anim_id: u8) -> u16 {
        match anim_id {
            Self::IDLE => 6,
            Self::RUN => 6,
            Self::THROW => 8,
            Self::DASH => 4,
            Self::HIT => 4,
            Self::CATCH => 3,
            Self::DEATH => 10,
            Self::CHARGE => 4,
            _ => 6,
        }
    }

    pub const fn atlas_offset(anim_id: u8) -> u16 {
        match anim_id {
            Self::IDLE => 0,
            Self::RUN => 6,
            Self::THROW => 12,
            Self::DASH => 20,
            Self::HIT => 24,
            Self::CATCH => 28,
            Self::DEATH => 31,
            Self::CHARGE => 41,
            _ => 0,
        }
    }

    /// Sim-ticks-per-displayed-frame. Tuned to read at the right
    /// emotional cadence: idle is gentle (~6.7 fps), run is brisk
    /// (12 fps), throw/hit/catch are snappy (20 fps), dash flickers
    /// (20 fps), death has weight (7.5 fps). All integers so the
    /// frame-stepping is deterministic.
    pub const fn ticks_per_frame(anim_id: u8) -> u16 {
        match anim_id {
            Self::IDLE => 9,
            Self::RUN => 5,
            Self::THROW => 3,
            Self::DASH => 3,
            Self::HIT => 3,
            Self::CATCH => 3,
            Self::DEATH => 8,
            Self::CHARGE => 6,
            _ => 9,
        }
    }

    /// One-shot anims cap at the last frame instead of looping. The
    /// state-machine in [`advance_animation`] returns to IDLE/RUN
    /// when a one-shot finishes.
    pub const fn is_oneshot(anim_id: u8) -> bool {
        matches!(anim_id, Self::THROW | Self::HIT | Self::CATCH | Self::DEATH)
    }

    /// Total frames in the player atlas strip (IDLE..DEATH = 41, + CHARGE = 4).
    pub const TOTAL_ATLAS_FRAMES: u16 = 45;

    /// Atlas index into the 41-frame player sheet for the current
    /// tick. Render layer reads this directly to pick a TextureAtlas
    /// rect.
    pub fn display_index(&self) -> u16 {
        let divider = Self::ticks_per_frame(self.anim_id).max(1);
        let count = Self::frame_count(self.anim_id);
        let raw = self.ticks / divider;
        let local = if Self::is_oneshot(self.anim_id) {
            raw.min(count.saturating_sub(1))
        } else {
            raw % count
        };
        Self::atlas_offset(self.anim_id) + local
    }

    /// True iff this anim has played out — used by the state-machine
    /// to decide when to return a one-shot to IDLE.
    pub fn is_finished(&self) -> bool {
        if !Self::is_oneshot(self.anim_id) {
            return false;
        }
        let total_ticks =
            Self::frame_count(self.anim_id) as u32 * Self::ticks_per_frame(self.anim_id) as u32;
        self.ticks as u32 >= total_ticks
    }
}

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
/// (2 * ARENA_HALF_HEIGHT_CM = 1500 cm) crosses in about 2 seconds at
/// 60 Hz: 1500 cm / (13 cm/tick * 60 tick/s) ~= 1.92 s.
///
/// Walk speed in cm/tick. Brought down to 8 (2026-06-30 charge pass) so the
/// DASH (30 cm/tick, unchanged) reads as a big, valuable burst by contrast —
/// ~3.75× walk. Slower, more deliberate spacing pairs with the charged fang.
pub const WALK_SPEED_CM_PER_TICK: i32 = 8;

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
    mut q: Query<(&Player, &Dead, &mut DashState, &mut StunFrames)>,
) {
    if !match_state.is_in_round() {
        return;
    }
    for (player, dead, mut dash, mut stun) in &mut q {
        if dead.is_dying() {
            continue;
        }
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
    mut q: Query<(&Player, &Dead, &mut PositionF, &mut VelocityF, &DashState)>,
) {
    if !match_state.is_in_round() {
        return;
    }
    let walk_speed = Fix::const_from_int(WALK_SPEED_CM_PER_TICK);
    let dash_speed = Fix::const_from_int(DASH_SPEED_CM_PER_TICK);
    for (player, dead, mut pos, mut vel, dash) in &mut q {
        if dead.is_dying() {
            continue;
        }
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
    mut players: Query<(Entity, &Player, &PositionF, &mut Dead, &StunFrames)>,
) {
    for (boom_entity, boom, boom_pos) in &boomerangs {
        // A Loose (dropped) fang is a harmless pickup — you walk over it to
        // steal/reclaim it (catch_boomerangs), it does NOT kill. Only Flying
        // and Returning fangs are lethal.
        if matches!(boom.state, BoomerangState::Loose) {
            continue;
        }
        let bb = boomerang_rect(boom_pos.0);
        // First pass: locate the kill target via an immutable iter.
        // Reading `&Dead` and `&StunFrames` lets us check liveness +
        // i-frames without holding a mut borrow on the query.
        //
        // Critically: hit-stop below is dispatched via
        // `Commands::insert` (deferred). If the bump wrote to
        // `&mut StunFrames` directly, the next boomerang's
        // inner-loop `stun.0 > 0` check would observe the bump and
        // skip its (still-deserved) kill. That created an
        // order-sensitive interaction where the boomerang iteration
        // order at frame N decided which player survived a
        // coincident two-way simultaneous hit — caught by the
        // nightly fuzzer (Phase 11 seeds 29/61) and by the canonical
        // demo gate. Deferred Commands keep mid-system reads
        // unaffected, so coincident kills remain commutative.
        let mut hit: Option<Entity> = None;
        for (player_entity, player, player_pos, dead, stun) in &players {
            if dead.is_dying() {
                continue;
            }
            if player.handle == boom.owner_handle {
                continue;
            }
            if stun.0 > 0 {
                continue;
            }
            if !player_rect(player_pos.0).overlaps(bb) {
                continue;
            }
            hit = Some(player_entity);
            break;
        }
        let Some(player_entity) = hit else {
            continue;
        };

        // Mutate the victim: mark dying via a value update on the
        // always-present `Dead` component (no archetype change, so
        // bevy_ggrs's snapshot/restore stays bit-stable across
        // rollback resimulation) and credit the kill to the thrower —
        // both through the shared `award_kill` path.
        if let Ok((_, _, _, mut dead, _)) = players.get_mut(player_entity) {
            award_kill(&mut dead, boom.owner_handle, frame.0, &mut score);
        }
        commands.entity(boom_entity).despawn();

        // Hit-stop on the killer, deferred via Commands. Snapshot
        // the existing `StunFrames` so a mid-dash killer keeps any
        // longer i-frame window. Skipped if the killer is itself
        // dying.
        let killer_handle = boom.owner_handle;
        let killer_data = players
            .iter()
            .find(|(_, p, _, d, _)| p.handle == killer_handle && !d.is_dying())
            .map(|(e, _, _, _, s)| (e, s.0));
        if let Some((killer_entity, existing_stun)) = killer_data {
            commands
                .entity(killer_entity)
                .insert(StunFrames(existing_stun.max(HIT_STOP_FRAMES)));
        }
    }
}

/// Single source of truth for a kill: mark the victim dying on the
/// always-present `Dead` value component and credit the kill to
/// `killer_handle`. Used by boomerang hits AND arena hazards (the chasm
/// credits the opponent) so the dying-flag + scoring stay byte-identical
/// across every kill source. `saturating_add` is bookkeeping-safe; in
/// practice MatchOver fires at 5 and resets long before the ceiling.
#[inline]
fn award_kill(victim: &mut Dead, killer_handle: usize, frame: u32, score: &mut MatchScore) {
    victim.respawn_at_frame = Some(frame + RESPAWN_FRAMES);
    match killer_handle {
        0 => score.p0 = score.p0.saturating_add(1),
        1 => score.p1 = score.p1.saturating_add(1),
        _ => {}
    }
}

/// `GgrsSchedule` system: the DASH is a melee strike. A player mid-`Dashing`
/// that overlaps a live opponent kills them on contact — no throw needed. The
/// dasher is invincible during the dash (its own `StunFrames` i-frames), so a
/// dash-into-a-dashing-opponent CLASHES (both protected, neither dies), while a
/// dash into a non-dashing, non-i-frame opponent is a clean kill. Runs after
/// `player_movement` + `wall_collision` so positions are settled; mirrors
/// `hit_boomerang_player`'s two-pass deferred-`Dead` pattern (immutable scan →
/// mutate) so coincident kills stay commutative and can't double-count.
pub fn dash_melee_kill(
    frame: Res<FrameCount>,
    match_state: Res<MatchState>,
    mut score: ResMut<MatchScore>,
    mut players: Query<(
        Entity,
        &Player,
        &PositionF,
        &DashState,
        &mut Dead,
        &StunFrames,
    )>,
) {
    if !match_state.is_in_round() {
        return;
    }
    // Scan (victim_entity, killer_handle) with the query held immutably.
    let mut kills: Vec<(Entity, usize)> = Vec::new();
    for (_, dasher, dpos, dash, ddead, _) in &players {
        if ddead.is_dying() || !matches!(dash, DashState::Dashing { .. }) {
            continue;
        }
        let drect = player_rect(dpos.0);
        for (ventity, victim, vpos, _, vdead, vstun) in &players {
            if victim.handle == dasher.handle || vdead.is_dying() || vstun.0 > 0 {
                // Same player, already dying, or invincible (incl. its own dash).
                continue;
            }
            if drect.overlaps(player_rect(vpos.0)) {
                kills.push((ventity, dasher.handle));
            }
        }
    }
    for (ventity, killer) in kills {
        if let Ok((_, _, _, _, mut dead, _)) = players.get_mut(ventity)
            && !dead.is_dying()
        {
            award_kill(&mut dead, killer, frame.0, &mut score);
        }
    }
}

/// `GgrsSchedule` system: wipe the round's boomerang litter and reset each
/// player's arsenal at the top of every round. While `Countdown` is active
/// (no throws happen — gameplay is gated on `is_in_round`) every fang is
/// despawned and `ThrowCapacity`/`ThrowCharge` reset to their defaults, so a
/// new round always starts clean — no leftover Loose fangs on the ground and no
/// carried-over stolen "second boomerang". Idempotent across the countdown.
pub fn reset_round_state(
    match_state: Res<MatchState>,
    mut commands: Commands,
    boomerangs: Query<Entity, With<Boomerang>>,
    mut players: Query<(&mut ThrowCapacity, &mut ThrowCharge)>,
) {
    if !matches!(*match_state, MatchState::Countdown { .. }) {
        return;
    }
    for entity in &boomerangs {
        commands.entity(entity).despawn();
    }
    for (mut cap, mut charge) in &mut players {
        cap.0 = 1;
        charge.0 = 0;
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
    frame: Res<FrameCount>,
    mut commands: Commands,
    mut players: Query<(
        &Player,
        &PositionF,
        &mut AnimState,
        &mut Empowered,
        &mut ThrowCapacity,
    )>,
    boomerangs: Query<(Entity, &Boomerang, &PositionF)>,
) {
    for (entity, boom, boom_pos) in &boomerangs {
        let bb = boomerang_rect(boom_pos.0);
        match boom.state {
            // Flying fangs aren't catchable, else a throw whose spawn overlaps
            // the owner would self-catch on tick 1.
            BoomerangState::Flying => continue,
            // A Returning fang is caught only by its OWNER; a catch inside the
            // recall window empowers the next throw (perfect catch).
            BoomerangState::Returning { since } => {
                let Some((_, owner_pos, mut anim, mut empowered, _)) = players
                    .iter_mut()
                    .find(|(p, _, _, _, _)| p.handle == boom.owner_handle)
                else {
                    continue;
                };
                if player_rect(owner_pos.0).overlaps(bb) {
                    commands.entity(entity).despawn();
                    // `frame >= since` always (recall began in the past).
                    if frame.0 - since <= PERFECT_CATCH_WINDOW_FRAMES {
                        empowered.0 = true;
                    }
                    anim.anim_id = AnimState::CATCH;
                    anim.ticks = 0;
                }
            }
            // A dropped (Loose) fang is picked up by ANYONE who walks over it.
            // The owner reclaims it (re-arm, its slot frees on despawn); an
            // OPPONENT steals it as a second boomerang (+1 ThrowCapacity). First
            // overlap in handle order wins (deterministic).
            BoomerangState::Loose => {
                let picker = players
                    .iter()
                    .find(|(_, pos, _, _, _)| player_rect(pos.0).overlaps(bb))
                    .map(|(p, _, _, _, _)| p.handle);
                let Some(handle) = picker else {
                    continue;
                };
                commands.entity(entity).despawn();
                if let Some((_, _, mut anim, _, mut cap)) = players
                    .iter_mut()
                    .find(|(p, _, _, _, _)| p.handle == handle)
                {
                    if handle != boom.owner_handle {
                        cap.0 = cap.0.saturating_add(1); // stolen → a 2nd boomerang
                    }
                    anim.anim_id = AnimState::CATCH;
                    anim.ticks = 0;
                }
            }
        }
    }
}

/// `GgrsSchedule` system: spawn boomerangs on THROW_DOWN release edges.
/// Runs after `wall_collision` so the spawn position is the post-
/// resolution player position, and after `boomerang_physics` so the
/// freshly-spawned boomerang doesn't take a phantom physics step on
/// its spawn frame.
/// The thrower query for [`throw_boomerangs`] — aliased to keep clippy's
/// type-complexity lint happy (the charge pass grew it to eight members).
type ThrowerQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static Player,
        &'static Dead,
        &'static PositionF,
        &'static ThrowCapacity,
        &'static mut AnimState,
        &'static mut Empowered,
        &'static mut HeldModifier,
        &'static mut ThrowCharge,
    ),
>;

pub fn throw_boomerangs(
    frame: Res<FrameCount>,
    match_state: Res<MatchState>,
    mut commands: Commands,
    inputs: Res<PlayerInputs<GgrsCfg>>,
    history: Res<InputHistory>,
    mut players: ThrowerQuery,
    boomerangs: Query<(&Boomerang, &BoomerangMods)>,
) {
    if !match_state.is_in_round() {
        return;
    }
    for (player, dead, pos, capacity, mut anim, mut empowered, mut held, mut charge) in &mut players
    {
        if dead.is_dying() {
            charge.0 = 0;
            continue;
        }
        // Each primary (non-secondary) fang the player owns occupies one of its
        // `ThrowCapacity` slots. A free slot means it can throw a new fang;
        // when every slot is out, a THROW press is a recall (recall_boomerangs).
        let owned = boomerangs
            .iter()
            .filter(|(b, m)| b.owner_handle == player.handle && !m.is_secondary)
            .count();
        let can_throw = owned < capacity.0 as usize;
        let Some(ring) = history.0.get(&player.handle) else {
            continue;
        };
        let (curr, _) = inputs[player.handle];
        // Throw fires on the THROW RELEASE edge iff a slot is free. Otherwise
        // the hold is either building charge (slot free) or, with every slot
        // out, a recall press consumed by recall_boomerangs.
        let Some(unit_dir) = try_throw_direction(ring, curr, !can_throw) else {
            let throw_held = curr.buttons & PlayerInput::THROW_DOWN != 0;
            charge.0 = if throw_held && can_throw {
                (charge.0 + 1).min(CHARGE_MAX_FRAMES)
            } else {
                0
            };
            continue;
        };
        // Release: the accumulated CHARGE sets the fang's speed AND reach — a
        // quick tap lobs a slow short fang, a full hold hurls a fast far one.
        let power = charge_power(charge.0);
        charge.0 = 0;
        // A held pickup rides this throw; consumed here. Perfect-catch
        // empowerment is also consumed.
        let modifier = held.0.take();
        let base_speed = modified_throw_speed(empowered.0, modifier);
        let velocity = unit_dir * aimed_throw_speed(base_speed, power);
        let reach = charged_reach(power);
        empowered.0 = false;
        // The primary (recallable, catchable) fang flies straight.
        commands.spawn((
            Boomerang {
                owner_handle: player.handle,
                state: BoomerangState::Flying,
            },
            BoomerangMods {
                modifier,
                is_secondary: false,
                despawn_at_frame: None,
            },
            ThrowOrigin(pos.0),
            ThrowReach(reach),
            PositionF(pos.0),
            PreviousPositionF(pos.0),
            VelocityF(velocity),
        ));
        // Multishot adds two fire-and-forget side-fangs at ±15°. They share
        // the modifier so they read as the same throw, but are flagged
        // `is_secondary` so recall/catch ignore them and a wall hit (or the
        // lifetime backstop) despawns them.
        if matches!(modifier, Some(PickupKind::Multishot)) {
            let expire = frame.0 + MULTISHOT_SECONDARY_LIFETIME_FRAMES;
            for theta in [MULTISHOT_FAN_RAD, -MULTISHOT_FAN_RAD] {
                commands.spawn((
                    Boomerang {
                        owner_handle: player.handle,
                        state: BoomerangState::Flying,
                    },
                    BoomerangMods {
                        modifier,
                        is_secondary: true,
                        despawn_at_frame: Some(expire),
                    },
                    PositionF(pos.0),
                    PreviousPositionF(pos.0),
                    VelocityF(velocity.rotate(theta)),
                ));
            }
        }
        // Phase 15: kick the throw animation. Reset to frame 0 so the
        // 6-frame wind-up plays from the start. `advance_animation`
        // will keep it ticking until is_finished(), then the
        // observable-state path in that system snaps back to Idle.
        anim.anim_id = AnimState::THROW;
        anim.ticks = 0;
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
    mut commands: Commands,
    walls: Query<&Wall>,
    mut boomerangs: Query<(
        Entity,
        &mut Boomerang,
        &BoomerangMods,
        &PreviousPositionF,
        &mut PositionF,
        &mut VelocityF,
    )>,
) {
    for (entity, mut boom, mods, prev, mut pos, mut vel) in &mut boomerangs {
        if matches!(boom.state, BoomerangState::Returning { .. }) {
            continue;
        }
        // Phantom phases through walls (through-wall snipe).
        if matches!(mods.modifier, Some(PickupKind::Phantom)) {
            continue;
        }
        let bouncy = matches!(mods.modifier, Some(PickupKind::Bouncy));
        for wall in &walls {
            // Swept contact: resolves the common in-wall overlap exactly as
            // before, and additionally catches a fast fang (Bouncy /
            // Fire+empowered at up to 80 cm/tick) that would step entirely
            // over a 50 cm wall in one tick without ever overlapping it.
            let Some((contact, push)) = swept_wall_contact(prev.0, pos.0, wall.rect) else {
                continue;
            };
            // Multishot side-fangs die on the first wall they touch rather than
            // ricocheting — the fan is a one-way burst.
            if mods.is_secondary {
                commands.entity(entity).despawn();
                break;
            }
            match wall.kind {
                // The outer ring is PERMEABLE: a fang that reaches the edge
                // turns around and homes back to the thrower (recall_boomerangs
                // drives the homing) instead of pinballing off a hard border
                // forever. Players are still contained by the boundary in
                // wall_collision; only boomerangs pass.
                // The outer ring is PERMEABLE to boomerangs: a primary fang
                // flies straight OUT past the boundary (open-field model — the
                // same way a player can now leave the field) and comes back via
                // recall_boomerangs' distance cap (BOOMERANG_MAX_THROW_DISTANCE_CM),
                // with the 4000cm despawn radius as the hard backstop. Players
                // stay contained by the boundary in wall_collision. `continue`
                // (not `break`) keeps checking the remaining obstacle walls so
                // inner-cover ricochet on the same tick is unaffected.
                WallKind::Boundary => continue,
                // Inner cover ricochets the fang — and knocks a Flying fang
                // LOOSE, so after the bounce it loses momentum and settles to
                // rest instead of pinballing forever (Boomerang-Fu drop). A
                // fang that's already Loose keeps ricocheting + decelerating.
                WallKind::Obstacle => {
                    pos.0 = contact + push;
                    vel.0 = reflect_velocity_for_push(vel.0, push);
                    if bouncy {
                        vel.0 = bouncy_accelerate(vel.0);
                    }
                    if matches!(boom.state, BoomerangState::Flying) {
                        boom.state = BoomerangState::Loose;
                    }
                }
            }
        }
    }
}

/// Maximum sub-segment length (cm) when sweeping a fast boomerang against
/// a wall. A boomerang overlaps a 50 cm-thick wall across a ~70 cm band
/// of center positions; sampling no farther apart than this guarantees at
/// least one sample lands inside that band, so a fang travelling up to
/// 80 cm/tick can't tunnel fully through between two ticks. Held well
/// under the band for margin.
pub const WALL_SWEEP_STEP_CM: i32 = 20;

/// Swept boomerang-vs-wall contact. Returns `(sample_position, push)`:
///
/// - **Common case** — the boomerang's current AABB overlaps the wall:
///   resolve in place, byte-identical to the old point check (no snap-back).
/// - **Tunnel case** — the current AABB is clear but the prev→cur segment
///   crossed the wall (a fast fang stepped over it in one tick): sub-sample
///   the segment and reflect at the FIRST crossing, so the fang bounces at
///   its entry face instead of escaping to the despawn radius.
///
/// Pure (no `f32`, no RNG) so it stays deterministic across the matrix.
pub fn swept_wall_contact(prev: Vec2F, cur: Vec2F, wall: RectF) -> Option<(Vec2F, Vec2F)> {
    // Common case: current position overlaps — resolve there directly.
    if let Some(push) = resolve_collision(boomerang_rect(cur), wall) {
        return Some((cur, push));
    }
    // Tunnel case: cur is clear; sub-sample prev->cur for a crossing.
    // Chebyshev span avoids a sqrt and bounds the longest axis travel.
    let dx = (cur.x - prev.x).abs();
    let dy = (cur.y - prev.y).abs();
    let span = if dx > dy { dx } else { dy };
    let span_cm: i32 = span.to_num();
    if span_cm <= WALL_SWEEP_STEP_CM {
        // Step too short to tunnel a wall thicker than the sweep step.
        return None;
    }
    let n = span_cm / WALL_SWEEP_STEP_CM + 1;
    let delta = cur - prev;
    let n_fix = Fix::const_from_int(n);
    // i in 1..n — skip n (== cur, already checked clear above).
    for i in 1..n {
        let t = Fix::const_from_int(i) / n_fix;
        let sample = prev + delta * t;
        if let Some(push) = resolve_collision(boomerang_rect(sample), wall) {
            return Some((sample, push));
        }
    }
    None
}

/// Speed of a Bouncy boomerang after a ricochet: ×1.1, capped at
/// `BOUNCY_MAX_SPEED`. Direction unchanged (already reflected).
fn bouncy_accelerate(vel: Vec2F) -> Vec2F {
    let speed = vel.length();
    let boosted = (speed * Fix::lit("1.1")).min(Fix::const_from_int(BOUNCY_MAX_SPEED_CM_PER_TICK));
    vel.normalize() * boosted
}

/// Bouncy speed ceiling — fast enough to be scary, bounded so it can't run
/// away past the despawn radius in a single tick. Halved with the rest of the
/// boomerang speeds (still 1.6× the base throw, as before).
pub const BOUNCY_MAX_SPEED_CM_PER_TICK: i32 = 40;

/// Curve's turn rate while flying: 1.5° per tick (≈90°/sec) in radians.
pub const CURVE_RAD_PER_TICK: Fix = Fix::lit("0.0261799");

/// Multishot fan half-angle: the two side-fangs launch at ±15° off the aim,
/// in radians. The center fang flies straight (the recallable primary).
pub const MULTISHOT_FAN_RAD: Fix = Fix::lit("0.2617994");

/// Multishot side-fangs are fire-and-forget: they despawn on first wall
/// contact, or after this many frames if they somehow miss every wall.
pub const MULTISHOT_SECONDARY_LIFETIME_FRAMES: u32 = 120;

/// A Fire boomerang drops a burning cell every this many ticks while flying.
pub const FIRE_TRAIL_INTERVAL_TICKS: u32 = 6;
/// How long a dropped fire cell stays lethal before burning out.
pub const FIRE_TRAIL_LIFETIME_FRAMES: u32 = 90;
/// Fire cell collision half-extent in cm — a 24 cm hot square, smaller than
/// the player so it reads as a hazard you can thread between, not a wall.
pub const FIRE_TRAIL_HALF_EXTENT_CM: i32 = 12;

/// A lingering patch of fire dropped by a Fire-modified boomerang. Rolled
/// back like every gameplay entity. Position lives in a paired `PositionF`
/// (the cell never moves), mirroring how floor `Pickup`s are placed.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[require(Rollback)]
pub struct FireTrailCell {
    /// The thrower — immune to their own fire; the opponent is not.
    pub owner_handle: usize,
    /// Frame at which the cell burns out and despawns.
    pub expires_at_frame: u32,
}

/// Collision AABB for a fire cell centered on `pos`.
pub fn fire_trail_rect(pos: Vec2F) -> RectF {
    let half = Vec2F::from_cm(FIRE_TRAIL_HALF_EXTENT_CM, FIRE_TRAIL_HALF_EXTENT_CM);
    RectF::from_center_half_extents(pos, half)
}

/// `GgrsSchedule` system: bend Curve boomerangs each flying tick (banana
/// throw). Runs before `boomerang_physics` so the rotated heading is what
/// moves this tick.
pub fn curve_boomerangs(mut q: Query<(&Boomerang, &BoomerangMods, &mut VelocityF)>) {
    for (boom, mods, mut vel) in &mut q {
        if matches!(boom.state, BoomerangState::Flying)
            && matches!(mods.modifier, Some(PickupKind::Curve))
        {
            vel.0 = vel.0.rotate(CURVE_RAD_PER_TICK);
        }
    }
}

/// `GgrsSchedule` system: despawn Multishot side-fangs that have outlived
/// their `despawn_at_frame` backstop without hitting a wall first. Primaries
/// and non-multishot fangs carry `None` and are never touched here.
pub fn expire_secondary_boomerangs(
    frame: Res<FrameCount>,
    mut commands: Commands,
    q: Query<(Entity, &BoomerangMods)>,
) {
    for (e, mods) in &q {
        if let Some(at) = mods.despawn_at_frame
            && frame.0 >= at
        {
            commands.entity(e).despawn();
        }
    }
}

/// `GgrsSchedule` system: a Fire boomerang lays down a burning cell at its
/// current position every `FIRE_TRAIL_INTERVAL_TICKS`. Gated on the global
/// frame counter (deterministic and rollback-stable) so every Fire fang in
/// flight drops on the same cadence. Runs after `boomerang_physics` so the
/// cell lands on the boomerang's post-move position.
pub fn drop_fire_trail(
    frame: Res<FrameCount>,
    mut commands: Commands,
    boomerangs: Query<(&Boomerang, &BoomerangMods, &PositionF)>,
) {
    if !frame.0.is_multiple_of(FIRE_TRAIL_INTERVAL_TICKS) {
        return;
    }
    for (boom, mods, pos) in &boomerangs {
        if matches!(boom.state, BoomerangState::Flying)
            && matches!(mods.modifier, Some(PickupKind::Fire))
        {
            commands.spawn((
                FireTrailCell {
                    owner_handle: boom.owner_handle,
                    expires_at_frame: frame.0 + FIRE_TRAIL_LIFETIME_FRAMES,
                },
                PositionF(pos.0),
            ));
        }
    }
}

/// `GgrsSchedule` system: a fire cell kills any non-owner player standing in
/// it, crediting the cell's owner through the shared `award_kill` path (same
/// dying-flag + scoring as a boomerang hit). Dash i-frames (`StunFrames`)
/// and already-dying players are immune. Direct `&mut Dead` is safe here
/// (unlike `hit_boomerang_player`'s deferred hit-stop): a victim flagged
/// dying by one cell is skipped by every other cell the same tick, so a
/// kill counts once and coincident two-way kills stay commutative — each
/// cell kills a *different* victim.
pub fn fire_trail_kills(
    frame: Res<FrameCount>,
    mut score: ResMut<MatchScore>,
    cells: Query<(&FireTrailCell, &PositionF)>,
    mut players: Query<(&Player, &PositionF, &mut Dead, &StunFrames)>,
) {
    for (cell, cell_pos) in &cells {
        let cr = fire_trail_rect(cell_pos.0);
        for (player, player_pos, mut dead, stun) in &mut players {
            if dead.is_dying() || player.handle == cell.owner_handle || stun.0 > 0 {
                continue;
            }
            if player_rect(player_pos.0).overlaps(cr) {
                award_kill(&mut dead, cell.owner_handle, frame.0, &mut score);
            }
        }
    }
}

/// `GgrsSchedule` system: despawn fire cells that have burned out.
pub fn expire_fire_trail(
    frame: Res<FrameCount>,
    mut commands: Commands,
    cells: Query<(Entity, &FireTrailCell)>,
) {
    for (e, cell) in &cells {
        if frame.0 >= cell.expires_at_frame {
            commands.entity(e).despawn();
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
/// The boomerang query for [`recall_boomerangs`] — aliased for the
/// type-complexity lint (per-throw `ThrowReach` grew it to six members).
type RecallQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Boomerang,
        &'static BoomerangMods,
        Option<&'static ThrowOrigin>,
        Option<&'static ThrowReach>,
        &'static PositionF,
        &'static mut VelocityF,
    ),
>;

pub fn recall_boomerangs(
    frame: Res<FrameCount>,
    match_state: Res<MatchState>,
    inputs: Res<PlayerInputs<GgrsCfg>>,
    history: Res<InputHistory>,
    players: Query<(&Player, &PositionF, &ThrowCapacity)>,
    mut boomerangs: RecallQuery,
) {
    if !match_state.is_in_round() {
        return;
    }
    let recall_speed = Fix::const_from_int(RECALL_SPEED_CM_PER_TICK);
    // Owned primary fangs per handle: a manual (THROW-press) recall only fires
    // when the owner has NO free throw slot (every fang out). With a slot free
    // a THROW press is a new charged throw (throw_boomerangs), not a recall.
    let mut owned: std::collections::BTreeMap<usize, u32> = std::collections::BTreeMap::new();
    for (boom, mods, _, _, _, _) in boomerangs.iter() {
        if !mods.is_secondary {
            *owned.entry(boom.owner_handle).or_default() += 1;
        }
    }
    for (mut boom, mods, origin, reach, boom_pos, mut vel) in &mut boomerangs {
        // Multishot side-fangs never return — they're throw-and-forget.
        if mods.is_secondary {
            continue;
        }
        let Some((_, owner_pos, capacity)) = players
            .iter()
            .find(|(p, _, _)| p.handle == boom.owner_handle)
        else {
            continue;
        };
        let no_free_slot = owned.get(&boom.owner_handle).copied().unwrap_or(0) >= capacity.0;
        // Manual recall: owner pressed THROW_DOWN this tick AND has no slot to
        // throw into (else the press is a fresh charged throw).
        let manual = no_free_slot
            && history.0.get(&boom.owner_handle).is_some_and(|ring| {
                let (curr, _) = inputs[boom.owner_handle];
                just_pressed(curr, previous_input(ring), PlayerInput::THROW_DOWN)
            });
        let max_dist = reach
            .map(|r| r.0)
            .unwrap_or_else(|| Fix::const_from_int(BOOMERANG_MAX_THROW_DISTANCE_CM));
        match boom.state {
            BoomerangState::Flying => {
                // Auto recall: the fang reached its charge-scaled reach from the
                // origin, so it turns back instead of sailing on / out.
                let reached_max = origin.is_some_and(|o| (boom_pos.0 - o.0).length() >= max_dist);
                if manual || reached_max {
                    boom.state = BoomerangState::Returning { since: frame.0 };
                    vel.0 = recall_velocity(boom_pos.0, owner_pos.0, recall_speed);
                }
            }
            BoomerangState::Returning { .. } => {
                vel.0 = recall_velocity(boom_pos.0, owner_pos.0, recall_speed);
            }
            BoomerangState::Loose => {
                // A dropped fang lies still; a THROW_DOWN edge hold-recalls it
                // back to the owner (you're disarmed until you retrieve it).
                if manual {
                    boom.state = BoomerangState::Returning { since: frame.0 };
                    vel.0 = recall_velocity(boom_pos.0, owner_pos.0, recall_speed);
                }
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
    mut q: Query<(Entity, &Boomerang, &mut PositionF, &mut VelocityF)>,
) {
    let max_r = Fix::const_from_int(BOOMERANG_DESPAWN_RADIUS_CM);
    let rest_speed = Fix::const_from_int(LOOSE_REST_SPEED_CM);
    for (entity, boom, mut pos, mut vel) in &mut q {
        // A loose fang bleeds momentum to rest and then PERSISTS on the ground
        // (no lifetime timer) until it's recalled, caught, stolen, or the round
        // resets — a permanent second boomerang for whoever picks it up.
        if matches!(boom.state, BoomerangState::Loose) {
            vel.0 = vel.0 * LOOSE_DRAG;
            if vel.0.length() <= rest_speed {
                vel.0 = Vec2F::ZERO;
            }
        }
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
/// For strictly-unequal overlaps the smaller-overlap axis is the
/// minimum-translation vector and is unambiguously correct.
///
/// **Tie-break (equal overlaps):** equal overlaps happen when a small
/// projectile is *fully embedded* in a long wall — both axes' overlaps
/// equal the projectile's full footprint. The MTV is then ambiguous, so
/// we reflect along the **wall's thin axis** (its short dimension),
/// which is always the correct ricochet normal for the long-thin
/// boundary walls (North/South are thin in Y, East/West thin in X). The
/// previous code hardcoded the X axis on a tie; that silently tunnelled
/// fast vertical boomerangs straight through the North/South walls (the
/// X-flip left their upward Y velocity intact, so they flew out the top
/// or bottom of the arena and despawned, never returning).
pub fn resolve_collision(player: RectF, wall: RectF) -> Option<Vec2F> {
    if !player.overlaps(wall) {
        return None;
    }
    let overlap_x =
        core::cmp::min(player.max.x, wall.max.x) - core::cmp::max(player.min.x, wall.min.x);
    let overlap_y =
        core::cmp::min(player.max.y, wall.max.y) - core::cmp::max(player.min.y, wall.min.y);

    // 2× center comparisons — sign of (player_2cx - wall_2cx) tells us
    // which side of the wall the player center sits on.
    let player_2cx = player.min.x + player.max.x;
    let wall_2cx = wall.min.x + wall.max.x;
    let player_2cy = player.min.y + player.max.y;
    let wall_2cy = wall.min.y + wall.max.y;

    // Smaller-overlap axis is the MTV; on a tie, fall back to the wall's
    // thin axis — the correct ricochet normal for a fully-embedded
    // projectile in a long-thin wall.
    let push_along_x = if overlap_x != overlap_y {
        overlap_x < overlap_y
    } else {
        wall.width() <= wall.height()
    };

    if push_along_x {
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
    // Reflect ONLY the velocity component still moving INTO the wall — i.e. with
    // a sign opposite the outward `push`. A fang already moving away is left
    // alone. Without this guard a fang in deep contact (e.g. a hard throw that
    // penetrates a wall thicker than itself) gets its wall-normal velocity
    // flipped EVERY tick, so it jitters in and out across the boundary forever
    // and machine-guns the bounce cue (the "deafening" oscillation).
    if push.x != Fix::ZERO {
        if (vel.x > Fix::ZERO) != (push.x > Fix::ZERO) {
            Vec2F::new(-vel.x, vel.y)
        } else {
            vel
        }
    } else if push.y != Fix::ZERO {
        if (vel.y > Fix::ZERO) != (push.y > Fix::ZERO) {
            Vec2F::new(vel.x, -vel.y)
        } else {
            vel
        }
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
pub fn wall_collision(walls: Query<&Wall>, mut players: Query<&mut PositionF, With<Player>>) {
    for mut pos in &mut players {
        for wall in &walls {
            // The outer ring no longer contains players — they can run off the
            // field into the out-of-bounds death zone (see `oob_death`), the
            // Boomerang-Fu open-field model. Only inner Obstacle cover blocks.
            if matches!(wall.kind, WallKind::Boundary) {
                continue;
            }
            let player = player_rect(pos.0);
            if let Some(push) = resolve_collision(player, wall.rect) {
                pos.0 = pos.0 + push;
            }
        }
    }
}

/// `GgrsSchedule` system: the out-of-bounds death zone. A player past the floor
/// edge accrues OOB frames; once continuously out for [`OOB_GRACE_FRAMES`] the
/// void claims them and the opponent scores (Boomerang-Fu environmental kill).
/// The counter resets the instant they're back inside; the already-dying are
/// skipped (tick_respawn drops them back inside on respawn).
pub fn oob_death(
    frame: Res<FrameCount>,
    match_state: Res<MatchState>,
    mut score: ResMut<MatchScore>,
    mut q: Query<(
        &Player,
        &mut PositionF,
        &mut PreviousPositionF,
        &mut Dead,
        &mut OobTimer,
    )>,
) {
    if !match_state.is_in_round() {
        return;
    }
    let half_w = Fix::const_from_int(ARENA_HALF_WIDTH_CM);
    let half_h = Fix::const_from_int(ARENA_HALF_HEIGHT_CM);
    for (player, mut pos, mut prev, mut dead, mut oob) in &mut q {
        if dead.is_dying() {
            oob.0 = 0;
            continue;
        }
        let out_of_bounds = pos.0.x.abs() > half_w || pos.0.y.abs() > half_h;
        if out_of_bounds {
            oob.0 += 1;
            if oob.0 >= OOB_GRACE_FRAMES {
                // Credit the opponent (2-player) so running out isn't free.
                award_kill(&mut dead, 1 - player.handle, frame.0, &mut score);
                // The void spits the corpse back to the spawn point immediately,
                // so the dying body doesn't lie out of bounds for the whole
                // death window (tick_respawn would otherwise only snap it on
                // respawn). snap collapses pos+prev so the render lerp doesn't
                // streak across the arena.
                snap_position(&mut pos, &mut prev, respawn_position(player.handle));
                oob.0 = 0;
            }
        } else {
            oob.0 = 0;
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

/// Per-handle respawn point. Symmetric on the Y axis: P0 near/bottom,
/// P1 far/top — the depth-duel axis. Both players re-enter the round
/// on equal footing rather than spawning on top of where they last died.
pub fn respawn_position(handle: usize) -> Vec2F {
    match handle {
        0 => Vec2F::from_cm(0, -300),
        _ => Vec2F::from_cm(0, 300),
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
type PlayerStateQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static Player,
        &'static mut Dead,
        &'static mut PositionF,
        &'static mut PreviousPositionF,
        &'static mut VelocityF,
        &'static mut DashState,
        &'static mut StunFrames,
        &'static mut ThrowCapacity,
        &'static mut ThrowCharge,
    ),
>;

pub fn tick_respawn(frame: Res<FrameCount>, mut q: PlayerStateQuery) {
    for (player, mut dead, mut pos, mut prev, mut vel, mut dash, mut stun, mut cap, mut charge) in
        &mut q
    {
        let Some(at) = dead.respawn_at_frame else {
            continue;
        };
        if frame.0 < at {
            continue;
        }
        snap_position(&mut pos, &mut prev, respawn_position(player.handle));
        vel.0 = Vec2F::ZERO;
        *dash = DashState::default();
        *stun = StunFrames(0);
        // Death forfeits any stolen "second boomerang" and pending charge.
        cap.0 = 1;
        charge.0 = 0;
        dead.respawn_at_frame = None;
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

/// `GgrsSchedule` system: deterministic "play again". `MatchOver` is terminal
/// in [`tick_match_state`]; this is the one escape — a THROW rising edge from
/// *either* player while `MatchOver` restarts the match: score back to 0-0,
/// state back to the top of the countdown, and the arena wiped to a clean
/// slate (players respawned to their symmetric spawns with all per-player
/// state cleared; every in-flight boomerang, floor pickup, and fire cell
/// despawned; pyres un-shattered; bridge and door cooldowns cleared).
///
/// Input-driven on purpose: it resimulates identically under rollback and
/// stays lockstep across netplay peers, exactly like every other gameplay
/// transition (CONVENTIONS § Determinism — never reset via out-of-band World
/// mutation). Reusing the existing `THROW_DOWN` level signal means no
/// wire-format change. Runs just before `tick_match_state`, which then ticks
/// the fresh countdown normally. The render-side `clear_stains_on_match_reset`
/// keys off this same `MatchOver → Countdown` edge to wipe the match's blood.
/// Every per-player component [`apply_rematch`] resets, aliased to keep the
/// system signature readable (and clippy's `type_complexity` quiet), mirroring
/// [`PlayerStateQuery`].
type RematchPlayerQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static Player,
        &'static mut PositionF,
        &'static mut PreviousPositionF,
        &'static mut VelocityF,
        &'static mut Dead,
        &'static mut DashState,
        &'static mut StunFrames,
        &'static mut Empowered,
        &'static mut HeldModifier,
        &'static mut AnimState,
    ),
>;

#[allow(clippy::too_many_arguments)]
pub fn apply_rematch(
    frame: Res<FrameCount>,
    inputs: Res<PlayerInputs<GgrsCfg>>,
    history: Res<InputHistory>,
    mut state: ResMut<MatchState>,
    mut score: ResMut<MatchScore>,
    mut bridge: ResMut<BridgeState>,
    mut door: ResMut<DoorCooldown>,
    mut commands: Commands,
    mut players: RematchPlayerQuery,
    boomerangs: Query<Entity, With<Boomerang>>,
    pickups: Query<Entity, With<Pickup>>,
    fire_cells: Query<Entity, With<FireTrailCell>>,
    mut pyres: Query<&mut BonePyre>,
) {
    if !matches!(*state, MatchState::MatchOver) {
        return;
    }

    // Restart on a THROW rising edge from either player (edge derived from the
    // rolled-back history, never a wire bit — Global rule 4).
    let restart = players.iter().any(|(player, ..)| {
        let (curr, _status) = inputs[player.handle];
        let prev = history
            .0
            .get(&player.handle)
            .map(previous_input)
            .unwrap_or_default();
        just_pressed(curr, prev, PlayerInput::THROW_DOWN)
    });
    if !restart {
        return;
    }

    // Players: symmetric clean reset — the same fields `tick_respawn` clears.
    for (
        player,
        mut pos,
        mut prev,
        mut vel,
        mut dead,
        mut dash,
        mut stun,
        mut emp,
        mut held,
        mut anim,
    ) in &mut players
    {
        snap_position(&mut pos, &mut prev, respawn_position(player.handle));
        vel.0 = Vec2F::ZERO;
        *dead = Dead::default();
        *dash = DashState::default();
        *stun = StunFrames(0);
        *emp = Empowered(false);
        *held = HeldModifier(None);
        anim.anim_id = AnimState::IDLE;
        anim.ticks = 0;
    }

    // Arena entities: wipe to a fresh slate.
    for entity in &boomerangs {
        commands.entity(entity).despawn();
    }
    for entity in &pickups {
        commands.entity(entity).despawn();
    }
    for entity in &fire_cells {
        commands.entity(entity).despawn();
    }
    for mut pyre in &mut pyres {
        pyre.shattered = false;
        pyre.chain_delay = None;
    }
    *bridge = BridgeState::default();
    *door = DoorCooldown::default();

    // Fresh match: score reset, top of the countdown.
    *score = MatchScore::default();
    *state = MatchState::Countdown {
        digit: 3,
        expires_at_frame: frame.0 + COUNTDOWN_DIGIT_FRAMES,
    };
}

pub fn record_last_tick_time(time: Res<Time<Real>>, mut last: ResMut<LastSimTickTime>) {
    last.0 = time.elapsed_secs_f64();
}

/// Pure helper for [`advance_animation`]'s state machine. Given the
/// observable state of one entity at the START of the tick, returns
/// the AnimState the entity should hold at END of tick.
///
/// Priority order (highest wins):
///   1. `Dead.is_dying()` -> DEATH
///   2. `StunFrames > 0` -> HIT
///   3. One-shot anim still in flight -> keep ticking the same anim
///   4. `DashState::Dashing` -> DASH
///   5. `is_moving` (stick magnitude > threshold) -> RUN
///   6. otherwise -> IDLE
///
/// THROW and CATCH transitions are set externally by
/// `throw_boomerangs` and `catch_boomerangs` on the tick the event
/// fires; this helper preserves them as one-shot via rule 3 until
/// their frames elapse.
///
/// Pure for property-test purposes — no `&mut World`, no resources,
/// no side effects. The system below is a thin wrapper.
pub fn next_anim_state(
    dead: Dead,
    dash: DashState,
    stun: StunFrames,
    current: AnimState,
    is_moving: bool,
    charging: bool,
) -> AnimState {
    let target_id = if dead.is_dying() {
        Some(AnimState::DEATH)
    } else if stun.0 > 0 {
        Some(AnimState::HIT)
    } else if AnimState::is_oneshot(current.anim_id) && !current.is_finished() {
        // Let the in-flight one-shot finish.
        return AnimState {
            anim_id: current.anim_id,
            ticks: current.ticks.saturating_add(1),
        };
    } else if matches!(dash, DashState::Dashing { .. }) {
        Some(AnimState::DASH)
    } else if charging {
        // Winding up a throw reads over run/idle — the coiled-charge pose.
        Some(AnimState::CHARGE)
    } else if is_moving {
        Some(AnimState::RUN)
    } else {
        Some(AnimState::IDLE)
    };
    let id = target_id.expect("priority above always assigns an anim_id");
    if id != current.anim_id {
        AnimState {
            anim_id: id,
            ticks: 0,
        }
    } else {
        AnimState {
            anim_id: id,
            ticks: current.ticks.saturating_add(1),
        }
    }
}

/// `GgrsSchedule` system: tick the AnimState frame counter and run
/// the animation state-machine. Delegates per-entity logic to
/// [`next_anim_state`] (which is pure and property-tested in
/// `tests/anim_state.rs`).
///
/// Per CONVENTIONS: animation does not interpolate — `display_index`
/// snaps to a single atlas frame per tick.
pub fn advance_animation(
    inputs: Res<PlayerInputs<GgrsCfg>>,
    mut q: Query<(
        &Player,
        &Dead,
        &DashState,
        &StunFrames,
        &ThrowCharge,
        &mut AnimState,
    )>,
) {
    for (player, dead, dash, stun, charge, mut anim) in &mut q {
        let (curr, _) = inputs[player.handle];
        // While AIM_ACTIVE the wire stick carries the aim, not movement, and
        // the player is anchored (player_movement zeroes velocity) — so it
        // must not read as "moving" or it'd play the run cycle while planted.
        let aiming = curr.buttons & PlayerInput::AIM_ACTIVE != 0;
        let is_moving = !aiming && decode_stick(curr).length() > DASH_MIN_STICK_MAG;
        let charging = charge.0 > 0;
        *anim = next_anim_state(*dead, *dash, *stun, *anim, is_moving, charging);
    }
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
            .init_resource::<SelectedArena>()
            .init_resource::<BridgeState>()
            .init_resource::<DoorCooldown>()
            .init_resource::<SimRng>()
            .init_resource::<PickupSpawnTimer>()
            .insert_resource(RollbackFrameRate(TICK_HZ));

        // Rollback registrations
        app.rollback_component_with_copy::<PositionF>()
            .rollback_component_with_copy::<PreviousPositionF>()
            .rollback_component_with_copy::<VelocityF>()
            .rollback_component_with_copy::<Player>()
            .rollback_component_with_copy::<NoInterpolate>()
            .rollback_component_with_copy::<DashState>()
            .rollback_component_with_copy::<StunFrames>()
            .rollback_component_with_copy::<OobTimer>()
            .rollback_component_with_copy::<Dead>()
            .rollback_component_with_copy::<Boomerang>()
            .rollback_component_with_copy::<BoomerangState>()
            .rollback_component_with_copy::<ThrowOrigin>()
            .rollback_component_with_copy::<ThrowReach>()
            .rollback_component_with_copy::<ThrowCharge>()
            .rollback_component_with_copy::<ThrowCapacity>()
            .rollback_component_with_copy::<AnimState>()
            .rollback_component_with_copy::<Empowered>()
            .rollback_component_with_copy::<HeldModifier>()
            .rollback_component_with_copy::<BoomerangMods>()
            .rollback_component_with_copy::<Pickup>()
            .rollback_component_with_copy::<FireTrailCell>()
            .rollback_component_with_copy::<BonePyre>()
            .rollback_resource_with_copy::<FrameCount>()
            .rollback_resource_with_copy::<MatchScore>()
            .rollback_resource_with_copy::<MatchState>()
            .rollback_resource_with_copy::<BridgeState>()
            .rollback_resource_with_copy::<DoorCooldown>()
            .rollback_resource_with_copy::<SimRng>()
            .rollback_resource_with_copy::<PickupSpawnTimer>()
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
            .checksum_component_with_hash::<OobTimer>()
            .checksum_component_with_hash::<Dead>()
            .checksum_component_with_hash::<Boomerang>()
            .checksum_component_with_hash::<ThrowOrigin>()
            .checksum_component_with_hash::<ThrowReach>()
            .checksum_component_with_hash::<ThrowCharge>()
            .checksum_component_with_hash::<ThrowCapacity>()
            .checksum_component_with_hash::<AnimState>()
            .checksum_component_with_hash::<Empowered>()
            .checksum_component_with_hash::<HeldModifier>()
            .checksum_component_with_hash::<BoomerangMods>()
            .checksum_component_with_hash::<Pickup>()
            .checksum_component_with_hash::<FireTrailCell>()
            .checksum_component_with_hash::<BonePyre>()
            .checksum_resource_with_hash::<FrameCount>()
            .checksum_resource_with_hash::<MatchScore>()
            .checksum_resource_with_hash::<MatchState>()
            .checksum_resource_with_hash::<BridgeState>()
            .checksum_resource_with_hash::<DoorCooldown>()
            .checksum_resource_with_hash::<SimRng>()
            .checksum_resource_with_hash::<PickupSpawnTimer>()
            .checksum_resource_with_hash::<InputHistory>();

        // Sim systems — explicitly ordered per CONVENTIONS.md.
        // snapshot_previous runs FIRST so the PositionF copy it captures
        // is the value at the start of this tick (== end of prior tick).
        // wall_collision runs immediately after player_movement so the
        // PositionF coming out of this tick is the post-resolution
        // position the render layer sees. advance_input_history runs
        // LAST so edge consumers see the ring's last entry as "previous
        // tick" until end-of-tick rolls it forward.
        // The boomerang + arena-interaction cluster is a nested chain: it
        // keeps the per-tick order explicit while holding the outer tuple
        // under Bevy's 20-element `.chain()` arity limit. Arena-specific
        // systems are gated declaratively with `run_if(arena_is(_))`.
        app.add_systems(
            GgrsSchedule,
            (
                snapshot_previous,
                tick_respawn,
                apply_rematch,
                tick_match_state,
                reset_round_state,
                start_dash,
                player_movement,
                wall_collision,
                oob_death,
                dash_melee_kill,
                (
                    recall_boomerangs,
                    curve_boomerangs,
                    boomerang_physics,
                    boomerang_wall_collision,
                    expire_secondary_boomerangs,
                    drop_fire_trail,
                    boomerang_pyre_collision,
                    chain_ignition.run_if(arena_is(ArenaId::Reliquary)),
                    boomerang_sigil_collision.run_if(arena_is(ArenaId::Crossing)),
                    hit_boomerang_player,
                    fire_trail_kills,
                    chasm_kills.run_if(arena_is(ArenaId::Crossing)),
                    sigil_door_teleport.run_if(arena_is(ArenaId::Reliquary)),
                    catch_boomerangs,
                    throw_boomerangs,
                )
                    .chain(),
                pickup_spawner,
                collect_pickups,
                expire_pickups,
                expire_fire_trail,
                tick_player_timers,
                advance_animation,
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

// ---- Phase 13: lifecycle logging ----

/// Edge-detector that emits `tracing` events when [`MatchState`] or
/// [`MatchScore`] transitions between observable values. Runs in
/// `Update` (NOT `GgrsSchedule`) so it sees the post-rollback
/// authoritative value — emitting from inside the rolled-back schedule
/// would spam the log with one line per resimulation pass.
///
/// Targets:
///   * `two_top::sim::lifecycle` for round/match transitions
///   * `two_top::sim::score`     for kill-credited score bumps
fn log_match_lifecycle_edges(
    state: Res<MatchState>,
    score: Res<MatchScore>,
    frame: Res<FrameCount>,
    mut prev_state: Local<Option<MatchState>>,
    mut prev_score: Local<Option<MatchScore>>,
) {
    if let Some(prev) = *prev_state
        && prev != *state
    {
        tracing::info!(
            target: "two_top::sim::lifecycle",
            frame = frame.0,
            prev = ?prev,
            next = ?*state,
            "match-state transition",
        );
    }
    *prev_state = Some(*state);

    if let Some(prev) = *prev_score
        && prev != *score
    {
        tracing::info!(
            target: "two_top::sim::score",
            frame = frame.0,
            p0 = score.p0,
            p1 = score.p1,
            delta_p0 = (score.p0 as i16 - prev.p0 as i16),
            delta_p1 = (score.p1 as i16 - prev.p1 as i16),
            "score change",
        );
    }
    *prev_score = Some(*score);
}

/// Opt-in plugin: install in the real game binary so round/match/score
/// transitions land in the diagnostic log. Headless ceremonies
/// (sync_test, replay_sync, unit tests) intentionally skip it — the
/// scripted scenarios already know what transitions they trigger and
/// don't benefit from the log noise.
pub struct SimLifecycleLogPlugin;

impl Plugin for SimLifecycleLogPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, log_match_lifecycle_edges);
    }
}

// ---- Phase 16: arenas ----

/// Identifies the active arena for a match. Selected once at match start
/// (loser-bans flow lands later); persists for the whole match.
///
/// Stored as a non-rolled-back `Resource` because the selection is fixed
/// for every tick of the match — rolling it back would be pointless
/// overhead. The arena's per-entity state (BonePyre shatter, etc.) IS
/// rolled back.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum ArenaId {
    /// The neutral arena: open box + one central bone pyre. Tournament
    /// default — even the "neutral" arena has one piece of interactive
    /// cover so every throw cycle has tactical texture.
    #[default]
    Anchor,
    /// Cycle 3: a blood chasm bisecting the arena + an altar sigil per
    /// side that triggers a temporary bone bridge. Empty until then.
    Crossing,
    /// Cycle 4: paired sigil-door teleporters + chain-linked bone pyres.
    /// Empty until then.
    Reliquary,
}

impl ArenaId {
    /// Stable wire encoding for the replay header. Do not renumber —
    /// archived replays decode against these values.
    pub fn as_u8(self) -> u8 {
        match self {
            ArenaId::Anchor => 0,
            ArenaId::Crossing => 1,
            ArenaId::Reliquary => 2,
        }
    }

    /// Decode a replay header's arena byte. Unknown values fall back to the
    /// tournament-default Anchor rather than failing the load.
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => ArenaId::Crossing,
            2 => ArenaId::Reliquary,
            _ => ArenaId::Anchor,
        }
    }
}

#[derive(Resource, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct SelectedArena(pub ArenaId);

/// Cover entity that blocks boomerang line-of-sight until shattered.
/// Boomerangs ricochet off intact pyres exactly the way they ricochet off
/// arena walls. On impact the pyre's `shattered` flag flips true and from
/// that tick on it stops blocking — the boomerang passes through.
///
/// Match-scoped persistence: shattered pyres stay shattered for the rest
/// of the match (combined with floor stains, the arena reads as a visual
/// scoreboard of the match's history).
///
/// The rect is embedded (rather than a separate PositionF + half-extent)
/// because every pyre is a fixed-position arena prop — there's no per-tick
/// movement, just the discrete shatter event. Rolled back via
/// `rollback_component_with_copy` so the shatter is deterministic across
/// resimulation, and checksummed so SyncTest catches any drift.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[require(Rollback)]
pub struct BonePyre {
    pub rect: RectF,
    pub shattered: bool,
    /// Chain group for the Reliquary's linked pyres (0 = unlinked).
    /// Shattering one pyre in a group ignites the rest after
    /// `CHAIN_IGNITION_DELAY_FRAMES`.
    pub chain_group: u8,
    /// Pending chain-ignition frame: `Some(f)` once a group-mate has
    /// shattered; this pyre auto-shatters when `frame >= f`.
    pub chain_delay: Option<u32>,
}

impl BonePyre {
    /// Construct an unlinked, intact pyre at the given rect.
    pub fn intact(rect: RectF) -> Self {
        Self {
            rect,
            shattered: false,
            chain_group: 0,
            chain_delay: None,
        }
    }
}

/// Half-extent of every bone pyre. 24 cm = 48 cm full extent — sized
/// between the player (32 cm) and an arena wall so it reads as cover at
/// the camera's framing distance without dwarfing the players.
pub const BONE_PYRE_HALF_EXTENT_CM: i32 = 24;

/// Per-arena pyre placements. Returned in a fixed deterministic order so
/// entity-id assignment is byte-identical across hosts. All pyres are
/// mirror-symmetric about the arena's center axis (competitive 1v1
/// fairness — players never get an asymmetric advantage).
pub fn arena_pyres_for(arena: ArenaId) -> Vec<BonePyre> {
    let pyre_half = Fix::const_from_int(BONE_PYRE_HALF_EXTENT_CM);
    let square = |cx: i32, cy: i32| {
        RectF::from_center_half_extents(Vec2F::from_cm(cx, cy), Vec2F::new(pyre_half, pyre_half))
    };
    match arena {
        // One central pyre on the y-axis (mirror-symmetric about x=0).
        ArenaId::Anchor => vec![BonePyre::intact(square(0, 0))],
        // The chasm owns the centre; no pyres.
        ArenaId::Crossing => Vec::new(),
        // Two chain-linked pyres flanking the centre: shattering one
        // ignites the other after CHAIN_IGNITION_DELAY_FRAMES.
        ArenaId::Reliquary => vec![
            BonePyre {
                chain_group: 1,
                ..BonePyre::intact(square(-200, 0))
            },
            BonePyre {
                chain_group: 1,
                ..BonePyre::intact(square(200, 0))
            },
        ],
    }
}

/// `GgrsSchedule` system: bounce flying boomerangs off intact bone pyres
/// (same ricochet semantics as `boomerang_wall_collision`) and shatter the
/// pyre on impact. Runs immediately after `boomerang_wall_collision` so a
/// boomerang that ricochets off the arena edge into a pyre resolves the
/// wall first, then the pyre.
///
/// `Returning` boomerangs phase through pyres just as they phase through
/// walls — the recall pull is uncanny by design.
pub fn boomerang_pyre_collision(
    mut pyres: Query<&mut BonePyre>,
    mut boomerangs: Query<(&Boomerang, &BoomerangMods, &mut PositionF, &mut VelocityF)>,
) {
    for (boom, mods, mut pos, mut vel) in &mut boomerangs {
        if matches!(boom.state, BoomerangState::Returning { .. }) {
            continue;
        }
        // Phantom phases through cover too.
        if matches!(mods.modifier, Some(PickupKind::Phantom)) {
            continue;
        }
        // Heavy plows through: it shatters the pyre but doesn't ricochet.
        let heavy = matches!(mods.modifier, Some(PickupKind::Heavy));
        for mut pyre in &mut pyres {
            if pyre.shattered {
                continue;
            }
            let bb = boomerang_rect(pos.0);
            if let Some(push) = resolve_collision(bb, pyre.rect) {
                if !heavy {
                    pos.0 = pos.0 + push;
                    vel.0 = reflect_velocity_for_push(vel.0, push);
                }
                pyre.shattered = true;
            }
        }
    }
}

// ---- Phase 16 cycle 3: Crossing arena (blood chasm + altar bridge) ----

/// Half-width of the Crossing arena's central blood chasm. The chasm is a
/// VERTICAL band on the x-axis (players spawn at `±100,0`, so a horizontal
/// chasm at y=0 would kill them on spawn — the chasm separates the two
/// sides instead). 60 cm leaves a 24 cm gap to each spawn's outer edge.
pub const CHASM_HALF_WIDTH_CM: i32 = 60;

/// How long an altar-sigil hit keeps the bone bridge raised (5 s at 60 Hz).
pub const BRIDGE_DURATION_FRAMES: u32 = 300;

/// Half-extent of an altar sigil (boomerang target that raises the bridge).
pub const ALTAR_SIGIL_HALF_EXTENT_CM: i32 = 24;

/// Rolled-back bridge timer for the Crossing arena. The bone bridge is
/// raised (the chasm is safe to cross) while `frame < active_until_frame`.
/// Rolled back + checksummed because it gates a determinism-affecting kill.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct BridgeState {
    pub active_until_frame: u32,
}

impl BridgeState {
    pub fn is_active(self, frame: u32) -> bool {
        frame < self.active_until_frame
    }
}

/// The Crossing arena's central chasm rect — a vertical band on the x-axis
/// spanning the full arena height.
pub fn crossing_chasm() -> RectF {
    let hw = Fix::const_from_int(CHASM_HALF_WIDTH_CM);
    let hh = Fix::const_from_int(ARENA_HALF_HEIGHT_CM);
    RectF::from_center_half_extents(Vec2F::ZERO, Vec2F::new(hw, hh))
}

/// The two altar sigils — one per side at `±250,0`, reachable by a thrown
/// boomerang. Mirror-symmetric about x=0. Hitting either raises the bridge.
pub fn crossing_sigils() -> Vec<RectF> {
    let h = Fix::const_from_int(ALTAR_SIGIL_HALF_EXTENT_CM);
    let half = Vec2F::new(h, h);
    vec![
        RectF::from_center_half_extents(Vec2F::from_cm(-250, 0), half),
        RectF::from_center_half_extents(Vec2F::from_cm(250, 0), half),
    ]
}

/// `GgrsSchedule` system: kill any player standing in the Crossing chasm
/// while the bridge is down. Environment kill — the opponent is credited,
/// reusing the same dying-flag + respawn path as `hit_boomerang_player`.
/// Boomerangs are untouched (they fly over the chasm freely).
pub fn chasm_kills(
    frame: Res<FrameCount>,
    bridge: Res<BridgeState>,
    match_state: Res<MatchState>,
    mut score: ResMut<MatchScore>,
    mut players: Query<(&Player, &PositionF, &mut Dead)>,
) {
    // Arena gating is a `run_if(arena_is(Crossing))` on the schedule.
    if !match_state.is_in_round() || bridge.is_active(frame.0) {
        return;
    }
    let chasm = crossing_chasm();
    for (player, pos, mut dead) in &mut players {
        if dead.is_dying() {
            continue;
        }
        if chasm.contains(pos.0) {
            // Environment kill — credit the opponent.
            award_kill(&mut dead, 1 - player.handle, frame.0, &mut score);
        }
    }
}

/// `GgrsSchedule` system: ricochet flying boomerangs off the Crossing
/// altar sigils (same reflection as walls/pyres) and raise the bone bridge
/// on impact. Sigils don't shatter — they can be re-triggered all match.
pub fn boomerang_sigil_collision(
    frame: Res<FrameCount>,
    mut bridge: ResMut<BridgeState>,
    mut boomerangs: Query<(&Boomerang, &mut PositionF, &mut VelocityF)>,
) {
    // Arena gating is a `run_if(arena_is(Crossing))` on the schedule.
    for (boom, mut pos, mut vel) in &mut boomerangs {
        if matches!(boom.state, BoomerangState::Returning { .. }) {
            continue;
        }
        for sigil in crossing_sigils() {
            let bb = boomerang_rect(pos.0);
            if let Some(push) = resolve_collision(bb, sigil) {
                pos.0 = pos.0 + push;
                vel.0 = reflect_velocity_for_push(vel.0, push);
                bridge.active_until_frame = frame.0 + BRIDGE_DURATION_FRAMES;
            }
        }
    }
}

// ---- Phase 16 cycle 4: Reliquary arena (sigil doors + chain pyres) ----

/// Cooldown after a sigil-door teleport (1.5 s at 60 Hz) — long enough that
/// you don't instantly bounce back through the paired door you land on.
pub const DOOR_COOLDOWN_FRAMES: u32 = 90;

/// Delay between a chain-linked pyre shattering and its group-mate igniting
/// (1 s at 60 Hz) — the visible "fuse" running between the linked pyres.
pub const CHAIN_IGNITION_DELAY_FRAMES: u32 = 60;

/// Half-extent of a sigil door (the teleport footprint).
pub const SIGIL_DOOR_HALF_EXTENT_CM: i32 = 28;

/// Rolled-back shared cooldown for the Reliquary's paired sigil doors. The
/// doors are inert while `frame < until_frame`.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct DoorCooldown {
    pub until_frame: u32,
}

/// The Reliquary's paired sigil doors as `(footprint, exit)` — stepping on
/// a door's footprint teleports the player to the paired door's position.
/// Mirror-symmetric through the origin (diagonal corners) so neither side
/// owns a positional advantage.
pub fn reliquary_doors() -> Vec<(RectF, Vec2F)> {
    let h = Fix::const_from_int(SIGIL_DOOR_HALF_EXTENT_CM);
    let half = Vec2F::new(h, h);
    let a = Vec2F::from_cm(350, -550);
    let b = Vec2F::from_cm(-350, 550);
    vec![
        (RectF::from_center_half_extents(a, half), b),
        (RectF::from_center_half_extents(b, half), a),
    ]
}

/// `GgrsSchedule` system (Reliquary): teleport a player standing on a sigil
/// door to its paired exit via `snap_position` (no interpolation streak),
/// then set the shared door cooldown. One teleport per tick; the cooldown
/// gates the rest, so you can't bounce back through the door you arrive on.
pub fn sigil_door_teleport(
    frame: Res<FrameCount>,
    match_state: Res<MatchState>,
    mut cooldown: ResMut<DoorCooldown>,
    mut players: Query<(&mut PositionF, &mut PreviousPositionF, &Dead), With<Player>>,
) {
    if !match_state.is_in_round() || frame.0 < cooldown.until_frame {
        return;
    }
    let doors = reliquary_doors();
    for (mut pos, mut prev, dead) in &mut players {
        if dead.is_dying() {
            continue;
        }
        for (footprint, exit) in &doors {
            if footprint.contains(pos.0) {
                snap_position(&mut pos, &mut prev, *exit);
                cooldown.until_frame = frame.0 + DOOR_COOLDOWN_FRAMES;
                return;
            }
        }
    }
}

/// `GgrsSchedule` system: propagate shatter across chain-linked pyres. When
/// any pyre in a group shatters, its intact group-mates arm a fuse and
/// shatter `CHAIN_IGNITION_DELAY_FRAMES` later. Runs right after
/// `boomerang_pyre_collision` so a fresh shatter arms the chain the same
/// tick. A no-op when no pyre carries a chain group (every other arena).
pub fn chain_ignition(frame: Res<FrameCount>, mut pyres: Query<&mut BonePyre>) {
    let mut shattered_groups: BTreeSet<u8> = BTreeSet::new();
    for pyre in &pyres {
        if pyre.shattered && pyre.chain_group != 0 {
            shattered_groups.insert(pyre.chain_group);
        }
    }
    for mut pyre in &mut pyres {
        if pyre.chain_group == 0 || pyre.shattered {
            continue;
        }
        if pyre.chain_delay.is_none() && shattered_groups.contains(&pyre.chain_group) {
            pyre.chain_delay = Some(frame.0 + CHAIN_IGNITION_DELAY_FRAMES);
        }
        if let Some(d) = pyre.chain_delay
            && frame.0 >= d
        {
            pyre.shattered = true;
            pyre.chain_delay = None;
        }
    }
}

/// Run condition: gate an arena-specific system to a single `ArenaId`.
/// Declarative replacement for `if selected.0 != X { return }` early-returns
/// inside the systems.
pub fn arena_is(id: ArenaId) -> impl Fn(Res<SelectedArena>) -> bool + Clone {
    move |selected: Res<SelectedArena>| selected.0 == id
}

// ---- Phase 17: pickups ----

/// Rolled-back deterministic RNG for gameplay randomness (pickup spawns).
/// xorshift64* — portable, no platform-dependent behavior, no float. Per
/// CONVENTIONS this is the ONLY randomness allowed in sim; cosmetics use a
/// separate non-rolled-back RNG on the render side.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SimRng {
    state: u64,
}

impl Default for SimRng {
    fn default() -> Self {
        // Fixed non-zero golden-ratio seed. (Wiring it from ReplayHeader.seed
        // is a follow-up; a constant seed already makes every replay of the
        // same input stream reproduce the same pickup sequence.)
        Self {
            state: 0x9E37_79B9_7F4A_7C15,
        }
    }
}

impl SimRng {
    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }

    /// Uniform integer in `[lo, hi)`. `hi` must be > `lo`.
    pub fn range(&mut self, lo: u32, hi: u32) -> u32 {
        lo + self.next_u32() % (hi - lo)
    }
}

/// The six pickup modifiers. Each changes the one thing you do — the throw —
/// so they deepen the core loop instead of bolting on new verbs.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PickupKind {
    /// Faster fang that drops a lethal fire trail.
    Fire,
    /// Slower fang that plows through cover without ricocheting.
    Heavy,
    /// Gains speed with every wall ricochet.
    Bouncy,
    /// Curves in flight (banana throw).
    Curve,
    /// Throws a 3-fang fan.
    Multishot,
    /// Phases through walls + cover (through-wall snipe).
    Phantom,
}

impl PickupKind {
    pub const ALL: [PickupKind; 6] = [
        PickupKind::Fire,
        PickupKind::Heavy,
        PickupKind::Bouncy,
        PickupKind::Curve,
        PickupKind::Multishot,
        PickupKind::Phantom,
    ];

    pub fn as_u8(self) -> u8 {
        match self {
            PickupKind::Fire => 0,
            PickupKind::Heavy => 1,
            PickupKind::Bouncy => 2,
            PickupKind::Curve => 3,
            PickupKind::Multishot => 4,
            PickupKind::Phantom => 5,
        }
    }
}

/// A pickup waiting on the floor. Rolled back so spawn/collect is rollback-
/// safe; lives on an entity with a fixed `PositionF` at one of the slots.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[require(Rollback)]
pub struct Pickup {
    pub kind: PickupKind,
    /// Which of the four fixed slots this occupies (stable restore key).
    pub slot: u8,
    pub despawn_at_frame: u32,
}

/// The pickup a player is carrying (one at a time; a new pickup replaces it).
/// Moved onto the next thrown boomerang and cleared. `#[require]`d on Player.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[require(Rollback)]
pub struct HeldModifier(pub Option<PickupKind>);

/// Rolled-back timer: the earliest frame the next pickup may appear.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PickupSpawnTimer {
    pub next_at_frame: u32,
}

impl Default for PickupSpawnTimer {
    fn default() -> Self {
        // Don't drop a pickup the instant the round opens — let the first
        // exchange breathe. First pickup lands ~8 s in.
        Self {
            next_at_frame: PICKUP_MIN_INTERVAL_FRAMES,
        }
    }
}

pub const PICKUP_MIN_INTERVAL_FRAMES: u32 = 480;
pub const PICKUP_MAX_INTERVAL_FRAMES: u32 = 720;
pub const PICKUP_LIFETIME_FRAMES: u32 = 360;
pub const PICKUP_HALF_EXTENT_CM: i32 = 16;

/// Four fixed, mirror-symmetric pickup slots (competitive fairness — neither
/// side is closer to a spawn point).
pub fn pickup_slots() -> [Vec2F; 4] {
    [
        Vec2F::from_cm(-250, -400),
        Vec2F::from_cm(250, -400),
        Vec2F::from_cm(-250, 400),
        Vec2F::from_cm(250, 400),
    ]
}

pub fn pickup_rect(pos: Vec2F) -> RectF {
    let h = Fix::const_from_int(PICKUP_HALF_EXTENT_CM);
    RectF::from_center_half_extents(pos, Vec2F::new(h, h))
}

/// `GgrsSchedule` system: spawn at most one pickup at a time on a randomized
/// interval. Deterministic — the spawn frame/slot/kind come from the rolled-
/// back `SimRng`, so two hosts (and a replay) agree exactly.
pub fn pickup_spawner(
    frame: Res<FrameCount>,
    match_state: Res<MatchState>,
    mut rng: ResMut<SimRng>,
    mut timer: ResMut<PickupSpawnTimer>,
    mut commands: Commands,
    existing: Query<&Pickup>,
) {
    if !match_state.is_in_round() || !existing.is_empty() || frame.0 < timer.next_at_frame {
        return;
    }
    let slot = rng.range(0, 4) as usize;
    let kind = PickupKind::ALL[rng.range(0, PickupKind::ALL.len() as u32) as usize];
    commands.spawn((
        Pickup {
            kind,
            slot: slot as u8,
            despawn_at_frame: frame.0 + PICKUP_LIFETIME_FRAMES,
        },
        PositionF(pickup_slots()[slot]),
    ));
    // Earliest frame the *next* pickup may appear. Interval > lifetime, so
    // there's always a gap between one pickup vanishing and the next.
    timer.next_at_frame =
        frame.0 + rng.range(PICKUP_MIN_INTERVAL_FRAMES, PICKUP_MAX_INTERVAL_FRAMES);
}

/// `GgrsSchedule` system: a living player walking over a pickup collects it
/// (filling/replacing its `HeldModifier`) and despawns the pickup.
pub fn collect_pickups(
    mut commands: Commands,
    mut players: Query<(&Dead, &PositionF, &mut HeldModifier), With<Player>>,
    pickups: Query<(Entity, &Pickup, &PositionF)>,
) {
    for (entity, pickup, ppos) in &pickups {
        for (dead, player_pos, mut held) in &mut players {
            if dead.is_dying() {
                continue;
            }
            if player_rect(player_pos.0).overlaps(pickup_rect(ppos.0)) {
                held.0 = Some(pickup.kind);
                commands.entity(entity).despawn();
                break;
            }
        }
    }
}

/// `GgrsSchedule` system: despawn pickups that have sat uncollected past
/// their lifetime.
pub fn expire_pickups(
    frame: Res<FrameCount>,
    mut commands: Commands,
    pickups: Query<(Entity, &Pickup)>,
) {
    for (entity, pickup) in &pickups {
        if frame.0 >= pickup.despawn_at_frame {
            commands.entity(entity).despawn();
        }
    }
}

// ---- Phase 14: SimSnapshot ----

/// In-memory snapshot of every rolled-back sim component + resource at
/// a given frame. Used by `replay_viewer` to scrub backward without
/// re-running the entire prefix from frame 0.
///
/// Snapshots intentionally **don't** serialize: they live entirely in
/// memory for the lifetime of the viewer session. The .bmrg replay
/// format is the on-disk source of truth; snapshots are an
/// optimisation. If a future tool needs persistent snapshots (e.g. a
/// "share this exact frame" feature), add a `Serialize` derive across
/// the relevant types — every component cloned here is already `Copy`
/// + plain-old-data, so the derive is mechanical.
#[derive(Clone, Debug)]
pub struct SimSnapshot {
    pub frame: u32,
    /// One bundle per Player entity. Order isn't significant — restore
    /// re-establishes entities in `handle` order so the spawn sequence
    /// is deterministic.
    pub players: Vec<PlayerSnap>,
    /// One bundle per live Boomerang entity.
    pub boomerangs: Vec<BoomerangSnap>,
    /// One per BonePyre entity, sorted by rect-min for a deterministic
    /// restore order. Captures the shatter state so a backward scrub past
    /// a shatter event correctly un-shatters the pyre.
    pub pyres: Vec<BonePyreSnap>,
    pub frame_count: FrameCount,
    pub match_state: MatchState,
    pub match_score: MatchScore,
    pub bridge: BridgeState,
    pub door_cooldown: DoorCooldown,
    /// One per live floor Pickup, sorted by slot for a deterministic
    /// restore order.
    pub pickups: Vec<PickupSnap>,
    /// One per live fire cell, sorted by (owner, position, expiry) for a
    /// deterministic restore order.
    pub fire_trail: Vec<FireTrailSnap>,
    pub rng: SimRng,
    pub pickup_timer: PickupSpawnTimer,
    pub input_history: InputHistory,
}

/// All rolled-back component values for a single Player entity.
/// `Dead`, `DashState`, `StunFrames`, `AnimState` are `#[require]`d on
/// `Player`, so every Player entity carries a value for each of these —
/// capture pulls them in lockstep.
#[derive(Clone, Copy, Debug)]
pub struct PlayerSnap {
    pub player: Player,
    pub pos: PositionF,
    pub prev_pos: PreviousPositionF,
    pub vel: VelocityF,
    pub dash: DashState,
    pub stun: StunFrames,
    pub dead: Dead,
    pub anim: AnimState,
    pub empowered: Empowered,
    pub held: HeldModifier,
}

/// All rolled-back component values for a single Boomerang entity.
#[derive(Clone, Copy, Debug)]
pub struct BoomerangSnap {
    pub boomerang: Boomerang,
    pub mods: BoomerangMods,
    pub pos: PositionF,
    pub prev_pos: PreviousPositionF,
    pub vel: VelocityF,
}

/// Captured state for a single floor Pickup entity.
#[derive(Clone, Copy, Debug)]
pub struct PickupSnap {
    pub pickup: Pickup,
    pub pos: PositionF,
}

/// Captured state for a single fire-trail cell.
#[derive(Clone, Copy, Debug)]
pub struct FireTrailSnap {
    pub cell: FireTrailCell,
    pub pos: PositionF,
}

/// Captured state for a single BonePyre entity. The position (`rect`) is
/// fixed per arena, so the snapshot just preserves the shatter state for
/// restore — backward-scrub past a shatter event must un-shatter it.
#[derive(Clone, Copy, Debug)]
pub struct BonePyreSnap {
    pub pyre: BonePyre,
}

impl SimSnapshot {
    /// Capture the current sim state into a snapshot. Run from any
    /// schedule — the World is read non-mutably. Call from `Update`
    /// (post-rollback) so the captured state is the authoritative
    /// post-tick value rather than a mid-resimulation intermediate.
    pub fn capture(world: &mut World) -> Self {
        let frame_count = *world.resource::<FrameCount>();
        let match_state = *world.resource::<MatchState>();
        let match_score = *world.resource::<MatchScore>();
        let bridge = *world.resource::<BridgeState>();
        let door_cooldown = *world.resource::<DoorCooldown>();
        let rng = *world.resource::<SimRng>();
        let pickup_timer = *world.resource::<PickupSpawnTimer>();
        let input_history = world.resource::<InputHistory>().clone();

        let mut players: Vec<PlayerSnap> = world
            .query::<(
                &Player,
                &PositionF,
                &PreviousPositionF,
                &VelocityF,
                &DashState,
                &StunFrames,
                &Dead,
                &AnimState,
                &Empowered,
                &HeldModifier,
            )>()
            .iter(world)
            .map(
                |(p, pos, prev, vel, dash, stun, dead, anim, empowered, held)| PlayerSnap {
                    player: *p,
                    pos: *pos,
                    prev_pos: *prev,
                    vel: *vel,
                    dash: *dash,
                    stun: *stun,
                    dead: *dead,
                    anim: *anim,
                    empowered: *empowered,
                    held: *held,
                },
            )
            .collect();
        players.sort_by_key(|s| s.player.handle);

        let mut boomerangs: Vec<BoomerangSnap> = world
            .query::<(
                &Boomerang,
                &BoomerangMods,
                &PositionF,
                &PreviousPositionF,
                &VelocityF,
            )>()
            .iter(world)
            .map(|(b, mods, pos, prev, vel)| BoomerangSnap {
                boomerang: *b,
                mods: *mods,
                pos: *pos,
                prev_pos: *prev,
                vel: *vel,
            })
            .collect();
        // Owner handle alone isn't unique once Multishot spawns several
        // boomerangs per owner — break ties by position bits for a stable
        // total order.
        boomerangs.sort_by_key(|s| {
            (
                s.boomerang.owner_handle,
                s.pos.0.x.to_bits(),
                s.pos.0.y.to_bits(),
            )
        });

        let mut pickups: Vec<PickupSnap> = world
            .query::<(&Pickup, &PositionF)>()
            .iter(world)
            .map(|(p, pos)| PickupSnap {
                pickup: *p,
                pos: *pos,
            })
            .collect();
        pickups.sort_by_key(|s| s.pickup.slot);

        let mut fire_trail: Vec<FireTrailSnap> = world
            .query::<(&FireTrailCell, &PositionF)>()
            .iter(world)
            .map(|(c, pos)| FireTrailSnap {
                cell: *c,
                pos: *pos,
            })
            .collect();
        // Cells have no stable identity; sort by (owner, position, expiry)
        // for a deterministic total order, then reconcile by count on restore.
        fire_trail.sort_by_key(|s| {
            (
                s.cell.owner_handle,
                s.pos.0.x.to_bits(),
                s.pos.0.y.to_bits(),
                s.cell.expires_at_frame,
            )
        });

        let mut pyres: Vec<BonePyreSnap> = world
            .query::<&BonePyre>()
            .iter(world)
            .map(|p| BonePyreSnap { pyre: *p })
            .collect();
        // Deterministic order keyed by rect-min raw bits — pyre positions
        // are unique per arena, so this is a stable total ordering.
        pyres.sort_by_key(|s| (s.pyre.rect.min.x.to_bits(), s.pyre.rect.min.y.to_bits()));

        Self {
            frame: frame_count.0,
            players,
            boomerangs,
            pyres,
            frame_count,
            match_state,
            match_score,
            bridge,
            door_cooldown,
            pickups,
            fire_trail,
            rng,
            pickup_timer,
            input_history,
        }
    }

    /// Restore the sim to this snapshot. Mutates rolled-back component
    /// VALUES in place on existing entities rather than the
    /// despawn-and-respawn pattern that earlier revisions used —
    /// per CONVENTIONS § Component Rules: "No mid-tick
    /// `Commands::insert / remove::<T>()` on existing rollback
    /// entities." Despawn-and-respawn churns each entity's bevy_ggrs
    /// `Rollback` component ID, which desyncs bevy_ggrs's internal
    /// SyncTest verification snapshot machinery (verification rolls
    /// back N ticks and compares checksums; mismatched Rollback IDs
    /// cause silent state divergence on the verification re-runs).
    ///
    /// Players match by handle (always count = 2 in the live game).
    /// Boomerangs match by `owner_handle`; missing ones spawn,
    /// extras despawn. The spawn/despawn for boomerangs is still
    /// necessary because count varies (0..N), but it's restricted
    /// to entities the snapshot legitimately added or removed —
    /// no churn on entities whose VALUE changed.
    ///
    /// The caller is expected to also reset any non-sim resources
    /// (the replay playback cursor, etc.) — `SimSnapshot` only owns
    /// the sim's own state.
    pub fn restore(&self, world: &mut World) {
        // ---- Players: mutate in place by handle ----
        // Players are #[require]'d into existence at app startup and
        // never despawned during normal play, so a snap of 2 players
        // and a world of 2 players is the universal case. Match by
        // `Player.handle` which is the canonical identifier.
        //
        // Defensive: despawn any Player whose handle isn't in the
        // snapshot. Live game has count=2 so this is a no-op, but
        // tests that seed extra Player handles need them removed.
        let snap_handles: BTreeSet<usize> = self.players.iter().map(|s| s.player.handle).collect();
        let extra_players: Vec<Entity> = world
            .query::<(Entity, &Player)>()
            .iter(world)
            .filter(|(_, p)| !snap_handles.contains(&p.handle))
            .map(|(e, _)| e)
            .collect();
        for e in extra_players {
            world.despawn(e);
        }
        for snap in &self.players {
            let target_entity = world
                .query::<(Entity, &Player)>()
                .iter(world)
                .find(|(_, p)| p.handle == snap.player.handle)
                .map(|(e, _)| e);
            if let Some(e) = target_entity {
                let mut entity_mut = world.entity_mut(e);
                if let Some(mut c) = entity_mut.get_mut::<PositionF>() {
                    *c = snap.pos;
                }
                if let Some(mut c) = entity_mut.get_mut::<PreviousPositionF>() {
                    *c = snap.prev_pos;
                }
                if let Some(mut c) = entity_mut.get_mut::<VelocityF>() {
                    *c = snap.vel;
                }
                if let Some(mut c) = entity_mut.get_mut::<DashState>() {
                    *c = snap.dash;
                }
                if let Some(mut c) = entity_mut.get_mut::<StunFrames>() {
                    *c = snap.stun;
                }
                if let Some(mut c) = entity_mut.get_mut::<Dead>() {
                    *c = snap.dead;
                }
                if let Some(mut c) = entity_mut.get_mut::<AnimState>() {
                    *c = snap.anim;
                }
                if let Some(mut c) = entity_mut.get_mut::<Empowered>() {
                    *c = snap.empowered;
                }
                if let Some(mut c) = entity_mut.get_mut::<HeldModifier>() {
                    *c = snap.held;
                }
            } else {
                // Snapshot has a player handle the live world doesn't.
                // Spawn it. Should not happen during normal scrub; the
                // app's startup already placed both Players.
                world.spawn((
                    snap.player,
                    snap.pos,
                    snap.prev_pos,
                    snap.vel,
                    snap.dash,
                    snap.stun,
                    snap.dead,
                    snap.anim,
                    snap.empowered,
                    snap.held,
                ));
            }
        }

        // ---- Boomerangs: reconcile by COUNT, mutate in place ----
        // `owner_handle` is no longer unique (Multishot spawns up to three
        // fangs per owner), and a boomerang has no rollback-stable identity
        // anyway — restore overwrites every component value. So we pair the
        // snapshot entries to existing entities positionally: mutate the
        // overlap in place (keeping bevy_ggrs Rollback IDs stable, the churn
        // 1204aa9 fixed), then spawn or despawn only the count difference.
        // Which specific entity receives which value is irrelevant — all
        // values are overwritten — and restore is a single-machine scrub
        // path (replay_viewer), never part of the cross-platform checksum.
        let world_booms: Vec<Entity> = world
            .query::<(Entity, &Boomerang)>()
            .iter(world)
            .map(|(e, _)| e)
            .collect();
        for (snap, &e) in self.boomerangs.iter().zip(world_booms.iter()) {
            let mut entity_mut = world.entity_mut(e);
            if let Some(mut c) = entity_mut.get_mut::<Boomerang>() {
                *c = snap.boomerang;
            }
            if let Some(mut c) = entity_mut.get_mut::<BoomerangMods>() {
                *c = snap.mods;
            }
            if let Some(mut c) = entity_mut.get_mut::<PositionF>() {
                *c = snap.pos;
            }
            if let Some(mut c) = entity_mut.get_mut::<PreviousPositionF>() {
                *c = snap.prev_pos;
            }
            if let Some(mut c) = entity_mut.get_mut::<VelocityF>() {
                *c = snap.vel;
            }
        }
        // Snapshot had more boomerangs than the world: spawn the remainder.
        for snap in self.boomerangs.iter().skip(world_booms.len()) {
            world.spawn((snap.boomerang, snap.mods, snap.pos, snap.prev_pos, snap.vel));
        }
        // World had more boomerangs than the snapshot: despawn the remainder.
        for &e in world_booms.iter().skip(self.boomerangs.len()) {
            world.despawn(e);
        }

        // ---- Bone pyres: mutate in place by rect key ----
        // Pyre count + positions are fixed per arena (only `shattered`
        // mutates), so this matches the in-place player pattern rather
        // than the despawn/respawn the reverted 58fd4ab used — keeping
        // bevy_ggrs Rollback IDs stable (the churn 1204aa9 fixed). Key on
        // rect-min raw bits, which uniquely identify a pyre within an arena.
        let snap_keys: BTreeSet<(i32, i32)> = self
            .pyres
            .iter()
            .map(|s| (s.pyre.rect.min.x.to_bits(), s.pyre.rect.min.y.to_bits()))
            .collect();
        let extra_pyres: Vec<Entity> = world
            .query::<(Entity, &BonePyre)>()
            .iter(world)
            .filter(|(_, p)| !snap_keys.contains(&(p.rect.min.x.to_bits(), p.rect.min.y.to_bits())))
            .map(|(e, _)| e)
            .collect();
        for e in extra_pyres {
            world.despawn(e);
        }
        for snap in &self.pyres {
            let key = (
                snap.pyre.rect.min.x.to_bits(),
                snap.pyre.rect.min.y.to_bits(),
            );
            let target_entity = world
                .query::<(Entity, &BonePyre)>()
                .iter(world)
                .find(|(_, p)| (p.rect.min.x.to_bits(), p.rect.min.y.to_bits()) == key)
                .map(|(e, _)| e);
            if let Some(e) = target_entity {
                if let Some(mut c) = world.entity_mut(e).get_mut::<BonePyre>() {
                    *c = snap.pyre;
                }
            } else {
                world.spawn(snap.pyre);
            }
        }

        // ---- Pickups: match by slot, spawn missing, despawn extras ----
        // Slot (0..3) uniquely identifies a pickup; count varies 0..1.
        let snap_slots: BTreeSet<u8> = self.pickups.iter().map(|s| s.pickup.slot).collect();
        let extra_pickups: Vec<Entity> = world
            .query::<(Entity, &Pickup)>()
            .iter(world)
            .filter(|(_, p)| !snap_slots.contains(&p.slot))
            .map(|(e, _)| e)
            .collect();
        for e in extra_pickups {
            world.despawn(e);
        }
        for snap in &self.pickups {
            let target = world
                .query::<(Entity, &Pickup)>()
                .iter(world)
                .find(|(_, p)| p.slot == snap.pickup.slot)
                .map(|(e, _)| e);
            if let Some(e) = target {
                let mut em = world.entity_mut(e);
                if let Some(mut c) = em.get_mut::<Pickup>() {
                    *c = snap.pickup;
                }
                if let Some(mut c) = em.get_mut::<PositionF>() {
                    *c = snap.pos;
                }
            } else {
                world.spawn((snap.pickup, snap.pos));
            }
        }

        // ---- Fire-trail cells: reconcile by COUNT, mutate in place ----
        // Cells have no rollback-stable identity (a single owner can have
        // many, and restore overwrites every value), so — exactly like
        // boomerangs — pair snapshot entries to existing entities
        // positionally and spawn/despawn only the count difference, keeping
        // bevy_ggrs Rollback IDs stable for the overlap.
        let world_cells: Vec<Entity> = world
            .query::<(Entity, &FireTrailCell)>()
            .iter(world)
            .map(|(e, _)| e)
            .collect();
        for (snap, &e) in self.fire_trail.iter().zip(world_cells.iter()) {
            let mut em = world.entity_mut(e);
            if let Some(mut c) = em.get_mut::<FireTrailCell>() {
                *c = snap.cell;
            }
            if let Some(mut c) = em.get_mut::<PositionF>() {
                *c = snap.pos;
            }
        }
        for snap in self.fire_trail.iter().skip(world_cells.len()) {
            world.spawn((snap.cell, snap.pos));
        }
        for &e in world_cells.iter().skip(self.fire_trail.len()) {
            world.despawn(e);
        }

        // ---- Resources ----
        *world.resource_mut::<SimRng>() = self.rng;
        *world.resource_mut::<PickupSpawnTimer>() = self.pickup_timer;
        *world.resource_mut::<FrameCount>() = self.frame_count;
        *world.resource_mut::<MatchState>() = self.match_state;
        *world.resource_mut::<MatchScore>() = self.match_score;
        *world.resource_mut::<BridgeState>() = self.bridge;
        *world.resource_mut::<DoorCooldown>() = self.door_cooldown;
        *world.resource_mut::<InputHistory>() = self.input_history.clone();
    }
}
