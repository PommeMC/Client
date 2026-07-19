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

mod block;
mod data;
mod engine;
mod queue;
mod sky;
mod sources;
mod storage;
mod world;

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use azalea_block::BlockState;
use block::BlockLightEngine;
use data::{DataLayer, LAYER_BYTES};
use engine::LightEngine;
use sky::SkyLightEngine;
use sources::ChunkSkyLightSources;
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

/// Vanilla `readSectionList`: expands a packet's y-mask pair into one entry
/// per light section. Malformed (non-2048-byte) payloads are skipped rather
/// than disconnecting.
pub(crate) fn section_entries(
    light_section_count: usize,
    y_mask: &azalea_core::bitset::BitSet,
    empty_y_mask: &azalea_core::bitset::BitSet,
    updates: &[Box<[u8]>],
) -> Vec<SectionEntry> {
    let mut next = 0usize;
    (0..light_section_count)
        .map(|i| {
            if i < y_mask.len() && y_mask.index(i) {
                let entry = match updates.get(next) {
                    Some(bytes) if bytes.len() == LAYER_BYTES => {
                        let mut data = Box::new([0u8; LAYER_BYTES]);
                        data.copy_from_slice(bytes);
                        SectionEntry::Data(data)
                    }
                    _ => SectionEntry::Skip,
                };
                next += 1;
                entry
            } else if i < empty_y_mask.len() && empty_y_mask.index(i) {
                SectionEntry::Empty
            } else {
                SectionEntry::Skip
            }
        })
        .collect()
}

/// The block-mutation sites' shared path: vanilla `LevelChunk.setBlockState`'s
/// write plus its light hooks. Returns the previous state.
pub(crate) fn set_block_and_light(
    store: &ChunkStore,
    engine: &mut LevelLightEngine,
    x: i32,
    y: i32,
    z: i32,
    state: BlockState,
) -> BlockState {
    let (old, empty_flip) = store.set_block_state_tracked(x, y, z, state);
    engine.on_block_changed(
        LightPos::new(x, y, z),
        old,
        state,
        empty_flip,
        &|column_y| store.get_block_state(x, column_y, z),
    );
    old
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
    Sky,
}

pub(crate) struct LevelLightEngine {
    block: BlockLightEngine,
    /// `None` in dimensions without skylight (vanilla constructs the sky
    /// engine only when `dimensionType().hasSkyLight()`).
    sky: Option<SkyLightEngine>,
    tasks: VecDeque<LightTask>,
    /// Lowest block section y (`min_y >> 4`); light sections span one extra
    /// section below and above.
    min_section_y: i32,
    section_count: i32,
}

impl LevelLightEngine {
    pub fn new(height: u32, min_y: i32, has_sky: bool) -> Self {
        Self {
            block: BlockLightEngine::new(),
            sky: has_sky.then(SkyLightEngine::new),
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
    /// first, then (when the light properties changed) the sky-source
    /// heightmap update followed by a relight check. Call with the block
    /// already written to the chunk; `column_state` reads the block column at
    /// the position's x/z by world Y.
    pub fn on_block_changed(
        &mut self,
        pos: LightPos,
        old: BlockState,
        new: BlockState,
        empty_flip: Option<bool>,
        column_state: &impl Fn(i32) -> BlockState,
    ) {
        if let Some(empty) = empty_flip {
            self.block.update_section_status(pos.section(), empty);
            if let Some(sky) = &mut self.sky {
                sky.update_section_status(pos.section(), empty);
            }
        }
        if has_different_light_properties(old, new) {
            if let Some(sky) = &mut self.sky
                && let Some(sources) = sky.sources.get_mut(&(pos.x >> 4, pos.z >> 4))
            {
                sources.update(column_state, pos.x & 15, pos.y, pos.z & 15);
            }
            self.block.check_block(pos);
            if let Some(sky) = &mut self.sky {
                sky.check_block(pos);
            }
        }
    }

    /// Relight hook for positions mutated without old-state tracking (the
    /// player-prediction paths, which apply and roll back edits internally):
    /// the same hooks as [`Self::on_block_changed`] with the
    /// `hasDifferentLightProperties` work-skip dropped — section status
    /// dedups and `checkNode` recomputes idempotently, so results match at a
    /// small recompute cost.
    pub fn on_block_dirty(&mut self, store: &ChunkStore, x: i32, y: i32, z: i32) {
        let pos = LightPos::new(x, y, z);
        let empty = store.section_is_empty((x >> 4, z >> 4), y >> 4);
        self.block.update_section_status(pos.section(), empty);
        if let Some(sky) = &mut self.sky {
            sky.update_section_status(pos.section(), empty);
            if let Some(sources) = sky.sources.get_mut(&(x >> 4, z >> 4)) {
                sources.update(&|cy| store.get_block_state(x, cy, z), x & 15, y, z & 15);
            }
        }
        self.block.check_block(pos);
        if let Some(sky) = &mut self.sky {
            sky.check_block(pos);
        }
    }

    /// Registers a freshly loaded chunk column: builds its sky-source
    /// heightmap (vanilla fills sources at chunk-data apply, before the
    /// queued light task) and creates its light column from any layers the
    /// engine already stores (border sections lit by loaded neighbors), so
    /// later publishes only need to touch changed sections.
    pub fn on_chunk_loaded(&mut self, store: &mut ChunkStore, pos: (i32, i32)) {
        let count = self.light_section_count();
        let min_section_y = self.min_section_y;
        let key_at = |index: usize| SectionKey::new(pos.0, min_section_y - 1 + index as i32, pos.1);
        let mut sky_sections = vec![None; count];
        let mut block_sections = vec![None; count];
        for (index, slot) in block_sections.iter_mut().enumerate() {
            *slot = self
                .block
                .storage
                .layer(key_at(index))
                .map(DataLayer::to_bytes);
        }
        let mut sky_top_section = None;
        if let Some(sky) = &mut self.sky {
            for (index, slot) in sky_sections.iter_mut().enumerate() {
                *slot = sky.storage.layer(key_at(index)).map(DataLayer::to_bytes);
            }
            sky_top_section = sky
                .sky
                .column_top(pos)
                .map(|top| top - (self.min_section_y - 1));
            if let Some(chunk) =
                store.get_chunk(&azalea_core::position::ChunkPos::new(pos.0, pos.1))
            {
                sky.sources.insert(
                    pos,
                    ChunkSkyLightSources::fill_from(store.min_y(), &chunk.read()),
                );
            }
        }
        store.light_data.insert(
            pos,
            Arc::new(ChunkLightData {
                sky_sections,
                block_sections,
                min_y: store.min_y(),
                has_sky: self.sky.is_some(),
                sky_top_section,
            }),
        );
    }

    /// Forgets a column's sky sources; vanilla's live on the chunk object and
    /// vanish when it drops. The stored light tears down via
    /// [`LightTask::Remove`].
    pub fn on_chunk_unloaded(&mut self, pos: (i32, i32)) {
        if let Some(sky) = &mut self.sky {
            sky.sources.remove(&pos);
        }
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
                // Vanilla readSectionList order: sky layer first, then block.
                for (index, entry) in sky.iter().enumerate() {
                    let key = self.light_section_key(pos, index);
                    if let Some(engine) = &mut self.sky
                        && let Some(layer) = entry.to_layer()
                    {
                        engine.queue_section_data(key, Some(layer));
                    }
                    if !enable && !matches!(entry, SectionEntry::Skip) {
                        // Vanilla `setSectionDirtyWithNeighbors`.
                        out.sections.extend(key.with_neighbors());
                    }
                }
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
                // The enable-before-status order is load-bearing: fresh
                // above-terrain sky sections initialize to 15 only while the
                // column's light is already on.
                self.block.set_light_enabled(pos, true);
                if let Some(engine) = &mut self.sky {
                    engine.set_light_enabled(pos, true);
                }
                if enable {
                    // Vanilla enableChunkLight: report each block section's
                    // emptiness, then schedule the column's meshes.
                    for section_y in self.min_section_y..self.min_section_y + self.section_count {
                        let key = SectionKey::new(pos.0, section_y, pos.1);
                        let empty = store.section_is_empty(pos, section_y);
                        self.block.update_section_status(key, empty);
                        if let Some(engine) = &mut self.sky {
                            engine.update_section_status(key, empty);
                        }
                    }
                    out.columns.push(pos);
                }
            }
            LightTask::Remove { pos } => {
                // Vanilla queueLightRemoval order: sources off, null out every
                // light section, then mark every block section empty.
                self.block.set_light_enabled(pos, false);
                if let Some(engine) = &mut self.sky {
                    engine.set_light_enabled(pos, false);
                }
                for index in 0..self.light_section_count() {
                    let key = self.light_section_key(pos, index);
                    self.block.queue_section_data(key, None);
                    if let Some(engine) = &mut self.sky {
                        engine.queue_section_data(key, None);
                    }
                }
                for section_y in self.min_section_y..self.min_section_y + self.section_count {
                    let key = SectionKey::new(pos.0, section_y, pos.1);
                    self.block.update_section_status(key, true);
                    if let Some(engine) = &mut self.sky {
                        engine.update_section_status(key, true);
                    }
                }
            }
        }
    }

    /// Vanilla `runLightUpdates` per engine (block first, then sky), with the
    /// publish step sitting where `swapSectionMap` does: after the decrease
    /// pass, before the increase pass.
    fn run_light_updates(&mut self, store: &mut ChunkStore, out: &mut LightDirty) {
        let (min_section_y, section_count) = (self.min_section_y, self.section_count);
        {
            let world = StoreWorld::new(store);
            self.block.begin_updates(&world);
        }
        Self::publish(
            &mut self.block.storage,
            None,
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
        if let Some(sky) = &mut self.sky {
            {
                let world = StoreWorld::new(store);
                sky.begin_updates(&world);
            }
            Self::publish(
                &mut sky.storage,
                Some(&sky.sky),
                store,
                LayerKind::Sky,
                min_section_y,
                section_count,
                out,
            );
            {
                let world = StoreWorld::new(store);
                sky.finish_updates(&world);
            }
        }
    }

    /// Writes changed sections into their columns' [`ChunkLightData`]
    /// (vanilla `swapSectionMap`) and drains the remesh scope (vanilla
    /// `onLightUpdate` fan-out).
    fn publish(
        storage: &mut StorageCore,
        sky: Option<&sky::SkyStorage>,
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
                LayerKind::Sky => &mut column.sky_sections[index as usize],
            };
            *slot = storage.layer(key).map(DataLayer::to_bytes);
            // A column's top only moves when one of its own sections
            // changed, so refreshing it per changed key keeps it current.
            if let Some(sky) = sky {
                column.sky_top_section = sky
                    .column_top(key.column())
                    .map(|top| top - (min_section_y - 1));
            }
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

    fn run(engine: &mut impl LightEngine, world: &TestWorld) {
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

    fn sky_level(engine: &SkyLightEngine, x: i32, y: i32, z: i32) -> u8 {
        engine.storage.get_stored_level(LightPos::new(x, y, z))
    }

    /// Sky engine storing light around section (0,0,0), with chunk (0,0)'s
    /// sources built from the world by updating each column at `source_ys`.
    fn sky_engine(world: &TestWorld, source_ys: &[(i32, i32, i32)]) -> SkyLightEngine {
        let mut engine = SkyLightEngine::new();
        engine.update_section_status(SectionKey::new(0, 0, 0), false);
        let mut sources = ChunkSkyLightSources::new(-64);
        for &(x, y, z) in source_ys {
            sources.update(&|sy| world.state(LightPos::new(x, sy, z)), x, y, z);
        }
        engine.sources.insert((0, 0), sources);
        engine.set_light_enabled((0, 0), true);
        engine
    }

    #[test]
    fn digging_through_a_floor_lets_sky_under_it() {
        let mut world = TestWorld::new();
        let stone = find_state("stone", &[]);
        for x in 0..16 {
            for z in 0..16 {
                world.set(x, 0, z, stone);
            }
        }
        let floor: Vec<(i32, i32, i32)> = (0..16)
            .flat_map(|x| (0..16).map(move |z| (x, 0, z)))
            .collect();
        let mut engine = sky_engine(&world, &floor);
        run(&mut engine, &world);
        // Enabling pre-filled the fully-above-terrain section with 15.
        assert_eq!(sky_level(&engine, 8, 20, 8), 15);
        assert_eq!(sky_level(&engine, 8, -1, 8), 0);

        // Dig one floor block: its column becomes open sky and light floods
        // sideways under the floor.
        world.clear(8, 0, 8);
        engine.sources.get_mut(&(0, 0)).unwrap().update(
            &|y| world.state(LightPos::new(8, y, 8)),
            8,
            0,
            8,
        );
        engine.check_block(LightPos::new(8, 0, 8));
        run(&mut engine, &world);
        assert_eq!(sky_level(&engine, 8, 0, 8), 15);
        assert_eq!(sky_level(&engine, 8, -10, 8), 15);
        assert_eq!(sky_level(&engine, 9, 0, 8), 0); // stone floor
        assert_eq!(sky_level(&engine, 9, -1, 8), 14);
        assert_eq!(sky_level(&engine, 12, -1, 8), 11);

        // Cover it again: the source column tears down and the decrease wave
        // re-darkens everything under the floor.
        world.set(8, 0, 8, stone);
        engine.sources.get_mut(&(0, 0)).unwrap().update(
            &|y| world.state(LightPos::new(8, y, 8)),
            8,
            0,
            8,
        );
        engine.check_block(LightPos::new(8, 0, 8));
        run(&mut engine, &world);
        for (x, y, z) in [(8, 0, 8), (8, -10, 8), (9, -1, 8), (12, -1, 8)] {
            assert_eq!(sky_level(&engine, x, y, z), 0, "at {x} {y} {z}");
        }
        assert_eq!(sky_level(&engine, 8, 20, 8), 15);
    }

    #[test]
    fn leaves_attenuate_falling_sky_light() {
        let mut world = TestWorld::new();
        world.set(8, 10, 8, find_state("oak_leaves", &[]));
        let mut engine = sky_engine(&world, &[(8, 10, 8)]);
        engine.check_block(LightPos::new(8, 10, 8));
        run(&mut engine, &world);

        // Sources stop above the leaf; below it the light falls with the
        // leaf's opacity of 1 and then keeps losing 1 per non-source step.
        assert_eq!(sky_level(&engine, 8, 12, 8), 15);
        assert_eq!(sky_level(&engine, 9, 11, 8), 14);
        assert_eq!(sky_level(&engine, 8, 10, 8), 14);
        assert_eq!(sky_level(&engine, 8, 9, 8), 13);
        assert_eq!(sky_level(&engine, 8, 5, 8), 9);
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
