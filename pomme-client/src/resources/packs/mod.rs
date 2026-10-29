use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use self::filter::ResourceFilter;
use self::format::{PackFormat, PackFormatRange};
use self::metadata::PackMetadata;
use self::providers::directory::{DirectoryLayout, DirectoryPackResourceProvider};
use self::providers::indexed::IndexedPackResourceProvider;
use self::providers::zip::ZipPackResourceProvider;
use self::providers::{SharedPackResourceProvider, embedded, overlay};
use crate::resources::{ResourceError, ResourceId, ResourceOrigin};

mod configuration;
mod download;
mod filter;
mod format;
mod id;
mod metadata;
pub(in crate::resources) mod providers;
mod theme;

pub(crate) use configuration::{ConfigurationError, InstallationLocation, ResourceConfiguration};
pub(crate) use download::{PackDownloadError, download_server_pack};
pub(crate) use id::PackId;
pub(crate) use providers::embedded::{POMME_ICON, POMME_ICONS_FONT, POMME_UI_FONT};
pub(crate) use theme::{ThemeSpecification, panorama_id};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PackCompat {
    Compatible,
    TooOld,
    TooNew,
    Unknown,
}

impl PackCompat {
    fn for_format(range: Option<PackFormatRange>, target: Option<PackFormat>) -> Self {
        let (Some(range), Some(target)) = (range, target) else {
            return Self::Unknown;
        };
        if target < range.min() {
            Self::TooNew
        } else if target > range.max() {
            Self::TooOld
        } else {
            Self::Compatible
        }
    }
}

#[derive(Clone)]
pub struct PackInfo {
    pack_id: PackId,
    pub name: String,
    pub description: String,
    pub compat: PackCompat,
    pub provenance: PackProvenance,
    pub enabled: bool,
}

impl PackInfo {
    pub fn pack_id(&self) -> &PackId {
        &self.pack_id
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum PackProvenance {
    Server,
    Local,
}

#[derive(Clone)]
pub(crate) enum ResourcePackLocation {
    Directory(PathBuf),
    Zip(PathBuf),
}

impl ResourcePackLocation {
    fn path(&self) -> &Path {
        match self {
            Self::Directory(path) | Self::Zip(path) => path,
        }
    }

    fn read_metadata(&self) -> io::Result<Option<PackMetadata>> {
        match self {
            Self::Directory(path) => PackMetadata::read(path, None),
            Self::Zip(path) => PackMetadata::read(path, Some(path)),
        }
    }
}

#[derive(Clone)]
pub(crate) struct ResourcePackSource {
    pub location: ResourcePackLocation,
    pub provenance: PackProvenance,
    pub fallback_name: String,
}

/// Storage selected by pack management, independent of identity and priority.
enum PackStorage {
    /// Pack root with base assets and metadata-selected overlays.
    Directory {
        root: PathBuf,
        overlays: Vec<String>,
    },
    Zip {
        path: PathBuf,
        overlays: Vec<String>,
    },
    Indexed {
        index_file: PathBuf,
        objects_dir: PathBuf,
    },
    NamespaceDirectory {
        root: PathBuf,
        namespace: String,
        remappings: Option<Arc<BTreeMap<ResourceId, PathBuf>>>,
    },
    Embedded,
}

/// One logical pack, with zero or more physical layers in low-to-high order.
/// Its filter affects only lower packs, never any of its own layers.
pub(super) struct ResourcePackDescriptor {
    storage: Vec<PackStorage>,
    origin: ResourceOrigin,
    filter: ResourceFilter,
}

#[derive(Clone)]
pub(super) struct ResourcePack {
    pub(super) sources: Vec<SharedPackResourceProvider>,
    pub(super) filter: Rc<ResourceFilter>,
}

impl ResourcePackDescriptor {
    pub(super) fn open(self) -> Result<ResourcePack, ResourceError> {
        let sources = self
            .storage
            .into_iter()
            .map(|storage| {
                let origin = self.origin.clone();
                let source: SharedPackResourceProvider = match storage {
                    PackStorage::Directory { root, overlays } => {
                        let mut children: Vec<SharedPackResourceProvider> =
                            vec![Rc::new(DirectoryPackResourceProvider::open(
                                DirectoryLayout::ResourcePack {
                                    root: root.clone(),
                                    overlay: None,
                                },
                                origin.clone(),
                            )?)];
                        for directory in overlays {
                            children.push(Rc::new(DirectoryPackResourceProvider::open(
                                DirectoryLayout::ResourcePack {
                                    root: root.clone(),
                                    overlay: Some(directory),
                                },
                                origin.clone(),
                            )?));
                        }
                        Rc::new(overlay::OverlayPackResourceProvider::new(origin, children))
                    }
                    PackStorage::Zip { path, overlays } => {
                        let base = Rc::new(ZipPackResourceProvider::open(path, origin.clone())?);
                        base.check_pack_root()?;
                        let mut children: Vec<SharedPackResourceProvider> = vec![base.clone()];
                        for directory in overlays {
                            children.push(Rc::new(base.overlay(&directory)?));
                        }
                        Rc::new(overlay::OverlayPackResourceProvider::new(origin, children))
                    }
                    PackStorage::Indexed {
                        index_file,
                        objects_dir,
                    } => Rc::new(IndexedPackResourceProvider::open(
                        &index_file,
                        &objects_dir,
                        origin,
                    )?),
                    PackStorage::NamespaceDirectory {
                        root,
                        namespace,
                        remappings,
                    } => Rc::new(DirectoryPackResourceProvider::open(
                        DirectoryLayout::Mapped {
                            root,
                            namespace,
                            remappings,
                        },
                        origin,
                    )?),
                    PackStorage::Embedded => {
                        Rc::new(embedded::EmbeddedPackResourceProvider::new(origin))
                    }
                };
                Ok(source)
            })
            .collect::<Result<_, ResourceError>>()?;
        Ok(ResourcePack {
            sources,
            filter: Rc::new(self.filter),
        })
    }
}
