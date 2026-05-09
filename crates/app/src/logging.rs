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
//! - **Release**: `fmt` layer (no ANSI) → daily-rotated file under
//!   `<exe-dir>/logs/two_top.log`. Writes go through
//!   `tracing-appender`'s non-blocking writer so disk I/O never stalls
//!   the game thread.
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
/// release, this carries the `tracing-appender` `WorkerGuard` that
/// flushes pending log writes. In dev it's a unit struct — the dev
/// path writes synchronously to stderr and has nothing to flush.
#[must_use = "drop the LogGuard at the end of run() to flush pending log writes"]
pub struct LogGuard {
    #[cfg(not(debug_assertions))]
    _appender_guard: tracing_appender::non_blocking::WorkerGuard,
}

/// Build the global subscriber. Idempotency: this calls `init()` once;
/// re-invoking it would panic on subscriber re-registration. The app
/// crate enforces single-call by making this private to `run()`.
pub fn init_logging() -> LogGuard {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
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

    #[cfg(not(debug_assertions))]
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
/// Android note: `cargo-apk`-launched processes have their cwd set to
/// the app's internal storage, so `./logs/` lands inside the
/// `/data/data/com.ampactorlabs.twotop/` sandbox — exactly where a
/// `.bmrg` companion file would live when match-recording lands.
#[cfg(not(debug_assertions))]
fn log_dir() -> std::path::PathBuf {
    use std::path::PathBuf;
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        return parent.join("logs");
    }
    PathBuf::from("./logs")
}
