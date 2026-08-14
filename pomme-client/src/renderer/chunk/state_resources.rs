// ChunkRendererState resources responsibilities.

use super::*;

impl ChunkRendererCore {
    pub fn new(
        device: &vk::Device,
        physical_device: vk::PhysicalDevice,
        allocator: &Arc<Mutex<Allocator>>,
    ) -> Self {
        let total_buckets = compute_bucket_count(physical_device);
        let vertex_size = total_buckets as u64 * BUCKET_VERTICES as u64 * VERTEX_SIZE;

        let dev_props = physical_device.get_properties();
        let use_staging = dev_props.device_type == vk::PhysicalDeviceType::DiscreteGpu;
        // Spec floor is 65535 even with multiDrawIndirect; meta_high_water
        // passes it from ~RD 32, so every draw clamps to the device cap.
        let max_draw_indirect_count = dev_props.limits.max_draw_indirect_count;

        let vertex_buffer = if use_staging {
            Buffer::device(
                device,
                allocator,
                vertex_size,
                vk::BufferUsageFlags::VertexBuffer,
                "vertex_pool",
            )
        } else {
            Buffer::host(
                device,
                allocator,
                vertex_size,
                vk::BufferUsageFlags::VertexBuffer,
                "vertex_pool",
            )
        };

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
            vertex_size / (1024 * 1024),
            staging_size / 1024,
        );

        // Per-section packing yields many more draws than buckets, so pre-size
        // generously: growth (`ensure_meta_capacity`) needs a `device.wait_idle`
        // to safely rewrite the descriptor sets, and that stall showed up as a
        // 27ms frame when an RD-32 world (~45k section draws) crossed 16x. The
        // grow path stays as a rare safety net.
        let max_meta = (total_buckets * 32).max(8192) as usize;
        let meta_size = (max_meta * size_of::<ChunkMeta>()) as u64;
        let indirect_size = (max_meta * size_of::<DrawCommand>()) as u64;
        let count_size = 4u64;
        let frustum_size = size_of::<FrustumData>() as u64;

        let mut meta_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut indirect_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut count_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut indirect_cutout_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut count_cutout_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut frustum_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut water_count_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut water_bucket_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);

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
                vk::BufferUsageFlags::StorageBuffer | vk::BufferUsageFlags::IndirectBuffer,
                "indirect_cmds",
            );
            indirect_buffers.push(buffer);

            let buffer = Buffer::host(
                device,
                allocator,
                count_size,
                vk::BufferUsageFlags::StorageBuffer | vk::BufferUsageFlags::IndirectBuffer,
                "draw_count",
            );
            count_buffers.push(buffer);

            let buffer = Buffer::host(
                device,
                allocator,
                indirect_size,
                vk::BufferUsageFlags::StorageBuffer | vk::BufferUsageFlags::IndirectBuffer,
                "indirect_cmds_cutout",
            );
            indirect_cutout_buffers.push(buffer);

            let buffer = Buffer::host(
                device,
                allocator,
                count_size,
                vk::BufferUsageFlags::StorageBuffer | vk::BufferUsageFlags::IndirectBuffer,
                "draw_count_cutout",
            );
            count_cutout_buffers.push(buffer);

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
        }

        let (water_indirect_buffers, water_candidate_buffers) =
            create_water_scaled_buffers(device, allocator, max_meta, indirect_size);

        let compute_desc_layout = create_cull_desc_layout(device);
        let layout_info = vk::PipelineLayoutCreateInfo {
            set_layout_count: 1,
            set_layouts: &compute_desc_layout,
            ..Default::default()
        };
        let compute_layout = device
            .create_pipeline_layout(&layout_info, None)
            .expect("failed to create compute pipeline layout");

        let compute_pipeline = create_compute_pipeline(
            device,
            compute_layout,
            shader::include_spirv!("cull.comp.spv"),
            None,
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
                // meta + solid indirect/count + cutout indirect/count +
                // water indirect/count/buckets/candidates = 9 per frame.
                descriptor_count: 9 * MAX_FRAMES_IN_FLIGHT as u32,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UniformBuffer,
                descriptor_count: MAX_FRAMES_IN_FLIGHT as u32,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::CombinedImageSampler,
                // The Hi-Z pyramid at binding 6.
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
            let (meta_info, mut meta_write) = desc_write(
                compute_sets[i],
                0,
                vk::DescriptorType::StorageBuffer,
                meta_buffers[i].buffer,
                meta_size,
            );

            let (frustum_info, mut frustum_write) = desc_write(
                compute_sets[i],
                1,
                vk::DescriptorType::UniformBuffer,
                frustum_buffers[i].buffer,
                frustum_size,
            );

            let (indirect_info, mut indirect_write) = desc_write(
                compute_sets[i],
                2,
                vk::DescriptorType::StorageBuffer,
                indirect_buffers[i].buffer,
                indirect_size,
            );

            let (count_info, mut count_write) = desc_write(
                compute_sets[i],
                3,
                vk::DescriptorType::StorageBuffer,
                count_buffers[i].buffer,
                count_size,
            );

            let (indirect_c_info, mut indirect_c_write) = desc_write(
                compute_sets[i],
                4,
                vk::DescriptorType::StorageBuffer,
                indirect_cutout_buffers[i].buffer,
                indirect_size,
            );

            let (count_c_info, mut count_c_write) = desc_write(
                compute_sets[i],
                5,
                vk::DescriptorType::StorageBuffer,
                count_cutout_buffers[i].buffer,
                count_size,
            );

            let (buckets_info, mut buckets_write) = desc_write(
                compute_sets[i],
                7,
                vk::DescriptorType::StorageBuffer,
                water_bucket_buffers[i].buffer,
                (WATER_BUCKETS as u64 + 1) * 4,
            );

            let (candidates_info, mut candidates_write) = desc_write(
                compute_sets[i],
                8,
                vk::DescriptorType::StorageBuffer,
                water_candidate_buffers[i].buffer,
                max_meta as u64 * 4,
            );

            let (water_ind_info, mut water_ind_write) = desc_write(
                compute_sets[i],
                9,
                vk::DescriptorType::StorageBuffer,
                water_indirect_buffers[i].buffer,
                indirect_size,
            );

            let (water_count_info, mut water_count_write) = desc_write(
                compute_sets[i],
                10,
                vk::DescriptorType::StorageBuffer,
                water_count_buffers[i].buffer,
                count_size,
            );

            meta_write.buffer_info = meta_info.as_ptr();
            frustum_write.buffer_info = frustum_info.as_ptr();
            indirect_write.buffer_info = indirect_info.as_ptr();
            count_write.buffer_info = count_info.as_ptr();
            indirect_c_write.buffer_info = indirect_c_info.as_ptr();
            count_c_write.buffer_info = count_c_info.as_ptr();
            buckets_write.buffer_info = buckets_info.as_ptr();
            candidates_write.buffer_info = candidates_info.as_ptr();
            water_ind_write.buffer_info = water_ind_info.as_ptr();
            water_count_write.buffer_info = water_count_info.as_ptr();

            let writes = [
                meta_write,
                frustum_write,
                indirect_write,
                count_write,
                indirect_c_write,
                count_c_write,
                buckets_write,
                candidates_write,
                water_ind_write,
                water_count_write,
            ];

            device.update_descriptor_sets(&writes, &[]);
        }

        let mut this = Self {
            last_pool_warn: None,
            vertex_buffer,
            uploads: ChunkUploads::new(staging_buffers, staging_size, use_staging),
            geometry: ChunkGeometry::new(total_buckets * BUCKET_VERTICES),
            metadata: ChunkMetadata::new(max_meta),
            culling: ChunkCulling {
                max_meta,
                max_draw_indirect_count,
                warned_draw_cap: false,
                compute_pipeline,
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
                water_indirect_buffers,
                water_count_buffers,
                water_bucket_buffers,
                water_candidate_buffers,
                frustum_buffers,
                fade_enabled: false,
                last_draw_count: 0,
            },
        };
        this.ensure_quad_index_capacity(device, allocator, INITIAL_QUAD_INDEX_QUADS);
        this
    }
}
