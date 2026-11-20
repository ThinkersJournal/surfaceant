//! SurfaceAnt VPS edge, slice 1: an authoritative DNS server for one zone loaded from a
//! standard master file (`docs/DESIGN-VPS-EDGE.md` section 9). Later slices replace the file
//! with database-backed synthesis; the server wiring here stays.

pub mod handler;
pub mod rate;
pub mod registry;
pub mod service;
pub mod synth;

use std::{net::SocketAddr, path::Path, sync::Arc, time::Duration};

use tokio::net::{TcpListener, UdpSocket};

use hickory_server::{
    Server,
    proto::rr::{LowerName, Name},
    store::file::{FileConfig, FileZoneHandler},
    zone_handler::{AxfrPolicy, Catalog, ZoneType},
};

/// Idle time after which a TCP connection that sent nothing is closed.
pub const TCP_IDLE: Duration = Duration::from_secs(5);

/// Most concurrent TCP connections hickory accepts; beyond it new connections wait.
pub const TCP_MAX_CONNECTIONS: usize = 4096;

/// A running server and the addresses it actually bound (port 0 resolves here).
pub struct Edge<H: hickory_server::server::RequestHandler = Catalog> {
    pub server: Server<H>,
    pub udp: SocketAddr,
    pub tcp: SocketAddr,
}

/// Load `zone_file` as the zone `origin` and serve it on UDP and TCP at `bind`.
pub async fn serve(origin: &str, zone_file: &Path, bind: SocketAddr) -> Result<Edge, String> {
    let origin = Name::from_ascii(origin).map_err(|e| format!("bad origin: {e}"))?;
    let handler = FileZoneHandler::try_from_config(
        origin.clone(),
        ZoneType::Primary,
        AxfrPolicy::Deny,
        None,
        &FileConfig {
            zone_path: zone_file.to_owned(),
        },
    )?;
    let mut catalog = Catalog::new();
    catalog.upsert(LowerName::new(&origin), vec![Arc::new(handler)]);
    bind_both(catalog, bind).await
}

/// Serve any handler (slice 2: [`handler::EdgeHandler`]) on UDP and TCP at `bind`.
pub async fn bind_both<H: hickory_server::server::RequestHandler>(
    handler: H,
    bind: SocketAddr,
) -> Result<Edge<H>, String> {
    let mut server = Server::new(handler);
    // UDP and TCP share one port number. With port 0 the OS picks the UDP port, and the same
    // number can be taken for TCP, so retry instead of failing the first collision. Windows hands
    // out ephemeral UDP ports in sequence and a run of them can sit in a TCP-excluded range
    // (os error 10013), so the retry count is generous.
    let attempts = if bind.port() == 0 { 500 } else { 1 };
    let mut last = String::new();
    for _ in 0..attempts {
        let udp = match UdpSocket::bind(bind).await {
            Ok(u) => u,
            Err(e) => {
                last = format!("udp bind: {e}");
                continue;
            }
        };
        let udp_addr = udp.local_addr().map_err(|e| e.to_string())?;
        match TcpListener::bind(SocketAddr::new(bind.ip(), udp_addr.port())).await {
            Ok(tcp) => {
                let tcp_addr = tcp.local_addr().map_err(|e| e.to_string())?;
                server.register_socket(udp);
                server.register_listener(tcp, TCP_IDLE, TCP_MAX_CONNECTIONS);
                return Ok(Edge {
                    server,
                    udp: udp_addr,
                    tcp: tcp_addr,
                });
            }
            Err(e) => last = format!("tcp bind: {e}"),
        }
    }
    Err(last)
}
