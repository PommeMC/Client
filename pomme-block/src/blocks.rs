//! `Box<dyn BlockTrait>` conversions resolve to a single unknown block.

use std::collections::HashMap;

use azalea_registry::builtin::BlockKind;

use crate::{BlockBehavior, BlockState, BlockTrait, InvalidPropertyError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnknownBlock;

impl BlockTrait for UnknownBlock {
    fn behavior(&self) -> BlockBehavior {
        BlockBehavior::default()
    }

    fn id(&self) -> &'static str {
        "unknown"
    }

    fn as_block_state(&self) -> BlockState {
        BlockState::AIR
    }

    fn as_block_kind(&self) -> BlockKind {
        BlockKind::Air
    }

    fn property_map(&self) -> HashMap<&'static str, &'static str> {
        HashMap::new()
    }

    fn get_property(&self, _name: &str) -> Option<&'static str> {
        None
    }

    fn set_property(&mut self, _name: &str, _value: &str) -> Result<(), InvalidPropertyError> {
        Err(InvalidPropertyError)
    }
}

impl From<BlockState> for Box<dyn BlockTrait> {
    fn from(_state: BlockState) -> Self {
        Box::new(UnknownBlock)
    }
}
