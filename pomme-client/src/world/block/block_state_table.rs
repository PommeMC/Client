//! Immutable, version-selected state-ID table, independent of the active
//! session. Each entry is one state's `BlockStateData`; this handle selects the
//! whole table.

use std::collections::HashMap;

use azalea_block::BlockState;

use super::{
    BEDROCK_LIGHT, BLOCK_DATA, BLOCK_TABLES, BlockStateData, FIRST_STATES, Fluid, FluidKind,
    NATIVE_SLOT, NO_FLUID, PLACEABLE_BLOCKS, PropMap, PushReaction, SpecialCollision,
    block_data_slot, build_table,
};

/// A cheap, immutable handle to one cached version-selected state-ID table.
/// Aliases share data; unsupported protocols use the native table.
#[derive(Clone, Copy)]
pub(crate) struct BlockStateTable {
    slot: usize,
    table: &'static [BlockStateData],
}

impl BlockStateTable {
    /// Builds only the explicitly selected table, without reading or changing
    /// the active protocol. The cache owns the data for the process lifetime.
    pub(crate) fn for_protocol(protocol: i32) -> Self {
        let slot = block_data_slot(protocol).unwrap_or(NATIVE_SLOT);
        let table = BLOCK_TABLES[slot].get_or_init(|| build_table(&BLOCK_DATA[slot]));
        Self { slot, table }
    }

    #[cfg(test)]
    pub(crate) fn native() -> Self {
        Self::for_protocol(pomme_protocol::version::NATIVE.protocol)
    }

    pub(crate) fn table_slot(self) -> usize {
        self.slot
    }

    /// Validates a wire-side state id in this selected table.
    pub(crate) fn try_state(self, id: u32) -> Option<BlockState> {
        if (id as usize) < self.table.len() {
            BlockState::try_from(id).ok()
        } else {
            None
        }
    }

    pub(crate) fn registry_name(self, registry_id: u32) -> Option<&'static str> {
        super::registry_names(self.table).nth(registry_id as usize)
    }

    pub(crate) fn first_state_of(self, name: &str) -> Option<BlockState> {
        let map = FIRST_STATES[self.slot].get_or_init(|| {
            let mut first = HashMap::new();
            for (id, data) in self.table.iter().enumerate() {
                first.entry(data.id).or_insert_with(|| state_at(id));
            }
            first
        });
        map.get(name).copied()
    }

    /// How many block states the table holds (vanilla
    /// `Block.BLOCK_STATE_REGISTRY.size()`).
    pub(crate) fn state_count(self) -> usize {
        self.table.len()
    }

    /// Vanilla `setValue`: `state` with property `key` set to `value`, searched
    /// within the block's own states. `None` when the block has no such
    /// property or value.
    pub(crate) fn with_property(
        self,
        state: BlockState,
        key: &str,
        value: &str,
    ) -> Option<BlockState> {
        let table = self.table;
        let id = u32::from(state) as usize;
        let data = table.get(id)?;
        let current = data.properties.get(key)?;
        if current == value {
            return Some(state);
        }
        // A block's states are contiguous and share one interned id.
        let same_block = |d: &BlockStateData| std::ptr::eq(d.id, data.id);
        let start = table[..id]
            .iter()
            .rposition(|d| !same_block(d))
            .map_or(0, |i| i + 1);
        let end = table[id..]
            .iter()
            .position(|d| !same_block(d))
            .map_or(table.len(), |i| id + i);
        let wanted = |k: &str, v: &str| {
            if k == key {
                v == value
            } else {
                data.properties.get(k) == Some(v)
            }
        };
        (start..end)
            .find(|&i| table[i].properties.entries().all(|(k, v)| wanted(k, v)))
            .map(state_at)
    }

    /// The first state of block `name` with the given property values set.
    pub(crate) fn state_with(self, name: &str, props: &[(&str, &str)]) -> Option<BlockState> {
        props
            .iter()
            .try_fold(self.first_state_of(name)?, |s, &(k, v)| {
                self.with_property(s, k, v)
            })
    }

    /// The block's state matching the given property values (unlisted
    /// properties take the block's first state's values).
    #[cfg(any(test, feature = "cpu-bench"))]
    pub(crate) fn find_state(self, name: &str, props: &[(&str, &str)]) -> BlockState {
        self.state_with(name, props)
            .unwrap_or_else(|| panic!("no state {name} {props:?}"))
    }

    /// Vanilla `FluidState.createLegacyBlock`: the block a lone fluid is.
    pub(crate) fn fluid_block(self, fluid: Fluid) -> BlockState {
        let name = match fluid.kind {
            FluidKind::Empty => return BlockState::AIR,
            FluidKind::Water => "water",
            FluidKind::Lava => "lava",
        };
        // `FlowingFluid.getLegacyLevel`.
        let level = if fluid.amount == 8 && !fluid.falling {
            0
        } else {
            8 - fluid.amount.min(8) + if fluid.falling { 8 } else { 0 }
        };
        self.state_with(name, &[("level", &level.to_string())])
            .unwrap_or(BlockState::AIR)
    }

    /// State metadata in wire-id order for binding the selected definitions.
    pub(super) fn table(self) -> &'static [BlockStateData] {
        self.table
    }

    /// Only single-state blocks support placement prediction by item name.
    /// Names are unprefixed, matching the item registry.
    pub(crate) fn placeable_block_for_item(self, item_name: &str) -> Option<BlockState> {
        let map = PLACEABLE_BLOCKS[self.slot].get_or_init(|| {
            let mut seen: HashMap<&'static str, Option<BlockState>> = HashMap::new();
            for (id, data) in self.table.iter().enumerate() {
                seen.entry(data.id)
                    .and_modify(|v| *v = None)
                    .or_insert(Some(state_at(id)));
            }
            seen.into_iter()
                .filter_map(|(name, state)| state.map(|s| (name, s)))
                .collect()
        });
        map.get(item_name).copied()
    }

    /// Unknown state ids use a solid, unlit fallback.
    pub(crate) fn block_state_data(self, state: BlockState) -> &'static BlockStateData {
        static UNKNOWN: std::sync::LazyLock<BlockStateData> =
            std::sync::LazyLock::new(|| BlockStateData {
                id: "unknown",
                properties: PropMap::from_pairs(Vec::new()),
                shape: None,
                outline: None,
                is_air: false,
                collides: true,
                special_collision: SpecialCollision::None,
                blocks_motion: None,
                legacy_solid: None,
                replaceable: None,
                full_face_sturdy: None,
                large_collision_shape: false,
                collision_shape_uses_offset: false,
                outline_shape_uses_offset: false,
                position_offset: None,
                fluid: NO_FLUID,
                light: BEDROCK_LIGHT,
                push_reaction: PushReaction::Normal,
                has_block_entity: false,
            });
        self.table
            .get(u32::from(state) as usize)
            .unwrap_or(&UNKNOWN)
    }
}

fn state_at(id: usize) -> BlockState {
    BlockState::try_from(id as u32).expect("table indices fit the id repr")
}
