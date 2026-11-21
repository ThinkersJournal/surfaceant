//! Slice 2: per-query answer synthesis (`docs/DESIGN-VPS-EDGE.md` sections 1.1 and 9).
//!
//! A pure decision function over registry data, with no sockets and no hickory types, so the
//! rules (any depth, reserved names, suspension, `_acme-challenge`, RFC 4592 empty
//! non-terminals, `ANY` as RFC 8482 `HINFO`) are unit-tested directly. The registry here is in memory;
//! `registry.db` will fill the same struct.

use std::{
    collections::{HashMap, HashSet},
    net::{Ipv4Addr, Ipv6Addr},
};

/// The record types the edge answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QType {
    A,
    Aaaa,
    Ns,
    Soa,
    Txt,
    Caa,
    Any,
    Other,
}

/// One synthesized record's data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Data {
    A(Ipv4Addr),
    Aaaa(Ipv6Addr),
    Ns(String),
    Soa,
    Txt(String),
    Caa(String),
    /// RFC 8482 section 4.2 answer to `ANY`: `HINFO "RFC8482" ""`.
    Hinfo,
}

/// What to send back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// The name exists and has records of the type.
    Records(Vec<Data>),
    /// The name exists, the type does not (answer NOERROR with SOA in authority).
    NoData,
    /// The name does not exist (NXDOMAIN with SOA in authority).
    NxDomain,
    /// Outside our zone: no recursion.
    Refused,
}

/// Addresses of the relay edge, served for every registered user name at any depth.
#[derive(Clone, Debug)]
pub struct Edge {
    pub a: Vec<Ipv4Addr>,
    pub aaaa: Vec<Ipv6Addr>,
}

/// Registry data the synthesis reads.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    /// Registered user labels (first label below the apex), lowercase.
    pub users: HashSet<String>,
    /// Suspended user labels: NXDOMAIN, the name-level kill switch.
    pub suspended: HashSet<String>,
    /// Reserved labels: our own records (`www`, `ns1`, ...), keyed by label.
    pub reserved: HashMap<String, Vec<Data>>,
    /// Live ACME challenge TXT values keyed by the full lowercase name without trailing dot,
    /// e.g. `_acme-challenge.alice.zone`.
    pub acme: HashMap<String, Vec<String>>,
}

/// One zone's synthesis.
#[derive(Clone, Debug)]
pub struct Synth {
    /// Apex, lowercase, no trailing dot.
    pub apex: String,
    pub edge: Edge,
    pub ns: Vec<String>,
    pub caa: Vec<String>,
    pub registry: Registry,
}

fn of_type(all: &[Data], t: QType) -> Vec<Data> {
    all.iter()
        .filter(|d| {
            matches!(
                (d, t),
                (Data::A(_), QType::A)
                    | (Data::Aaaa(_), QType::Aaaa)
                    | (Data::Ns(_), QType::Ns)
                    | (Data::Soa, QType::Soa)
                    | (Data::Txt(_), QType::Txt)
                    | (Data::Caa(_), QType::Caa)
            )
        })
        .cloned()
        .collect()
}

fn pick(all: Vec<Data>, t: QType) -> Answer {
    // `ANY` is answered as RFC 8482 section 4.2 says: one synthesised `HINFO`, never the
    // real records (no bulk answer, so no amplification). The name still has to exist.
    let r = if t == QType::Any {
        if all.is_empty() {
            vec![]
        } else {
            vec![Data::Hinfo]
        }
    } else {
        of_type(&all, t)
    };
    if r.is_empty() {
        Answer::NoData
    } else {
        Answer::Records(r)
    }
}

impl Synth {
    fn edge_records(&self) -> Vec<Data> {
        self.edge
            .a
            .iter()
            .map(|a| Data::A(*a))
            .chain(self.edge.aaaa.iter().map(|a| Data::Aaaa(*a)))
            .collect()
    }

    /// Decide the answer for `qname` (any case, trailing dot allowed) and `qtype`.
    pub fn answer(&self, qname: &str, qtype: QType) -> Answer {
        let name = qname.trim_end_matches('.').to_ascii_lowercase();
        let rel = if name == self.apex {
            None
        } else if let Some(r) = name.strip_suffix(&format!(".{}", self.apex)) {
            Some(r.to_owned())
        } else {
            return Answer::Refused;
        };

        let Some(rel) = rel else {
            // Apex.
            let mut all = vec![Data::Soa];
            all.extend(self.ns.iter().cloned().map(Data::Ns));
            all.extend(self.edge_records());
            all.extend(self.caa.iter().cloned().map(Data::Caa));
            return if qtype == QType::Any {
                Answer::Records(vec![Data::Hinfo])
            } else {
                pick(all, qtype)
            };
        };

        if rel.split('.').any(str::is_empty) {
            return Answer::NxDomain;
        }
        // The label directly below the apex decides who owns the name.
        let owner = rel.rsplit('.').next().unwrap_or(&rel);
        let depth = rel.split('.').count();

        if let Some(own) = self.registry.reserved.get(owner) {
            // Reserved names are our own single records; nothing exists below them.
            return if depth == 1 {
                pick(own.clone(), qtype)
            } else {
                Answer::NxDomain
            };
        }
        if self.registry.suspended.contains(owner) || !self.registry.users.contains(owner) {
            return Answer::NxDomain;
        }
        // `_acme-challenge.<user>[.<more>]`: TXT from the live table; the name exists only
        // while a challenge is live (otherwise it falls through to the edge address below,
        // so a CNAME-free DNS-01 lookup still gets NODATA for TXT).
        if (rel.starts_with("_acme-challenge.") || rel == "_acme-challenge")
            && let Some(vals) = self.registry.acme.get(&name)
        {
            let all: Vec<Data> = vals.iter().cloned().map(Data::Txt).collect();
            return pick(all, qtype);
        }
        pick(self.edge_records(), qtype)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth() -> Synth {
        let mut r = Registry::default();
        r.users.insert("alice".into());
        r.users.insert("bob".into());
        r.suspended.insert("bob".into());
        r.reserved
            .insert("www".into(), vec![Data::A("192.0.2.20".parse().unwrap())]);
        r.acme
            .insert("_acme-challenge.alice.zone.test".into(), vec!["tok".into()]);
        Synth {
            apex: "zone.test".into(),
            edge: Edge {
                a: vec!["192.0.2.10".parse().unwrap()],
                aaaa: vec!["2001:db8::10".parse().unwrap()],
            },
            ns: vec!["ns1.zone.test".into()],
            caa: vec!["letsencrypt.org".into()],
            registry: r,
        }
    }

    #[test]
    fn user_name_resolves_at_every_depth() {
        let s = synth();
        for n in [
            "alice.zone.test",
            "x.alice.zone.test",
            "a.b.c.alice.zone.test.",
        ] {
            assert_eq!(
                s.answer(n, QType::A),
                Answer::Records(vec![Data::A("192.0.2.10".parse().unwrap())]),
                "{n}"
            );
        }
    }

    #[test]
    fn unknown_and_suspended_are_nxdomain() {
        let s = synth();
        assert_eq!(s.answer("nobody.zone.test", QType::A), Answer::NxDomain);
        assert_eq!(s.answer("x.nobody.zone.test", QType::A), Answer::NxDomain);
        assert_eq!(s.answer("bob.zone.test", QType::A), Answer::NxDomain);
        assert_eq!(s.answer("x.bob.zone.test", QType::A), Answer::NxDomain);
    }

    #[test]
    fn reserved_is_ours_and_has_nothing_below() {
        let s = synth();
        assert_eq!(
            s.answer("www.zone.test", QType::A),
            Answer::Records(vec![Data::A("192.0.2.20".parse().unwrap())])
        );
        assert_eq!(s.answer("www.zone.test", QType::Aaaa), Answer::NoData);
        assert_eq!(s.answer("x.www.zone.test", QType::A), Answer::NxDomain);
    }

    #[test]
    fn out_of_zone_is_refused_and_suffix_lookalike_is_not_in_zone() {
        let s = synth();
        assert_eq!(s.answer("example.org", QType::A), Answer::Refused);
        assert_eq!(s.answer("evilzone.test", QType::A), Answer::Refused);
    }

    #[test]
    fn case_and_trailing_dot_are_ignored() {
        let s = synth();
        assert!(matches!(
            s.answer("ALICE.Zone.Test.", QType::A),
            Answer::Records(_)
        ));
    }

    #[test]
    fn acme_txt_from_live_table_else_nodata() {
        let s = synth();
        assert_eq!(
            s.answer("_acme-challenge.alice.zone.test", QType::Txt),
            Answer::Records(vec![Data::Txt("tok".into())])
        );
        // No live challenge: the name exists (user's), no TXT.
        assert_eq!(
            s.answer("_acme-challenge.carol.zone.test", QType::Txt),
            Answer::NxDomain
        );
        assert_eq!(s.answer("x.alice.zone.test", QType::Txt), Answer::NoData);
    }

    #[test]
    fn apex_records_and_minimal_any() {
        let s = synth();
        assert_eq!(
            s.answer("zone.test", QType::Soa),
            Answer::Records(vec![Data::Soa])
        );
        assert_eq!(
            s.answer("zone.test", QType::Ns),
            Answer::Records(vec![Data::Ns("ns1.zone.test".into())])
        );
        // RFC 8482 section 4.2: ANY gets one HINFO, at the apex and on a user name alike;
        // a name that does not exist is still NXDOMAIN.
        assert_eq!(
            s.answer("zone.test", QType::Any),
            Answer::Records(vec![Data::Hinfo])
        );
        assert_eq!(
            s.answer("alice.zone.test", QType::Any),
            Answer::Records(vec![Data::Hinfo])
        );
        assert_eq!(s.answer("nobody.zone.test", QType::Any), Answer::NxDomain);
    }

    #[test]
    fn empty_label_is_nxdomain() {
        let s = synth();
        assert_eq!(s.answer("a..alice.zone.test", QType::A), Answer::NxDomain);
    }
}
