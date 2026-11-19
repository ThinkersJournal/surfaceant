//! `surfaceant-edge` has two modes, both UDP and TCP until interrupted:
//!
//! * zone file (slice 1): `--origin <zone.> --zone-file <path> [--bind <addr:port>]`
//! * registry (slice 2): `--origin <zone.> --registry <registry.db> --edge-a <ip[,ip]>
//!   [--edge-aaaa <ip[,ip]>] [--ns <host[,host]>] [--caa <issuer[,issuer]>] [--bind <addr:port>]
//!   [--rate <queries/sec/source>] [--burst <n>] [--reload-secs <n>]`
//!
//! Registry mode answers from `registry.db` per query (`docs/DESIGN-VPS-EDGE.md` 1.1) and
//! reloads it every `--reload-secs` (default 5), keeping the last good copy on a failed reload.

use std::{net::SocketAddr, path::PathBuf, process::ExitCode, time::Duration};

use surfaceant_edge::{
    rate::Limit,
    service::{RegistryConfig, serve_registry},
};

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn list<T: std::str::FromStr>(args: &[String], name: &str) -> Result<Vec<T>, String> {
    match arg(args, name) {
        None => Ok(vec![]),
        Some(v) => v
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.parse().map_err(|_| format!("bad {name} value {s:?}")))
            .collect(),
    }
}

fn num<T: std::str::FromStr>(args: &[String], name: &str, default: T) -> Result<T, String> {
    match arg(args, name) {
        None => Ok(default),
        Some(v) => v.parse().map_err(|_| format!("bad {name} value {v:?}")),
    }
}

const USAGE: &str = "usage: surfaceant-edge --origin <zone.> (--zone-file <path> | --registry <db> --edge-a <ip,..> [--edge-aaaa <ip,..>] [--ns <host,..>] [--caa <issuer,..>] [--rate <q/s>] [--burst <n>] [--reload-secs <n>]) [--bind <addr:port>]";

fn registry_config(args: &[String], origin: &str, db: String) -> Result<RegistryConfig, String> {
    let edge_a = list(args, "--edge-a")?;
    if edge_a.is_empty() {
        return Err("--edge-a is required in registry mode".into());
    }
    let rate: f64 = num(args, "--rate", 20.0)?;
    let burst: f64 = num(args, "--burst", 40.0)?;
    Ok(RegistryConfig {
        origin: origin.to_owned(),
        db: PathBuf::from(db),
        bind: num(args, "--bind", "127.0.0.1:5353".parse().unwrap())?,
        edge_a,
        edge_aaaa: list(args, "--edge-aaaa")?,
        ns: list(args, "--ns")?,
        caa: {
            let c: Vec<String> = list(args, "--caa")?;
            if c.is_empty() {
                vec!["letsencrypt.org".into()]
            } else {
                c
            }
        },
        ttl: num(args, "--ttl", 300)?,
        limit: (rate > 0.0).then_some(Limit {
            burst,
            per_sec: rate,
        }),
        reload: Duration::from_secs(num(args, "--reload-secs", 5)?),
    })
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let Some(origin) = arg(&args, "--origin") else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    if let Some(db) = arg(&args, "--registry") {
        let cfg = match registry_config(&args, &origin, db) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("surfaceant-edge: {e}\n{USAGE}");
                return ExitCode::from(2);
            }
        };
        let mut run = match serve_registry(cfg).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("surfaceant-edge: {e}");
                return ExitCode::FAILURE;
            }
        };
        eprintln!(
            "surfaceant-edge: serving {origin} from the registry on udp {} and tcp {}",
            run.edge.udp, run.edge.tcp
        );
        let _ = tokio::signal::ctrl_c().await;
        run.reloader.abort();
        let _ = run.edge.server.shutdown_gracefully().await;
        return ExitCode::SUCCESS;
    }
    let Some(zone) = arg(&args, "--zone-file") else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let bind: SocketAddr = match arg(&args, "--bind")
        .unwrap_or_else(|| "127.0.0.1:5353".into())
        .parse()
    {
        Ok(b) => b,
        Err(e) => {
            eprintln!("bad --bind: {e}");
            return ExitCode::from(2);
        }
    };
    let mut edge = match surfaceant_edge::serve(&origin, &PathBuf::from(zone), bind).await {
        Ok(e) => e,
        Err(e) => {
            eprintln!("surfaceant-edge: {e}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!(
        "surfaceant-edge: serving {origin} on udp {} and tcp {}",
        edge.udp, edge.tcp
    );
    let _ = tokio::signal::ctrl_c().await;
    let _ = edge.server.shutdown_gracefully().await;
    ExitCode::SUCCESS
}
