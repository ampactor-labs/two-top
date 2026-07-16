//! Practice bot — a solo opponent on any build, any platform.
//!
//! The single biggest gap between "netplay works" and "this is a game you
//! can hand to someone": being able to play *alone*. In practice mode the
//! local session runs as usual (same SyncTest couch path, same sim, same
//! determinism story) and this module supplies handle 1's inputs — the bot
//! is just another input source, so nothing about rollback, recording, or
//! the sim changes at all. Replays of practice matches record the bot's
//! inputs like anyone else's.
//!
//! The policy is a readable duelist, not an aimbot: it keeps throwing
//! range, orbits, PLANTS visibly before it throws (AIM_ACTIVE during the
//! wind-up — the same telegraph a human shows), dashes through incoming
//! fangs, steers its recalls, walks to its dropped fang, and respects the
//! island edge and the sudden-death crumble. Deliberate imperfections
//! (aim wobble, fixed decision cadence) keep it beatable.
//!
//! Wiring: the platform input source inserts `LocalInputs` via `Commands`;
//! the bot system is ordered after it and QUEUES a world closure, so its
//! patch applies to the freshly-inserted map before ggrs reads it.

use bevy::prelude::*;
use bevy_ggrs::LocalInputs;
use sim::{
    ARENA_HALF_HEIGHT_CM, ARENA_HALF_WIDTH_CM, Boomerang, BoomerangMods, BoomerangState,
    CHARGE_MAX_FRAMES, DashState, Dead, FrameCount, GgrsCfg, MatchState, Player, PlayerInput,
    PositionF, ThrowCapacity, ThrowCharge, VelocityF, sudden_death_factor,
};

/// Practice mode: a local match against the bot (forces a local session
/// even on online builds; `start_matchbox` checks it).
#[derive(Resource, Default)]
pub struct PracticeMode(pub bool);

/// The handle the bot drives (the far/top duelist; the human is handle 0).
pub const BOT_HANDLE: usize = 1;

/// Everything the policy looks at, in plain f32 (inputs are not sim state —
/// they only become deterministic once they enter the wire pipeline).
#[derive(Debug, Clone, Copy, Default)]
pub struct BotView {
    pub frame: u32,
    pub me: Vec2,
    pub foe: Vec2,
    pub foe_alive: bool,
    pub my_charge: u32,
    /// Primary fangs the bot has out (0 = free to throw).
    pub fangs_out: u32,
    /// The bot's own fang in flight/returning: (pos, is_returning).
    pub my_fang: Option<(Vec2, bool)>,
    /// The bot's dropped fang, if any.
    pub my_loose: Option<Vec2>,
    /// Nearest lethal enemy fang: (pos, vel).
    pub threat: Option<(Vec2, Vec2)>,
    pub can_dash: bool,
    /// Current safe half-extents (sudden-death aware).
    pub bounds: Vec2,
    /// Practice difficulty = how many kills the PLAYER has landed on the bot
    /// so far this match (0..=4). 0 = passive sparring dummy; it ramps up one
    /// notch each time the player scores.
    pub difficulty: u32,
}

/// Preferred dueling range (cm). Inside it the bot backs off, outside it
/// closes — with an orbit component so it never runs a straight line.
const PREFERRED_RANGE: f32 = 440.0;
/// Ticks of visible plant (AIM_ACTIVE) before the release.
const PLANT_TICKS: u32 = 5;

// ---- Difficulty ramp (practice mode) ----
// The bot starts as a passive dummy and sharpens one notch per player kill.
// Level 0 never attacks or dodges; each level raises the commit charge (so
// throws hit harder and reach farther), the dodge range (so it protects
// itself sooner), and the aim accuracy.

/// Charge the bot commits its throw at, by level. Level 1 lobs a barely-
/// charged fang; level 4 throws at ~70% charge. Past 4 the GAUNTLET tiers
/// take over: +3% per tier, capped at 85% — hard, never a full-power wall.
fn throw_at_charge(lvl: u32) -> u32 {
    let frac = match lvl {
        0 | 1 => 0.30,
        2 => 0.42,
        3 => 0.55,
        4 => 0.70,
        n => (0.70 + 0.03 * (n - 4) as f32).min(0.85),
    };
    (CHARGE_MAX_FRAMES as f32 * frac) as u32
}

/// Fang distance that triggers the dodge reflex, by level. Dodging is off
/// below level 2; then it reacts progressively sooner. Gauntlet tiers past
/// 4 widen the reflex up to a 300 cm bubble.
fn threat_radius(lvl: u32) -> f32 {
    match lvl {
        0 | 1 => 0.0,
        2 => 120.0,
        3 => 165.0,
        4 => 210.0,
        n => (210.0 + 15.0 * (n - 4) as f32).min(300.0),
    }
}

/// Peak aim wobble (radians), by level: a wide spray early, tightening as
/// the bot levels up but never fully honing in — the gauntlet floor is
/// 0.05 rad (~3°), beatable by a mover forever.
fn wobble_amp(lvl: u32) -> f32 {
    match lvl {
        0 | 1 => 0.42,
        2 => 0.30,
        3 => 0.20,
        4 => 0.12,
        n => (0.12 - 0.015 * (n - 4) as f32).max(0.05),
    }
}

fn quantize(dir: Vec2) -> (i8, i8) {
    let d = dir.clamp_length_max(1.0);
    (
        (d.x * 127.0).round().clamp(-127.0, 127.0) as i8,
        // Wire stick_y is y-up already in this codebase's PlayerInput terms
        // (quantize_inputs negates the SCREEN y; we work in world y-up).
        (d.y * 127.0).round().clamp(-127.0, 127.0) as i8,
    )
}

fn input_from(dir: Vec2, buttons: u8) -> PlayerInput {
    let (x, y) = quantize(dir);
    PlayerInput {
        stick_x: x,
        stick_y: y,
        aim_angle: 0,
        buttons,
    }
}

/// The whole duelist, pure and testable.
pub fn bot_decide(v: &BotView) -> PlayerInput {
    let lvl = v.difficulty;

    // Dead opponent / out of round: drift back toward the safe center —
    // and, once the bot has sharpened up (level >= 2), flex over the
    // corpse. The taunt is a real mechanic (a completed flex feeds the
    // streak ladder), so the bot both teaches it and profits from it.
    // It only taunts with no lethal fang inbound (the victim's own
    // throw can still be flying), and holds TAUNT through one flex
    // window per death beat — the level signal needs a fresh edge to
    // re-trigger, so this reads as one clean taunt, not a stutter.
    if !v.foe_alive {
        let beat = v.frame % 180;
        if lvl >= 2 && v.threat.is_none() && (30..90).contains(&beat) {
            return input_from(Vec2::ZERO, PlayerInput::TAUNT_DOWN);
        }
        return input_from(-v.me * 0.002, 0);
    }

    // Level 0 — a passive sparring dummy: it just ambles around slowly and
    // never throws or dodges, so the player warms up and lands the first free
    // kill. Every kill the player scores raises the level by one.
    if lvl == 0 {
        return input_from(wander_dir(v), 0);
    }

    // 1) Survival dodge: a lethal fang closing in → dash through it. The bot
    //    only starts protecting itself once it has been beaten a couple of
    //    times (level >= 2); below that it eats the player's throws.
    if lvl >= 2
        && let Some((tpos, tvel)) = v.threat
    {
        let to_me = v.me - tpos;
        if tvel.dot(to_me) > 0.0 && to_me.length() < threat_radius(lvl) {
            // Perpendicular to the fang's path, biased toward the center so
            // the dodge never carries the bot off the island.
            let perp = Vec2::new(-tvel.y, tvel.x).normalize_or_zero();
            let dir = if (v.me + perp * 100.0).length() < (v.me - perp * 100.0).length() {
                perp
            } else {
                -perp
            };
            let buttons = if v.can_dash { PlayerInput::DASH_DOWN } else { 0 };
            return input_from(dir, buttons);
        }
    }

    // 2) Housekeeping: a dropped fang is a liability (the human can steal
    //    it) — walk it down.
    if v.fangs_out > 0
        && let Some(loose) = v.my_loose
    {
        let dir = (loose - v.me).normalize_or_zero();
        return input_from(edge_safe(v, dir), 0);
    }

    // 3) A fang in flight: steer the recall arc at the foe (Returning), or
    //    recall it once it's far and no longer threatening.
    if let Some((fpos, returning)) = v.my_fang {
        if returning {
            // Bend the return arc across the foe — AIM carries the steer.
            let steer = (v.foe - fpos).normalize_or_zero();
            return input_from(steer, PlayerInput::AIM_ACTIVE);
        }
        let far = (fpos - v.me).length() > 520.0;
        // A short periodic press window creates the recall edge.
        let press = far && v.frame % 24 < 2;
        let dir = orbit_dir(v);
        return input_from(
            edge_safe(v, dir),
            if press { PlayerInput::THROW_DOWN } else { 0 },
        );
    }

    // 4) Armed and free (level >= 1): charge while positioning; plant + aim
    //    for the final ticks; release at the level's commit charge, which
    //    grows with the level so throws hit harder as the player wins.
    let commit = throw_at_charge(lvl);
    if v.my_charge >= commit {
        // RELEASE tick: drop THROW, keep AIM + the aim vector on the stick.
        return input_from(aim_at_foe(v), PlayerInput::AIM_ACTIVE);
    }
    if v.my_charge >= commit.saturating_sub(PLANT_TICKS) {
        // The plant: still holding, aim visible — the human gets the read.
        return input_from(
            aim_at_foe(v),
            PlayerInput::THROW_DOWN | PlayerInput::AIM_ACTIVE,
        );
    }
    // Charge while orbiting at range. A charge only ARMS on a fresh THROW
    // press edge (SIM_VERSION 8): if the button was still down when the
    // recall landed in hand, that hold is inert — drop it for one beat so
    // the next frame presses fresh, else the bot would orbit forever
    // squeezing a dead button.
    if v.my_charge == 0 && v.frame.is_multiple_of(8) {
        return input_from(edge_safe(v, orbit_dir(v)), 0);
    }
    input_from(edge_safe(v, orbit_dir(v)), PlayerInput::THROW_DOWN)
}

/// Range-keeping orbit: radial correction toward the preferred ring plus a
/// slowly alternating tangential strafe.
fn orbit_dir(v: &BotView) -> Vec2 {
    let to_foe = v.foe - v.me;
    let dist = to_foe.length().max(1.0);
    let radial = to_foe / dist * ((dist - PREFERRED_RANGE) / 200.0).clamp(-1.0, 1.0);
    let swing = if (v.frame / 120).is_multiple_of(2) {
        1.0
    } else {
        -1.0
    };
    let tangent = Vec2::new(-to_foe.y, to_foe.x) / dist * 0.6 * swing;
    (radial + tangent).clamp_length_max(1.0)
}

/// Aim at the foe with a slow wobble — the imperfection that makes the bot
/// beatable at the dash-dodge game.
fn aim_at_foe(v: &BotView) -> Vec2 {
    let base = (v.foe - v.me).normalize_or_zero();
    // Wobble amplitude shrinks as the bot levels up: a wide early spray, a
    // tighter (still imperfect) aim once the player has been winning.
    let wobble = (v.frame as f32 * 0.11).sin() * wobble_amp(v.difficulty);
    Vec2::from_angle(wobble).rotate(base)
}

/// Level-0 idle: a slow, gentle wander (a Lissajous drift), clamped to the
/// island. Reads as a dummy ambling around, not tracking or attacking.
fn wander_dir(v: &BotView) -> Vec2 {
    let f = v.frame as f32;
    let drift = Vec2::new((f * 0.018).sin(), (f * 0.013).cos());
    edge_safe(v, drift) * 0.42
}

/// Clamp a movement intent so it never walks the bot over the (possibly
/// crumbling) edge.
fn edge_safe(v: &BotView, dir: Vec2) -> Vec2 {
    let mut d = dir;
    let margin = 0.82;
    if v.me.x.abs() > v.bounds.x * margin && (d.x * v.me.x.signum()) > 0.0 {
        d.x = -v.me.x.signum() * 0.6;
    }
    if v.me.y.abs() > v.bounds.y * margin && (d.y * v.me.y.signum()) > 0.0 {
        d.y = -v.me.y.signum() * 0.6;
    }
    d
}

/// Collect the view + queue the input patch. Runs in `ReadInputs`, ordered
/// after the platform source so the queued closure lands on the fresh map.
pub fn drive_bot(world: &mut World) {
    if !world.resource::<PracticeMode>().0 {
        return;
    }
    let frame = world.resource::<FrameCount>().0;
    let in_round = world.resource::<MatchState>().is_in_round();

    // Difficulty = the persisted GAUNTLET tier plus the player's kills so
    // far this match (score.p0 — the player is always handle 0 in
    // practice). A fresh install starts at the passive dummy; a tier-6
    // gauntlet runner faces a bot that opens sharp and still sharpens as
    // it loses.
    let tier = world.resource::<crate::grudge::CareerRecord>().gauntlet_tier;
    let difficulty = tier + world.resource::<sim::MatchScore>().p0 as u32;

    let mut view = BotView {
        frame,
        bounds: Vec2::new(ARENA_HALF_WIDTH_CM as f32, ARENA_HALF_HEIGHT_CM as f32),
        difficulty,
        ..default()
    };
    // Sudden-death crumble awareness — only where the storm exists (the
    // Pit and the Vigil never shrink; the bot shouldn't hug center there).
    if world.resource::<sim::SelectedArena>().0.crumbles()
        && let MatchState::InRound { expires_at_frame } = *world.resource::<MatchState>()
    {
        let remaining = expires_at_frame.saturating_sub(frame);
        view.bounds *= sudden_death_factor(remaining).to_num::<f32>();
    }

    let mut me_alive = true;
    {
        let mut players = world.query::<(
            &Player,
            &PositionF,
            &Dead,
            &DashState,
            &ThrowCharge,
            &ThrowCapacity,
        )>();
        for (p, pos, dead, dash, charge, _cap) in players.iter(world) {
            let (x, y) = pos.0.to_f32();
            if p.handle == BOT_HANDLE {
                view.me = Vec2::new(x, y);
                view.my_charge = charge.0;
                view.can_dash = matches!(dash, DashState::Idle) && !dead.is_dying();
                me_alive = !dead.is_dying();
            } else {
                view.foe = Vec2::new(x, y);
                view.foe_alive = !dead.is_dying();
            }
        }
    }
    {
        let mut fangs =
            world.query::<(&Boomerang, &BoomerangMods, &PositionF, &VelocityF)>();
        let mut nearest = f32::MAX;
        for (boom, mods, pos, vel) in fangs.iter(world) {
            let (x, y) = pos.0.to_f32();
            let (vx, vy) = vel.0.to_f32();
            let p = Vec2::new(x, y);
            if boom.owner_handle == BOT_HANDLE {
                if mods.is_secondary {
                    continue;
                }
                match boom.state {
                    BoomerangState::Flying => view.my_fang = Some((p, false)),
                    BoomerangState::Returning { .. } => view.my_fang = Some((p, true)),
                    BoomerangState::Loose => view.my_loose = Some(p),
                }
                if !matches!(boom.state, BoomerangState::Loose) {
                    view.fangs_out += 1;
                }
            } else if !matches!(boom.state, BoomerangState::Loose) {
                let d = (p - view.me).length_squared();
                if d < nearest {
                    nearest = d;
                    view.threat = Some((p, Vec2::new(vx, vy)));
                }
            }
        }
    }
    // Loose fangs still occupy a throw slot until reclaimed.
    if view.my_loose.is_some() {
        view.fangs_out += 1;
    }

    let input = if in_round && me_alive {
        bot_decide(&view)
    } else {
        PlayerInput::default()
    };
    if let Some(mut local) = world.get_resource_mut::<LocalInputs<GgrsCfg>>() {
        local.0.insert(BOT_HANDLE, input);
    }
}

pub struct BotPlugin;

impl Plugin for BotPlugin {
    fn build(&self, app: &mut App) {
        // TWOTOP_PRACTICE=1 boots straight into practice (pairs with
        // TWOTOP_AUTOSTART for headless capture verification of the bot).
        let boot_practice = std::env::var("TWOTOP_PRACTICE").is_ok_and(|v| v == "1");
        app.insert_resource(PracticeMode(boot_practice));
        // The exclusive system is registered per-platform in lib.rs so it
        // orders after that platform's input source.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mid-ramp view (difficulty 3) so the fighting behaviors are active;
    /// level-0 passivity is exercised by its own test.
    fn base_view() -> BotView {
        BotView {
            frame: 600,
            me: Vec2::new(0.0, 300.0),
            foe: Vec2::new(0.0, -300.0),
            foe_alive: true,
            can_dash: true,
            bounds: Vec2::new(500.0, 750.0),
            difficulty: 3,
            ..default()
        }
    }

    #[test]
    fn flexes_over_the_corpse_when_safe_and_sharpened() {
        let mut v = base_view();
        v.foe_alive = false;
        v.frame = 600; // beat 60 — inside the 30..90 flex window
        assert_ne!(
            bot_decide(&v).buttons & PlayerInput::TAUNT_DOWN,
            0,
            "level 3 with no threat inbound taunts the kill"
        );
        // A lethal fang still flying at it: no flex, survival first.
        v.threat = Some((v.me + Vec2::new(0.0, -80.0), Vec2::new(0.0, 24.0)));
        assert_eq!(bot_decide(&v).buttons & PlayerInput::TAUNT_DOWN, 0);
        // A green bot (level < 2) doesn't know the move yet.
        v.threat = None;
        v.difficulty = 1;
        assert_eq!(bot_decide(&v).buttons & PlayerInput::TAUNT_DOWN, 0);
    }

    #[test]
    fn level_zero_is_a_passive_dummy() {
        let mut v = base_view();
        v.difficulty = 0;
        // Even with a fang bearing down, level 0 never throws or dashes.
        v.threat = Some((v.me + Vec2::new(0.0, -80.0), Vec2::new(0.0, 24.0)));
        let input = bot_decide(&v);
        assert!(input.buttons & PlayerInput::THROW_DOWN == 0, "no throw");
        assert!(input.buttons & PlayerInput::DASH_DOWN == 0, "no dodge");
        assert!(input.buttons & PlayerInput::AIM_ACTIVE == 0, "no aim");
    }

    #[test]
    fn charges_while_free_and_unthreatened() {
        let mut v = base_view();
        v.frame += 1; // off the re-arm beat (frame % 8 == 0 releases at charge 0)
        let input = bot_decide(&v);
        assert!(input.buttons & PlayerInput::THROW_DOWN != 0, "should charge");
        assert!(input.buttons & PlayerInput::AIM_ACTIVE == 0, "no plant yet");
    }

    #[test]
    fn rearm_beat_releases_a_dead_hold_but_not_a_live_charge() {
        // At charge 0 the beat frame drops THROW for one tick so the next
        // frame is a fresh press edge — without it a hold kept down through
        // the catch would never arm under the press-edge rule.
        let v = base_view(); // frame 600 — on the beat, charge 0
        assert!(
            bot_decide(&v).buttons & PlayerInput::THROW_DOWN == 0,
            "beat releases the dead hold"
        );
        let mut armed = base_view();
        armed.my_charge = 3; // a live wind-up must survive the beat
        assert!(bot_decide(&armed).buttons & PlayerInput::THROW_DOWN != 0);
    }

    #[test]
    fn plants_then_releases_at_threshold() {
        let mut v = base_view();
        let commit = throw_at_charge(v.difficulty);
        v.my_charge = commit - 2;
        let plant = bot_decide(&v);
        assert!(plant.buttons & PlayerInput::THROW_DOWN != 0);
        assert!(plant.buttons & PlayerInput::AIM_ACTIVE != 0, "visible plant");
        v.my_charge = commit;
        let release = bot_decide(&v);
        assert!(
            release.buttons & PlayerInput::THROW_DOWN == 0,
            "release edge fires the throw"
        );
        assert!(release.buttons & PlayerInput::AIM_ACTIVE != 0);
        // Aim points broadly at the foe (down-table from the bot's spawn).
        assert!(release.stick_y < 0, "aims toward the foe");
    }

    #[test]
    fn difficulty_ramps_commit_charge_and_dodge_range() {
        assert!(throw_at_charge(1) < throw_at_charge(4), "throws harden");
        assert_eq!(threat_radius(1), 0.0, "no dodge below level 2");
        assert!(threat_radius(2) < threat_radius(4), "dodges sooner");
        assert!(wobble_amp(1) > wobble_amp(4), "aim tightens");
    }

    #[test]
    fn gauntlet_tiers_keep_sharpening_but_hit_ceilings() {
        // Past level 4 the ramps keep moving...
        assert!(throw_at_charge(6) > throw_at_charge(4));
        assert!(threat_radius(6) > threat_radius(4));
        assert!(wobble_amp(6) < wobble_amp(4));
        // ...and saturate instead of becoming an aimbot wall.
        assert_eq!(throw_at_charge(40), throw_at_charge(12));
        assert_eq!(threat_radius(40), 300.0);
        assert_eq!(wobble_amp(40), 0.05);
        // The commit charge never reaches a human's full-power shot.
        assert!(throw_at_charge(40) < CHARGE_MAX_FRAMES);
    }

    #[test]
    fn dashes_through_an_incoming_fang() {
        let mut v = base_view();
        v.threat = Some((v.me + Vec2::new(0.0, -120.0), Vec2::new(0.0, 24.0)));
        let input = bot_decide(&v);
        assert!(input.buttons & PlayerInput::DASH_DOWN != 0, "graze reflex");
    }

    #[test]
    fn steers_the_returning_fang_at_the_foe() {
        let mut v = base_view();
        v.fangs_out = 1;
        v.my_fang = Some((Vec2::new(200.0, 0.0), true));
        let input = bot_decide(&v);
        assert!(input.buttons & PlayerInput::AIM_ACTIVE != 0, "steering");
        assert!(input.stick_x < 0 || input.stick_y < 0, "bends toward the foe");
    }

    #[test]
    fn edge_override_pulls_back_from_the_rim() {
        let mut v = base_view();
        v.me = Vec2::new(480.0, 300.0); // near the +x rim
        let d = edge_safe(&v, Vec2::new(1.0, 0.0));
        assert!(d.x < 0.0, "never walks off the island");
    }
}
