//! Honest kill beats: a kill is announced once, when it can no longer be
//! rolled back.
//!
//! Online, a phone simulates ticks it has not heard the other player's
//! input for yet. A kill on such a tick can be undone when the real input
//! arrives, and every irreversible beat that fired on it (the sting, the
//! buzz, the stain, the score pip slamming in) was a lie the player saw.
//! Small, instant feedback (the burst, the flash, the shake) stays on the
//! raw edge: it reads as "something hit" and fades on its own. Everything
//! that says "a kill happened" listens for [`KillConfirmed`] instead.
//!
//! No log is needed. `Dead::respawn_at_frame` carries the tick of the kill
//! (`kill tick + RESPAWN_FRAMES`), and the world the renderer sees is
//! already rollback-corrected: a mispredicted kill simply stops being in
//! it. So each frame [`confirm_kills`] notes every death in the live world
//! and announces it once its tick is final ([`FactHorizon`]) and the death
//! is still there.

use bevy::prelude::*;
use sim::{Dead, FrameCount, Player, RESPAWN_FRAMES};

/// The set [`confirm_kills`] announces in: consumers of [`KillConfirmed`]
/// order themselves `.after(KillFacts)` to hear a kill the frame it lands.
#[derive(bevy::ecs::schedule::SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct KillFacts;

/// A kill that can no longer be undone.
#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub struct KillConfirmed {
    /// Who died.
    pub victim: usize,
    /// The sim tick it happened on.
    pub tick: u32,
    /// Where (world units, as drawn on the tick it was first seen).
    pub pos: Vec2,
}

/// How many sim ticks are final: ticks `0..horizon` can no longer change.
/// `None` (offline, the theater, the viewer): everything simulated is
/// final. Online the netplay driver sets it every frame from the confirmed
/// frame.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct FactHorizon(pub Option<u32>);

/// Per seat: the death currently in the world (its kill tick and where it
/// was first seen), and the last kill tick already announced.
#[derive(Resource, Default, Debug)]
pub struct KillWatch {
    seen: [Option<(u32, Vec2)>; 2],
    announced: [Option<u32>; 2],
}

impl KillWatch {
    /// Has the death of `seat` that is in the world now been announced?
    /// (The respawn cue plays only for a death that was.)
    pub fn announced(&self, seat: usize) -> bool {
        matches!((self.seen[seat], self.announced[seat]), (Some((k, _)), Some(a)) if k == a)
    }
}

/// `Update`: note deaths, announce the final ones. Pure core in
/// [`step_watch`].
pub fn confirm_kills(
    frame: Res<FrameCount>,
    horizon: Res<FactHorizon>,
    players: Query<(&Player, &Dead, &Transform)>,
    mut watch: ResMut<KillWatch>,
    mut out: MessageWriter<KillConfirmed>,
) {
    let mut now = [None; 2];
    for (player, dead, xform) in &players {
        if let (Some(at), Some(slot)) = (dead.respawn_at_frame, now.get_mut(player.handle)) {
            *slot = Some((
                at.saturating_sub(RESPAWN_FRAMES),
                xform.translation.truncate(),
            ));
        }
    }
    let final_ticks = horizon.0.unwrap_or(frame.0);
    for fact in step_watch(&mut watch, now, final_ticks, frame.0) {
        out.write(fact);
    }
}

/// One frame of the watch: `now` is each seat's death in the world (kill
/// tick, position), `final_ticks` how many ticks are final, `frame` where
/// the world is. Returns the kills to announce. Pure.
pub fn step_watch(
    watch: &mut KillWatch,
    now: [Option<(u32, Vec2)>; 2],
    final_ticks: u32,
    frame: u32,
) -> Vec<KillConfirmed> {
    let mut out = Vec::new();
    for (seat, now) in now.into_iter().enumerate() {
        // Keep the position from when the death was first seen: the corpse
        // is where the kill landed, whatever the body does after.
        watch.seen[seat] = match (watch.seen[seat], now) {
            (Some((k, pos)), Some((k2, _))) if k == k2 => Some((k, pos)),
            (_, fresh) => fresh,
        };
        // The world went back before an announced kill (a theater seek):
        // it may be announced again when play reaches it.
        if watch.announced[seat].is_some_and(|a| a >= frame) {
            watch.announced[seat] = None;
        }
        if let Some((tick, pos)) = watch.seen[seat]
            && tick < final_ticks
            && watch.announced[seat] != Some(tick)
        {
            watch.announced[seat] = Some(tick);
            out.push(KillConfirmed {
                victim: seat,
                tick,
                pos,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: Vec2 = Vec2::new(10.0, 20.0);

    #[test]
    fn a_kill_is_announced_once_when_its_tick_is_final() {
        let mut w = KillWatch::default();
        let dead = [None, Some((100, AT))];
        // Seen but not final yet: nothing.
        assert!(step_watch(&mut w, dead, 98, 104).is_empty());
        assert!(step_watch(&mut w, dead, 100, 105).is_empty());
        // Final: once.
        let facts = step_watch(&mut w, dead, 101, 106);
        assert_eq!(
            facts,
            vec![KillConfirmed {
                victim: 1,
                tick: 100,
                pos: AT
            }]
        );
        assert!(w.announced(1));
        assert!(step_watch(&mut w, dead, 140, 150).is_empty());
    }

    #[test]
    fn a_mispredicted_kill_is_never_announced() {
        let mut w = KillWatch::default();
        assert!(step_watch(&mut w, [Some((100, AT)), None], 97, 103).is_empty());
        // The rollback undid it before its tick was final.
        assert!(step_watch(&mut w, [None, None], 110, 113).is_empty());
        assert!(!w.announced(0));
    }

    #[test]
    fn the_position_is_where_the_death_was_first_seen() {
        let mut w = KillWatch::default();
        step_watch(&mut w, [Some((100, AT)), None], 0, 101);
        let facts = step_watch(&mut w, [Some((100, Vec2::ZERO)), None], 200, 201);
        assert_eq!(facts[0].pos, AT);
    }

    #[test]
    fn offline_everything_simulated_is_final() {
        let mut w = KillWatch::default();
        // Horizon = the frame count: tick 100 is final once frame is 101.
        assert_eq!(
            step_watch(&mut w, [Some((100, AT)), None], 101, 101).len(),
            1
        );
    }

    #[test]
    fn seeking_back_before_a_kill_lets_it_be_announced_again() {
        let mut w = KillWatch::default();
        assert_eq!(
            step_watch(&mut w, [Some((100, AT)), None], 150, 150).len(),
            1
        );
        // The theater seeks to tick 50: the death is gone from the world.
        assert!(step_watch(&mut w, [None, None], 50, 50).is_empty());
        // Play reaches it again.
        assert_eq!(
            step_watch(&mut w, [Some((100, AT)), None], 102, 102).len(),
            1
        );
    }
}
