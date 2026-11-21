//! Slice 2: the signed snapshot box A sends to box B (`docs/DESIGN-VPS-EDGE.md` section 1.1).
//! Only the byte format and its checks live here; carrying the bytes over WireGuard is a
//! later slice. A signs with an ed25519 key and B holds only the public key, so a compromised
//! B cannot forge a registry. B installs a snapshot only if the signature verifies, the
//! sequence number is higher than the last one it installed (no rollback to an old registry),
//! and the file opens as a registry and passes SQLite's integrity check. The install is an
//! atomic rename, so B always serves a whole registry.
//!
//! Wire format: `SASNAP1\n` | seq u64 BE | created_unix u64 BE | len u32 BE | database | sig 64.
//! The signature covers every byte before it.

use std::path::Path;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rusqlite::Connection;

const MAGIC: &[u8; 8] = b"SASNAP1\n";
const HEADER: usize = 8 + 8 + 8 + 4;
const SIG: usize = 64;
/// Largest database B accepts; a registry of names is small, so a huge body is an attack.
pub const MAX_DB_BYTES: usize = 256 * 1024 * 1024;

/// Why a snapshot was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    Malformed,
    BadSignature,
    /// `seq` is not greater than the last installed sequence.
    Stale {
        seq: u64,
        last: u64,
    },
    /// Signed correctly but not a usable registry.
    NotARegistry(String),
    Io(String),
}

/// Build a snapshot of `conn` with sequence `seq`. Uses `VACUUM INTO`, which writes a
/// consistent copy even while the DNS reload reads the live file.
pub fn make(
    conn: &Connection,
    scratch: &Path,
    seq: u64,
    created_unix: u64,
    key: &SigningKey,
) -> Result<Vec<u8>, String> {
    let _ = std::fs::remove_file(scratch);
    conn.execute("VACUUM INTO ?1", [scratch.to_string_lossy().as_ref()])
        .map_err(|e| e.to_string())?;
    let db = std::fs::read(scratch).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(scratch);
    if db.len() > MAX_DB_BYTES {
        return Err("database larger than MAX_DB_BYTES".into());
    }
    let mut out = Vec::with_capacity(HEADER + db.len() + SIG);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&seq.to_be_bytes());
    out.extend_from_slice(&created_unix.to_be_bytes());
    out.extend_from_slice(&(db.len() as u32).to_be_bytes());
    out.extend_from_slice(&db);
    let sig = key.sign(&out);
    out.extend_from_slice(&sig.to_bytes());
    Ok(out)
}

/// Check `bytes` and, if good, atomically install the database at `dest`. Returns the
/// installed sequence number. `tmp` must be on the same filesystem as `dest`.
pub fn verify_and_install(
    bytes: &[u8],
    key: &VerifyingKey,
    last_seq: Option<u64>,
    tmp: &Path,
    dest: &Path,
) -> Result<u64, Refused> {
    if bytes.len() < HEADER + SIG || &bytes[..8] != MAGIC {
        return Err(Refused::Malformed);
    }
    let seq = u64::from_be_bytes(bytes[8..16].try_into().unwrap());
    let len = u32::from_be_bytes(bytes[24..28].try_into().unwrap()) as usize;
    if len > MAX_DB_BYTES || bytes.len() != HEADER + len + SIG {
        return Err(Refused::Malformed);
    }
    let (body, sig) = bytes.split_at(HEADER + len);
    let sig = Signature::from_slice(sig).map_err(|_| Refused::Malformed)?;
    key.verify(body, &sig).map_err(|_| Refused::BadSignature)?;
    if let Some(last) = last_seq
        && seq <= last
    {
        return Err(Refused::Stale { seq, last });
    }
    std::fs::write(tmp, &body[HEADER..]).map_err(|e| Refused::Io(e.to_string()))?;
    let check = (|| -> Result<(), String> {
        let c = Connection::open(tmp).map_err(|e| e.to_string())?;
        let ok: String = c
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if ok != "ok" {
            return Err(format!("integrity_check: {ok}"));
        }
        crate::registry::load(&c, 0).map_err(|e| e.to_string())?;
        Ok(())
    })();
    if let Err(e) = check {
        let _ = std::fs::remove_file(tmp);
        return Err(Refused::NotARegistry(e));
    }
    std::fs::rename(tmp, dest).map_err(|e| Refused::Io(e.to_string()))?;
    Ok(seq)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("sa-snap-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn key(b: u8) -> SigningKey {
        SigningKey::from_bytes(&[b; 32])
    }
    fn source(d: &Path) -> Connection {
        let c = crate::registry::open(&d.join("a.db")).unwrap();
        c.execute("INSERT INTO names VALUES ('alice', 1, 0)", [])
            .unwrap();
        c
    }

    #[test]
    fn round_trip_installs_a_registry_b_can_load() {
        let d = dir("rt");
        let k = key(1);
        let snap = make(&source(&d), &d.join("s.tmp"), 5, 1000, &k).unwrap();
        let seq = verify_and_install(
            &snap,
            &k.verifying_key(),
            None,
            &d.join("b.tmp"),
            &d.join("b.db"),
        )
        .unwrap();
        assert_eq!(seq, 5);
        let r = crate::registry::load(&crate::registry::open(&d.join("b.db")).unwrap(), 0).unwrap();
        assert!(r.users.contains("alice"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn refuses_wrong_key_tampering_truncation_and_rollback_and_installs_nothing() {
        let d = dir("bad");
        let k = key(1);
        let snap = make(&source(&d), &d.join("s.tmp"), 5, 1000, &k).unwrap();
        let (tmp, dest) = (d.join("b.tmp"), d.join("b.db"));
        let vk = k.verifying_key();

        assert_eq!(
            verify_and_install(&snap, &key(2).verifying_key(), None, &tmp, &dest),
            Err(Refused::BadSignature)
        );
        let mut t = snap.clone();
        let mid = HEADER + 10;
        t[mid] ^= 1;
        assert_eq!(
            verify_and_install(&t, &vk, None, &tmp, &dest),
            Err(Refused::BadSignature)
        );
        assert_eq!(
            verify_and_install(&snap[..snap.len() - 1], &vk, None, &tmp, &dest),
            Err(Refused::Malformed)
        );
        assert_eq!(
            verify_and_install(b"nope", &vk, None, &tmp, &dest),
            Err(Refused::Malformed)
        );
        assert_eq!(
            verify_and_install(&snap, &vk, Some(5), &tmp, &dest),
            Err(Refused::Stale { seq: 5, last: 5 })
        );
        assert_eq!(
            verify_and_install(&snap, &vk, Some(9), &tmp, &dest),
            Err(Refused::Stale { seq: 5, last: 9 })
        );
        assert!(!dest.exists(), "nothing installed by any refusal");
        // control: the same bytes with the right key and a lower last_seq do install
        assert_eq!(verify_and_install(&snap, &vk, Some(4), &tmp, &dest), Ok(5));
        assert!(dest.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn correctly_signed_garbage_is_not_installed_and_keeps_the_old_registry() {
        let d = dir("garbage");
        let k = key(1);
        let (tmp, dest) = (d.join("b.tmp"), d.join("b.db"));
        std::fs::write(&dest, b"previous").unwrap();
        // a signed body that is not a SQLite database
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&7u64.to_be_bytes());
        out.extend_from_slice(&0u64.to_be_bytes());
        let junk = vec![0xABu8; 4096];
        out.extend_from_slice(&(junk.len() as u32).to_be_bytes());
        out.extend_from_slice(&junk);
        let sig = k.sign(&out);
        out.extend_from_slice(&sig.to_bytes());
        let r = verify_and_install(&out, &k.verifying_key(), None, &tmp, &dest);
        assert!(matches!(r, Err(Refused::NotARegistry(_))), "{r:?}");
        assert_eq!(std::fs::read(&dest).unwrap(), b"previous");
        assert!(!tmp.exists(), "temp file removed");
        let _ = std::fs::remove_dir_all(&d);
    }
}
