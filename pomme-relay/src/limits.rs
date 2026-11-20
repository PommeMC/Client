//! Rate limits per client, so no one client can spend the relay's bandwidth,
//! or the Mojang and Microsoft rate limits every player shares through the
//! relay's one address.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Mutex, MutexGuard, PoisonError};
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

/// Every client's API calls together, which Mojang counts against the relay.
const API_TOTAL: Rate = (600.0, 10.0);

const MIB: f64 = 1024.0 * 1024.0;

/// What a client's bucket counts.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Meter {
    Requests(Spend),
    Bytes,
}

impl Meter {
    fn rate(self) -> Rate {
        match self {
            // A first install fetches several thousand asset files.
            Self::Requests(Spend::Download) => (5000.0, 100.0),
            Self::Requests(Spend::Api) => (60.0, 1.0),
            // Server list pings connect too.
            Self::Requests(Spend::Connect) => (60.0, 1.0),
            // A few first installs at once, then a megabyte a second, game
            // connections included.
            Self::Bytes => (2048.0 * MIB, MIB),
        }
    }
}

/// At this many buckets, those that have refilled are forgotten, and while it
/// stays full a client with none is refused.
const MAX_TRACKED: usize = 100_000;
/// How often a full table is searched for refilled buckets.
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

    /// What it holds at `now`.
    fn tokens_at(&self, (size, refill): Rate, now: Instant) -> f64 {
        let refilled = now.duration_since(self.at).as_secs_f64() * refill;
        (self.tokens + refilled).min(size)
    }

    fn refill(&mut self, rate: Rate, now: Instant) {
        self.tokens = self.tokens_at(rate, now);
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
    /// `client`'s bucket for `meter`, refilled up to `now`, or `None` when
    /// the table is full of other clients.
    fn bucket(&mut self, meter: Meter, client: IpAddr, now: Instant) -> Option<&mut Bucket> {
        let key = (meter, client);
        if self.clients.len() >= MAX_TRACKED && !self.clients.contains_key(&key) {
            if now.duration_since(self.pruned) >= PRUNE_EVERY {
                // A full bucket says what a missing one does, so a client
                // still paying back what it spent keeps its bucket.
                self.clients.retain(|(meter, _), bucket| {
                    let rate = meter.rate();
                    bucket.tokens_at(rate, now) < rate.0
                });
                self.pruned = now;
            }
            if self.clients.len() >= MAX_TRACKED {
                return None;
            }
        }
        let rate = meter.rate();
        let bucket = self
            .clients
            .entry(key)
            .or_insert_with(|| Bucket::full(rate, now));
        bucket.refill(rate, now);
        Some(bucket)
    }
}

impl Limits {
    /// Whether `client` may make one more `spend` request now.
    pub fn allow(&self, spend: Spend, client: IpAddr) -> bool {
        self.allow_at(spend, client, Instant::now())
    }

    /// Counts `bytes` relayed for `client`, a download's or a game
    /// connection's, against what it may have, and says whether it may have
    /// more. They were already carried, so the bucket can go below empty, and
    /// stays shut until that is paid back.
    pub fn relayed(&self, client: IpAddr, bytes: usize) -> bool {
        self.relayed_at(client, bytes, Instant::now())
    }

    fn allow_at(&self, spend: Spend, client: IpAddr, now: Instant) -> bool {
        let mut buckets = self.buckets();
        // A download or connection starts with a second's refill in hand, so
        // one retried at the limit is refused here and not cut off at its
        // first bytes.
        if spend != Spend::Api
            && !buckets
                .bucket(Meter::Bytes, client, now)
                .is_some_and(|bytes| bytes.tokens >= Meter::Bytes.rate().1)
        {
            return false;
        }
        let meter = Meter::Requests(spend);
        // The shared total is only read here, and spent once the client's own
        // token is, so neither is spent on a call the other refuses.
        let shared = spend == Spend::Api;
        if shared && buckets.api_total.tokens_at(API_TOTAL, now) < 1.0 {
            return false;
        }
        let allowed = buckets
            .bucket(meter, client, now)
            .is_some_and(|bucket| bucket.take(meter.rate(), now));
        if allowed && shared {
            buckets.api_total.take(API_TOTAL, now);
        }
        allowed
    }

    fn relayed_at(&self, client: IpAddr, bytes: usize, now: Instant) -> bool {
        let mut buckets = self.buckets();
        // With no room to track it, the client is taken to be in credit.
        buckets
            .bucket(Meter::Bytes, client, now)
            .is_none_or(|bucket| {
                bucket.tokens -= bytes as f64;
                bucket.tokens > 0.0
            })
    }

    /// Leaves `client` `bytes` it may still have relayed.
    #[cfg(test)]
    pub fn leave(&self, client: IpAddr, bytes: usize) {
        let spent = Meter::Bytes.rate().0 as usize - bytes;
        self.relayed(client, spent);
    }

    /// How far below a full allowance `client` was when last counted.
    #[cfg(test)]
    pub fn spent(&self, client: IpAddr) -> usize {
        let left = self.buckets().clients[&(Meter::Bytes, client)].tokens;
        (Meter::Bytes.rate().0 - left) as usize
    }

    fn buckets(&self) -> MutexGuard<'_, Buckets> {
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

    /// A download that leaves a client `seconds` of refill below empty.
    fn debt(seconds: f64) -> usize {
        let (size, refill) = Meter::Bytes.rate();
        (size + seconds * refill) as usize
    }

    /// Limits whose table is full of clients a minute in debt at `start`.
    fn full_table(start: Instant) -> Limits {
        let limits = Limits::default();
        for n in 0..MAX_TRACKED as u32 {
            limits.relayed_at(client(n), debt(60.0), start);
        }
        limits
    }

    #[test]
    fn a_client_runs_out_then_refills() {
        let limits = Limits::default();
        let start = Instant::now();
        let (size, refill) = Meter::Requests(Spend::Connect).rate();
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
        let per_client = Meter::Requests(Spend::Api).rate().0 as usize;
        let clients = API_TOTAL.0 as usize / per_client;
        for n in 0..clients as u32 {
            for _ in 0..per_client {
                assert!(limits.allow_at(Spend::Api, client(n + 1), start));
            }
        }
        let late = client(u32::MAX);
        assert!(!limits.allow_at(Spend::Api, late, start));
        // Refused by the total, the late client kept its own tokens.
        let total_refill = Duration::from_secs_f64(per_client as f64 / API_TOTAL.1);
        for _ in 0..per_client {
            assert!(limits.allow_at(Spend::Api, late, start + total_refill));
        }
    }

    #[test]
    fn a_download_no_bucket_tracks_is_not_cut() {
        let start = Instant::now();
        let limits = full_table(start);
        assert!(limits.relayed_at(client(u32::MAX), 1, start));
    }

    #[test]
    fn downloads_stop_until_their_bytes_are_paid_back() {
        let limits = Limits::default();
        let start = Instant::now();
        assert!(limits.relayed_at(client(1), 1, start), "in credit");
        assert!(!limits.relayed_at(client(1), debt(10.0), start), "spent");
        assert!(!limits.allow_at(Spend::Download, client(1), start));
        assert!(limits.allow_at(Spend::Download, client(2), start));
        let nearly = start + Duration::from_secs(9);
        assert!(!limits.allow_at(Spend::Download, client(1), nearly));
        let paid = start + Duration::from_secs(10);
        assert!(
            !limits.allow_at(Spend::Download, client(1), paid),
            "no margin"
        );
        let ahead = start + Duration::from_secs(12);
        assert!(limits.allow_at(Spend::Download, client(1), ahead));
    }

    #[test]
    fn a_full_table_refuses_new_clients_until_others_refill() {
        let start = Instant::now();
        let limits = full_table(start);
        let newcomer = client(u32::MAX);
        for wait in [
            Duration::ZERO,
            PRUNE_EVERY / 2,
            PRUNE_EVERY,
            PRUNE_EVERY * 3,
        ] {
            assert!(!limits.allow_at(Spend::Connect, newcomer, start + wait));
        }
        // A search for refilled buckets has run, and left those in debt.
        assert!(!limits.allow_at(Spend::Download, client(0), start + PRUNE_EVERY * 4));
        let refilled = start + Duration::from_secs(3600);
        assert!(limits.allow_at(Spend::Connect, newcomer, refilled));
        assert!(limits.allow_at(Spend::Download, client(0), refilled));
    }
}
