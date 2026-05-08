//! Phase 5 cross-platform determinism gate.
//!
//! `compute_checksum_tsv` runs a `Replay` through the sim and emits a
//! per-frame, per-component checksum TSV. Two of these TSVs produced on
//! different platforms (linux-x64, linux-aarch64, macos-aarch64) must be
//! byte-identical — that's the verifiable contract that proves the sim is
//! portable.
//!
//! Hashing uses `bevy_ggrs::checksum_hasher` (`SeaHasher`) per CONVENTIONS
//! invariant #7: never `std::hash::DefaultHasher` (random hasher state is
//! non-portable across runs and platforms).

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy_ggrs::GgrsPlugin;
use bevy_ggrs::prelude::*;
use bevy_ggrs::{Session, checksum_hasher};
use core::hash::{Hash, Hasher};
use core::time::Duration;
use fixed_math::Vec2F;
use replay::{
    DEV_SIM_VERSION, FORMAT_VERSION, FrameInputs, MAGIC, Replay, ReplayHeader, ReplayPlayback,
    ReplayPlaybackPlugin,
};
use sim::{
    Boomerang, DashState, GgrsCfg, MatchScore, MatchState, Player, PlayerInput, PositionF,
    PreviousPositionF, SimPlugin, StunFrames, VelocityF, arena_walls,
};
use std::fmt::Write as _;
use std::path::PathBuf;

pub mod fuzz;

pub const TSV_HEADER: &str =
    "frame\ttotal_checksum\tpositionf_part\tvelocityf_part\tdashstate_part\tstunframes_part\tboomerang_part\tmatch_score_part\tmatch_state_part";

// ---- Canonical demo (Phase 5) ----

/// Frame count of the canonical 30 s demo at 60 Hz.
pub const CANONICAL_FRAMES: u32 = 1800;
/// Frames between stick-direction reversals (0.5 s @ 60 Hz). Ten reversals
/// across the demo give the sim both signs of velocity and several
/// zero-crossings of position.
pub const CANONICAL_PERIOD: u32 = 30;
/// Stick magnitude — well below the i8 cap so the input itself is
/// unambiguous on the wire.
pub const CANONICAL_STICK_AMP: i8 = 80;

/// Period between DASH_DOWN edges in the canonical demo (2 s @ 60 Hz).
/// Long enough that each dash fully completes (10 + 20 = 30 frame
/// dash + cooldown) before the next edge fires.
pub const CANONICAL_DASH_PERIOD: u32 = 120;
/// Stick-y amplitude for the secondary axis. Smaller than stick-x so
/// north/south runs don't reach the walls as fast as east/west.
pub const CANONICAL_STICK_Y_AMP: i8 = 60;

/// Frames where a throw/recall/catch cycle takes over from the base
/// movement pattern. Three cycles spread across the 1800-frame demo
/// give the cross-platform matrix three independent shots at catching
/// a boomerang-system desync.
pub const CANONICAL_THROW_CYCLE_STARTS: [u32; 3] = [240, 720, 1200];
/// Length of one throw/recall/catch cycle (~1.3 s). See
/// `throw_cycle_input` for the per-frame breakdown.
pub const CANONICAL_THROW_CYCLE_LEN: u32 = 80;

/// Returns an override input for offsets within a throw cycle, scripting
/// throw → flight → ricochet → recall → catch:
///   * `0..=7`: THROW_DOWN held + stick aimed +x (windup, no AIM_ACTIVE
///     so player can still drift with stick — exercises the
///     run-and-throw flow).
///   * `8`: release THROW_DOWN with stick still aimed → throw fires
///     (rising release edge consumed by `try_throw_direction`).
///   * `9..=38`: idle. Boomerang flies east, hits east wall and
///     ricochets within ~10 ticks at THROW_SPEED 50 cm/tick.
///   * `39`: THROW_DOWN press (rising edge) → `recall_boomerangs`
///     transitions Flying → Returning.
///   * `40..=79`: hold THROW_DOWN. Returning boomerang homes at
///     RECALL_SPEED until `catch_boomerangs` despawns it on overlap.
fn throw_cycle_input(offset: u32) -> PlayerInput {
    match offset {
        0..=7 => PlayerInput {
            stick_x: CANONICAL_STICK_AMP,
            stick_y: 0,
            aim_angle: 0,
            buttons: PlayerInput::THROW_DOWN,
        },
        8 => PlayerInput {
            stick_x: CANONICAL_STICK_AMP,
            stick_y: 0,
            aim_angle: 0,
            buttons: 0,
        },
        9..=38 => PlayerInput::default(),
        39..=79 => PlayerInput {
            stick_x: 0,
            stick_y: 0,
            aim_angle: 0,
            buttons: PlayerInput::THROW_DOWN,
        },
        _ => PlayerInput::default(),
    }
}

/// Returns the cycle offset for `f` if it falls inside any of the
/// throw-cycle windows, else `None`. Cycles do not overlap — see
/// `CANONICAL_THROW_CYCLE_STARTS` placement.
fn throw_cycle_offset(f: u32) -> Option<u32> {
    CANONICAL_THROW_CYCLE_STARTS.iter().find_map(|&start| {
        if f >= start && f < start + CANONICAL_THROW_CYCLE_LEN {
            Some(f - start)
        } else {
            None
        }
    })
}

pub fn canonical_inputs() -> Vec<FrameInputs> {
    (0..CANONICAL_FRAMES)
        .map(|f| {
            // Throw cycles override the base movement pattern entirely
            // for their duration so the boomerang state machine has a
            // clean, predictable input track to drive.
            if let Some(offset) = throw_cycle_offset(f) {
                let p = throw_cycle_input(offset);
                return [p, p];
            }
            let dir_x = if (f / CANONICAL_PERIOD).is_multiple_of(2) {
                CANONICAL_STICK_AMP
            } else {
                -CANONICAL_STICK_AMP
            };
            // y-axis reverses on a phase offset of half the x-period
            // so the players sweep through all four quadrants of the
            // arena instead of just east/west.
            let dir_y = if ((f + CANONICAL_PERIOD / 2) / CANONICAL_PERIOD).is_multiple_of(2) {
                CANONICAL_STICK_Y_AMP
            } else {
                -CANONICAL_STICK_Y_AMP
            };
            // DASH_DOWN as a single-frame edge every CANONICAL_DASH_PERIOD
            // frames. The off-frame returns to 0 so sim's edge-detection
            // sees a clean rising edge.
            let buttons = if f % CANONICAL_DASH_PERIOD == 0 {
                PlayerInput::DASH_DOWN
            } else {
                0
            };
            let p = PlayerInput {
                stick_x: dir_x,
                stick_y: dir_y,
                aim_angle: 0,
                buttons,
            };
            [p, p]
        })
        .collect()
}

pub fn canonical_replay() -> Replay {
    Replay {
        header: ReplayHeader {
            magic: MAGIC,
            format_version: FORMAT_VERSION,
            sim_version: DEV_SIM_VERSION,
            seed: 0,
            num_players: 2,
            frame_rate: 60,
            frame_count: CANONICAL_FRAMES,
            recorded_at: 0,
            winner: None,
            player_handles: [None, None],
            arena_id: 0,
        },
        inputs: canonical_inputs(),
    }
}

/// Path to the committed canonical .bmrg file, relative to the workspace
/// root (two levels up from this crate's manifest).
pub fn canonical_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("tests/demos/canonical/match_v1.bmrg")
}

/// Build a fresh app configured for replay-driven sim with two players at
/// the origin. Mirrors the integration-test setup so that local and CI runs
/// produce comparable state.
fn build_app(replay: Replay) -> App {
    let mut sb = SessionBuilder::<GgrsCfg>::new()
        .with_num_players(replay.header.num_players as usize)
        .expect("with_num_players")
        .with_check_distance(2)
        .with_input_delay(2);
    for i in 0..replay.header.num_players as usize {
        sb = sb.add_player(PlayerType::Local, i).expect("add_player");
    }
    let session = sb.start_synctest_session().expect("synctest");

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
        1.0 / sim::TICK_HZ as f64,
    )));
    app.add_plugins(GgrsPlugin::<GgrsCfg>::default());
    app.add_plugins(SimPlugin);
    app.add_plugins(sim::InfiniteRoundPlugin);
    app.add_plugins(ReplayPlaybackPlugin);
    app.insert_resource(Session::SyncTest(session));

    for handle in 0..replay.header.num_players as usize {
        app.world_mut().spawn((
            Player { handle },
            PositionF(Vec2F::ZERO),
            PreviousPositionF(Vec2F::ZERO),
            VelocityF(Vec2F::ZERO),
        ));
    }

    // Phase 9: spawn arena walls in their canonical fixed order so the
    // `wall_collision` system has geometry to resolve against. Walls
    // aren't rollback subjects (they don't change), so this is a one-
    // shot spawn at app build time.
    for wall in arena_walls() {
        app.world_mut().spawn(wall);
    }

    app.insert_resource(ReplayPlayback::new(replay));
    app
}

/// Compute the per-component checksum for a single component type by
/// iterating entities in `Player.handle` order (the only stable ordering
/// available pre-Phase-9). For each entity we hash the handle then the
/// component value, so a pair of swapped entities cannot collide.
fn hash_component<C: Component + Hash>(world: &mut World) -> u64 {
    let mut rows: Vec<(usize, u64)> = world
        .query::<(&Player, &C)>()
        .iter(world)
        .map(|(p, c)| {
            let mut h = checksum_hasher();
            c.hash(&mut h);
            (p.handle, h.finish())
        })
        .collect();
    rows.sort_by_key(|(h, _)| *h);

    let mut h = checksum_hasher();
    for (handle, part) in &rows {
        handle.hash(&mut h);
        part.hash(&mut h);
    }
    h.finish()
}

/// Per-frame checksum for a single rolled-back resource. Used for
/// `MatchScore` + `MatchState` so a Phase 11 desync (an off-by-one
/// score increment, an early/late state transition) surfaces in the
/// cross-platform matrix rather than getting masked by the rest of
/// the world checksum.
fn hash_resource<R: Resource + Hash>(world: &World) -> u64 {
    let mut h = checksum_hasher();
    world.resource::<R>().hash(&mut h);
    h.finish()
}

/// Per-frame checksum across all live `Boomerang` entities. Hashes
/// (state, position, velocity) per boomerang and sorts by `owner_handle`
/// so two coincident boomerangs from the same player can't collide
/// (they can't — at most one per owner — but the sort makes the
/// invariant explicit). When no boomerangs exist this returns the
/// hasher's empty-state value, which is identical across platforms so
/// the early-game frames stay byte-stable.
fn hash_boomerangs(world: &mut World) -> u64 {
    let mut rows: Vec<(usize, u64)> = world
        .query::<(&Boomerang, &PositionF, &VelocityF)>()
        .iter(world)
        .map(|(b, pos, vel)| {
            let mut h = checksum_hasher();
            b.hash(&mut h);
            pos.hash(&mut h);
            vel.hash(&mut h);
            (b.owner_handle, h.finish())
        })
        .collect();
    rows.sort_by_key(|(h, _)| *h);

    let mut h = checksum_hasher();
    for (handle, part) in &rows {
        handle.hash(&mut h);
        part.hash(&mut h);
    }
    h.finish()
}

/// Run `replay` through the sim and return a TSV with one header row plus
/// one row per simulated frame. The string is ASCII and ends with a final
/// newline so byte-by-byte cross-platform diff is unambiguous.
pub fn compute_checksum_tsv(replay: &Replay) -> String {
    let frames = replay.header.frame_count;
    let mut app = build_app(replay.clone());

    let mut out = String::new();
    out.push_str(TSV_HEADER);
    out.push('\n');

    for frame in 0..frames {
        app.update();
        let world = app.world_mut();
        let pos_part = hash_component::<PositionF>(world);
        let vel_part = hash_component::<VelocityF>(world);
        let dash_part = hash_component::<DashState>(world);
        let stun_part = hash_component::<StunFrames>(world);
        let boom_part = hash_boomerangs(world);
        let score_part = hash_resource::<MatchScore>(world);
        let state_part = hash_resource::<MatchState>(world);
        let total =
            pos_part ^ vel_part ^ dash_part ^ stun_part ^ boom_part ^ score_part ^ state_part;
        writeln!(
            &mut out,
            "{frame}\t{total:016x}\t{pos_part:016x}\t{vel_part:016x}\t{dash_part:016x}\t{stun_part:016x}\t{boom_part:016x}\t{score_part:016x}\t{state_part:016x}"
        )
        .expect("write tsv row");
    }

    out
}

/// Run `replay` to the requested frame and return a human-readable dump of
/// every entity's state. Used by `diagnose_desync.sh` to print sides of a
/// divergence. Format: one comment header line, then one line per entity in
/// `Player.handle` order with `pos=` and `vel=` columns showing both the
/// fixed-bit representation and the float approximation.
pub fn dump_state_at(replay: &Replay, frame: u32) -> String {
    let mut app = build_app(replay.clone());
    let target = frame.min(replay.header.frame_count.saturating_sub(1));
    for _ in 0..=target {
        app.update();
    }

    let world = app.world_mut();
    let mut rows: Vec<(usize, PositionF, VelocityF)> = world
        .query::<(&Player, &PositionF, &VelocityF)>()
        .iter(world)
        .map(|(p, pos, vel)| (p.handle, *pos, *vel))
        .collect();
    rows.sort_by_key(|(h, _, _)| *h);

    let mut boom_rows: Vec<(usize, Boomerang, PositionF, VelocityF)> = world
        .query::<(&Boomerang, &PositionF, &VelocityF)>()
        .iter(world)
        .map(|(b, pos, vel)| (b.owner_handle, *b, *pos, *vel))
        .collect();
    boom_rows.sort_by_key(|(h, _, _, _)| *h);

    let mut out = String::new();
    writeln!(&mut out, "# replay_sync state dump @ frame {frame}").unwrap();
    for (h, pos, vel) in rows {
        let (px, py) = pos.0.to_f32();
        let (vx, vy) = vel.0.to_f32();
        writeln!(
            &mut out,
            "handle={h}\tpos=({:#010x},{:#010x})/({px:.6},{py:.6})\tvel=({:#010x},{:#010x})/({vx:.6},{vy:.6})",
            pos.0.x.to_bits(),
            pos.0.y.to_bits(),
            vel.0.x.to_bits(),
            vel.0.y.to_bits(),
        )
        .unwrap();
    }
    for (owner, boom, pos, vel) in boom_rows {
        let (px, py) = pos.0.to_f32();
        let (vx, vy) = vel.0.to_f32();
        writeln!(
            &mut out,
            "boomerang owner={owner} state={:?}\tpos=({:#010x},{:#010x})/({px:.6},{py:.6})\tvel=({:#010x},{:#010x})/({vx:.6},{vy:.6})",
            boom.state,
            pos.0.x.to_bits(),
            pos.0.y.to_bits(),
            vel.0.x.to_bits(),
            vel.0.y.to_bits(),
        )
        .unwrap();
    }
    out
}
