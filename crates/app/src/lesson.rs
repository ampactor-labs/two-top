//! First Blood: the first-launch lesson, as a pure state machine.
//!
//! A newcomer used to be dropped into a duel with an unlabelled screen.
//! First Blood teaches one verb at a time, in the arena, by making the
//! player do it: one short line on screen, the bot doing just enough to
//! make the verb mean something, and the stage advancing on the real event
//! (read off the sim the way the haptics and audio already read it). The
//! screen carries no instructional prose beyond that one line.
//!
//! This module is the kernel — stages, what moves them on, what the bot
//! does in each and what the line says — with no Bevy in it, so every
//! transition is a unit test. `first_blood.rs` is the adapter that reads
//! the match into [`Events`] and puts [`line`] on screen.

/// The lesson's stages, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Stage {
    /// Walk around: the stick.
    #[default]
    Move,
    /// Let a fang go.
    Throw,
    /// Call it back and catch it.
    Recall,
    /// Charge, aim, kill the bot.
    Kill,
    /// The bot throws back: dash out of the way.
    Dodge,
    /// Catch it as it comes back fast: the empowered throw.
    Perfect,
    /// Where the floor ends.
    Edge,
    /// A real match against the bottom rung, to five.
    Finale,
    /// Over: the summary takes it from here.
    Done,
}

/// What the bot does while a stage runs. The bot is an input source, so
/// none of this touches the sim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BotMode {
    /// Stands still: a target.
    Stand,
    /// Wanders slowly, never throws: a moving target.
    Amble,
    /// The sparring partner (tier 0): slow, telegraphed lobs.
    Spar,
    /// A real duel at this tier (rubber band included).
    Duel(u32),
}

/// Which way the player is playing, for the wording of the line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Device {
    #[default]
    Touch,
    Keys,
}

/// What happened this frame, read off the match by the adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Events {
    /// Centimetres the player walked this frame.
    pub moved_cm: f32,
    /// The player's fang left their hand.
    pub threw: bool,
    /// The player's fang came back to their hand (caught or picked up).
    pub caught: bool,
    /// The player killed the bot.
    pub killed: bool,
    /// The player started a dash.
    pub dashed: bool,
    /// The player's throw became empowered (a perfect catch).
    pub empowered: bool,
    /// The match is over.
    pub match_over: bool,
    /// Seconds since the last frame.
    pub dt: f32,
}

/// The lesson's state: the stage, and what has built up inside it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Lesson {
    pub stage: Stage,
    /// Seconds spent in this stage.
    pub stage_secs: f32,
    /// Centimetres walked in this stage.
    pub walked_cm: f32,
}

/// Distance the Move stage asks for: a few steps, not a lap.
pub const MOVE_CM: f32 = 250.0;
/// Seconds the Edge stage's line stays up before the finale.
pub const EDGE_SECS: f32 = 4.0;
/// Seconds after which the Perfect stage lets the player on anyway: a
/// perfect catch is a timing skill, and a lesson must never be a wall.
pub const PERFECT_GIVE_UP_SECS: f32 = 30.0;

impl Lesson {
    /// The lesson after this frame's events.
    pub fn step(self, ev: &Events) -> Lesson {
        let mut now = Lesson {
            stage_secs: self.stage_secs + ev.dt,
            walked_cm: self.walked_cm + ev.moved_cm,
            ..self
        };
        let next = match self.stage {
            Stage::Move if now.walked_cm >= MOVE_CM => Stage::Throw,
            Stage::Throw if ev.threw => Stage::Recall,
            Stage::Recall if ev.caught => Stage::Kill,
            Stage::Kill if ev.killed => Stage::Dodge,
            Stage::Dodge if ev.dashed => Stage::Perfect,
            Stage::Perfect if ev.empowered || now.stage_secs >= PERFECT_GIVE_UP_SECS => Stage::Edge,
            Stage::Edge if now.stage_secs >= EDGE_SECS => Stage::Finale,
            Stage::Finale if ev.match_over => Stage::Done,
            stage => stage,
        };
        // A match that ends early (the bot won it while the player was
        // still learning) ends the lesson where it stands.
        let next = if ev.match_over { Stage::Done } else { next };
        if next != self.stage {
            now = Lesson {
                stage: next,
                ..Lesson::default()
            };
        }
        now
    }
}

/// What the bot does during `stage`.
pub fn bot_mode(stage: Stage) -> BotMode {
    match stage {
        Stage::Move | Stage::Throw | Stage::Recall | Stage::Perfect | Stage::Edge => BotMode::Stand,
        Stage::Kill => BotMode::Amble,
        Stage::Dodge => BotMode::Spar,
        Stage::Finale | Stage::Done => BotMode::Duel(1),
    }
}

/// The one line on screen during `stage`, for the way the player plays.
/// `None` once the lesson has handed over to the match.
pub fn line(stage: Stage, device: Device) -> Option<&'static str> {
    use Device::{Keys, Touch};
    Some(match (stage, device) {
        (Stage::Move, Touch) => "DRAG ON THE LEFT TO MOVE",
        (Stage::Move, Keys) => "WASD TO MOVE",
        (Stage::Throw, Touch) => "TAP THE RIGHT SIDE TO THROW",
        (Stage::Throw, Keys) => "SPACE TO THROW",
        (Stage::Recall, Touch) => "TAP AGAIN TO CALL IT BACK",
        (Stage::Recall, Keys) => "SPACE AGAIN TO CALL IT BACK",
        (Stage::Kill, Touch) => "HOLD THE RIGHT SIDE TO CHARGE\nAIM ON THE LEFT, LET GO",
        (Stage::Kill, Keys) => "HOLD SPACE TO CHARGE\nAIM WITH WASD, LET GO",
        (Stage::Dodge, Touch) => "IT THROWS BACK\nHOLD A DIRECTION, TAP DASH",
        (Stage::Dodge, Keys) => "IT THROWS BACK\nHOLD A DIRECTION, LEFT SHIFT",
        (Stage::Perfect, _) => "CALL IT BACK, CATCH IT FAST\nTHE NEXT THROW HITS HARDER",
        (Stage::Edge, _) => "THE FLOOR ENDS\nDON'T",
        (Stage::Finale, _) => "FIRST TO 5",
        (Stage::Done, _) => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(lesson: Lesson, ev: Events) -> Lesson {
        lesson.step(&Events {
            dt: 1.0 / 60.0,
            ..ev
        })
    }

    #[test]
    fn every_stage_advances_on_its_own_event_and_no_other() {
        let mut l = Lesson::default();
        // Walking a little is not enough; walking far enough is.
        l = run(
            l,
            Events {
                moved_cm: 100.0,
                ..default()
            },
        );
        assert_eq!(l.stage, Stage::Move);
        l = run(
            l,
            Events {
                moved_cm: 160.0,
                ..default()
            },
        );
        assert_eq!(l.stage, Stage::Throw);
        // A dash does not count as a throw.
        l = run(
            l,
            Events {
                dashed: true,
                ..default()
            },
        );
        assert_eq!(l.stage, Stage::Throw);
        l = run(
            l,
            Events {
                threw: true,
                ..default()
            },
        );
        assert_eq!(l.stage, Stage::Recall);
        l = run(
            l,
            Events {
                caught: true,
                ..default()
            },
        );
        assert_eq!(l.stage, Stage::Kill);
        l = run(
            l,
            Events {
                killed: true,
                ..default()
            },
        );
        assert_eq!(l.stage, Stage::Dodge);
        l = run(
            l,
            Events {
                dashed: true,
                ..default()
            },
        );
        assert_eq!(l.stage, Stage::Perfect);
        l = run(
            l,
            Events {
                empowered: true,
                ..default()
            },
        );
        assert_eq!(l.stage, Stage::Edge);
        for _ in 0..(EDGE_SECS * 60.0) as usize + 1 {
            l = run(l, Events::default());
        }
        assert_eq!(l.stage, Stage::Finale);
        l = run(
            l,
            Events {
                match_over: true,
                ..default()
            },
        );
        assert_eq!(l.stage, Stage::Done);
    }

    fn default() -> Events {
        Events::default()
    }

    #[test]
    fn a_stage_starts_from_nothing() {
        // Walking done during the Move stage does not carry into the next.
        let l = run(
            Lesson::default(),
            Events {
                moved_cm: 900.0,
                ..default()
            },
        );
        assert_eq!(l.stage, Stage::Throw);
        assert_eq!(l.walked_cm, 0.0);
        assert_eq!(l.stage_secs, 0.0);
    }

    #[test]
    fn the_perfect_catch_is_never_a_wall() {
        let mut l = Lesson {
            stage: Stage::Perfect,
            ..Lesson::default()
        };
        for _ in 0..(PERFECT_GIVE_UP_SECS * 60.0) as usize + 1 {
            l = run(l, Events::default());
        }
        assert_eq!(l.stage, Stage::Edge);
    }

    #[test]
    fn a_match_that_ends_early_ends_the_lesson() {
        let l = Lesson {
            stage: Stage::Kill,
            ..Lesson::default()
        };
        assert_eq!(
            run(
                l,
                Events {
                    match_over: true,
                    ..default()
                }
            )
            .stage,
            Stage::Done
        );
    }

    #[test]
    fn the_bot_only_fights_once_the_player_can() {
        for stage in [Stage::Move, Stage::Throw, Stage::Recall] {
            assert_eq!(bot_mode(stage), BotMode::Stand, "{stage:?}");
        }
        assert_eq!(bot_mode(Stage::Kill), BotMode::Amble);
        assert_eq!(bot_mode(Stage::Dodge), BotMode::Spar);
        assert_eq!(bot_mode(Stage::Finale), BotMode::Duel(1));
    }

    #[test]
    fn every_stage_but_the_end_has_a_line_for_each_device() {
        for stage in [
            Stage::Move,
            Stage::Throw,
            Stage::Recall,
            Stage::Kill,
            Stage::Dodge,
            Stage::Perfect,
            Stage::Edge,
            Stage::Finale,
        ] {
            for device in [Device::Touch, Device::Keys] {
                let text = line(stage, device).expect("a line");
                // Short enough to read at a glance, and to fit a phone's
                // width unwrapped: at most two lines of at most 30
                // characters.
                assert!(text.lines().count() <= 2, "{text}");
                assert!(text.lines().all(|l| l.len() <= 30), "{text}");
            }
        }
        assert_eq!(line(Stage::Done, Device::Touch), None);
    }
}
