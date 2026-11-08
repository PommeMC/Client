//! Logical pack snapshots and reopenable suppliers, independent of discovery
//! and opened resource managers. Metadata computation belongs to pack
//! factories.

use std::io;
use std::rc::Rc;

use super::features::{self, FEATURES_SECTION};
use super::metadata::{self, OVERLAYS_SECTION, PACK_SECTION};
use super::{PackCompat, PackFormat, PackId, PackLocationInfo};
use crate::chat_component::Component;
use crate::resources::ResourceOrigin;
use crate::resources::pack_resources::{PackMetadataResources, PackResources, PackType};

/// Computed metadata for one target format, not the raw pack.mcmeta parser.
/// Applicable overlay directories are already validated, in declaration order.
#[derive(Clone)]
pub(crate) struct PackMetadata {
    pub(crate) description: Component,
    pub(crate) compatibility: PackCompat,
    pub(crate) overlays: Vec<String>,
}

#[cfg(test)]
impl PackMetadata {
    pub(crate) fn test() -> Self {
        Self {
            description: Component::text("test"),
            compatibility: PackCompat::Compatible,
            overlays: Vec::new(),
        }
    }
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

impl PackEnablementConfig {
    /// Vanilla `FolderRepositorySource.DISCOVERED_PACK_SELECTION_CONFIG`.
    pub(crate) const DISCOVERED: Self = Self {
        required: false,
        default_position: PackPosition::Top,
        fixed_position: false,
    };
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

    pub(crate) fn enablement_config(&self) -> PackEnablementConfig {
        self.enablement_config
    }

    pub(crate) fn origin(&self) -> ResourceOrigin {
        ResourceOrigin {
            name: self.location.id.clone(),
        }
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
    if let Some(section) = features {
        features::validate(&section)
            .map_err(|error| metadata_error(location, "decode features section", error))?;
    }
    let overlays = resources
        .metadata_section_json(OVERLAYS_SECTION)
        .map_err(|error| metadata_error(location, "read overlays section", error))?;
    let overlays =
        metadata::selected_overlay_directories(overlays.as_ref(), pack_type, target, &location.id)
            .map_err(|error| metadata_error(location, "decode overlays section", error))?;
    Ok(Some(PackMetadata {
        description,
        compatibility,
        overlays,
    }))
}

fn metadata_error(location: &PackLocationInfo, phase: &str, error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!("pack {:?}: {phase}: {error}", location.id),
    )
}

#[cfg(test)]
impl dyn ResourcesSupplier {
    /// Opens `supplier`'s metadata and resources, expecting `layers` resource
    /// views that all carry the requested location.
    pub(crate) fn test_open<S: ResourcesSupplier + ?Sized>(supplier: &S, layers: usize) {
        let location = PackLocationInfo::test("supplier-api");
        let metadata = supplier
            .open_metadata(&location)
            .expect("open_metadata should succeed");
        assert_eq!(
            metadata.location().id,
            location.id,
            "open_metadata location ID"
        );
        let ids: Vec<_> = supplier
            .open_resources(&location, &PackMetadata::test())
            .expect("open_resources should succeed")
            .iter()
            .map(|pack| pack.location().id.clone())
            .collect();
        assert_eq!(
            ids,
            vec![location.id; layers],
            "open_resources layer location IDs"
        );
    }
}

#[cfg(test)]
mod pack_api_tests {
    use super::{
        Component, Pack, PackEnablementConfig, PackFormat, PackLocationInfo, PackMetadata,
        PackType, Rc, ResourcesSupplier,
    };

    pub(super) fn test_read_metadata(
        location: &PackLocationInfo,
        resources: &dyn ResourcesSupplier,
        pack_type: PackType,
        target: PackFormat,
        expected_description: &Component,
    ) {
        let actual = Pack::read_metadata(location, resources, pack_type, Some(target))
            .expect("read_metadata should succeed")
            .expect("read_metadata should return metadata");
        assert_eq!(
            &actual.description, expected_description,
            "read_metadata description"
        );
    }

    pub(super) fn test_new(
        location: PackLocationInfo,
        metadata: PackMetadata,
        enablement: PackEnablementConfig,
        resources: Rc<dyn ResourcesSupplier>,
        expected_location_id: &str,
    ) {
        let actual = Pack::new(location, metadata, enablement, resources);
        assert_eq!(
            actual.location().id,
            expected_location_id,
            "new location ID"
        );
    }

    pub(super) fn test_location(pack: &Pack, expected_location_id: &str) {
        assert_eq!(pack.location().id, expected_location_id, "location ID");
    }

    pub(super) fn test_enablement_config(pack: &Pack, expected: PackEnablementConfig) {
        assert_eq!(pack.enablement_config(), expected, "enablement_config");
    }

    pub(super) fn test_open_resources(pack: &Pack, expected_location_ids: &[&str]) {
        let actual = pack
            .open_resources()
            .expect("open_resources should succeed");
        let ids: Vec<_> = actual
            .iter()
            .map(|resources| resources.location().id.as_str())
            .collect();
        assert_eq!(ids, expected_location_ids, "open_resources location IDs");
    }
}

#[cfg(test)]
mod pack_tests {
    use std::fs;

    use super::{
        Component, Pack, PackEnablementConfig, PackFormat, PackLocationInfo, PackMetadata,
        PackType, Rc, pack_api_tests,
    };
    use crate::resources::pack_resources::{EmbeddedResourcesSupplier, PathResourcesSupplier};
    use crate::resources::test_dir::TempDir;

    fn location() -> PackLocationInfo {
        PackLocationInfo::test("api-pack")
    }

    fn metadata() -> PackMetadata {
        PackMetadata::test()
    }

    fn fixture() -> Pack {
        Pack::new(
            location(),
            metadata(),
            PackEnablementConfig::DISCOVERED,
            Rc::new(EmbeddedResourcesSupplier::new()),
        )
    }

    #[test]
    fn test_new() {
        pack_api_tests::test_new(
            location(),
            metadata(),
            PackEnablementConfig::DISCOVERED,
            Rc::new(EmbeddedResourcesSupplier::new()),
            "api-pack",
        );
    }

    #[test]
    fn test_read_metadata() {
        let temp = TempDir::new().unwrap();
        fs::write(
            temp.0.join("pack.mcmeta"),
            br#"{"pack":{"pack_format":47,"description":"API pack description"}}"#,
        )
        .unwrap();
        let supplier = PathResourcesSupplier::new(temp.0.clone());
        pack_api_tests::test_read_metadata(
            &location(),
            &supplier,
            PackType::ClientResources,
            PackFormat::new(47, 0).unwrap(),
            &Component::text("API pack description"),
        );
    }

    #[test]
    fn test_location() {
        pack_api_tests::test_location(&fixture(), "api-pack");
    }

    #[test]
    fn test_enablement_config() {
        pack_api_tests::test_enablement_config(&fixture(), PackEnablementConfig::DISCOVERED);
    }

    #[test]
    fn test_open_resources() {
        pack_api_tests::test_open_resources(&fixture(), &["api-pack"]);
    }
}
