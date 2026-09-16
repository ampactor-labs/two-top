# Persona 1 — "Dani & Mo, first time, on a café table"

Read-only audit. Two friends, two Androids, two fresh sideloads, zero prior
knowledge, sixty seconds of patience. Every finding below is traced through
the shipped code; line numbers are from the working tree at audit time.

---

## What the session felt like

Mo opens the app first and lands, with no warning, on **a keyboard**. No
header, no "what's this for", no back arrow — just a blinking caret and 37
keys over a dim arena. Mo types MO, hunts for a while, finds DONE. Dani
skips it by mashing the bottom of the screen, which turns out to *be* DONE.

Then the good part: the Title is genuinely clear. FIND OPPONENT is big and
bottom. Dani taps it, sees a table appear, taps it again because nothing
obviously happened — and is now in a match against **the bot**, because the
button under her thumb silently became PLAY THE BOT between the two taps.
Mo is still waiting alone in the room.

They reset and try the private dial. Mo reads the roster screen and picks
THE FOREST because "the grove burns" sounds great. Dani leaves hers on
Anchor. Both dial C-U-R-S. Both tap DUEL AT C-U-R-S. Both phones show the
**same** waiting text — `AWAITING A CHALLENGER / room C U R S / dial the
same code over there` — and they are in two different rooms. Fifteen
seconds later both phones tell them their *networks* probably need a TURN
relay. They conclude the game is broken and put the phones down.

---

## Findings

| # | Sev | Finding | Evidence |
|---|-----|---------|----------|
| 1 | 🔴 | Arena pick silently partitions the room; both phones show identical waiting text, then blame the network | `room_code.rs:80`, `lobby_overlay.rs:105`,`:121`,`:125` |
| 2 | 🟠 | A second tap on FIND OPPONENT starts a **bot** match — same button slot, instant swap, no debounce | `screen.rs:710`,`:1727`,`:1745`,`:1793` |
| 3 | 🟠 | Signaling failure is fully silent: both lobby texts hide, driver is dropped, nothing retries | `netplay.rs:485`,`:493`, `lobby_overlay.rs:142`,`:189` |
| 4 | 🟠 | The join deep-link is a no-op whenever 2-Top is already running, and the app never says so | `room_code.rs:159`,`:189`,`:553`, `Cargo.toml:179` |
| 5 | 🟡 | No build/protocol version anywhere: mismatched APKs pair and desync with a log line only | `net/src/lib.rs:210`, `netplay.rs:643` |
| 6 | 🟡 | First screen in the game is an unlabeled keyboard — no title, no BACK, no SKIP | `profile.rs:441`,`:351`, `screen.rs:1236` |
| 7 | 🟡 | The whole bottom band of that keyboard is DEL/DONE; one stray low tap commits and exits | `profile.rs:71`,`:493` |
| 8 | 🟡 | Nothing in the repo handles the Android Back gesture (consequence SPECULATIVE) | grep: zero hits |

---

## 1. 🔴 Two phones in one room code still never meet if their arenas differ — and the app blames the network

The arena tag is part of the **room name**, on the private path and the
quick path alike:

```rust
// crates/app/src/room_code.rs:80
pub fn room_url(&self, arena: sim::ArenaId) -> Option<String> {
    let base = self.base_url.as_ref()?;
    let code = self.custom.then(|| self.code_string());
    Some(room_url_with_parts(base, code.as_deref(), arena_room_tag(arena)))
}
```

Its own test pins it: `custom_room_carries_the_code_and_the_arena`
(`room_code.rs:634`) → `ws://h/two-top-CURS-pit?next=2`. So CURS+Pit and
CURS+Anchor are two different rooms. That is a deliberate design (PLAYBOOK
line 302 states it), and it is sound — the problem is entirely in what the
running app tells the player.

The waiting overlay names the code and **not** the table:

```rust
// crates/app/src/lobby_overlay.rs:105
let room_line = if room.custom {
    format!("room {}", code_spaced(&room.code_string()))
} else {
    "quick match".to_string()
};
...
// :121
"AWAITING A CHALLENGER{dots}\n\n{room_line}\ndial the same code over there"
```

Two phones sitting in *different* rooms therefore render **byte-identical
status text**. There is no way to spot the mismatch from the screen that is
supposed to be diagnosing it. And the primary button they both pressed reads
`DUEL AT C-U-R-S` (`screen.rs:1119`) — the code, never the table.

Then the diagnosis actively points the wrong way:

```rust
// crates/app/src/lobby_overlay.rs:123
if since > STALL_DIAGNOSIS_SECS {           // 15.0s, :65
    m.push_str("\n\nif the other phone shows this too,\nthe networks may need the relay (TURN)");
}
```

"If the other phone shows this too" is exactly the state a mismatched arena
produces, and the text tells them it is a NAT/TURN problem. This is the
single most likely way a first session dies, because the arena line on the
Title is tappable bait (`screen.rs:957`, band `SUBLINE_RECT` 0.52–0.58) and
the roster sells each table with a verb ("the grove burns").

Quick match has the same shape: `"quick match"` never says *which* table you
are queued for, and the public queue is silently split seven ways.

**Fix (cheap, no sim change):** put the table in `room_line` —
`format!("room {} · {}", code, arena_title(selected.0))` — so a mismatch is
visible by comparing the two screens. Then reorder the stall diagnosis to
lead with "both phones must be on the same table" and demote TURN to second.
Optionally make `DUEL AT C-U-R-S` read `DUEL AT C-U-R-S · THE PIT`.

---

## 2. 🟠 Double-tapping FIND OPPONENT puts you in a bot match

The title's primary and the waiting room's bot offer are **the same
rectangle**: same anchor, same fill size, same tap band.

```rust
// crates/app/src/screen.rs:710
const PLAY_BTN_RECT: (f32, f32) = (0.82, 0.93);
// :853  title PLAY      → Vec2::new(0.0, PLAY_ANCHOR_Y), Vec2::new(760.0, 150.0)
// :1732 bot offer       → Vec2::new(0.0, PLAY_ANCHOR_Y), Vec2::new(760.0, 150.0)
```

The offer is shown the instant `AwaitingPeer` goes up, which is the first
frame of `InMatch` (`screen.rs:1745`, `update_awaiting_peer` at `:1185` —
`Idle`/`Connecting` are both `!is_in_match()`):

```rust
// crates/app/src/screen.rs:1745
fn update_bot_offer_button(awaiting: Res<AwaitingPeer>, ...) {
    *vis = if awaiting.0 { Visibility::Visible } else { Visibility::Hidden };
```

and any tap in that band converts the session on the spot, with no
debounce, no arm/confirm, and no minimum age:

```rust
// crates/app/src/screen.rs:1793
let tapped = win.y > 0.0 && world.resource::<Touches>().iter_just_pressed()
    .any(|t| in_band(t.position().y / win.y, PLAY_BTN_RECT));
if !(key || tapped) { return; }
world.resource_mut::<crate::bot::PracticeMode>().0 = true;
crate::netplay::leave_online_match(world);
```

Tap 1 (frame N) sets `NextState(InMatch)`; the transition applies on N+1;
any second physical tap from N+1 onward lands on PLAY THE BOT. At 60 Hz a
mashed double-tap is ~9 frames apart. The author's comment at `:145` shows
same-*frame* double-fire was considered ("After summary_buttons_input so the
same tap can't double-fire") — the second-tap case was not.

The damage is asymmetric and confusing: Dani is in a bot match (the intro
card does at least say `DANI vs THE BOT`, `intro_card.rs:41`), while Mo's
phone still says AWAITING A CHALLENGER and will say it forever.

**Fix:** two lines. Gate `bot_fallback_input` on time-in-state (ignore taps
for ~0.5 s after `OnEnter(InMatch)`), and don't *show* PLAY THE BOT until
the summons has actually hung — 5–8 s is the honest threshold, and it also
makes the button mean what its doc comment says it means ("while the summons
hangs", `:1742`).

---

## 3. 🟠 When signaling dies, the screen just goes quiet

The pre-pairing poll drops the driver and parks the lobby at `Idle` on any
socket error:

```rust
// crates/app/src/netplay.rs:485
let (our_id, peer_updates) = match polled {
    Ok(v) => v,
    Err(e) => {
        tracing::error!(..., "signaling connection lost before pairing — abandoning the summons");
        world.remove_non_send_resource::<MatchboxDriver>();
        *world.resource_mut::<LobbyState>() = LobbyState::Idle;   // :493
        return;
    }
```

`Idle` is the one lobby state with **no** player-facing text:

```rust
// crates/app/src/lobby_overlay.rs:142
LobbyState::Idle | LobbyState::Connected { .. } => None,
```

and the dev corner label hides on `Idle` too (`lobby_overlay.rs:189`:
*"hide the label entirely while idle"*). So SUMMONING… vanishes mid-breath
and is replaced by nothing. `AwaitingPeer` stays true (`Idle` is
`!is_in_match()`), so PLAY THE BOT and CANCEL stay on screen — the player
gets a waiting room that has stopped waiting, with no error, forever:
`start_matchbox` only runs on `OnEnter(InMatch)` (`netplay.rs:239`), so
nothing ever retries.

The comment justifying this path asserts something the code cannot deliver:

> *"The SUMMONING overlay's stall diagnosis has already told the player to
> check the connection"* — `netplay.rs:477`

The stall diagnosis fires at `STALL_DIAGNOSIS_SECS = 15.0`
(`lobby_overlay.rs:65`), while the comment three lines above records the
failure arriving at **~6 s** for a phone with no network
(`netplay.rs:475`). At 6 s the player has seen nothing but dots.

**Fix:** add a terminal `LobbyState` (or a `SummonFailed` flag) instead of
falling back to `Idle`, and render it: "COULDN'T REACH THE ROOM SERVER —
check this phone's connection" plus a RETRY that re-runs `start_matchbox`.
`Idle` should never be reachable while the player is sitting in `InMatch`.

---

## 4. 🟠 The QR deep-link does nothing if the app is already open

The launch URI is read exactly once, in `PostStartup`:

```rust
// crates/app/src/room_code.rs:553
#[cfg(target_os = "android")]
app.add_systems(PostStartup, apply_launch_join);
```

and it reads it through `getIntent()`:

```rust
// crates/app/src/room_code.rs:159
let intent = env.call_method(&activity, "getIntent", "()Landroid/content/Intent;", &[])...
let data  = env.call_method(&intent, "getDataString", "()Ljava/lang/String;", &[])...
```

The activity is `launch_mode = "singleTask"` (`crates/app/Cargo.toml:179`)
with a `twotop` VIEW filter (`:186`–`:194`). With singleTask, a VIEW intent
arriving while the task exists is delivered to `onNewIntent` and the existing
activity is brought to front — the process is not restarted, `PostStartup`
does not run again, and `getIntent()` keeps returning the *original* launch
intent unless something calls `setIntent`. Nothing in the repo handles
`onNewIntent` or `setIntent` (grep for both: zero hits outside `launch_uri`).

So Mo's phone flips to 2-Top showing whatever screen it was on, with the
dial untouched and QUICK MATCH still selected. There is no toast, no
"joined C-U-R-S", nothing — and if Mo then taps FIND OPPONENT they land in
the public queue while Dani waits in the private room.

**Honest caveat:** `web/join.html:31` already documents this in its fine
print (*"already OPEN on your phone: dial the four glyphs above by hand (a
running app keeps its own counsel about new links)"*). So it is a known
limitation — but it is 14px dim text under the big button, the app itself
gives zero feedback, and this is the single ritual the product is built
around. Dani will not read it.

**Fix:** the right fix is a thin Java activity subclass whose `onNewIntent`
calls `setIntent(intent)`, plus re-reading `launch_uri()` on resume. Failing
that, make the app *confirm*: on a successful `apply_launch_join`, flash
"JOINED C-U-R-S · THE PIT" on the Title for a couple of seconds, so its
absence is at least visible.

---

## 5. 🟡 Nothing negotiates a build version, so mismatched APKs pair and desync in silence

The handshake carries identity and keys only — there is no version field in
any variant:

```rust
// crates/net/src/lib.rs:210
pub enum NetMsg { Profile(ProfileData), RematchWant, Bye,
                  Profile2(ProfileData2), MatchSig { sig: [[u8; 32]; 2] } }
```

and the room name carries code + arena only (`room_code.rs:108`), never
`SIM_VERSION`. When the sims diverge the only reaction is a log line:

```rust
// crates/app/src/netplay.rs:643
GgrsEvent::DesyncDetected { frame, local_checksum, remote_checksum, addr } => {
    tracing::error!(..., "DESYNC DETECTED — local and remote state diverged");
}
```

No UI, no abort, no forfeit. Two friends who sideloaded the APK a week apart
get a match where each phone shows a different fight, each sees themselves
winning, and neither is told why. Replay files are strictly version-matched
(`CLAUDE.md`: "Strict replay version matching, no migrations") — the live
wire, which matters more, is not.

**Fix:** put `sim::SIM_VERSION` in the room name suffix (one line in
`room_url_with_parts`) so incompatible builds cannot even see each other,
and surface `DesyncDetected` as a visible "THIS MATCH DESYNCED — builds may
differ" card instead of a log.

---

## 6. 🟡 The first screen in the game is an unlabeled keyboard

First boot jumps off the Title immediately:

```rust
// crates/app/src/profile.rs:441
fn open_name_entry_on_first_boot(...) {
    if *done || profile.named || netplay.room_url.is_none() { return; }
    if *screen.get() != AppScreen::Title { return; }
    *done = true;
    next.set(AppScreen::NameEntry);
}
```

What `NameEntry` renders is *only* the typed name + caret and the key
glyphs (`update_name_ui`, `profile.rs:572`–`:640`). The 2-TOP banner is
hidden the moment you leave Title (`screen.rs:1236`), the title buttons hide
(`screen.rs:1080`), the room pad hides (`room_code.rs:406`). So the first
thing a stranger ever sees is 37 floating letters with no prompt, no
"YOUR NAME", no BACK, and no SKIP. The only exit is DONE — discoverable, but
only after you work out what the screen is.

The caret is doing all the explaining, deliberately (`profile.rs:607`: *"it
says 'type here' without a word of instruction"*). Against a first-time
player who has never seen the Title yet, that is too much weight on one
blinking underscore.

**Fix:** one anchored line above the entry — `WHAT DO THEY CALL YOU?` — and
let DONE read `DONE` when a name is typed and `SKIP` when it is empty (the
code already deals a placeholder in that case, `profile.rs:557`). Cheaper
alternative: show the Title for a beat first, then open the keyboard.

---

## 7. 🟡 A stray low tap while typing commits the name and leaves

The DEL/DONE hit test is the **entire width** of a horizontal band, split at
the midline:

```rust
// crates/app/src/profile.rs:71
const ACTION_BAND: (f32, f32) = (ACTION_ROW_Y - 0.042, ACTION_ROW_Y + 0.042);
// :493
if (ACTION_BAND.0..ACTION_BAND.1).contains(&fy) {
    hit = Some(if fx < 0.5 { GridKey::Del } else { GridKey::Done });
}
```

The glyphs are drawn at fx 0.28 and 0.72 (`profile.rs:440`,`:447`) — two
small words — but the live target is 100% of the width and ~8% of the
height, checked *before* the letter grid. Anything Dani's palm brushes at
fy ∈ [0.766, 0.850] on the right half saves the name and exits to Title. The
band also overlaps the bottom 0.002 of the Z row (rows end at 0.768), so the
bottom sliver of Z/X/C/V/B/N/M is DEL or DONE.

Not fatal — the name is recoverable by tapping your demon
(`profile.rs:459`) — but it is the "invisible hit zone" pattern the rest of
the app deliberately retired (`screen.rs:968`: *"No more invisible
screen-half zones"*).

**Fix:** bound the DEL/DONE hit rects to the drawn boxes (they already have
box geometry elsewhere via `spawn_button_part`), and check the letter grid
first so the Z row keeps its last two thousandths.

---

## 8. 🟡 Nothing handles the Android Back gesture — SPECULATIVE consequence

Provable: every menu exit in the app is either a tap band or
`KeyCode::Escape` (`arena_select.rs:227`, `settings.rs:152`,
`rivals.rs:347`, `theater.rs:791`, `screen.rs:1179`, `profile.rs:544`), and
a repo-wide grep for `GoBack`, `BrowserBack`, `back_button` or any Android
back handling returns **zero hits**. Mo, who reads everything, will press the
system Back button to leave the arena roster.

Not provable from this repo: what actually happens. Depending on how
winit/android-activity reports the key, Back is either swallowed entirely
(Back does nothing anywhere in the app — confusing) or unhandled and
finishes the activity (the app closes mid-summons, taking the room with it).
I could not determine which without running on a device, so this is
**SPECULATIVE** as to impact and should be verified on hardware before any
fix. If it is the second case, it belongs at 🔴.

---

## Checked and found sound (not findings)

- **CANCEL always exists while waiting.** `quit_label(awaiting=true)` →
  `"CANCEL"`, one tap, no confirm (`screen.rs:1554`,`:1610`), and the quit
  chip is visible for the whole pre-peer wait (`:1652`). The waiting room is
  not a dead end.
- **Cancel → re-summon works.** `leave_online_match` drops the socket, which
  ends the message loop, and clears every peer resource
  (`netplay.rs:888`–`:917`); the next `OnEnter(InMatch)` builds a fresh one.
- **Name entry handles the edges correctly.** Empty DONE re-deals the
  install-id placeholder rather than shipping a blank name
  (`profile.rs:557`); over-length input is cut, not wrapped
  (`slots_from_name`, `:199`); identical names on both phones are legal by
  design and disambiguated by `identity_tag` (`:226`).
- **First-run file absence is handled.** Missing `profile.json` mints an
  identity (`profile.rs:272`), a corrupt one is quarantined not clobbered
  (`:298`), missing `settings.json` falls back to defaults with
  `arena: 0` on both phones (`settings.rs:58`,`:97`), and Android's config
  dir is resolved through `internal_data_path()` not `dirs`
  (`paths.rs:21`).
- **The keyboard's draw and hit-test do agree.** `key_at` is an exact
  inverse of `key_center` (`profile.rs:76` vs `:93`), and `ScreenAnchor`'s
  frac→world map is camera-relative on both axes (`anchor.rs:92`,`:125`,`:142`),
  so the fraction↔anchor conversion holds at any aspect. (I tried to make
  this a new consequence of audit finding #2 and it does not hold up.)
- **Controls are discoverable.** MOVE / THROW / TAUNT hints show on first
  live play and graduate on first use (`touch_controls.rs:191`,`:555`);
  DASH draws its own labeled ring sized to its real zone
  (`touch_controls.rs:78`,`:152`).
- **The double-drawn PLAY THE BOT** on a fled-opponent summary (summary
  primary at `screen.rs:1483` and bot offer at `:1732` occupy the same slot
  with the same label and colors) is invisible in practice. Not worth a row.
