//! Slice 1 tests: the server answers from the FIXTURE zone `fixture.test` (made-up data, see
//! `fixtures/fixture.test.zone`) over real UDP and TCP sockets. Raw wire messages, no resolver.

use std::{net::SocketAddr, path::PathBuf};

use hickory_proto::{
    op::{Message, Query, ResponseCode},
    rr::{Name, RData, RecordType},
};
use surfaceant_edge::{Edge, serve};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpStream, UdpSocket},
};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/fixture.test.zone")
}

async fn start() -> Edge {
    serve("fixture.test.", &fixture(), "127.0.0.1:0".parse().unwrap())
        .await
        .expect("serve")
}

async fn start_nowild() -> Edge {
    let f = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/nowild.test.zone");
    serve("nowild.test.", &f, "127.0.0.1:0".parse().unwrap())
        .await
        .expect("serve")
}

fn query(name: &str, rtype: RecordType) -> Vec<u8> {
    let mut m = Message::query();
    m.metadata.id = 0x5a17;
    m.metadata.recursion_desired = false;
    m.add_query(Query::query(Name::from_ascii(name).unwrap(), rtype));
    m.to_vec().unwrap()
}

async fn udp(addr: SocketAddr, name: &str, rtype: RecordType) -> Message {
    let s = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    s.send_to(&query(name, rtype), addr).await.unwrap();
    let mut buf = vec![0u8; 4096];
    let (n, _) = tokio::time::timeout(std::time::Duration::from_secs(5), s.recv_from(&mut buf))
        .await
        .expect("udp reply in time")
        .unwrap();
    Message::from_vec(&buf[..n]).unwrap()
}

async fn tcp(addr: SocketAddr, name: &str, rtype: RecordType) -> Message {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let q = query(name, rtype);
    s.write_all(&(q.len() as u16).to_be_bytes()).await.unwrap();
    s.write_all(&q).await.unwrap();
    let mut len = [0u8; 2];
    s.read_exact(&mut len).await.unwrap();
    let mut buf = vec![0u8; u16::from_be_bytes(len) as usize];
    s.read_exact(&mut buf).await.unwrap();
    Message::from_vec(&buf).unwrap()
}

#[tokio::test]
async fn apex_soa_ns_a_caa() {
    let e = start().await;
    let soa = udp(e.udp, "fixture.test.", RecordType::SOA).await;
    assert_eq!(soa.metadata.response_code, ResponseCode::NoError);
    assert!(soa.metadata.authoritative, "AA bit");
    assert!(matches!(soa.answers[0].data, RData::SOA(_)));

    let ns = udp(e.udp, "fixture.test.", RecordType::NS).await;
    assert_eq!(ns.answers.len(), 2);
    let a = udp(e.udp, "fixture.test.", RecordType::A).await;
    assert_eq!(a.answers[0].data.to_string(), "192.0.2.10");
    let caa = udp(e.udp, "fixture.test.", RecordType::CAA).await;
    assert_eq!(caa.answers.len(), 1);
}

#[tokio::test]
async fn named_record_wins_over_wildcard_and_gives_nodata() {
    let e = start().await;
    let a = udp(e.udp, "alice.fixture.test.", RecordType::A).await;
    assert_eq!(a.answers[0].data.to_string(), "192.0.2.30");
    // alice exists with an A only: AAAA is NODATA (NoError, empty), not the wildcard.
    let aaaa = udp(e.udp, "alice.fixture.test.", RecordType::AAAA).await;
    assert_eq!(aaaa.metadata.response_code, ResponseCode::NoError);
    assert!(aaaa.answers.is_empty());
    assert!(
        !aaaa.authorities.is_empty(),
        "SOA in authority for negative caching"
    );
}

#[tokio::test]
async fn wildcard_matches_a_direct_child_of_the_apex() {
    let e = start().await;
    let w = udp(e.udp, "anything.fixture.test.", RecordType::A).await;
    assert_eq!(w.metadata.response_code, ResponseCode::NoError);
    assert_eq!(w.answers[0].data.to_string(), "192.0.2.99");
    // The synthesised answer is owned by the queried name, not by `*`.
    assert_eq!(w.answers[0].name.to_ascii(), "anything.fixture.test.");
}

/// FINDING (hickory-server 0.26.3, file store): the apex wildcard answers at ANY depth, even
/// below a name that exists. RFC 4592 says `x.alice` must be NXDOMAIN once `alice` exists
/// (the closest encloser is `alice`, and `*.alice` does not exist). Cloudflare follows the RFC,
/// so a static zone copy would differ from Cloudflare here; slice 2's synthesis, which decides
/// per query, is how DESIGN-VPS-EDGE section 1.1 avoids it. This test pins today's behaviour:
/// if a hickory upgrade makes it fail, the upgrade fixed the deviation; re-read section 1.1.
#[tokio::test]
async fn hickory_file_store_wildcard_matches_at_any_depth_unlike_rfc4592() {
    let e = start().await;
    for name in [
        "x.alice.fixture.test.",
        "a.www.fixture.test.",
        "a.b.c.fixture.test.",
    ] {
        let r = udp(e.udp, name, RecordType::A).await;
        assert_eq!(r.metadata.response_code, ResponseCode::NoError, "{name}");
        assert_eq!(r.answers[0].data.to_string(), "192.0.2.99", "{name}");
    }
}

#[tokio::test]
async fn nxdomain_for_an_unknown_name_in_a_zone_without_a_wildcard() {
    let e = start_nowild().await;
    let r = udp(e.udp, "nobody.nowild.test.", RecordType::A).await;
    assert_eq!(r.metadata.response_code, ResponseCode::NXDomain);
    assert!(r.metadata.authoritative);
    assert!(r.answers.is_empty());
    assert!(
        !r.authorities.is_empty(),
        "SOA in authority for negative caching"
    );
    // A name that exists still answers in the same zone (positive control for the NXDOMAIN).
    let ok = udp(e.udp, "alice.nowild.test.", RecordType::A).await;
    assert_eq!(ok.answers[0].data.to_string(), "192.0.2.30");
}

#[tokio::test]
async fn tcp_answers_the_same() {
    let e = start().await;
    let a = tcp(e.tcp, "www.fixture.test.", RecordType::A).await;
    assert_eq!(a.answers[0].data.to_string(), "192.0.2.20");
    let n = start_nowild().await;
    let nx = tcp(n.tcp, "nobody.nowild.test.", RecordType::A).await;
    assert_eq!(nx.metadata.response_code, ResponseCode::NXDomain);
}

#[tokio::test]
async fn large_answer_truncates_on_udp_and_is_whole_on_tcp() {
    let e = start().await;
    let u = udp(e.udp, "big.fixture.test.", RecordType::TXT).await;
    assert!(
        u.metadata.truncation,
        "TC bit set when the answer exceeds 512 bytes (no EDNS)"
    );
    assert!(
        u.to_vec().unwrap().len() <= 512,
        "UDP reply stays within 512 bytes"
    );
    let t = tcp(e.tcp, "big.fixture.test.", RecordType::TXT).await;
    assert!(!t.metadata.truncation);
    assert_eq!(t.answers.len(), 4);
}

#[tokio::test]
async fn out_of_zone_is_refused() {
    let e = start().await;
    let r = udp(e.udp, "example.org.", RecordType::A).await;
    assert_eq!(r.metadata.response_code, ResponseCode::Refused);
    assert!(r.answers.is_empty());
    let t = tcp(e.tcp, "example.org.", RecordType::A).await;
    assert_eq!(t.metadata.response_code, ResponseCode::Refused);
}

/// Direct-mode design question (DESIGN-VPS-EDGE "Direct mode" 2): two A records at one name.
/// Measured on hickory-server 0.26.3: the order of the answers over 8 queries. Pinned as a
/// set (both present, one RRset, one TTL); the ORDER is printed, not asserted, because DNS
/// gives no priority between records and clients may reorder anyway (RFC 1034 s3.6).
#[tokio::test]
async fn two_a_records_are_one_rrset_with_no_priority() {
    let e = start().await;
    let mut orders = Vec::new();
    for _ in 0..8 {
        let r = udp(e.udp, "multi.fixture.test.", RecordType::A).await;
        assert_eq!(r.answers.len(), 2);
        assert_eq!(r.answers[0].ttl, r.answers[1].ttl, "one RRset, one TTL");
        orders.push(
            r.answers
                .iter()
                .map(|a| a.data.to_string())
                .collect::<Vec<_>>()
                .join(","),
        );
    }
    println!("orders over 8 queries: {orders:?}");
    let mut all: Vec<_> = orders
        .iter()
        .flat_map(|o| o.split(',').map(String::from))
        .collect();
    all.sort();
    all.dedup();
    assert_eq!(all, ["192.0.2.41", "192.0.2.42"]);
}
