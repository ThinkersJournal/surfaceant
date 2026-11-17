//! `surfaceant-edge --origin <zone.> --zone-file <path> [--bind <addr:port>]`
//!
//! Serves one zone from a standard master file on UDP and TCP until interrupted. The zone
//! file is a config input: a fixture in tests, the real export at cutover.

use std::{net::SocketAddr, path::PathBuf, process::ExitCode};

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let (Some(origin), Some(zone)) = (arg(&args, "--origin"), arg(&args, "--zone-file")) else {
        eprintln!(
            "usage: surfaceant-edge --origin <zone.> --zone-file <path> [--bind <addr:port>]"
        );
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
