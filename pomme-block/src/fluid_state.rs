use crate::block_state::BlockState;

/// Data-free stub: azalea-world's `get_fluid_state` compiles against this, but
/// every state resolves to [`FluidKind::Empty`]. Pomme computes fluid content
/// from its own per-version tables (`world::block::fluid`).
#[derive(Clone, Debug, Default)]
pub struct FluidState {
    pub kind: FluidKind,
    /// 0 = empty, 8 = full source; height is measured against 9.
    pub amount: u8,
    /// Whether this fluid is at the max level and there's another fluid of the
    /// same type above it.
    pub falling: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FluidKind {
    #[default]
    Empty,
    Water,
    Lava,
}

impl FluidState {
    /// A floating point number in between 0 and 1 representing the height (as a
    /// percentage of a full block) of the fluid.
    pub fn height(&self) -> f32 {
        self.amount as f32 / 9.
    }
}

impl From<BlockState> for FluidState {
    fn from(_state: BlockState) -> Self {
        Self::default()
    }
}
