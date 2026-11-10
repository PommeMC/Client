//! Source-driven catalog and Vanilla-style enablement reconciliation.
//! Application settings, filesystem validation and backend construction live
//! elsewhere.

use std::collections::HashMap;
use std::rc::Rc;

use super::{Pack, PackEnablementConfig, PackId, RepositorySource};
use crate::resources::ResourceOrigin;
use crate::resources::pack_resources::PackResources;

/// Available catalog plus low-to-high enabled registration IDs.
/// Catalog order is Rust's case-sensitive textual ID order. As in vanilla's
/// `TreeMap` keyed by `Pack.getId`, a later pack with the same textual ID
/// replaces an earlier one. Required insertion follows the sorted catalog.
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

    /// Rebuilds the catalog and reconciles the enabled list. A source that
    /// fails is logged and contributes nothing, as vanilla's sources catch
    /// their own I/O errors.
    pub(crate) fn reload(&mut self) {
        let mut available: Vec<Pack> = Vec::new();
        let mut by_name: HashMap<String, usize> = HashMap::new();
        for source in &mut self.sources {
            let packs = match source.load_packs() {
                Ok(packs) => packs,
                Err(error) => {
                    tracing::warn!(%error, "Failed to load packs from a repository source");
                    continue;
                }
            };
            for pack in packs {
                if let Some(&index) = by_name.get(&pack.location().id) {
                    available[index] = pack;
                } else {
                    by_name.insert(pack.location().id.clone(), available.len());
                    available.push(pack);
                }
            }
        }
        let mut by_id = HashMap::new();
        // COMPAT: Rust compares Unicode scalar order; Java compares UTF-16 code units.
        // Ordering can differ for supplementary characters versus some BMP characters.
        available.sort_by(|a, b| a.location().id.cmp(&b.location().id));
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

    /// Preserve requested order and duplicates, omit unknown IDs, then insert
    /// missing required packs. Compatibility and PackSource's automatic-add
    /// flag are not enablement gates; fixed positions do not reorder requests.
    pub(crate) fn set_enabled(&mut self, ids: &[PackId]) {
        self.enabled = Self::rebuild_enabled(&self.available, &self.by_id, ids);
    }

    /// Vanilla `PackSelectionModel.EntryBase.toggleSelection`: in at the pack's
    /// default edge, inside the fixed block there. Unreversed, as this list
    /// runs low to high where the model's runs high to low.
    pub(crate) fn select_pack(&mut self, id: &PackId) -> bool {
        let Some(pack) = self.get_pack(id) else {
            return false;
        };
        if self.enabled.contains(id) {
            return false;
        }
        let position = pack.enablement_config().default_position;
        let (available, by_id) = (&self.available, &self.by_id);
        position.insert(
            &mut self.enabled,
            id.clone(),
            |entry| {
                by_id
                    .get(entry)
                    .and_then(|&index| available.get(index))
                    .map_or(PackEnablementConfig::DISCOVERED, Pack::enablement_config)
            },
            false,
        );
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
    /// A pack that fails to open is logged and left out, where vanilla's
    /// suppliers open lazily and never fail here. No manager is activated.
    pub(crate) fn open_all_enabled(&self) -> Vec<(ResourceOrigin, Rc<dyn PackResources>)> {
        let mut opened = Vec::new();
        for pack in self.enabled_packs() {
            let origin = pack.origin();
            match pack.open_resources() {
                Ok(layers) => opened.extend(
                    layers
                        .into_iter()
                        .map(|resources| (origin.clone(), resources)),
                ),
                Err(error) => {
                    tracing::warn!(pack = %pack.location().id, %error, "Failed to open pack");
                }
            }
        }
        opened
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
    use super::{PackId, PackRepository, RepositorySource};

    pub(super) fn test_new(sources: Vec<Box<dyn RepositorySource>>) {
        let actual = PackRepository::new(sources);
        assert_eq!(actual.available_packs().count(), 0, "new available packs");
        assert!(actual.enabled_ids().is_empty(), "new enabled IDs");
    }

    pub(super) fn test_reload(repository: &mut PackRepository, expected_location_ids: &[&str]) {
        repository.reload();
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

    pub(super) fn test_set_enabled(repository: &mut PackRepository, ids: &[PackId]) {
        repository.set_enabled(ids);
        assert_eq!(repository.enabled_ids(), ids, "set_enabled IDs");
    }

    pub(super) fn test_select_pack(repository: &mut PackRepository, id: &PackId) {
        assert!(
            repository.select_pack(id),
            "select_pack should add the pack"
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
        let actual = repository.open_all_enabled();
        let ids: Vec<_> = actual
            .iter()
            .map(|(_, resources)| resources.location().id.as_str())
            .collect();
        assert_eq!(ids, expected_location_ids, "open_all_enabled location IDs");
    }
}

#[cfg(test)]
mod pack_repository_tests {
    use std::io;

    use super::{Pack, PackId, PackRepository, Rc, RepositorySource, pack_repository_api_tests};
    use crate::resources::pack_resources::EmbeddedResourcesSupplier;
    use crate::resources::packs::{PackEnablementConfig, PackLocationInfo, PackMetadata};

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
            PackLocationInfo::test("api-pack"),
            PackMetadata::test(),
            PackEnablementConfig::DISCOVERED,
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
        repository.reload();
        pack_repository_api_tests::test_get_pack(&repository, &id, Some("api-pack"));
    }

    #[test]
    fn test_get_pack_missing() {
        let (mut repository, _) = fixture();
        repository.reload();
        let unregistered = input_pack();
        pack_repository_api_tests::test_get_pack(&repository, unregistered.id(), None);
    }

    #[test]
    fn test_available_packs() {
        let (mut repository, _) = fixture();
        repository.reload();
        pack_repository_api_tests::test_available_packs(&repository, &["api-pack"]);
    }

    #[test]
    fn test_enabled_ids() {
        let (mut repository, id) = fixture();
        repository.reload();
        repository.set_enabled(std::slice::from_ref(&id));
        pack_repository_api_tests::test_enabled_ids(&repository, &[id]);
    }

    #[test]
    fn test_enabled_packs() {
        let (mut repository, id) = fixture();
        repository.reload();
        repository.set_enabled(&[id]);
        pack_repository_api_tests::test_enabled_packs(&repository, &["api-pack"]);
    }

    #[test]
    fn test_set_enabled() {
        let (mut repository, id) = fixture();
        repository.reload();
        pack_repository_api_tests::test_set_enabled(&mut repository, &[id]);
    }

    #[test]
    fn test_select_pack() {
        let (mut repository, id) = fixture();
        repository.reload();
        pack_repository_api_tests::test_select_pack(&mut repository, &id);
    }

    #[test]
    fn test_disable_pack() {
        let (mut repository, id) = fixture();
        repository.reload();
        repository.set_enabled(std::slice::from_ref(&id));
        pack_repository_api_tests::test_disable_pack(&mut repository, &id);
    }

    #[test]
    fn test_open_all_enabled() {
        let (mut repository, id) = fixture();
        repository.reload();
        repository.set_enabled(&[id]);
        pack_repository_api_tests::test_open_all_enabled(&repository, &["api-pack"]);
    }
}

#[cfg(test)]
mod catalog_tests {
    use std::cell::RefCell;
    use std::io;
    use std::rc::Rc;

    use super::{Pack, PackRepository, RepositorySource};
    use crate::chat_component::Component;
    use crate::resources::pack_resources::EmbeddedResourcesSupplier;
    use crate::resources::packs::{
        PackEnablementConfig, PackLocationInfo, PackMetadata, PackPosition,
    };

    struct Packs(Vec<Pack>);

    impl RepositorySource for Packs {
        fn load_packs(&mut self) -> io::Result<Vec<Pack>> {
            Ok(self.0.clone())
        }
    }

    struct Shared(Rc<RefCell<Packs>>);

    impl RepositorySource for Shared {
        fn load_packs(&mut self) -> io::Result<Vec<Pack>> {
            self.0.borrow_mut().load_packs()
        }
    }

    struct Broken;

    impl RepositorySource for Broken {
        fn load_packs(&mut self) -> io::Result<Vec<Pack>> {
            Err(io::Error::other("source is broken"))
        }
    }

    fn pack(id: &str, title: &str, required: bool, position: PackPosition, fixed: bool) -> Pack {
        Pack::new(
            PackLocationInfo {
                title: Component::text(title),
                ..PackLocationInfo::test(id)
            },
            PackMetadata::test(),
            PackEnablementConfig {
                required,
                default_position: position,
                fixed_position: fixed,
            },
            Rc::new(EmbeddedResourcesSupplier::new()),
        )
    }

    fn titles(repository: &PackRepository) -> Vec<String> {
        repository
            .enabled_packs()
            .map(|pack| pack.location().title.plain_text())
            .collect()
    }

    #[test]
    fn a_selected_pack_goes_below_a_pinned_top_pack() {
        let server = pack("server", "server", true, PackPosition::Top, true);
        let player = pack("file/player", "player", false, PackPosition::Top, false);
        let player_id = player.id().clone();
        let mut repository = PackRepository::new(vec![Box::new(Packs(vec![server, player]))]);
        repository.reload();
        assert_eq!(titles(&repository), ["server"]);
        assert!(repository.select_pack(&player_id));
        assert!(!repository.select_pack(&player_id));
        assert_eq!(titles(&repository), ["player", "server"]);
    }

    #[test]
    fn a_later_pack_with_the_same_id_replaces_the_earlier() {
        let mut repository = PackRepository::new(vec![
            Box::new(Packs(vec![pack(
                "file/a",
                "first",
                true,
                PackPosition::Top,
                false,
            )])),
            Box::new(Packs(vec![pack(
                "file/a",
                "second",
                true,
                PackPosition::Top,
                false,
            )])),
        ]);
        repository.reload();
        assert_eq!(repository.available_packs().count(), 1);
        assert_eq!(titles(&repository), ["second"]);
    }

    #[test]
    fn a_failing_source_contributes_nothing() {
        let mut repository = PackRepository::new(vec![
            Box::new(Broken),
            Box::new(Packs(vec![pack(
                "pomme",
                "pomme",
                true,
                PackPosition::Bottom,
                true,
            )])),
        ]);
        repository.reload();
        assert_eq!(titles(&repository), ["pomme"]);
    }

    fn builtin(theme: &str) -> Packs {
        Packs(vec![
            pack("vanilla", "vanilla", true, PackPosition::Bottom, false),
            pack(theme, theme, true, PackPosition::Top, false),
            pack("pomme", "pomme", true, PackPosition::Bottom, true),
            pack("file/x", "user", false, PackPosition::Top, false),
        ])
    }

    /// The built-in order, and the same order once a reload swaps the theme for
    /// a pack the enabled list hasn't seen.
    #[test]
    fn required_packs_insert_at_their_edge() {
        let source = Rc::new(RefCell::new(builtin("theme/pomme")));
        let mut repository = PackRepository::new(vec![Box::new(Shared(source.clone()))]);
        repository.reload();
        assert_eq!(titles(&repository), ["pomme", "vanilla", "theme/pomme"]);
        *source.borrow_mut() = builtin("theme/bedrock");
        repository.reload();
        assert_eq!(titles(&repository), ["pomme", "vanilla", "theme/bedrock"]);
    }
}
