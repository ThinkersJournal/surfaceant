// SPDX-License-Identifier: MIT OR Apache-2.0
//! Post-deploy smoke test against a LIVE relay (https/wss). Row 4.
//!
//! Env: `SMOKE_RELAY` (`https://host`, or `http://127.0.0.1:port` for `wrangler dev`; no trailing slash) and `SMOKE_ADMIN_SECRET`.
//! Uses the system `curl` for the HTTPS calls (the client crate speaks WebSocket only); the admin secret
//! goes to curl on stdin, never argv. Provisions a throwaway channel, exercises offline buffer + replay,
//! live delivery and a forged id, prints latency numbers, then disables the channel.
//! Latency here is client-observed (includes the network); Worker CPU time is read from the dashboard
//! (see docs/DEPLOY.md), since the Worker cannot report its own CPU.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use surfaceant_client::{Config, run};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn curl(
    method: &str,
    url: &str,
    auth: Option<&str>,
    body: &str,
    extra: &[(&str, &str)],
) -> (u16, String) {
    let mut cfg = String::new();
    if let Some(a) = auth {
        cfg.push_str(&format!("header = \"Authorization: Bearer {a}\"\n"));
    }
    for (k, v) in extra {
        cfg.push_str(&format!("header = \"{k}: {v}\"\n"));
    }
    let mut child = Command::new("curl")
        .args([
            "-sS",
            "--config",
            "-",
            "-X",
            method,
            "--max-time",
            "20",
            "-w",
            "\n%{http_code}",
            "--data-binary",
            body,
            url,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("curl on PATH");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(cfg.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let (b, code) = text.rsplit_once('\n').unwrap_or(("", "0"));
    (code.trim().parse().unwrap_or(0), b.to_string())
}

fn check(cond: bool, what: &str) {
    if cond {
        println!("ok   {what}");
    } else {
        eprintln!("FAIL {what}");
        std::process::exit(1);
    }
}

/// A local program that answers 200 to everything.
async fn local_ok() -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 8192];
                let _ = s.read(&mut buf).await;
                let _ = s
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await;
            });
        }
    });
    format!("http://{addr}")
}

#[tokio::main]
async fn main() {
    let relay = std::env::var("SMOKE_RELAY").expect("SMOKE_RELAY=https://host");
    let admin = std::env::var("SMOKE_ADMIN_SECRET").expect("SMOKE_ADMIN_SECRET");
    let ws = relay
        .replacen("https://", "wss://", 1)
        .replacen("http://", "ws://", 1);

    let (st, body) = curl(
        "POST",
        &format!("{relay}/admin/channels"),
        Some(&admin),
        "",
        &[],
    );
    check(st == 201, "admin provisions a channel");
    let v: serde_json::Value = serde_json::from_str(&body).expect("provision json");
    let id = v["channel_id"].as_str().unwrap().to_string();
    let token = v["token"].as_str().unwrap().to_string();

    let forged = "00".repeat(32);
    let (st, _) = curl("POST", &format!("{relay}/hooks/{forged}"), None, "x", &[]);
    check(st == 404, "a forged channel id is 404");
    let (st, _) = curl("POST", &format!("{relay}/admin/channels"), None, "", &[]);
    check(st == 401, "admin without the secret is 401");

    let post = |n: u32| {
        let t = Instant::now();
        let (st, _) = curl(
            "POST",
            &format!("{relay}/hooks/{id}?n={n}"),
            None,
            &format!("{{\"n\":{n}}}"),
            &[
                ("X-GitHub-Delivery", &format!("smoke-{n}")),
                ("Content-Type", "application/json"),
            ],
        );
        (st, t.elapsed())
    };

    // Offline: buffered until the client connects.
    for n in 1..=2 {
        check(
            post(n).0 == 202,
            &format!("offline event {n} accepted (202 after durable write)"),
        );
    }
    let (dup_st, _) = post(2);
    check(
        dup_st == 200,
        "a repeated delivery id is answered 200 and not stored again",
    );

    let got: Arc<Mutex<Vec<(i64, Instant)>>> = Arc::default();
    let sink = got.clone();
    let cfg = Config {
        relay: ws,
        channel_id: id.clone(),
        token,
        target: local_ok().await,
    };
    let started = Instant::now();
    tokio::spawn(async move {
        run(cfg, move |e, status| {
            if status.is_some() {
                sink.lock().unwrap().push((e.seq, Instant::now()));
            }
        })
        .await
    });
    let wait = |want: usize| {
        let got = got.clone();
        async move {
            let t = Instant::now();
            while got.lock().unwrap().len() < want {
                if t.elapsed() > Duration::from_secs(20) {
                    return false;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            true
        }
    };
    check(
        wait(2).await,
        "client receives the 2 buffered events exactly once (duplicate suppressed)",
    );
    println!(
        "info connect + replay of 2 buffered events: {:?}",
        started.elapsed()
    );

    let mut post_ms = Vec::new();
    let mut e2e_ms = Vec::new();
    for n in 10..20 {
        let before = got.lock().unwrap().len();
        let sent = Instant::now();
        let (st, d) = post(n);
        check(st == 202, &format!("live event {n} accepted"));
        post_ms.push(d.as_millis());
        check(wait(before + 1).await, &format!("live event {n} delivered"));
        e2e_ms.push(sent.elapsed().as_millis());
    }
    check(
        got.lock().unwrap().len() == 12,
        "exactly 12 distinct deliveries in total",
    );
    post_ms.sort();
    e2e_ms.sort();
    println!(
        "info POST->202 ms: median {} max {} (includes curl start-up and TLS)",
        post_ms[5], post_ms[9]
    );
    println!(
        "info POST->delivered-to-local-program ms: median {} max {}",
        e2e_ms[5], e2e_ms[9]
    );

    let (st, _) = curl(
        "POST",
        &format!("{relay}/admin/channels/{id}/disable"),
        Some(&admin),
        "",
        &[],
    );
    check(st == 200 || st == 204, "admin disables the channel");
    let (st, _) = post(99);
    check(st != 202, "a disabled channel no longer accepts events");
    println!("smoke OK (channel {id} disabled)");
}
