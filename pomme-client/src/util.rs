//! Small shared utilities.

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

    /// Matches `Random.nextInt(int)`, in `[0, bound)`.
    pub fn next_int(&mut self, bound: i32) -> i32 {
        assert!(bound > 0);
        if bound & (bound - 1) == 0 {
            return ((bound as i64).wrapping_mul(self.next(31) as i64) >> 31) as i32;
        }
        loop {
            let bits = self.next(31);
            let val = bits % bound;
            // Java relies on int overflow here to reject biased samples.
            if bits.wrapping_sub(val).wrapping_add(bound - 1) >= 0 {
                return val;
            }
        }
    }
}
