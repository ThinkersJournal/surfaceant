// SPDX-License-Identifier: MIT OR Apache-2.0

//! End-to-end check: relay Worker (`wrangler dev`) -> `surfaceant-client` -> a GitHub-style receiver.
//!
//! Env: `E2E_RELAY` (host:port of the running relay, default 127.0.0.1:8787) and `E2E_ADMIN_SECRET`.
//! By default an in-process receiver verifies `X-Hub-Signature-256` over the raw body and records
//! the bytes it saw. To run against `tools/gh_webhook_receiver.py` instead, set `E2E_TARGET`
//! (`http://127.0.0.1:<port>/<secret path>`) and `E2E_SECRET` (its webhook secret); the example then
//! checks the statuses that script answers (202 stored, 401 bad signature).
//!
//! Exits non-zero on the first failed check, so a CI job running it cannot pass by skipping.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use surfaceant_client::{Config, Event};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

type Seen = Arc<Mutex<Vec<(String, Option<u16>)>>>;

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn sign(secret: &[u8], body: &[u8]) -> String {
    let mut m = Hmac::<Sha256>::new_from_slice(secret).unwrap();
    m.update(body);
    format!("sha256={}", hex(&m.finalize().into_bytes()))
}

/// Minimal HTTP/1.1 round trip: (status, response body).
async fn http(
    addr: &str,
    method: &str,
    path: &str,
    headers: &[(&str, String)],
    body: &[u8],
) -> (u16, Vec<u8>) {
    let mut s = TcpStream::connect(addr).await.expect("connect");
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (n, v) in headers {
        head.push_str(&format!("{n}: {v}\r\n"));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).await.unwrap();
    s.write_all(body).await.unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).await.unwrap();
    let split = out
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map_or(out.len(), |i| i + 4);
    let status = String::from_utf8_lossy(&out[..split])
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .expect("status");
    (status, out[split..].to_vec())
}

/// In-process stand-in for the GitHub receiver: 202 when the signature matches, else 401.
async fn receiver(secret: Vec<u8>, bodies: Arc<Mutex<Vec<Vec<u8>>>>) -> String {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = l.accept().await.unwrap();
            let (secret, bodies) = (secret.clone(), bodies.clone());
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let (head_end, len) = loop {
                    let n = s.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..i]).to_ascii_lowercase();
                        let len = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        break (i + 4, len);
                    }
                };
                while buf.len() < head_end + len {
                    let n = s.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let body = buf[head_end..head_end + len].to_vec();
                let sig = head.lines().find_map(|l| {
                    let (n, v) = l.split_once(':')?;
                    n.eq_ignore_ascii_case("x-hub-signature-256")
                        .then(|| v.trim().to_string())
                });
                let ok = sig.as_deref() == Some(sign(&secret, &body).as_str());
                if ok {
                    bodies.lock().unwrap().push(body);
                }
                let status = if ok {
                    "202 Accepted"
                } else {
                    "401 Unauthorized"
                };
                let _ = s
                    .write_all(
                        format!(
                            "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        )
                        .as_bytes(),
                    )
                    .await;
            });
        }
    });
    format!("http://{addr}")
}

fn check(cond: bool, what: &str) {
    if cond {
        println!("ok   {what}");
    } else {
        eprintln!("FAIL {what}");
        std::process::exit(1);
    }
}

async fn wait_for(seen: &Seen, n: usize) {
    for _ in 0..200 {
        if seen.lock().unwrap().len() >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    eprintln!(
        "FAIL timed out waiting for {n} delivered events, got {:?}",
        seen.lock().unwrap()
    );
    std::process::exit(1);
}

#[tokio::main]
async fn main() {
    let relay = std::env::var("E2E_RELAY").unwrap_or_else(|_| "127.0.0.1:8787".into());
    let admin = std::env::var("E2E_ADMIN_SECRET").expect("E2E_ADMIN_SECRET");
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let (target, secret, external) = match std::env::var("E2E_TARGET") {
        Ok(t) => (
            t,
            std::env::var("E2E_SECRET")
                .expect("E2E_SECRET with E2E_TARGET")
                .into_bytes(),
            true,
        ),
        Err(_) => {
            let secret = b"e2e-webhook-secret-0123456789abcdef".to_vec();
            (
                receiver(secret.clone(), bodies.clone()).await,
                secret,
                false,
            )
        }
    };

    let (st, resp) = http(
        &relay,
        "POST",
        "/admin/channels",
        &[("Authorization", format!("Bearer {admin}"))],
        b"",
    )
    .await;
    check(st == 201, "admin provisions a channel");
    let v: serde_json::Value = serde_json::from_slice(&resp).unwrap();
    let (id, token) = (
        v["channel_id"].as_str().unwrap().to_string(),
        v["token"].as_str().unwrap().to_string(),
    );

    // GitHub-shaped payloads (UTF-8 with non-ASCII and a trailing newline, as GitHub sends).
    let good = "{\"action\":\"opened\",\"repository\":{\"full_name\":\"ThinkersJournal/surfaceant\"},\"sender\":{\"login\":\"octo\u{e9}\"},\"pull_request\":{\"number\":1,\"state\":\"open\",\"head\":{\"sha\":\"abc\"},\"base\":{\"ref\":\"main\"}}}\n".as_bytes().to_vec();
    let mut tampered = good.clone();
    tampered[20] ^= 0x01; // one byte changed after signing
    let post = |delivery: &'static str, body: Vec<u8>, signed_over: Vec<u8>| {
        let (relay, id, secret) = (relay.clone(), id.clone(), secret.clone());
        async move {
            let h = [
                ("X-GitHub-Delivery", delivery.to_string()),
                ("X-GitHub-Event", "pull_request".to_string()),
                ("X-Hub-Signature-256", sign(&secret, &signed_over)),
                ("Content-Type", "application/json".to_string()),
            ];
            http(&relay, "POST", &format!("/hooks/{id}"), &h, &body)
                .await
                .0
        }
    };

    // Offline: the client is not connected yet, so the relay buffers these.
    check(
        post("e2e-good", good.clone(), good.clone()).await == 202,
        "relay accepts the signed event while the client is offline",
    );
    check(
        post("e2e-bad", tampered.clone(), good.clone()).await == 202,
        "relay accepts the tampered event too (it never verifies the sender's signature)",
    );
    check(
        post("e2e-good", good.clone(), good.clone()).await == 200,
        "a repeated delivery id is answered 200 and not stored twice",
    );

    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let cb_seen = seen.clone();
    let cfg = Config {
        relay: format!("ws://{relay}"),
        channel_id: id.clone(),
        token,
        target,
    };
    let client = tokio::spawn(surfaceant_client::run(cfg, move |e: &Event, st| {
        cb_seen.lock().unwrap().push((e.delivery_id.clone(), st))
    }));

    wait_for(&seen, 2).await;
    {
        let s = seen.lock().unwrap();
        check(
            s[0] == ("e2e-good".into(), Some(202)),
            "buffered signed event replays first and the receiver accepts it (202)",
        );
        check(
            s[1] == ("e2e-bad".into(), Some(401)),
            "negative control: the tampered byte is rejected by the receiver (401)",
        );
    }
    if !external {
        check(
            bodies.lock().unwrap().as_slice() == [good.clone()],
            "the receiver saw exactly the posted bytes, and only the untampered event",
        );
    }
    // A live event after the client is connected.
    check(
        post("e2e-live", good.clone(), good.clone()).await == 202,
        "relay accepts a live event",
    );
    wait_for(&seen, 3).await;
    check(
        seen.lock().unwrap()[2] == ("e2e-live".into(), Some(202)),
        "the live event is delivered (202)",
    );
    check(
        seen.lock().unwrap().len() == 3,
        "nothing was delivered twice",
    );

    let (st, _) = http(
        &relay,
        "POST",
        &format!("/admin/channels/{id}/disable"),
        &[("Authorization", format!("Bearer {admin}"))],
        b"",
    )
    .await;
    check(st == 204, "admin disables the channel");
    let why = tokio::time::timeout(Duration::from_secs(10), client)
        .await
        .expect("client stops")
        .unwrap();
    check(
        why.starts_with("disabled"),
        "the client stops when told the channel is disabled",
    );
    println!("e2e ok");
}
