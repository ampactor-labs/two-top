# Operator Checklist — 2-Top v1.0.0-rc1

These are the **release-acceptance checks that CI cannot run** — they need a real
Android device, a real network, or a human watching/listening. CI and the
4-platform determinism matrix already prove the sim is bit-identical across
linux-x64 / aarch64-linux / macOS-ARM / android; this checklist covers the
device, network, and game-feel surface that sits on top of that guarantee.

Run it **after the `v1.0.0-rc1` tag is pushed and both workflows are green.**
Tick each box; file an issue for any failure and link it back to the failing
item. `manual:` items have no single command — follow the instruction and judge
the pass condition.

---

## 0. Pre-flight

- [ ] On the tagged commit, **CI** (`check / test / clippy`) and the
      **Determinism** matrix (4 platforms, byte-identical checksum TSV diff) are
      both green. `gh run list --limit 4` shows `success` for both on the tag's
      commit.

---

## A. Android — sideload + on-device smoke

Full walkthrough in `SIDELOAD.md`. The whole point of this section: **audio and
haptics are device-only** (excluded from the determinism matrix by design — a
vibrator/speaker is a device), so the phone is the *only* place to confirm them.

### A.1 Prerequisites
- [ ] `rustup target list --installed | grep aarch64-linux-android` lists the target;
      `$ANDROID_NDK_ROOT` (or `$ANDROID_NDK_HOME`) points at an NDK (r26+);
      `cargo apk --version`, `adb version` both report; `adb devices` lists **one**
      device with status `device` (not `unauthorized`/`offline`).
- [ ] **Assets present + deterministic:** `python3 scripts/generate_audio.py`
      prints `Generating 12 cues → assets/audio/` (exit 0) and `git status --porcelain
      assets/audio/` is clean (regeneration is byte-identical). `ls assets/audio/*.wav
      | wc -l` == 12.

### A.2 Build, install, launch
- [ ] `cargo apk run -p app --target aarch64-linux-android` cross-compiles,
      packages (bundling `assets/` via the `assets = "../../assets"` manifest key),
      installs, and launches. A launcher icon **2-Top** appears; the app opens to the
      portrait **Title** screen and does not immediately exit.
- [ ] **No panic / no desync on launch:** `adb logcat --pid=$(adb shell pidof
      com.ampactorlabs.twotop)` shows no Rust panic backtrace and **no
      `SyncTestMismatch`** line (that would be a real determinism violation, not a
      build issue).
- [ ] **Assets bundled (the load-bearing gate):** the Title screen renders the
      cathedral floor backdrop + the `2-TOP` overlay with the three arenas and the
      `— settings —` line (rendered as `[H] haptics on   [-/=] sfx 70%   [ [ / ] ]
      music 60%   [ , / . ] deadzone 12%`). A
      **black/blank/untextured** screen here means the `assets` dir did not package —
      fix the manifest `assets` path and rebuild before continuing.

### A.3 On-device gameplay
- [ ] **Touch arena picker:** tapping the **upper half** on Title cycles the
      bracketed selection Anchor → Crossing → Reliquary → Anchor (1/2/3 keys are
      desktop-only).
- [ ] **Start + play:** tapping the **lower half** starts the match; duelists
      spawn, the 3-2-1-GO countdown runs, a fresh SyncTest session drives the sim.
      Lower-left drag = move; right-side touch held/dragged = aim → release throws.
      (**Note:** DASH/TAUNT have no touch affordance — not testable on touch.)
- [ ] **Couch-on-one-device is lockstep, not 1v1:** both duelists move identically
      from one touch (Android SyncTest = both players local). A true 1v1 needs two
      devices over netplay (§B). Confirm the mirror behavior is what's seen.
- [ ] **Full match completes:** throw/catch/death/respawn + round flow work to
      `MATCH_WIN_THRESHOLD` kills → the **MatchOver summary** shows (`CUR/STAG WINS`,
      score, `press THROW to play again`).
- [ ] **Rematch:** a throw gesture on the summary restarts the match (countdown
      runs again, no return to Title).

### A.4 Audio, haptics, perf (device-only)
- [ ] **Audio:** media volume up, Settings sfx/music non-zero. The ambient bed
      plays from boot; SFX cues fire on their edges (throw/empowered, ricochet,
      catch/perfect, kill, shatter, pickup spawn/collect, the 3-2-1 + pitched GO
      tolls, round/match-over sting). Silence (with volume up) ⇒ asset-bundling gate.
- [ ] **Haptics fire (`Settings.haptics = on`, Title shows `haptics on`):** a light
      buzz on **your** throw (~10 ms), a heavy buzz on a kill (~60 ms, either player),
      a crisp buzz on **your** perfect catch (~15 ms). Requires the `VIBRATE`
      permission (now declared). Single-device couch → both handles count as local.
- [ ] **Haptics are best-effort:** if the device denies the vibrator,
      `adb logcat ... | grep two_top::haptics` may show swallowed `vibrate failed`
      warns — but the game loop never crashes/hangs. Absence of those warns on a
      Pixel-6-class device = healthy.
- [ ] **60 fps feel:** `adb logcat --pid=$(adb shell pidof com.ampactorlabs.twotop)
      | grep two_top::perf` during active play shows `frame-time window` lines with
      `avg_ms` ~16.7 and `over_budget` near 0; movement/throws/kill-cam read smooth.
- [ ] **Vulkan guard:** if the app launches to a black screen with no error,
      `adb shell pm list features | grep vulkan` confirms the device is Vulkan-capable
      (wgpu needs it; a Pixel 6 qualifies).

---

## B. Netplay — real-network gates

Per `SIGNALING.md`. The online build skips the Title menu and boots straight into
`InMatch`; its lobby lifecycle is the netplay FSM (top-right yellow overlay).
Arena selection online is the `TWOTOP_ARENA=anchor|crossing|reliquary` env var.

### B.0 Loopback pre-flight (one box)
- [ ] **Single-box sanity:** `cargo install matchbox_server && matchbox_server`,
      then two `cargo run -p app -- --room ws://127.0.0.1:3536/two-top?next=2`
      instances. Both reach `lobby: connected`, swap to P2P with agreed handles,
      run a live match with **zero `DESYNC` events**; killing one forfeits the
      survivor within ~2.5 s. Confirms the room/build plumbing before real networks.

### B.1 Gate 1 — connect + full match across networks
- [ ] **Two devices on DIFFERENT networks** (e.g. one on cellular, one on wifi),
      both launched at the same `ws://<host>:3536/two-top?next=2`. Within **~2 s**
      both overlays reach `lobby: connected (<peer8>)` and the countdown begins.
      (Handles: lower PeerId = handle 0, both peers compute the same assignment.)
- [ ] **Full match, zero desync:** play every round to first-to-5. Both screens
      stay bit-identical; **no `DESYNC DETECTED` error** on target `two_top::net`
      (ggrs checksums every 30 frames). Match ends naturally at MatchOver.
- [ ] **(If a desync appears)** save both devices' `.bmrg.log` per-frame checksum
      files and run `bash scripts/diagnose_desync.sh` — it reports the first
      diverging frame + component.

### B.2 Gate 2 — brief blip reconnects bit-identical
- [ ] Mid-round, airplane-mode **ON ~1.5 s then OFF** on one device (inside the 3 s
      forfeit window). Once silence crosses ~1 s both overlays show
      `lobby: reconnecting… peer=<id8> since=f<frame>`; on reconnect both return to
      `lobby: connected`, the round resumes, and post-blip sim stays bit-identical
      (no `DESYNC`).

### B.3 Gate 3 — long disconnect forfeits
- [ ] Mid-match, kill one device's connectivity for **5+ s**. The **surviving**
      device transitions `connected → reconnecting… → lobby: FORFEIT (peer=<id8>)`
      within ~3 s, and `MatchState::MatchOver` fires.
- [ ] On restoring connectivity, the **disconnected** device shows
      `reconnecting…` then a **permanent** `FORFEIT` (terminal — no recovery to
      Connected).

### B.4 Rematch over netplay
- [ ] After a **natural** MatchOver (not a forfeit), a single THROW from either
      player (desktop: Space; Android: throw gesture) restarts the match on **both**
      devices in lockstep: score 0-0, fresh countdown, both respawn, arena wiped
      (boomerangs/pickups/fire-cells despawned, pyres un-shattered, stains cleared).
      **No `DESYNC`** (rematch reuses the `THROW_DOWN` level signal — no wire change).
- [ ] After a **forfeit**, pressing THROW does **not** rematch (Forfeited is
      terminal; relaunch to play again). Confirms the boundary isn't a bug.

---

## C. Game-feel (desktop, human-observed)

Run with `cargo run -p app --release`. Couch controls: P0 = WASD / Space throw /
LShift dash; P1 = arrows / RShift throw / RCtrl dash.

### C.1 Screens + flow
- [ ] **Title:** boots to Title over an empty floor (no players). Overlay shows
      `2-TOP`, the three arenas with the selected one bracketed `> Anchor <`, the
      pick/start hints, and the `— settings —` line at defaults.
- [ ] **Picker (keyboard):** 1/2/3 move the bracket to Anchor/Crossing/Reliquary.
- [ ] **Start:** Space/Enter → InMatch with the **selected** arena's props
      (Anchor pyres / Crossing chasm-bridge / Reliquary doors); countdown plays.
- [ ] **Summary:** play to first-to-5 → centered overlay with the correct winner
      (`CUR`/`STAG`), the correct `p0 — p1` score, `press THROW to play again`, and
      (couch only) `press ESC for lobby`.
- [ ] **Play-again:** THROW restarts in the **same** arena, score 0-0, fresh
      countdown, entities cleared; summary disappears; stays InMatch.
- [ ] **Back-to-lobby:** ESC on the summary → Title (match fully torn down, sim
      idle). ESC during an **undecided** match does nothing.
- [ ] **Arena switch across matches:** Title → Anchor → start → finish → ESC →
      Crossing → start loads the new arena cleanly (no leftover props).

### C.2 Settings (persist + clamp)
- [ ] H toggles `haptics on/off`; `-`/`=` step sfx by 10% (clamp 0–100); `[`/`]`
      step music by 10% (the ambient bed changes loudness **live**); `,`/`.` step
      deadzone by 2% (clamp 0–40).
- [ ] **Persist:** change all four, fully quit, relaunch → the same values show.
      `~/.config/two-top/settings.json` (Linux) holds them.
- [ ] **Forward-compat:** hand-edit the JSON to `{ "haptics": false }` → launches
      with haptics off and every other field at default (missing → default,
      out-of-range/NaN → clamped, no crash).

### C.3 Audio cues (listen)
- [ ] Countdown: three descending tolls on 3/2/1, a brighter **pitched-up GO**
      toll the instant the round goes live.
- [ ] Throw (and the brighter **empowered** variant after a perfect catch);
      ricochet only on a >45° wall/pyre bounce (not Curve/Bouncy/recall);
      catch + the distinct **perfect-catch bell**; kill; pyre shatter (Anchor);
      pickup spawn + collect; round-over + match-over sting.
- [ ] **Seamless ambient loop:** idle on Title for minutes — the bed loops with
      **no click/pop/gap** at the wrap and keeps playing across Title↔InMatch.

### C.4 Screen shake / kill-cam
- [ ] Kill → strong shake (decays ~0.4 s) + a brief (~2-frame) white **kill flash**
      that doesn't linger/stack; pyre shatter + perfect catch shake lighter. Shake
      never drifts — the camera returns exactly to base framing.
- [ ] **Kill-cam beat:** ending a round/match on a kill eases in + zooms ~1.6×
      onto the kill position (soft smoothstep, no jerk), holds through the beat,
      eases back on the next countdown. MatchOver holds the zoom under the summary.
- [ ] **Camera modes:** desktop frames the **whole arena** statically; Android
      uses a zoomed **follow** cam damping toward the player centroid.

---

## D. Performance

- [ ] **Desktop 5-minute frame-time session:** `cargo run -p app --release`, then
      play/idle a full match for **≥5 min**. `grep two_top::perf
      target/release/logs/two_top.log.*` — across the ~60 `frame-time window` lines
      every window shows `avg_ms` ≤ 16.67 ms and `over_budget` near 0. **Investigate**
      (do not pass) any window with persistently elevated `over_budget` or `max_ms`
      well above 16.67 ms. Isolated single-frame blips at scene transitions are OK.
- [ ] **EffectSprite stress:** start a match, collect **Fire + Multishot**, sustain
      a rapid throw/recall slugfest (both players) ~20–30 s. The particle count stays
      bounded (`EFFECT_SPRITE_CAP = 500`, oldest/most-finished culled first — no
      pop-out of fresh effects), and frame rate does **not** monotonically decline.
- [ ] **On-device 60 fps (Phase 18 exit criterion):** on a Pixel-6 / iPhone-12
      baseline, a full match (incl. a Fire+Multishot slugfest) holds a stable 60 fps
      (`two_top::perf` windows: `avg_ms` ≤ 16.67, `over_budget` ~0). Older hardware
      degrades gracefully (lower but steady, no crash, no runaway particles).
- [ ] **Release log lens works:** the `two_top::perf` `frame-time window` lines
      appear in a **release** build's log **without** any `RUST_LOG` override
      (confirms `release_max_level_info` keeps `frame_time_watch`'s `info!` in). If
      absent, the profiling lens is broken and the perf checks can't be done.

---

## E. 30-minute feel session

- [ ] A **30-minute** play session on Pixel-6-class hardware (Phase 18 exit
      criterion). Note anything that feels off — input latency, audio timing, shake
      intensity, kill-cam pacing, deadzone, readability under effect-heavy moments —
      back into issues. This is the subjective "does it feel like a shipping title"
      gate that no automated check can replace.
