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
//! honored per platform convention. Single-threaded on purpose — the
//! request rate is "a duel is starting somewhere".

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
#[derive(Serialize, Debug, PartialEq)]
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

// ---- Abuse throttling ------------------------------------------------------
//
// Anyone who extracts TWOTOP_ICE_URL from the public APK can hit /ice in a
// loop, and each Cloudflare credential is billable relay capacity minted on
// this project's key. The TTL bounds one credential's life, not the farming
// rate; these two raises bound the rate.

/// Per-IP token bucket for `/ice`. The capacity covers a rematch flurry;
/// after that it is one fetch per [`BUCKET_REFILL_SECS`]. A phone fetches
/// once per match entry, so a human never sees a 429.
const BUCKET_CAPACITY: u32 = 5;
const BUCKET_REFILL_SECS: u64 = 30;
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

/// The caller's IP for rate-limiting. Behind Railway's edge every TCP
/// connection is the proxy, so the real client rides `X-Forwarded-For`
/// (first hop); a local or direct run falls back to the socket address.
/// XFF is client-forgeable only where clients reach the socket directly,
/// and in that deployment `remote_addr` is the truth anyway — the bucket
/// is abuse throttling, not authentication.
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

fn respond(backend: &Backend, ttl: u64) -> IceResponse {
    match backend {
        Backend::StunOnly => IceResponse {
            urls: stun_urls(),
            username: None,
            credential: None,
            ttl_secs: ttl,
        },
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
        Backend::Cloudflare(endpoint, token) => cloudflare_credentials(endpoint, token, ttl)
            .unwrap_or_else(|e| {
                eprintln!("ice_vendor: cloudflare generate failed, answering STUN-only: {e}");
                IceResponse {
                    urls: stun_urls(),
                    username: None,
                    credential: None,
                    ttl_secs: ttl,
                }
            }),
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
    let mut limiter = RateLimiter::new();
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
        let (status, body, content_type) = match request.url() {
            "/ice" if !authorized(&request, app_key.as_deref()) => (
                403,
                "{\"error\":\"forbidden\"}".to_string(),
                "application/json",
            ),
            "/ice"
                if !client_ip(&request).is_none_or(|ip| limiter.allow_at(ip, Instant::now())) =>
            {
                (
                    429,
                    "{\"error\":\"slow down\"}".to_string(),
                    "application/json",
                )
            }
            "/ice" => match serde_json::to_string(&respond(&backend, ttl)) {
                Ok(json) => (200, json, "application/json"),
                Err(e) => (500, format!("{{\"error\":\"{e}\"}}"), "application/json"),
            },
            "/healthz" => (200, "ok".to_string(), "text/plain"),
            _ => (404, "not found".to_string(), "text/plain"),
        };
        let header =
            tiny_http::Header::from_bytes("Content-Type", content_type).expect("static header");
        let response = tiny_http::Response::from_string(body)
            .with_status_code(status)
            .with_header(header);
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
        let r = respond(&Backend::StunOnly, 60);
        assert_eq!(r.urls, stun_urls());
        assert!(r.username.is_none() && r.credential.is_none());
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
        let r = respond(&backend, 120);
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
