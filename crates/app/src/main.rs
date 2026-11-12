//! Desktop entry — calls into the shared `app::run()` (in lib.rs).
//!
//! Android builds skip this binary entirely: cargo-apk targets the
//! cdylib output and uses the `android_main` extern that
//! `#[bevy_main]` generates inside lib.rs.

// A released Windows build is a windowed app: no console window behind
// the game. Dev builds keep the console for stderr logs.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    app::run();
}
