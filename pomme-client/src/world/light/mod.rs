//! Client-side light engine: a port of vanilla's `LevelLightEngine` stack
//! (26.2 `net/minecraft/world/level/lighting/`). Server light packets are
//! queued as tasks and applied per tick, block changes trigger local
//! relights, and changed sections publish into the per-column
//! [`ChunkLightData`] the mesher already snapshots.
//!
//! Tick flow (vanilla `ClientLevel.update`): [`LevelLightEngine::poll_and_run`]
//! drains queued light tasks at vanilla's rate limit, then runs each engine's
//! update passes, publishing between the decrease and increase passes exactly
//! where vanilla's `swapSectionMap` sits.

// TODO(light-wire): drop once the integration PR calls the facade from the
// game loop; until then only the tests reach this module.
#![allow(dead_code)]

mod block;
mod data;
mod queue;
mod storage;
mod world;

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use azalea_block::BlockState;
use block::BlockLightEngine;
use data::{DataLayer, LAYER_BYTES};
use storage::StorageCore;
pub(crate) use storage::{LightPos, SectionKey};
use world::StoreWorld;

use crate::world::block as block_data;
use crate::world::chunk::{ChunkLightData, ChunkStore};

/// Vanilla `LightEngine.getOpacity`: light always attenuates by at least 1.
pub(crate) fn opacity(state: BlockState) -> u8 {
    block_data::light_props(state).dampening.max(1)
}

/// Vanilla `LightEngine.hasDifferentLightProperties`: whether a block change
/// requires a relight.
pub(crate) fn has_different_light_properties(old: BlockState, new: BlockState) -> bool {
    if old == new {
        return false;
    }
    let (o, n) = (block_data::light_props(old), block_data::light_props(new));
    n.dampening != o.dampening
        || n.emission != o.emission
        || n.use_shape_for_light_occlusion
        || o.use_shape_for_light_occlusion
}

/// One light section's payload from a chunk-load or light-update packet,
/// mirroring vanilla `readSectionList`: present bytes, explicitly empty, or
/// untouched.
pub(crate) enum SectionEntry {
    Data(Box<[u8; LAYER_BYTES]>),
    Empty,
    Skip,
}

impl SectionEntry {
    /// The layer to queue, or `None` for untouched sections.
    fn to_layer(&self) -> Option<DataLayer> {
        match self {
            SectionEntry::Data(bytes) => Some(DataLayer::from_bytes(bytes.clone())),
            SectionEntry::Empty => Some(DataLayer::new()),
            SectionEntry::Skip => None,
        }
    }
}

/// A queued light task (vanilla `ClientLevel.lightUpdateQueue` runnables).
pub(crate) enum LightTask {
    /// Server light for a column: chunk-load light (`enable: true`, vanilla
    /// `handleLevelChunkWithLight` + `enableChunkLight`) or a standalone
    /// light-update packet (`enable: false`).
    ApplyLight {
        pos: (i32, i32),
        sky: Vec<SectionEntry>,
        block: Vec<SectionEntry>,
        enable: bool,
    },
    /// Chunk unload (vanilla `queueLightRemoval`).
    Remove { pos: (i32, i32) },
}

/// Remesh requests produced by a light run.
#[derive(Default)]
pub(crate) struct LightDirty {
    /// World-space light sections whose rendered light may have changed.
    pub sections: HashSet<SectionKey>,
    /// Columns whose chunk-load light applied: remesh column + neighbors.
    pub columns: Vec<(i32, i32)>,
}

#[derive(Clone, Copy)]
enum LayerKind {
    Block,
    // TODO(light-sky): Sky, once the sky engine lands.
}

pub(crate) struct LevelLightEngine {
    block: BlockLightEngine,
    // TODO(light-sky): sky: Option<SkyLightEngine>.
    tasks: VecDeque<LightTask>,
    /// Lowest block section y (`min_y >> 4`); light sections span one extra
    /// section below and above.
    min_section_y: i32,
    section_count: i32,
}

impl LevelLightEngine {
    pub fn new(height: u32, min_y: i32, _has_sky: bool) -> Self {
        Self {
            block: BlockLightEngine::new(),
            tasks: VecDeque::new(),
            min_section_y: min_y >> 4,
            section_count: (height / 16) as i32,
        }
    }

    pub fn light_section_count(&self) -> usize {
        (self.section_count + 2) as usize
    }

    fn light_section_key(&self, pos: (i32, i32), index: usize) -> SectionKey {
        SectionKey::new(pos.0, self.min_section_y - 1 + index as i32, pos.1)
    }

    pub fn queue_task(&mut self, task: LightTask) {
        self.tasks.push_back(task);
    }

    /// Vanilla `LevelChunk.setBlockState`'s light lines: section empty-status
    /// first, then a relight check when the light properties changed. Call
    /// with the block already written to the chunk.
    pub fn on_block_changed(
        &mut self,
        pos: LightPos,
        old: BlockState,
        new: BlockState,
        empty_flip: Option<bool>,
    ) {
        if let Some(empty) = empty_flip {
            self.block.update_section_status(pos.section(), empty);
        }
        if has_different_light_properties(old, new) {
            // TODO(light-sky): ChunkSkyLightSources.update before checkBlock.
            self.block.check_block(pos);
        }
    }

    /// Registers a freshly loaded chunk column: creates its light column from
    /// any layers the engine already stores (border sections lit by loaded
    /// neighbors), so later publishes only need to touch changed sections.
    pub fn on_chunk_loaded(&mut self, store: &mut ChunkStore, pos: (i32, i32)) {
        let count = self.light_section_count();
        let min_section_y = self.min_section_y;
        let key_at = |index: usize| SectionKey::new(pos.0, min_section_y - 1 + index as i32, pos.1);
        let sky_sections = vec![None; count];
        let mut block_sections = vec![None; count];
        for (index, slot) in block_sections.iter_mut().enumerate() {
            *slot = self
                .block
                .storage
                .layer(key_at(index))
                .map(DataLayer::to_bytes);
        }
        // TODO(light-sky): fill sky sections + build ChunkSkyLightSources.
        store.light_data.insert(
            pos,
            Arc::new(ChunkLightData {
                sky_sections,
                block_sections,
                min_y: store.min_y(),
            }),
        );
    }

    /// Drains queued light tasks at vanilla's per-tick rate limit, then runs
    /// the light updates. Call once per game tick.
    pub fn poll_and_run(&mut self, store: &mut ChunkStore, out: &mut LightDirty) {
        // Vanilla ClientLevel.pollLightUpdates: size < 1000 ? max(10, size/10)
        // : size tasks per tick.
        let size = self.tasks.len();
        let quota = if size < 1000 {
            (size / 10).max(10)
        } else {
            size
        };
        for _ in 0..quota {
            let Some(task) = self.tasks.pop_front() else {
                break;
            };
            self.apply_task(task, store, out);
        }
        self.run_light_updates(store, out);
    }

    fn apply_task(&mut self, task: LightTask, store: &mut ChunkStore, out: &mut LightDirty) {
        match task {
            LightTask::ApplyLight {
                pos,
                sky,
                block,
                enable,
            } => {
                // TODO(light-sky): queue sky entries into the sky engine.
                let _ = sky;
                for (index, entry) in block.iter().enumerate() {
                    let key = self.light_section_key(pos, index);
                    if let Some(layer) = entry.to_layer() {
                        self.block.queue_section_data(key, Some(layer));
                    }
                    if !enable && !matches!(entry, SectionEntry::Skip) {
                        // Vanilla `setSectionDirtyWithNeighbors`.
                        out.sections.extend(key.with_neighbors());
                    }
                }
                // Vanilla applyLightData tail: sources on for both task kinds.
                self.block.set_light_enabled(pos, true);
                if enable {
                    // Vanilla enableChunkLight: report each block section's
                    // emptiness, then schedule the column's meshes.
                    for section_y in self.min_section_y..self.min_section_y + self.section_count {
                        let key = SectionKey::new(pos.0, section_y, pos.1);
                        let empty = store.section_is_empty(pos, section_y);
                        self.block.update_section_status(key, empty);
                    }
                    out.columns.push(pos);
                }
            }
            LightTask::Remove { pos } => {
                // Vanilla queueLightRemoval order: sources off, null out every
                // light section, then mark every block section empty.
                self.block.set_light_enabled(pos, false);
                for index in 0..self.light_section_count() {
                    let key = self.light_section_key(pos, index);
                    self.block.queue_section_data(key, None);
                }
                for section_y in self.min_section_y..self.min_section_y + self.section_count {
                    self.block
                        .update_section_status(SectionKey::new(pos.0, section_y, pos.1), true);
                }
            }
        }
    }

    /// Vanilla `runLightUpdates` per engine, with the publish step sitting
    /// where `swapSectionMap` does: after the decrease pass, before the
    /// increase pass.
    fn run_light_updates(&mut self, store: &mut ChunkStore, out: &mut LightDirty) {
        let (min_section_y, section_count) = (self.min_section_y, self.section_count);
        {
            let world = StoreWorld::new(store);
            self.block.begin_updates(&world);
        }
        Self::publish(
            &mut self.block.storage,
            store,
            LayerKind::Block,
            min_section_y,
            section_count,
            out,
        );
        {
            let world = StoreWorld::new(store);
            self.block.finish_updates(&world);
        }
    }

    /// Writes changed sections into their columns' [`ChunkLightData`]
    /// (vanilla `swapSectionMap`) and drains the remesh scope (vanilla
    /// `onLightUpdate` fan-out).
    fn publish(
        storage: &mut StorageCore,
        store: &mut ChunkStore,
        layer: LayerKind,
        min_section_y: i32,
        section_count: i32,
        out: &mut LightDirty,
    ) {
        let count = (section_count + 2) as usize;
        let changed: Vec<SectionKey> = storage.changed_sections.drain().collect();
        for key in changed {
            // Columns without chunk data have no ChunkLightData; their layers
            // republish via on_chunk_loaded when the chunk arrives.
            let Some(column) = store.light_data.get_mut(&key.column()) else {
                continue;
            };
            let index = key.y - (min_section_y - 1);
            debug_assert!(
                (0..count as i32).contains(&index),
                "light section {index} outside 0..{count}"
            );
            if !(0..count as i32).contains(&index) {
                continue;
            }
            let column = Arc::make_mut(column);
            let slot = match layer {
                LayerKind::Block => &mut column.block_sections[index as usize],
            };
            *slot = storage.layer(key).map(DataLayer::to_bytes);
        }
        out.sections.extend(storage.affected_sections.drain());
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::world::LightBlockGetter;
    use super::*;
    use crate::world::block::find_state;

    struct TestWorld {
        blocks: HashMap<LightPos, BlockState>,
    }

    impl TestWorld {
        fn new() -> Self {
            crate::world::block::init("26.2");
            Self {
                blocks: HashMap::new(),
            }
        }

        fn set(&mut self, x: i32, y: i32, z: i32, state: BlockState) {
            self.blocks.insert(LightPos::new(x, y, z), state);
        }

        fn clear(&mut self, x: i32, y: i32, z: i32) {
            self.blocks.remove(&LightPos::new(x, y, z));
        }
    }

    impl LightBlockGetter for TestWorld {
        fn state(&self, pos: LightPos) -> BlockState {
            self.blocks.get(&pos).copied().unwrap_or(BlockState::AIR)
        }
    }

    /// Engine storing light for section (0,0,0) and its 26 neighbors, with
    /// sources enabled in the center column.
    fn block_engine() -> BlockLightEngine {
        let mut engine = BlockLightEngine::new();
        engine.update_section_status(SectionKey::new(0, 0, 0), false);
        engine.set_light_enabled((0, 0), true);
        engine
    }

    fn run(engine: &mut BlockLightEngine, world: &TestWorld) {
        engine.begin_updates(world);
        engine.finish_updates(world);
    }

    fn level(engine: &BlockLightEngine, x: i32, y: i32, z: i32) -> u8 {
        engine.storage.get_stored_level(LightPos::new(x, y, z))
    }

    #[test]
    fn torch_diamond() {
        let mut world = TestWorld::new();
        let mut engine = block_engine();
        world.set(8, 8, 8, find_state("torch", &[]));
        engine.check_block(LightPos::new(8, 8, 8));
        run(&mut engine, &world);

        // Level = 14 - manhattan distance in open air.
        assert_eq!(level(&engine, 8, 8, 8), 14);
        assert_eq!(level(&engine, 9, 8, 8), 13);
        assert_eq!(level(&engine, 8, 8, 4), 10);
        assert_eq!(level(&engine, 12, 10, 9), 7);
        // The diamond's edge crosses into the neighbor section.
        assert_eq!(level(&engine, 21, 8, 8), 1);
        assert_eq!(level(&engine, 22, 8, 8), 0);
    }

    #[test]
    fn torch_against_wall() {
        let mut world = TestWorld::new();
        let mut engine = block_engine();
        world.set(8, 8, 8, find_state("torch", &[]));
        world.set(9, 8, 8, find_state("stone", &[]));
        engine.check_block(LightPos::new(8, 8, 8));
        engine.check_block(LightPos::new(9, 8, 8));
        run(&mut engine, &world);

        // Opaque cells stay dark; light reaches behind them the long way.
        assert_eq!(level(&engine, 9, 8, 8), 0);
        assert_eq!(level(&engine, 10, 8, 8), 10);
    }

    #[test]
    fn torch_removal_clears_light() {
        let mut world = TestWorld::new();
        let mut engine = block_engine();
        world.set(8, 8, 8, find_state("torch", &[]));
        engine.check_block(LightPos::new(8, 8, 8));
        run(&mut engine, &world);
        assert_eq!(level(&engine, 12, 8, 8), 10);

        world.clear(8, 8, 8);
        engine.check_block(LightPos::new(8, 8, 8));
        run(&mut engine, &world);
        for (x, y, z) in [(8, 8, 8), (9, 8, 8), (12, 8, 8), (8, 12, 8)] {
            assert_eq!(level(&engine, x, y, z), 0, "at {x} {y} {z}");
        }
    }

    #[test]
    fn overlapping_sources_keep_brighter() {
        let mut world = TestWorld::new();
        let mut engine = block_engine();
        let torch = find_state("torch", &[]);
        world.set(4, 8, 8, torch);
        world.set(12, 8, 8, torch);
        engine.check_block(LightPos::new(4, 8, 8));
        engine.check_block(LightPos::new(12, 8, 8));
        run(&mut engine, &world);
        assert_eq!(level(&engine, 8, 8, 8), 10);
        assert_eq!(level(&engine, 6, 8, 8), 12);

        // Removing one source re-floods from the survivor (the decrease
        // pass's brighter-neighbor re-injection).
        world.clear(4, 8, 8);
        engine.check_block(LightPos::new(4, 8, 8));
        run(&mut engine, &world);
        assert_eq!(level(&engine, 6, 8, 8), 8);
        assert_eq!(level(&engine, 4, 8, 8), 6);
        assert_eq!(level(&engine, 12, 8, 8), 14);
    }

    #[test]
    fn emission_off_in_disabled_column() {
        let mut world = TestWorld::new();
        let mut engine = BlockLightEngine::new();
        engine.update_section_status(SectionKey::new(0, 0, 0), false);
        // No set_light_enabled: vanilla getEmission returns 0.
        world.set(8, 8, 8, find_state("torch", &[]));
        engine.check_block(LightPos::new(8, 8, 8));
        run(&mut engine, &world);
        assert_eq!(level(&engine, 8, 8, 8), 0);
        assert_eq!(level(&engine, 9, 8, 8), 0);
    }

    #[test]
    fn queued_packet_data_installs_without_propagating() {
        let world = TestWorld::new();
        let mut engine = BlockLightEngine::new();
        let mut layer = DataLayer::new();
        layer.set(1, 1, 1, 9);
        engine.queue_section_data(SectionKey::new(0, 0, 0), Some(layer));
        engine.update_section_status(SectionKey::new(0, 0, 0), false);
        run(&mut engine, &world);

        // Server data lands verbatim; nothing spreads without a checkBlock.
        assert_eq!(level(&engine, 1, 1, 1), 9);
        assert_eq!(level(&engine, 2, 1, 1), 0);

        // Marking the section empty tears the whole neighborhood down.
        engine.update_section_status(SectionKey::new(0, 0, 0), true);
        run(&mut engine, &world);
        for key in SectionKey::new(0, 0, 0).with_neighbors() {
            assert!(!engine.storage.storing_light_for_section(key), "{key:?}");
        }
    }

    #[test]
    fn slab_occludes_directionally() {
        let mut world = TestWorld::new();
        let mut engine = block_engine();
        let slab = find_state("oak_slab", &[("type", "top"), ("waterlogged", "false")]);
        world.set(8, 8, 8, find_state("torch", &[]));
        world.set(8, 9, 8, slab);
        engine.check_block(LightPos::new(8, 8, 8));
        engine.check_block(LightPos::new(8, 9, 8));
        run(&mut engine, &world);

        // Light enters the slab's own cell from below (its DOWN face is
        // open), but its full UP face blocks the straight path; above the
        // slab arrives the long way around.
        assert_eq!(level(&engine, 8, 9, 8), 13);
        assert_eq!(level(&engine, 9, 9, 8), 12);
        assert_eq!(level(&engine, 8, 10, 8), 10);
    }

    #[test]
    fn light_property_change_detection() {
        let _world = TestWorld::new();
        let air = BlockState::AIR;
        let stone = find_state("stone", &[]);
        let torch = find_state("torch", &[]);
        let glass = find_state("glass", &[]);
        let slab = find_state("oak_slab", &[("type", "bottom"), ("waterlogged", "false")]);

        assert!(!has_different_light_properties(stone, stone));
        assert!(has_different_light_properties(air, stone));
        assert!(has_different_light_properties(air, torch));
        // Glass matches air in every light property: no relight, as vanilla.
        assert!(!has_different_light_properties(air, glass));
        // Shaped states always relight.
        assert!(has_different_light_properties(air, slab));
        assert!(has_different_light_properties(slab, air));
    }
}
