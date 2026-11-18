//! SurfaceAnt VPS edge, slice 1: an authoritative DNS server for one zone loaded from a
//! standard master file (`docs/DESIGN-VPS-EDGE.md` section 9). Later slices replace the file
//! with database-backed synthesis; the server wiring here stays.

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

/// A running server and the addresses it actually bound (port 0 resolves here).
pub struct Edge {
    pub server: Server<Catalog>,
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

    let mut server = Server::new(catalog);
    // UDP and TCP share one port number. With port 0 the OS picks the UDP port, and the same
    // number can be taken for TCP, so retry a few times instead of failing the first collision.
    let attempts = if bind.port() == 0 { 20 } else { 1 };
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
                server.register_listener(tcp, TCP_IDLE, 4096);
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
