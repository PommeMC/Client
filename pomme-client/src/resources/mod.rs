//! Resource bytes, independent of particles, render assets and the GPU.
//! Asset consumers share the repository's activated view. Server-data domains
//! are not implemented.
//!
//! Repository sources own locations and discovery; the repository owns pack
//! selection. Downloading is separate. PackResources suppliers open storage
//! and hydrate asset indexes. Root metadata decoding is private; resource
//! sidecars remain queryable. As in vanilla, a pack or source that fails is
//! logged and left out rather than failing a lookup or a reload, so the
//! embedded Pomme pack always loads. AppCore activates that bootstrap first,
//! then configures local discovery, installation and theme sources. Local UI
//! and server selections activate this same view before refreshing consumers.
//! The Vanilla supplier exposes extracted and indexed entries with their own
//! root/fallback metadata: managers apply filters between those entries too,
//! even with the same logical origin.
//! Ordinary directory/ZIP overlays remain one composed entry with primary-only
//! root metadata.
//! Overlays belong in source composition, not loaders. This layer
//! does not merge JSON, decode images or build atlases.
//!
//! A manager fixes source order, not filesystem contents. Handles outlive it
//! and pin a source or pathname, not immutable bytes; reopening may change or
//! fail. Sources must retain required storage leases. Streams close on drop.
//! Reload opens candidates before activation, then invokes a synchronous
//! rebuild callback. Managers, pack views and lazy metadata lookup remain
//! application-thread-local; selected ResourceContent handles and their owned
//! readers can move across threads.

pub(crate) mod managers;
pub(crate) mod pack_resources;
pub(crate) mod packs;
mod resource;

/// Builds a resource ID value from a static string expression, validated at
/// compile time. Use `ResourceId::new` for fallible construction from dynamic
/// strings.
macro_rules! resource_id {
    ($value:expr $(,)?) => {
        const { $crate::resources::ResourceId::literal($value) }
    };
}

pub(crate) use managers::ResourceManager;
pub(crate) use resource::{
    ReadStream, Resource, ResourceContent, ResourceError, ResourceId, ResourceOrigin,
};
pub(crate) use resource_id;

/// A scratch directory for tests, removed on drop. Names never repeat within
/// a process: Windows refuses to create a directory whose deletion is still
/// pending, which a reused name hits when tests run in parallel.
#[cfg(test)]
pub(crate) mod test_dir {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::{fs, io};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    pub(crate) struct TempDir(pub(crate) PathBuf);

    impl TempDir {
        pub(crate) fn new() -> io::Result<Self> {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("pomme-resources-{}-{id}", std::process::id()));
            // A leftover from an earlier process with the same id.
            let _ = fs::remove_dir_all(&path);
            fs::create_dir(&path)?;
            Ok(Self(path))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            if let Err(error) = fs::remove_dir_all(&self.0) {
                tracing::warn!(path = %self.0.display(), %error, "Failed to remove test temporary directory");
            }
        }
    }
}
