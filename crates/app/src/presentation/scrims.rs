//! Full-screen washes: the ritual wash and the menu scrim.

use crate::*;

/// Marker for the match-point ritual's full-screen darkness wash.
#[derive(Component)]
pub(crate) struct RitualWash;

/// Marker for the menu scrim (darkens the arena behind title / waiting text).
#[derive(Component)]
pub(crate) struct MenuScrim;

/// Show the menu scrim on every menu screen and while awaiting a peer;
/// hide it the instant a real match is being played so gameplay is never
/// dimmed. The TITLE gets a lighter one: that screen is a diorama of the
/// table, and you have to be able to see the table. The text screens
/// (replays, settings, name entry) keep the full-contrast wash.
pub(crate) fn update_menu_scrim(
    screen: Res<State<AppScreen>>,
    awaiting: Res<screen::AwaitingPeer>,
    mut q: Query<(&mut Visibility, &mut Sprite), With<MenuScrim>>,
) {
    let show = screen.get().is_menu() || awaiting.0;
    let alpha = if *screen.get() == AppScreen::Title {
        0.55
    } else {
        0.85
    };
    for (mut vis, mut sprite) in &mut q {
        *vis = if show {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        sprite.color = render::palette::VOID.with_alpha(alpha);
    }
}

/// Ease the ritual wash in/out with `render::MatchPointRitual`. The eased
/// alpha (not a hard cut) makes the room feel like it *dims* — lights going
/// down for the last rite — rather than a state toggle.
pub(crate) fn animate_ritual_wash(
    time: Res<Time<Real>>,
    ritual: Res<render::MatchPointRitual>,
    mut q: Query<(&mut Sprite, &mut Visibility), With<RitualWash>>,
) {
    const WASH_MAX_ALPHA: f32 = 0.38;
    let dt = time.delta_secs();
    for (mut sprite, mut vis) in &mut q {
        let current = sprite.color.alpha();
        let target = if ritual.0 { WASH_MAX_ALPHA } else { 0.0 };
        let next = camera::damped_step(current, target, 3.0, dt);
        sprite.color = render::palette::VOID.with_alpha(next);
        // A fully-faded wash still rasterizes as a full-screen transparent
        // quad — real fill cost on tile GPUs. Cull it outside the ritual.
        *vis = if next < 0.01 {
            Visibility::Hidden
        } else {
            Visibility::Visible
        };
    }
}
