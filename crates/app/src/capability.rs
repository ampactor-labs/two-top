//! What the device in front of us can actually do.
//!
//! The browser build is one binary that serves a phone and a laptop, so
//! the questions that used to be answered by `#[cfg(target_os = ...)]`
//! have to be answered at runtime instead. Answering them at compile
//! time is what shipped a web build with a keyboard as its only input
//! source on a touchscreen, and a phone paying a desktop's render cost:
//! the gate was asking "which target am I?" when the thing it needed to
//! know was "is there a finger on this, and how much GPU is there?".
//!
//! These are cheap, boot-time answers. Nothing here is on a hot path.

/// Is the primary pointer a finger?
///
/// Android is a phone. A browser asks CSS the question directly —
/// `(pointer: coarse)` is exactly "the primary input is imprecise", which
/// is the thing worth branching on and not a guess from a screen width.
/// A native desktop build is mouse-and-keyboard by definition (the touch
/// layer keeps its own mouse-drag fallback for testing).
pub fn touch_primary() -> bool {
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

/// Should this device skip the HDR + bloom chain?
///
/// Measured on a Galaxy A16 (Mali class): HDR + bloom cost ~65 ms/frame
/// and MSAA another ~17, pinning the phone at 10 fps; without them it
/// locks to 60. That cost tracks the GPU, not the operating system — so
/// the test is "is this a phone", and a touch-primary device is the best
/// proxy for that we can get without probing the adapter. A desktop
/// browser on a real GPU keeps the full glow, exactly like the native
/// desktop build; a phone browser gets what the APK gets.
pub fn lean_render() -> bool {
    touch_primary()
}
