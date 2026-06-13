use std::path::Path;
use std::sync::{Arc, Mutex};

use pomme_gpu_allocator::MemoryLocation;
use pomme_gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};
use pyronyx::vk;

pub fn create_gpu_image(
    device: &vk::Device,
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
        vk::Format::R8G8B8A8Srgb,
        name,
    )
}

pub fn create_gpu_image_with_format(
    device: &vk::Device,
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
    device: &vk::Device,
    allocator: &Arc<Mutex<Allocator>>,
    width: u32,
    height: u32,
    format: vk::Format,
    mip_levels: u32,
    name: &str,
) -> (vk::Image, vk::ImageView, Allocation, u32) {
    let usage = vk::ImageUsageFlags::TransferDst | vk::ImageUsageFlags::Sampled;

    let image_info = vk::ImageCreateInfo {
        image_type: vk::ImageType::Type2D,
        format,
        extent: vk::Extent3D {
            width,
            height,
            depth: 1,
        },
        mip_levels,
        array_layers: 1,
        samples: vk::SampleCountFlags::Type1,
        tiling: vk::ImageTiling::Optimal,
        usage,
        ..Default::default()
    };

    let image = device
        .create_image(&image_info, None)
        .expect("failed to create image");
    let mem_reqs = device.get_image_memory_requirements(image);

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

    let view_info = vk::ImageViewCreateInfo {
        image,
        view_type: vk::ImageViewType::Type2D,
        format,
        subresource_range: vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::Color,
            base_mip_level: 0,
            level_count: mip_levels,
            base_array_layer: 0,
            layer_count: 1,
        },
        ..Default::default()
    };
    let view = device
        .create_image_view(&view_info, None)
        .expect("failed to create image view");

    (image, view, allocation, mip_levels)
}

pub fn create_mapped_buffer(
    device: &vk::Device,
    allocator: &Arc<Mutex<Allocator>>,
    data: &[u8],
    usage: vk::BufferUsageFlags,
    name: &str,
) -> (vk::Buffer, Allocation) {
    let buffer_info = vk::BufferCreateInfo {
        size: data.len() as u64,
        usage,
        sharing_mode: vk::SharingMode::Exclusive,
        ..Default::default()
    };

    let buffer = device
        .create_buffer(&buffer_info, None)
        .expect("failed to create buffer");
    let mem_reqs = device.get_buffer_memory_requirements(buffer);

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
    device: &vk::Device,
    allocator: &Arc<Mutex<Allocator>>,
    data: &[u8],
    name: &str,
) -> (vk::Buffer, Allocation) {
    create_mapped_buffer(
        device,
        allocator,
        data,
        vk::BufferUsageFlags::TransferSrc,
        name,
    )
}

pub fn create_host_buffer(
    device: &vk::Device,
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
    device: &vk::Device,
    allocator: &Arc<Mutex<Allocator>>,
    size: u64,
    usage: vk::BufferUsageFlags,
    location: MemoryLocation,
    name: &str,
) -> (vk::Buffer, Allocation) {
    let buffer_info = vk::BufferCreateInfo {
        size,
        usage,
        sharing_mode: vk::SharingMode::Exclusive,
        ..Default::default()
    };

    let buffer = device
        .create_buffer(&buffer_info, None)
        .expect("failed to create buffer");
    let mem_reqs = device.get_buffer_memory_requirements(buffer);

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
    device: &vk::Device,
    allocator: &Arc<Mutex<Allocator>>,
    size: u64,
    usage: vk::BufferUsageFlags,
    name: &str,
) -> (vk::Buffer, Allocation) {
    create_buffer(
        device,
        allocator,
        size,
        usage | vk::BufferUsageFlags::TransferDst,
        MemoryLocation::GpuOnly,
        name,
    )
}

pub fn create_uniform_buffer(
    device: &vk::Device,
    allocator: &Arc<Mutex<Allocator>>,
    size: u64,
    name: &str,
) -> (vk::Buffer, Allocation) {
    create_host_buffer(
        device,
        allocator,
        size,
        vk::BufferUsageFlags::UniformBuffer,
        name,
    )
}

pub fn upload_image(
    device: &vk::Device,
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
        rgba8_bytes(width, height),
        image,
        width,
        height,
        1,
    );
}

pub fn submit_one_time<F: FnOnce(&vk::CommandBuffer)>(
    device: &vk::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    record: F,
) {
    let alloc_info = vk::CommandBufferAllocateInfo {
        command_pool,
        level: vk::CommandBufferLevel::Primary,
        command_buffer_count: 1,
        ..Default::default()
    };
    let mut cmd = vk::CommandBuffer::null();
    unsafe { device.allocate_command_buffers(&alloc_info, std::slice::from_mut(&mut cmd)) }
        .expect("failed to allocate one-time command buffer");

    let begin_info = vk::CommandBufferBeginInfo {
        flags: vk::CommandBufferUsageFlags::OneTimeSubmit,
        ..Default::default()
    };
    cmd.begin(&begin_info)
        .expect("failed to begin command buffer");

    record(&cmd);

    cmd.end().expect("failed to end command buffer");

    let submit_info = vk::SubmitInfo {
        command_buffer_count: 1,
        command_buffers: &cmd.handle(),
        ..Default::default()
    };
    queue
        .submit(&[submit_info], vk::Fence::null())
        .expect("failed to submit one-time command buffer");
    queue
        .wait_idle()
        .expect("failed to wait for one-time command buffer");
    device.free_command_buffers(command_pool, &[cmd.handle()]);
}

pub fn transition_image_to_shader_read(
    device: &vk::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    image: vk::Image,
) {
    submit_one_time(device, queue, command_pool, |cmd| {
        let barrier = vk::ImageMemoryBarrier {
            image,
            old_layout: vk::ImageLayout::Undefined,
            new_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
            src_access_mask: vk::AccessFlags::empty(),
            dst_access_mask: vk::AccessFlags::ShaderRead,
            subresource_range: vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::Color,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            },
            ..Default::default()
        };
        cmd.pipeline_barrier(
            vk::PipelineStageFlags::TopOfPipe,
            vk::PipelineStageFlags::FragmentShader,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier],
        );
    });
}

pub fn create_descriptor_set_layout(
    device: &vk::Device,
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
    let info = vk::DescriptorSetLayoutCreateInfo {
        binding_count: bindings.len() as u32,
        bindings: bindings.as_ptr(),
        ..Default::default()
    };
    device
        .create_descriptor_set_layout(&info, None)
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
    aspect_mask: vk::ImageAspectFlags::Color,
    base_mip_level: 0,
    level_count: 1,
    base_array_layer: 0,
    layer_count: 1,
};

pub const DEPTH_SUBRESOURCE_RANGE: vk::ImageSubresourceRange = vk::ImageSubresourceRange {
    aspect_mask: vk::ImageAspectFlags::Depth,
    base_mip_level: 0,
    level_count: 1,
    base_array_layer: 0,
    layer_count: 1,
};

pub unsafe fn create_nearest_sampler(device: &vk::Device) -> vk::Sampler {
    unsafe { create_nearest_sampler_mipmapped(device, 1) }
}

pub unsafe fn create_linear_sampler(device: &vk::Device) -> vk::Sampler {
    let info = vk::SamplerCreateInfo {
        mag_filter: vk::Filter::Linear,
        min_filter: vk::Filter::Linear,
        address_mode_u: vk::SamplerAddressMode::ClampToEdge,
        address_mode_v: vk::SamplerAddressMode::ClampToEdge,
        address_mode_w: vk::SamplerAddressMode::ClampToEdge,
        ..Default::default()
    };
    device
        .create_sampler(&info, None)
        .expect("failed to create linear sampler")
}

pub fn create_gpu_image_mipmapped(
    device: &vk::Device,
    allocator: &Arc<Mutex<Allocator>>,
    width: u32,
    height: u32,
    mip_levels: u32,
    name: &str,
) -> (vk::Image, vk::ImageView, Allocation, u32) {
    create_gpu_image_core(
        device,
        allocator,
        width,
        height,
        vk::Format::R8G8B8A8Srgb,
        mip_levels,
        name,
    )
}

/// Uploads an image whose mip levels are all present in the staging buffer,
/// packed tightly with level 0 first.
#[allow(clippy::too_many_arguments)]
pub fn upload_image_mipmapped(
    device: &vk::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    staging_buffer: vk::Buffer,
    staging_size: u64,
    image: vk::Image,
    width: u32,
    height: u32,
    mip_levels: u32,
) {
    submit_one_time(device, queue, command_pool, |cmd| {
        record_image_upload(
            cmd,
            staging_buffer,
            staging_size,
            image,
            width,
            height,
            mip_levels,
        );
    });
}

pub struct PendingImageUpload {
    pub staging_buffer: vk::Buffer,
    pub staging_size: u64,
    pub image: vk::Image,
    pub width: u32,
    pub height: u32,
    pub mip_levels: u32,
}

pub fn upload_images_batched(
    device: &vk::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    uploads: &[PendingImageUpload],
) {
    if uploads.is_empty() {
        return;
    }
    submit_one_time(device, queue, command_pool, |cmd| {
        for u in uploads {
            record_image_upload(
                cmd,
                u.staging_buffer,
                u.staging_size,
                u.image,
                u.width,
                u.height,
                u.mip_levels,
            );
        }
    });
}

fn rgba8_bytes(width: u32, height: u32) -> u64 {
    u64::from(width) * u64::from(height) * 4
}

fn record_image_upload(
    cmd: &vk::CommandBuffer,
    staging_buffer: vk::Buffer,
    staging_size: u64,
    image: vk::Image,
    width: u32,
    height: u32,
    mip_levels: u32,
) {
    let barrier_all = vk::ImageMemoryBarrier {
        image,
        old_layout: vk::ImageLayout::Undefined,
        new_layout: vk::ImageLayout::TransferDstOptimal,
        src_access_mask: vk::AccessFlags::empty(),
        dst_access_mask: vk::AccessFlags::TransferWrite,
        subresource_range: vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::Color,
            base_mip_level: 0,
            level_count: mip_levels,
            base_array_layer: 0,
            layer_count: 1,
        },
        ..Default::default()
    };

    cmd.pipeline_barrier(
        vk::PipelineStageFlags::TopOfPipe,
        vk::PipelineStageFlags::Transfer,
        vk::DependencyFlags::empty(),
        &[],
        &[],
        &[barrier_all],
    );

    let mut buffer_offset = 0u64;
    let copy_regions: Vec<vk::BufferImageCopy> = (0..mip_levels)
        .map(|level| {
            let w = (width >> level).max(1);
            let h = (height >> level).max(1);
            let region = vk::BufferImageCopy {
                buffer_offset,
                buffer_row_length: 0,
                buffer_image_height: 0,
                image_subresource: vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::Color,
                    mip_level: level,
                    base_array_layer: 0,
                    layer_count: 1,
                },
                image_offset: vk::Offset3D { x: 0, y: 0, z: 0 },
                image_extent: vk::Extent3D {
                    width: w,
                    height: h,
                    depth: 1,
                },
            };
            buffer_offset += rgba8_bytes(w, h);
            region
        })
        .collect();

    debug_assert_eq!(
        buffer_offset, staging_size,
        "staging buffer ({staging_size} bytes) must hold exactly all {mip_levels} mip levels \
         ({buffer_offset} bytes); a mismatch would copy out of bounds"
    );

    cmd.copy_buffer_to_image(
        staging_buffer,
        image,
        vk::ImageLayout::TransferDstOptimal,
        &copy_regions,
    );

    let barrier = vk::ImageMemoryBarrier {
        image,
        old_layout: vk::ImageLayout::TransferDstOptimal,
        new_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
        src_access_mask: vk::AccessFlags::TransferWrite,
        dst_access_mask: vk::AccessFlags::ShaderRead,
        subresource_range: vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::Color,
            base_mip_level: 0,
            level_count: mip_levels,
            base_array_layer: 0,
            layer_count: 1,
        },
        ..Default::default()
    };

    cmd.pipeline_barrier(
        vk::PipelineStageFlags::Transfer,
        vk::PipelineStageFlags::FragmentShader,
        vk::DependencyFlags::empty(),
        &[],
        &[],
        &[barrier],
    );
}

pub unsafe fn create_nearest_sampler_mipmapped(
    device: &vk::Device,
    mip_levels: u32,
) -> vk::Sampler {
    let mipmap_mode = if mip_levels > 1 {
        vk::SamplerMipmapMode::Linear
    } else {
        vk::SamplerMipmapMode::Nearest
    };

    let info = vk::SamplerCreateInfo {
        mag_filter: vk::Filter::Nearest,
        min_filter: vk::Filter::Nearest,
        mipmap_mode,
        address_mode_u: vk::SamplerAddressMode::ClampToEdge,
        address_mode_v: vk::SamplerAddressMode::ClampToEdge,
        address_mode_w: vk::SamplerAddressMode::ClampToEdge,
        min_lod: 0.0,
        max_lod: mip_levels as f32,
        ..Default::default()
    };

    device
        .create_sampler(&info, None)
        .expect("failed to create nearest sampler")
}

// The code below is extracted from ash-rs/ash under the MIT license;
// for the full license, see THIRD_PARTY_LICENSES.md

use core::slice;
use std::io;

/// Decode SPIR-V from bytes.
///
/// This function handles SPIR-V of arbitrary endianness gracefully, and returns
/// correctly aligned storage.
///
/// # Examples
/// ```no_run
/// // Decode SPIR-V from a file
/// let mut file = std::fs::File::open("/path/to/shader.spv").unwrap();
/// let words = ash::util::read_spv(&mut file).unwrap();
/// ```
/// ```
/// // Decode SPIR-V from memory
/// const SPIRV: &[u8] = &[
///     // ...
/// #   0x03, 0x02, 0x23, 0x07,
/// ];
/// let words = ash::util::read_spv(&mut std::io::Cursor::new(&SPIRV[..])).unwrap();
/// ```
pub fn read_spv<R: io::Read + io::Seek>(x: &mut R) -> io::Result<Vec<u32>> {
    // TODO use stream_len() once it is stabilized and remove the subsequent
    // rewind() call
    let size = x.seek(io::SeekFrom::End(0))?;
    x.rewind()?;
    if size % 4 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "input length not divisible by 4",
        ));
    }
    if size > usize::MAX as u64 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "input too long"));
    }
    let words = (size / 4) as usize;
    // https://github.com/ash-rs/ash/issues/354:
    // Zero-initialize the result to prevent read_exact from possibly
    // reading uninitialized memory.
    let mut result = vec![0u32; words];
    x.read_exact(unsafe {
        slice::from_raw_parts_mut(result.as_mut_ptr().cast::<u8>(), words * 4)
    })?;
    const MAGIC_NUMBER: u32 = 0x0723_0203;
    if !result.is_empty() && result[0] == MAGIC_NUMBER.swap_bytes() {
        for word in &mut result {
            *word = word.swap_bytes();
        }
    }
    if result.is_empty() || result[0] != MAGIC_NUMBER {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "input missing SPIR-V magic number",
        ));
    }
    Ok(result)
}
