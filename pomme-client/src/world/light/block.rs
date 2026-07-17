//! Vanilla `BlockLightEngine` plus the generic drive loop from
//! `LightEngine.runLightUpdates` (checkNode drain, decrease/increase passes
//! with the emission self-seed and staleness guard).

use std::collections::{HashSet, VecDeque};

use azalea_block::BlockState;

use super::data::DataLayer;
use super::opacity;
use super::queue::{self as entry, Dir};
use super::storage::{LightPos, NoHooks, SectionKey, StorageCore};
use super::world::LightBlockGetter;
use crate::world::block;

pub(crate) struct BlockLightEngine {
    nodes_to_check: HashSet<LightPos>,
    decrease_queue: VecDeque<(LightPos, u64)>,
    increase_queue: VecDeque<(LightPos, u64)>,
    pub(crate) storage: StorageCore,
}

impl BlockLightEngine {
    pub fn new() -> Self {
        Self {
            nodes_to_check: HashSet::new(),
            decrease_queue: VecDeque::new(),
            increase_queue: VecDeque::new(),
            storage: StorageCore::new(),
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
            .update_section_status(key, section_empty, &mut NoHooks);
    }

    pub fn set_light_enabled(&mut self, column: (i32, i32), enable: bool) {
        self.storage.set_light_enabled(column, enable);
    }

    /// First half of vanilla `runLightUpdates`: drain the check set, run the
    /// decrease pass, and apply queued/removed sections. The caller publishes
    /// changed sections (vanilla `swapSectionMap`), then runs
    /// [`Self::finish_updates`].
    pub fn begin_updates(&mut self, world: &impl LightBlockGetter) {
        let nodes: Vec<LightPos> = self.nodes_to_check.drain().collect();
        for pos in nodes {
            self.check_node(world, pos);
        }
        self.propagate_decreases(world);
        world.clear_cache();
        self.storage.mark_new_inconsistencies(&mut NoHooks);
    }

    /// Second half of vanilla `runLightUpdates`: the increase pass.
    pub fn finish_updates(&mut self, world: &impl LightBlockGetter) {
        while let Some((from, data)) = self.increase_queue.pop_front() {
            // Sections can drop out between enqueue and drain (unload tasks
            // processed by mark_new_inconsistencies mid-run).
            if !self.storage.storing_light_for_section(from.section()) {
                continue;
            }
            let mut from_level = self.storage.get_stored_level(from);
            let target = entry::get_from_level(data);
            if entry::is_increase_from_emission(data) && from_level < target {
                self.storage.set_stored_level(from, target);
                from_level = target;
            }
            if from_level == target {
                self.propagate_increase(world, from, data, from_level);
            }
        }
    }

    fn propagate_decreases(&mut self, world: &impl LightBlockGetter) {
        while let Some((from, data)) = self.decrease_queue.pop_front() {
            self.propagate_decrease(world, from, data);
        }
    }

    fn check_node(&mut self, world: &impl LightBlockGetter, pos: LightPos) {
        if !self.storage.storing_light_for_section(pos.section()) {
            return;
        }
        let state = world.state(pos);
        let emission = self.emission(pos, state);
        let old_level = self.storage.get_stored_level(pos);
        if emission < old_level {
            self.storage.set_stored_level(pos, 0);
            self.decrease_queue
                .push_back((pos, entry::decrease_all_directions(old_level)));
        } else {
            self.decrease_queue
                .push_back((pos, entry::PULL_LIGHT_IN_ENTRY));
        }
        if emission > 0 {
            self.increase_queue.push_back((
                pos,
                entry::increase_light_from_emission(emission, block::is_empty_shape(state)),
            ));
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
            if new_to_level <= 1 {
                continue;
            }
            self.increase_queue.push_back((
                to,
                entry::increase_skip_one_direction(
                    new_to_level as u8,
                    block::is_empty_shape(to_state),
                    dir.opposite(),
                ),
            ));
        }
    }

    fn propagate_decrease(&mut self, world: &impl LightBlockGetter, from: LightPos, data: u64) {
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
                let to_state = world.state(to);
                let to_emission = self.emission(to, to_state);
                self.storage.set_stored_level(to, 0);
                if to_emission < to_level {
                    self.decrease_queue.push_back((
                        to,
                        entry::decrease_skip_one_direction(to_level, dir.opposite()),
                    ));
                }
                if to_emission > 0 {
                    self.increase_queue.push_back((
                        to,
                        entry::increase_light_from_emission(
                            to_emission,
                            block::is_empty_shape(to_state),
                        ),
                    ));
                }
            } else {
                // A brighter independent neighbor: re-inject its light back
                // toward the darkened area.
                self.increase_queue.push_back((
                    to,
                    entry::increase_only_one_direction(to_level, false, dir.opposite()),
                ));
            }
        }
    }

    fn emission(&self, pos: LightPos, state: BlockState) -> u8 {
        let emission = block::light_props(state).emission;
        if emission > 0 && self.storage.light_on_in_section(pos.section()) {
            emission
        } else {
            0
        }
    }
}
