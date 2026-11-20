//! The relay's two newer kinds of thing, beside sealed tapes:
//!
//! * **Live tapes**: a duel streamed as it is played. The phone opens one
//!   (`POST /live` → an id and a secret token), appends numbered chunks of
//!   confirmed input in order (`PUT /live/<id>/<seq>`, the token in
//!   `X-Live-Token`), and ends it. Anyone with the id reads it from a byte
//!   offset (`GET /live/<id>?from=<n>`), and a reader caught up with an
//!   open tape waits for the next chunk (a long poll) instead of hammering.
//!   The bytes are the stream the app writes: a postcard `ReplayHeader`,
//!   then input pairs, so a live tape that has ended is a tape.
//! * **Results**: dual-signed match results posted for the ladder
//!   (`POST /results`), listed newest first (`GET /results?since=<unix>`).
//!   The relay keeps them; the ladder page verifies each one itself in the
//!   browser (re-simulating the tape, checking both signatures), so the
//!   relay is never the thing trusted.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// A live tape's ceiling: a five-kill match is ~160 KB of input.
pub const LIVE_MAX_BYTES: usize = 2 * 1024 * 1024;
/// One appended chunk's ceiling (a second of input is ~0.5 KB).
pub const CHUNK_MAX_BYTES: usize = 64 * 1024;
/// How long a live tape lives after its last append.
pub const LIVE_TTL: Duration = Duration::from_secs(6 * 3600);
/// Results kept (newest first, older ones fall off).
pub const RESULTS_MAX: usize = 2000;
/// One posted result's ceiling (an attestation is ~1 KB of JSON).
pub const RESULT_MAX_BYTES: usize = 8 * 1024;

pub struct Live {
    pub bytes: Vec<u8>,
    token: String,
    next_seq: u32,
    pub open: bool,
    expiry: Instant,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AppendError {
    NoSuchTape,
    BadToken,
    /// Not the next chunk (a retry of one already taken is `Ok`).
    OutOfOrder {
        expected: u32,
    },
    Closed,
    TooLarge,
}

#[derive(Default)]
pub struct Lives {
    tapes: BTreeMap<String, Live>,
}

impl Lives {
    pub fn open(&mut self, id: String, token: String, now: Instant) {
        self.evict(now);
        self.tapes.insert(
            id,
            Live {
                bytes: Vec::new(),
                token,
                next_seq: 0,
                open: true,
                expiry: now + LIVE_TTL,
            },
        );
    }

    /// Append chunk `seq`. A repeat of the last chunk taken is accepted and
    /// ignored (an upload retried after a lost reply), anything else out of
    /// order is refused. Returns the tape's length after.
    pub fn append(
        &mut self,
        id: &str,
        token: &str,
        seq: u32,
        chunk: &[u8],
        now: Instant,
    ) -> Result<usize, AppendError> {
        let live = self.tapes.get_mut(id).ok_or(AppendError::NoSuchTape)?;
        if live.token != token {
            return Err(AppendError::BadToken);
        }
        if seq + 1 == live.next_seq {
            return Ok(live.bytes.len());
        }
        if !live.open {
            return Err(AppendError::Closed);
        }
        if seq != live.next_seq {
            return Err(AppendError::OutOfOrder {
                expected: live.next_seq,
            });
        }
        if chunk.len() > CHUNK_MAX_BYTES || live.bytes.len() + chunk.len() > LIVE_MAX_BYTES {
            return Err(AppendError::TooLarge);
        }
        live.bytes.extend_from_slice(chunk);
        live.next_seq += 1;
        live.expiry = now + LIVE_TTL;
        Ok(live.bytes.len())
    }

    pub fn end(&mut self, id: &str, token: &str) -> Result<(), AppendError> {
        let live = self.tapes.get_mut(id).ok_or(AppendError::NoSuchTape)?;
        if live.token != token {
            return Err(AppendError::BadToken);
        }
        live.open = false;
        Ok(())
    }

    /// The bytes from `from` on, and whether more may come.
    pub fn read(&mut self, id: &str, from: usize, now: Instant) -> Option<(Vec<u8>, bool)> {
        self.evict(now);
        let live = self.tapes.get(id)?;
        let from = from.min(live.bytes.len());
        Some((live.bytes[from..].to_vec(), live.open))
    }

    fn evict(&mut self, now: Instant) {
        self.tapes.retain(|_, live| live.expiry > now);
    }
}

/// A posted result, as the ladder lists it.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub struct ResultRow {
    pub posted_unix: u64,
    /// The relay id of the tape the result is about (a sealed tape).
    pub tape: String,
    /// The `.attest.json` document, as the phone wrote it.
    pub attestation: serde_json::Value,
}

#[derive(Default)]
pub struct Results {
    rows: Vec<ResultRow>,
}

impl Results {
    pub fn post(&mut self, row: ResultRow) {
        self.rows.insert(0, row);
        self.rows.truncate(RESULTS_MAX);
    }

    /// Newest first, posted at or after `since` (unix seconds).
    pub fn since(&self, since: u64) -> Vec<ResultRow> {
        self.rows
            .iter()
            .filter(|r| r.posted_unix >= since)
            .cloned()
            .collect()
    }
}

/// A body posted to `/results`: `{"tape": <id>, "attestation": {...}}`.
pub fn parse_result(body: &[u8], posted_unix: u64) -> Option<ResultRow> {
    #[derive(serde::Deserialize)]
    struct Posted {
        tape: String,
        attestation: serde_json::Value,
    }
    if body.len() > RESULT_MAX_BYTES {
        return None;
    }
    let posted: Posted = serde_json::from_slice(body).ok()?;
    let id_ok = posted.tape.len() == 12 && posted.tape.bytes().all(|b| b.is_ascii_hexdigit());
    (id_ok && posted.attestation.is_object()).then_some(ResultRow {
        posted_unix,
        tape: posted.tape,
        attestation: posted.attestation,
    })
}

/// `n` random bytes as hex, from the system's randomness (the relay runs
/// on Linux). Ids need only be unguessable enough not to collide; tokens
/// must not be guessable at all.
pub fn random_hex(n: usize) -> String {
    use std::io::Read as _;
    let mut buf = vec![0u8; n];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut buf);
    } else {
        // Nowhere to get randomness: the clock's nanoseconds, hashed. Only
        // ever a local dev run.
        use sha2::Digest as _;
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let digest = sha2::Sha256::digest(t.to_le_bytes());
        for (b, d) in buf.iter_mut().zip(digest.iter().cycle()) {
            *b = *d;
        }
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// The `from=` of a query string, 0 when absent. Pure.
pub fn query_from(query: &str) -> usize {
    query
        .split('&')
        .find_map(|kv| kv.strip_prefix("from="))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// The `since=` of a query string, 0 when absent. Pure.
pub fn query_since(query: &str) -> u64 {
    query
        .split('&')
        .find_map(|kv| kv.strip_prefix("since="))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_live_tape_takes_its_chunks_in_order_from_its_owner_only() {
        let mut lives = Lives::default();
        let t0 = Instant::now();
        lives.open("id".into(), "secret".into(), t0);
        assert_eq!(lives.append("id", "secret", 0, b"head", t0), Ok(4));
        assert_eq!(
            lives.append("id", "guess", 1, b"x", t0),
            Err(AppendError::BadToken)
        );
        assert_eq!(
            lives.append("id", "secret", 2, b"x", t0),
            Err(AppendError::OutOfOrder { expected: 1 })
        );
        // A retried chunk is taken once.
        assert_eq!(lives.append("id", "secret", 0, b"head", t0), Ok(4));
        assert_eq!(lives.append("id", "secret", 1, b"ers", t0), Ok(7));
        assert_eq!(lives.read("id", 0, t0), Some((b"headers".to_vec(), true)));
        assert_eq!(lives.read("id", 4, t0), Some((b"ers".to_vec(), true)));
        assert_eq!(lives.read("id", 99, t0), Some((Vec::new(), true)));
        lives.end("id", "secret").unwrap();
        assert_eq!(lives.read("id", 7, t0), Some((Vec::new(), false)));
        assert_eq!(
            lives.append("id", "secret", 2, b"late", t0),
            Err(AppendError::Closed)
        );
        assert_eq!(
            lives.append("nope", "secret", 0, b"x", t0),
            Err(AppendError::NoSuchTape)
        );
    }

    #[test]
    fn a_live_tape_dies_after_its_ttl() {
        let mut lives = Lives::default();
        let t0 = Instant::now();
        lives.open("id".into(), "s".into(), t0);
        assert!(
            lives
                .read("id", 0, t0 + LIVE_TTL - Duration::from_secs(1))
                .is_some()
        );
        assert!(
            lives
                .read("id", 0, t0 + LIVE_TTL + Duration::from_secs(1))
                .is_none()
        );
    }

    #[test]
    fn results_are_parsed_strictly_and_listed_newest_first() {
        let ok = br#"{"tape":"0123456789ab","attestation":{"statement":{}}}"#;
        let row = parse_result(ok, 100).unwrap();
        assert_eq!(row.tape, "0123456789ab");
        assert!(parse_result(br#"{"tape":"../etc","attestation":{}}"#, 1).is_none());
        assert!(parse_result(br#"{"tape":"0123456789ab","attestation":3}"#, 1).is_none());
        assert!(parse_result(b"not json", 1).is_none());
        let mut results = Results::default();
        results.post(row.clone());
        results.post(ResultRow {
            posted_unix: 200,
            ..row
        });
        let listed = results.since(150);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].posted_unix, 200);
        assert_eq!(results.since(0).len(), 2);
    }

    #[test]
    fn queries_read_their_numbers() {
        assert_eq!(query_from("from=42"), 42);
        assert_eq!(query_from("x=1&from=7"), 7);
        assert_eq!(query_from(""), 0);
        assert_eq!(query_since("since=1700000000"), 1_700_000_000);
    }

    #[test]
    fn random_hex_is_the_right_length_and_varies() {
        let a = random_hex(16);
        assert_eq!(a.len(), 32);
        assert_ne!(a, random_hex(16));
    }
}
