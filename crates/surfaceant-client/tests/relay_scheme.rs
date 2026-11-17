// SPDX-License-Identifier: MIT OR Apache-2.0

//! CireSnave's rule (2026-11-17, `docs/DESIGN-VPS-EDGE.md` 3.6; PM ruling LOOPBACK-ONLY): the client accepts
//! `ws://` only when the relay host RESOLVES to a loopback address; anything else must be `wss://` with a valid
//! certificate. The check is on the resolved address, never the string, so `localhost.evil.example` cannot pass.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use surfaceant_client::{Config, Dedup, SessionEnd, check_ws_addrs, session};
use tokio::net::TcpListener;

fn cfg(relay: String) -> Config {
    Config {
        relay,
        channel_id: "c".into(),
        token: "t".into(),
        target: "http://127.0.0.1:1".into(),
    }
}

async fn run(relay: String) -> SessionEnd {
    let mut seen = Dedup::new(8);
    let mut last = 0;
    tokio::time::timeout(
        Duration::from_secs(20),
        session(&cfg(relay), &mut seen, &mut last, &|_, _| {}),
    )
    .await
    .expect("session returns")
}

/// A listener that accepts and hangs up, so a connect that is allowed fails later and cleanly.
async fn hangup_listener() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            if let Ok((s, _)) = l.accept().await {
                drop(s);
            }
        }
    });
    port
}

fn refused(end: &SessionEnd) -> &str {
    match end {
        SessionEnd::Stop(m) if m.contains("loopback") && m.contains("wss://") => m,
        other => panic!("expected a refusal naming the rule, got {other:?}"),
    }
}

#[tokio::test]
async fn ws_to_a_public_address_is_refused_naming_the_rule() {
    // TEST-NET-3: never contacted, the refusal comes before any connect.
    let end = run("ws://203.0.113.5:9".into()).await;
    refused(&end);
}

#[test]
fn a_name_that_resolves_to_a_private_non_loopback_address_is_refused() {
    for ip in [
        "10.0.0.5:80",
        "192.168.1.1:80",
        "172.16.0.9:80",
        "169.254.169.254:80",
        "[fd00::1]:80",
        "[fe80::1]:80",
        "100.64.0.1:80",
    ] {
        let a: SocketAddr = ip.parse().unwrap();
        // The name looks like localhost; only the resolved address counts.
        let e = check_ws_addrs("localhost.evil.example", &[a]).expect_err(ip);
        assert!(e.to_string().contains("loopback"), "{e}");
    }
}

#[test]
fn every_resolved_address_must_be_loopback() {
    let mixed: Vec<SocketAddr> = vec![
        "127.0.0.1:80".parse().unwrap(),
        "10.0.0.5:80".parse().unwrap(),
    ];
    assert!(
        check_ws_addrs("localhost", &mixed).is_err(),
        "one non-loopback answer refuses the name"
    );
    assert!(
        check_ws_addrs("localhost", &[]).is_err(),
        "no answer is a refusal, not a pass"
    );
}

#[test]
fn loopback_addresses_are_accepted() {
    for ip in ["127.0.0.1:80", "127.255.255.254:80", "[::1]:80"] {
        let a: SocketAddr = ip.parse().unwrap();
        check_ws_addrs("localhost", &[a]).expect(ip); // positive control for the refusals above
    }
}

#[tokio::test]
async fn ws_to_loopback_is_accepted_and_reaches_the_connection() {
    let port = hangup_listener().await;
    for host in ["127.0.0.1", "localhost"] {
        let end = run(format!("ws://{host}:{port}")).await;
        // Allowed: the connect proceeds and the hang-up is a retryable error, not a rule refusal.
        assert!(matches!(end, SessionEnd::Retry(_)), "{host}: {end:?}");
    }
}

#[tokio::test]
async fn a_lookalike_name_does_not_pass_on_its_string() {
    // `.invalid` never resolves, so the lookup fails: a retryable error, never a connection and never a pass
    // because the string starts with `localhost`.
    let end = run("ws://localhost.evil.example.invalid:9".into()).await;
    assert!(matches!(end, SessionEnd::Retry(_)), "{end:?}");
}

#[tokio::test]
async fn wss_to_an_invalid_certificate_is_refused() {
    // A TLS server with a self-signed certificate for 127.0.0.1 that no root vouches for.
    let ck = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string()]).expect("cert");
    let cert = ck.cert.der().clone();
    let key = rustls_pki_types_key(&ck);
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    let server_cfg = tokio_rustls::rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .expect("server config");
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server_cfg));
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (s, _) = l.accept().await.unwrap();
        acceptor
            .accept(s)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    });
    let end = run(format!("wss://127.0.0.1:{port}")).await;
    let SessionEnd::Retry(err) = &end else {
        panic!("expected a refused connection, got {end:?}")
    };
    // Control: the SERVER side saw the client abort the handshake (certificate rejected), so the failure
    // is certificate validation and not a dead server.
    let server_side = server.await.unwrap();
    assert!(
        server_side.is_err(),
        "server handshake should fail when the client rejects the certificate"
    );
    let msg = err.to_string().to_lowercase();
    assert!(
        msg.contains("certificate") || msg.contains("unknownissuer") || msg.contains("invalid"),
        "{err}"
    );
}

fn rustls_pki_types_key(
    ck: &rcgen::CertifiedKey<rcgen::KeyPair>,
) -> tokio_rustls::rustls::pki_types::PrivateKeyDer<'static> {
    tokio_rustls::rustls::pki_types::PrivateKeyDer::Pkcs8(ck.signing_key.serialize_der().into())
}

#[tokio::test]
async fn ws_to_loopback_completes_a_real_websocket_handshake_and_sends_hello() {
    // The path the e2e job and `wrangler dev` use: connect to the checked address, upgrade, send `hello`.
    use futures_util::StreamExt;
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (s, _) = l.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(s)
            .await
            .expect("websocket upgrade");
        ws.next()
            .await
            .expect("a frame")
            .expect("ok")
            .into_text()
            .expect("text")
            .to_string()
    });
    let mut seen = Dedup::new(8);
    let mut last = 0;
    let cfg = cfg(format!("ws://127.0.0.1:{port}"));
    let client = tokio::spawn(async move {
        let _ = session(&cfg, &mut seen, &mut last, &|_, _| {}).await;
    });
    let first = tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .expect("server in time")
        .unwrap();
    assert!(
        first.contains("hello"),
        "first frame from the client: {first}"
    );
    client.abort();
}
