# vendor/

Third-party source carried in-tree because a fix we need has no release
we can move to. Everything here is wired in through `[patch.crates-io]`
in the workspace `Cargo.toml` and is excluded from the workspace, so
`cargo fmt --all`, `cargo clippy --workspace` and the test run never
touch it.

## bevy_render 0.18.1

An unmodified copy of the crates.io release (commit `chore(vendor):
bevy_render 0.18.1, verbatim from crates.io`) plus **one** change, in
`src/view/window/mod.rs::prepare_windows`:

```diff
-            Err(err) => {
-                panic!("Couldn't get swap chain texture, operation unrecoverable: {err}");
-            }
+            Err(err) => {
+                tracing::error!("Couldn't get swap chain texture: {err}");
+                continue;
+            }
```

`git diff <verbatim commit> -- vendor/bevy_render` shows exactly that,
plus the comment above it.

### Why

0.18.1 treats every swapchain-acquire failure except `Outdated` as
fatal. On Android that is wrong twice over:

1. wgpu gives the acquire a one second timeout, and Android 11+ honours
   it (`vkAcquireNextImageKHR` returns `VK_TIMEOUT`). The compositor
   stops taking frames the moment the display goes off or the app slides
   behind another one, while the surface itself lives until the activity
   is stopped. Any frame drawn in that gap times out, or reports the
   surface lost, and 0.18.1 panics.
2. The panic does not look like a crash. `android-activity` catches a
   panic out of `android_main`, finishes the activity and leaves the
   **process alive**. The next tap on the icon starts a second
   `android_main` in that same process, winit refuses to build a second
   event loop, and the app closes the instant it opens, every time,
   until Android gets round to killing the cached process. "Sometimes it
   crashes right after opening" is this.

`crates/app/src/platform/android.rs` closes the second half on its own (no path
out of `android_main` leaves a process behind any more). This patch
closes the first, so the frame is skipped and the match survives.

Upstream made the same change after 0.18.1: on `main`,
`prepare_windows` logs the error and carries on. There is no 0.18.2 to
pick it up from.

### Removing it

On the next Bevy upgrade: delete `vendor/bevy_render`, delete the
`[patch.crates-io]` table and the `exclude` line from the workspace
`Cargo.toml`, and let `cargo update -p bevy_render` put the registry
source back in `Cargo.lock`. Check that the release you land on still
skips the frame in `prepare_windows`.
