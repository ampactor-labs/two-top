//! The game's one voice on the floor: a short line on a dark plate, always
//! in the same spot. First Blood's lesson and the in-match nudges both
//! speak through it, so where it sits, how big it is and what backs it are
//! decided here, once.
//!
//! Each speaker owns a slot and writes it every frame (`None` when it has
//! nothing to say); the lesson outranks a nudge.

use bevy::prelude::*;

use crate::anchor::ScreenAnchor;

/// One thing to say: the text, already broken into lines (it never wraps),
/// and its colour this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    pub text: &'static str,
    pub color: Color,
}

/// What each speaker wants on screen this frame.
#[derive(Resource, Default, Debug)]
pub struct Callout {
    /// First Blood's lesson line.
    pub lesson: Option<Line>,
    /// An in-match nudge.
    pub nudge: Option<Line>,
}

impl Callout {
    /// The line that shows: the lesson's, else the nudge's.
    pub fn showing(&self) -> Option<Line> {
        self.lesson.or(self.nudge)
    }
}

/// Where the line sits (window fraction, y-down): between the intro card's
/// stakes (0.325) and the countdown glyph at the centre, over the far half
/// of the floor, clear of both thumbs.
pub const LINE_FY: f32 = 0.395;
/// Its size.
const FONT: f32 = 40.0;
/// The plate's height per line of text, and its padding (world units).
const PLATE_LINE_H: f32 = 52.0;
const PLATE_PAD: f32 = 28.0;

#[derive(Component)]
struct CalloutText;

#[derive(Component)]
struct CalloutPlate;

fn spawn_callout(mut commands: Commands) {
    commands.spawn((
        CalloutPlate,
        Sprite {
            color: render::palette::DEEP_ASH.with_alpha(0.72),
            custom_size: Some(Vec2::new(4000.0, PLATE_LINE_H + PLATE_PAD)),
            ..default()
        },
        ScreenAnchor::new(0.0, crate::ui::anchor_y(LINE_FY), 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, 205.5),
        Visibility::Hidden,
    ));
    commands.spawn((
        CalloutText,
        Text2d::new(String::new()),
        TextFont {
            font_size: FONT,
            ..default()
        },
        TextColor(render::palette::HOT_BONE),
        // Lines break where the speaker breaks them, never mid-phrase.
        TextLayout {
            justify: Justify::Center,
            linebreak: bevy::text::LineBreak::NoWrap,
        },
        ScreenAnchor::new(0.0, crate::ui::anchor_y(LINE_FY), 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, 206.0),
        Visibility::Hidden,
    ));
}

#[allow(clippy::type_complexity)]
fn draw_callout(
    callout: Res<Callout>,
    mut text: Query<(&mut Text2d, &mut TextColor, &mut Visibility), With<CalloutText>>,
    mut plate: Query<(&mut Sprite, &mut Visibility), (With<CalloutPlate>, Without<CalloutText>)>,
) {
    let (Ok((mut text, mut color, mut vis)), Ok((mut plate, mut plate_vis))) =
        (text.single_mut(), plate.single_mut())
    else {
        return;
    };
    let Some(line) = callout.showing() else {
        *vis = Visibility::Hidden;
        *plate_vis = Visibility::Hidden;
        return;
    };
    *vis = Visibility::Visible;
    *plate_vis = Visibility::Visible;
    color.0 = line.color;
    if text.0 != line.text {
        text.0 = line.text.to_string();
        let lines = line.text.lines().count() as f32;
        plate.custom_size = Some(Vec2::new(4000.0, PLATE_LINE_H * lines + PLATE_PAD));
    }
}

/// The set the speakers write in before the line is drawn.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct CalloutSpeak;

pub struct CalloutPlugin;

impl Plugin for CalloutPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Callout>()
            .add_systems(Startup, spawn_callout)
            .add_systems(Update, draw_callout.after(CalloutSpeak));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lesson_outranks_a_nudge() {
        let lesson = Line {
            text: "WASD TO MOVE",
            color: Color::WHITE,
        };
        let nudge = Line {
            text: "GET BACK ON THE FLOOR",
            color: Color::BLACK,
        };
        let both = Callout {
            lesson: Some(lesson),
            nudge: Some(nudge),
        };
        assert_eq!(both.showing(), Some(lesson));
        let just_nudge = Callout {
            lesson: None,
            nudge: Some(nudge),
        };
        assert_eq!(just_nudge.showing(), Some(nudge));
        assert_eq!(Callout::default().showing(), None);
    }

    #[test]
    fn the_line_sits_clear_of_the_intro_card_and_the_exit_strip() {
        const { assert!(LINE_FY > 0.325 + 0.04) };
        assert!(!crate::ui::in_band(LINE_FY, crate::ui::TOP_EXIT));
    }
}
