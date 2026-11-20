# vendor/

Third-party source carried in-tree because a fix we need has no release
we can move to: `bevy_render`, with one change. It is wired in through
`[patch.crates-io]` in the workspace `Cargo.toml` and is excluded from
the workspace, so `cargo fmt --all`, `cargo clippy --workspace` and the
test run never touch it.

## bevy_render 0.19.1

An unmodified copy of the crates.io release (commit `chore(vendor):
bevy_render 0.19.1, verbatim from crates.io`) plus **one** change, in
`src/view/window/mod.rs::prepare_windows`, the swapchain acquire:

```diff
-            wgpu::CurrentSurfaceTexture::Outdated => {
+            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                 render_device.configure_surface(surface, &surface_data.configuration);
```

A lost surface is rebuilt the way an outdated one is (wgpu's own
examples do the same). `git diff <verbatim commit> -- vendor/bevy_render`
shows exactly that, plus a comment.

### Why

On Android the compositor stops taking frames the moment the display
goes off or the app slides behind another one, while the surface lives
on until the activity is stopped; a frame drawn in that gap can report
the surface lost. 0.18.1 panicked on it (the history is in
MORGAN_NOTES: a panic there became "crashes right after opening").
0.19.1 no longer panics, but its `Lost` arm only logs: the surface is
never reconfigured, so the game would sit on a frozen screen logging an
error every frame. Rebuilding it, as `Outdated` already is, brings the
picture back.

`Timeout` and any other failure still log and skip the frame, as stock
0.19.1 does; only `Lost` changes.

### Removing it

On the next Bevy upgrade, read `prepare_windows`. If `Lost` is
reconfigured there (or no longer exists), delete `vendor/bevy_render`,
the `[patch.crates-io]` table and the `exclude` line in the workspace
`Cargo.toml`, and let `cargo update -p bevy_render` put the registry
source back in `Cargo.lock`. Otherwise re-vendor the new release with the
same one-arm change.

## ggrs (removed)

ggrs 0.12.0 was vendored to stop a remote input packet that skipped
ahead of what we had received from tripping an `assert!`. ggrs 0.13.0
no longer asserts: a packet whose base frame it does not hold is not
decoded, and is dropped. The acceptance test
`net::input_stream_tests::an_input_that_skips_ahead_is_dropped_not_fatal`
still drives a real `P2PSession` into that case, now against the
registry crate.
