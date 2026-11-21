//! Slice 2: the registry-backed server: `registry.db` -> [`Synth`] -> [`EdgeHandler`], with a
//! periodic reload. A reload that fails keeps serving the last good registry (names and
//! addresses stay up; changes wait), the same posture as box B without box A.

use std::{
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
    path::PathBuf,
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use hickory_proto::rr::{Name, rdata::SOA};

use crate::{
    Edge, bind_both,
    handler::{EdgeHandler, SynthCell, cell},
    rate::{Limit, RateLimiter},
    registry,
    synth::{Edge as EdgeAddrs, Synth},
};

/// Everything the registry-backed server needs.
#[derive(Clone, Debug)]
pub struct RegistryConfig {
    /// The zone apex, e.g. `thinkersjournal.net` (a trailing dot is accepted).
    pub origin: String,
    pub db: PathBuf,
    pub bind: SocketAddr,
    pub edge_a: Vec<Ipv4Addr>,
    pub edge_aaaa: Vec<Ipv6Addr>,
    /// Nameserver host names; the first is the SOA primary. Default `ns1.<origin>`, `ns2.<origin>`.
    pub ns: Vec<String>,
    /// CAA issuer domains (e.g. `letsencrypt.org`).
    pub caa: Vec<String>,
    pub ttl: u32,
    /// Per-source UDP limit; `None` = unlimited (tests only).
    pub limit: Option<Limit>,
    pub reload: Duration,
    /// Box B: where the carrier drops signed snapshots, and the key they must verify under.
    pub snapshot: Option<SnapshotIngest>,
}

/// Where B finds a snapshot file and which public key vouches for it.
#[derive(Clone, Debug)]
pub struct SnapshotIngest {
    /// A file the carrier (WireGuard transport, a later slice) writes; consumed on success or refusal.
    pub inbox: PathBuf,
    pub key: ed25519_dalek::VerifyingKey,
}

/// Install a snapshot from the inbox into `cfg.db` if one is waiting. Returns the new last
/// sequence. A refused snapshot is logged and removed so it is not retried every tick; the
/// registry in place keeps serving.
pub fn ingest(cfg: &RegistryConfig, last: Option<u64>) -> Option<u64> {
    let ing = cfg.snapshot.as_ref()?;
    let bytes = std::fs::read(&ing.inbox).ok()?;
    let tmp = cfg.db.with_extension("incoming");
    let r = crate::snapshot::verify_and_install(&bytes, &ing.key, last, &tmp, &cfg.db);
    let _ = std::fs::remove_file(&ing.inbox);
    match r {
        Ok(seq) => Some(seq),
        Err(e) => {
            eprintln!("surfaceant-edge: snapshot refused, keeping current registry: {e:?}");
            last
        }
    }
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn build(cfg: &RegistryConfig, apex: &str) -> Result<Synth, String> {
    let conn = registry::open(&cfg.db).map_err(|e| format!("registry open: {e}"))?;
    let reg = registry::load(&conn, now_unix()).map_err(|e| format!("registry load: {e}"))?;
    let ns = if cfg.ns.is_empty() {
        vec![format!("ns1.{apex}"), format!("ns2.{apex}")]
    } else {
        cfg.ns
            .iter()
            .map(|n| n.trim_end_matches('.').to_ascii_lowercase())
            .collect()
    };
    Ok(Synth {
        apex: apex.to_owned(),
        edge: EdgeAddrs {
            a: cfg.edge_a.clone(),
            aaaa: cfg.edge_aaaa.clone(),
        },
        ns,
        caa: cfg.caa.clone(),
        registry: reg,
    })
}

/// A running registry-backed server and its reload task.
pub struct Running {
    pub edge: Edge<EdgeHandler>,
    pub reloader: tokio::task::JoinHandle<()>,
}

pub async fn serve_registry(cfg: RegistryConfig) -> Result<Running, String> {
    let apex = cfg.origin.trim_end_matches('.').to_ascii_lowercase();
    let first = build(&cfg, &apex)?;
    let n = |s: String| Name::from_str(&format!("{s}.")).map_err(|e| format!("bad name {s}: {e}"));
    let soa = SOA::new(
        n(first.ns[0].clone())?,
        n(format!("hostmaster.{apex}"))?,
        now_unix() as u32,
        3600,
        600,
        1_209_600,
        cfg.ttl,
    );
    let cell: SynthCell = cell(first);
    let handler = EdgeHandler {
        synth: cell.clone(),
        ttl: cfg.ttl,
        soa,
        limiter: cfg.limit.map(|l| RateLimiter::new(l, 100_000)),
        started: Instant::now(),
    };
    let edge = bind_both(handler, cfg.bind).await?;
    let reloader = tokio::spawn(async move {
        let mut last_seq: Option<u64> = None;
        let mut tick = tokio::time::interval(cfg.reload);
        tick.tick().await; // the first tick is immediate; we just loaded
        loop {
            tick.tick().await;
            let (c, a) = (cfg.clone(), apex.clone());
            let ls = last_seq;
            match tokio::task::spawn_blocking(move || {
                let seq = ingest(&c, ls);
                build(&c, &a).map(|s| (s, seq))
            })
            .await
            {
                Ok(Ok((s, seq))) => {
                    last_seq = seq;
                    match cell.write() {
                        Ok(mut g) => *g = Arc::new(s),
                        Err(p) => *p.into_inner() = Arc::new(s),
                    }
                }
                Ok(Err(e)) => {
                    eprintln!("surfaceant-edge: reload failed, keeping last registry: {e}")
                }
                Err(e) => eprintln!("surfaceant-edge: reload task: {e}"),
            }
        }
    });
    Ok(Running { edge, reloader })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    #[test]
    fn ingest_installs_once_then_refuses_replay_and_a_wrong_key_and_always_consumes_the_inbox() {
        let d = std::env::temp_dir().join(format!("sa-ingest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let k = SigningKey::from_bytes(&[3; 32]);
        let src = registry::open(&d.join("a.db")).unwrap();
        src.execute("INSERT INTO names VALUES ('bob', 1, 0)", [])
            .unwrap();
        let snap = crate::snapshot::make(&src, &d.join("s.tmp"), 7, 1, &k).unwrap();
        let mut cfg = RegistryConfig {
            origin: "fixture.test.".into(),
            db: d.join("b.db"),
            bind: "127.0.0.1:0".parse().unwrap(),
            edge_a: vec![],
            edge_aaaa: vec![],
            ns: vec![],
            caa: vec![],
            ttl: 300,
            limit: None,
            reload: Duration::from_secs(1),
            snapshot: Some(SnapshotIngest {
                inbox: d.join("in.snap"),
                key: k.verifying_key(),
            }),
        };
        assert_eq!(ingest(&cfg, None), None, "empty inbox: nothing happens");
        std::fs::write(d.join("in.snap"), &snap).unwrap();
        assert_eq!(ingest(&cfg, None), Some(7));
        assert!(!d.join("in.snap").exists());
        assert!(
            registry::load(&registry::open(&d.join("b.db")).unwrap(), 0)
                .unwrap()
                .users
                .contains("bob")
        );
        std::fs::write(d.join("in.snap"), &snap).unwrap();
        assert_eq!(ingest(&cfg, Some(7)), Some(7), "replay refused, last kept");
        assert!(!d.join("in.snap").exists());
        cfg.snapshot.as_mut().unwrap().key = SigningKey::from_bytes(&[4; 32]).verifying_key();
        std::fs::write(d.join("in.snap"), &snap).unwrap();
        assert_eq!(ingest(&cfg, Some(1)), Some(1), "wrong key refused");
        let _ = std::fs::remove_dir_all(&d);
    }
}
