# Persona 6 — Priya, whose phone has other ideas
### Lifecycle & crash-safety audit of 2-Top — READ-ONLY, code-traced

> My sister calls in round three, I'm one pip from the set. Decline screen,
> four seconds, back in. The table says **MATCH ABANDONED / you left the duel**
> and my record went 4-2 → 4-3. I didn't leave. My phone left.
>
> Later the screen dimmed while I was waiting for a challenger and the summons
> just stopped. When battery saver made everything stutter for a second, the
> *other* guy walked off and the game still put the loss on me.
>
> I went and read it. The code does not know my phone exists outside this app.

---

## Verification of the starting facts

**Fact 1 holds, and it is broader than stated.** Across the entire repo
(source, manifests, docs; `target/` excluded):

```
$ grep -rn "AppLifecycle|WillSuspend|WillResume|on_pause|onPause|onResume|onNewIntent|onSaveInstanceState" \
    --include=*.rs --include=*.toml --include=*.xml --include=*.md . | grep -v ^./target
(no output)

$ grep -rn "bevy::window::" crates/ --include=*.rs
crates/app/src/lib.rs:350:    use bevy::window::{MonitorSelection, WindowMode};
crates/app/src/netplay.rs:798:    mut focus_events: MessageReader<bevy::window::WindowFocused>,
```

One consumer of one window event in 13 crates — and it is a blame heuristic
(F4), not lifecycle handling. Nothing suspends, pauses, saves or resumes.
No `WinitSettings`, no `Time<Virtual>` tuning, no surface-loss path.

**Fact 2 holds, and better than expected.** I hunted for bypasses of
`paths::write_atomic` and found **none**. Every persisted file — `settings.json`
(`settings.rs:117`), `profile.json` (`profile.rs:329`), `career.json`
(`grudge.rs:249`), `room_code` (`room_code.rs:241`), `*.bmrg` tapes
(`recorder.rs:272`), `*.attest.json` (`attest.rs:232`), `crash.log`
(`logging.rs:108`) — routes through it. The one direct `fs::rename` at
`profile.rs:315` is the corrupt-file *quarantine*, not a write, and is correct.
**That part of the system is done right.** The damage is elsewhere.

---

## Findings

| # | Finding | Sev |
|---|---|---|
| 1 | A peer's malformed input packet panics the process inside ggrs | 🔴 |
| 2 | No lifecycle handling + 9 s hard disconnect + no reconnect = a phone call is a lost match | 🔴 |
| 3 | Activity recreation re-enters `android_main` → `init_logging()` panics | 🔴 |
| 4 | Forfeit blame is a hair trigger: a 2 s hitch or a notification shade convicts you | 🟠 |
| 5 | Process death → the two ledgers permanently disagree about the same match | 🟠 |
| 6 | No keep-screen-on / `WAKE_LOCK`: the screen timeout kills the summons and the theater | 🟠 |
| 7 | Version skew undefended at four layers at once | 🟠 |
| 8 | `debuggable = true` ships in the release APK — the signing key is `adb`-readable | 🟠 |
| 9 | The deep link is read once, at PostStartup, under `launch_mode = singleTask` | 🟠 |
| 10 | Storage hygiene: `.tmp` corpse on ENOSPC, no fsync, no pruning, no log rotation | 🟡 |

---

### 1. 🔴 A stranger's packet kills the app — and the repo already knows

> **Correction (2026-09-17, on fixing):** the reachable panic is not the
> `assert!` — a 2-Top peer speaks for one handle, so `len % 1` never fires.
> It is the delta decoder trusting a remote-written two-byte length prefix
> per frame (`compression.rs`), plus three hazards underneath in
> `bitfield_rle`/`varinteger` (an unchecked read past the buffer, an
> attacker-sized allocation, a wrapping shift). All closed by the guard in
> `crates/net`. One upstream `assert!` on `start_frame` remains open — see
> `ROLEPLAY_AUDIT.md` § Corrections.

`crates/net/src/lib.rs:116-127` closes half of a hole and documents the other half:

> "…the matchbox reference impl panics on them, but this app has a better
> answer than aborting mid-duel — drop the packet… **(ggrs itself still
> `expect`s on per-player input bytes inside a well-formed `Message` — an
> upstream issue; this closes the cheap half.)**"

The upstream half is live, in `ggrs-0.12.0/src/network/protocol.rs:98-110`:

```rust
fn to_player_inputs<T: Config>(&self, num_players: usize) -> Vec<PlayerInput<T::Input>> {
    assert!(self.bytes.len().is_multiple_of(num_players));      // :99  -> panic
    let size = self.bytes.len() / num_players;
    ...
    let input: T::Input =
        bincode::deserialize(player_byte_slice).expect("input deserialization failed"); // :107 -> panic
```

`decode_packet` (`net/lib.rs:127-142`) validates only that the *envelope* is
valid bincode. `MessageBody::Input.bytes` is a `Vec<u8>` — any length decodes.
`sim::PlayerInput` is 4 bytes (`sim/lib.rs:22-27`), so the expected payload is
8. A peer sending 5 bytes trips the `assert!`; 6 bytes trips the `expect`.
Either is an unwinding panic on the main thread → process death, no dialog.

Reachability is the worst case: the APK **"pairs on the public quick-match room
by default"** (`.github/workflows/apk.yml:148`), so the sender is any stranger,
or simply any peer running a build whose `PlayerInput` layout differs.

`MessageBody` is `pub(crate)` in ggrs (`messages.rs:122-141`), so the app
**cannot** filter this after decode.

**Fix:** validate before ggrs sees it. `decode_packet` already owns the raw
bytes — decode into a local mirror of the ggrs wire format, drop any `Input`
whose `bytes.len() != size_of::<PlayerInput>() * num_players`, then hand the
survivors on. Failing that, wrap the session advance in `catch_unwind`.
The existing `Option`-returning shape of `decode_packet` is the right seam.

---

### 2. 🔴 A phone call is an unrecoverable forfeit — nine seconds, no way back

Chain, all proven:

- `crates/app/src/netplay.rs:65` — `const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(9);`
- `crates/app/src/netplay.rs:636-638` — `GgrsEvent::Disconnected { addr } => { tracing::error!(… "peer disconnected — forfeiting match"); forfeited_peer = Some(addr); }`
- `crates/app/src/netplay.rs:673-678` — `LobbyState::Forfeited { .. }` + `*world.resource_mut::<sim::MatchState>() = sim::MatchState::MatchOver;`
- `crates/net/src/lib.rs:603` — `Forfeited { peer_id }` is terminal (`lib.rs:1232-1237` asserts it never leaves).

Nothing pauses the session when the app goes away, because nothing *knows* it
went away. The only wall-clock timer in the loop is ggrs's own 9 s. A call
screen, a Maps navigation prompt, the camera app for a QR scan — any of these
past 9 s and the match is over with no reconnect path in the codebase.

Note the interaction with the frame-based fallback: `net::FORFEIT_AFTER_FRAMES
= 600` (`net/lib.rs:725`) is counted in *sim frames* against
`sim::FrameCount`. When the session stalls, both peers' frame counters stall
too, so the "10 s grace that lets a phone survive a notification-shade peek or
a short call screen" (`net/lib.rs:726-728`) **never runs during a real
suspend**. The comment describes a grace that in practice only ggrs's 9 s
wall-clock timer can reach, and it fires first. The documented mitigation is
inert exactly when it's needed.

**Fix:** handle `AppLifecycle::{WillSuspend, Suspended, WillResume, Running}`.
On `WillSuspend` send a `NetMsg` "pausing" on the reliable channel; on resume,
re-sync. Minimally: raise `DISCONNECT_TIMEOUT` toward the 30 s that matches the
documented intent, and surface a RECONNECTING state instead of a terminal one.

---

### 3. 🔴 Sunset, dark mode, activity recreated, app dead

`crates/app/Cargo.toml:177-179`:

```toml
orientation = "portrait"
config_changes = "orientation|keyboardHidden|screenSize"
launch_mode = "singleTask"
```

Rotation is genuinely safe — `screenOrientation=portrait` means it never
happens. What is **missing** from `configChanges` is the problem:
`uiMode`, `density`, `smallestScreenSize`, `screenLayout`, `fontScale`,
`layoutDirection`, `locale`, `navigation`, `touchscreen`, `mcc`, `mnc`.

Any of those changing destroys and recreates the activity **without killing the
process**. The reachable ones on a real phone: Android's scheduled dark theme
flipping at sunset (`uiMode`), battery-saver enabling dark theme on several
OEM skins (`uiMode`), the user changing Display Size or font scale
(`density`/`fontScale`), entering split-screen or unfolding a foldable
(`smallestScreenSize`/`screenLayout` — `resizeableActivity` is undeclared and
defaults to true at `target_sdk_version = 34`, `Cargo.toml:153`).

Then this, which the codebase documents against itself:

- `crates/app/src/lib.rs:424-427` — `#[bevy_main] fn main() { run(); }`, i.e. the generated `android_main`
- `crates/app/src/lib.rs:88-93` — `pub fn run() { … let _log_guard = logging::init_logging(); … }`
- `crates/app/src/logging.rs:79-81` — *"Build the global subscriber. Idempotency: this calls `init()` once; **re-invoking it would panic on subscriber re-registration.** The app crate enforces single-call by making this private to `run()`."*
- `crates/app/src/logging.rs:140,177,226` — `.init()`

`run()` **is** the Android entry point, so "single-call by making it private to
`run()`" is only true for one activity instance per process. `android_main` is
invoked per `ANativeActivity_onCreate` (`SIDELOAD.md:55` confirms the
native-activity backend), so a config-change recreation calls it a second time
in the same process and the documented panic fires. Even if the subscriber
survived, a second winit event loop in one process is its own abort.

*Confidence:* the manifest gap, the panic-on-reinit and the entry-point
identity are all quoted above. The one inferential step is that
`ANativeActivity_onCreate` runs twice in-process on recreation — standard
NativeActivity behaviour, but it is the piece I could not execute.

**Fix:** `config_changes = "orientation|keyboardHidden|keyboard|screenSize|smallestScreenSize|screenLayout|density|uiMode|fontScale|layoutDirection|locale|navigation|touchscreen|mcc|mnc"`,
and make `init_logging` idempotent (`try_init()` + an `AtomicBool` guard)
so a recreation degrades to logcat-only instead of dying.

---

### 4. 🟠 Convicted by a stutter: the forfeit-blame heuristic is a hair trigger

`crates/app/src/netplay.rs:793-809`:

```rust
const ABSENCE_FREEZE_SECS: f32 = 2.0;

pub fn track_absence(time: Res<Time<Real>>, mut focus_events: MessageReader<WindowFocused>, mut absence: ResMut<RecentAbsence>) {
    let now = time.elapsed_secs();
    if time.delta_secs() > ABSENCE_FREEZE_SECS { absence.0 = Some(now); }
    for ev in focus_events.read() { if !ev.focused { absence.0 = Some(now); } }
}
```

That stamp then decides the record, for a 20 s window
(`RecentAbsence::FORFEIT_BLAME_SECS = 20.0`, `netplay.rs:784`):

```rust
// crates/app/src/grudge.rs:258-267
pub fn match_won(our_score: u8, their_score: u8, forfeited: bool, we_went_absent: bool) -> bool {
    if our_score >= MATCH_WIN_THRESHOLD { return true; }
    if their_score >= MATCH_WIN_THRESHOLD { return false; }
    forfeited && !we_went_absent
}
```

Two false positives, both ordinary on a phone:

1. **`WindowFocused(false)` fires for the notification shade.** Pulling it
   down, a heads-up call banner, or any system dialog over the app stamps an
   absence. Priya peeks at a notification; the *opponent* then walks off; her
   phone hands her the loss.
2. **A 2 s hitch is not an absence.** Thermal throttling, battery saver, a GC
   pause, an asset load — `crates/app/src/lib.rs:959` itself contemplates "a
   phone at 10 fps". At 10 fps a single 2.1 s stall stamps absence.

The stamp is also **never cleared**. `grep -n "absence.0\s*=" crates/app/src/*.rs`
returns only `netplay.rs:803` and `:807` — nothing resets it on match entry,
and `leave_online_match` (`netplay.rs:878-919`) resets eleven other resources
but not this one. So an absence taken on the Title screen can still convict a
match decided 19 s later.

The consequence is not cosmetic. It writes `record.losses += 1` and
`rival.losses += 1` (`grudge.rs:307-320`), flips the streak, and puts
`"MATCH ABANDONED\nyou left the duel"` on screen (`lobby_overlay.rs:136-139`,
`screen.rs:1398-1403`).

**Fix:** require corroboration before blaming the local phone — a freeze
`>= DISCONNECT_TIMEOUT` (not 2 s), and ignore `WindowFocused(false)` entirely
in favour of a real `AppLifecycle::Suspended` once F2's handler exists. Clear
`RecentAbsence` on `OnEnter(InMatch)` and in `leave_online_match`.

---

### 5. 🟠 Android kills the app; the two ledgers disagree forever

`record_abandoned_loss` exists and is honest, but only for one path
(`crates/app/src/grudge.rs:189-201`):

> "Called by the in-match QUIT path right before the socket teardown
> (`record_match_result` can't cover it: the quitter leaves the screen before
> any `MatchOver` tick happens on their side)."

`record_match_result` requires `MatchState::MatchOver` to be *observed*
(`grudge.rs:287-292`). If Android kills the cached process — Priya swipes it
from recents, or the low-memory killer takes it — neither runs. Her phone
records nothing; the opponent's phone hits ggrs's 9 s, forfeits, and records a
win against her install-id. Her ledger says the match never happened; his says
he beat her. The rivalry home (RIVALS) renders both as truth.

There is no hook to fix this, because there is no lifecycle handling (F2) —
`onSaveInstanceState`/`WillSuspend` is exactly where the provisional loss would
be written.

**Fix:** once F2 lands, write a provisional "match in progress" marker
(atomically, the helper is already there) on `WillSuspend`, and reconcile it on
next launch: marker present and no result recorded → `record_abandoned_loss`.

---

### 6. 🟠 The screen times out and the summons dies

The full permission list is two entries (`crates/app/Cargo.toml:155-162`):
`INTERNET` and `VIBRATE`. No `WAKE_LOCK`. And:

```
$ grep -rniE "keep_screen_on|KEEP_SCREEN_ON|WakeLock" crates/ --include=*.rs
(no output)
```

During a match, touch input keeps resetting the system screen timeout, so play
itself is fine. The exposure is everywhere else the app expects you to *watch*:

- `LobbyState::WaitingForPeer` — the **AWAITING A CHALLENGER** overlay
  (`screen.rs:1375-1382`) can sit for minutes with zero touches. Screen off →
  suspend → the socket stops being pumped → the summons is gone.
- The **theater** (`theater.rs`) plays tapes with no input at all; a 30 s round
  plus scrubbing outlasts a 30 s display timeout.

**Fix:** one JNI hop — the pattern already exists in `haptics.rs:55-91` and
`room_code.rs:159-182`. Set `FLAG_KEEP_SCREEN_ON` on the window when the screen
is InMatch, lobby-waiting or theater, and clear it on the Title.

---

### 7. 🟠 Old APK over new: four undefended layers at once

**(a) Nothing stops the downgrade.** No `version_code` or `version_name`
anywhere: `grep -n "version_code\|versionCode" crates/app/Cargo.toml
SIDELOAD.md PLAYBOOK.md scripts/*.sh .github/workflows/apk.yml` → no hits. The
APK ships from a *rolling* `apk-latest` tag (`.github/workflows/apk.yml:141-149`,
`gh release delete apk-latest --yes --cleanup-tag`), so every build carries
cargo-apk's default versionCode. Android's downgrade protection is keyed on
versionCode, so it never engages: an older APK installs as a same-version
reinstall and keeps `/data/data/com.ampactorlabs.twotop` intact.

**(b) No version field on any persisted file.** `Settings` (`settings.rs:31-48`),
`LocalProfile` (`profile.rs:107-127`), `CareerRecord`/`RivalRecord`
(`grudge.rs:31-49,117-120`) all carry `#[serde(default)]` — good backward
compat (missing fields default), **zero forward compat**. `serde_json` silently
ignores unknown fields, and the structs have no catch-all, so an old build
*reads* a new file fine and then **writes it back with the new fields gone**.

The casualty that matters: `LocalProfile::signing_key` (`profile.rs:119-126`) —
*"the result-signing key (NORTH N2)… minted beside the install-id"*. A
downgrade save drops it; re-upgrading mints a fresh one (`ensure_signing_key`,
`profile.rs:162-170`). Every `.attest.json` sidecar she already holds now names
a pubkey she cannot reproduce, and every peer's `attested_wins` against her is
orphaned. Same mechanism zeroes `attested_wins`, `streak`, `last_met_unix` and
the `tapes` ring in `career.json`.

**(c) No SIM_VERSION in the handshake or the room name.** The room name carries
the dial code and the arena tag only (`room_code.rs:80-90,108-119`). `NetMsg`
(`net/lib.rs:210-230`) carries `Profile`, `RematchWant`, `Bye`, `Profile2`,
`MatchSig` — no build/sim version. Two peers on different `SIM_VERSION`s pair
happily and diverge, and the only response is a log line:

```rust
// crates/app/src/netplay.rs:641-655
GgrsEvent::DesyncDetected { frame, local_checksum, remote_checksum, addr } => {
    tracing::error!(…, "DESYNC DETECTED — local and remote state diverged");
}
```

No UI, no abort, no match invalidation. Two people play two different games to
two different conclusions and each records a result.

**(d) The one layer that *is* defended:** tapes.
`decode_for_sim_version` refuses mismatches and `theater.rs:245` surfaces
`foreign_version` in the list instead of failing. `ArenaId::from_wire`
(`sim/lib.rs:3640-3649`) falls back to `Anchor` for unknown ids rather than
panicking, and `Settings::clamped` (`settings.rs:72-84`) scrubs NaN and
out-of-range floats. That is the standard the JSON files should be held to.

**Fix:** set an explicit monotonic `version_code`; add `schema_version: u32` to
each persisted struct and refuse to *save over* a file whose version is newer
than the build's; put `SIM_VERSION` in the room-name tag so mismatched builds
simply never pair; and make `DesyncDetected` end the match visibly.

---

### 8. 🟠 The release APK is debuggable, so the signing key is `adb`-readable

`crates/app/Cargo.toml:170`:

```toml
debuggable = true
```

This is a manifest flag, independent of the cargo profile, and
`.github/workflows/apk.yml:115` builds `--release` with it in place, then
publishes to a public GitHub release (`:141-149`). A debuggable APK means
`adb shell run-as com.ampactorlabs.twotop` reads and writes the private data
dir without root — including `profile.json`, i.e. the ed25519 seed that the
whole dual-signed-results pillar rests on (`profile.rs:119-126`). Anyone can
extract their own key, or edit `career.json`, or drop in someone else's
identity. For Priya's chaos remit it is also a data-integrity surface: any
"phone cleaner" style app with debug access can corrupt state the app trusts.

**Fix:** `debuggable = true` behind `cfg(debug_assertions)` / a separate dev
manifest; keep the published APK non-debuggable. (`SIDELOAD.md:166` already
flags release signing as unfinished — same bucket.)

---

### 9. 🟠 The sit-down ritual only works on a cold boot

`crates/app/src/room_code.rs:157-182` reads the join URI with one JNI hop:

```rust
let intent = env.call_method(&activity, "getIntent", "()Landroid/content/Intent;", &[])
```

and `:553` schedules it as `app.add_systems(PostStartup, apply_launch_join);`
— the doc comment at `:186` says so plainly: *"Runs once in PostStartup."*

The manifest sets `launch_mode = "singleTask"` (`Cargo.toml:179`). Under
singleTask, a `twotop://join/<CODE>-<arena>` tap while the process is already
alive brings the **existing** task forward and delivers the intent via
`onNewIntent` — which (a) nothing handles, and (b) does not change what
`getIntent()` returns unless `setIntent()` is called. And `PostStartup` does not
run again. So the link silently no-ops on every launch except a genuine cold
start, which is the *less* common case — the app is usually still cached.

Priya taps her friend's link, 2-Top comes to the front on whatever screen it
was, nothing is dialled, and the zero-typing promise becomes "type the code".
Minor extra: unlike `haptics.rs:91`, `launch_uri` never calls
`exception_clear()`, so a throwing JNI call leaves a pending exception on the
thread.

**Fix:** re-read the launch URI on resume (once F2's lifecycle handler exists),
or add a JNI `onNewIntent`/`setIntent` bridge, and move the read out of
`PostStartup` into a system that runs whenever the app returns to the Title.

---

### 10. 🟡 Storage hygiene on a 98%-full phone

**The good news first:** an ENOSPC write never breaks the match. Every writer
degrades: `recorder.rs:284-290` warns and returns `None`; `grudge.rs:249-251`,
`settings.rs:117-119`, `profile.rs:329-331`, `attest.rs:232-234` all warn and
continue. `attest.rs:200-208` even guards against pairing this match's
signatures with the previous tape when the save fails. Round flow is untouched.
Truncated JSON never panics: `settings.rs:103-107` and `grudge.rs:234-240` use
`.ok().and_then(...).unwrap_or_default()`; `profile.rs:297-317` quarantines the
bad file as `.corrupt` and remints. `replay::decode` is `Result`-based
(`replay/src/lib.rs:85-91`) with no length-driven pre-allocation, and
`theater.rs:246-253` indexes defensively (`[w as usize % 2]`,
`h.frame_rate.max(1)`).

Three gaps remain:

**(a) `write_atomic` leaks a partial temp file on ENOSPC.** `paths.rs:71-75`:

```rust
std::fs::write(&tmp, bytes)?;                 // <- early return, NO cleanup
std::fs::rename(&tmp, path).inspect_err(|_| {
    let _ = std::fs::remove_file(&tmp);        // <- cleanup only on rename failure
})
```

A full-disk failure is a *write* failure, so it takes the `?` and leaves
`settings.json.tmp` / `career.json.tmp` / `match_….bmrg.tmp` on a disk that had
no room for it. (Harmless on the next successful save — `paths.rs:114-122`
tests exactly that — but it is the wrong branch to skip cleanup on.)

**(b) No `fsync` before the rename.** `write_atomic`'s doc (`paths.rs:56-63`)
claims durability against "a process kill mid-write", which the rename does
give. It does not give durability against power loss or an f2fs/ext4 writeback
window, because neither the temp file nor the directory is synced. The file the
comment is most worried about — `profile.json`, whose loss "would mint a fresh
install-id and orphan the rivalry ledger on both phones" — is precisely the one
worth an `f.sync_all()` before rename plus a directory sync after.

**(c) Nothing prunes.** Tapes accumulate forever — `grudge.rs:51-53` is explicit
that the 4-deep rival ring is a *ledger* cap and *"older ones stay on disk for
the REPLAYS screen"*; `recorder.rs:284-290` never deletes; the only cap is
`theater.rs:275`'s `tapes.truncate(LIST_MAX)`, which trims the **display list**,
not the directory. Logs use `tracing_appender::rolling::daily`
(`logging.rs:217`) with no `max_log_files`, so a file per day accumulates in the
external files dir indefinitely. Individually small; on a 98%-full phone this is
the class of thing that quietly wins.

**Fix:** move the `remove_file` cleanup to cover both failure branches;
`sync_all()` the temp file before rename for `profile.json` at minimum; add a
retention sweep for `replays/` and `max_log_files` to the appender.

---

## What is already right (so it does not get "fixed" into a regression)

- **Atomic writes are universal.** No writer bypasses `paths::write_atomic`.
- **Nothing panics on untrusted bytes at the app's own boundaries.**
  `decode_packet` (`net/lib.rs:127`), `decode_net_msg` (`net/lib.rs:240`),
  `replay::decode`, `from_hex32` (`net/lib.rs:477-486`) all return
  `Option`/`Result`. Only 20 panic sites exist in all runtime-reachable code
  (app/sim/render/net/input_touch/replay/fixed_math, test modules excluded) and
  every one I traced is on a compile-time-known-good value or a guarded
  invariant — `netplay.rs:547`'s `take_channel` is gated by `channel_taken`
  (`netplay.rs:454-462`), `share.rs:281-282` by a completeness check,
  `screen.rs:190-196` and `theater.rs:660-666` by fixed player counts.
  **The only reachable panic on remote data is F1, and it is upstream.**
- **Timestep is frame-locked and does not death-spiral under throttling.**
  `sim::TICK_HZ = 60` (`sim/lib.rs:53`) → `RollbackFrameRate(TICK_HZ)`
  (`sim/lib.rs:3328`) → bevy_ggrs's integer-nanosecond accumulator
  (`bevy_ggrs-0.21.0/src/schedule_systems.rs:31-60`), fed from `Time`, whose
  virtual clock clamps at `DEFAULT_MAX_DELTA = 250ms`
  (`bevy_time-0.18.1/src/virt.rs:85`). At 30 fps the sim runs two ticks per
  render frame at correct speed; a resume after a freeze catches up at most
  15 ticks per frame instead of unbounded. `run_slow` handles the
  frames-ahead case. Determinism is unaffected — the matrix builds `--release`
  (`.github/workflows/determinism.yml:110`), same as the APK, so overflow
  semantics match.
- **Teardown does not leak.** `despawn_match` (`screen.rs:622-675`) clears ten
  entity classes and resets the whole rolled-back resource set with a comment
  explaining each one that was learned the hard way;
  `leave_online_match` (`netplay.rs:878-919`) drops the socket (ending the
  detached message loop), removes the session and resets eleven resources; and
  `OnExit(InMatch)` drops a pending ICE fetch (`netplay.rs:240-246`). Every
  exit path I found (`screen.rs:1632,1802,1974`) goes through it. The one thing
  it forgets to reset is `RecentAbsence` — see F4.
- **The crash hook is thoughtful.** `logging.rs:91-114` writes
  `crash.log` atomically to the external files dir precisely because logcat is
  a ring buffer. It just cannot help with F3, which panics inside logging setup.

---

## The one-line answer to "what turns a phone call into a loss?"

F2 ends the match at 9 s with no recovery, and F4 hands Priya the blame for it —
including when the interruption was a one-second glance at a notification and
the person who actually walked away was her opponent. Fixing F4's thresholds is
an afternoon; fixing F2 properly is the lifecycle handler that this codebase
has never had.
