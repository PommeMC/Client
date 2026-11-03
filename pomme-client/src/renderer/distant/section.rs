//! Distant terrain's world sections: 32x32x32 voxels at each of five levels
//! of detail, what a vanilla section becomes at every level before it's
//! written in, and a section's file.

use std::io::{Read, Write};

use super::voxel::{self, States};
use crate::world::chunk::ChunkLightData;

/// Levels of detail: at level `l` a voxel is `1 << l` blocks wide.
pub(crate) const LEVELS: u8 = 5;
pub(crate) const SIDE: usize = 32;
pub(crate) const VOLUME: usize = SIDE * SIDE * SIDE;

/// A section at one level, in section coordinates of that level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct SectionKey {
    pub level: u8,
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl SectionKey {
    /// The level-`level` section holding vanilla section `x`/`y`/`z`.
    pub fn of_vanilla(level: u8, x: i32, y: i32, z: i32) -> Self {
        let shift = level + 1;
        Self {
            level,
            x: x >> shift,
            y: y >> shift,
            z: z >> shift,
        }
    }

    /// The key as one word: level in the top nibble, then y (8 bits), z and
    /// x (24 each), 4 bits spare.
    pub fn pack(self) -> u64 {
        u64::from(self.level) << 60
            | (self.y as u64 & 0xFF) << 52
            | (self.z as u64 & 0xFF_FFFF) << 28
            | (self.x as u64 & 0xFF_FFFF) << 4
    }

    pub fn unpack(id: u64) -> Self {
        Self {
            level: (id >> 60) as u8,
            x: ((id << 36) as i64 >> 40) as i32,
            y: ((id << 4) as i64 >> 56) as i32,
            z: ((id << 12) as i64 >> 40) as i32,
        }
    }

    /// The level-`level` section this one lies in.
    pub fn ancestor(self, level: u8) -> Self {
        let shift = level - self.level;
        Self {
            level,
            x: self.x >> shift,
            y: self.y >> shift,
            z: self.z >> shift,
        }
    }

    /// Blocks along a side.
    pub fn width(self) -> i32 {
        (SIDE as i32) << self.level
    }

    /// The block position of the section's corner.
    pub fn origin(self) -> [i32; 3] {
        [self.x, self.y, self.z].map(|c| c * self.width())
    }

    /// Which of its parent's eight children this is.
    pub fn child_index(self) -> u8 {
        (self.x & 1 | (self.z & 1) << 1 | (self.y & 1) << 2) as u8
    }

    /// The horizontal distance from `x`/`z` to the section's nearest edge.
    pub fn distance(self, x: f64, z: f64) -> f64 {
        let [ox, _, oz] = self.origin();
        let width = f64::from(self.width());
        let dx = (f64::from(ox) - x).max(x - f64::from(ox) - width).max(0.0);
        let dz = (f64::from(oz) - z).max(z - f64::from(oz) - width).max(0.0);
        dx.hypot(dz)
    }
}

/// Air under full sky, which an unseen section is made of.
pub(crate) const UNSEEN: u64 = voxel::compose(0x0F, voxel::AIR, 0);

/// A voxel's index: `y`, then `z`, then `x`.
pub(crate) fn index(x: usize, y: usize, z: usize) -> usize {
    y << 10 | z << 5 | x
}

/// 32³ voxels at one level.
pub(crate) struct WorldSection {
    pub data: Box<[u64]>,
    /// A bit per child octant with anything in it; at level 0, 0xFF if any
    /// voxel is a block.
    pub non_empty_children: u8,
    /// Level 0 only: how many voxels are blocks.
    pub non_empty_count: u32,
}

impl WorldSection {
    /// What a section nothing was seen in holds: air under full sky.
    pub fn unseen() -> Self {
        Self {
            data: vec![UNSEEN; VOLUME].into(),
            non_empty_children: 0,
            non_empty_count: 0,
        }
    }

    /// Whether it still holds only that, so a file for it says nothing.
    pub fn is_unseen(&self) -> bool {
        self.non_empty_children == 0 && self.data.iter().all(|&v| v == UNSEEN)
    }

    /// Level 0's `non_empty_children` from its count; whether it changed.
    fn update_lvl0_state(&mut self) -> bool {
        let next = if self.non_empty_count == 0 { 0 } else { 0xFF };
        std::mem::replace(&mut self.non_empty_children, next) != next
    }

    /// Records whether child `index` has anything in it: 0 for no change,
    /// 1 for a change, 2 for this section going empty or non-empty.
    fn update_empty_child_state(&mut self, index: u8, child_non_empty: bool) -> u8 {
        let mask = 1 << index;
        let prev = self.non_empty_children;
        let next = prev & !mask | if child_non_empty { mask } else { 0 };
        self.non_empty_children = next;
        if (prev != 0) != (next != 0) {
            2
        } else {
            u8::from(prev != next)
        }
    }
}

/// Sides of the five levels of a vanilla section: 16, 8, 4, 2, 1.
const fn level_side(level: u8) -> usize {
    1 << level_shift(level)
}

/// `log2` of [`level_side`].
const fn level_shift(level: u8) -> u32 {
    4 - level as u32
}

/// Voxel `i` of a cube `1 << shift` wide laid out `y`-major, then `z`, then
/// `x`.
fn position(i: usize, shift: u32) -> (usize, usize, usize) {
    let mask = (1 << shift) - 1;
    (i & mask, i >> (2 * shift), i >> shift & mask)
}

const fn level_base(level: u8) -> usize {
    let mut base = 0;
    let mut l = 0;
    while l < level {
        base += level_side(l).pow(3);
        l += 1;
    }
    base
}

const WORK_VOLUME: usize = level_base(LEVELS);

/// A vanilla 16³ section as voxels at every level, mipped down to one.
pub(crate) struct VoxelizedSection {
    /// Vanilla section coordinates.
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub non_air_count: u32,
    data: Box<[u64; WORK_VOLUME]>,
}

impl VoxelizedSection {
    pub fn new(x: i32, y: i32, z: i32) -> Self {
        Self {
            x,
            y,
            z,
            non_air_count: 0,
            data: Box::new([0; WORK_VOLUME]),
        }
    }

    /// The level's voxels, `side³` of them, `y`-major then `z` then `x`.
    pub fn level(&self, level: u8) -> &[u64] {
        let base = level_base(level);
        &self.data[base..base + level_side(level).pow(3)]
    }

    /// `section`'s blocks, biomes and light as level 0; `light` gives a
    /// block's `block << 4 | sky`.
    pub fn convert(
        &mut self,
        states: &States,
        section: &pomme_protocol::section::Section,
        light: impl Fn(usize, usize, usize) -> u8,
    ) {
        let biomes: [u32; 64] = std::array::from_fn(|i| {
            let (x, y, z) = position(i, 2);
            section.biome(x as u32, y as u32, z as u32) & 0x1FF
        });
        self.non_air_count = 0;
        for i in 0..level_side(0).pow(3) {
            let (x, y, z) = position(i, level_shift(0));
            let block = states.block(section.states.get(x as u32, y as u32, z as u32));
            self.non_air_count += u32::from(block != voxel::AIR);
            let biome = biomes[(y >> 2) << 4 | (z >> 2) << 2 | x >> 2];
            self.data[i] = voxel::compose(light(x, y, z), block, biome);
        }
    }

    /// Air, unlit.
    pub fn zero(&mut self) {
        self.non_air_count = 0;
        self.data.fill(0);
    }

    /// Level 0 all `voxel`, mipped as if every level were too.
    #[cfg(test)]
    pub fn fill_for_test(&mut self, voxel: u64, non_air_count: u32) {
        self.data.fill(voxel);
        self.non_air_count = non_air_count;
    }

    /// Each level from the one below, eight voxels to one.
    pub fn mip(&mut self, states: &States) {
        for level in 1..LEVELS {
            let (from, to) = (level_base(level - 1), level_base(level));
            let finer = level_side(level - 1);
            for i in 0..level_side(level).pow(3) {
                let (x, y, z) = position(i, level_shift(level));
                let child = |dx: usize, dy: usize, dz: usize| {
                    self.data
                        [from + (y * 2 + dy) * finer * finer + (z * 2 + dz) * finer + x * 2 + dx]
                };
                self.data[to + i] = states.mip(
                    std::array::from_fn(|c| child(c >> 2, c >> 1 & 1, c & 1)),
                    level,
                );
            }
        }
    }
}

/// A column's light at vanilla section `section_y`: `block << 4 | sky`, 0
/// without data.
pub(crate) fn light_of(
    light: Option<&ChunkLightData>,
    section_y: i32,
) -> impl Fn(usize, usize, usize) -> u8 + '_ {
    move |x, y, z| {
        let Some(light) = light else {
            return 0;
        };
        let (x, y, z) = (x as i32, section_y * 16 + y as i32, z as i32);
        light.get_block_light(x, y, z) << 4 | light.get_sky_light(x, y, z)
    }
}

/// Writes `section` over `into` where the vanilla section lands at the
/// section's level: whether anything changed, and how many blocks the
/// covered voxels held before.
pub(crate) fn insert_level(
    section: &VoxelizedSection,
    into: &mut WorldSection,
    level: u8,
) -> (bool, u32) {
    let mask = (1 << (level + 1)) - 1;
    let base = |c: i32| ((c & mask) << level_shift(level)) as usize;
    let (bx, by, bz) = (base(section.x), base(section.y), base(section.z));
    let (mut changed, mut non_air_before) = (false, 0);
    for (i, &new) in section.level(level).iter().enumerate() {
        let (x, y, z) = position(i, level_shift(level));
        let slot = &mut into.data[index(bx + x, by + y, bz + z)];
        non_air_before += u32::from(!voxel::is_air(*slot));
        changed |= *slot != new;
        *slot = new;
    }
    (changed, non_air_before)
}

/// The per-level bookkeeping after [`insert_level`]: level 0's count and
/// emptiness; whether the section changed and whether its emptiness flipped
/// (bubbled to the parent).
pub(crate) fn settle_level(
    into: &mut WorldSection,
    level: u8,
    changed: bool,
    non_air_before: u32,
    non_air_now: u32,
    child: Option<(u8, bool)>,
) -> (bool, u8) {
    let mut emptiness = match child {
        Some((index, non_empty)) if level != 0 => into.update_empty_child_state(index, non_empty),
        _ => 0,
    };
    if level == 0 && non_air_now != non_air_before {
        into.non_empty_count = into.non_empty_count + non_air_now - non_air_before;
        emptiness = if into.update_lvl0_state() { 2 } else { 0 };
    }
    (changed || emptiness != 0, emptiness)
}

/// A section's file: the key, a palette's size and the non-empty children,
/// a `u16` palette index per voxel, then the palette; little-endian,
/// zlib-compressed.
pub(crate) fn encode(key: SectionKey, section: &WorldSection) -> Vec<u8> {
    let mut palette: Vec<u64> = Vec::new();
    let mut lookup = std::collections::HashMap::new();
    let mut indices = Vec::with_capacity(VOLUME * 2);
    for &voxel in &section.data {
        let next = palette.len();
        let index = *lookup.entry(voxel).or_insert_with(|| {
            palette.push(voxel);
            next
        });
        indices.extend((index as u16).to_le_bytes());
    }
    let meta = palette.len() as u64 | u64::from(section.non_empty_children) << 16;
    let mut out = Vec::with_capacity(16 + indices.len() + palette.len() * 8);
    out.extend(key.pack().to_le_bytes());
    out.extend(meta.to_le_bytes());
    out.extend(indices);
    for voxel in palette {
        out.extend(voxel.to_le_bytes());
    }
    let mut zlib = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    zlib.write_all(&out)
        .and_then(|()| zlib.finish())
        .expect("compressing into memory")
}

/// The section [`encode`] wrote for `key`; `None` for another's or a
/// damaged file.
pub(crate) fn decode(key: SectionKey, file: &[u8]) -> Option<WorldSection> {
    let mut bytes = Vec::new();
    flate2::read::ZlibDecoder::new(file)
        .read_to_end(&mut bytes)
        .ok()?;
    let mut rest = bytes.as_slice();
    if u64::from_le_bytes(take(&mut rest)?) != key.pack() {
        return None;
    }
    let meta = u64::from_le_bytes(take(&mut rest)?);
    let palette_len = (meta & 0xFFFF) as usize;
    let (indices, after) = rest.split_at_checked(VOLUME * 2)?;
    let palette: Vec<u64> = after
        .as_chunks::<8>()
        .0
        .iter()
        .take(palette_len)
        .map(|&bytes| u64::from_le_bytes(bytes))
        .collect();
    if palette.len() != palette_len {
        return None;
    }
    let data: Option<Box<[u64]>> = indices
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&bytes| palette.get(u16::from_le_bytes(bytes) as usize).copied())
        .collect();
    let data = data?;
    let non_empty_count = if key.level == 0 {
        data.iter().filter(|&&v| !voxel::is_air(v)).count() as u32
    } else {
        0
    };
    Some(WorldSection {
        data,
        non_empty_children: (meta >> 16) as u8,
        non_empty_count,
    })
}

/// The next `N` bytes of `rest`, taking them off it.
fn take<const N: usize>(rest: &mut &[u8]) -> Option<[u8; N]> {
    let (taken, after) = rest.split_first_chunk()?;
    *rest = after;
    Some(*taken)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::block::BlockStateTable;

    #[test]
    fn keys_pack_and_find_their_parents() {
        for key in [
            SectionKey {
                level: 0,
                x: 0,
                y: 0,
                z: 0,
            },
            SectionKey {
                level: 4,
                x: -1,
                y: -2,
                z: 8_000_000,
            },
            SectionKey {
                level: 2,
                x: 123_456,
                y: 127,
                z: -654_321,
            },
        ] {
            assert_eq!(SectionKey::unpack(key.pack()), key);
        }
        let vanilla = SectionKey::of_vanilla(0, -1, 3, 33);
        assert_eq!(
            vanilla,
            SectionKey {
                level: 0,
                x: -1,
                y: 1,
                z: 16
            }
        );
        assert_eq!(
            SectionKey::of_vanilla(4, -1, 3, 33),
            SectionKey {
                level: 4,
                x: -1,
                y: 0,
                z: 1
            }
        );
        assert_eq!(vanilla.child_index(), 1 | 1 << 2, "x odd, z even, y odd");
        assert_eq!(
            SectionKey {
                level: 1,
                x: 1,
                y: 0,
                z: 0
            }
            .origin(),
            [64, 0, 0]
        );
    }

    fn states() -> States {
        States::new(BlockStateTable::native())
    }

    fn state_id(state: &str) -> u32 {
        u32::from(BlockStateTable::native().find_state(state, &[]).id())
    }

    /// A vanilla section of `state` everywhere, lit `light`.
    fn uniform(state: &str, light: u8) -> VoxelizedSection {
        let table = BlockStateTable::native();
        let mut section = pomme_protocol::section::Section::default();
        let id = state_id(state);
        for i in 0..4096u32 {
            section
                .states
                .get_and_set(i & 15, i >> 8, i >> 4 & 15, id, table.state_count());
        }
        let mut voxelized = VoxelizedSection::new(3, -2, 5);
        voxelized.convert(&states(), &section, |_, _, _| light);
        voxelized
    }

    #[test]
    fn a_section_converts_and_mips_down_to_one_voxel() {
        let mut stone = uniform("stone", 0x0F);
        assert_eq!(stone.non_air_count, 4096);
        stone.mip(&states());
        for level in 0..LEVELS {
            let voxels = stone.level(level);
            assert_eq!(voxels.len(), 4096 >> (3 * level));
            assert!(
                voxels
                    .iter()
                    .all(|&v| voxel::block_id(v) == state_id("stone"))
            );
        }
        let mut air = uniform("cave_air", 0x3F);
        assert_eq!(air.non_air_count, 0);
        air.mip(&states());
        assert!(
            air.level(4)
                .iter()
                .all(|&v| voxel::is_air(v) && voxel::light(v) == 0x3F)
        );
    }

    #[test]
    fn insertion_lands_each_level_in_its_sub_cube() {
        let mut stone = uniform("stone", 0);
        stone.mip(&states());
        // Where vanilla section (3, -2, 5)'s sub-cube starts at each level.
        let corners = [
            [16, 0, 16],
            [24, 16, 8],
            [12, 24, 20],
            [6, 28, 10],
            [3, 30, 5],
        ];
        for (level, [x, y, z]) in (0..LEVELS).zip(corners) {
            let mut into = WorldSection::unseen();
            let (changed, before) = insert_level(&stone, &mut into, level);
            assert!(changed);
            assert_eq!(before, 0);
            let side = level_side(level);
            let filled = into.data.iter().filter(|&&v| !voxel::is_air(v)).count();
            assert_eq!(filled, side.pow(3));
            assert!(!voxel::is_air(into.data[index(x, y, z)]));
            assert!(voxel::is_air(into.data[index(x + side, y, z)]));
            let (changed, before) = insert_level(&stone, &mut into, level);
            assert!(!changed);
            assert_eq!(before, side.pow(3) as u32);
        }
    }

    #[test]
    fn emptiness_bubbles_up() {
        let mut level0 = WorldSection::unseen();
        assert_eq!(settle_level(&mut level0, 0, true, 0, 10, None), (true, 2));
        assert_eq!(level0.non_empty_children, 0xFF);
        assert_eq!(settle_level(&mut level0, 0, true, 10, 4, None), (true, 0));
        let mut parent = WorldSection::unseen();
        assert_eq!(
            settle_level(&mut parent, 1, false, 0, 0, Some((5, true))),
            (true, 2)
        );
        assert_eq!(parent.non_empty_children, 1 << 5);
        assert_eq!(
            settle_level(&mut parent, 1, false, 0, 0, Some((2, true))),
            (true, 1)
        );
        assert_eq!(
            settle_level(&mut parent, 1, false, 0, 0, Some((2, true))),
            (false, 0)
        );
    }

    #[test]
    fn sections_survive_a_file() {
        let key = SectionKey {
            level: 0,
            x: -5,
            y: 2,
            z: 9,
        };
        let mut section = WorldSection::unseen();
        section.data[index(1, 2, 3)] = voxel::compose(0x12, 77, 3);
        section.data[index(31, 31, 31)] = voxel::compose(0, 78, 3);
        section.non_empty_count = 2;
        section.non_empty_children = 0xFF;
        let read = decode(key, &encode(key, &section)).unwrap();
        assert_eq!(read.data, section.data);
        assert_eq!(read.non_empty_children, 0xFF);
        assert_eq!(read.non_empty_count, 2);
        assert!(decode(SectionKey { level: 1, ..key }, &encode(key, &section)).is_none());
        assert!(decode(key, b"not a section").is_none());
    }
}
