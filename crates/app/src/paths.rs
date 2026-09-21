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
//!
//! The browser is the third answer. wasm32 has no filesystem at all:
//! `dirs` hands back `None` and every `std::fs` call returns
//! `Unsupported`, so every save on the web build silently did nothing and
//! every load came back empty. A visitor got a freshly minted identity,
//! the name grid, and an empty rivalry ledger on EVERY page load. The
//! backend for the browser is `localStorage`, reached through the same
//! three seams the native path already went through — [`read_document`],
//! [`write_atomic`] and [`quarantine_corrupt`] — so the four persisted
//! documents (profile, settings, room code, career) keep their exact
//! native shape and only the floor under them changes.

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
    // The browser has no directories. This is a VIRTUAL root: nothing is
    // ever created at it, and the three document seams below key
    // `localStorage` off the file name under it. It exists so the callers
    // can keep speaking in paths.
    #[cfg(target_family = "wasm")]
    {
        Some(PathBuf::from(WEB_ROOT))
    }
    #[cfg(not(any(target_os = "android", target_family = "wasm")))]
    {
        dirs::config_dir().map(|d| d.join("two-top"))
    }
}

/// The virtual config root for the browser build (see [`config_dir`]).
#[cfg(target_family = "wasm")]
const WEB_ROOT: &str = "/two-top";

/// `localStorage` key for a document path: everything below the virtual
/// root, namespaced so the game cannot collide with anything else served
/// from the same origin (GitHub Pages hosts a whole account on one).
/// Keeping the subpath (rather than the bare file name) is what lets
/// `shared/replays/` be listed as if it were a directory.
#[cfg(target_family = "wasm")]
fn web_key(path: &Path) -> Option<String> {
    let rel = path.strip_prefix(WEB_ROOT).ok()?;
    Some(format!("two-top/{}", rel.to_str()?))
}

/// The marker that says a stored value is base64, not text. Tapes are
/// binary and `localStorage` holds strings, but the JSON documents should
/// stay readable in devtools — so only what needs encoding gets encoded.
#[cfg(target_family = "wasm")]
const WEB_B64: &str = "b64:";

/// Decode one stored value back to bytes.
#[cfg(target_family = "wasm")]
fn web_decode(value: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    match value.strip_prefix(WEB_B64) {
        Some(encoded) => base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .ok(),
        None => Some(value.as_bytes().to_vec()),
    }
}

/// The page's `localStorage`, when there is one. Safari in Lockdown /
/// private browsing and any "block all cookies" setting make this throw
/// or hand back `None` rather than a store — the whole point of routing
/// every access through here is that the game degrades to in-memory-only
/// instead of panicking in someone's browser.
#[cfg(target_family = "wasm")]
fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// Read a persisted document. Native: the file. Browser: `localStorage`.
/// The error shape matches `std::fs::read_to_string`, so callers that
/// treat "no document" as a first boot need no browser-specific arm.
pub fn read_document(path: &Path) -> std::io::Result<String> {
    #[cfg(target_family = "wasm")]
    {
        let key = web_key(path).ok_or_else(|| std::io::Error::other("no document name in path"))?;
        let raw = local_storage()
            .ok_or_else(|| std::io::Error::other("no localStorage in this browser"))?
            .get_item(&key)
            .map_err(|_| std::io::Error::other("localStorage read refused"))?
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no such document"))?;
        let bytes =
            web_decode(&raw).ok_or_else(|| std::io::Error::other("stored document is corrupt"))?;
        String::from_utf8(bytes).map_err(|_| std::io::Error::other("document is not text"))
    }
    #[cfg(not(target_family = "wasm"))]
    {
        std::fs::read_to_string(path)
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
    // The browser keeps its tapes in `localStorage` under the same
    // virtual root the config documents use. A page still cannot drop a
    // file into someone's Files app unprompted — that half is a download
    // button (`share::download_tape`), not a directory — but "the web
    // build cannot record" was never true, and cost it REPLAYS, SHARE
    // and the rivalry tape rings for no reason. A tape is ~14 KB.
    #[cfg(target_family = "wasm")]
    {
        Some(PathBuf::from(WEB_ROOT).join("shared"))
    }
    #[cfg(not(any(target_os = "android", target_family = "wasm")))]
    {
        dirs::download_dir()
            .or_else(dirs::data_dir)
            .map(|d| d.join("two-top"))
    }
}

/// Read a persisted document as raw bytes — the tape path's twin of
/// [`read_document`]. Native: the file. Browser: the (base64-decoded)
/// `localStorage` value.
pub fn read_bytes(path: &Path) -> std::io::Result<Vec<u8>> {
    #[cfg(target_family = "wasm")]
    {
        let key = web_key(path).ok_or_else(|| std::io::Error::other("no document name in path"))?;
        let raw = local_storage()
            .ok_or_else(|| std::io::Error::other("no localStorage in this browser"))?
            .get_item(&key)
            .map_err(|_| std::io::Error::other("localStorage read refused"))?
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no such document"))?;
        web_decode(&raw).ok_or_else(|| std::io::Error::other("stored document is corrupt"))
    }
    #[cfg(not(target_family = "wasm"))]
    {
        std::fs::read(path)
    }
}

/// Every document directly inside `dir` whose extension is `ext`.
///
/// Native: a filtered `read_dir`. Browser: the `localStorage` keys under
/// this directory's prefix — which is why [`web_key`] keeps the whole
/// subpath. Returns paths the other seams here accept, so callers never
/// learn which backend answered. Unordered on both (the tape screens
/// sort by the header's own timestamp).
pub fn list_dir(dir: &Path, ext: &str) -> Vec<PathBuf> {
    #[cfg(target_family = "wasm")]
    {
        let (Some(store), Some(prefix)) = (local_storage(), web_key(dir)) else {
            return Vec::new();
        };
        let prefix = format!("{}/", prefix.trim_end_matches('/'));
        let mut out = Vec::new();
        for i in 0..store.length().unwrap_or(0) {
            let Ok(Some(key)) = store.key(i) else {
                continue;
            };
            let Some(rest) = key.strip_prefix(&prefix) else {
                continue;
            };
            // Directly inside: no further separator, and the right suffix.
            if rest.contains('/') || !rest.ends_with(&format!(".{ext}")) {
                continue;
            }
            out.push(dir.join(rest));
        }
        out
    }
    #[cfg(not(target_family = "wasm"))]
    {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        entries
            .filter_map(|e| {
                let path = e.ok()?.path();
                (path.extension().and_then(|s| s.to_str()) == Some(ext)).then_some(path)
            })
            .collect()
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
    // The browser: one `localStorage` write, which is already atomic (the
    // store is a map, not a byte stream — there is no torn half to leave
    // behind, and no sibling to clean up). Quota is the failure that
    // matters here, and it arrives as an Err the caller already logs.
    #[cfg(target_family = "wasm")]
    {
        use base64::Engine as _;
        let key = web_key(path).ok_or_else(|| std::io::Error::other("no document name in path"))?;
        // Text stays text (a JSON document should be readable in
        // devtools); anything that is not valid UTF-8 — a tape — rides
        // base64 behind the marker.
        let owned = match std::str::from_utf8(bytes) {
            Ok(t) if !t.starts_with(WEB_B64) => t.to_string(),
            _ => format!(
                "{WEB_B64}{}",
                base64::engine::general_purpose::STANDARD.encode(bytes)
            ),
        };
        let text = owned.as_str();
        local_storage()
            .ok_or_else(|| std::io::Error::other("no localStorage in this browser"))?
            .set_item(&key, text)
            .map_err(|_| {
                std::io::Error::other("localStorage write refused (quota or private mode)")
            })
    }
    #[cfg(not(target_family = "wasm"))]
    {
        write_atomic_fs(path, bytes)
    }
}

#[cfg(not(target_family = "wasm"))]
fn write_atomic_fs(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
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

/// Move a file that exists but will not parse aside as a `.corrupt`
/// sibling: evidence a human can hand back, instead of bytes the next
/// save silently replaces. With [`write_atomic`] on every save, the only
/// way to a corrupt file is outside interference (a hand edit, a bad
/// disk) — and that is exactly when the bytes should survive. One helper,
/// so every persisted file gets the same answer (the profile had it; the
/// career ledger, which holds strictly more, did not).
pub fn quarantine_corrupt(path: &Path) {
    // The browser keeps the same promise: the unparseable bytes move to a
    // `.corrupt` key rather than being dropped, so they are still there
    // to hand back from a console.
    #[cfg(target_family = "wasm")]
    {
        let (Some(store), Some(key)) = (local_storage(), web_key(path)) else {
            return;
        };
        if let Ok(Some(text)) = store.get_item(&key) {
            let _ = store.set_item(&format!("{key}.corrupt"), &text);
            let _ = store.remove_item(&key);
        }
    }
    #[cfg(not(target_family = "wasm"))]
    {
        let mut name = path
            .file_name()
            .map(std::ffi::OsStr::to_os_string)
            .unwrap_or_default();
        name.push(".corrupt");
        let _ = std::fs::rename(path, path.with_file_name(name));
    }
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
