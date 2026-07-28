//! Phase 13 — diagnostic logging setup.
//!
//! Owns the process-global `tracing` subscriber. Initialized once at
//! the top of [`crate::run`] before any other code emits an event;
//! bevy's `LogPlugin` is disabled in `DefaultPlugins` so this is the
//! sole subscriber.
//!
//! ## Outputs
//!
//! - **Dev (`debug_assertions`)**: `fmt` layer → `stderr`. Matches the
//!   prior bevy-default behavior so existing dev workflow is preserved.
//! - **Release desktop**: `fmt` layer (no ANSI) → daily-rotated file under
//!   `<exe-dir>/logs/two_top.log`. Writes go through `tracing-appender`'s
//!   non-blocking writer so disk I/O never stalls the game thread.
//! - **Release Android**: stderr/logcat AND a daily-rotated file beside the
//!   replays (`crate::paths::shared_dir()/logs/`). The file half is what
//!   makes a bug report possible: logcat is a ring buffer, so by the time a
//!   tester walks back to a computer and says "it crashed", the panic has
//!   long since rolled out of it — which is exactly how one on-device crash
//!   stayed invisible through two reports. This used to be stderr-only
//!   because NativeActivity gives no writable cwd; `paths::shared_dir()`
//!   answers that now (the recorder has been writing tapes there all along),
//!   and a failure to open it degrades to stderr instead of panicking.
//!
//! ## Filter
//!
//! `RUST_LOG` overrides everything. Default suppresses noisy bevy/wgpu
//! internals at `WARN` and lets our own `two_top::*` targets through at
//! `INFO`. `release_max_level_info` is enabled in `app/Cargo.toml`, so
//! `debug!` / `trace!` macros across the workspace compile to no-ops in
//! release regardless of filter — the per-call dispatch cost is gone.
//!
//! ## Lifetime
//!
//! The release build's non-blocking appender owns a worker thread; the
//! returned [`LogGuard`] must outlive the app or the worker is dropped
//! and pending writes are lost. `run()` binds it with `let _guard = …;`
//! immediately so it lives for the entire `App::run()` scope.

use tracing_subscriber::{EnvFilter, prelude::*};

/// Default filter when `RUST_LOG` is unset. Keeps wgpu/bevy chatter out
/// of the way; lets our own crates through at `INFO` and above. `app`,
/// `sim`, `net`, `render`, `input_touch` are the workspace's runtime
/// crates — bevy/wgpu/winit are third-party noise.
const DEFAULT_FILTER: &str = "info,\
    wgpu_core=warn,wgpu_hal=warn,naga=warn,\
    bevy_app=warn,bevy_winit=warn,bevy_render=warn,bevy_ecs=warn";

/// Holds resources whose `Drop` must run when the app exits. In
/// release desktop, this carries the `tracing-appender` `WorkerGuard` that
/// flushes pending log writes. In dev and Android release it's a unit struct —
/// those paths write synchronously to stderr/logcat and have nothing to flush.
#[must_use = "drop the LogGuard at the end of run() to flush pending log writes"]
pub struct LogGuard {
    #[cfg(all(not(debug_assertions), not(target_os = "android")))]
    _appender_guard: tracing_appender::non_blocking::WorkerGuard,
    /// `None` when the phone gave us nowhere to write (then the log is
    /// logcat-only, exactly as it was before) — never a reason to panic.
    #[cfg(all(not(debug_assertions), target_os = "android"))]
    _appender_guard: Option<tracing_appender::non_blocking::WorkerGuard>,
}

/// Where the phone's log file lives: beside the replays, in the app's
/// external files dir, so `adb pull` (or the tester's own Files app) can
/// retrieve it with no permissions and no root.
#[cfg(all(not(debug_assertions), target_os = "android"))]
fn android_log_dir() -> Option<std::path::PathBuf> {
    let dir = crate::paths::shared_dir()?.join("logs");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Build the global subscriber. Idempotency: this calls `init()` once;
/// re-invoking it would panic on subscriber re-registration. The app
/// crate enforces single-call by making this private to `run()`.
/// Persist panics where a human can hand them back.
///
/// A panic on the phone kills the process and prints to logcat, which is a
/// ring buffer — by the time the tester says "it crashed", the message has
/// rolled out and there is nothing to read (Android leaves no tombstone for
/// an unwinding Rust panic). Writing the payload + location beside the
/// replays means the next crash survives the walk back to the desk:
/// `adb pull /sdcard/Android/data/<pkg>/files/crash.log`.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown".to_string());
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic>".to_string());
        tracing::error!(target: "two_top::panic", %location, %payload, "PANIC");
        if let Some(dir) = crate::paths::shared_dir() {
            let _ = std::fs::create_dir_all(&dir);
            let _ = crate::paths::write_atomic(
                &dir.join("crash.log"),
                format!("2-Top panic\n  at {location}\n  {payload}\n").as_bytes(),
            );
        }
        previous(info);
    }));
}

pub fn init_logging() -> LogGuard {
    install_panic_hook();
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    // Compile-time sanity check: in release builds, the
    // `release_max_level_info` cargo feature on the `tracing` crate
    // must be enabled so `debug!` / `trace!` calls compile to no-ops.
    // STATIC_MAX_LEVEL surfaces the resolved compile-time level filter;
    // a release-build regression that drops the feature would surface
    // as `STATIC_MAX_LEVEL == TRACE` here. Captured into the log
    // immediately so the value is visible in any bug-report log.
    let static_max = tracing::level_filters::STATIC_MAX_LEVEL;
    debug_assert!(
        static_max >= tracing::level_filters::LevelFilter::INFO,
        "STATIC_MAX_LEVEL ({static_max:?}) must be >= INFO",
    );

    #[cfg(debug_assertions)]
    {
        tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
            .init();
        tracing::info!(
            target: "two_top::logging",
            static_max_level = ?static_max,
            mode = "dev",
            "tracing subscriber installed",
        );
        LogGuard {}
    }

    #[cfg(all(not(debug_assertions), target_os = "android"))]
    {
        // stderr (logcat) for live `adb logcat` monitoring, plus a file the
        // tester can hand back after the fact. Both, because each covers the
        // other's hole: logcat rolls, and a file cannot be watched live.
        let file_layer = android_log_dir().map(|dir| {
            let (writer, guard) = tracing_appender::non_blocking(tracing_appender::rolling::daily(
                &dir,
                "two_top.log",
            ));
            let layer = tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(false);
            (layer, guard, dir)
        });
        let (file_layer, appender_guard, dir) = match file_layer {
            Some((layer, guard, dir)) => (Some(layer), Some(guard), Some(dir)),
            None => (None, None, None),
        };
        tracing_subscriber::registry()
            .with(filter)
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(std::io::stderr)
                    .with_ansi(false),
            )
            .with(file_layer)
            .init();
        tracing::info!(
            target: "two_top::logging",
            static_max_level = ?static_max,
            mode = "android-release",
            log_file = ?dir,
            "tracing subscriber installed",
        );
        LogGuard {
            _appender_guard: appender_guard,
        }
    }

    #[cfg(all(not(debug_assertions), not(target_os = "android")))]
    {
        let log_dir = log_dir();
        // Best-effort directory creation; if it fails the appender's
        // open() will surface the underlying error.
        let _ = std::fs::create_dir_all(&log_dir);
        let appender = tracing_appender::rolling::daily(&log_dir, "two_top.log");
        let (non_blocking, guard) = tracing_appender::non_blocking(appender);
        tracing_subscriber::registry()
            .with(filter)
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(non_blocking)
                    .with_ansi(false),
            )
            .init();
        eprintln!(
            "two-top: release logging to {}/two_top.log.<date>",
            log_dir.display()
        );
        tracing::info!(
            target: "two_top::logging",
            static_max_level = ?static_max,
            mode = "release",
            log_dir = %log_dir.display(),
            "tracing subscriber installed",
        );
        LogGuard {
            _appender_guard: guard,
        }
    }
}

/// Resolve the release log directory. Order:
/// 1. `<exe-dir>/logs/` if the current exe path is resolvable.
/// 2. `./logs/` as a last-resort cwd fallback.
///
#[cfg(all(not(debug_assertions), not(target_os = "android")))]
fn log_dir() -> std::path::PathBuf {
    use std::path::PathBuf;
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        return parent.join("logs");
    }
    PathBuf::from("./logs")
}
