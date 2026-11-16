// SPDX-License-Identifier: MIT OR Apache-2.0

//! SurfaceAnt client: holds one outbound WebSocket to the relay and replays each forwarded request,
//! untouched, to a program on this machine over plain HTTP. See `docs/DESIGN.md`.
//!
//! Delivery is at-least-once: an event is acknowledged once the local program has answered (any HTTP
//! status: the program decided, so a rejected signature is not retried forever); if the local program
//! cannot be reached the event is not acknowledged and the relay replays it after reconnect.
//! Duplicates are dropped by `delivery_id`.

use std::collections::{HashSet, VecDeque};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;

pub use surfaceant_proto::{DecodeError, Event, Frame, PROTOCOL_VERSION};

/// Boxed error used across the client.
pub type Error = Box<dyn std::error::Error + Send + Sync>;

/// What the client needs to run.
#[derive(Clone)]
pub struct Config {
    /// Relay base, `wss://host` (or `ws://host:port` for a local relay).
    pub relay: String,
    /// The channel id.
    pub channel_id: String,
    /// The connection token (sent as a Bearer header, never in the URL).
    pub token: String,
    /// Local program, `http://host:port` plus an optional base path; events are replayed under it.
    pub target: String,
}

impl std::fmt::Debug for Config {
    /// The connection token is a credential: it is never printed.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("relay", &self.relay)
            .field("channel_id", &self.channel_id)
            .field("token", &"<redacted>")
            .field("target", &self.target)
            .finish()
    }
}

/// Why a session ended.
#[derive(Debug)]
pub enum SessionEnd {
    /// Connection lost or local program unreachable: reconnect after a pause.
    Retry(Error),
    /// The relay told us to stop (`replaced`, `disabled`, bad version): do not reconnect.
    Stop(String),
}

/// Decode a relay text frame; an `event` yields `(frame, raw body)`.
pub fn decode(text: &str) -> Result<(Frame, Option<Vec<u8>>), DecodeError> {
    let frame = Frame::from_text(text)?;
    let body = match &frame {
        Frame::Event(e) => Some(e.body()?),
        _ => None,
    };
    Ok((frame, body))
}

/// The WebSocket URL for a channel: `<relay>/connect/<channel_id>`.
pub fn connect_url(relay: &str, channel_id: &str) -> String {
    format!("{}/connect/{channel_id}", relay.trim_end_matches('/'))
}

/// Remembers the most recent delivery ids so a replayed event is not delivered twice.
#[derive(Debug)]
pub struct Dedup {
    order: VecDeque<String>,
    seen: HashSet<String>,
    capacity: usize,
}

impl Dedup {
    /// A memory of at most `capacity` ids.
    pub fn new(capacity: usize) -> Self {
        Self {
            order: VecDeque::new(),
            seen: HashSet::new(),
            capacity: capacity.max(1),
        }
    }

    /// Whether `id` was already recorded.
    pub fn contains(&self, id: &str) -> bool {
        self.seen.contains(id)
    }

    /// Record `id`, forgetting the oldest beyond capacity.
    pub fn insert(&mut self, id: &str) {
        if self.seen.insert(id.to_string()) {
            self.order.push_back(id.to_string());
            if self.order.len() > self.capacity
                && let Some(old) = self.order.pop_front()
            {
                self.seen.remove(&old);
            }
        }
    }
}

/// Reconnect pause that doubles from one second up to a minute and resets after a good session.
#[derive(Debug, Default)]
pub struct Backoff {
    failures: u32,
}

impl Backoff {
    /// The pause before the next attempt (and count a failure).
    pub fn next_delay(&mut self) -> Duration {
        let secs = 1u64 << self.failures.min(6);
        self.failures = self.failures.saturating_add(1);
        Duration::from_secs(secs.min(60))
    }

    /// A session was established: start over.
    pub fn reset(&mut self) {
        self.failures = 0;
    }
}

/// Where and what to send to the local program for `event`.
#[derive(Debug, PartialEq, Eq)]
pub struct LocalRequest {
    /// `host:port` to connect to.
    pub addr: String,
    /// Request target (path and query).
    pub path_and_query: String,
    /// Header lines to send, minus the ones this client sets itself.
    pub headers: Vec<(String, String)>,
}

/// Split `http://host:port/base` into `(host:port, base path)`.
pub fn parse_target(target: &str) -> Result<(String, String), Error> {
    let rest = target
        .strip_prefix("http://")
        .ok_or("target must start with http:// (the local program is on this machine)")?;
    let (addr, base) = match rest.find('/') {
        Some(i) => (&rest[..i], rest[i..].trim_end_matches('/')),
        None => (rest, ""),
    };
    if addr.is_empty() {
        return Err("target has no host".into());
    }
    let addr = if addr.contains(':') {
        addr.to_string()
    } else {
        format!("{addr}:80")
    };
    Ok((addr, base.to_string()))
}

/// Build the local request for an event.
pub fn local_request(target: &str, e: &Event) -> Result<LocalRequest, Error> {
    let (addr, base) = parse_target(target)?;
    let mut path = format!("{base}{}", e.path_suffix);
    if path.is_empty() {
        path.push('/');
    }
    if !e.query.is_empty() {
        path.push('?');
        path.push_str(&e.query);
    }
    let headers = e
        .headers
        .iter()
        .filter(|(n, _)| {
            !matches!(
                n.to_ascii_lowercase().as_str(),
                "content-length" | "host" | "connection"
            )
        })
        .cloned()
        .collect();
    Ok(LocalRequest {
        addr,
        path_and_query: path,
        headers,
    })
}

/// Replay `event` to the local program; the HTTP status it answered with.
pub async fn deliver(target: &str, e: &Event) -> Result<u16, Error> {
    let body = e.body()?;
    if e.method.bytes().any(|b| !b.is_ascii_uppercase()) {
        return Err("event method is not a plain HTTP method".into());
    }
    let req = local_request(target, e)?;
    let mut head = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Length: {}\r\n",
        e.method,
        req.path_and_query,
        req.addr,
        body.len()
    );
    for (n, v) in &req.headers {
        if n.bytes().any(|b| b == b'\r' || b == b'\n' || b == b':')
            || v.bytes().any(|b| b == b'\r' || b == b'\n')
        {
            return Err("header with a control character refused".into());
        }
        head.push_str(&format!("{n}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let mut s = TcpStream::connect(&req.addr).await?;
    s.write_all(head.as_bytes()).await?;
    s.write_all(&body).await?;
    let mut buf = Vec::new();
    // Only the status line matters; read until the headers end or the peer closes, with a deadline.
    tokio::time::timeout(Duration::from_secs(30), async {
        let mut chunk = [0u8; 1024];
        loop {
            let n = s.read(&mut chunk).await?;
            if n == 0 || buf.windows(2).any(|w| w == b"\r\n") {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        Ok::<_, std::io::Error>(())
    })
    .await
    .map_err(|_| "local program did not answer within 30 s")??;
    let line = String::from_utf8_lossy(&buf);
    line.split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| "local program sent no HTTP status line".into())
}

/// One connection: connect, `hello`, deliver and ack until it ends.
pub async fn session(
    cfg: &Config,
    seen: &mut Dedup,
    last_acked: &mut i64,
    on_event: &(dyn Fn(&Event, Option<u16>) + Sync),
) -> SessionEnd {
    match session_inner(cfg, seen, last_acked, on_event).await {
        Ok(stop) => SessionEnd::Stop(stop),
        Err(e) => SessionEnd::Retry(e),
    }
}

async fn session_inner(
    cfg: &Config,
    seen: &mut Dedup,
    last_acked: &mut i64,
    on_event: &(dyn Fn(&Event, Option<u16>) + Sync),
) -> Result<String, Error> {
    let mut req = connect_url(&cfg.relay, &cfg.channel_id).into_client_request()?;
    req.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {}", cfg.token))?,
    );
    let (ws, _) = tokio_tungstenite::connect_async(req).await?;
    let (mut tx, mut rx) = ws.split();
    let hello = Frame::Hello {
        v: PROTOCOL_VERSION,
        last_acked_seq: *last_acked,
    };
    tx.send(Message::text(hello.to_text())).await?;
    let mut keepalive = tokio::time::interval(Duration::from_secs(25));
    keepalive.tick().await;
    loop {
        tokio::select! {
            _ = keepalive.tick() => tx.send(Message::text(Frame::Ping.to_text())).await?,
            msg = rx.next() => {
                let Some(msg) = msg else { return Err("relay closed the connection".into()) };
                match msg? {
                    Message::Text(text) => match Frame::from_text(text.as_str())? {
                        Frame::Event(e) => {
                            let status = if seen.contains(&e.delivery_id) {
                                None // replayed duplicate: already delivered, just ack
                            } else {
                                let st = deliver(&cfg.target, &e).await?; // unreachable program: no ack, reconnect
                                seen.insert(&e.delivery_id);
                                Some(st)
                            };
                            on_event(&e, status);
                            *last_acked = (*last_acked).max(e.seq);
                            tx.send(Message::text(Frame::Ack { seq: e.seq }.to_text())).await?;
                        }
                        Frame::Error { code, message } => return Ok(format!("{code}: {message}")),
                        Frame::Welcome { .. } | Frame::Pong | Frame::Ping | Frame::Hello { .. } | Frame::Ack { .. } => {}
                    },
                    Message::Close(_) => return Err("relay closed the connection".into()),
                    _ => {}
                }
            }
        }
    }
}

/// Run until the relay says stop. Reconnects with backoff.
pub async fn run(cfg: Config, on_event: impl Fn(&Event, Option<u16>) + Sync) -> String {
    let mut seen = Dedup::new(1024);
    let mut last_acked = 0;
    let mut backoff = Backoff::default();
    loop {
        match session(&cfg, &mut seen, &mut last_acked, &on_event).await {
            SessionEnd::Stop(why) => return why,
            SessionEnd::Retry(_) => tokio::time::sleep(backoff.next_delay()).await,
        }
        // A session that reached the relay is not a failure streak; a refused connect is.
        if last_acked > 0 {
            backoff.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(path: &str, query: &str) -> Event {
        Event {
            seq: 1,
            delivery_id: "d".into(),
            received_at: "2026-11-14T00:00:00Z".into(),
            method: "POST".into(),
            path_suffix: path.into(),
            query: query.into(),
            headers: vec![
                ("X-Hub-Signature-256".into(), "sha256=ab".into()),
                ("Content-Length".into(), "999".into()),
                ("Host".into(), "relay".into()),
            ],
            body_b64: String::new(),
        }
        .with_body(b"\xff\x00raw")
    }

    #[test]
    fn config_debug_never_prints_the_token() {
        let cfg = Config {
            relay: "wss://relay".into(),
            channel_id: "chan".into(),
            token: "s3cret-token".into(),
            target: "http://127.0.0.1:1".into(),
        };
        let shown = format!("{cfg:?}");
        assert!(!shown.contains("s3cret-token"), "{shown}");
        assert!(shown.contains("chan") && shown.contains("<redacted>"));
    }

    #[test]
    fn decodes_an_event_to_raw_bytes() {
        let (frame, body) = decode(&Frame::Event(ev("", "")).to_text()).unwrap();
        assert!(matches!(frame, Frame::Event(_)));
        assert_eq!(body.unwrap(), b"\xff\x00raw");
        assert_eq!(decode(&Frame::Ping.to_text()).unwrap().1, None);
    }

    #[test]
    fn connect_url_has_no_double_slash_and_no_token() {
        assert_eq!(
            connect_url("wss://r.example/", "abc"),
            "wss://r.example/connect/abc"
        );
    }

    #[test]
    fn target_parsing() {
        assert_eq!(
            parse_target("http://127.0.0.1:8787").unwrap(),
            ("127.0.0.1:8787".into(), "".into())
        );
        assert_eq!(
            parse_target("http://localhost/hook/").unwrap(),
            ("localhost:80".into(), "/hook".into())
        );
        assert!(
            parse_target("https://x").is_err(),
            "local target is plain http"
        );
        assert!(parse_target("http:///x").is_err());
    }

    #[test]
    fn local_request_keeps_signature_and_drops_framing_headers() {
        let r = local_request("http://127.0.0.1:8787/base", &ev("/x", "a=1&b=2")).unwrap();
        assert_eq!(r.path_and_query, "/base/x?a=1&b=2");
        assert_eq!(
            r.headers,
            vec![("X-Hub-Signature-256".to_string(), "sha256=ab".to_string())]
        );
        assert_eq!(
            local_request("http://h:1", &ev("", ""))
                .unwrap()
                .path_and_query,
            "/"
        );
    }

    #[test]
    fn dedup_remembers_recent_and_forgets_old() {
        let mut d = Dedup::new(2);
        d.insert("a");
        d.insert("b");
        assert!(d.contains("a") && d.contains("b"));
        d.insert("c");
        assert!(!d.contains("a"), "oldest forgotten");
        assert!(d.contains("b") && d.contains("c"));
        d.insert("c");
        assert!(d.contains("b"), "re-inserting does not evict");
    }

    #[test]
    fn backoff_doubles_caps_and_resets() {
        let mut b = Backoff::default();
        let d: Vec<u64> = (0..9).map(|_| b.next_delay().as_secs()).collect();
        assert_eq!(d, vec![1, 2, 4, 8, 16, 32, 60, 60, 60]);
        b.reset();
        assert_eq!(b.next_delay().as_secs(), 1);
    }

    #[tokio::test]
    async fn deliver_sends_exact_bytes_and_reads_status() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut got = Vec::new();
            let mut buf = [0u8; 4096];
            while !got.ends_with(b"\xff\x00raw") {
                let n = s.read(&mut buf).await.unwrap();
                got.extend_from_slice(&buf[..n]);
            }
            s.write_all(b"HTTP/1.1 204 No Content\r\n\r\n")
                .await
                .unwrap();
            got
        });
        let status = deliver(&format!("http://{addr}"), &ev("/p", ""))
            .await
            .unwrap();
        assert_eq!(status, 204);
        let got = server.await.unwrap();
        assert!(got.starts_with(b"POST /p HTTP/1.1\r\n"));
        assert!(got.windows(25).any(|w| w == b"X-Hub-Signature-256: sha2"));
        assert!(got.windows(17).any(|w| w == b"Content-Length: 5"));
        assert!(got.ends_with(b"\r\n\r\n\xff\x00raw"));
    }

    #[tokio::test]
    async fn unreachable_program_is_an_error_not_a_status() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        drop(l); // nothing listens now
        assert!(
            deliver(&format!("http://{addr}"), &ev("", ""))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn header_injection_is_refused() {
        let mut e = ev("", "");
        e.headers.push(("X-A".into(), "v\r\nX-Evil: 1".into()));
        assert!(deliver("http://127.0.0.1:1", &e).await.is_err());
    }
}
