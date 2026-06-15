//! Property tests for the AnimState state machine.
//!
//! `next_anim_state` is the pure helper that
//! `sim::advance_animation` delegates to. Testing the pure helper
//! lets us drive arbitrary observable-state inputs through the
//! priority ladder without spinning up a Bevy World per case.
//!
//! The properties below check structural invariants that hold for
//! ANY input combination — bugs that violate these surface here as
//! failing proptest minimizations rather than as nightly fuzz
//! desyncs hours into a soak.

use fixed_math::Vec2F;
use proptest::prelude::*;
use sim::{AnimState, DashState, Dead, StunFrames, next_anim_state};

fn dead_strategy() -> impl Strategy<Value = Dead> {
    proptest::option::of(any::<u32>())
        .prop_map(|maybe_at| Dead { respawn_at_frame: maybe_at })
}

fn dash_strategy() -> impl Strategy<Value = DashState> {
    prop_oneof![
        Just(DashState::Idle),
        any::<u32>().prop_map(|f| DashState::Dashing {
            frames_remaining: f,
            // Dir doesn't affect the anim state machine — `Dashing`
            // is the only thing it pattern-matches on. Pick a
            // canonical non-zero vector to keep the strategy simple.
            dir: Vec2F::from_cm(1, 0),
        }),
        any::<u32>().prop_map(|f| DashState::Cooldown { frames_remaining: f }),
    ]
}

fn stun_strategy() -> impl Strategy<Value = StunFrames> {
    any::<u32>().prop_map(StunFrames)
}

fn anim_strategy() -> impl Strategy<Value = AnimState> {
    // Constrain anim_id to the valid range [0, 6] — outside values
    // are programmer errors, not legitimate input the state machine
    // needs to handle.
    (0u8..=6, any::<u16>()).prop_map(|(anim_id, ticks)| AnimState { anim_id, ticks })
}

proptest! {
    /// Pure-function determinism: same inputs MUST produce same outputs
    /// across two independent calls. Catches accidental introduction
    /// of any global / static / RNG state into the helper.
    #[test]
    fn next_anim_is_deterministic(
        dead in dead_strategy(),
        dash in dash_strategy(),
        stun in stun_strategy(),
        current in anim_strategy(),
        is_moving in any::<bool>(),
    ) {
        let a = next_anim_state(dead, dash, stun, current, is_moving);
        let b = next_anim_state(dead, dash, stun, current, is_moving);
        prop_assert_eq!(a, b);
    }

    /// `Dead.is_dying()` always pre-empts anything else. Once dying,
    /// the next anim_id is DEATH (no exceptions, including from inside
    /// a HIT or THROW one-shot).
    #[test]
    fn death_priority_overrides_all(
        respawn_at in any::<u32>(),
        dash in dash_strategy(),
        stun in stun_strategy(),
        current in anim_strategy(),
        is_moving in any::<bool>(),
    ) {
        let dead = Dead { respawn_at_frame: Some(respawn_at) };
        let next = next_anim_state(dead, dash, stun, current, is_moving);
        prop_assert_eq!(next.anim_id, AnimState::DEATH);
    }

    /// `StunFrames > 0` (while alive) pre-empts everything except
    /// DEATH. A non-zero stun while alive should always land on HIT.
    #[test]
    fn hit_priority_overrides_dash_and_idle_and_throw(
        dash in dash_strategy(),
        stun_amount in 1u32..,  // strictly positive
        current in anim_strategy(),
        is_moving in any::<bool>(),
    ) {
        let dead = Dead { respawn_at_frame: None };
        let next = next_anim_state(dead, dash, StunFrames(stun_amount), current, is_moving);
        prop_assert_eq!(next.anim_id, AnimState::HIT);
    }

    /// Transitions reset `ticks` to 0. If the next anim_id differs
    /// from the current one, `ticks` MUST be 0 — animation frames
    /// always restart from the beginning of their atlas range. A bug
    /// that incremented ticks across a transition would visually
    /// skip the first few frames of the new anim.
    #[test]
    fn transition_resets_ticks(
        dead in dead_strategy(),
        dash in dash_strategy(),
        stun in stun_strategy(),
        current in anim_strategy(),
        is_moving in any::<bool>(),
    ) {
        let next = next_anim_state(dead, dash, stun, current, is_moving);
        if next.anim_id != current.anim_id {
            prop_assert_eq!(next.ticks, 0);
        }
    }

    /// Same-anim continuation increments ticks by exactly 1
    /// (saturating at u16::MAX). Any other delta is a bug — a
    /// per-tick anim accumulator that jumped 2 ticks would burn
    /// frames at 2x the intended rate.
    #[test]
    fn continuation_increments_ticks_by_one(
        dead in dead_strategy(),
        dash in dash_strategy(),
        stun in stun_strategy(),
        current in anim_strategy(),
        is_moving in any::<bool>(),
    ) {
        let next = next_anim_state(dead, dash, stun, current, is_moving);
        if next.anim_id == current.anim_id {
            prop_assert_eq!(next.ticks, current.ticks.saturating_add(1));
        }
    }

    /// One-shot anim (THROW/HIT/DEATH/CATCH) that hasn't finished should
    /// continue regardless of underlying observable state — UNLESS
    /// preempted by a higher-priority rule (DEATH, then HIT).
    /// Enforce: if the current is THROW and not finished, and the
    /// entity is alive + unstunned, the next must still be THROW.
    #[test]
    fn throw_one_shot_runs_to_completion_when_unpreempted(
        dash in dash_strategy(),
        is_moving in any::<bool>(),
        ticks in 0u16..(AnimState::frame_count(AnimState::THROW)
            * AnimState::ticks_per_frame(AnimState::THROW)),
    ) {
        let current = AnimState { anim_id: AnimState::THROW, ticks };
        let dead = Dead { respawn_at_frame: None };
        let next = next_anim_state(dead, dash, StunFrames(0), current, is_moving);
        prop_assert_eq!(next.anim_id, AnimState::THROW);
    }

    /// Display index always returns a value within the player sprite
    /// sheet's 41-frame atlas range (0..=40). A bug that produced
    /// out-of-range indices would crash the sprite renderer or
    /// silently clip to a wrong frame.
    #[test]
    fn display_index_in_atlas_range(anim in anim_strategy()) {
        let idx = anim.display_index();
        prop_assert!(idx < AnimState::TOTAL_ATLAS_FRAMES,
            "display_index {} out of 41-frame atlas range for anim_id={} ticks={}",
            idx, anim.anim_id, anim.ticks);
    }
}

// ---- Targeted (non-property) regression tests ----

#[test]
fn idle_alive_unstunned_not_dashing_lands_idle() {
    let next = next_anim_state(
        Dead { respawn_at_frame: None },
        DashState::Idle,
        StunFrames(0),
        AnimState { anim_id: AnimState::IDLE, ticks: 7 },
        false, // not moving
    );
    assert_eq!(next.anim_id, AnimState::IDLE);
    assert_eq!(next.ticks, 8); // continuation
}

#[test]
fn run_selected_when_moving() {
    let next = next_anim_state(
        Dead { respawn_at_frame: None },
        DashState::Idle,
        StunFrames(0),
        AnimState { anim_id: AnimState::IDLE, ticks: 7 },
        true, // moving
    );
    assert_eq!(next.anim_id, AnimState::RUN);
    assert_eq!(next.ticks, 0); // transition from IDLE -> RUN resets
}

#[test]
fn run_continues_when_moving() {
    let next = next_anim_state(
        Dead { respawn_at_frame: None },
        DashState::Idle,
        StunFrames(0),
        AnimState { anim_id: AnimState::RUN, ticks: 5 },
        true,
    );
    assert_eq!(next.anim_id, AnimState::RUN);
    assert_eq!(next.ticks, 6);
}

#[test]
fn run_to_idle_when_stopped() {
    let next = next_anim_state(
        Dead { respawn_at_frame: None },
        DashState::Idle,
        StunFrames(0),
        AnimState { anim_id: AnimState::RUN, ticks: 5 },
        false,
    );
    assert_eq!(next.anim_id, AnimState::IDLE);
    assert_eq!(next.ticks, 0);
}

#[test]
fn catch_one_shot_runs_to_completion() {
    let current = AnimState { anim_id: AnimState::CATCH, ticks: 0 };
    let next = next_anim_state(
        Dead { respawn_at_frame: None },
        DashState::Idle,
        StunFrames(0),
        current,
        true,
    );
    assert_eq!(next.anim_id, AnimState::CATCH);
    assert_eq!(next.ticks, 1);
}

#[test]
fn catch_finished_yields_to_run_when_moving() {
    // CATCH: 3 frames × 3 ticks/frame = 9 total ticks
    let current = AnimState { anim_id: AnimState::CATCH, ticks: 9 };
    assert!(current.is_finished());
    let next = next_anim_state(
        Dead { respawn_at_frame: None },
        DashState::Idle,
        StunFrames(0),
        current,
        true,
    );
    assert_eq!(next.anim_id, AnimState::RUN);
    assert_eq!(next.ticks, 0);
}

#[test]
fn catch_finished_yields_to_idle_when_stationary() {
    let current = AnimState { anim_id: AnimState::CATCH, ticks: 9 };
    assert!(current.is_finished());
    let next = next_anim_state(
        Dead { respawn_at_frame: None },
        DashState::Idle,
        StunFrames(0),
        current,
        false,
    );
    assert_eq!(next.anim_id, AnimState::IDLE);
    assert_eq!(next.ticks, 0);
}

#[test]
fn fresh_dash_transition_resets_ticks() {
    let next = next_anim_state(
        Dead { respawn_at_frame: None },
        DashState::Dashing {
            frames_remaining: 10,
            dir: Vec2F::from_cm(1, 0),
        },
        StunFrames(0),
        AnimState { anim_id: AnimState::IDLE, ticks: 100 },
        false,
    );
    assert_eq!(next.anim_id, AnimState::DASH);
    assert_eq!(next.ticks, 0);
}

#[test]
fn hit_preempts_throw_one_shot() {
    let next = next_anim_state(
        Dead { respawn_at_frame: None },
        DashState::Idle,
        StunFrames(6),
        // Mid-throw — not finished
        AnimState { anim_id: AnimState::THROW, ticks: 4 },
        false,
    );
    assert_eq!(next.anim_id, AnimState::HIT, "HIT must preempt mid-flight THROW");
    assert_eq!(next.ticks, 0);
}

#[test]
fn death_caps_at_last_frame_via_display_index() {
    // Walk DEATH ticks past completion — display_index should
    // saturate at the corpse-mark frame, never overflow.
    let total_ticks = AnimState::frame_count(AnimState::DEATH) as u32
        * AnimState::ticks_per_frame(AnimState::DEATH) as u32;
    let anim = AnimState {
        anim_id: AnimState::DEATH,
        ticks: (total_ticks * 4).min(u16::MAX as u32) as u16,
    };
    let idx = anim.display_index();
    let last = AnimState::atlas_offset(AnimState::DEATH)
        + AnimState::frame_count(AnimState::DEATH).saturating_sub(1);
    assert_eq!(idx, last, "DEATH display_index past completion must cap at corpse mark");
}

/// Priority ladder: DEATH > HIT > in-flight oneshot > DASH > RUN > IDLE
#[test]
fn priority_death_gt_hit_gt_oneshot_gt_dash_gt_run_gt_idle() {
    let alive = Dead { respawn_at_frame: None };
    let dying = Dead { respawn_at_frame: Some(100) };

    // DEATH > everything
    assert_eq!(
        next_anim_state(dying, DashState::Dashing { frames_remaining: 5, dir: Vec2F::from_cm(1, 0) }, StunFrames(3), AnimState { anim_id: AnimState::THROW, ticks: 1 }, true).anim_id,
        AnimState::DEATH
    );

    // HIT > in-flight oneshot
    assert_eq!(
        next_anim_state(alive, DashState::Idle, StunFrames(1), AnimState { anim_id: AnimState::CATCH, ticks: 1 }, true).anim_id,
        AnimState::HIT
    );

    // In-flight oneshot > DASH
    assert_eq!(
        next_anim_state(alive, DashState::Dashing { frames_remaining: 5, dir: Vec2F::from_cm(1, 0) }, StunFrames(0), AnimState { anim_id: AnimState::THROW, ticks: 1 }, true).anim_id,
        AnimState::THROW
    );

    // DASH > RUN
    assert_eq!(
        next_anim_state(alive, DashState::Dashing { frames_remaining: 5, dir: Vec2F::from_cm(1, 0) }, StunFrames(0), AnimState { anim_id: AnimState::IDLE, ticks: 0 }, true).anim_id,
        AnimState::DASH
    );

    // RUN > IDLE (when moving)
    assert_eq!(
        next_anim_state(alive, DashState::Idle, StunFrames(0), AnimState { anim_id: AnimState::IDLE, ticks: 0 }, true).anim_id,
        AnimState::RUN
    );

    // IDLE when not moving
    assert_eq!(
        next_anim_state(alive, DashState::Idle, StunFrames(0), AnimState { anim_id: AnimState::RUN, ticks: 5 }, false).anim_id,
        AnimState::IDLE
    );
}
