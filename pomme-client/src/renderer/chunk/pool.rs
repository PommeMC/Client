//! CPU-side allocation primitives used by the chunk renderer.
//!
//! The renderer keeps GPU retirement policy separate from the small, reusable
//! range allocator.  This module is intentionally Vulkan-independent so its
//! ordering and reclamation rules can be tested without a device.

#[derive(Clone, Debug)]
pub(crate) struct FreeList {
    capacity: u32,
    free: Vec<(u32, u32)>,
}

impl FreeList {
    pub(crate) fn new(capacity: u32) -> Self {
        Self {
            capacity,
            free: if capacity == 0 {
                Vec::new()
            } else {
                vec![(0, capacity)]
            },
        }
    }

    pub(crate) fn alloc(&mut self, n: u32) -> Option<u32> {
        if n == 0 {
            return Some(0);
        }
        for i in 0..self.free.len() {
            let (off, len) = self.free[i];
            if len >= n {
                if len == n {
                    self.free.remove(i);
                } else {
                    self.free[i] = (off + n, len - n);
                }
                return Some(off);
            }
        }
        None
    }

    pub(crate) fn free(&mut self, off: u32, len: u32) {
        if len == 0 {
            return;
        }
        let pos = self.free.partition_point(|&(start, _)| start < off);
        self.free.insert(pos, (off, len));
        if pos + 1 < self.free.len() && self.free[pos].0 + self.free[pos].1 == self.free[pos + 1].0
        {
            self.free[pos].1 += self.free[pos + 1].1;
            self.free.remove(pos + 1);
        }
        if pos > 0 && self.free[pos - 1].0 + self.free[pos - 1].1 == self.free[pos].0 {
            self.free[pos - 1].1 += self.free[pos].1;
            self.free.remove(pos);
        }
    }

    pub(crate) fn free_region(&mut self, off: u32, len: u32) {
        self.free(off, len);
    }

    pub(crate) fn reset(&mut self) {
        self.free.clear();
        if self.capacity != 0 {
            self.free.push((0, self.capacity));
        }
    }

    pub(crate) fn largest_free(&self) -> u32 {
        self.free.iter().map(|&(_, len)| len).max().unwrap_or(0)
    }

    pub(crate) fn grow(&mut self, capacity: u32) {
        if capacity > self.capacity {
            let old = self.capacity;
            self.capacity = capacity;
            self.free(old, capacity - old);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FreeList;

    #[test]
    fn allocations_coalesce() {
        let mut list = FreeList::new(10);
        assert_eq!(list.alloc(3), Some(0));
        assert_eq!(list.alloc(2), Some(3));
        list.free(0, 3);
        list.free(3, 2);
        assert_eq!(list.alloc(10), Some(0));
    }

    #[test]
    fn growth_adds_only_the_tail() {
        let mut list = FreeList::new(4);
        assert_eq!(list.alloc(4), Some(0));
        list.grow(8);
        assert_eq!(list.alloc(4), Some(4));
    }
}
