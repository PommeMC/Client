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

/// A bucket's size, and how much it refills a second.
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

const MIB: f64 = 1024.0 * 1024.0;

/// The bytes one client may download: a few first installs at once, then a
/// megabyte a second.
const DOWNLOAD_BYTES: Rate = (2048.0 * MIB, MIB);

/// What a client's bucket counts.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Meter {
    Requests(Spend),
    DownloadBytes,
}

/// At this many buckets, idle ones are forgotten, and while it stays full a
/// client with none is refused.
const MAX_TRACKED: usize = 100_000;
const IDLE: Duration = Duration::from_secs(600);
/// How often a full table is searched for idle buckets.
const PRUNE_EVERY: Duration = Duration::from_secs(10);

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

    fn refill(&mut self, (size, refill): Rate, now: Instant) {
        let refilled = now.duration_since(self.at).as_secs_f64() * refill;
        self.tokens = (self.tokens + refilled).min(size);
        self.at = now;
    }

    fn take(&mut self, rate: Rate, now: Instant) -> bool {
        self.refill(rate, now);
        let allowed = self.tokens >= 1.0;
        if allowed {
            self.tokens -= 1.0;
        }
        allowed
    }
}

pub struct Limits(Mutex<Buckets>);

struct Buckets {
    clients: HashMap<(Meter, IpAddr), Bucket>,
    api_total: Bucket,
    pruned: Instant,
}

impl Default for Limits {
    fn default() -> Self {
        let now = Instant::now();
        Self(Mutex::new(Buckets {
            clients: HashMap::new(),
            api_total: Bucket::full(API_TOTAL, now),
            pruned: now,
        }))
    }
}

impl Buckets {
    /// `client`'s bucket for `meter`, or `None` when the table is full of
    /// other clients.
    fn bucket(
        &mut self,
        meter: Meter,
        client: IpAddr,
        rate: Rate,
        now: Instant,
    ) -> Option<&mut Bucket> {
        let key = (meter, client);
        if self.clients.len() >= MAX_TRACKED && !self.clients.contains_key(&key) {
            if now.duration_since(self.pruned) >= PRUNE_EVERY {
                self.clients
                    .retain(|_, bucket| now.duration_since(bucket.at) < IDLE);
                self.pruned = now;
            }
            if self.clients.len() >= MAX_TRACKED {
                return None;
            }
        }
        Some(
            self.clients
                .entry(key)
                .or_insert_with(|| Bucket::full(rate, now)),
        )
    }

    /// The bytes `client` may still download, refilled up to `now`.
    fn download_bytes(&mut self, client: IpAddr, now: Instant) -> Option<&mut Bucket> {
        let bucket = self.bucket(Meter::DownloadBytes, client, DOWNLOAD_BYTES, now)?;
        bucket.refill(DOWNLOAD_BYTES, now);
        Some(bucket)
    }
}

impl Limits {
    /// Whether `client` may make one more `spend` request now.
    pub fn allow(&self, spend: Spend, client: IpAddr) -> bool {
        self.allow_at(spend, client, Instant::now())
    }

    /// Counts `bytes` relayed to `client` against what it may download. They
    /// were already sent, so the bucket can go below empty, and stays shut
    /// until that is paid back.
    pub fn downloaded(&self, client: IpAddr, bytes: usize) {
        self.downloaded_at(client, bytes, Instant::now());
    }

    fn allow_at(&self, spend: Spend, client: IpAddr, now: Instant) -> bool {
        let mut buckets = self.buckets();
        if spend == Spend::Download
            && !buckets
                .download_bytes(client, now)
                .is_some_and(|bytes| bytes.tokens > 0.0)
        {
            return false;
        }
        let rate = spend.per_client();
        buckets
            .bucket(Meter::Requests(spend), client, rate, now)
            .is_some_and(|bucket| bucket.take(rate, now))
            && (spend != Spend::Api || buckets.api_total.take(API_TOTAL, now))
    }

    fn downloaded_at(&self, client: IpAddr, bytes: usize, now: Instant) {
        let mut buckets = self.buckets();
        if let Some(bucket) = buckets.download_bytes(client, now) {
            bucket.tokens -= bytes as f64;
        }
    }

    fn buckets(&self) -> std::sync::MutexGuard<'_, Buckets> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    fn client(n: u32) -> IpAddr {
        IpAddr::V4(Ipv4Addr::from(n))
    }

    #[test]
    fn a_client_runs_out_then_refills() {
        let limits = Limits::default();
        let start = Instant::now();
        let (size, refill) = Spend::Connect.per_client();
        for _ in 0..size as usize {
            assert!(limits.allow_at(Spend::Connect, client(1), start));
        }
        assert!(!limits.allow_at(Spend::Connect, client(1), start));
        let later = start + Duration::from_secs_f64(1.0 / refill);
        assert!(limits.allow_at(Spend::Connect, client(1), later));
    }

    #[test]
    fn api_calls_share_one_total() {
        let limits = Limits::default();
        let start = Instant::now();
        let per_client = Spend::Api.per_client().0 as usize;
        let clients = API_TOTAL.0 as usize / per_client;
        for n in 0..clients as u32 {
            for _ in 0..per_client {
                assert!(limits.allow_at(Spend::Api, client(n + 1), start));
            }
        }
        assert!(!limits.allow_at(Spend::Api, client(u32::MAX), start));
    }

    #[test]
    fn downloads_stop_until_their_bytes_are_paid_back() {
        let limits = Limits::default();
        let start = Instant::now();
        let (size, refill) = DOWNLOAD_BYTES;
        let over = 10.0 * refill;
        limits.downloaded_at(client(1), (size + over) as usize, start);
        assert!(!limits.allow_at(Spend::Download, client(1), start));
        assert!(limits.allow_at(Spend::Download, client(2), start));
        let nearly = start + Duration::from_secs(9);
        assert!(!limits.allow_at(Spend::Download, client(1), nearly));
        let paid = start + Duration::from_secs(11);
        assert!(limits.allow_at(Spend::Download, client(1), paid));
    }

    #[test]
    fn a_full_table_refuses_new_clients_until_idle_ones_go() {
        let limits = Limits::default();
        let start = Instant::now();
        for n in 0..MAX_TRACKED as u32 {
            assert!(limits.allow_at(Spend::Connect, client(n), start));
        }
        let newcomer = client(u32::MAX);
        let soon = start + PRUNE_EVERY;
        assert!(!limits.allow_at(Spend::Connect, newcomer, soon));
        assert!(limits.allow_at(Spend::Connect, client(0), soon));
        assert!(limits.allow_at(Spend::Connect, newcomer, start + IDLE));
    }
}
