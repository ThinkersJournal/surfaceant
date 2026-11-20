//! Slice 2: per-source token bucket (`docs/DESIGN-VPS-EDGE.md` 1.1: a public authoritative
//! server is an amplification target). Over-limit queries are dropped without a reply, so a
//! spoofed source never receives anything from us. Pure and clock-injected for tests.

use std::{collections::HashMap, net::IpAddr, sync::Mutex, time::Duration};

/// Bucket parameters: `burst` queries at once, refilled at `per_sec`.
#[derive(Clone, Copy, Debug)]
pub struct Limit {
    pub burst: f64,
    pub per_sec: f64,
}

/// Per-IP token buckets. IPv6 sources are keyed by their /64 (one subscriber owns a /64).
pub struct RateLimiter {
    limit: Limit,
    /// Upper bound on tracked sources; at the cap, idle full buckets are swept first.
    max_sources: usize,
    buckets: Mutex<HashMap<IpAddr, (f64, Duration)>>,
}

fn key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => {
            let mut o = v6.octets();
            o[8..].fill(0);
            IpAddr::V6(o.into())
        }
        v4 => v4,
    }
}

impl RateLimiter {
    pub fn new(limit: Limit, max_sources: usize) -> Self {
        Self {
            limit,
            max_sources,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// `now` is any monotonic duration (e.g. since process start). True = answer.
    pub fn allow(&self, ip: IpAddr, now: Duration) -> bool {
        let Ok(mut b) = self.buckets.lock() else {
            return true;
        };
        if b.len() >= self.max_sources && !b.contains_key(&key(ip)) {
            let l = self.limit;
            b.retain(|_, (tokens, at)| {
                *tokens + now.saturating_sub(*at).as_secs_f64() * l.per_sec < l.burst
            });
            if b.len() >= self.max_sources {
                return false; // table full of active sources: shed new ones, not old ones
            }
        }
        let l = self.limit;
        let e = b.entry(key(ip)).or_insert((l.burst, now));
        let refill = now.saturating_sub(e.1).as_secs_f64() * l.per_sec;
        e.0 = (e.0 + refill).min(l.burst);
        e.1 = now;
        if e.0 >= 1.0 {
            e.0 -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lim() -> RateLimiter {
        RateLimiter::new(
            Limit {
                burst: 3.0,
                per_sec: 1.0,
            },
            100,
        )
    }
    const S: Duration = Duration::from_secs(1);

    #[test]
    fn burst_then_drop_then_refill() {
        let r = lim();
        let ip: IpAddr = "192.0.2.1".parse().unwrap();
        assert!([0, 0, 0].iter().all(|_| r.allow(ip, Duration::ZERO)));
        assert!(
            !r.allow(ip, Duration::ZERO),
            "fourth in the same instant is dropped"
        );
        assert!(r.allow(ip, S), "one token back after a second");
        assert!(!r.allow(ip, S));
    }

    #[test]
    fn sources_are_independent_and_v6_shares_a_64() {
        let r = lim();
        let a: IpAddr = "192.0.2.1".parse().unwrap();
        let b: IpAddr = "192.0.2.2".parse().unwrap();
        for _ in 0..3 {
            assert!(r.allow(a, Duration::ZERO));
        }
        assert!(!r.allow(a, Duration::ZERO));
        assert!(r.allow(b, Duration::ZERO), "another source is unaffected");
        let x: IpAddr = "2001:db8:0:1::1".parse().unwrap();
        let y: IpAddr = "2001:db8:0:1::ffff".parse().unwrap();
        for _ in 0..3 {
            assert!(r.allow(x, Duration::ZERO));
        }
        assert!(!r.allow(y, Duration::ZERO), "same /64 shares one bucket");
    }

    #[test]
    fn full_table_sheds_new_sources_not_old_ones() {
        let r = RateLimiter::new(
            Limit {
                burst: 1.0,
                per_sec: 0.001,
            },
            2,
        );
        let ips: Vec<IpAddr> = (1..=3)
            .map(|i| format!("192.0.2.{i}").parse().unwrap())
            .collect();
        assert!(r.allow(ips[0], Duration::ZERO));
        assert!(r.allow(ips[1], Duration::ZERO));
        assert!(
            !r.allow(ips[2], Duration::ZERO),
            "cap reached by active sources"
        );
        assert!(
            !r.allow(ips[0], Duration::ZERO),
            "existing source still limited, not evicted"
        );
    }
}
