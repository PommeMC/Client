//! Distant terrain's voxel: a `u64` of block state, biome and light; and how
//! eight of them merge into one of the next level.

use crate::world::block::BlockStateTable;

const BLOCK_SHIFT: u32 = 27;
const BIOME_SHIFT: u32 = 47;
const LIGHT_SHIFT: u32 = 56;
const BLOCK_MASK: u64 = ((1 << 20) - 1) << BLOCK_SHIFT;
pub(crate) const BIOME_MASK: u64 = 0x1FF << BIOME_SHIFT;
pub(crate) const LIGHT_MASK: u64 = 0xFF << LIGHT_SHIFT;

/// Block state 0 is air, whatever kind.
pub(crate) const AIR: u32 = 0;

/// `light` is `block << 4 | sky`. Air keeps no biome.
pub(crate) const fn compose(light: u8, block: u32, biome: u32) -> u64 {
    let light = (light as u64) << LIGHT_SHIFT;
    if block == AIR {
        return light;
    }
    light | (biome as u64) << BIOME_SHIFT | (block as u64) << BLOCK_SHIFT
}

pub(crate) fn is_air(voxel: u64) -> bool {
    voxel & BLOCK_MASK == 0
}

pub(crate) fn block_id(voxel: u64) -> u32 {
    ((voxel & BLOCK_MASK) >> BLOCK_SHIFT) as u32
}

pub(crate) fn light(voxel: u64) -> u8 {
    (voxel >> LIGHT_SHIFT) as u8
}

pub(crate) fn with_light(voxel: u64, light: u8) -> u64 {
    voxel & !LIGHT_MASK | u64::from(light) << LIGHT_SHIFT
}

/// What voxelizing and mipping ask of each block state: whether it's air,
/// and its opacity, leaves counting as solid and the rest as their light
/// dampening.
pub(crate) struct States {
    air: Box<[bool]>,
    opacity: Box<[u8]>,
}

impl States {
    pub fn new(table: BlockStateTable) -> Self {
        let data = (0..table.state_count() as u32).map(|id| {
            azalea_block::BlockState::try_from(id).map(|state| table.block_state_data(state))
        });
        let air = data
            .clone()
            .map(|data| data.is_ok_and(|data| data.is_air()));
        let opacity = data.map(|data| match data {
            Ok(data) if data.is_leaves() => 15,
            Ok(data) => data.light_props().dampening,
            Err(_) => 0,
        });
        Self {
            air: air.collect(),
            opacity: opacity.collect(),
        }
    }

    /// State `id` as a voxel's block: `AIR` for every kind of air.
    pub fn block(&self, id: u32) -> u32 {
        if self.air.get(id as usize).copied().unwrap_or(true) {
            AIR
        } else {
            id
        }
    }

    fn opacity(&self, voxel: u64) -> u8 {
        self.opacity
            .get(block_id(voxel) as usize)
            .copied()
            .unwrap_or(0)
    }

    /// Of eight children, indexed `x << 2 | y << 1 | z`, the most opaque
    /// block wins, ties going to the highest index (x, then y, then z). All
    /// air: air lit by the mean of their light, block light rounded down,
    /// sky up.
    pub fn mip(&self, children: [u64; 8]) -> u64 {
        let best = children
            .iter()
            .enumerate()
            .filter(|&(_, &child)| !is_air(child))
            .map(|(i, &child)| (u32::from(self.opacity(child)) << 4 | i as u32, child))
            .max_by_key(|&(score, _)| score);
        if let Some((_, winner)) = best {
            return winner;
        }
        let lights = children.map(|child| u32::from(light(child)));
        let block = (lights.iter().map(|l| l & 0xF0).sum::<u32>() / 8) & 0xF0;
        let sky = lights.iter().map(|l| l & 0x0F).sum::<u32>().div_ceil(8);
        with_light(children[7], (block | sky) as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voxels_pack_block_biome_and_light() {
        let voxel = compose(0xA5, 1234, 300);
        assert_eq!(block_id(voxel), 1234);
        assert_eq!(voxel >> BIOME_SHIFT & 0x1FF, 300);
        assert_eq!(light(voxel), 0xA5);
        assert!(!is_air(voxel));
        // Air keeps only its light.
        let air = compose(0x0F, AIR, 300);
        assert!(is_air(air));
        assert_eq!(air, 0x0F << LIGHT_SHIFT);
        assert_eq!(light(with_light(voxel, 3)), 3);
        assert_eq!(block_id(with_light(voxel, 3)), 1234);
    }

    fn states(opacity: &[u8]) -> States {
        States {
            air: vec![false; opacity.len()].into(),
            opacity: opacity.into(),
        }
    }

    #[test]
    fn the_most_opaque_child_wins_and_ties_go_to_the_top_corner() {
        // States: 1 glass (0), 2 stone (15), 3 leaves (15).
        let states = states(&[0, 0, 15, 15]);
        let (glass, stone, leaves) = (compose(0, 1, 0), compose(0, 2, 0), compose(0, 3, 0));
        let mut children = [0; 8];
        children[0] = stone;
        children[7] = glass;
        assert_eq!(states.mip(children), stone, "stone beats glass");
        children[3] = leaves;
        assert_eq!(
            states.mip(children),
            leaves,
            "a tie goes to the higher corner"
        );
        assert_eq!(
            states.mip([0, 0, 0, 0, 0, 0, 0, glass]),
            glass,
            "any block beats air"
        );
    }

    #[test]
    fn all_air_children_average_their_light() {
        let states = states(&[0]);
        // Block light 0x10 x 4 and 0: mean 8 floors to 0; sky 1 x 3 ceils to 1.
        let mut children = [0u64; 8];
        for (i, child) in children.iter_mut().enumerate() {
            let block = if i < 4 { 0x10 } else { 0 };
            let sky = u8::from(i < 3);
            *child = compose(block | sky, AIR, 0);
        }
        assert_eq!(light(states.mip(children)), 0x01);
        // Full sky everywhere stays full.
        assert_eq!(light(states.mip([compose(0x0F, AIR, 0); 8])), 0x0F);
    }
}
