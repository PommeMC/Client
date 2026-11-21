#![doc = include_str!("../README.md")]

use std::fmt::{self, Debug};

/// A block state id: one number per combination of a block and its property
/// values, in the order the connection's version numbers them.
#[derive(Clone, Copy, Default, Eq, Hash, PartialEq)]
pub struct BlockState {
    id: u16,
}

impl BlockState {
    /// Plain air, which is state 0 in every version; cave and void air have
    /// ids of their own.
    pub const AIR: BlockState = BlockState { id: 0 };

    /// Whether this is plain air.
    #[inline]
    pub fn is_air(&self) -> bool {
        *self == Self::AIR
    }

    /// The id, in the numbering of the version it was read from.
    #[inline]
    pub const fn id(&self) -> u16 {
        self.id
    }
}

impl TryFrom<u32> for BlockState {
    type Error = ();

    /// Fails for an id past `u16::MAX`, which would otherwise wrap onto
    /// another state.
    fn try_from(state_id: u32) -> Result<Self, Self::Error> {
        u16::try_from(state_id)
            .map(|id| BlockState { id })
            .map_err(|_| ())
    }
}

impl From<BlockState> for u32 {
    fn from(value: BlockState) -> Self {
        value.id as u32
    }
}

impl Debug for BlockState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BlockState(id: {})", self.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_past_the_range_are_refused() {
        assert_eq!(BlockState::try_from(0u32), Ok(BlockState::AIR));
        assert!(BlockState::try_from(u32::from(u16::MAX)).is_ok());
        assert!(BlockState::try_from(u32::from(u16::MAX) + 1).is_err());
    }
}
