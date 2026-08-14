// ChunkRendererState upload responsibilities.

use super::*;

fn build_mesh_payload(mesh: &SectionMeshData, pool_off: u32, uploaded_ms: u32) -> Vec<u8> {
    let counts = mesh.batch_counts;
    debug_assert_eq!(
        counts.regular_solid + counts.opaque_fluid + counts.cutout + counts.translucent_fluid,
        mesh.batches.len() as u32,
    );
    let layout = mesh.layout();
    let base = pool_off as usize * POOL_UNIT as usize;
    let mut bytes = vec![0u8; layout.size];
    let mut write = |offset: usize, src: &[u8]| {
        bytes[offset..offset + src.len()].copy_from_slice(src);
    };
    write(layout.regular_faces, bytemuck::cast_slice(&mesh.faces));
    write(
        layout.regular_cuboids,
        bytemuck::cast_slice(&mesh.section_cuboids),
    );
    write(layout.fluid_faces, bytemuck::cast_slice(&mesh.fluid_faces));
    write(
        layout.fluid_cuboids,
        bytemuck::cast_slice(&mesh.fluid_cuboids),
    );
    write(
        layout.fluid_heights,
        bytemuck::cast_slice(&mesh.fluid_heights),
    );

    let origin = [mesh.spos.x * 16, mesh.spos.y * 16, mesh.spos.z * 16];
    let gpu_batches: Vec<GpuFaceBatch> = mesh
        .batches
        .iter()
        .enumerate()
        .map(|(index, batch)| {
            let index = index as u32;
            let opaque_fluid_start = counts.regular_solid;
            let cutout_start = opaque_fluid_start + counts.opaque_fluid;
            let translucent_fluid_start = cutout_start + counts.cutout;
            let fluid = (opaque_fluid_start..cutout_start).contains(&index)
                || index >= translucent_fluid_start;
            let face_base = if fluid {
                layout.fluid_faces
            } else {
                layout.regular_faces
            };
            let cuboid_base = if fluid {
                layout.fluid_cuboids
            } else {
                layout.regular_cuboids
            };
            GpuFaceBatch {
                face_word_offset: ((base + face_base) / 4) as u32 + batch.face_offset,
                face_count: batch.face_count,
                cuboid_word_offset: ((base + cuboid_base) / 4) as u32 + batch.cuboid_base * 2,
                fluid_height_word_offset: if fluid {
                    ((base + layout.fluid_heights) / 4) as u32 + batch.cuboid_base
                } else {
                    u32::MAX
                },
                origin,
                uploaded_ms,
            }
        })
        .collect();
    write(layout.batches, bytemuck::cast_slice(&gpu_batches));
    bytes
}

impl ChunkRendererCore {
    pub fn record_copies(&mut self, cmd: vk::CommandBuffer, frame: usize) {
        if self.uploads.pending_copies.is_empty() {
            return;
        }
        let mut copies: Vec<vk::BufferCopy> = Vec::with_capacity(self.uploads.pending_copies.len());
        let mut staging_offset = 0usize;
        {
            let buf = self.uploads.staging_buffers[frame].mapped_slice_mut();
            for pending in &self.uploads.pending_copies {
                let end = staging_offset + pending.bytes.len();
                buf[staging_offset..end].copy_from_slice(&pending.bytes);
                copies.push(vk::BufferCopy {
                    src_offset: staging_offset as u64,
                    dst_offset: pending.pool_off as u64 * POOL_UNIT,
                    size: pending.bytes.len() as u64,
                });
                staging_offset = end;
            }
        }
        cmd.copy_buffer(
            self.uploads.staging_buffers[frame].buffer,
            self.mesh_buffer.buffer,
            &copies,
        );
        let barrier = vk::MemoryBarrier {
            src_access_mask: vk::AccessFlags::TransferWrite,
            dst_access_mask: vk::AccessFlags::ShaderRead,
            ..Default::default()
        };
        cmd.pipeline_barrier(
            vk::PipelineStageFlags::Transfer,
            vk::PipelineStageFlags::ComputeShader | vk::PipelineStageFlags::VertexShader,
            vk::DependencyFlags::empty(),
            &[barrier],
            &[],
            &[],
        );
        self.drop_pending_copies();
    }

    pub(super) fn drop_pending_copies(&mut self) {
        self.uploads.clear_pending();
    }

    pub(super) fn drop_pending_copies_for(&mut self, freed: &[(u32, u32, u32)]) {
        self.uploads.clear_pending_for(freed);
    }

    pub fn stage_mesh_batch(
        &mut self,
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        mesh_queue: &mut VecDeque<SectionMeshData>,
        eye: DVec3,
    ) {
        self.ensure_draw_capacity(device, allocator);
        self.geometry.last_reclaim_ms = 0.0;
        // Keep only the newest result per section before draining: the stale
        // check below reads `self.geometry.chunks`, which only reflects this batch's
        // uploads after the loop, so two same-section results in one drain would
        // otherwise both be accepted and the section drawn twice. Recency is
        // the `(content_gen, epoch)` pair — the slot version the mesh saw,
        // then the enqueue stamp — because a lower-epoch job can snapshot
        // *after* a higher-epoch one and carry the newer world state.
        // (Keyed by packed pos: azalea's ChunkSectionPos doesn't impl Hash.)
        let order = |m: &SectionMeshData| (m.content_gen, m.upload_epoch);
        let mut best: HashMap<u64, (u64, u64)> = HashMap::new();
        for mesh in mesh_queue.iter() {
            let key = pack_section_pos(mesh.spos);
            let cur = best.entry(key).or_insert_with(|| order(mesh));
            *cur = (*cur).max(order(mesh));
        }
        if best.len() < mesh_queue.len() {
            let mut seen = HashSet::new();
            mesh_queue.retain(|m| {
                let key = pack_section_pos(m.spos);
                order(m) == best[&key] && seen.insert(key)
            });
        }
        if mesh_queue.is_empty() {
            return;
        }

        let staging_budget = self.uploads.staging_size as usize;

        struct BatchEntry {
            mesh: SectionMeshData,
            col_pos: ChunkPos,
            si: i32,
            was_present: bool,
            pool_off: u32,
            pool_len: u32,
            meta_slot: u32,
            uploaded_ms: u32,
        }
        let mut entries: Vec<BatchEntry> = Vec::new();

        // Include copies carried over from a skipped frame in the budget.
        let mut current_mesh_bytes = self.uploads.pending_mesh_bytes;
        while let Some(mesh) = mesh_queue.front() {
            let col_pos = ChunkPos::new(mesh.spos.x, mesh.spos.z);
            let si = mesh.relative_si;
            let stored = self
                .geometry
                .chunks
                .get(&col_pos)
                .and_then(|c| c.sections.iter().find(|s| s.section_index == si))
                .map(|s| s.order_key())
                .unwrap_or((0, 0));
            // Reject a stale upload a newer result already superseded.
            if order(mesh) < stored {
                mesh_queue.pop_front();
                continue;
            }
            if mesh.is_empty() {
                self.take_section(col_pos, si);
                // Keep a tombstone: the `(content_gen, epoch)` floor must
                // outlive the geometry, or a slower pre-edit mesh would pass
                // the gate above and resurrect the section the empty result
                // just cleared. Pruned with the column on unload.
                let (content_gen, epoch) = order(mesh);
                self.geometry
                    .chunks
                    .entry(col_pos)
                    .or_insert_with(|| ChunkAlloc {
                        sections: Vec::new(),
                    })
                    .sections
                    .push(SectionAlloc {
                        section_index: si,
                        meta_slot: TOMBSTONE_SLOT,
                        pool_offset: 0,
                        pool_len: 0,
                        content_gen,
                        epoch,
                    });
                mesh_queue.pop_front();
                continue;
            }
            let layout = mesh.layout();
            let mesh_bytes = layout.size;
            let pool_len = mesh_bytes.div_ceil(POOL_UNIT as usize) as u32;
            if self.uploads.use_staging {
                // A section too large for the staging slab is skipped, not overflowed.
                if mesh_bytes > staging_budget {
                    tracing::warn!(
                        "Section {:?} too large for staging ({} bytes), skipping",
                        mesh.spos,
                        mesh_bytes,
                    );
                    mesh_queue.pop_front();
                    continue;
                }
                // This transfer's staging budget is full; leave the rest queued.
                if current_mesh_bytes + mesh_bytes > staging_budget {
                    break;
                }
                current_mesh_bytes += mesh_bytes;
            }
            let Some(pool_off) = self.alloc_mesh(device, pool_len) else {
                // Rate-limited: exhaustion persists across frames.
                let now = std::time::Instant::now();
                if self
                    .last_pool_warn
                    .is_none_or(|t| now.duration_since(t).as_secs() >= 5)
                {
                    self.last_pool_warn = Some(now);
                    tracing::warn!(
                        "vertex pool exhausted (largest free run {} verts, wanted {});                          uploads stalled for {:?}",
                        self.geometry.mesh_free.largest_free(),
                        pool_len,
                        mesh.spos,
                    );
                }
                break;
            };
            let meta_slot = self.alloc_meta_slot(device, allocator);
            let mesh = mesh_queue.pop_front().unwrap();
            let was_present = self.take_section(col_pos, si);
            entries.push(BatchEntry {
                mesh,
                col_pos,
                si,
                was_present,
                pool_off,
                pool_len,
                meta_slot,
                uploaded_ms: 0,
            });
        }

        if entries.is_empty() {
            return;
        }

        let now_ms = crate::renderer::camera::session_millis();
        for entry in &mut entries {
            let spos = entry.mesh.spos;
            // A re-meshed section swaps instantly and near columns never fade
            // (vanilla `isNearby`); everything else fades in from its upload
            // stamp, computed shader-side against the session clock.
            let backdate = !self.culling.fade_enabled
                || entry.was_present
                || section_is_near(entry.mesh.spos, eye);
            let uploaded_ms = if backdate {
                now_ms.wrapping_sub(2 * FADE_DURATION_MS as u32)
            } else {
                now_ms
            };
            entry.uploaded_ms = uploaded_ms;
            let layout = entry.mesh.layout();
            let base_bytes = entry.pool_off as usize * POOL_UNIT as usize;
            self.queue_meta_write(
                entry.meta_slot,
                ChunkMeta {
                    aabb_min: entry.mesh.aabb.min,
                    aabb_max: entry.mesh.aabb.max,
                    batch_word_offset: ((base_bytes + layout.batches) / 4) as u32,
                    solid_batch_count: entry.mesh.batch_counts.solid_draws(),
                    cutout_batch_count: entry.mesh.batch_counts.cutout,
                    fluid_batch_count: entry.mesh.batch_counts.translucent_fluid,
                    origin: [spos.x * 16, spos.y * 16, spos.z * 16],
                    _pad: 0,
                },
            );
            self.geometry
                .chunks
                .entry(entry.col_pos)
                .or_insert_with(|| ChunkAlloc {
                    sections: Vec::new(),
                })
                .sections
                .push(SectionAlloc {
                    section_index: entry.si,
                    meta_slot: entry.meta_slot,
                    pool_offset: entry.pool_off,
                    pool_len: entry.pool_len,
                    content_gen: entry.mesh.content_gen,
                    epoch: entry.mesh.upload_epoch,
                });
        }

        if self.uploads.use_staging {
            for entry in &entries {
                let bytes = build_mesh_payload(&entry.mesh, entry.pool_off, entry.uploaded_ms);
                self.uploads.pending_mesh_bytes += bytes.len();
                self.uploads.pending_copies.push(PendingCopy {
                    bytes,
                    pool_off: entry.pool_off,
                });
            }
        } else {
            let vbuf = self.mesh_buffer.allocation.mapped_slice_mut().unwrap();
            for entry in &entries {
                let bytes = build_mesh_payload(&entry.mesh, entry.pool_off, entry.uploaded_ms);
                let base = entry.pool_off as usize * POOL_UNIT as usize;
                vbuf[base..base + bytes.len()].copy_from_slice(&bytes);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::chunk::mesher::{ChunkAABB, FaceBatch, FaceBatchCounts, FaceRecord};

    #[test]
    fn payload_rebases_regular_and_fluid_batches_into_one_allocation() {
        let mesh = SectionMeshData {
            spos: ChunkSectionPos::new(2, 3, 4),
            relative_si: 3,
            faces: vec![FaceRecord::new(0, [1; 4]); 2],
            section_cuboids: vec![SectionCuboid { packed: 0 }; 3],
            fluid_faces: vec![FaceRecord::new(6, [2; 4]); 2],
            fluid_cuboids: vec![SectionCuboid { packed: 0 }; 3],
            fluid_heights: vec![0x4321; 3],
            batches: vec![
                FaceBatch {
                    face_offset: 1,
                    face_count: 1,
                    cuboid_base: 0,
                },
                FaceBatch {
                    face_offset: 0,
                    face_count: 2,
                    cuboid_base: 1,
                },
            ],
            batch_counts: FaceBatchCounts {
                regular_solid: 1,
                opaque_fluid: 0,
                cutout: 0,
                translucent_fluid: 1,
            },
            aabb: ChunkAABB {
                min: [0.0; 4],
                max: [1.0; 4],
            },
            content_gen: 0,
            upload_epoch: 0,
            queue_ms: 0.0,
            mesh_ms: 0.0,
        };
        let pool_off = 2;
        let uploaded_ms = 77;
        let layout = mesh.layout();
        let payload = build_mesh_payload(&mesh, pool_off, uploaded_ms);
        assert_eq!(payload.len(), layout.size);
        let read_batch = |index: usize| {
            let start = layout.batches + index * size_of::<GpuFaceBatch>();
            bytemuck::pod_read_unaligned::<GpuFaceBatch>(
                &payload[start..start + size_of::<GpuFaceBatch>()],
            )
        };
        let base = pool_off as usize * POOL_UNIT as usize;
        let regular = read_batch(0);
        assert_eq!(
            regular.face_word_offset,
            ((base + layout.regular_faces) / 4) as u32 + 1
        );
        assert_eq!(
            regular.cuboid_word_offset,
            ((base + layout.regular_cuboids) / 4) as u32
        );
        assert_eq!(regular.fluid_height_word_offset, u32::MAX);
        assert_eq!(regular.origin, [32, 48, 64]);
        assert_eq!(regular.uploaded_ms, uploaded_ms);

        let fluid = read_batch(1);
        assert_eq!(
            fluid.face_word_offset,
            ((base + layout.fluid_faces) / 4) as u32
        );
        assert_eq!(
            fluid.cuboid_word_offset,
            ((base + layout.fluid_cuboids) / 4) as u32 + 2
        );
        assert_eq!(
            fluid.fluid_height_word_offset,
            ((base + layout.fluid_heights) / 4) as u32 + 1
        );
    }
}
