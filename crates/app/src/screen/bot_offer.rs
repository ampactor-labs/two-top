//! The bot fallback: PLAY THE BOT while a summons hangs, or after an
//! opponent has gone.

use super::*;

// ---- The bot fallback ----
// "Play against a bot" must stay one tap away in every state, including
// the two online states that used to strand a player without it: the
// waiting room (the summons can hang forever if nobody else is online) and
// the fled-opponent summary (whose primary slot was dead). Both convert to
// a practice match on the spot.

/// The waiting room's bot offer. Shown only while [`AwaitingPeer`] is up,
/// in the PLAY slot — the thumb that just tapped FIND OPPONENT is already
/// there, and the touch overlays hide while awaiting so the band is free.
#[derive(Component)]
pub(super) struct BotOfferButton {
    role: BtnRole,
}

pub(super) fn spawn_bot_offer_button(commands: &mut Commands) {
    for role in [BtnRole::Border, BtnRole::Fill, BtnRole::Label] {
        spawn_button_part(
            commands,
            role,
            Vec2::new(0.0, PLAY_ANCHOR_Y),
            Vec2::new(760.0, 150.0),
            44.0,
            &mut |ec, role| {
                ec.insert(BotOfferButton { role });
            },
        );
    }
}

/// Show the waiting room's PLAY THE BOT button while the summons hangs.
/// The fled-opponent summary offers the same escape through the summary
/// primary (`primary_label`), so this one keys off [`AwaitingPeer`] alone.
pub(super) fn update_bot_offer_button(
    offer: Res<BotOffer>,
    mut q: Query<(&BotOfferButton, &mut Visibility, Option<&mut Text2d>)>,
) {
    for (btn, mut vis, text) in &mut q {
        *vis = if offer.armed {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        if offer.armed
            && btn.role == BtnRole::Label
            && let Some(mut t) = text
        {
            t.0 = "PLAY THE BOT".to_string();
        }
    }
}

/// A tap in the PLAY slot (or P) while the bot is on offer: arm practice,
/// fold the online attempt, and bounce through Title — [`PendingBotMatch`]
/// re-enters InMatch on arrival, so the full teardown/spawn path runs
/// (sessionless clock reset included) instead of a hand-rolled in-place
/// session swap. Exclusive-world because the teardown touches the
/// non-send socket. No result is recorded: the wait staked nothing, and a
/// fled opponent's forfeit was filed when they fled.
pub(super) fn bot_fallback_input(world: &mut World) {
    if world.resource::<crate::theater::TheaterMode>().active() {
        return;
    }
    let online = world.resource::<NetplayConfig>().room_url.is_some()
        && !world.resource::<crate::bot::PracticeMode>().0;
    if !online {
        return;
    }
    let awaiting = world.resource::<BotOffer>().armed;
    let fled_summary = matches!(*world.resource::<MatchState>(), MatchState::MatchOver)
        && world.resource::<net::LobbyState>().is_terminal();
    if !(awaiting || fled_summary) {
        return;
    }
    let key = world
        .resource::<ButtonInput<KeyCode>>()
        .just_pressed(KeyCode::KeyP);
    let win = world.resource::<WindowSize>().0;
    let tapped = ui::tapped(
        world
            .resource::<Touches>()
            .iter_just_pressed()
            .map(|t| t.position()),
        PLAY_BTN_RECT,
        win,
    );
    if !(key || tapped) {
        return;
    }
    world.resource_mut::<crate::bot::PracticeMode>().0 = true;
    crate::netplay::leave_online_match(world);
    world.resource_mut::<PendingBotMatch>().0 = true;
    world
        .resource_mut::<NextState<AppScreen>>()
        .set(AppScreen::Title);
}
