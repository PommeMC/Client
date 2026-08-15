pub const MAX_TINT_INDEX: u32 = 511;
pub const TINTS_PER_BATCH: usize = 512;

const BLOCK_SHIFT: u32 = 52;
const DIR_SHIFT: u32 = 49;
const WIDTH_SHIFT: u32 = 45;
const HEIGHT_SHIFT: u32 = 41;
const GLOBAL_SHIFT: u32 = 25;
const SHADE_SHIFTS: [u32; 4] = [21, 17, 13, 9];
const TINT_SHIFT: u32 = 0;

const SHADE_MASK: u64 = 0xF;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GreedyFaceRecord {
    pub packed: u64,
}

impl GreedyFaceRecord {
    pub fn new(
        block_index: u16,
        direction: u8,
        width: u8,
        height: u8,
        global_id: u16,
        shades: [u8; 4],
        tint_index: u16,
    ) -> Self {
        assert!(u32::from(block_index) <= 0xFFF);
        assert!(direction <= 5);
        assert!(width >= 1 && width <= 16);
        assert!(height >= 1 && height <= 16);
        assert!(u32::from(tint_index) <= MAX_TINT_INDEX);
        let mut packed = u64::from(block_index) << BLOCK_SHIFT;
        packed |= u64::from(direction) << DIR_SHIFT;
        packed |= u64::from(width - 1) << WIDTH_SHIFT;
        packed |= u64::from(height - 1) << HEIGHT_SHIFT;
        packed |= u64::from(global_id) << GLOBAL_SHIFT;
        for (i, shade) in shades.into_iter().enumerate() {
            packed |= u64::from(shade.min(15)) << SHADE_SHIFTS[i];
        }
        packed |= u64::from(tint_index) << TINT_SHIFT;
        Self { packed }
    }

    pub fn fields(self) -> (u16, u8, u8, u8, u16, [u8; 4], u16) {
        (
            ((self.packed >> BLOCK_SHIFT) & 0xFFF) as u16,
            ((self.packed >> DIR_SHIFT) & 0x7) as u8,
            (((self.packed >> WIDTH_SHIFT) & 0xF) as u8) + 1,
            (((self.packed >> HEIGHT_SHIFT) & 0xF) as u8) + 1,
            ((self.packed >> GLOBAL_SHIFT) & 0xFFFF) as u16,
            std::array::from_fn(|i| ((self.packed >> SHADE_SHIFTS[i]) & SHADE_MASK) as u8),
            (self.packed & 0x1FF) as u16,
        )
    }
}

pub fn block_index(x: u8, y: u8, z: u8) -> u16 {
    u16::from(x) | (u16::from(z) << 4) | (u16::from(y) << 8)
}

pub const fn block_coords(index: u16) -> (u8, u8, u8) {
    (
        (index & 0xF) as u8,
        ((index >> 8) & 0xF) as u8,
        ((index >> 4) & 0xF) as u8,
    )
}

#[cfg(test)]
mod tests {
    use std::mem::size_of;

    use super::*;

    #[test]
    fn roundtrips_layout_a_fields() {
        let f = GreedyFaceRecord::new(0xabc, 3, 4, 2, 0x1234, [1, 2, 14, 15], 511);
        assert_eq!(
            f.fields(),
            (0xabc, 3, 4, 2, 0x1234, [1, 2, 14, 15], 511)
        );
        assert_eq!(size_of::<GreedyFaceRecord>(), 8);
    }

    #[test]
    fn block_index_roundtrips_xyz() {
        let (x, y, z) = block_coords(block_index(15, 7, 3));
        assert_eq!((x, y, z), (15, 7, 3));
    }
}
