//! Desktop (Windows, macOS, Linux): where a shipped build finds its files,
//! the join link it was opened with, the one running instance a second
//! launch hands its link to, the `twotop://` scheme, and its window icon.

use std::io::Write as _;
use std::path::{Path, PathBuf};

/// The join link among the launch arguments: `--join CURS-pit`,
/// `--join=CURS-pit`, or a bare link (`CURS-pit`, `twotop://join/CURS-pit`,
/// a join page URL), which is what an OS passes when the `twotop://`
/// scheme opens the game. Whether it really is a join link is
/// `room_code::parse_join`'s to say. Pure over the arguments.
pub fn link_from_args(args: impl IntoIterator<Item = String>) -> Option<String> {
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--join" {
            return args.next();
        }
        if let Some(link) = arg.strip_prefix("--join=") {
            return Some(link.to_string());
        }
        if !arg.starts_with('-') && (arg.contains("://") || arg.contains('-')) {
            return Some(arg);
        }
    }
    None
}

/// The join link this launch was opened with.
pub fn launch_link() -> Option<String> {
    link_from_args(std::env::args().skip(1))
}

const PENDING_JOIN: &str = "pending_join.txt";

/// The lock that marks this process as the running game, held for life.
static INSTANCE: std::sync::OnceLock<std::fs::File> = std::sync::OnceLock::new();

/// One game per player: if another 2-Top already runs on this profile and
/// this launch carries a join link (the browser just opened a `twotop://`
/// link), leave the link for the running game ([`pending_join_link`]) and
/// return `true`: the caller exits. A launch with no link always runs (two
/// windows are a dev tool's couch). The lock is a file in the config dir,
/// held with the OS's advisory lock, which a crashed process releases.
pub fn hand_off_to_running_instance(link: Option<&str>) -> bool {
    let Some(path) = crate::paths::config_file("instance.lock") else {
        return false;
    };
    // The first launch on a profile has no config dir yet; without it the
    // lock was never taken and a second launch ran beside the first.
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
    else {
        return false;
    };
    match file.try_lock() {
        Ok(()) => {
            let _ = INSTANCE.set(file);
            false
        }
        Err(_) => {
            let Some(link) = link else {
                return false;
            };
            let Some(pending) = crate::paths::config_file(PENDING_JOIN) else {
                return false;
            };
            let handed = crate::paths::write_atomic(&pending, link.as_bytes()).is_ok();
            if handed {
                let _ = writeln!(std::io::stderr(), "2-Top is already running: joining there");
            }
            handed
        }
    }
}

/// A join link a second launch left for this running game. Claimed once
/// (the file is removed), on focus, like Android's clipboard hand-off.
pub fn pending_join_link() -> Option<String> {
    let path = crate::paths::config_file(PENDING_JOIN)?;
    let link = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    let link = link.trim().to_string();
    (!link.is_empty()).then_some(link)
}

/// The `reg` commands that make `twotop://` open this executable for the
/// current user (no admin rights: HKCU). Pure, so every platform tests it.
#[cfg(any(windows, test))]
pub fn scheme_registration(exe: &str) -> Vec<Vec<String>> {
    let key = r"HKCU\Software\Classes\twotop";
    let open = format!("\"{exe}\" \"%1\"");
    [
        vec!["add", key, "/ve", "/d", "URL:2-Top", "/f"],
        vec!["add", key, "/v", "URL Protocol", "/d", "", "/f"],
        vec![
            "add",
            &format!(r"{key}\shell\open\command"),
            "/ve",
            "/d",
            &open,
            "/f",
        ],
    ]
    .into_iter()
    .map(|args| args.into_iter().map(str::to_string).collect())
    .collect()
}

/// Windows: register `twotop://` for this user on launch (re-registered
/// every launch, so a moved install keeps working). The join page's OPEN
/// IN 2-TOP then lands here with the link as the argument.
#[cfg(windows)]
pub fn register_scheme() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    for args in scheme_registration(&exe.to_string_lossy()) {
        let _ = std::process::Command::new("reg").args(&args).output();
    }
}

/// `Update`, until it has run once: give the window the game's icon.
pub fn set_window_icon(
    windows: Option<bevy::prelude::NonSend<bevy::winit::WinitWindows>>,
    mut done: bevy::prelude::Local<bool>,
) {
    if *done {
        return;
    }
    let Some(windows) = windows else {
        return;
    };
    let Some(window) = windows.windows.values().next() else {
        return;
    };
    *done = true;
    let png = include_bytes!("../../../../web/icon-192.png");
    let Ok(image) = bevy::image::Image::from_buffer(
        png,
        bevy::image::ImageType::Extension("png"),
        bevy::image::CompressedImageFormats::NONE,
        true,
        bevy::image::ImageSampler::Default,
        bevy::asset::RenderAssetUsages::MAIN_WORLD,
    ) else {
        return;
    };
    let (w, h) = (image.width(), image.height());
    let Some(rgba) = image.data else {
        return;
    };
    if let Ok(icon) = winit::window::Icon::from_rgba(rgba, w, h) {
        window.set_window_icon(Some(icon));
    }
}

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

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_join_link_is_found_among_the_arguments() {
        assert_eq!(
            link_from_args(args(&["--join", "CURS-pit"])).as_deref(),
            Some("CURS-pit")
        );
        assert_eq!(
            link_from_args(args(&["--join=CURS-pit"])).as_deref(),
            Some("CURS-pit")
        );
        assert_eq!(
            link_from_args(args(&["twotop://join/CURS-pit"])).as_deref(),
            Some("twotop://join/CURS-pit")
        );
        assert_eq!(
            link_from_args(args(&["CURS-pit"])).as_deref(),
            Some("CURS-pit")
        );
        // Other flags are not links.
        assert_eq!(link_from_args(args(&["--room=ws://h/two-top"])), None);
        assert_eq!(link_from_args(args(&[])), None);
        // Every form the scheme and the page produce parses as a join.
        for link in [
            "CURS-pit",
            "twotop://join/CURS-pit",
            "https://x.io/join.html#CURS-pit",
        ] {
            assert!(crate::room_code::parse_join(link).is_some(), "{link}");
        }
    }

    #[test]
    fn the_scheme_registers_for_this_user_with_the_link_as_argument() {
        let cmds = scheme_registration(r"C:\Games\2-Top\2-Top.exe");
        assert!(
            cmds.iter()
                .all(|c| c[0] == "add" && c[1].starts_with(r"HKCU\Software\Classes\twotop"))
        );
        assert!(cmds.iter().any(|c| c.contains(&"URL Protocol".to_string())));
        let open = cmds.last().unwrap();
        assert!(open[1].ends_with(r"shell\open\command"));
        assert!(open.contains(&r#""C:\Games\2-Top\2-Top.exe" "%1""#.to_string()));
    }

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
