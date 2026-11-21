//! Slice 2: the registry-backed server over real sockets, FIXTURE zone `fixture.test`.
//! A name added to `registry.db` starts resolving after a reload; a suspended one stops; a
//! corrupt reload keeps the last good registry.

use std::time::Duration;

use hickory_proto::{
    op::{Message, Query, ResponseCode},
    rr::{Name, RecordType},
};
use surfaceant_edge::{
    registry,
    service::{RegistryConfig, serve_registry},
};
use tokio::net::UdpSocket;

async fn rcode(addr: std::net::SocketAddr, name: &str) -> ResponseCode {
    let mut m = Message::query();
    m.metadata.id = 3;
    m.metadata.recursion_desired = false;
    m.add_query(Query::query(Name::from_ascii(name).unwrap(), RecordType::A));
    let s = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    s.send_to(&m.to_vec().unwrap(), addr).await.unwrap();
    let mut buf = vec![0u8; 1500];
    let (n, _) = tokio::time::timeout(Duration::from_secs(5), s.recv_from(&mut buf))
        .await
        .expect("reply")
        .unwrap();
    Message::from_vec(&buf[..n]).unwrap().metadata.response_code
}

async fn until(addr: std::net::SocketAddr, name: &str, want: ResponseCode) -> bool {
    for _ in 0..40 {
        if rcode(addr, name).await == want {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

#[tokio::test]
async fn reload_adds_suspends_and_survives_a_broken_database() {
    let dir = std::env::temp_dir().join(format!("sa-regsrv-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("registry.db");
    {
        let c = registry::open(&db).unwrap();
        c.execute("INSERT INTO names VALUES ('alice', 1, 0)", [])
            .unwrap();
    }
    let cfg = RegistryConfig {
        origin: "fixture.test.".into(),
        db: db.clone(),
        bind: "127.0.0.1:0".parse().unwrap(),
        edge_a: vec!["192.0.2.10".parse().unwrap()],
        edge_aaaa: vec![],
        ns: vec![],
        caa: vec!["letsencrypt.org".into()],
        ttl: 300,
        limit: None,
        reload: Duration::from_millis(150),
        snapshot: None,
    };
    let mut run = serve_registry(cfg).await.unwrap();
    let a = run.edge.udp;

    assert_eq!(
        rcode(a, "x.alice.fixture.test.").await,
        ResponseCode::NoError
    );
    assert_eq!(
        rcode(a, "carol.fixture.test.").await,
        ResponseCode::NXDomain
    );

    let c = registry::open(&db).unwrap();
    c.execute("INSERT INTO names VALUES ('carol', 2, 0)", [])
        .unwrap();
    assert!(
        until(a, "carol.fixture.test.", ResponseCode::NoError).await,
        "added name resolves after reload"
    );
    c.execute("UPDATE names SET suspended = 1 WHERE label = 'alice'", [])
        .unwrap();
    assert!(
        until(a, "x.alice.fixture.test.", ResponseCode::NXDomain).await,
        "suspension takes effect"
    );

    // Break the file: every reload now fails; the last good registry keeps answering.
    // Fold the WAL into the main file first: a reload racing the next lines then sees either
    // the full data or garbage, never an older database that loads successfully.
    c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .unwrap();
    drop(c);
    for ext in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(dir.join(format!("registry.db{ext}")));
    }
    std::fs::write(
        &db,
        b"this is not a sqlite database, just garbage bytes.....................",
    )
    .unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await; // several reload ticks
    assert!(
        !run.reloader.is_finished(),
        "reloader survives a failing reload"
    );
    assert_eq!(
        rcode(a, "carol.fixture.test.").await,
        ResponseCode::NoError,
        "last good registry still served"
    );
    assert_eq!(
        rcode(a, "x.alice.fixture.test.").await,
        ResponseCode::NXDomain
    );
    run.reloader.abort();
    let _ = run.reloader.await;
    let _ = run.edge.server.shutdown_gracefully().await;
    let _ = std::fs::remove_dir_all(&dir);
}
