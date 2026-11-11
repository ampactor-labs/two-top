//! What the device is, and what it is doing to the app.
//!
//! A phone moves an app through states a desktop never shows it: behind
//! another window, behind the lock screen, with its drawing surface taken
//! away and handed back. None of that was in the log, so a tester's "it
//! closed on me" arrived with no record of what the phone had been doing a
//! second earlier. This module writes that record down: one line naming
//! the device and its GPU at boot, and one line per lifecycle or focus
//! change after it. The crash report (`crate::crash_report`) carries the
//! tail of that log.
//!
//! The pieces of phone policy that are plain decisions rather than JNI
//! (when the screen is held on, what Back may do, which process exits are
//! worth reporting) live here too, so they compile and test on the dev
//! box, and the screen-on policy is shared by the APK and the browser.

use std::sync::OnceLock;

use bevy::prelude::*;
use bevy::window::{AppLifecycle, WindowFocused, WindowResized};

use crate::screen::{AppScreen, BotOffer};

/// The device line, set once at `Startup`. The panic hook and the crash
/// report read it; before `Startup` it is empty.
static DEVICE: OnceLock<String> = OnceLock::new();

/// "samsung SM-A165F, Android 14 (sdk 34), Mali-G52 MC2 (Vulkan, ...)",
/// or as much of it as this platform knows.
pub fn device_summary() -> &'static str {
    DEVICE.get().map(String::as_str).unwrap_or("")
}

/// The platform's half of the device line (the GPU half comes from wgpu).
fn platform_summary() -> String {
    #[cfg(target_os = "android")]
    {
        crate::android::device_summary()
    }
    // platform-gate-ok: the browser's OS/arch line is as good as desktop's.
    #[cfg(not(target_os = "android"))]
    {
        format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
    }
}

/// `Startup`: name the device and the GPU, once, where a bug report will
/// carry it. The renderer already exists by `Startup`, so the adapter is
/// the one the game is actually drawing with.
fn describe_device(adapter: Option<Res<bevy::render::renderer::RenderAdapterInfo>>) {
    let mut line = platform_summary();
    if let Some(adapter) = adapter {
        line.push_str(&format!(
            ", {} ({:?}, {} {})",
            adapter.name, adapter.backend, adapter.driver, adapter.driver_info
        ));
    }
    tracing::info!(target: "two_top::device", device = %line, "running on");
    let _ = DEVICE.set(line);
}

/// One log line per lifecycle or focus change, and per settled resize.
/// Resizes arrive in bursts (a desktop drag, a phone's first frames), so
/// only a size that differs from the last one logged is written.
fn log_lifecycle(
    mut lifecycle: MessageReader<AppLifecycle>,
    mut focus: MessageReader<WindowFocused>,
    mut resized: MessageReader<WindowResized>,
    mut last_size: Local<(u32, u32)>,
) {
    for state in lifecycle.read() {
        tracing::info!(target: "two_top::lifecycle", ?state, "app lifecycle");
    }
    for event in focus.read() {
        tracing::info!(target: "two_top::lifecycle", focused = event.focused, "window focus");
    }
    if let Some(event) = resized.read().last() {
        let size = (event.width.round() as u32, event.height.round() as u32);
        if size != *last_size {
            *last_size = size;
            tracing::info!(
                target: "two_top::lifecycle",
                width = size.0,
                height = size.1,
                "window size",
            );
        }
    }
}

/// Why the previous process of this app ended, as the OS recorded it
/// (Android 11+: `ApplicationExitInfo`). Nothing else can see a crash in
/// a GPU driver or an ANR: neither runs a line of our code on the way out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastExit {
    /// `ApplicationExitInfo.getReason()`.
    pub reason: i32,
    /// Exit code or signal number, depending on the reason.
    pub status: i32,
    /// Milliseconds since the epoch.
    pub timestamp_ms: i64,
    /// The system's own words for it; often empty.
    pub description: String,
}

impl LastExit {
    /// The `REASON_*` constant's name.
    pub fn reason_name(&self) -> &'static str {
        match self.reason {
            1 => "EXIT_SELF",
            2 => "SIGNALED",
            3 => "LOW_MEMORY",
            4 => "CRASH",
            5 => "CRASH_NATIVE",
            6 => "ANR",
            7 => "INITIALIZATION_FAILURE",
            8 => "PERMISSION_CHANGE",
            9 => "EXCESSIVE_RESOURCE_USAGE",
            10 => "USER_REQUESTED",
            11 => "USER_STOPPED",
            12 => "DEPENDENCY_DIED",
            13 => "OTHER",
            14 => "FREEZER",
            15 => "PACKAGE_STATE_CHANGE",
            16 => "PACKAGE_UPDATED",
            _ => "UNKNOWN",
        }
    }

    /// Did the app die of something worth a report: a crash, a native
    /// crash, an ANR, or a failed start? Being killed for memory, swiped
    /// away or updated is ordinary life on a phone.
    pub fn is_abnormal(&self) -> bool {
        matches!(self.reason, 4..=7)
    }

    /// One line for a log or a report.
    pub fn describe(&self) -> String {
        let mut line = format!("{} (status {})", self.reason_name(), self.status);
        if !self.description.is_empty() {
            line.push_str(": ");
            line.push_str(&self.description);
        }
        line
    }
}

/// The most recent exit record for this app, where the platform keeps one.
pub fn last_exit() -> Option<LastExit> {
    #[cfg(target_os = "android")]
    {
        crate::android::last_exit()
    }
    // platform-gate-ok: no platform but Android keeps an exit record.
    #[cfg(not(target_os = "android"))]
    {
        None
    }
}

/// Does a press of the phone's Back reach the game (as Escape)?
///
/// Everywhere but live play. On gesture navigation a thumb that drags the
/// move stick off the screen's edge IS the system Back gesture, and in a
/// round Escape arms QUIT: two stray swipes would end a ranked match. A
/// live round is in a match, with an opponent present (not waiting, not a
/// tape), and not yet decided.
// Android's to apply; it lives here so it is tested where the tests run.
// platform-gate-ok: a browser has no Back key for the game to read.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub fn back_reaches_the_game(in_match: bool, no_opponent: bool, match_over: bool) -> bool {
    !in_match || no_opponent || match_over
}

/// Seconds a summons may hang with the screen held on. A phone left on the
/// table in a waiting room should get to sleep eventually.
pub const WAITING_SCREEN_ON_SECS: f32 = 180.0;

/// Should the screen be held on? Through a match, its summary and a tape
/// (all `InMatch`), and through the first minutes of waiting for an
/// opponent (`waiting_secs`). Menus keep the system's own timeout.
pub fn wants_screen_on(in_match: bool, waiting_secs: Option<f32>) -> bool {
    in_match && waiting_secs.is_none_or(|waited| waited < WAITING_SCREEN_ON_SECS)
}

/// Hold the screen on, or hand it back to the system's timeout.
///
/// Android sets `FLAG_KEEP_SCREEN_ON` on the window. The browser asks the
/// page, which owns the Screen Wake Lock (`web/index.html` defines
/// `twotopKeepAwake`; a page without it, or a browser without the API, is
/// simply a screen that times out as before). Desktop has nothing to hold.
fn keep_screen_on(on: bool) {
    #[cfg(target_os = "android")]
    crate::android::keep_screen_on(on);
    #[cfg(target_family = "wasm")]
    {
        use wasm_bindgen::{JsCast, JsValue};
        let Some(window) = web_sys::window() else {
            return;
        };
        let Ok(hook) = js_sys::Reflect::get(&window, &JsValue::from_str("twotopKeepAwake")) else {
            return;
        };
        if let Some(hook) = hook.dyn_ref::<js_sys::Function>() {
            let _ = hook.call1(&window, &JsValue::from_bool(on));
        }
    }
    #[cfg(not(any(target_os = "android", target_family = "wasm")))]
    let _ = on;
}

/// Apply [`wants_screen_on`] whenever its answer changes.
///
/// Nothing reset the phone's screen timeout while a player watched without
/// touching: waiting for a challenger, a replay, the summary. On Android
/// the display going dark under a live surface is also where a swapchain
/// acquire starts timing out (`vendor/README.md`); in a browser the tab
/// stops being drawn at all and the opponent sees a walkout.
fn hold_screen_on(
    screen: Res<State<AppScreen>>,
    offer: Res<BotOffer>,
    time: Res<Time<Real>>,
    mut applied: Local<Option<bool>>,
) {
    let waiting = offer.since.map(|since| time.elapsed_secs() - since);
    let want = wants_screen_on(*screen.get() == AppScreen::InMatch, waiting);
    if *applied != Some(want) {
        keep_screen_on(want);
        *applied = Some(want);
    }
}

/// Device line at boot, lifecycle breadcrumbs after it, and the screen-on
/// policy.
pub struct LifecyclePlugin;

impl Plugin for LifecyclePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, describe_device)
            .add_systems(Update, (log_lifecycle, hold_screen_on));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exit(reason: i32) -> LastExit {
        LastExit {
            reason,
            status: 6,
            timestamp_ms: 1,
            description: String::new(),
        }
    }

    #[test]
    fn only_crashes_anrs_and_failed_starts_are_abnormal() {
        for reason in [4, 5, 6, 7] {
            assert!(exit(reason).is_abnormal(), "{}", exit(reason).reason_name());
        }
        // Killed for memory, swiped away, updated, or left on purpose.
        for reason in [0, 1, 2, 3, 8, 10, 11, 16, 99] {
            assert!(
                !exit(reason).is_abnormal(),
                "{}",
                exit(reason).reason_name()
            );
        }
    }

    #[test]
    fn describe_names_the_reason_and_keeps_the_systems_words() {
        assert_eq!(exit(5).describe(), "CRASH_NATIVE (status 6)");
        let anr = LastExit {
            description: "Input dispatching timed out".into(),
            ..exit(6)
        };
        assert_eq!(
            anr.describe(),
            "ANR (status 6): Input dispatching timed out"
        );
        assert_eq!(exit(42).reason_name(), "UNKNOWN");
    }

    #[test]
    fn the_screen_is_held_through_a_match_but_not_a_menu() {
        assert!(wants_screen_on(true, None));
        assert!(!wants_screen_on(false, None));
        assert!(!wants_screen_on(false, Some(1.0)));
    }

    #[test]
    fn a_forgotten_waiting_room_lets_the_phone_sleep() {
        assert!(wants_screen_on(true, Some(5.0)));
        assert!(wants_screen_on(true, Some(WAITING_SCREEN_ON_SECS - 0.1)));
        assert!(!wants_screen_on(true, Some(WAITING_SCREEN_ON_SECS)));
    }

    #[test]
    fn back_is_ignored_only_during_a_live_round() {
        // Menus, a waiting room, a tape, a decided match: Back is Escape.
        assert!(back_reaches_the_game(false, false, false));
        assert!(back_reaches_the_game(true, true, false));
        assert!(back_reaches_the_game(true, false, true));
        // A round in play: an edge swipe must not arm QUIT.
        assert!(!back_reaches_the_game(true, false, false));
    }
}
