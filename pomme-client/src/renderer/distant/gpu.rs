//! Distant terrain on the GPU: section meshes in vertex arenas, the
//! loaded-column mask that keeps them out of real chunks, and the draws.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use glam::DVec3;

use super::mesh::DistantVertex;
use super::store::{LEVELS, SectionKey};
use super::worker::DistantUpdate;
use crate::position::ChunkPos;
use crate::renderer::camera::Camera;
use crate::renderer::chunk::tlsf::Tlsf;
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, rhi};
use crate::world::chunk::{RING_SIDE, ring_slot};

/// Vertices an arena holds: 48 MB at 12 bytes each.
const ARENA_VERTICES: u32 = 4 << 20;
/// Arenas there may be: 384 MB of vertices.
const MAX_ARENAS: usize = 8;
/// Quads one draw covers, which the shared index buffer has room for; a
/// larger section draws in several.
const QUADS_PER_DRAW: u32 = 1 << 16;
const VERTEX_SIZE: u64 = size_of::<DistantVertex>() as u64;
/// `distant.frag` hard-codes the ring's side.
const _: () = assert!(RING_SIDE == 257);
const MASK_WORDS: usize = ((RING_SIDE * RING_SIDE) as usize).div_ceil(32);

/// A vertex buffer, sub-allocated.
struct Arena {
    buffer: rhi::backend::Buffer,
    vertices: Tlsf,
}

impl Arena {
    fn new(device: &rhi::backend::Device) -> Self {
        Self {
            buffer: rhi::backend::Buffer::new(
                device,
                "distant_vertices",
                u64::from(ARENA_VERTICES) * VERTEX_SIZE,
                rhi::BufferUsages::VERTEX,
                rhi::MemoryLocation::Host,
            ),
            vertices: Tlsf::new(ARENA_VERTICES),
        }
    }
}

/// A section's mesh in an arena, and the height it spans; no vertices for
/// a section with nothing in it.
struct Uploaded {
    arena: usize,
    first_vertex: u32,
    vertices: u32,
    min_y: u16,
    max_y: u16,
}

pub(crate) struct DistantBuffers {
    /// Grown one at a time as sections fill them.
    arenas: Vec<Arena>,
    index_buffer: rhi::backend::Buffer,
    sections: HashMap<SectionKey, Uploaded>,
    /// The dimension the sections are of, and its `min_y`.
    dimension: Option<Arc<str>>,
    min_y: i32,
    /// Bit per [`ring_slot`]: a real chunk is loaded there.
    mask: rhi::backend::Buffer,
    mask_words: Vec<u32>,
    frame_seq: u64,
    /// Freed vertex ranges as (arena, first vertex), by the frame from
    /// which no in-flight frame can still draw them.
    pending_free: VecDeque<(u64, usize, u32)>,
}

impl DistantBuffers {
    pub fn new(device: &rhi::backend::Device) -> Self {
        let quad_indices: Vec<u32> = (0..QUADS_PER_DRAW)
            .flat_map(|quad| [0, 1, 2, 2, 3, 0].map(|i| quad * 4 + i))
            .collect();
        Self {
            arenas: vec![Arena::new(device)],
            index_buffer: rhi::backend::Buffer::with_data(
                device,
                "distant_quad_indices",
                bytemuck::cast_slice(&quad_indices),
                rhi::BufferUsages::INDEX,
            ),
            sections: HashMap::new(),
            dimension: None,
            min_y: 0,
            mask: rhi::backend::Buffer::new(
                device,
                "distant_mask",
                (MASK_WORDS * size_of::<u32>()) as u64,
                rhi::BufferUsages::STORAGE,
                rhi::MemoryLocation::Host,
            ),
            mask_words: vec![0; MASK_WORDS],
            frame_seq: 0,
            pending_free: VecDeque::new(),
        }
    }

    pub fn mask(&self) -> &rhi::backend::Buffer {
        &self.mask
    }

    pub fn mask_bytes() -> u64 {
        (MASK_WORDS * size_of::<u32>()) as u64
    }

    /// Reclaims the ranges no in-flight frame reads any more. Call once a
    /// frame, after its fence wait.
    pub fn begin_frame(&mut self) {
        self.frame_seq += 1;
        while let Some(&(at, arena, first_vertex)) = self.pending_free.front()
            && at <= self.frame_seq
        {
            self.pending_free.pop_front();
            self.arenas[arena].vertices.free(first_vertex);
        }
    }

    fn retire(&mut self, key: SectionKey) {
        if let Some(old) = self.sections.remove(&key)
            && old.vertices > 0
        {
            let at = self.frame_seq + MAX_FRAMES_IN_FLIGHT as u64;
            self.pending_free
                .push_back((at, old.arena, old.first_vertex));
        }
    }

    /// Drops every section, as on a dimension change. The mask stays: it
    /// tracks the chunk store, which [`Self::clear_mask`] follows.
    pub fn clear(&mut self) {
        let keys: Vec<SectionKey> = self.sections.keys().copied().collect();
        for key in keys {
            self.retire(key);
        }
        self.dimension = None;
    }

    pub fn clear_mask(&mut self) {
        self.mask_words.fill(0);
        self.mask.zero();
    }

    pub fn set_column_loaded(&mut self, pos: ChunkPos, loaded: bool) {
        let slot = ring_slot(pos);
        let (word, bit) = (slot / 32, 1u32 << (slot % 32));
        let value = &mut self.mask_words[word];
        if loaded {
            *value |= bit;
        } else {
            *value &= !bit;
        }
        self.mask
            .write_at(word * size_of::<u32>(), &value.to_ne_bytes());
    }

    /// Room for `count` vertices: in an arena with it, else in a new one
    /// while there may be more.
    fn alloc(&mut self, device: &rhi::backend::Device, count: u32) -> Option<(usize, u32)> {
        if let Some(found) = self
            .arenas
            .iter_mut()
            .enumerate()
            .find_map(|(i, arena)| Some((i, arena.vertices.alloc(count)?)))
        {
            return Some(found);
        }
        if self.arenas.len() == MAX_ARENAS {
            return None;
        }
        let mut arena = Arena::new(device);
        let first_vertex = arena.vertices.alloc(count)?;
        self.arenas.push(arena);
        Some((self.arenas.len() - 1, first_vertex))
    }

    pub fn upload(&mut self, device: &rhi::backend::Device, update: DistantUpdate) {
        if self.dimension.as_ref() != Some(&update.dimension) {
            self.clear();
            self.dimension = Some(Arc::clone(&update.dimension));
        }
        self.min_y = update.min_y;
        for key in update.removed {
            self.retire(key);
        }
        for mesh in update.meshes {
            self.retire(mesh.key);
            let count = mesh.vertices.len() as u32;
            let (arena, first_vertex) = if count == 0 {
                (0, 0)
            } else {
                let Some((arena, first_vertex)) = self.alloc(device, count) else {
                    tracing::warn!("No room for distant terrain section {:?}", mesh.key);
                    continue;
                };
                self.arenas[arena].buffer.write_at(
                    (u64::from(first_vertex) * VERTEX_SIZE) as usize,
                    bytemuck::cast_slice(&mesh.vertices),
                );
                (arena, first_vertex)
            };
            self.sections.insert(
                mesh.key,
                Uploaded {
                    arena,
                    first_vertex,
                    vertices: count,
                    min_y: mesh.min_y,
                    max_y: mesh.max_y,
                },
            );
        }
    }

    /// Whether the finest section holding block column `x`/`z` is uploaded
    /// with something in it.
    #[cfg(feature = "test-harness")]
    pub fn has_column(&self, x: i32, z: i32) -> bool {
        let width = super::store::SECTION_COLUMNS;
        let key = SectionKey {
            level: 0,
            x: x.div_euclid(width),
            z: z.div_euclid(width),
        };
        self.sections
            .get(&key)
            .is_some_and(|section| section.vertices > 0)
    }

    /// The sections to draw from `eye` out to `radius` blocks: each at the
    /// finest level its distance allows, or its parent's until all four
    /// children in range are uploaded (Distant Horizons'
    /// `LodQuadTree.onDetailLevelTooLow`).
    fn visible(&self, eye: DVec3, radius: f64) -> Vec<SectionKey> {
        let mut out = Vec::new();
        let mut stack: Vec<SectionKey> = self
            .sections
            .keys()
            .copied()
            .filter(|key| key.level == LEVELS - 1)
            .collect();
        while let Some(key) = stack.pop() {
            if key.distance(eye.x, eye.z) > radius {
                continue;
            }
            let ready = key.children().filter(|children| {
                key.splits(eye.x, eye.z)
                    && children.iter().all(|child| {
                        self.sections.contains_key(child) || child.distance(eye.x, eye.z) > radius
                    })
            });
            match ready {
                Some(children) => stack.extend(children),
                None => out.push(key),
            }
        }
        out
    }

    /// Draws the sections in view out to `radius` blocks. The caller has
    /// bound the distant pipeline.
    pub fn draw(
        &self,
        pass: &rhi::backend::RenderPassEncoder,
        camera: &Camera,
        eye: DVec3,
        radius: f64,
    ) {
        if self.sections.is_empty() {
            return;
        }
        // Past the radius, so the edge fogs out before it clips.
        let view_proj = camera.distant_view_projection((radius * 1.5) as f32);
        let anchor = camera.anchor();
        let planes = Camera::planes_from_view_projection(view_proj);
        pass.set_push_constants(
            rhi::ShaderStages::VERTEX,
            0,
            bytemuck::cast_slice(&view_proj.to_cols_array()),
        );
        pass.set_index_buffer(self.index_buffer.handle());
        let mut visible = self.visible(eye, radius);
        visible.sort_unstable_by_key(|key| self.sections[key].arena);
        let mut bound = None;
        for key in visible {
            let section = &self.sections[&key];
            if section.vertices == 0 {
                continue;
            }
            let (ox, oz) = key.origin();
            let origin = DVec3::new(f64::from(ox), f64::from(self.min_y), f64::from(oz));
            let width = f64::from(key.width());
            let min = origin + DVec3::new(0.0, f64::from(section.min_y), 0.0) - eye;
            let max = origin + DVec3::new(width, f64::from(section.max_y), width) - eye;
            if !in_frustum(&planes, min.as_vec3(), max.as_vec3()) {
                continue;
            }
            if bound != Some(section.arena) {
                pass.set_vertex_buffer(0, self.arenas[section.arena].buffer.handle());
                bound = Some(section.arena);
            }
            let rel = (origin - anchor).as_vec3();
            pass.set_push_constants(
                rhi::ShaderStages::VERTEX,
                64,
                bytemuck::cast_slice(&[rel.x, rel.y, rel.z, 0.0]),
            );
            let quads = section.vertices / 4;
            for first_quad in (0..quads).step_by(QUADS_PER_DRAW as usize) {
                let count = (quads - first_quad).min(QUADS_PER_DRAW);
                pass.draw_indexed(
                    count * 6,
                    1,
                    0,
                    (section.first_vertex + first_quad * 4) as i32,
                    0,
                );
            }
        }
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        for arena in self.arenas.drain(..) {
            arena.buffer.destroy(device);
        }
        for buffer in [&mut self.index_buffer, &mut self.mask] {
            std::mem::take(buffer).destroy(device);
        }
    }
}

/// Whether the box between `min` and `max` (eye-relative) is at least
/// partly inside `planes`.
fn in_frustum(planes: &[[f32; 4]; 6], min: glam::Vec3, max: glam::Vec3) -> bool {
    planes.iter().all(|p| {
        let corner = |n: f32, lo: f32, hi: f32| if n >= 0.0 { hi } else { lo };
        p[0] * corner(p[0], min.x, max.x)
            + p[1] * corner(p[1], min.y, max.y)
            + p[2] * corner(p[2], min.z, max.z)
            + p[3]
            >= 0.0
    })
}
