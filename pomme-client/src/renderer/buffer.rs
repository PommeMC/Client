use std::sync::{Arc, Mutex};

use pomme_gpu_allocator::MemoryLocation;
use pomme_gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};
use pyronyx::vk;

/// A Vulkan buffer together with the allocation that backs it.
///
/// The wrapper deliberately does not implement `Drop`: Vulkan destruction
/// requires both the device and allocator, and callers must explicitly
/// destroy resources after the device is idle.
#[derive(Default)]
pub(crate) struct Buffer {
    pub buffer: vk::Buffer,
    pub allocation: Allocation,
}

impl Buffer {
    pub(crate) fn mapped(
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        data: &[u8],
        usage: vk::BufferUsageFlags,
        name: &str,
    ) -> Self {
        let mut buffer = Self::allocate(
            device,
            allocator,
            data.len() as u64,
            usage,
            MemoryLocation::CpuToGpu,
            name,
        );
        buffer.allocation.mapped_slice_mut().unwrap()[..data.len()].copy_from_slice(data);
        buffer
    }

    pub(crate) fn staging(
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        data: &[u8],
        name: &str,
    ) -> Self {
        Self::mapped(
            device,
            allocator,
            data,
            vk::BufferUsageFlags::TransferSrc,
            name,
        )
    }

    pub(crate) fn host(
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        size: u64,
        usage: vk::BufferUsageFlags,
        name: &str,
    ) -> Self {
        Self::allocate(
            device,
            allocator,
            size,
            usage,
            MemoryLocation::CpuToGpu,
            name,
        )
    }

    pub(crate) fn device(
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        size: u64,
        usage: vk::BufferUsageFlags,
        name: &str,
    ) -> Self {
        Self::allocate(
            device,
            allocator,
            size,
            usage | vk::BufferUsageFlags::TransferDst,
            MemoryLocation::GpuOnly,
            name,
        )
    }

    pub(crate) fn uniform(
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        size: u64,
        name: &str,
    ) -> Self {
        Self::host(
            device,
            allocator,
            size,
            vk::BufferUsageFlags::UniformBuffer,
            name,
        )
    }

    pub(crate) fn mapped_slice_mut(&mut self) -> &mut [u8] {
        self.allocation.mapped_slice_mut().unwrap()
    }

    /// Compatibility escape hatch for APIs that still store a handle and an
    /// allocation separately. New resource owners should retain `Buffer`.
    pub(crate) fn into_parts(self) -> (vk::Buffer, Allocation) {
        (self.buffer, self.allocation)
    }

    pub(crate) fn destroy(self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        if self.buffer.is_null() {
            return;
        }
        device.destroy_buffer(self.buffer, None);
        allocator.lock().unwrap().free(self.allocation).ok();
    }

    fn allocate(
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        size: u64,
        usage: vk::BufferUsageFlags,
        location: MemoryLocation,
        name: &str,
    ) -> Self {
        let info = vk::BufferCreateInfo {
            size,
            usage,
            sharing_mode: vk::SharingMode::Exclusive,
            ..Default::default()
        };
        let buffer = device
            .create_buffer(&info, None)
            .expect("failed to create buffer");
        let requirements = device.get_buffer_memory_requirements(buffer);
        let allocation = allocator
            .lock()
            .unwrap()
            .allocate(&AllocationCreateDesc {
                name,
                requirements,
                location,
                linear: true,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .expect("failed to allocate buffer memory");
        unsafe {
            device
                .bind_buffer_memory(buffer, allocation.memory(), allocation.offset())
                .expect("failed to bind buffer memory");
        }
        Self { buffer, allocation }
    }
}
