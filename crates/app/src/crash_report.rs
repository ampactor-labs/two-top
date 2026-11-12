//! The crash a tester can hand back.
//!
//! `crash.log` has always been written on a panic, into the app's external
//! files dir. That is a directory no stock Files app on Android 11+ will
//! open, so in practice the log was reachable with a USB cable and `adb`
//! and nothing else. A friend whose phone closed the game could say that it
//! closed, and that was the whole report.
//!
//! So the app carries the report itself. At boot it looks for evidence that
//! the last run ended badly: a `crash.log` newer than the last one it
//! handed over, or (Android 11+) the system's own exit record saying crash,
//! native crash, ANR or failed start. The second matters as much as the
//! first: a crash inside a GPU driver and a hung UI thread never run a line
//! of Rust on the way out. If there is evidence, the title shows one line,
//! and tapping it opens the share sheet on a plain-text report: version,
//! device and GPU, the exit record, the panic, and the tail of the log. The
//! tester picks whoever they would have told anyway.
//!
//! The report is assembled on the phone and leaves it only through that
//! tap. It contains what the log does: the device model, the room that was
//! dialed, and the names of the two players.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use input_touch::WindowSize;

use crate::anchor::ScreenAnchor;
use crate::lifecycle::LastExit;
use crate::screen::AppScreen;

/// The panic log's file name, inside [`crate::paths::diagnostics_dir`].
pub const CRASH_LOG: &str = "crash.log";

/// Holds the token (below) of the newest crash already handed over.
const ACK_FILE: &str = "crash_ack";

/// How much of `crash.log` and of the running log a report carries. A
/// share sheet's text has to survive whatever app it is handed to.
const REPORT_CRASH_CHARS: usize = 3000;
const REPORT_LOG_LINES: usize = 60;
const REPORT_LOG_CHARS: usize = 6000;

/// The title strip the notice sits in (window fractions, y-down): under
/// the banner, above the far seat. No other title target reaches it.
const NOTICE_BAND: (f32, f32) = (0.16, 0.215);
const NOTICE_ANCHOR_Y: f32 = 1.0 - (NOTICE_BAND.0 + NOTICE_BAND.1);

/// Evidence that the last run ended badly, held until the player hands it
/// over. `None` everywhere a crash did not just happen.
#[derive(Resource, Default)]
pub struct CrashNotice(Option<Evidence>);

/// What was found at boot. The report text is built at the tap, when the
/// device line exists.
struct Evidence {
    /// Orders crashes in time: the newer of `crash.log`'s modification
    /// time and the exit record's timestamp, in milliseconds.
    token: u64,
    last_exit: Option<LastExit>,
    crash_log: Option<String>,
    log_tail: String,
}

/// The last `max` bytes of `text`, cut forward to a character boundary.
pub(crate) fn tail_chars(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

/// The last `lines` lines of `text`.
fn tail_lines(text: &str, lines: usize) -> &str {
    let mut seen = 0;
    for (index, byte) in text.trim_end().bytes().enumerate().rev() {
        if byte == b'\n' {
            seen += 1;
            if seen == lines {
                return &text[index + 1..];
            }
        }
    }
    text
}

/// The plain-text report a tester sends. Pure for tests.
pub fn build_report(
    version: &str,
    sim_version: u32,
    device: &str,
    last_exit: Option<&LastExit>,
    crash_log: Option<&str>,
    log_tail: &str,
) -> String {
    let mut out = format!("2-Top crash report\napp {version} (sim {sim_version})\n");
    if !device.is_empty() {
        out.push_str(&format!("device: {device}\n"));
    }
    if let Some(exit) = last_exit {
        out.push_str(&format!("last exit: {}\n", exit.describe()));
    }
    out.push_str("\n--- crash.log ---\n");
    match crash_log.map(str::trim).filter(|text| !text.is_empty()) {
        Some(text) => out.push_str(tail_chars(text, REPORT_CRASH_CHARS)),
        None => out.push_str("(no panic was recorded: the process died outside Rust)"),
    }
    out.push('\n');
    let log_tail = tail_chars(tail_lines(log_tail, REPORT_LOG_LINES), REPORT_LOG_CHARS).trim();
    if !log_tail.is_empty() {
        out.push_str("\n--- log tail ---\n");
        out.push_str(log_tail);
        out.push('\n');
    }
    out
}

/// Which token, if any, deserves a notice: the newest sign of a bad exit,
/// when it is newer than the last one acknowledged. Pure for tests.
fn pending_token(
    crash_log_ms: Option<u64>,
    abnormal_exit_ms: Option<u64>,
    acked: u64,
) -> Option<u64> {
    let newest = crash_log_ms.max(abnormal_exit_ms)?;
    (newest > acked).then_some(newest)
}

fn modified_ms(path: &Path) -> Option<u64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let since_epoch = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(since_epoch.as_millis() as u64)
}

/// The end of a file, without reading the rest of it: a day's log runs to
/// megabytes and this happens at boot.
fn read_tail(path: &Path, max_bytes: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(max_bytes)))
        .ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// The newest log file's tail. The appender stamps the date into the file
/// name, so the newest file is the last one by name.
fn newest_log_tail(logs: &Path) -> String {
    let newest = std::fs::read_dir(logs)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(crate::logging::LOG_FILE_PREFIX))
        })
        .max();
    newest
        .and_then(|path| read_tail(&path, 4 * REPORT_LOG_CHARS as u64))
        .unwrap_or_default()
}

fn ack_path() -> Option<PathBuf> {
    crate::paths::config_file(ACK_FILE)
}

fn acknowledged() -> u64 {
    ack_path()
        .and_then(|path| crate::paths::read_document(&path).ok())
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0)
}

fn acknowledge(token: u64) {
    let Some(path) = ack_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = crate::paths::write_atomic(&path, token.to_string().as_bytes()) {
        tracing::warn!(target: "two_top::crash", error = %e, "could not record the handed-over crash");
    }
}

/// `Startup`: look for a bad ending to the last run, and say in the log how
/// it ended either way. The lines above this one in the log file are the
/// last thing the previous run wrote.
fn collect(mut notice: ResMut<CrashNotice>) {
    let last_exit = crate::platform::last_exit();
    if let Some(exit) = &last_exit {
        tracing::info!(
            target: "two_top::crash",
            reason = exit.reason_name(),
            status = exit.status,
            description = %exit.description,
            "how the previous run ended",
        );
    }
    let dir = crate::paths::diagnostics_dir();
    let crash_path = dir.as_ref().map(|dir| dir.join(CRASH_LOG));
    let crash_log_ms = crash_path.as_deref().and_then(modified_ms);
    let abnormal_exit_ms = last_exit
        .as_ref()
        .filter(|exit| exit.is_abnormal())
        .map(|exit| exit.timestamp_ms.max(0) as u64);
    let Some(token) = pending_token(crash_log_ms, abnormal_exit_ms, acknowledged()) else {
        return;
    };
    tracing::warn!(target: "two_top::crash", "the previous run crashed; offering the report");
    notice.0 = Some(Evidence {
        token,
        last_exit,
        crash_log: crash_path.and_then(|path| std::fs::read_to_string(path).ok()),
        log_tail: dir
            .map(|dir| newest_log_tail(&dir.join("logs")))
            .unwrap_or_default(),
    });
}

#[derive(Component)]
struct NoticeText;

fn spawn_notice(mut commands: Commands) {
    commands.spawn((
        NoticeText,
        Text2d::new("LAST RUN CRASHED\nTAP HERE TO SEND THE REPORT"),
        TextFont {
            font_size: 26.0,
            ..default()
        },
        TextColor(render::palette::EMBER),
        TextLayout::new_with_justify(Justify::Center),
        ScreenAnchor::new(0.0, NOTICE_ANCHOR_Y, 0.0, 0.0),
        Transform::from_xyz(0.0, 0.0, 200.0),
        Visibility::Hidden,
    ));
}

/// The notice shows on the title for as long as there is a report nobody
/// has taken.
fn update_notice(
    screen: Res<State<AppScreen>>,
    notice: Res<CrashNotice>,
    mut text: Query<&mut Visibility, With<NoticeText>>,
) {
    let show = notice.0.is_some() && *screen.get() == AppScreen::Title;
    let want = if show {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
    for mut visibility in &mut text {
        if *visibility != want {
            *visibility = want;
        }
    }
}

/// A tap on the notice hands the report to the share sheet and retires it.
/// Retired on the tap, not on a confirmed send: the sheet does not report
/// back, and a notice that reappears after being dealt with would be worse
/// than one report lost to a cancelled share (`crash.log` stays on disk).
fn notice_input(touches: Res<Touches>, window: Res<WindowSize>, mut notice: ResMut<CrashNotice>) {
    let win = window.0;
    if notice.0.is_none() || win.y <= 0.0 {
        return;
    }
    let tapped = touches.iter_just_pressed().any(|touch| {
        let y = touch.position().y / win.y;
        y >= NOTICE_BAND.0 && y < NOTICE_BAND.1
    });
    if !tapped {
        return;
    }
    let Some(evidence) = notice.0.take() else {
        return;
    };
    let report = build_report(
        env!("CARGO_PKG_VERSION"),
        sim::SIM_VERSION,
        crate::lifecycle::device_summary(),
        evidence.last_exit.as_ref(),
        evidence.crash_log.as_deref(),
        &evidence.log_tail,
    );
    crate::platform::share_text("2-Top crash report", &report);
    acknowledge(evidence.token);
}

/// A synthetic notice for laying the banner out without crashing a phone:
/// `TWOTOP_CRASH_NOTICE=1` on the desktop build.
fn preview(mut notice: ResMut<CrashNotice>) {
    if std::env::var("TWOTOP_CRASH_NOTICE").is_ok_and(|v| v == "1") {
        notice.0 = Some(Evidence {
            token: u64::MAX,
            last_exit: None,
            crash_log: Some("(preview)".into()),
            log_tail: String::new(),
        });
    }
}

pub struct CrashReportPlugin;

impl Plugin for CrashReportPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CrashNotice>()
            .add_systems(Startup, spawn_notice)
            .add_systems(
                Update,
                (
                    notice_input.run_if(in_state(AppScreen::Title)),
                    update_notice,
                )
                    .chain(),
            );
        // Collected wherever there is a share sheet to hand the report to;
        // elsewhere a notice would offer a tap that goes nowhere.
        app.add_systems(
            Startup,
            (
                collect
                    .run_if(|profile: Res<crate::platform::DeviceProfile>| profile.has_share_sheet),
                preview,
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tails_cut_on_character_boundaries() {
        assert_eq!(tail_chars("abcdef", 3), "def");
        assert_eq!(tail_chars("abc", 10), "abc");
        // 'é' is two bytes: a cut landing inside it moves forward.
        assert_eq!(tail_chars("aéb", 2), "b");
        assert_eq!(tail_lines("a\nb\nc\n", 2), "b\nc\n");
        assert_eq!(tail_lines("a\nb", 5), "a\nb");
    }

    #[test]
    fn a_notice_needs_evidence_newer_than_the_last_one_handed_over() {
        assert_eq!(pending_token(None, None, 0), None);
        assert_eq!(pending_token(Some(10), None, 0), Some(10));
        assert_eq!(pending_token(None, Some(20), 0), Some(20));
        // An abort leaves both; one notice, keyed on the later.
        assert_eq!(pending_token(Some(10), Some(12), 0), Some(12));
        // Already handed over: nothing to show.
        assert_eq!(pending_token(Some(10), Some(12), 12), None);
        // A new crash after that one.
        assert_eq!(pending_token(Some(30), Some(12), 12), Some(30));
    }

    #[test]
    fn the_report_carries_the_device_the_exit_and_the_panic() {
        let exit = LastExit {
            reason: 5,
            status: 6,
            timestamp_ms: 1,
            description: "Abort message: boom".into(),
        };
        let report = build_report(
            "0.15.4",
            15,
            "Google Pixel 7, Android 15 (sdk 35), Mali-G710",
            Some(&exit),
            Some("=== panic ===\n  at lib.rs:1:1\n  boom\n"),
            "line one\nline two\n",
        );
        assert!(report.starts_with("2-Top crash report\napp 0.15.4 (sim 15)\n"));
        assert!(report.contains("device: Google Pixel 7"));
        assert!(report.contains("last exit: CRASH_NATIVE (status 6): Abort message: boom"));
        assert!(report.contains("--- crash.log ---\n=== panic ==="));
        assert!(report.contains("--- log tail ---\nline one\nline two\n"));
    }

    #[test]
    fn a_death_outside_rust_says_so_instead_of_showing_nothing() {
        let report = build_report("0.15.4", 15, "", None, None, "");
        assert!(report.contains("the process died outside Rust"));
        assert!(!report.contains("device:"));
        assert!(!report.contains("--- log tail ---"));
    }

    #[test]
    fn the_report_stays_small_enough_to_share() {
        let crash = "c".repeat(50_000);
        let log = "a line of log\n".repeat(5_000);
        let report = build_report("0.15.4", 15, "device", None, Some(&crash), &log);
        assert!(report.len() < REPORT_CRASH_CHARS + REPORT_LOG_CHARS + 400);
    }
}
