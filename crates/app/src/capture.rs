//! Verification capture (`TWOTOP_CAPTURE`): a screenshot after a settle,
//! then exit. Headless checks and CI read the PNG.

use super::*;

/// Verification capture mode, one knob:
/// `TWOTOP_CAPTURE=<path.png>[@<when>]`. The app renders until `<when>`,
/// grabs a PNG of the primary window's framebuffer, then exits — letting a
/// render change be *seen* (or screenshot-diffed) without a human watching.
/// `<when>` is a frame count (`shot.png@120`), `matchover` (the moment the
/// sim decides a match: the summary card) or `matchover+<frames>`; bare
/// `shot.png` waits 90 frames. Unset → nothing is inserted or registered.
///
/// A capture runs on a manual clock (every frame advances time by exactly
/// one sim tick, however slow the renderer) and a fixed system order, so
/// two captures of one build and scene stand on the same tick with the same
/// cosmetic draws. They are not yet byte-identical: a few hundred pixels on
/// the duelists still vary between runs under lavapipe (cause not yet
/// found), so comparisons allow a small tolerance.
#[derive(Resource, Debug, PartialEq, Eq)]
pub(crate) struct CaptureConfig {
    path: String,
    settle_frames: u32,
    wait_matchover: bool,
}

pub(crate) fn capture_config_from_env() -> Option<CaptureConfig> {
    parse_capture(&crate::config::Knob::Capture.value()?)
}

/// `<path.png>[@<frames>|@matchover[+<frames>]]`. Pure.
fn parse_capture(spec: &str) -> Option<CaptureConfig> {
    let (path, when) = match spec.rsplit_once('@') {
        Some((path, when)) => (path, Some(when)),
        None => (spec, None),
    };
    if path.is_empty() {
        return None;
    }
    let (wait_matchover, frames) = match when {
        None => (false, Some(90)),
        Some("matchover") => (true, Some(0)),
        Some(w) => match w.strip_prefix("matchover+") {
            Some(n) => (true, n.parse().ok()),
            None => (false, w.parse().ok()),
        },
    };
    Some(CaptureConfig {
        path: path.to_string(),
        settle_frames: frames?,
        wait_matchover,
    })
}

/// Install a capture: the manual clock, a fixed system order, the driver.
///
/// Several presentation systems draw from the one cosmetic stream; run in
/// parallel, their order (and so who gets which number) changes from run
/// to run. Single-threaded, the schedule's order is the same every time.
pub(crate) fn install(app: &mut App, cfg: CaptureConfig) {
    use bevy::ecs::schedule::SingleThreadedExecutor;
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        sim::tick_duration(),
    ))
    .insert_resource(cfg)
    .add_systems(Last, capture_frame);
    fn serial(app: &mut App, label: impl bevy::ecs::schedule::ScheduleLabel) {
        app.edit_schedule(label, |schedule| {
            schedule.set_executor(SingleThreadedExecutor::new());
        });
    }
    serial(app, First);
    serial(app, PreUpdate);
    serial(app, Update);
    serial(app, PostUpdate);
    serial(app, Last);
}

/// `Last`-schedule capture driver: (optionally wait for a gate, then) count
/// settle frames, fire one screenshot at the mark, and quit a few frames
/// later so the async GPU readback + file write have flushed. The
/// `save_to_disk` observer owns the actual encode.
pub(crate) fn capture_frame(
    mut commands: Commands,
    cfg: Res<CaptureConfig>,
    match_state: Res<sim::MatchState>,
    sim_frame: Res<sim::FrameCount>,
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
            tracing::warn!(target: "two_top::capture", path = %path, frame = *frame, tick = sim_frame.0, "capturing frame");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_capture_spec_reads_every_form() {
        let at = |path: &str, frames, matchover| CaptureConfig {
            path: path.into(),
            settle_frames: frames,
            wait_matchover: matchover,
        };
        assert_eq!(parse_capture("a.png"), Some(at("a.png", 90, false)));
        assert_eq!(parse_capture("a.png@120"), Some(at("a.png", 120, false)));
        assert_eq!(parse_capture("a.png@matchover"), Some(at("a.png", 0, true)));
        assert_eq!(
            parse_capture("/t/a.png@matchover+30"),
            Some(at("/t/a.png", 30, true))
        );
        assert_eq!(parse_capture(""), None);
        assert_eq!(parse_capture("a.png@soon"), None);
    }
}
