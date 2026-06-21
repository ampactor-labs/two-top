# Sideload to Android

How to put 2-Top on a physical Android device for testing. This is the dev-time install path — cheap, fast, and intentionally minimal. Store packaging (Play Store, signing, ProGuard, R8 minification) is a later concern.

> **Status:** Phases 0–18 complete, v1.0.0-rc1. A sideloaded build now launches the *full* game: it boots into the Title/arena-picker screen and plays full matches — movement, dash, boomerang throw/recall, pickups, audio, and Android haptics. Online is opt-in via `--room`/`MATCHBOX_ROOM` (see `SIGNALING.md`).

## Prerequisites (one-time)

### 1. Rust target

```sh
rustup target add aarch64-linux-android
```

Modern Android phones ship arm64. Adding `armv7-linux-androideabi` is only needed if you have a 32-bit-only device (rare since 2019).

### 2. Android NDK

The NDK is what cross-compiles Rust to Android. Download from <https://developer.android.com/ndk/downloads> (pick the latest LTS line — r26 or newer at time of writing) and unzip it somewhere stable.

Set one of these env vars (cargo-apk reads either):

```sh
export ANDROID_NDK_ROOT="$HOME/Android/Ndk/android-ndk-r26d"
# or:
export ANDROID_NDK_HOME="$HOME/Android/Ndk/android-ndk-r26d"
```

Persist this in your shell profile (`~/.bashrc`, `~/.zshrc`, etc.) so you don't have to re-export each session.

### 3. Android SDK (build-tools + platform)

`cargo-apk` needs the SDK too — its `aapt2` (resource/manifest compile), `zipalign`, and `apksigner` come from **build-tools**, and it links against a **platform** `android.jar`. The lean, no-Android-Studio path is Google's command-line tools + `sdkmanager`:

```sh
export ANDROID_HOME="$HOME/Android/Sdk"
mkdir -p "$ANDROID_HOME/cmdline-tools" && cd "$ANDROID_HOME/cmdline-tools"
# Grab the current "Command line tools only (Linux)" zip from
# https://developer.android.com/studio#command-tools and swap the build number:
curl -fL -o clt.zip "https://dl.google.com/android/repository/commandlinetools-linux-11076708_latest.zip"
unzip -q clt.zip && mv cmdline-tools latest && rm clt.zip
yes | latest/bin/sdkmanager --licenses
# platform-tools also gives you adb; build-tools/platform match target_sdk = 34:
latest/bin/sdkmanager "platform-tools" "build-tools;34.0.0" "platforms;android-34" "ndk;26.3.11579264"
```

Then set `ANDROID_HOME`/`ANDROID_SDK_ROOT` (and `ANDROID_NDK_ROOT="$ANDROID_HOME/ndk/26.3.11579264"`) in your shell profile, and add `$ANDROID_HOME/platform-tools` to `PATH`. Installing the NDK via `sdkmanager` (as above) is an alternative to the manual unzip in step 2 — either works, just point `ANDROID_NDK_ROOT` at the resulting dir.

### 4. cargo-apk

```sh
cargo install cargo-apk
```

The vanilla `rust-mobile/cargo-apk` is what 2-Top targets. It uses `android.app.NativeActivity` (the system-framework activity), which matches `crates/sim/Cargo.toml`'s `android-activity` feature choice (`native-activity`). Switching to GameActivity later requires `cargo-apk2` or `xbuild`.

### 5. ADB

If you installed `platform-tools` via `sdkmanager` (step 3), you already have `adb` at `$ANDROID_HOME/platform-tools/adb` — just make sure it's on `PATH`. Otherwise use your distro's package:

```sh
# Fedora
sudo dnf install android-tools

# Ubuntu/Debian
sudo apt install adb

# macOS (Homebrew)
brew install android-platform-tools
```

Confirm it works:

```sh
adb version
```

### 6. Phone in dev mode

On the device:

1. **Settings → About phone → tap "Build number" seven times.** Developer options unlocks.
2. **Settings → System → Developer options → USB debugging.** Toggle on.
3. Plug the phone into the dev box with a USB-C/USB-A cable that supports data (some "charging-only" cables don't).
4. The phone will prompt **"Allow USB debugging from this computer?"** — accept, and check "Always allow" so you don't get prompted every session.

Confirm the host sees the device:

```sh
adb devices
```

You should see one device listed with status `device` (not `unauthorized` and not `offline`).

## Day-to-day: build + install + run

```sh
cargo apk run -p app --target aarch64-linux-android
```

That single command:

1. Cross-compiles `crates/app` to `aarch64-linux-android` as a `cdylib` (`libapp.so`).
2. Generates `AndroidManifest.xml` from the `[package.metadata.android.*]` block in `crates/app/Cargo.toml`.
3. Packages the `.so` + manifest + a debug-signing certificate into an APK.
4. Pushes the APK to the connected device over ADB.
5. Installs and launches the activity.

You'll see the launcher icon labeled **2-Top** on the device. The first launch may take a second; subsequent launches are immediate.

### Assets are bundled

The runtime assets are packaged into the APK via `assets = "../../assets"` in the `[package.metadata.android]` block. The app loads `assets/audio/*.wav` (12 cues) + `assets/sprites/**` through the `AssetServer` at runtime, and the bundled dir becomes the APK asset root so the in-code load paths line up unchanged. On your first sideload, confirm sprites render and audio plays — a blank/silent build means the assets path is wrong.

### Audio and haptics are device-only

Synthesized SFX (`bevy_audio` + `wav`, 12 cues) and Android haptics (the JNI `Vibrator`) are excluded from CI and the determinism matrix *by design* — a real device is the only verification surface. After install, verify that throw / kill / perfect-catch buzz (needs `VIBRATE` granted **and** haptics enabled in `Settings`) and that the SFX play. On desktop, `vibrate` is a no-op.

### Build only (no install)

```sh
cargo apk build -p app --target aarch64-linux-android --release
```

The APK lands under `target/aarch64-linux-android/release/apk/`. Copy it off the dev box and sideload manually if you don't have the phone tethered.

### Tail logs

`cargo apk run` doesn't pipe stdout/stderr back. Use logcat:

```sh
adb logcat -s "RustStdoutStderr:*" "Bevy:*"
```

Or watch everything from the app's process:

```sh
adb logcat --pid=$(adb shell pidof com.ampactorlabs.twotop)
```

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `error: ANDROID_NDK_ROOT is not set` | NDK env var missing | Re-export per step 2 above |
| `linker `aarch64-linux-android-clang` not found` | NDK toolchain not on PATH | cargo-apk should set this automatically; verify NDK version is 23+ (older NDKs have different toolchain layouts) |
| `adb: no devices/emulators found` | Phone not authorized or cable is charge-only | Re-check USB debugging toggle, accept the host fingerprint prompt, swap cable |
| `INSTALL_FAILED_USER_RESTRICTED` | Some Samsung devices block sideload | Settings → Apps → Special access → Install unknown apps → enable for the source (file manager, etc.) |
| App installs but launches to a black screen, no error | wgpu can't find a Vulkan-capable GPU | Verify device supports Vulkan 1.1+ (`adb shell pm list features | grep vulkan`); some pre-2018 devices won't. |
| App immediately exits | Look at `adb logcat` for a Rust panic — the SyncTest mismatch observer is intentional and any output containing "SyncTestMismatch" indicates a real determinism violation, not a build issue |
| Launches but silent / untextured sprites | Assets dir not packaged into the APK | Confirm `assets = "../../assets"` in `[package.metadata.android]` packages `assets/audio` + `assets/sprites` into the APK |
| Throw / kill don't vibrate | `VIBRATE` missing, or haptics disabled in settings | `VIBRATE` is now declared in the manifest; enable haptics in the Title-screen settings |

## What's deliberately deferred

* **GameActivity** — better gamepad routing. One feature flip + a packager swap when the time comes (see `crates/sim/Cargo.toml` comment).
* **Multi-arch APK** — `build_targets` currently lists arm64 only. Add `armv7-linux-androideabi` and `x86_64-linux-android` (the latter for emulators) when broader compatibility matters.
* **Release signing** — the debug cert cargo-apk generates is fine for sideload but not for distribution. Play Store submission needs an upload key and Play App Signing.
* **Permissions** — `INTERNET` (netplay) and `VIBRATE` (haptics) are both declared in the metadata block. Without `VIBRATE`, the JNI Vibrator call throws a swallowed `SecurityException` and haptics silently never fire. Further networking-permission opt-ins (Wi-Fi state, Bluetooth for LAN play, etc.) are added when the relevant phase lands.
