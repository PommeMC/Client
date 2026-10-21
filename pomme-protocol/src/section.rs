//! Chunk sections: vanilla `LevelChunkSection` over two `PalettedContainer`s
//! (block states and biomes), each packed in a `SimpleBitStorage`.

use crate::codec::{DecodeResult, Reader};

/// Vanilla `SimpleBitStorage.MAGIC`: per values-per-long, the multiplier,
/// addend and shift that turn an index into its cell without a division.
#[rustfmt::skip]
const MAGIC: [u32; 192] = [
    0xFFFFFFFF, 0xFFFFFFFF, 0, 0x80000000, 0, 0, 0x55555555, 0x55555555, 0, 0x80000000, 0, 1,
    0x33333333, 0x33333333, 0, 0x2AAAAAAA, 0x2AAAAAAA, 0, 0x24924924, 0x24924924, 0, 0x80000000, 0, 2,
    0x1C71C71C, 0x1C71C71C, 0, 0x19999999, 0x19999999, 0, 390451572, 390451572, 0, 0x15555555, 0x15555555, 0,
    0x13B13B13, 0x13B13B13, 0, 306783378, 306783378, 0, 0x11111111, 0x11111111, 0, 0x80000000, 0, 3,
    0x0F0F0F0F, 0x0F0F0F0F, 0, 0x0E38E38E, 0x0E38E38E, 0, 226050910, 226050910, 0, 0x0CCCCCCC, 0x0CCCCCCC, 0,
    0x0C30C30C, 0x0C30C30C, 0, 195225786, 195225786, 0, 186737708, 186737708, 0, 0x0AAAAAAA, 0x0AAAAAAA, 0,
    171798691, 171798691, 0, 0x09D89D89, 0x09D89D89, 0, 159072862, 159072862, 0, 0x09249249, 0x09249249, 0,
    148102320, 148102320, 0, 0x08888888, 0x08888888, 0, 138547332, 138547332, 0, 0x80000000, 0, 4,
    130150524, 130150524, 0, 0x07878787, 0x07878787, 0, 0x07507507, 0x07507507, 0, 0x071C71C7, 0x071C71C7, 0,
    116080197, 116080197, 0, 113025455, 113025455, 0, 0x06906906, 0x06906906, 0, 0x06666666, 0x06666666, 0,
    104755299, 104755299, 0, 0x06186186, 0x06186186, 0, 99882960, 99882960, 0, 97612893, 97612893, 0,
    0x05B05B05, 0x05B05B05, 0, 93368854, 93368854, 0, 91382282, 91382282, 0, 0x05555555, 0x05555555, 0,
    87652393, 87652393, 0, 85899345, 85899345, 0, 0x05050505, 0x05050505, 0, 0x04EC4EC4, 0x04EC4EC4, 0,
    81037118, 81037118, 0, 79536431, 79536431, 0, 78090314, 78090314, 0, 0x04924924, 0x04924924, 0,
    75350303, 75350303, 0, 74051160, 74051160, 0, 72796055, 72796055, 0, 0x04444444, 0x04444444, 0,
    70409299, 70409299, 0, 69273666, 69273666, 0, 0x04104104, 0x04104104, 0, 0x80000000, 0, 5,
];

/// Vanilla `SimpleBitStorage`, or `ZeroBitStorage` at zero bits: `size`
/// values of `bits` bits each, packed into longs without spanning a boundary.
#[derive(Clone, Debug, PartialEq)]
pub struct BitStorage {
    data: Box<[u64]>,
    bits: u32,
    mask: u64,
    values_per_long: u32,
    divide_mul: u64,
    divide_add: u64,
    divide_shift: u32,
}

impl BitStorage {
    pub fn new(bits: u32, size: usize) -> Self {
        if bits == 0 {
            return Self {
                data: Box::new([]),
                bits: 0,
                mask: 0,
                values_per_long: 0,
                divide_mul: 0,
                divide_add: 0,
                divide_shift: 0,
            };
        }
        debug_assert!(bits <= 32, "{bits} bits per value");
        let values_per_long = 64 / bits;
        let row = 3 * (values_per_long as usize - 1);
        Self {
            data: vec![0; size.div_ceil(values_per_long as usize)].into(),
            bits,
            mask: (1 << bits) - 1,
            values_per_long,
            divide_mul: u64::from(MAGIC[row]),
            divide_add: u64::from(MAGIC[row + 1]),
            divide_shift: MAGIC[row + 2],
        }
    }

    fn cell_index(&self, index: usize) -> usize {
        ((index as u64 * self.divide_mul + self.divide_add) >> 32 >> self.divide_shift) as usize
    }

    /// The cell holding `index` and the value's bit offset within it.
    fn locate(&self, index: usize) -> (usize, u32) {
        let cell = self.cell_index(index);
        let bit = (index - cell * self.values_per_long as usize) as u32 * self.bits;
        (cell, bit)
    }

    #[inline]
    pub fn get(&self, index: usize) -> u32 {
        if self.bits == 0 {
            return 0;
        }
        let (cell, bit) = self.locate(index);
        ((self.data[cell] >> bit) & self.mask) as u32
    }

    pub fn get_and_set(&mut self, index: usize, value: u32) -> u32 {
        if self.bits == 0 {
            return 0;
        }
        let (cell, bit) = self.locate(index);
        let old = self.data[cell];
        self.data[cell] = (old & !(self.mask << bit)) | ((u64::from(value) & self.mask) << bit);
        ((old >> bit) & self.mask) as u32
    }

    pub fn set(&mut self, index: usize, value: u32) {
        self.get_and_set(index, value);
    }

    /// The packed longs, sized for the bit count (`readFixedSizeLongArray`).
    pub fn raw(&self) -> &[u64] {
        &self.data
    }

    pub fn raw_mut(&mut self) -> &mut [u64] {
        &mut self.data
    }
}

/// Which values a container holds, fixing its size and palette choice
/// (vanilla `Strategy.createForBlockStates` / `createForBiomes`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strategy {
    BlockStates,
    Biomes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PaletteKind {
    SingleValue,
    List,
    Global,
}

impl Strategy {
    fn bits_per_axis(self) -> u32 {
        match self {
            Strategy::BlockStates => 4,
            Strategy::Biomes => 2,
        }
    }

    fn entry_count(self) -> usize {
        1 << (self.bits_per_axis() * 3)
    }

    /// Vanilla `getConfigurationForBitCount`: the palette for a bit count and
    /// the bits its storage uses. Global storage is as wide as the registry,
    /// whatever count the wire gave.
    fn configuration(self, bits: u32, registry_size: usize) -> (PaletteKind, u32) {
        match (self, bits) {
            (_, 0) => (PaletteKind::SingleValue, 0),
            (Strategy::BlockStates, 1..=4) => (PaletteKind::List, 4),
            (Strategy::BlockStates, 5..=8) | (Strategy::Biomes, 1..=3) => (PaletteKind::List, bits),
            _ => (PaletteKind::Global, ceil_log2(registry_size)),
        }
    }
}

/// Vanilla `Mth.ceillog2`: the bits that tell `n` values apart
/// (`globalPaletteBitsInMemory` for a registry of `n`).
pub fn ceil_log2(n: usize) -> u32 {
    n.next_power_of_two().trailing_zeros()
}

/// Vanilla `Palette`: maps a container's stored ids to registry ids. `List`
/// is both `LinearPalette` and `HashMapPalette`, which differ only in lookup.
#[derive(Clone, Debug, PartialEq)]
enum Palette {
    SingleValue(u32),
    List(Vec<u32>),
    Global,
}

impl Palette {
    fn empty(kind: PaletteKind) -> Self {
        match kind {
            PaletteKind::SingleValue => Palette::SingleValue(0),
            PaletteKind::List => Palette::List(Vec::new()),
            PaletteKind::Global => Palette::Global,
        }
    }

    fn read(kind: PaletteKind, r: &mut Reader) -> DecodeResult<Self> {
        Ok(match kind {
            PaletteKind::SingleValue => Palette::SingleValue(r.varint()?),
            PaletteKind::List => {
                let len = r.len(usize::MAX)?;
                Palette::List((0..len).map(|_| r.varint()).collect::<DecodeResult<_>>()?)
            }
            PaletteKind::Global => Palette::Global,
        })
    }

    /// A stored id's registry id. Ids past a list palette read as 0, where
    /// vanilla throws `MissingPaletteEntryException`.
    #[inline]
    fn value_for(&self, id: u32) -> u32 {
        match self {
            Palette::SingleValue(value) => *value,
            Palette::List(values) => values.get(id as usize).copied().unwrap_or_default(),
            Palette::Global => id,
        }
    }
}

/// Vanilla `PalettedContainer` of registry ids.
#[derive(Clone, Debug, PartialEq)]
pub struct PalettedContainer {
    strategy: Strategy,
    palette: Palette,
    storage: BitStorage,
}

impl PalettedContainer {
    /// An all-zero container (air, or the first biome).
    pub fn new(strategy: Strategy) -> Self {
        Self {
            strategy,
            palette: Palette::SingleValue(0),
            storage: BitStorage::new(0, strategy.entry_count()),
        }
    }

    /// Vanilla `PalettedContainer.read`: the bit count, the palette, then the
    /// storage's longs with no length prefix. `registry_size` is the size of
    /// the registry the ids come from.
    pub fn read(strategy: Strategy, r: &mut Reader, registry_size: usize) -> DecodeResult<Self> {
        let bits = u32::from(r.u8()?);
        let (kind, storage_bits) = strategy.configuration(bits, registry_size);
        let palette = Palette::read(kind, r)?;
        let mut storage = BitStorage::new(storage_bits, strategy.entry_count());
        for long in storage.raw_mut() {
            *long = r.i64()? as u64;
        }
        Ok(Self {
            strategy,
            palette,
            storage,
        })
    }

    /// Vanilla `PalettedContainer.Data.write`: the storage's bits in memory,
    /// the palette, then the longs.
    pub fn write(&self, out: &mut Vec<u8>) {
        use crate::wire::write_varint;

        out.push(self.storage.bits as u8);
        match &self.palette {
            Palette::SingleValue(value) => write_varint(out, *value),
            Palette::List(values) => {
                write_varint(out, values.len() as u32);
                for &value in values {
                    write_varint(out, value);
                }
            }
            Palette::Global => {}
        }
        for long in self.storage.raw() {
            out.extend_from_slice(&long.to_be_bytes());
        }
    }

    /// Vanilla `Strategy.getIndex`.
    #[inline]
    fn index(&self, x: u32, y: u32, z: u32) -> usize {
        let b = self.strategy.bits_per_axis();
        ((((y << b) | z) << b) | x) as usize
    }

    #[inline]
    pub fn get(&self, x: u32, y: u32, z: u32) -> u32 {
        self.palette
            .value_for(self.storage.get(self.index(x, y, z)))
    }

    /// The single value every entry holds, if the palette has only one.
    pub fn single_value(&self) -> Option<u32> {
        match self.palette {
            Palette::SingleValue(value) => Some(value),
            _ => None,
        }
    }

    /// Stores `value`, returning the one it replaced. `registry_size` sizes
    /// the global palette should the container outgrow its own.
    pub fn get_and_set(&mut self, x: u32, y: u32, z: u32, value: u32, registry_size: usize) -> u32 {
        let id = self.id_for(value, registry_size);
        let index = self.index(x, y, z);
        self.palette.value_for(self.storage.get_and_set(index, id))
    }

    /// Vanilla `Palette.idFor`, growing the container through `on_resize`
    /// when the palette is full. A list palette holds as many values as its
    /// storage bits can tell apart.
    fn id_for(&mut self, value: u32, registry_size: usize) -> u32 {
        let bits = self.storage.bits;
        match &mut self.palette {
            Palette::SingleValue(v) if *v == value => 0,
            Palette::SingleValue(_) => self.on_resize(1, value, registry_size),
            Palette::List(values) => {
                if let Some(i) = values.iter().position(|&v| v == value) {
                    return i as u32;
                }
                if values.len() < 1 << bits {
                    values.push(value);
                    (values.len() - 1) as u32
                } else {
                    self.on_resize(bits + 1, value, registry_size)
                }
            }
            Palette::Global => value,
        }
    }

    /// Vanilla `PalettedContainer.onResize`: rebuilds the container for
    /// `bits`, copies every entry over, then places `value`.
    fn on_resize(&mut self, bits: u32, value: u32, registry_size: usize) -> u32 {
        let strategy = self.strategy;
        let (kind, storage_bits) = strategy.configuration(bits, registry_size);
        let mut resized = Self {
            strategy,
            palette: Palette::empty(kind),
            storage: BitStorage::new(storage_bits, strategy.entry_count()),
        };
        for i in 0..strategy.entry_count() {
            let entry = self.palette.value_for(self.storage.get(i));
            let id = resized.id_for(entry, registry_size);
            resized.storage.set(i, id);
        }
        *self = resized;
        self.id_for(value, registry_size)
    }
}

/// Vanilla `LevelChunkSection`, holding what the client reads of it: the
/// block counts it was sent, block-state ids and biome ids.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub block_count: u16,
    pub fluid_count: u16,
    pub states: PalettedContainer,
    pub biomes: PalettedContainer,
}

impl Default for Section {
    fn default() -> Self {
        Self {
            block_count: 0,
            fluid_count: 0,
            states: PalettedContainer::new(Strategy::BlockStates),
            biomes: PalettedContainer::new(Strategy::Biomes),
        }
    }
}

impl Section {
    /// Vanilla `LevelChunkSection.read`, given how many block states and
    /// biomes the ids can name.
    pub fn read(r: &mut Reader, block_states: usize, biomes: usize) -> DecodeResult<Self> {
        let block_count = r.i16()? as u16;
        let fluid_count = r.i16()? as u16;
        let states = PalettedContainer::read(Strategy::BlockStates, r, block_states)?;
        let biomes = PalettedContainer::read(Strategy::Biomes, r, biomes)?;
        Ok(Self {
            block_count,
            fluid_count,
            states,
            biomes,
        })
    }

    /// Vanilla `LevelChunkSection.write`.
    pub fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.block_count.to_be_bytes());
        out.extend_from_slice(&self.fluid_count.to_be_bytes());
        self.states.write(out);
        self.biomes.write(out);
    }

    /// The biome registry id at biome-cell coordinates (4³ cells).
    pub fn biome(&self, x: u32, y: u32, z: u32) -> u32 {
        self.biomes.get(x, y, z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_storage_round_trips_without_spanning_longs() {
        for bits in 1..=32 {
            let mut storage = BitStorage::new(bits, 4096);
            let mask = ((1u64 << bits) - 1) as u32;
            for i in 0..4096 {
                storage.set(i, (i as u32).wrapping_mul(2654435761) & mask);
            }
            for i in 0..4096 {
                assert_eq!(
                    storage.get(i),
                    (i as u32).wrapping_mul(2654435761) & mask,
                    "{bits} bits, index {i}"
                );
            }
            assert_eq!(storage.data.len(), 4096usize.div_ceil((64 / bits) as usize));
        }
    }

    fn container_bytes(bits: u8, palette: &[u32], longs: &[u64]) -> Vec<u8> {
        let mut out = vec![bits];
        if bits == 0 {
            crate::wire::write_varint(&mut out, palette[0]);
        } else if !palette.is_empty() {
            crate::wire::write_varint(&mut out, palette.len() as u32);
            for &v in palette {
                crate::wire::write_varint(&mut out, v);
            }
        }
        for long in longs {
            out.extend_from_slice(&long.to_be_bytes());
        }
        out
    }

    use crate::vanilla_fixtures::registry_size;

    fn block_states() -> usize {
        registry_size("block_state")
    }

    fn biomes() -> usize {
        registry_size("worldgen/biome")
    }

    #[test]
    fn reads_a_linear_block_container_at_four_bits() {
        // Entry 0 -> palette[1], entry 1 -> palette[2], the rest palette[0].
        let mut longs = vec![0u64; 256];
        longs[0] = 0x21;
        let bytes = container_bytes(4, &[0, 1, 9], &longs);
        let mut r = Reader::new(&bytes);
        let container =
            PalettedContainer::read(Strategy::BlockStates, &mut r, block_states()).unwrap();
        assert_eq!(r.remaining(), 0);
        assert_eq!(container.get(0, 0, 0), 1);
        assert_eq!(container.get(1, 0, 0), 9);
        assert_eq!(container.get(2, 0, 0), 0);
    }

    #[test]
    fn reads_a_single_value_container_with_no_longs() {
        let bytes = container_bytes(0, &[7], &[]);
        let mut r = Reader::new(&bytes);
        let container = PalettedContainer::read(Strategy::Biomes, &mut r, biomes()).unwrap();
        assert_eq!(r.remaining(), 0);
        assert_eq!(container.single_value(), Some(7));
        assert_eq!(container.get(3, 3, 3), 7);
    }

    #[test]
    fn global_storage_is_as_wide_as_the_registry() {
        // A hostile count: vanilla falls through to the registry's width, so
        // this reads 15-bit storage and runs out of bytes instead of panicking.
        let bytes = container_bytes(0xFF, &[], &[0; 8]);
        assert!(
            PalettedContainer::read(
                Strategy::BlockStates,
                &mut Reader::new(&bytes),
                block_states()
            )
            .is_err()
        );

        // 9 bits for the 66 biomes still reads 7-bit storage: 9 per long, 8 longs.
        let bytes = container_bytes(9, &[], &[0; 8]);
        let mut r = Reader::new(&bytes);
        let container = PalettedContainer::read(Strategy::Biomes, &mut r, biomes()).unwrap();
        assert_eq!(r.remaining(), 0);
        assert_eq!(container.storage.bits, 7);
    }

    #[test]
    fn list_palettes_grow_from_four_bits() {
        let mut container = PalettedContainer::new(Strategy::BlockStates);
        container.get_and_set(0, 0, 0, 1, block_states());
        assert_eq!(container.storage.bits, 4);
        for i in 1..16 {
            container.get_and_set(i, 0, 0, i + 1, block_states());
        }
        assert_eq!(container.storage.bits, 5);
    }

    #[test]
    fn setting_grows_the_palette_and_keeps_every_entry() {
        let mut container = PalettedContainer::new(Strategy::BlockStates);
        // 300 distinct values pass the hashmap palette and land in global.
        for i in 0..300u32 {
            let (x, y, z) = (i % 16, i / 256, (i / 16) % 16);
            container.get_and_set(x, y, z, i + 1, block_states());
        }
        assert_eq!(container.palette, Palette::Global);
        assert_eq!(container.storage.bits, 15);
        for i in 0..300u32 {
            let (x, y, z) = (i % 16, i / 256, (i / 16) % 16);
            assert_eq!(container.get(x, y, z), i + 1);
        }
        assert_eq!(container.get(15, 15, 15), 0);
    }

    /// Sections vanilla's `LevelChunkSection.write` produced, one per palette
    /// kind (`CodecOracle.sections`), read back entry by entry and written
    /// out byte for byte.
    #[test]
    fn matches_vanilla_encoder() {
        for (name, distinct_states, distinct_biomes) in [
            ("empty", 0, 0),
            ("linear", 3, 3),
            ("hashmap", 20, 1),
            ("global", 300, 20),
        ] {
            let bytes = crate::vanilla_fixtures::case(&format!("chunk_section/{name}"));
            let mut r = Reader::new(&bytes);
            let section = Section::read(&mut r, block_states(), biomes()).unwrap();
            assert_eq!(r.remaining(), 0, "{name}");

            let expected_count = if distinct_states == 0 { 0 } else { 4096 };
            assert_eq!(section.block_count, expected_count, "{name}");
            for i in 0..4096u32 {
                let expected = if distinct_states == 0 {
                    0
                } else {
                    1 + i % distinct_states
                };
                let state = section.states.get(i & 15, i >> 8, i >> 4 & 15);
                assert_eq!(state, expected, "{name} block {i}");
            }
            if distinct_biomes > 0 {
                for i in 0..64u32 {
                    let biome = section.biome(i & 3, i >> 4, i >> 2 & 3);
                    assert_eq!(biome, i % distinct_biomes, "{name} biome {i}");
                }
            } else {
                assert!(section.biomes.single_value().is_some(), "{name}");
            }

            let mut written = Vec::new();
            section.write(&mut written);
            assert_eq!(written, bytes, "{name}");
        }
    }
}
