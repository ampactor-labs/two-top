# P4 — Jules, who posts clips
### An audit of the share loop, the tape drop, the web theater, and the "proof"

> I just won 5-2 on the Pit off a ricochet I will never hit again. I tap SHARE.
> The label vanishes. Nothing happens for a beat — no spinner, no "posting" —
> and then a QR card appears with a URL under it. Good. I screenshot it, paste
> the link into the group chat, and type "look at the last 20 seconds."
>
> Twelve people click. I don't know what any of them see. I know what *I* see,
> which is nothing at all if the post had failed — the card just wouldn't have
> come up, and I'd have tapped again thinking my thumb missed.
>
> Somebody asks "is that real or did you make it?" And here's the thing: the
> game told me my results are *signed*. That's the pitch on the box. But the
> signature is a file sitting on my phone that the link doesn't carry — and
> even if it did, I minted both halves of the crypto myself. There is nobody
> on the other end of it. I can't prove the clip. I can only post it.

Method: `share.rs` → `tape_drop` → `web/index.html` → `web.rs` → `theater.rs`,
then `attest.rs` → `net::MatchStatement` → `replay_sync::verify_attestation`.
`tape_drop` is audited as a live public HTTP service, because it is one.
`docs/GAME_DESIGN_AUDIT.md` read first; none of its six findings are repeated.

---

## Findings

| # | Finding | Severity | Evidence |
|---|---------|----------|----------|
| 1 | The rate limiter is bypassed by one forged request header | 🔴 security | `tape_drop/src/main.rs:78-91`, `ice_vendor/src/main.rs:150-153` |
| 2 | The drop is an unauthenticated public blob host — no content validation, CORS `*`, no delete | 🔴 security | `main.rs:207-252`, `:262-265` |
| 3 | One slow client stalls the whole service; GETs are unlimited | 🔴 availability | `main.rs:201`, `:215-219`, `:245-251` |
| 4 | "Dual-signed" proves far less than the docs claim; no tape binding | 🔴 crypto / doc | `net/src/lib.rs:376-386,514-523`, `replay_sync/src/lib.rs:420-458`, `docs/NORTH.md:64,68,71` |
| 5 | A tape that fetches but won't decode shows the viewer nothing | 🔴 total failure | `web/index.html:50-65`, `app/src/web.rs:87-89` |
| 6 | An ad-blocked/offline fetch kills the whole page with a raw TypeError | 🟠 bad | `web/index.html:40,50-51,66-74` |
| 7 | SHARE has no progress, no error, no retry — and a blank-screen state | 🟠 bad | `share.rs:230-241,276-279,296-314,336-340` |
| 8 | ~5.4 MB of assets ship eagerly; no `wasm-opt`, no size budget anywhere | 🟠 bad | `audio.rs:199-236`, `lib.rs:174`, `web.yml:51-57` |
| 9 | The proof never rides the share | 🟠 product gap | `share.rs:276,287`, `PLAYBOOK.md:483-484` |
| 10 | Desktop-browser viewers get a UI whose visible controls are dead | 🟠 bad | `theater.rs:774,791-816,822-863`, `input_touch/src/lib.rs:431-470` |
| 11 | The opponent's handle is published without consent and can never be revoked | 🟡 privacy | `replay/src/lib.rs:33-34`, `recorder.rs:120-145`, `main.rs:111-114,205-252` |

---

## 1. The rate limiter is bypassed by one forged header — 🔴 security

The drop's *only* abuse control is a per-IP token bucket. The IP it buckets on
is attacker-chosen.

`crates/tape_drop/src/main.rs:78-91`:

```rust
fn client_ip(request: &tiny_http::Request) -> Option<IpAddr> {
    request.headers().iter()
        .find(|h| h.field.equiv("x-forwarded-for"))
        .and_then(|h| {
            h.value.as_str().split(',').next()          // <-- LEFTMOST
                .and_then(|s| s.trim().parse().ok())
        })
        .or_else(|| request.remote_addr().map(|a| a.ip()))
}
```

`X-Forwarded-For` is built left-to-right: each proxy **appends** the address it
received from. The leftmost entry is therefore whatever the *client* put there;
the rightmost is what the edge observed. This code takes the leftmost, and only
falls back to `remote_addr` when the header is absent or unparseable — so behind
a proxy, `remote_addr` is never consulted. Rotating a synthetic leftmost value
per request gives every request a fresh full bucket.

The reasoning is recorded, and it is inverted. `crates/ice_vendor/src/main.rs:150-153`,
which `tape_drop`'s module doc explicitly says it mirrors (`main.rs:18-21`):

```rust
/// (first hop); a local or direct run falls back to the socket address.
/// XFF is client-forgeable only where clients reach the socket directly,
/// and in that deployment `remote_addr` is the truth anyway — the bucket
/// is abuse throttling, not authentication.
```

The premise is backwards. Forgery is *only* exploitable when there **is** a
proxy, because without one the header is absent and `remote_addr` wins. And
this service is deployed behind Railway's edge (`PLAYBOOK.md:464-473`,
`railway.json`), which is exactly the deployment where the header is both
present and client-supplied.

This finding is load-bearing for #2 and #3: with the bucket defeated, every
quantitative bound in this file becomes `∞`.

**Fix.** Take the **rightmost** XFF entry, or better, a configured
trusted-proxy hop count (`TRUSTED_PROXY_HOPS=1` → take the last entry;
unset → ignore XFF entirely and use `remote_addr`). The same one-line change
is needed in `ice_vendor`, and the comment above it should be corrected rather
than copied forward again.

---

## 2. The drop is an unauthenticated public blob host — 🔴 security

`POST /tape` accepts any bytes at all. `main.rs:215-231`:

```rust
let mut bytes = Vec::new();
let take = request.as_reader().take(TAPE_MAX_BYTES as u64 + 1).read_to_end(&mut bytes);
match take {
    Ok(_) if bytes.len() <= TAPE_MAX_BYTES && !bytes.is_empty() => {
        let receipt = drop.store(bytes, now);
```

The only predicates are `≤ 64 KB` and `non-empty`. Nothing checks
`replay::MAGIC` (`b"BMRG"`, `crates/replay/src/lib.rs:7`), nothing calls
`replay::decode`, nothing looks at `Content-Type`. The blob is then served back
to any origin (`main.rs:245-251, 262-265`):

```rust
Some(bytes) => (200, bytes, "application/octet-stream"),
...
response.add_header(
    tiny_http::Header::from_bytes("Access-Control-Allow-Origin", "*")...
```

So the deployed service is: an anonymous, write-once, 64 KB-per-object,
**256 MB** (`main.rs:38`) object store with 7-day retention
(`main.rs:36`), stable content-addressed IDs (`main.rs:111-114`), and
`Access-Control-Allow-Origin: *` — reachable from any web page on the
internet. Because IDs are `SHA-256[..6]` of the content, **an uploader knows
the retrieval URL before uploading**, which is the canonical shape of a covert
dead-drop. It will be found and used as free hosting on the project's own
domain, and there is **no DELETE route** (`main.rs:205-252` lists exactly three:
`GET /healthz`, `POST /tape`, `GET /tape/<id>`) — nothing can be taken down
short of restarting the container, which destroys every honest link too.

Two related gaps in the same response path:

- No `X-Content-Type-Options: nosniff` and no `Content-Disposition: attachment`
  on the served blob.
- Even for a *well-formed* tape, nothing cross-checks the header against the
  body. `replay::decode` (`crates/replay/src/lib.rs:85-95`) validates magic and
  `format_version` but never `header.frame_count == inputs.len()`, and the
  theater trusts the header field verbatim (`theater.rs:635`
  `theater.total_frames = header.frame_count;`, and `:908-913` plays until
  `cursor >= total`). A crafted tape declaring `frame_count = u32::MAX` with
  three real frames pins a viewer's browser simulating neutral inputs
  indefinitely.

**Checked and not a finding:** there is **no path traversal**. `main.rs:246`
does `let id = &path["/tape/".len()..];` and hands it to
`BTreeMap::get` (`main.rs:171-174`) — an in-memory keyspace, never a filesystem
path. `../` simply misses. Likewise the ID space is not enumerable: 48 bits of
SHA-256 is not walkable, even with the unlimited GET of #3.

**Fix.** Reject on ingest anything that isn't a decodable tape:
`replay::decode(&bytes)` (magic + `format_version`), plus
`header.frame_count as usize == inputs.len()`, plus a sane frame-count ceiling.
That single gate turns the service from "any bytes" into "this game's tapes"
and closes both the file-host and the hostile-tape vectors. Add `nosniff`.
Add an authenticated or capability-token DELETE so a share can be withdrawn
(see #11). Add a `USER` to `crates/tape_drop/Dockerfile` — it currently runs
as root.

---

## 3. One slow client stalls the service; GETs are unlimited — 🔴 availability

The server is a single sequential loop (`main.rs:201`):

```rust
for mut request in server.incoming_requests() {
```

and inside it the body is read **synchronously, in that loop**, with no
deadline (`main.rs:216-219`):

```rust
let take = request.as_reader()
    .take(TAPE_MAX_BYTES as u64 + 1)
    .read_to_end(&mut bytes);
```

`tiny_http` 0.12 never sets a socket read timeout — `grep -rn "set_read_timeout"`
over `tiny_http-0.12.0/src` returns nothing, and its own docs say "in a
real-case scenario, you will probably want to spawn multiple worker tasks"
(`tiny_http-0.12.0/src/lib.rs:38`). A client that sends
`POST /tape` with `Content-Length: 65536` and then trickles one byte a minute
holds the only request-processing thread for as long as it likes. One socket
takes the service down for everyone, including Jules's twelve viewers.

Independently: the bucket is only consulted on `POST` (`main.rs:207-208`). The
`GET /tape/<id>` arm (`main.rs:245-251`) has **no rate limit at all**, and each
hit runs a full `evict()` sweep (`main.rs:171-174` → `:128-154`, an O(n)
`retain` plus an O(n) `values().sum()`) and clones up to 64 KB. That is free
outbound bandwidth amplification against a single-threaded process.

One more availability note: the store is in-memory with
`DEFAULT_BUDGET_BYTES = 256 * 1024 * 1024` (`main.rs:38`), counting only tape
payload bytes — not the `BTreeMap` nodes, the `String` keys, the `Vec`
headers, or allocator fragmentation. On a small Railway instance the process
OOMs before the budget ever bites, `restartPolicyType: ALWAYS`
(`railway.json`) brings it back **empty**, and every live link dies. The docs
promise a week (`PLAYBOOK.md:480-481`: "Links live about a week"); the
architecture cannot promise more than "until the next deploy or crash," and the
module header is honest about that (`main.rs:8-10`) while the operator-facing
doc is not.

**Fix.** Set an explicit read/write timeout on the accepted socket (tiny_http
0.12 exposes the stream; otherwise move to a server that supports deadlines),
run a small worker pool instead of one sequential loop, rate-limit `GET` as
well as `POST`, and cache-bust `evict()` so it runs on a timer rather than per
request. Size the budget against the container's actual memory limit, or move
to disk/object storage so a restart isn't a mass link extinction.

---

## 4. "Dual-signed" proves far less than the docs claim — 🔴 crypto / doc

This is the finding Jules personally cares about, and it has three distinct
layers. Taking them honestly:

**(a) The attestation is self-referential.** `crates/net/src/lib.rs:512-523`:

```rust
pub fn verify(&self) -> bool {
    let (Some(low), Some(high)) = (sig_from_hex(&self.sig_low), sig_from_hex(&self.sig_high)) else {
        return false;
    };
    self.statement.verify(&self.statement.seat_low.pubkey, &low)
        && self.statement.verify(&self.statement.seat_high.pubkey, &high)
}
```

Both signatures are checked against public keys **carried inside the statement
being signed**. Keys are minted locally with no CA, registry, or enrollment —
`crates/app/src/profile.rs:162-170`:

```rust
fn ensure_signing_key(profile: &mut LocalProfile) -> bool {
    ...
    rand::rngs::OsRng.fill_bytes(&mut seed);
    profile.signing_key = net::hex32(&seed);
```

So: mint two keypairs, fill a `MatchStatement` with any install-ids, any
session ids, any arena, 5-0, sign it twice, and `Attestation::verify()` returns
`true`. **Nothing in the system distinguishes that from a real match.** The
attack costs one `OsRng` call.

This is a real property of the design, not a bug — but `docs/NORTH.md:68` calls
a signed result **"self-certifying"**, and `:71` goes further:

> "Ranked play, when it comes, needs only a dumb relay that collects signed
> statements; disputes are settled by resimulation."

A dumb relay collecting self-minted statements is a sybil faucet. What the
signature actually proves is narrow and should be stated that way: *"the holder
of these two keys asserts this result, and the tape's inputs re-simulate to
it."* Its only real value is **continuity** — the rivalry ledger keys on
install-id, so a rival who signed your last ten meetings with the same key is
plausibly the same rival. That is worth having. It is not "a proof."

**(b) There is no tape binding.** `MatchStatement` (`net/src/lib.rs:376-386`):

```rust
pub struct MatchStatement {
    pub magic: [u8; 4],
    pub version: u16,
    pub sim_version: u32,
    pub arena_id: u8,
    pub session_low: u128,
    pub session_high: u128,
    pub match_index: u32,
    pub seat_low: SeatStatement,
    pub seat_high: SeatStatement,
}
```

No hash of the replay. No frame count. No timestamp. `verify_attestation`
(`crates/replay_sync/src/lib.rs:420-458`) therefore binds the statement to the
tape by only three facts — `sim_version`, `arena_id`, and the re-simulated
final score:

```rust
if stmt.sim_version != replay.header.sim_version { ... }
if stmt.arena_id != replay.header.arena_id { ... }
if !attestation.verify() { ... }
let score = final_score(replay);
for (handle, replayed) in [(0u8, score.p0), (1u8, score.p1)] { ... }
```

Consequence, entirely within the system's own rules: a **genuine** attestation
for "5-2 on the Pit, sim v14" verifies against **any other** tape that
re-simulates to 5-2 on the Pit at sim v14. `replay_sync --attest` prints
`ATTESTED` (`crates/replay_sync/src/main.rs:90-101`). The claim "this
attestation describes this clip" is not what is being checked.

`docs/NORTH.md:64` says the statement contains "the deciding frame." It does
not — that field was designed and never built, and it is precisely the field
that would have made the binding tight.

**(c) Two hardening notes.**

- `MatchStatement::verify` uses `key.verify(...)` (`net/src/lib.rs:437`), not
  `verify_strict`. With ed25519-dalek 2.2.0 (`Cargo.lock`) that permits
  small-order verifying keys, which admit signatures that verify for arbitrary
  messages. Given (a) this adds nothing an attacker didn't already have, but
  the moment key provenance is fixed it becomes the next hole.
- **Canonical encoding is actually sound** — I tried to break it and could not.
  `encode()` (`net/src/lib.rs:424-426`) postcard-serializes the in-memory
  struct, and verification *re-serializes from the struct* rather than
  verifying over received bytes, so any decode ambiguity is normalized away
  before a signature is checked. Seat and session ordering are canonicalized in
  `new()` (`:398-408`) and there's a test for it (`:918-935`). No malleability
  here.

**Fix.** In order of value: (1) put `sha256(tape_bytes)` and `frame_count` in
`MatchStatement` (bump `STATEMENT_VERSION`) and check them in
`verify_attestation` — this is cheap and closes (b) completely; (2) switch to
`verify_strict`; (3) rewrite the `NORTH.md:64-72` paragraph to state what a
signature proves and what it does not, and strike the "dumb relay" sentence
until there is a key-enrollment story, because a future ranked mode will
otherwise be built on it.

---

## 5. A tape that fetches but won't decode shows the viewer nothing — 🔴

`web/index.html` sets a user-visible `notice` for exactly two failures: no drop
configured (`:35-37`) and a non-OK HTTP status (`:41-45`). It has no branch for
"the bytes arrived and the game refused them." So on a version mismatch the
page cheerfully removes the boot text and starts the game
(`web/index.html:62-65`):

```js
} else {
    boot.remove();
}
web_start(tape);
```

and the refusal lands in the console (`crates/app/src/web.rs:78-90`):

```rust
match replay::decode_for_sim_version(bytes, sim::SIM_VERSION) {
    Ok(replay) => { ... crate::theater::start_playback(world, replay); }
    Err(e) => {
        tracing::error!(target: "two_top::web", error = %e, "shared tape rejected");
    }
}
```

The viewer clicked "watch Jules's match" and got the **title screen of a game
they have never heard of**, with no explanation. Twelve people, twelve shrugs.

This is not a rare edge. `decode_for_sim_version` enforces strict equality
(`crates/replay/src/lib.rs:108-118`) with no migrations by design, and the web
theater's `SIM_VERSION` is whatever `main` last deployed to Pages
(`.github/workflows/web.yml:9-10`, `push: branches: [main]`), while the tape's
is whatever APK Jules is running. **Every `SIM_VERSION` bump silently breaks
every link already posted** — and `docs/GAME_DESIGN_AUDIT.md` #1 recommends a
bump as its top priority. Worse, links posted *after* a deploy break for anyone
still on the old APK, and vice versa. The in-app REPLAYS screen already learned
this lesson and fixed it — `theater.rs:190-199` has a whole `TapeNoticeState`
whose comment reads *"on a phone that is indistinguishable from a dead button…
the REFUSAL just becomes something the thumb can see."* The web path never got
that treatment.

Same silence for a **malformed id**: `web/index.html:33` matches
`/^#watch=([0-9a-f]{12})$/` and on no match returns `undefined` with `notice`
untouched — so a link a chat app mangled, truncated, uppercased, or appended a
tracking fragment to boots the game with no word about the tape.

**Fix.** Return a discriminated result from `web_start` (or have `watch_autoplay`
call a `wasm_bindgen` callback) so the page can say *"this clip was recorded on
an older version of the game"* / *"that link doesn't look right"*. Have the page
itself read the tape's 4-byte magic and `sim_version` before booting the wasm —
it's a fixed-offset field and would let it fail fast, before a multi-megabyte
download. Long term, keep the last N `SIM_VERSION` wasm builds at
`/<version>/app.wasm` and let `index.html` load the one the tape needs; that
turns "links rot on deploy" into "links keep working."

---

## 6. An ad-blocked or offline fetch kills the whole page — 🟠

`res.ok` (`web/index.html:41`) covers HTTP errors. It does not cover `fetch`
**rejecting**, which is what happens on DNS filtering, a blocked cross-origin
host, a captive portal, a TLS failure, or airplane mode. The `await` is inside
the outer `try` (`:50-51`), so the rejection propagates to (`:66-74`):

```js
} catch (e) {
  if (!`${e}`.includes("Using exceptions for control flow")) {
    say(`boot failed: ${e}`);
```

Result: `boot failed: TypeError: Failed to fetch`, and **the game never boots at
all**. A tape-fetch problem takes down the entire page. This is precisely the
"some with ad-blockers" case — the drop is a separate cross-origin host
(`web.yml:49` bakes `TWOTOP_DROP_URL`, a `*.up.railway.app`-class domain) and
network-level blocklists do hit domains like that.

A second, quieter one in the same file: `say("FETCHING THE TAPE…")` fires at
`:39`, and the text is not touched again until after `await init()` at `:52-53`.
The multi-megabyte wasm download therefore happens under a caption that says
the tape is being fetched, with no progress indication at all. On a bare visit
it says `SUMMONING…` for the same duration. See #8 for how long that is.

**Fix.** Wrap `watchTape()` in its own try/catch that sets `notice` and returns
`undefined`, so a dead drop degrades to "we couldn't reach the tape drop" over a
working game. Add a `say("LOADING THE GAME…")` before `import()` and, ideally,
a byte-progress readout from a streamed `fetch` of the wasm.

---

## 7. SHARE has no progress, no error, and no retry — 🟠

Jules's side is as quiet as the viewers'. Four distinct failure paths, four
different flavours of nothing.

**While posting, the screen is empty.** `share.rs:229-241` hides both labels the
moment the flow is busy:

```rust
let busy = !matches!(*state, ShareState::Idle);
for (label, mut vis) in &mut labels {
    let on = !busy && can_share.as_ref()...
```

and nothing replaces them. There is no "POSTING…" text in the module. The
`ureq` timeout is 5 s (`share.rs:154`), so that's up to five seconds of a
summary screen that looks like the tap didn't register.

**Network failure is a log line.** `share.rs:306-313`:

```rust
Ok(Err(why)) => {
    tracing::warn!(target: "two_top::share", %why, "share failed");
    *state = ShareState::Idle;
}
...
Err(std::sync::mpsc::TryRecvError::Disconnected) => {
    *state = ShareState::Idle;
}
```

The label reappears; that is the entire user-facing signal. The `Disconnected`
arm doesn't even log. **There is no retry anywhere in the module.**

**Unreadable tape is a log line.** `share.rs:276-279`.

**A QR render failure produces a blank interactive screen.** `spawn_overlay`
early-returns without spawning anything (`share.rs:335-340`):

```rust
let Some((image, side)) = qr_image(url) else {
    tracing::warn!(target: "two_top::share", "QR render failed — link is still in the log");
    tracing::info!(target: "two_top::share", %url, "watch link");
    return;
};
```

but the caller has already committed (`share.rs:302-305`):

```rust
Ok(Ok(url)) => {
    spawn_overlay(&mut commands, &mut images, &url);
    *state = ShareState::Showing;
}
```

so the app sits in `Showing` with zero overlay entities. Jules taps to dismiss
a card that was never drawn, and the link exists only in a log they cannot
reach on a phone.

**Answering the persona's questions directly:** the upload is **not** blocking —
it runs on `IoTaskPool` and is polled with `try_recv` (`share.rs:285-289`,
`:296-300`), which is right. And **no**, Jules cannot get two IDs for one match:
IDs are `SHA-256[..6]` of the bytes (`tape_drop/src/main.rs:111-114`) and a
re-share replaces and refreshes rather than duplicating (`:156-169`, tested at
`:307-318`). That part is well built.

**Fix.** Give the flow three visible states — `POSTING…`, the QR card, and a
`SHARE FAILED — TAP TO RETRY` label that re-enters `Idle` armed. Set
`ShareState::Showing` only when `spawn_overlay` actually spawned. Render the
URL as selectable text even when the QR fails.

---

## 8. Payload: 5.4 MB of assets, eagerly, with no budget — 🟠

Real numbers from the tree:

```
$ du -sh assets/            5.4M
assets/audio               4.6M      assets/sprites    304K
assets/concepts            312K      assets/arenas      96K
846764  assets/audio/title_loop.wav
756044  assets/audio/match_loop.wav
352844  assets/audio/air_{anchor,crossing,reliquary,pit,vigil,gallery,forest}.wav   × 7
```

All of it is **uncompressed WAV**, and all of it is requested at startup.
`crates/app/src/audio.rs:199-236` loads every cue eagerly, including both music
beds and all seven arenas' ambience:

```rust
fn load_audio_and_start_music(
    mut commands: Commands,
    asset_server: Res<AssetServer>, ...
    let assets = AudioAssets {
        throw: asset_server.load("audio/throw.wav"),
        ...
        title_loop: asset_server.load("audio/title_loop.wav"),
        match_loop: asset_server.load("audio/match_loop.wav"),
        air_anchor: asset_server.load("audio/air_anchor.wav"),
        ... air_forest
```

`GameAudioPlugin` is registered unconditionally (`crates/app/src/lib.rs:174`)
and `audio.rs` contains no `target_family`/`wasm` gate. On wasm that is one
HTTP fetch per file: a viewer watching a 30-second Pit clip downloads 2.4 MB of
ambience for six arenas they will never see, plus 1.6 MB of music that
`docs/plans/COMPLETION_PLAN.md:64` confirms **stays suspended until a user
gesture** and may never play at all.

On top of that sits the wasm bundle. `.github/workflows/web.yml:51-57`:

```yaml
wasm-bindgen --target web --no-typescript \
  --out-dir dist --out-name app \
  target/wasm32-unknown-unknown/release/app.wasm
...
cp -r assets dist/assets
```

There is **no `wasm-opt` step** — `grep -rniE "wasm-opt|size budget"` over all
workflows and TOMLs returns nothing but the Android note in `Cargo.toml:35-36`.
The release profile (`Cargo.toml:32-38`) sets `lto = "thin"`, `codegen-units = 1`
and `strip = "symbols"`, but no `opt-level = "z"/"s"` for wasm. `app` pulls
`bevy 0.18.1` with `bevy_render`, `bevy_post_process`, `bevy_sprite`,
`bevy_text`, `default_font` and `bevy_audio` (`crates/app/Cargo.toml:44-60`).
**SPECULATIVE** on the exact figure — no wasm artifact exists in the tree to
measure and I did not build one — but the only in-repo anchor is
`Cargo.toml:35-36`, which records the stripped aarch64 `libapp.so` at **46.6 MiB**;
a comparable wasm module without `wasm-opt` lands in the tens of megabytes
uncompressed, single-digit megabytes after Pages' gzip. What is *provable* is
that nobody has ever measured it: no size gate exists in any of the six
workflows.

Also shipped: `assets/concepts/` (312 KB of dev contact sheets) is `cp -r`'d to
the public site by `web.yml:57` and is referenced by **no Rust code**
(`grep -rn "concepts" crates/ --include=*.rs` is empty). It doesn't cost the
viewer a download, but it publishes internal art-process artifacts.

**Fix.** Add `wasm-opt -Oz` to `web.yml` and a hard size assertion in CI that
fails the build over a budget. Convert audio to OGG/Opus (Bevy's `vorbis`
feature) — that alone is roughly a 10× cut on 4.6 MB. Load ambience lazily, per
arena. Drop `assets/concepts/` from the dist copy. Serve the wasm with a
`Content-Encoding` the browser can stream.

---

## 9. The proof never rides the share — 🟠 product gap

`share.rs:276` reads exactly one file, the `.bmrg`:

```rust
let Ok(bytes) = std::fs::read(&path) else { ... };
```

and `:287` posts exactly that. `tape_drop` stores one blob per id. The
`<stem>.attest.json` that `attest.rs:223` writes beside it never leaves the
phone. `PLAYBOOK.md:483-484` is honest about it:

> "Attestations (`.attest.json` beside a tape) do not travel with the share;
> `replay_sync --attest` verifies them wherever the files are."

But read that against the pitch. `docs/NORTH.md:58` titles the pillar **"every
result is a proof"**, `README.md:97` sells "a result-signing key that dual-signs
decided matches," and `docs/NORTH.md:66` says "the result lands beside the tape
as an attestation file" — *beside* it, in the one place the tape is about to
leave without it. Pillar II (proof) and Pillar III (the tape as broadcast) do
not intersect at any point in the code. The artifact twelve people look at
carries zero attestation, and the only way to check one is to have both files
on a machine with a Rust toolchain.

Note this compounds #4(b): even if the sidecar *were* uploaded, it binds to the
tape only by sim_version + arena + score.

**Fix.** Post the attestation alongside the tape (a two-field container, or a
second `POST /attest/<id>`), have `index.html` fetch it, and put a verified
badge on the web theater — with copy that says what it actually means
("both players' devices signed this result") rather than what it doesn't.
Fixing #4(b)'s tape hash first is what makes the badge honest.

---

## 10. On a desktop browser, the theater's visible controls are dead — 🟠

Every UI surface in `app` reads Bevy's `Res<Touches>`:

```
arena_select.rs:188  profile.rs:461,484  room_code.rs:331
screen.rs:925,972,1159,1912  settings.rs:143,365
share.rs:211  theater.rs:774
```

Mouse-to-touch synthesis does exist — but it lands somewhere else.
`crates/input_touch/src/lib.rs:431-470`:

```rust
pub fn update_touch_state(
    mut state: ResMut<TouchState>,
    bevy_touches: Res<Touches>,
    mouse_btn: Res<ButtonInput<MouseButton>>, ...
    apply_touch_events(&mut state, frame,
        bevy_touches.iter_just_pressed().map(...).chain(mouse_pressed_iter), ...
```

It writes `TouchState`, input_touch's own resource for the virtual stick. It
does not write `Touches`. So the UI layer — including the theater — never sees
a mouse click.

The consequence for a laptop viewer: `theater.rs:822-863` draws and hit-tests
the scrub strip, the speed pips and the top exit strip entirely through
`touches.iter_just_pressed()` / `touches.iter()`. All three are rendered and
**none of them respond to the mouse**. What's left is the keyboard
(`theater.rs:791-816`): Space to pause, Escape to leave, Home/End, and
ArrowLeft/Right which seek **one frame** — on an 1800-frame tape
(`wasm.yml:58` uses 1800 as the canonical length) that is not a scrub. No speed
control at all, since the pips are touch-only. Nothing on screen says any of
this.

And Escape is a trapdoor: `theater.rs:791-793` sets
`AppScreen::Replays` — on the web that is the on-device tape list, which in a
browser is empty, and its own BACK affordance (`theater.rs:48-49`,
`BACK_BAND`) is touch-only too.

For completeness of the viewer experience: when the tape *does* reach its end it
simply pauses on the last frame (`theater.rs:908-913`, "hold on the last frame").
There is no "watch again," no "get the game," no link back to Jules's match —
the single best moment to convert twelve curious people is a frozen frame.

**Fix.** Synthesize mouse presses into `Touches`, or add a shared
`PointerTaps` resource that both touch and mouse feed and every UI module reads
(`share.rs` and `theater.rs` first). Independently, give the web theater a
one-line control hint and an end-of-tape card with REPLAY and a link to the
game.

---

## 11. The opponent's handle is published without consent, and can't be revoked — 🟡

What leaves the phone on a share is the `.bmrg`, whose header is
(`crates/replay/src/lib.rs:24-36`):

```rust
pub struct ReplayHeader {
    ... pub recorded_at: u64,
    pub winner: Option<u8>,
    pub player_handles: [Option<String>; 2],
    pub arena_id: u8,
}
```

`player_handles` is filled with **both** players' names
(`crates/app/src/recorder.rs:135-144`):

```rust
let mut names = [None, None];
let me = local.unwrap_or(0);
names[me] = Some(profile.name_string());
if let Some(peer) = peer.0 {
    names[1 - me] = Some(crate::profile::name_from_slots(&peer.name));
}
```

and the theater puts them on the marquee (`theater.rs:637`,
`theater.names = header.player_handles.clone()`).

**The good news, stated plainly:** install-id and public key do **not** ride the
tape — the header has no field for either, and I checked. That is the right
call and it should stay that way.

**The gap:** Jules's opponent chose a handle for a 1v1 game and it is now on a
public URL forwarded through a group chat, with no consent step anywhere in the
share flow (`share.rs:253-294` goes straight from tap to upload) and **no way
to take it down** — `tape_drop` has no DELETE (`main.rs:205-252`) and the id is
content-addressed (`main.rs:111-114`), so re-uploading can never rotate to a new
link either. `recorded_at` (`recorder.rs:208-211`) publishes the wall-clock
second the match ended.

Severity is 🟡 rather than 🟠 because the handle is 4 glyphs from a 36-character
alphabet (`profile.rs:44-47`) — a gamertag, not a name. One caveat worth
recording: an install that never dialed a name carries a **default** derived
deterministically from its install-id (`profile.rs:238-245`):

```rust
let idx = ((install_id >> (i * 8)) as u8) as usize % NAME_ALPHABET.len();
```

so across several of Jules's posted clips, an unnamed opponent's tapes are
weakly linkable to one device. Four bytes mod 36 is a coarse fingerprint, not an
identifier — but it is more than zero, and it's free to fix.

**Fix.** Show what will be published before uploading ("this clip will show
CURS vs STAG") with a one-tap **share anonymously** that blanks
`player_handles`. Round `recorded_at` to the day or drop it. Add a
capability-token DELETE so a share can be pulled. Make `default_name` a function
of a random per-install nonce rather than the install-id itself.

---

## Minor doc drift noted in passing

- `CLAUDE.md:7` lists "`#watch=<id>` playback verified headless" among the
  shipped share-loop items. That verification was a one-off manual screenshot
  (`docs/plans/COMPLETION_PLAN.md:64`, "verified headless with a screenshot of
  the Anchor mid-countdown"). The only CI wasm lane is `wasm.yml`, which runs
  `checksum_golden_probe` (`crates/app/src/web.rs:37-62`) under
  `--enable-unsafe-swiftshader` and never touches `index.html`, the drop fetch,
  the renderer, or the theater. **The `#watch=` path has no regression test.**
- `README.md:176` says "wasm32 is the fifth [lane] and is **not yet** in the
  matrix" while `CLAUDE.md:7` calls the wasm checksum lane shipped. `wasm.yml`
  exists and runs on every push; the README is stale.
- `PLAYBOOK.md:480-481` promises "Links live about a week" against an
  in-memory store on `restartPolicyType: ALWAYS` (see #3).

## Checked and sound

- **No path traversal** in `tape_drop` — the id keys a `BTreeMap`, not a file
  (`main.rs:171-174, 246`).
- **IDs are not enumerable** — 48 bits of SHA-256, not walkable.
- **Canonical encoding is genuinely canonical** — verification re-encodes from
  the struct, so decode ambiguity can't reach a signature (`net/src/lib.rs:424-439`);
  seat/session ordering is normalized in `new()` and tested (`:918-935`).
- **Re-sharing is idempotent** — content addressing means one match, one link
  (`main.rs:156-169`, tested `:307-318`).
- **Upload is non-blocking** — `IoTaskPool` + `try_recv` (`share.rs:285-300`).
- **Malformed `#watch` ids are strictly rejected** by the regex
  (`web/index.html:33`) — the problem is the silence afterwards, not the
  validation.
- **Postcard decode of a hostile tape won't allocation-bomb** — serde's
  cautious size hint plus the 64 KB ingest cap bound it.
- **`ArenaId::from_u8` is total** (`sim/src/lib.rs:3640-3650`) and
  `playback_inputs_system` bounds-checks its cursor
  (`replay/src/lib.rs:188-193`) — a crafted arena byte or short input vector
  can't panic the viewer.

## Recommended order

1. **#1** — one line, and every other server bound depends on it.
2. **#2** ingest validation + **#3** timeouts — together they turn an open
   file host into a tape relay.
3. **#5** — the twelve-viewer failure that will happen on the very next
   `SIM_VERSION` bump, which `GAME_DESIGN_AUDIT` #1 is already asking for.
4. **#4(b)** tape hash in the statement, and the `NORTH.md:64-72` rewrite.
   Cheap, and it stops the next feature being built on a claim that doesn't hold.
5. **#7** and **#6** — the two silences, one on each end of the link.
6. **#8**, **#10**, **#11**, **#9** — the polish that decides whether twelve
   clicks become twelve players.
