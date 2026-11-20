//! Cutover diff (`docs/DESIGN-VPS-EDGE.md` section 7): serve a zone file with our edge and ask
//! the same questions of the live authoritative server, then print where the answers differ.
//! Read-only: the zone file is read from the given path (never copied into the repo) and the
//! upstream is asked ordinary public DNS queries.
//!
//!   cargo run -p surfaceant-edge --example zone_diff -- <origin> <zone-file> <upstream-ip:53>
//!
//! Proxied (Cloudflare) records are answered with Cloudflare's own anycast addresses, so an
//! address in an `A`/`AAAA` answer is reduced to its type and count; everything else
//! (response code, AA, answer and authority record types, TXT/NS/SOA data) is compared as is.
//! Addresses are reduced to a type, not a count, so two proxied A answers equal one.

use std::{net::SocketAddr, time::Duration};

use hickory_proto::{
    op::{Message, Query},
    rr::{Name, RData, RecordType},
};
use tokio::net::UdpSocket;

async fn ask(to: SocketAddr, name: &str, t: RecordType) -> Result<Message, String> {
    let s = UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| e.to_string())?;
    let mut m = Message::query();
    m.metadata.id = 0x4242;
    m.metadata.recursion_desired = false;
    m.add_query(Query::query(
        Name::from_ascii(name).map_err(|e| e.to_string())?,
        t,
    ));
    s.send_to(&m.to_vec().map_err(|e| e.to_string())?, to)
        .await
        .map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 4096];
    let (n, _) = tokio::time::timeout(Duration::from_secs(5), s.recv_from(&mut buf))
        .await
        .map_err(|_| "timeout".to_string())?
        .map_err(|e| e.to_string())?;
    Message::from_vec(&buf[..n]).map_err(|e| e.to_string())
}

fn shape(m: &Message) -> String {
    let rec = |rs: &[hickory_proto::rr::Record]| {
        let mut v: Vec<String> = rs
            .iter()
            .map(|r| match &r.data {
                RData::A(_) => "A".to_string(),
                RData::AAAA(_) => "AAAA".to_string(),
                RData::SOA(s) => format!("SOA {} {}", s.mname, s.rname),
                other => format!("{} {}", r.record_type(), other),
            })
            .collect();
        v.sort();
        v.dedup();
        v.join(" | ")
    };
    format!(
        "{:?} aa={} ans=[{}] auth=[{}]",
        m.metadata.response_code,
        m.metadata.authoritative,
        rec(&m.answers),
        rec(&m.authorities)
    )
}

#[tokio::main]
async fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() != 3 {
        eprintln!("usage: zone_diff <origin> <zone-file> <upstream-ip:53>");
        std::process::exit(2);
    }
    let origin = a[0].trim_end_matches('.').to_string();
    let upstream: SocketAddr = a[2].parse().expect("upstream ip:port");
    // Cloudflare's export writes the SOA owner without a trailing dot, which a master-file
    // parser reads as relative (`thinkersjournal.net.thinkersjournal.net.`). Fix owners into a
    // temp copy; the original is untouched.
    let text = std::fs::read_to_string(&a[1]).expect("read zone file");
    let mut fixed = 0;
    let norm: Vec<String> = text
        .lines()
        .map(|l| {
            let owner = l.split_whitespace().next().unwrap_or("");
            if l.starts_with(|c: char| c.is_alphanumeric() || c == '*') && !owner.ends_with('.') {
                fixed += 1;
                format!("{}.{}", owner, &l[owner.len()..])
            } else {
                l.to_string()
            }
        })
        .collect();
    println!("owner names given a trailing dot: {fixed}");
    let tmp = std::env::temp_dir().join(format!("zone_diff-{}.zone", std::process::id()));
    std::fs::write(
        &tmp,
        norm.join(
            "
",
        ),
    )
    .expect("write temp zone");
    let edge = surfaceant_edge::serve(&format!("{origin}."), &tmp, "127.0.0.1:0".parse().unwrap())
        .await
        .expect("serve");
    let _ = std::fs::remove_file(&tmp);
    let local = edge.udp;

    let names: Vec<String> = [
        "",
        "www.",
        "foo.",
        "x.foo.",
        "a.b.c.foo.",
        "x.www.",
        "a.b.www.",
        "_acme-challenge.",
        "_acme-challenge.foo.",
        "ns1.",
        "mail.",
    ]
    .iter()
    .map(|p| format!("{p}{origin}."))
    .collect();

    let types = [
        RecordType::A,
        RecordType::AAAA,
        RecordType::NS,
        RecordType::SOA,
        RecordType::TXT,
        RecordType::MX,
        RecordType::CAA,
        RecordType::ANY,
    ];
    let (mut same, mut diff) = (0, 0);
    for n in &names {
        for t in types {
            let (l, u) = (ask(local, n, t).await, ask(upstream, n, t).await);
            let (ls, us) = match (&l, &u) {
                (Ok(l), Ok(u)) => (shape(l), shape(u)),
                _ => (format!("{l:?}"), format!("{u:?}")),
            };
            if ls == us {
                same += 1;
            } else {
                diff += 1;
                println!("DIFF {n} {t}\n  ours:     {ls}\n  upstream: {us}");
            }
        }
    }
    println!("{same} same, {diff} different, of {} queries", same + diff);
}
