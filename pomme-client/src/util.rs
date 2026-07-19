//! Small shared utilities.
use azalea_core::position::{ChunkPos, ChunkSectionPos};

pub const MAX_RD: u32 = 64;

pub const MAX_SIZE: usize = (MAX_RD * 2 + 1) as usize;
pub const SIZE_Y: usize = 32;
pub const CHUNK_RING_SIZE: usize = MAX_SIZE * MAX_SIZE;
pub const SECTION_RING_SIZE: usize = MAX_SIZE * MAX_SIZE * SIZE_Y;

/// Java `java.util.Random` (`LegacyRandomSource`) reimplementation: a 48-bit
/// LCG matching the JVM bit-for-bit so seeded sequences line up with vanilla.
///
/// TODO: `renderer/pipelines/{weather,sky}.rs` and `renderer/chunk/mesher.rs`
/// each carry a private copy of this; unify them onto this type.
pub struct JavaRandom {
    seed: u64,
}

impl JavaRandom {
    const MULTIPLIER: u64 = 0x5DEECE66D;
    const INCREMENT: u64 = 0xB;
    const MASK: u64 = (1 << 48) - 1;

    pub fn new(seed: i64) -> Self {
        let mut rng = Self { seed: 0 };
        rng.set_seed(seed);
        rng
    }

    /// Matches `Random.setSeed`: scrambles with the multiplier before use.
    pub fn set_seed(&mut self, seed: i64) {
        self.seed = (seed as u64 ^ Self::MULTIPLIER) & Self::MASK;
    }

    fn next(&mut self, bits: u32) -> i32 {
        self.seed = self
            .seed
            .wrapping_mul(Self::MULTIPLIER)
            .wrapping_add(Self::INCREMENT)
            & Self::MASK;
        (self.seed >> (48 - bits)) as i32
    }

    /// Matches `Random.nextFloat`: `next(24) / 2^24`, in `[0, 1)`.
    pub fn next_float(&mut self) -> f32 {
        self.next(24) as f32 / (1u32 << 24) as f32
    }
}

/// A ring buffer for chunk data, indexed by ChunkPos.
/// Uses a flattened 2D buffer of size SIZE x SIZE.
pub struct ChunkRing<T> {
    pub buf: Box<[T]>,
}

impl<T> ChunkRing<T> {
    /// Creates a new ChunkRing with all elements initialized to `init`.
    pub fn new(init: T) -> Self
    where
        T: Copy,
    {
        Self::from_fn(|_, _| init)
    }

    /// Creates a new ChunkRing using a function to initialize each element.
    /// The function receives (x, z) coordinates in the ring's local space.
    pub fn from_fn(mut init: impl FnMut(usize, usize) -> T) -> Self {
        let mut v = Vec::with_capacity(CHUNK_RING_SIZE);
        for x in 0..MAX_SIZE {
            for z in 0..MAX_SIZE {
                v.push(init(x, z));
            }
        }
        Self {
            buf: v.into_boxed_slice(),
        }
    }

    /// Gets a reference to the element at the given chunk position.
    #[inline]
    pub fn get(&self, pos: ChunkPos) -> &T {
        let x = pos.x.rem_euclid(MAX_SIZE as i32) as usize;
        let z = pos.z.rem_euclid(MAX_SIZE as i32) as usize;
        let idx = x * MAX_SIZE + z;
        &self.buf[idx]
    }

    /// `get`, but `None` when `pos` is outside the ring's addressable window
    /// around `center`. Slots carry no position tag and alias every MAX_SIZE,
    /// so reading beyond ±MAX_RD of the writer's center returns another
    /// position's slot.
    #[inline]
    pub fn get_in_range(&self, pos: ChunkPos, center: ChunkPos) -> Option<&T> {
        let in_range =
            (pos.x - center.x).abs() <= MAX_RD as i32 && (pos.z - center.z).abs() <= MAX_RD as i32;
        in_range.then(|| self.get(pos))
    }
}

/// A ring buffer for chunk section data, indexed by ChunkSectionPos.
/// Uses a flattened 3D buffer of size SIZE x SIZE x SIZE_Y.
pub struct SectionRing<T> {
    pub buf: Box<[T]>,
}

impl<T> SectionRing<T> {
    /// Creates a new SectionRing using a function to initialize each element.
    /// The function receives (x, z, y) coordinates in the ring's local space.
    pub fn from_fn(mut init: impl FnMut(usize, usize, usize) -> T) -> Self {
        let mut v = Vec::with_capacity(SECTION_RING_SIZE);
        for x in 0..MAX_SIZE {
            for z in 0..MAX_SIZE {
                for y in 0..SIZE_Y {
                    v.push(init(x, z, y));
                }
            }
        }
        Self {
            buf: v.into_boxed_slice(),
        }
    }

    /// Gets a reference to the element at the given chunk section position.
    #[inline]
    pub fn get(&self, pos: ChunkSectionPos) -> &T {
        let x = pos.x.rem_euclid(MAX_SIZE as i32) as usize;
        let z = pos.z.rem_euclid(MAX_SIZE as i32) as usize;
        let y = pos.y.rem_euclid(SIZE_Y as i32) as usize;
        let idx = (x * MAX_SIZE + z) * SIZE_Y + y;
        &self.buf[idx]
    }
}
