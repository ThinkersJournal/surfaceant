// SPDX-License-Identifier: MIT OR Apache-2.0

//! Pure relay logic with no Workers runtime dependency, so it is unit-tested on the host:
//! channel ids, connection tokens, delivery ids, header filtering, rate limiting and limits.

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

/// Bytes of randomness in a channel id.
pub const ID_RANDOM_BYTES: usize = 16;
/// Bytes of the truncated MAC appended to a channel id.
pub const ID_MAC_BYTES: usize = 16;

/// Lower-case hex of `bytes`.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// Decode lower- or upper-case hex; `None` for odd length or a non-hex digit.
pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !s.is_ascii() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

fn id_mac(key: &[u8], random: &[u8]) -> [u8; ID_MAC_BYTES] {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(b"surfaceant/channel-id/v1");
    mac.update(random);
    let full = mac.finalize().into_bytes();
    let mut out = [0u8; ID_MAC_BYTES];
    out.copy_from_slice(&full[..ID_MAC_BYTES]);
    out
}

/// A self-validating channel id: `hex(random) || hex(MAC(key, random))`, 64 hex digits.
/// The Worker can reject a forged id without touching a Durable Object.
pub fn make_channel_id(key: &[u8], random: [u8; ID_RANDOM_BYTES]) -> String {
    let mut raw = random.to_vec();
    raw.extend_from_slice(&id_mac(key, &random));
    hex(&raw)
}

/// Whether `id` was minted under `key` (constant-time MAC comparison).
pub fn channel_id_is_valid(key: &[u8], id: &str) -> bool {
    let Some(raw) = unhex(id) else { return false };
    if raw.len() != ID_RANDOM_BYTES + ID_MAC_BYTES || id.len() != raw.len() * 2 {
        return false;
    }
    let (random, mac) = raw.split_at(ID_RANDOM_BYTES);
    id_mac(key, random).ct_eq(mac).into()
}

/// The stored form of a connection token: lower-case hex SHA-256.
pub fn hash_token(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

/// Constant-time equality of two strings (length differences are not secret here: both are fixed-size hex).
pub fn secret_eq(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

/// The token from an `Authorization: Bearer <token>` header value.
pub fn bearer(header: Option<&str>) -> Option<&str> {
    let (scheme, token) = header?.split_once(' ')?;
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then_some(token.trim())
}

/// The de-duplication id of a forwarded request: the sender's own delivery id when it has one.
pub fn delivery_id_from(headers: &[(String, String)]) -> Option<String> {
    ["x-github-delivery", "x-delivery-id", "idempotency-key"]
        .iter()
        .find_map(|want| {
            headers
                .iter()
                .find(|(n, v)| n.eq_ignore_ascii_case(want) && !v.is_empty())
                .map(|(_, v)| v.clone())
        })
}

/// Whether a request header is forwarded: hop-by-hop headers and ones Cloudflare adds are dropped.
pub fn forward_header(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    !(matches!(
        n.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "host"
            | "x-real-ip"
    ) || n.starts_with("cf-")
        || n.starts_with("x-forwarded-"))
}

/// A per-service abuse contact: an email address or an `https://` URL, no whitespace or control characters, at most 254 bytes.
pub fn abuse_contact_is_valid(s: &str) -> bool {
    let s = s.trim();
    if s.len() < 8 || s.len() > 254 || s.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    if let Some(rest) = s.strip_prefix("https://") {
        return rest.contains('.') && !rest.starts_with('/') && !rest.starts_with('.');
    }
    match s.split_once('@') {
        Some((local, domain)) => {
            !local.is_empty()
                && !domain.contains('@')
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
        }
        None => false,
    }
}

/// How long an unacknowledged event is kept for an offline service: 5 minutes (CireSnave 2026-11-16, board 447:
/// "only kept for long enough for a service that is online to receive it"). Was 24 hours.
pub const BUFFER_TTL_SECONDS: u64 = 300;

/// Operator-tunable limits (Worker vars), defaults from `docs/DESIGN.md` section 4.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    /// Largest accepted request body, in bytes.
    pub max_body_bytes: usize,
    /// How long an unacknowledged event is kept, in seconds.
    pub buffer_ttl_seconds: u64,
    /// Most events kept per channel.
    pub buffer_max_events: i64,
    /// Most body bytes kept per channel.
    pub buffer_max_bytes: i64,
    /// Sustained requests per second per channel.
    pub rate_per_sec: f64,
    /// Burst size per channel.
    pub rate_burst: f64,
    /// Events sent to the client before it must ack.
    pub window: i64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_body_bytes: 1 << 20,
            buffer_ttl_seconds: BUFFER_TTL_SECONDS,
            buffer_max_events: 1_000,
            buffer_max_bytes: 64 << 20,
            rate_per_sec: 10.0,
            rate_burst: 50.0,
            window: 16,
        }
    }
}

/// A token bucket; the Durable Object serialises requests, so no locking is needed.
#[derive(Debug, Clone, Copy)]
pub struct TokenBucket {
    tokens: f64,
    last_ms: f64,
}

impl TokenBucket {
    /// A full bucket.
    pub fn new(limits: &Limits, now_ms: f64) -> Self {
        Self {
            tokens: limits.rate_burst,
            last_ms: now_ms,
        }
    }

    /// Take one token, or return the whole seconds to wait.
    pub fn take(&mut self, limits: &Limits, now_ms: f64) -> Result<(), u64> {
        let elapsed = ((now_ms - self.last_ms) / 1000.0).max(0.0);
        self.tokens = (self.tokens + elapsed * limits.rate_per_sec).min(limits.rate_burst);
        self.last_ms = now_ms;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            Ok(())
        } else {
            Err(((1.0 - self.tokens) / limits.rate_per_sec).ceil().max(1.0) as u64)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8] = b"test-channel-key";

    #[test]
    fn hex_round_trips_and_rejects_garbage() {
        assert_eq!(hex(&[0, 0xab, 0xff]), "00abff");
        assert_eq!(unhex("00ABff").unwrap(), vec![0, 0xab, 0xff]);
        assert!(unhex("abc").is_none());
        assert!(unhex("zz").is_none());
        assert!(unhex("é1").is_none());
    }

    #[test]
    fn minted_ids_validate_and_forged_ones_do_not() {
        let id = make_channel_id(KEY, [7; 16]);
        assert_eq!(id.len(), 64);
        assert!(channel_id_is_valid(KEY, &id));
        assert!(!channel_id_is_valid(b"other-key", &id), "wrong key");
        let mut bad = id.clone().into_bytes();
        bad[63] = if bad[63] == b'0' { b'1' } else { b'0' };
        assert!(
            !channel_id_is_valid(KEY, std::str::from_utf8(&bad).unwrap()),
            "one MAC nibble flipped"
        );
        let mut bad = id.clone().into_bytes();
        bad[0] = if bad[0] == b'0' { b'1' } else { b'0' };
        assert!(
            !channel_id_is_valid(KEY, std::str::from_utf8(&bad).unwrap()),
            "one random nibble flipped"
        );
        assert!(!channel_id_is_valid(KEY, &id[..62]));
        assert!(!channel_id_is_valid(KEY, ""));
        assert!(!channel_id_is_valid(KEY, &"0".repeat(64)));
    }

    #[test]
    fn different_randomness_gives_different_ids() {
        assert_ne!(make_channel_id(KEY, [1; 16]), make_channel_id(KEY, [2; 16]));
    }

    #[test]
    fn token_hash_is_sha256_hex() {
        // SHA-256("abc"), FIPS 180-4 example.
        assert_eq!(
            hash_token("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(secret_eq(&hash_token("a"), &hash_token("a")));
        assert!(!secret_eq(&hash_token("a"), &hash_token("b")));
    }

    #[test]
    fn bearer_parsing() {
        assert_eq!(bearer(Some("Bearer abc")), Some("abc"));
        assert_eq!(bearer(Some("bearer abc")), Some("abc"));
        assert_eq!(bearer(Some("Basic abc")), None);
        assert_eq!(bearer(Some("Bearer ")), None);
        assert_eq!(bearer(None), None);
    }

    #[test]
    fn delivery_id_prefers_github_then_generic_headers() {
        let h = |v: &[(&str, &str)]| -> Vec<(String, String)> {
            v.iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect()
        };
        assert_eq!(
            delivery_id_from(&h(&[("X-GitHub-Delivery", "g1"), ("Idempotency-Key", "i")])),
            Some("g1".into())
        );
        assert_eq!(
            delivery_id_from(&h(&[("idempotency-key", "i")])),
            Some("i".into())
        );
        assert_eq!(delivery_id_from(&h(&[("X-GitHub-Delivery", "")])), None);
        assert_eq!(delivery_id_from(&h(&[("Content-Type", "x")])), None);
    }

    #[test]
    fn header_filter_keeps_signatures_and_drops_hop_by_hop_and_cf() {
        for keep in [
            "X-Hub-Signature-256",
            "X-GitHub-Event",
            "Content-Type",
            "User-Agent",
            "Content-Length",
        ] {
            assert!(forward_header(keep), "{keep}");
        }
        for drop in [
            "Host",
            "Connection",
            "Transfer-Encoding",
            "CF-Connecting-IP",
            "cf-ray",
            "X-Forwarded-For",
            "X-Real-IP",
        ] {
            assert!(!forward_header(drop), "{drop}");
        }
    }

    #[test]
    fn bucket_allows_burst_then_refuses_then_refills() {
        let l = Limits {
            rate_burst: 3.0,
            rate_per_sec: 1.0,
            ..Limits::default()
        };
        let mut b = TokenBucket::new(&l, 0.0);
        for _ in 0..3 {
            assert!(b.take(&l, 0.0).is_ok());
        }
        assert_eq!(b.take(&l, 0.0), Err(1), "burst spent");
        assert!(b.take(&l, 999.0).is_err(), "not yet refilled");
        assert!(b.take(&l, 1000.0).is_ok(), "one second refills one token");
        assert!(b.take(&l, 1000.0).is_err());
    }

    #[test]
    fn abuse_contacts_are_emails_or_https_urls() {
        for ok in [
            "abuse@example.org",
            "https://example.org/report",
            " a.b@x.example ",
            "https://x.io",
        ] {
            assert!(abuse_contact_is_valid(ok), "{ok}");
        }
        for bad in [
            "",
            "abuse",
            "abuse@",
            "@example.org",
            "a@b",
            "a@@example.org",
            "a b@example.org",
            "http://example.org/report",
            "https://",
            "https:///x.y",
            "javascript:alert(1)//x.y",
            "a@example.org
Bcc: x@y.z",
            "a@.example.org",
        ] {
            assert!(!abuse_contact_is_valid(bad), "{bad:?}");
        }
        assert!(!abuse_contact_is_valid(&format!(
            "a@{}.org",
            "x".repeat(260)
        )));
    }

    #[test]
    fn buffer_ttl_is_five_minutes() {
        // CireSnave 2026-11-16 (board 447): content is kept only long enough for an online service to receive it.
        assert_eq!(BUFFER_TTL_SECONDS, 300);
        assert_eq!(Limits::default().buffer_ttl_seconds, BUFFER_TTL_SECONDS);
    }
}
