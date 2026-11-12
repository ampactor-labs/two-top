//! Desktop (Windows, macOS, Linux): where a shipped build finds its files.

use std::path::{Path, PathBuf};

/// The asset root for this run, as an absolute path for Bevy's file reader.
///
/// The desktop build used to point the reader at the workspace's `assets/`
/// through `CARGO_MANIFEST_DIR` — an absolute path on the machine that
/// compiled it. That is right under `cargo run` and wrong everywhere else:
/// a build copied to another machine (or another folder) launched with no
/// sprites, no floors and no sound. Each place a packaged build can carry
/// its assets is probed, then the checkout for a dev run.
pub fn asset_root() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let dev = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
    pick_asset_root(exe_dir.as_deref(), &dev, |p| p.join("sprites").is_dir())
}

/// The candidates, in order: beside the executable (the Windows and Linux
/// archives), in a macOS bundle's `Resources`, in a Linux install's
/// `share/two-top` (an AppImage or a distro package), then the checkout.
/// The first that holds the game's sprites wins; with none, the place
/// beside the executable (its absence is then logged by every load, which
/// is the honest failure). Pure over `holds_assets` for tests.
fn pick_asset_root(
    exe_dir: Option<&Path>,
    dev: &Path,
    holds_assets: impl Fn(&Path) -> bool,
) -> PathBuf {
    let mut candidates = Vec::new();
    if let Some(dir) = exe_dir {
        candidates.push(dir.join("assets"));
        candidates.push(dir.join("../Resources/assets"));
        candidates.push(dir.join("../share/two-top/assets"));
    }
    candidates.push(dev.to_path_buf());
    candidates
        .iter()
        .find(|c| holds_assets(c))
        .cloned()
        .unwrap_or_else(|| candidates[0].clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_packaged_build_finds_the_assets_it_carries() {
        let exe = Path::new("/opt/2-Top");
        let dev = Path::new("/build/two-top/assets");
        // Archive layout: beside the executable, even with a checkout around.
        let root = pick_asset_root(Some(exe), dev, |_| true);
        assert_eq!(root, exe.join("assets"));
        // A macOS bundle: Contents/MacOS/2-Top -> Contents/Resources/assets.
        let mac = Path::new("/Applications/2-Top.app/Contents/MacOS");
        let root = pick_asset_root(Some(mac), dev, |p| p.ends_with("Resources/assets"));
        assert_eq!(root, mac.join("../Resources/assets"));
        // A Linux install: usr/bin -> usr/share/two-top/assets.
        let bin = Path::new("/tmp/.mount/usr/bin");
        let root = pick_asset_root(Some(bin), dev, |p| p.ends_with("share/two-top/assets"));
        assert_eq!(root, bin.join("../share/two-top/assets"));
    }

    #[test]
    fn a_dev_run_falls_back_to_the_checkout() {
        let dev = Path::new("/build/two-top/assets");
        let root = pick_asset_root(Some(Path::new("/build/two-top/target/debug")), dev, |p| {
            p == dev
        });
        assert_eq!(root, dev);
    }

    #[test]
    fn with_no_assets_anywhere_it_names_the_place_beside_the_executable() {
        let exe = Path::new("/somewhere");
        let root = pick_asset_root(Some(exe), Path::new("/nope"), |_| false);
        assert_eq!(root, exe.join("assets"));
    }

    #[test]
    fn this_checkout_is_found() {
        assert!(asset_root().join("sprites").is_dir());
    }
}
