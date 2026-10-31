//! One native directory mounted as a client namespace with sparse aliases.

use std::collections::{BTreeMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::{fs, io};

use serde_json::Value;

use super::path::{
    file_supplier, list_namespace, missing_directory, validate_namespace, validate_path,
};
use super::{
    IoSupplier, METADATA_EXTENSION, PackMetadataResources, PackResources, PackType,
    PathPackResources, ResourceOutput,
};
use crate::resources::packs::{PackLocationInfo, PackMetadata, ResourcesSupplier};
use crate::resources::resource::{ReadStream, ResourceId};

pub(crate) struct MappedPackResources {
    root: PathPackResources,
    mapping: Rc<PreparedMapping>,
}

/// Immutable lexical configuration only; no opened views or metadata caches.
pub(crate) struct MappedResourcesSupplier {
    root: PathBuf,
    mapping: Rc<PreparedMapping>,
}

impl MappedResourcesSupplier {
    /// Resolves aliases and their companions once, without filesystem I/O.
    pub(crate) fn new(
        root: PathBuf,
        namespace: String,
        remappings: BTreeMap<ResourceId, PathBuf>,
    ) -> io::Result<Self> {
        Ok(Self {
            root,
            mapping: Rc::new(PreparedMapping::new(namespace, remappings)?),
        })
    }

    fn open(&self, location: &PackLocationInfo) -> MappedPackResources {
        MappedPackResources::from_prepared(
            location.clone(),
            self.root.clone(),
            self.mapping.clone(),
        )
    }
}

impl ResourcesSupplier for MappedResourcesSupplier {
    fn open_metadata(
        &self,
        location: &PackLocationInfo,
    ) -> io::Result<Rc<dyn PackMetadataResources>> {
        Ok(Rc::new(self.open(location)))
    }

    fn open_resources(
        &self,
        location: &PackLocationInfo,
        _metadata: &PackMetadata,
    ) -> io::Result<Vec<Rc<dyn PackResources>>> {
        Ok(vec![Rc::new(self.open(location))])
    }
}

struct PreparedMapping {
    namespace: String,
    /// Includes one derived companion per explicit mapping. Exact mappings
    /// always win, even when their target is absent. Translation is
    /// single-step.
    remappings: BTreeMap<ResourceId, PathBuf>,
}

impl MappedPackResources {
    /// Validates lexical configuration once, without filesystem I/O. As for
    /// PathPackResources, setup owns root availability and symlink policy.
    /// The namespace is a declared mount, not a live existence assertion;
    /// an unavailable root can still be constructed and lists no files.
    pub(crate) fn new(
        location: PackLocationInfo,
        root: PathBuf,
        namespace: String,
        remappings: BTreeMap<ResourceId, PathBuf>,
    ) -> io::Result<Self> {
        Ok(Self::from_prepared(
            location,
            root,
            Rc::new(PreparedMapping::new(namespace, remappings)?),
        ))
    }

    fn from_prepared(
        location: PackLocationInfo,
        root: PathBuf,
        mapping: Rc<PreparedMapping>,
    ) -> Self {
        Self {
            root: PathPackResources::new(location, root),
            mapping,
        }
    }
}

impl PreparedMapping {
    fn new(namespace: String, remappings: BTreeMap<ResourceId, PathBuf>) -> io::Result<Self> {
        validate_namespace(&namespace)?;
        for (id, path) in &remappings {
            if id.namespace() != namespace
                || validate_path(id.path()).is_err()
                || !valid_relative_path(path)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("invalid directory remapping {id} -> {}", path.display()),
                ));
            }
        }
        let mut resolved = remappings.clone();
        for (id, path) in remappings {
            let companion =
                ResourceId::new(&format!("{id}{METADATA_EXTENSION}")).map_err(io::Error::other)?;
            let mut companion_path = path.into_os_string();
            companion_path.push(METADATA_EXTENSION);
            resolved.entry(companion).or_insert(companion_path.into());
        }
        Ok(Self {
            namespace,
            remappings: resolved,
        })
    }
}

impl PackMetadataResources for MappedPackResources {
    fn location(&self) -> &PackLocationInfo {
        self.root.location()
    }

    fn get_root_resource(&self, path: &[&str]) -> io::Result<Option<IoSupplier<ReadStream>>> {
        self.root.get_root_resource(path)
    }

    fn metadata_section_json(&self, name: &str) -> io::Result<Option<Value>> {
        self.root.metadata_section_json(name)
    }

    /// No retained open handles. Suppliers and metadata survive repeated close.
    fn close(&self) -> io::Result<()> {
        self.root.close()
    }
}

impl PackResources for MappedPackResources {
    fn get_resource(
        &self,
        pack_type: PackType,
        id: &ResourceId,
    ) -> io::Result<Option<IoSupplier<ReadStream>>> {
        validate_namespace(id.namespace())?;
        validate_path(id.path())?;
        if pack_type != PackType::ClientResources || id.namespace() != self.mapping.namespace {
            return Ok(None);
        }
        let relative = self
            .mapping
            .remappings
            .get(id)
            .map_or(Path::new(id.path()), PathBuf::as_path);
        let path = self.root.root().join(relative);
        // Mapped resources require a regular file.
        // Only the selected candidate is inspected, never source ancestors.
        match fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => Ok(Some(file_supplier(path))),
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("resource is not a file: {}", path.display()),
            )),
            Err(error) if missing_directory(&error) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn list_resources(
        &self,
        pack_type: PackType,
        namespace: &str,
        directory: &str,
        output: &mut ResourceOutput<'_>,
    ) -> io::Result<()> {
        validate_namespace(namespace)?;
        if !directory.is_empty() {
            validate_path(directory)?;
        }
        if pack_type != PackType::ClientResources || namespace != self.mapping.namespace {
            return Ok(());
        }
        list_namespace(
            self.root.root(),
            namespace,
            directory,
            &mut |id, supplier| {
                // Only files reach this callback: a remapped directory name must
                // not prune its ordinary descendants. Derived sidecars shadow too.
                if !self.mapping.remappings.contains_key(&id) {
                    output(id, supplier);
                }
            },
        )?;
        for (id, relative) in &self.mapping.remappings {
            if !(directory.is_empty()
                || id.path() == directory
                || id
                    .path()
                    .strip_prefix(directory)
                    .is_some_and(|tail| tail.starts_with('/')))
            {
                continue;
            }
            let path = self.root.root().join(relative);
            // Explicitly configured aliases use ordinary candidate metadata,
            // including trusted links, independently of the physical prefix.
            match fs::metadata(&path) {
                Ok(metadata) if metadata.is_file() => output(id.clone(), file_supplier(path)),
                Ok(_) => {}
                Err(error) if missing_directory(&error) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn get_namespaces(&self, pack_type: PackType) -> io::Result<HashSet<String>> {
        Ok(if pack_type == PackType::ClientResources {
            HashSet::from([self.mapping.namespace.clone()])
        } else {
            HashSet::new()
        })
    }
}

// Native targets need not use ResourceId's alphabet (spaces/Unicode are valid),
// but must satisfy the setup-only, relative normal-component policy.
fn valid_relative_path(path: &Path) -> bool {
    path.to_str().is_some_and(|path| {
        !path.contains(['\\', ':'])
            && path
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != "..")
    }) && path
        .components()
        .all(|part| matches!(part, Component::Normal(_)))
}
