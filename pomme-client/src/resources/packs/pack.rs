//! Logical pack snapshots and reopenable suppliers, independent of discovery
//! and opened resource managers. Metadata computation belongs to pack
//! factories.

use std::io;
use std::rc::Rc;

use super::features::{FEATURES_SECTION, FeatureFlagSet};
use super::metadata::{self, OVERLAYS_SECTION, PACK_SECTION};
use super::{PackCompat, PackFormat, PackId, PackLocationInfo};
use crate::chat_component::Component;
use crate::resources::ResourceOrigin;
use crate::resources::pack_resources::{PackMetadataResources, PackResources, PackType};

/// Computed metadata for one target format, not the raw pack.mcmeta parser.
/// Applicable overlay directories are already validated, in declaration order.
/// Requested flags use the fixed Minecraft 26.3 main feature universe.
#[derive(Clone)]
pub(crate) struct PackMetadata {
    pub(crate) description: Component,
    pub(crate) compatibility: PackCompat,
    pub(crate) requested_features: FeatureFlagSet,
    pub(crate) overlays: Vec<String>,
}

/// Default insertion edge in a low-to-high priority list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PackPosition {
    Bottom,
    Top,
}

impl PackPosition {
    pub(crate) fn opposite(self) -> Self {
        match self {
            Self::Bottom => Self::Top,
            Self::Top => Self::Bottom,
        }
    }

    /// Insert inside the contiguous fixed-position block at the chosen edge.
    /// Reversal changes the scan direction, not the position being compared,
    /// matching Vanilla's Pack.Position.insert. This does not normalize a list.
    pub(crate) fn insert<T>(
        self,
        list: &mut Vec<T>,
        value: T,
        enablement: impl Fn(&T) -> PackEnablementConfig,
        reverse: bool,
    ) -> usize {
        let fixed_at_edge = |entry: &T| {
            let config = enablement(entry);
            config.fixed_position && config.default_position == self
        };
        let direction = if reverse { self.opposite() } else { self };
        let index = match direction {
            Self::Bottom => list.iter().take_while(|entry| fixed_at_edge(entry)).count(),
            Self::Top => {
                list.len()
                    - list
                        .iter()
                        .rev()
                        .take_while(|entry| fixed_at_edge(entry))
                        .count()
            }
        };
        list.insert(index, value);
        index
    }
}

/// Independent enablement and ordering attributes, not global mutation
/// restrictions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PackEnablementConfig {
    /// Insert the pack if missing when reconciling the enabled list.
    pub(crate) required: bool,
    /// Default insertion edge in the enabled pack order.
    pub(crate) default_position: PackPosition,
    /// Participate in the contiguous fixed-position block at the default edge.
    pub(crate) fixed_position: bool,
}

/// Object-safe, application-thread factory retaining the state needed to
/// reopen. Metadata access must be independent of active resource views:
/// closing a temporary metadata view must not invalidate an active ZIP
/// generation. Resource opening must prepare fresh views for each reload,
/// rather than reuse close-invalidated generations or stale backend metadata
/// caches. Returned views and their suppliers retain their own storage
/// ownership.
pub(crate) trait ResourcesSupplier {
    fn open_metadata(
        &self,
        location: &PackLocationInfo,
    ) -> io::Result<Rc<dyn PackMetadataResources>>;

    /// One logical pack can emit multiple low-to-high physical layers (notably
    /// built-ins). A directory/ZIP plus its overlays is instead one composed
    /// view. Metadata is the pack's already-computed discovery snapshot.
    fn open_resources(
        &self,
        location: &PackLocationInfo,
        metadata: &PackMetadata,
    ) -> io::Result<Vec<Rc<dyn PackResources>>>;
}

/// Immutable logical snapshot. Cloning preserves registration identity and the
/// reopenable supplier, not an already-open resource view.
#[derive(Clone)]
pub(crate) struct Pack {
    id: PackId,
    location: PackLocationInfo,
    metadata: PackMetadata,
    enablement_config: PackEnablementConfig,
    resources: Rc<dyn ResourcesSupplier>,
}

impl Pack {
    /// Read and close one independent temporary metadata view. None means only
    /// an absent pack section; sources decide whether to log/skip errors.
    /// A missing bootstrap target yields Unknown/no overlays, but still fully
    /// validates features and overlays. Java always supplies a known target.
    pub(crate) fn read_metadata(
        location: &PackLocationInfo,
        resources: &dyn ResourcesSupplier,
        pack_type: PackType,
        target: Option<PackFormat>,
    ) -> io::Result<Option<PackMetadata>> {
        let opened = resources
            .open_metadata(location)
            .map_err(|error| metadata_error(location, "open metadata", error))?;
        let result = compute_metadata(location, opened.as_ref(), pack_type, target);
        let closed = opened
            .close()
            .map_err(|error| metadata_error(location, "close metadata", error));
        match (result, closed) {
            (result, Ok(())) => result,
            (Ok(_), Err(error)) => Err(error),
            (Err(error), Err(close_error)) => Err(io::Error::new(
                error.kind(),
                format!("{error}; additionally: {close_error}"),
            )),
        }
    }

    /// Allocate registration identity only after metadata and close succeed.
    /// Refreshing sources can instead use read_metadata followed by with_id.
    pub(crate) fn read_meta_and_create(
        location: PackLocationInfo,
        resources: Rc<dyn ResourcesSupplier>,
        pack_type: PackType,
        target: Option<PackFormat>,
        enablement_config: PackEnablementConfig,
    ) -> io::Result<Option<Self>> {
        let metadata = Self::read_metadata(&location, resources.as_ref(), pack_type, target)?;
        Ok(metadata.map(|metadata| Self::new(location, metadata, enablement_config, resources)))
    }

    /// Allocate a new opaque registration once, independently of textual ID.
    pub(crate) fn new(
        location: PackLocationInfo,
        metadata: PackMetadata,
        enablement_config: PackEnablementConfig,
        resources: Rc<dyn ResourcesSupplier>,
    ) -> Self {
        Self::with_id(
            PackId::allocate(),
            location,
            metadata,
            enablement_config,
            resources,
        )
    }

    /// Refresh an existing registration's snapshot without allocating identity.
    /// Discovery sources own the association between entries and retained IDs.
    pub(crate) fn with_id(
        id: PackId,
        location: PackLocationInfo,
        metadata: PackMetadata,
        enablement_config: PackEnablementConfig,
        resources: Rc<dyn ResourcesSupplier>,
    ) -> Self {
        Self {
            id,
            location,
            metadata,
            enablement_config,
            resources,
        }
    }

    pub(crate) fn id(&self) -> &PackId {
        &self.id
    }

    /// Textual location identity and presentation, distinct from `id()`.
    pub(crate) fn location(&self) -> &PackLocationInfo {
        &self.location
    }

    pub(crate) fn metadata(&self) -> &PackMetadata {
        &self.metadata
    }

    pub(crate) fn requested_features(&self) -> FeatureFlagSet {
        self.metadata.requested_features
    }

    pub(crate) fn chat_link(&self, enabled: bool) -> Component {
        self.location
            .create_chat_link(enabled, &self.metadata.description)
    }

    pub(crate) fn enablement_config(&self) -> PackEnablementConfig {
        self.enablement_config
    }

    pub(crate) fn origin(&self) -> ResourceOrigin {
        ResourceOrigin {
            pack_id: self.id.clone(),
            name: self.location.title.plain_text(),
        }
    }

    pub(crate) fn open_metadata(&self) -> io::Result<Rc<dyn PackMetadataResources>> {
        self.resources.open_metadata(&self.location)
    }

    pub(crate) fn open_resources(&self) -> io::Result<Vec<Rc<dyn PackResources>>> {
        self.resources
            .open_resources(&self.location, &self.metadata)
    }
}

/// Borrowing computation permits early error returns without bypassing the
/// owning reader's single explicit close. Never opens/closes active layers.
fn compute_metadata(
    location: &PackLocationInfo,
    resources: &dyn PackMetadataResources,
    pack_type: PackType,
    target: Option<PackFormat>,
) -> io::Result<Option<PackMetadata>> {
    let section = resources
        .metadata_section_json(PACK_SECTION)
        .map_err(|error| metadata_error(location, "read pack section", error))?;
    let Some(section) = section else {
        return Ok(None);
    };
    let (description, compatibility) =
        metadata::decode_pack_section(&section, pack_type, target, &location.id)
            .map_err(|error| metadata_error(location, "decode pack section", error))?;
    let features = resources
        .metadata_section_json(FEATURES_SECTION)
        .map_err(|error| metadata_error(location, "read features section", error))?;
    let requested_features = match features {
        Some(section) => FeatureFlagSet::decode(&section)
            .map_err(|error| metadata_error(location, "decode features section", error))?,
        None => FeatureFlagSet::empty(),
    };
    let overlays = resources
        .metadata_section_json(OVERLAYS_SECTION)
        .map_err(|error| metadata_error(location, "read overlays section", error))?;
    let overlays =
        metadata::selected_overlay_directories(overlays.as_ref(), pack_type, target, &location.id)
            .map_err(|error| metadata_error(location, "decode overlays section", error))?;
    Ok(Some(PackMetadata {
        description,
        compatibility,
        requested_features,
        overlays,
    }))
}

fn metadata_error(location: &PackLocationInfo, phase: &str, error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!("pack {:?}: {phase}: {error}", location.id),
    )
}
