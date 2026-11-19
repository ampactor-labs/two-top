//! Verification capture (`TWOTOP_CAPTURE`): a screenshot after a settle,
//! then exit. Headless checks and CI read the PNG.

use super::*;

/// Verification capture mode. With `TWOTOP_CAPTURE=<path.png>` set, the app
/// renders for a settle window (so assets/atlases/visibility resolve), grabs a
/// PNG of the primary window's framebuffer, then exits — letting a render
/// change be *seen* (or screenshot-diffed) without a human watching the window.
/// Unset → the resource is never inserted and the system never registered, so a
/// normal `cargo run -p app` pays nothing. `TWOTOP_CAPTURE_FRAMES` overrides the
/// settle window (default 90 ≈ 1.5 s at 60 Hz).
///
/// `TWOTOP_CAPTURE_ON=matchover` gates the settle countdown on the sim
/// reaching `MatchState::MatchOver` first — the fps-independent way to grab
/// an endgame screen (the summary card + buttons) under a software renderer,
/// where a fixed render-frame target maps to an unknowable wall-clock time.
#[derive(Resource)]
pub(crate) struct CaptureConfig {
    path: String,
    settle_frames: u32,
    wait_matchover: bool,
}

pub(crate) fn capture_config_from_env() -> Option<CaptureConfig> {
    let path = crate::config::Knob::Capture
        .value()
        .filter(|p| !p.is_empty())?;
    let settle_frames = crate::config::Knob::CaptureFrames.parse().unwrap_or(90);
    let wait_matchover = crate::config::Knob::CaptureOn.value().as_deref() == Some("matchover");
    Some(CaptureConfig {
        path,
        settle_frames,
        wait_matchover,
    })
}

/// `Last`-schedule capture driver: (optionally wait for a gate, then) count
/// settle frames, fire one screenshot at the mark, and quit a few frames
/// later so the async GPU readback + file write have flushed. The
/// `save_to_disk` observer owns the actual encode.
pub(crate) fn capture_frame(
    mut commands: Commands,
    cfg: Res<CaptureConfig>,
    match_state: Res<sim::MatchState>,
    mut frame: Local<u32>,
    mut fired: Local<bool>,
    mut exit: MessageWriter<AppExit>,
) {
    use bevy::render::view::screenshot::{Screenshot, save_to_disk};
    // Hold the settle counter at zero until the gate opens (if any), so
    // `settle_frames` is a delay measured from MatchOver, not app start.
    if cfg.wait_matchover && *frame == 0 && !match_state.is_match_over() {
        return;
    }
    *frame += 1;
    if !*fired {
        if *frame >= cfg.settle_frames {
            let path = cfg.path.clone();
            tracing::warn!(target: "two_top::capture", path = %path, frame = *frame, "capturing frame");
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk(path));
            *fired = true;
        }
        return;
    }
    if *frame >= cfg.settle_frames + 20 {
        exit.write(AppExit::Success);
    }
}
