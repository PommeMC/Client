// ChunkRendererState resources responsibilities.

use super::*;

impl ChunkRendererCore {
    fn create_global_cuboid_buffer(
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        global_cuboids: &[CuboidData],
    ) -> Buffer {
        let global_bytes = (global_cuboids.len().max(1) as u64 * GLOBAL_CUBOID_SIZE).max(8);
        let mut buffer = Buffer::host(
            device,
            allocator,
            global_bytes,
            vk::BufferUsageFlags::StorageBuffer,
            "global_cuboids_constant",
        );
        if !global_cuboids.is_empty() {
            let bytes: &[u8] = bytemuck::cast_slice(global_cuboids);
            buffer.mapped_slice_mut()[..bytes.len()].copy_from_slice(bytes);
        }
        buffer
    }

    pub(in crate::renderer::chunk) fn replace_global_cuboids(
        &mut self,
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        global_cuboids: &[CuboidData],
    ) {
        let replacement = Self::create_global_cuboid_buffer(device, allocator, global_cuboids);
        std::mem::replace(&mut self.global_cuboid_buffer, replacement).destroy(device, allocator);
    }

    pub fn new(
        device: &vk::Device,
        physical_device: vk::PhysicalDevice,
        allocator: &Arc<Mutex<Allocator>>,
        global_cuboids: &[CuboidData],
        render_distance: u32,
        backend: ChunkDrawBackend,
        task_limits: TaskShaderLimits,
    ) -> Self {
        let dev_props = physical_device.get_properties();
        let descriptor_buckets =
            u64::from(dev_props.limits.max_storage_buffer_range) / MESH_POOL_BYTES_PER_BUCKET;
        let total_buckets = compute_bucket_count(physical_device)
            .min(u32::try_from(descriptor_buckets).unwrap_or(u32::MAX).max(1));
        let mesh_size = total_buckets as u64 * MESH_POOL_BYTES_PER_BUCKET;

        let use_staging = dev_props.device_type == vk::PhysicalDeviceType::DiscreteGpu;
        // Spec floor is 65535 even with multiDrawIndirect; meta_high_water
        // passes it from ~RD 32, so every draw clamps to the device cap.
        let task_dispatch_width = task_limits.max_task_work_group_count[0]
            .min(task_limits.max_task_work_group_total_count)
            .max(1);
        let max_draw_indirect_count = if backend == ChunkDrawBackend::Task {
            let total = task_limits.max_task_work_group_total_count;
            let rectangular_safe = if total <= task_dispatch_width {
                total
            } else {
                total.saturating_sub(task_dispatch_width - 1)
            };
            rectangular_safe
                .min(task_dispatch_width.saturating_mul(task_limits.max_task_work_group_count[1]))
                .min(dev_props.limits.max_draw_indirect_count)
        } else {
            dev_props.limits.max_draw_indirect_count
        };

        let mesh_buffer = if use_staging {
            Buffer::device(
                device,
                allocator,
                mesh_size,
                vk::BufferUsageFlags::StorageBuffer,
                "mesh_pool",
            )
        } else {
            Buffer::host(
                device,
                allocator,
                mesh_size,
                vk::BufferUsageFlags::StorageBuffer,
                "mesh_pool",
            )
        };
        let global_cuboid_buffer =
            Self::create_global_cuboid_buffer(device, allocator, global_cuboids);

        // Discrete GPUs batch a frame's uploads through this buffer in one
        // transfer, so size it to hold several columns and keep sub-flushes rare.
        // The integrated path writes mapped memory directly and never touches it.
        let staging_size = if use_staging {
            BYTES_PER_BUCKET * 16
        } else {
            BYTES_PER_BUCKET * 4
        };
        let mut staging_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        if use_staging {
            for _ in 0..MAX_FRAMES_IN_FLIGHT {
                let buffer = Buffer::host(
                    device,
                    allocator,
                    staging_size,
                    vk::BufferUsageFlags::TransferSrc,
                    "staging",
                );
                staging_buffers.push(buffer);
            }
        }

        tracing::info!(
            "Chunk buffers: {} (vertex={} MB, staging={} KB)",
            if use_staging {
                "DEVICE_LOCAL + staging"
            } else {
                "HOST_VISIBLE"
            },
            mesh_size / (1024 * 1024),
            staging_size / 1024,
        );

        // Per-section packing yields many more draws than buckets, so pre-size
        // generously: growth (`ensure_meta_capacity`) needs a `device.wait_idle`
        // to safely rewrite the descriptor sets, and that stall showed up as a
        // 27ms frame when an RD-32 world (~45k section draws) crossed 16x. The
        // grow path stays as a rare safety net.
        let max_meta = (total_buckets * 32).max(8192) as usize;
        // Size draws from the requested startup view: a few terrain batches
        // plus one translucent batch for each of the default overworld's 24
        // sections per column. Runtime overflow readback grows this
        // independently from metadata by doubling.
        let draw_capacity = initial_draw_capacity(render_distance)
            .min(max_draw_indirect_count as usize)
            .max(1);
        let max_regions = (max_meta / 64).max(256);
        let meta_size = (max_meta * size_of::<ChunkMeta>()) as u64;
        let region_meta_size = (max_regions * size_of::<RegionMeta>()) as u64;
        let indirect_size = (draw_capacity * size_of::<DrawCommand>()) as u64;
        tracing::info!(
            "Chunk indirect capacity: {} commands (render distance {})",
            draw_capacity,
            render_distance,
        );
        // The first word is the indirect/task candidate count. The second is
        // its allocation capacity, used by task_dispatch.comp to clamp an
        // intentionally overflowing atomic count before dispatching tasks.
        let count_size = 8u64;
        let task_command_size = size_of::<vk::DrawMeshTasksIndirectCommandEXT>() as u64;
        let aabb_command_size = 20u64;
        let frustum_size = size_of::<FrustumData>() as u64;

        let mut meta_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut indirect_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut count_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut indirect_cutout_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut count_cutout_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut task_command_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut task_command_cutout_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut frustum_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut water_count_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut water_bucket_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut region_meta_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut region_candidate_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut region_command_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut region_visibility_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut section_candidate_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut section_command_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut section_visibility_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut stats_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);

        for _ in 0..MAX_FRAMES_IN_FLIGHT {
            let buffer = Buffer::host(
                device,
                allocator,
                meta_size,
                vk::BufferUsageFlags::StorageBuffer | vk::BufferUsageFlags::VertexBuffer,
                "chunk_meta",
            );
            meta_buffers.push(buffer);

            let buffer = Buffer::host(
                device,
                allocator,
                indirect_size,
                vk::BufferUsageFlags::StorageBuffer
                    | vk::BufferUsageFlags::IndirectBuffer
                    | vk::BufferUsageFlags::TransferDst,
                "indirect_cmds",
            );
            indirect_buffers.push(buffer);

            let mut buffer = Buffer::host(
                device,
                allocator,
                count_size,
                vk::BufferUsageFlags::StorageBuffer
                    | vk::BufferUsageFlags::IndirectBuffer
                    | vk::BufferUsageFlags::TransferDst,
                "draw_count",
            );
            buffer.mapped_slice_mut()[..8]
                .copy_from_slice(bytemuck::bytes_of(&[0u32, draw_capacity as u32]));
            count_buffers.push(buffer);

            let buffer = Buffer::host(
                device,
                allocator,
                indirect_size,
                vk::BufferUsageFlags::StorageBuffer
                    | vk::BufferUsageFlags::IndirectBuffer
                    | vk::BufferUsageFlags::TransferDst,
                "indirect_cmds_cutout",
            );
            indirect_cutout_buffers.push(buffer);

            let mut buffer = Buffer::host(
                device,
                allocator,
                count_size,
                vk::BufferUsageFlags::StorageBuffer
                    | vk::BufferUsageFlags::IndirectBuffer
                    | vk::BufferUsageFlags::TransferDst,
                "draw_count_cutout",
            );
            buffer.mapped_slice_mut()[..8]
                .copy_from_slice(bytemuck::bytes_of(&[0u32, draw_capacity as u32]));
            count_cutout_buffers.push(buffer);

            task_command_buffers.push(Buffer::device(
                device,
                allocator,
                task_command_size,
                vk::BufferUsageFlags::StorageBuffer | vk::BufferUsageFlags::IndirectBuffer,
                "task_command",
            ));
            task_command_cutout_buffers.push(Buffer::device(
                device,
                allocator,
                task_command_size,
                vk::BufferUsageFlags::StorageBuffer | vk::BufferUsageFlags::IndirectBuffer,
                "task_command_cutout",
            ));

            let buffer = Buffer::host(
                device,
                allocator,
                frustum_size,
                vk::BufferUsageFlags::UniformBuffer,
                "frustum_ubo",
            );
            frustum_buffers.push(buffer);

            let buffer = Buffer::host(
                device,
                allocator,
                count_size,
                vk::BufferUsageFlags::StorageBuffer | vk::BufferUsageFlags::IndirectBuffer,
                "water_count",
            );
            water_count_buffers.push(buffer);

            // +1 slot past the buckets holds the candidate counter.
            let buffer = Buffer::device(
                device,
                allocator,
                (WATER_BUCKETS as u64 + 1) * 4,
                vk::BufferUsageFlags::StorageBuffer,
                "water_buckets",
            );
            water_bucket_buffers.push(buffer);

            region_meta_buffers.push(Buffer::host(
                device,
                allocator,
                region_meta_size,
                vk::BufferUsageFlags::StorageBuffer,
                "region_meta",
            ));
            region_candidate_buffers.push(Buffer::device(
                device,
                allocator,
                max_regions as u64 * 4,
                vk::BufferUsageFlags::StorageBuffer,
                "region_candidates",
            ));
            region_command_buffers.push(Buffer::host(
                device,
                allocator,
                aabb_command_size,
                vk::BufferUsageFlags::StorageBuffer | vk::BufferUsageFlags::IndirectBuffer,
                "region_aabb_command",
            ));
            region_visibility_buffers.push(Buffer::device(
                device,
                allocator,
                max_regions as u64 * 4,
                vk::BufferUsageFlags::StorageBuffer | vk::BufferUsageFlags::TransferDst,
                "region_visibility",
            ));
            section_candidate_buffers.push(Buffer::device(
                device,
                allocator,
                max_meta as u64 * 4,
                vk::BufferUsageFlags::StorageBuffer,
                "section_candidates",
            ));
            section_command_buffers.push(Buffer::host(
                device,
                allocator,
                aabb_command_size,
                vk::BufferUsageFlags::StorageBuffer | vk::BufferUsageFlags::IndirectBuffer,
                "section_aabb_command",
            ));
            section_visibility_buffers.push(Buffer::device(
                device,
                allocator,
                max_meta as u64 * 4,
                vk::BufferUsageFlags::StorageBuffer | vk::BufferUsageFlags::TransferDst,
                "section_visibility",
            ));
            let mut stats = Buffer::host(
                device,
                allocator,
                32,
                vk::BufferUsageFlags::StorageBuffer,
                "occlusion_stats",
            );
            stats.mapped_slice_mut().fill(0);
            stats_buffers.push(stats);
        }

        let history_buffer = Buffer::device(
            device,
            allocator,
            max_meta as u64 * 4,
            vk::BufferUsageFlags::StorageBuffer | vk::BufferUsageFlags::TransferDst,
            "section_visibility_history",
        );

        let (water_indirect_buffers, water_candidate_buffers) =
            create_water_scaled_buffers(device, allocator, max_meta, indirect_size);

        let compute_desc_layout = create_cull_desc_layout(device, backend);
        let layout_info = vk::PipelineLayoutCreateInfo {
            set_layout_count: 1,
            set_layouts: &compute_desc_layout,
            ..Default::default()
        };
        let compute_layout = device
            .create_pipeline_layout(&layout_info, None)
            .expect("failed to create compute pipeline layout");

        let backend_value = u32::from(backend == ChunkDrawBackend::Task);
        let map_entry = [vk::SpecializationMapEntry {
            constant_id: 0,
            offset: 0,
            size: size_of::<u32>(),
        }];
        let specialization = vk::SpecializationInfo {
            map_entry_count: 1,
            map_entries: map_entry.as_ptr(),
            data_size: size_of::<u32>(),
            data: (&backend_value as *const u32).cast(),
            ..Default::default()
        };
        let compute_pipeline = create_compute_pipeline(
            device,
            compute_layout,
            shader::include_spirv!("cull.comp.spv"),
            Some(&specialization),
        );
        let region_prepare_pipeline = create_compute_pipeline(
            device,
            compute_layout,
            shader::include_spirv!("region_prepare.comp.spv"),
            None,
        );
        let section_expand_pipeline = create_compute_pipeline(
            device,
            compute_layout,
            shader::include_spirv!("section_expand.comp.spv"),
            None,
        );
        let finalize_pipeline = create_compute_pipeline(
            device,
            compute_layout,
            shader::include_spirv!("cull_finalize.comp.spv"),
            Some(&specialization),
        );
        let task_width_entry = [vk::SpecializationMapEntry {
            constant_id: 0,
            offset: 0,
            size: size_of::<u32>(),
        }];
        let task_width_specialization = vk::SpecializationInfo {
            map_entry_count: 1,
            map_entries: task_width_entry.as_ptr(),
            data_size: size_of::<u32>(),
            data: (&task_dispatch_width as *const u32).cast(),
            ..Default::default()
        };
        let task_dispatch_pipeline = create_compute_pipeline(
            device,
            compute_layout,
            shader::include_spirv!("task_dispatch.comp.spv"),
            Some(&task_width_specialization),
        );
        let water_scan_pipeline = create_compute_pipeline(
            device,
            compute_layout,
            shader::include_spirv!("water_scan.comp.spv"),
            None,
        );
        let water_emit_pipeline = create_compute_pipeline(
            device,
            compute_layout,
            shader::include_spirv!("water_emit.comp.spv"),
            None,
        );

        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::StorageBuffer,
                descriptor_count: 25 * MAX_FRAMES_IN_FLIGHT as u32,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UniformBuffer,
                descriptor_count: MAX_FRAMES_IN_FLIGHT as u32,
            },
        ];
        let pool_info = vk::DescriptorPoolCreateInfo {
            max_sets: MAX_FRAMES_IN_FLIGHT as u32,
            pool_size_count: pool_sizes.len() as u32,
            pool_sizes: pool_sizes.as_ptr(),
            ..Default::default()
        };
        let compute_pool = device
            .create_descriptor_pool(&pool_info, None)
            .expect("failed to create cull desc pool");

        let layouts: Vec<_> = (0..MAX_FRAMES_IN_FLIGHT)
            .map(|_| compute_desc_layout)
            .collect();
        let alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool: compute_pool,
            descriptor_set_count: layouts.len() as u32,
            set_layouts: layouts.as_ptr(),
            ..Default::default()
        };
        let mut compute_sets = vec![vk::DescriptorSet::null(); layouts.len()];
        device
            .allocate_descriptor_sets(&alloc_info, &mut compute_sets)
            .expect("failed to allocate cull desc sets");

        for i in 0..MAX_FRAMES_IN_FLIGHT {
            let bindings = [
                (
                    0,
                    vk::DescriptorType::StorageBuffer,
                    meta_buffers[i].buffer,
                    meta_size,
                ),
                (
                    1,
                    vk::DescriptorType::UniformBuffer,
                    frustum_buffers[i].buffer,
                    frustum_size,
                ),
                (
                    2,
                    vk::DescriptorType::StorageBuffer,
                    indirect_buffers[i].buffer,
                    indirect_size,
                ),
                (
                    3,
                    vk::DescriptorType::StorageBuffer,
                    count_buffers[i].buffer,
                    count_size,
                ),
                (
                    4,
                    vk::DescriptorType::StorageBuffer,
                    indirect_cutout_buffers[i].buffer,
                    indirect_size,
                ),
                (
                    5,
                    vk::DescriptorType::StorageBuffer,
                    count_cutout_buffers[i].buffer,
                    count_size,
                ),
                (
                    6,
                    vk::DescriptorType::StorageBuffer,
                    region_meta_buffers[i].buffer,
                    region_meta_size,
                ),
                (
                    7,
                    vk::DescriptorType::StorageBuffer,
                    region_candidate_buffers[i].buffer,
                    max_regions as u64 * 4,
                ),
                (
                    8,
                    vk::DescriptorType::StorageBuffer,
                    region_command_buffers[i].buffer,
                    aabb_command_size,
                ),
                (
                    9,
                    vk::DescriptorType::StorageBuffer,
                    region_visibility_buffers[i].buffer,
                    max_regions as u64 * 4,
                ),
                (
                    10,
                    vk::DescriptorType::StorageBuffer,
                    section_candidate_buffers[i].buffer,
                    max_meta as u64 * 4,
                ),
                (
                    11,
                    vk::DescriptorType::StorageBuffer,
                    section_command_buffers[i].buffer,
                    aabb_command_size,
                ),
                (
                    12,
                    vk::DescriptorType::StorageBuffer,
                    section_visibility_buffers[i].buffer,
                    max_meta as u64 * 4,
                ),
                (
                    13,
                    vk::DescriptorType::StorageBuffer,
                    history_buffer.buffer,
                    max_meta as u64 * 4,
                ),
                (
                    18,
                    vk::DescriptorType::StorageBuffer,
                    water_bucket_buffers[i].buffer,
                    (WATER_BUCKETS as u64 + 1) * 4,
                ),
                (
                    19,
                    vk::DescriptorType::StorageBuffer,
                    water_candidate_buffers[i].buffer,
                    max_meta as u64 * 4,
                ),
                (
                    20,
                    vk::DescriptorType::StorageBuffer,
                    water_indirect_buffers[i].buffer,
                    indirect_size,
                ),
                (
                    21,
                    vk::DescriptorType::StorageBuffer,
                    water_count_buffers[i].buffer,
                    4,
                ),
                (
                    22,
                    vk::DescriptorType::StorageBuffer,
                    mesh_buffer.buffer,
                    mesh_size,
                ),
                (
                    23,
                    vk::DescriptorType::StorageBuffer,
                    stats_buffers[i].buffer,
                    32,
                ),
                (
                    24,
                    vk::DescriptorType::StorageBuffer,
                    task_command_buffers[i].buffer,
                    task_command_size,
                ),
                (
                    25,
                    vk::DescriptorType::StorageBuffer,
                    task_command_cutout_buffers[i].buffer,
                    task_command_size,
                ),
            ];
            let infos: Vec<_> = bindings
                .iter()
                .map(|&(_, _, buffer, range)| vk::DescriptorBufferInfo {
                    buffer,
                    offset: 0,
                    range,
                })
                .collect();
            let writes: Vec<_> = bindings
                .iter()
                .zip(&infos)
                .map(|(&(binding, ty, _, _), info)| vk::WriteDescriptorSet {
                    dst_set: compute_sets[i],
                    dst_binding: binding,
                    descriptor_type: ty,
                    descriptor_count: 1,
                    buffer_info: info,
                    ..Default::default()
                })
                .collect();
            device.update_descriptor_sets(&writes, &[]);
        }

        Self {
            last_pool_warn: None,
            mesh_buffer,
            global_cuboid_buffer,
            uploads: ChunkUploads::new(staging_buffers, staging_size, use_staging),
            geometry: ChunkGeometry::new((mesh_size / POOL_UNIT) as u32),
            metadata: ChunkMetadata::new(max_meta),
            regions: crate::renderer::chunk::region::RegionStore::new(max_regions),
            next_visibility_generation: 0,
            culling: ChunkCulling {
                backend,
                max_meta,
                draw_capacity,
                requested_draw_capacity: 0,
                max_draw_indirect_count,
                warned_draw_cap: false,
                compute_pipeline,
                region_prepare_pipeline,
                section_expand_pipeline,
                finalize_pipeline,
                task_dispatch_pipeline,
                water_scan_pipeline,
                water_emit_pipeline,
                compute_layout,
                compute_desc_layout,
                compute_pool,
                compute_sets,
                meta_buffers,
                indirect_buffers,
                count_buffers,
                indirect_cutout_buffers,
                count_cutout_buffers,
                task_command_buffers,
                task_command_cutout_buffers,
                water_indirect_buffers,
                water_count_buffers,
                water_bucket_buffers,
                water_candidate_buffers,
                frustum_buffers,
                region_meta_buffers,
                region_candidate_buffers,
                region_command_buffers,
                region_visibility_buffers,
                section_candidate_buffers,
                section_command_buffers,
                section_visibility_buffers,
                history_buffer,
                stats_buffers,
                history_reset_pending: true,
                fade_enabled: false,
                last_draw_count: 0,
            },
        }
    }
}
