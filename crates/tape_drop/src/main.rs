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
//! Routes: `POST /tape` (body ≤ [`TAPE_MAX_BYTES`]) → `{"id", "expires_secs"}`;
//! `GET /tape/<id>` → the bytes; `GET /healthz` → `ok`.
//!
//! Env: `PORT` (Railway convention), `TAPE_TTL_SECS` (default 7 days),
//! `TAPE_BUDGET_BYTES` (default 256 MiB).
//!
//! Abuse posture mirrors ice_vendor: a per-IP token bucket (same
//! constants, same X-Forwarded-For reasoning — see
//! `crates/ice_vendor/src/main.rs`), sized so a human sharing a few
//! matches never sees a 429 and a scripted flood rents nothing.

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

struct RateLimiter {
    buckets: BTreeMap<IpAddr, (u32, Instant)>,
}

impl RateLimiter {
    fn new() -> Self {
        Self {
            buckets: BTreeMap::new(),
        }
    }

    fn allow_at(&mut self, ip: IpAddr, now: Instant) -> bool {
        if self.buckets.len() > BUCKET_SWEEP_LEN {
            let stale = Duration::from_secs(BUCKET_REFILL_SECS * BUCKET_CAPACITY as u64);
            self.buckets
                .retain(|_, (_, at)| now.duration_since(*at) < stale);
        }
        let (tokens, last) = self.buckets.entry(ip).or_insert((BUCKET_CAPACITY, now));
        let refilled = now.duration_since(*last).as_secs() / BUCKET_REFILL_SECS;
        if refilled > 0 {
            *tokens = (*tokens + refilled as u32).min(BUCKET_CAPACITY);
            *last = now;
        }
        if *tokens == 0 {
            return false;
        }
        *tokens -= 1;
        true
    }
}

fn client_ip(request: &tiny_http::Request) -> Option<IpAddr> {
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv("x-forwarded-for"))
        .and_then(|h| {
            h.value
                .as_str()
                .split(',')
                .next()
                .and_then(|s| s.trim().parse().ok())
        })
        .or_else(|| request.remote_addr().map(|a| a.ip()))
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

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn main() {
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8081);
    let ttl = Duration::from_secs(env_u64("TAPE_TTL_SECS", DEFAULT_TTL_SECS));
    let budget = env_u64("TAPE_BUDGET_BYTES", DEFAULT_BUDGET_BYTES as u64) as usize;
    let mut drop = Drop_::new(ttl, budget);
    let mut limiter = RateLimiter::new();
    let server = tiny_http::Server::http(("0.0.0.0", port))
        .unwrap_or_else(|e| panic!("tape_drop: cannot bind port {port}: {e}"));
    println!(
        "tape_drop: listening on :{port}, ttl = {}s, budget = {} bytes",
        ttl.as_secs(),
        budget,
    );

    for mut request in server.incoming_requests() {
        let now = Instant::now();
        let url = request.url().to_string();
        let method = request.method().clone();
        let (status, body, content_type) = match (method, url.as_str()) {
            (tiny_http::Method::Get, "/healthz") => (200, b"ok".to_vec(), "text/plain"),
            (tiny_http::Method::Post, "/tape") => {
                if !client_ip(&request).is_none_or(|ip| limiter.allow_at(ip, now)) {
                    (
                        429,
                        b"{\"error\":\"slow down\"}".to_vec(),
                        "application/json",
                    )
                } else {
                    let mut bytes = Vec::new();
                    let take = request
                        .as_reader()
                        .take(TAPE_MAX_BYTES as u64 + 1)
                        .read_to_end(&mut bytes);
                    match take {
                        Ok(_) if bytes.len() <= TAPE_MAX_BYTES && !bytes.is_empty() => {
                            let receipt = drop.store(bytes, now);
                            match serde_json::to_string(&receipt) {
                                Ok(json) => (200, json.into_bytes(), "application/json"),
                                Err(e) => (
                                    500,
                                    format!("{{\"error\":\"{e}\"}}").into_bytes(),
                                    "application/json",
                                ),
                            }
                        }
                        Ok(_) => (
                            413,
                            b"{\"error\":\"tape too large\"}".to_vec(),
                            "application/json",
                        ),
                        Err(_) => (
                            400,
                            b"{\"error\":\"unreadable body\"}".to_vec(),
                            "application/json",
                        ),
                    }
                }
            }
            (tiny_http::Method::Get, path) if path.starts_with("/tape/") => {
                let id = &path["/tape/".len()..];
                match drop.get(id, now) {
                    Some(bytes) => (200, bytes, "application/octet-stream"),
                    None => (404, b"not found".to_vec(), "text/plain"),
                }
            }
            _ => (404, b"not found".to_vec(), "text/plain"),
        };
        let mut response = tiny_http::Response::from_data(body)
            .with_status_code(status)
            .with_header(
                tiny_http::Header::from_bytes("Content-Type", content_type).expect("static header"),
            );
        // The web theater fetches tapes from a different origin (the
        // static page host); a plain GET is a CORS "simple request", so
        // this one header is the whole story.
        response.add_header(
            tiny_http::Header::from_bytes("Access-Control-Allow-Origin", "*")
                .expect("static header"),
        );
        let _ = request.respond(response);
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
        let mut limiter = RateLimiter::new();
        let ip: IpAddr = "203.0.113.9".parse().unwrap();
        let t0 = Instant::now();
        for _ in 0..BUCKET_CAPACITY {
            assert!(limiter.allow_at(ip, t0));
        }
        assert!(!limiter.allow_at(ip, t0));
        assert!(limiter.allow_at(ip, t0 + Duration::from_secs(BUCKET_REFILL_SECS)));
    }
}
