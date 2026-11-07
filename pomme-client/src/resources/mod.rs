//! Resource bytes, independent of particles, render assets and the GPU.
//! Asset consumers share the repository's activated view. Server-data domains
//! are not implemented.
//!
//! Repository sources own locations and discovery; the repository owns pack
//! selection. Downloading is separate. PackResources suppliers open storage
//! and hydrate asset indexes. Root metadata decoding is private; resource
//! sidecars remain queryable. Bootstrap needs only the embedded Pomme pack.
//! AppCore activates that bootstrap first, then configures local discovery,
//! installation and theme sources. Local UI and server selections activate
//! this same view before refreshing consumers.
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

#[allow(
    dead_code,
    reason = "Complete manager contract includes operations not used by every consumer"
)]
pub(crate) mod managers;
#[allow(
    dead_code,
    reason = "Complete backend contract includes operations not used by every consumer"
)]
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
