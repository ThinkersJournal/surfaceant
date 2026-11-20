//! Slice 2: `registry.db`, the durable store (`docs/DESIGN-VPS-EDGE.md` section 1.2), as a
//! SQLite file. It fills [`crate::synth::Registry`]; the DNS path reads that in-memory copy, never
//! the file, so a query never waits on disk. The disposable `buffer.db` is a different file
//! and is never mentioned here.

use std::{collections::HashMap, path::Path};

use rusqlite::{Connection, params};

use crate::synth::{Data, Registry};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS names (
    label     TEXT PRIMARY KEY,           -- first label below the apex, lowercase
    holder    INTEGER NOT NULL,           -- opaque id of the person; never exported
    suspended INTEGER NOT NULL DEFAULT 0  -- 1 = NXDOMAIN (name-level kill switch)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS reserved (
    label TEXT NOT NULL,
    kind  TEXT NOT NULL CHECK (kind IN ('A','AAAA')),
    value TEXT NOT NULL,
    PRIMARY KEY (label, kind, value)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS acme (
    name       TEXT NOT NULL,             -- full lowercase name, no trailing dot
    value      TEXT NOT NULL,
    expires_at INTEGER NOT NULL,          -- unix seconds
    PRIMARY KEY (name, value)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS usage_log (
    date_utc               TEXT PRIMARY KEY,  -- YYYY-MM-DD
    distinct_holders       INTEGER NOT NULL,
    names_total            INTEGER NOT NULL,
    le_issuances_this_week INTEGER NOT NULL
) WITHOUT ROWID;
";

/// Open (creating if needed) `registry.db` with WAL and the schema.
pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

/// An in-memory registry with the schema, for tests.
pub fn open_memory() -> rusqlite::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

/// Read the whole registry into the in-memory form the DNS path uses. ACME rows past
/// `now_unix` are ignored (and not served), so an expired challenge stops answering without
/// a sweep having run.
pub fn load(conn: &Connection, now_unix: i64) -> rusqlite::Result<Registry> {
    let mut r = Registry::default();
    let mut st = conn.prepare("SELECT label, suspended FROM names")?;
    for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
        let (label, suspended) = row?;
        let label = label.to_ascii_lowercase();
        if suspended != 0 {
            r.suspended.insert(label.clone());
        }
        r.users.insert(label);
    }
    let mut st = conn.prepare("SELECT label, kind, value FROM reserved")?;
    for row in st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })? {
        let (label, kind, value) = row?;
        let d = match kind.as_str() {
            "A" => value.parse().ok().map(Data::A),
            _ => value.parse().ok().map(Data::Aaaa),
        };
        // A row that does not parse is skipped, not served as garbage.
        if let Some(d) = d {
            r.reserved
                .entry(label.to_ascii_lowercase())
                .or_default()
                .push(d);
        }
    }
    let mut st = conn.prepare("SELECT name, value FROM acme WHERE expires_at > ?1")?;
    let mut acme: HashMap<String, Vec<String>> = HashMap::new();
    for row in st.query_map(params![now_unix], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })? {
        let (name, value) = row?;
        acme.entry(name.to_ascii_lowercase())
            .or_default()
            .push(value);
    }
    r.acme = acme;
    Ok(r)
}

/// Write (or rewrite) the usage-log row for `date_utc` from the registry itself: numbers and
/// a date only, no way back to a person. Returns `(distinct_holders, names_total)`.
pub fn record_usage(
    conn: &Connection,
    date_utc: &str,
    le_issuances_this_week: i64,
) -> rusqlite::Result<(i64, i64)> {
    let (holders, names): (i64, i64) = conn.query_row(
        "SELECT COUNT(DISTINCT holder), COUNT(*) FROM names",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    conn.execute(
        "INSERT INTO usage_log (date_utc, distinct_holders, names_total, le_issuances_this_week)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(date_utc) DO UPDATE SET
           distinct_holders = excluded.distinct_holders,
           names_total = excluded.names_total,
           le_issuances_this_week = excluded.le_issuances_this_week",
        params![date_utc, holders, names, le_issuances_this_week],
    )?;
    Ok((holders, names))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded() -> Connection {
        let c = open_memory().unwrap();
        c.execute_batch(
            "INSERT INTO names VALUES ('Alice', 1, 0), ('alice2', 1, 0), ('bob', 2, 1);
             INSERT INTO reserved VALUES ('www','A','192.0.2.20'), ('www','AAAA','2001:db8::20'), ('bad','A','not-an-ip');
             INSERT INTO acme VALUES ('_acme-challenge.alice.zone.test','live',2000), ('_acme-challenge.alice.zone.test','old',500);",
        )
        .unwrap();
        c
    }

    #[test]
    fn load_fills_users_suspended_reserved_and_only_live_acme() {
        let r = load(&seeded(), 1000).unwrap();
        assert!(r.users.contains("alice") && r.users.contains("alice2") && r.users.contains("bob"));
        assert!(r.suspended.contains("bob") && !r.suspended.contains("alice"));
        assert_eq!(r.reserved["www"].len(), 2);
        assert!(
            !r.reserved.contains_key("bad"),
            "unparseable reserved row is skipped"
        );
        assert_eq!(
            r.acme["_acme-challenge.alice.zone.test"],
            vec!["live".to_string()]
        );
    }

    #[test]
    fn usage_counts_a_holder_of_two_names_once_and_rewrites_the_day() {
        let c = seeded();
        assert_eq!(record_usage(&c, "2026-11-19", 3).unwrap(), (2, 3));
        c.execute("INSERT INTO names VALUES ('carol', 3, 0)", [])
            .unwrap();
        assert_eq!(record_usage(&c, "2026-11-19", 4).unwrap(), (3, 4));
        let (rows, le): (i64, i64) = c
            .query_row(
                "SELECT COUNT(*), MAX(le_issuances_this_week) FROM usage_log",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((rows, le), (1, 4), "one row per day, rewritten");
    }

    #[test]
    fn file_database_reopens_with_its_data() {
        let dir = std::env::temp_dir().join(format!("sa-reg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("registry.db");
        {
            let c = open(&p).unwrap();
            c.execute("INSERT INTO names VALUES ('dave', 9, 0)", [])
                .unwrap();
        }
        let r = load(&open(&p).unwrap(), 0).unwrap();
        assert!(r.users.contains("dave"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
