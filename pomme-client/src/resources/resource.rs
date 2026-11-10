use std::borrow::Cow;
use std::cell::OnceCell;
use std::io::Read;
use std::rc::Rc;
use std::{fmt, io};

use super::pack_resources::{IoSupplier, METADATA_EXTENSION};

/// Owned, transferable byte stream; dropping it releases its backing state.
pub(crate) type ReadStream = Box<dyn Read + Send + 'static>;

/// The pack a resource came from, by ID, as vanilla's `Resource.sourcePackId`.
/// Metadata carries its own origin, which may belong to a different pack.
#[derive(Clone, Debug)]
pub(crate) struct ResourceOrigin {
    pub(super) name: String,
}

#[cfg(test)]
impl ResourceOrigin {
    pub(crate) fn test(name: &str) -> Self {
        Self {
            name: name.to_owned(),
        }
    }
}

/// Vanilla `Identifier`: the same namespace and path characters, and the same
/// path-then-namespace order (`Identifier.compareTo`), so listings iterate as
/// vanilla's `TreeMap`s do. Storage resolution belongs to the source adapter.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct ResourceId {
    namespace: Cow<'static, str>,
    path: Cow<'static, str>,
}

impl ResourceId {
    #[cfg(test)]
    pub(crate) fn new(value: &str) -> Result<Self, ResourceError> {
        let (namespace, path) = Self::parse_parts(value)?;
        Ok(Self::from_validated_parts(namespace, path))
    }

    /// Validates and owns separate namespace and path components.
    pub(crate) fn from_parts(namespace: &str, path: &str) -> Result<Self, ResourceError> {
        if !valid_resource_id_parts(namespace, path) {
            return Err(ResourceError::InvalidId(format!("{namespace}:{path}")));
        }
        Ok(Self::from_validated_parts(namespace, path))
    }

    fn from_validated_parts(namespace: &str, path: &str) -> Self {
        Self {
            namespace: Cow::Owned(namespace.to_owned()),
            path: Cow::Owned(path.to_owned()),
        }
    }

    /// Validates an identifier, borrowing its parts without allocation on
    /// success.
    #[cfg(test)]
    fn parse_parts(value: &str) -> Result<(&str, &str), ResourceError> {
        let (namespace, path) = resource_id_parts(value);
        if !valid_resource_id_parts(namespace, path) {
            return Err(ResourceError::InvalidId(value.to_owned()));
        }
        Ok((namespace, path))
    }

    /// Borrows a static identifier without allocation. Used by `resource_id!`
    /// to validate identifiers at compile time.
    pub(crate) const fn literal(value: &'static str) -> Self {
        let (namespace, path) = resource_id_parts(value);
        assert!(
            valid_resource_id_parts(namespace, path),
            "invalid resource identifier literal"
        );
        Self {
            namespace: Cow::Borrowed(namespace),
            path: Cow::Borrowed(path),
        }
    }

    pub(crate) fn namespace(&self) -> &str {
        &self.namespace
    }

    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    /// The path is `directory` or lies inside it. Nothing lies inside the
    /// empty directory, as `FileUtil.decomposePath` refuses it.
    pub(crate) fn is_under(&self, directory: &str) -> bool {
        self.path
            .strip_prefix(directory)
            .is_some_and(|tail| !directory.is_empty() && (tail.is_empty() || tail.starts_with('/')))
    }

    /// Appending the metadata suffix preserves the already validated ID.
    pub(super) fn sidecar(&self) -> Self {
        Self {
            namespace: self.namespace.clone(),
            path: Cow::Owned(format!("{}{METADATA_EXTENSION}", self.path)),
        }
    }

    /// The resource a `.mcmeta` sidecar describes, which is always a valid ID.
    pub(super) fn sidecar_base(&self) -> Option<Self> {
        let path = self.path.strip_suffix(METADATA_EXTENSION)?;
        Some(Self {
            namespace: self.namespace.clone(),
            path: Cow::Owned(path.to_owned()),
        })
    }

    /// Omits only the default `minecraft` namespace.
    pub(crate) fn to_short_string(&self) -> String {
        if self.namespace() == "minecraft" {
            self.path().to_owned()
        } else {
            format!("{}:{}", self.namespace(), self.path())
        }
    }
}

impl Ord for ResourceId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.path
            .cmp(&other.path)
            .then_with(|| self.namespace.cmp(&other.namespace))
    }
}

impl PartialOrd for ResourceId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Vanilla `Identifier.bySeparator`: no separator, or one at the start, means
/// the `minecraft` namespace.
const fn resource_id_parts(value: &str) -> (&str, &str) {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b':' {
            let (namespace, rest) = value.split_at(index);
            let path = rest.split_at(1).1;
            return (if index == 0 { "minecraft" } else { namespace }, path);
        }
        index += 1;
    }
    ("minecraft", value)
}

const fn valid_resource_id_parts(namespace: &str, path: &str) -> bool {
    resource_namespace_chars(namespace) && resource_path_chars(path)
}

/// Vanilla `Identifier.isValidNamespace`, which also refuses `..`.
pub(super) const fn resource_namespace_chars(namespace: &str) -> bool {
    let bytes = namespace.as_bytes();
    if bytes.len() == 2 && bytes[0] == b'.' && bytes[1] == b'.' {
        return false;
    }
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if !(byte.is_ascii_lowercase()
            || byte.is_ascii_digit()
            || matches!(byte, b'_' | b'.' | b'-'))
        {
            return false;
        }
        index += 1;
    }
    true
}

/// Vanilla `Identifier.validPathChar`: lowercase only.
pub(super) const fn resource_path_chars(path: &str) -> bool {
    let bytes = path.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if !(byte.is_ascii_lowercase()
            || byte.is_ascii_digit()
            || matches!(byte, b'_' | b'.' | b'/' | b'-'))
        {
            return false;
        }
        index += 1;
    }
    true
}

/// The full identifier always includes its namespace.
impl fmt::Display for ResourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.namespace, self.path)
    }
}

impl fmt::Debug for ResourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ResourceId")
            .field(&self.to_short_string())
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ResourceError {
    #[error("invalid resource identifier: {0}")]
    InvalidId(String),
    /// Source construction, enumeration and root pack.mcmeta have no ID.
    #[error("{operation} failed for {id:?} in {origin:?}: {source}")]
    Io {
        operation: &'static str,
        id: Option<ResourceId>,
        origin: ResourceOrigin,
        #[source]
        source: io::Error,
    },
}

impl From<ResourceError> for io::Error {
    fn from(error: ResourceError) -> Self {
        let kind = match &error {
            ResourceError::Io { source, .. } => source.kind(),
            ResourceError::InvalidId(_) => io::ErrorKind::InvalidInput,
        };
        io::Error::new(kind, error)
    }
}

/// Reopenable resource or sidecar, independent of the manager's lifetime.
/// Pins a source, not immutable bytes. Setup validation is trusted; each open
/// performs ordinary candidate I/O, without revalidating source roots or
/// policy.
/// Content handles and their streams can move across threads; pack views,
/// managers and lazy metadata lookup remain application-thread-local.
#[derive(Clone)]
pub(crate) struct ResourceContent {
    id: ResourceId,
    origin: ResourceOrigin,
    supplier: IoSupplier<ReadStream>,
}

impl ResourceContent {
    /// Retains exactly the selected supplier, without relookup or opening it.
    pub(super) fn from_supplier(
        id: ResourceId,
        origin: ResourceOrigin,
        supplier: IoSupplier<ReadStream>,
    ) -> Self {
        Self {
            id,
            origin,
            supplier,
        }
    }

    pub(crate) fn id(&self) -> &ResourceId {
        &self.id
    }

    /// Empty content under `id`, for tests that only need a handle.
    #[cfg(test)]
    pub(crate) fn test(id: ResourceId) -> Self {
        Self::from_supplier(
            id,
            ResourceOrigin::test("test"),
            std::sync::Arc::new(|| Ok(Box::new(std::io::empty()) as ReadStream)),
        )
    }

    pub(crate) fn origin(&self) -> &ResourceOrigin {
        &self.origin
    }

    /// Missing or changed backing content never triggers source fallback.
    /// Later Read errors are io::Error; the handle retains ID/origin context.
    pub(crate) fn open(&self) -> Result<ReadStream, ResourceError> {
        (self.supplier)().map_err(|source| ResourceError::Io {
            operation: "open resource",
            id: Some(self.id.clone()),
            origin: self.origin.clone(),
            source,
        })
    }

    /// Reopens this content and reads at most one byte past the limit to
    /// detect oversized input, retaining ID/origin context on read errors.
    pub(crate) fn read_bytes(&self, limit: usize) -> Result<Vec<u8>, ResourceError> {
        read_bounded(self.open()?, limit, &self.id, &self.origin)
    }

    pub(crate) fn read_all(&self) -> Result<Vec<u8>, ResourceError> {
        self.read_bytes(usize::MAX)
    }
}

fn read_bounded(
    stream: ReadStream,
    limit: usize,
    id: &ResourceId,
    origin: &ResourceOrigin,
) -> Result<Vec<u8>, ResourceError> {
    let mut bytes = Vec::new();
    let result = stream
        .take((limit as u64).saturating_add(1))
        .read_to_end(&mut bytes);
    let result = result.and_then(|_| {
        if bytes.len() > limit {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "resource exceeds byte limit",
            ))
        } else {
            Ok(())
        }
    });
    result.map_err(|source| ResourceError::Io {
        operation: "read resource",
        id: Some(id.clone()),
        origin: origin.clone(),
        source,
    })?;
    Ok(bytes)
}

/// Resolved content and lazy .mcmeta lookup, independent of the manager.
/// Each content handle has its own origin; metadata is not merged or
/// interpreted.
#[derive(Clone)]
pub(crate) struct Resource {
    content: ResourceContent,
    metadata: Rc<dyn Fn() -> Option<ResourceContent>>,
    cached_metadata: Rc<OnceCell<Option<ResourceContent>>>,
}

impl Resource {
    pub(super) fn new(
        content: ResourceContent,
        metadata: Rc<dyn Fn() -> Option<ResourceContent>>,
    ) -> Self {
        Self {
            content,
            metadata,
            cached_metadata: Rc::default(),
        }
    }

    pub(crate) fn content(&self) -> &ResourceContent {
        &self.content
    }

    /// Vanilla `Resource.metadata`: the sidecar is looked up on the first call
    /// and kept, for this resource and its clones.
    pub(crate) fn metadata(&self) -> Option<ResourceContent> {
        self.cached_metadata
            .get_or_init(|| (self.metadata)())
            .clone()
    }
}

#[cfg(test)]
mod path_validation {
    use super::{ResourceError, ResourceId};

    #[test]
    fn from_parts_representability() {
        for (namespace, path) in [
            ("test", "C:/outside/sentinel.txt"),
            ("test", "outside:sentinel.txt"),
            ("test", r"outside\sentinel.txt"),
            ("test", "textures/block/Dirt.png"),
            ("bad/namespace", "sentinel.txt"),
            ("..", "x"),
        ] {
            assert!(matches!(
                ResourceId::from_parts(namespace, path),
                Err(ResourceError::InvalidId(_))
            ));
        }
        for (namespace, path) in [("test", "../x"), ("test", "/x"), ("test", ""), (".", "x")] {
            let id = ResourceId::from_parts(namespace, path).unwrap();
            assert_eq!(id.namespace(), namespace);
            assert_eq!(id.path(), path);
        }
    }

    #[test]
    fn parses_like_identifier() {
        let id = ResourceId::new(":foo").unwrap();
        assert_eq!((id.namespace(), id.path()), ("minecraft", "foo"));
        let id = ResourceId::new("foo").unwrap();
        assert_eq!((id.namespace(), id.path()), ("minecraft", "foo"));
        let id = ResourceId::new("pomme:a:b").unwrap_err();
        assert!(matches!(id, ResourceError::InvalidId(_)));
    }

    #[test]
    fn orders_by_path_then_namespace() {
        let mut ids = [
            ResourceId::new("minecraft:b/x.png").unwrap(),
            ResourceId::new("pomme:a/y.png").unwrap(),
            ResourceId::new("aaa:b/x.png").unwrap(),
        ];
        ids.sort();
        let names: Vec<String> = ids.iter().map(ToString::to_string).collect();
        assert_eq!(names, ["pomme:a/y.png", "aaa:b/x.png", "minecraft:b/x.png"]);
    }
}
