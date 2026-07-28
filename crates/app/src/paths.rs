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

use std::path::{Path, PathBuf};

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

/// Write `bytes` to `path` by writing a `.tmp` sibling and renaming it over
/// the target. A sibling is on the same filesystem by construction, where
/// `rename` is atomic on every platform we ship, so a process kill mid-write
/// leaves the old file or the new one on disk — never a truncated half.
/// Android kills backgrounded apps freely, and the files routed through here
/// (the identity, the ledger, settings, tapes) are exactly the ones a
/// truncation would quietly destroy: a half-written profile.json reads as no
/// identity at all, and the code downstream would mint a fresh install-id
/// and orphan the rivalry ledger on both phones.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let file_name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("write_atomic needs a file path"))?;
    let mut tmp_name = file_name.to_os_string();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        // Leave the target alone; just don't leak the corpse.
        let _ = std::fs::remove_file(&tmp);
    })
}

/// A per-test scratch directory under the repo's `target/` (never the
/// system temp dir — the dev box's /tmp is a small tmpfs with a hard
/// quota). Shared by the persistence tests across this crate's modules.
#[cfg(test)]
pub(crate) fn test_scratch(test: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test_scratch")
        .join(format!("{test}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("test scratch dir");
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_atomic_replaces_the_target_and_cleans_the_sibling() {
        let dir = test_scratch("atomic_replace");
        let path = dir.join("state.json");
        write_atomic(&path, b"old").unwrap();
        write_atomic(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert!(
            !dir.join("state.json.tmp").exists(),
            "no sibling left behind"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_crashed_write_leaves_the_old_file_readable() {
        // The crash shape: the process died after writing the sibling but
        // before the rename. The target still holds the old bytes, and the
        // next successful save replaces the corpse instead of tripping on it.
        let dir = test_scratch("atomic_crash");
        let path = dir.join("state.json");
        write_atomic(&path, b"the identity").unwrap();
        std::fs::write(dir.join("state.json.tmp"), b"trunca").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"the identity");
        write_atomic(&path, b"next save").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"next save");
        assert!(!dir.join("state.json.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_atomic_refuses_a_bare_directory_path() {
        assert!(write_atomic(Path::new("/"), b"x").is_err());
    }
}
