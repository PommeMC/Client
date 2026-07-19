//! Vanilla `SkyLightEngine` + the sky half of `SkyLightSectionStorage`:
//! source-column maintenance driven by [`ChunkSkyLightSources`], top-section
//! tracking so everything above terrain is an implicit 15, and the
//! empty-section fall-through that streams skylight down tall air gaps at
//! section borders.

use std::collections::{HashMap, HashSet, VecDeque};

use azalea_block::BlockState;

use super::data::{DataLayer, LAYER_BYTES};
use super::engine::LightEngine;
use super::opacity;
use super::queue::{self as entry, Dir};
use super::sources::ChunkSkyLightSources;
use super::storage::{LightPos, SectionKey, StorageCore, StorageHooks};
use super::world::LightBlockGetter;
use crate::world::block;

const REMOVE_TOP_SKY_SOURCE_ENTRY: u64 = entry::decrease_all_directions(15);
const REMOVE_SKY_SOURCE_ENTRY: u64 = entry::decrease_skip_one_direction(15, Dir::Up);
const ADD_SKY_SOURCE_ENTRY: u64 = entry::increase_skip_one_direction(15, false, Dir::Up);

fn is_source_level(value: u8) -> bool {
    value == 15
}

/// The sky extras of vanilla `SkyLightSectionStorage` /
/// `SkyDataLayerStorageMap`: per-column one-above-the-highest stored section,
/// and the lowest stored section y anywhere. Implements the storage hooks the
/// core calls where vanilla subclasses.
pub(crate) struct SkyStorage {
    top_sections: HashMap<(i32, i32), i32>,
    current_lowest_y: i32,
}

impl SkyStorage {
    fn new() -> Self {
        Self {
            top_sections: HashMap::new(),
            current_lowest_y: i32::MAX,
        }
    }

    /// One above the column's highest stored section (vanilla `topSections`
    /// with `currentLowestY` as the default value).
    fn top_section_y(&self, column: (i32, i32)) -> i32 {
        self.top_sections
            .get(&column)
            .copied()
            .unwrap_or(self.current_lowest_y)
    }

    fn has_light_data_at_or_below(&self, section_y: i32) -> bool {
        section_y >= self.current_lowest_y
    }

    fn is_above_data(&self, key: SectionKey) -> bool {
        let top = self.top_section_y(key.column());
        top == self.current_lowest_y || key.y >= top
    }

    /// The column's tracked top section, if any — the publish side's input
    /// for `ChunkLightData::sky_top_section`.
    pub fn column_top(&self, column: (i32, i32)) -> Option<i32> {
        self.top_sections.get(&column).copied()
    }
}

impl StorageHooks for SkyStorage {
    fn on_node_added(&mut self, key: SectionKey) {
        if self.current_lowest_y > key.y {
            self.current_lowest_y = key.y;
        }
        let column = key.column();
        if self.top_section_y(column) < key.y + 1 {
            self.top_sections.insert(column, key.y + 1);
        }
    }

    fn on_node_removed(&mut self, core: &StorageCore, key: SectionKey) {
        let column = key.column();
        if self.top_section_y(column) != key.y + 1 {
            return;
        }
        // The column's top section went away: walk down to the next stored
        // one, or forget the column entirely.
        let mut candidate = key;
        while !core.storing_light_for_section(candidate)
            && self.has_light_data_at_or_below(candidate.y)
        {
            candidate = candidate.offset(0, -1, 0);
        }
        if core.storing_light_for_section(candidate) {
            self.top_sections.insert(column, candidate.y + 1);
        } else {
            self.top_sections.remove(&column);
        }
    }

    fn create_data_layer(&mut self, core: &StorageCore, key: SectionKey) -> DataLayer {
        if let Some(queued) = core.queued_layer(key) {
            return queued.clone();
        }
        if self.is_above_data(key) {
            return if core.light_on_in_section(key) {
                DataLayer::filled(15)
            } else {
                DataLayer::new()
            };
        }
        // Below the top: seed from the nearest stored layer above so the new
        // section starts close to the light falling into it.
        let mut above = key.offset(0, 1, 0);
        loop {
            if let Some(layer) = core.layer(above) {
                return repeat_first_layer(layer);
            }
            above = above.offset(0, 1, 0);
        }
    }
}

/// Vanilla `repeatFirstLayer`: broadcast the layer's bottom 16x16 plane up
/// all 16 layers.
fn repeat_first_layer(data: &DataLayer) -> DataLayer {
    if data.is_homogeneous() {
        return data.clone();
    }
    let source = data.to_bytes();
    let mut out = Box::new([0u8; LAYER_BYTES]);
    for layer in 0..16 {
        out[layer * 128..(layer + 1) * 128].copy_from_slice(&source[..128]);
    }
    DataLayer::from_bytes(out)
}

pub(crate) struct SkyLightEngine {
    nodes_to_check: HashSet<LightPos>,
    decrease_queue: VecDeque<(LightPos, u64)>,
    increase_queue: VecDeque<(LightPos, u64)>,
    pub(crate) storage: StorageCore,
    pub(crate) sky: SkyStorage,
    /// Vanilla keeps these on each chunk; keyed by chunk pos here. A missing
    /// entry behaves like vanilla's null chunk.
    pub(crate) sources: HashMap<(i32, i32), ChunkSkyLightSources>,
}

impl SkyLightEngine {
    pub fn new() -> Self {
        Self {
            nodes_to_check: HashSet::new(),
            decrease_queue: VecDeque::new(),
            increase_queue: VecDeque::new(),
            storage: StorageCore::new(),
            sky: SkyStorage::new(),
            sources: HashMap::new(),
        }
    }

    pub fn check_block(&mut self, pos: LightPos) {
        self.nodes_to_check.insert(pos);
    }

    pub fn queue_section_data(&mut self, key: SectionKey, data: Option<DataLayer>) {
        self.storage.queue_section_data(key, data);
    }

    pub fn update_section_status(&mut self, key: SectionKey, section_empty: bool) {
        self.storage
            .update_section_status(key, section_empty, &mut self.sky);
    }

    /// Vanilla `SkyLightEngine.setLightEnabled`: besides flipping the column
    /// source flag, enabling pre-fills full-15 into empty stored layers at or
    /// above the column's highest terrain, so open sky reads 15 without
    /// propagation.
    pub fn set_light_enabled(&mut self, column: (i32, i32), enable: bool) {
        self.storage.set_light_enabled(column, enable);
        if !enable {
            return;
        }
        let highest = self
            .sources
            .get(&column)
            .map(|s| s.highest_lowest_source_y())
            .unwrap_or(i32::MIN);
        // Fully open columns (vanilla's empty sources) wrap the arithmetic in
        // Java and skip the loop; everything reads 15 via the implicit top.
        if highest == i32::MIN {
            return;
        }
        let lowest_fully_source_section = ((highest - 1) >> 4) + 1;
        let top = self.sky.top_section_y(column);
        let bottom = self.sky.current_lowest_y.max(lowest_fully_source_section);
        for section_y in (bottom..top).rev() {
            let key = SectionKey::new(column.0, section_y, column.1);
            if let Some(layer) = self.storage.layer_to_write(key)
                && layer.is_empty()
            {
                layer.fill(15);
            }
        }
    }

    fn lowest_source_y(&self, x: i32, z: i32, default: i32) -> i32 {
        match self.sources.get(&(x >> 4, z >> 4)) {
            Some(sources) => sources.lowest_source_y(x & 15, z & 15),
            None => default,
        }
    }

    fn update_sources_in_column(&mut self, x: i32, z: i32, lowest_source_y: i32) {
        let world_bottom_y = self.sky.current_lowest_y << 4;
        self.remove_sources_below(x, z, lowest_source_y, world_bottom_y);
        self.add_sources_above(x, z, lowest_source_y, world_bottom_y);
    }

    /// Clears source-level cells below a heightmap edge that rose.
    fn remove_sources_below(&mut self, x: i32, z: i32, lowest_source_y: i32, world_bottom_y: i32) {
        if lowest_source_y <= world_bottom_y {
            return;
        }
        let start_y = lowest_source_y - 1;
        let mut section_y = start_y >> 4;
        while self.sky.has_light_data_at_or_below(section_y) {
            let key = SectionKey::new(x >> 4, section_y, z >> 4);
            if self.storage.storing_light_for_section(key) {
                let section_bottom_y = section_y << 4;
                for y in (section_bottom_y..=start_y.min(section_bottom_y + 15)).rev() {
                    let pos = LightPos::new(x, y, z);
                    if !is_source_level(self.storage.get_stored_level(pos)) {
                        return;
                    }
                    self.storage.set_stored_level(pos, 0);
                    self.decrease_queue.push_back((
                        pos,
                        if y == lowest_source_y - 1 {
                            REMOVE_TOP_SKY_SOURCE_ENTRY
                        } else {
                            REMOVE_SKY_SOURCE_ENTRY
                        },
                    ));
                }
            }
            section_y -= 1;
        }
    }

    /// Fills source-level cells above a heightmap edge that dropped. Cells
    /// already shadowed by every lateral neighbor's sources are set without
    /// enqueueing (their surroundings are lit already).
    fn add_sources_above(&mut self, x: i32, z: i32, lowest_source_y: i32, world_bottom_y: i32) {
        let neighbor_lowest_source_y = self
            .lowest_source_y(x - 1, z, i32::MIN)
            .max(self.lowest_source_y(x + 1, z, i32::MIN))
            .max(self.lowest_source_y(x, z - 1, i32::MIN))
            .max(self.lowest_source_y(x, z + 1, i32::MIN));
        let start_y = lowest_source_y.max(world_bottom_y);
        let mut key = SectionKey::new(x >> 4, start_y >> 4, z >> 4);
        while !self.sky.is_above_data(key) {
            if self.storage.storing_light_for_section(key) {
                let section_bottom_y = key.y << 4;
                for y in section_bottom_y.max(start_y)..=section_bottom_y + 15 {
                    let pos = LightPos::new(x, y, z);
                    if is_source_level(self.storage.get_stored_level(pos)) {
                        return;
                    }
                    self.storage.set_stored_level(pos, 15);
                    if y < neighbor_lowest_source_y || y == lowest_source_y {
                        self.increase_queue.push_back((pos, ADD_SKY_SOURCE_ENTRY));
                    }
                }
            }
            key = key.offset(0, 1, 0);
        }
    }
}

impl LightEngine for SkyLightEngine {
    fn storage(&mut self) -> &mut StorageCore {
        &mut self.storage
    }

    fn nodes_to_check(&mut self) -> &mut HashSet<LightPos> {
        &mut self.nodes_to_check
    }

    fn increase_queue(&mut self) -> &mut VecDeque<(LightPos, u64)> {
        &mut self.increase_queue
    }

    fn decrease_queue(&mut self) -> &mut VecDeque<(LightPos, u64)> {
        &mut self.decrease_queue
    }

    fn mark_new_inconsistencies(&mut self) {
        self.storage.mark_new_inconsistencies(&mut self.sky);
    }

    fn check_node(&mut self, _world: &impl LightBlockGetter, pos: LightPos) {
        let section = pos.section();
        let lowest_source_y = if self.storage.light_on_in_section(section) {
            self.lowest_source_y(pos.x, pos.z, i32::MAX)
        } else {
            i32::MAX
        };
        if lowest_source_y != i32::MAX {
            self.update_sources_in_column(pos.x, pos.z, lowest_source_y);
        }
        if !self.storage.storing_light_for_section(section) {
            return;
        }
        if pos.y >= lowest_source_y {
            self.decrease_queue
                .push_back((pos, REMOVE_SKY_SOURCE_ENTRY));
            self.increase_queue.push_back((pos, ADD_SKY_SOURCE_ENTRY));
        } else {
            let old_level = self.storage.get_stored_level(pos);
            if old_level > 0 {
                self.storage.set_stored_level(pos, 0);
                self.decrease_queue
                    .push_back((pos, entry::decrease_all_directions(old_level)));
            } else {
                self.decrease_queue
                    .push_back((pos, entry::PULL_LIGHT_IN_ENTRY));
            }
        }
    }

    fn propagate_increase(
        &mut self,
        world: &impl LightBlockGetter,
        from: LightPos,
        data: u64,
        from_level: u8,
    ) {
        let mut from_state: Option<BlockState> = None;
        let empty_sections_below = self.count_empty_sections_below_if_at_border(from);
        for dir in Dir::ALL {
            if !entry::should_propagate_in_direction(data, dir) {
                continue;
            }
            let to = from.offset(dir);
            if !self.storage.storing_light_for_section(to.section()) {
                continue;
            }
            let to_level = self.storage.get_stored_level(to) as i32;
            if from_level as i32 - 1 <= to_level {
                continue;
            }
            let to_state = world.state(to);
            let new_to_level = from_level as i32 - opacity(to_state) as i32;
            if new_to_level <= to_level {
                continue;
            }
            let from_state = *from_state.get_or_insert_with(|| {
                if entry::is_from_empty_shape(data) {
                    BlockState::AIR
                } else {
                    world.state(from)
                }
            });
            if block::shape_occludes(from_state, to_state, dir as usize) {
                continue;
            }
            self.storage.set_stored_level(to, new_to_level as u8);
            if new_to_level > 1 {
                self.increase_queue.push_back((
                    to,
                    entry::increase_skip_one_direction(
                        new_to_level as u8,
                        block::is_empty_shape(to_state),
                        dir.opposite(),
                    ),
                ));
            }
            self.propagate_from_empty_sections(
                to,
                dir,
                new_to_level as u8,
                true,
                empty_sections_below,
            );
        }
    }

    fn propagate_decrease(&mut self, _world: &impl LightBlockGetter, from: LightPos, data: u64) {
        let empty_sections_below = self.count_empty_sections_below_if_at_border(from);
        let old_from_level = entry::get_from_level(data) as i32;
        for dir in Dir::ALL {
            if !entry::should_propagate_in_direction(data, dir) {
                continue;
            }
            let to = from.offset(dir);
            if !self.storage.storing_light_for_section(to.section()) {
                continue;
            }
            let to_level = self.storage.get_stored_level(to);
            if to_level == 0 {
                continue;
            }
            if (to_level as i32) < old_from_level {
                self.storage.set_stored_level(to, 0);
                self.decrease_queue.push_back((
                    to,
                    entry::decrease_skip_one_direction(to_level, dir.opposite()),
                ));
                self.propagate_from_empty_sections(to, dir, to_level, false, empty_sections_below);
            } else {
                self.increase_queue.push_back((
                    to,
                    entry::increase_only_one_direction(to_level, false, dir.opposite()),
                ));
            }
        }
    }
}

impl SkyLightEngine {
    /// How many non-storing sections sit directly below a section-boundary
    /// border cell — the only place light can cross into empty sections.
    fn count_empty_sections_below_if_at_border(&self, pos: LightPos) -> i32 {
        if pos.y & 15 != 0 {
            return 0;
        }
        let (local_x, local_z) = (pos.x & 15, pos.z & 15);
        if local_x != 0 && local_x != 15 && local_z != 0 && local_z != 15 {
            return 0;
        }
        let section = pos.section();
        let mut empty_below = 0;
        while !self
            .storage
            .storing_light_for_section(section.offset(0, -empty_below - 1, 0))
            && self
                .sky
                .has_light_data_at_or_below(section.y - empty_below - 1)
        {
            empty_below += 1;
        }
        empty_below
    }

    /// Streams a level change down the whole column of every storing section
    /// under an empty-section gap the light just crossed sideways over.
    fn propagate_from_empty_sections(
        &mut self,
        to: LightPos,
        dir: Dir,
        level: u8,
        increase: bool,
        empty_sections_below: i32,
    ) {
        if empty_sections_below == 0 {
            return;
        }
        if !crossed_section_edge(dir, to.x & 15, to.z & 15) {
            return;
        }
        let top_section_y = (to.y >> 4) - 1;
        let bottom_section_y = top_section_y - empty_sections_below + 1;
        let mut section_y = top_section_y;
        while section_y >= bottom_section_y {
            let key = SectionKey::new(to.x >> 4, section_y, to.z >> 4);
            if self.storage.storing_light_for_section(key) {
                let section_min_y = section_y << 4;
                for local_y in (0..16).rev() {
                    let pos = LightPos::new(to.x, section_min_y + local_y, to.z);
                    if increase {
                        self.storage.set_stored_level(pos, level);
                        if level > 1 {
                            self.increase_queue.push_back((
                                pos,
                                entry::increase_skip_one_direction(level, true, dir.opposite()),
                            ));
                        }
                    } else {
                        self.storage.set_stored_level(pos, 0);
                        self.decrease_queue.push_back((
                            pos,
                            entry::decrease_skip_one_direction(level, dir.opposite()),
                        ));
                    }
                }
            }
            section_y -= 1;
        }
    }
}

/// Whether a sideways propagation into local (x, z) just crossed a section
/// edge (vanilla `crossedSectionEdge`).
fn crossed_section_edge(dir: Dir, local_x: i32, local_z: i32) -> bool {
    match dir {
        Dir::North => local_z == 15,
        Dir::South => local_z == 0,
        Dir::West => local_x == 15,
        Dir::East => local_x == 0,
        Dir::Down | Dir::Up => false,
    }
}
