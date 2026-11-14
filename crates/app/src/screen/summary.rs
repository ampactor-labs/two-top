//! The match-over summary: the card's facts, its action buttons, and the
//! rematch and leave inputs they drive.

use super::*;

/// Compose the MatchOver summary CARD — the facts only (winner, score,
/// rivalry, the fled note, the saved-tape note). The ACTIONS live on real
/// buttons now (`update_summary_buttons`): the card never again instructs
/// a phone player to "press ESC" or tap an invisible zone. Pure for tests.
#[allow(clippy::too_many_arguments)]
pub(super) fn summary_text(
    score: MatchScore,
    online: bool,
    practice: bool,
    theater_names: Option<[Option<String>; 2]>,
    local_handle: Option<usize>,
    local_name: &str,
    peer: Option<net::ProfileData>,
    rivalry: Option<String>,
    opponent_gone: bool,
    we_fled: bool,
    desynced: bool,
    unfinished: bool,
    saved: bool,
) -> String {
    let p0_won = score.p0 >= MATCH_WIN_THRESHOLD;

    // Theater: the tape ran out — say who won it; the marquee up top is
    // already the way out.
    if let Some(names) = theater_names {
        let idx = if p0_won { 0 } else { 1 };
        let fallback = if p0_won { "CUR" } else { "STAG" };
        let who = names[idx].clone().unwrap_or_else(|| fallback.to_string());
        return format!("{who} WINS\n\n{}  -  {}\n\nTAPE ENDS", score.p0, score.p1);
    }

    if !online || practice {
        let winner = winner_label(score);
        let replay_line = if saved { "\n\nreplay saved" } else { "" };
        return format!("{winner}\n\n{}  -  {}{replay_line}", score.p0, score.p1);
    }

    // Online: name the winner by who they ARE when we know it. An earned
    // score is the verdict; a forfeit goes to whoever stayed at the table
    // (same rule the grudge ledger scores by) — the old score-only check
    // crowned the FLED player whenever nobody had reached the threshold.
    // A desync: this phone's score is not a fact the other phone shares.
    // Say so, crown nobody, and promise no tape and no ledger line.
    if desynced {
        return format!(
            "THE TWO PHONES STOPPED AGREEING\n\n{}  -  {}\n\nthis one doesn't count",
            score.p0, score.p1
        );
    }
    let me = local_handle.unwrap_or(0);
    let peer_name = crate::profile::peer_name(peer);
    let threshold_hit = score.p0 >= MATCH_WIN_THRESHOLD || score.p1 >= MATCH_WIN_THRESHOLD;
    let i_won = if threshold_hit {
        if me == 0 { p0_won } else { !p0_won }
    } else {
        opponent_gone && !we_fled
    };
    let winner = if i_won {
        format!("{local_name} WINS")
    } else {
        format!("{peer_name} WINS")
    };
    let rivalry_line = rivalry.map(|r| format!("\n{r}")).unwrap_or_default();
    // Nobody reached the threshold, nobody conceded, and we did not freeze:
    // a dropped link is not a result. Crown nobody.
    //
    // `!threshold_hit` is load-bearing and matches `grudge::match_outcome`,
    // which tests the score BEFORE the forfeit: a peer whose link dies in
    // the same window as the deciding kill still lost 5-2, and this card
    // read CONNECTION LOST over a win the ledger had already banked.
    if unfinished && !threshold_hit {
        return format!(
            "CONNECTION LOST\n\n{}  -  {}{rivalry_line}\n\nunfinished - nobody's win",
            score.p0, score.p1
        );
    }
    let gone_line = if opponent_gone {
        if we_fled {
            "\n\nmatch abandoned - you left the duel"
        } else {
            "\n\nthe field is yours"
        }
    } else {
        ""
    };
    let replay_line = if saved { "\nreplay saved" } else { "" };
    format!(
        "{winner}\n\n{}  -  {}{rivalry_line}{gone_line}{replay_line}",
        score.p0, score.p1
    )
}

/// The practice summary card: who won, said to the player ("YOU WIN", not
/// the Cur's name — the human is always the Cur in practice and nobody
/// reads themselves as an animal), the score, where the gauntlet went,
/// and the saved-tape note. A shade is named as one. Pure for tests.
pub(super) fn practice_summary(
    score: MatchScore,
    shade_name: Option<&str>,
    step: Option<(u32, u32)>,
    saved: bool,
) -> String {
    let won = score.p0 >= MATCH_WIN_THRESHOLD;
    let headline = match (won, shade_name) {
        (true, _) => "YOU WIN".to_string(),
        (false, Some(name)) => format!("{name} SHADE WINS"),
        (false, None) => "THE BOT WINS".to_string(),
    };
    let tier = step
        .and_then(|(from, to)| crate::grudge::gauntlet_line(from, to, won))
        .map(|line| format!("\n\n{line}"))
        .unwrap_or_default();
    let replay_line = if saved { "\n\nreplay saved" } else { "" };
    format!(
        "{headline}\n\n{}  -  {}{tier}{replay_line}",
        score.p0, score.p1
    )
}

/// The card at the end of First Blood: the lesson is over whoever took
/// the match, so it says so — and what the ladder made of it. Pure.
pub(super) fn first_blood_summary(
    score: MatchScore,
    step: Option<(u32, u32)>,
    saved: bool,
) -> String {
    let won = score.p0 >= MATCH_WIN_THRESHOLD;
    let verdict = if won {
        "FIRST BLOOD"
    } else {
        "THE BOT TOOK THIS ONE"
    };
    let tier = step
        .and_then(|(from, to)| crate::grudge::gauntlet_line(from, to, won))
        .map(|line| format!("\n{line}"))
        .unwrap_or_default();
    let replay_line = if saved { "\n\nreplay saved" } else { "" };
    format!(
        "YOU'RE READY\n\n{}  -  {}\n{verdict}{tier}{replay_line}",
        score.p0, score.p1
    )
}

/// The primary summary button's label — the RUN IT BACK handshake as a
/// state machine the thumb can read. A gone opponent flips the slot to the
/// bot offer (`bot_fallback_input` owns that tap); `None` is kept for any
/// state that must hide the button outright. Pure for tests.
pub fn primary_label(
    online: bool,
    practice: bool,
    consent: net::RematchConsent,
    peer_name: &str,
    opponent_gone: bool,
) -> Option<String> {
    if !online || practice {
        return Some("PLAY AGAIN".to_string());
    }
    if opponent_gone {
        // Nobody left to run it back with — offer the opponent who never
        // leaves instead. `bot_fallback_input` owns the tap.
        return Some("PLAY THE BOT".to_string());
    }
    Some(match (consent.local, consent.peer) {
        (false, false) => "RUN IT BACK".to_string(),
        (true, false) => format!("WAITING ON {peer_name}..."),
        (false, true) => format!("{peer_name} IS IN - RUN IT BACK"),
        (true, true) => "RUNNING IT BACK...".to_string(),
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn update_summary_overlay(
    screen: Res<State<AppScreen>>,
    state: Res<MatchState>,
    score: Res<MatchScore>,
    netplay: Res<NetplayConfig>,
    practice: Res<crate::bot::PracticeMode>,
    theater: Res<crate::theater::TheaterMode>,
    local: Res<crate::netplay::LocalPlayerHandle>,
    profile: Res<crate::profile::LocalProfile>,
    peer: Res<net::PeerProfile>,
    career: Res<crate::grudge::CareerRecord>,
    lobby: Res<net::LobbyState>,
    saved: Res<crate::recorder::LastSavedReplay>,
    absence: Res<crate::netplay::RecentAbsence>,
    time: Res<Time<Real>>,
    (shade, step, first_blood): (
        Res<crate::bot::ShadeStyle>,
        Res<crate::grudge::GauntletStep>,
        Res<crate::first_blood::FirstBlood>,
    ),
    mut q: Query<(&mut Text2d, &mut Visibility), With<SummaryOverlay>>,
) {
    let Ok((mut text, mut vis)) = q.single_mut() else {
        return;
    };
    // Belt and braces with the reset in `despawn_match`: the card is a
    // MATCH screen element, so it checks the screen too. It used to key on
    // MatchState alone and burned itself onto the title and the replay list.
    if *screen.get() == AppScreen::InMatch && matches!(*state, MatchState::MatchOver) {
        *vis = Visibility::Visible;
        let opponent_gone = matches!(*lobby, net::LobbyState::Forfeited { .. });
        let desynced = matches!(*lobby, net::LobbyState::Desynced { .. });
        // If OUR phone went away just before the forfeit, the walk-out is
        // ours — the card owns the blame line (the summoning overlay that
        // used to carry it stands down at MatchOver).
        let we_fled = opponent_gone
            && absence.within(
                time.elapsed_secs(),
                crate::netplay::RecentAbsence::FORFEIT_BLAME_SECS,
            );
        // A silent drop nobody conceded and we did not cause: unfinished,
        // the same verdict the ledger reaches (`grudge::match_outcome`).
        let unfinished = matches!(
            *lobby,
            net::LobbyState::Forfeited {
                conceded: false,
                ..
            }
        ) && !we_fled;
        if first_blood.finished() {
            text.0 = first_blood_summary(*score, step.0, saved.0.is_some());
            return;
        }
        if practice.0 && !theater.active() {
            text.0 = practice_summary(
                *score,
                shade.0.as_ref().map(|s| s.name.as_str()),
                step.0,
                saved.0.is_some(),
            );
            return;
        }
        text.0 = summary_text(
            *score,
            netplay.room_url.is_some(),
            practice.0,
            theater.active().then(|| theater.header_names()),
            local.0,
            &profile.name_string(),
            peer.0,
            career.rivalry_line(peer.0),
            opponent_gone,
            we_fled,
            desynced,
            unfinished,
            saved.0.is_some(),
        );
    } else {
        *vis = Visibility::Hidden;
    }
}

// ---- Summary action buttons ----
// The endgame's actions are BUTTONS, not instructions: the touch controls
// hide at MatchOver, so "tap THROW" pointed at an invisible zone, and
// "press ESC" pointed at a key the phone doesn't have. The primary button
// carries the whole RUN IT BACK handshake state; the top strip is the one
// universal BACK gesture, labeled.

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SummaryAction {
    /// PLAY AGAIN (couch/practice) / the RUN IT BACK handshake (online).
    Primary,
    /// The labeled top-exit strip: LOBBY (couch/practice) / LEAVE (online).
    Exit,
}

#[derive(Component)]
pub(super) struct SummaryButton {
    action: SummaryAction,
    role: BtnRole,
}

/// A tap on PLAY AGAIN must become a real THROW input so the in-sim
/// `apply_rematch` restarts the match the rollback-correct way. The tap
/// arms this counter; `inject_ui_throw` (ReadInputs) ORs THROW_DOWN into
/// the local player's input while it runs down — a held press with a clean
/// rising edge, exactly as if the thumb had pressed the throw zone.
#[derive(Resource, Default)]
pub struct PendingUiThrow(pub u8);

/// Frames the injected press holds. A few ticks survive the forgiveness
/// window and any same-frame ordering; release follows for a clean edge
/// next time.
pub(super) const UI_THROW_FRAMES: u8 = 4;

/// Seconds after a match is decided before anything may restart it: the
/// devouring's hold and flight (`devour.rs`, 0.7 + 0.9).
///
/// `MatchOver` begins on the very tick of the deciding kill, and the sim
/// restarts on the next THROW press. The press right after a kill is the
/// one every player makes — calling the fang home — so the match-winning
/// kill ran straight into a fresh 0-0: no summary, no devouring, no
/// gauntlet tier, often no tape. Online the same press latched RUN IT BACK
/// consent before either player had seen the result.
pub const REMATCH_LOCKOUT_SECS: f32 = 1.6;

/// When (`Time<Real>` seconds) the current `MatchOver` began; `None`
/// outside one. Kept by [`track_match_over`].
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct MatchOverSince(pub Option<f32>);

impl MatchOverSince {
    /// Has the decided match been on screen for the whole lockout? `false`
    /// outside a `MatchOver`, and on its first frame (before the clock has
    /// been stamped).
    pub fn settled(&self, now: f32) -> bool {
        self.0
            .is_some_and(|since| now - since >= REMATCH_LOCKOUT_SECS)
    }
}

/// Stamp the moment a match is decided; clear it when the match leaves
/// `MatchOver` (a rematch, or the screen closing).
pub(super) fn track_match_over(
    time: Res<Time<Real>>,
    state: Res<MatchState>,
    mut since: ResMut<MatchOverSince>,
) {
    if matches!(*state, MatchState::MatchOver) {
        since.0.get_or_insert(time.elapsed_secs());
    } else {
        since.0 = None;
    }
}

/// May a THROW now restart the decided match? Only once the lockout has
/// run AND the thumb has come up at least once since: a press held through
/// the lockout would otherwise surface as a fresh edge the moment it ends.
/// Once armed, it stays armed until the match leaves `MatchOver`. Pure.
pub fn rematch_armed(was_armed: bool, settled: bool, throw_held: bool) -> bool {
    was_armed || (settled && !throw_held)
}

/// `ReadInputs`, after every input source and before the UI injector and
/// the online rematch gate: during a decided match's lockout, local THROW
/// presses do not reach the wire. Pre-wire shaping of what this device
/// sends, the legal kind (the rematch gate does the same); the sim, the
/// peer and the tape only ever see the presses that remain.
pub fn hold_rematch_until_settled(
    time: Res<Time<Real>>,
    state: Res<MatchState>,
    since: Res<MatchOverSince>,
    inputs: Option<ResMut<bevy_ggrs::LocalInputs<sim::GgrsCfg>>>,
    mut armed: Local<bool>,
) {
    if !matches!(*state, MatchState::MatchOver) {
        *armed = false;
        return;
    }
    let Some(mut inputs) = inputs else {
        return;
    };
    let throw_held = inputs
        .0
        .values()
        .any(|input| input.buttons & sim::PlayerInput::THROW_DOWN != 0);
    *armed = rematch_armed(*armed, since.settled(time.elapsed_secs()), throw_held);
    if !*armed {
        for input in inputs.0.values_mut() {
            input.buttons &= !sim::PlayerInput::THROW_DOWN;
        }
    }
}

pub fn inject_ui_throw(
    mut pending: ResMut<PendingUiThrow>,
    local_players: Res<bevy_ggrs::LocalPlayers>,
    inputs: Option<ResMut<bevy_ggrs::LocalInputs<sim::GgrsCfg>>>,
) {
    if pending.0 == 0 {
        return;
    }
    let Some(mut inputs) = inputs else {
        return;
    };
    pending.0 -= 1;
    // Couch has two local handles; P0's press restarts for everyone
    // (apply_rematch fires on either player's edge).
    if let Some(&handle) = local_players.0.first()
        && let Some(input) = inputs.0.get_mut(&handle)
    {
        input.buttons |= sim::PlayerInput::THROW_DOWN;
    }
}

pub(super) fn spawn_summary_buttons(commands: &mut Commands) {
    // Primary sits in the PLAY slot — the same thumb position that started
    // the match offers to restart it.
    for role in [BtnRole::Border, BtnRole::Fill, BtnRole::Label] {
        spawn_button_part(
            commands,
            role,
            Vec2::new(0.0, PLAY_ANCHOR_Y),
            Vec2::new(760.0, 150.0),
            44.0,
            &mut |ec, role| {
                ec.insert(SummaryButton {
                    action: SummaryAction::Primary,
                    role,
                });
            },
        );
    }
    for role in [BtnRole::Border, BtnRole::Fill, BtnRole::Label] {
        spawn_button_part(
            commands,
            role,
            Vec2::new(0.0, TOP_EXIT_ANCHOR_Y),
            Vec2::new(340.0, 76.0),
            30.0,
            &mut |ec, role| {
                ec.insert(SummaryButton {
                    action: SummaryAction::Exit,
                    role,
                });
            },
        );
    }
}

pub(super) type SummaryButtonQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static SummaryButton,
        &'static mut Visibility,
        Option<&'static mut Sprite>,
        Option<&'static mut Text2d>,
        Option<&'static mut TextColor>,
    ),
>;

#[allow(clippy::too_many_arguments)]
pub(super) fn update_summary_buttons(
    screen: Res<State<AppScreen>>,
    state: Res<MatchState>,
    time: Res<Time<Real>>,
    since: Res<MatchOverSince>,
    netplay: Res<NetplayConfig>,
    practice: Res<crate::bot::PracticeMode>,
    theater: Res<crate::theater::TheaterMode>,
    peer: Res<net::PeerProfile>,
    consent: Res<net::RematchConsent>,
    lobby: Res<net::LobbyState>,
    first_blood: Res<crate::first_blood::FirstBlood>,
    mut q: SummaryButtonQuery,
) {
    let over = *screen.get() == AppScreen::InMatch
        && matches!(*state, MatchState::MatchOver)
        // The theater's marquee owns the top strip and a tape needs no
        // rematch button — the summary card alone says TAPE ENDS.
        && !theater.active();
    let online = netplay.room_url.is_some();
    // Fled or desynced: either way the slot reads PLAY THE BOT.
    let opponent_gone = lobby.is_terminal();
    let peer_name = crate::profile::peer_name(peer.0);
    // After the lesson, the next step on an online build is a real duel.
    let primary = if first_blood.finished() && online {
        Some("FIND OPPONENT".to_string())
    } else {
        primary_label(online, practice.0, *consent, &peer_name, opponent_gone)
    };
    // Local consent shows as a pressed/armed button: inverted fill — unless
    // the opponent has gone, where the slot now reads PLAY THE BOT and a
    // stale pre-flight consent must not render it pressed.
    let armed = online && !practice.0 && consent.local && !consent.peer && !opponent_gone;
    // The primary reads as not-yet-pressable through the lockout.
    let ink = if since.settled(time.elapsed_secs()) {
        render::palette::HOT_BONE
    } else {
        render::palette::HOT_BONE.with_alpha(0.35)
    };

    for (btn, mut vis, sprite, text, color) in &mut q {
        let shown = over
            && match btn.action {
                SummaryAction::Primary => primary.is_some(),
                SummaryAction::Exit => true,
            };
        *vis = if shown {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        if !shown {
            continue;
        }
        let inverted = btn.action == SummaryAction::Primary && armed;
        let ink = if btn.action == SummaryAction::Primary {
            ink
        } else {
            render::palette::HOT_BONE
        };
        match btn.role {
            BtnRole::Border => {
                if let Some(mut s) = sprite {
                    s.color = ink;
                }
            }
            BtnRole::Fill => {
                if let Some(mut s) = sprite {
                    s.color = if inverted {
                        render::palette::HOT_BONE
                    } else {
                        render::palette::DEEP_ASH
                    };
                }
            }
            BtnRole::Label => {
                if let Some(mut t) = text {
                    t.0 = match btn.action {
                        SummaryAction::Primary => primary.clone().unwrap_or_default(),
                        SummaryAction::Exit => if online && !practice.0 {
                            "LEAVE"
                        } else {
                            "LOBBY"
                        }
                        .to_string(),
                    };
                }
                if let Some(mut c) = color {
                    c.0 = if inverted { render::palette::VOID } else { ink };
                }
            }
        }
    }
}

/// Taps on the summary's PRIMARY button. Couch/practice: arm the injected
/// THROW (the input-driven rematch). Online: give consent — the same thing
/// a raw THROW press means at MatchOver (`netplay::gate_rematch_inputs`
/// converts those), so both paths converge on one handshake.
#[allow(clippy::too_many_arguments)]
pub(super) fn summary_buttons_input(
    screen: Res<State<AppScreen>>,
    state: Res<MatchState>,
    time: Res<Time<Real>>,
    since: Res<MatchOverSince>,
    netplay: Res<NetplayConfig>,
    practice: Res<crate::bot::PracticeMode>,
    theater: Res<crate::theater::TheaterMode>,
    touches: Res<Touches>,
    window: Res<WindowSize>,
    lobby: Res<net::LobbyState>,
    mut consent: ResMut<net::RematchConsent>,
    mut queue: ResMut<net::NetSendQueue>,
    mut pending: ResMut<PendingUiThrow>,
    (first_blood, mut find, mut next): (
        Res<crate::first_blood::FirstBlood>,
        ResMut<PendingFindOpponent>,
        ResMut<NextState<AppScreen>>,
    ),
) {
    if *screen.get() != AppScreen::InMatch
        || !matches!(*state, MatchState::MatchOver)
        || theater.active()
        // A thumb still on the stick or the dash corner when the match is
        // decided sits right on this button: let the result land first.
        || !since.settled(time.elapsed_secs())
    {
        return;
    }
    if !ui::tapped(
        touches.iter_just_pressed().map(|t| t.position()),
        PLAY_BTN_RECT,
        window.0,
    ) {
        return;
    }
    // FIND OPPONENT after the lesson: out through the Title (which tears
    // the practice match down and disarms practice) and straight on in.
    if first_blood.finished() && netplay.room_url.is_some() {
        find.0 = true;
        next.set(AppScreen::Title);
        return;
    }
    let online = netplay.room_url.is_some() && !practice.0;
    if !online {
        pending.0 = UI_THROW_FRAMES;
        return;
    }
    if lobby.is_terminal() {
        return; // nobody left to run it back with, or no session worth it
    }
    if !consent.local {
        consent.local = true;
        queue.0.push(net::NetMsg::RematchWant);
    }
}

/// Online summary exit: a top-band tap or Escape LEAVES the match cleanly —
/// goodbye on the side-channel, socket + session torn down, back to Title.
/// Exclusive-world because the teardown touches the non-send socket.
pub(super) fn online_leave_input(world: &mut World) {
    let online = world.resource::<NetplayConfig>().room_url.is_some();
    let practice = world.resource::<crate::bot::PracticeMode>().0;
    let theater = world.resource::<crate::theater::TheaterMode>().active();
    if !online || practice || theater {
        return;
    }
    if !matches!(*world.resource::<MatchState>(), MatchState::MatchOver) {
        return;
    }
    let esc = world
        .resource::<ButtonInput<KeyCode>>()
        .just_pressed(KeyCode::Escape);
    let win = world.resource::<WindowSize>().0;
    let tapped = ui::tapped(
        world
            .resource::<Touches>()
            .iter_just_pressed()
            .map(|t| t.position()),
        TOP_EXIT_BAND,
        win,
    );
    if !(esc || tapped) {
        return;
    }
    crate::netplay::leave_online_match(world);
    world
        .resource_mut::<NextState<AppScreen>>()
        .set(AppScreen::Title);
}

/// The match decided when a player's kills reached `MATCH_WIN_THRESHOLD`. P0 is
/// the Cur, P1 the Stag (ART_DIRECTION v2). Pure for testing; the deciding
/// kill always belongs to exactly one side (it's a one-hit-kill game).
pub(super) fn winner_label(score: MatchScore) -> &'static str {
    if score.p0 >= MATCH_WIN_THRESHOLD {
        "CUR WINS"
    } else {
        "STAG WINS"
    }
}
