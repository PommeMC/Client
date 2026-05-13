#![allow(clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]

use pomme_gpu_allocator::{
    MemoryLocation,
    vulkan::{AllocationCreateDesc, AllocationScheme, Allocator, AllocatorCreateDesc},
};
use pyronyx::vk;
use tracing::info;
use tracing_subscriber::EnvFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("trace")),
        )
        .init();

    // Create Vulkan instance
    let instance = {
        let app_name = c"Vulkan gpu-allocator test";

        let appinfo = vk::ApplicationInfo {
            application_name: app_name.as_ptr(),
            application_version: 0,
            engine_name: app_name.as_ptr(),
            engine_version: 0,
            api_version: vk::make_api_version(0, 1, 0, 0),
            ..Default::default()
        };

        let layer_names_raw = [c"VK_LAYER_KHRONOS_validation".as_ptr()];

        let create_info = vk::InstanceCreateInfo {
            application_info: &appinfo,
            enabled_layer_count: layer_names_raw.len() as u32,
            enabled_layer_names: layer_names_raw.as_ptr(),
            ..Default::default()
        };

        unsafe { vk::Instance::create(&create_info, None).expect("Instance creation error") }
    };

    // Look for vulkan physical device
    let (pdevice, queue_family_index) = {
        let pdevices = unsafe {
            instance
                .enumerate_physical_devices()
                .expect("Physical device error")
        };
        pdevices
            .into_iter()
            .find_map(|pdevice| {
                pdevice
                    .get_queue_family_properties()
                    .iter()
                    .enumerate()
                    .find_map(|(index, info)| {
                        let supports_graphics = info.queue_flags.contains(vk::QueueFlags::Graphics);
                        if supports_graphics {
                            Some((pdevice, index))
                        } else {
                            None
                        }
                    })
            })
            .expect("Couldn't find suitable device.")
    };

    // Create vulkan device
    let device = {
        let features = vk::PhysicalDeviceFeatures {
            shader_clip_distance: 1,
            ..Default::default()
        };
        let priorities = [1.0];

        let queue_info = vk::DeviceQueueCreateInfo {
            queue_family_index: queue_family_index as u32,
            queue_count: 1,
            queue_priorities: priorities.as_ptr(),
            ..Default::default()
        };

        let create_info = vk::DeviceCreateInfo {
            queue_create_info_count: 1,
            queue_create_infos: &queue_info,
            enabled_features: &features,
            ..Default::default()
        };

        unsafe {
            pdevice
                .create_device(&create_info, None, &instance)
                .unwrap()
        }
    };

    // Set up the allocator
    let mut allocator = Allocator::new(&AllocatorCreateDesc {
        instance: instance.clone(),
        device: device.clone(),
        physical_device: pdevice,
        debug_settings: Default::default(),
        buffer_device_address: false,
        allocation_sizes: Default::default(),
    })
    .unwrap();

    // Test allocating Gpu Only memory
    {
        let test_buffer_info = vk::BufferCreateInfo {
            size: 512,
            usage: vk::BufferUsageFlags::StorageBuffer,
            sharing_mode: vk::SharingMode::Exclusive,
            ..Default::default()
        };
        let test_buffer = device.create_buffer(&test_buffer_info, None).unwrap();
        let requirements = device.get_buffer_memory_requirements(test_buffer);
        let location = MemoryLocation::GpuOnly;

        let allocation = allocator
            .allocate(&AllocationCreateDesc {
                requirements,
                location,
                linear: true,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                name: "Test allocation (Gpu Only)",
            })
            .unwrap();

        unsafe {
            device
                .bind_buffer_memory(test_buffer, allocation.memory(), allocation.offset())
                .unwrap()
        };

        allocator.free(allocation).unwrap();

        device.destroy_buffer(test_buffer, None);

        info!("Allocation and deallocation of GpuOnly memory was successful.");
    }

    // Test allocating Cpu to Gpu memory
    {
        let test_buffer_info = vk::BufferCreateInfo {
            size: 512,
            usage: vk::BufferUsageFlags::StorageBuffer,
            sharing_mode: vk::SharingMode::Exclusive,
            ..Default::default()
        };
        let test_buffer = device.create_buffer(&test_buffer_info, None).unwrap();
        let requirements = device.get_buffer_memory_requirements(test_buffer);
        let location = MemoryLocation::CpuToGpu;

        let allocation = allocator
            .allocate(&AllocationCreateDesc {
                requirements,
                location,
                linear: true,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                name: "Test allocation (Cpu to Gpu)",
            })
            .unwrap();

        unsafe {
            device
                .bind_buffer_memory(test_buffer, allocation.memory(), allocation.offset())
                .unwrap()
        };

        allocator.free(allocation).unwrap();

        device.destroy_buffer(test_buffer, None);

        info!("Allocation and deallocation of CpuToGpu memory was successful.");
    }

    // Test allocating Gpu to Cpu memory
    {
        let test_buffer_info = vk::BufferCreateInfo {
            size: 512,
            usage: vk::BufferUsageFlags::StorageBuffer,
            sharing_mode: vk::SharingMode::Exclusive,
            ..Default::default()
        };
        let test_buffer = device.create_buffer(&test_buffer_info, None).unwrap();
        let requirements = device.get_buffer_memory_requirements(test_buffer);
        let location = MemoryLocation::GpuToCpu;

        let allocation = allocator
            .allocate(&AllocationCreateDesc {
                requirements,
                location,
                linear: true,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                name: "Test allocation (Gpu to Cpu)",
            })
            .unwrap();

        unsafe {
            device
                .bind_buffer_memory(test_buffer, allocation.memory(), allocation.offset())
                .unwrap()
        };

        allocator.free(allocation).unwrap();

        device.destroy_buffer(test_buffer, None);

        info!("Allocation and deallocation of GpuToCpu memory was successful.");
    }

    drop(allocator); // Explicitly drop before destruction of device and instance.
    device.destroy(None);
    instance.destroy(None);
}
