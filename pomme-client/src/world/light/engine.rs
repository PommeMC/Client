//! The drive loop of vanilla's abstract `LightEngine` (`runLightUpdates`:
//! checkNode drain, decrease pass, increase pass with the emission self-seed
//! and staleness guard); the block and sky engines supply the per-layer
//! passes.

use std::collections::{HashSet, VecDeque};

use super::queue as entry;
use super::storage::{LightPos, StorageCore};
use super::world::LightBlockGetter;

pub(crate) trait LightEngine {
    fn storage(&mut self) -> &mut StorageCore;
    fn nodes_to_check(&mut self) -> &mut HashSet<LightPos>;
    fn increase_queue(&mut self) -> &mut VecDeque<(LightPos, u64)>;
    fn decrease_queue(&mut self) -> &mut VecDeque<(LightPos, u64)>;

    fn check_node(&mut self, world: &impl LightBlockGetter, pos: LightPos);
    fn propagate_increase(
        &mut self,
        world: &impl LightBlockGetter,
        from: LightPos,
        data: u64,
        from_level: u8,
    );
    fn propagate_decrease(&mut self, world: &impl LightBlockGetter, from: LightPos, data: u64);
    /// `StorageCore::mark_new_inconsistencies` with the layer's hooks.
    fn mark_new_inconsistencies(&mut self);

    /// First half of vanilla `runLightUpdates`: drain the check set, run the
    /// decrease pass, and apply queued/removed sections. The caller publishes
    /// changed sections (vanilla `swapSectionMap`), then runs
    /// [`Self::finish_updates`].
    fn begin_updates(&mut self, world: &impl LightBlockGetter) {
        let nodes: Vec<LightPos> = self.nodes_to_check().drain().collect();
        for pos in nodes {
            self.check_node(world, pos);
        }
        self.propagate_decreases(world);
        world.clear_cache();
        self.mark_new_inconsistencies();
    }

    /// Second half of vanilla `runLightUpdates`: the increase pass.
    fn finish_updates(&mut self, world: &impl LightBlockGetter) {
        while let Some((from, data)) = self.increase_queue().pop_front() {
            // Sections can drop out between enqueue and drain (unload tasks
            // processed by mark_new_inconsistencies mid-run).
            if !self.storage().storing_light_for_section(from.section()) {
                continue;
            }
            let mut from_level = self.storage().get_stored_level(from);
            let target = entry::get_from_level(data);
            if entry::is_increase_from_emission(data) && from_level < target {
                self.storage().set_stored_level(from, target);
                from_level = target;
            }
            if from_level == target {
                self.propagate_increase(world, from, data, from_level);
            }
        }
    }

    fn propagate_decreases(&mut self, world: &impl LightBlockGetter) {
        while let Some((from, data)) = self.decrease_queue().pop_front() {
            self.propagate_decrease(world, from, data);
        }
    }
}
