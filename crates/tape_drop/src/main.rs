//! The tape drop — persistent-enough links for shared matches.
//!
//! A clutch comeback is a 14 KB file, and a link that dies when the phone
//! sleeps is not worth sending. This service holds tapes in memory with a
//! TTL: `POST /tape` stores a `.bmrg` under the first 12 hex chars of its
//! SHA-256 (content-addressed: re-sharing the same match re-mints the same
//! id), `GET /tape/<id>` hands the bytes to the web theater, and the whole
//! store evicts on expiry and on a hard byte budget. Restarting the
//! container empties it; a tape drop is a relay, not an archive — the tape
//! itself lives on the phones that played it.
//!
//! One resource, a tape, sealed or open (live):
//!
//! * `POST /tape` (body ≤ [`TAPE_MAX_BYTES`]) → `{"id", "expires_secs"}`:
//!   a sealed tape.
//! * `POST /tape?live=1` → `{"id", "token"}`: an open tape, empty.
//! * `PUT /tape/<id>/<seq>` (`X-Live-Token`) appends chunk `seq`;
//!   `POST /tape/<id>/end` closes it.
//! * `GET /tape/<id>?from=<byte>` → the bytes from there. On an open tape
//!   the reply says `X-Live-Open: 1`, and a reader already caught up is
//!   held until the next chunk lands (a long poll).
//! * `POST /results` / `GET /results?since=<unix>`: the ladder's posted
//!   dual-signed results (see `relay.rs`).
//! * `GET /healthz` → `ok`.
//!
//! Env: `PORT` (Railway convention), `TAPE_TTL_SECS` (default 7 days),
//! `TAPE_BUDGET_BYTES` (default 256 MiB), `TAPE_DIR` (optional: sealed
//! tapes are also written there and reloaded on restart — a Railway
//! volume makes links survive a redeploy).
//!
//! Abuse posture mirrors ice_vendor: a per-IP token bucket (same
//! constants, same X-Forwarded-For reasoning — see
//! `crates/ice_vendor/src/main.rs`), sized so a human sharing a few
//! matches never sees a 429 and a scripted flood rents nothing.

mod relay;

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read as _;
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// One tape's ceiling. The canonical full match is ~14 KB; a long
/// rematch-chain tape a few times that. 64 KB is generous headroom and a
/// hard wall against using the drop as a file host.
const TAPE_MAX_BYTES: usize = 64 * 1024;
/// Default life of a link: a week. Long enough to send tonight's comeback
/// around tomorrow; short enough that the store curates itself.
const DEFAULT_TTL_SECS: u64 = 7 * 24 * 3600;
/// Default total byte budget. At 64 KB a tape this holds ~4000 tapes.
const DEFAULT_BUDGET_BYTES: usize = 256 * 1024 * 1024;

// Token bucket, in lockstep with ice_vendor's (same rationale, same
// constants; the duplication is 40 lines and a shared crate for it would
// be ceremony).
const BUCKET_CAPACITY: u32 = 5;
const BUCKET_REFILL_SECS: u64 = 30;
const BUCKET_SWEEP_LEN: usize = 10_000;

/// The read class: a viewer of a live tape asks about once a second, and a
/// long poll holds it most of that.
const READ_CAPACITY: u32 = 120;
const READ_REFILL_SECS: u64 = 1;

struct RateLimiter {
    buckets: BTreeMap<IpAddr, (u32, Instant)>,
    capacity: u32,
    refill_secs: u64,
}

impl RateLimiter {
    fn new(capacity: u32, refill_secs: u64) -> Self {
        Self {
            buckets: BTreeMap::new(),
            capacity,
            refill_secs,
        }
    }

    fn allow_at(&mut self, ip: IpAddr, now: Instant) -> bool {
        if self.buckets.len() > BUCKET_SWEEP_LEN {
            let stale = Duration::from_secs(self.refill_secs * self.capacity as u64);
            self.buckets
                .retain(|_, (_, at)| now.duration_since(*at) < stale);
        }
        let (tokens, last) = self.buckets.entry(ip).or_insert((self.capacity, now));
        let refilled = now.duration_since(*last).as_secs() / self.refill_secs;
        if refilled > 0 {
            *tokens = (*tokens + refilled as u32).min(self.capacity);
            *last = now;
        }
        if *tokens == 0 {
            return false;
        }
        *tokens -= 1;
        true
    }
}

/// How many proxies in front of us are trusted to have appended to
/// `X-Forwarded-For`. Railway's edge is one hop, so `1` is the default;
/// a direct or local run sets `TRUSTED_PROXY_HOPS=0` to ignore the header
/// outright, because there the socket address is the truth and any XFF
/// present was typed by the client.
fn trusted_proxy_hops() -> usize {
    std::env::var("TRUSTED_PROXY_HOPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
}

/// The address the bucket keys on. Each proxy APPENDS the address it
/// accepted the connection from, so after `trusted_hops` trusted proxies
/// the rightmost `trusted_hops` entries are theirs and the client sits at
/// index `len - trusted_hops` — for one hop, the LAST entry. Everything to
/// the left of that was supplied by the client and is worth nothing:
/// taking the leftmost entry (the previous code) handed every caller a
/// fresh bucket per forged header, behind exactly the proxy that made the
/// header trustworthy in the first place. A header with fewer entries than
/// trusted hops is malformed for this deployment and falls back to the
/// socket address, as does an absent header.
fn forwarded_ip(xff: Option<&str>, remote: Option<IpAddr>, trusted_hops: usize) -> Option<IpAddr> {
    if trusted_hops == 0 {
        return remote;
    }
    let from_header = xff.and_then(|v| {
        let entries: Vec<&str> = v.split(',').map(str::trim).collect();
        let idx = entries.len().checked_sub(trusted_hops)?;
        entries[idx].parse().ok()
    });
    from_header.or(remote)
}

fn client_ip(request: &tiny_http::Request, trusted_hops: usize) -> Option<IpAddr> {
    let xff = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("x-forwarded-for"))
        .map(|h| h.value.as_str());
    forwarded_ip(xff, request.remote_addr().map(|a| a.ip()), trusted_hops)
}

/// The store: id → (bytes, expiry). BTreeMap keeps eviction scans
/// deterministic and the dependency count at zero.
struct Drop_ {
    tapes: BTreeMap<String, (Vec<u8>, Instant)>,
    stored_bytes: usize,
    ttl: Duration,
    budget: usize,
}

#[derive(Serialize)]
struct DropReceipt {
    id: String,
    expires_secs: u64,
}

/// A tape's id: the first 12 hex chars of its SHA-256. Content-addressed,
/// so re-sharing the same match re-mints the same link, and 48 bits is
/// collision-safe at "a few thousand live tapes".
fn tape_id(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest[..6].iter().map(|b| format!("{b:02x}")).collect()
}

impl Drop_ {
    fn new(ttl: Duration, budget: usize) -> Self {
        Self {
            tapes: BTreeMap::new(),
            stored_bytes: 0,
            ttl,
            budget,
        }
    }

    /// Drop everything expired; then, while over budget, drop whatever
    /// expires soonest — the links closest to dying anyway.
    fn evict(&mut self, now: Instant) {
        let before = self.tapes.len();
        self.tapes.retain(|_, (bytes, expiry)| {
            let live = *expiry > now;
            if !live {
                // retain runs per entry; the running total adjusts below.
                let _ = bytes;
            }
            live
        });
        if self.tapes.len() != before {
            self.stored_bytes = self.tapes.values().map(|(b, _)| b.len()).sum();
        }
        while self.stored_bytes > self.budget {
            let Some(soonest) = self
                .tapes
                .iter()
                .min_by_key(|(_, (_, expiry))| *expiry)
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            if let Some((bytes, _)) = self.tapes.remove(&soonest) {
                self.stored_bytes -= bytes.len();
            }
        }
    }

    fn store(&mut self, bytes: Vec<u8>, now: Instant) -> DropReceipt {
        self.evict(now);
        let id = tape_id(&bytes);
        let expiry = now + self.ttl;
        if let Some((old, _)) = self.tapes.insert(id.clone(), (bytes, expiry)) {
            // Same content re-shared: replace, refresh the clock.
            self.stored_bytes -= old.len();
        }
        self.stored_bytes += self.tapes.get(&id).map(|(b, _)| b.len()).unwrap_or(0);
        DropReceipt {
            id,
            expires_secs: self.ttl.as_secs(),
        }
    }

    fn get(&mut self, id: &str, now: Instant) -> Option<Vec<u8>> {
        self.evict(now);
        self.tapes.get(id).map(|(bytes, _)| bytes.clone())
    }
}

/// A postcard `ReplayHeader` starts with its `[u8; 4]` magic unprefixed,
/// so the first four bytes of every tape are literally `BMRG` — enough to
/// turn "any bytes up to 64 KB, served back to any origin" into "this
/// game's tapes" without pulling the codec (and Bevy) into a 200-line
/// server. Not a validation of the tape; the theater does that, on
/// screen, when it plays.
///
/// The length floor only rejects a bare magic probe. It is deliberately
/// low: an empty dev tape (no inputs, no handles, zero timestamp) varints
/// down to about fifteen bytes, so a floor near that size could refuse a
/// legitimate tape at upload — a worse failure than storing a short one.
const TAPE_MIN_BYTES: usize = 8;

fn looks_like_a_tape(bytes: &[u8]) -> bool {
    bytes.len() >= TAPE_MIN_BYTES && bytes.starts_with(b"BMRG")
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Everything the workers share. Readers of an open live tape wait on
/// `grew`, which every append and end notifies.
struct Shared {
    state: std::sync::Mutex<State>,
    grew: std::sync::Condvar,
}

struct State {
    drop: Drop_,
    lives: relay::Lives,
    results: relay::Results,
    /// Uploads, opens and result posts: a human's few a minute.
    writes: RateLimiter,
    /// Reads: a viewer polls an open tape about once a second.
    reads: RateLimiter,
    dir: Option<std::path::PathBuf>,
}

/// A request reduced to what routing needs, so routes test without sockets.
struct Req<'a> {
    method: tiny_http::Method,
    path: &'a str,
    query: &'a str,
    token: Option<&'a str>,
    ip: Option<IpAddr>,
    body: Vec<u8>,
}

struct Resp {
    status: u16,
    body: Vec<u8>,
    content_type: &'static str,
    /// For a live read: whether more may come (`X-Live-Open`).
    live_open: Option<bool>,
}

impl Resp {
    fn json(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            body: body.into(),
            content_type: "application/json",
            live_open: None,
        }
    }
    fn err(status: u16, msg: &str) -> Self {
        Self::json(status, format!("{{\"error\":\"{msg}\"}}"))
    }
}

/// How long a reader caught up with an open live tape is held for the next
/// chunk before an empty reply (it simply asks again).
const LONG_POLL: Duration = Duration::from_secs(20);

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Route one request. Pure but for the clock, the disk store and the
/// long-poll wait.
fn handle(shared: &Shared, req: Req<'_>, now: Instant) -> Resp {
    use tiny_http::Method::{Get, Options, Post, Put};
    let parts: Vec<&str> = req.path.trim_start_matches('/').split('/').collect();
    let mut st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
    let limited = |bucket: &mut RateLimiter| !req.ip.is_none_or(|ip| bucket.allow_at(ip, now));
    match (&req.method, parts.as_slice()) {
        (Options, _) => Resp {
            status: 204,
            body: Vec::new(),
            content_type: "text/plain",
            live_open: None,
        },
        (Get, ["healthz"]) => Resp {
            status: 200,
            body: b"ok".to_vec(),
            content_type: "text/plain",
            live_open: None,
        },
        // A live tape opens empty; its owner holds the token.
        (Post, ["tape"]) if req.query.split('&').any(|kv| kv == "live=1") => {
            if limited(&mut st.writes) {
                return Resp::err(429, "slow down");
            }
            let id = relay::random_hex(6);
            let token = relay::random_hex(16);
            st.lives.open(id.clone(), token.clone(), now);
            Resp::json(200, format!("{{\"id\":\"{id}\",\"token\":\"{token}\"}}"))
        }
        (Post, ["tape"]) => {
            if limited(&mut st.writes) {
                return Resp::err(429, "slow down");
            }
            if req.body.len() > TAPE_MAX_BYTES {
                return Resp::err(413, "tape too large");
            }
            if !looks_like_a_tape(&req.body) {
                return Resp::err(400, "not a tape");
            }
            let dir = st.dir.clone();
            let receipt = st.drop.store(req.body.clone(), now);
            if let Some(dir) = dir {
                let _ = std::fs::write(dir.join(format!("{}.bmrg", receipt.id)), &req.body);
            }
            match serde_json::to_string(&receipt) {
                Ok(json) => Resp::json(200, json),
                Err(_) => Resp::err(500, "receipt"),
            }
        }
        (Put, ["tape", id, seq]) => {
            let Ok(seq) = seq.parse::<u32>() else {
                return Resp::err(400, "bad seq");
            };
            let token = req.token.unwrap_or("");
            let out = st.lives.append(id, token, seq, &req.body, now);
            drop(st);
            shared.grew.notify_all();
            match out {
                Ok(len) => Resp::json(200, format!("{{\"len\":{len}}}")),
                Err(e) => append_error(e),
            }
        }
        (Post, ["tape", id, "end"]) => {
            let out = st.lives.end(id, req.token.unwrap_or(""));
            drop(st);
            shared.grew.notify_all();
            match out {
                Ok(()) => Resp::json(200, "{}"),
                Err(e) => append_error(e),
            }
        }
        (Get, ["tape", id]) => {
            if limited(&mut st.reads) {
                return Resp::err(429, "slow down");
            }
            let from = relay::query_from(req.query);
            if let Some(bytes) = st.drop.get(id, now) {
                let from = from.min(bytes.len());
                return Resp {
                    status: 200,
                    body: bytes[from..].to_vec(),
                    content_type: "application/octet-stream",
                    live_open: None,
                };
            }
            let deadline = now + LONG_POLL;
            loop {
                let Some((bytes, open)) = st.lives.read(id, from, now) else {
                    return Resp {
                        status: 404,
                        body: b"not found".to_vec(),
                        content_type: "text/plain",
                        live_open: None,
                    };
                };
                let waited = Instant::now();
                if !bytes.is_empty() || !open || waited >= deadline {
                    return Resp {
                        status: 200,
                        body: bytes,
                        content_type: "application/octet-stream",
                        live_open: Some(open),
                    };
                }
                st = shared
                    .grew
                    .wait_timeout(st, deadline - waited)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
        }
        (Post, ["results"]) => {
            if limited(&mut st.writes) {
                return Resp::err(429, "slow down");
            }
            let Some(tape) = relay::parse_result_tape(&req.body) else {
                return Resp::err(400, "not a result");
            };
            // The result is the one the tape carries, and the tape is one
            // the relay holds: the ladder can re-simulate it and check it.
            let Some(bytes) = st.drop.get(&tape, now) else {
                return Resp::err(404, "no such tape");
            };
            let Some(attestation) = relay::seal_of(&bytes) else {
                return Resp::err(422, "that tape carries no signed result");
            };
            st.results.post(relay::ResultRow {
                posted_unix: unix_now(),
                tape,
                attestation,
            });
            Resp::json(200, "{}")
        }
        (Get, ["results"]) => {
            if limited(&mut st.reads) {
                return Resp::err(429, "slow down");
            }
            let rows = st.results.since(relay::query_since(req.query));
            match serde_json::to_vec(&rows) {
                Ok(json) => Resp::json(200, json),
                Err(_) => Resp::err(500, "results"),
            }
        }
        _ => Resp {
            status: 404,
            body: b"not found".to_vec(),
            content_type: "text/plain",
            live_open: None,
        },
    }
}

fn append_error(e: relay::AppendError) -> Resp {
    use relay::AppendError as E;
    match e {
        E::NoSuchTape => Resp::err(404, "no such live tape"),
        E::BadToken => Resp::err(403, "not your tape"),
        E::OutOfOrder { expected } => Resp::json(
            409,
            format!("{{\"error\":\"out of order\",\"expected\":{expected}}}"),
        ),
        E::Closed => Resp::err(410, "ended"),
        E::TooLarge => Resp::err(413, "too large"),
    }
}

/// Tapes on disk from an earlier run come back with a fresh TTL.
fn load_dir(drop: &mut Drop_, dir: &std::path::Path, now: Instant) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "bmrg")
            && let Ok(bytes) = std::fs::read(&path)
            && looks_like_a_tape(&bytes)
            && bytes.len() <= TAPE_MAX_BYTES
        {
            drop.store(bytes, now);
        }
    }
}

/// Files whose tape the store has evicted go too, so the volume curates
/// itself the way memory does.
fn sweep_dir(drop: &Drop_, dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if path.extension().is_some_and(|e| e == "bmrg") && !drop.tapes.contains_key(stem) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

fn header<'a>(request: &'a tiny_http::Request, name: &'static str) -> Option<&'a str> {
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str())
}

fn serve(shared: &Shared, server: &tiny_http::Server, trusted_hops: usize) {
    loop {
        let Ok(mut request) = server.recv() else {
            continue;
        };
        let now = Instant::now();
        let url = request.url().to_string();
        let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));
        let mut body = Vec::new();
        let read = request
            .as_reader()
            .take(TAPE_MAX_BYTES.max(relay::CHUNK_MAX_BYTES) as u64 + 1)
            .read_to_end(&mut body);
        let resp = if read.is_err() {
            Resp::err(400, "unreadable body")
        } else {
            let req = Req {
                method: request.method().clone(),
                path,
                query,
                token: header(&request, "x-live-token"),
                ip: client_ip(&request, trusted_hops),
                body,
            };
            handle(shared, req, now)
        };
        let h = |k: &str, v: &str| tiny_http::Header::from_bytes(k, v).expect("static header");
        let mut response = tiny_http::Response::from_data(resp.body)
            .with_status_code(resp.status)
            .with_header(h("Content-Type", resp.content_type));
        // The web theater and the ladder live on another origin (the
        // static page host). Live appends carry a custom header and PUT,
        // so they preflight; the preflight is answered above.
        for (k, v) in [
            ("Access-Control-Allow-Origin", "*"),
            ("Access-Control-Allow-Methods", "GET, POST, PUT, OPTIONS"),
            ("Access-Control-Allow-Headers", "Content-Type, X-Live-Token"),
            ("Access-Control-Expose-Headers", "X-Live-Open"),
            // A served blob is a blob: never let a browser sniff it into a page.
            ("X-Content-Type-Options", "nosniff"),
        ] {
            response.add_header(h(k, v));
        }
        if let Some(open) = resp.live_open {
            response.add_header(h("X-Live-Open", if open { "1" } else { "0" }));
        }
        let _ = request.respond(response);
    }
}

/// Workers: enough that readers held in a long poll never starve an upload.
const WORKERS: usize = 16;

fn main() {
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8081);
    let ttl = Duration::from_secs(env_u64("TAPE_TTL_SECS", DEFAULT_TTL_SECS));
    let budget = env_u64("TAPE_BUDGET_BYTES", DEFAULT_BUDGET_BYTES as u64) as usize;
    let dir = std::env::var("TAPE_DIR").ok().map(std::path::PathBuf::from);
    let now = Instant::now();
    let mut drop = Drop_::new(ttl, budget);
    if let Some(dir) = &dir {
        let _ = std::fs::create_dir_all(dir);
        load_dir(&mut drop, dir, now);
    }
    let shared = std::sync::Arc::new(Shared {
        state: std::sync::Mutex::new(State {
            drop,
            lives: relay::Lives::default(),
            results: relay::Results::default(),
            writes: RateLimiter::new(BUCKET_CAPACITY, BUCKET_REFILL_SECS),
            reads: RateLimiter::new(READ_CAPACITY, READ_REFILL_SECS),
            dir: dir.clone(),
        }),
        grew: std::sync::Condvar::new(),
    });
    let trusted_hops = trusted_proxy_hops();
    let server = std::sync::Arc::new(
        tiny_http::Server::http(("0.0.0.0", port))
            .unwrap_or_else(|e| panic!("tape_drop: cannot bind port {port}: {e}")),
    );
    println!(
        "tape_drop: listening on :{port}, ttl = {}s, budget = {} bytes, dir = {:?}",
        ttl.as_secs(),
        budget,
        dir,
    );
    if let Some(dir) = dir {
        let shared = shared.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(600));
                let mut st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                st.drop.evict(Instant::now());
                sweep_dir(&st.drop, &dir);
            }
        });
    }
    let workers: Vec<_> = (0..WORKERS)
        .map(|_| {
            let (shared, server) = (shared.clone(), server.clone());
            std::thread::spawn(move || serve(&shared, &server, trusted_hops))
        })
        .collect();
    for w in workers {
        let _ = w.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drop_with(ttl_secs: u64, budget: usize) -> Drop_ {
        Drop_::new(Duration::from_secs(ttl_secs), budget)
    }

    #[test]
    fn ids_are_content_addressed_and_stable() {
        let a = tape_id(b"the same tape");
        assert_eq!(a, tape_id(b"the same tape"), "same bytes, same link");
        assert_eq!(a.len(), 12);
        assert_ne!(a, tape_id(b"a different tape"));
    }

    #[test]
    fn a_stored_tape_round_trips_until_its_ttl() {
        let mut drop = drop_with(60, 1 << 20);
        let t0 = Instant::now();
        let receipt = drop.store(b"match bytes".to_vec(), t0);
        assert_eq!(
            drop.get(&receipt.id, t0).as_deref(),
            Some(&b"match bytes"[..])
        );
        // One second before expiry: alive. One past: gone.
        assert!(
            drop.get(&receipt.id, t0 + Duration::from_secs(59))
                .is_some()
        );
        assert!(
            drop.get(&receipt.id, t0 + Duration::from_secs(61))
                .is_none()
        );
        assert_eq!(drop.stored_bytes, 0, "eviction reclaims the budget");
    }

    #[test]
    fn resharing_the_same_match_refreshes_instead_of_duplicating() {
        let mut drop = drop_with(60, 1 << 20);
        let t0 = Instant::now();
        let first = drop.store(b"same".to_vec(), t0);
        let later = t0 + Duration::from_secs(50);
        let second = drop.store(b"same".to_vec(), later);
        assert_eq!(first.id, second.id);
        assert_eq!(drop.stored_bytes, 4, "one copy, not two");
        // The re-share reset the clock: alive past the FIRST expiry.
        assert!(drop.get(&first.id, t0 + Duration::from_secs(70)).is_some());
    }

    #[test]
    fn the_budget_evicts_the_soonest_dying_links_first() {
        let mut drop = drop_with(1000, 25);
        let t0 = Instant::now();
        let old = drop.store(vec![1u8; 10], t0);
        let newer = drop.store(vec![2u8; 10], t0 + Duration::from_secs(10));
        // A third 10-byte tape breaches the 25-byte budget: the entry
        // expiring soonest (the oldest share) goes first.
        let third = drop.store(vec![3u8; 10], t0 + Duration::from_secs(20));
        let now = t0 + Duration::from_secs(21);
        assert!(drop.get(&old.id, now).is_none(), "oldest link died early");
        assert!(drop.get(&newer.id, now).is_some());
        assert!(drop.get(&third.id, now).is_some());
        assert!(drop.stored_bytes <= 25);
    }

    #[test]
    fn the_bucket_is_ice_vendors() {
        let mut limiter = RateLimiter::new(BUCKET_CAPACITY, BUCKET_REFILL_SECS);
        let ip: IpAddr = "203.0.113.9".parse().unwrap();
        let t0 = Instant::now();
        for _ in 0..BUCKET_CAPACITY {
            assert!(limiter.allow_at(ip, t0));
        }
        assert!(!limiter.allow_at(ip, t0));
        assert!(limiter.allow_at(ip, t0 + Duration::from_secs(BUCKET_REFILL_SECS)));
    }
}

#[cfg(test)]
mod forwarded_ip_tests {
    use super::forwarded_ip;
    use std::net::IpAddr;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn one_trusted_hop_takes_the_entry_the_edge_appended_not_the_forged_one() {
        // A client behind Railway sends a forged leftmost entry; the edge
        // appends what it actually saw. The bucket must key on the latter.
        let got = forwarded_ip(Some("1.2.3.4, 203.0.113.9"), Some(ip("10.0.0.1")), 1);
        assert_eq!(got, Some(ip("203.0.113.9")));
        // Rotating the forged entry does not rotate the bucket.
        let again = forwarded_ip(Some("5.6.7.8, 203.0.113.9"), Some(ip("10.0.0.1")), 1);
        assert_eq!(again, got);
    }

    #[test]
    fn zero_trusted_hops_ignores_the_header_entirely() {
        let got = forwarded_ip(Some("1.2.3.4"), Some(ip("198.51.100.7")), 0);
        assert_eq!(got, Some(ip("198.51.100.7")));
    }

    #[test]
    fn absent_or_short_header_falls_back_to_the_socket() {
        assert_eq!(
            forwarded_ip(None, Some(ip("198.51.100.7")), 1),
            Some(ip("198.51.100.7"))
        );
        // Two trusted hops but only one entry: malformed for this
        // deployment, so the socket address wins over a guess.
        assert_eq!(
            forwarded_ip(Some("203.0.113.9"), Some(ip("198.51.100.7")), 2),
            Some(ip("198.51.100.7"))
        );
        assert_eq!(
            forwarded_ip(Some("garbage"), Some(ip("198.51.100.7")), 1),
            Some(ip("198.51.100.7"))
        );
    }

    #[test]
    fn two_trusted_hops_reach_past_the_inner_proxy() {
        let got = forwarded_ip(Some("forged, 203.0.113.9, 10.1.1.1"), None, 2);
        assert_eq!(got, Some(ip("203.0.113.9")));
    }
}

#[cfg(test)]
mod ingest_tests {
    use super::looks_like_a_tape;

    #[test]
    fn only_bytes_that_start_like_a_tape_are_stored() {
        let mut tape = b"BMRG".to_vec();
        tape.extend_from_slice(&[1u8; 40]);
        assert!(looks_like_a_tape(&tape));
        assert!(!looks_like_a_tape(b"BMRG"), "a bare magic is not a tape");
        // A minimal dev tape (empty inputs, no handles) must still pass.
        assert!(looks_like_a_tape(b"BMRG\x01\x00\x0e\x00\x02\x3c\x00"));
        assert!(!looks_like_a_tape(b"GIF89a....................."));
        assert!(!looks_like_a_tape(&[0u8; 64]));
        assert!(!looks_like_a_tape(b""));
    }
}

#[cfg(test)]
mod route_tests {
    use super::*;
    use tiny_http::Method;

    fn shared() -> Shared {
        Shared {
            state: std::sync::Mutex::new(State {
                drop: Drop_::new(Duration::from_secs(60), 1 << 20),
                lives: relay::Lives::default(),
                results: relay::Results::default(),
                writes: RateLimiter::new(BUCKET_CAPACITY, BUCKET_REFILL_SECS),
                reads: RateLimiter::new(READ_CAPACITY, READ_REFILL_SECS),
                dir: None,
            }),
            grew: std::sync::Condvar::new(),
        }
    }

    fn req<'a>(
        method: Method,
        path: &'a str,
        query: &'a str,
        token: Option<&'a str>,
        body: &[u8],
    ) -> Req<'a> {
        Req {
            method,
            path,
            query,
            token,
            ip: None,
            body: body.to_vec(),
        }
    }

    fn json(resp: &Resp) -> serde_json::Value {
        serde_json::from_slice(&resp.body).unwrap()
    }

    #[test]
    fn a_live_tape_is_opened_appended_read_and_ended_through_one_resource() {
        let s = shared();
        let t0 = Instant::now();
        let opened = handle(&s, req(Method::Post, "/tape", "live=1", None, b""), t0);
        assert_eq!(opened.status, 200);
        let v = json(&opened);
        let (id, token) = (v["id"].as_str().unwrap(), v["token"].as_str().unwrap());
        let path0 = format!("/tape/{id}/0");
        let path1 = format!("/tape/{id}/1");
        let get = format!("/tape/{id}");
        assert_eq!(
            handle(&s, req(Method::Put, &path0, "", Some("nope"), b"BMRG"), t0).status,
            403
        );
        assert_eq!(
            handle(&s, req(Method::Put, &path0, "", Some(token), b"BMRG"), t0).status,
            200
        );
        assert_eq!(
            handle(&s, req(Method::Put, &path1, "", Some(token), b"1234"), t0).status,
            200
        );
        let read = handle(&s, req(Method::Get, &get, "from=4", None, b""), t0);
        assert_eq!(
            (read.body.as_slice(), read.live_open),
            (&b"1234"[..], Some(true))
        );
        let end = format!("/tape/{id}/end");
        assert_eq!(
            handle(&s, req(Method::Post, &end, "", Some(token), b""), t0).status,
            200
        );
        // Caught up with an ended tape: an empty reply at once, no wait.
        let done = handle(&s, req(Method::Get, &get, "from=8", None, b""), t0);
        assert_eq!((done.body.len(), done.live_open), (0, Some(false)));
    }

    #[test]
    fn a_caught_up_reader_is_woken_by_the_next_chunk() {
        let s = std::sync::Arc::new(shared());
        let t0 = Instant::now();
        let v = json(&handle(
            &s,
            req(Method::Post, "/tape", "live=1", None, b""),
            t0,
        ));
        let (id, token) = (
            v["id"].as_str().unwrap().to_string(),
            v["token"].as_str().unwrap().to_string(),
        );
        let reader = {
            let (s, id) = (s.clone(), id.clone());
            std::thread::spawn(move || {
                let path = format!("/tape/{id}");
                handle(
                    &s,
                    req(Method::Get, &path, "from=0", None, b""),
                    Instant::now(),
                )
            })
        };
        std::thread::sleep(Duration::from_millis(100));
        let path = format!("/tape/{id}/0");
        handle(
            &s,
            req(Method::Put, &path, "", Some(&token), b"BMRGxx"),
            Instant::now(),
        );
        let got = reader.join().unwrap();
        assert_eq!(got.body, b"BMRGxx");
    }

    #[test]
    fn a_result_is_the_seal_of_a_tape_the_relay_holds() {
        let s = shared();
        let t0 = Instant::now();
        let mut plain = b"BMRG".to_vec();
        plain.extend_from_slice(&[7u8; 40]);
        let mut sealed = plain.clone();
        sealed.extend_from_slice(b"SEAL\x07{\"a\":1}");
        let post = |body: &[u8]| handle(&s, req(Method::Post, "/tape", "", None, body), t0);
        let plain_id = json(&post(&plain))["id"].as_str().unwrap().to_string();
        let sealed_id = json(&post(&sealed))["id"].as_str().unwrap().to_string();
        let result = |id: &str| {
            let body = format!(r#"{{"tape":"{id}"}}"#);
            handle(
                &s,
                req(Method::Post, "/results", "", None, body.as_bytes()),
                t0,
            )
            .status
        };
        assert_eq!(result("000000000000"), 404);
        assert_eq!(result(&plain_id), 422, "an unsigned tape is no result");
        assert_eq!(result(&sealed_id), 200);
        let listed = json(&handle(
            &s,
            req(Method::Get, "/results", "since=0", None, b""),
            t0,
        ));
        assert_eq!(listed[0]["tape"], sealed_id.as_str());
        assert_eq!(listed[0]["attestation"]["a"], 1);
        // A sealed tape reads from an offset too.
        let path = format!("/tape/{plain_id}");
        assert_eq!(
            handle(&s, req(Method::Get, &path, "from=40", None, b""), t0).body,
            [7u8; 4]
        );
    }

    #[test]
    fn a_preflight_is_answered() {
        let s = shared();
        assert_eq!(
            handle(
                &s,
                req(Method::Options, "/tape/x/0", "", None, b""),
                Instant::now()
            )
            .status,
            204
        );
    }
}
