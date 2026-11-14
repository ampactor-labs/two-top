//! `TWOTOP_STATE_LOG=<path>`: every simulated frame's inputs and state,
//! one line per frame, appended at the end of the rollback schedule.
//!
//! For chasing a desync between two peers: run both with the knob, keep
//! the LAST line per frame (a rollback re-simulates frames, and the last
//! pass is the one that stood), and diff — the first frame that differs,
//! and the first field in it, is where the two worlds parted. Dev only;
//! nothing is written without the variable.

use std::io::Write;

use bevy::prelude::*;
use bevy_ggrs::PlayerInputs;
use sim::{
    AnimState, Boomerang, BoomerangMods, CatchStreak, DashState, Dead, Empowered, FrameCount,
    GgrsCfg, HeldModifier, InputHistory, MatchScore, MatchState, OobTimer, Player, PositionF,
    SimRng, SpawnGuard, StunFrames, Taunt, ThrowCapacity, ThrowCharge, VelocityF,
};

/// The open log, if the knob is set.
struct Sink(Option<std::fs::File>);

impl Default for Sink {
    fn default() -> Self {
        let file = std::env::var_os("TWOTOP_STATE_LOG").and_then(|path| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok()
        });
        Sink(file)
    }
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn log_state(
    frame: Res<FrameCount>,
    state: Res<MatchState>,
    score: Res<MatchScore>,
    rng: Res<SimRng>,
    history: Res<InputHistory>,
    inputs: Option<Res<PlayerInputs<GgrsCfg>>>,
    players: Query<(
        &Player,
        (
            &PositionF,
            &VelocityF,
            &DashState,
            &StunFrames,
            &Dead,
            &AnimState,
        ),
        (
            &ThrowCharge,
            &ThrowCapacity,
            &CatchStreak,
            &Empowered,
            &HeldModifier,
        ),
        (&Taunt, &SpawnGuard, &OobTimer),
    )>,
    fangs: Query<(&Boomerang, &BoomerangMods, &PositionF, &VelocityF)>,
    ordered: Res<bevy_ggrs::RollbackOrdered>,
    ids: Query<(
        &bevy_ggrs::RollbackId,
        Option<&Player>,
        Option<&Boomerang>,
        Option<&Name>,
    )>,
    mut sink: Local<Sink>,
) {
    let Some(file) = sink.0.as_mut() else {
        return;
    };
    let mut players: Vec<_> = players.iter().collect();
    players.sort_by_key(|(p, ..)| p.handle);
    let mut fangs: Vec<String> = fangs.iter().map(|f| format!("{f:?}")).collect();
    fangs.sort();
    // The input values only: their status (confirmed or predicted)
    // legitimately differs between the two peers.
    let inputs = inputs
        .map(|i| {
            format!(
                "{:?}",
                i.iter().map(|(input, _)| *input).collect::<Vec<_>>()
            )
        })
        .unwrap_or_default();
    // The rollback order bevy_ggrs mixes into every checksum: the number
    // of ids ever registered, and the index of each live player and fang.
    let mut order: Vec<String> = ids
        .iter()
        .map(|(id, player, fang, _)| {
            let what = match (player, fang) {
                (Some(p), _) => format!("p{}", p.handle),
                (_, Some(b)) => format!("f{}", b.owner_handle),
                _ => "other".to_string(),
            };
            format!("{what}@{}", ordered.order(*id))
        })
        .collect();
    order.sort();
    let _ = writeln!(
        file,
        "{}\tin={inputs}\tst={:?}\tsc={:?}\trng={:?}\thist={:?}\tplayers={players:?}\tfangs={fangs:?}\tro_len={}\torder={order:?}",
        frame.0,
        *state,
        *score,
        *rng,
        *history,
        ordered.len(),
    );
}

pub struct StateLogPlugin;

impl Plugin for StateLogPlugin {
    fn build(&self, app: &mut App) {
        if std::env::var_os("TWOTOP_STATE_LOG").is_none() {
            return;
        }
        app.add_systems(
            bevy_ggrs::GgrsSchedule,
            log_state.after(sim::advance_input_history),
        );
    }
}
