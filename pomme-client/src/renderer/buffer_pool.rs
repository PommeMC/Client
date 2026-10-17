//! Standalone append-only buffer reservations, measured in elements rather than
//! bytes.
//!
//! One pool belongs to one frame-in-flight slot. The caller must wait for that
//! slot's GPU work to complete before resetting or clearing its pool. This type
//! does not wait for fences, allocate Vulkan resources, or perform mapped
//! writes.
//!
//! Allocation is supplied by the caller and must produce a resource with room
//! for the requested capacity. Reservations borrow that resource mutably so its
//! allocation can be mapped by the renderer adapter. Copying a GPU
//! handle out of a reservation does not extend the pool's lifetime: the caller
//! remains responsible for keeping resources alive until GPU completion,
//! including before dropping the pool. The allocator must check the conversion
//! from element capacity to its resource's byte size.

use std::num::NonZeroUsize;

struct Buffer<B> {
    resource: B,
    capacity: usize,
}

pub struct BufferPool<B> {
    initial_capacity: NonZeroUsize,
    buffers: Vec<Buffer<B>>,
    used: usize,
}

pub struct Reservation<'a, B> {
    pub buffer: &'a mut B,
    pub first_element: usize,
    pub element_count: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ReserveError<E> {
    EmptyRequest,
    CapacityOverflow,
    Allocation(E),
}

impl<B> BufferPool<B> {
    /// Construction is lazy: no resource exists until the first reservation.
    pub fn new(initial_capacity: NonZeroUsize) -> Self {
        Self {
            initial_capacity,
            buffers: Vec::new(),
            used: 0,
        }
    }

    /// Reserve one contiguous range. Earlier ranges are never moved or reused
    /// until reset, including when growth requires another resource.
    ///
    /// New capacity is at least the request and otherwise doubles the previous
    /// capacity. Allocation and arithmetic failures leave the pool unchanged.
    /// The allocator must clean up any partial allocation before returning Err.
    pub fn reserve<E>(
        &mut self,
        element_count: usize,
        mut allocate: impl FnMut(usize) -> Result<B, E>,
    ) -> Result<Reservation<'_, B>, ReserveError<E>> {
        if element_count == 0 {
            return Err(ReserveError::EmptyRequest);
        }
        let fits = self
            .buffers
            .last()
            .is_some_and(|buffer| element_count <= buffer.capacity - self.used);
        if !fits {
            let capacity = match self.buffers.last() {
                Some(buffer) => buffer
                    .capacity
                    .checked_mul(2)
                    .ok_or(ReserveError::CapacityOverflow)?,
                None => self.initial_capacity.get(),
            }
            .max(element_count);
            let resource = allocate(capacity).map_err(ReserveError::Allocation)?;
            self.buffers.push(Buffer { resource, capacity });
            self.used = 0;
        }
        let first_element = self.used;
        self.used += element_count;
        Ok(Reservation {
            buffer: &mut self.buffers.last_mut().unwrap().resource,
            first_element,
            element_count,
        })
    }

    /// Call only after all GPU references to this pool have completed.
    ///
    /// Retain the largest resource, reset its cursor, and transfer ownership of
    /// superseded resources to the caller for destruction. Retaining the
    /// largest avoids keeping every historical growth step; steady repeated
    /// workloads converge to a single sufficiently large retained resource.
    /// Smaller future workloads retain that high-water capacity until
    /// explicitly cleared.
    #[must_use = "retired resources must be released by the caller"]
    pub fn reset_after_completion(&mut self) -> Vec<B> {
        let retained = self.buffers.pop();
        let retired = self
            .buffers
            .drain(..)
            .map(|buffer| buffer.resource)
            .collect();
        if let Some(buffer) = retained {
            self.buffers.push(buffer);
        }
        self.used = 0;
        retired
    }

    /// Call only after GPU completion. Transfers every resource to the caller
    /// for destruction, for example at shutdown or to release high-water
    /// memory.
    #[must_use = "retired resources must be released by the caller"]
    pub fn clear_after_completion(&mut self) -> Vec<B> {
        let retired = self
            .buffers
            .drain(..)
            .map(|buffer| buffer.resource)
            .collect();
        self.used = 0;
        retired
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct TestBuffer {
        id: usize,
        elements: Vec<u32>,
    }

    #[derive(Default)]
    struct TestAllocator {
        capacities: Vec<usize>,
    }

    impl TestAllocator {
        fn allocate(&mut self, capacity: usize) -> Result<TestBuffer, &'static str> {
            self.capacities.push(capacity);
            Ok(TestBuffer {
                id: self.capacities.len(),
                elements: vec![0; capacity],
            })
        }
    }

    fn pool<B>() -> BufferPool<B> {
        BufferPool::new(NonZeroUsize::new(8).unwrap())
    }

    #[test]
    fn reservations_are_lazy_contiguous_and_do_not_overwrite_earlier_writes() {
        let mut pool = pool();
        let mut allocator = TestAllocator::default();
        assert!(allocator.capacities.is_empty());
        for (count, expected_offset, value) in [(3, 0, 1), (5, 3, 2)] {
            let reservation = pool
                .reserve(count, |size| allocator.allocate(size))
                .unwrap();
            assert_eq!(reservation.buffer.id, 1);
            assert_eq!(reservation.first_element, expected_offset);
            assert_eq!(reservation.element_count, count);
            reservation.buffer.elements[expected_offset..expected_offset + count].fill(value);
        }
        assert_eq!(allocator.capacities, [8]);
        assert_eq!(pool.buffers[0].resource.elements, [1, 1, 1, 2, 2, 2, 2, 2]);
    }

    #[test]
    fn growth_uses_a_new_resource_and_large_requests_fit_one_reservation() {
        let mut pool = pool();
        let mut allocator = TestAllocator::default();
        let first = pool.reserve(7, |size| allocator.allocate(size)).unwrap();
        first.buffer.elements[..7].fill(42);
        let next = pool.reserve(3, |size| allocator.allocate(size)).unwrap();
        assert_eq!((next.buffer.id, next.first_element), (2, 0));
        let large = pool.reserve(40, |size| allocator.allocate(size)).unwrap();
        assert_eq!(
            (large.buffer.id, large.first_element, large.element_count),
            (3, 0, 40)
        );
        assert_eq!(allocator.capacities, [8, 16, 40]);
        assert_eq!(&pool.buffers[0].resource.elements[..7], &[42; 7]);
        assert_eq!(pool.buffers.len(), 3);
    }

    #[test]
    fn completed_reset_retires_superseded_buffers_and_reuses_largest() {
        let mut pool = pool();
        let mut allocator = TestAllocator::default();
        pool.reserve(8, |size| allocator.allocate(size)).unwrap();
        pool.reserve(9, |size| allocator.allocate(size)).unwrap();
        let retired = pool.reset_after_completion();
        assert_eq!(
            retired.iter().map(|buffer| buffer.id).collect::<Vec<_>>(),
            [1]
        );
        for _ in 0..3 {
            let first = pool.reserve(8, |size| allocator.allocate(size)).unwrap();
            assert_eq!((first.buffer.id, first.first_element), (2, 0));
            let second = pool.reserve(8, |size| allocator.allocate(size)).unwrap();
            assert_eq!((second.buffer.id, second.first_element), (2, 8));
            assert!(pool.reset_after_completion().is_empty());
        }
        assert_eq!(allocator.capacities, [8, 16]);
        let cleared = pool.clear_after_completion();
        assert_eq!(cleared[0].id, 2);
        assert!(pool.clear_after_completion().is_empty());
        let restarted = pool.reserve(1, |size| allocator.allocate(size)).unwrap();
        assert_eq!((restarted.buffer.id, restarted.first_element), (3, 0));
        assert_eq!(allocator.capacities, [8, 16, 8]);
    }

    #[test]
    fn allocation_failure_does_not_consume_space_or_lose_existing_resources() {
        let mut pool = pool();
        let mut allocator = TestAllocator::default();
        pool.reserve(7, |size| allocator.allocate(size)).unwrap();
        let failed = pool.reserve(2, |_| Err("out of memory"));
        assert!(matches!(
            failed,
            Err(ReserveError::Allocation("out of memory"))
        ));
        let remainder = pool.reserve(1, |size| allocator.allocate(size)).unwrap();
        assert_eq!((remainder.buffer.id, remainder.first_element), (1, 7));
        let retry = pool.reserve(2, |size| allocator.allocate(size)).unwrap();
        assert_eq!((retry.buffer.id, retry.first_element), (2, 0));
        assert_eq!(allocator.capacities, [8, 16]);
    }

    #[test]
    fn empty_and_overflow_requests_fail_without_allocating_or_changing_state() {
        let mut pool = pool::<()>();
        assert!(matches!(
            pool.reserve(0, |_| panic!("must not allocate")),
            Err(ReserveError::<()>::EmptyRequest)
        ));
        assert!(pool.reset_after_completion().is_empty());
        pool.reserve(usize::MAX, |_| Ok::<_, ()>(())).unwrap();
        assert!(matches!(
            pool.reserve(1, |_| panic!("must not allocate")),
            Err(ReserveError::<()>::CapacityOverflow)
        ));
        assert_eq!(pool.used, usize::MAX);
        assert_eq!(pool.buffers.len(), 1);
    }
}
