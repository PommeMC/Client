//! Vanilla `BlockLightEngine`; the generic drive loop lives in
//! [`LightEngine`].

use std::collections::{HashSet, VecDeque};

use azalea_block::BlockState;

use super::data::DataLayer;
use super::engine::LightEngine;
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

    fn emission(&self, pos: LightPos, state: BlockState) -> u8 {
        let emission = block::light_props(state).emission;
        if emission > 0 && self.storage.light_on_in_section(pos.section()) {
            emission
        } else {
            0
        }
    }
}

impl LightEngine for BlockLightEngine {
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
        self.storage.mark_new_inconsistencies(&mut NoHooks);
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
}
