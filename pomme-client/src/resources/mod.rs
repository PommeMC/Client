//! Resource bytes, independent of particles, render assets and the GPU.
//! Shared resource view for all standard asset consumers through registered
//! sources. Server-data domains are not implemented.
//!
//! Resource configuration owns locations, discovery and selection; downloading
//! is a separate operation. Providers open storage and hydrate asset indexes.
//! Its root metadata parser is private; resource sidecars remain queryable.
//! Source priority: selected packs above selected theme assets above the
//! optional external index above extracted assets above embedded Pomme
//! resources. Bootstrap needs only the embedded pack.
//! Built-ins are separate layers of one pack, not a composite source.
//! Filters apply between logical packs, not within their physical layers.
//! Overlays belong in source composition, not loaders. This layer
//! does not merge JSON, decode images or build atlases.
//!
//! A manager fixes source order, not filesystem contents. Handles outlive it
//! and pin a source or pathname, not immutable bytes; reopening may change or
//! fail. Sources must retain required storage leases. Streams close on drop.
//! Reload opens candidates before activation, then rebuilds consumers
//! synchronously. Providers and ordinary handles remain on the application
//! thread; content alone can opt into a transferable handle with local streams.

mod manager;
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

pub(crate) use manager::ResourceManager;
pub(crate) use resource::{
    ReadStream, Resource, ResourceContent, ResourceError, ResourceId, ResourceOrigin,
    ThreadSafeResourceContent,
};
pub(crate) use resource_id;

/// Owned registration snapshot issued by pack management in low-to-high order.
/// Storage, compiled filters and physical layers stay private to resources.
/// This fixes registrations, not filesystem contents; reload rereads metadata.
pub(crate) struct ResourceSources(Vec<packs::ResourcePackDescriptor>);
