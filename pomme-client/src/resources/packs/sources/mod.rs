//! Discovery and registration sources that supply logical packs to
//! PackRepository.

use std::io;
use std::path::Path;

use crate::resources::packs::Pack;

#[allow(
    dead_code,
    reason = "Built-in source API beyond initial application activation"
)]
mod builtin;
#[allow(
    dead_code,
    reason = "Folder source API beyond initial application activation"
)]
mod folder;
#[allow(
    dead_code,
    reason = "Registration API; server asset consumers are not yet migrated"
)]
mod registered;
mod symlinks;

pub(crate) use builtin::{BuiltinRepositorySource, InstallationLocation, ThemeSpecification};
pub(crate) use folder::FolderRepositorySource;
pub(crate) use registered::{PhysicalPack, RegisteredRepositorySource};
pub(crate) use symlinks::SymlinkPolicy;

/// Produces logical snapshots, retaining source-local discovery identity across
/// calls. Sources decide whether an invalid candidate is skipped or is an
/// error; the repository never infers filesystem identity or performs
/// validation.
pub(crate) trait RepositorySource {
    fn load_packs(&mut self) -> io::Result<Vec<Pack>>;
}

fn path_error(operation: &str, path: &Path, error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!("{operation} at {}: {error}", path.display()),
    )
}
