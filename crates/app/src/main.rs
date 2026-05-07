//! Desktop entry — calls into the shared `app::run()` (in lib.rs).
//!
//! Android builds skip this binary entirely: cargo-apk targets the
//! cdylib output and uses the `android_main` extern that
//! `#[bevy_main]` generates inside lib.rs.

fn main() {
    app::run();
}
