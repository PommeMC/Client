pub mod model;
pub mod registry;
pub mod sound;

use std::collections::HashMap;
use std::sync::LazyLock;

use azalea_block::{BlockBehavior, BlockState, BlockTrait};

/// Per-state block data extracted once from the generated block structs.
/// Hot paths (meshing classifies and textures every block) index this
/// instead of building a `Box<dyn BlockTrait>` and a property map per call.
struct BlockData {
    id: &'static str,
    properties: HashMap<&'static str, &'static str>,
    behavior: BlockBehavior,
}

static BLOCK_TABLE: LazyLock<Vec<BlockData>> = LazyLock::new(|| {
    (0u32..)
        .map_while(|id| BlockState::try_from(id).ok())
        .map(|state| {
            let block: Box<dyn BlockTrait> = state.into();
            BlockData {
                id: block.id(),
                properties: block.property_map(),
                behavior: block.behavior(),
            }
        })
        .collect()
});

fn block_data(state: BlockState) -> &'static BlockData {
    &BLOCK_TABLE[u32::from(state) as usize]
}

pub fn block_id(state: BlockState) -> &'static str {
    block_data(state).id
}

pub fn block_properties(state: BlockState) -> &'static HashMap<&'static str, &'static str> {
    &block_data(state).properties
}

pub fn block_behavior(state: BlockState) -> &'static BlockBehavior {
    &block_data(state).behavior
}
