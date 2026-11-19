//! Slice 2 over real sockets: the synthesising handler. FIXTURE names only (`fixture.test`).
//! The key case is the one hickory's file store gets wrong: `x.alice.<zone>` is NXDOMAIN
//! only when `alice` is NOT a user; for a registered user every depth resolves, and an
//! unregistered or suspended owner never inherits a wildcard (RFC 4592 / Cloudflare).

use std::{net::SocketAddr, sync::Arc, time::Duration};

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
        synth: Arc::new(synth()),
        ttl: 300,
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
