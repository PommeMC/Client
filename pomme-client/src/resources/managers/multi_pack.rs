//! Namespace routing and filter preparation across low-to-high priority packs.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io;
use std::rc::Rc;

use super::fallback::{FallbackResourceManager, PathFilter};
use super::{ResourceManager, Selector};
use crate::resources::pack_resources::{PackResources, PackType};
use crate::resources::packs::{ResourceFilter, parse_filter};
use crate::resources::{Resource, ResourceId, ResourceOrigin};

/// Owns a prepared view, not immutable bytes. Supplied origins preserve opaque
/// registration identity across reloads; textual pack IDs are diagnostics only.
/// Dropping this manager never explicitly closes shared backends.
pub(crate) struct MultiPackResourceManager {
    namespaces: BTreeMap<String, FallbackResourceManager>,
}

impl MultiPackResourceManager {
    /// A pack whose namespaces can't be listed contributes none, and one whose
    /// filter section can't be read or parsed filters nothing, as vanilla's
    /// `getPackFilterSection` logs and returns null.
    pub(crate) fn new(
        pack_type: PackType,
        packs: Vec<(ResourceOrigin, Rc<dyn PackResources>)>,
    ) -> Self {
        let mut prepared = Vec::with_capacity(packs.len());
        let mut namespaces = BTreeSet::new();
        for (origin, pack) in &packs {
            let provided = pack.get_namespaces(pack_type).unwrap_or_else(|error| {
                tracing::warn!(pack = %origin.name, %error, "Failed to list pack namespaces");
                Default::default()
            });
            namespaces.extend(provided.iter().cloned());
            let filter = pack
                .metadata_section_json("filter")
                .and_then(|section| parse_filter(section.as_ref()))
                .unwrap_or_else(|error| {
                    tracing::warn!(pack = %origin.name, %error, "Failed to get filter section from pack");
                    ResourceFilter::default()
                });
            prepared.push((provided, Rc::new(filter)));
        }
        let mut managers = BTreeMap::new();
        for ((origin, pack), (provided, filter)) in packs.iter().zip(prepared) {
            let path_filter: PathFilter = {
                let filter = filter.clone();
                Rc::new(move |path| filter.path_blocked(path))
            };
            for namespace in &namespaces {
                let has_content = provided.contains(namespace);
                let is_filtered = filter.namespace_blocked(namespace);
                if !has_content && !is_filtered {
                    continue;
                }
                let manager = managers
                    .entry(namespace.clone())
                    .or_insert_with(|| FallbackResourceManager::new(pack_type, namespace.clone()));
                if has_content {
                    manager.push(
                        origin.clone(),
                        pack.clone(),
                        is_filtered.then(|| path_filter.clone()),
                    );
                } else {
                    manager.push_filter_only(pack.pack_id().to_owned(), path_filter.clone());
                }
            }
        }
        Self {
            namespaces: managers,
        }
    }

    pub(super) fn empty() -> Self {
        Self {
            namespaces: BTreeMap::new(),
        }
    }
}

impl ResourceManager for MultiPackResourceManager {
    fn get_resource(&self, id: &ResourceId) -> io::Result<Option<Resource>> {
        match self.namespaces.get(id.namespace()) {
            Some(manager) => manager.get_resource(id),
            None => Ok(None),
        }
    }

    fn get_namespaces(&self) -> io::Result<HashSet<String>> {
        Ok(self.namespaces.keys().cloned().collect())
    }

    fn get_resource_stack(&self, id: &ResourceId) -> io::Result<Vec<Resource>> {
        match self.namespaces.get(id.namespace()) {
            Some(manager) => manager.get_resource_stack(id),
            None => Ok(Vec::new()),
        }
    }

    fn list_resources(
        &self,
        directory: &str,
        selector: &Selector<'_>,
    ) -> io::Result<BTreeMap<ResourceId, Resource>> {
        check_directory(directory)?;
        let mut result = BTreeMap::new();
        for manager in self.namespaces.values() {
            result.extend(manager.list_resources(directory, selector)?);
        }
        Ok(result)
    }

    fn list_resource_stacks(
        &self,
        directory: &str,
        selector: &Selector<'_>,
    ) -> io::Result<BTreeMap<ResourceId, Vec<Resource>>> {
        check_directory(directory)?;
        let mut result = BTreeMap::new();
        for manager in self.namespaces.values() {
            result.extend(manager.list_resource_stacks(directory, selector)?);
        }
        Ok(result)
    }
}

fn check_directory(directory: &str) -> io::Result<()> {
    if directory.ends_with('/') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Trailing slash in resource directory {directory:?}"),
        ));
    }
    Ok(())
}

#[cfg(test)]
impl MultiPackResourceManager {
    /// Client resources from `packs`, low to high, as the repository opens
    /// them.
    pub(crate) fn test(packs: Vec<crate::resources::pack_resources::test_pack::TestPack>) -> Self {
        Self::new(
            PackType::ClientResources,
            packs
                .into_iter()
                .map(|pack| {
                    let origin = ResourceOrigin::test(&pack.location.id);
                    (origin, Rc::new(pack) as Rc<dyn PackResources>)
                })
                .collect(),
        )
    }
}

#[cfg(test)]
mod resource_manager_tests {
    use super::MultiPackResourceManager;
    use crate::resources::managers::tests;
    use crate::resources::pack_resources::PackType;

    fn fixture() -> MultiPackResourceManager {
        MultiPackResourceManager::new(PackType::ClientResources, vec![tests::embedded_pack()])
    }

    tests::embedded_pack_tests!(fixture);
}

/// Vanilla `FallbackResourceManager` / `MultiPackResourceManager` behaviour
/// across several packs: priority, filters, sidecar pairing and packs that
/// fail.
#[cfg(test)]
mod priority_tests {
    use std::collections::BTreeMap;
    use std::io::Read;

    use serde_json::{Value, json};

    use super::MultiPackResourceManager;
    use crate::resources::managers::ResourceManager;
    use crate::resources::pack_resources::test_pack::TestPack;
    use crate::resources::{Resource, ResourceContent, ResourceId};

    fn id(value: &str) -> ResourceId {
        ResourceId::new(value).unwrap()
    }

    fn text(content: &ResourceContent) -> String {
        let mut text = String::new();
        content.open().unwrap().read_to_string(&mut text).unwrap();
        text
    }

    fn read(resource: &Resource) -> String {
        text(resource.content())
    }

    fn metadata(resource: &Resource) -> Option<String> {
        resource.metadata().as_ref().map(text)
    }

    fn stack(manager: &MultiPackResourceManager, path: &str) -> Vec<String> {
        manager
            .get_resource_stack(&id(path))
            .unwrap()
            .iter()
            .map(read)
            .collect()
    }

    fn filter(path: &str) -> Value {
        json!({"filter": {"block": [{"namespace": "minecraft", "path": path}]}})
    }

    #[test]
    fn the_top_pack_wins_and_stacks_run_low_to_high() {
        let manager = MultiPackResourceManager::test(vec![
            TestPack::new("low", &[("minecraft/a.txt", "low")]),
            TestPack::new("high", &[("minecraft/a.txt", "high")]),
        ]);
        let resource = manager.get_resource(&id("a.txt")).unwrap().unwrap();
        assert_eq!(read(&resource), "high");
        assert_eq!(stack(&manager, "a.txt"), ["low", "high"]);
    }

    #[test]
    fn a_filter_hides_lower_packs_but_not_its_own_files() {
        let manager = MultiPackResourceManager::test(vec![
            TestPack::new(
                "low",
                &[("minecraft/a.txt", "low"), ("minecraft/b.txt", "low")],
            ),
            TestPack::new("high", &[("minecraft/b.txt", "high")])
                .with_metadata(filter("^[ab]\\.txt$")),
        ]);
        assert!(manager.get_resource(&id("a.txt")).unwrap().is_none());
        let resource = manager.get_resource(&id("b.txt")).unwrap().unwrap();
        assert_eq!(read(&resource), "high");
        assert_eq!(stack(&manager, "b.txt"), ["high"]);
    }

    /// `FallbackResourceManager.pushFilterOnly`: a pack with no files in a
    /// namespace still filters the packs below it there.
    #[test]
    fn a_filter_only_pack_still_filters() {
        let manager = MultiPackResourceManager::test(vec![
            TestPack::new("low", &[("minecraft/a.txt", "low")]),
            TestPack::new("high", &[("pomme/other.txt", "high")]).with_metadata(filter("a\\.txt")),
        ]);
        assert!(manager.get_resource(&id("a.txt")).unwrap().is_none());
    }

    /// `createStackMetadataFinder`: the sidecar comes from the highest pack
    /// at or above the one that supplied the file.
    #[test]
    fn metadata_comes_from_the_highest_pack_at_or_above_the_file() {
        let manager = MultiPackResourceManager::test(vec![
            TestPack::new(
                "low",
                &[
                    ("minecraft/a.txt", "low"),
                    ("minecraft/a.txt.mcmeta", "low meta"),
                ],
            ),
            TestPack::new("high", &[("minecraft/a.txt.mcmeta", "high meta")]),
        ]);
        let resource = manager.get_resource(&id("a.txt")).unwrap().unwrap();
        assert_eq!(read(&resource), "low");
        assert_eq!(metadata(&resource).as_deref(), Some("high meta"));
    }

    /// A filter that hides only the sidecar leaves the file but drops the
    /// metadata from under it, in lookups, stacks and listings alike.
    #[test]
    fn a_filtered_sidecar_drops_the_metadata_below_it() {
        let manager = MultiPackResourceManager::test(vec![
            TestPack::new(
                "low",
                &[
                    ("minecraft/d/a.txt", "low"),
                    ("minecraft/d/a.txt.mcmeta", "low meta"),
                ],
            ),
            TestPack::new("high", &[("pomme/other.txt", "high")])
                .with_metadata(filter("a\\.txt\\.mcmeta")),
        ]);
        let resource = manager.get_resource(&id("d/a.txt")).unwrap().unwrap();
        assert_eq!(read(&resource), "low");
        assert_eq!(metadata(&resource), None);
        let stack = manager.get_resource_stack(&id("d/a.txt")).unwrap();
        assert_eq!(stack.iter().map(metadata).collect::<Vec<_>>(), [None]);
        let listed = manager.list_resources("d", &|_| true).unwrap();
        assert_eq!(metadata(&listed[&id("d/a.txt")]), None);
    }

    /// `listResources`: a sidecar below the winning file isn't paired with it.
    #[test]
    fn listing_pairs_only_sidecars_at_or_above_the_file() {
        let manager = MultiPackResourceManager::test(vec![
            TestPack::new("low", &[("minecraft/d/a.txt.mcmeta", "low meta")]),
            TestPack::new("high", &[("minecraft/d/a.txt", "high")]),
        ]);
        let listed = manager.list_resources("d", &|_| true).unwrap();
        let resource = &listed[&id("d/a.txt")];
        assert_eq!(read(resource), "high");
        assert_eq!(metadata(resource), None);
    }

    /// Listings iterate in `Identifier.compareTo` order: path, then namespace.
    #[test]
    fn listing_order_is_path_then_namespace() {
        let manager = MultiPackResourceManager::test(vec![TestPack::new(
            "pack",
            &[("minecraft/d/b.txt", "1"), ("pomme/d/a.txt", "2")],
        )]);
        let listed: Vec<String> = manager
            .list_resources("d", &|_| true)
            .unwrap()
            .keys()
            .map(ToString::to_string)
            .collect();
        assert_eq!(listed, ["pomme:d/a.txt", "minecraft:d/b.txt"]);
    }

    /// Vanilla packs never throw from lookups: a pack that fails counts as
    /// not having the resource, and its filter section counts as absent.
    #[test]
    fn a_failing_pack_is_skipped() {
        let manager = MultiPackResourceManager::test(vec![
            TestPack::new("low", &[("minecraft/d/a.txt", "low")]),
            TestPack::new("broken", &[("minecraft/d/a.txt", "broken")]).broken(),
        ]);
        let resource = manager.get_resource(&id("d/a.txt")).unwrap().unwrap();
        assert_eq!(read(&resource), "low");
        let listed: BTreeMap<_, _> = manager.list_resources("d", &|_| true).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(read(&listed[&id("d/a.txt")]), "low");
    }

    /// `getPackFilterSection` logs a bad section and filters nothing.
    #[test]
    fn an_invalid_filter_filters_nothing() {
        let manager = MultiPackResourceManager::test(vec![
            TestPack::new("low", &[("minecraft/a.txt", "low")]),
            TestPack::new("high", &[("pomme/x.txt", "x")])
                .with_metadata(json!({"filter": {"block": "not a list"}})),
        ]);
        assert!(manager.get_resource(&id("a.txt")).unwrap().is_some());
    }

    /// `listResourceStacks`: each file pairs with the sidecar from its own
    /// pack.
    #[test]
    fn stacks_pair_each_file_with_its_own_packs_sidecar() {
        let manager = MultiPackResourceManager::test(vec![
            TestPack::new(
                "low",
                &[
                    ("minecraft/d/a.txt", "low"),
                    ("minecraft/d/a.txt.mcmeta", "low meta"),
                ],
            ),
            TestPack::new("high", &[("minecraft/d/a.txt", "high")]),
        ]);
        let stacks = manager.list_resource_stacks("d", &|_| true).unwrap();
        let stack = &stacks[&id("d/a.txt")];
        assert_eq!(stack.iter().map(read).collect::<Vec<_>>(), ["low", "high"]);
        assert_eq!(
            stack.iter().map(metadata).collect::<Vec<_>>(),
            [Some("low meta".to_owned()), None]
        );
    }

    #[test]
    fn a_filter_drops_lower_files_from_listings() {
        let manager = MultiPackResourceManager::test(vec![
            TestPack::new(
                "low",
                &[("minecraft/d/a.txt", "low"), ("minecraft/d/b.txt", "low")],
            ),
            TestPack::new("high", &[("minecraft/d/b.txt", "high")])
                .with_metadata(filter("^d/[ab]\\.txt$")),
        ]);
        let listed = manager.list_resources("d", &|_| true).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(read(&listed[&id("d/b.txt")]), "high");
        let stacks = manager.list_resource_stacks("d", &|_| true).unwrap();
        assert_eq!(stacks.len(), 1);
        assert_eq!(
            stacks[&id("d/b.txt")].iter().map(read).collect::<Vec<_>>(),
            ["high"]
        );
    }
}
