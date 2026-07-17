//! Where the app's own files live.
//!
//! `dirs::config_dir()` resolves from XDG vars / `$HOME`, neither of which
//! an Android NativeActivity has — so on the phone it returns a path
//! nothing can write (or nothing at all), and every save silently did
//! nothing. Four modules independently made that mistake: the profile
//! (a fresh identity minted on EVERY launch, so the name grid reopened
//! forever and the rivalry ledger could never key on a stable install-id),
//! settings, the room code, and the career record. Only the recorder got
//! it right, by asking Android where its files go.
//!
//! One helper now, so there is a single place to be wrong.

use std::path::PathBuf;

/// The app's private config directory, created on demand by the callers.
/// Android: the app's internal data path (private, survives updates,
/// removed on uninstall). Desktop: `~/.config/two-top`, unchanged.
pub fn config_dir() -> Option<PathBuf> {
    #[cfg(target_os = "android")]
    {
        bevy::android::ANDROID_APP
            .get()
            .and_then(|app| app.internal_data_path())
    }
    #[cfg(not(target_os = "android"))]
    {
        dirs::config_dir().map(|d| d.join("two-top"))
    }
}

/// A file inside [`config_dir`], e.g. `config_file("profile.json")`.
pub fn config_file(name: &str) -> Option<PathBuf> {
    config_dir().map(|d| d.join(name))
}

/// Where crash reports and replays land: somewhere a human can reach with
/// a Files app and hand back. Android: the app's external files dir (no
/// permission needed). Desktop: `~/Downloads/two-top`.
pub fn shared_dir() -> Option<PathBuf> {
    #[cfg(target_os = "android")]
    {
        bevy::android::ANDROID_APP
            .get()
            .and_then(|app| app.external_data_path())
    }
    #[cfg(not(target_os = "android"))]
    {
        dirs::download_dir()
            .or_else(dirs::data_dir)
            .map(|d| d.join("two-top"))
    }
}
