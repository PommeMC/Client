// ChunkRendererState culling responsibilities.

use super::*;

impl ChunkRendererCore {
    pub fn sections_drawn(&self) -> u32 {
        self.culling.last_draw_count
    }

    pub fn meta_rebuild_ms(&self) -> f32 {
        0.0
    }

    pub(super) fn grow_meta(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        let new_max = self.culling.max_meta * 2;
        // cull.comp packs water candidates as (meta slot << 9) | bucket.
        debug_assert!(
            new_max <= 1 << 23,
            "meta slots exceed the water candidate packing"
        );

        device.wait_idle().ok();

        {
            for i in 0..MAX_FRAMES_IN_FLIGHT {
                std::mem::take(&mut self.culling.meta_buffers[i]).destroy(device, allocator);
            }
        }

        let meta_size = (new_max * size_of::<ChunkMeta>()) as u64;
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            let buffer = Buffer::host(
                device,
                allocator,
                meta_size,
                vk::BufferUsageFlags::StorageBuffer | vk::BufferUsageFlags::VertexBuffer,
                "chunk_meta",
            );
            self.culling.meta_buffers[i] = buffer;

            let (meta_info, mut meta_write) = desc_write(
                self.culling.compute_sets[i],
                0,
                vk::DescriptorType::StorageBuffer,
                self.culling.meta_buffers[i].buffer,
                meta_size,
            );
            meta_write.buffer_info = meta_info.as_ptr();
            device.update_descriptor_sets(&[meta_write], &[]);
        }

        // Candidate storage is one entry per metadata slot. Indirect command
        // buffers have their own independently growing draw capacity.
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            std::mem::take(&mut self.culling.water_candidate_buffers[i]).destroy(device, allocator);
        }
        self.culling.water_candidate_buffers = (0..MAX_FRAMES_IN_FLIGHT)
            .map(|_| {
                Buffer::device(
                    device,
                    allocator,
                    new_max as u64 * 4,
                    vk::BufferUsageFlags::StorageBuffer,
                    "water_candidates",
                )
            })
            .collect();
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            let (candidates_info, mut candidates_write) = desc_write(
                self.culling.compute_sets[i],
                8,
                vk::DescriptorType::StorageBuffer,
                self.culling.water_candidate_buffers[i].buffer,
                new_max as u64 * 4,
            );
            candidates_write.buffer_info = candidates_info.as_ptr();
            device.update_descriptor_sets(&[candidates_write], &[]);
        }

        self.metadata.slots.grow(new_max as u32);
        self.metadata
            .mirror
            .resize(new_max, bytemuck::Zeroable::zeroed());
        // Fresh buffers: repopulate every slot from the mirror and drop the
        // catch-up queue it superseded.
        let mirror_bytes: &[u8] = bytemuck::cast_slice(&self.metadata.mirror);
        for buffer in &mut self.culling.meta_buffers {
            buffer.mapped_slice_mut()[..mirror_bytes.len()].copy_from_slice(mirror_bytes);
        }
        self.metadata.writes.clear();
        self.metadata.applied = [0; MAX_FRAMES_IN_FLIGHT];

        self.culling.max_meta = new_max;
    }

    /// Grow all indirect command buffers together after the previous frame's
    /// counters reported overflow. Capacity only grows, and always by powers
    /// of two relative to its current size.
    pub(super) fn ensure_draw_capacity(
        &mut self,
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
    ) {
        let required = std::mem::take(&mut self.culling.requested_draw_capacity);
        if required <= self.culling.draw_capacity {
            return;
        }

        let device_limit = self.culling.max_draw_indirect_count as usize;
        let mut new_capacity = self.culling.draw_capacity.max(1);
        while new_capacity < required && new_capacity < device_limit {
            new_capacity = new_capacity.saturating_mul(2).min(device_limit);
        }
        if new_capacity <= self.culling.draw_capacity {
            if !self.culling.warned_draw_cap {
                self.culling.warned_draw_cap = true;
                tracing::warn!(
                    "chunk draws require {required} commands, but the device limit is {device_limit}; excess draws are dropped"
                );
            }
            return;
        }

        device.wait_idle().ok();
        let indirect_size = (new_capacity * size_of::<DrawCommand>()) as u64;
        for frame in 0..MAX_FRAMES_IN_FLIGHT {
            std::mem::take(&mut self.culling.indirect_buffers[frame]).destroy(device, allocator);
            std::mem::take(&mut self.culling.indirect_cutout_buffers[frame])
                .destroy(device, allocator);
            std::mem::take(&mut self.culling.water_indirect_buffers[frame])
                .destroy(device, allocator);

            self.culling.indirect_buffers[frame] = Buffer::host(
                device,
                allocator,
                indirect_size,
                vk::BufferUsageFlags::StorageBuffer | vk::BufferUsageFlags::IndirectBuffer,
                "indirect_cmds",
            );
            self.culling.indirect_cutout_buffers[frame] = Buffer::host(
                device,
                allocator,
                indirect_size,
                vk::BufferUsageFlags::StorageBuffer | vk::BufferUsageFlags::IndirectBuffer,
                "indirect_cmds_cutout",
            );
            self.culling.water_indirect_buffers[frame] = Buffer::host(
                device,
                allocator,
                indirect_size,
                vk::BufferUsageFlags::StorageBuffer | vk::BufferUsageFlags::IndirectBuffer,
                "water_indirect",
            );

            let (solid_info, mut solid_write) = desc_write(
                self.culling.compute_sets[frame],
                2,
                vk::DescriptorType::StorageBuffer,
                self.culling.indirect_buffers[frame].buffer,
                indirect_size,
            );
            let (cutout_info, mut cutout_write) = desc_write(
                self.culling.compute_sets[frame],
                4,
                vk::DescriptorType::StorageBuffer,
                self.culling.indirect_cutout_buffers[frame].buffer,
                indirect_size,
            );
            let (water_info, mut water_write) = desc_write(
                self.culling.compute_sets[frame],
                9,
                vk::DescriptorType::StorageBuffer,
                self.culling.water_indirect_buffers[frame].buffer,
                indirect_size,
            );
            solid_write.buffer_info = solid_info.as_ptr();
            cutout_write.buffer_info = cutout_info.as_ptr();
            water_write.buffer_info = water_info.as_ptr();
            device.update_descriptor_sets(&[solid_write, cutout_write, water_write], &[]);
        }

        tracing::info!(
            "Chunk indirect capacity grew from {} to {} commands",
            self.culling.draw_capacity,
            new_capacity,
        );
        self.culling.draw_capacity = new_capacity;
        self.culling.warned_draw_cap = false;
    }

    pub(super) fn alloc_meta_slot(
        &mut self,
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
    ) -> u32 {
        let slot = self
            .metadata
            .slots
            .alloc(1)
            .or_else(|| {
                self.reclaim_retired(device)
                    .then(|| self.metadata.slots.alloc(1))
                    .flatten()
            })
            .unwrap_or_else(|| {
                self.grow_meta(device, allocator);
                self.metadata
                    .slots
                    .alloc(1)
                    .expect("meta pool empty after grow")
            });
        self.metadata.high_water = self.metadata.high_water.max(slot + 1);
        slot
    }

    pub(super) fn queue_meta_write(&mut self, slot: u32, entry: ChunkMeta) {
        self.metadata.queue_write(slot, entry);
    }

    pub(super) fn apply_meta_writes(&mut self, frame: usize) {
        self.metadata.apply_writes(
            frame,
            &mut self.culling.meta_buffers[frame],
            self.culling.max_meta,
        );
    }

    pub fn set_hiz_descriptors(
        &mut self,
        device: &vk::Device,
        view: vk::ImageView,
        sampler: vk::Sampler,
    ) {
        let info = vk::DescriptorImageInfo {
            sampler,
            image_view: view,
            image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
        };
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            let write = vk::WriteDescriptorSet {
                dst_set: self.culling.compute_sets[i],
                dst_binding: 6,
                descriptor_type: vk::DescriptorType::CombinedImageSampler,
                descriptor_count: 1,
                image_info: &info,
                ..Default::default()
            };
            device.update_descriptor_sets(&[write], &[]);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn dispatch_cull(
        &mut self,
        cmd: vk::CommandBuffer,
        frame: usize,
        frustum: &[[f32; 4]; 6],
        anchor: DVec3,
        eye: DVec3,
        player_chunk: ChunkPos,
        limit_rd: Option<u32>,
        occlusion: Option<OcclusionCamera>,
    ) {
        if self.metadata.high_water == 0 {
            return;
        }
        // Catch this frame slot's persistent meta buffer up with the entries
        // written since it last ran (its fence was waited at frame start, so
        // no in-flight cull reads it). This is the only per-frame CPU cost and
        // it scales with *changed* sections, never with loaded ones.
        self.apply_meta_writes(frame);
        let count = self.metadata.high_water;

        let occ = occlusion.unwrap_or_default();
        let frustum_data = FrustumData {
            planes: *frustum,
            prev_view_proj: occ.view_proj,
            chunk_count: count,
            cam_block: anchor.as_ivec3().to_array(),
            frac: (eye - anchor).as_vec3().to_array(),
            prev_cam_block: occ.cam_block.to_array(),
            prev_frac: occ.frac.to_array(),
            occlusion_valid: occlusion.is_some() as u32,
            player_chunk: [player_chunk.x, player_chunk.z],
            limit_rd: limit_rd.unwrap_or(0),
            _pad: [self.culling.clamped_draw_count(), 0, 0],
        };
        let frustum_bytes = bytemuck::bytes_of(&frustum_data);
        self.culling.frustum_buffers[frame]
            .allocation
            .mapped_slice_mut()
            .unwrap()[..frustum_bytes.len()]
            .copy_from_slice(frustum_bytes);

        // This frame slot's GPU work has completed (fence-waited at frame start),
        // so the count buffers still hold their previous cull result; capture the
        // total (solid + cutout draws) for the debug overlay before clearing them.
        {
            let read_and_clear = |buffer: &mut Buffer| {
                let s = buffer.mapped_slice_mut();
                let n = u32::from_ne_bytes([s[0], s[1], s[2], s[3]]);
                s[..4].copy_from_slice(&0u32.to_ne_bytes());
                n
            };
            let solid = read_and_clear(&mut self.culling.count_buffers[frame]);
            let cutout = read_and_clear(&mut self.culling.count_cutout_buffers[frame]);
            let water_bytes = self.culling.water_count_buffers[frame]
                .allocation
                .mapped_slice()
                .unwrap();
            let water = u32::from_ne_bytes([
                water_bytes[0],
                water_bytes[1],
                water_bytes[2],
                water_bytes[3],
            ]);
            self.culling.last_draw_count = solid + cutout;
            let capacity = self.culling.clamped_draw_count();
            let observed = solid.max(cutout).max(water);
            if observed > capacity {
                self.culling.requested_draw_capacity =
                    self.culling.requested_draw_capacity.max(observed as usize);
            }
        }

        // macOS draws the whole indirect buffer (no drawIndirectCount), so slots
        // the cull shader leaves unfilled must read as no-op draws, not stale data.
        #[cfg(target_os = "macos")]
        {
            // Without drawIndirectCount the driver executes the complete
            // command capacity, so every command the cull does not overwrite
            // this frame must be a zeroed no-op.
            let live = self.culling.clamped_draw_count() as usize * size_of::<DrawCommand>();
            for a in [
                &mut self.culling.indirect_buffers[frame].allocation,
                &mut self.culling.indirect_cutout_buffers[frame].allocation,
                &mut self.culling.water_indirect_buffers[frame].allocation,
            ] {
                a.mapped_slice_mut().unwrap()[..live].fill(0);
            }
        }

        // Clear the water bucket counters (+ candidate counter) before the
        // cull accumulates into them; the slot's previous use is fence-waited.
        cmd.fill_buffer(
            self.culling.water_bucket_buffers[frame].buffer,
            0,
            (WATER_BUCKETS as u64 + 1) * 4,
            0,
        );
        let fill_barrier = vk::MemoryBarrier {
            src_access_mask: vk::AccessFlags::TransferWrite,
            dst_access_mask: vk::AccessFlags::ShaderRead | vk::AccessFlags::ShaderWrite,
            ..Default::default()
        };
        cmd.pipeline_barrier(
            vk::PipelineStageFlags::Transfer,
            vk::PipelineStageFlags::ComputeShader,
            vk::DependencyFlags::empty(),
            &[fill_barrier],
            &[],
            &[],
        );

        cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Compute,
            self.culling.compute_layout,
            0,
            &[self.culling.compute_sets[frame]],
            &[],
        );
        cmd.bind_pipeline(
            vk::PipelineBindPoint::Compute,
            self.culling.compute_pipeline,
        );
        cmd.dispatch(count.div_ceil(64), 1, 1);

        // Cull → scan → emit: each pass reads what the previous wrote.
        let compute_barrier = vk::MemoryBarrier {
            src_access_mask: vk::AccessFlags::ShaderWrite,
            dst_access_mask: vk::AccessFlags::ShaderRead | vk::AccessFlags::ShaderWrite,
            ..Default::default()
        };
        let compute_to_compute = |cmd: vk::CommandBuffer| {
            cmd.pipeline_barrier(
                vk::PipelineStageFlags::ComputeShader,
                vk::PipelineStageFlags::ComputeShader,
                vk::DependencyFlags::empty(),
                &[compute_barrier],
                &[],
                &[],
            );
        };
        compute_to_compute(cmd);
        cmd.bind_pipeline(
            vk::PipelineBindPoint::Compute,
            self.culling.water_scan_pipeline,
        );
        cmd.dispatch(1, 1, 1);
        compute_to_compute(cmd);
        cmd.bind_pipeline(
            vk::PipelineBindPoint::Compute,
            self.culling.water_emit_pipeline,
        );
        cmd.dispatch(count.div_ceil(64), 1, 1);

        let barrier = vk::MemoryBarrier {
            src_access_mask: vk::AccessFlags::ShaderWrite,
            dst_access_mask: vk::AccessFlags::IndirectCommandRead,
            ..Default::default()
        };
        cmd.pipeline_barrier(
            vk::PipelineStageFlags::ComputeShader,
            vk::PipelineStageFlags::DrawIndirect,
            vk::DependencyFlags::empty(),
            &[barrier],
            &[],
            &[],
        );

        if !self.culling.fade_enabled {
            self.culling.fade_enabled = true;
        }
    }

    pub(super) fn clamped_draw_count(&mut self) -> u32 {
        self.culling.clamped_draw_count()
    }

    pub fn draw_indirect(&mut self, cmd: vk::CommandBuffer, frame: usize, cutout: bool) {
        if self.metadata.high_water == 0 {
            return;
        }

        let max_draws = self.clamped_draw_count();
        let (indirect, count) = if cutout {
            (
                self.culling.indirect_cutout_buffers[frame].buffer,
                self.culling.count_cutout_buffers[frame].buffer,
            )
        } else {
            (
                self.culling.indirect_buffers[frame].buffer,
                self.culling.count_buffers[frame].buffer,
            )
        };

        // Binding 0: the per-instance meta buffer, read for section origin and
        // fade (indexed by `first_instance`). Face and cuboid data are pulled
        // from graphics storage descriptors by the vertex shader.
        if cfg!(target_os = "macos") {
            cmd.draw_indirect(indirect, 0, max_draws, size_of::<DrawCommand>() as u32);
        } else {
            cmd.draw_indirect_count(
                indirect,
                0,
                count,
                0,
                max_draws,
                size_of::<DrawCommand>() as u32,
            );
        }
    }

    pub fn draw_water(&mut self, cmd: vk::CommandBuffer, frame: usize) {
        if self.metadata.high_water == 0 {
            return;
        }

        let max_draws = self.clamped_draw_count();
        if cfg!(target_os = "macos") {
            cmd.draw_indirect(
                self.culling.water_indirect_buffers[frame].buffer,
                0,
                max_draws,
                size_of::<DrawCommand>() as u32,
            );
        } else {
            cmd.draw_indirect_count(
                self.culling.water_indirect_buffers[frame].buffer,
                0,
                self.culling.water_count_buffers[frame].buffer,
                0,
                max_draws,
                size_of::<DrawCommand>() as u32,
            );
        }
    }

    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        std::mem::take(&mut self.mesh_buffer).destroy(device, allocator);
        std::mem::take(&mut self.global_cuboid_buffer).destroy(device, allocator);
        for buffer in self
            .culling
            .meta_buffers
            .drain(..)
            .chain(self.culling.indirect_buffers.drain(..))
            .chain(self.culling.count_buffers.drain(..))
            .chain(self.culling.indirect_cutout_buffers.drain(..))
            .chain(self.culling.count_cutout_buffers.drain(..))
            .chain(self.culling.water_indirect_buffers.drain(..))
            .chain(self.culling.water_count_buffers.drain(..))
            .chain(self.culling.water_bucket_buffers.drain(..))
            .chain(self.culling.water_candidate_buffers.drain(..))
            .chain(self.culling.frustum_buffers.drain(..))
            .chain(self.uploads.staging_buffers.drain(..))
        {
            buffer.destroy(device, allocator);
        }

        device.destroy_pipeline(self.culling.compute_pipeline, None);
        device.destroy_pipeline(self.culling.water_scan_pipeline, None);
        device.destroy_pipeline(self.culling.water_emit_pipeline, None);
        device.destroy_pipeline_layout(self.culling.compute_layout, None);
        device.destroy_descriptor_pool(self.culling.compute_pool, None);
        device.destroy_descriptor_set_layout(self.culling.compute_desc_layout, None);
    }
}
