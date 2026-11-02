//! Vanilla's distinct lookup, stack and enumeration algorithms for one
//! namespace.

use std::collections::{BTreeMap, HashSet};
use std::io;
use std::rc::Rc;

use super::{ResourceManager, Selector, io_error, listing_error};
use crate::resources::pack_resources::{IoSupplier, METADATA_EXTENSION, PackResources, PackType};
use crate::resources::{
    ReadStream, Resource, ResourceContent, ResourceError, ResourceId, ResourceOrigin,
};

pub(super) type PathFilter = Rc<dyn Fn(&str) -> bool>;
type MetadataFinder = Rc<dyn Fn() -> Result<Option<ResourceContent>, ResourceError>>;

// Deferred metadata lookups own both provenance and the backend, not the
// manager.
#[derive(Clone)]
struct ContentSource {
    origin: ResourceOrigin,
    pack: Rc<dyn PackResources>,
}

impl ContentSource {
    fn content(&self, id: ResourceId, supplier: IoSupplier<ReadStream>) -> ResourceContent {
        ResourceContent::from_supplier(id, self.origin.clone(), supplier)
    }

    fn metadata(
        &self,
        pack_type: PackType,
        id: &ResourceId,
    ) -> Result<Option<ResourceContent>, ResourceError> {
        self.pack
            .get_resource(pack_type, id)
            .map(|supplier| supplier.map(|supplier| self.content(id.clone(), supplier)))
            .map_err(|source| ResourceError::Io {
                operation: "lookup resource metadata",
                id: Some(id.clone()),
                origin: self.origin.clone(),
                source,
            })
    }
}

#[derive(Clone)]
struct PackEntry {
    name: String,
    source: Option<ContentSource>,
    filter: Option<PathFilter>,
}

impl PackEntry {
    fn is_filtered(&self, id: &ResourceId) -> bool {
        self.filter.as_ref().is_some_and(|filter| filter(id.path()))
    }
}

// Borrow sources only during enumeration; returned handles own their suppliers.
struct PackContentReference<'a> {
    source: &'a ContentSource,
    supplier: IoSupplier<ReadStream>,
}

struct EntryStack<'a> {
    metadata_id: ResourceId,
    files: Vec<PackContentReference<'a>>,
    metadata: Vec<PackContentReference<'a>>,
}

impl<'a> EntryStack<'a> {
    fn new(id: &ResourceId) -> Self {
        Self {
            metadata_id: id.sidecar(),
            files: Vec::new(),
            metadata: Vec::new(),
        }
    }

    fn apply_filter(&mut self, base_id: &ResourceId, entry: &PackEntry) {
        if entry.is_filtered(base_id) {
            self.files.clear();
        } else if entry.is_filtered(&self.metadata_id) {
            // Deliberate Vanilla else-if: a file filter does not also erase metadata.
            self.metadata.clear();
        }
    }

    fn push_file(&mut self, source: &'a ContentSource, supplier: IoSupplier<ReadStream>) {
        self.files.push(PackContentReference { source, supplier });
    }

    fn set_metadata(&mut self, source: &'a ContentSource, supplier: IoSupplier<ReadStream>) {
        let metadata = PackContentReference { source, supplier };
        if let Some(previous) = self
            .metadata
            .iter_mut()
            .find(|entry| Rc::ptr_eq(&entry.source.pack, &source.pack))
        {
            *previous = metadata;
        } else {
            self.metadata.push(metadata);
        }
    }

    fn into_resources(self, file_id: &ResourceId) -> Option<Vec<Resource>> {
        if self.files.is_empty() {
            return None;
        }
        Some(
            self.files
                .into_iter()
                .map(|PackContentReference { source, supplier }| {
                    let content = source.content(file_id.clone(), supplier);
                    let metadata = self
                        .metadata
                        .iter()
                        .find(|entry| Rc::ptr_eq(&entry.source.pack, &source.pack))
                        .map(|entry| {
                            entry
                                .source
                                .content(self.metadata_id.clone(), entry.supplier.clone())
                        });
                    Resource::new(content, constant_metadata(metadata))
                })
                .collect(),
        )
    }
}

/// Entries are ordered low to high. Pushes cannot change previously returned
/// resources' owned metadata snapshots. Direct lookup respects this namespace
/// partition, even if a registered pack supplies other namespaces as well.
pub(crate) struct FallbackResourceManager {
    pack_type: PackType,
    namespace: String,
    entries: Vec<PackEntry>,
}

impl FallbackResourceManager {
    pub(crate) fn new(pack_type: PackType, namespace: String) -> Self {
        Self {
            pack_type,
            namespace,
            entries: Vec::new(),
        }
    }

    pub(super) fn push(
        &mut self,
        origin: ResourceOrigin,
        pack: Rc<dyn PackResources>,
        filter: Option<PathFilter>,
    ) {
        self.entries.push(PackEntry {
            name: origin.name.clone(),
            source: Some(ContentSource { origin, pack }),
            filter,
        });
    }

    pub(super) fn push_filter_only(&mut self, name: String, filter: PathFilter) {
        self.entries.push(PackEntry {
            name,
            source: None,
            filter: Some(filter),
        });
    }

    fn stack_metadata_finder(&self, id: &ResourceId, winning_index: usize) -> MetadataFinder {
        let entries = self.entries[winning_index..].to_vec();
        let metadata_id = id.sidecar();
        let pack_type = self.pack_type;
        Rc::new(move || {
            for entry in entries.iter().rev() {
                if let Some(source) = &entry.source
                    && let Some(metadata) = source.metadata(pack_type, &metadata_id)?
                {
                    return Ok(Some(metadata));
                }
                if entry.is_filtered(&metadata_id) {
                    break;
                }
            }
            Ok(None)
        })
    }
}

impl ResourceManager for FallbackResourceManager {
    /// Scan high to low; defer metadata lookup to the owned winning-to-top
    /// snapshot.
    fn get_resource(&self, id: &ResourceId) -> io::Result<Option<Resource>> {
        if id.namespace() != self.namespace {
            return Ok(None);
        }
        for (index, entry) in self.entries.iter().enumerate().rev() {
            if let Some(source) = &entry.source
                && let Some(supplier) = source
                    .pack
                    .get_resource(self.pack_type, id)
                    .map_err(|error| io_error("get resource", &source.origin, Some(id), error))?
            {
                return Ok(Some(Resource::new(
                    source.content(id.clone(), supplier),
                    self.stack_metadata_finder(id, index),
                )));
            }
            // Own content wins before this pack's filter applies to lower packs.
            if entry.is_filtered(id) {
                tracing::warn!(%id, pack = %entry.name, "Resource not found, but filtered by pack");
                break;
            }
        }
        Ok(None)
    }

    fn get_namespaces(&self) -> io::Result<HashSet<String>> {
        Ok(HashSet::from([self.namespace.clone()]))
    }

    /// Scan high to low with source-local metadata; filters affect only lower
    /// entries.
    fn get_resource_stack(&self, id: &ResourceId) -> io::Result<Vec<Resource>> {
        if id.namespace() != self.namespace {
            return Ok(Vec::new());
        }
        let metadata_id = id.sidecar();
        let mut resources = Vec::new();
        let mut metadata_blocked = false;
        for entry in self.entries.iter().rev() {
            if let Some(source) = &entry.source
                && let Some(supplier) =
                    source
                        .pack
                        .get_resource(self.pack_type, id)
                        .map_err(|error| {
                            io_error("get resource stack", &source.origin, Some(id), error)
                        })?
            {
                let metadata: MetadataFinder = if metadata_blocked {
                    constant_metadata(None)
                } else {
                    let source = source.clone();
                    let metadata_id = metadata_id.clone();
                    let pack_type = self.pack_type;
                    Rc::new(move || source.metadata(pack_type, &metadata_id))
                };
                resources.push(Resource::new(
                    source.content(id.clone(), supplier),
                    metadata,
                ));
            }
            if entry.is_filtered(id) {
                if resources.is_empty() {
                    tracing::warn!(%id, pack = %entry.name, "Resource not found, but filtered by pack");
                }
                break;
            }
            if entry.is_filtered(&metadata_id) {
                metadata_blocked = true;
            }
        }
        resources.reverse();
        Ok(resources)
    }

    /// Filter previous winners, then enumerate low to high; metadata uses
    /// priority, not identity.
    fn list_resources(
        &self,
        directory: &str,
        selector: &Selector<'_>,
    ) -> io::Result<BTreeMap<ResourceId, Resource>> {
        let mut files: BTreeMap<ResourceId, (usize, ResourceContent)> = BTreeMap::new();
        let mut metadata: BTreeMap<ResourceId, (usize, ResourceContent)> = BTreeMap::new();
        for (priority, entry) in self.entries.iter().enumerate() {
            files.retain(|id, _| !entry.is_filtered(id));
            metadata.retain(|id, _| !entry.is_filtered(id));
            if let Some(source) = &entry.source {
                source
                    .pack
                    .list_resources(
                        self.pack_type,
                        &self.namespace,
                        directory,
                        &mut |id, supplier| {
                            if id.path().ends_with(METADATA_EXTENSION) {
                                if let Some(base) = metadata_base(&id, &source.origin)
                                    && selector(&base)
                                {
                                    let content = source.content(id.clone(), supplier);
                                    metadata.insert(id, (priority, content));
                                }
                            } else if selector(&id) {
                                let content = source.content(id.clone(), supplier);
                                files.insert(id, (priority, content));
                            }
                        },
                    )
                    .map_err(|error| {
                        listing_error(
                            "list resources",
                            &source.origin,
                            &self.namespace,
                            directory,
                            error,
                        )
                    })?;
            }
        }
        Ok(files
            .into_iter()
            .map(|(id, (priority, content))| {
                let sidecar = metadata
                    .remove(&id.sidecar())
                    .filter(|(metadata_priority, _)| *metadata_priority >= priority)
                    .map(|(_, content)| content);
                (id, Resource::new(content, constant_metadata(sidecar)))
            })
            .collect())
    }

    /// Filter then append low to high; each stack pairs metadata by source
    /// identity, not priority.
    fn list_resource_stacks(
        &self,
        directory: &str,
        selector: &Selector<'_>,
    ) -> io::Result<BTreeMap<ResourceId, Vec<Resource>>> {
        let mut stacks: BTreeMap<ResourceId, EntryStack<'_>> = BTreeMap::new();
        for entry in &self.entries {
            for (id, stack) in &mut stacks {
                stack.apply_filter(id, entry);
            }
            if let Some(source) = &entry.source {
                source
                    .pack
                    .list_resources(
                        self.pack_type,
                        &self.namespace,
                        directory,
                        &mut |id, supplier| {
                            if id.path().ends_with(METADATA_EXTENSION) {
                                if let Some(base) = metadata_base(&id, &source.origin)
                                    && selector(&base)
                                {
                                    let stack = stacks
                                        .entry(base.clone())
                                        .or_insert_with(|| EntryStack::new(&base));
                                    stack.set_metadata(source, supplier);
                                }
                            } else if selector(&id) {
                                let stack = stacks
                                    .entry(id.clone())
                                    .or_insert_with(|| EntryStack::new(&id));
                                stack.push_file(source, supplier);
                            }
                        },
                    )
                    .map_err(|error| {
                        listing_error(
                            "list resource stacks",
                            &source.origin,
                            &self.namespace,
                            directory,
                            error,
                        )
                    })?;
            }
        }
        Ok(stacks
            .into_iter()
            .filter_map(|(id, stack)| stack.into_resources(&id).map(|resources| (id, resources)))
            .collect())
    }

    fn list_packs(&self) -> Box<dyn Iterator<Item = &dyn PackResources> + '_> {
        Box::new(
            self.entries
                .iter()
                .filter_map(|entry| entry.source.as_ref().map(|source| source.pack.as_ref())),
        )
    }
}

fn constant_metadata(content: Option<ResourceContent>) -> MetadataFinder {
    // Enumeration retains the exact supplier; metadata access must not relookup.
    Rc::new(move || Ok(content.clone()))
}

fn metadata_base(id: &ResourceId, origin: &ResourceOrigin) -> Option<ResourceId> {
    let path = id.path().strip_suffix(METADATA_EXTENSION)?;
    match ResourceId::from_parts(id.namespace(), path) {
        Ok(base) => Some(base),
        Err(error) => {
            // A bare .mcmeta cannot name a base under Rust's nonempty-ID rule.
            tracing::warn!(%id, pack = %origin.name, pack_id = ?origin.pack_id, %error,
                "Skipping metadata with an unrepresentable base resource ID");
            None
        }
    }
}

#[cfg(test)]
mod resource_manager_tests {
    use std::collections::{BTreeMap, HashSet};
    use std::rc::Rc;

    use super::FallbackResourceManager;
    use crate::chat_component::Component;
    use crate::resources::managers::tests;
    use crate::resources::pack_resources::{
        EmbeddedPackResources, EmbeddedResourcesSupplier, POMME_ICON, POMME_ICONS_FONT,
        POMME_UI_FONT, PackResources, PackType,
    };
    use crate::resources::packs::{
        FeatureFlagSet, Pack, PackCompat, PackEnablementConfig, PackLocationInfo, PackMetadata,
        PackPosition, PackSource,
    };
    use crate::resources::{ResourceId, resource_id};

    const ICON: &[u8] = include_bytes!("../../../assets/icon.png");
    const UI_FONT: &[u8] = include_bytes!("../../renderer/fonts/Montserrat-Medium.ttf");
    const ICONS_FONT: &[u8] = include_bytes!("../../renderer/fonts/fa-solid-900.ttf");

    fn fixture() -> FallbackResourceManager {
        let location = PackLocationInfo {
            id: "resource-manager-api".to_owned(),
            title: Component::text("Resource manager API fixture"),
            source: PackSource::built_in(),
            known_pack_info: None,
        };
        let metadata = PackMetadata {
            description: Component::text("Resource manager API fixture"),
            compatibility: PackCompat::Compatible,
            requested_features: FeatureFlagSet::empty(),
            overlays: Vec::new(),
        };
        let enablement = PackEnablementConfig {
            required: false,
            default_position: PackPosition::Top,
            fixed_position: false,
        };
        let pack = Pack::new(
            location.clone(),
            metadata,
            enablement,
            Rc::new(EmbeddedResourcesSupplier::new()),
        );
        let backend: Rc<dyn PackResources> = Rc::new(EmbeddedPackResources::new(location));
        let mut manager =
            FallbackResourceManager::new(PackType::ClientResources, "pomme".to_owned());
        manager.push(pack.origin(), backend, None);
        manager
    }

    #[test]
    fn test_get_resource() {
        tests::test_get_resource(&fixture(), &POMME_ICON, ICON);
    }

    #[test]
    fn test_get_resource_missing() {
        tests::test_get_resource_missing(&fixture(), &resource_id!("pomme:textures/missing.png"));
    }

    #[test]
    fn test_get_resource_stack() {
        tests::test_get_resource_stack(&fixture(), &POMME_ICON, &[ICON.to_vec()]);
    }

    #[test]
    fn test_get_resource_stack_missing() {
        tests::test_get_resource_stack(
            &fixture(),
            &resource_id!("pomme:textures/missing.png"),
            &[],
        );
    }

    #[test]
    fn test_list_resources() {
        tests::test_list_resources(
            &fixture(),
            "font",
            &|_| true,
            &BTreeMap::from([
                (POMME_UI_FONT, UI_FONT.to_vec()),
                (POMME_ICONS_FONT, ICONS_FONT.to_vec()),
            ]),
        );
    }

    #[test]
    fn test_list_resources_missing_directory() {
        tests::test_list_resources(&fixture(), "missing", &|_| true, &BTreeMap::new());
    }

    #[test]
    fn test_list_resource_stacks() {
        tests::test_list_resource_stacks(
            &fixture(),
            "font",
            &|_| true,
            &BTreeMap::from([
                (POMME_UI_FONT, vec![UI_FONT.to_vec()]),
                (POMME_ICONS_FONT, vec![ICONS_FONT.to_vec()]),
            ]),
        );
    }

    #[test]
    fn test_list_resource_stacks_missing_directory() {
        tests::test_list_resource_stacks(&fixture(), "missing", &|_| true, &BTreeMap::new());
    }

    #[test]
    fn test_get_namespaces() {
        tests::test_get_namespaces(&fixture(), &HashSet::from(["pomme".to_owned()]));
    }

    #[test]
    fn test_list_packs() {
        tests::test_list_packs(&fixture(), &["resource-manager-api"]);
    }
}
