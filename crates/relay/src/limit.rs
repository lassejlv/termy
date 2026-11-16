//! Who a request comes from, and how many it may make.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Context;
use axum::http::HeaderMap;
use uuid::Uuid;

/// Addresses tracked at once; beyond this, unseen addresses wait for a window to end.
const MAX_CLIENTS: usize = 100_000;

/// An address block such as `10.0.0.0/8`, or a single address.
struct Net {
    v4: bool,
    addr: u128,
    mask: u128,
}

fn bits(ip: IpAddr) -> (bool, u128, u32) {
    match ip.to_canonical() {
        IpAddr::V4(a) => (true, u32::from(a).into(), 32),
        IpAddr::V6(a) => (false, a.into(), 128),
    }
}

impl Net {
    fn parse(text: &str) -> Option<Self> {
        let (ip, len) = match text.split_once('/') {
            Some((ip, len)) => (ip, Some(len)),
            None => (text, None),
        };
        let (v4, addr, width) = bits(ip.trim().parse().ok()?);
        let len = match len {
            Some(len) => len.trim().parse().ok().filter(|len| *len <= width)?,
            None => width,
        };
        let mask = match len {
            0 => 0,
            len => (u128::MAX << (width - len)) & (u128::MAX >> (128 - width)),
        };
        Some(Self { v4, addr, mask })
    }

    fn contains(&self, ip: IpAddr) -> bool {
        let (v4, addr, _) = bits(ip);
        v4 == self.v4 && addr & self.mask == self.addr & self.mask
    }
}

/// Addresses many users can share: a proxy on a private network, or this machine.
fn internal(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => {
            let [first, second, ..] = a.octets();
            a.is_loopback()
                || a.is_private()
                || a.is_link_local()
                || (first == 100 && second & 0xc0 == 64)
        }
        IpAddr::V6(a) => a.is_loopback() || a.is_unique_local() || a.is_unicast_link_local(),
    }
}

/// The address a request came from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Client {
    pub ip: IpAddr,
    /// False when `ip` is a proxy the relay wasn't told to trust (or this
    /// machine), so everyone behind it shares the address.
    pub distinct: bool,
}

/// The reverse proxies whose `X-Forwarded-For` the relay believes.
#[derive(Default)]
pub struct Proxies(Vec<Net>);

impl Proxies {
    /// Parses `TRUSTED_PROXIES`: comma-separated addresses or CIDR blocks.
    pub fn parse(list: &str) -> anyhow::Result<Self> {
        list.split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(|entry| {
                Net::parse(entry).with_context(|| format!("TRUSTED_PROXIES: invalid entry {entry}"))
            })
            .collect::<anyhow::Result<_>>()
            .map(Self)
    }

    fn trusts(&self, ip: IpAddr) -> bool {
        self.0.iter().any(|net| net.contains(ip))
    }

    /// The nearest address in `X-Forwarded-For` that isn't a trusted proxy.
    /// Entries to its left are whatever the client chose to send.
    fn forwarded(&self, headers: &HeaderMap) -> Option<IpAddr> {
        let mut lines = vec![];
        for value in headers.get_all("x-forwarded-for") {
            lines.push(value.to_str().ok()?);
        }
        for entry in lines.iter().flat_map(|line| line.split(',')).rev() {
            let ip = entry.trim().parse::<IpAddr>().ok()?.to_canonical();
            if !self.trusts(ip) {
                return Some(ip);
            }
        }
        None
    }
}

/// A fixed-window request counter per client address.
pub struct RateLimiter {
    limit: u32,
    window: Duration,
    state: Mutex<Windows>,
}

struct Windows {
    counts: HashMap<IpAddr, (Instant, u32)>,
    pruned: Instant,
}

impl RateLimiter {
    pub fn new(limit: u32, window: Duration) -> Self {
        Self {
            limit,
            window,
            state: Mutex::new(Windows {
                counts: HashMap::new(),
                pruned: Instant::now(),
            }),
        }
    }

    pub fn allow(&self, ip: IpAddr) -> bool {
        self.allow_at(ip, Instant::now())
    }

    fn allow_at(&self, ip: IpAddr, now: Instant) -> bool {
        // An IPv6 customer holds a whole /64, so that is what gets counted.
        let key = match ip.to_canonical() {
            IpAddr::V6(a) => IpAddr::V6(Ipv6Addr::from(u128::from(a) & (u128::MAX << 64))),
            v4 => v4,
        };
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let since_prune = now.saturating_duration_since(state.pruned);
        if since_prune >= self.window
            || (state.counts.len() >= MAX_CLIENTS && since_prune >= Duration::from_secs(1))
        {
            let window = self.window;
            state
                .counts
                .retain(|_, (start, _)| now.saturating_duration_since(*start) < window);
            state.pruned = now;
        }
        let full = state.counts.len() >= MAX_CLIENTS;
        match state.counts.get_mut(&key) {
            Some((start, count)) => {
                if now.saturating_duration_since(*start) >= self.window {
                    (*start, *count) = (now, 0);
                }
                if *count >= self.limit {
                    return false;
                }
                *count += 1;
                true
            }
            None if full => false,
            None => {
                state.counts.insert(key, (now, 1));
                true
            }
        }
    }
}

/// A token bucket per device: `burst` at once, then one each `every`.
pub struct DeviceLimiter {
    burst: u32,
    every: Duration,
    state: Mutex<Buckets>,
}

struct Buckets {
    /// When each device's bucket is full again; past that it can be forgotten.
    full_at: HashMap<Uuid, Instant>,
    pruned: Instant,
}

impl DeviceLimiter {
    pub fn new(burst: u32, every: Duration) -> Self {
        Self {
            burst,
            every,
            state: Mutex::new(Buckets {
                full_at: HashMap::new(),
                pruned: Instant::now(),
            }),
        }
    }

    pub fn allow(&self, device: Uuid) -> bool {
        self.allow_at(device, Instant::now())
    }

    fn allow_at(&self, device: Uuid, now: Instant) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let since_prune = now.saturating_duration_since(state.pruned);
        if since_prune >= self.every * self.burst
            || (state.full_at.len() >= MAX_CLIENTS && since_prune >= Duration::from_secs(1))
        {
            state.full_at.retain(|_, full| *full > now);
            state.pruned = now;
        }
        let full = state.full_at.len() >= MAX_CLIENTS;
        let capacity = self.every * self.burst;
        match state.full_at.get_mut(&device) {
            Some(full_at) => {
                let start = (*full_at).max(now);
                if start.saturating_duration_since(now) + self.every > capacity {
                    return false;
                }
                *full_at = start + self.every;
                true
            }
            None if full => false,
            None => {
                state.full_at.insert(device, now + self.every);
                true
            }
        }
    }
}

pub struct Limits {
    proxies: Proxies,
    /// New device registrations, the only unauthenticated write.
    pub registrations: RateLimiter,
    /// Every API request, counted before it costs a database query.
    pub requests: RateLimiter,
    /// Push notifications, per sending Mac.
    pub pushes: DeviceLimiter,
    warned: AtomicBool,
}

impl Limits {
    pub fn new(proxies: Proxies) -> Self {
        Self {
            proxies,
            registrations: RateLimiter::new(30, Duration::from_secs(300)),
            requests: RateLimiter::new(600, Duration::from_secs(60)),
            pushes: DeviceLimiter::new(20, Duration::from_secs(3)),
            warned: AtomicBool::new(false),
        }
    }

    pub fn client(&self, peer: IpAddr, headers: &HeaderMap) -> Client {
        let peer = peer.to_canonical();
        if self.proxies.trusts(peer) {
            return match self.proxies.forwarded(headers) {
                Some(ip) => Client { ip, distinct: true },
                None => Client {
                    ip: peer,
                    distinct: false,
                },
            };
        }
        let distinct = !internal(peer);
        if !distinct
            && headers.contains_key("x-forwarded-for")
            && !self.warned.swap(true, Ordering::Relaxed)
        {
            tracing::warn!(
                "requests arrive through a proxy at {peer} that TRUSTED_PROXIES doesn't list: \
                 its users share one registration limit and have no request limit"
            );
        }
        Client { ip: peer, distinct }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }
    fn headers(forwarded: &[&str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for line in forwarded {
            headers.append("x-forwarded-for", line.parse().unwrap());
        }
        headers
    }

    #[test]
    fn blocks_match_only_their_addresses() {
        let proxies = Proxies::parse("172.16.0.0/12, 10.1.2.3 ,fc00::/7").unwrap();
        assert!(proxies.trusts(ip("172.31.255.255")));
        assert!(!proxies.trusts(ip("172.32.0.0")));
        assert!(proxies.trusts(ip("10.1.2.3")));
        assert!(!proxies.trusts(ip("10.1.2.4")));
        assert!(proxies.trusts(ip("fd12::1")));
        assert!(!proxies.trusts(ip("2001:db8::1")));
        assert!(proxies.trusts(ip("::ffff:10.1.2.3")));
        assert!(Proxies::parse("0.0.0.0/0").unwrap().trusts(ip("8.8.8.8")));
        assert!(Proxies::parse("10.0.0.0/33").is_err());
        assert!(Proxies::parse("caddy").is_err());
        assert!(!Proxies::parse("").unwrap().trusts(ip("127.0.0.1")));
    }

    #[test]
    fn forwarded_address_counts_only_from_a_trusted_proxy() {
        let limits = Limits::new(Proxies::parse("172.16.0.0/12").unwrap());
        // The client's own claim (1.1.1.1) is left of what the proxy appended.
        let through_proxy = limits.client(ip("172.18.0.3"), &headers(&["1.1.1.1, 203.0.113.9"]));
        assert_eq!(through_proxy.ip, ip("203.0.113.9"));
        assert!(through_proxy.distinct);
        let chained = limits.client(
            ip("172.18.0.3"),
            &headers(&["1.1.1.1", "203.0.113.9, 172.18.0.2"]),
        );
        assert_eq!(chained.ip, ip("203.0.113.9"));
        let direct = limits.client(ip("198.51.100.7"), &headers(&["1.1.1.1"]));
        assert_eq!(direct.ip, ip("198.51.100.7"));
        assert!(direct.distinct);
        for unusable in [headers(&[]), headers(&["203.0.113.9, nonsense"])] {
            let client = limits.client(ip("172.18.0.3"), &unusable);
            assert_eq!(client.ip, ip("172.18.0.3"));
            assert!(!client.distinct);
        }
        let unlisted_proxy = limits.client(ip("10.0.0.5"), &headers(&["203.0.113.9"]));
        assert_eq!(unlisted_proxy.ip, ip("10.0.0.5"));
        assert!(!unlisted_proxy.distinct);
    }

    #[test]
    fn counts_per_address_and_window() {
        let limiter = RateLimiter::new(2, Duration::from_secs(60));
        let start = Instant::now();
        assert!(limiter.allow_at(ip("203.0.113.9"), start));
        assert!(limiter.allow_at(ip("203.0.113.9"), start));
        assert!(!limiter.allow_at(ip("203.0.113.9"), start));
        assert!(limiter.allow_at(ip("203.0.113.10"), start));
        assert!(limiter.allow_at(ip("203.0.113.9"), start + Duration::from_secs(60)));
    }

    #[test]
    fn a_device_bursts_then_refills_one_at_a_time() {
        let limiter = DeviceLimiter::new(3, Duration::from_secs(3));
        let (a, b) = (Uuid::from_u128(1), Uuid::from_u128(2));
        let start = Instant::now();
        for _ in 0..3 {
            assert!(limiter.allow_at(a, start));
        }
        assert!(!limiter.allow_at(a, start));
        assert!(limiter.allow_at(b, start));
        assert!(!limiter.allow_at(a, start + Duration::from_millis(2999)));
        assert!(limiter.allow_at(a, start + Duration::from_secs(3)));
        assert!(!limiter.allow_at(a, start + Duration::from_secs(3)));
        // A rested device has its whole burst again, and no more.
        let rested = start + Duration::from_secs(3600);
        for _ in 0..3 {
            assert!(limiter.allow_at(a, rested));
        }
        assert!(!limiter.allow_at(a, rested));
    }

    #[test]
    fn an_ipv6_network_is_one_client() {
        let limiter = RateLimiter::new(1, Duration::from_secs(60));
        let start = Instant::now();
        assert!(limiter.allow_at(ip("2001:db8:1:2::1"), start));
        assert!(!limiter.allow_at(ip("2001:db8:1:2:ffff::9"), start));
        assert!(limiter.allow_at(ip("2001:db8:1:3::1"), start));
    }
}
