//! The practice bot's brain: pure, platform-free, shared by every build.
//!
//! The bot is an input source, nothing more: it looks at the match (a
//! [`BotView`] in plain f32, built from sim components by [`view_for`]) and
//! answers with a wire [`PlayerInput`], exactly what a thumb or a keyboard
//! would send. Rollback, recording and determinism never notice it.
//!
//! The policy is a readable duelist, not an aimbot: it keeps throwing range,
//! orbits, PLANTS visibly before it throws (AIM_ACTIVE during the wind-up —
//! the same telegraph a human shows), dashes through incoming fangs, steers
//! its recalls, walks to its dropped fang, and respects the island edge and
//! the sudden-death crumble. Deliberate imperfections (aim wobble, a reflex
//! that fires on some throws, and a perception lag through [`Brain`]) keep
//! it beatable.
//!
//! It lives in its own crate so the same brain can be driven by the app
//! (practice, First Blood, shades) and by the headless bench
//! (`src/bin/bot_bench.rs`) that tunes the gauntlet ladder against numbers.

pub mod bench;

use bevy::prelude::*;
use sim::{
    ARENA_HALF_HEIGHT_CM, ARENA_HALF_WIDTH_CM, BoneTree, Boomerang, BoomerangMods, BoomerangState,
    CHARGE_MAX_FRAMES, DashState, Dead, FrameCount, MatchState, Player, PlayerInput, PositionF,
    ThrowCharge, VelocityF, Wall, WallKind, sudden_death_factor,
};

/// Everything the policy looks at, in plain f32 (inputs are not sim state —
/// they only become deterministic once they enter the wire pipeline).
#[derive(Debug, Clone, Default)]
pub struct BotView {
    pub frame: u32,
    pub me: Vec2,
    pub foe: Vec2,
    /// The foe's velocity (cm/tick), for leading a throw ([`lead_frac`]).
    pub foe_vel: Vec2,
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
    /// Practice difficulty: the persistent gauntlet tier, nudged by the
    /// in-match rubber band ([`rubber_band`]). 0 = the sparring partner:
    /// slow, telegraphed throws, no dodging.
    pub difficulty: u32,
    /// Varies the bot's imperfections (aim-wobble phase, which dodge windows
    /// fire) between matches. The live game leaves it 0; the bench sets one
    /// per match so 200 matches are not one match 200 times.
    pub seed: u32,
    /// Blocking solids on the field — cover blocks and standing trees, as
    /// (center, half-extents) in cm. The steering slides along these; the
    /// old bounds-only policy walked orbit paths straight through cover
    /// and ground against the face every tick (the wall-jitter read).
    pub obstacles: Vec<(Vec2, Vec2)>,
    /// Lethal ground — the Crossing's chasm while its bridge is down, as
    /// (center, half-extents). Steered around and never walked into for a
    /// fang, like cover; unlike cover it blocks nothing a fang flies over,
    /// so it is not a line-of-sight blocker. The brain knew nothing of it,
    /// and on the Crossing every tier's orbit wandered into the moat: the
    /// sparring partner lost 5-0 to a player who never touched the screen.
    pub hazards: Vec<(Vec2, Vec2)>,
    /// A shade's fitted habits (NORTH N6). `Some` replaces the level-keyed
    /// knobs with a rival's measured ones; the tier ladder is untouched.
    pub style: Option<BotStyle>,
}

/// A rival's measured habits, fitted from their tape ring by
/// `crate::shade`. Every field lands on a knob the tier ladder already
/// turns, so the shade is the same readable duelist — planted throws,
/// orbit, dodge — with THEIR numbers in it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BotStyle {
    /// Charge fraction the shade commits its throws at (their mean hold).
    pub commit_frac: f32,
    /// Dodge-reflex bubble in cm (their dash appetite).
    pub dodge_radius: f32,
    /// Aim wobble amplitude in radians (their plant discipline, inverted).
    pub wobble: f32,
    /// Preferred dueling range in cm (their tempo: pressers close in).
    pub range: f32,
}

/// Preferred dueling range (cm). Inside it the bot backs off, outside it
/// closes — with an orbit component so it never runs a straight line.
const PREFERRED_RANGE: f32 = 440.0;
/// Ticks of visible plant (AIM_ACTIVE) before the release.
const PLANT_TICKS: u32 = 5;

// ---- Difficulty ramp (practice mode) ----
// The bot starts as a sparring partner and sharpens one notch per gauntlet
// tier. Level 0 lobs slow, planted throws on a fixed beat and never dodges
// (`spar`); from level 1 each level raises the commit charge (so throws hit
// harder and reach farther), the dodge range and the odds it actually
// reacts, the aim accuracy, and how fresh its read of the foe is
// (`perception_lag`).
//
// The ramp used to cover its whole range in four levels: tier 2 already
// dodged, tier 4 threw at 70% with a 210 cm frame-perfect dash reflex, and
// with the in-match notch on top a new player met that bot on their second
// or third match — "one or two rounds of practice, then he's cracked."
// Every knob now climbs in small, even steps across the full ten-tier
// ladder, dodging waits until tier 3, and even a sharp bot only reacts to
// SOME throws (`dodge_chance`) instead of all of them.

/// Charge the bot commits its throw at, by level: a barely-charged lob at
/// level 1, +4.5% per level, capped at 75% by tier 10 — hard, never a
/// full-power wall.
fn throw_at_charge(lvl: u32) -> u32 {
    let frac = (0.30 + 0.045 * lvl.saturating_sub(1) as f32).min(0.75);
    (CHARGE_MAX_FRAMES as f32 * frac) as u32
}

/// Fang distance that triggers the dodge reflex, by level. Dodging is off
/// below level 3; then the bubble widens 20 cm a level up to 240 cm.
fn threat_radius(lvl: u32) -> f32 {
    if lvl < 3 {
        0.0
    } else {
        (100.0 + 20.0 * (lvl - 3) as f32).min(240.0)
    }
}

/// How often the dodge reflex actually fires, by level: one throw in four
/// at level 3, climbing to three in four at the cap. A reflex that fires
/// every time is what made the bot unhittable — a human who reads the
/// throw still eats one now and then, and so should the bot.
fn dodge_chance(lvl: u32) -> f32 {
    if lvl < 3 {
        0.0
    } else {
        (0.25 + 0.07 * (lvl - 3) as f32).min(0.75)
    }
}

/// Peak aim wobble (radians), by level: a wide spray early, tightening as
/// the bot levels up but never fully honing in — the floor is 0.10 rad
/// (~6°), beatable by a mover forever.
fn wobble_amp(lvl: u32) -> f32 {
    (0.45 - 0.035 * lvl.saturating_sub(1) as f32).max(0.10)
}

/// Frames one dodge decision holds for. The roll is per window, not per
/// frame: re-rolling every tick would turn a 25% reflex into a near-certain
/// one over the dozen frames a fang spends inside the bubble.
const DODGE_ROLL_WINDOW: u32 = 30;

/// The in-match rubber band: the gauntlet tier, one notch up while the
/// player leads by two or more, one notch down while the bot does. It used
/// to be a single notch once the player had three kills, which only ever
/// made a match harder; a player losing 0-3 kept facing the same bot. Never
/// below the sparring partner (0).
pub fn rubber_band(tier: u32, player_score: u8, bot_score: u8) -> u32 {
    let lead = i32::from(player_score) - i32::from(bot_score);
    match lead {
        2.. => tier + 1,
        ..=-2 => tier.saturating_sub(1),
        _ => tier,
    }
}

/// Deterministic 0..1 roll for the dodge window containing `frame`. The bot
/// is an input source, not sim state — but a pure function of the frame
/// (and the view's seed) keeps `bot_decide` testable and the tape
/// reproducible.
fn dodge_roll(frame: u32, seed: u32) -> f32 {
    let mut x = (frame / DODGE_ROLL_WINDOW).wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B ^ seed;
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    (x % 1000) as f32 / 1000.0
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
        return input_from(steer(v, -v.me * 0.002), 0);
    }

    // Level 0 — the sparring partner. It used to be a dummy that never
    // threw: a newcomer never saw a fang coming at them, and a player who
    // did nothing looped 30 s rounds at 0-0 forever. Now it closes slowly
    // and lobs a slow, planted, telegraphed fang every few seconds; it still
    // never dodges and never steers a recall.
    if lvl == 0 && v.style.is_none() {
        return spar(v);
    }

    // 1) Survival dodge: a lethal fang closing in → dash through it. The bot
    //    only starts protecting itself at level 3; below that it eats the
    //    player's throws, and above it the reflex fires on some throws, not
    //    all (`dodge_chance`). A shade dodges from the first tick and every
    //    time — its reflex is the rival's, not the ladder's.
    if reacts(v)
        && let Some((tpos, tvel)) = v.threat
    {
        let to_me = v.me - tpos;
        let radius = v
            .style
            .map_or_else(|| threat_radius(lvl), |s| s.dodge_radius);
        // Only a fang whose lane the bot is in: one that will pass wide is
        // left to the lane rule below, and the dash kept for one that won't.
        let off = lane_offset(v, (tpos, tvel)).unwrap_or(Vec2::ZERO);
        if tvel.dot(to_me) > 0.0 && to_me.length() < radius && off.length() < LANE_CLEAR {
            // Mid-charge there is no dashing (a charge commits you in
            // place), and letting go of THROW fires the fang along the
            // stick. The dodge used to do exactly that with the stick
            // pointed sideways: every reflex that caught the bot winding
            // up threw its fang away at nothing — and the sharper the bot,
            // the more often it dodged and disarmed itself (the bench
            // ranked tier 5 over tier 6, 8 over 9). Let it go at the foe
            // instead and press DASH with it: the press is buffered, and
            // the dash fires the tick after the release.
            if v.my_charge > 0 {
                return input_from(
                    aim_at_foe(v),
                    PlayerInput::AIM_ACTIVE | PlayerInput::DASH_DOWN,
                );
            }
            let dir = dodge_side(v, tvel, off);
            let buttons = if v.can_dash {
                PlayerInput::DASH_DOWN
            } else {
                0
            };
            return input_from(steer(v, dir), buttons);
        }
    }

    // 1b) Out of the lane. The dodge is a reflex at close range; once it
    //     had carried the bot clear, the footwork walked it straight back
    //     into the same fang's line — a slow, far fang is still coming, and
    //     big — and most deaths of a dodging tier came a few ticks after a
    //     dash that had worked. A bot that is reading the fang walks out of
    //     its lane (rooted while charging, so not then), and its footwork
    //     will not step back in (`keep_out_of_lane`).
    if v.my_charge == 0
        && reacts(v)
        && let Some(threat) = v.threat
        && let Some(off) = lane_offset(v, threat)
        && off.length() < LANE_CLEAR
    {
        let out = if off.length() > 5.0 {
            off.normalize()
        } else {
            // Dead on the line: out toward the centre.
            let perp = Vec2::new(-threat.1.y, threat.1.x).normalize_or_zero();
            if (v.me + perp).length() < (v.me - perp).length() {
                perp
            } else {
                -perp
            }
        };
        return input_from(steer(v, out), 0);
    }

    // 2) Housekeeping: a dropped fang is a liability (the human can steal
    //    it) — walk it down when the steering can actually deliver the bot
    //    to it, and REEL IT IN when it can't.
    //
    //    The walk used to be unconditional, and that was a hang. Every
    //    movement intent runs through `steer`, which refuses to cross the
    //    edge cushion or enter cover's padded ring; a fang that settles in
    //    either place is one the bot walks at forever and never arrives
    //    at. Worse, the refusal alternates with the intent frame by frame,
    //    so the bot vibrated on the spot (the sprite's facing flips with
    //    the velocity sign — on screen it read as a duelist stuck in place,
    //    strobing) and, because this branch preempts both the recall and
    //    the throw, it stayed a punching bag for the rest of the round.
    //    A Loose fang is hold-recallable (`recall_boomerangs`), so the
    //    unreachable case has a clean answer: pulse THROW to make the
    //    press edge and let the fang come to the bot.
    if v.fangs_out > 0
        && let Some(loose) = v.my_loose
    {
        let to_loose = loose - v.me;
        if retrievable(v, loose) {
            let dir = edge_clamp(v, to_loose.normalize_or_zero(), RETRIEVE_MARGIN);
            let reach = to_loose.length().min(AVOID_LOOKAHEAD);
            return input_from(slide_around_cover_within(v, dir, reach), 0);
        }
        // Out of walking reach: hold-recall it (the edge is what fires the
        // recall, so pulse rather than hold) and keep orbiting meanwhile.
        let press = v.frame % 24 < 2;
        return input_from(
            duel_move(v),
            if press { PlayerInput::THROW_DOWN } else { 0 },
        );
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
        return input_from(
            duel_move(v),
            if press { PlayerInput::THROW_DOWN } else { 0 },
        );
    }

    // 4) Armed and free (level >= 1): charge while positioning; plant + aim
    //    for the final ticks; release at the level's commit charge, which
    //    grows with the level so throws hit harder as the player wins.
    let commit = v.style.map_or_else(
        || throw_at_charge(lvl),
        |s| (CHARGE_MAX_FRAMES as f32 * s.commit_frac) as u32,
    );
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
        return input_from(duel_move(v), 0);
    }
    input_from(duel_move(v), PlayerInput::THROW_DOWN)
}

/// Range-keeping orbit: radial correction toward the preferred ring plus a
/// slowly alternating tangential strafe.
fn orbit_dir(v: &BotView) -> Vec2 {
    let to_foe = v.foe - v.me;
    let dist = to_foe.length().max(1.0);
    let range = v.style.map_or(PREFERRED_RANGE, |s| s.range);
    let radial = to_foe / dist * ((dist - range) / 200.0).clamp(-1.0, 1.0);
    let tangent_axis = Vec2::new(-to_foe.y, to_foe.x) / dist;
    let swing = match line_blocked(v) {
        // Cover in the way: commit to the side of it the bot already leans
        // toward until the line is clear. The strafe used to flip every two
        // seconds regardless, which behind a wide crate meant pacing back
        // and forth behind the same crate for good (the Gallery stalled a
        // match against an idle player at 0-0 for ten minutes).
        Some(cover) => {
            if tangent_axis.dot(v.me - cover) >= 0.0 {
                1.0
            } else {
                -1.0
            }
        }
        None => {
            if (v.frame / 120).is_multiple_of(2) {
                1.0
            } else {
                -1.0
            }
        }
    };
    let tangent = tangent_axis * 0.6 * swing;
    (radial + tangent).clamp_length_max(1.0)
}

/// How much of the foe's motion the throw leads, by level: none up to
/// level 1, then a seventh more each level, all of it from level 8.
///
/// A fang crosses the dueling range in about a quarter second, and a
/// walking foe covers well over a body width in that time, so a throw at
/// where the foe IS misses anyone who keeps moving. Without a lead, the
/// sharper aim of a high tier was aim at the wrong spot, and the bench
/// had tier 1 beating tier 10: the wide spray of a green bot hit a mover
/// as often as the tight aim of a sharp one, and threw sooner. Leading is
/// the skill; the wobble ([`wobble_amp`]) stays the imperfection on top.
fn lead_frac(lvl: u32) -> f32 {
    (lvl.saturating_sub(1) as f32 / 7.0).min(1.0)
}

/// A shade leads like a sharp human: most of the way, not all of it.
const SHADE_LEAD: f32 = 0.7;

/// The fastest a lead believes the foe moves (cm/tick): walking pace. A
/// dash is a five-tick burst, not a heading worth leading.
const LEAD_MAX_SPEED: f32 = sim::WALK_SPEED_CM_PER_TICK as f32;

/// Launch speed (cm/tick) of a throw released at `charge` frames — the
/// sim's own formula, unempowered.
fn release_speed(charge: u32) -> f32 {
    sim::aimed_throw_speed(sim::throw_speed_for(false), sim::charge_power(charge)).to_num::<f32>()
}

/// Where to throw at: the foe as last seen, carried `lead` of the way along
/// its motion through the perception lag and the fang's flight (four
/// passes of the intercept: each closes it by the speed ratio, ~0.45).
fn led_target(v: &BotView, lead: f32) -> Vec2 {
    if lead <= 0.0 {
        return v.foe;
    }
    let vel = v.foe_vel.clamp_length_max(LEAD_MAX_SPEED);
    let lag = perception_lag(v.difficulty, v.style.is_some()) as f32;
    let speed = release_speed(v.my_charge.max(1)).max(1.0);
    let mut target = v.foe;
    for _ in 0..4 {
        let flight = target.distance(v.me) / speed;
        target = v.foe + vel * (lag + flight) * lead;
    }
    target
}

/// Aim at the foe — led by the level's share of its motion — with a slow
/// wobble: the imperfection that makes the bot beatable at the dash-dodge
/// game.
fn aim_at_foe(v: &BotView) -> Vec2 {
    let lead = v
        .style
        .map_or_else(|| lead_frac(v.difficulty), |_| SHADE_LEAD);
    let base = (led_target(v, lead) - v.me).normalize_or_zero();
    // Wobble amplitude shrinks as the bot levels up: a wide early spray, a
    // tighter (still imperfect) aim once the player has been winning. A
    // shade sprays exactly as wide as its rival planted tight.
    let amp = v
        .style
        .map_or_else(|| wobble_amp(v.difficulty), |s| s.wobble);
    let wobble = (v.frame as f32 * 0.11 + seed_phase(v.seed)).sin() * amp;
    Vec2::from_angle(wobble).rotate(base)
}

/// The seed as a phase in [0, 2π). Hashed down in integers first: the
/// seed itself is any u32, and `seed as f32` near 2^31 is so coarse (one
/// step is 128) that adding the frame term to it changed nothing — the
/// wobble froze into a constant aim offset and every throw of the match
/// missed the same way.
fn seed_phase(seed: u32) -> f32 {
    let bits = seed.wrapping_mul(0x9E37_79B9) >> 8;
    bits as f32 / (1u32 << 24) as f32 * std::f32::consts::TAU
}

/// One sparring throw per this many ticks (3 s at 60 Hz).
const SPAR_PERIOD: u32 = 180;
/// The tick in each period the plant begins: the rest of it is drifting.
const SPAR_PLANT_AT: u32 = 150;
/// Charge the sparring throw is released at: the slowest fang on the
/// ladder (half a full throw's speed), still reaching ~490 cm, past the
/// range the bot closes to.
const SPAR_COMMIT: u32 = 8;

/// The sparring partner (level 0): drift into range, then once a period
/// plant (aim visible, rooted) and lob a slow fang. Recalls are left to the
/// fang's own reach; a dropped fang is walked down or reeled in like any
/// level's.
fn spar(v: &BotView) -> PlayerInput {
    if v.fangs_out > 0 {
        if let Some(loose) = v.my_loose {
            if retrievable(v, loose) {
                let dir = edge_clamp(v, (loose - v.me).normalize_or_zero(), RETRIEVE_MARGIN);
                return input_from(dir * 0.6, 0);
            }
            let press = v.frame % 24 < 2;
            return input_from(
                wander_dir(v),
                if press { PlayerInput::THROW_DOWN } else { 0 },
            );
        }
        // Flying or returning: never steered, never recalled early.
        return input_from(wander_dir(v), 0);
    }
    // A plant, once begun, runs to its release. Before it begins, the bot
    // drifts until its beat comes round — and past it while cover is in the
    // way and there is somewhere to see the foe from: a fang thrown into a
    // crate drops at its foot and teaches nothing.
    let beat = v.frame % SPAR_PERIOD;
    let waits = beat < SPAR_PLANT_AT || (line_blocked(v).is_some() && route_to_line(v).is_some());
    if v.my_charge == 0 && waits {
        // Close to throwing range at an amble (THROW up, so the plant
        // starts on a fresh press edge).
        return input_from(duel_move(v) * 0.5, 0);
    }
    if v.my_charge >= SPAR_COMMIT {
        return input_from(aim_at_foe(v), PlayerInput::AIM_ACTIVE);
    }
    input_from(
        aim_at_foe(v),
        PlayerInput::THROW_DOWN | PlayerInput::AIM_ACTIVE,
    )
}

/// A moving target that never fights back: the sparring partner's slow
/// drift and nothing else. First Blood's kill lesson aims at this — a
/// standing bot teaches nothing about leading a throw.
pub fn amble(v: &BotView) -> PlayerInput {
    input_from(wander_dir(v), 0)
}

/// Level-0 drift: a slow, gentle wander (a Lissajous drift), clamped to the
/// island.
fn wander_dir(v: &BotView) -> Vec2 {
    let f = v.frame as f32;
    let drift = Vec2::new((f * 0.018).sin(), (f * 0.013).cos());
    steer(v, drift) * 0.42
}

/// Cushion the orbit keeps between itself and the (possibly crumbling)
/// island rim: the bot never *fights* for the outer fifth of the floor.
const EDGE_MARGIN: f32 = 0.82;
/// The retrieval walk is allowed closer to the rim than the orbit is —
/// otherwise a fang that settles in the cushion is a fang the bot is
/// permitted to want and forbidden to reach.
const RETRIEVE_MARGIN: f32 = 0.94;

/// Clamp a movement intent so it never walks the bot over the (possibly
/// crumbling) edge. `margin` is the fraction of the safe bounds the intent
/// is allowed to push out to.
fn edge_clamp(v: &BotView, dir: Vec2, margin: f32) -> Vec2 {
    let mut d = dir;
    if v.me.x.abs() > v.bounds.x * margin && (d.x * v.me.x.signum()) > 0.0 {
        d.x = -v.me.x.signum() * 0.6;
    }
    if v.me.y.abs() > v.bounds.y * margin && (d.y * v.me.y.signum()) > 0.0 {
        d.y = -v.me.y.signum() * 0.6;
    }
    d
}

/// The orbit's edge clamp — the default cushion.
fn edge_safe(v: &BotView, dir: Vec2) -> Vec2 {
    edge_clamp(v, dir, EDGE_MARGIN)
}

/// Can the retrieval walk actually *arrive* at this dropped fang? Three
/// things must hold, and each one is a way the walk could otherwise
/// approach forever without arriving:
///
///   * the fang lies inside the box the walk is allowed to enter (else
///     `edge_clamp` shoves back exactly as hard as the walk pushes out);
///   * it is clear of every padded ring `slide_around_cover` refuses to
///     cross (else the slide skims past it, every pass);
///   * and the straight corridor to it is clear of those rings too — with
///     cover in the way the slide steers around the block, and around is
///     a detour the walk has no memory to complete.
///
/// Anything else is a recall, not a walk — see the housekeeping branch.
/// Recall is no worse tactically (a Returning fang is lethal, and only
/// its owner can catch it); it is just less of a stroll.
/// Everything the bot's feet must go around: cover, and lethal ground.
fn solids(v: &BotView) -> impl Iterator<Item = &(Vec2, Vec2)> {
    v.obstacles.iter().chain(&v.hazards)
}

/// The cover block between the bot and its foe, if one is in the way. Only
/// cover: a hazard is open air to a fang.
fn line_blocked(v: &BotView) -> Option<Vec2> {
    v.obstacles
        .iter()
        .find(|(center, half)| segment_hits_box(v.me, v.foe, *center, *half))
        .map(|(center, _)| *center)
}

fn retrievable(v: &BotView, loose: Vec2) -> bool {
    if loose.x.abs() > v.bounds.x * RETRIEVE_MARGIN || loose.y.abs() > v.bounds.y * RETRIEVE_MARGIN
    {
        return false;
    }
    !solids(v).any(|(center, half)| {
        segment_hits_box(v.me, loose, *center, *half + Vec2::splat(AVOID_PAD))
    })
}

/// Slab test: does the segment `a`→`b` touch the axis-aligned box? Used
/// on the padded rings, so "touches" already means "close enough that the
/// steering would refuse to go through here".
fn segment_hits_box(a: Vec2, b: Vec2, center: Vec2, pad: Vec2) -> bool {
    let d = b - a;
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for axis in 0..2 {
        let (lo, hi) = (center[axis] - pad[axis], center[axis] + pad[axis]);
        if d[axis].abs() < 1e-6 {
            // Parallel to this slab: inside it for all t, or never.
            if a[axis] < lo || a[axis] > hi {
                return false;
            }
            continue;
        }
        let mut near = (lo - a[axis]) / d[axis];
        let mut far = (hi - a[axis]) / d[axis];
        if near > far {
            std::mem::swap(&mut near, &mut far);
        }
        t0 = t0.max(near);
        t1 = t1.min(far);
        if t0 > t1 {
            return false;
        }
    }
    true
}

/// How far ahead (cm) a movement intent is probed for cover — ~12 ticks of
/// walking, far enough to turn before contact instead of at it.
const AVOID_LOOKAHEAD: f32 = 130.0;
/// Padding (cm) added to a block's half-extents for the probe: the bot's
/// own half-extent plus clearance, so the slide path keeps the body off
/// the face instead of grinding it.
const AVOID_PAD: f32 = 44.0;

/// Slide a movement intent along cover instead of into it. The old policy
/// steered purely by the orbit ring and the island edge, so any cover on
/// the path became a wall the bot pushed into every tick — the collision
/// solver held it out, and the fight between the two read as the bot
/// jittering/clipping against the block. Three cases, all pure:
///
///   * probe clear → intent unchanged;
///   * approaching a face → the into-wall component is dropped, the
///     tangent kept (the clean slide along the wall);
///   * dead head-on (no tangent left) or wedged inside the padded ring →
///     walk along/off the face, biased toward the foe so the detour stays
///     a duel move.
fn slide_around_cover(v: &BotView, dir: Vec2) -> Vec2 {
    slide_around_cover_within(v, dir, AVOID_LOOKAHEAD)
}

/// [`slide_around_cover`] with an explicit probe distance. The retrieval
/// walk shortens it to the fang, so the last stride cannot read a block
/// BEHIND the fang and slide the bot past the thing it came for.
fn slide_around_cover_within(v: &BotView, dir: Vec2, lookahead: f32) -> Vec2 {
    let d = dir.clamp_length_max(1.0);
    if d.length_squared() < 1e-4 {
        return d;
    }
    let probe = v.me + d.normalize_or_zero() * lookahead;
    for (center, half) in solids(v) {
        let pad = *half + Vec2::splat(AVOID_PAD);
        let rel = probe - *center;
        if rel.x.abs() >= pad.x || rel.y.abs() >= pad.y {
            continue;
        }
        let me_rel = v.me - *center;
        let outside_x = me_rel.x.abs() >= pad.x;
        let outside_y = me_rel.y.abs() >= pad.y;
        if !outside_x && !outside_y {
            // Already inside the padded ring (hugging the face): step out
            // along the shallow axis, keeping the along-wall intent.
            let push_x = pad.x - me_rel.x.abs();
            let push_y = pad.y - me_rel.y.abs();
            let out = if push_x <= push_y {
                Vec2::new(me_rel.x.signum(), d.y)
            } else {
                Vec2::new(d.x, me_rel.y.signum())
            };
            return out.clamp_length_max(1.0);
        }
        let mut out = d;
        if outside_x && (out.x * me_rel.x.signum()) < 0.0 {
            out.x = 0.0;
        }
        if outside_y && (out.y * me_rel.y.signum()) < 0.0 {
            out.y = 0.0;
        }
        if out.length_squared() < 0.05 {
            // Dead head-on: pick the tangent that rounds the block toward
            // the foe (a detour that still closes the duel).
            let tangent = if outside_x {
                Vec2::new(0.0, 1.0)
            } else {
                Vec2::new(1.0, 0.0)
            };
            let toward = (v.foe - v.me).dot(tangent);
            out = tangent * if toward < 0.0 { -1.0 } else { 1.0 };
        }
        return out.clamp_length_max(1.0);
    }
    d
}

/// Does the bot read the incoming fang this window? A shade always does;
/// the ladder from level 3, on some windows ([`dodge_chance`]).
fn reacts(v: &BotView) -> bool {
    v.style.is_some() || dodge_roll(v.frame, v.seed) < dodge_chance(v.difficulty)
}

/// Clearance (cm) kept from an incoming fang's line: a far fang's kill box
/// (13 cm grown 1.6x) plus the body's, with room to spare.
const LANE_CLEAR: f32 = 90.0;
/// How far down an incoming fang's line the bot keeps out of it.
const LANE_WATCH: f32 = 520.0;

/// The bot's offset from the line a fang is flying along, when the bot is
/// ahead of it on that line and close enough to care. `None` once the fang
/// has passed, or for one that is not moving.
fn lane_offset(v: &BotView, (tpos, tvel): (Vec2, Vec2)) -> Option<Vec2> {
    let dir = tvel.normalize_or_zero();
    if dir == Vec2::ZERO {
        return None;
    }
    let rel = v.me - tpos;
    let along = rel.dot(dir);
    (along > 0.0 && along < LANE_WATCH).then(|| rel - dir * along)
}

/// How far a dash carries (cm): its ticks at dash speed.
const DASH_REACH: f32 = (sim::DASH_DURATION_FRAMES as i32 * sim::DASH_SPEED_CM_PER_TICK) as f32;

/// Which way to dodge a fang flying along `tvel`, the bot `off` its line:
/// across the lane on the side the bot is already on, so a second press
/// (or the walk during the dash's cooldown) carries it further out, not
/// back over the line. It used to pick the side nearer the centre every
/// tick, and once a dodge had carried the bot off-centre that was back
/// across the lane: the bench caught tier 10 walking back into the fang
/// it had just dodged, at walking pace, in most of its deaths. Dead on the
/// line, or when its own side would land off the floor or in the chasm,
/// it takes the other.
fn dodge_side(v: &BotView, tvel: Vec2, off: Vec2) -> Vec2 {
    let perp = Vec2::new(-tvel.y, tvel.x).normalize_or_zero();
    let centre_ward = if (v.me + perp).length() < (v.me - perp).length() {
        perp
    } else {
        -perp
    };
    let own = if off.length() > 5.0 {
        if off.dot(perp) >= 0.0 { perp } else { -perp }
    } else {
        centre_ward
    };
    let lands = |dir: Vec2| {
        let land = v.me + dir * DASH_REACH;
        land.x.abs() < v.bounds.x * EDGE_MARGIN
            && land.y.abs() < v.bounds.y * EDGE_MARGIN
            && !v.hazards.iter().any(|(center, half)| {
                segment_hits_box(v.me, land, *center, *half + Vec2::splat(ROUTE_PAD))
            })
    };
    if lands(own) || !lands(-own) {
        own
    } else {
        -own
    }
}

/// Drop the part of a movement intent that would carry the bot back into
/// an incoming fang's lane, while it is reading the fang.
fn keep_out_of_lane(v: &BotView, dir: Vec2) -> Vec2 {
    let Some(threat) = v.threat.filter(|_| reacts(v)) else {
        return dir;
    };
    let Some(off) = lane_offset(v, threat) else {
        return dir;
    };
    if off.length() >= LANE_CLEAR * 2.0 || off.length() < 1.0 {
        return dir;
    }
    let away = off.normalize();
    let toward = dir.dot(-away);
    if toward > 0.0 {
        dir + away * toward
    } else {
        dir
    }
}

/// The duel's footwork. With a clear line to the foe it is the
/// range-keeping orbit; with cover in the way it is the walk to the
/// nearest spot the foe can be seen from ([`route_to_line`]). The orbit
/// alone could not leave a box: on the Gallery, whose centre is fenced by
/// two rails and two bars, the orbit's pull and the cover slide cancelled
/// out a few metres from an idle player hidden behind a bar, and the bot
/// stood still there, throwing into the bar, for ten minutes.
fn duel_move(v: &BotView) -> Vec2 {
    if line_blocked(v).is_some()
        && let Some(waypoint) = route_to_line(v)
    {
        // Straight at it: the route already keeps the body off every face,
        // and the slide's wider ring would close the very gaps it threads.
        return keep_out_of_lane(v, (waypoint - v.me).normalize_or_zero());
    }
    keep_out_of_lane(v, steer(v, orbit_dir(v)))
}

/// Grid pitch (cm) of the route search: half a body width, so every gap
/// a body fits through has a column of open cells in it.
const ROUTE_CELL: f32 = 20.0;
/// Padding (cm) the route keeps around solids: the body's half-extent plus
/// a little. Narrower than the slide's ring on purpose — the Gallery's
/// corner gaps are 92 cm wide, and the slide's ring leaves 4 cm of them.
const ROUTE_PAD: f32 = sim::PLAYER_HALF_EXTENT_CM as f32 + 8.0;
/// Clearance (cm) a fang's line keeps from cover: the fang's half-extent
/// plus a hair, so a line that grazes a corner counts as blocked.
const FANG_PAD: f32 = 16.0;
/// A spot closer to the foe than this is not one to walk to for a throw.
const ROUTE_MIN_RANGE: f32 = 220.0;

/// Where to walk, when cover blocks the throwing line, to get one back: a
/// breadth-first search over a grid of the floor — cells inside the edge
/// cushion and a body's width clear of every solid, cover and lethal ground
/// alike — from the bot's cell to the nearest cell with a clear line to the
/// foe. Returns the farthest point along that path the bot can walk to in a
/// straight line, so it cuts corners instead of stepping cell by cell.
/// `None` when no such spot exists (the foe is boxed in, or the floor has
/// crumbled to nothing), and the orbit takes over.
fn route_to_line(v: &BotView) -> Option<Vec2> {
    let half = v.bounds * EDGE_MARGIN;
    let cols = (2.0 * half.x / ROUTE_CELL) as i32 + 1;
    let rows = (2.0 * half.y / ROUTE_CELL) as i32 + 1;
    if half.x <= 0.0 || half.y <= 0.0 {
        return None;
    }
    let at = |c: i32, r: i32| {
        Vec2::new(
            -half.x + c as f32 * ROUTE_CELL,
            -half.y + r as f32 * ROUTE_CELL,
        )
    };
    let open = |p: Vec2| {
        !solids(v).any(|(center, h)| {
            let d = (p - *center).abs();
            d.x < h.x + ROUTE_PAD && d.y < h.y + ROUTE_PAD
        })
    };
    let sees_foe = |p: Vec2| {
        p.distance(v.foe) >= ROUTE_MIN_RANGE
            && !v
                .obstacles
                .iter()
                .any(|(center, h)| segment_hits_box(p, v.foe, *center, *h + Vec2::splat(FANG_PAD)))
    };
    let walkable = |a: Vec2, b: Vec2, pad: f32| {
        !solids(v).any(|(center, h)| segment_hits_box(a, b, *center, *h + Vec2::splat(pad)))
    };

    let index = |c: i32, r: i32| (r * cols + c) as usize;
    let start = (
        (((v.me.x + half.x) / ROUTE_CELL).round() as i32).clamp(0, cols - 1),
        (((v.me.y + half.y) / ROUTE_CELL).round() as i32).clamp(0, rows - 1),
    );
    let mut came_from = vec![u32::MAX; (cols * rows) as usize];
    came_from[index(start.0, start.1)] = index(start.0, start.1) as u32;
    let mut queue = std::collections::VecDeque::from([start]);
    let mut goal = None;
    while let Some((c, r)) = queue.pop_front() {
        let p = at(c, r);
        if (c, r) != start && sees_foe(p) {
            goal = Some((c, r));
            break;
        }
        for (dc, dr) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let (nc, nr) = (c + dc, r + dr);
            if nc < 0 || nr < 0 || nc >= cols || nr >= rows {
                continue;
            }
            let i = index(nc, nr);
            if came_from[i] != u32::MAX || !open(at(nc, nr)) {
                continue;
            }
            came_from[i] = index(c, r) as u32;
            queue.push_back((nc, nr));
        }
    }
    let (gc, gr) = goal?;

    // Walk the path back from the goal; the first cell (from the goal end)
    // the bot can reach in a straight line is where it heads. The line is
    // tested a little tighter than the cells, so a bot skimming a face is
    // not read as unable to leave it.
    let mut path = vec![index(gc, gr)];
    while let Some(&last) = path.last() {
        let prev = came_from[last] as usize;
        if prev == last {
            break;
        }
        path.push(prev);
    }
    let cell = |i: usize| at(i as i32 % cols, i as i32 / cols);
    let reach = path
        .iter()
        .take(path.len().saturating_sub(1))
        .map(|&i| cell(i))
        .find(|&p| walkable(v.me, p, ROUTE_PAD - 4.0));
    // Nothing reachable straight on (the bot is pressed against a face):
    // the next cell along, which steps it off.
    reach.or_else(|| path.iter().rev().nth(1).map(|&i| cell(i)))
}

/// The one movement filter: island-edge clamp, then the cover slide.
/// Every movement intent the policy emits goes through here; aim vectors
/// never do (aim is aim).
fn steer(v: &BotView, dir: Vec2) -> Vec2 {
    slide_around_cover(v, edge_safe(v, dir))
}

/// A sim rect as the (center, half-extents) pair the steering reads.
fn rect_center_half(rect: fixed_math::RectF) -> (Vec2, Vec2) {
    let (min_x, min_y) = rect.min.to_f32();
    let (max_x, max_y) = rect.max.to_f32();
    (
        Vec2::new((min_x + max_x) * 0.5, (min_y + max_y) * 0.5),
        Vec2::new((max_x - min_x) * 0.5, (max_y - min_y) * 0.5),
    )
}

/// Ticks between the match and the bot's picture of it, by level: 15 for
/// the sparring partner, one fewer per level, 5 from level 10 (a shade
/// reacts like a sharp human, 6). The brain used to see every frame as it
/// happened and answer on that same frame, which is what made a sharp bot
/// feel like a machine rather than a rival: no human dodges a fang the
/// tick it appears.
pub fn perception_lag(lvl: u32, shade: bool) -> usize {
    if shade {
        return 6;
    }
    15 - lvl.min(10) as usize
}

/// What the bot perceives of the other side on one tick.
#[derive(Clone, Copy, Debug)]
struct Perceived {
    foe: Vec2,
    foe_vel: Vec2,
    foe_alive: bool,
    threat: Option<(Vec2, Vec2)>,
}

/// The bot's memory: its picture of the opponent arrives
/// [`perception_lag`] ticks late. Its own body (position, charge, dash) is
/// felt at once. An incoming fang seen late is extrapolated along its
/// velocity, as a human tracks a moving thing — so the lag costs the bot
/// fangs that appeared within the window (a reaction time) without turning
/// every dodge into a dodge at where the fang used to be. The opponent's
/// position is not extrapolated here: what the bot does with the late
/// picture is its aim's business ([`lead_frac`]) — a low tier aims where
/// you WERE, a sharp one where you will be.
#[derive(Default, Debug)]
pub struct Brain {
    seen: std::collections::VecDeque<Perceived>,
}

impl Brain {
    /// Decide on `view`, with the opponent's part of it seen late.
    pub fn decide(&mut self, mut view: BotView) -> PlayerInput {
        let lag = perception_lag(view.difficulty, view.style.is_some());
        self.seen.push_back(Perceived {
            foe: view.foe,
            foe_vel: view.foe_vel,
            foe_alive: view.foe_alive,
            threat: view.threat,
        });
        while self.seen.len() > lag + 1 {
            self.seen.pop_front();
        }
        if let Some(old) = self.seen.front() {
            let late = (self.seen.len() - 1) as f32;
            view.foe = old.foe;
            view.foe_vel = old.foe_vel;
            view.foe_alive = old.foe_alive;
            view.threat = old.threat.map(|threat| ahead(threat, late));
        }
        bot_decide(&view)
    }

    /// Forget everything seen (a new match, a respawn of the bot itself).
    pub fn forget(&mut self) {
        self.seen.clear();
    }
}

/// A fang seen `ticks` ago, carried along its velocity to where it is now.
fn ahead((pos, vel): (Vec2, Vec2), ticks: f32) -> (Vec2, Vec2) {
    (pos + vel * ticks, vel)
}

/// Read the match for the duelist at `me` (the other handle is the foe).
/// `None` when it has nothing to decide — out of a round, or dead — and the
/// caller sends a neutral input.
pub fn view_for(
    world: &mut World,
    me: usize,
    difficulty: u32,
    style: Option<BotStyle>,
    seed: u32,
) -> Option<BotView> {
    let frame = world.resource::<FrameCount>().0;
    let state = *world.resource::<MatchState>();
    if !state.is_in_round() {
        return None;
    }
    let mut view = BotView {
        frame,
        seed,
        bounds: Vec2::new(ARENA_HALF_WIDTH_CM as f32, ARENA_HALF_HEIGHT_CM as f32),
        difficulty,
        style,
        ..default()
    };
    // Sudden-death crumble awareness — only where the storm exists (the
    // Pit and the Vigil never shrink; the bot shouldn't hug center there).
    if world.resource::<sim::SelectedArena>().0.crumbles()
        && let MatchState::InRound { expires_at_frame } = state
    {
        let remaining = expires_at_frame.saturating_sub(frame);
        view.bounds *= sudden_death_factor(remaining).to_num::<f32>();
    }
    // Blocking solids for the steering slide: cover blocks always, trees
    // while they still block (a felled stump is open ground). Read live so
    // a mid-round felling opens the path the same tick the sim opens it.
    {
        let mut walls = world.query::<&Wall>();
        for wall in walls.iter(world) {
            if matches!(wall.kind, WallKind::Obstacle) {
                view.obstacles.push(rect_center_half(wall.rect));
            }
        }
        let mut trees = world.query::<&BoneTree>();
        for tree in trees.iter(world) {
            if tree.blocks() {
                view.obstacles.push(rect_center_half(tree.rect));
            }
        }
    }
    // The Crossing's moat is lethal while its bridge is down.
    if world.resource::<sim::SelectedArena>().0 == sim::ArenaId::Crossing
        && !world.resource::<sim::BridgeState>().is_active(frame)
    {
        view.hazards.push(rect_center_half(sim::crossing_chasm()));
    }
    let mut me_alive = true;
    {
        let mut players = world.query::<(
            &Player,
            &PositionF,
            &VelocityF,
            &Dead,
            &DashState,
            &ThrowCharge,
        )>();
        for (p, pos, vel, dead, dash, charge) in players.iter(world) {
            let (x, y) = pos.0.to_f32();
            if p.handle == me {
                view.me = Vec2::new(x, y);
                view.my_charge = charge.0;
                view.can_dash = matches!(dash, DashState::Idle) && !dead.is_dying();
                me_alive = !dead.is_dying();
            } else {
                view.foe = Vec2::new(x, y);
                let (vx, vy) = vel.0.to_f32();
                view.foe_vel = Vec2::new(vx, vy);
                view.foe_alive = !dead.is_dying();
            }
        }
    }
    {
        let mut fangs = world.query::<(&Boomerang, &BoomerangMods, &PositionF, &VelocityF)>();
        let mut nearest = f32::MAX;
        for (boom, mods, pos, vel) in fangs.iter(world) {
            let (x, y) = pos.0.to_f32();
            let (vx, vy) = vel.0.to_f32();
            let p = Vec2::new(x, y);
            if boom.owner_handle == me {
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
    me_alive.then_some(view)
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
    fn level_zero_is_a_sparring_partner() {
        let mut v = base_view();
        v.difficulty = 0;
        // A fang bearing down: level 0 never dodges.
        v.threat = Some((v.me + Vec2::new(0.0, -80.0), Vec2::new(0.0, 24.0)));
        v.frame = 10; // drifting part of the period
        let drift = bot_decide(&v);
        assert_eq!(drift.buttons & PlayerInput::DASH_DOWN, 0, "no dodge");
        assert_eq!(
            drift.buttons & PlayerInput::THROW_DOWN,
            0,
            "drifting, not charging"
        );
        // The plant: charging with the aim showing.
        v.frame = SPAR_PLANT_AT;
        let plant = bot_decide(&v);
        assert_ne!(plant.buttons & PlayerInput::THROW_DOWN, 0);
        assert_ne!(
            plant.buttons & PlayerInput::AIM_ACTIVE,
            0,
            "the plant is telegraphed"
        );
        assert_eq!(plant.buttons & PlayerInput::DASH_DOWN, 0);
        // At the commit charge it lets go: a slow fang.
        v.my_charge = SPAR_COMMIT;
        let release = bot_decide(&v);
        assert_eq!(release.buttons & PlayerInput::THROW_DOWN, 0);
        assert!(
            SPAR_COMMIT < throw_at_charge(1),
            "slower than any laddered throw"
        );
    }

    #[test]
    fn the_lag_shrinks_up_the_ladder() {
        assert_eq!(perception_lag(0, false), 15);
        assert_eq!(perception_lag(1, false), 14);
        assert_eq!(perception_lag(10, false), 5);
        assert_eq!(perception_lag(11, false), 5);
        assert_eq!(perception_lag(0, true), 6);
    }

    #[test]
    fn the_brain_sees_the_foe_late() {
        let mut brain = Brain::default();
        let mut v = base_view();
        v.difficulty = 10; // lag 5
        for _ in 0..6 {
            brain.decide(v.clone());
        }
        let before = v.foe;
        v.foe = Vec2::new(200.0, -300.0);
        brain.decide(v.clone());
        assert_eq!(
            brain.seen.front().unwrap().foe,
            before,
            "the move has not reached it"
        );
        for _ in 0..5 {
            brain.decide(v.clone());
        }
        assert_eq!(
            brain.seen.front().unwrap().foe,
            v.foe,
            "five ticks later it has"
        );
        brain.forget();
        assert!(brain.seen.is_empty());
    }

    #[test]
    fn a_late_fang_is_tracked_to_where_it_is_now() {
        let (pos, vel) = ahead((Vec2::new(0.0, -100.0), Vec2::new(0.0, 30.0)), 5.0);
        assert_eq!(pos, Vec2::new(0.0, 50.0));
        assert_eq!(vel, Vec2::new(0.0, 30.0));
    }

    #[test]
    fn the_bot_does_not_walk_into_a_hazard() {
        let mut v = base_view();
        v.me = Vec2::new(0.0, 150.0);
        v.foe = Vec2::new(0.0, -300.0);
        v.hazards = vec![(Vec2::ZERO, Vec2::new(500.0, 60.0))];
        // Straight at the moat: the step toward it is refused.
        let step = steer(&v, Vec2::new(0.0, -1.0));
        assert!(step.y >= 0.0, "walks into the chasm: {step:?}");
        // A fang lying in the moat is not walked to.
        assert!(!retrievable(&v, Vec2::new(0.0, 0.0)));
    }

    #[test]
    fn a_hazard_does_not_block_the_line_but_cover_does() {
        let mut v = base_view();
        v.hazards = vec![(Vec2::ZERO, Vec2::new(500.0, 60.0))];
        assert_eq!(line_blocked(&v), None, "fangs fly over the chasm");
        v.obstacles = vec![(Vec2::new(0.0, 0.0), Vec2::new(60.0, 60.0))];
        assert_eq!(line_blocked(&v), Some(Vec2::ZERO));
    }

    #[test]
    fn behind_cover_the_orbit_commits_to_one_side() {
        let mut v = base_view();
        v.me = Vec2::new(30.0, 300.0);
        v.foe = Vec2::new(0.0, -300.0);
        v.obstacles = vec![(Vec2::new(0.0, 0.0), Vec2::new(120.0, 40.0))];
        // The strafe direction does not depend on which two-second beat it is.
        let a = {
            v.frame = 0;
            orbit_dir(&v)
        };
        let b = {
            v.frame = 120;
            orbit_dir(&v)
        };
        assert!(a.x.signum() == b.x.signum(), "{a:?} vs {b:?}");
    }

    /// The Gallery's cover, as the brain sees it.
    fn gallery() -> Vec<(Vec2, Vec2)> {
        sim::arena_obstacles_for(sim::ArenaId::Gallery)
            .into_iter()
            .map(|w| rect_center_half(w.rect))
            .collect()
    }

    #[test]
    fn boxed_in_the_gallery_the_bot_walks_out_to_a_clear_line() {
        // The stall the bench found: the bot inside the Gallery's fenced
        // centre, an idle player hidden behind the lower bar. Walking the
        // footwork tick by tick (11 cm per tick at full stick) must reach a
        // spot with a clear line within a few seconds.
        let mut v = base_view();
        v.obstacles = gallery();
        v.me = Vec2::new(70.0, 60.0);
        v.foe = Vec2::new(0.0, -300.0);
        assert!(line_blocked(&v).is_some());
        for tick in 0..300 {
            v.frame = tick;
            if line_blocked(&v).is_none() {
                return;
            }
            let step = duel_move(&v);
            assert!(step.length() > 0.3, "stalled at {:?} (step {step:?})", v.me);
            v.me += step * 11.0;
        }
        panic!("still blocked after 5 s, at {:?}", v.me);
    }

    #[test]
    fn the_route_never_leads_into_cover_or_off_the_floor() {
        let mut v = base_view();
        v.obstacles = gallery();
        v.foe = Vec2::new(0.0, -300.0);
        for me in [(70.0, 60.0), (-90.0, 100.0), (0.0, 120.0), (150.0, -60.0)] {
            v.me = Vec2::from(me);
            let goal = route_to_line(&v).expect("a spot that sees the foe");
            assert!(goal.x.abs() <= v.bounds.x * EDGE_MARGIN + 1.0);
            assert!(goal.y.abs() <= v.bounds.y * EDGE_MARGIN + 1.0);
            for (center, half) in &v.obstacles {
                let d = (goal - *center).abs();
                assert!(d.x >= half.x || d.y >= half.y, "{goal:?} is inside cover");
            }
        }
    }

    #[test]
    fn the_sparring_partner_holds_its_throw_while_cover_is_in_the_way() {
        let mut v = base_view();
        v.difficulty = 0;
        v.obstacles = gallery();
        v.me = Vec2::new(70.0, 60.0);
        v.foe = Vec2::new(0.0, -300.0);
        v.frame = SPAR_PLANT_AT + 1; // its beat has come round
        assert_eq!(bot_decide(&v).buttons & PlayerInput::THROW_DOWN, 0);
        // With the line clear it plants.
        v.obstacles.clear();
        assert_ne!(bot_decide(&v).buttons & PlayerInput::THROW_DOWN, 0);
    }

    #[test]
    fn the_aim_wobble_sweeps_whatever_the_seed() {
        for seed in [0, 7, 0x5EED_0000, u32::MAX] {
            let mut v = base_view();
            v.seed = seed;
            let angles: Vec<f32> = (0..120)
                .map(|f| {
                    v.frame = f;
                    let aim = aim_at_foe(&v);
                    aim.y.atan2(aim.x)
                })
                .collect();
            let lo = angles.iter().copied().fold(f32::MAX, f32::min);
            let hi = angles.iter().copied().fold(f32::MIN, f32::max);
            assert!(
                hi - lo > wobble_amp(v.difficulty),
                "seed {seed:#x}: {lo}..{hi}"
            );
        }
    }

    #[test]
    fn a_dodge_mid_charge_lets_the_fang_go_at_the_foe() {
        let mut v = base_view();
        v.difficulty = 10;
        v.style = Some(BotStyle {
            commit_frac: 0.7,
            dodge_radius: 240.0,
            wobble: 0.0,
            range: 440.0,
        });
        v.my_charge = 6;
        v.threat = Some((v.me + Vec2::new(-100.0, 0.0), Vec2::new(30.0, 0.0)));
        let input = bot_decide(&v);
        // THROW up (the release) with the stick on the foe, DASH pressed.
        assert_eq!(input.buttons & PlayerInput::THROW_DOWN, 0);
        assert_ne!(input.buttons & PlayerInput::DASH_DOWN, 0);
        let stick = Vec2::new(f32::from(input.stick_x), f32::from(input.stick_y));
        let to_foe = (v.foe - v.me).normalize();
        assert!(stick.normalize().dot(to_foe) > 0.99, "{stick:?}");
    }

    #[test]
    fn a_sharp_bot_throws_where_a_walker_will_be() {
        let mut v = base_view();
        v.me = Vec2::new(0.0, 300.0);
        v.foe = Vec2::new(0.0, -140.0);
        v.foe_vel = Vec2::new(11.0, 0.0); // walking right
        v.my_charge = 20;
        let angle_x = |v: &BotView| led_target(v, lead_frac(v.difficulty)).x;
        // A green bot throws at where the foe is.
        v.difficulty = 1;
        assert_eq!(angle_x(&v), 0.0);
        // A sharp one leads, and leads more the sharper it is.
        v.difficulty = 5;
        let mid = angle_x(&v);
        v.difficulty = 10;
        let sharp = angle_x(&v);
        assert!(mid > 30.0 && sharp > mid, "{mid} then {sharp}");
        // The lead lands where the walker is when the fang arrives.
        let speed = release_speed(20);
        let lag = perception_lag(10, false) as f32;
        let arrival = v.foe + v.foe_vel * (lag + v.me.distance(Vec2::new(sharp, -140.0)) / speed);
        assert!((arrival.x - sharp).abs() < 4.0, "{arrival:?} vs {sharp}");
    }

    #[test]
    fn a_dash_is_not_a_heading_worth_leading() {
        let mut v = base_view();
        v.difficulty = 10;
        v.foe_vel = Vec2::new(42.0, 0.0); // mid-dash
        let dashing = led_target(&v, 1.0);
        v.foe_vel = Vec2::new(LEAD_MAX_SPEED, 0.0);
        assert_eq!(dashing, led_target(&v, 1.0));
    }

    /// A view whose dodge roll fires on this frame (a shade always reads).
    fn reading(mut v: BotView) -> BotView {
        v.style = Some(BotStyle {
            commit_frac: 0.5,
            dodge_radius: 100.0,
            wobble: 0.2,
            range: 440.0,
        });
        v
    }

    #[test]
    fn a_bot_in_a_fang_lane_walks_out_of_it() {
        let mut v = reading(base_view());
        v.me = Vec2::new(30.0, 0.0);
        // A fang 400 cm off, flying straight at the bot's stretch of line.
        v.threat = Some((Vec2::new(0.0, -400.0), Vec2::new(0.0, 15.0)));
        let input = bot_decide(&v);
        assert!(
            input.stick_x > 60,
            "should step out to its own side: {input:?}"
        );
        assert_eq!(
            input.buttons & PlayerInput::DASH_DOWN,
            0,
            "a walk, not a dash, out there"
        );
    }

    #[test]
    fn the_footwork_does_not_step_back_into_the_lane() {
        let mut v = reading(base_view());
        v.me = Vec2::new(120.0, 0.0);
        v.threat = Some((Vec2::new(0.0, -400.0), Vec2::new(0.0, 15.0)));
        let into = keep_out_of_lane(&v, Vec2::new(-1.0, 0.3));
        assert!(into.x >= -1e-4, "{into:?}");
        // Once the fang has passed, the lane is just floor.
        v.threat = Some((Vec2::new(0.0, 100.0), Vec2::new(0.0, 15.0)));
        assert_eq!(
            keep_out_of_lane(&v, Vec2::new(-1.0, 0.3)),
            Vec2::new(-1.0, 0.3)
        );
    }

    #[test]
    fn a_dodge_goes_out_on_its_own_side_not_back_across_the_lane() {
        let mut v = reading(base_view());
        // Already right of a fang flying up the x = -60 line, and right of
        // centre: the centre is back across the lane.
        v.me = Vec2::new(-20.0, 0.0);
        v.threat = Some((Vec2::new(-60.0, -80.0), Vec2::new(0.0, 15.0)));
        let input = bot_decide(&v);
        assert!(input.stick_x > 60, "{input:?}");
        assert_ne!(input.buttons & PlayerInput::DASH_DOWN, 0);
        // Its own side would land off the floor: the other one.
        v.me = Vec2::new(v.bounds.x * 0.78, 0.0);
        v.threat = Some((Vec2::new(v.me.x - 40.0, -80.0), Vec2::new(0.0, 15.0)));
        assert!(bot_decide(&v).stick_x < -60);
    }

    #[test]
    fn a_fang_that_will_pass_wide_is_not_dodged() {
        let mut v = reading(base_view());
        v.me = Vec2::new(150.0, 0.0);
        v.threat = Some((Vec2::new(0.0, -60.0), Vec2::new(0.0, 15.0)));
        assert_eq!(bot_decide(&v).buttons & PlayerInput::DASH_DOWN, 0);
    }

    #[test]
    fn the_amble_moves_and_never_throws() {
        let v = base_view();
        let input = amble(&v);
        assert_eq!(input.buttons, 0);
        assert!(input.stick_x != 0 || input.stick_y != 0);
    }

    #[test]
    fn level_zero_never_steers_a_recall() {
        let mut v = base_view();
        v.difficulty = 0;
        v.fangs_out = 1;
        v.my_fang = Some((Vec2::new(0.0, 0.0), true));
        assert_eq!(bot_decide(&v).buttons & PlayerInput::AIM_ACTIVE, 0);
    }

    #[test]
    fn charges_while_free_and_unthreatened() {
        let mut v = base_view();
        v.frame += 1; // off the re-arm beat (frame % 8 == 0 releases at charge 0)
        let input = bot_decide(&v);
        assert!(
            input.buttons & PlayerInput::THROW_DOWN != 0,
            "should charge"
        );
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
        assert!(
            plant.buttons & PlayerInput::AIM_ACTIVE != 0,
            "visible plant"
        );
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
        assert_eq!(threat_radius(2), 0.0, "no dodge below level 3");
        assert_eq!(dodge_chance(2), 0.0);
        assert!(threat_radius(3) < threat_radius(6), "dodges sooner");
        assert!(dodge_chance(3) < dodge_chance(6), "dodges more often");
        assert!(wobble_amp(1) > wobble_amp(4), "aim tightens");
    }

    /// The report that reshaped the ramp: after one or two matches the bot
    /// was "way too cracked". A player's first few gauntlet tiers must
    /// stay soft — low commit charge, no reflex, a wide spray.
    #[test]
    fn the_first_tiers_stay_practice_partners() {
        for lvl in 0..=2 {
            assert_eq!(dodge_chance(lvl), 0.0, "level {lvl} never dodges");
            assert!(throw_at_charge(lvl) <= CHARGE_MAX_FRAMES * 2 / 5);
            assert!(wobble_amp(lvl) >= 0.35);
        }
        // Even a tier-3 bot lets most throws through.
        assert!(dodge_chance(3) <= 0.25);
        // The rubber band: a notch up for a player leading by two, a notch
        // down for a player trailing by two, never below the sparring
        // partner, and level otherwise.
        assert_eq!(rubber_band(3, 0, 0), 3);
        assert_eq!(rubber_band(3, 1, 0), 3);
        assert_eq!(rubber_band(3, 2, 0), 4);
        assert_eq!(rubber_band(3, 0, 2), 2);
        assert_eq!(rubber_band(0, 0, 3), 0);
        assert_eq!(rubber_band(0, 4, 1), 1);
    }

    #[test]
    fn gauntlet_tiers_keep_sharpening_but_hit_ceilings() {
        // Up the ladder the ramps keep moving...
        assert!(throw_at_charge(6) > throw_at_charge(4));
        assert!(threat_radius(6) > threat_radius(4));
        assert!(wobble_amp(6) < wobble_amp(4));
        // ...and saturate by the ladder's top instead of becoming an
        // aimbot wall.
        let top = 10 + 1; // the gauntlet's cap (app: grudge::GAUNTLET_MAX_TIER) + a rubber-band notch
        assert_eq!(throw_at_charge(40), throw_at_charge(top));
        assert_eq!(threat_radius(40), threat_radius(top));
        assert_eq!(dodge_chance(40), dodge_chance(top));
        assert_eq!(wobble_amp(40), wobble_amp(top));
        assert_eq!(threat_radius(40), 240.0);
        assert_eq!(dodge_chance(40), 0.75);
        assert_eq!(wobble_amp(40), 0.10);
        // The commit charge never reaches a human's full-power shot.
        assert!(throw_at_charge(40) < CHARGE_MAX_FRAMES);
    }

    /// The first frame at or after `from` whose dodge window rolls `hit`.
    fn frame_rolling(from: u32, lvl: u32, hit: bool) -> u32 {
        (from..from + 100_000)
            .step_by(DODGE_ROLL_WINDOW as usize)
            .find(|&f| (dodge_roll(f, 0) < dodge_chance(lvl)) == hit)
            .expect("the roll covers both outcomes")
    }

    #[test]
    fn dashes_through_an_incoming_fang() {
        let mut v = base_view();
        v.difficulty = 6;
        v.threat = Some((v.me + Vec2::new(0.0, -120.0), Vec2::new(0.0, 24.0)));
        v.frame = frame_rolling(600, v.difficulty, true);
        let input = bot_decide(&v);
        assert!(input.buttons & PlayerInput::DASH_DOWN != 0, "graze reflex");
    }

    #[test]
    fn the_reflex_misses_some_throws() {
        let mut v = base_view();
        v.difficulty = 6;
        v.threat = Some((v.me + Vec2::new(0.0, -120.0), Vec2::new(0.0, 24.0)));
        v.frame = frame_rolling(600, v.difficulty, false);
        let input = bot_decide(&v);
        assert!(
            input.buttons & PlayerInput::DASH_DOWN == 0,
            "an off-window throw goes unanswered"
        );
        // And the rate tracks the table: over many windows a tier-6 bot
        // reacts roughly dodge_chance(6) of the time, never always.
        let windows = 2000;
        let hits = (0..windows)
            .filter(|w| dodge_roll(w * DODGE_ROLL_WINDOW, 0) < dodge_chance(6))
            .count() as f32;
        let rate = hits / windows as f32;
        assert!((rate - dodge_chance(6)).abs() < 0.06, "rate {rate}");
        // One decision per window: every frame inside it agrees.
        let base = 90 * DODGE_ROLL_WINDOW;
        assert!((base..base + DODGE_ROLL_WINDOW).all(|f| dodge_roll(f, 0) == dodge_roll(base, 0)));
    }

    #[test]
    fn steers_the_returning_fang_at_the_foe() {
        let mut v = base_view();
        v.fangs_out = 1;
        v.my_fang = Some((Vec2::new(200.0, 0.0), true));
        let input = bot_decide(&v);
        assert!(input.buttons & PlayerInput::AIM_ACTIVE != 0, "steering");
        assert!(
            input.stick_x < 0 || input.stick_y < 0,
            "bends toward the foe"
        );
    }

    #[test]
    fn edge_override_pulls_back_from_the_rim() {
        let mut v = base_view();
        v.me = Vec2::new(480.0, 300.0); // near the +x rim
        let d = edge_safe(&v, Vec2::new(1.0, 0.0));
        assert!(d.x < 0.0, "never walks off the island");
    }

    /// A cover block dead ahead on the +x path, the bot west of it.
    fn view_with_block() -> BotView {
        let mut v = base_view();
        v.me = Vec2::new(-200.0, 0.0);
        v.foe = Vec2::new(400.0, 300.0);
        v.obstacles = vec![(Vec2::new(0.0, 0.0), Vec2::new(60.0, 60.0))];
        v
    }

    #[test]
    fn clear_intent_passes_through_untouched() {
        let v = view_with_block();
        // Walking away from the block: no obstacle on the probe, no change.
        let d = steer(&v, Vec2::new(-0.8, 0.2));
        assert!((d - Vec2::new(-0.8, 0.2)).length() < 1e-4);
    }

    #[test]
    fn diagonal_intent_slides_along_the_face() {
        let v = view_with_block();
        // Aiming through the block's west face at a diagonal: the into-wall
        // x is dropped, the tangent y survives — the clean slide.
        let d = steer(&v, Vec2::new(0.7, 0.5));
        assert_eq!(d.x, 0.0, "into-wall component dropped");
        assert!(d.y > 0.0, "tangent kept: {d:?}");
    }

    #[test]
    fn head_on_intent_rounds_the_block_toward_the_foe() {
        let v = view_with_block();
        // Dead head-on leaves no tangent; the detour goes the foe's way
        // (+y here) instead of an arbitrary side.
        let d = steer(&v, Vec2::new(1.0, 0.0));
        assert!(d.x.abs() < 1e-4, "not into the wall");
        assert!(d.y > 0.0, "rounds toward the foe: {d:?}");
    }

    /// The hang the gauntlet showed: the bot's own fang knocked Loose into
    /// the edge cushion. The walk wanted out, `edge_safe` shoved back in,
    /// and the two alternated frame by frame — a duelist stuck on the spot,
    /// strobing (the facing row flips with the velocity sign), and, because
    /// this branch preempts the throw, disarmed for the rest of the round.
    #[test]
    fn a_fang_dropped_past_the_cushion_is_recalled_not_chased() {
        let mut v = base_view();
        v.me = Vec2::new(0.0, 300.0);
        v.fangs_out = 1;
        v.my_loose = Some(Vec2::new(v.bounds.x * 0.97, 300.0)); // out in the rim
        assert!(!retrievable(&v, v.my_loose.unwrap()));

        // Over a full pulse period the bot both MOVES and lands a recall
        // press — never the frozen, buttonless stare the walk produced.
        let mut moved = false;
        let mut recalled = false;
        for f in 0..24 {
            v.frame = 600 + f;
            let input = bot_decide(&v);
            moved |= input.stick_x != 0 || input.stick_y != 0;
            recalled |= input.buttons & PlayerInput::THROW_DOWN != 0;
        }
        assert!(moved, "keeps orbiting instead of vibrating in place");
        assert!(recalled, "pulses THROW so the loose fang is reeled home");
    }

    #[test]
    fn a_fang_dropped_inside_cover_is_recalled_not_chased() {
        let mut v = view_with_block();
        v.fangs_out = 1;
        // Settled against the block's west face, inside the padded ring the
        // cover slide will not cross.
        v.my_loose = Some(Vec2::new(-70.0, 0.0));
        assert!(!retrievable(&v, v.my_loose.unwrap()));
        let mut recalled = false;
        for f in 0..24 {
            v.frame = 600 + f;
            recalled |= bot_decide(&v).buttons & PlayerInput::THROW_DOWN != 0;
        }
        assert!(recalled, "reels it out of the cover it cannot walk into");
    }

    /// Cover between the bot and the fang is the third way the walk could
    /// approach forever: the slide steers AROUND the block, and around is
    /// a detour a memoryless policy re-decides every frame.
    #[test]
    fn a_fang_behind_cover_is_recalled_not_chased() {
        let mut v = view_with_block(); // block at origin, bot at (-200, 0)
        v.fangs_out = 1;
        // Open floor, well clear of the block's ring — but straight through
        // the block from where the bot is standing.
        v.my_loose = Some(Vec2::new(200.0, 0.0));
        assert!(!retrievable(&v, v.my_loose.unwrap()), "corridor is blocked");
        // Step around to the same distance with the block off the line, and
        // the bot walks it down as before.
        v.me = Vec2::new(0.0, -300.0);
        v.my_loose = Some(Vec2::new(0.0, -450.0));
        assert!(retrievable(&v, v.my_loose.unwrap()), "clear corridor walks");
    }

    #[test]
    fn a_short_probe_does_not_read_past_the_fang() {
        // The fang is 60 cm east. A block sits behind it, near enough that
        // the full-length probe (AVOID_LOOKAHEAD = 130) lands inside the
        // block's padded ring — so the slide swerves the bot sideways past
        // the very thing it walked over for, on every pass.
        let mut v = base_view();
        v.me = Vec2::new(0.0, 0.0);
        v.obstacles = vec![(Vec2::new(180.0, 0.0), Vec2::new(60.0, 60.0))];
        let fang = Vec2::new(60.0, 0.0); // clear of the ring, which starts at 76
        assert!(retrievable(&v, fang), "the corridor to the fang is clear");
        assert!(
            slide_around_cover(&v, Vec2::new(1.0, 0.0)).x < 1.0,
            "the full-length probe reads the block behind the fang and swerves"
        );
        let short = slide_around_cover_within(&v, Vec2::new(1.0, 0.0), fang.length());
        assert_eq!(
            short,
            Vec2::new(1.0, 0.0),
            "clamped probe walks straight in"
        );
    }

    #[test]
    fn a_fang_on_open_ground_is_still_walked_down() {
        let mut v = base_view();
        v.fangs_out = 1;
        v.my_loose = Some(Vec2::new(200.0, 300.0)); // open floor, east of it
        assert!(retrievable(&v, v.my_loose.unwrap()));
        let input = bot_decide(&v);
        assert!(input.stick_x > 0, "walks at it: {input:?}");
        assert_eq!(input.buttons, 0, "no recall needed, no charge armed");
    }

    #[test]
    fn the_retrieval_walk_reaches_further_out_than_the_orbit() {
        let mut v = base_view();
        v.me = Vec2::new(v.bounds.x * 0.88, 0.0); // inside the orbit cushion
        // The orbit refuses to push further out here...
        assert!(edge_safe(&v, Vec2::new(1.0, 0.0)).x < 0.0);
        // ...while the retrieval walk is allowed to finish the job.
        assert!(edge_clamp(&v, Vec2::new(1.0, 0.0), RETRIEVE_MARGIN).x > 0.0);
    }

    #[test]
    fn wedged_against_the_face_walks_off_it() {
        let mut v = view_with_block();
        // Standing inside the padded ring (grinding the west face) while
        // still pushing east: the slide steps it OFF the face.
        v.me = Vec2::new(-80.0, 0.0);
        let d = steer(&v, Vec2::new(1.0, 0.0));
        assert!(d.x < 0.0, "steps away from the face: {d:?}");
    }
}
