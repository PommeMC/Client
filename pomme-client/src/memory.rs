use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use sysinfo::{MemoryRefreshKind, System};

const STRIPES: usize = 16;

#[repr(align(64))]
struct Stripe {
    allocated_bytes: AtomicU64,
    freed_bytes: AtomicU64,
    alloc_count: AtomicU64,
    free_count: AtomicU64,
}

#[allow(clippy::declare_interior_mutable_const)]
const ZERO: Stripe = Stripe {
    allocated_bytes: AtomicU64::new(0),
    freed_bytes: AtomicU64::new(0),
    alloc_count: AtomicU64::new(0),
    free_count: AtomicU64::new(0),
};
static COUNTERS: [Stripe; STRIPES] = [ZERO; STRIPES];
static NEXT_STRIPE: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static STRIPE: Cell<usize> = const { Cell::new(usize::MAX) };
}

fn stripe() -> &'static Stripe {
    let index = STRIPE.with(|s| {
        if s.get() == usize::MAX {
            s.set(NEXT_STRIPE.fetch_add(1, Relaxed) % STRIPES);
        }
        s.get()
    });
    &COUNTERS[index]
}

pub struct CountingAllocator<A>(A);

impl<A> CountingAllocator<A> {
    pub const fn new(inner: A) -> Self {
        Self(inner)
    }
}

fn on_alloc(size: usize) {
    let s = stripe();
    s.allocated_bytes.fetch_add(size as u64, Relaxed);
    s.alloc_count.fetch_add(1, Relaxed);
}

fn on_free(size: usize) {
    let s = stripe();
    s.freed_bytes.fetch_add(size as u64, Relaxed);
    s.free_count.fetch_add(1, Relaxed);
}

unsafe impl<A: GlobalAlloc> GlobalAlloc for CountingAllocator<A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { self.0.alloc(layout) };
        if !ptr.is_null() {
            on_alloc(layout.size());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { self.0.alloc_zeroed(layout) };
        if !ptr.is_null() {
            on_alloc(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { self.0.dealloc(ptr, layout) };
        on_free(layout.size());
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new = unsafe { self.0.realloc(ptr, layout, new_size) };
        if !new.is_null() {
            // A realloc counts as freeing the old block and allocating the new one.
            on_free(layout.size());
            on_alloc(new_size);
        }
        new
    }
}

struct Totals {
    allocated_bytes: u64,
    freed_bytes: u64,
    live_bytes: u64,
    live_allocs: u64,
}

fn totals() -> Totals {
    let (mut ab, mut fb, mut ac, mut fc) = (0u64, 0u64, 0u64, 0u64);
    for s in &COUNTERS {
        ab += s.allocated_bytes.load(Relaxed);
        fb += s.freed_bytes.load(Relaxed);
        ac += s.alloc_count.load(Relaxed);
        fc += s.free_count.load(Relaxed);
    }
    Totals {
        allocated_bytes: ab,
        freed_bytes: fb,
        live_bytes: ab.saturating_sub(fb),
        live_allocs: ac.saturating_sub(fc),
    }
}

struct RateCalculator {
    last_time: Option<Instant>,
    last_value: u64,
    last_rate: u64,
}

impl RateCalculator {
    const INTERVAL: Duration = Duration::from_millis(500);

    const fn new() -> Self {
        Self {
            last_time: None,
            last_value: 0,
            last_rate: 0,
        }
    }

    fn per_second(&mut self, value: u64) -> u64 {
        let now = Instant::now();
        if let Some(last) = self.last_time {
            let elapsed = now.duration_since(last);
            if elapsed < Self::INTERVAL {
                return self.last_rate;
            }
            let delta = value.saturating_sub(self.last_value);
            self.last_rate = (delta as f64 / elapsed.as_secs_f64()).round() as u64;
        }
        self.last_time = Some(now);
        self.last_value = value;
        self.last_rate
    }
}

#[derive(Clone, Copy, Default)]
pub struct MemoryStats {
    pub max: u64,
    pub live_bytes: u64,
    pub live_allocs: u64,
    pub alloc_bytes_per_sec: u64,
    pub free_bytes_per_sec: u64,
}

impl MemoryStats {
    pub fn sample() -> Self {
        struct State {
            sys: System,
            alloc_rate: RateCalculator,
            free_rate: RateCalculator,
        }
        static STATE: Mutex<Option<State>> = Mutex::new(None);
        let mut guard = STATE.lock();
        let st = guard.get_or_insert_with(|| State {
            sys: System::new(),
            alloc_rate: RateCalculator::new(),
            free_rate: RateCalculator::new(),
        });

        st.sys
            .refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());

        let t = totals();
        Self {
            max: st.sys.total_memory(),
            live_bytes: t.live_bytes,
            live_allocs: t.live_allocs,
            alloc_bytes_per_sec: st.alloc_rate.per_second(t.allocated_bytes),
            free_bytes_per_sec: st.free_rate.per_second(t.freed_bytes),
        }
    }
}
