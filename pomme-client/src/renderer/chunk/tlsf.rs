//! A two-level segregated fit (TLSF) allocator over a range of `u32`
//! elements: a good fit in constant time from per-size-class free lists, and
//! freed blocks merged with free neighbours so the range doesn't fragment.

use std::collections::{BTreeMap, BTreeSet};

/// Second-level classes per power of two.
const SL_BITS: u32 = 4;
const SL_COUNT: u32 = 1 << SL_BITS;
/// Sizes below this are classed one per size, in first level 0.
const SMALL: u32 = SL_COUNT;
/// First level 0 plus one per power of two from `SMALL` up to `u32::MAX`'s.
const FL_COUNT: usize = (32 - SL_BITS) as usize + 1;

/// The `(first, second)` level class holding blocks of `size`.
fn class(size: u32) -> (usize, usize) {
    if size < SMALL {
        return (0, size as usize);
    }
    let log = 31 - size.leading_zeros();
    let fl = (log - SL_BITS + 1) as usize;
    let sl = ((size >> (log - SL_BITS)) ^ SL_COUNT) as usize;
    (fl, sl)
}

/// The class whose every block holds `size`: `size` rounded up to the next
/// class boundary.
fn fitting_class(size: u32) -> (usize, usize) {
    if size < SMALL {
        return class(size);
    }
    let log = 31 - size.leading_zeros();
    let round = (1u32 << (log - SL_BITS)) - 1;
    class(size.saturating_add(round))
}

pub struct Tlsf {
    capacity: u32,
    /// Every block by offset: its length and whether it's free.
    blocks: BTreeMap<u32, (u32, bool)>,
    /// Free block offsets per class.
    free: Vec<[BTreeSet<u32>; SL_COUNT as usize]>,
    /// Bit `fl` set when first level `fl` has a free block.
    fl_bitmap: u64,
    /// Per first level, bit `sl` set when that class has a free block.
    sl_bitmap: [u32; FL_COUNT],
    used: u32,
}

impl Tlsf {
    pub fn new(capacity: u32) -> Self {
        let mut this = Self {
            capacity,
            blocks: BTreeMap::new(),
            free: (0..FL_COUNT).map(|_| Default::default()).collect(),
            fl_bitmap: 0,
            sl_bitmap: [0; FL_COUNT],
            used: 0,
        };
        this.reset();
        this
    }

    /// Frees everything.
    pub fn reset(&mut self) {
        self.blocks.clear();
        for level in &mut self.free {
            level.iter_mut().for_each(BTreeSet::clear);
        }
        self.fl_bitmap = 0;
        self.sl_bitmap = [0; FL_COUNT];
        self.used = 0;
        if self.capacity > 0 {
            self.insert_free(0, self.capacity);
        }
    }

    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    /// Elements in allocated blocks.
    pub fn used(&self) -> u32 {
        self.used
    }

    /// `n` contiguous elements, or `None` if no free block holds them.
    pub fn alloc(&mut self, n: u32) -> Option<u32> {
        let (offset, len) = self.find(n)?;
        self.remove_free(offset, len);
        self.blocks.insert(offset, (n, false));
        if len > n {
            self.insert_free(offset + n, len - n);
        }
        self.used += n;
        Some(offset)
    }

    /// Whether [`Self::alloc`] would find `n` elements.
    pub fn fits(&self, n: u32) -> bool {
        self.find(n).is_some()
    }

    /// The free block `alloc(n)` takes, as `(offset, len)`.
    fn find(&self, n: u32) -> Option<(u32, u32)> {
        if n == 0 || n > self.capacity {
            return None;
        }
        self.find_class(fitting_class(n))
            .and_then(|(fl, sl)| self.free[fl][sl].first().map(|&off| (off, self.len(off))))
            // Rounding up can pass over a block of `n`'s own class that fits.
            .or_else(|| {
                let (fl, sl) = class(n);
                self.free[fl][sl]
                    .iter()
                    .map(|&off| (off, self.len(off)))
                    .find(|&(_, len)| len >= n)
            })
    }

    /// Returns the block at `offset`, merging it with free neighbours.
    pub fn free(&mut self, offset: u32) {
        let Some(&(len, false)) = self.blocks.get(&offset) else {
            debug_assert!(false, "freeing {offset}, which isn't an allocated block");
            return;
        };
        self.blocks.remove(&offset);
        self.used -= len;
        let (mut start, mut len) = (offset, len);
        if let Some(&(next_len, true)) = self.blocks.get(&(start + len)) {
            self.remove_free(start + len, next_len);
            len += next_len;
        }
        if let Some((&prev, &(prev_len, true))) = self.blocks.range(..start).next_back()
            && prev + prev_len == start
        {
            self.remove_free(prev, prev_len);
            start = prev;
            len += prev_len;
        }
        self.insert_free(start, len);
    }

    /// The length of the block at `offset`.
    fn len(&self, offset: u32) -> u32 {
        self.blocks[&offset].0
    }

    /// The first non-empty class at or above `(fl, sl)`.
    fn find_class(&self, (fl, sl): (usize, usize)) -> Option<(usize, usize)> {
        let in_level = self.sl_bitmap[fl] & (u32::MAX << sl);
        if in_level != 0 {
            return Some((fl, in_level.trailing_zeros() as usize));
        }
        let above = self.fl_bitmap & u64::MAX.checked_shl(fl as u32 + 1).unwrap_or(0);
        if above == 0 {
            return None;
        }
        let fl = above.trailing_zeros() as usize;
        Some((fl, self.sl_bitmap[fl].trailing_zeros() as usize))
    }

    fn insert_free(&mut self, offset: u32, len: u32) {
        self.blocks.insert(offset, (len, true));
        let (fl, sl) = class(len);
        self.free[fl][sl].insert(offset);
        self.sl_bitmap[fl] |= 1 << sl;
        self.fl_bitmap |= 1 << fl;
    }

    fn remove_free(&mut self, offset: u32, len: u32) {
        self.blocks.remove(&offset);
        let (fl, sl) = class(len);
        self.free[fl][sl].remove(&offset);
        if self.free[fl][sl].is_empty() {
            self.sl_bitmap[fl] &= !(1 << sl);
            if self.sl_bitmap[fl] == 0 {
                self.fl_bitmap &= !(1 << fl);
            }
        }
    }

    /// Free blocks as `(offset, len)`, by offset.
    #[cfg(any(test, feature = "test-harness"))]
    pub fn free_blocks(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.blocks
            .iter()
            .filter(|(_, (_, free))| *free)
            .map(|(&off, &(len, _))| (off, len))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_split_each_power_of_two_sixteen_ways() {
        assert_eq!(class(5), (0, 5));
        assert_eq!(class(16), (1, 0));
        assert_eq!(class(31), (1, 15));
        assert_eq!(class(32), (2, 0));
        assert_eq!(class(33), (2, 0));
        assert_eq!(class(34), (2, 1));
        // Every block in the fitting class holds the request.
        assert_eq!(fitting_class(33), (2, 1));
        assert_eq!(fitting_class(32), (2, 0));
        assert_eq!(class(u32::MAX).0, FL_COUNT - 1);
    }

    #[test]
    fn freed_blocks_merge_with_free_neighbours() {
        let mut tlsf = Tlsf::new(100);
        let a = tlsf.alloc(10).unwrap();
        let b = tlsf.alloc(20).unwrap();
        let c = tlsf.alloc(30).unwrap();
        assert_eq!((a, b, c), (0, 10, 30));
        assert_eq!(tlsf.used(), 60);
        tlsf.free(b);
        tlsf.free(a);
        assert_eq!(tlsf.free_blocks().collect::<Vec<_>>(), [(0, 30), (60, 40)]);
        tlsf.free(c);
        assert_eq!(tlsf.free_blocks().collect::<Vec<_>>(), [(0, 100)]);
        assert_eq!(tlsf.used(), 0);
    }

    #[test]
    fn a_block_of_the_requests_own_class_is_found() {
        let mut tlsf = Tlsf::new(33);
        // The only free block, 33, shares 33's class rather than rounding up.
        assert_eq!(tlsf.alloc(33), Some(0));
        assert_eq!(tlsf.alloc(1), None);
    }

    /// Random allocs and frees never overlap and always succeed while some
    /// free block is big enough.
    #[test]
    fn matches_a_reference_model() {
        let mut rng = fastrand::Rng::with_seed(7);
        let capacity = 4096;
        let mut tlsf = Tlsf::new(capacity);
        let mut live: Vec<(u32, u32)> = Vec::new();
        for _ in 0..20_000 {
            if live.is_empty() || rng.bool() {
                let n = rng.u32(1..200);
                let largest = tlsf.free_blocks().map(|(_, len)| len).max().unwrap_or(0);
                let fits = tlsf.fits(n);
                let alloc = tlsf.alloc(n);
                assert_eq!(alloc.is_some(), fits);
                match alloc {
                    Some(off) => {
                        assert!(off + n <= capacity);
                        assert!(live.iter().all(|&(o, l)| off + n <= o || o + l <= off));
                        live.push((off, n));
                    }
                    None => assert!(largest < n, "{n} refused with {largest} free"),
                }
            } else {
                let (off, _) = live.swap_remove(rng.usize(..live.len()));
                tlsf.free(off);
            }
            assert_eq!(tlsf.used(), live.iter().map(|&(_, l)| l).sum::<u32>());
        }
        for (off, _) in live.drain(..) {
            tlsf.free(off);
        }
        assert_eq!(tlsf.free_blocks().collect::<Vec<_>>(), [(0, capacity)]);
    }
}
