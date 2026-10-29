use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_PACK_ID: AtomicU64 = AtomicU64::new(1);

/// Monotonic process-unique registration identity, independent of source and
/// provenance.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) struct PackId(u64);

impl PackId {
    pub(super) fn allocate() -> Self {
        Self(NEXT_PACK_ID.fetch_add(1, Ordering::Relaxed))
    }
}
