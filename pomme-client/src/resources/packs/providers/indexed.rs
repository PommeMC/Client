use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::{fs, io};

use super::{
    PackResourceProvider, ReadStream, ResourceOrigin, ThreadSafePackResourceProvider,
    canonical_directory, enumeration_error, id_from_key, lookup_error, matches_prefix,
    open_directory, open_file, source_file, validate_query,
};
use crate::resources::resource::{ResourceContent, ResourceError, ResourceId};

/// An external asset-index source layer.
/// Fallback belongs to the manager; handles retain this fixed index mapping.
#[derive(Clone)]
pub(in crate::resources::packs) struct IndexedPackResourceProvider {
    asset_index: Arc<AssetIndex>,
    origin: ResourceOrigin,
}

#[derive(Clone)]
struct AssetIndex {
    objects_dir: PathBuf,
    hashes: HashMap<String, String>,
}

impl AssetIndex {
    fn load(index_file: &Path, objects_dir: &Path) -> io::Result<Self> {
        let content = fs::read_to_string(index_file)?;
        let parsed: serde_json::Value = serde_json::from_str(&content)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let objects = parsed
            .get("objects")
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "asset index objects must be an object",
                )
            })?;
        let hashes = objects
            .iter()
            .filter_map(|(k, v)| {
                let hash = v.get("hash")?.as_str()?;
                Some((k.clone(), hash.to_owned()))
            })
            .collect();

        Ok(Self {
            objects_dir: objects_dir.to_path_buf(),
            hashes,
        })
    }

    fn keys(&self) -> impl Iterator<Item = &str> {
        self.hashes.keys().map(String::as_str)
    }

    fn objects_dir(&self) -> &Path {
        &self.objects_dir
    }

    /// Error-aware lookup for the resource boundary. A malformed hash must not
    /// become a filesystem path.
    fn resolve_checked(&self, asset_key: &str) -> io::Result<Option<PathBuf>> {
        let Some(hash) = self.hashes.get(asset_key) else {
            return Ok(None);
        };
        if hash.len() != 40 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid asset-index hash for {asset_key}"),
            ));
        }
        let path = self.objects_dir.join(&hash[..2]).join(hash);
        match fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => Ok(Some(path)),
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("asset-index object is not a file: {}", path.display()),
            )),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
}

impl IndexedPackResourceProvider {
    pub(in crate::resources::packs) fn open(
        index_file: &Path,
        objects_dir: &Path,
        origin: ResourceOrigin,
    ) -> Result<Self, ResourceError> {
        let asset_index =
            AssetIndex::load(index_file, objects_dir).map_err(|source| ResourceError::Io {
                operation: "read asset index",
                id: None,
                origin: origin.clone(),
                source,
            })?;
        open_directory(asset_index.objects_dir(), &origin)?;
        Ok(Self {
            asset_index: Arc::new(asset_index),
            origin,
        })
    }

    fn locate(&self, id: &ResourceId) -> io::Result<Option<PathBuf>> {
        let root = canonical_directory(self.asset_index.objects_dir())?;
        let key = format!("{}/{}", id.namespace(), id.path());
        let Some(path) = self.asset_index.resolve_checked(&key)? else {
            return Ok(None);
        };
        source_file(&root, &path)
    }
}

impl PackResourceProvider for IndexedPackResourceProvider {
    fn thread_safe_source(&self) -> io::Result<Arc<dyn ThreadSafePackResourceProvider>> {
        Ok(Arc::new(self.clone()))
    }

    fn origin(&self) -> &ResourceOrigin {
        &self.origin
    }

    fn get_resource(
        self: Rc<Self>,
        id: &ResourceId,
    ) -> Result<Option<ResourceContent>, ResourceError> {
        let path = self
            .locate(id)
            .map_err(|source| lookup_error(id, &self.origin, source))?;
        Ok(path.map(|_| ResourceContent::new(id.clone(), self)))
    }

    fn open(&self, id: &ResourceId) -> io::Result<ReadStream> {
        let path = self.locate(id)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, format!("missing resource {id:?}"))
        })?;
        open_file(&path)
    }

    fn namespaces(&self) -> Result<BTreeSet<String>, ResourceError> {
        canonical_directory(self.asset_index.objects_dir())
            .map_err(|source| enumeration_error(&self.origin, "enumerate namespaces", source))?;
        Ok(self
            .asset_index
            .keys()
            .filter_map(id_from_key)
            .map(|id| id.namespace().to_owned())
            .collect())
    }

    fn list_resources(
        &self,
        namespace: &str,
        prefix: &str,
    ) -> Result<BTreeSet<ResourceId>, ResourceError> {
        validate_query(Some(namespace), prefix)?;
        let enumerate = || -> io::Result<_> {
            let root = canonical_directory(self.asset_index.objects_dir())?;
            let mut resources = BTreeSet::new();
            for key in self.asset_index.keys() {
                let Some(id) = id_from_key(key) else {
                    continue;
                };
                if id.namespace() != namespace || !matches_prefix(id.path(), prefix) {
                    continue;
                }
                if let Some(path) = self.asset_index.resolve_checked(key)?
                    && source_file(&root, &path)?.is_some()
                {
                    resources.insert(id);
                }
            }
            Ok(resources)
        };
        enumerate().map_err(|source| enumeration_error(&self.origin, "list resources", source))
    }
}
