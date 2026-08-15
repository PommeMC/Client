//! Culling and indirect-draw responsibilities for the chunk renderer.
//!
//! This boundary owns the metadata/indirect ABI and the cull, water-scan, and
//! water-emit synchronization protocol.

use pyronyx::vk;

use crate::renderer::buffer::Buffer;
use crate::renderer::{ChunkDrawBackend, shader};

pub(crate) struct ChunkCulling {
    pub(crate) backend: ChunkDrawBackend,
    pub(crate) max_meta: usize,
    /// Allocated commands in each solid, cutout, and water indirect buffer.
    pub(crate) draw_capacity: usize,
    /// Largest count observed beyond the current capacity. The upload phase
    /// consumes this request and doubles the buffers before the next cull.
    pub(crate) requested_draw_capacity: usize,
    pub(crate) max_draw_indirect_count: u32,
    pub(crate) warned_draw_cap: bool,
    pub(crate) compute_pipeline: vk::Pipeline,
    pub(crate) region_prepare_pipeline: vk::Pipeline,
    pub(crate) section_expand_pipeline: vk::Pipeline,
    pub(crate) finalize_pipeline: vk::Pipeline,
    pub(crate) task_dispatch_pipeline: vk::Pipeline,
    pub(crate) water_scan_pipeline: vk::Pipeline,
    pub(crate) water_emit_pipeline: vk::Pipeline,
    pub(crate) compute_layout: vk::PipelineLayout,
    pub(crate) compute_desc_layout: vk::DescriptorSetLayout,
    pub(crate) compute_pool: vk::DescriptorPool,
    pub(crate) compute_sets: Vec<vk::DescriptorSet>,
    pub(crate) meta_buffers: Vec<Buffer>,
    pub(crate) indirect_buffers: Vec<Buffer>,
    pub(crate) count_buffers: Vec<Buffer>,
    pub(crate) indirect_cutout_buffers: Vec<Buffer>,
    pub(crate) count_cutout_buffers: Vec<Buffer>,
    pub(crate) task_command_buffers: Vec<Buffer>,
    pub(crate) task_command_cutout_buffers: Vec<Buffer>,
    pub(crate) water_indirect_buffers: Vec<Buffer>,
    pub(crate) water_count_buffers: Vec<Buffer>,
    pub(crate) water_bucket_buffers: Vec<Buffer>,
    pub(crate) water_candidate_buffers: Vec<Buffer>,
    pub(crate) frustum_buffers: Vec<Buffer>,
    pub(crate) region_meta_buffers: Vec<Buffer>,
    pub(crate) region_candidate_buffers: Vec<Buffer>,
    pub(crate) region_command_buffers: Vec<Buffer>,
    pub(crate) region_visibility_buffers: Vec<Buffer>,
    pub(crate) section_candidate_buffers: Vec<Buffer>,
    pub(crate) section_command_buffers: Vec<Buffer>,
    pub(crate) section_visibility_buffers: Vec<Buffer>,
    pub(crate) history_buffer: Buffer,
    pub(crate) stats_buffers: Vec<Buffer>,
    pub(crate) history_reset_pending: bool,
    pub(crate) fade_enabled: bool,
    pub(crate) last_draw_count: u32,
}

impl ChunkCulling {
    pub(crate) fn clamped_draw_count(&self) -> u32 {
        u32::try_from(self.draw_capacity)
            .unwrap_or(u32::MAX)
            .min(self.max_draw_indirect_count)
    }
}

pub(crate) fn create_compute_pipeline(
    device: &vk::Device,
    layout: vk::PipelineLayout,
    spirv: &[u8],
    spec_info: Option<&vk::SpecializationInfo>,
) -> vk::Pipeline {
    let module = shader::create_shader_module(device, spirv);
    let stage = vk::PipelineShaderStageCreateInfo {
        stage: vk::ShaderStageFlags::Compute,
        module,
        name: c"main".as_ptr(),
        specialization_info: spec_info.map_or(std::ptr::null(), |s| s as *const _),
        ..Default::default()
    };
    let pipe_info = [vk::ComputePipelineCreateInfo {
        stage,
        layout,
        ..Default::default()
    }];
    let mut pipeline = vk::Pipeline::null();
    device
        .create_compute_pipelines(
            vk::PipelineCache::null(),
            &pipe_info,
            None,
            std::slice::from_mut(&mut pipeline),
        )
        .expect("failed to create compute pipeline");
    device.destroy_shader_module(module, None);
    pipeline
}

pub(crate) fn create_cull_desc_layout(
    device: &vk::Device,
    backend: ChunkDrawBackend,
) -> vk::DescriptorSetLayout {
    let bindings: Vec<vk::DescriptorSetLayoutBinding> = (0..=25)
        .map(|binding| vk::DescriptorSetLayoutBinding {
            binding,
            descriptor_type: match binding {
                1 => vk::DescriptorType::UniformBuffer,
                _ => vk::DescriptorType::StorageBuffer,
            },
            descriptor_count: 1,
            stage_flags: vk::ShaderStageFlags::Compute
                | if backend == ChunkDrawBackend::Task && matches!(binding, 2 | 3 | 4 | 5 | 23) {
                    vk::ShaderStageFlags::TaskEXT
                } else {
                    vk::ShaderStageFlags::empty()
                },
            ..Default::default()
        })
        .collect();
    let info = vk::DescriptorSetLayoutCreateInfo {
        binding_count: bindings.len() as u32,
        bindings: bindings.as_ptr(),
        ..Default::default()
    };
    device
        .create_descriptor_set_layout(&info, None)
        .expect("failed to create cull desc layout")
}

pub(crate) fn desc_write(
    set: vk::DescriptorSet,
    binding: u32,
    ty: vk::DescriptorType,
    buffer: vk::Buffer,
    range: u64,
) -> (
    [vk::DescriptorBufferInfo; 1],
    vk::WriteDescriptorSet<'static>,
) {
    let info = [vk::DescriptorBufferInfo {
        buffer,
        offset: 0,
        range,
    }];
    let write = vk::WriteDescriptorSet {
        dst_set: set,
        dst_binding: binding,
        descriptor_count: 1,
        descriptor_type: ty,
        ..Default::default()
    };
    (info, write)
}
