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

#[cfg(all(test, unix))]
mod path_validation {
    use std::io::ErrorKind;

    use super::{RepositorySource, tests};

    // Fixtures supply otherwise-valid packs under trusted discovery roots,
    // default-deny policy, and no filesystem mutation after validation.
    pub(super) fn assert_load_packs_catalog<S: RepositorySource + ?Sized>(
        source: &mut S,
        expected_location_ids: &[&str],
    ) {
        assert!(
            !expected_location_ids.is_empty(),
            "load_packs needs a nonempty successful catalog control"
        );
        tests::test_load_packs(source, expected_location_ids);
    }

    pub(super) fn assert_load_packs_permission_denied(
        source: &mut dyn RepositorySource,
        clean_source: &mut dyn RepositorySource,
        clean_location_ids: &[&str],
    ) {
        assert_load_packs_catalog(clean_source, clean_location_ids);
        match source.load_packs() {
            Err(error) => assert_eq!(
                error.kind(),
                ErrorKind::PermissionDenied,
                "load_packs must reject the forbidden registered pack"
            ),
            Ok(_) => panic!(
                "load_packs must fail rather than admit or skip the forbidden registered pack"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        dead_code,
        reason = "Generic API assertions pending user review before fixtures"
    )]

    use super::RepositorySource;

    pub(super) fn test_load_packs_empty<S: RepositorySource + ?Sized>(source: &mut S) {
        let actual = source.load_packs().expect("load_packs should succeed");
        assert!(actual.is_empty(), "load_packs should return no packs");
    }

    pub(super) fn test_load_packs<S: RepositorySource + ?Sized>(
        source: &mut S,
        expected_location_ids: &[&str],
    ) {
        let actual = source.load_packs().expect("load_packs should succeed");
        let mut ids: Vec<_> = actual
            .iter()
            .map(|pack| pack.location().id.as_str())
            .collect();
        ids.sort_unstable();
        let mut expected = expected_location_ids.to_vec();
        expected.sort_unstable();
        assert_eq!(
            ids, expected,
            "load_packs location IDs including multiplicity"
        );
    }
}
