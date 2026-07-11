use std::fmt::{self, Debug};
use std::io::{self, Cursor, Write};

use azalea_buf::{AzBuf, AzBufVar, BufReadError};
use azalea_registry::builtin::BlockKind;

use crate::Property;

/// The type that's used internally to represent a block state ID.
///
/// This does not affect protocol serialization, it just allows you to make the
/// internal type smaller if you want.
pub type BlockStateIntegerRepr = u16;

/// A representation of a state a block can be in.
///
/// For example, a stone block only has one state but each possible stair
/// rotation is a different state.
///
/// Note that this type is internally either a `u16` or `u32`, depending on
/// [`BlockStateIntegerRepr`].
#[derive(Clone, Copy, Default, Eq, Hash, PartialEq)]
pub struct BlockState {
    id: BlockStateIntegerRepr,
}

impl BlockState {
    /// A shortcut for getting the air block state, since it always has an ID of
    /// 0.
    ///
    /// This does not include the other types of air like cave air.
    pub const AIR: BlockState = BlockState { id: 0 };

    /// Data-free shim: every id in the integer-repr range is valid transport.
    #[inline]
    pub const fn is_valid_state(_state_id: BlockStateIntegerRepr) -> bool {
        true
    }

    /// Returns true if the block is air.
    ///
    /// This only checks for normal air, not other types like cave air.
    #[inline]
    pub fn is_air(&self) -> bool {
        *self == Self::AIR
    }

    /// Returns the protocol ID for the block state.
    ///
    /// These IDs may change across Minecraft versions, so you shouldn't
    /// hard-code them or store them in databases.
    #[inline]
    pub const fn id(&self) -> BlockStateIntegerRepr {
        self.id
    }

    /// Upper bound of the id transport range; ids carry no block meaning in
    /// this shim.
    pub const MAX_STATE: BlockStateIntegerRepr = BlockStateIntegerRepr::MAX;

    /// Always `None` in the data-free shim.
    pub fn property<P: Property>(self) -> Option<P::Value> {
        P::try_from_block_state(self)
    }

    /// Always [`BlockKind::Air`] in the data-free shim.
    pub fn as_block_kind(self) -> BlockKind {
        BlockKind::Air
    }
}

impl TryFrom<u32> for BlockState {
    type Error = ();

    /// Safely converts a u32 state ID to a block state.
    fn try_from(state_id: u32) -> Result<Self, Self::Error> {
        // Range-check before truncating so out-of-range ids can't alias
        // modulo the integer repr.
        if state_id <= BlockStateIntegerRepr::MAX as u32 {
            Ok(BlockState {
                id: state_id as BlockStateIntegerRepr,
            })
        } else {
            Err(())
        }
    }
}
impl TryFrom<i32> for BlockState {
    type Error = ();

    fn try_from(state_id: i32) -> Result<Self, Self::Error> {
        Self::try_from(state_id as u32)
    }
}

impl TryFrom<u16> for BlockState {
    type Error = ();

    /// Safely converts a u16 state ID to a block state.
    fn try_from(id: u16) -> Result<Self, Self::Error> {
        Ok(BlockState { id })
    }
}
impl From<BlockState> for u32 {
    /// See [`BlockState::id`].
    fn from(value: BlockState) -> Self {
        value.id as u32
    }
}

impl AzBuf for BlockState {
    fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
        let state_id = u32::azalea_read_var(buf)?;
        Self::try_from(state_id).map_err(|_| BufReadError::UnexpectedEnumVariant {
            id: state_id as i32,
        })
    }
    fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()> {
        u32::azalea_write_var(&(self.id as u32), buf)
    }
}

impl Debug for BlockState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BlockState(id: {})", self.id)
    }
}

impl From<BlockState> for BlockKind {
    fn from(block_state: BlockState) -> Self {
        block_state.as_block_kind()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_u32() {
        assert_eq!(
            BlockState::try_from(0 as BlockStateIntegerRepr).unwrap(),
            BlockState::AIR
        );

        // The shim accepts the whole integer-repr range as transport.
        assert!(BlockState::try_from(BlockState::MAX_STATE as u32).is_ok());
        assert!(BlockState::try_from(BlockState::MAX_STATE as u32 + 1).is_err());
    }
}
