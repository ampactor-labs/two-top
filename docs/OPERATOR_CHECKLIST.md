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
      `cargo apk version`, `adb version` both report; `adb devices` lists **one**
      device with status `device` (not `unauthorized`/`offline`).
- [ ] **Assets present + deterministic:** `python3 scripts/generate_audio.py`
      prints `Generating 12 cues → assets/audio/` (exit 0) and `git status --porcelain
      assets/audio/` is clean (regeneration is byte-identical). `ls assets/audio/*.wav
      | wc -l` == 12.

### A.2 Build, install, launch
- [ ] `cargo apk run -p app --lib --target aarch64-linux-android` cross-compiles,
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
      Left-half drag = move (a joystick spawns under the thumb); right-half
      hold = charge, the LEFT stick aims while held, release throws;
      bottom-right corner tap = dash; **top-strip tap = taunt** (roots you
      0.7 s; finishing the flex feeds the perfect-catch streak one tier).
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

Per `SIGNALING.md`. The online build boots to the Title screen, shows
`TAP TO FIND OPPONENT`, and starts the signaling connection only after Play
(tap lower half / Start). The top-right yellow overlay shows the netplay FSM.
Arena selection works on the Title screen before connecting.

> **Two peers, not two phones.** These gates need two *peers* on different
> networks — pair your one Android phone (on cellular) with a **desktop** build
> (on Wi-Fi). Android↔desktop is a valid (and stronger) cross-platform test;
> a second Android is not required.

### B.0 Loopback pre-flight (one box)
- [ ] **Single-box sanity:** `cargo install matchbox_server && matchbox_server`,
      then two `cargo run -p app -- --room ws://127.0.0.1:3536/two-top?next=2`
      instances. Both boot to Title; press Start in both. Both reach
      `lobby: connected`, swap to P2P with agreed handles, run a live match with
      **zero `DESYNC` events**; killing one forfeits the survivor within ~2.5 s.
      Confirms the room/build plumbing before real networks.

### B.1 Gate 1 — connect + full match across networks
- [ ] **Two devices on DIFFERENT networks** (e.g. one on cellular, one on wifi),
      both launched from builds pointed at the same
      `ws://<host>:3536/two-top?next=2`. Both boot to Title; both press Play.
      Within **~2 s** both overlays reach `lobby: connected (<peer8>)` and the
      countdown begins. (Handles: lower PeerId = handle 0, both peers compute
      the same assignment.)
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
      (`two_top::perf` windows: `avg_ms` ≤ 16.67, `over_budget` ~0). On a
      **below-baseline budget device** (e.g. a Galaxy A-series — Dimensity-class
      SoC / Mali GPU), the pass condition is **graceful degradation**, not a strict
      60 fps: a *steady* frame rate (even if 30–60), no crash, no runaway particle
      growth, and a `two_top::perf` `avg_ms` that holds flat rather than climbing
      over a match. A monotonically rising `avg_ms`/`over_budget` is the real fail.
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

---

## F. Social batch — two-phone verification (2026-07-10 batch)

Everything here rides the 2026-07-10 social batch (side-channel identity,
RUN IT BACK, away grace, theater, gauntlet, southpaw). Flash BOTH phones
from the same commit first: `scripts/phone.sh` per phone (bakes the Railway
room by default). The old APK predates the batch.

### F.1 Solo smoke (one phone)

- [ ] **Title additions:** NAME row dials glyphs and persists across relaunch;
      REPLAYS button present; five settings rows ending in `southpaw off`;
      all bands tappable without hitting the notch or each other.
- [ ] **Gauntlet:** fresh install's bot is a passive dummy; beat it and the
      practice button reads GAUNTLET TIER 1; the tier-1 bot visibly attacks
      sooner. Lose on purpose — the tier resets, the button reverts.
- [ ] **Taunt mark:** the ring + TAUNT label sits top-center in a live match,
      dim; tapping the strip fires the flex and the mark lights.
- [ ] **Theater:** after a decided match, REPLAYS lists the tape with the right
      winner/arena/duration/date; it plays back with HUD + audio + kill-cam;
      tap pauses, dragging the bottom strip scrubs (backward too), speeds
      switch, top edge exits, BACK returns to Title.
- [ ] **Southpaw:** toggle on — dash ring moves bottom-LEFT, move stick lives on
      the right half, throw on the left, hints mirrored; toggle off restores.

### F.2 Two phones, same room

- [ ] **Names exchange:** pair via quick match; after the match each summary
      names the winner by its dialed name and shows FIRST MEETING with the
      other's name. The tape header carries both names (check in the theater).
- [ ] **Pip slam + victory pose:** every kill slams its pip; the decided match's
      winner stands in the charge pose through the summary.
- [ ] **RUN IT BACK, both directions:** A presses THROW → A shows `waiting on
      <B>...`, B shows `<A> WANTS TO RUN IT BACK`; B taps THROW → both restart
      in lockstep, 0-0, no desync. Repeat with B initiating.
- [ ] **Rivalry counts:** the rematch's summary reads 2ND MEETING with the
      standing; career W-L on the title moves.
- [ ] **Away grace:** mid-round, pull the notification shade on A ~4 s → B shows
      `<A> AWAY`, sim frozen under it → release → the round RESUMES, no desync,
      no forfeit.
- [ ] **Abandonment:** A locks its screen 15+ s → B gets `<A> FLED — the field
      is yours` + a career WIN; A on waking sees `MATCH ABANDONED — you left
      the duel` + a career LOSS. Both leave via the top band and can
      FIND OPPONENT again on a fresh socket.
- [ ] **Clean leave:** at any online summary, A taps the top band → B flips to
      FLED *immediately* (the goodbye message, not the 9 s grace).
- [ ] **Private room:** both dial the same 4 glyphs → they pair; one on QUICK and
      one on a code → they never meet.

### F.3 Cross-network (the Rung 5 gate)

- [ ] One phone on cellular, one on wifi, both at the Railway room: they pair
      and complete a match. **If both sit at AWAITING >15 s** and the overlay
      shows the relay hint, ICE is failing behind the carrier NAT — stand up a
      TURN relay and rebuild with `TWOTOP_TURN_URL/_USER/_PASS`
      (SIGNALING.md § NAT traversal), then re-run this gate.

### F.4 Tuning notes (collect, don't fix live)

- [ ] Audio mix on the phone speaker: cue trims vs music beds
      (`scripts/generate_audio.py` constants).

### F.5 UX batch (2026-07-16): follow stick, QUIT chip, endgame card

- [ ] **Follow stick:** press the left half and drag a long swipe right —
      the base ring gets towed once the thumb passes its edge and the
      character keeps full speed; reversing direction responds within a
      thumb-width instead of a dead swim back. Same behavior mid-aim.
- [ ] **QUIT chip:** visible top-right during live play (top-left with
      southpaw on); first tap arms it to SURE?, second tap exits to Title;
      letting it sit ~3 s disarms. Tapping the taunt strip just LEFT of
      the chip still flexes — the corner never taunts.
- [ ] **Online quit honesty:** quitting a live duel from phone A → B flips
      to FLED with the career win; A's career shows the loss.
- [ ] **Summoning escape:** in an empty room the chip reads CANCEL and one
      tap returns to Title (no more being stuck at SUMMONING).
- [ ] **Endgame card:** WINNER and score render on single lines (no
      mid-phrase wrapping); when an opponent flees mid-match the card names
      YOU the winner and no big AWAY/FLED text stacks over it.
- [ ] **Replay actually saves:** finish one practice match and check
      REPLAYS lists it (the per-tick recorder must survive the match-start
      hitch that used to poison every tape).
- [ ] **New arenas:** cycle the picker to The Pit / The Vigil / The
      Gallery. Pit: a thrown fang ricochets off the outer wall and stays
      live (two bounces, then drops); walking into the edge never starts
      the void timer; the floor never crumbles. Vigil: play a full
      killless round — it expires scoreless with the island intact.
      Gallery: corridors read clearly and no spawn sits against a block.
- [ ] **Feel tune v10:** movement/dash/fangs noticeably calmer; dash
      travels half as far; the dash corner button is visibly bigger and
      easier to hit blind.
- [ ] **The Forest:** trees block walking and ricochet fangs; two plain
      hits fell one (a Heavy fells in one); a FIRE fang sets a tree
      alight — it kills on touch, the fire jumps to clustered neighbors
      within a second, and burnt trees stay down as stumps until the
      rematch regrows them. The two lone center trees never catch from
      a neighbor. Scrub a Forest tape backward in the theater — felled
      trees stand back up.
- [ ] **Ephemeral ICE (once a vendor is deployed):** with
      `TWOTOP_ICE_URL` baked, match entry logs `fetching ephemeral ICE
      credentials` then `ice config resolved fetched=true`, and a
      cellular-vs-wifi pair connects through the relay. Kill the vendor
      and re-enter — the fetch times out within ~2.5 s and the match
      still works STUN-only on wifi.
- [ ] Perspective: focal + UI reserve, spawn spread — do the duelists open too
      close to center?
- [ ] Anything the 30-minute feel session (section E) surfaces, now including
      the social loop: does RUN IT BACK feel faster than re-queueing? Does the
      AWAY overlay read at a glance?
