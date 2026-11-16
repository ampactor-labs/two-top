//! Every way to configure a build from outside it, in one place.
//!
//! * [`Knob`]: the runtime `TWOTOP_*` environment variables a developer,
//!   a smoke script or CI sets (autostart, autopilot, capture, faults...).
//!   None of them exists for a player; most are read once at boot.
//! * [`Endpoint`]: the servers a build talks to (room, relay credentials,
//!   tape drop, watch page). Each is the runtime variable when set and
//!   non-empty, else the value baked in at compile time (an APK launched
//!   from its icon has no environment to read).
//!
//! The table below is the documentation: `docs` and `--help`-style output
//! read [`Knob::ALL`], and a test fails if any `TWOTOP_` name appears in
//! the app's source outside this file, so a new knob cannot be added
//! without its line here.

/// A runtime development knob.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Knob {
    Autostart,
    Practice,
    Autopilot,
    Arena,
    AutoplayTape,
    Capture,
    CaptureFrames,
    CaptureOn,
    CrashNotice,
    Fault,
    FirstBlood,
    LeanGpu,
    LessonStage,
    ShowTouch,
    Window,
    StateLog,
    Tuning,
    TuningDump,
}

impl Knob {
    pub const ALL: [Knob; 18] = [
        Knob::Autostart,
        Knob::Practice,
        Knob::Autopilot,
        Knob::Arena,
        Knob::AutoplayTape,
        Knob::Capture,
        Knob::CaptureFrames,
        Knob::CaptureOn,
        Knob::CrashNotice,
        Knob::Fault,
        Knob::FirstBlood,
        Knob::LeanGpu,
        Knob::LessonStage,
        Knob::ShowTouch,
        Knob::Window,
        Knob::StateLog,
        Knob::Tuning,
        Knob::TuningDump,
    ];

    /// The environment variable, and what it does.
    pub const fn spec(self) -> (&'static str, &'static str) {
        match self {
            Knob::Autostart => (
                "TWOTOP_AUTOSTART",
                "skip the Title: straight into a match (any value; 1 where noted)",
            ),
            Knob::Practice => ("TWOTOP_PRACTICE", "1: boot into practice against the bot"),
            Knob::Autopilot => (
                "TWOTOP_AUTOPILOT",
                "<tier>: the bot brain plays this device's side online",
            ),
            Knob::Arena => (
                "TWOTOP_ARENA",
                "<tag>: the table to play (overrides the persisted pick)",
            ),
            Knob::AutoplayTape => (
                "TWOTOP_AUTOPLAY_TAPE",
                "1: open the first listed tape in the theater at boot",
            ),
            Knob::Capture => (
                "TWOTOP_CAPTURE",
                "<path.png>: screenshot after a settle, then exit",
            ),
            Knob::CaptureFrames => (
                "TWOTOP_CAPTURE_FRAMES",
                "<n>: render frames before the capture",
            ),
            Knob::CaptureOn => (
                "TWOTOP_CAPTURE_ON",
                "matchover: start the capture countdown at MatchOver",
            ),
            Knob::CrashNotice => (
                "TWOTOP_CRASH_NOTICE",
                "1: show the crash notice as if the last run crashed",
            ),
            Knob::Fault => (
                "TWOTOP_FAULT",
                "desync@<tick>: debug builds only, a one-sided desync",
            ),
            Knob::FirstBlood => (
                "TWOTOP_FIRST_BLOOD",
                "1: run the newcomer lesson as if never seen",
            ),
            Knob::LeanGpu => ("TWOTOP_LEAN_GPU", "1: the lean render path weak phones get"),
            Knob::LessonStage => (
                "TWOTOP_LESSON_STAGE",
                "<n>: start the newcomer lesson at stage n",
            ),
            Knob::ShowTouch => ("TWOTOP_SHOW_TOUCH", "1: draw the touch zones on desktop"),
            Knob::Window => ("TWOTOP_WINDOW", "<w>x<h>: the desktop window size"),
            Knob::StateLog => (
                "TWOTOP_STATE_LOG",
                "<path>: log every frame's sim state (desync hunting)",
            ),
            Knob::Tuning => (
                "TWOTOP_TUNING",
                "<path>: live-reload presentation tuning from this file",
            ),
            Knob::TuningDump => (
                "TWOTOP_TUNING_DUMP",
                "<path>: write the current tuning out and exit",
            ),
        }
    }

    pub const fn name(self) -> &'static str {
        self.spec().0
    }

    /// Its value, if set (any value, empty included).
    pub fn value(self) -> Option<String> {
        std::env::var(self.name()).ok()
    }

    /// Set at all (any value, empty included).
    pub fn is_set(self) -> bool {
        std::env::var_os(self.name()).is_some()
    }

    /// Set to exactly `1`.
    pub fn is_on(self) -> bool {
        self.value().as_deref() == Some("1")
    }

    /// Its value parsed, if set and well-formed.
    pub fn parse<T: std::str::FromStr>(self) -> Option<T> {
        self.value().and_then(|v| v.trim().parse().ok())
    }
}

/// A server a build talks to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endpoint {
    /// The matchbox room (`ws://host/two-top?next=2`); none: couch build.
    Room,
    /// The TURN credential vendor (`crates/ice_vendor`) and its app key.
    IceUrl,
    IceKey,
    /// A static TURN server (when there is no vendor).
    TurnUrl,
    TurnUser,
    TurnPass,
    /// The tape drop relay and the watch page that plays its tapes.
    DropUrl,
    WatchUrl,
}

impl Endpoint {
    pub const ALL: [Endpoint; 8] = [
        Endpoint::Room,
        Endpoint::IceUrl,
        Endpoint::IceKey,
        Endpoint::TurnUrl,
        Endpoint::TurnUser,
        Endpoint::TurnPass,
        Endpoint::DropUrl,
        Endpoint::WatchUrl,
    ];

    /// The runtime variable, and the value baked at compile time.
    pub fn spec(self) -> (&'static str, Option<&'static str>) {
        match self {
            // The room's runtime name predates the TWOTOP_ prefix.
            Endpoint::Room => ("MATCHBOX_ROOM", option_env!("TWOTOP_ROOM")),
            Endpoint::IceUrl => ("TWOTOP_ICE_URL", option_env!("TWOTOP_ICE_URL")),
            Endpoint::IceKey => ("TWOTOP_ICE_KEY", option_env!("TWOTOP_ICE_KEY")),
            Endpoint::TurnUrl => ("TWOTOP_TURN_URL", option_env!("TWOTOP_TURN_URL")),
            Endpoint::TurnUser => ("TWOTOP_TURN_USER", option_env!("TWOTOP_TURN_USER")),
            Endpoint::TurnPass => ("TWOTOP_TURN_PASS", option_env!("TWOTOP_TURN_PASS")),
            Endpoint::DropUrl => ("TWOTOP_DROP_URL", option_env!("TWOTOP_DROP_URL")),
            Endpoint::WatchUrl => ("TWOTOP_WATCH_URL", option_env!("TWOTOP_WATCH_URL")),
        }
    }

    /// The runtime value when set and non-empty, else the baked one when
    /// non-empty. A stray `TWOTOP_ROOM=` can never force a blank URL.
    pub fn get(self) -> Option<String> {
        let (name, baked) = self.spec();
        resolve(std::env::var(name).ok(), baked)
    }
}

/// Runtime over baked, empty meaning unset. Pure.
pub fn resolve(runtime: Option<String>, baked: Option<&str>) -> Option<String> {
    runtime
        .filter(|s| !s.is_empty())
        .or_else(|| baked.filter(|s| !s.is_empty()).map(str::to_string))
}

/// One boot line: the knobs set (with their values: none is a secret) and
/// which endpoints are configured (presence only: relay credentials are).
pub fn log_configuration() {
    let knobs: Vec<String> = Knob::ALL
        .iter()
        .filter_map(|k| k.value().map(|v| format!("{}={v}", k.name())))
        .collect();
    let endpoints: Vec<&str> = Endpoint::ALL
        .iter()
        .filter(|e| e.get().is_some())
        .map(|e| e.spec().0)
        .collect();
    tracing::info!(
        target: "two_top::config",
        knobs = %knobs.join(" "),
        endpoints = %endpoints.join(" "),
        "configuration",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_beats_baked_and_empty_is_unset() {
        assert_eq!(resolve(Some("a".into()), Some("b")).as_deref(), Some("a"));
        assert_eq!(
            resolve(Some(String::new()), Some("b")).as_deref(),
            Some("b")
        );
        assert_eq!(resolve(None, Some("")), None);
        assert_eq!(resolve(None, None), None);
    }

    #[test]
    fn every_knob_is_named_once_and_documented() {
        let mut names = std::collections::BTreeSet::new();
        for knob in Knob::ALL {
            let (name, doc) = knob.spec();
            assert!(name.starts_with("TWOTOP_"), "{name}");
            assert!(!doc.is_empty(), "{name} has no doc line");
            assert!(names.insert(name), "{name} twice");
        }
        for endpoint in Endpoint::ALL {
            assert!(
                names.insert(endpoint.spec().0),
                "{:?} reuses a knob's name",
                endpoint
            );
        }
    }

    /// Every `TWOTOP_` name the app reads goes through this file, so the
    /// table above is the whole list.
    #[test]
    fn no_knob_is_read_anywhere_else() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stray = Vec::new();
        let mut stack = vec![src];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") || path.ends_with("config.rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                for (i, line) in text.lines().enumerate() {
                    let code = line.split("//").next().unwrap_or("");
                    if code.contains("env::var")
                        && (code.contains("\"TWOTOP_") || code.contains("\"MATCHBOX_ROOM\""))
                        || code.contains("option_env!(\"TWOTOP_")
                    {
                        stray.push(format!("{}:{}: {}", path.display(), i + 1, line.trim()));
                    }
                }
            }
        }
        assert!(
            stray.is_empty(),
            "read through config::Knob / config::Endpoint:\n{}",
            stray.join("\n")
        );
    }
}
