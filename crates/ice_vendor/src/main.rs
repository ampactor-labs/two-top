//! The ephemeral-ICE credential vendor.
//!
//! Phones must never carry TURN credentials: anything compiled into a
//! distributed APK is one `strings` away from public, and a leaked relay
//! credential spends someone else's bandwidth. This service holds the
//! secret server-side and hands each caller a THROWAWAY credential pair
//! that the TURN server will refuse once its timestamp expires — the
//! "TURN REST API" convention (draft-uberti-behave-turn-rest).
//!
//! Two backends, selected by which environment variables are set:
//!
//!   * **HMAC mode** (`ICE_STATIC_AUTH_SECRET` + `ICE_TURN_URLS`): for a
//!     self-hosted coturn running `use-auth-secret`. username =
//!     `<expiry-unix>:twotop`, credential = base64(HMAC-SHA1(secret,
//!     username)). coturn verifies with the shared secret alone.
//!   * **Cloudflare mode** (`CF_TURN_KEY_ID` + `CF_TURN_API_TOKEN`): the
//!     vendor proxies Cloudflare's TURN credential generator, so the only
//!     secret anywhere is the API token held HERE. `CF_TURN_API_URL`
//!     overrides the endpoint if Cloudflare moves it (`{key_id}` is
//!     substituted).
//!
//! Neither set ⇒ STUN-only responses, which still exercises the whole
//! client fetch path (useful before any relay exists).
//!
//! Routes: `GET /ice` → the JSON below; `GET /healthz` → `ok`.
//!
//! ```json
//! {"urls": ["stun:...", "turn:..."], "username": "...",
//!  "credential": "...", "ttl_secs": 14400}
//! ```
//!
//! Deploy: Railway service off this repo with start command
//! `cargo run -p ice_vendor --release` (or any container). `PORT` is
//! honored per platform convention. Requests are served on one thread,
//! and every answer is ready before it is asked for: HMAC credentials are
//! computed on the spot, and in Cloudflare mode a background thread keeps
//! one minted credential fresh ([`Minted`]) that every caller shares. No
//! request ever waits on Cloudflare: the phone gives the whole fetch
//! 2.5 s, and one slow upstream call used to hold every caller queued
//! behind it past that, down to STUN-only.

use base64::Engine as _;
use hmac::{Hmac, Mac};
use serde::Serialize;
use sha1::Sha1;
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Default credential lifetime: 4 hours. Long enough that a credential
/// fetched at match start never expires mid-session, short enough that a
/// leaked one is dead by dinnertime.
const DEFAULT_TTL_SECS: u64 = 4 * 3600;

/// The same public STUN defaults matchbox ships with — always included so
/// direct connections keep working even when the relay path is down.
const STUN_DEFAULTS: [&str; 2] = [
    "stun:stun.l.google.com:19302",
    "stun:stun1.l.google.com:19302",
];

/// The response contract shared with `app::netplay` — keep the field
/// names in lockstep with the client's `IceResponse`.
#[derive(Serialize, Debug, PartialEq, Clone)]
struct IceResponse {
    urls: Vec<String>,
    username: Option<String>,
    credential: Option<String>,
    ttl_secs: u64,
}

enum Backend {
    /// Self-hosted coturn `use-auth-secret`: (shared secret, turn urls).
    Hmac(String, Vec<String>),
    /// Cloudflare TURN: (endpoint with `{key_id}` resolved, api token).
    Cloudflare(String, String),
    /// No relay configured — STUN-only answers.
    StunOnly,
}

fn backend_from_env() -> Backend {
    let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    if let (Some(secret), Some(urls)) = (get("ICE_STATIC_AUTH_SECRET"), get("ICE_TURN_URLS")) {
        let urls = urls.split(',').map(|u| u.trim().to_string()).collect();
        return Backend::Hmac(secret, urls);
    }
    if let (Some(key_id), Some(token)) = (get("CF_TURN_KEY_ID"), get("CF_TURN_API_TOKEN")) {
        let endpoint = get("CF_TURN_API_URL").unwrap_or_else(|| {
            "https://rtc.live.cloudflare.com/v1/turn/keys/{key_id}/credentials/generate".to_string()
        });
        return Backend::Cloudflare(endpoint.replace("{key_id}", &key_id), token);
    }
    Backend::StunOnly
}

fn ttl_from_env() -> u64 {
    std::env::var("ICE_TTL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_TTL_SECS)
}

// ---- Cloudflare: one credential, minted ahead -------------------------------
//
// A Cloudflare credential is minted by a call to Cloudflare, which took up to
// its 5 s timeout while the request that needed it, and every request queued
// behind it, waited. It is now minted in the background, ahead of need, and
// every caller is handed the current one: one upstream call per refresh
// interval, however many duels start, and none of them waits.

/// A Cloudflare credential set and when it was minted.
#[derive(Clone, Debug)]
struct Minted {
    response: IceResponse,
    at: Instant,
}

/// How often the background thread mints a fresh credential: a quarter of
/// its lifetime, so the one being handed out is always well inside the
/// half-life [`cached_answer`] serves it to.
fn refresh_every(ttl: u64) -> Duration {
    Duration::from_secs((ttl / 4).max(10))
}

/// After a failed mint, try again this soon.
const REFRESH_RETRY: Duration = Duration::from_secs(30);

/// The minted credential as a caller should see it, while less than half
/// its lifetime is spent: `ttl_secs` is what is left of it, so a phone
/// never holds one past its expiry. `None` when there is none yet, or it
/// is too old (the refresher has been failing): answer STUN-only.
fn cached_answer(minted: Option<&Minted>, now: Instant, ttl: u64) -> Option<IceResponse> {
    let minted = minted?;
    let age = now.saturating_duration_since(minted.at).as_secs();
    (age < ttl / 2).then(|| IceResponse {
        ttl_secs: ttl - age,
        ..minted.response.clone()
    })
}

/// Keep `shared` holding a fresh Cloudflare credential, forever.
fn refresh_cloudflare(
    endpoint: String,
    token: String,
    ttl: u64,
    shared: std::sync::Arc<std::sync::Mutex<Option<Minted>>>,
) {
    loop {
        let wait = match cloudflare_credentials(&endpoint, &token, ttl) {
            Ok(response) => {
                let at = Instant::now();
                *shared.lock().unwrap_or_else(|e| e.into_inner()) = Some(Minted { response, at });
                refresh_every(ttl)
            }
            Err(e) => {
                eprintln!("ice_vendor: cloudflare generate failed, retrying in 30 s: {e}");
                REFRESH_RETRY
            }
        };
        std::thread::sleep(wait);
    }
}

// ---- Abuse throttling ------------------------------------------------------
//
// Anyone who extracts TWOTOP_ICE_URL from the public APK can hit /ice in a
// loop. A flood no longer mints anything (Cloudflare credentials are minted
// on a timer, HMAC ones cost a hash), so the bucket only bounds request
// volume, and it is sized for the callers who need a relay most: phones on
// mobile data share their carrier's public addresses, many to one, and the
// old 5-then-1-per-30-s bucket turned them away to STUN-only, the one case
// a relay exists for.

/// Per-IP token bucket for `/ice`: a burst of [`BUCKET_CAPACITY`], then one
/// fetch per [`BUCKET_REFILL_SECS`]. Thirty phones behind one carrier
/// address can start a duel at once.
const BUCKET_CAPACITY: u32 = 30;
const BUCKET_REFILL_SECS: u64 = 2;
/// Sweep threshold: past this many tracked addresses, entries idle long
/// enough to have refilled completely are dropped — an attacker cycling
/// source addresses rents memory instead of keeping it.
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

    /// Take one token for `ip`; `false` means refused. Bucket math on a
    /// caller-supplied `now`, so the tests need no real clock.
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

/// The static app-key filter (`ICE_APP_KEY`): a shared value baked into the
/// APK, one `strings` away from public — a turnstile that filters drive-by
/// scrapers, not authentication. Unset means open, exactly as before.
fn authorized(request: &tiny_http::Request, key: Option<&str>) -> bool {
    let Some(key) = key else {
        return true;
    };
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv("x-app-key"))
        .is_some_and(|h| h.value.as_str() == key)
}

/// The coturn `use-auth-secret` credential pair for an expiry instant:
/// username carries the expiry, credential proves we knew the secret.
/// Pure for the test vector.
fn hmac_credentials(secret: &str, expiry_unix: u64) -> (String, String) {
    let username = format!("{expiry_unix}:twotop");
    let mut mac =
        Hmac::<Sha1>::new_from_slice(secret.as_bytes()).expect("hmac accepts any key length");
    mac.update(username.as_bytes());
    let credential = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
    (username, credential)
}

fn stun_urls() -> Vec<String> {
    STUN_DEFAULTS.iter().map(|s| s.to_string()).collect()
}

fn stun_only(ttl: u64) -> IceResponse {
    IceResponse {
        urls: stun_urls(),
        username: None,
        credential: None,
        ttl_secs: ttl,
    }
}

/// The answer to `GET /ice`. Never blocks: Cloudflare mode reads the
/// credential the refresher minted (`minted`, with `now`), and answers
/// STUN-only while there is none fresh enough.
fn respond(backend: &Backend, ttl: u64, minted: Option<&Minted>, now: Instant) -> IceResponse {
    match backend {
        Backend::StunOnly => stun_only(ttl),
        Backend::Hmac(secret, turn_urls) => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let (username, credential) = hmac_credentials(secret, now + ttl);
            let mut urls = stun_urls();
            urls.extend(turn_urls.iter().cloned());
            IceResponse {
                urls,
                username: Some(username),
                credential: Some(credential),
                ttl_secs: ttl,
            }
        }
        Backend::Cloudflare(..) => {
            cached_answer(minted, now, ttl).unwrap_or_else(|| stun_only(ttl))
        }
    }
}

/// Ask Cloudflare for a short-lived ICE server set. The response shape is
/// parsed defensively (`iceServers` as object or array) so a docs-level
/// change on their side degrades to STUN-only instead of a crash.
fn cloudflare_credentials(
    endpoint: &str,
    token: &str,
    ttl: u64,
) -> Result<IceResponse, Box<dyn std::error::Error>> {
    let body: serde_json::Value = ureq::post(endpoint)
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_secs(5))
        .send_json(serde_json::json!({ "ttl": ttl }))?
        .into_json()?;
    let server = match &body["iceServers"] {
        serde_json::Value::Array(list) => list
            .iter()
            .find(|s| s["username"].is_string())
            .or_else(|| list.first())
            .cloned()
            .ok_or("empty iceServers array")?,
        obj @ serde_json::Value::Object(_) => obj.clone(),
        _ => return Err("no iceServers in response".into()),
    };
    let mut urls = stun_urls();
    match &server["urls"] {
        serde_json::Value::Array(list) => {
            urls.extend(list.iter().filter_map(|u| u.as_str().map(String::from)));
        }
        serde_json::Value::String(u) => urls.push(u.clone()),
        _ => return Err("no urls in iceServers".into()),
    }
    Ok(IceResponse {
        urls,
        username: server["username"].as_str().map(String::from),
        credential: server["credential"].as_str().map(String::from),
        ttl_secs: ttl,
    })
}

/// CORS for the browser build's cross-origin `GET /ice`.
fn cors_headers() -> [tiny_http::Header; 3] {
    let h = |k: &str, v: &str| tiny_http::Header::from_bytes(k, v).expect("static header");
    [
        h("Access-Control-Allow-Origin", "*"),
        h("Access-Control-Allow-Methods", "GET, OPTIONS"),
        h("Access-Control-Allow-Headers", "X-App-Key"),
    ]
}

fn main() {
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let backend = backend_from_env();
    let ttl = ttl_from_env();
    let mode = match &backend {
        Backend::Hmac(..) => "hmac (coturn use-auth-secret)",
        Backend::Cloudflare(..) => "cloudflare",
        Backend::StunOnly => "stun-only (no relay configured)",
    };
    let app_key = std::env::var("ICE_APP_KEY").ok().filter(|v| !v.is_empty());
    let minted = std::sync::Arc::new(std::sync::Mutex::new(None::<Minted>));
    if let Backend::Cloudflare(endpoint, token) = &backend {
        let (endpoint, token, shared) = (endpoint.clone(), token.clone(), minted.clone());
        std::thread::spawn(move || refresh_cloudflare(endpoint, token, ttl, shared));
    }
    let mut limiter = RateLimiter::new();
    let trusted_hops = trusted_proxy_hops();
    let server = tiny_http::Server::http(("0.0.0.0", port))
        .unwrap_or_else(|e| panic!("ice_vendor: cannot bind port {port}: {e}"));
    println!(
        "ice_vendor: listening on :{port}, mode = {mode}, ttl = {ttl}s, app_key = {}",
        if app_key.is_some() { "required" } else { "off" },
    );

    for request in server.incoming_requests() {
        // /healthz stays exempt from the key and the bucket: Railway's
        // healthcheck (railway.json) must never be throttled into a restart
        // loop, and it hands out nothing billable.
        // The browser build fetches /ice cross-origin (the Pages site is
        // not this host). Without CORS the page's fetch fails and the web
        // game silently falls back to STUN-only — the very gap a relay
        // closes. `X-App-Key` makes the request non-simple, so answer the
        // preflight too. Open origin on purpose: the key (when set) and the
        // rate limiter are the gates, never the Origin header.
        if *request.method() == tiny_http::Method::Options {
            let mut response = tiny_http::Response::empty(204);
            for header in cors_headers() {
                response.add_header(header);
            }
            let _ = request.respond(response);
            continue;
        }
        let (status, body, content_type) = match request.url() {
            "/ice" if !authorized(&request, app_key.as_deref()) => (
                403,
                "{\"error\":\"forbidden\"}".to_string(),
                "application/json",
            ),
            "/ice"
                if !client_ip(&request, trusted_hops)
                    .is_none_or(|ip| limiter.allow_at(ip, Instant::now())) =>
            {
                (
                    429,
                    "{\"error\":\"slow down\"}".to_string(),
                    "application/json",
                )
            }
            "/ice" => match serde_json::to_string(&respond(
                &backend,
                ttl,
                minted.lock().unwrap_or_else(|e| e.into_inner()).as_ref(),
                Instant::now(),
            )) {
                Ok(json) => (200, json, "application/json"),
                Err(e) => (500, format!("{{\"error\":\"{e}\"}}"), "application/json"),
            },
            "/healthz" => (200, "ok".to_string(), "text/plain"),
            _ => (404, "not found".to_string(), "text/plain"),
        };
        let header =
            tiny_http::Header::from_bytes("Content-Type", content_type).expect("static header");
        let mut response = tiny_http::Response::from_string(body)
            .with_status_code(status)
            .with_header(header);
        for header in cors_headers() {
            response.add_header(header);
        }
        let _ = request.respond(response);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_matches_the_coturn_convention() {
        // Independently computed:
        //   python3 -c "import hmac,hashlib,base64; print(base64.b64encode(
        //     hmac.new(b'sesame', b'1700000000:twotop', hashlib.sha1)
        //     .digest()).decode())"
        let (username, credential) = hmac_credentials("sesame", 1_700_000_000);
        assert_eq!(username, "1700000000:twotop");
        assert_eq!(credential, "5qrcx5XkYi6dvbeiSMJF7UgDbag=");
    }

    #[test]
    fn stun_only_answers_carry_no_credentials() {
        let r = respond(&Backend::StunOnly, 60, None, Instant::now());
        assert_eq!(r.urls, stun_urls());
        assert!(r.username.is_none() && r.credential.is_none());
    }

    fn cloudflare() -> Backend {
        Backend::Cloudflare("https://cf.invalid/{key_id}".into(), "token".into())
    }

    fn relay(ttl: u64) -> IceResponse {
        let mut urls = stun_urls();
        urls.push("turn:turn.cloudflare.com:3478?transport=udp".into());
        IceResponse {
            urls,
            username: Some("u".into()),
            credential: Some("c".into()),
            ttl_secs: ttl,
        }
    }

    #[test]
    fn cloudflare_mode_answers_from_the_minted_credential_without_waiting() {
        let ttl = 4 * 3600;
        let t0 = Instant::now();
        let minted = Minted {
            response: relay(ttl),
            at: t0,
        };
        // An hour in: the same credential, with the hour taken off its life.
        let r = respond(
            &cloudflare(),
            ttl,
            Some(&minted),
            t0 + Duration::from_secs(3600),
        );
        assert_eq!(r.username.as_deref(), Some("u"));
        assert_eq!(r.ttl_secs, ttl - 3600);
        // Nothing minted yet (first seconds after boot): STUN-only, at once.
        let r = respond(&cloudflare(), ttl, None, t0);
        assert!(r.username.is_none());
        assert_eq!(r.urls, stun_urls());
    }

    #[test]
    fn a_credential_past_its_half_life_is_not_handed_out() {
        let ttl = 4 * 3600;
        let t0 = Instant::now();
        let minted = Minted {
            response: relay(ttl),
            at: t0,
        };
        let half = t0 + Duration::from_secs(ttl / 2);
        assert!(cached_answer(Some(&minted), half - Duration::from_secs(1), ttl).is_some());
        assert!(cached_answer(Some(&minted), half, ttl).is_none());
        // The refresher mints well before that.
        assert!(refresh_every(ttl) < Duration::from_secs(ttl / 2));
    }

    #[test]
    fn a_carrier_address_full_of_phones_is_not_turned_away() {
        let mut limiter = RateLimiter::new();
        let carrier: IpAddr = "100.64.0.1".parse().unwrap();
        let t0 = Instant::now();
        for phone in 0..30 {
            assert!(
                limiter.allow_at(carrier, t0),
                "phone {phone} gets its relay"
            );
        }
    }

    #[test]
    fn the_bucket_refuses_a_flood_and_refills_on_schedule() {
        let mut limiter = RateLimiter::new();
        let ip: IpAddr = "203.0.113.9".parse().unwrap();
        let t0 = Instant::now();
        for _ in 0..BUCKET_CAPACITY {
            assert!(limiter.allow_at(ip, t0), "burst within capacity passes");
        }
        assert!(!limiter.allow_at(ip, t0), "the next one is refused");
        let refill = t0 + Duration::from_secs(BUCKET_REFILL_SECS);
        assert!(
            limiter.allow_at(ip, refill),
            "one token comes back after the window"
        );
        assert!(!limiter.allow_at(ip, refill), "exactly one");
    }

    #[test]
    fn one_abuser_does_not_starve_the_neighbors() {
        let mut limiter = RateLimiter::new();
        let bad: IpAddr = "203.0.113.9".parse().unwrap();
        let good: IpAddr = "198.51.100.7".parse().unwrap();
        let t0 = Instant::now();
        for _ in 0..100 {
            let _ = limiter.allow_at(bad, t0);
        }
        assert!(limiter.allow_at(good, t0), "buckets are per-IP");
    }

    #[test]
    fn the_sweep_keeps_the_map_rented_not_owned() {
        // An attacker cycling source addresses grows the map; once it
        // passes the sweep threshold, fully-idle entries are dropped on
        // the next call instead of living forever.
        let mut limiter = RateLimiter::new();
        let t0 = Instant::now();
        for i in 0..(BUCKET_SWEEP_LEN + 10) {
            let ip = IpAddr::from([10, 0, (i >> 8) as u8, i as u8]);
            let _ = limiter.allow_at(ip, t0);
        }
        assert!(limiter.buckets.len() > BUCKET_SWEEP_LEN);
        let later = t0 + Duration::from_secs(BUCKET_REFILL_SECS * BUCKET_CAPACITY as u64 + 1);
        let fresh: IpAddr = "192.0.2.1".parse().unwrap();
        assert!(limiter.allow_at(fresh, later));
        assert!(limiter.buckets.len() <= 2, "stale entries swept");
    }

    #[test]
    fn hmac_mode_appends_turn_urls_after_the_stun_defaults() {
        let backend = Backend::Hmac(
            "sesame".into(),
            vec!["turn:relay.example.com:3478?transport=udp".into()],
        );
        let r = respond(&backend, 120, None, Instant::now());
        assert!(r.urls.starts_with(&stun_urls()));
        assert_eq!(
            r.urls.last().unwrap(),
            "turn:relay.example.com:3478?transport=udp"
        );
        let user = r.username.expect("hmac mode names a user");
        assert!(user.ends_with(":twotop"));
        let expiry: u64 = user.split(':').next().unwrap().parse().unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(expiry >= now + 100, "expiry sits ttl in the future");
        assert!(r.credential.is_some());
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
