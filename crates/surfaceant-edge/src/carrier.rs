//! Slice 2: the carrier that moves a signed snapshot from box A to box B
//! (`docs/DESIGN-VPS-EDGE.md` section 1.1). B pulls; A only serves a file.
//!
//! The carrier is deliberately dumb: it has no trust of its own. Every byte B receives goes
//! to the inbox, where `snapshot::verify_and_install` checks the ed25519 signature and the
//! sequence number, so a hostile or broken carrier can at worst waste B's time. Bind A's
//! listener to its WireGuard address only; WireGuard is the transport's access control.
//!
//! Wire protocol (one request per connection):
//! B -> A: `SAPULL1\n` | last_seq u64 BE (0 = none installed)
//! A -> B: len u32 BE | snapshot bytes. `len` 0 means "nothing newer than last_seq".

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Semaphore,
    task::JoinHandle,
    time::timeout,
};

const REQ_MAGIC: &[u8; 8] = b"SAPULL1\n";
/// Per-step network time limit on both sides.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(10);
/// Concurrent pulls A serves; B is the only legitimate client, so this is generous.
pub const MAX_CONNECTIONS: usize = 8;
/// A reply is a snapshot, so it can be at most the snapshot limit plus its framing.
const MAX_REPLY: usize = crate::snapshot::MAX_DB_BYTES + 64;

/// Sequence number in a snapshot's header (bytes 8..16), if `bytes` has one.
fn seq_of(bytes: &[u8]) -> Option<u64> {
    (bytes.len() >= 16 && &bytes[..8] == b"SASNAP1\n")
        .then(|| u64::from_be_bytes(bytes[8..16].try_into().unwrap()))
}

async fn serve_one(mut s: TcpStream, file: &Path) -> std::io::Result<()> {
    let mut req = [0u8; 16];
    timeout(STEP_TIMEOUT, s.read_exact(&mut req))
        .await
        .map_err(|_| std::io::ErrorKind::TimedOut)??;
    if &req[..8] != REQ_MAGIC {
        return Ok(()); // not our protocol: close without a reply
    }
    let last = u64::from_be_bytes(req[8..16].try_into().unwrap());
    let body = tokio::fs::read(file).await.unwrap_or_default();
    let send: &[u8] = match seq_of(&body) {
        Some(seq) if seq > last => &body,
        _ => &[],
    };
    timeout(STEP_TIMEOUT, async {
        s.write_all(&(send.len() as u32).to_be_bytes()).await?;
        s.write_all(send).await?;
        s.shutdown().await
    })
    .await
    .map_err(|_| std::io::ErrorKind::TimedOut)?
}

/// Box A: serve the newest snapshot in `file` (re-read per request, so a writer that
/// replaces it atomically is always seen whole). Returns the bound address.
pub async fn serve(
    bind: SocketAddr,
    file: PathBuf,
) -> std::io::Result<(SocketAddr, JoinHandle<()>)> {
    let l = TcpListener::bind(bind).await?;
    let addr = l.local_addr()?;
    let limit = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let task = tokio::spawn(async move {
        loop {
            let Ok((s, _)) = l.accept().await else {
                continue;
            };
            let Ok(permit) = limit.clone().try_acquire_owned() else {
                continue; // over the cap: drop the connection
            };
            let file = file.clone();
            tokio::spawn(async move {
                let _ = serve_one(s, &file).await;
                drop(permit);
            });
        }
    });
    Ok((addr, task))
}

/// Box B: ask A for anything newer than `last_seq` and, if it sends one, place it in
/// `inbox` (written to `inbox` + `.part` and renamed, so ingest never reads a partial file).
/// Returns `Ok(true)` when a snapshot was written. Does NOT verify it; ingest does.
pub async fn pull(a: SocketAddr, last_seq: Option<u64>, inbox: &Path) -> Result<bool, String> {
    let step = |e: &str| format!("snapshot pull {a}: {e}");
    let mut s = timeout(STEP_TIMEOUT, TcpStream::connect(a))
        .await
        .map_err(|_| step("connect timed out"))?
        .map_err(|e| step(&e.to_string()))?;
    let mut req = Vec::with_capacity(16);
    req.extend_from_slice(REQ_MAGIC);
    req.extend_from_slice(&last_seq.unwrap_or(0).to_be_bytes());
    timeout(STEP_TIMEOUT, s.write_all(&req))
        .await
        .map_err(|_| step("send timed out"))?
        .map_err(|e| step(&e.to_string()))?;
    let mut len = [0u8; 4];
    timeout(STEP_TIMEOUT, s.read_exact(&mut len))
        .await
        .map_err(|_| step("reply timed out"))?
        .map_err(|e| step(&e.to_string()))?;
    let len = u32::from_be_bytes(len) as usize;
    if len == 0 {
        return Ok(false);
    }
    if len > MAX_REPLY {
        return Err(step("reply larger than the snapshot limit"));
    }
    let mut body = vec![0u8; len];
    // The whole body gets a deadline that grows with size (1 s per MiB on top of the step).
    let budget = STEP_TIMEOUT + Duration::from_secs((len / (1 << 20)) as u64);
    timeout(budget, s.read_exact(&mut body))
        .await
        .map_err(|_| step("body timed out"))?
        .map_err(|e| step(&e.to_string()))?;
    let part = inbox.with_extension("part");
    tokio::fs::write(&part, &body)
        .await
        .map_err(|e| step(&e.to_string()))?;
    tokio::fs::rename(&part, inbox)
        .await
        .map_err(|e| step(&e.to_string()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sa-carrier-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn snap(d: &Path, seq: u64, k: &SigningKey) -> Vec<u8> {
        let c = crate::registry::open(&d.join(format!("src{seq}.db"))).unwrap();
        c.execute("INSERT INTO names VALUES ('carol', 1, 0)", [])
            .unwrap();
        crate::snapshot::make(&c, &d.join("s.tmp"), seq, 1, k).unwrap()
    }

    #[tokio::test]
    async fn pull_delivers_only_newer_snapshots_and_the_bytes_verify_on_b() {
        let d = dir("pull");
        let k = SigningKey::from_bytes(&[5; 32]);
        let file = d.join("a.snap");
        let (addr, task) = serve("127.0.0.1:0".parse().unwrap(), file.clone())
            .await
            .unwrap();
        let inbox = d.join("in.snap");

        // A has nothing yet
        assert_eq!(pull(addr, None, &inbox).await, Ok(false));
        assert!(!inbox.exists());

        let bytes = snap(&d, 3, &k);
        std::fs::write(&file, &bytes).unwrap();
        assert_eq!(pull(addr, None, &inbox).await, Ok(true));
        assert_eq!(std::fs::read(&inbox).unwrap(), bytes, "bytes arrive intact");
        assert!(!d.join("in.part").exists());
        let seq = crate::snapshot::verify_and_install(
            &std::fs::read(&inbox).unwrap(),
            &k.verifying_key(),
            None,
            &d.join("b.tmp"),
            &d.join("b.db"),
        );
        assert_eq!(seq, Ok(3), "what the carrier delivered installs");

        // up to date: nothing newer; an older last_seq still gets it (control)
        std::fs::remove_file(&inbox).unwrap();
        assert_eq!(pull(addr, Some(3), &inbox).await, Ok(false));
        assert!(!inbox.exists());
        assert_eq!(pull(addr, Some(2), &inbox).await, Ok(true));
        task.abort();
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn pull_refuses_an_oversized_reply_and_a_dead_peer_and_a_wrong_protocol_gets_no_reply() {
        let d = dir("hostile");
        let inbox = d.join("in.snap");
        // a fake A that claims a 4 GiB body
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut r = [0u8; 16];
            s.read_exact(&mut r).await.unwrap();
            s.write_all(&u32::MAX.to_be_bytes()).await.unwrap();
        });
        let e = pull(addr, None, &inbox).await.unwrap_err();
        assert!(e.contains("larger than the snapshot limit"), "{e}");
        assert!(!inbox.exists());

        // nothing listening
        let dead = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dead_addr = dead.local_addr().unwrap();
        drop(dead);
        assert!(pull(dead_addr, None, &inbox).await.is_err());

        // a client that is not ours gets the connection closed with no bytes
        let (a, task) = serve("127.0.0.1:0".parse().unwrap(), d.join("a.snap"))
            .await
            .unwrap();
        let mut s = TcpStream::connect(a).await.unwrap();
        s.write_all(b"GET / HTTP/1.1\r\n\r\n").await.unwrap();
        let mut got = Vec::new();
        // closing with unread bytes may surface as a reset (Windows) rather than EOF
        let _ = s.read_to_end(&mut got).await;
        assert!(got.is_empty(), "no reply to a wrong protocol");
        task.abort();
        let _ = std::fs::remove_dir_all(&d);
    }
}
