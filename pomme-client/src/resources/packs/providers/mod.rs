use std::collections::BTreeSet;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use crate::resources::resource::{resource_namespace_chars, resource_path_chars};
use crate::resources::{ReadStream, ResourceContent, ResourceError, ResourceId, ResourceOrigin};

pub(super) mod directory;
pub(super) mod embedded;
pub(super) mod indexed;
pub(super) mod overlay;
pub(super) mod zip;

pub(in crate::resources) type SharedPackResourceProvider = Rc<dyn PackResourceProvider>;

/// Exact-source opening only; streams remain local to the opening thread.
pub(in crate::resources) trait ThreadSafePackResourceProvider:
    Send + Sync
{
    fn open(&self, id: &ResourceId) -> io::Result<ReadStream>;
}

impl<T: PackResourceProvider + Send + Sync> ThreadSafePackResourceProvider for T {
    fn open(&self, id: &ResourceId) -> io::Result<ReadStream> {
        PackResourceProvider::open(self, id)
    }
}

/// Resource bytes from one source layer, with syntax-checked logical IDs.
/// Filesystem adapters lexically validate storage paths at lookup and open.
/// Source roots and local-pack symlink policy are validated during setup, then
/// trusted: access never revalidates source usability or policy. Later on-disk
/// changes are outside that guarantee; providers do not pin immutable bytes.
/// Enumeration classifies entries, not their ancestors: tree walks do not
/// follow encountered links, while standalone mapped/indexed files use ordinary
/// candidate metadata. Setup does not guarantee link-free descendants.
/// Current adapters access the assets tree; other resource domains are not
/// wired. Ordinary candidate I/O errors must not become absence or empty
/// results. Providers and resource acquisition stay on the application thread.
pub(in crate::resources) trait PackResourceProvider {
    fn origin(&self) -> &ResourceOrigin;

    /// Exact lookup: candidate NotFound is None, other I/O failures are Err.
    /// A root removed after setup may therefore permit lower-source fallback.
    /// Handles may retain this source. Sidecars are ordinary IDs; the
    /// manager selects metadata.
    fn get_resource(
        self: Rc<Self>,
        id: &ResourceId,
    ) -> Result<Option<ResourceContent>, ResourceError>;

    /// Exact-source open for a located ID. NotFound after lookup is an error.
    /// The returned stream must not borrow this source or a temporary archive.
    fn open(&self, id: &ResourceId) -> io::Result<ReadStream>;

    /// Cheap source retention only: no lookup, filesystem checks or entry open.
    /// Composite sources must return child content rather than reselect here.
    fn thread_safe_source(&self) -> io::Result<Arc<dyn ThreadSafePackResourceProvider>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "source does not support thread-safe resource content",
        ))
    }

    fn namespaces(&self) -> Result<BTreeSet<String>, ResourceError>;

    /// Recursive logical directory prefix, not a physical path or glob. Empty
    /// means all paths in the namespace; otherwise matching descendants under
    /// `prefix/` only. Namespace and prefix use resource ID characters, not
    /// filesystem component rules. Invalid syntax is Err; filesystem adapters
    /// additionally reject unsafe components. Nonempty prefixes cannot end in
    /// `/`. Returns unique checked IDs, including .mcmeta files; callers
    /// may filter.
    fn list_resources(
        &self,
        namespace: &str,
        prefix: &str,
    ) -> Result<BTreeSet<ResourceId>, ResourceError>;
}

/// Setup-only accessibility check, not future access or descendant contents.
fn check_directory(root: &Path) -> io::Result<()> {
    std::fs::read_dir(root).map(|_| ())
}

fn overlay_directory(directory: &str) -> io::Result<&str> {
    if directory.is_empty()
        || directory == ".."
        || !directory
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "overlay directory escapes pack root or is invalid",
        ));
    }
    Ok(if directory == "." { "" } else { directory })
}

/// Validate and anchor the source once during provider setup.
fn canonical_directory(root: &Path) -> io::Result<PathBuf> {
    check_directory(root)?;
    root.canonicalize()
}

/// Logical IDs intentionally have a broader character contract. Native paths
/// must not normalize empty/dot/parent segments or replace the anchored root.
fn validate_storage_path(path: &str) -> io::Result<()> {
    if path
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
        || path.contains(['\\', ':'])
        || Path::new(path)
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid filesystem resource path",
        ));
    }
    Ok(())
}

fn is_file(path: &Path) -> io::Result<bool> {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("resource is not a file: {}", path.display()),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn open_file(path: &Path) -> io::Result<ReadStream> {
    let file = File::open(path)?;
    // Inspect only the opened candidate, not its path or source ancestors.
    // File::open alone can accept a directory on some platforms.
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("resource is not a file: {}", path.display()),
        ));
    }
    Ok(Box::new(file))
}

fn lookup_error(id: &ResourceId, origin: &ResourceOrigin, source: io::Error) -> ResourceError {
    ResourceError::Io {
        operation: "locate resource",
        id: Some(id.clone()),
        origin: origin.clone(),
        source,
    }
}

fn enumeration_error(
    origin: &ResourceOrigin,
    operation: &'static str,
    source: io::Error,
) -> ResourceError {
    ResourceError::Io {
        operation,
        id: None,
        origin: origin.clone(),
        source,
    }
}

fn valid_namespace(namespace: &str) -> bool {
    !namespace.is_empty() && resource_namespace_chars(namespace)
}

pub(in crate::resources) fn validate_query(
    namespace: Option<&str>,
    prefix: &str,
) -> Result<(), ResourceError> {
    if namespace.is_some_and(|namespace| !valid_namespace(namespace))
        || !resource_path_chars(prefix)
        || prefix.ends_with('/')
    {
        return Err(ResourceError::InvalidQuery {
            namespace: namespace.unwrap_or_default().to_owned(),
            prefix: prefix.to_owned(),
        });
    }
    Ok(())
}

fn matches_prefix(path: &str, prefix: &str) -> bool {
    prefix.is_empty()
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Index and archive keys are external input; malformed logical names are
/// ignored.
fn id_from_key(key: &str) -> Option<ResourceId> {
    let (namespace, path) = key.split_once('/')?;
    ResourceId::new(&format!("{namespace}:{path}")).ok()
}
