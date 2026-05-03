use std::path::Path;
use std::sync::{Arc, Mutex};

use ash::vk;
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};

pub fn create_gpu_image(
    device: &ash::Device,
    allocator: &Arc<Mutex<Allocator>>,
    width: u32,
    height: u32,
    name: &str,
) -> (vk::Image, vk::ImageView, Allocation) {
    create_gpu_image_with_format(
        device,
        allocator,
        width,
        height,
        vk::Format::R8G8B8A8_SRGB,
        name,
    )
}

pub fn create_gpu_image_with_format(
    device: &ash::Device,
    allocator: &Arc<Mutex<Allocator>>,
    width: u32,
    height: u32,
    format: vk::Format,
    name: &str,
) -> (vk::Image, vk::ImageView, Allocation) {
    let (image, view, allocation, _) =
        create_gpu_image_core(device, allocator, width, height, format, 1, name);
    (image, view, allocation)
}

fn create_gpu_image_core(
    device: &ash::Device,
    allocator: &Arc<Mutex<Allocator>>,
    width: u32,
    height: u32,
    format: vk::Format,
    mip_levels: u32,
    name: &str,
) -> (vk::Image, vk::ImageView, Allocation, u32) {
    let mut usage = vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED;
    if mip_levels > 1 {
        usage |= vk::ImageUsageFlags::TRANSFER_SRC;
    }

    let image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(format)
        .extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        })
        .mip_levels(mip_levels)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(usage);

    let image = unsafe { device.create_image(&image_info, None) }.expect("failed to create image");
    let mem_reqs = unsafe { device.get_image_memory_requirements(image) };

    let allocation = allocator
        .lock()
        .unwrap()
        .allocate(&AllocationCreateDesc {
            name,
            requirements: mem_reqs,
            location: MemoryLocation::GpuOnly,
            linear: false,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })
        .expect("failed to allocate image memory");

    unsafe {
        device
            .bind_image_memory(image, allocation.memory(), allocation.offset())
            .expect("failed to bind image memory");
    }

    let view_info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(format)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: mip_levels,
            base_array_layer: 0,
            layer_count: 1,
        });
    let view =
        unsafe { device.create_image_view(&view_info, None) }.expect("failed to create image view");

    (image, view, allocation, mip_levels)
}

pub fn create_mapped_buffer(
    device: &ash::Device,
    allocator: &Arc<Mutex<Allocator>>,
    data: &[u8],
    usage: vk::BufferUsageFlags,
    name: &str,
) -> (vk::Buffer, Allocation) {
    let buffer_info = vk::BufferCreateInfo::default()
        .size(data.len() as u64)
        .usage(usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);

    let buffer =
        unsafe { device.create_buffer(&buffer_info, None) }.expect("failed to create buffer");
    let mem_reqs = unsafe { device.get_buffer_memory_requirements(buffer) };

    let mut allocation = allocator
        .lock()
        .unwrap()
        .allocate(&AllocationCreateDesc {
            name,
            requirements: mem_reqs,
            location: MemoryLocation::CpuToGpu,
            linear: true,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })
        .expect("failed to allocate buffer memory");

    unsafe {
        device
            .bind_buffer_memory(buffer, allocation.memory(), allocation.offset())
            .expect("failed to bind buffer memory");
    }

    allocation.mapped_slice_mut().unwrap()[..data.len()].copy_from_slice(data);

    (buffer, allocation)
}

pub fn create_staging_buffer(
    device: &ash::Device,
    allocator: &Arc<Mutex<Allocator>>,
    data: &[u8],
    name: &str,
) -> (vk::Buffer, Allocation) {
    create_mapped_buffer(
        device,
        allocator,
        data,
        vk::BufferUsageFlags::TRANSFER_SRC,
        name,
    )
}

pub fn create_host_buffer(
    device: &ash::Device,
    allocator: &Arc<Mutex<Allocator>>,
    size: u64,
    usage: vk::BufferUsageFlags,
    name: &str,
) -> (vk::Buffer, Allocation) {
    create_buffer(
        device,
        allocator,
        size,
        usage,
        MemoryLocation::CpuToGpu,
        name,
    )
}

fn create_buffer(
    device: &ash::Device,
    allocator: &Arc<Mutex<Allocator>>,
    size: u64,
    usage: vk::BufferUsageFlags,
    location: MemoryLocation,
    name: &str,
) -> (vk::Buffer, Allocation) {
    let buffer_info = vk::BufferCreateInfo::default()
        .size(size)
        .usage(usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);

    let buffer =
        unsafe { device.create_buffer(&buffer_info, None) }.expect("failed to create buffer");
    let mem_reqs = unsafe { device.get_buffer_memory_requirements(buffer) };

    let allocation = allocator
        .lock()
        .unwrap()
        .allocate(&AllocationCreateDesc {
            name,
            requirements: mem_reqs,
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

    (buffer, allocation)
}

pub fn create_device_buffer(
    device: &ash::Device,
    allocator: &Arc<Mutex<Allocator>>,
    size: u64,
    usage: vk::BufferUsageFlags,
    name: &str,
) -> (vk::Buffer, Allocation) {
    create_buffer(
        device,
        allocator,
        size,
        usage | vk::BufferUsageFlags::TRANSFER_DST,
        MemoryLocation::GpuOnly,
        name,
    )
}

pub fn create_uniform_buffer(
    device: &ash::Device,
    allocator: &Arc<Mutex<Allocator>>,
    size: u64,
    name: &str,
) -> (vk::Buffer, Allocation) {
    create_host_buffer(
        device,
        allocator,
        size,
        vk::BufferUsageFlags::UNIFORM_BUFFER,
        name,
    )
}

pub fn upload_image(
    device: &ash::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    staging_buffer: vk::Buffer,
    image: vk::Image,
    width: u32,
    height: u32,
) {
    upload_image_mipmapped(
        device,
        queue,
        command_pool,
        staging_buffer,
        image,
        width,
        height,
        1,
    );
}

pub fn create_descriptor_set_layout(
    device: &ash::Device,
    descriptor_type: vk::DescriptorType,
    stage_flags: vk::ShaderStageFlags,
) -> vk::DescriptorSetLayout {
    let bindings = [vk::DescriptorSetLayoutBinding {
        binding: 0,
        descriptor_type,
        descriptor_count: 1,
        stage_flags,
        ..Default::default()
    }];
    let info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
    unsafe { device.create_descriptor_set_layout(&info, None) }
        .expect("failed to create descriptor set layout")
}

pub fn load_png(path: &Path) -> Option<(Vec<u8>, u32, u32)> {
    let file = std::fs::File::open(path).ok()?;
    let mut decoder = png::Decoder::new(file);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;

    let data = match info.color_type {
        png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => {
            let pixels = info.width as usize * info.height as usize;
            let mut rgba = Vec::with_capacity(pixels * 4);
            for chunk in buf[..pixels * 3].chunks_exact(3) {
                rgba.extend_from_slice(chunk);
                rgba.push(255);
            }
            rgba
        }
        png::ColorType::GrayscaleAlpha => {
            let pixels = info.width as usize * info.height as usize;
            let mut rgba = Vec::with_capacity(pixels * 4);
            for chunk in buf[..pixels * 2].chunks_exact(2) {
                rgba.extend_from_slice(&[chunk[0], chunk[0], chunk[0], chunk[1]]);
            }
            rgba
        }
        png::ColorType::Grayscale => {
            let pixels = info.width as usize * info.height as usize;
            let mut rgba = Vec::with_capacity(pixels * 4);
            for &g in &buf[..pixels] {
                rgba.extend_from_slice(&[g, g, g, 255]);
            }
            rgba
        }
        other => {
            tracing::warn!("Unsupported PNG color type {other:?}: {}", path.display());
            return None;
        }
    };

    Some((data, info.width, info.height))
}

pub const COLOR_SUBRESOURCE_RANGE: vk::ImageSubresourceRange = vk::ImageSubresourceRange {
    aspect_mask: vk::ImageAspectFlags::COLOR,
    base_mip_level: 0,
    level_count: 1,
    base_array_layer: 0,
    layer_count: 1,
};

pub const DEPTH_SUBRESOURCE_RANGE: vk::ImageSubresourceRange = vk::ImageSubresourceRange {
    aspect_mask: vk::ImageAspectFlags::DEPTH,
    base_mip_level: 0,
    level_count: 1,
    base_array_layer: 0,
    layer_count: 1,
};

pub unsafe fn create_nearest_sampler(device: &ash::Device) -> vk::Sampler {
    unsafe { create_nearest_sampler_mipmapped(device, 1) }
}

pub unsafe fn create_linear_sampler(device: &ash::Device) -> vk::Sampler {
    unsafe {
        let info = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::LINEAR)
            .min_filter(vk::Filter::LINEAR)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE);
        device
            .create_sampler(&info, None)
            .expect("failed to create linear sampler")
    }
}

pub fn calculate_mip_levels(w: u32, h: u32) -> u32 {
    (w.max(h) as f32).log2().floor() as u32 + 1
}

pub fn create_gpu_image_mipmapped(
    device: &ash::Device,
    allocator: &Arc<Mutex<Allocator>>,
    width: u32,
    height: u32,
    name: &str,
) -> (vk::Image, vk::ImageView, Allocation, u32) {
    let mip_levels = calculate_mip_levels(width, height);
    create_gpu_image_core(
        device,
        allocator,
        width,
        height,
        vk::Format::R8G8B8A8_SRGB,
        mip_levels,
        name,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn upload_image_mipmapped(
    device: &ash::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    staging_buffer: vk::Buffer,
    image: vk::Image,
    width: u32,
    height: u32,
    mip_levels: u32,
) {
    let alloc_info = vk::CommandBufferAllocateInfo::default()
        .command_pool(command_pool)
        .level(vk::CommandBufferLevel::PRIMARY)
        .command_buffer_count(1);
    let cmd = unsafe { device.allocate_command_buffers(&alloc_info) }
        .expect("failed to allocate upload command buffer")[0];

    let begin_info =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    unsafe { device.begin_command_buffer(cmd, &begin_info) }
        .expect("failed to begin command buffer");

    let barrier_all = vk::ImageMemoryBarrier::default()
        .image(image)
        .old_layout(vk::ImageLayout::UNDEFINED)
        .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .src_access_mask(vk::AccessFlags::empty())
        .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: mip_levels,
            base_array_layer: 0,
            layer_count: 1,
        });

    unsafe {
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier_all],
        );
    }

    let copy_region = vk::BufferImageCopy {
        buffer_offset: 0,
        buffer_row_length: 0,
        buffer_image_height: 0,
        image_subresource: vk::ImageSubresourceLayers {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            mip_level: 0,
            base_array_layer: 0,
            layer_count: 1,
        },
        image_offset: vk::Offset3D { x: 0, y: 0, z: 0 },
        image_extent: vk::Extent3D {
            width,
            height,
            depth: 1,
        },
    };

    unsafe {
        device.cmd_copy_buffer_to_image(
            cmd,
            staging_buffer,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &[copy_region],
        );
    }

    let mut mip_w = width as i32;
    let mut mip_h = height as i32;

    for i in 1..mip_levels {
        let barrier = vk::ImageMemoryBarrier::default()
            .image(image)
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: i - 1,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });

        unsafe {
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier],
            );
        }

        let next_w = (mip_w / 2).max(1);
        let next_h = (mip_h / 2).max(1);

        let blit = vk::ImageBlit {
            src_subresource: vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: i - 1,
                base_array_layer: 0,
                layer_count: 1,
            },
            src_offsets: [
                vk::Offset3D { x: 0, y: 0, z: 0 },
                vk::Offset3D {
                    x: mip_w,
                    y: mip_h,
                    z: 1,
                },
            ],
            dst_subresource: vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: i,
                base_array_layer: 0,
                layer_count: 1,
            },
            dst_offsets: [
                vk::Offset3D { x: 0, y: 0, z: 0 },
                vk::Offset3D {
                    x: next_w,
                    y: next_h,
                    z: 1,
                },
            ],
        };

        unsafe {
            device.cmd_blit_image(
                cmd,
                image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[blit],
                vk::Filter::NEAREST,
            );
        }

        let barrier = vk::ImageMemoryBarrier::default()
            .image(image)
            .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .src_access_mask(vk::AccessFlags::TRANSFER_READ)
            .dst_access_mask(vk::AccessFlags::SHADER_READ)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: i - 1,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });

        unsafe {
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier],
            );
        }

        mip_w = next_w;
        mip_h = next_h;
    }

    let barrier = vk::ImageMemoryBarrier::default()
        .image(image)
        .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
        .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
        .dst_access_mask(vk::AccessFlags::SHADER_READ)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: mip_levels - 1,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        });

    unsafe {
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier],
        );

        device
            .end_command_buffer(cmd)
            .expect("failed to end command buffer");

        let cmd_buffers = [cmd];
        let submit_info = vk::SubmitInfo::default().command_buffers(&cmd_buffers);
        device
            .queue_submit(queue, &[submit_info], vk::Fence::null())
            .expect("failed to submit upload");
        device
            .queue_wait_idle(queue)
            .expect("failed to wait for upload");
        device.free_command_buffers(command_pool, &[cmd]);
    }
}

pub unsafe fn create_nearest_sampler_mipmapped(
    device: &ash::Device,
    mip_levels: u32,
) -> vk::Sampler {
    unsafe {
        let mipmap_mode = if mip_levels > 1 {
            vk::SamplerMipmapMode::LINEAR
        } else {
            vk::SamplerMipmapMode::NEAREST
        };
        let info = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::NEAREST)
            .min_filter(vk::Filter::NEAREST)
            .mipmap_mode(mipmap_mode)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .min_lod(0.0)
            .max_lod(mip_levels as f32);
        device
            .create_sampler(&info, None)
            .expect("failed to create nearest sampler")
    }
}
