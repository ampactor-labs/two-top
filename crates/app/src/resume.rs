//! Reconnect by replay: a live duel survives its connection dying.
//!
//! A phone moving from Wi-Fi to mobile data, the room server restarting, a
//! call that held the app in the background long enough for the link to
//! drop: the transport under the match is gone and matchbox has no
//! reconnect. Before this, every one of those ended the match as an
//! unfinished drop. Now both phones:
//!
//! 1. notice the drop the way they always did (the socket ending, the
//!    peer's connection closing, ggrs or the silence timer giving up),
//!    and instead of booking a forfeit ([`begin_resume`]), keep what they
//!    know: the inputs of every tick both had confirmed, the seats, the
//!    input delay, the table, the identities;
//! 2. dial the room named after the broken session ([`net::resume_room`]),
//!    where only the two of them land;
//! 3. say hello with how many ticks each holds, and send the tail the
//!    other is missing with a hash of the part both must share
//!    ([`net::merge_resume`]): both end up with the same N ticks of input;
//! 4. lay the table fresh and re-simulate all N ticks ([`sim::fast_forward`],
//!    in chunks so a long match never stalls a frame for long): the world
//!    on both phones is the world at tick N, by construction;
//! 5. start a new ggrs session on the new connection with the same seats
//!    and delay, its frame 0 at sim tick N ([`ClockBase`]), and both send
//!    HOLD for a moment, so the round restarts with a fair countdown
//!    (SIM 17, `sim::apply_resume_hold`).
//!
//! If the two never meet again within [`RESUME_WINDOW_SECS`], the match is
//! booked exactly as the drop always was: unfinished, nobody's win.
//!
//! The app itself can die too (Android reclaims a backgrounded app during
//! a long call). Once a second a live duel writes a [`Checkpoint`]; a
//! relaunch inside the window finds it and goes straight back into the
//! resume room, where the other phone is still waiting.

use bevy::prelude::*;
use bevy_ggrs::Session;
use net::{LobbyState, MatchboxPeerId as PeerId, PeerState, ResumeMsg};
use sim::{GgrsCfg, PlayerInput};

use crate::sim_clock::ClockBase;

/// How long both phones keep trying to meet again.
pub const RESUME_WINDOW_SECS: f32 = 60.0;

/// Ticks re-simulated per frame while catching up: a full five-kill match
/// is ~20 000 ticks, so this keeps every frame short while finishing in
/// seconds even on a slow phone.
const REPLAY_CHUNK: usize = 1_200;

/// Ticks of input both phones send HOLD on after a resume: enough to
/// overlap on the same ticks whatever the input delay.
const HOLD_TICKS: u32 = 30;

/// A resume room dial that has not produced an agreed match by now is
/// re-dialed: a WebRTC handshake started while the network was still
/// down fails, and matchbox never retries it on its own.
const REDIAL_SECS: f32 = 8.0;

/// At most this many resumes per session: a link that keeps dying is not
/// a match anyone is enjoying.
const MAX_RESUMES: u32 = 3;

/// The resume in flight, if any.
#[derive(Resource)]
pub struct Resume {
    /// The broken session's original peer ids (ours, theirs).
    ids: (u128, u128),
    local_handle: usize,
    input_delay: usize,
    peer_id: PeerId,
    /// Confirmed inputs from the session's first tick.
    inputs: Vec<[PlayerInput; 2]>,
    /// The broken session, kept so giving up books it as the drop it was.
    old_session: Option<Session<GgrsCfg>>,
    /// The resume room URL, and when it was last dialed.
    url: String,
    dialed_at: f32,
    deadline: f32,
    phase: Phase,
}

enum Phase {
    /// Dialing the resume room; talking to `peer` once it is there.
    Talk {
        peer: Option<PeerId>,
        hello_sent: bool,
        their_count: Option<u32>,
        tail_sent: bool,
    },
    /// Re-simulating up to `inputs.len()`.
    Replay { next: usize, peer: PeerId },
}

/// Resumes so far this session.
#[derive(Resource, Default)]
pub struct ResumeCount(pub u32);

/// Ticks before which the local input carries HOLD (after a resume).
#[derive(Resource, Default)]
pub struct ResumeHold(pub Option<u32>);

/// What a relaunched app needs to resume the duel it died in.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub struct Checkpoint {
    /// Unix seconds when written: a checkpoint older than the resume
    /// window is a duel the other phone has given up on.
    pub saved_unix: u64,
    pub ids: (u128, u128),
    pub local_handle: u8,
    pub input_delay: u8,
    pub arena: u8,
    pub peer_id: u128,
    pub peer: Option<net::ProfileData>,
    pub peer_key: Option<[u8; 32]>,
    pub url: String,
    pub inputs: Vec<[PlayerInput; 2]>,
}

const CHECKPOINT_FILE: &str = "resume.bin";

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Seconds of the resume window a checkpoint written at `saved` still has
/// at `now`; `None` once it has run out. Pure.
pub fn checkpoint_left(saved: u64, now: u64) -> Option<f32> {
    let age = now.saturating_sub(saved) as f32;
    (age < RESUME_WINDOW_SECS).then_some(RESUME_WINDOW_SECS - age)
}

fn clear_checkpoint() {
    if let Some(path) = crate::paths::config_file(CHECKPOINT_FILE) {
        let _ = std::fs::remove_file(path);
    }
}

/// Once a second during a live online duel: the checkpoint a relaunch
/// resumes from. Removed the moment the duel cannot be resumed any more
/// (decided, or no longer live).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn write_checkpoint(
    time: Res<Time<Real>>,
    session: Option<Res<Session<GgrsCfg>>>,
    resume: Option<Res<Resume>>,
    began: Res<crate::netplay::DuelBegan>,
    state: Res<sim::MatchState>,
    lobby: Res<LobbyState>,
    world_bits: (
        Res<crate::netplay::SessionIds>,
        Res<crate::netplay::LocalPlayerHandle>,
        Res<crate::netplay::SessionLink>,
        Res<sim::SelectedArena>,
        Res<net::PeerProfile>,
        Res<net::PeerKeys>,
        Res<crate::room_code::RoomCode>,
        Res<crate::recorder::MatchRecorder>,
    ),
    clock: crate::sim_clock::SimClock,
    mut next_at: Local<f32>,
    mut written: Local<bool>,
) {
    let live = matches!(session.as_deref(), Some(Session::P2P(_)))
        && began.0
        && !state.is_match_over()
        && lobby.is_in_match();
    if resume.is_some() {
        return; // a resume in flight keeps the last live checkpoint
    }
    if !live {
        if std::mem::take(&mut *written) {
            clear_checkpoint();
        }
        return;
    }
    let now = time.elapsed_secs();
    if now < *next_at {
        return;
    }
    *next_at = now + 1.0;
    let (ids, local, link, arena, peer, keys, room, recorder) = world_bits;
    let (Some(ids), Some(handle), Some(peer_id)) = (ids.0, local.0, lobby_peer(&lobby)) else {
        return;
    };
    let (lo, hi) = (ids.0.min(ids.1), ids.0.max(ids.1));
    let Some(url) = room.resume_url(&net::resume_room(lo, hi)) else {
        return;
    };
    let checkpoint = Checkpoint {
        saved_unix: unix_now(),
        ids,
        local_handle: handle as u8,
        input_delay: link.input_delay as u8,
        arena: arena.0.as_u8(),
        peer_id,
        peer: peer.0,
        peer_key: keys.0,
        url,
        inputs: recorder.inputs_through(clock.confirmed_tick().max(0) as usize),
    };
    if let (Some(path), Ok(bytes)) = (
        crate::paths::config_file(CHECKPOINT_FILE),
        postcard::to_allocvec(&checkpoint),
    ) && crate::paths::write_atomic(&path, &bytes).is_ok()
    {
        *written = true;
    }
}

fn lobby_peer(lobby: &LobbyState) -> Option<u128> {
    match lobby {
        LobbyState::Connected { peer_id } | LobbyState::Disconnected { peer_id, .. } => {
            Some(peer_id.0.as_u128())
        }
        _ => None,
    }
}

/// A checkpoint found at boot, still inside its window: the next match
/// entry resumes it instead of looking for a new opponent.
#[derive(Resource)]
pub struct PendingCheckpoint(pub Checkpoint);

fn load_checkpoint(mut commands: Commands, netplay: Res<crate::netplay::NetplayConfig>) {
    let Some(path) = crate::paths::config_file(CHECKPOINT_FILE) else {
        return;
    };
    let Ok(bytes) = crate::paths::read_bytes(&path) else {
        return;
    };
    clear_checkpoint();
    let Ok(checkpoint) = postcard::from_bytes::<Checkpoint>(&bytes) else {
        return;
    };
    if netplay.room_url.is_none() || checkpoint_left(checkpoint.saved_unix, unix_now()).is_none() {
        return;
    }
    tracing::warn!(
        target: "two_top::resume",
        ticks = checkpoint.inputs.len(),
        "relaunched inside a dropped duel's window — going back in",
    );
    commands.insert_resource(PendingCheckpoint(checkpoint));
}

/// On the Title with a pending checkpoint: straight into the match.
fn enter_pending_resume(
    pending: Option<Res<PendingCheckpoint>>,
    mut practice: ResMut<crate::bot::PracticeMode>,
    mut selected: ResMut<sim::SelectedArena>,
    mut next: ResMut<NextState<crate::screen::AppScreen>>,
) {
    let Some(pending) = pending else {
        return;
    };
    practice.0 = false;
    selected.0 = sim::ArenaId::from_u8(pending.0.arena);
    next.set(crate::screen::AppScreen::InMatch);
}

/// Match entry with a pending checkpoint (`netplay::start_matchbox`): set
/// the duel back up from it and dial the resume room. `true` if it did.
pub fn begin_from_checkpoint(world: &mut World) -> bool {
    let Some(PendingCheckpoint(cp)) = world.remove_resource::<PendingCheckpoint>() else {
        return false;
    };
    let Some(left) = checkpoint_left(cp.saved_unix, unix_now()) else {
        return false;
    };
    let handle = cp.local_handle as usize;
    let peer_id = PeerId(uuid::Uuid::from_u128(cp.peer_id));
    world.resource_mut::<crate::netplay::LocalPlayerHandle>().0 = Some(handle);
    world.resource_mut::<crate::netplay::SessionIds>().0 = Some(cp.ids);
    world.resource_mut::<crate::netplay::DuelBegan>().0 = true;
    world.resource_mut::<net::PeerProfile>().0 = cp.peer;
    world.resource_mut::<net::PeerKeys>().0 = cp.peer_key;
    world.resource_mut::<render::PerspectiveFlip>().0 = if handle == 1 { -1.0 } else { 1.0 };
    world.resource_mut::<ResumeCount>().0 += 1;
    let now = world.resource::<Time<Real>>().elapsed_secs();
    world.insert_resource(Resume {
        ids: cp.ids,
        local_handle: handle,
        input_delay: cp.input_delay as usize,
        peer_id,
        inputs: cp.inputs,
        old_session: None,
        url: cp.url.clone(),
        dialed_at: now,
        deadline: now + left,
        phase: Phase::Talk {
            peer: None,
            hello_sent: false,
            their_count: None,
            tail_sent: false,
        },
    });
    crate::netplay::open_resume_socket(world, cp.url);
    *world.resource_mut::<LobbyState>() = LobbyState::Reconnecting { peer_id };
    true
}

/// May this ending be resumed instead? A live duel (begun, not decided)
/// whose link died, not a goodbye; within the per-session budget. Pure.
pub fn resumable(
    lobby: &LobbyState,
    duel_began: bool,
    decided: bool,
    resumes: u32,
    has_room: bool,
) -> bool {
    matches!(
        lobby,
        LobbyState::Forfeited {
            conceded: false,
            ..
        }
    ) && duel_began
        && !decided
        && resumes < MAX_RESUMES
        && has_room
}

/// In the ending set, before the session is booked: turn a dropped live
/// duel into a resume. Everything the rebuild needs is captured here,
/// while the old session's confirmed frame is still in the world.
pub fn begin_resume(world: &mut World) {
    if world.contains_resource::<Resume>() {
        return;
    }
    let lobby = world.resource::<LobbyState>().clone();
    let decided = world.resource::<sim::MatchState>().is_match_over();
    let began = world.resource::<crate::netplay::DuelBegan>().0;
    let resumes = world.resource::<ResumeCount>().0;
    let ids = world.resource::<crate::netplay::SessionIds>().0;
    let handle = world.resource::<crate::netplay::LocalPlayerHandle>().0;
    let room = ids.and_then(|(a, b)| {
        let (lo, hi) = (a.min(b), a.max(b));
        world
            .resource::<crate::room_code::RoomCode>()
            .resume_url(&net::resume_room(lo, hi))
    });
    let LobbyState::Forfeited { peer_id, .. } = lobby else {
        return;
    };
    if !resumable(&lobby, began, decided, resumes, room.is_some()) {
        return;
    }
    let (Some(ids), Some(local_handle), Some(url)) = (ids, handle, room) else {
        return;
    };
    let confirmed = crate::sim_clock::confirmed_tick(world).max(0) as usize;
    let inputs = world
        .resource::<crate::recorder::MatchRecorder>()
        .inputs_through(confirmed);
    let input_delay = world.resource::<crate::netplay::SessionLink>().input_delay;
    let old_session = world.remove_resource::<Session<GgrsCfg>>();
    crate::netplay::drop_connection(world);
    let now = world.resource::<Time<Real>>().elapsed_secs();
    tracing::warn!(
        target: "two_top::resume",
        confirmed = inputs.len(),
        resumes,
        "the link under a live duel died — dialing the resume room",
    );
    world.resource_mut::<ResumeCount>().0 += 1;
    world.insert_resource(Resume {
        ids,
        local_handle,
        input_delay,
        peer_id,
        inputs,
        old_session,
        url: url.clone(),
        dialed_at: now,
        deadline: now
            + crate::config::Knob::ResumeSecs
                .parse()
                .unwrap_or(RESUME_WINDOW_SECS),
        phase: Phase::Talk {
            peer: None,
            hello_sent: false,
            their_count: None,
            tail_sent: false,
        },
    });
    crate::netplay::open_resume_socket(world, url);
    *world.resource_mut::<LobbyState>() = LobbyState::Reconnecting { peer_id };
}

impl Resume {
    /// Seconds left in the resume window, for the overlay.
    pub fn seconds_left(&self, now: f32) -> f32 {
        (self.deadline - now).max(0.0)
    }
}

/// Per frame while a resume is in flight: talk, then replay, then start
/// the new session. Exclusive (the socket is non-send).
pub fn drive_resume(world: &mut World) {
    let Some(mut resume) = world.remove_resource::<Resume>() else {
        return;
    };
    let now = world.resource::<Time<Real>>().elapsed_secs();
    let outcome = match resume.phase {
        Phase::Talk { .. } if now > resume.deadline => Step::GiveUp("the peer never came back"),
        Phase::Talk { .. } if now - resume.dialed_at > REDIAL_SECS => {
            tracing::info!(target: "two_top::resume", "no agreement yet — dialing the resume room again");
            crate::netplay::drop_connection(world);
            crate::netplay::open_resume_socket(world, resume.url.clone());
            *world.resource_mut::<LobbyState>() = LobbyState::Reconnecting {
                peer_id: resume.peer_id,
            };
            resume.dialed_at = now;
            resume.phase = Phase::Talk {
                peer: None,
                hello_sent: false,
                their_count: None,
                tail_sent: false,
            };
            Step::Continue
        }
        Phase::Talk { .. } => talk(world, &mut resume),
        Phase::Replay { .. } => replay(world, &mut resume),
    };
    match outcome {
        Step::Continue => {
            world.insert_resource(resume);
        }
        Step::GiveUp(why) => give_up(world, resume, why),
        Step::Done(peer) => start_session(world, resume, peer),
    }
}

enum Step {
    Continue,
    GiveUp(&'static str),
    Done(PeerId),
}

fn talk(world: &mut World, resume: &mut Resume) -> Step {
    let Phase::Talk {
        peer,
        hello_sent,
        their_count,
        tail_sent,
    } = &mut resume.phase
    else {
        return Step::Continue;
    };
    let (lo, hi) = (
        resume.ids.0.min(resume.ids.1),
        resume.ids.0.max(resume.ids.1),
    );
    let Some(updates) = crate::netplay::poll_resume_socket(world) else {
        return Step::GiveUp("the resume room could not be reached");
    };
    for (id, state) in updates {
        match state {
            PeerState::Connected => {
                *peer = Some(id);
                *hello_sent = false;
                *their_count = None;
                *tail_sent = false;
            }
            PeerState::Disconnected if *peer == Some(id) => *peer = None,
            PeerState::Disconnected => {}
        }
    }
    let Some(p) = *peer else {
        return Step::Continue;
    };
    if !*hello_sent {
        crate::netplay::send_resume(
            world,
            p,
            &ResumeMsg::Hello {
                lo,
                hi,
                confirmed: resume.inputs.len() as u32,
            },
        );
        *hello_sent = true;
    }
    for msg in crate::netplay::receive_resume(world, p) {
        match msg {
            ResumeMsg::Hello {
                lo: their_lo,
                hi: their_hi,
                confirmed,
            } => {
                if (their_lo, their_hi) != (lo, hi) {
                    return Step::GiveUp("a stranger in the resume room");
                }
                *their_count = Some(confirmed);
            }
            ResumeMsg::Tail {
                from,
                prefix,
                inputs,
            } => {
                if !*tail_sent {
                    // Ordered channel: their hello came first, so our tail
                    // went out below on the frame it did; never otherwise.
                    return Step::GiveUp("the resume handshake arrived out of order");
                }
                return match net::merge_resume(&resume.inputs, from, prefix, &inputs) {
                    Ok(merged) => {
                        tracing::info!(
                            target: "two_top::resume",
                            ticks = merged.len(),
                            "both phones agree on the match so far — rebuilding it",
                        );
                        resume.inputs = merged;
                        rebuild_table(world);
                        resume.phase = Phase::Replay { next: 0, peer: p };
                        Step::Continue
                    }
                    Err(why) => Step::GiveUp(why),
                };
            }
        }
        if let (Some(theirs), false) = (*their_count, *tail_sent) {
            let from = theirs.min(resume.inputs.len() as u32) as usize;
            crate::netplay::send_resume(
                world,
                p,
                &ResumeMsg::Tail {
                    from: from as u32,
                    prefix: net::inputs_hash(&resume.inputs[..from]),
                    inputs: resume.inputs[from..].to_vec(),
                },
            );
            *tail_sent = true;
        }
    }
    Step::Continue
}

/// Clear the match off the table and lay it again, as a fresh session
/// would: the re-simulation starts from exactly the table the match
/// started on.
fn rebuild_table(world: &mut World) {
    let doomed: Vec<Entity> = world
        .query_filtered::<Entity, With<bevy_ggrs::Rollback>>()
        .iter(world)
        .collect();
    for entity in doomed {
        world.despawn(entity);
    }
    if let Err(e) = world.run_system_cached(crate::screen::despawn_match_effects) {
        tracing::error!(target: "two_top::resume", error = %e, "could not clear the table");
    }
    if let Err(e) = world.run_system_cached(crate::screen::rebuild_match) {
        tracing::error!(target: "two_top::resume", error = %e, "could not relay the table");
    }
    sim::reset_match_resources(world);
}

fn replay(world: &mut World, resume: &mut Resume) -> Step {
    let Phase::Replay { next, peer } = &mut resume.phase else {
        return Step::Continue;
    };
    let end = (*next + REPLAY_CHUNK).min(resume.inputs.len());
    sim::fast_forward(world, &resume.inputs[*next..end], |_| {});
    *next = end;
    if *next < resume.inputs.len() {
        Step::Continue
    } else {
        Step::Done(*peer)
    }
}

fn start_session(world: &mut World, resume: Resume, peer: PeerId) {
    let ticks = resume.inputs.len() as u32;
    crate::screen::reset_rollback_clock(world);
    world.insert_resource(ClockBase(ticks as i32));
    world.resource_mut::<ResumeHold>().0 = Some(ticks + HOLD_TICKS);
    crate::netplay::start_resumed_session(world, peer, resume.local_handle, resume.input_delay);
    // The broken session dies with this frame: its ggrs channel belonged
    // to a socket that is already gone.
    drop(resume.old_session);
    tracing::info!(
        target: "two_top::resume",
        ticks,
        "resumed — the match continues after a countdown",
    );
}

/// The two never met again (or disagreed): book the match the way the
/// drop always was, as if no resume had been tried.
fn give_up(world: &mut World, resume: Resume, why: &'static str) {
    tracing::warn!(target: "two_top::resume", why, "giving up the resume");
    crate::netplay::drop_connection(world);
    if let Some(session) = resume.old_session {
        world.insert_resource(session);
    }
    // What was confirmed when the link died is what gets booked.
    crate::sim_clock::set_confirmed_tick(world, resume.inputs.len() as i32);
    *world.resource_mut::<LobbyState>() = LobbyState::Forfeited {
        peer_id: resume.peer_id,
        conceded: false,
    };
    crate::netplay::conclude_session(world);
}

/// `ReadInputs`, after the local input is shaped: while a resume's
/// countdown is due, the local input carries HOLD.
pub fn hold_after_resume(
    hold: Res<ResumeHold>,
    frame: Res<sim::FrameCount>,
    local: Res<crate::netplay::LocalPlayerHandle>,
    inputs: Option<ResMut<bevy_ggrs::LocalInputs<GgrsCfg>>>,
) {
    let (Some(until), Some(handle), Some(mut inputs)) = (hold.0, local.0, inputs) else {
        return;
    };
    if frame.0 >= until {
        return;
    }
    if let Some(input) = inputs.0.get_mut(&handle) {
        input.buttons |= PlayerInput::HOLD;
    }
}

pub struct ResumePlugin;

impl Plugin for ResumePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ResumeCount>()
            .init_resource::<ResumeHold>()
            .add_systems(PostStartup, load_checkpoint)
            .add_systems(
                Update,
                (
                    write_checkpoint,
                    enter_pending_resume.run_if(in_state(crate::screen::AppScreen::Title)),
                ),
            )
            .add_systems(
                Update,
                (
                    // After everything that can call the link dead (the
                    // netplay driver, the silence timer), before anything
                    // books the ending.
                    begin_resume
                        .in_set(crate::netplay::EndingSet)
                        .after(crate::netplay::drive_netplay)
                        .after(net::tick_disconnection_grace)
                        .before(crate::netplay::conclude_session),
                    drive_resume.after(begin_resume),
                ),
            )
            .add_systems(
                bevy_ggrs::prelude::ReadInputs,
                hold_after_resume.after(crate::LocalInputSet::Shape),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer() -> PeerId {
        PeerId(uuid::Uuid::from_u128(5))
    }

    #[test]
    fn a_checkpoint_lives_for_the_resume_window() {
        assert_eq!(checkpoint_left(100, 100), Some(RESUME_WINDOW_SECS));
        assert_eq!(checkpoint_left(100, 130), Some(RESUME_WINDOW_SECS - 30.0));
        assert_eq!(checkpoint_left(100, 100 + RESUME_WINDOW_SECS as u64), None);
        // A clock that went backwards reads as fresh, not as a panic.
        assert_eq!(checkpoint_left(200, 100), Some(RESUME_WINDOW_SECS));
    }

    #[test]
    fn only_a_live_undecided_drop_resumes() {
        let drop = LobbyState::Forfeited {
            peer_id: peer(),
            conceded: false,
        };
        assert!(resumable(&drop, true, false, 0, true));
        // A goodbye is a goodbye.
        let bye = LobbyState::Forfeited {
            peer_id: peer(),
            conceded: true,
        };
        assert!(!resumable(&bye, true, false, 0, true));
        // Before the first GO nothing was at stake: the room re-dials.
        assert!(!resumable(&drop, false, false, 0, true));
        // A decided match is booked, not resumed.
        assert!(!resumable(&drop, true, true, 0, true));
        // A link that keeps dying stops being retried.
        assert!(!resumable(&drop, true, false, MAX_RESUMES, true));
        // No room to meet in.
        assert!(!resumable(&drop, true, false, 0, false));
        let desync = LobbyState::Desynced {
            peer_id: peer(),
            frame: 3,
        };
        assert!(!resumable(&desync, true, false, 0, true));
    }
}
