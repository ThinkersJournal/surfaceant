// SPDX-License-Identifier: MIT OR Apache-2.0

//! A `wss://` connection must reach the TLS handshake without panicking: rustls 0.23 panics at the first
//! connection unless a process-level `CryptoProvider` is selected, and a real consumer hits the same panic
//! (found by the live smoke test, 2026-11-16). The peer here accepts a TCP connection and hangs up, so the
//! handshake fails cleanly; what is asserted is "an error, not a panic".

use std::time::Duration;

use surfaceant_client::{Config, Dedup, SessionEnd, session};
use tokio::net::TcpListener;

#[tokio::test]
async fn wss_connect_reaches_the_tls_handshake_without_panicking() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        loop {
            if let Ok((s, _)) = listener.accept().await {
                drop(s);
            }
        }
    });
    let cfg = Config {
        relay: format!("wss://127.0.0.1:{port}"),
        channel_id: "c".into(),
        token: "t".into(),
        target: "http://127.0.0.1:1".into(),
    };
    let mut seen = Dedup::new(8);
    let mut last = 0;
    let end = tokio::time::timeout(
        Duration::from_secs(20),
        session(&cfg, &mut seen, &mut last, &|_, _| {}),
    )
    .await
    .expect("session returns");
    assert!(
        matches!(end, SessionEnd::Retry(_)),
        "a failed handshake is a retryable error, got {end:?}"
    );
}
