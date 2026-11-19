//! Rate limits per client, so no one client can spend the relay's bandwidth,
//! or the Mojang and Microsoft rate limits every player shares through the
//! relay's one address.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// What a request spends.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Spend {
    /// Game files and skins.
    Download,
    /// The account, session and sign-in services.
    Api,
    /// A game server connection or ping.
    Connect,
}

/// A bucket's size, and how many it refills a second.
type Rate = (f64, f64);

impl Spend {
    fn per_client(self) -> Rate {
        match self {
            // A first install fetches several thousand asset files.
            Self::Download => (5000.0, 100.0),
            Self::Api => (60.0, 1.0),
            // Server list pings connect too.
            Self::Connect => (60.0, 1.0),
        }
    }
}

/// Every client's API calls together, which Mojang counts against the relay.
const API_TOTAL: Rate = (600.0, 10.0);

/// Past this many tracked clients, idle ones are forgotten.
const MAX_TRACKED: usize = 10_000;
const IDLE: Duration = Duration::from_secs(600);

struct Bucket {
    tokens: f64,
    at: Instant,
}

impl Bucket {
    fn full((size, _): Rate, now: Instant) -> Self {
        Self {
            tokens: size,
            at: now,
        }
    }

    fn take(&mut self, (size, refill): Rate, now: Instant) -> bool {
        let refilled = now.duration_since(self.at).as_secs_f64() * refill;
        self.tokens = (self.tokens + refilled).min(size);
        self.at = now;
        let allowed = self.tokens >= 1.0;
        if allowed {
            self.tokens -= 1.0;
        }
        allowed
    }
}

pub struct Limits(Mutex<Buckets>);

struct Buckets {
    clients: HashMap<(Spend, IpAddr), Bucket>,
    api_total: Bucket,
}

impl Default for Limits {
    fn default() -> Self {
        Self(Mutex::new(Buckets {
            clients: HashMap::new(),
            api_total: Bucket::full(API_TOTAL, Instant::now()),
        }))
    }
}

impl Limits {
    /// Whether `client` may make one more `spend` request now.
    pub fn allow(&self, spend: Spend, client: IpAddr) -> bool {
        self.allow_at(spend, client, Instant::now())
    }

    fn allow_at(&self, spend: Spend, client: IpAddr, now: Instant) -> bool {
        let mut buckets = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let Buckets { clients, api_total } = &mut *buckets;
        if clients.len() > MAX_TRACKED {
            clients.retain(|_, bucket| now.duration_since(bucket.at) < IDLE);
        }
        let rate = spend.per_client();
        clients
            .entry((spend, client))
            .or_insert_with(|| Bucket::full(rate, now))
            .take(rate, now)
            && (spend != Spend::Api || api_total.take(API_TOTAL, now))
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    #[test]
    fn a_client_runs_out_then_refills() {
        let limits = Limits::default();
        let client: IpAddr = "1.1.1.1".parse().unwrap();
        let start = Instant::now();
        let (size, refill) = Spend::Connect.per_client();
        for _ in 0..size as usize {
            assert!(limits.allow_at(Spend::Connect, client, start));
        }
        assert!(!limits.allow_at(Spend::Connect, client, start));
        let later = start + Duration::from_secs_f64(1.0 / refill);
        assert!(limits.allow_at(Spend::Connect, client, later));
    }

    #[test]
    fn api_calls_share_one_total() {
        let limits = Limits::default();
        let start = Instant::now();
        let per_client = Spend::Api.per_client().0 as usize;
        let clients = API_TOTAL.0 as usize / per_client;
        for client in 0..clients as u32 {
            let client = IpAddr::V4(Ipv4Addr::from(client + 1));
            for _ in 0..per_client {
                assert!(limits.allow_at(Spend::Api, client, start));
            }
        }
        let fresh = IpAddr::V4(Ipv4Addr::new(9, 9, 9, 9));
        assert!(!limits.allow_at(Spend::Api, fresh, start));
    }
}
