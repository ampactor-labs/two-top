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
    ARENA_HALF_HEIGHT_CM, ARENA_HALF_WIDTH_CM, BoneTree, Boomerang, BoomerangMods, BoomerangState,
    CHARGE_MAX_FRAMES, DashState, Dead, FrameCount, GgrsCfg, MatchState, Player, PlayerInput,
    PositionF, ThrowCapacity, ThrowCharge, VelocityF, Wall, WallKind, sudden_death_factor,
};

/// Practice mode: a local match against the bot (forces a local session
/// even on online builds; `start_matchbox` checks it).
#[derive(Resource, Default)]
pub struct PracticeMode(pub bool);

/// The handle the bot drives (the far/top duelist; the human is handle 0).
pub const BOT_HANDLE: usize = 1;

/// Everything the policy looks at, in plain f32 (inputs are not sim state —
/// they only become deterministic once they enter the wire pipeline).
#[derive(Debug, Clone, Default)]
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
    /// Practice difficulty: the persistent gauntlet tier plus a gentle
    /// in-match ramp (`in_match_ramp`). 0 = passive sparring dummy. The ramp
    /// used to add a notch for every kill, then every second kill — either
    /// way the bot sharpened fastest exactly when you were winning. It is
    /// now a single notch late in the match.
    pub difficulty: u32,
    /// Blocking solids on the field — cover blocks and standing trees, as
    /// (center, half-extents) in cm. The steering slides along these; the
    /// old bounds-only policy walked orbit paths straight through cover
    /// and ground against the face every tick (the wall-jitter read).
    pub obstacles: Vec<(Vec2, Vec2)>,
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

/// The armed shade, if any: the fitted style plus the identity it wears
/// (mint axes + the tape header's honest name). Set by the rivals screen,
/// cleared on returning to the Title.
#[derive(Resource, Default)]
pub struct ShadeStyle(pub Option<ShadeSpec>);

#[derive(Clone, Debug)]
pub struct ShadeSpec {
    pub style: BotStyle,
    pub install_id: u128,
    pub name: String,
}

/// Preferred dueling range (cm). Inside it the bot backs off, outside it
/// closes — with an orbit component so it never runs a straight line.
const PREFERRED_RANGE: f32 = 440.0;
/// Ticks of visible plant (AIM_ACTIVE) before the release.
const PLANT_TICKS: u32 = 5;

// ---- Difficulty ramp (practice mode) ----
// The bot starts as a passive dummy and sharpens one notch per gauntlet
// tier. Level 0 never attacks or dodges; each level raises the commit
// charge (so throws hit harder and reach farther), the dodge range and the
// odds it actually reacts, and the aim accuracy.
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

/// The in-match notch: +1 once the player has landed three kills, never
/// more. Keeps the "it's learning you" read without a mid-match spike.
fn in_match_ramp(player_kills: u8) -> u32 {
    u32::from(player_kills >= 3)
}

/// Deterministic 0..1 roll for the dodge window containing `frame`. The bot
/// is an input source, not sim state — but a pure function of the frame
/// keeps `bot_decide` testable and the tape reproducible.
fn dodge_roll(frame: u32) -> f32 {
    let mut x = (frame / DODGE_ROLL_WINDOW).wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
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

    // Level 0 — a passive sparring dummy: it just ambles around slowly and
    // never throws or dodges, so the player warms up and lands the first free
    // kill. Every kill the player scores raises the level by one.
    if lvl == 0 && v.style.is_none() {
        return input_from(wander_dir(v), 0);
    }

    // 1) Survival dodge: a lethal fang closing in → dash through it. The bot
    //    only starts protecting itself at level 3; below that it eats the
    //    player's throws, and above it the reflex fires on some throws, not
    //    all (`dodge_chance`). A shade dodges from the first tick and every
    //    time — its reflex is the rival's, not the ladder's.
    let reacts = v.style.is_some() || dodge_roll(v.frame) < dodge_chance(lvl);
    if reacts && let Some((tpos, tvel)) = v.threat {
        let to_me = v.me - tpos;
        let radius = v
            .style
            .map_or_else(|| threat_radius(lvl), |s| s.dodge_radius);
        if tvel.dot(to_me) > 0.0 && to_me.length() < radius {
            // Perpendicular to the fang's path, biased toward the center so
            // the dodge never carries the bot off the island.
            let perp = Vec2::new(-tvel.y, tvel.x).normalize_or_zero();
            let dir = if (v.me + perp * 100.0).length() < (v.me - perp * 100.0).length() {
                perp
            } else {
                -perp
            };
            let buttons = if v.can_dash {
                PlayerInput::DASH_DOWN
            } else {
                0
            };
            return input_from(steer(v, dir), buttons);
        }
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
            steer(v, orbit_dir(v)),
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
        let dir = orbit_dir(v);
        return input_from(
            steer(v, dir),
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
        return input_from(steer(v, orbit_dir(v)), 0);
    }
    input_from(steer(v, orbit_dir(v)), PlayerInput::THROW_DOWN)
}

/// Range-keeping orbit: radial correction toward the preferred ring plus a
/// slowly alternating tangential strafe.
fn orbit_dir(v: &BotView) -> Vec2 {
    let to_foe = v.foe - v.me;
    let dist = to_foe.length().max(1.0);
    let range = v.style.map_or(PREFERRED_RANGE, |s| s.range);
    let radial = to_foe / dist * ((dist - range) / 200.0).clamp(-1.0, 1.0);
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
    // tighter (still imperfect) aim once the player has been winning. A
    // shade sprays exactly as wide as its rival planted tight.
    let amp = v
        .style
        .map_or_else(|| wobble_amp(v.difficulty), |s| s.wobble);
    let wobble = (v.frame as f32 * 0.11).sin() * amp;
    Vec2::from_angle(wobble).rotate(base)
}

/// Level-0 idle: a slow, gentle wander (a Lissajous drift), clamped to the
/// island. Reads as a dummy ambling around, not tracking or attacking.
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
fn retrievable(v: &BotView, loose: Vec2) -> bool {
    if loose.x.abs() > v.bounds.x * RETRIEVE_MARGIN || loose.y.abs() > v.bounds.y * RETRIEVE_MARGIN
    {
        return false;
    }
    !v.obstacles.iter().any(|(center, half)| {
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
    for (center, half) in &v.obstacles {
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

/// Collect the view + queue the input patch. Runs in `ReadInputs`, ordered
/// after the platform source so the queued closure lands on the fresh map.
pub fn drive_bot(world: &mut World) {
    if !world.resource::<PracticeMode>().0 {
        return;
    }
    let frame = world.resource::<FrameCount>().0;
    let in_round = world.resource::<MatchState>().is_in_round();

    // Difficulty = the persisted GAUNTLET tier plus one late notch once the
    // player has three kills this match (score.p0 — the player is always
    // handle 0 in practice). A fresh install starts at the passive dummy,
    // which wakes up into a lobber only near the end of the first match.
    let tier = world
        .resource::<crate::grudge::CareerRecord>()
        .gauntlet_tier;
    let difficulty = tier + in_match_ramp(world.resource::<sim::MatchScore>().p0);

    let mut view = BotView {
        frame,
        bounds: Vec2::new(ARENA_HALF_WIDTH_CM as f32, ARENA_HALF_HEIGHT_CM as f32),
        difficulty,
        style: world.resource::<ShadeStyle>().0.as_ref().map(|s| s.style),
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
        let mut fangs = world.query::<(&Boomerang, &BoomerangMods, &PositionF, &VelocityF)>();
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

/// Returning to the Title dissolves the shade: practice reverts to the
/// plain gauntlet bot, and nothing armed lingers into the next summons.
fn disarm_shade(mut shade: ResMut<ShadeStyle>, mut practice: ResMut<PracticeMode>) {
    if shade.0.take().is_some() {
        practice.0 = false;
    }
}

pub struct BotPlugin;

impl Plugin for BotPlugin {
    fn build(&self, app: &mut App) {
        // TWOTOP_PRACTICE=1 boots straight into practice (pairs with
        // TWOTOP_AUTOSTART for headless capture verification of the bot).
        let boot_practice = std::env::var("TWOTOP_PRACTICE").is_ok_and(|v| v == "1");
        app.insert_resource(PracticeMode(boot_practice));
        app.init_resource::<ShadeStyle>();
        app.add_systems(
            bevy::state::state::OnEnter(crate::screen::AppScreen::Title),
            disarm_shade,
        );
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
        // The in-match ramp is one late notch, not a notch per kill.
        assert_eq!(in_match_ramp(0), 0);
        assert_eq!(in_match_ramp(2), 0);
        assert_eq!(in_match_ramp(3), 1);
        assert_eq!(in_match_ramp(4), 1);
    }

    #[test]
    fn gauntlet_tiers_keep_sharpening_but_hit_ceilings() {
        // Up the ladder the ramps keep moving...
        assert!(throw_at_charge(6) > throw_at_charge(4));
        assert!(threat_radius(6) > threat_radius(4));
        assert!(wobble_amp(6) < wobble_amp(4));
        // ...and saturate by the ladder's top instead of becoming an
        // aimbot wall.
        let top = crate::grudge::GAUNTLET_MAX_TIER + 1; // cap + the late notch
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
            .find(|&f| (dodge_roll(f) < dodge_chance(lvl)) == hit)
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
            .filter(|w| dodge_roll(w * DODGE_ROLL_WINDOW) < dodge_chance(6))
            .count() as f32;
        let rate = hits / windows as f32;
        assert!((rate - dodge_chance(6)).abs() < 0.06, "rate {rate}");
        // One decision per window: every frame inside it agrees.
        let base = 90 * DODGE_ROLL_WINDOW;
        assert!((base..base + DODGE_ROLL_WINDOW).all(|f| dodge_roll(f) == dodge_roll(base)));
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
