pub mod model;
pub mod registry;
pub mod sound;

use std::collections::HashMap;
use std::sync::LazyLock;

use azalea_block::{BlockBehavior, BlockState, BlockTrait};

use crate::physics::block_shape::{LocalBox, compute_shape};

/// Compact per-state property map: `(key, value)` pairs sorted by key. States
/// have at most ~10 properties, so binary search on a boxed slice beats a
/// `HashMap` in both footprint and lookup cost.
pub struct PropMap(Box<[(&'static str, &'static str)]>);

impl PropMap {
    fn new(props: HashMap<&'static str, &'static str>) -> Self {
        let mut pairs: Vec<_> = props.into_iter().collect();
        pairs.sort_unstable_by_key(|&(k, _)| k);
        Self(pairs.into_boxed_slice())
    }

    pub fn get(&self, key: &str) -> Option<&'static str> {
        self.0
            .binary_search_by_key(&key, |&(k, _)| k)
            .ok()
            .map(|i| self.0[i].1)
    }
}

/// Per-state block data extracted once from the generated block structs.
/// Hot paths (meshing classifies and textures every block) index this
/// instead of building a `Box<dyn BlockTrait>` and a property map per call.
struct BlockData {
    id: &'static str,
    properties: PropMap,
    behavior: BlockBehavior,
    /// Collision shape; see `block_shape::partial_shape` for the encoding.
    shape: Option<Box<[LocalBox]>>,
}

static BLOCK_TABLE: LazyLock<Vec<BlockData>> = LazyLock::new(|| {
    let table: Vec<BlockData> = (0u32..)
        .map_while(|id| BlockState::try_from(id).ok())
        .map(|state| {
            let block: Box<dyn BlockTrait> = state.into();
            let id = block.id();
            let properties = PropMap::new(block.property_map());
            let shape = compute_shape(id, &properties).map(Vec::into_boxed_slice);
            BlockData {
                id,
                properties,
                behavior: block.behavior(),
                shape,
            }
        })
        .collect();
    // The helpers index by state id, so the table must cover the dense id
    // space exactly.
    assert_eq!(table.len(), BlockState::MAX_STATE as usize + 1);
    table
});

fn block_data(state: BlockState) -> &'static BlockData {
    &BLOCK_TABLE[u32::from(state) as usize]
}

/// Every valid block state paired with its cached data, in id order.
fn all_states() -> impl Iterator<Item = (BlockState, &'static BlockData)> {
    BLOCK_TABLE.iter().enumerate().map(|(id, data)| {
        let state = BlockState::try_from(id as u32).expect("table indices are valid state ids");
        (state, data)
    })
}

pub fn block_id(state: BlockState) -> &'static str {
    block_data(state).id
}

pub fn block_properties(state: BlockState) -> &'static PropMap {
    &block_data(state).properties
}

pub fn block_behavior(state: BlockState) -> &'static BlockBehavior {
    &block_data(state).behavior
}

pub(crate) fn block_shape(state: BlockState) -> Option<&'static [LocalBox]> {
    block_data(state).shape.as_deref()
}
