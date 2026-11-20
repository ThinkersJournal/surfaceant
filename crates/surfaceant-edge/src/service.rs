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
        let mut tick = tokio::time::interval(cfg.reload);
        tick.tick().await; // the first tick is immediate; we just loaded
        loop {
            tick.tick().await;
            let (c, a) = (cfg.clone(), apex.clone());
            match tokio::task::spawn_blocking(move || build(&c, &a)).await {
                Ok(Ok(s)) => match cell.write() {
                    Ok(mut g) => *g = Arc::new(s),
                    Err(p) => *p.into_inner() = Arc::new(s),
                },
                Ok(Err(e)) => {
                    eprintln!("surfaceant-edge: reload failed, keeping last registry: {e}")
                }
                Err(e) => eprintln!("surfaceant-edge: reload task: {e}"),
            }
        }
    });
    Ok(Running { edge, reloader })
}
