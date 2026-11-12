//! The in-match QUIT button: arm, confirm, and what leaving costs.

use super::*;

// ---- The in-match QUIT button ----
// Live play had no exit at all: the top strip is the TAUNT zone, Escape and
// the top-exit band only work at MatchOver, and the online SUMMONING wait
// could hold a player hostage forever. A small corner rect (the quit zone,
// carved out of the taunt strip in `input_touch` so a tap there never
// flexes) carries a bordered QUIT chip: first tap arms it (SURE?), second
// tap within the window quits. Quitting a live online duel records an
// honest loss first — same blame the away-grace forfeit assigns.

/// Armed-confirm state: `Some(elapsed_secs)` while the first tap waits for
/// its confirmation.
#[derive(Resource, Default)]
pub struct QuitArm(pub Option<f32>);

/// Seconds the armed state waits for the confirming tap.
pub(super) const QUIT_CONFIRM_SECS: f32 = 2.5;

/// Screen-anchor X of the quit chip: the center of the quit-zone rect
/// ([QUIT_ZONE_X_FRAC, 1.0] in window fractions, mapped to [-1, 1]).
pub(super) const QUIT_BTN_ANCHOR_X: f32 = input_touch::QUIT_ZONE_X_FRAC;

#[derive(Component)]
pub(super) struct QuitButton {
    role: BtnRole,
}

pub(super) fn spawn_quit_button(commands: &mut Commands) {
    for role in [BtnRole::Border, BtnRole::Fill, BtnRole::Label] {
        spawn_button_part(
            commands,
            role,
            Vec2::new(QUIT_BTN_ANCHOR_X, TOP_EXIT_ANCHOR_Y),
            Vec2::new(190.0, 62.0),
            24.0,
            &mut |ec, role| {
                ec.insert(QuitButton { role });
            },
        );
    }
}

/// The quit label states, pure for tests: the summoning wait needs no
/// confirmation (nothing at stake yet), nor does the lesson (SKIP), and a
/// live match asks twice.
pub fn quit_label(awaiting: bool, lesson: bool, armed: bool) -> &'static str {
    if lesson {
        "SKIP"
    } else if awaiting {
        "CANCEL"
    } else if armed {
        "SURE?"
    } else {
        "QUIT"
    }
}

/// Taps on the quit corner (and Escape on desktop) — the arm/confirm state
/// machine plus the actual teardown. Exclusive-world because the online
/// leave touches the non-send socket.
pub(super) fn quit_button_input(world: &mut World) {
    if world.resource::<crate::theater::TheaterMode>().active() {
        return; // the theater's marquee owns the exit gesture
    }
    if matches!(*world.resource::<MatchState>(), MatchState::MatchOver) {
        // The summary's LEAVE/LOBBY buttons own the endgame; a stale arm
        // must not leak into the next match.
        world.resource_mut::<QuitArm>().0 = None;
        return;
    }
    let now = world.resource::<Time<Real>>().elapsed_secs();
    {
        let mut arm = world.resource_mut::<QuitArm>();
        if let Some(at) = arm.0
            && now - at > QUIT_CONFIRM_SECS
        {
            arm.0 = None;
        }
    }

    let esc = world
        .resource::<ButtonInput<KeyCode>>()
        .just_pressed(KeyCode::Escape);
    let win = world.resource::<WindowSize>().0;
    let southpaw = world.resource::<input_touch::Southpaw>().0;
    let tapped = win.y > 0.0
        && world.resource::<Touches>().iter_just_pressed().any(|t| {
            let p = if southpaw {
                input_touch::mirror_x(t.position(), win)
            } else {
                t.position()
            };
            input_touch::is_quit_zone(p, win)
        });
    if !(esc || tapped) {
        return;
    }

    // Waiting alone in an online room stakes nothing — one tap cancels.
    // The lesson stakes nothing either: one tap skips it, for good.
    let awaiting = world.resource::<AwaitingPeer>().0;
    let lesson = world.resource::<crate::first_blood::FirstBlood>().active;
    if !awaiting && !lesson && world.resource::<QuitArm>().0.is_none() {
        world.resource_mut::<QuitArm>().0 = Some(now);
        return;
    }
    world.resource_mut::<QuitArm>().0 = None;
    if lesson {
        crate::first_blood::skip(world);
        world
            .resource_mut::<NextState<AppScreen>>()
            .set(AppScreen::Title);
        return;
    }

    let online = world.resource::<NetplayConfig>().room_url.is_some()
        && !world.resource::<crate::bot::PracticeMode>().0;
    if online {
        // Abandoning a live duel is a loss, filed before the goodbye. A
        // wait (no session yet, or one still synchronizing) or a peer who
        // already fled stakes no result.
        let established = crate::netplay::duel_underway(world);
        let peer_fled = world.resource::<net::LobbyState>().is_terminal();
        if established && !peer_fled {
            let peer = world.resource::<net::PeerProfile>().0;
            let mut record = world.resource_mut::<crate::grudge::CareerRecord>();
            crate::grudge::record_abandoned_loss(&mut record, peer);
        }
        crate::netplay::leave_online_match(world);
    } else if world.resource::<crate::bot::PracticeMode>().0
        && world.resource::<crate::bot::ShadeStyle>().0.is_none()
        && matches!(
            *world.resource::<MatchState>(),
            MatchState::InRound { .. } | MatchState::RoundOver { .. }
        )
    {
        // Quitting a live gauntlet match is a loss — the same rule as the
        // duel above. The countdown stakes nothing; a shade spar moves
        // nothing.
        let mut record = world.resource_mut::<crate::grudge::CareerRecord>();
        crate::grudge::record_gauntlet_quit(&mut record);
    }
    world
        .resource_mut::<NextState<AppScreen>>()
        .set(AppScreen::Title);
}

pub(super) type QuitButtonQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static QuitButton,
        &'static mut Visibility,
        &'static mut ScreenAnchor,
        Option<&'static mut Sprite>,
        Option<&'static mut Text2d>,
        Option<&'static mut TextColor>,
    ),
>;

#[allow(clippy::too_many_arguments)]
pub(super) fn update_quit_button(
    screen: Res<State<AppScreen>>,
    state: Res<MatchState>,
    theater: Res<crate::theater::TheaterMode>,
    awaiting: Res<AwaitingPeer>,
    arm: Res<QuitArm>,
    southpaw: Res<input_touch::Southpaw>,
    first_blood: Res<crate::first_blood::FirstBlood>,
    mut q: QuitButtonQuery,
) {
    let shown = *screen.get() == AppScreen::InMatch
        && !matches!(*state, MatchState::MatchOver)
        && !theater.active();
    let armed = arm.0.is_some();
    // Southpaw mirrors the quit corner to the top-left with the other zones.
    let anchor_x = if southpaw.0 {
        -QUIT_BTN_ANCHOR_X
    } else {
        QUIT_BTN_ANCHOR_X
    };
    for (btn, mut vis, mut anchor, sprite, text, color) in &mut q {
        *vis = if shown {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        if !shown {
            continue;
        }
        anchor.frac.x = anchor_x;
        match btn.role {
            BtnRole::Border => {
                if let Some(mut s) = sprite {
                    s.color = render::palette::HOT_BONE.with_alpha(if armed { 1.0 } else { 0.55 });
                }
            }
            BtnRole::Fill => {
                if let Some(mut s) = sprite {
                    s.color = if armed {
                        render::palette::HOT_BONE
                    } else {
                        render::palette::DEEP_ASH.with_alpha(0.75)
                    };
                }
            }
            BtnRole::Label => {
                if let Some(mut t) = text {
                    t.0 = quit_label(awaiting.0, first_blood.active, armed).to_string();
                }
                if let Some(mut c) = color {
                    c.0 = if armed {
                        render::palette::VOID
                    } else {
                        render::palette::HOT_BONE.with_alpha(0.75)
                    };
                }
            }
        }
    }
}
