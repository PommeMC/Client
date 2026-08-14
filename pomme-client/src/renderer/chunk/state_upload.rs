// ChunkRendererState upload responsibilities.

use super::*;

impl ChunkRendererCore {
    pub(super) fn ensure_quad_index_capacity(
        &mut self,
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        quads: u32,
    ) {
        if quads <= self.uploads.quad_index_quads {
            return;
        }
        let new_quads = quads.next_power_of_two().max(INITIAL_QUAD_INDEX_QUADS);
        let size = new_quads as u64 * 6 * size_of::<u32>() as u64;

        if self.uploads.quad_index_quads > 0 {
            // In-flight frames may still reference the old buffer (and the
            // staged path's copy source).
            device.wait_idle().ok();
            let old = std::mem::take(&mut self.uploads.quad_index_buffer);
            old.destroy(device, allocator);
            if let Some(src) = self.uploads.quad_index_src.take() {
                src.destroy(device, allocator);
            }
        }

        let mut pattern: Vec<u32> = Vec::with_capacity(new_quads as usize * 6);
        for q in 0..new_quads {
            let base = q * 4;
            pattern.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 3, base]);
        }
        let bytes: &[u8] = bytemuck::cast_slice(&pattern);

        let mut buffer = if self.uploads.use_staging {
            Buffer::device(
                device,
                allocator,
                size,
                vk::BufferUsageFlags::IndexBuffer,
                "quad_index",
            )
        } else {
            Buffer::host(
                device,
                allocator,
                size,
                vk::BufferUsageFlags::IndexBuffer,
                "quad_index",
            )
        };
        if self.uploads.use_staging {
            let mut src = Buffer::host(
                device,
                allocator,
                size,
                vk::BufferUsageFlags::TransferSrc,
                "quad_index_src",
            );
            src.mapped_slice_mut()[..bytes.len()].copy_from_slice(bytes);
            self.uploads.quad_index_src = Some(src);
            self.uploads.quad_index_copy_pending = true;
        } else {
            buffer.mapped_slice_mut()[..bytes.len()].copy_from_slice(bytes);
        }
        self.uploads.quad_index_buffer = buffer;
        self.uploads.quad_index_quads = new_quads;
        tracing::info!(
            "Quad index buffer: {} quads ({} KB)",
            new_quads,
            size / 1024
        );
    }

    pub fn record_copies(&mut self, cmd: vk::CommandBuffer, frame: usize) {
        // One-shot pattern upload for a (re)created quad index buffer; it
        // precedes the frame's draws in the same command buffer, so the
        // barrier below covers it.
        let quad_copy = self.uploads.quad_index_copy_pending;
        if quad_copy {
            self.uploads.quad_index_copy_pending = false;
            let src = self.uploads.quad_index_src.as_ref().unwrap();
            let copy = [vk::BufferCopy {
                src_offset: 0,
                dst_offset: 0,
                size: self.uploads.quad_index_quads as u64 * 6 * size_of::<u32>() as u64,
            }];
            cmd.copy_buffer(src.buffer, self.uploads.quad_index_buffer.buffer, &copy);
        }
        if self.uploads.pending_copies.is_empty() && !quad_copy {
            return;
        }
        if !self.uploads.pending_copies.is_empty() {
            let mut copy_v: Vec<vk::BufferCopy> =
                Vec::with_capacity(self.uploads.pending_copies.len());
            let mut stg_v = 0usize;
            {
                let buf = self.uploads.staging_buffers[frame].mapped_slice_mut();
                for pending in &self.uploads.pending_copies {
                    write_verts(buf, stg_v, &pending.vertices);
                    let vbytes = pending.vertices.len() * VERTEX_SIZE as usize;
                    copy_v.push(vk::BufferCopy {
                        src_offset: stg_v as u64,
                        dst_offset: pending.vtx_off as u64 * VERTEX_SIZE,
                        size: vbytes as u64,
                    });
                    stg_v += vbytes;
                }
            }
            cmd.copy_buffer(
                self.uploads.staging_buffers[frame].buffer,
                self.vertex_buffer.buffer,
                &copy_v,
            );
        }
        let barrier = vk::MemoryBarrier {
            src_access_mask: vk::AccessFlags::TransferWrite,
            dst_access_mask: vk::AccessFlags::VertexAttributeRead | vk::AccessFlags::IndexRead,
            ..Default::default()
        };
        cmd.pipeline_barrier(
            vk::PipelineStageFlags::Transfer,
            vk::PipelineStageFlags::VertexInput,
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
            vtx_off: u32,
            vcount: u32,
            meta_slot: u32,
        }
        let mut entries: Vec<BatchEntry> = Vec::new();

        // Include copies carried over from a skipped frame in the budget.
        let mut current_v_bytes = self.uploads.pending_v_bytes;
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
                        vertex_offset: 0,
                        vtx_len: 0,
                        content_gen,
                        epoch,
                    });
                mesh_queue.pop_front();
                continue;
            }
            let vcount = mesh.vertices.len() as u32;
            if self.uploads.use_staging {
                let v_bytes = vcount as usize * VERTEX_SIZE as usize;
                // A section too large for the staging slab is skipped, not overflowed.
                if v_bytes > staging_budget {
                    tracing::warn!(
                        "Section {:?} too large for staging ({} bytes), skipping",
                        mesh.spos,
                        v_bytes,
                    );
                    mesh_queue.pop_front();
                    continue;
                }
                // This transfer's staging budget is full; leave the rest queued.
                if current_v_bytes + v_bytes > staging_budget {
                    break;
                }
                current_v_bytes += v_bytes;
            }
            // The shared quad index buffer must cover the section's largest
            // single-pass draw.
            let max_quads = mesh
                .solid_quads
                .max(mesh.cutout_quads)
                .max(mesh.water_quads);
            self.ensure_quad_index_capacity(device, allocator, max_quads);
            let Some(vtx_off) = self.alloc_vertices(device, vcount) else {
                // Rate-limited: exhaustion persists across frames.
                let now = std::time::Instant::now();
                if self
                    .last_pool_warn
                    .is_none_or(|t| now.duration_since(t).as_secs() >= 5)
                {
                    self.last_pool_warn = Some(now);
                    tracing::warn!(
                        "vertex pool exhausted (largest free run {} verts, wanted {});                          uploads stalled for {:?}",
                        self.geometry.vertex_free.largest_free(),
                        vcount,
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
                vtx_off,
                vcount,
                meta_slot,
            });
        }

        if entries.is_empty() {
            return;
        }

        let now_ms = crate::renderer::camera::session_millis();
        for entry in &entries {
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
            self.queue_meta_write(
                entry.meta_slot,
                ChunkMeta {
                    aabb_min: entry.mesh.aabb.min,
                    aabb_max: entry.mesh.aabb.max,
                    solid_quads: entry.mesh.solid_quads,
                    cutout_quads: entry.mesh.cutout_quads,
                    vertex_offset: entry.vtx_off as i32,
                    uploaded_ms,
                    origin: [spos.x * 16, spos.y * 16, spos.z * 16],
                    water_quads: entry.mesh.water_quads,
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
                    vertex_offset: entry.vtx_off as i32,
                    vtx_len: entry.vcount,
                    content_gen: entry.mesh.content_gen,
                    epoch: entry.mesh.upload_epoch,
                });
        }

        if self.uploads.use_staging {
            for entry in &mut entries {
                self.uploads.pending_v_bytes += entry.mesh.vertices.len() * VERTEX_SIZE as usize;
                self.uploads.pending_copies.push(PendingCopy {
                    vertices: std::mem::take(&mut entry.mesh.vertices),
                    vtx_off: entry.vtx_off,
                });
            }
        } else {
            let vbuf = self.vertex_buffer.allocation.mapped_slice_mut().unwrap();
            for entry in &entries {
                let base = entry.vtx_off as usize * VERTEX_SIZE as usize;
                write_verts(vbuf, base, &entry.mesh.vertices);
            }
        }
    }
}
