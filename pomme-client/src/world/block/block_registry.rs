//! App-owned block definitions and the session's version-bound registry.

use std::rc::Rc;
use std::sync::Arc;

use azalea_block::BlockState;

use super::render_assets::{BlockRenderAssets, BlockRenderView, OwnedBlockRenderView};
use super::{
    BlockStateData, BlockStateTable, BlockTags, load_behaviors, registry_names, resolve_block_tags,
};
use crate::position::BlockPos;
use crate::world::level::LevelContext;

/// Shared, immutable block rules, independent of the compact wire state ID.
pub trait Block {
    fn block_data(&self) -> &BlockData;

    fn animate_tick(
        &self,
        _state: BlockState,
        _pos: BlockPos,
        _level: &mut LevelContext<'_>,
        _random: &mut fastrand::Rng,
    ) {
    }
}

/// Immutable definition data shared by all states of one registered block.
/// Distinct from the state's key/value properties in [`super::PropMap`].
#[derive(Clone, Copy, Debug, Default)]
pub struct BlockData {
    pub destroy_time: f32,
    pub requires_correct_tool_for_drops: bool,
}

struct DefaultBlock {
    block_data: BlockData,
}

impl super::Block for DefaultBlock {
    fn block_data(&self) -> &BlockData {
        &self.block_data
    }
}

/// Selects and constructs block implementations, without owning definitions
/// or protocol bindings. Add name-specific selections here as rules land;
/// currently every name uses the default no-op animation behavior.
pub(crate) struct BlockFactory;

impl BlockFactory {
    pub(crate) fn create(&self, _name: &str, block_data: BlockData) -> Box<dyn Block> {
        Box::new(DefaultBlock { block_data })
    }
}

pub(crate) struct BlockDefinitions {
    definitions: Vec<(&'static str, Box<dyn Block>)>,
}

impl BlockDefinitions {
    pub(crate) fn new() -> Self {
        let factory = BlockFactory;
        let mut entries: Vec<_> = load_behaviors().iter().collect();
        entries.sort_unstable_by_key(|(name, _)| *name);
        let mut registry = Self {
            definitions: Vec::with_capacity(entries.len() + 1),
        };
        registry.register("", factory.create("", BlockData::default()));
        for (name, entry) in entries {
            registry.register(name.as_str(), factory.create(name, entry.block_data()));
        }
        registry
    }

    /// Register one shared rule for all property states of a known block.
    fn register(&mut self, name: &'static str, definition: Box<dyn Block>) {
        self.definitions.push((name, definition));
    }
}

/// Session-owned metadata selection and numeric bindings to shared definitions.
pub(crate) struct BlockRegistry {
    definitions: Rc<BlockDefinitions>,
    render_assets: Arc<BlockRenderAssets>,
    protocol: i32,
    block_state_table: BlockStateTable,
    bindings: BlockBindings,
    tags: BlockTags,
}

impl BlockRegistry {
    pub(crate) fn new(
        definitions: Rc<BlockDefinitions>,
        render_assets: Arc<BlockRenderAssets>,
        protocol: i32,
    ) -> Self {
        let block_state_table = BlockStateTable::for_protocol(protocol);
        let bindings = BlockBindings::new(&definitions, block_state_table);
        Self {
            definitions,
            render_assets,
            protocol,
            block_state_table,
            bindings,
            tags: BlockTags::default(),
        }
    }

    /// The native version's registry with no render assets.
    #[cfg(test)]
    pub(crate) fn native() -> Self {
        Self::new(
            Rc::new(BlockDefinitions::new()),
            Arc::new(BlockRenderAssets::default()),
            pomme_protocol::version::NATIVE.protocol,
        )
    }

    /// Keep wire identity even when two protocols share a canonical table.
    pub(crate) fn select_protocol(&mut self, protocol: i32) {
        if self.protocol != protocol {
            let block_state_table = BlockStateTable::for_protocol(protocol);
            if self.block_state_table.table_slot() != block_state_table.table_slot() {
                self.bindings = BlockBindings::new(&self.definitions, block_state_table);
            }
            self.block_state_table = block_state_table;
            self.protocol = protocol;
        }
        self.clear_tags();
    }

    pub(crate) fn block_state_table(&self) -> BlockStateTable {
        self.block_state_table
    }

    /// Requested wire protocol, including aliases sharing a metadata table.
    pub(crate) fn protocol(&self) -> i32 {
        self.protocol
    }

    pub(crate) fn render_view(&self) -> BlockRenderView<'_> {
        self.render_assets.for_version(self.block_state_table)
    }

    pub(crate) fn owned_render_view(&self) -> OwnedBlockRenderView {
        OwnedBlockRenderView::new(self.render_assets.clone(), self.block_state_table)
    }

    pub(crate) fn replace_render_assets(&mut self, render_assets: Arc<BlockRenderAssets>) {
        self.render_assets = render_assets;
    }

    pub(crate) fn placeable_block_for_item(&self, item_name: &str) -> Option<BlockState> {
        self.block_state_table.placeable_block_for_item(item_name)
    }

    pub(crate) fn replace_tags(&mut self, raw_tags: Vec<(String, Vec<i32>)>) {
        let names: Vec<_> = registry_names(self.block_state_table.table()).collect();
        self.tags = resolve_block_tags(raw_tags, &names);
    }

    pub(crate) fn clear_tags(&mut self) {
        self.tags = BlockTags::default();
    }

    pub(crate) fn tags(&self) -> &BlockTags {
        &self.tags
    }

    /// Includes the state's key/value `PropMap`, not definition properties.
    pub(crate) fn block_state_data(&self, state: BlockState) -> &'static BlockStateData {
        self.block_state_table.block_state_data(state)
    }

    /// The shared rule exposes immutable `BlockData` via `block_data()`.
    pub(crate) fn block(&self, state: BlockState) -> &dyn Block {
        self.bindings.get(&self.definitions, state)
    }

    /// Select and invoke together so callbacks receive the queried state.
    pub(crate) fn animate_tick(
        &self,
        pos: BlockPos,
        level: &mut LevelContext<'_>,
        random: &mut fastrand::Rng,
    ) {
        let state = level.get_block_state(pos);
        self.block(state).animate_tick(state, pos, level, random);
    }
}

/// Private O(1) wire-state indices into the immutable global definitions.
struct BlockBindings {
    state_definitions: Vec<usize>,
}

impl BlockBindings {
    fn new(definitions: &BlockDefinitions, block_state_table: BlockStateTable) -> Self {
        let table = block_state_table.table();
        let mut state_definitions = Vec::with_capacity(table.len());
        for states in table.chunk_by(|a, b| a.id == b.id) {
            // Sorted by name in `BlockDefinitions::new`; a miss is the default.
            let index = definitions
                .definitions
                .binary_search_by_key(&states[0].id, |(name, _)| *name)
                .unwrap_or(0);
            state_definitions.extend(std::iter::repeat_n(index, states.len()));
        }
        Self { state_definitions }
    }

    /// O(1) wire-state lookup, including an out-of-table fallback.
    fn get<'a>(&self, definitions: &'a BlockDefinitions, state: BlockState) -> &'a dyn Block {
        let index = self
            .state_definitions
            .get(u32::from(state) as usize)
            .copied()
            .unwrap_or(0);
        definitions.definitions[index].1.as_ref()
    }
}
