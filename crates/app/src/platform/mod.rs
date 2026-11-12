//! The platform adapter: everything that differs between Android, the
//! browser and desktop, behind one face.
//!
//! The rest of the app is the kernel. It decides WHAT should happen (hold
//! the screen on through a match, buzz on a kill, offer the crash report,
//! read a join link) and asks this module to do it, the same call on every
//! build. HOW it happens lives here and only here: JNI on the phone, a page
//! hook or a JS call in the browser, nothing (yet) on desktop. A change of
//! policy is made once, in the kernel, and every build gets it; a change to
//! one platform's plumbing touches one file.
//!
//! Two kinds of thing live here:
//!
//! * [`DeviceProfile`]: runtime answers about the device, decided once at
//!   boot. Policy branches on these, never on the compile target: the
//!   browser build is one binary serving phones and laptops, so "which
//!   target am I?" was always the wrong question (it shipped a touchscreen
//!   web build whose only input was a keyboard, and a phone browser paying a
//!   desktop's render cost).
//! * The facade functions below: the device actions, each a no-op where the
//!   platform has no such thing.
//!
//! `scripts/check_platform_gates.py` keeps target `cfg`s out of the kernel:
//! they belong in this directory, in the bootstrap (`lib.rs`), and in the
//! few files that are themselves platform I/O backends (storage, logging,
//! the network fetches).

#[cfg(target_os = "android")]
pub mod android;
#[cfg(not(any(target_os = "android", target_family = "wasm")))]
pub mod desktop;
#[cfg(target_family = "wasm")]
pub mod web;

use bevy::prelude::*;

use crate::lifecycle::LastExit;

/// What the device in front of us is and can do, decided once at boot and
/// held as a resource. Every presentation policy that used to be a
/// `#[cfg(...)]` reads this instead.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceProfile {
    /// The primary pointer is a finger: draw the touch controls, teach with
    /// touch words.
    pub touch_primary: bool,
    /// Skip HDR + bloom. Measured on a Galaxy A16 (Mali): ~65 ms/frame for
    /// the chain and ~17 for MSAA, pinning it at 10 fps; without them it
    /// locks to 60. The cost tracks the GPU, and a touch-primary device is
    /// the best proxy for "phone GPU" available without probing the adapter.
    pub lean_render: bool,
    /// Request the GPU device with the portable core feature set only. A
    /// phone's Vulkan driver is where "advertised" and "survives being
    /// enabled" part ways (the APK that ran on a Galaxy A16 died at launch
    /// on a Pixel 6); this 2D game uses none of the optional features.
    pub core_gpu_only: bool,
    /// The OS has a share sheet the app can hand text to (the crash report).
    pub has_share_sheet: bool,
    /// Show the couch keyboard legend: a keyboard is the likely input.
    pub shows_keyboard_legend: bool,
    /// F11 toggles the app's own fullscreen (a browser's is the browser's).
    pub fullscreen_key: bool,
    /// A finished tape can be handed over as a download even with no tape
    /// drop configured (a browser player has no replays folder to reach).
    pub can_download_tapes: bool,
}

impl DeviceProfile {
    /// Ask the device. Cheap; called once in `run()`.
    pub fn detect() -> Self {
        let touch_primary = touch_primary();
        let android = cfg!(target_os = "android");
        let web = cfg!(target_family = "wasm");
        let desktop = !android && !web;
        Self {
            touch_primary,
            lean_render: touch_primary,
            core_gpu_only: !web
                && (android || std::env::var("TWOTOP_LEAN_GPU").is_ok_and(|v| v == "1")),
            has_share_sheet: android,
            shows_keyboard_legend: !touch_primary,
            fullscreen_key: desktop,
            can_download_tapes: web,
        }
    }
}

/// Is the primary pointer a finger?
///
/// Android is a phone. A browser asks CSS the question directly —
/// `(pointer: coarse)` is exactly "the primary input is imprecise", which
/// is the thing worth branching on, not a guess from a screen width. A
/// native desktop build is mouse-and-keyboard by definition.
fn touch_primary() -> bool {
    #[cfg(target_os = "android")]
    {
        true
    }
    #[cfg(target_family = "wasm")]
    {
        web_sys::window()
            .and_then(|w| w.match_media("(pointer: coarse)").ok().flatten())
            .is_some_and(|m| m.matches())
    }
    #[cfg(not(any(target_os = "android", target_family = "wasm")))]
    {
        false
    }
}

/// One vibration of `ms` milliseconds. Best-effort everywhere: a missing
/// or refused vibrator must never take down the game loop.
pub fn vibrate(ms: i64) {
    #[cfg(target_os = "android")]
    android::vibrate(ms);
    #[cfg(target_family = "wasm")]
    web::vibrate(ms);
    #[cfg(not(any(target_os = "android", target_family = "wasm")))]
    let _ = ms;
}

/// Hold the screen on, or hand it back to the system's timeout.
pub fn keep_screen_on(on: bool) {
    #[cfg(target_os = "android")]
    android::keep_screen_on(on);
    #[cfg(target_family = "wasm")]
    web::keep_screen_on(on);
    #[cfg(not(any(target_os = "android", target_family = "wasm")))]
    let _ = on;
}

/// Hand a piece of text to the OS share sheet, where there is one
/// ([`DeviceProfile::has_share_sheet`]); printed to stderr elsewhere.
pub fn share_text(title: &str, text: &str) {
    #[cfg(target_os = "android")]
    android::share_text(title, text);
    // platform-gate-ok: no browser or desktop share sheet to hand it to.
    #[cfg(not(target_os = "android"))]
    eprintln!("{title}\n{text}");
}

/// The join link this launch was opened with (`twotop://join/CURS-pit`
/// from the APK's intent, `#join=CURS-pit` from the page), if any.
pub fn launch_link() -> Option<String> {
    #[cfg(target_os = "android")]
    {
        android::launch_uri()
    }
    #[cfg(target_family = "wasm")]
    {
        web::join_link()
    }
    #[cfg(not(any(target_os = "android", target_family = "wasm")))]
    {
        None
    }
}

/// A join link the join page left on the clipboard for an app that was
/// already running (Android's warm start). Claimed once, then wiped.
pub fn clipboard_join_link() -> Option<String> {
    #[cfg(target_os = "android")]
    {
        android::clipboard_join_link()
    }
    // platform-gate-ok: the browser gets its link from the URL, at launch.
    #[cfg(not(target_os = "android"))]
    {
        None
    }
}

/// "samsung SM-A165F, Android 14 (sdk 34)", or the OS and architecture.
pub fn device_summary() -> String {
    #[cfg(target_os = "android")]
    {
        android::device_summary()
    }
    // platform-gate-ok: the browser's OS/arch line is as good as desktop's.
    #[cfg(not(target_os = "android"))]
    {
        format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
    }
}

/// How the previous run of the app ended, where the OS keeps a record
/// (Android 11+).
pub fn last_exit() -> Option<LastExit> {
    #[cfg(target_os = "android")]
    {
        android::last_exit()
    }
    // platform-gate-ok: no platform but Android keeps an exit record.
    #[cfg(not(target_os = "android"))]
    {
        None
    }
}

/// Run blocking work on a thread of its own.
///
/// Bevy's task pools are NOT that in this build. The curated feature list
/// in `Cargo.toml` leaves `multi_threaded` off (one thread runs every
/// system, which is what the phone build was tuned and verified on), and
/// without it `IoTaskPool` is the single-threaded pool: `spawn` runs the
/// task right there on the caller's thread until it first waits, and a
/// task that never waits — a blocking HTTP request — simply runs to
/// completion inside the frame that spawned it. The ICE credential fetch
/// and the tape upload were both "on the IO pool" in exactly that sense.
///
/// A thread that cannot be spawned drops `work` unrun, and with it the
/// channel sender it holds, so the caller's poll sees a disconnect and
/// takes its fallback. A browser has no threads to offer; its callers
/// use the page's own async `fetch` instead.
#[cfg(not(target_family = "wasm"))]
pub fn off_main_thread(name: &'static str, work: impl FnOnce() + Send + 'static) {
    if let Err(e) = std::thread::Builder::new().name(name.into()).spawn(work) {
        tracing::warn!(target: "two_top::app", name, error = %e, "could not spawn a worker thread");
    }
}

/// The platform's own systems: Android's Back key, the browser's shared-
/// tape autoplay. Desktop has none yet.
pub struct PlatformPlugin;

impl Plugin for PlatformPlugin {
    fn build(&self, app: &mut App) {
        #[cfg(target_os = "android")]
        app.add_plugins(android::AndroidPlugin);
        #[cfg(target_family = "wasm")]
        app.add_systems(Update, web::watch_autoplay);
        let _ = app;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_desktop_profile_is_a_keyboard_machine_with_full_rendering() {
        let p = DeviceProfile::detect();
        assert!(!p.touch_primary);
        assert!(!p.lean_render);
        assert!(p.shows_keyboard_legend);
        assert!(p.fullscreen_key);
        assert!(!p.has_share_sheet);
        assert!(!p.can_download_tapes);
    }
}
