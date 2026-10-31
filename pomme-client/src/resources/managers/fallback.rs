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

struct ListedResource {
    source: Rc<dyn PackResources>,
    content: ResourceContent,
    priority: usize,
}

impl ListedResource {
    fn new(
        source: &ContentSource,
        id: ResourceId,
        supplier: IoSupplier<ReadStream>,
        priority: usize,
    ) -> Self {
        Self {
            source: source.pack.clone(),
            content: source.content(id, supplier),
            priority,
        }
    }
}

struct EntryStack {
    metadata_id: ResourceId,
    files: Vec<ListedResource>,
    // Small per-pack map: Rc object identity, not registration position or name.
    metadata: Vec<ListedResource>,
}

impl EntryStack {
    fn new(id: &ResourceId) -> Self {
        Self {
            metadata_id: id.sidecar(),
            files: Vec::new(),
            metadata: Vec::new(),
        }
    }

    fn insert_metadata(&mut self, metadata: ListedResource) {
        if let Some(previous) = self
            .metadata
            .iter_mut()
            .find(|entry| Rc::ptr_eq(&entry.source, &metadata.source))
        {
            *previous = metadata;
        } else {
            self.metadata.push(metadata);
        }
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

    fn list_resources(
        &self,
        directory: &str,
        selector: &Selector<'_>,
    ) -> io::Result<BTreeMap<ResourceId, Resource>> {
        let mut files: BTreeMap<ResourceId, ListedResource> = BTreeMap::new();
        let mut metadata: BTreeMap<ResourceId, ListedResource> = BTreeMap::new();
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
                                    let record =
                                        ListedResource::new(source, id.clone(), supplier, priority);
                                    metadata.insert(id, record);
                                }
                            } else if selector(&id) {
                                let record =
                                    ListedResource::new(source, id.clone(), supplier, priority);
                                files.insert(id, record);
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
            .map(|(id, file)| {
                let sidecar = metadata
                    .remove(&id.sidecar())
                    .filter(|metadata| metadata.priority >= file.priority)
                    .map(|metadata| metadata.content);
                (id, Resource::new(file.content, constant_metadata(sidecar)))
            })
            .collect())
    }

    fn list_resource_stacks(
        &self,
        directory: &str,
        selector: &Selector<'_>,
    ) -> io::Result<BTreeMap<ResourceId, Vec<Resource>>> {
        let mut stacks: BTreeMap<ResourceId, EntryStack> = BTreeMap::new();
        for (priority, entry) in self.entries.iter().enumerate() {
            for (id, stack) in &mut stacks {
                if entry.is_filtered(id) {
                    stack.files.clear();
                } else if entry.is_filtered(&stack.metadata_id) {
                    // Deliberate Vanilla else-if: a file filter does not also
                    // erase source-local metadata, including repeated Rc entries.
                    stack.metadata.clear();
                }
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
                                    stack.insert_metadata(ListedResource::new(
                                        source, id, supplier, priority,
                                    ));
                                }
                            } else if selector(&id) {
                                let stack = stacks
                                    .entry(id.clone())
                                    .or_insert_with(|| EntryStack::new(&id));
                                stack
                                    .files
                                    .push(ListedResource::new(source, id, supplier, priority));
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
        let mut result = BTreeMap::new();
        for (id, stack) in stacks {
            if stack.files.is_empty() {
                continue;
            }
            let resources = stack
                .files
                .into_iter()
                .map(|file| {
                    let metadata = stack
                        .metadata
                        .iter()
                        .find(|metadata| Rc::ptr_eq(&metadata.source, &file.source))
                        .map(|metadata| metadata.content.clone());
                    Resource::new(file.content, constant_metadata(metadata))
                })
                .collect();
            result.insert(id, resources);
        }
        Ok(result)
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
    match ResourceId::new(&format!("{}:{path}", id.namespace())) {
        Ok(base) => Some(base),
        Err(error) => {
            // A bare .mcmeta cannot name a base under Rust's nonempty-ID rule.
            tracing::warn!(%id, pack = %origin.name, pack_id = ?origin.pack_id, %error,
                "Skipping metadata with an unrepresentable base resource ID");
            None
        }
    }
}
