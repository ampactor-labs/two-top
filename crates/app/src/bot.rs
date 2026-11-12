//! Practice: a solo opponent on any build, any platform.
//!
//! The single biggest gap between "netplay works" and "this is a game you
//! can hand to someone": being able to play *alone*. In practice mode the
//! local session runs as usual (same SyncTest couch path, same sim, same
//! determinism story) and this module supplies handle 1's inputs — the bot
//! is just another input source, so nothing about rollback, recording, or
//! the sim changes at all. Replays of practice matches record the bot's
//! inputs like anyone else's.
//!
//! The duelist itself — what it sees, how it decides, the gauntlet's knob
//! ladder — is the `bot_brain` crate, shared with the headless bench that
//! tunes it. This module is the glue: whether practice is on, which shade
//! (if any) it wears, the tier it plays at, and handing its answer to ggrs.
//!
//! Wiring: the input sources insert `LocalInputs` via `Commands`; the bot
//! system is ordered after them in the same `ReadInputs` chain, so its
//! patch applies to the freshly-inserted map before ggrs reads it.

use bevy::prelude::*;
use bevy_ggrs::LocalInputs;

use crate::lesson::BotMode;
pub use bot_brain::BotStyle;
use sim::{GgrsCfg, PlayerInput};

/// Practice mode: a local match against the bot (forces a local session
/// even on online builds; `start_matchbox` checks it).
#[derive(Resource, Default)]
pub struct PracticeMode(pub bool);

/// The handle the bot drives (the far/top duelist; the human is handle 0).
pub const BOT_HANDLE: usize = 1;

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

/// The practice bot's memory (its lagged picture of the player).
#[derive(Resource, Default)]
pub struct BotBrain(bot_brain::Brain);

/// `ReadInputs`, after the player's own sources: decide the bot's input for
/// this tick and patch it into handle 1.
///
/// The difficulty is the persisted GAUNTLET tier, nudged by the in-match
/// rubber band (`bot_brain::rubber_band`: a notch up while the player leads
/// by two, a notch down while the bot does). A fresh install starts at the
/// sparring partner.
pub fn drive_bot(world: &mut World) {
    if !world.resource::<PracticeMode>().0 {
        return;
    }
    let lesson = world
        .get_resource::<crate::first_blood::FirstBlood>()
        .and_then(|fb| fb.bot_mode());
    let tier = match lesson {
        // First Blood decides how hard the bot plays while it teaches.
        Some(BotMode::Duel(tier)) => tier,
        Some(_) => 0,
        None => {
            world
                .resource::<crate::grudge::CareerRecord>()
                .gauntlet_tier
        }
    };
    let score = *world.resource::<sim::MatchScore>();
    let difficulty = bot_brain::rubber_band(tier, score.p0, score.p1);
    let style = world.resource::<ShadeStyle>().0.as_ref().map(|s| s.style);
    let input = match bot_brain::view_for(world, BOT_HANDLE, difficulty, style, 0) {
        Some(view) => match lesson {
            // A target, then a moving target, while the lesson teaches the
            // verbs; the sparring partner's lobs for the dodge; a real
            // duel to finish.
            Some(BotMode::Stand) => PlayerInput::default(),
            Some(BotMode::Amble) => bot_brain::amble(&view),
            Some(BotMode::Spar) => {
                let view = bot_brain::BotView {
                    difficulty: 0,
                    ..view
                };
                world.resource_mut::<BotBrain>().0.decide(view)
            }
            Some(BotMode::Duel(_)) | None => world.resource_mut::<BotBrain>().0.decide(view),
        },
        None => {
            // Out of the round or dead: whatever it saw is stale by the
            // time it is back.
            world.resource_mut::<BotBrain>().0.forget();
            PlayerInput::default()
        }
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
        app.init_resource::<BotBrain>();
        app.add_systems(
            bevy::state::state::OnEnter(crate::screen::AppScreen::Title),
            disarm_shade,
        );
        // The exclusive system is registered per-platform in lib.rs so it
        // orders after that platform's input source.
    }
}
