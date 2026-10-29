//! The Vulkan backend of [`super`].

use std::collections::HashMap;
use std::mem::ManuallyDrop;
use std::ops::BitOr;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::{io, slice};

use pomme_gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};
use pyronyx::khr::present_wait::PresentWaitDevice;
use pyronyx::khr::swapchain::{SwapchainDevice, SwapchainQueue};
use pyronyx::vk;

use super::{
    Acquired, AddressMode, BindGroupLayoutEntry, BindingType, BlendFactor, BlendState, Blit,
    BufferCopy, BufferUsages, BufferUse, CompareFunction, CullMode, Extent, FilterMode, FrontFace,
    LoadOp, MAX_COLOR_TARGETS, MemoryLocation, Presented, PushConstants, Rect, RenderPipelineDesc,
    SamplerDesc, ShaderStages, StoreOp, TexelCopy, TextureDesc, TextureFormat, TextureRange,
    TextureUsages, TextureUse, TextureViewDimension, Topology, VertexFormat, VertexStepMode,
};
use crate::renderer::MAX_FRAMES_IN_FLIGHT;

/// A Vulkan object its maker owns and frees with `destroy`. The default is an
/// empty placeholder.
macro_rules! handle_type {
    ($name:ident($raw:ty), $destroy:ident) => {
        #[repr(transparent)]
        #[derive(Clone, Copy, Default, PartialEq, Eq)]
        pub struct $name($raw);

        impl $name {
            pub fn destroy(self, device: &Device) {
                device.raw.$destroy(self.0, None);
            }
        }
    };
}

handle_type!(
    BindGroupLayout(vk::DescriptorSetLayout),
    destroy_descriptor_set_layout
);
handle_type!(PipelineLayout(vk::PipelineLayout), destroy_pipeline_layout);
handle_type!(Sampler(vk::Sampler), destroy_sampler);

/// Locks `mutex`, carrying on past a panic in an earlier holder.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

mod context;
mod swapchain;

pub use context::{Context, ContextError};
pub use swapchain::Swapchain;

pub type Error = vk::Error;

/// The GPU and what every resource is made with: its memory allocator, the
/// queue and command pool one-off uploads go through, and the render pass
/// cache. WebGPU's `GPUDevice` with its queue.
pub struct Device {
    raw: vk::Device,
    allocator: ManuallyDrop<Mutex<Allocator>>,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    targets: RenderTargets,
    bind_groups: BindGroupPools,
    /// The largest heap of GPU-local memory, in bytes.
    pub device_local_bytes: u64,
    /// A GPU with memory of its own, which uploads reach through a copy.
    pub discrete: bool,
    pub max_texture_array_layers: u32,
    /// The widest 2D texture, in texels.
    pub max_texture_size: u32,
    depth_clamp: bool,
}

impl Device {
    pub fn new(
        raw: vk::Device,
        physical_device: &vk::PhysicalDevice,
        allocator: Allocator,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
    ) -> Self {
        let memory = physical_device.get_memory_properties();
        let device_local_bytes = memory.memory_types[..memory.memory_type_count as usize]
            .iter()
            .filter(|ty| {
                ty.property_flags
                    .contains(vk::MemoryPropertyFlags::DeviceLocal)
            })
            .map(|ty| memory.memory_heaps[ty.heap_index as usize].size)
            .max()
            .unwrap_or(0);
        let properties = physical_device.get_properties();
        Self {
            raw,
            allocator: ManuallyDrop::new(Mutex::new(allocator)),
            queue,
            command_pool,
            targets: RenderTargets::default(),
            bind_groups: BindGroupPools::default(),
            device_local_bytes,
            discrete: properties.device_type == vk::PhysicalDeviceType::DiscreteGpu,
            max_texture_array_layers: properties.limits.max_image_array_layers,
            max_texture_size: properties.limits.max_image_dimension2_d,
            depth_clamp: physical_device.get_features().depth_clamp == vk::TRUE,
        }
    }

    pub fn wait_idle(&self) {
        if let Err(error) = self.raw.wait_idle() {
            tracing::error!(%error, "Failed to wait for Vulkan device idle");
        }
    }

    /// Drops the framebuffers holding any of `textures`, made outside the RHI
    /// and about to be destroyed. [`Texture::destroy`] does this itself.
    pub fn forget_targets(&self, textures: &[TextureHandle]) {
        self.targets.forget(self, textures);
    }

    /// Records commands and queues them after all earlier work. Once it
    /// returns, the resources they use may be rewritten or destroyed through
    /// the RHI; their results reach the CPU only through a readback buffer.
    /// Vulkan waits on a fence for this, where WebGPU's queue order gives it.
    pub fn submit(&self, record: impl FnOnce(&CommandEncoder)) {
        self.try_submit(record)
            .unwrap_or_else(|error| panic!("{error}"));
    }

    pub fn try_submit(&self, record: impl FnOnce(&CommandEncoder)) -> Result<(), String> {
        let alloc_info = vk::CommandBufferAllocateInfo {
            command_pool: self.command_pool,
            level: vk::CommandBufferLevel::Primary,
            command_buffer_count: 1,
            ..Default::default()
        };
        let mut cmd = vk::CommandBuffer::null();
        unsafe {
            self.raw
                .allocate_command_buffers(&alloc_info, slice::from_mut(&mut cmd))
        }
        .map_err(|error| format!("failed to allocate a command buffer: {error}"))?;
        let fence = match self.raw.create_fence(&vk::FenceCreateInfo::default(), None) {
            Ok(fence) => fence,
            Err(error) => {
                self.raw
                    .free_command_buffers(self.command_pool, &[cmd.handle()]);
                return Err(format!("failed to create a fence: {error}"));
            }
        };
        let result = cmd
            .begin(&vk::CommandBufferBeginInfo {
                flags: vk::CommandBufferUsageFlags::OneTimeSubmit,
                ..Default::default()
            })
            .and_then(|()| {
                record(&CommandEncoder::new(cmd));
                cmd.end()
            })
            .and_then(|()| {
                let submit_info = vk::SubmitInfo {
                    command_buffer_count: 1,
                    command_buffers: &cmd.handle(),
                    ..Default::default()
                };
                self.queue.submit(&[submit_info], fence)
            })
            .and_then(|()| self.raw.wait_for_fences(&[fence], true, u64::MAX))
            .map_err(|error| format!("failed to run a command buffer: {error}"));
        self.raw.destroy_fence(fence, None);
        self.raw
            .free_command_buffers(self.command_pool, &[cmd.handle()]);
        result
    }

    /// A bind group of `layout` holding `bindings`.
    pub fn create_bind_group(
        &self,
        layout: BindGroupLayout,
        bindings: &[(u32, BindingResource)],
    ) -> BindGroup {
        let (raw, pool) = self.bind_groups.allocate(&self.raw, layout.0);
        write_bind_group(self, raw, bindings);
        BindGroup { raw, pool }
    }

    fn allocator(&self) -> MutexGuard<'_, Allocator> {
        lock(&self.allocator)
    }

    /// Frees what the device made, then the device, once the GPU is idle.
    ///
    /// # Safety
    /// Nothing may use the device afterwards, and every resource made from
    /// it must already be destroyed.
    pub unsafe fn destroy(&mut self) {
        if let Err(error) = self.raw.wait_idle() {
            tracing::error!(%error, "Failed to wait for Vulkan device idle during destruction");
        }
        self.targets.destroy(self);
        self.bind_groups.destroy(&self.raw);
        self.raw.destroy_command_pool(self.command_pool, None);
        unsafe { ManuallyDrop::drop(&mut self.allocator) };
        self.raw.destroy(None);
    }
}

/// What commands and bindings take a [`Buffer`] as: a small copy, valid until
/// the buffer is destroyed.
#[repr(transparent)]
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct BufferHandle(vk::Buffer);

/// A buffer and the memory it owns. The default is an empty placeholder, for
/// `std::mem::take` when an owner frees a buffer it holds by value.
#[derive(Default)]
pub struct Buffer {
    raw: vk::Buffer,
    allocation: Allocation,
}

impl Buffer {
    pub fn new(
        device: &Device,
        label: &str,
        size: u64,
        usage: BufferUsages,
        location: MemoryLocation,
    ) -> Self {
        Self::try_new(device, label, size, usage, location)
            .unwrap_or_else(|error| panic!("{error}"))
    }

    pub fn try_new(
        device: &Device,
        label: &str,
        size: u64,
        usage: BufferUsages,
        location: MemoryLocation,
    ) -> Result<Self, String> {
        let info = vk::BufferCreateInfo {
            size,
            usage: buffer_usage_flags(usage),
            sharing_mode: vk::SharingMode::Exclusive,
            ..Default::default()
        };
        let raw = device
            .raw
            .create_buffer(&info, None)
            .map_err(|error| format!("failed to create buffer {label}: {error}"))?;
        let allocation = match allocate(
            device,
            label,
            device.raw.get_buffer_memory_requirements(raw),
            match location {
                MemoryLocation::Host => pomme_gpu_allocator::MemoryLocation::CpuToGpu,
                MemoryLocation::Device => pomme_gpu_allocator::MemoryLocation::GpuOnly,
                MemoryLocation::Readback => pomme_gpu_allocator::MemoryLocation::GpuToCpu,
            },
            true,
        ) {
            Ok(allocation) => allocation,
            Err(error) => {
                device.raw.destroy_buffer(raw, None);
                return Err(error);
            }
        };
        let mut buffer = Self { raw, allocation };
        if let Err(error) = unsafe {
            device.raw.bind_buffer_memory(
                raw,
                buffer.allocation.memory(),
                buffer.allocation.offset(),
            )
        } {
            buffer.destroy(device);
            return Err(format!("failed to bind buffer memory for {label}: {error}"));
        }
        if let MemoryLocation::Readback = location {
            buffer.zero();
        }
        Ok(buffer)
    }

    /// A host buffer holding `data`.
    pub fn with_data(device: &Device, label: &str, data: &[u8], usage: BufferUsages) -> Self {
        Self::try_with_data(device, label, data, usage).unwrap_or_else(|error| panic!("{error}"))
    }

    pub fn try_with_data(
        device: &Device,
        label: &str,
        data: &[u8],
        usage: BufferUsages,
    ) -> Result<Self, String> {
        let mut buffer = Self::try_new(
            device,
            label,
            data.len() as u64,
            usage,
            MemoryLocation::Host,
        )?;
        buffer.write(data);
        Ok(buffer)
    }

    pub fn handle(&self) -> BufferHandle {
        BufferHandle(self.raw)
    }

    /// Reads a [`MemoryLocation::Readback`] buffer; the caller has already
    /// waited on the frame that wrote it.
    pub fn read<R>(&mut self, read: impl FnOnce(&[u8]) -> R) -> R {
        read(self.mapped_mut())
    }

    /// Writes `data` at the start of a [`MemoryLocation::Host`] buffer.
    pub fn write(&mut self, data: &[u8]) {
        self.write_at(0, data);
    }

    /// Writes `data` at byte `offset` of a [`MemoryLocation::Host`] buffer.
    pub fn write_at(&mut self, offset: usize, data: &[u8]) {
        self.mapped_mut()[offset..offset + data.len()].copy_from_slice(data);
    }

    /// Zeroes a whole host-mapped buffer.
    pub fn zero(&mut self) {
        self.mapped_mut().fill(0);
    }

    fn mapped_mut(&mut self) -> &mut [u8] {
        self.allocation
            .mapped_slice_mut()
            .expect("buffer is not host-mapped")
    }

    pub fn destroy(self, device: &Device) {
        device.raw.destroy_buffer(self.raw, None);
        if let Err(error) = device.allocator().free(self.allocation) {
            tracing::error!(%error, "Failed to free Vulkan buffer allocation");
        }
    }
}

/// Memory for a buffer (`linear`) or an image.
fn allocate(
    device: &Device,
    label: &str,
    requirements: vk::MemoryRequirements,
    location: pomme_gpu_allocator::MemoryLocation,
    linear: bool,
) -> Result<Allocation, String> {
    device
        .allocator()
        .allocate(&AllocationCreateDesc {
            name: label,
            requirements,
            location,
            linear,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })
        .map_err(|error| format!("failed to allocate memory for {label}: {error}"))
}

/// The union of the flags in `table` whose bit `has`.
fn union_flags<B, F: BitOr<Output = F>>(
    empty: F,
    table: impl IntoIterator<Item = (B, F)>,
    has: impl Fn(B) -> bool,
) -> F {
    table
        .into_iter()
        .filter_map(|(bit, flag)| has(bit).then_some(flag))
        .fold(empty, |flags, flag| flags | flag)
}

fn buffer_usage_flags(usage: BufferUsages) -> vk::BufferUsageFlags {
    union_flags(
        vk::BufferUsageFlags::empty(),
        [
            (BufferUsages::VERTEX, vk::BufferUsageFlags::VertexBuffer),
            (BufferUsages::INDEX, vk::BufferUsageFlags::IndexBuffer),
            (BufferUsages::UNIFORM, vk::BufferUsageFlags::UniformBuffer),
            (BufferUsages::STORAGE, vk::BufferUsageFlags::StorageBuffer),
            (BufferUsages::INDIRECT, vk::BufferUsageFlags::IndirectBuffer),
            (BufferUsages::COPY_SRC, vk::BufferUsageFlags::TransferSrc),
            (BufferUsages::COPY_DST, vk::BufferUsageFlags::TransferDst),
        ],
        |bit| usage.contains(bit),
    )
}

/// A texture, its view of every level and layer, and the memory it owns.
/// The default is an empty placeholder, like [`Buffer`]'s.
#[derive(Default)]
pub struct Texture {
    raw: vk::Image,
    view: vk::ImageView,
    allocation: Allocation,
}

impl Texture {
    pub fn new(device: &Device, desc: &TextureDesc<'_>) -> Self {
        Self::try_new(device, desc).unwrap_or_else(|error| panic!("{error}"))
    }

    pub fn try_new(device: &Device, desc: &TextureDesc<'_>) -> Result<Self, String> {
        let label = desc.label;
        let format = texture_format(desc.format);
        let cube = desc.dimension == TextureViewDimension::Cube;
        let info = vk::ImageCreateInfo {
            image_type: vk::ImageType::Type2D,
            format,
            extent: vk::Extent3D {
                width: desc.width,
                height: desc.height,
                depth: 1,
            },
            mip_levels: desc.mip_levels,
            array_layers: desc.layers,
            samples: vk::SampleCountFlags::Type1,
            tiling: vk::ImageTiling::Optimal,
            usage: texture_usage_flags(desc.usage, desc.format),
            flags: if cube {
                vk::ImageCreateFlags::CubeCompatible
            } else {
                vk::ImageCreateFlags::empty()
            },
            ..Default::default()
        };
        let raw = device
            .raw
            .create_image(&info, None)
            .map_err(|error| format!("failed to create image {label}: {error}"))?;
        let allocation = match allocate(
            device,
            label,
            device.raw.get_image_memory_requirements(raw),
            pomme_gpu_allocator::MemoryLocation::GpuOnly,
            false,
        ) {
            Ok(allocation) => allocation,
            Err(error) => {
                device.raw.destroy_image(raw, None);
                return Err(error);
            }
        };
        let texture = Self {
            raw,
            view: vk::ImageView::null(),
            allocation,
        };
        if let Err(error) = unsafe {
            device.raw.bind_image_memory(
                raw,
                texture.allocation.memory(),
                texture.allocation.offset(),
            )
        } {
            texture.destroy(device);
            return Err(format!("failed to bind image memory for {label}: {error}"));
        }
        let view_info = vk::ImageViewCreateInfo {
            image: raw,
            view_type: match desc.dimension {
                TextureViewDimension::D2 => vk::ImageViewType::Type2D,
                TextureViewDimension::D2Array => vk::ImageViewType::Type2DArray,
                TextureViewDimension::Cube => vk::ImageViewType::Cube,
            },
            format,
            subresource_range: vk::ImageSubresourceRange {
                level_count: desc.mip_levels,
                layer_count: desc.layers,
                ..subresource_range(desc.format)
            },
            ..Default::default()
        };
        match device.raw.create_image_view(&view_info, None) {
            Ok(view) => Ok(Self { view, ..texture }),
            Err(error) => {
                texture.destroy(device);
                Err(format!("failed to create image view for {label}: {error}"))
            }
        }
    }

    pub fn handle(&self) -> TextureHandle {
        TextureHandle {
            image: self.raw,
            view: self.view,
        }
    }

    /// Frees the texture and any cached framebuffers holding it.
    pub fn destroy(self, device: &Device) {
        device.targets.forget(device, &[self.handle()]);
        device.raw.destroy_image_view(self.view, None);
        device.raw.destroy_image(self.raw, None);
        if let Err(error) = device.allocator().free(self.allocation) {
            tracing::error!(%error, "Failed to free Vulkan image allocation");
        }
    }
}

/// What barriers, copies, bindings and attachments take a texture as: its
/// image and its view of every level and layer, valid until their owner
/// destroys them.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct TextureHandle {
    image: vk::Image,
    view: vk::ImageView,
}

impl TextureHandle {
    /// A texture made outside the RHI, such as a swapchain image.
    pub fn new(image: vk::Image, view: vk::ImageView) -> Self {
        Self { image, view }
    }
}

pub fn texture_format(format: TextureFormat) -> vk::Format {
    match format {
        TextureFormat::R8Unorm => vk::Format::R8Unorm,
        TextureFormat::Rg8Unorm => vk::Format::R8G8Unorm,
        TextureFormat::Rgba8Unorm => vk::Format::R8G8B8A8Unorm,
        TextureFormat::Rgba16Unorm => vk::Format::R16G16B16A16Unorm,
        TextureFormat::Rgb10a2Unorm => vk::Format::A2B10G10R10UnormPack32,
        TextureFormat::R16Float => vk::Format::R16Sfloat,
        TextureFormat::Rg16Float => vk::Format::R16G16Sfloat,
        TextureFormat::Rgba16Float => vk::Format::R16G16B16A16Sfloat,
        TextureFormat::R32Float => vk::Format::R32Sfloat,
        TextureFormat::Rg32Float => vk::Format::R32G32Sfloat,
        TextureFormat::Rgba32Float => vk::Format::R32G32B32A32Sfloat,
        TextureFormat::Rg11b10Float => vk::Format::B10G11R11UfloatPack32,
        TextureFormat::Depth32Float => vk::Format::D32Sfloat,
    }
}

/// The base level and layer of `format`'s aspect.
fn subresource_range(format: TextureFormat) -> vk::ImageSubresourceRange {
    match format {
        TextureFormat::Depth32Float => DEPTH_SUBRESOURCE_RANGE,
        _ => COLOR_SUBRESOURCE_RANGE,
    }
}

fn texture_usage_flags(usage: TextureUsages, format: TextureFormat) -> vk::ImageUsageFlags {
    let attachment = match format {
        TextureFormat::Depth32Float => vk::ImageUsageFlags::DepthStencilAttachment,
        _ => vk::ImageUsageFlags::ColorAttachment,
    };
    union_flags(
        vk::ImageUsageFlags::empty(),
        [
            (TextureUsages::COPY_SRC, vk::ImageUsageFlags::TransferSrc),
            (TextureUsages::COPY_DST, vk::ImageUsageFlags::TransferDst),
            (TextureUsages::TEXTURE_BINDING, vk::ImageUsageFlags::Sampled),
            (TextureUsages::RENDER_ATTACHMENT, attachment),
        ],
        |bit| usage.contains(bit),
    )
}

pub fn create_sampler(device: &Device, desc: &SamplerDesc) -> Sampler {
    try_create_sampler(device, desc).unwrap_or_else(|error| panic!("{error}"))
}

pub fn try_create_sampler(device: &Device, desc: &SamplerDesc) -> Result<Sampler, String> {
    let filter = |mode| match mode {
        FilterMode::Nearest => vk::Filter::Nearest,
        FilterMode::Linear => vk::Filter::Linear,
    };
    let address = match desc.address_mode {
        AddressMode::ClampToEdge => vk::SamplerAddressMode::ClampToEdge,
        AddressMode::Repeat => vk::SamplerAddressMode::Repeat,
    };
    let info = vk::SamplerCreateInfo {
        mag_filter: filter(desc.filter),
        min_filter: filter(desc.filter),
        mipmap_mode: match desc.mipmap_filter {
            FilterMode::Nearest => vk::SamplerMipmapMode::Nearest,
            FilterMode::Linear => vk::SamplerMipmapMode::Linear,
        },
        address_mode_u: address,
        address_mode_v: address,
        address_mode_w: address,
        max_lod: desc.lod_max_clamp,
        compare_enable: if desc.compare.is_some() {
            vk::TRUE
        } else {
            vk::FALSE
        },
        compare_op: match desc.compare {
            Some(CompareFunction::Less) => vk::CompareOp::Less,
            Some(CompareFunction::LessEqual) | None => vk::CompareOp::LessOrEqual,
        },
        ..Default::default()
    };
    device
        .raw
        .create_sampler(&info, None)
        .map(Sampler)
        .map_err(|error| format!("failed to create sampler: {error}"))
}

/// What one binding of a bind group points at. Views must be in
/// `ShaderReadOnlyOptimal` layout.
#[derive(Clone, Copy)]
pub enum BindingResource {
    UniformBuffer { buffer: BufferHandle, size: u64 },
    StorageBuffer { buffer: BufferHandle, size: u64 },
    Texture(TextureHandle),
    Sampler(Sampler),
}

impl BindingResource {
    pub fn uniform(buffer: &Buffer, size: u64) -> Self {
        Self::UniformBuffer {
            buffer: buffer.handle(),
            size,
        }
    }

    pub fn storage(buffer: &Buffer, size: u64) -> Self {
        Self::StorageBuffer {
            buffer: buffer.handle(),
            size,
        }
    }

    fn ty(self) -> BindingType {
        match self {
            Self::UniformBuffer { .. } => BindingType::UniformBuffer,
            Self::StorageBuffer { .. } => BindingType::StorageBuffer,
            Self::Texture(_) => BindingType::Texture,
            Self::Sampler(_) => BindingType::Sampler,
        }
    }
}

/// Points each listed binding of `group` at its resource.
fn write_bind_group(
    device: &Device,
    group: vk::DescriptorSet,
    bindings: &[(u32, BindingResource)],
) {
    let image = |image_view, sampler| vk::DescriptorImageInfo {
        sampler,
        image_view,
        image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
    };
    // Every write gets both infos so their addresses are fixed before the
    // writes point at them.
    let infos: Vec<_> = bindings
        .iter()
        .map(|&(_, resource)| match resource {
            BindingResource::UniformBuffer { buffer, size }
            | BindingResource::StorageBuffer { buffer, size } => (
                vk::DescriptorBufferInfo {
                    buffer: buffer.0,
                    offset: 0,
                    range: size,
                },
                vk::DescriptorImageInfo::default(),
            ),
            BindingResource::Texture(texture) => (
                vk::DescriptorBufferInfo::default(),
                image(texture.view, vk::Sampler::null()),
            ),
            BindingResource::Sampler(sampler) => (
                vk::DescriptorBufferInfo::default(),
                image(vk::ImageView::null(), sampler.0),
            ),
        })
        .collect();
    let writes: Vec<_> = bindings
        .iter()
        .zip(&infos)
        .map(
            |(&(binding, resource), (buffer_info, image_info))| vk::WriteDescriptorSet {
                dst_set: group,
                dst_binding: binding,
                descriptor_count: 1,
                descriptor_type: descriptor_type(resource.ty()),
                buffer_info,
                image_info,
                ..Default::default()
            },
        )
        .collect();
    device.raw.update_descriptor_sets(&writes, &[]);
}

pub fn create_bind_group_layout(
    device: &Device,
    entries: &[BindGroupLayoutEntry],
) -> BindGroupLayout {
    let bindings: Vec<_> = entries
        .iter()
        .enumerate()
        .map(|(binding, entry)| vk::DescriptorSetLayoutBinding {
            binding: binding as u32,
            descriptor_type: descriptor_type(entry.ty),
            descriptor_count: 1,
            stage_flags: stage_flags(entry.visibility),
            ..Default::default()
        })
        .collect();
    let info = vk::DescriptorSetLayoutCreateInfo {
        binding_count: bindings.len() as u32,
        bindings: bindings.as_ptr(),
        ..Default::default()
    };
    BindGroupLayout(
        device
            .raw
            .create_descriptor_set_layout(&info, None)
            .expect("failed to create descriptor set layout"),
    )
}

pub fn create_pipeline_layout(
    device: &Device,
    bind_group_layouts: &[BindGroupLayout],
    push_constants: Option<PushConstants>,
) -> PipelineLayout {
    let push_range = push_constants.map(|push| vk::PushConstantRange {
        stage_flags: stage_flags(push.visibility),
        offset: 0,
        size: push.size,
    });
    let info = vk::PipelineLayoutCreateInfo {
        set_layout_count: bind_group_layouts.len() as u32,
        set_layouts: bind_group_layouts.as_ptr().cast(),
        push_constant_range_count: push_range.is_some() as u32,
        push_constant_ranges: push_range.as_ref().map_or(std::ptr::null(), |range| range),
        ..Default::default()
    };
    PipelineLayout(
        device
            .raw
            .create_pipeline_layout(&info, None)
            .expect("failed to create pipeline layout"),
    )
}

/// A pipeline and the layout its bind groups and push constants follow, as
/// WebGPU's pipelines keep theirs. Layouts can be shared, so whoever made one
/// destroys it. The default is an empty placeholder.
macro_rules! pipeline_type {
    ($name:ident) => {
        #[derive(Clone, Copy, Default)]
        pub struct $name {
            raw: vk::Pipeline,
            layout: PipelineLayout,
        }

        impl $name {
            pub fn destroy(self, device: &Device) {
                device.raw.destroy_pipeline(self.raw, None);
            }
        }
    };
}

pipeline_type!(RenderPipeline);
pipeline_type!(ComputePipeline);

pub fn create_compute_pipeline(
    device: &Device,
    layout: PipelineLayout,
    label: &str,
    shader: &[u8],
) -> ComputePipeline {
    let module = shader_module(device, shader);
    let info = vk::ComputePipelineCreateInfo {
        stage: vk::PipelineShaderStageCreateInfo {
            stage: vk::ShaderStageFlags::Compute,
            module,
            name: c"main".as_ptr(),
            ..Default::default()
        },
        layout: layout.0,
        ..Default::default()
    };
    let mut raw = vk::Pipeline::null();
    device
        .raw
        .create_compute_pipelines(
            vk::PipelineCache::null(),
            slice::from_ref(&info),
            None,
            slice::from_mut(&mut raw),
        )
        .unwrap_or_else(|e| panic!("failed to create {label} pipeline: {e:?}"));
    device.raw.destroy_shader_module(module, None);
    ComputePipeline { raw, layout }
}

pub type PipelineTargets = super::PipelineTargets<vk::Format>;

pub fn create_render_pipeline(
    device: &Device,
    targets: PipelineTargets,
    layout: PipelineLayout,
    desc: &RenderPipelineDesc,
) -> RenderPipeline {
    try_create_render_pipeline(device, targets, layout, desc)
        .unwrap_or_else(|e| panic!("failed to create {} pipeline: {e}", desc.label))
}

/// [`create_render_pipeline`] for shaders that may not be valid, such as a
/// shader pack's.
pub fn try_create_render_pipeline(
    device: &Device,
    targets: PipelineTargets,
    layout: PipelineLayout,
    desc: &RenderPipelineDesc,
) -> Result<RenderPipeline, String> {
    let render_pass = device.targets.compatible_pass(device, &targets);
    let vert_module = try_shader_module(device, desc.vertex_shader)?;
    let frag_module = match try_shader_module(device, desc.fragment_shader) {
        Ok(module) => module,
        Err(e) => {
            device.raw.destroy_shader_module(vert_module, None);
            return Err(e);
        }
    };
    let stages = [
        (vk::ShaderStageFlags::Vertex, vert_module),
        (vk::ShaderStageFlags::Fragment, frag_module),
    ]
    .map(|(stage, module)| vk::PipelineShaderStageCreateInfo {
        stage,
        module,
        name: c"main".as_ptr(),
        ..Default::default()
    });

    let bindings: Vec<_> = desc
        .vertex_buffers
        .iter()
        .enumerate()
        .map(|(binding, buffer)| vk::VertexInputBindingDescription {
            binding: binding as u32,
            stride: buffer.stride,
            input_rate: match buffer.step_mode {
                VertexStepMode::Vertex => vk::VertexInputRate::Vertex,
                VertexStepMode::Instance => vk::VertexInputRate::Instance,
            },
        })
        .collect();
    let attributes: Vec<_> = desc
        .vertex_buffers
        .iter()
        .enumerate()
        .flat_map(|(binding, buffer)| {
            buffer
                .attributes
                .iter()
                .map(move |attr| vk::VertexInputAttributeDescription {
                    location: attr.location,
                    binding: binding as u32,
                    format: vertex_format(attr.format),
                    offset: attr.offset,
                })
        })
        .collect();
    let vertex_input = vk::PipelineVertexInputStateCreateInfo {
        vertex_binding_description_count: bindings.len() as u32,
        vertex_binding_descriptions: bindings.as_ptr(),
        vertex_attribute_description_count: attributes.len() as u32,
        vertex_attribute_descriptions: attributes.as_ptr(),
        ..Default::default()
    };

    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo {
        topology: match desc.topology {
            Topology::TriangleList => vk::PrimitiveTopology::TriangleList,
            Topology::LineList => vk::PrimitiveTopology::LineList,
        },
        ..Default::default()
    };

    let viewport_state = vk::PipelineViewportStateCreateInfo {
        viewport_count: 1,
        scissor_count: 1,
        ..Default::default()
    };

    let mut rasterizer = vk::PipelineRasterizationStateCreateInfo {
        polygon_mode: vk::PolygonMode::Fill,
        cull_mode: match desc.cull_mode {
            CullMode::None => vk::CullModeFlags::None,
            CullMode::Front => vk::CullModeFlags::Front,
            CullMode::Back => vk::CullModeFlags::Back,
        },
        front_face: match desc.front_face {
            FrontFace::Ccw => vk::FrontFace::CounterClockwise,
            FrontFace::Cw => vk::FrontFace::Clockwise,
        },
        line_width: 1.0,
        depth_clamp_enable: (desc.depth_clamp && device.depth_clamp).into(),
        ..Default::default()
    };
    if let Some(bias) = desc.depth.and_then(|depth| depth.bias) {
        rasterizer.depth_bias_enable = vk::TRUE;
        rasterizer.depth_bias_constant_factor = bias.constant as f32;
        rasterizer.depth_bias_slope_factor = bias.slope;
    }

    let multisampling = vk::PipelineMultisampleStateCreateInfo {
        rasterization_samples: vk::SampleCountFlags::Type1,
        ..Default::default()
    };

    let depth_stencil = match desc.depth {
        Some(depth) => vk::PipelineDepthStencilStateCreateInfo {
            depth_test_enable: vk::TRUE,
            depth_write_enable: if depth.write { vk::TRUE } else { vk::FALSE },
            depth_compare_op: match depth.compare {
                CompareFunction::Less => vk::CompareOp::Less,
                CompareFunction::LessEqual => vk::CompareOp::LessOrEqual,
            },
            ..Default::default()
        },
        None => vk::PipelineDepthStencilStateCreateInfo::default(),
    };

    // Vulkan doesn't guarantee blending 32-bit float targets: they're
    // written unblended.
    let blend_attachments: Vec<_> = targets
        .color
        .into_iter()
        .chain(targets.extra_colors.iter().flatten().copied())
        .map(|format| {
            let blendable = !matches!(
                format,
                vk::Format::R32Sfloat | vk::Format::R32G32Sfloat | vk::Format::R32G32B32A32Sfloat
            );
            color_blend_attachment(desc.blend.filter(|_| blendable))
        })
        .collect();
    let color_blending = vk::PipelineColorBlendStateCreateInfo {
        attachment_count: blend_attachments.len() as u32,
        attachments: blend_attachments.as_ptr(),
        ..Default::default()
    };

    let dynamic_states = [vk::DynamicState::Viewport, vk::DynamicState::Scissor];
    let dynamic_state = vk::PipelineDynamicStateCreateInfo {
        dynamic_state_count: dynamic_states.len() as u32,
        dynamic_states: dynamic_states.as_ptr(),
        ..Default::default()
    };

    let pipeline_info = [vk::GraphicsPipelineCreateInfo {
        stage_count: stages.len() as u32,
        stages: stages.as_ptr(),
        vertex_input_state: &vertex_input,
        input_assembly_state: &input_assembly,
        viewport_state: &viewport_state,
        rasterization_state: &rasterizer,
        multisample_state: &multisampling,
        depth_stencil_state: &depth_stencil,
        color_blend_state: &color_blending,
        dynamic_state: &dynamic_state,
        layout: layout.0,
        render_pass,
        subpass: 0,
        ..Default::default()
    }];

    let mut pipeline = vk::Pipeline::null();
    let created = device.raw.create_graphics_pipelines(
        vk::PipelineCache::null(),
        &pipeline_info,
        None,
        slice::from_mut(&mut pipeline),
    );
    device.raw.destroy_shader_module(vert_module, None);
    device.raw.destroy_shader_module(frag_module, None);
    created.map_err(|e| format!("{e:?}"))?;
    Ok(RenderPipeline {
        raw: pipeline,
        layout,
    })
}

pub type TextureTransition = super::TextureTransition<TextureHandle>;

impl TextureUse {
    fn layout(self) -> vk::ImageLayout {
        match self {
            Self::Undefined => vk::ImageLayout::Undefined,
            Self::CopySrc | Self::DepthCopySrc => vk::ImageLayout::TransferSrcOptimal,
            Self::CopyDst | Self::DepthCopyDst => vk::ImageLayout::TransferDstOptimal,
            Self::ShaderRead => vk::ImageLayout::ShaderReadOnlyOptimal,
            Self::ColorAttachment => vk::ImageLayout::ColorAttachmentOptimal,
            Self::Present => vk::ImageLayout::PresentSrcKHR,
            Self::DepthAttachment => vk::ImageLayout::DepthStencilAttachmentOptimal,
            Self::DepthRead => vk::ImageLayout::ShaderReadOnlyOptimal,
        }
    }

    fn is_depth(self) -> bool {
        matches!(
            self,
            Self::DepthAttachment | Self::DepthRead | Self::DepthCopySrc | Self::DepthCopyDst
        )
    }

    /// The access and stage that must finish before leaving this use.
    fn src(self) -> (vk::AccessFlags, vk::PipelineStageFlags) {
        match self {
            Self::Undefined => (vk::AccessFlags::empty(), vk::PipelineStageFlags::TopOfPipe),
            // Presented images were last written as the pass's color target.
            Self::Present => (
                vk::AccessFlags::ColorAttachmentWrite,
                vk::PipelineStageFlags::ColorAttachmentOutput,
            ),
            _ => self.dst(),
        }
    }

    /// The access and stage that wait on entering this use.
    fn dst(self) -> (vk::AccessFlags, vk::PipelineStageFlags) {
        match self {
            Self::Undefined => unreachable!("textures can't move to Undefined"),
            Self::CopySrc | Self::DepthCopySrc => (
                vk::AccessFlags::TransferRead,
                vk::PipelineStageFlags::Transfer,
            ),
            Self::CopyDst | Self::DepthCopyDst => (
                vk::AccessFlags::TransferWrite,
                vk::PipelineStageFlags::Transfer,
            ),
            Self::ShaderRead | Self::DepthRead => (
                vk::AccessFlags::ShaderRead,
                vk::PipelineStageFlags::VertexShader | vk::PipelineStageFlags::FragmentShader,
            ),
            Self::DepthAttachment => (
                vk::AccessFlags::DepthStencilAttachmentRead
                    | vk::AccessFlags::DepthStencilAttachmentWrite,
                vk::PipelineStageFlags::EarlyFragmentTests
                    | vk::PipelineStageFlags::LateFragmentTests,
            ),
            Self::ColorAttachment => (
                vk::AccessFlags::ColorAttachmentWrite,
                vk::PipelineStageFlags::ColorAttachmentOutput,
            ),
            Self::Present => (
                vk::AccessFlags::empty(),
                vk::PipelineStageFlags::BottomOfPipe,
            ),
        }
    }
}

impl BufferUse {
    fn masks(self) -> (vk::AccessFlags, vk::PipelineStageFlags) {
        match self {
            Self::CopySrc => (
                vk::AccessFlags::TransferRead,
                vk::PipelineStageFlags::Transfer,
            ),
            Self::CopyDst => (
                vk::AccessFlags::TransferWrite,
                vk::PipelineStageFlags::Transfer,
            ),
            Self::StorageWrite => (
                vk::AccessFlags::ShaderWrite,
                vk::PipelineStageFlags::ComputeShader,
            ),
            Self::Indirect => (
                vk::AccessFlags::IndirectCommandRead,
                vk::PipelineStageFlags::DrawIndirect,
            ),
            Self::HostRead => (vk::AccessFlags::HostRead, vk::PipelineStageFlags::Host),
        }
    }
}

/// Records transfers, barriers and passes into a command buffer, in the
/// shape of WebGPU's `GPUCommandEncoder`.
#[derive(Clone, Copy)]
pub struct CommandEncoder {
    cmd: vk::CommandBuffer,
}

impl CommandEncoder {
    /// Wraps `cmd`, which must already be recording.
    pub fn new(cmd: vk::CommandBuffer) -> Self {
        Self { cmd }
    }

    /// One barrier for all of `transitions`.
    pub fn transition_textures(&self, transitions: &[TextureTransition]) {
        let mut src_stages = vk::PipelineStageFlags::empty();
        let mut dst_stages = vk::PipelineStageFlags::empty();
        let barriers: Vec<_> = transitions
            .iter()
            .map(|t| {
                let (src_access_mask, src_stage) = t.from.src();
                let (dst_access_mask, dst_stage) = t.to.dst();
                src_stages |= src_stage;
                dst_stages |= dst_stage;
                vk::ImageMemoryBarrier {
                    image: t.texture.image,
                    old_layout: t.from.layout(),
                    new_layout: t.to.layout(),
                    src_access_mask,
                    dst_access_mask,
                    subresource_range: vk::ImageSubresourceRange {
                        aspect_mask: if t.from.is_depth() || t.to.is_depth() {
                            vk::ImageAspectFlags::Depth
                        } else {
                            vk::ImageAspectFlags::Color
                        },
                        ..vk_range(t.range)
                    },
                    ..Default::default()
                }
            })
            .collect();
        self.cmd.pipeline_barrier(
            src_stages,
            dst_stages,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &barriers,
        );
    }

    /// Makes every buffer write of `from` visible to `to`.
    pub fn transition_buffers(&self, from: BufferUse, to: BufferUse) {
        let (src_access_mask, src_stage) = from.masks();
        let (dst_access_mask, dst_stage) = to.masks();
        self.cmd.pipeline_barrier(
            src_stage,
            dst_stage,
            vk::DependencyFlags::empty(),
            &[vk::MemoryBarrier {
                src_access_mask,
                dst_access_mask,
                ..Default::default()
            }],
            &[],
            &[],
        );
    }

    pub fn copy_buffer(&self, src: BufferHandle, dst: BufferHandle, copies: &[BufferCopy]) {
        let regions: Vec<_> = copies
            .iter()
            .map(|copy| vk::BufferCopy {
                src_offset: copy.src_offset,
                dst_offset: copy.dst_offset,
                size: copy.size,
            })
            .collect();
        self.cmd.copy_buffer(src.0, dst.0, &regions);
    }

    /// Copies into `image`, which must be in [`TextureUse::CopyDst`].
    pub fn copy_buffer_to_texture(
        &self,
        buffer: BufferHandle,
        image: TextureHandle,
        copies: &[TexelCopy],
    ) {
        self.cmd.copy_buffer_to_image(
            buffer.0,
            image.image,
            TextureUse::CopyDst.layout(),
            &buffer_image_copies(copies),
        );
    }

    /// Copies out of `image`, which must be in [`TextureUse::CopySrc`].
    pub fn copy_texture_to_buffer(
        &self,
        image: TextureHandle,
        buffer: BufferHandle,
        copies: &[TexelCopy],
    ) {
        self.cmd.copy_image_to_buffer(
            image.image,
            TextureUse::CopySrc.layout(),
            buffer.0,
            &buffer_image_copies(copies),
        );
    }

    /// `src` must be in [`TextureUse::CopySrc`] and `dst` in
    /// [`TextureUse::CopyDst`].
    pub fn blit(&self, src: TextureHandle, dst: TextureHandle, blit: &Blit) {
        let level = |mip_level| vk::ImageSubresourceLayers {
            aspect_mask: vk::ImageAspectFlags::Color,
            mip_level,
            base_array_layer: 0,
            layer_count: blit.layers,
        };
        let corners = |[width, height]: [u32; 2]| {
            [
                vk::Offset3D { x: 0, y: 0, z: 0 },
                vk::Offset3D {
                    x: width as i32,
                    y: height as i32,
                    z: 1,
                },
            ]
        };
        self.cmd.blit_image(
            src.image,
            TextureUse::CopySrc.layout(),
            dst.image,
            TextureUse::CopyDst.layout(),
            &[vk::ImageBlit {
                src_subresource: level(blit.src_level),
                src_offsets: corners(blit.src_size),
                dst_subresource: level(blit.dst_level),
                dst_offsets: corners(blit.dst_size),
            }],
            vk::Filter::Linear,
        );
    }

    /// Clears `range` of `image` ([`TextureUse::CopyDst`]) to `color`.
    pub fn clear_texture(&self, image: TextureHandle, range: TextureRange, color: [f32; 4]) {
        self.cmd.clear_color_image(
            image.image,
            TextureUse::CopyDst.layout(),
            &vk::ClearColorValue { float32: color },
            &[vk_range(range)],
        );
    }

    /// Clears a depth `image` ([`TextureUse::DepthCopyDst`]) to `depth`.
    pub fn clear_depth(&self, image: TextureHandle, depth: f32) {
        self.cmd.clear_depth_stencil_image(
            image.image,
            TextureUse::DepthCopyDst.layout(),
            &vk::ClearDepthStencilValue { depth, stencil: 0 },
            &[DEPTH_SUBRESOURCE_RANGE],
        );
    }

    /// Copies `extent` of a depth texture `src` ([`TextureUse::DepthCopySrc`])
    /// into `dst` ([`TextureUse::DepthCopyDst`]).
    pub fn copy_depth(&self, src: TextureHandle, dst: TextureHandle, extent: Extent) {
        let layers = vk::ImageSubresourceLayers {
            aspect_mask: vk::ImageAspectFlags::Depth,
            mip_level: 0,
            base_array_layer: 0,
            layer_count: 1,
        };
        self.cmd.copy_image(
            src.image,
            TextureUse::DepthCopySrc.layout(),
            dst.image,
            TextureUse::DepthCopyDst.layout(),
            &[vk::ImageCopy {
                src_subresource: layers,
                src_offset: vk::Offset3D::default(),
                dst_subresource: layers,
                dst_offset: vk::Offset3D::default(),
                extent: vk::Extent3D {
                    width: extent.width,
                    height: extent.height,
                    depth: 1,
                },
            }],
        );
    }

    /// Uploads `copies` of `staging` into `range` of a fresh `image`, leaving
    /// it ready to sample.
    pub fn upload_texture(
        &self,
        staging: BufferHandle,
        image: TextureHandle,
        range: TextureRange,
        copies: &[TexelCopy],
    ) {
        let transition = |from, to| TextureTransition {
            texture: image,
            range,
            from,
            to,
        };
        self.transition_textures(&[transition(TextureUse::Undefined, TextureUse::CopyDst)]);
        self.copy_buffer_to_texture(staging, image, copies);
        self.transition_textures(&[transition(TextureUse::CopyDst, TextureUse::ShaderRead)]);
    }

    /// Begins the pass `desc` describes, with the viewport and scissor
    /// covering its extent, as WebGPU starts every pass.
    pub fn begin_render_pass(&self, device: &Device, desc: &RenderPassDesc) -> RenderPassEncoder {
        let (render_pass, framebuffer) = device.targets.get(&device.raw, desc);
        let mut clear_values: Vec<_> = pass_colors(desc)
            .map(|color| vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: match color.load {
                        LoadOp::Clear(color) => color,
                        LoadOp::Load => [0.0; 4],
                    },
                },
            })
            .collect();
        if let Some(depth) = desc.depth {
            clear_values.push(vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: match depth.load {
                        LoadOp::Clear(depth) => depth,
                        LoadOp::Load => 1.0,
                    },
                    stencil: 0,
                },
            });
        }
        RenderPassEncoder::begin(
            self.cmd,
            render_pass,
            framebuffer,
            desc.extent,
            &clear_values,
        )
    }

    /// Dispatches `groups` workgroups of `pipeline` with `group` bound at 0.
    pub fn dispatch(&self, pipeline: &ComputePipeline, group: BindGroup, groups: [u32; 3]) {
        self.cmd
            .bind_pipeline(vk::PipelineBindPoint::Compute, pipeline.raw);
        self.cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Compute,
            pipeline.layout.0,
            0,
            &[group.raw],
            &[],
        );
        self.cmd.dispatch(groups[0], groups[1], groups[2]);
    }
}

fn buffer_image_copies(copies: &[TexelCopy]) -> Vec<vk::BufferImageCopy> {
    copies
        .iter()
        .map(|copy| vk::BufferImageCopy {
            buffer_offset: copy.buffer_offset,
            buffer_row_length: 0,
            buffer_image_height: 0,
            image_subresource: vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::Color,
                mip_level: copy.mip_level,
                base_array_layer: copy.layer,
                layer_count: 1,
            },
            image_offset: vk::Offset3D {
                x: copy.origin[0] as i32,
                y: copy.origin[1] as i32,
                z: 0,
            },
            image_extent: vk::Extent3D {
                width: copy.width,
                height: copy.height,
                depth: 1,
            },
        })
        .collect()
}

/// Records one render pass, in the shape of WebGPU's `GPURenderPassEncoder`:
/// bind groups and push constants go to the layout of the last pipeline set.
pub struct RenderPassEncoder {
    cmd: vk::CommandBuffer,
    layout: vk::PipelineLayout,
}

impl RenderPassEncoder {
    fn begin(
        cmd: vk::CommandBuffer,
        render_pass: vk::RenderPass,
        framebuffer: vk::Framebuffer,
        extent: Extent,
        clear_values: &[vk::ClearValue],
    ) -> Self {
        let full = Rect::covering(extent);
        let info = vk::RenderPassBeginInfo {
            render_pass,
            framebuffer,
            render_area: vk_rect(full),
            clear_value_count: clear_values.len() as u32,
            clear_values: clear_values.as_ptr(),
            ..Default::default()
        };
        cmd.begin_render_pass(&info, vk::SubpassContents::Inline);
        let pass = Self {
            cmd,
            layout: vk::PipelineLayout::null(),
        };
        pass.set_viewport(full);
        pass.set_scissor_rect(full);
        pass
    }

    pub fn end(self) {
        self.cmd.end_render_pass();
    }

    pub fn set_pipeline(&mut self, pipeline: &RenderPipeline) {
        self.cmd
            .bind_pipeline(vk::PipelineBindPoint::Graphics, pipeline.raw);
        self.layout = pipeline.layout.0;
    }

    /// Binds `groups` at consecutive indices from `first`.
    pub fn set_bind_groups(&self, first: u32, groups: &[BindGroup]) {
        debug_assert!(self.layout != vk::PipelineLayout::null(), "no pipeline set");
        let mut raw = [vk::DescriptorSet::null(); MAX_BIND_GROUPS];
        for (raw, group) in raw.iter_mut().zip(groups) {
            *raw = group.raw;
        }
        self.cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Graphics,
            self.layout,
            first,
            &raw[..groups.len()],
            &[],
        );
    }

    pub fn set_bind_group(&self, index: u32, group: BindGroup) {
        self.set_bind_groups(index, &[group]);
    }

    pub fn set_push_constants(&self, visibility: ShaderStages, offset: u32, data: &[u8]) {
        debug_assert!(self.layout != vk::PipelineLayout::null(), "no pipeline set");
        self.cmd
            .push_constants(self.layout, stage_flags(visibility), offset, data);
    }

    pub fn set_vertex_buffer(&self, slot: u32, buffer: BufferHandle) {
        self.cmd.bind_vertex_buffers(slot, &[buffer.0], &[0]);
    }

    /// Binds `buffer` as `u32` indices.
    pub fn set_index_buffer(&self, buffer: BufferHandle) {
        self.cmd
            .bind_index_buffer(buffer.0, 0, vk::IndexType::Uint32);
    }

    /// A viewport over `rect` with the full 0..1 depth range.
    pub fn set_viewport(&self, rect: Rect) {
        self.cmd.set_viewport(
            0,
            &[vk::Viewport {
                x: rect.x as f32,
                y: rect.y as f32,
                width: rect.width as f32,
                height: rect.height as f32,
                min_depth: 0.0,
                max_depth: 1.0,
            }],
        );
    }

    pub fn set_scissor_rect(&self, rect: Rect) {
        self.cmd.set_scissor(0, &[vk_rect(rect)]);
    }

    pub fn draw(
        &self,
        vertex_count: u32,
        instance_count: u32,
        first_vertex: u32,
        first_instance: u32,
    ) {
        self.cmd
            .draw(vertex_count, instance_count, first_vertex, first_instance);
    }

    pub fn draw_indexed(
        &self,
        index_count: u32,
        instance_count: u32,
        first_index: u32,
        base_vertex: i32,
        first_instance: u32,
    ) {
        self.cmd.draw_indexed(
            index_count,
            instance_count,
            first_index,
            base_vertex,
            first_instance,
        );
    }

    /// `draw_count` indexed draws read from `buffer`, `stride` bytes apart.
    pub fn draw_indexed_indirect(
        &self,
        buffer: BufferHandle,
        offset: u64,
        draw_count: u32,
        stride: u32,
    ) {
        self.cmd
            .draw_indexed_indirect(buffer.0, offset, draw_count, stride);
    }

    /// Like [`Self::draw_indexed_indirect`], with the draw count read from
    /// `count_buffer` and capped at `max_draws`.
    pub fn draw_indexed_indirect_count(
        &self,
        buffer: BufferHandle,
        count_buffer: BufferHandle,
        max_draws: u32,
        stride: u32,
    ) {
        self.cmd
            .draw_indexed_indirect_count(buffer.0, 0, count_buffer.0, 0, max_draws, stride);
    }

    /// Clears depth to 1.0 inside `rect` mid-pass.
    pub fn clear_depth(&self, rect: Rect) {
        self.clear(
            vk::ImageAspectFlags::Depth,
            vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: 1.0,
                    stencil: 0,
                },
            },
            rect,
        );
    }

    /// Clears the color target to `color` inside `rect` mid-pass (see
    /// [`Self::clear_depth`]).
    pub fn clear_color(&self, rect: Rect, color: [f32; 4]) {
        self.clear(
            vk::ImageAspectFlags::Color,
            vk::ClearValue {
                color: vk::ClearColorValue { float32: color },
            },
            rect,
        );
    }

    fn clear(&self, aspect_mask: vk::ImageAspectFlags, clear_value: vk::ClearValue, rect: Rect) {
        self.cmd.clear_attachments(
            &[vk::ClearAttachment {
                aspect_mask,
                color_attachment: 0,
                clear_value,
            }],
            &[vk::ClearRect {
                rect: vk_rect(rect),
                base_array_layer: 0,
                layer_count: 1,
            }],
        );
    }
}

fn vk_rect(rect: Rect) -> vk::Rect2D {
    vk::Rect2D {
        offset: vk::Offset2D {
            x: rect.x as i32,
            y: rect.y as i32,
        },
        extent: vk::Extent2D {
            width: rect.width,
            height: rect.height,
        },
    }
}

fn vk_range(range: TextureRange) -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange {
        base_mip_level: range.first_mip,
        level_count: range.mips,
        base_array_layer: range.first_layer,
        layer_count: range.layers,
        ..COLOR_SUBRESOURCE_RANGE
    }
}

const COLOR_SUBRESOURCE_RANGE: vk::ImageSubresourceRange = vk::ImageSubresourceRange {
    aspect_mask: vk::ImageAspectFlags::Color,
    base_mip_level: 0,
    level_count: 1,
    base_array_layer: 0,
    layer_count: 1,
};

const DEPTH_SUBRESOURCE_RANGE: vk::ImageSubresourceRange = vk::ImageSubresourceRange {
    aspect_mask: vk::ImageAspectFlags::Depth,
    ..COLOR_SUBRESOURCE_RANGE
};

fn vertex_format(format: VertexFormat) -> vk::Format {
    match format {
        VertexFormat::Float32 => vk::Format::R32Sfloat,
        VertexFormat::Float32x2 => vk::Format::R32G32Sfloat,
        VertexFormat::Float32x3 => vk::Format::R32G32B32Sfloat,
        VertexFormat::Float32x4 => vk::Format::R32G32B32A32Sfloat,
        VertexFormat::Sint32x3 => vk::Format::R32G32B32Sint,
        VertexFormat::Uint32 => vk::Format::R32Uint,
        VertexFormat::Sint16x2 => vk::Format::R16G16Sint,
        VertexFormat::Uint16 => vk::Format::R16Uint,
        VertexFormat::Unorm16 => vk::Format::R16Unorm,
        VertexFormat::Unorm16x2 => vk::Format::R16G16Unorm,
        VertexFormat::Uint8x2 => vk::Format::R8G8Uint,
        VertexFormat::Unorm8x4 => vk::Format::R8G8B8A8Unorm,
        VertexFormat::Snorm8x4 => vk::Format::R8G8B8A8Snorm,
    }
}

fn descriptor_type(ty: BindingType) -> vk::DescriptorType {
    match ty {
        BindingType::UniformBuffer => vk::DescriptorType::UniformBuffer,
        BindingType::StorageBuffer => vk::DescriptorType::StorageBuffer,
        BindingType::Texture => vk::DescriptorType::SampledImage,
        BindingType::Sampler => vk::DescriptorType::Sampler,
    }
}

fn stage_flags(stages: ShaderStages) -> vk::ShaderStageFlags {
    union_flags(
        vk::ShaderStageFlags::empty(),
        [
            (ShaderStages::VERTEX, vk::ShaderStageFlags::Vertex),
            (ShaderStages::FRAGMENT, vk::ShaderStageFlags::Fragment),
            (ShaderStages::COMPUTE, vk::ShaderStageFlags::Compute),
        ],
        |bit| stages.contains(bit),
    )
}

fn color_blend_attachment(blend: Option<BlendState>) -> vk::PipelineColorBlendAttachmentState {
    let Some(blend) = blend else {
        return vk::PipelineColorBlendAttachmentState {
            blend_enable: vk::FALSE,
            color_write_mask: vk::ColorComponentFlags::RGBA,
            ..Default::default()
        };
    };
    let factor = |factor| match factor {
        BlendFactor::Zero => vk::BlendFactor::Zero,
        BlendFactor::One => vk::BlendFactor::One,
        BlendFactor::SrcColor => vk::BlendFactor::SrcColor,
        BlendFactor::OneMinusSrcColor => vk::BlendFactor::OneMinusSrcColor,
        BlendFactor::DstColor => vk::BlendFactor::DstColor,
        BlendFactor::OneMinusDstColor => vk::BlendFactor::OneMinusDstColor,
        BlendFactor::SrcAlpha => vk::BlendFactor::SrcAlpha,
        BlendFactor::OneMinusSrcAlpha => vk::BlendFactor::OneMinusSrcAlpha,
    };
    vk::PipelineColorBlendAttachmentState {
        blend_enable: vk::TRUE,
        src_color_blend_factor: factor(blend.color.src),
        dst_color_blend_factor: factor(blend.color.dst),
        color_blend_op: vk::BlendOp::Add,
        src_alpha_blend_factor: factor(blend.alpha.src),
        dst_alpha_blend_factor: factor(blend.alpha.dst),
        alpha_blend_op: vk::BlendOp::Add,
        color_write_mask: vk::ColorComponentFlags::RGBA,
    }
}

/// The frames in flight: each one's command buffer, the fence its reuse waits
/// on, and the semaphores ordering it against the swapchain. WebGPU's canvas
/// does all of this itself.
pub struct Frames {
    command_buffers: [vk::CommandBuffer; MAX_FRAMES_IN_FLIGHT],
    image_available: [vk::Semaphore; MAX_FRAMES_IN_FLIGHT],
    in_flight: [vk::Fence; MAX_FRAMES_IN_FLIGHT],
    /// One per swapchain image, since an image's present waits on its own.
    render_finished: Vec<vk::Semaphore>,
    index: usize,
    /// The last present's `VK_KHR_present_id`, counted per swapchain.
    present_id: u64,
}

impl Frames {
    pub fn new(device: &Device, image_count: usize) -> Result<Self, vk::Error> {
        let alloc_info = vk::CommandBufferAllocateInfo {
            command_pool: device.command_pool,
            level: vk::CommandBufferLevel::Primary,
            command_buffer_count: MAX_FRAMES_IN_FLIGHT as u32,
            ..Default::default()
        };
        let mut command_buffers = [vk::CommandBuffer::default(); MAX_FRAMES_IN_FLIGHT];
        unsafe {
            device
                .raw
                .allocate_command_buffers(&alloc_info, &mut command_buffers)?
        };
        let fence_info = vk::FenceCreateInfo {
            flags: vk::FenceCreateFlags::Signaled,
            ..Default::default()
        };
        let mut in_flight = [vk::Fence::default(); MAX_FRAMES_IN_FLIGHT];
        for fence in &mut in_flight {
            *fence = device.raw.create_fence(&fence_info, None)?;
        }
        let image_available = create_semaphores(&device.raw, MAX_FRAMES_IN_FLIGHT)?
            .try_into()
            .expect("one per frame");
        Ok(Self {
            command_buffers,
            image_available,
            in_flight,
            render_finished: create_semaphores(&device.raw, image_count)?,
            index: 0,
            present_id: 0,
        })
    }

    /// The frame being recorded; per-frame resources are indexed by it.
    pub fn index(&self) -> usize {
        self.index
    }

    /// Waits until the GPU has finished this frame's last use.
    pub fn wait(&self, device: &Device) -> Result<(), vk::Error> {
        device
            .raw
            .wait_for_fences(&[self.in_flight[self.index]], true, u64::MAX)
    }

    pub fn wait_all(&self, device: &Device) {
        if let Err(error) = device.raw.wait_for_fences(&self.in_flight, true, u64::MAX) {
            tracing::error!(%error, "Failed to wait for all Vulkan in-flight frame fences");
        }
    }

    pub fn acquire(&self, device: &Device, swapchain: &Swapchain) -> Result<Acquired, vk::Error> {
        match device.raw.acquire_next_image(
            swapchain.handle,
            u64::MAX,
            self.image_available[self.index],
            vk::Fence::null(),
        ) {
            Ok(image) => Ok(Acquired::Image {
                index: image.value,
                suboptimal: image.suboptimal,
            }),
            Err(vk::Error::OutOfDateKHR) => Ok(Acquired::OutOfDate),
            Err(error) => Err(error),
        }
    }

    /// Starts recording the frame, once its image is acquired.
    pub fn begin(&self, device: &Device) -> Result<CommandEncoder, vk::Error> {
        device.raw.reset_fences(&[self.in_flight[self.index]])?;
        let cmd = self.command_buffers[self.index];
        cmd.reset(vk::CommandBufferResetFlags::empty())?;
        cmd.begin(&vk::CommandBufferBeginInfo {
            flags: vk::CommandBufferUsageFlags::OneTimeSubmit,
            ..Default::default()
        })?;
        Ok(CommandEncoder::new(cmd))
    }

    /// Submits the frame and presents `image` once it's drawn, then moves on
    /// to the next frame. With present wait, the present carries an id for
    /// [`Self::pace`] to wait on.
    pub fn submit_and_present(
        &mut self,
        encoder: CommandEncoder,
        ctx: &Context,
        swapchain: &Swapchain,
        image: u32,
    ) -> Result<Presented, vk::Error> {
        let queues = ctx.queues();
        encoder.cmd.end()?;
        let render_finished = self.render_finished[image as usize];
        let submit_info = vk::SubmitInfo {
            wait_semaphore_count: 1,
            wait_semaphores: &self.image_available[self.index],
            wait_dst_stage_mask: &vk::PipelineStageFlags::ColorAttachmentOutput,
            command_buffer_count: 1,
            command_buffers: &encoder.cmd.handle(),
            signal_semaphore_count: 1,
            signal_semaphores: &render_finished,
            ..Default::default()
        };
        queues
            .graphics
            .submit(&[submit_info], self.in_flight[self.index])?;

        let mut present_info = vk::PresentInfoKHR {
            wait_semaphore_count: 1,
            wait_semaphores: &render_finished,
            swapchain_count: 1,
            swapchains: &swapchain.handle,
            image_indices: &image,
            ..Default::default()
        };
        self.present_id += 1;
        let mut present_id = vk::PresentIdKHR {
            swapchain_count: 1,
            present_ids: &self.present_id,
            ..Default::default()
        };
        if ctx.present_wait {
            present_info = present_info.next(&mut present_id);
        }
        let presented = match queues.present.present(&present_info) {
            Ok(()) => Presented::Done,
            Err(vk::Error::OutOfDateKHR) => Presented::OutOfDate,
            Err(vk::Error::SuboptimalKHR) => Presented::Suboptimal,
            Err(error) => return Err(error),
        };
        self.index = (self.index + 1) % MAX_FRAMES_IN_FLIGHT;
        Ok(presented)
    }

    /// Blocks, for at most `timeout_ns`, until at most one frame is queued
    /// for display: on the present before last with `present_wait`, else on
    /// the previous frame's GPU work. False if the swapchain turned out to be
    /// out of date.
    pub fn pace(&self, ctx: &Context, swapchain: &Swapchain, timeout_ns: u64) -> bool {
        let device = &ctx.device;
        if !ctx.present_wait {
            let previous = (self.index + MAX_FRAMES_IN_FLIGHT - 1) % MAX_FRAMES_IN_FLIGHT;
            match device
                .raw
                .wait_for_fences(&[self.in_flight[previous]], true, timeout_ns)
            {
                Ok(()) => {}
                Err(vk::Error::Timeout) => {
                    tracing::trace!(timeout_ns, "Vulkan frame pacing wait exhausted its budget");
                }
                Err(error) => {
                    tracing::error!(%error, "Failed to wait for Vulkan frame pacing fence");
                }
            }
            return true;
        }
        if self.present_id < 2 {
            return true;
        }
        // Timeouts and surface loss resurface at acquire/present.
        !matches!(
            device
                .raw
                .wait_for_present(swapchain.handle, self.present_id - 1, timeout_ns),
            Err(vk::Error::OutOfDateKHR)
        )
    }

    /// Replaces the per-image semaphores for a new swapchain, whose present
    /// ids start over.
    pub fn resize(&mut self, device: &Device, image_count: usize) -> Result<(), vk::Error> {
        destroy_semaphores(&device.raw, self.render_finished.drain(..));
        self.render_finished = create_semaphores(&device.raw, image_count)?;
        self.present_id = 0;
        Ok(())
    }

    /// Frees everything but the command buffers, which go with their pool.
    pub fn destroy(&mut self, device: &Device) {
        for &fence in &self.in_flight {
            device.raw.destroy_fence(fence, None);
        }
        destroy_semaphores(&device.raw, self.image_available);
        destroy_semaphores(&device.raw, self.render_finished.drain(..));
    }
}

/// The queues a frame is submitted and presented on.
#[derive(Clone, Copy)]
struct Queues {
    graphics: vk::Queue,
    present: vk::Queue,
}

fn create_semaphores(device: &vk::Device, count: usize) -> Result<Vec<vk::Semaphore>, vk::Error> {
    (0..count)
        .map(|_| device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None))
        .collect()
}

fn destroy_semaphores(device: &vk::Device, semaphores: impl IntoIterator<Item = vk::Semaphore>) {
    for semaphore in semaphores {
        device.destroy_semaphore(semaphore, None);
    }
}

pub type ColorAttachment = super::ColorAttachment<TextureHandle, vk::Format>;
pub type DepthAttachment = super::DepthAttachment<TextureHandle>;
pub type RenderPassDesc = super::RenderPassDesc<TextureHandle, vk::Format>;

/// The render passes and framebuffers Vulkan begins a pass with, each made
/// the first time it's wanted: WebGPU begins one from its attachments alone.
/// A pass leaves its attachments in their attachment layout, for a
/// [`TextureTransition`] to move on.
#[derive(Default)]
pub struct RenderTargets(Mutex<TargetCache>);

#[derive(Default)]
struct TargetCache {
    passes: HashMap<PassKey, vk::RenderPass>,
    framebuffers: HashMap<FramebufferKey, vk::Framebuffer>,
}

/// A pass's color targets: format, and whether it loads, in location order.
type ColorKeys = [Option<(vk::Format, bool)>; MAX_COLOR_TARGETS];

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct PassKey {
    colors: ColorKeys,
    color_store: StoreOp,
    depth: Option<(bool, StoreOp)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct FramebufferKey {
    render_pass: vk::RenderPass,
    colors: [vk::ImageView; MAX_COLOR_TARGETS],
    depth: vk::ImageView,
    width: u32,
    height: u32,
}

/// The color keys of `targets`, none loading.
fn target_colors(targets: &PipelineTargets) -> ColorKeys {
    let mut colors = [None; MAX_COLOR_TARGETS];
    colors[0] = targets.color.map(|f| (f, false));
    for (slot, format) in colors[1..].iter_mut().zip(targets.extra_colors) {
        *slot = format.map(|f| (f, false));
    }
    colors
}

/// The color attachments of `desc`, in location order.
fn pass_colors(desc: &RenderPassDesc) -> impl Iterator<Item = &ColorAttachment> {
    desc.color.iter().chain(&desc.extra_colors)
}

impl RenderTargets {
    /// A pass for making pipelines that draw into `color`, and a depth target
    /// if `depth`: Vulkan makes a pipeline against a pass, and any with the
    /// same formats will do.
    fn compatible_pass(&self, device: &Device, targets: &PipelineTargets) -> vk::RenderPass {
        self.cache().pass(
            &device.raw,
            PassKey {
                colors: target_colors(targets),
                color_store: StoreOp::Store,
                depth: targets.depth.then_some((false, StoreOp::Discard)),
            },
        )
    }

    fn get(&self, device: &vk::Device, desc: &RenderPassDesc) -> (vk::RenderPass, vk::Framebuffer) {
        let mut cache = self.cache();
        let mut colors = [None; MAX_COLOR_TARGETS];
        let mut views = [vk::ImageView::null(); MAX_COLOR_TARGETS];
        for (i, color) in pass_colors(desc).enumerate() {
            colors[i] = Some((color.format, matches!(color.load, LoadOp::Load)));
            views[i] = color.texture.view;
        }
        let render_pass = cache.pass(
            device,
            PassKey {
                colors,
                color_store: desc.color.as_ref().map_or(StoreOp::Store, |c| c.store),
                depth: desc
                    .depth
                    .map(|depth| (matches!(depth.load, LoadOp::Load), depth.store)),
            },
        );
        let key = FramebufferKey {
            render_pass,
            colors: views,
            depth: desc
                .depth
                .map_or(vk::ImageView::null(), |depth| depth.texture.view),
            width: desc.extent.width,
            height: desc.extent.height,
        };
        let framebuffer = *cache.framebuffers.entry(key).or_insert_with(|| {
            let mut attachments: Vec<vk::ImageView> = key
                .colors
                .iter()
                .copied()
                .filter(|view| *view != vk::ImageView::null())
                .collect();
            if desc.depth.is_some() {
                attachments.push(key.depth);
            }
            let info = vk::FramebufferCreateInfo {
                render_pass,
                attachment_count: attachments.len() as u32,
                attachments: attachments.as_ptr(),
                width: key.width,
                height: key.height,
                layers: 1,
                ..Default::default()
            };
            device
                .create_framebuffer(&info, None)
                .expect("failed to create framebuffer")
        });
        (render_pass, framebuffer)
    }

    fn forget(&self, device: &Device, textures: &[TextureHandle]) {
        self.cache().framebuffers.retain(|key, &mut framebuffer| {
            let stale = textures
                .iter()
                .any(|texture| key.colors.contains(&texture.view) || texture.view == key.depth);
            if stale {
                device.raw.destroy_framebuffer(framebuffer, None);
            }
            !stale
        });
    }

    pub fn destroy(&self, device: &Device) {
        let mut cache = self.cache();
        for (_, framebuffer) in cache.framebuffers.drain() {
            device.raw.destroy_framebuffer(framebuffer, None);
        }
        for (_, render_pass) in cache.passes.drain() {
            device.raw.destroy_render_pass(render_pass, None);
        }
    }

    fn cache(&self) -> MutexGuard<'_, TargetCache> {
        lock(&self.0)
    }
}

impl TargetCache {
    fn pass(&mut self, device: &vk::Device, key: PassKey) -> vk::RenderPass {
        *self
            .passes
            .entry(key)
            .or_insert_with(|| create_render_pass(device, key))
    }
}

fn create_render_pass(device: &vk::Device, key: PassKey) -> vk::RenderPass {
    let attachment = |format, load: bool, store, layout| vk::AttachmentDescription {
        format,
        samples: vk::SampleCountFlags::Type1,
        load_op: if load {
            vk::AttachmentLoadOp::Load
        } else {
            vk::AttachmentLoadOp::Clear
        },
        store_op: match store {
            StoreOp::Store => vk::AttachmentStoreOp::Store,
            StoreOp::Discard => vk::AttachmentStoreOp::DontCare,
        },
        stencil_load_op: vk::AttachmentLoadOp::DontCare,
        stencil_store_op: vk::AttachmentStoreOp::DontCare,
        initial_layout: if load {
            layout
        } else {
            vk::ImageLayout::Undefined
        },
        final_layout: layout,
        ..Default::default()
    };
    let mut attachments: Vec<_> = key
        .colors
        .iter()
        .flatten()
        .map(|&(format, load)| {
            attachment(
                format,
                load,
                key.color_store,
                vk::ImageLayout::ColorAttachmentOptimal,
            )
        })
        .collect();
    let color_count = attachments.len() as u32;
    if let Some((load, store)) = key.depth {
        attachments.push(attachment(
            vk::Format::D32Sfloat,
            load,
            store,
            vk::ImageLayout::DepthStencilAttachmentOptimal,
        ));
    }
    let color_refs: Vec<_> = (0..color_count)
        .map(|attachment| vk::AttachmentReference {
            attachment,
            layout: vk::ImageLayout::ColorAttachmentOptimal,
        })
        .collect();
    let depth_ref = vk::AttachmentReference {
        attachment: color_count,
        layout: vk::ImageLayout::DepthStencilAttachmentOptimal,
    };
    let subpass = vk::SubpassDescription {
        pipeline_bind_point: vk::PipelineBindPoint::Graphics,
        color_attachment_count: color_count,
        color_attachments: color_refs.as_ptr(),
        depth_stencil_attachment: if key.depth.is_some() {
            &depth_ref
        } else {
            std::ptr::null()
        },
        ..Default::default()
    };
    // After earlier writes to the targets, and earlier shader reads of them.
    let attachment_stages = vk::PipelineStageFlags::ColorAttachmentOutput
        | vk::PipelineStageFlags::EarlyFragmentTests
        | vk::PipelineStageFlags::LateFragmentTests;
    let dependency = vk::SubpassDependency {
        src_subpass: vk::SUBPASS_EXTERNAL,
        dst_subpass: 0,
        src_stage_mask: attachment_stages | vk::PipelineStageFlags::FragmentShader,
        src_access_mask: vk::AccessFlags::ColorAttachmentWrite
            | vk::AccessFlags::DepthStencilAttachmentWrite,
        dst_stage_mask: attachment_stages,
        dst_access_mask: vk::AccessFlags::ColorAttachmentRead
            | vk::AccessFlags::ColorAttachmentWrite
            | vk::AccessFlags::DepthStencilAttachmentRead
            | vk::AccessFlags::DepthStencilAttachmentWrite,
        ..Default::default()
    };
    let info = vk::RenderPassCreateInfo {
        attachment_count: attachments.len() as u32,
        attachments: attachments.as_ptr(),
        subpass_count: 1,
        subpasses: &subpass,
        dependency_count: 1,
        dependencies: &dependency,
        ..Default::default()
    };
    device
        .create_render_pass(&info, None)
        .expect("failed to create render pass")
}

/// WebGPU's default, and the fewest Vulkan guarantees.
const MAX_BIND_GROUPS: usize = 4;

/// Bind groups per pool, and descriptors of each type.
const GROUPS_PER_POOL: u32 = 256;

/// A descriptor set written once, as a WebGPU `GPUBindGroup` can't change.
/// The default is an empty placeholder.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct BindGroup {
    raw: vk::DescriptorSet,
    pool: vk::DescriptorPool,
}

impl BindGroup {
    pub fn destroy(self, device: &Device) {
        if self.raw != vk::DescriptorSet::null()
            && let Err(error) = device.raw.free_descriptor_sets(self.pool, &[self.raw])
        {
            tracing::error!(%error, "Failed to free Vulkan descriptor set");
        }
    }

    /// Points the group at new resources. A bind group can't change, so this
    /// makes a new one and frees the old, which the GPU must be done with.
    pub fn replace(
        &mut self,
        device: &Device,
        layout: BindGroupLayout,
        bindings: &[(u32, BindingResource)],
    ) {
        std::mem::replace(self, device.create_bind_group(layout, bindings)).destroy(device);
    }
}

/// The pools bind groups come from, added as they fill: WebGPU makes bind
/// groups from the device alone.
#[derive(Default)]
struct BindGroupPools(Mutex<Vec<vk::DescriptorPool>>);

impl BindGroupPools {
    fn allocate(
        &self,
        device: &vk::Device,
        layout: vk::DescriptorSetLayout,
    ) -> (vk::DescriptorSet, vk::DescriptorPool) {
        let mut pools = lock(&self.0);
        let allocate = |pool| {
            let info = vk::DescriptorSetAllocateInfo {
                descriptor_pool: pool,
                descriptor_set_count: 1,
                set_layouts: &layout,
                ..Default::default()
            };
            let mut set = vk::DescriptorSet::null();
            device
                .allocate_descriptor_sets(&info, slice::from_mut(&mut set))
                .map(|()| (set, pool))
        };
        if let Some(found) = pools.iter().rev().find_map(|&pool| allocate(pool).ok()) {
            return found;
        }
        let sizes = [
            BindingType::UniformBuffer,
            BindingType::StorageBuffer,
            BindingType::Texture,
            BindingType::Sampler,
        ]
        .map(|ty| vk::DescriptorPoolSize {
            ty: descriptor_type(ty),
            descriptor_count: GROUPS_PER_POOL,
        });
        let info = vk::DescriptorPoolCreateInfo {
            flags: vk::DescriptorPoolCreateFlags::FreeDescriptorSet,
            max_sets: GROUPS_PER_POOL,
            pool_size_count: sizes.len() as u32,
            pool_sizes: sizes.as_ptr(),
            ..Default::default()
        };
        let pool = device
            .create_descriptor_pool(&info, None)
            .expect("failed to create descriptor pool");
        pools.push(pool);
        allocate(pool).expect("failed to allocate a bind group")
    }

    fn destroy(&self, device: &vk::Device) {
        for pool in lock(&self.0).drain(..) {
            device.destroy_descriptor_pool(pool, None);
        }
    }
}

fn shader_module(device: &Device, spirv: &[u8]) -> vk::ShaderModule {
    try_shader_module(device, spirv).unwrap_or_else(|e| panic!("{e}"))
}

fn try_shader_module(device: &Device, spirv: &[u8]) -> Result<vk::ShaderModule, String> {
    let code = read_spv(&mut io::Cursor::new(spirv))
        .map_err(|e| format!("failed to read SPIR-V bytecode: {e}"))?;
    let info = vk::ShaderModuleCreateInfo {
        code_size: code.len() * 4,
        code: code.as_ptr(),
        ..Default::default()
    };
    device
        .raw
        .create_shader_module(&info, None)
        .map_err(|e| format!("failed to create shader module: {e:?}"))
}

// The code below is extracted from ash-rs/ash under the MIT license;
// for the full license, see THIRD_PARTY_LICENSES.md

/// Decode SPIR-V from bytes.
///
/// This function handles SPIR-V of arbitrary endianness gracefully, and returns
/// correctly aligned storage.
fn read_spv<R: io::Read + io::Seek>(x: &mut R) -> io::Result<Vec<u32>> {
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
