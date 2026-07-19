pub mod model;
pub mod registry;
pub mod sound;

/// `BlockState::to_trait()` shim over the pinned azalea, which only offers the
/// boxing `From` conversion. Single point to swap in an allocation-free lookup
/// later.
pub trait BlockStateExt {
    fn to_trait(self) -> Box<dyn azalea_block::BlockTrait>;
}

impl BlockStateExt for azalea_block::BlockState {
    #[inline]
    fn to_trait(self) -> Box<dyn azalea_block::BlockTrait> {
        self.into()
    }
}
