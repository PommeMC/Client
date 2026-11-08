pub(in crate::resources) use self::filter::{ResourceFilter, parse_filter};
pub(crate) use self::format::PackFormat;
use self::format::PackFormatRange;

mod download;
mod features;
mod filter;
mod format;
mod id;
mod location;
mod metadata;
mod pack;
mod repository;
mod sources;

pub(crate) use download::{PackDownloadError, download_server_pack};
pub(crate) use id::PackId;
pub(crate) use location::PackLocationInfo;
pub(crate) use pack::{Pack, PackEnablementConfig, PackMetadata, PackPosition, ResourcesSupplier};
pub(crate) use repository::PackRepository;
pub(crate) use sources::{
    BuiltinRepositorySource, FolderRepositorySource, InstallationLocation, PhysicalPack,
    RegisteredRepositorySource, RepositorySource, SymlinkPolicy, ThemeSpecification,
};

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
        if range.min().major() == format::MAX_COMPONENT {
            Self::Unknown
        } else if target < range.min() {
            Self::TooNew
        } else if target > range.max() {
            Self::TooOld
        } else {
            Self::Compatible
        }
    }
}
