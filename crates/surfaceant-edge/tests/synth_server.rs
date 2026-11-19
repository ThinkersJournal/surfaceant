//! Slice 2 over real sockets: the synthesising handler. FIXTURE names only (`fixture.test`).
//! The key case is the one hickory's file store gets wrong: `x.alice.<zone>` is NXDOMAIN
//! only when `alice` is NOT a user; for a registered user every depth resolves, and an
//! unregistered or suspended owner never inherits a wildcard (RFC 4592 / Cloudflare).

use std::{net::SocketAddr, time::Duration};

use hickory_proto::{
    op::{Message, Query, ResponseCode},
    rr::{Name, RData, RecordType, rdata::SOA},
};
use surfaceant_edge::{
    bind_both,
    handler::EdgeHandler,
    synth::{Data, Edge as EdgeAddrs, Registry, Synth},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpStream, UdpSocket},
};

fn synth() -> Synth {
    let mut r = Registry::default();
    r.users.insert("alice".into());
    r.users.insert("bob".into());
    r.suspended.insert("bob".into());
    r.reserved
        .insert("www".into(), vec![Data::A("192.0.2.20".parse().unwrap())]);
    Synth {
        apex: "fixture.test".into(),
        edge: EdgeAddrs {
            a: vec!["192.0.2.10".parse().unwrap()],
            aaaa: vec![],
        },
        ns: vec!["ns1.fixture.test".into()],
        caa: vec!["letsencrypt.org".into()],
        registry: r,
    }
}

fn handler() -> EdgeHandler {
    let n = |s: &str| Name::from_ascii(s).unwrap();
    EdgeHandler {
        synth: surfaceant_edge::handler::cell(synth()),
        ttl: 300,
        limiter: None,
        started: std::time::Instant::now(),
        soa: SOA::new(
            n("ns1.fixture.test."),
            n("hostmaster.fixture.test."),
            1,
            3600,
            600,
            1209600,
            300,
        ),
    }
}

fn query(name: &str, t: RecordType) -> Vec<u8> {
    let mut m = Message::query();
    m.metadata.id = 7;
    m.metadata.recursion_desired = false;
    m.add_query(Query::query(Name::from_ascii(name).unwrap(), t));
    m.to_vec().unwrap()
}

async fn udp(addr: SocketAddr, name: &str, t: RecordType) -> Message {
    let s = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    s.send_to(&query(name, t), addr).await.unwrap();
    let mut buf = vec![0u8; 4096];
    let (n, _) = tokio::time::timeout(Duration::from_secs(5), s.recv_from(&mut buf))
        .await
        .expect("reply in time")
        .unwrap();
    Message::from_vec(&buf[..n]).unwrap()
}

#[tokio::test]
async fn synthesises_at_any_depth_and_denies_the_rest() {
    let mut edge = bind_both(handler(), "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let a = edge.udp;

    for n in [
        "alice.fixture.test.",
        "x.alice.fixture.test.",
        "a.b.alice.fixture.test.",
    ] {
        let m = udp(a, n, RecordType::A).await;
        assert_eq!(m.metadata.response_code, ResponseCode::NoError, "{n}");
        assert!(m.metadata.authoritative, "{n}");
        assert!(
            matches!(m.answers[0].data, RData::A(ip) if ip.0.to_string() == "192.0.2.10"),
            "{n}"
        );
        assert_eq!(
            m.answers[0].name,
            Name::from_ascii(n).unwrap(),
            "{n}: owner is the queried name"
        );
    }
    for n in [
        "nobody.fixture.test.",
        "x.nobody.fixture.test.",
        "bob.fixture.test.",
        "x.bob.fixture.test.",
    ] {
        let m = udp(a, n, RecordType::A).await;
        assert_eq!(m.metadata.response_code, ResponseCode::NXDomain, "{n}");
        assert!(
            m.answers.is_empty() && !m.authorities.is_empty(),
            "{n}: SOA in authority"
        );
    }
    let m = udp(a, "www.fixture.test.", RecordType::AAAA).await;
    assert_eq!(m.metadata.response_code, ResponseCode::NoError);
    assert!(m.answers.is_empty(), "NODATA");
    let m = udp(a, "example.org.", RecordType::A).await;
    assert_eq!(m.metadata.response_code, ResponseCode::Refused);
    let m = udp(a, "fixture.test.", RecordType::ANY).await;
    assert_eq!(m.answers.len(), 1, "minimal ANY");

    // TCP answers too.
    let mut s = TcpStream::connect(edge.tcp).await.unwrap();
    let q = query("x.alice.fixture.test.", RecordType::A);
    s.write_all(&(q.len() as u16).to_be_bytes()).await.unwrap();
    s.write_all(&q).await.unwrap();
    let mut len = [0u8; 2];
    s.read_exact(&mut len).await.unwrap();
    let mut buf = vec![0u8; u16::from_be_bytes(len) as usize];
    s.read_exact(&mut buf).await.unwrap();
    assert_eq!(
        Message::from_vec(&buf).unwrap().metadata.response_code,
        ResponseCode::NoError
    );

    let _ = edge.server.shutdown_gracefully().await;
}

fn big_handler(limiter: Option<surfaceant_edge::rate::RateLimiter>) -> EdgeHandler {
    let mut h = handler();
    let mut s = synth();
    let vals: Vec<String> = (0..12).map(|i| format!("{i:x}").repeat(200)).collect();
    s.registry
        .acme
        .insert("_acme-challenge.alice.fixture.test".into(), vals);
    h.synth = surfaceant_edge::handler::cell(s);
    h.limiter = limiter;
    h
}

fn query_edns(name: &str, t: RecordType, payload: u16) -> Vec<u8> {
    let mut m = Message::query();
    m.metadata.id = 9;
    m.metadata.recursion_desired = false;
    m.add_query(Query::query(Name::from_ascii(name).unwrap(), t));
    let mut e = hickory_proto::op::Edns::new();
    e.set_max_payload(payload);
    m.edns = Some(e);
    m.to_vec().unwrap()
}

#[tokio::test]
async fn udp_reply_is_capped_at_1232_and_truncated_even_if_the_client_offers_4096() {
    let mut edge = bind_both(big_handler(None), "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let s = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    s.send_to(
        &query_edns("_acme-challenge.alice.fixture.test.", RecordType::TXT, 4096),
        edge.udp,
    )
    .await
    .unwrap();
    let mut buf = vec![0u8; 8192];
    let (n, _) = tokio::time::timeout(Duration::from_secs(5), s.recv_from(&mut buf))
        .await
        .expect("reply")
        .unwrap();
    assert!(n <= 1232, "UDP reply was {n} bytes");
    let m = Message::from_vec(&buf[..n]).unwrap();
    assert!(
        m.metadata.truncation,
        "TC set so the client retries over TCP"
    );
    assert_eq!(m.edns.as_ref().map(|e| e.max_payload()), Some(1232));

    // The same query over TCP is whole.
    let mut t = TcpStream::connect(edge.tcp).await.unwrap();
    let q = query("_acme-challenge.alice.fixture.test.", RecordType::TXT);
    t.write_all(&(q.len() as u16).to_be_bytes()).await.unwrap();
    t.write_all(&q).await.unwrap();
    let mut len = [0u8; 2];
    t.read_exact(&mut len).await.unwrap();
    let mut b = vec![0u8; u16::from_be_bytes(len) as usize];
    t.read_exact(&mut b).await.unwrap();
    let m = Message::from_vec(&b).unwrap();
    assert!(!m.metadata.truncation);
    assert_eq!(m.answers.len(), 12);
    let _ = edge.server.shutdown_gracefully().await;
}

#[tokio::test]
async fn over_limit_udp_gets_no_reply_but_tcp_is_not_limited() {
    use surfaceant_edge::rate::{Limit, RateLimiter};
    let lim = RateLimiter::new(
        Limit {
            burst: 2.0,
            per_sec: 0.001,
        },
        100,
    );
    let mut edge = bind_both(big_handler(Some(lim)), "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    for _ in 0..2 {
        let m = udp(edge.udp, "alice.fixture.test.", RecordType::A).await;
        assert_eq!(m.metadata.response_code, ResponseCode::NoError);
    }
    let s = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    s.send_to(&query("alice.fixture.test.", RecordType::A), edge.udp)
        .await
        .unwrap();
    let mut buf = vec![0u8; 512];
    let r = tokio::time::timeout(Duration::from_millis(400), s.recv_from(&mut buf)).await;
    assert!(
        r.is_err(),
        "third UDP query from the same source must be silently dropped"
    );

    let mut t = TcpStream::connect(edge.tcp).await.unwrap();
    let q = query("alice.fixture.test.", RecordType::A);
    t.write_all(&(q.len() as u16).to_be_bytes()).await.unwrap();
    t.write_all(&q).await.unwrap();
    let mut len = [0u8; 2];
    t.read_exact(&mut len).await.unwrap();
    assert!(u16::from_be_bytes(len) > 0, "TCP still answers");
    let _ = edge.server.shutdown_gracefully().await;
}
