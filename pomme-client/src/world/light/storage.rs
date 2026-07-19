//! Vanilla `LayerLightSectionStorage`: per-section light layers keyed by
//! section states (has-data bit + non-empty-neighbor count), the queued
//! packet-data install path, and the changed/affected bookkeeping the publish
//! step consumes.
//!
//! Differences from vanilla, both results-identical:
//! - No double-buffered visible map: the visible side lives in the per-column
//!   `ChunkLightData` the mesher already snapshots, so `setStoredLevel` skips
//!   vanilla's copy-on-write (its only purpose is protecting the visible
//!   buffer) and the publish step writes changed sections out instead of
//!   `swapSectionMap`.
//! - `retainData` is never called on the vanilla client, so the retained-
//!   column re-stash in `markNewInconsistencies` is omitted.

use std::collections::{HashMap, HashSet};

use super::data::DataLayer;
use super::queue::Dir;

/// A block position in the light engine's world space.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct LightPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl LightPos {
    pub fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    pub fn offset(self, dir: Dir) -> Self {
        let (dx, dy, dz) = dir.offset();
        Self::new(self.x + dx, self.y + dy, self.z + dz)
    }

    pub fn section(self) -> SectionKey {
        SectionKey::new(self.x >> 4, self.y >> 4, self.z >> 4)
    }
}

/// A 16³ light section position in section coordinates.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct SectionKey {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl SectionKey {
    pub fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    pub fn offset(self, dx: i32, dy: i32, dz: i32) -> Self {
        Self::new(self.x + dx, self.y + dy, self.z + dz)
    }

    /// This section and its 26 neighbors (vanilla
    /// `setSectionDirtyWithNeighbors` / `queueSectionData` dirtying).
    pub fn with_neighbors(self) -> impl Iterator<Item = SectionKey> {
        (-1..=1).flat_map(move |dx| {
            (-1..=1).flat_map(move |dy| (-1..=1).map(move |dz| self.offset(dx, dy, dz)))
        })
    }

    /// Vanilla `SectionPos.getZeroNode`: the column key.
    pub fn column(self) -> (i32, i32) {
        (self.x, self.z)
    }
}

/// Vanilla `SectionState`: bit 5 = has block data, bits 0-4 = count of
/// non-empty neighbor sections (0..=26). A section stores light iff its state
/// is non-zero.
mod section_state {
    const HAS_DATA_BIT: u8 = 0x20;
    const NEIGHBOR_COUNT_BITS: u8 = 0x1F;

    pub fn with_has_data(state: u8, has_data: bool) -> u8 {
        if has_data {
            state | HAS_DATA_BIT
        } else {
            state & !HAS_DATA_BIT
        }
    }

    pub fn neighbor_count(state: u8) -> u8 {
        state & NEIGHBOR_COUNT_BITS
    }

    pub fn with_neighbor_count(state: u8, count: i32) -> u8 {
        assert!(
            (0..=26).contains(&count),
            "neighbor count was not within [0; 26]"
        );
        state & !NEIGHBOR_COUNT_BITS | count as u8 & NEIGHBOR_COUNT_BITS
    }
}

/// Storage hook points vanilla implements via subclassing (`onNodeAdded` /
/// `onNodeRemoved` / `createDataLayer`); the sky engine overrides them, block
/// light uses [`NoHooks`].
pub(crate) trait StorageHooks {
    fn on_node_added(&mut self, _key: SectionKey) {}

    fn on_node_removed(&mut self, _core: &StorageCore, _key: SectionKey) {}

    /// Initial layer for a section that started storing light. Vanilla
    /// installs the queued layer object itself; the clone here is installed
    /// again by `mark_new_inconsistencies` with identical bytes.
    fn create_data_layer(&mut self, core: &StorageCore, key: SectionKey) -> DataLayer {
        core.queued_layer(key).cloned().unwrap_or_default()
    }
}

pub(crate) struct NoHooks;

impl StorageHooks for NoHooks {}

pub(crate) struct StorageCore {
    section_states: HashMap<SectionKey, u8>,
    columns_with_sources: HashSet<(i32, i32)>,
    /// The engine-side (vanilla "updating") layers; the visible side is the
    /// per-column `ChunkLightData`.
    updating: HashMap<SectionKey, DataLayer>,
    queued: HashMap<SectionKey, DataLayer>,
    to_remove: HashSet<SectionKey>,
    /// Sections whose layer bytes changed since the last publish.
    pub(crate) changed_sections: HashSet<SectionKey>,
    /// Sections whose rendered light may have changed (remesh scope).
    pub(crate) affected_sections: HashSet<SectionKey>,
    has_inconsistencies: bool,
}

impl StorageCore {
    pub fn new() -> Self {
        Self {
            section_states: HashMap::new(),
            columns_with_sources: HashSet::new(),
            updating: HashMap::new(),
            queued: HashMap::new(),
            to_remove: HashSet::new(),
            changed_sections: HashSet::new(),
            affected_sections: HashSet::new(),
            has_inconsistencies: false,
        }
    }

    pub fn storing_light_for_section(&self, key: SectionKey) -> bool {
        self.updating.contains_key(&key)
    }

    pub fn layer(&self, key: SectionKey) -> Option<&DataLayer> {
        self.updating.get(&key)
    }

    pub fn queued_layer(&self, key: SectionKey) -> Option<&DataLayer> {
        self.queued.get(&key)
    }

    /// Vanilla `getDataLayerToWrite` (sans the visible-buffer copy; see the
    /// module docs): the section's layer, marked as changed for publish.
    pub fn layer_to_write(&mut self, key: SectionKey) -> Option<&mut DataLayer> {
        if !self.updating.contains_key(&key) {
            return None;
        }
        self.changed_sections.insert(key);
        self.updating.get_mut(&key)
    }

    pub fn get_stored_level(&self, pos: LightPos) -> u8 {
        self.updating
            .get(&pos.section())
            .expect("getStoredLevel outside a stored section")
            .get(pos.x & 15, pos.y & 15, pos.z & 15)
    }

    pub fn set_stored_level(&mut self, pos: LightPos, level: u8) {
        let key = pos.section();
        self.changed_sections.insert(key);
        self.updating
            .get_mut(&key)
            .expect("setStoredLevel outside a stored section")
            .set(pos.x & 15, pos.y & 15, pos.z & 15, level);
        // Vanilla SectionPos.aroundAndAtBlockPos: every section within one
        // block of the position.
        for x in (pos.x - 1) >> 4..=(pos.x + 1) >> 4 {
            for y in (pos.y - 1) >> 4..=(pos.y + 1) >> 4 {
                for z in (pos.z - 1) >> 4..=(pos.z + 1) >> 4 {
                    self.affected_sections.insert(SectionKey::new(x, y, z));
                }
            }
        }
    }

    fn mark_section_and_neighbors_affected(&mut self, key: SectionKey) {
        self.affected_sections.extend(key.with_neighbors());
    }

    pub fn queue_section_data(&mut self, key: SectionKey, data: Option<DataLayer>) {
        match data {
            Some(data) => {
                self.queued.insert(key, data);
                self.has_inconsistencies = true;
            }
            None => {
                self.queued.remove(&key);
            }
        }
    }

    pub fn update_section_status(
        &mut self,
        key: SectionKey,
        section_empty: bool,
        hooks: &mut impl StorageHooks,
    ) {
        let state = self.section_states.get(&key).copied().unwrap_or(0);
        let new_state = section_state::with_has_data(state, !section_empty);
        if state == new_state {
            return;
        }
        self.put_section_state(key, new_state, hooks);
        let increment = if section_empty { -1 } else { 1 };
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if dx == 0 && dy == 0 && dz == 0 {
                        continue;
                    }
                    let neighbor = key.offset(dx, dy, dz);
                    let neighbor_state = self.section_states.get(&neighbor).copied().unwrap_or(0);
                    self.put_section_state(
                        neighbor,
                        section_state::with_neighbor_count(
                            neighbor_state,
                            section_state::neighbor_count(neighbor_state) as i32 + increment,
                        ),
                        hooks,
                    );
                }
            }
        }
    }

    fn put_section_state(&mut self, key: SectionKey, state: u8, hooks: &mut impl StorageHooks) {
        if state != 0 {
            if self.section_states.insert(key, state).unwrap_or(0) == 0 {
                self.initialize_section(key, hooks);
            }
        } else if self.section_states.remove(&key).unwrap_or(0) != 0 {
            self.remove_section(key);
        }
    }

    fn initialize_section(&mut self, key: SectionKey, hooks: &mut impl StorageHooks) {
        if !self.to_remove.remove(&key) {
            let layer = hooks.create_data_layer(self, key);
            self.updating.insert(key, layer);
            self.changed_sections.insert(key);
            hooks.on_node_added(key);
            self.mark_section_and_neighbors_affected(key);
            self.has_inconsistencies = true;
        }
    }

    fn remove_section(&mut self, key: SectionKey) {
        self.to_remove.insert(key);
        self.has_inconsistencies = true;
    }

    /// Vanilla `markNewInconsistencies`: process deferred removals, then
    /// install queued packet layers into sections that store light. Runs
    /// between the decrease and increase passes.
    pub fn mark_new_inconsistencies(&mut self, hooks: &mut impl StorageHooks) {
        if !self.has_inconsistencies {
            return;
        }
        self.has_inconsistencies = false;
        let removed: Vec<SectionKey> = self.to_remove.drain().collect();
        for &key in &removed {
            self.queued.remove(&key);
            self.updating.remove(&key);
        }
        for &key in &removed {
            hooks.on_node_removed(self, key);
            self.changed_sections.insert(key);
        }
        let installable: Vec<SectionKey> = self
            .queued
            .keys()
            .copied()
            .filter(|key| self.updating.contains_key(key))
            .collect();
        for key in installable {
            let data = self.queued.remove(&key).expect("key collected above");
            self.updating.insert(key, data);
            self.changed_sections.insert(key);
        }
    }

    pub fn set_light_enabled(&mut self, column: (i32, i32), enable: bool) {
        if enable {
            self.columns_with_sources.insert(column);
        } else {
            self.columns_with_sources.remove(&column);
        }
    }

    pub fn light_on_in_section(&self, key: SectionKey) -> bool {
        self.light_on_in_column(key.column())
    }

    pub fn light_on_in_column(&self, column: (i32, i32)) -> bool {
        self.columns_with_sources.contains(&column)
    }
}
