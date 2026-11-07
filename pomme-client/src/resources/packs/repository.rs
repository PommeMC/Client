//! Source-driven catalog and Vanilla-style enablement reconciliation.
//! Application settings, filesystem validation and backend construction live
//! elsewhere.

use std::collections::HashMap;
use std::io;
use std::rc::Rc;

use super::{FeatureFlagSet, Pack, PackId, RepositorySource};
use crate::resources::ResourceOrigin;
use crate::resources::pack_resources::PackResources;

/// Available catalog plus low-to-high enabled registration IDs.
/// Catalog order is Rust's case-sensitive textual ID order.
/// Repeated opaque IDs replace the snapshot at its first appearance; different
/// opaque IDs remain distinct even if their textual locations are identical.
/// Equal textual IDs retain first-emission order. Required insertion follows
/// the sorted catalog, never opaque numeric order or sorted enabled IDs.
pub(crate) struct PackRepository {
    sources: Vec<Box<dyn RepositorySource>>,
    available: Vec<Pack>,
    by_id: HashMap<PackId, usize>,
    enabled: Vec<PackId>,
}

impl PackRepository {
    /// Construction performs no I/O or discovery.
    pub(crate) fn new(sources: Vec<Box<dyn RepositorySource>>) -> Self {
        Self {
            sources,
            available: Vec::new(),
            by_id: HashMap::new(),
            enabled: Vec::new(),
        }
    }

    /// Stage the entire catalog and reconciled enabled list before replacing
    /// them. A source error preserves both repository fields, but
    /// already-called sources may have advanced their own state; this is
    /// not global rollback.
    pub(crate) fn reload(&mut self) -> io::Result<()> {
        let mut available = Vec::new();
        let mut by_id = HashMap::new();
        for source in &mut self.sources {
            for pack in source.load_packs()? {
                if let Some(&index) = by_id.get(pack.id()) {
                    available[index] = pack;
                } else {
                    by_id.insert(pack.id().clone(), available.len());
                    available.push(pack);
                }
            }
        }
        // COMPAT: Rust compares Unicode scalar order; Java compares UTF-16 code units.
        // Ordering can differ for supplementary characters versus some BMP characters.
        available.sort_by(|a, b| a.location().id.cmp(&b.location().id));
        by_id.clear();
        by_id.extend(
            available
                .iter()
                .enumerate()
                .map(|(index, pack)| (pack.id().clone(), index)),
        );
        let enabled = Self::rebuild_enabled(&available, &by_id, &self.enabled);
        self.available = available;
        self.by_id = by_id;
        self.enabled = enabled;
        Ok(())
    }

    pub(crate) fn get_pack(&self, id: &PackId) -> Option<&Pack> {
        self.by_id
            .get(id)
            .and_then(|&index| self.available.get(index))
    }

    pub(crate) fn available_packs(&self) -> impl Iterator<Item = &Pack> {
        self.available.iter()
    }

    pub(crate) fn enabled_ids(&self) -> &[PackId] {
        &self.enabled
    }

    pub(crate) fn enabled_packs(&self) -> impl Iterator<Item = &Pack> {
        self.enabled.iter().filter_map(|id| self.get_pack(id))
    }

    pub(crate) fn requested_feature_flags(&self) -> FeatureFlagSet {
        self.enabled_packs()
            .fold(FeatureFlagSet::empty(), |flags, pack| {
                flags.union(pack.requested_features())
            })
    }

    /// Preserve requested order and duplicates, omit unknown IDs, then insert
    /// missing required packs. Compatibility and PackSource's automatic-add
    /// flag are not enablement gates; fixed positions do not reorder requests.
    pub(crate) fn set_enabled(&mut self, ids: &[PackId]) {
        self.enabled = Self::rebuild_enabled(&self.available, &self.by_id, ids);
    }

    /// Append a known, disabled pack to the enabled list without changing the
    /// available catalog. An already-enabled pack is not moved.
    pub(crate) fn enable_pack(&mut self, id: &PackId) -> bool {
        if !self.by_id.contains_key(id) || self.enabled.contains(id) {
            return false;
        }
        self.enabled.push(id.clone());
        true
    }

    /// Remove only the first enabled occurrence, even if required, without
    /// changing the available catalog. Required packs are reinserted by the
    /// next set_enabled/reload, not by every mutation.
    pub(crate) fn disable_pack(&mut self, id: &PackId) -> bool {
        if let Some(index) = self.enabled.iter().position(|enabled| enabled == id) {
            self.enabled.remove(index);
            true
        } else {
            false
        }
    }

    /// Flatten enabled packs and their physical layers in low-to-high order,
    /// including repeated enabled IDs, with one logical origin per opening.
    /// An opening error returns no partial result. Previously opened candidates
    /// are dropped normally, never explicitly closed: closure could invalidate
    /// retained active views sharing backend state. No manager is activated
    /// here.
    pub(crate) fn open_all_enabled(
        &self,
    ) -> io::Result<Vec<(ResourceOrigin, Rc<dyn PackResources>)>> {
        let mut opened = Vec::new();
        for pack in self.enabled_packs() {
            let origin = pack.origin();
            for resources in pack.open_resources()? {
                opened.push((origin.clone(), resources));
            }
        }
        Ok(opened)
    }

    fn rebuild_enabled(
        available: &[Pack],
        by_id: &HashMap<PackId, usize>,
        ids: &[PackId],
    ) -> Vec<PackId> {
        let mut enabled: Vec<&Pack> = ids
            .iter()
            .filter_map(|id| by_id.get(id).and_then(|&index| available.get(index)))
            .collect();
        for pack in available {
            let config = pack.enablement_config();
            if config.required && !enabled.iter().any(|entry| entry.id() == pack.id()) {
                config.default_position.insert(
                    &mut enabled,
                    pack,
                    |entry| entry.enablement_config(),
                    false,
                );
            }
        }
        enabled.into_iter().map(|pack| pack.id().clone()).collect()
    }
}

#[cfg(test)]
mod pack_repository_api_tests {
    use super::{FeatureFlagSet, PackId, PackRepository, RepositorySource};

    pub(super) fn test_new(sources: Vec<Box<dyn RepositorySource>>) {
        let actual = PackRepository::new(sources);
        assert_eq!(actual.available_packs().count(), 0, "new available packs");
        assert!(actual.enabled_ids().is_empty(), "new enabled IDs");
    }

    pub(super) fn test_reload(repository: &mut PackRepository, expected_location_ids: &[&str]) {
        repository.reload().expect("reload should succeed");
        let ids: Vec<_> = repository
            .available_packs()
            .map(|pack| pack.location().id.as_str())
            .collect();
        assert_eq!(ids, expected_location_ids, "reload available location IDs");
    }

    pub(super) fn test_get_pack(
        repository: &PackRepository,
        id: &PackId,
        expected_location_id: Option<&str>,
    ) {
        let actual = repository
            .get_pack(id)
            .map(|pack| pack.location().id.as_str());
        assert_eq!(actual, expected_location_id, "get_pack location ID");
    }

    pub(super) fn test_available_packs(
        repository: &PackRepository,
        expected_location_ids: &[&str],
    ) {
        let ids: Vec<_> = repository
            .available_packs()
            .map(|pack| pack.location().id.as_str())
            .collect();
        assert_eq!(ids, expected_location_ids, "available_packs location IDs");
    }

    pub(super) fn test_enabled_ids(repository: &PackRepository, expected: &[PackId]) {
        assert_eq!(repository.enabled_ids(), expected, "enabled_ids");
    }

    pub(super) fn test_enabled_packs(repository: &PackRepository, expected_location_ids: &[&str]) {
        let ids: Vec<_> = repository
            .enabled_packs()
            .map(|pack| pack.location().id.as_str())
            .collect();
        assert_eq!(ids, expected_location_ids, "enabled_packs location IDs");
    }

    pub(super) fn test_requested_feature_flags(
        repository: &PackRepository,
        expected: FeatureFlagSet,
    ) {
        assert_eq!(
            repository.requested_feature_flags(),
            expected,
            "requested_feature_flags"
        );
    }

    pub(super) fn test_set_enabled(repository: &mut PackRepository, ids: &[PackId]) {
        repository.set_enabled(ids);
        assert_eq!(repository.enabled_ids(), ids, "set_enabled IDs");
    }

    pub(super) fn test_enable_pack(repository: &mut PackRepository, id: &PackId) {
        assert!(
            repository.enable_pack(id),
            "enable_pack should add the pack"
        );
    }

    pub(super) fn test_disable_pack(repository: &mut PackRepository, id: &PackId) {
        assert!(
            repository.disable_pack(id),
            "disable_pack should remove the pack"
        );
    }

    pub(super) fn test_open_all_enabled(
        repository: &PackRepository,
        expected_location_ids: &[&str],
    ) {
        let actual = repository
            .open_all_enabled()
            .expect("open_all_enabled should succeed");
        let ids: Vec<_> = actual
            .iter()
            .map(|(_, resources)| resources.location().id.as_str())
            .collect();
        assert_eq!(ids, expected_location_ids, "open_all_enabled location IDs");
    }
}

#[cfg(test)]
mod pack_repository_tests {
    use super::{
        FeatureFlagSet, Pack, PackId, PackRepository, Rc, RepositorySource, io,
        pack_repository_api_tests,
    };
    use crate::chat_component::Component;
    use crate::resources::pack_resources::EmbeddedResourcesSupplier;
    use crate::resources::packs::{
        PackCompat, PackEnablementConfig, PackLocationInfo, PackMetadata, PackPosition, PackSource,
    };

    struct Source {
        pack: Pack,
    }

    impl RepositorySource for Source {
        fn load_packs(&mut self) -> io::Result<Vec<Pack>> {
            Ok(vec![self.pack.clone()])
        }
    }

    fn input_pack() -> Pack {
        Pack::new(
            PackLocationInfo {
                id: "api-pack".to_owned(),
                title: Component::text("API pack"),
                source: PackSource::default(),
                known_pack_info: None,
            },
            PackMetadata {
                description: Component::text("API pack description"),
                compatibility: PackCompat::Compatible,
                requested_features: FeatureFlagSet::VANILLA,
                overlays: Vec::new(),
            },
            PackEnablementConfig {
                required: false,
                default_position: PackPosition::Top,
                fixed_position: false,
            },
            Rc::new(EmbeddedResourcesSupplier::new()),
        )
    }

    fn fixture() -> (PackRepository, PackId) {
        let source = Source { pack: input_pack() };
        let id = source.pack.id().clone();
        (PackRepository::new(vec![Box::new(source)]), id)
    }

    #[test]
    fn test_new() {
        pack_repository_api_tests::test_new(vec![Box::new(Source { pack: input_pack() })]);
    }

    #[test]
    fn test_reload() {
        let (mut repository, _) = fixture();
        pack_repository_api_tests::test_reload(&mut repository, &["api-pack"]);
    }

    #[test]
    fn test_get_pack() {
        let (mut repository, id) = fixture();
        repository.reload().unwrap();
        pack_repository_api_tests::test_get_pack(&repository, &id, Some("api-pack"));
    }

    #[test]
    fn test_get_pack_missing() {
        let (mut repository, _) = fixture();
        repository.reload().unwrap();
        let unregistered = input_pack();
        pack_repository_api_tests::test_get_pack(&repository, unregistered.id(), None);
    }

    #[test]
    fn test_available_packs() {
        let (mut repository, _) = fixture();
        repository.reload().unwrap();
        pack_repository_api_tests::test_available_packs(&repository, &["api-pack"]);
    }

    #[test]
    fn test_enabled_ids() {
        let (mut repository, id) = fixture();
        repository.reload().unwrap();
        repository.set_enabled(std::slice::from_ref(&id));
        pack_repository_api_tests::test_enabled_ids(&repository, &[id]);
    }

    #[test]
    fn test_enabled_packs() {
        let (mut repository, id) = fixture();
        repository.reload().unwrap();
        repository.set_enabled(&[id]);
        pack_repository_api_tests::test_enabled_packs(&repository, &["api-pack"]);
    }

    #[test]
    fn test_requested_feature_flags() {
        let (mut repository, id) = fixture();
        repository.reload().unwrap();
        repository.set_enabled(&[id]);
        pack_repository_api_tests::test_requested_feature_flags(
            &repository,
            FeatureFlagSet::VANILLA,
        );
    }

    #[test]
    fn test_set_enabled() {
        let (mut repository, id) = fixture();
        repository.reload().unwrap();
        pack_repository_api_tests::test_set_enabled(&mut repository, &[id]);
    }

    #[test]
    fn test_enable_pack() {
        let (mut repository, id) = fixture();
        repository.reload().unwrap();
        pack_repository_api_tests::test_enable_pack(&mut repository, &id);
    }

    #[test]
    fn test_disable_pack() {
        let (mut repository, id) = fixture();
        repository.reload().unwrap();
        repository.set_enabled(std::slice::from_ref(&id));
        pack_repository_api_tests::test_disable_pack(&mut repository, &id);
    }

    #[test]
    fn test_open_all_enabled() {
        let (mut repository, id) = fixture();
        repository.reload().unwrap();
        repository.set_enabled(&[id]);
        pack_repository_api_tests::test_open_all_enabled(&repository, &["api-pack"]);
    }
}
