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

#[cfg(test)]
impl dyn ResourcesSupplier {
    #![allow(
        dead_code,
        reason = "Generic API assertions pending user review before fixtures"
    )]

    pub(crate) fn test_open_metadata<S: ResourcesSupplier + ?Sized>(
        supplier: &S,
        location: &PackLocationInfo,
        expected_location_id: &str,
    ) {
        let actual = supplier
            .open_metadata(location)
            .expect("open_metadata should succeed");
        assert_eq!(
            actual.location().id,
            expected_location_id,
            "open_metadata location ID"
        );
    }

    pub(crate) fn test_open_resources<S: ResourcesSupplier + ?Sized>(
        supplier: &S,
        location: &PackLocationInfo,
        metadata: &PackMetadata,
        expected_location_ids: &[&str],
    ) {
        let actual = supplier
            .open_resources(location, metadata)
            .expect("open_resources should succeed");
        let ids: Vec<_> = actual
            .iter()
            .map(|pack| pack.location().id.as_str())
            .collect();
        assert_eq!(
            ids, expected_location_ids,
            "open_resources layer location IDs in order"
        );
    }
}

#[cfg(test)]
mod pack_api_tests {
    use super::{
        Component, FeatureFlagSet, Pack, PackEnablementConfig, PackFormat, PackLocationInfo,
        PackMetadata, PackType, Rc, ResourcesSupplier,
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

    pub(super) fn test_read_meta_and_create(
        location: PackLocationInfo,
        resources: Rc<dyn ResourcesSupplier>,
        pack_type: PackType,
        target: PackFormat,
        enablement: PackEnablementConfig,
        expected_location_id: &str,
    ) {
        let actual =
            Pack::read_meta_and_create(location, resources, pack_type, Some(target), enablement)
                .expect("read_meta_and_create should succeed")
                .expect("read_meta_and_create should return a pack");
        assert_eq!(
            actual.location().id,
            expected_location_id,
            "read_meta_and_create location ID"
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

    pub(super) fn test_requested_features(pack: &Pack, expected: FeatureFlagSet) {
        assert_eq!(pack.requested_features(), expected, "requested_features");
    }

    pub(super) fn test_chat_link(pack: &Pack, enabled: bool, expected: &Component) {
        assert_eq!(&pack.chat_link(enabled), expected, "chat_link component");
    }

    pub(super) fn test_enablement_config(pack: &Pack, expected: PackEnablementConfig) {
        assert_eq!(pack.enablement_config(), expected, "enablement_config");
    }

    pub(super) fn test_open_metadata(pack: &Pack, expected_location_id: &str) {
        let actual = pack.open_metadata().expect("open_metadata should succeed");
        assert_eq!(
            actual.location().id,
            expected_location_id,
            "open_metadata location ID"
        );
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
    use std::path::PathBuf;

    use super::{
        Component, FeatureFlagSet, Pack, PackCompat, PackEnablementConfig, PackFormat,
        PackLocationInfo, PackMetadata, PackPosition, PackType, Rc, io, pack_api_tests,
    };
    use crate::chat_component::{Argument, HoverEvent};
    use crate::resources::pack_resources::{EmbeddedResourcesSupplier, PathResourcesSupplier};
    use crate::resources::packs::PackSource;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> io::Result<Self> {
            for attempt in 0.. {
                let path = std::env::temp_dir()
                    .join(format!("pomme-pack-api-{}-{attempt}", std::process::id()));
                match fs::create_dir(&path) {
                    Ok(()) => return Ok(Self(path)),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                }
            }
            unreachable!()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            if let Err(error) = fs::remove_dir_all(&self.0) {
                tracing::warn!(path = %self.0.display(), %error, "Failed to remove test temporary directory");
            }
        }
    }

    fn location() -> PackLocationInfo {
        PackLocationInfo {
            id: "api-pack".to_owned(),
            title: Component::text("API pack"),
            source: PackSource::default(),
            known_pack_info: None,
        }
    }

    fn metadata() -> PackMetadata {
        PackMetadata {
            description: Component::text("API pack description"),
            compatibility: PackCompat::Compatible,
            requested_features: FeatureFlagSet::VANILLA,
            overlays: Vec::new(),
        }
    }

    fn enablement() -> PackEnablementConfig {
        PackEnablementConfig {
            required: false,
            default_position: PackPosition::Top,
            fixed_position: false,
        }
    }

    fn fixture() -> Pack {
        Pack::new(
            location(),
            metadata(),
            enablement(),
            Rc::new(EmbeddedResourcesSupplier::new()),
        )
    }

    #[test]
    fn test_new() {
        pack_api_tests::test_new(
            location(),
            metadata(),
            enablement(),
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
    fn test_read_meta_and_create() {
        let temp = TempDir::new().unwrap();
        fs::write(
            temp.0.join("pack.mcmeta"),
            br#"{"pack":{"pack_format":47,"description":"API pack description"}}"#,
        )
        .unwrap();
        pack_api_tests::test_read_meta_and_create(
            location(),
            Rc::new(PathResourcesSupplier::new(temp.0.clone())),
            PackType::ClientResources,
            PackFormat::new(47, 0).unwrap(),
            enablement(),
            "api-pack",
        );
    }

    #[test]
    fn test_location() {
        pack_api_tests::test_location(&fixture(), "api-pack");
    }

    #[test]
    fn test_requested_features() {
        pack_api_tests::test_requested_features(&fixture(), FeatureFlagSet::VANILLA);
    }

    #[test]
    fn test_chat_link() {
        let mut expected = Component::translate(
            "chat.square_brackets",
            vec![Argument::Component(Box::new(Component::text("api-pack")))],
        );
        expected.style.color = Some(0x55ff55);
        expected.style.insertion = Some("api-pack".to_owned());
        let mut hover = Component::text("");
        hover.siblings = vec![
            Component::text("API pack"),
            Component::text("\n"),
            Component::text("API pack description"),
        ];
        expected.style.hover_event = Some(HoverEvent::Text(Box::new(hover)));
        pack_api_tests::test_chat_link(&fixture(), true, &expected);
    }

    #[test]
    fn test_enablement_config() {
        pack_api_tests::test_enablement_config(
            &fixture(),
            PackEnablementConfig {
                required: false,
                default_position: PackPosition::Top,
                fixed_position: false,
            },
        );
    }

    #[test]
    fn test_open_metadata() {
        pack_api_tests::test_open_metadata(&fixture(), "api-pack");
    }

    #[test]
    fn test_open_resources() {
        pack_api_tests::test_open_resources(&fixture(), &["api-pack"]);
    }
}
