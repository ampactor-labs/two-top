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
}

/// Preferred dueling range (cm). Inside it the bot backs off, outside it
/// closes — with an orbit component so it never runs a straight line. A
/// touch farther out (the practice bot keeps its distance, giving the
/// player room to breathe).
const PREFERRED_RANGE: f32 = 440.0;
/// Charge frames before the bot commits to the throw. The practice bot
/// throws at roughly HALF charge — weaker, slower, shorter fangs that are
/// much easier to read, dodge, and recall-punish than a full-power shot.
const THROW_AT_CHARGE: u32 = CHARGE_MAX_FRAMES / 2;
/// Ticks of visible plant (AIM_ACTIVE) before the release.
const PLANT_TICKS: u32 = 5;
/// Enemy fang distance that triggers the dodge/graze reflex. Tightened so
/// the practice bot reacts LATE (it eats more of the player's throws).
const THREAT_RADIUS: f32 = 150.0;

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
    // Dead opponent / out of round: drift back toward the safe center.
    if !v.foe_alive {
        return input_from(-v.me * 0.002, 0);
    }

    // 1) Survival reflex: a lethal fang closing in → dash through it
    //    (i-frames + the graze reward), or strafe off its line if the dash
    //    is spent. Highest priority — nothing else matters while dying.
    if let Some((tpos, tvel)) = v.threat {
        let to_me = v.me - tpos;
        let closing = tvel.dot(to_me) > 0.0;
        if closing && to_me.length() < THREAT_RADIUS {
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

    // 4) Armed and free: charge while positioning; plant + aim for the
    //    final ticks; release at the throw threshold.
    if v.my_charge >= THROW_AT_CHARGE {
        // RELEASE tick: drop THROW, keep AIM + the aim vector on the stick.
        return input_from(aim_at_foe(v), PlayerInput::AIM_ACTIVE);
    }
    if v.my_charge >= THROW_AT_CHARGE.saturating_sub(PLANT_TICKS) {
        // The plant: still holding, aim visible — the human gets the read.
        return input_from(
            aim_at_foe(v),
            PlayerInput::THROW_DOWN | PlayerInput::AIM_ACTIVE,
        );
    }
    // Charge while orbiting at range.
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
    // A wide, slow wobble (~13° peak) — the practice bot sprays wide of the
    // mark often enough that a player who keeps moving rarely gets clipped.
    let wobble = (v.frame as f32 * 0.11).sin() * 0.22;
    Vec2::from_angle(wobble).rotate(base)
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

    let mut view = BotView {
        frame,
        bounds: Vec2::new(ARENA_HALF_WIDTH_CM as f32, ARENA_HALF_HEIGHT_CM as f32),
        ..default()
    };
    // Sudden-death crumble awareness.
    if let MatchState::InRound { expires_at_frame } = *world.resource::<MatchState>() {
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

    fn base_view() -> BotView {
        BotView {
            frame: 600,
            me: Vec2::new(0.0, 300.0),
            foe: Vec2::new(0.0, -300.0),
            foe_alive: true,
            can_dash: true,
            bounds: Vec2::new(500.0, 750.0),
            ..default()
        }
    }

    #[test]
    fn charges_while_free_and_unthreatened() {
        let v = base_view();
        let input = bot_decide(&v);
        assert!(input.buttons & PlayerInput::THROW_DOWN != 0, "should charge");
        assert!(input.buttons & PlayerInput::AIM_ACTIVE == 0, "no plant yet");
    }

    #[test]
    fn plants_then_releases_at_threshold() {
        let mut v = base_view();
        v.my_charge = THROW_AT_CHARGE - 2;
        let plant = bot_decide(&v);
        assert!(plant.buttons & PlayerInput::THROW_DOWN != 0);
        assert!(plant.buttons & PlayerInput::AIM_ACTIVE != 0, "visible plant");
        v.my_charge = THROW_AT_CHARGE;
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
