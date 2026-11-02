use std::collections::{HashMap, HashSet, VecDeque};
use std::mem::offset_of;

use glam::DVec3;

use super::mesher::{
    ChunkAABB, ChunkMeshData, PackVertex, PackedVertex, SectionMesh, TerrainFormat,
};
use super::tlsf::Tlsf;
use crate::position::{BlockPos, ChunkPos};
use crate::renderer::rhi::IndirectDrawFeatures;
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, rhi, shader, util};

/// An arena of vanilla's 128 MiB vertex heap (`UberGpuBuffer`) at the
/// vanilla format's 16-byte vertex.
const ARENA_SIZE: ArenaSize = ArenaSize::new(8 << 20);
const INDEX_SIZE: u64 = size_of::<u32>() as u64;
/// Discrete GPUs batch a frame's uploads through the staging buffer in one
/// transfer, so it holds several columns to keep sub-flushes rare. Sized for
/// vanilla vertices; a shader pack's larger ones flush sooner.
const STAGING_SIZE: u64 = 12 << 20;
/// Without `VK_EXT_memory_budget`, the share of the arenas' memory heaps
/// they may take.
const BUDGET_FRACTION: f64 = 0.25;
/// With it, the least of the budget left for everything else.
const MIN_BUDGET_HEADROOM: u64 = 512 << 20;
/// Compaction starts once the arenas' contents would fit one fewer arena
/// filled this far.
const COMPACT_FILL: f64 = 0.8;
/// Vertices compaction moves per frame.
const COMPACT_VERTICES_PER_FRAME: u32 = 1 << 18;
/// Past the eviction frontier, starved columns come back only while this
/// fraction of an arena (as `1 / n`) stays free after them.
const FRONTIER_SLACK: u64 = 16;
/// Frames before compaction tries again after running out of room.
const COMPACT_BACKOFF_FRAMES: u64 = 600;
/// Frames between checks, while columns are starved, for a budget that grew.
const BUDGET_RECHECK_FRAMES: u64 = 60;
/// Draws the meta and indirect buffers start with room for. Growing them
/// waits the GPU out, so start generously.
const INITIAL_META: usize = 16384;
/// Arenas the draw count buffers have room for: far more than any budget
/// allows.
const MAX_ARENAS: usize = 256;

/// One arena's vertex and index heap lengths.
#[derive(Clone, Copy, PartialEq, Eq)]
struct ArenaSize {
    vertices: u32,
    indices: u32,
}

impl ArenaSize {
    /// Room for `vertices`, and six indices per four-vertex quad.
    const fn new(vertices: u32) -> Self {
        Self {
            vertices,
            indices: vertices / 2 * 3,
        }
    }

    /// Both heaps in bytes, at `format`'s vertex size.
    fn bytes(self, format: TerrainFormat) -> u64 {
        u64::from(self.vertices) * format.vertex_size() + u64::from(self.indices) * INDEX_SIZE
    }
}

/// The cull pass's bindings: meta, frustum, then indirect draws and a count
/// per [`Layer`], and the columns' visibility masks.
const CULL_BINDINGS: &[rhi::BindGroupLayoutEntry] = &[
    rhi::BindGroupLayoutEntry::storage(rhi::ShaderStages::COMPUTE),
    rhi::BindGroupLayoutEntry::uniform(rhi::ShaderStages::COMPUTE),
    rhi::BindGroupLayoutEntry::storage(rhi::ShaderStages::COMPUTE),
    rhi::BindGroupLayoutEntry::storage(rhi::ShaderStages::COMPUTE),
    rhi::BindGroupLayoutEntry::storage(rhi::ShaderStages::COMPUTE),
    rhi::BindGroupLayoutEntry::storage(rhi::ShaderStages::COMPUTE),
    rhi::BindGroupLayoutEntry::storage(rhi::ShaderStages::COMPUTE),
    rhi::BindGroupLayoutEntry::storage(rhi::ShaderStages::COMPUTE),
    rhi::BindGroupLayoutEntry::storage(rhi::ShaderStages::COMPUTE),
];
/// A draw count per arena.
const COUNTS_SIZE: u64 = (MAX_ARENAS * size_of::<u32>()) as u64;
const FRUSTUM_SIZE: u64 = size_of::<FrustumData>() as u64;
/// Column slots the visibility masks start with room for.
const INITIAL_COLUMNS: usize = 1 << 14;
/// The least meta slots an arena's segment holds; segments grow by doubling
/// so a section landing in one rarely moves the rest.
const MIN_SEGMENT: u32 = 64;
/// Staging buffers in flight: one per frame that may still read one, and one
/// written while they do.
const STAGING_SLOTS: usize = MAX_FRAMES_IN_FLIGHT + 1;
/// Columns within this squared X/Z distance of the camera render opaque
/// immediately and never fade in.
const NEARBY_DIST_SQ: f32 = 768.0;
/// A camera move this far, squared, re-sorts the draw lists front to back.
const SORT_RECAM_SQ: f64 = 64.0;

/// A render layer of the terrain: the index ranges the cull splits each
/// section's draw into, each drawn by its own pipeline.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// Opaque, no discard; first, so early-Z rejects what it hides.
    Solid,
    /// Alpha-tested.
    Cutout,
    /// Translucent water, blended after everything opaque.
    Water,
}

/// Whether a column's center is within the always-near X/Z radius of the eye
/// (vanilla `isNearby`), rebased in f64 for precision at extreme coordinates.
/// Also gates mesh-scheduling tiers (`in_game::apply_visibility`).
pub fn column_is_near(pos: ChunkPos, eye: DVec3) -> bool {
    column_dist_sq(pos, eye) < NEARBY_DIST_SQ as f64
}

/// The squared X/Z distance from `eye` to a column's center.
fn column_dist_sq(pos: ChunkPos, eye: DVec3) -> f64 {
    let dx = pos.x as f64 * 16.0 + 8.0 - eye.x;
    let dz = pos.z as f64 * 16.0 + 8.0 - eye.z;
    dx * dx + dz * dz
}

/// A section's entry in the draw lists (`cull.comp`'s `ChunkMeta`): what the
/// cull needs to test and draw it, static from its upload on but for
/// `visibility`, which the cull writes each frame.
#[repr(C)]
#[derive(Copy, Clone, PartialEq, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct ChunkMeta {
    /// Section-local vertex bounds; the cull shader rebases them via `origin`.
    aabb_min: [f32; 3],
    /// The section's arena: its draw count, and where its draws start.
    arena: u32,
    aabb_max: [f32; 3],
    index_count: u32,
    first_index: u32,
    vertex_offset: i32,
    /// The fade-in factor, read per instance by the vertex stage.
    visibility: u32,
    /// Leading indices belonging to the solid (no-discard) pass; the rest
    /// are cutout.
    solid_index_count: u32,
    /// Absolute section origin as integers (vanilla `ChunkPosition`), bound
    /// as a per-instance vertex attribute; the vertex shader subtracts the
    /// camera block position in integer math, so no large f32 is ever
    /// formed.
    origin: [i32; 3],
    /// Water indices, right after the opaque ones in the same slice.
    water_index_count: u32,
    /// The column's slot in the visibility masks and the section's bit.
    column: u32,
    section: u32,
    /// Milliseconds since the store was made at the upload, for the fade; 0
    /// for none.
    uploaded_ms: u32,
    _pad: u32,
}

impl ChunkMeta {
    /// An instance entry for a draw outside the section pool: just the
    /// origin, fully faded in.
    pub(crate) fn instance(origin: [i32; 3]) -> Self {
        Self {
            origin,
            visibility: 1.0f32.to_bits(),
            ..bytemuck::Zeroable::zeroed()
        }
    }

    /// The squared distance from `eye` to the section's center, as the
    /// front-to-back sort orders by; the origin is rebased in f64 for
    /// precision at extreme coordinates.
    fn center_dist_sq(&self, eye: DVec3) -> f64 {
        let center = DVec3::new(
            f64::from((self.aabb_min[0] + self.aabb_max[0]) * 0.5),
            f64::from((self.aabb_min[1] + self.aabb_max[1]) * 0.5),
            f64::from((self.aabb_min[2] + self.aabb_max[2]) * 0.5),
        );
        (origin_dvec(self.origin) + center - eye).length_squared()
    }
}

/// [`PackedVertex`]'s attributes; a [`PackVertex`] starts with one.
const VERTEX_ATTRIBUTES: [rhi::VertexAttribute; 5] = {
    const POS: u32 = offset_of!(PackedVertex, pos) as u32;
    [
        // pos, split into xy + z lanes
        rhi::VertexAttribute {
            location: 0,
            format: rhi::VertexFormat::Unorm16x2,
            offset: POS,
        },
        rhi::VertexAttribute {
            location: 1,
            format: rhi::VertexFormat::Unorm16,
            offset: POS + 4,
        },
        rhi::VertexAttribute {
            location: 2,
            format: rhi::VertexFormat::Uint32,
            offset: offset_of!(PackedVertex, uv) as u32,
        },
        rhi::VertexAttribute {
            location: 3,
            format: rhi::VertexFormat::Uint16,
            offset: offset_of!(PackedVertex, sprite) as u32,
        },
        rhi::VertexAttribute {
            location: 4,
            format: rhi::VertexFormat::Unorm8x4,
            offset: offset_of!(PackedVertex, light_tint) as u32,
        },
    ]
};

const PACK_VERTEX_ATTRIBUTES: [rhi::VertexAttribute; 7] = {
    let [pos_xy, pos_z, uv, sprite, light_tint] = VERTEX_ATTRIBUTES;
    [
        pos_xy,
        pos_z,
        uv,
        sprite,
        light_tint,
        rhi::VertexAttribute {
            location: 7,
            format: rhi::VertexFormat::Unorm8x4,
            offset: offset_of!(PackVertex, shade) as u32,
        },
        rhi::VertexAttribute {
            location: 8,
            format: rhi::VertexFormat::Uint32,
            offset: offset_of!(PackVertex, entity) as u32,
        },
    ]
};

/// Vertex input for the chunk pipeline: buffer 0 is the packed per-vertex
/// pool, buffer 1 is the meta buffer read per-instance (origin + fade),
/// indexed by the `first_instance` the cull shader writes.
pub const CHUNK_VERTEX_BUFFERS: [rhi::VertexBufferLayout<'static>; 2] =
    chunk_vertex_buffers(size_of::<PackedVertex>(), &VERTEX_ATTRIBUTES);

/// [`CHUNK_VERTEX_BUFFERS`] over [`PackVertex`]es, for a shader pack.
pub const PACK_CHUNK_VERTEX_BUFFERS: [rhi::VertexBufferLayout<'static>; 2] =
    chunk_vertex_buffers(size_of::<PackVertex>(), &PACK_VERTEX_ATTRIBUTES);

const fn chunk_vertex_buffers(
    stride: usize,
    attributes: &'static [rhi::VertexAttribute],
) -> [rhi::VertexBufferLayout<'static>; 2] {
    [
        rhi::VertexBufferLayout {
            stride: stride as u32,
            step_mode: rhi::VertexStepMode::Vertex,
            attributes,
        },
        rhi::VertexBufferLayout {
            stride: size_of::<ChunkMeta>() as u32,
            step_mode: rhi::VertexStepMode::Instance,
            attributes: &[
                rhi::VertexAttribute {
                    location: 5,
                    format: rhi::VertexFormat::Sint32x3,
                    offset: offset_of!(ChunkMeta, origin) as u32,
                },
                rhi::VertexAttribute {
                    location: 6,
                    format: rhi::VertexFormat::Float32,
                    offset: offset_of!(ChunkMeta, visibility) as u32,
                },
            ],
        },
    ]
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct DrawCommand {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    vertex_offset: i32,
    first_instance: u32,
}

/// An integer section origin widened for f64 math.
fn origin_dvec(origin: [i32; 3]) -> DVec3 {
    DVec3::new(origin[0] as f64, origin[1] as f64, origin[2] as f64)
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct FrustumData {
    planes: [[f32; 4]; 6],
    /// Meta slots to walk: every segment, headroom included.
    chunk_count: u32,
    /// Camera block position (the render anchor as integers); the cull
    /// subtracts it from the absolute integer section origins.
    cam_block: [i32; 3],
    /// Eye position relative to `cam_block` (small, full precision).
    frac: [f32; 3],
    /// Milliseconds since the store was made, for the fade.
    now_ms: u32,
    /// Whether the columns' occlusion masks apply.
    use_visibility: u32,
    _pad: [u32; 3],
    /// Per arena, where its segment starts and how many sections it holds,
    /// four to a row as std140 lays `uint` arrays out.
    arena_base: [[u32; 4]; MAX_ARENAS / 4],
    arena_len: [[u32; 4]; MAX_ARENAS / 4],
}

/// The columns with uploaded sections, each in a slot the visibility masks
/// and the metas refer to it by.
#[derive(Default)]
pub(crate) struct Columns {
    slots: Vec<Option<Column>>,
    free: Vec<u32>,
    by_pos: HashMap<ChunkPos, u32>,
    /// Sections over every column.
    sections: usize,
}

pub(crate) struct Column {
    pos: ChunkPos,
    sections: Vec<SectionAlloc>,
    /// What the sections take, summed.
    vertices: u64,
    indices: u64,
}

impl Column {
    /// What the column's sections take.
    fn need(&self) -> Need {
        let mut need = Need::default();
        for sec in &self.sections {
            need.add(sec.vtx_len, sec.idx_len);
        }
        need
    }
}

impl Columns {
    fn get(&self, pos: &ChunkPos) -> Option<&Column> {
        self.by_pos.get(pos).map(|&slot| self.by_slot(slot))
    }

    fn slot(&self, pos: &ChunkPos) -> Option<u32> {
        self.by_pos.get(pos).copied()
    }

    fn by_slot(&self, slot: u32) -> &Column {
        self.slots[slot as usize]
            .as_ref()
            .expect("a column slot in use")
    }

    fn by_slot_mut(&mut self, slot: u32) -> &mut Column {
        self.slots[slot as usize]
            .as_mut()
            .expect("a column slot in use")
    }

    /// `pos`'s slot, taking one if it has none.
    fn slot_or_insert(&mut self, pos: ChunkPos) -> u32 {
        if let Some(&slot) = self.by_pos.get(&pos) {
            return slot;
        }
        let column = Column {
            pos,
            sections: Vec::new(),
            vertices: 0,
            indices: 0,
        };
        let slot = match self.free.pop() {
            Some(slot) => {
                self.slots[slot as usize] = Some(column);
                slot
            }
            None => {
                self.slots.push(Some(column));
                self.slots.len() as u32 - 1
            }
        };
        self.by_pos.insert(pos, slot);
        slot
    }

    fn remove(&mut self, pos: &ChunkPos) -> Option<Column> {
        let slot = self.by_pos.remove(pos)?;
        let column = self.slots[slot as usize].take()?;
        self.free.push(slot);
        self.sections -= column.sections.len();
        Some(column)
    }

    /// Adds `section` to `slot`'s column, returning its index there.
    fn push(&mut self, slot: u32, section: SectionAlloc) -> usize {
        self.sections += 1;
        let column = self.by_slot_mut(slot);
        column.vertices += u64::from(section.vtx_len);
        column.indices += u64::from(section.idx_len);
        column.sections.push(section);
        column.sections.len() - 1
    }

    /// Drops `slot`'s sections `keep` rejects, handing each to `dropped`.
    fn retain(
        &mut self,
        slot: u32,
        mut keep: impl FnMut(&SectionAlloc) -> bool,
        mut dropped: impl FnMut(SectionAlloc),
    ) {
        let column = self.by_slot_mut(slot);
        let mut i = 0;
        let mut removed = 0;
        while i < column.sections.len() {
            if keep(&column.sections[i]) {
                i += 1;
                continue;
            }
            let section = column.sections.swap_remove(i);
            column.vertices -= u64::from(section.vtx_len);
            column.indices -= u64::from(section.idx_len);
            removed += 1;
            dropped(section);
        }
        self.sections -= removed;
    }

    fn iter(&self) -> impl Iterator<Item = (u32, &Column)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(slot, column)| Some((slot as u32, column.as_ref()?)))
    }

    fn len(&self) -> usize {
        self.by_pos.len()
    }

    fn is_empty(&self) -> bool {
        self.by_pos.is_empty()
    }

    fn clear(&mut self) {
        self.slots.clear();
        self.free.clear();
        self.by_pos.clear();
        self.sections = 0;
    }
}

/// One arena's run of meta slots in the meta and indirect buffers: its
/// sections' metas, then headroom up to `cap`, so a section arriving or
/// leaving touches only this run.
#[derive(Default)]
struct Segment {
    base: u32,
    cap: u32,
    metas: Vec<ChunkMeta>,
    /// Slots of the run to write to the GPU, over the next `pending` frame
    /// slots; the end may lie past `metas` to clear slots that emptied.
    dirty: Option<(u32, u32)>,
    pending: u32,
}

impl Segment {
    fn mark(&mut self, lo: u32, hi: u32) {
        self.dirty = Some(match self.dirty {
            Some((a, b)) => (a.min(lo), b.max(hi)),
            None => (lo, hi),
        });
        self.pending = MAX_FRAMES_IN_FLIGHT as u32;
    }
}

/// The per-arena draw lists, kept up to date as sections come and go rather
/// than rebuilt, laid out back to back in the meta buffer.
#[derive(Default)]
pub(crate) struct Lists {
    columns: Columns,
    segments: Vec<Segment>,
    /// The next segment the front-to-back sort reaches, while one is under
    /// way.
    sort_at: Option<usize>,
    /// Eye position at the last front-to-back sort.
    last_sort_cam: DVec3,
}

impl Lists {
    /// Meta slots over every segment, headroom included.
    fn slots(&self) -> u32 {
        self.segments
            .iter()
            .map(|s| s.base + s.cap)
            .max()
            .unwrap_or(0)
    }

    /// Adds `section` to `slot`'s column and lists it, as
    /// [`Self::list_meta`] does.
    fn insert(&mut self, slot: u32, section: SectionAlloc) -> bool {
        let section_index = section.section_index;
        self.columns.push(slot, section);
        self.list_meta(slot, section_index)
    }

    /// Lists the meta of `slot`'s section `section_index` in its arena's
    /// segment, laying the segments out anew when that is full. Returns
    /// whether they moved.
    fn list_meta(&mut self, slot: u32, section_index: i32) -> bool {
        let sec = self.section_at(slot, section_index);
        let (arena, meta) = (sec.arena as usize, sec.meta(slot));
        if self.segments.len() <= arena {
            self.segments.resize_with(arena + 1, Segment::default);
        }
        let at = self.segments[arena].metas.len() as u32;
        self.section_at(slot, section_index).meta_index = at;
        let segment = &mut self.segments[arena];
        segment.metas.push(meta);
        if at < segment.cap {
            segment.mark(at, at + 1);
            return false;
        }
        self.relayout();
        true
    }

    /// Drops the meta at `index` of `arena`'s list, moving its last into the
    /// gap.
    fn remove(&mut self, arena: u16, index: u32) {
        let segment = &mut self.segments[arena as usize];
        let last = segment.metas.len() as u32 - 1;
        segment.metas.swap_remove(index as usize);
        segment.mark(index, last + 1);
        if index < last {
            let moved = segment.metas[index as usize];
            self.section_mut(&moved).meta_index = index;
        }
    }

    /// The section `meta` stands for.
    fn section_mut(&mut self, meta: &ChunkMeta) -> &mut SectionAlloc {
        self.section_at(meta.column, meta.section as i32)
    }

    /// `slot`'s section `section_index`.
    fn section_at(&mut self, slot: u32, section_index: i32) -> &mut SectionAlloc {
        self.columns
            .by_slot_mut(slot)
            .sections
            .iter_mut()
            .find(|s| s.section_index == section_index)
            .expect("a column's section")
    }

    /// Drops `slot`'s sections `keep` rejects from their lists and the
    /// column, returning them.
    fn remove_sections(
        &mut self,
        slot: u32,
        keep: impl Fn(&SectionAlloc) -> bool,
    ) -> Vec<SectionAlloc> {
        let to_drop: Vec<i32> = self
            .columns
            .by_slot(slot)
            .sections
            .iter()
            .filter(|s| !keep(s))
            .map(|s| s.section_index)
            .collect();
        for si in to_drop {
            let (arena, index) = {
                let sec = self.section_at(slot, si);
                (sec.arena, sec.meta_index)
            };
            self.remove(arena, index);
        }
        let mut dropped = Vec::new();
        self.columns
            .retain(slot, keep, |section| dropped.push(section));
        dropped
    }

    /// Takes `slot`'s section `section_index` out of its list, lets `update`
    /// change where its geometry lives, and lists it again. Returns whether
    /// the segments moved.
    fn move_section(
        &mut self,
        slot: u32,
        section_index: i32,
        update: impl FnOnce(&mut SectionAlloc),
    ) -> bool {
        let (arena, index) = {
            let sec = self.section_at(slot, section_index);
            (sec.arena, sec.meta_index)
        };
        self.remove(arena, index);
        update(self.section_at(slot, section_index));
        self.list_meta(slot, section_index)
    }

    /// Lays the segments out back to back, each with headroom.
    fn relayout(&mut self) {
        let mut base = 0;
        for segment in &mut self.segments {
            let len = segment.metas.len() as u32;
            segment.base = base;
            segment.cap = len.max(MIN_SEGMENT).next_power_of_two();
            base += segment.cap;
            segment.mark(0, segment.cap);
        }
    }

    /// Starts a front-to-back sort of every list when the eye has moved far
    /// enough since the last, then sorts one list per call until done.
    pub(crate) fn sort_step(&mut self, eye: DVec3) {
        if (eye - self.last_sort_cam).length_squared() > SORT_RECAM_SQ {
            self.last_sort_cam = eye;
            self.sort_at = Some(0);
        }
        let Some(start) = self.sort_at else {
            return;
        };
        let Some(arena) = (start..self.segments.len()).find(|&i| self.segments[i].metas.len() > 1)
        else {
            self.sort_at = None;
            return;
        };
        self.sort_at = Some(arena + 1);
        let eye = self.last_sort_cam;
        let mut metas = std::mem::take(&mut self.segments[arena].metas);
        metas.sort_by_cached_key(|m| (m.center_dist_sq(eye) as f32).to_bits());
        for (i, meta) in metas.iter().enumerate() {
            self.section_mut(meta).meta_index = i as u32;
        }
        let segment = &mut self.segments[arena];
        segment.metas = metas;
        segment.mark(0, segment.metas.len() as u32);
    }

    /// Hands the slots marked since to `write`, as `(byte offset, metas)`,
    /// for one frame slot.
    fn write_pending(&mut self, mut write: impl FnMut(usize, &[ChunkMeta])) {
        for segment in &mut self.segments {
            let Some((lo, hi)) = segment.dirty else {
                continue;
            };
            let hi = hi.min(segment.cap);
            let len = segment.metas.len() as u32;
            let metas: Vec<ChunkMeta> = (lo..hi)
                .map(|i| {
                    if i < len {
                        segment.metas[i as usize]
                    } else {
                        bytemuck::Zeroable::zeroed()
                    }
                })
                .collect();
            write(
                (segment.base + lo) as usize * size_of::<ChunkMeta>(),
                &metas,
            );
            segment.pending -= 1;
            if segment.pending == 0 {
                segment.dirty = None;
            }
        }
    }

    fn clear(&mut self) {
        self.columns.clear();
        for segment in &mut self.segments {
            segment.metas.clear();
        }
        self.relayout();
        self.sort_at = None;
    }
}

/// One uploaded 16³ section: a self-contained indexed draw plus its tight AABB.
/// `first_index`/`index_count` are the section's index slice and
/// `vertex_offset` its vertex slice base, both in `arena`; `vtx_len` is the
/// slice length, kept so the slices can be returned to the arena on removal.
/// `uploaded_ms` drives the per-section fade so editing one section never
/// re-fades the rest of the column.
pub(crate) struct SectionAlloc {
    section_index: i32,
    aabb: ChunkAABB,
    /// Section world origin (`chunk*16`, `min_y + si*16`), used to rebase the
    /// quantized vertices and passed to the GPU via `ChunkMeta.origin`.
    origin: [i32; 3],
    arena: u16,
    first_index: u32,
    /// Opaque index count (the GPU-culled draw); water is excluded.
    index_count: u32,
    /// Leading indices belonging to the solid (no-discard) pass; the rest are
    /// cutout.
    solid_index_count: u32,
    /// Translucent water indices, stored right after the opaque ones in the
    /// same index allocation and drawn in a separate blended pass.
    water_index_count: u32,
    /// Total allocated index slice length (opaque + water), for freeing.
    idx_len: u32,
    vertex_offset: i32,
    vtx_len: u32,
    /// Milliseconds since the store was made at the upload; 0 for no fade.
    uploaded_ms: u32,
    /// Upload epoch this section's geometry came from; an older upload is
    /// rejected. See [`ChunkMeshData::upload_epoch`].
    epoch: u64,
    /// Where its meta sits in its arena's list.
    meta_index: u32,
}

impl SectionAlloc {
    fn slice(&self) -> Slice {
        Slice {
            arena: self.arena,
            vertex_offset: self.vertex_offset as u32,
            #[cfg(feature = "test-harness")]
            vertex_len: self.vtx_len,
            first_index: self.first_index,
            #[cfg(feature = "test-harness")]
            index_len: self.idx_len,
        }
    }

    /// The section's draw-list entry, for a column in `slot`.
    fn meta(&self, slot: u32) -> ChunkMeta {
        let [min_x, min_y, min_z, _] = self.aabb.min;
        let [max_x, max_y, max_z, _] = self.aabb.max;
        ChunkMeta {
            aabb_min: [min_x, min_y, min_z],
            arena: u32::from(self.arena),
            aabb_max: [max_x, max_y, max_z],
            index_count: self.index_count,
            first_index: self.first_index,
            vertex_offset: self.vertex_offset,
            visibility: 1.0f32.to_bits(),
            solid_index_count: self.solid_index_count,
            origin: self.origin,
            water_index_count: self.water_index_count,
            column: slot,
            section: self.section_index as u32,
            uploaded_ms: self.uploaded_ms,
            _pad: 0,
        }
    }
}

#[cfg(any(test, feature = "cpu-bench"))]
impl Lists {
    /// Lists synthetic `sections` of the column at `pos`, all in `arena`.
    pub(crate) fn list(
        &mut self,
        pos: ChunkPos,
        min_y: i32,
        sections: impl IntoIterator<Item = i32>,
        arena: u16,
    ) {
        let slot = self.columns.slot_or_insert(pos);
        for si in sections {
            self.insert(slot, SectionAlloc::synthetic(pos, min_y, si, arena));
        }
    }
}

#[cfg(feature = "cpu-bench")]
impl Lists {
    /// Sets the next sort step at `arena`, keyed to `eye`.
    pub(crate) fn sort_from(&mut self, arena: usize, eye: DVec3) {
        self.last_sort_cam = eye;
        self.sort_at = Some(arena);
    }

    /// Drops the column at `pos` from its lists.
    pub(crate) fn drop_column(&mut self, pos: ChunkPos) {
        if let Some(slot) = self.columns.slot(&pos) {
            self.remove_sections(slot, |_| false);
            self.columns.remove(&pos);
        }
    }
}

#[cfg(any(test, feature = "cpu-bench"))]
impl SectionAlloc {
    /// A synthetic full-size section `si` of the column at `pos`, in `arena`,
    /// for tests and the CPU bench.
    pub(crate) fn synthetic(pos: ChunkPos, min_y: i32, si: i32, arena: u16) -> Self {
        Self {
            section_index: si,
            aabb: ChunkAABB {
                min: [0.0, 0.0, 0.0, 0.0],
                max: [16.0, 16.0, 16.0, 0.0],
            },
            origin: [pos.x * 16, min_y + si * 16, pos.z * 16],
            arena,
            first_index: 0,
            index_count: 6,
            solid_index_count: 6,
            water_index_count: 0,
            idx_len: 6,
            vertex_offset: 0,
            vtx_len: 4,
            uploaded_ms: 0,
            epoch: 0,
            meta_index: 0,
        }
    }
}

/// A slice of one arena's vertex and index heaps. Freeing goes by offset;
/// the lengths are for the harness's overlap check.
#[derive(Clone, Copy)]
struct Slice {
    arena: u16,
    vertex_offset: u32,
    #[cfg(feature = "test-harness")]
    vertex_len: u32,
    first_index: u32,
    #[cfg(feature = "test-harness")]
    index_len: u32,
}

/// What a column's sections take: in total, and the largest one's slices.
#[derive(Clone, Copy, Default)]
struct Need {
    vertices: u64,
    indices: u64,
    max_vertices: u32,
    max_indices: u32,
}

impl Need {
    fn add(&mut self, vertices: u32, indices: u32) {
        self.vertices += u64::from(vertices);
        self.indices += u64::from(indices);
        self.max_vertices = self.max_vertices.max(vertices);
        self.max_indices = self.max_indices.max(indices);
    }

    /// Whether `free_vertices` and `free_indices` hold this.
    fn fits_in(self, free_vertices: u64, free_indices: u64) -> bool {
        self.vertices <= free_vertices && self.indices <= free_indices
    }

    fn max(self, other: Self) -> Self {
        Self {
            vertices: self.vertices.max(other.vertices),
            indices: self.indices.max(other.indices),
            max_vertices: self.max_vertices.max(other.max_vertices),
            max_indices: self.max_indices.max(other.max_indices),
        }
    }
}

/// A column's sections that found no room, and what they need.
#[derive(Default)]
struct Starved {
    sections: HashSet<i32>,
    need: Need,
}

/// One vertex and one index heap, sub-allocated by [`Tlsf`]: a heap of
/// vanilla's `UberGpuBuffer`, its vertex and index halves paired so each
/// arena draws in one call. Device-local behind the staging buffer, in
/// memory of its own so destroying it hands the VRAM back; host-visible on
/// integrated GPUs.
struct Arena {
    vertex_buffer: rhi::backend::Buffer,
    index_buffer: rhi::backend::Buffer,
    vertices: Tlsf,
    indices: Tlsf,
    /// Empty, and destroyed at this `frame_seq` once no in-flight frame reads
    /// it. Never allocated from or drawn meanwhile.
    doomed: Option<u64>,
}

impl Arena {
    fn new(
        device: &rhi::backend::Device,
        use_staging: bool,
        format: TerrainFormat,
        size: ArenaSize,
    ) -> Self {
        Self {
            vertex_buffer: create_pool(
                device,
                use_staging,
                "vertex_arena",
                u64::from(size.vertices) * format.vertex_size(),
                rhi::BufferUsages::VERTEX,
            ),
            index_buffer: create_pool(
                device,
                use_staging,
                "index_arena",
                u64::from(size.indices) * INDEX_SIZE,
                rhi::BufferUsages::INDEX,
            ),
            vertices: Tlsf::new(size.vertices),
            indices: Tlsf::new(size.indices),
            doomed: None,
        }
    }

    fn is_empty(&self) -> bool {
        self.vertices.used() == 0 && self.indices.used() == 0
    }

    fn destroy(self, device: &rhi::backend::Device) {
        self.vertex_buffer.destroy(device);
        self.index_buffer.destroy(device);
    }
}

/// The arena that can be drawn from and allocated in: neither destroyed nor
/// doomed.
fn live(slot: &Option<Arena>) -> Option<&Arena> {
    slot.as_ref().filter(|arena| arena.doomed.is_none())
}

/// A vertex and an index slice from one live arena other than `exclude`,
/// trying the fullest first so the emptiest can drain.
fn alloc_in(
    arenas: &mut [Option<Arena>],
    exclude: Option<u16>,
    vertices: u32,
    indices: u32,
) -> Option<(u16, u32, u32)> {
    let mut order: Vec<usize> = (0..arenas.len())
        .filter(|&id| live(&arenas[id]).is_some() && exclude != Some(id as u16))
        .collect();
    order.sort_unstable_by_key(|&id| {
        std::cmp::Reverse(arenas[id].as_ref().map_or(0, |a| a.vertices.used()))
    });
    for id in order {
        let Some(arena) = arenas[id].as_mut() else {
            continue;
        };
        let Some(vertex) = arena.vertices.alloc(vertices) else {
            continue;
        };
        let Some(index) = arena.indices.alloc(indices) else {
            arena.vertices.free(vertex);
            continue;
        };
        return Some((id as u16, vertex, index));
    }
    None
}

/// A copy into an arena: the arena, and its vertex and index copies.
type ArenaCopy = (u16, rhi::BufferCopy, rhi::BufferCopy);

/// Groups `copies` by destination arena and records them from the `src`
/// vertex and index buffers.
fn record_copies(
    encoder: &rhi::backend::CommandEncoder,
    arenas: &[Option<Arena>],
    src: (rhi::backend::BufferHandle, rhi::backend::BufferHandle),
    copies: &mut [ArenaCopy],
) {
    copies.sort_unstable_by_key(|&(arena, ..)| arena);
    for group in copies.chunk_by(|a, b| a.0 == b.0) {
        let Some(dst) = arenas[group[0].0 as usize].as_ref() else {
            continue;
        };
        let vertex: Vec<_> = group.iter().map(|&(_, v, _)| v).collect();
        let index: Vec<_> = group.iter().map(|&(.., i)| i).collect();
        encoder.copy_buffer(src.0, dst.vertex_buffer.handle(), &vertex);
        encoder.copy_buffer(src.1, dst.index_buffer.handle(), &index);
    }
}

/// Records `copies` grouped by their tag, from the vertex and index buffers
/// `src_of` names for each tag; a tag it names none for is skipped.
fn record_grouped<T: Copy + Ord>(
    encoder: &rhi::backend::CommandEncoder,
    arenas: &[Option<Arena>],
    copies: &mut [(T, ArenaCopy)],
    mut src_of: impl FnMut(T) -> Option<(rhi::backend::BufferHandle, rhi::backend::BufferHandle)>,
) {
    copies.sort_unstable_by_key(|&(tag, _)| tag);
    for group in copies.chunk_by(|a, b| a.0 == b.0) {
        let Some(src) = src_of(group[0].0) else {
            continue;
        };
        let mut copies: Vec<_> = group.iter().map(|&(_, copy)| copy).collect();
        record_copies(encoder, arenas, src, &mut copies);
    }
}

/// The camera's draw list and a shader pack's `shadow` one, if made.
fn draw_lists<'a>(
    camera: &'a mut DrawList,
    shadow: &'a mut Option<DrawList>,
) -> impl Iterator<Item = &'a mut DrawList> {
    std::iter::once(camera).chain(shadow)
}

pub struct ChunkBufferStore {
    indirect_draw: IndirectDrawFeatures,
    /// By id; a destroyed arena's slot is reused by the next one.
    arenas: Vec<Option<Arena>>,
    /// A frame's uploads, written during the update and copied into the
    /// arenas as the frame starts; one per frame that may still read one.
    staging: Vec<rhi::backend::Buffer>,
    /// How much of each staging buffer's halves the pending copies use, and
    /// the `frame_seq` until which frames may still read it.
    staging_used: [(usize, usize); STAGING_SLOTS],
    staging_busy: [u64; STAGING_SLOTS],
    /// Staging copies to record as the next frame starts, by staging slot.
    pending_copies: Vec<(usize, ArenaCopy)>,
    /// Compaction's copies to record as the next frame starts, by source
    /// arena.
    pending_moves: Vec<(u16, ArenaCopy)>,
    use_staging: bool,
    format: TerrainFormat,
    arena_size: ArenaSize,
    /// The most arenas there may be, whatever the budget.
    max_arenas: usize,
    /// Columns whose sections found no room, or were evicted for nearer
    /// ones; handed back for re-meshing once there's room for them.
    starved: HashMap<ChunkPos, Starved>,
    /// The starved columns are worth retrying in the next
    /// [`Self::upload_batch`]: a slice was reclaimed, the eye reached another
    /// column, or the budget grew.
    retry_due: bool,
    /// Columns this far from the eye, squared, or farther were evicted for
    /// nearer ones, or are past those that were, and wait for room. It moves
    /// back out as room frees.
    frontier_sq: f64,
    /// The arena compaction is emptying, kept out of allocation.
    draining: Option<u16>,
    /// The `frame_seq` before which compaction doesn't start.
    compact_after: u64,

    lists: Lists,
    /// Per-column bitmask of occlusion-visible section indices (bit `si`), from
    /// the CPU visibility graph. A column absent here defaults to fully
    /// visible, so freshly-loaded-but-not-yet-graphed columns still draw.
    chunk_visibility: HashMap<ChunkPos, u32>,
    /// The masks by column slot, as the cull reads them, and the frame slots
    /// still to receive them.
    column_visibility: Vec<u32>,
    visibility_pending: u32,
    /// Per frame slot, the metas and the column masks the culls read.
    meta_buffers: Vec<rhi::backend::Buffer>,
    visibility_buffers: Vec<rhi::backend::Buffer>,
    /// Capacity, in meta slots and column slots, of those buffers.
    max_meta: usize,
    max_columns: usize,
    camera: DrawList,
    /// A shader pack's shadow view, made on its first cull.
    shadow: Option<DrawList>,
    /// The camera's per-arena draw counts, solid then cutout, copied after
    /// each cull for the debug overlay.
    count_readbacks: Vec<rhi::backend::Buffer>,
    /// The eye of the last cull, which eviction measures distance from.
    eye: Option<DVec3>,
    /// What `uploaded_ms` counts from.
    epoch: std::time::Instant,

    compute_pipeline: rhi::backend::ComputePipeline,
    compute_layout: rhi::backend::PipelineLayout,
    cull_layout: rhi::backend::BindGroupLayout,

    /// Sections uploaded before the first cull appear at once; later ones
    /// fade in.
    fade_enabled: bool,
    /// Post-cull section draw count read back from the GPU (lags a few frames);
    /// exposed for the debug overlay so occlusion's effect is visible.
    last_draw_count: u32,

    /// Monotonic frame counter, bumped once per rendered frame in
    /// `begin_frame`.
    frame_seq: u64,
    /// Slices freed by a re-mesh, unload, eviction or compaction, each tagged
    /// with the `frame_seq` at which it's safe to reclaim
    /// (`MAX_FRAMES_IN_FLIGHT` out, so no in-flight frame still draws it).
    /// Drained in `begin_frame`.
    pending_free: VecDeque<(u64, Slice)>,
}

impl ChunkBufferStore {
    pub fn new(device: &rhi::backend::Device, indirect_draw: IndirectDrawFeatures) -> Self {
        let use_staging = device.discrete;
        let format = TerrainFormat::Vanilla;

        // The integrated path writes mapped memory directly and never touches
        // the staging buffers.
        let staging = (0..STAGING_SLOTS)
            .map(|_| {
                rhi::backend::Buffer::new(
                    device,
                    "staging",
                    if use_staging { STAGING_SIZE } else { 64 << 10 },
                    rhi::BufferUsages::COPY_SRC,
                    rhi::MemoryLocation::Host,
                )
            })
            .collect();

        let location = arena_location(use_staging);
        tracing::info!(
            "Chunk arenas: {}, {} MB each; their heaps {} MB, budget query {}",
            if use_staging {
                "DEVICE_LOCAL + staging"
            } else {
                "HOST_VISIBLE"
            },
            ARENA_SIZE.bytes(format) / (1024 * 1024),
            device.memory_bytes(location) / (1024 * 1024),
            if device.memory_budget(location).is_some() {
                "on"
            } else {
                "off"
            },
        );

        let cull_layout = rhi::backend::create_bind_group_layout(device, CULL_BINDINGS);
        let compute_layout = rhi::backend::create_pipeline_layout(device, &[cull_layout], None);
        let compute_pipeline = rhi::backend::create_compute_pipeline(
            device,
            compute_layout,
            "cull",
            shader::include_shader!("cull.comp"),
        );
        let meta_buffers = meta_buffers(device, INITIAL_META);
        let visibility_buffers = visibility_buffers(device, INITIAL_COLUMNS);
        let camera = DrawList::new(
            device,
            cull_layout,
            &meta_buffers,
            &visibility_buffers,
            INITIAL_META,
        );

        Self {
            indirect_draw,
            arenas: Vec::new(),
            staging,
            staging_used: [(0, 0); STAGING_SLOTS],
            staging_busy: [0; STAGING_SLOTS],
            pending_copies: Vec::new(),
            pending_moves: Vec::new(),
            use_staging,
            format,
            arena_size: ARENA_SIZE,
            max_arenas: MAX_ARENAS,
            starved: HashMap::new(),
            retry_due: false,
            frontier_sq: f64::INFINITY,
            draining: None,
            compact_after: 0,
            lists: Lists::default(),
            chunk_visibility: HashMap::new(),
            column_visibility: Vec::new(),
            visibility_pending: 0,
            meta_buffers,
            visibility_buffers,
            max_meta: INITIAL_META,
            max_columns: INITIAL_COLUMNS,
            camera,
            shadow: None,
            count_readbacks: (0..MAX_FRAMES_IN_FLIGHT)
                .map(|_| {
                    rhi::backend::Buffer::new(
                        device,
                        "draw_count_readback",
                        2 * COUNTS_SIZE,
                        rhi::BufferUsages::COPY_DST,
                        rhi::MemoryLocation::Readback,
                    )
                })
                .collect(),
            eye: None,
            epoch: std::time::Instant::now(),
            compute_pipeline,
            compute_layout,
            cull_layout,
            fade_enabled: false,
            last_draw_count: 0,
            frame_seq: 0,
            pending_free: VecDeque::new(),
        }
    }

    /// Sections drawn last time this frame slot ran (post frustum + occlusion
    /// cull). Read back from the GPU count buffer, so it lags a few frames.
    pub fn sections_drawn(&self) -> u32 {
        self.last_draw_count
    }

    /// Milliseconds since the store was made, never 0 (no fade).
    fn now_ms(&self) -> u32 {
        (self.epoch.elapsed().as_millis() as u32).max(1)
    }

    /// The staging buffer the next frame's uploads go in.
    fn upload_slot(&self) -> usize {
        (self.frame_seq as usize + 1) % STAGING_SLOTS
    }

    /// Records the pending staging copies and compaction moves into the
    /// frame, ahead of its draws.
    fn record_uploads(&mut self, encoder: &rhi::backend::CommandEncoder) {
        if self.pending_copies.is_empty() && self.pending_moves.is_empty() {
            return;
        }
        let busy_until = self.frame_seq + MAX_FRAMES_IN_FLIGHT as u64;
        record_grouped(encoder, &self.arenas, &mut self.pending_copies, |slot| {
            self.staging_used[slot] = (0, 0);
            self.staging_busy[slot] = busy_until;
            let staging = self.staging[slot].handle();
            Some((staging, staging))
        });
        self.pending_copies.clear();
        record_grouped(encoder, &self.arenas, &mut self.pending_moves, |from| {
            let src = self.arenas[from as usize].as_ref()?;
            Some((src.vertex_buffer.handle(), src.index_buffer.handle()))
        });
        self.pending_moves.clear();
        encoder.transition_buffers(rhi::BufferUse::CopyDst, rhi::BufferUse::DrawInput);
    }

    /// Submits the pending staging copies now and waits for them, when a
    /// batch outgrows its staging buffer, or a frame that never drew left it
    /// in use.
    fn flush_now(&mut self, device: &rhi::backend::Device) {
        device.submit(|encoder| {
            record_grouped(encoder, &self.arenas, &mut self.pending_copies, |slot| {
                let staging = self.staging[slot].handle();
                Some((staging, staging))
            });
        });
        self.pending_copies.clear();
        self.staging_used = [(0, 0); STAGING_SLOTS];
        self.staging_busy = [0; STAGING_SLOTS];
    }

    /// Upload a batch of mesh results, each replacing the sections in its
    /// `mesh.replaced` range. The vertices go to this frame's staging buffer
    /// and the copies into the arenas are recorded as the frame starts, so a
    /// streaming frame never waits for the GPU; only a batch outgrowing the
    /// staging buffer is flushed and waited for on the spot. Returns the
    /// starved sections that may fit now, per column, for re-meshing.
    pub fn upload_batch(
        &mut self,
        device: &rhi::backend::Device,
        meshes: &[ChunkMeshData],
    ) -> Vec<(ChunkPos, Vec<i32>)> {
        let mut needs_remesh: Vec<(ChunkPos, Vec<i32>)> = Vec::new();
        // Until something changes, the arenas are as full as before and the
        // retry would fail too.
        if std::mem::take(&mut self.retry_due) {
            self.retry_starved(device, &mut needs_remesh);
        }
        if meshes.is_empty() {
            return needs_remesh;
        }

        // Sub-allocate an exact-size vertex + index slice for each non-empty
        // section. Indices stay section-local and `vertex_offset` rebases the draw,
        // so no packing or rebasing is needed — just one slice per section.
        struct Plan<'a> {
            section_index: i32,
            verts: &'a [u8],
            vertex_count: u32,
            indices: &'a [u32],
            water_indices: &'a [u32],
            arena: u16,
            vtx_off: u32,
            idx_off: u32,
            solid_index_count: u32,
            aabb: ChunkAABB,
            origin: [i32; 3],
        }

        impl Plan<'_> {
            /// Writes the opaque then water indices at byte `off`, returning
            /// how many bytes that took.
            fn write_indices(&self, buffer: &mut rhi::backend::Buffer, off: usize) -> usize {
                let opaque: &[u8] = bytemuck::cast_slice(self.indices);
                buffer.write_at(off, opaque);
                buffer.write_at(off + opaque.len(), bytemuck::cast_slice(self.water_indices));
                opaque.len() + size_of_val(self.water_indices)
            }

            /// Opaque then water indices, which share one slice.
            fn index_len(&self) -> u32 {
                (self.indices.len() + self.water_indices.len()) as u32
            }

            fn slice(&self) -> Slice {
                Slice {
                    arena: self.arena,
                    vertex_offset: self.vtx_off,
                    #[cfg(feature = "test-harness")]
                    vertex_len: self.vertex_count,
                    first_index: self.idx_off,
                    #[cfg(feature = "test-harness")]
                    index_len: self.index_len(),
                }
            }
        }

        // Retired slices only reclaim in `begin_frame`; if rendering is paused
        // while meshing continues (e.g. minimized window) the backlog grows
        // unbounded. Past a sane bound, force a GPU wait and reclaim it all.
        const PENDING_FREE_DRAIN_THRESHOLD: usize = 8192;
        if self.pending_free.len() > PENDING_FREE_DRAIN_THRESHOLD {
            device.wait_idle();
            while let Some((_, slice)) = self.pending_free.pop_front() {
                self.free_slice(slice);
                self.retry_due = true;
            }
        }

        let staging_half = STAGING_SIZE as usize / 2;
        let vertex_size = self.format.vertex_size();
        let slot = self.upload_slot();
        // A frame that began but never drew left its staging buffer waiting
        // on copies no frame recorded; a frame may still be reading it.
        if self.staging_busy[slot] > self.frame_seq {
            device.wait_idle();
            self.flush_now(device);
        }
        let now_ms = self.now_ms();

        for mesh in meshes {
            // The covered sections this job is authoritative for: reject any where
            // a newer upload (higher epoch) already landed. See
            // `ChunkMeshData::upload_epoch`.
            let accepted: HashSet<i32> = mesh
                .replaced
                .clone()
                .filter(|si| {
                    let stored = self
                        .lists
                        .columns
                        .get(&mesh.pos)
                        .and_then(|c| c.sections.iter().find(|s| s.section_index == *si))
                        .map(|s| s.epoch)
                        .unwrap_or(0);
                    mesh.upload_epoch >= stored
                })
                .collect();

            // A mesh from before a format change is remeshed anyway.
            let upload_secs: Vec<&SectionMesh> = mesh
                .sections
                .iter()
                .filter(|s| accepted.contains(&s.section_index))
                .filter(|s| s.vertices.format() == self.format)
                .collect();

            if self.use_staging && !upload_secs.is_empty() {
                // Verts and indices share the staging buffer (two halves). A chunk
                // too large for one half is dropped rather than overflowing the
                // buffer. This is permanent, so it's not reported for retry.
                let v_bytes: usize = upload_secs.iter().map(|s| s.vertices.bytes().len()).sum();
                let i_bytes: usize = upload_secs
                    .iter()
                    .map(|s| (s.indices.len() + s.water_indices.len()) * INDEX_SIZE as usize)
                    .sum();
                if v_bytes > staging_half || i_bytes > staging_half {
                    tracing::warn!(
                        "Chunk {:?} too large for staging ({} v / {} i bytes), skipping",
                        mesh.pos,
                        v_bytes,
                        i_bytes,
                    );
                    self.retire_sections(mesh.pos, &accepted);
                    self.drop_if_empty(mesh.pos);
                    continue;
                }
            }

            // Allocate the new slices before retiring the old: no room then
            // keeps the old mesh drawn instead of dropping it.
            let mut plans: Vec<Plan> = Vec::with_capacity(upload_secs.len());
            let mut need = Need::default();
            // Past the eviction frontier a column waits for room instead.
            let beyond = self.beyond_frontier(mesh.pos);
            let mut no_room = beyond;
            for sec in &upload_secs {
                let vcount = sec.vertices.len() as u32;
                // Opaque and water indices share one slice (opaque first, water after).
                let icount = (sec.indices.len() + sec.water_indices.len()) as u32;
                if vcount == 0 || icount == 0 {
                    continue;
                }
                need.add(vcount, icount);
                if no_room {
                    continue;
                }
                let Some((arena, vtx_off, idx_off)) = self.alloc_slice(device, vcount, icount)
                else {
                    no_room = true;
                    continue;
                };
                debug_assert_eq!(
                    sec.vertices.bytes().len() as u64,
                    u64::from(vcount) * vertex_size
                );
                plans.push(Plan {
                    section_index: sec.section_index,
                    verts: sec.vertices.bytes(),
                    vertex_count: vcount,
                    indices: &sec.indices,
                    water_indices: &sec.water_indices,
                    arena,
                    vtx_off,
                    idx_off,
                    solid_index_count: sec.solid_index_count,
                    aabb: sec.aabb,
                    origin: [
                        mesh.pos.x * 16,
                        mesh.min_y + sec.section_index * 16,
                        mesh.pos.z * 16,
                    ],
                });
            }
            if no_room {
                // Never drawn, so these go straight back.
                for p in &plans {
                    self.free_slice(p.slice());
                }
                if beyond {
                    // Its old mesh would go stale; it waits with the rest.
                    self.evict(mesh.pos);
                } else {
                    tracing::debug!("No room for {:?}'s mesh, keeping its old one", mesh.pos);
                    self.evict_beyond(mesh.pos, need);
                }
                self.starve(mesh.pos, accepted, need);
                continue;
            }

            // Remember which sections were present so a re-meshed one swaps
            // instantly while a freshly revealed one still fades in.
            let was_present = self.retire_sections(mesh.pos, &accepted);
            if plans.is_empty() {
                // Every accepted section is now empty.
                self.drop_if_empty(mesh.pos);
                continue;
            }

            if self.use_staging {
                let mv: usize = plans.iter().map(|p| p.verts.len()).sum();
                let mi: usize = plans
                    .iter()
                    .map(|p| p.index_len() as usize * INDEX_SIZE as usize)
                    .sum();
                // This mesh alone fits a half (checked above), so a flush always
                // makes room.
                let (stg_v, stg_i) = self.staging_used[slot];
                if stg_v + mv > staging_half || stg_i + mi > staging_half {
                    self.flush_now(device);
                }
                let (mut stg_v, mut stg_i) = self.staging_used[slot];
                for p in &plans {
                    self.staging[slot].write_at(stg_v, p.verts);
                    let vbytes = p.verts.len();
                    let vertex = rhi::BufferCopy {
                        src_offset: stg_v as u64,
                        dst_offset: u64::from(p.vtx_off) * vertex_size,
                        size: vbytes as u64,
                    };
                    stg_v += vbytes;

                    let off = staging_half + stg_i;
                    let ibytes = p.write_indices(&mut self.staging[slot], off);
                    let index = rhi::BufferCopy {
                        src_offset: off as u64,
                        dst_offset: u64::from(p.idx_off) * INDEX_SIZE,
                        size: ibytes as u64,
                    };
                    stg_i += ibytes;
                    self.pending_copies.push((slot, (p.arena, vertex, index)));
                }
                self.staging_used[slot] = (stg_v, stg_i);
            } else {
                for p in &plans {
                    let Some(arena) = self.arenas[p.arena as usize].as_mut() else {
                        continue;
                    };
                    let base = u64::from(p.vtx_off) * vertex_size;
                    arena.vertex_buffer.write_at(base as usize, p.verts);
                    p.write_indices(
                        &mut arena.index_buffer,
                        p.idx_off as usize * INDEX_SIZE as usize,
                    );
                }
            }

            let column = self.column_slot(mesh.pos);
            for p in &plans {
                let section = SectionAlloc {
                    section_index: p.section_index,
                    aabb: p.aabb,
                    origin: p.origin,
                    arena: p.arena,
                    first_index: p.idx_off,
                    index_count: p.indices.len() as u32,
                    solid_index_count: p.solid_index_count,
                    water_index_count: p.water_indices.len() as u32,
                    idx_len: p.index_len(),
                    vertex_offset: p.vtx_off as i32,
                    vtx_len: p.vertex_count,
                    // A re-meshed section swaps instantly; a freshly revealed
                    // one fades in, once the first cull has run.
                    uploaded_ms: if was_present.contains(&p.section_index) || !self.fade_enabled {
                        0
                    } else {
                        now_ms
                    },
                    epoch: mesh.upload_epoch,
                    meta_index: 0,
                };
                self.lists.insert(column, section);
            }
        }

        self.ensure_meta_capacity(device);
        needs_remesh
    }

    /// `pos`'s column slot, made with its visibility mask if it has none.
    fn column_slot(&mut self, pos: ChunkPos) -> u32 {
        let slot = self.lists.columns.slot_or_insert(pos) as usize;
        if self.column_visibility.len() <= slot {
            self.column_visibility.resize(slot + 1, u32::MAX);
        }
        let mask = self.chunk_visibility.get(&pos).copied().unwrap_or(u32::MAX);
        if self.column_visibility[slot] != mask {
            self.column_visibility[slot] = mask;
            self.visibility_pending = MAX_FRAMES_IN_FLIGHT as u32;
        }
        slot as u32
    }

    /// Grow the meta and indirect buffers to hold every segment, and the
    /// visibility masks every column. No-op while capacity suffices.
    fn ensure_meta_capacity(&mut self, device: &rhi::backend::Device) {
        let slots = self.lists.slots() as usize;
        let columns = self.column_visibility.len();
        if slots <= self.max_meta && columns <= self.max_columns {
            return;
        }
        // The buffers are referenced by every in-flight frame's bind group;
        // wait the GPU out before freeing them.
        device.wait_idle();
        let grow_meta = slots > self.max_meta;
        if grow_meta {
            self.max_meta = (slots.saturating_mul(3) / 2)
                .next_power_of_two()
                .max(self.max_meta * 2);
            util::destroy_buffers(device, &mut self.meta_buffers);
            self.meta_buffers = meta_buffers(device, self.max_meta);
            for segment in &mut self.lists.segments {
                segment.mark(0, segment.cap);
            }
        }
        if columns > self.max_columns {
            self.max_columns = columns.next_power_of_two().max(self.max_columns * 2);
            util::destroy_buffers(device, &mut self.visibility_buffers);
            self.visibility_buffers = visibility_buffers(device, self.max_columns);
            self.visibility_pending = MAX_FRAMES_IN_FLIGHT as u32;
        }
        for list in draw_lists(&mut self.camera, &mut self.shadow) {
            if grow_meta {
                list.regrow(device, self.max_meta);
            }
            list.bind(
                device,
                self.cull_layout,
                &self.meta_buffers,
                &self.visibility_buffers,
            );
        }
    }

    /// Drops `pos`'s column once none of its sections remain.
    fn drop_if_empty(&mut self, pos: ChunkPos) {
        if self
            .lists
            .columns
            .get(&pos)
            .is_some_and(|c| c.sections.is_empty())
        {
            self.lists.columns.remove(&pos);
        }
    }

    /// Return one slice's vertex and index ranges to its arena.
    fn free_slice(&mut self, slice: Slice) {
        if let Some(arena) = self.arenas[slice.arena as usize].as_mut() {
            arena.vertices.free(slice.vertex_offset);
            arena.indices.free(slice.first_index);
        }
    }

    /// A vertex and an index slice in one arena, adding an arena when they're
    /// all full; `None` once that would pass the budget.
    fn alloc_slice(
        &mut self,
        device: &rhi::backend::Device,
        vertices: u32,
        indices: u32,
    ) -> Option<(u16, u32, u32)> {
        if let Some(found) = alloc_in(&mut self.arenas, self.draining, vertices, indices) {
            return Some(found);
        }
        if !self.can_add_arena(device) {
            return None;
        }
        self.add_arena(device);
        alloc_in(&mut self.arenas, self.draining, vertices, indices)
    }

    /// Whether one more arena fits the budget of the heaps arenas go in:
    /// what `VK_EXT_memory_budget` says this process may use, less headroom
    /// for everything else, or without it a share of them. The first always
    /// fits.
    fn can_add_arena(&self, device: &rhi::backend::Device) -> bool {
        let arenas = self.arena_count();
        if arenas == 0 {
            return true;
        }
        if arenas as usize >= self.max_arenas {
            return false;
        }
        let bytes = self.arena_size.bytes(self.format);
        let location = arena_location(self.use_staging);
        match device.memory_budget(location) {
            Some(budget) => {
                let headroom = MIN_BUDGET_HEADROOM.max(budget.budget / 8);
                budget.usage + bytes + headroom <= budget.budget
            }
            None => {
                (arenas + 1) * bytes
                    <= (device.memory_bytes(location) as f64 * BUDGET_FRACTION) as u64
            }
        }
    }

    fn add_arena(&mut self, device: &rhi::backend::Device) {
        let arena = Arena::new(device, self.use_staging, self.format, self.arena_size);
        match self.arenas.iter().position(Option::is_none) {
            Some(id) => self.arenas[id] = Some(arena),
            None => self.arenas.push(Some(arena)),
        }
        self.log_arenas("added");
    }

    /// Arenas allocated, doomed ones included.
    fn arena_count(&self) -> u64 {
        self.arenas.iter().flatten().count() as u64
    }

    fn log_arenas(&self, change: &str) {
        let arenas = self.arena_count();
        tracing::info!(
            "Chunk arena {change}: {arenas} arenas, {} MB",
            arenas * self.arena_size.bytes(self.format) / (1024 * 1024)
        );
    }

    /// Evicts the columns farthest from the eye, all farther than `pos`, until
    /// they free what `pos` needs, so a full budget draws a shorter distance
    /// instead of leaving holes nearby. The evicted columns starve until
    /// there's room for them again; when even all of them can't make room,
    /// so does `pos`.
    fn evict_beyond(&mut self, pos: ChunkPos, need: Need) {
        let Some(eye) = self.eye else {
            return;
        };
        let dist = column_dist_sq(pos, eye);
        let loaded = self.lists.columns.iter().map(|(_, column)| {
            let taken = Need {
                vertices: column.vertices,
                indices: column.indices,
                ..Need::default()
            };
            (column_dist_sq(column.pos, eye), column.pos, taken)
        });
        let (victims, freed) = eviction_victims(loaded, dist, need);
        let frontier = match victims.last() {
            Some(&(nearest, _)) if freed => nearest,
            _ => dist,
        };
        for (_, victim) in victims {
            self.evict(victim);
            tracing::debug!("Evicted {victim:?} for {pos:?}");
        }
        self.frontier_sq = self.frontier_sq.min(frontier);
    }

    /// Drops `pos`'s mesh, starving the column until there's room for it.
    fn evict(&mut self, pos: ChunkPos) {
        let Some(column) = self.take_column(pos) else {
            return;
        };
        let sections = column.sections.iter().map(|s| s.section_index);
        self.starve(pos, sections, column.need());
    }

    /// Drops `pos`'s column from the draw lists and retires its slices.
    fn take_column(&mut self, pos: ChunkPos) -> Option<Column> {
        let slot = self.lists.columns.slot(&pos)?;
        let dropped = self.lists.remove_sections(slot, |_| false);
        self.retire_slices(dropped.iter().map(SectionAlloc::slice));
        let mut column = self.lists.columns.remove(&pos)?;
        column.sections = dropped;
        Some(column)
    }

    fn starve(&mut self, pos: ChunkPos, sections: impl IntoIterator<Item = i32>, need: Need) {
        let starved = self.starved.entry(pos).or_default();
        starved.sections.extend(sections);
        starved.need = starved.need.max(need);
    }

    /// Whether `pos` is at or past the eviction frontier.
    fn beyond_frontier(&self, pos: ChunkPos) -> bool {
        self.eye
            .is_some_and(|eye| column_dist_sq(pos, eye) >= self.frontier_sq)
    }

    /// Hands the starved columns that could fit now back for re-meshing: all
    /// of them while another arena fits the budget, else those
    /// [`columns_to_retry`] picks.
    fn retry_starved(
        &mut self,
        device: &rhi::backend::Device,
        out: &mut Vec<(ChunkPos, Vec<i32>)>,
    ) {
        if self.starved.is_empty() {
            return;
        }
        if self.can_add_arena(device) {
            self.frontier_sq = f64::INFINITY;
            out.extend(
                self.starved
                    .drain()
                    .map(|(pos, starved)| (pos, starved.sections.into_iter().collect())),
            );
            return;
        }
        let arenas: Vec<&Arena> = self
            .arenas
            .iter()
            .enumerate()
            .filter(|&(id, _)| self.draining != Some(id as u16))
            .filter_map(|(_, slot)| live(slot))
            .collect();
        let free_vertices: u64 = arenas
            .iter()
            .map(|a| u64::from(a.vertices.capacity() - a.vertices.used()))
            .sum();
        let free_indices: u64 = arenas
            .iter()
            .map(|a| u64::from(a.indices.capacity() - a.indices.used()))
            .sum();
        let dist = |pos| self.eye.map_or(0.0, |eye| column_dist_sq(pos, eye));
        let farthest_loaded = self
            .lists
            .columns
            .iter()
            .map(|(_, column)| dist(column.pos))
            .fold(f64::NEG_INFINITY, f64::max);
        let waiting = self.starved.iter().map(|(&pos, s)| Waiting {
            dist: dist(pos),
            pos,
            need: s.need,
            fits_an_arena: arenas.iter().any(|a| {
                a.vertices.fits(s.need.max_vertices) && a.indices.fits(s.need.max_indices)
            }),
        });
        let (retry, frontier_sq) = columns_to_retry(
            waiting,
            (free_vertices, free_indices),
            self.frontier_sq,
            farthest_loaded,
            self.arena_size,
        );
        self.frontier_sq = frontier_sq;
        for pos in retry {
            if let Some(starved) = self.starved.remove(&pos) {
                out.push((pos, starved.sections.into_iter().collect()));
            }
        }
    }

    /// Retires `pos`'s `sections`, returning those it had; the others stay.
    fn retire_sections(&mut self, pos: ChunkPos, sections: &HashSet<i32>) -> HashSet<i32> {
        let Some(slot) = self.lists.columns.slot(&pos) else {
            return HashSet::new();
        };
        let dropped = self
            .lists
            .remove_sections(slot, |s| !sections.contains(&s.section_index));
        self.retire_slices(dropped.iter().map(SectionAlloc::slice));
        dropped.iter().map(|s| s.section_index).collect()
    }

    /// Defer returning slices to the arenas until `MAX_FRAMES_IN_FLIGHT`
    /// frames have passed, so the GPU can't still be reading them from an
    /// in-flight frame. Use for slices that were potentially drawn.
    fn retire_slices(&mut self, slices: impl IntoIterator<Item = Slice>) {
        let retire_at = self.frame_seq + MAX_FRAMES_IN_FLIGHT as u64;
        for slice in slices {
            self.pending_free.push_back((retire_at, slice));
        }
    }

    /// Advance one frame, reclaim any slices whose retirement deadline has
    /// passed, then free empty arenas and compact. Call once per rendered
    /// frame, right after the frame's fence has been waited (that wait
    /// guarantees the frame from `MAX_FRAMES_IN_FLIGHT` ago — and everything
    /// before it — is done on the GPU).
    pub fn begin_frame(&mut self, device: &rhi::backend::Device) {
        self.frame_seq += 1;
        while self
            .pending_free
            .front()
            .is_some_and(|&(retire_at, _)| retire_at <= self.frame_seq)
        {
            let (_, slice) = self.pending_free.pop_front().unwrap();
            self.free_slice(slice);
            self.retry_due = true;
        }
        if !self.starved.is_empty()
            && self.frame_seq.is_multiple_of(BUDGET_RECHECK_FRAMES)
            && self.can_add_arena(device)
        {
            self.retry_due = true;
        }
        self.free_empty_arenas(device);
        self.compact(device);
    }

    /// Destroys doomed arenas no frame reads any more, and dooms empty ones,
    /// as vanilla destroys an empty heap, keeping one for the next upload.
    fn free_empty_arenas(&mut self, device: &rhi::backend::Device) {
        let mut destroyed = false;
        for slot in self.arenas.iter_mut() {
            if slot
                .as_ref()
                .and_then(|a| a.doomed)
                .is_some_and(|at| at <= self.frame_seq)
            {
                if let Some(arena) = slot.take() {
                    arena.destroy(device);
                }
                destroyed = true;
            }
        }
        if destroyed {
            self.log_arenas("freed");
        }
        let mut live_count = self.arenas.iter().filter_map(live).count();
        for (id, slot) in self.arenas.iter_mut().enumerate() {
            let Some(arena) = slot.as_mut() else {
                continue;
            };
            if live_count > 1 && arena.doomed.is_none() && arena.is_empty() {
                // Frames from here on skip it; earlier ones are done by then.
                arena.doomed = Some(self.frame_seq + MAX_FRAMES_IN_FLIGHT as u64);
                live_count -= 1;
                if self.draining == Some(id as u16) {
                    self.draining = None;
                }
            }
        }
    }

    /// Once the arenas' contents would fit one fewer, drains the emptiest
    /// into the rest, a bounded amount per frame, so freeing it can shrink
    /// the total. The copies are recorded as the frame starts.
    ///
    /// Not while columns are starved: they want the room, and the arena it
    /// frees would only be added back for them.
    fn compact(&mut self, device: &rhi::backend::Device) {
        if !self.starved.is_empty() {
            self.draining = None;
            return;
        }
        if self.frame_seq < self.compact_after {
            return;
        }
        let from = match self.draining {
            Some(id) => id,
            None => {
                let Some(id) = arena_to_drain(self.live_use(), self.arena_size) else {
                    return;
                };
                tracing::debug!("Compacting chunk arena {id}");
                self.draining = Some(id);
                id
            }
        };

        let vertex_size = self.format.vertex_size();
        let mut retired = Vec::new();
        let mut moved = 0;
        // The arena's list is the sections in it.
        let in_from: Vec<(u32, i32)> =
            self.lists
                .segments
                .get(from as usize)
                .map_or_else(Vec::new, |segment| {
                    segment
                        .metas
                        .iter()
                        .map(|m| (m.column, m.section as i32))
                        .collect()
                });
        for (column, section_index) in in_from {
            if moved >= COMPACT_VERTICES_PER_FRAME {
                break;
            }
            let (vtx_len, idx_len) = {
                let sec = self.lists.section_at(column, section_index);
                (sec.vtx_len, sec.idx_len)
            };
            let Some((to, vertex, index)) =
                alloc_in(&mut self.arenas, Some(from), vtx_len, idx_len)
            else {
                tracing::debug!("No room to compact chunk arena {from}");
                self.draining = None;
                self.compact_after = self.frame_seq + COMPACT_BACKOFF_FRAMES;
                break;
            };
            let relaid = self.lists.move_section(column, section_index, |sec| {
                self.pending_moves.push((
                    from,
                    (
                        to,
                        rhi::BufferCopy {
                            src_offset: sec.vertex_offset as u64 * vertex_size,
                            dst_offset: u64::from(vertex) * vertex_size,
                            size: u64::from(sec.vtx_len) * vertex_size,
                        },
                        rhi::BufferCopy {
                            src_offset: u64::from(sec.first_index) * INDEX_SIZE,
                            dst_offset: u64::from(index) * INDEX_SIZE,
                            size: u64::from(sec.idx_len) * INDEX_SIZE,
                        },
                    ),
                ));
                retired.push(sec.slice());
                sec.arena = to;
                sec.first_index = index;
                sec.vertex_offset = vertex as i32;
            });
            if relaid {
                self.ensure_meta_capacity(device);
            }
            moved += vtx_len;
        }
        self.retire_slices(retired);
    }

    /// Each live arena as (id, vertices used, indices used).
    fn live_use(&self) -> impl Iterator<Item = (u16, u32, u32)> + '_ {
        self.arenas.iter().enumerate().filter_map(|(id, slot)| {
            live(slot).map(|a| (id as u16, a.vertices.used(), a.indices.used()))
        })
    }

    pub fn remove(&mut self, pos: &ChunkPos) {
        self.starved.remove(pos);
        self.take_column(*pos);
    }

    /// Drops every mesh and, for a new format, the arenas, remade at its
    /// stride on the next upload. The caller waits for in-flight frames first.
    pub fn set_format(&mut self, device: &rhi::backend::Device, format: TerrainFormat) {
        self.clear();
        if format == self.format {
            return;
        }
        self.format = format;
        self.drop_arenas(device);
    }

    /// Destroys every arena; the next upload makes them anew.
    fn drop_arenas(&mut self, device: &rhi::backend::Device) {
        for arena in self.arenas.drain(..).flatten() {
            arena.destroy(device);
        }
        self.lists.segments.clear();
    }

    /// Arenas of `vertices`, at most `max` of them, so the harness fills
    /// them at a small render distance. A new size drops every mesh as
    /// [`Self::set_format`] does; returns whether it did. The caller waits
    /// for in-flight frames first.
    #[cfg(feature = "test-harness")]
    pub fn set_arena_limits(
        &mut self,
        device: &rhi::backend::Device,
        vertices: u32,
        max: usize,
    ) -> bool {
        self.max_arenas = max.clamp(1, MAX_ARENAS);
        let size = ArenaSize::new(vertices);
        if size == self.arena_size {
            return false;
        }
        self.arena_size = size;
        self.clear();
        self.drop_arenas(device);
        true
    }

    pub fn format(&self) -> TerrainFormat {
        self.format
    }

    pub fn clear(&mut self) {
        self.lists.clear();
        self.column_visibility.clear();
        for arena in self.arenas.iter_mut().flatten() {
            arena.vertices.reset();
            arena.indices.reset();
        }
        self.pending_free.clear();
        self.pending_copies.clear();
        self.pending_moves.clear();
        self.staging_used = [(0, 0); STAGING_SLOTS];
        self.starved.clear();
        self.frontier_sq = f64::INFINITY;
        self.draining = None;
        self.fade_enabled = false;
    }

    /// A column not drawing its newest mesh, or none, for lack of room,
    /// nearer than the eviction frontier: one that should fit.
    #[cfg(feature = "test-harness")]
    pub fn starved_column(&self) -> Option<ChunkPos> {
        self.starved
            .keys()
            .find(|&&pos| !self.beyond_frontier(pos))
            .copied()
    }

    /// Compaction has an arena to drain, or a drained one to free.
    #[cfg(feature = "test-harness")]
    pub fn compacting(&self) -> bool {
        self.draining.is_some()
            || self.arenas.iter().flatten().any(|a| a.doomed.is_some())
            || arena_to_drain(self.live_use(), self.arena_size).is_some()
    }

    /// Where an arena's ranges overlap, if they do: live sections, slices
    /// awaiting reclaim and free ranges must never share a vertex or index.
    #[cfg(feature = "test-harness")]
    pub fn slice_overlap(&self) -> Option<String> {
        let mut ranges: Vec<[Vec<(u32, u32)>; 2]> = self
            .arenas
            .iter()
            .map(|slot| match slot {
                Some(a) => [
                    a.vertices.free_blocks().collect(),
                    a.indices.free_blocks().collect(),
                ],
                None => Default::default(),
            })
            .collect();
        let live = self
            .lists
            .columns
            .iter()
            .flat_map(|(_, c)| &c.sections)
            .map(SectionAlloc::slice);
        let retired = self.pending_free.iter().map(|&(_, slice)| slice);
        for slice in live.chain(retired) {
            let Some([vertex, index]) = ranges
                .get_mut(slice.arena as usize)
                .filter(|_| self.arenas[slice.arena as usize].is_some())
            else {
                return Some(format!("a slice in arena {}, which is gone", slice.arena));
            };
            vertex.push((slice.vertex_offset, slice.vertex_len));
            index.push((slice.first_index, slice.index_len));
        }
        ranges
            .into_iter()
            .enumerate()
            .flat_map(|(id, [vertex, index])| [(id, "vertex", vertex), (id, "index", index)])
            .find_map(|(id, pool, mut ranges)| {
                ranges.sort_unstable();
                ranges
                    .windows(2)
                    .find(|w| w[0].0 + w[0].1 > w[1].0)
                    .map(|w| {
                        format!(
                            "arena {id}'s {pool} ranges {:?} and {:?} overlap",
                            w[0], w[1]
                        )
                    })
            })
    }

    pub fn chunk_count(&self) -> u32 {
        self.lists.columns.len() as u32
    }

    /// Push the CPU visibility graph's per-column visible-section masks.
    /// Columns not present default to fully visible, so the cull only omits
    /// sections the graph proved occluded.
    pub fn set_chunk_visibility(&mut self, vis: HashMap<ChunkPos, u32>) {
        self.chunk_visibility = vis;
        for (slot, column) in self.lists.columns.iter() {
            self.column_visibility[slot as usize] = self
                .chunk_visibility
                .get(&column.pos)
                .copied()
                .unwrap_or(u32::MAX);
        }
        self.visibility_pending = MAX_FRAMES_IN_FLIGHT as u32;
    }

    /// The cull's view of the frame: the camera, the clock and the segments.
    fn frustum_data(
        &self,
        frustum: &[[f32; 4]; 6],
        anchor: DVec3,
        eye: DVec3,
        use_visibility: bool,
    ) -> FrustumData {
        let mut data = FrustumData {
            planes: *frustum,
            chunk_count: self.lists.slots(),
            cam_block: anchor.as_ivec3().to_array(),
            frac: (eye - anchor).as_vec3().to_array(),
            now_ms: self.now_ms(),
            use_visibility: u32::from(use_visibility),
            _pad: [0; 3],
            arena_base: [[0; 4]; MAX_ARENAS / 4],
            arena_len: [[0; 4]; MAX_ARENAS / 4],
        };
        for (id, segment) in self.lists.segments.iter().enumerate() {
            data.arena_base[id / 4][id % 4] = segment.base;
            data.arena_len[id / 4][id % 4] = segment.metas.len() as u32;
        }
        data
    }

    /// `anchor` must be the same `Camera::anchor()` this frame's
    /// `CameraUniform` was built with, so the cull's block/fraction split
    /// matches the vertex shader's.
    pub fn dispatch_cull(
        &mut self,
        encoder: &rhi::backend::CommandEncoder,
        frame: usize,
        frustum: &[[f32; 4]; 6],
        anchor: DVec3,
        eye: DVec3,
    ) {
        let column = |eye| ChunkPos::containing(BlockPos::containing(eye));
        if !self.starved.is_empty() && self.eye.is_some_and(|last| column(last) != column(eye)) {
            self.retry_due = true;
        }
        self.eye = Some(eye);
        self.record_uploads(encoder);
        if self.lists.columns.is_empty() {
            return;
        }

        // Front-to-back order is an early-Z optimization, so it follows the
        // camera a list per frame, once it has moved far enough.
        self.lists.sort_step(eye);
        let meta = &mut self.meta_buffers[frame];
        self.lists.write_pending(|offset, metas| {
            meta.write_at(offset, bytemuck::cast_slice(metas));
        });
        if self.visibility_pending > 0 {
            self.visibility_buffers[frame].write(bytemuck::cast_slice(&self.column_visibility));
            self.visibility_pending -= 1;
        }

        // This frame slot's GPU work has completed (fence-waited at frame start),
        // so its readback holds the previous cull's totals (solid + cutout draws).
        self.last_draw_count = self.count_readbacks[frame].read(|bytes| {
            let (counts, _) = bytes.as_chunks::<4>();
            counts.iter().map(|&count| u32::from_ne_bytes(count)).sum()
        });
        let data = self.frustum_data(frustum, anchor, eye, true);
        self.camera.cull(
            encoder,
            &self.compute_pipeline,
            frame,
            &data,
            !self.indirect_draw.count,
        );
        encoder.transition_buffers(rhi::BufferUse::StorageWrite, rhi::BufferUse::CopySrc);
        let readback = self.count_readbacks[frame].handle();
        for (i, count) in [
            &self.camera.counts[Layer::Solid as usize][frame],
            &self.camera.counts[Layer::Cutout as usize][frame],
        ]
        .into_iter()
        .enumerate()
        {
            encoder.copy_buffer(
                count.handle(),
                readback,
                &[rhi::BufferCopy {
                    src_offset: 0,
                    dst_offset: i as u64 * COUNTS_SIZE,
                    size: COUNTS_SIZE,
                }],
            );
        }
        encoder.transition_buffers(rhi::BufferUse::CopyDst, rhi::BufferUse::HostRead);
        self.fade_enabled = true;
    }

    /// Culls every section, the camera's occluded ones too, against a shader
    /// pack's shadow view `frustum`, after this frame's
    /// [`Self::dispatch_cull`].
    pub fn dispatch_shadow_cull(
        &mut self,
        device: &rhi::backend::Device,
        encoder: &rhi::backend::CommandEncoder,
        frame: usize,
        frustum: &[[f32; 4]; 6],
        anchor: DVec3,
        eye: DVec3,
    ) {
        if self.lists.columns.is_empty() {
            return;
        }
        let data = self.frustum_data(frustum, anchor, eye, false);
        let shadow = self.shadow.get_or_insert_with(|| {
            DrawList::new(
                device,
                self.cull_layout,
                &self.meta_buffers,
                &self.visibility_buffers,
                self.max_meta,
            )
        });
        shadow.cull(
            encoder,
            &self.compute_pipeline,
            frame,
            &data,
            !self.indirect_draw.count,
        );
    }

    /// Issue one render `layer`'s indirect draws for `view`, one call per
    /// arena over its segment of the draw lists; the caller binds the
    /// matching pipeline first. The layers share each arena's vertex/index
    /// buffers, the meta buffer and the cull-written draw lists.
    pub fn draw_indirect(
        &self,
        pass: &rhi::backend::RenderPassEncoder,
        frame: usize,
        layer: Layer,
        view: View,
    ) {
        if self.lists.columns.is_empty() {
            return;
        }
        let list = match (view, &self.shadow) {
            (View::Camera, _) => &self.camera,
            (View::Shadow, Some(shadow)) => shadow,
            (View::Shadow, None) => return,
        };
        let indirect = list.draws[layer as usize][frame].handle();
        let count = list.counts[layer as usize][frame].handle();
        // Binding 1: the meta buffer, read per instance for the section
        // origin + fade (indexed by `first_instance`).
        pass.set_vertex_buffer(1, self.meta_buffers[frame].handle());
        let stride = size_of::<DrawCommand>() as u32;
        for (id, (slot, segment)) in self.arenas.iter().zip(&self.lists.segments).enumerate() {
            let Some(arena) = live(slot).filter(|_| !segment.metas.is_empty()) else {
                continue;
            };
            // Binding 0: the arena's packed vertices.
            pass.set_vertex_buffer(0, arena.vertex_buffer.handle());
            pass.set_index_buffer(arena.index_buffer.handle());
            let max_draws = segment.metas.len() as u32;
            let offset = u64::from(segment.base * stride);
            if self.indirect_draw.count {
                let count_offset = (id * size_of::<u32>()) as u64;
                pass.draw_indexed_indirect_count(
                    indirect,
                    offset,
                    count,
                    count_offset,
                    max_draws,
                    stride,
                );
            } else if self.indirect_draw.multi_draw {
                pass.draw_indexed_indirect(indirect, offset, max_draws, stride);
            } else {
                for i in 0..max_draws {
                    pass.draw_indexed_indirect(indirect, offset + u64::from(i * stride), 1, stride);
                }
            }
        }
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        self.drop_arenas(device);
        util::destroy_buffers(device, &mut self.staging);
        util::destroy_buffers(device, &mut self.meta_buffers);
        util::destroy_buffers(device, &mut self.visibility_buffers);
        for list in draw_lists(&mut self.camera, &mut self.shadow) {
            list.destroy(device);
        }
        util::destroy_buffers(device, &mut self.count_readbacks);

        self.compute_pipeline.destroy(device);
        self.compute_layout.destroy(device);
        self.cull_layout.destroy(device);
    }
}

/// Per frame slot, a meta buffer of `draws` slots.
fn meta_buffers(device: &rhi::backend::Device, draws: usize) -> Vec<rhi::backend::Buffer> {
    util::create_per_frame_host_buffers(
        device,
        (draws * size_of::<ChunkMeta>()) as u64,
        rhi::BufferUsages::STORAGE | rhi::BufferUsages::VERTEX,
        "chunk_meta",
    )
}

/// Per frame slot, a visibility mask buffer of `columns` slots.
fn visibility_buffers(device: &rhi::backend::Device, columns: usize) -> Vec<rhi::backend::Buffer> {
    util::create_per_frame_host_buffers(
        device,
        (columns * size_of::<u32>()) as u64,
        rhi::BufferUsages::STORAGE,
        "column_visibility",
    )
}

/// Of the `loaded` columns, as (distance from the eye squared, position,
/// what they take), those to evict for `need`: the farthest first, all
/// farther than `limit`, until they free it, nearest last; and whether they
/// do.
fn eviction_victims(
    loaded: impl Iterator<Item = (f64, ChunkPos, Need)>,
    limit: f64,
    need: Need,
) -> (Vec<(f64, ChunkPos)>, bool) {
    let mut far: Vec<_> = loaded.filter(|&(dist, ..)| dist > limit).collect();
    far.sort_unstable_by(|a, b| b.0.total_cmp(&a.0));
    let mut freed = Need::default();
    let mut victims = Vec::new();
    for (dist, pos, taken) in far {
        if need.fits_in(freed.vertices, freed.indices) {
            break;
        }
        freed.vertices += taken.vertices;
        freed.indices += taken.indices;
        victims.push((dist, pos));
    }
    (victims, need.fits_in(freed.vertices, freed.indices))
}

/// A starved column, for [`columns_to_retry`].
struct Waiting {
    /// From the eye, squared.
    dist: f64,
    pos: ChunkPos,
    need: Need,
    /// Its largest section fits some arena.
    fits_an_arena: bool,
}

/// The starved columns to hand back, nearest first, and the eviction
/// frontier after. Inside the frontier a column comes back when it fits the
/// free vertices and indices, or when a loaded column farther out (the
/// farthest is at `farthest_loaded`) can be evicted for it; either way the
/// free room is its, so no farther column takes it first. Past the frontier,
/// only while [`FRONTIER_SLACK`] of an arena stays free after, moving the
/// frontier out past it, so the frontier doesn't swing back and forth as
/// columns come and go.
fn columns_to_retry(
    waiting: impl Iterator<Item = Waiting>,
    (mut free_vertices, mut free_indices): (u64, u64),
    mut frontier_sq: f64,
    farthest_loaded: f64,
    arena: ArenaSize,
) -> (Vec<ChunkPos>, f64) {
    let mut waiting: Vec<Waiting> = waiting.collect();
    waiting.sort_unstable_by(|a, b| a.dist.total_cmp(&b.dist));
    let mut retry = Vec::new();
    for column in waiting {
        let past = column.dist >= frontier_sq;
        let (keep_vertices, keep_indices) = if past {
            (
                u64::from(arena.vertices) / FRONTIER_SLACK,
                u64::from(arena.indices) / FRONTIER_SLACK,
            )
        } else {
            (0, 0)
        };
        let fits = column.fits_an_arena
            && column.need.fits_in(
                free_vertices.saturating_sub(keep_vertices),
                free_indices.saturating_sub(keep_indices),
            );
        if !fits {
            if past {
                break;
            }
            if column.dist >= farthest_loaded {
                continue;
            }
        }
        free_vertices = free_vertices.saturating_sub(column.need.vertices);
        free_indices = free_indices.saturating_sub(column.need.indices);
        if past {
            frontier_sq = column.dist.next_up();
        }
        retry.push(column.pos);
    }
    (retry, frontier_sq)
}

/// The arena compaction should drain, of the `live` ones as (id, vertices
/// used, indices used): the emptiest, once the others could hold everything
/// filled to [`COMPACT_FILL`].
fn arena_to_drain(live: impl Iterator<Item = (u16, u32, u32)>, arena: ArenaSize) -> Option<u16> {
    let live: Vec<_> = live.collect();
    if live.len() < 2 {
        return None;
    }
    let used_vertices: u64 = live.iter().map(|&(_, v, _)| u64::from(v)).sum();
    let used_indices: u64 = live.iter().map(|&(.., i)| u64::from(i)).sum();
    let room = (live.len() - 1) as f64 * COMPACT_FILL;
    if used_vertices as f64 > room * f64::from(arena.vertices)
        || used_indices as f64 > room * f64::from(arena.indices)
    {
        return None;
    }
    live.iter().min_by_key(|&&(_, v, _)| v).map(|&(id, ..)| id)
}

/// Where arenas live: device-local behind the staging buffer, or
/// host-visible.
fn arena_location(use_staging: bool) -> rhi::MemoryLocation {
    if use_staging {
        rhi::MemoryLocation::Device
    } else {
        rhi::MemoryLocation::Host
    }
}

/// An arena heap, in [`arena_location`]; a copy source and target, for
/// compaction.
fn create_pool(
    device: &rhi::backend::Device,
    use_staging: bool,
    label: &str,
    size: u64,
    usage: rhi::BufferUsages,
) -> rhi::backend::Buffer {
    let usage = usage | rhi::BufferUsages::COPY_SRC | rhi::BufferUsages::COPY_DST;
    let location = arena_location(use_staging);
    if use_staging {
        rhi::backend::Buffer::dedicated(device, label, size, usage, location)
    } else {
        rhi::backend::Buffer::new(device, label, size, usage, location)
    }
}

/// Which view's draw lists [`ChunkBufferStore::draw_indirect`] draws.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    Camera,
    /// A shader pack's shadow pass.
    Shadow,
}

/// One view's cull: per frame slot, its frustum and the draw lists it
/// writes over the shared metas, a range and a draw count per arena and
/// [`Layer`].
struct DrawList {
    cull_sets: Vec<rhi::backend::BindGroup>,
    frustum_buffers: Vec<rhi::backend::Buffer>,
    /// By [`Layer`], then frame slot.
    draws: [Vec<rhi::backend::Buffer>; 3],
    counts: [Vec<rhi::backend::Buffer>; 3],
}

impl DrawList {
    fn new(
        device: &rhi::backend::Device,
        layout: rhi::backend::BindGroupLayout,
        meta: &[rhi::backend::Buffer],
        visibility: &[rhi::backend::Buffer],
        draws: usize,
    ) -> Self {
        let count = |name: &str| {
            util::create_per_frame_host_buffers(
                device,
                COUNTS_SIZE,
                rhi::BufferUsages::STORAGE
                    | rhi::BufferUsages::INDIRECT
                    | rhi::BufferUsages::COPY_SRC,
                name,
            )
        };
        let mut this = Self {
            cull_sets: vec![rhi::backend::BindGroup::default(); MAX_FRAMES_IN_FLIGHT],
            frustum_buffers: util::create_per_frame_host_buffers(
                device,
                FRUSTUM_SIZE,
                rhi::BufferUsages::UNIFORM,
                "frustum_ubo",
            ),
            draws: Default::default(),
            counts: [
                count("draw_count"),
                count("draw_count_cutout"),
                count("draw_count_water"),
            ],
        };
        this.regrow(device, draws);
        this.bind(device, layout, meta, visibility);
        this
    }

    /// Remakes the per-frame indirect buffers to hold `draws`; the caller
    /// binds after.
    fn regrow(&mut self, device: &rhi::backend::Device, draws: usize) {
        let size = (draws * size_of::<DrawCommand>()) as u64;
        for (buffers, name) in self.draws.iter_mut().zip([
            "indirect_cmds",
            "indirect_cmds_cutout",
            "indirect_cmds_water",
        ]) {
            util::destroy_buffers(device, buffers);
            *buffers = util::create_per_frame_host_buffers(
                device,
                size,
                rhi::BufferUsages::STORAGE | rhi::BufferUsages::INDIRECT,
                name,
            );
        }
    }

    /// Points each frame's cull group at its buffers.
    fn bind(
        &mut self,
        device: &rhi::backend::Device,
        layout: rhi::backend::BindGroupLayout,
        meta: &[rhi::backend::Buffer],
        visibility: &[rhi::backend::Buffer],
    ) {
        let storage = rhi::backend::BindingResource::storage;
        let uniform = rhi::backend::BindingResource::uniform;
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            let draws = |layer: Layer| {
                let buffer = &self.draws[layer as usize][i];
                storage(buffer, buffer.size())
            };
            let counts = |layer: Layer| storage(&self.counts[layer as usize][i], COUNTS_SIZE);
            let bindings = [
                (0, storage(&meta[i], meta[i].size())),
                (1, uniform(&self.frustum_buffers[i], FRUSTUM_SIZE)),
                (2, draws(Layer::Solid)),
                (3, counts(Layer::Solid)),
                (4, draws(Layer::Cutout)),
                (5, counts(Layer::Cutout)),
                (6, draws(Layer::Water)),
                (7, counts(Layer::Water)),
                (8, storage(&visibility[i], visibility[i].size())),
            ];
            self.cull_sets[i].replace(device, layout, &bindings);
        }
    }

    /// Records the cull of every segment against `data`'s frustum into this
    /// frame slot's draw lists.
    /// `zero_indirect`: without drawIndirectCount the whole draw list is
    /// drawn, so slots the cull leaves unfilled must read as no-op draws.
    fn cull(
        &mut self,
        encoder: &rhi::backend::CommandEncoder,
        pipeline: &rhi::backend::ComputePipeline,
        frame: usize,
        data: &FrustumData,
        zero_indirect: bool,
    ) {
        self.frustum_buffers[frame].write(bytemuck::bytes_of(data));
        for counts in &mut self.counts {
            counts[frame].zero();
        }
        if zero_indirect {
            for draws in &mut self.draws {
                draws[frame].zero();
            }
        }
        encoder.dispatch(
            pipeline,
            self.cull_sets[frame],
            [data.chunk_count.div_ceil(64), 1, 1],
        );
        encoder.transition_buffers(rhi::BufferUse::StorageWrite, rhi::BufferUse::DrawInput);
    }

    fn destroy(&mut self, device: &rhi::backend::Device) {
        util::destroy_buffers(device, &mut self.frustum_buffers);
        for buffers in self.draws.iter_mut().chain(&mut self.counts) {
            util::destroy_buffers(device, buffers);
        }
        util::destroy_bind_groups(device, self.cull_sets.drain(..));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every section's `meta_index` finds its own meta.
    fn indices_hold(lists: &Lists) {
        for (slot, column) in lists.columns.iter() {
            for section in &column.sections {
                let meta =
                    lists.segments[section.arena as usize].metas[section.meta_index as usize];
                assert_eq!(
                    (meta.column, meta.section),
                    (slot, section.section_index as u32)
                );
            }
        }
    }

    #[test]
    fn lists_keep_their_indices_through_removal_and_sorting() {
        let mut lists = Lists::default();
        let near = ChunkPos::new(0, 0);
        let far = ChunkPos::new(4, 0);
        lists.list(far, -64, [4, 5, 6], 0);
        lists.list(near, -64, [4, 5], 0);
        indices_hold(&lists);
        assert_eq!(lists.segments[0].metas.len(), 5);

        // The last meta moves into the gap.
        let dropped = lists.remove_sections(0, |s| s.section_index != 5);
        assert_eq!(dropped.len(), 1);
        assert_eq!(lists.segments[0].metas.len(), 4);
        indices_hold(&lists);

        // One step sorts the list front to back and keeps the indices.
        lists.sort_step(DVec3::new(8.0, 8.0, 8.0));
        indices_hold(&lists);
        let origins: Vec<i32> = lists.segments[0]
            .metas
            .iter()
            .map(|m| m.origin[0])
            .collect();
        assert_eq!(origins, [0, 0, 64, 64]);
        assert_eq!(lists.sort_at, Some(1));
        lists.sort_step(DVec3::new(8.0, 8.0, 8.0));
        assert_eq!(lists.sort_at, None);
    }

    #[test]
    fn segments_lie_back_to_back_with_headroom() {
        let mut lists = Lists::default();
        let pos = ChunkPos::new(0, 0);
        lists.list(pos, -64, [0, 1], 0);
        lists.list(pos, -64, [2], 2);
        let layout: Vec<(u32, u32, usize)> = lists
            .segments
            .iter()
            .map(|s| (s.base, s.cap, s.metas.len()))
            .collect();
        assert_eq!(layout, [(0, 64, 2), (64, 64, 0), (128, 64, 1)]);
        assert_eq!(lists.slots(), 192);

        // Filling a segment lays them out anew, doubling it.
        lists.list(pos, -64, 3..66, 0);
        assert_eq!(lists.segments[0].cap, 128);
        assert_eq!(lists.segments[2].base, 192);
        indices_hold(&lists);

        // A frame's pending write covers the slots that changed, cleared
        // slots included, for each frame slot in flight.
        let mut writes = Vec::new();
        lists.write_pending(|offset, metas| writes.push((offset, metas.len())));
        assert_eq!(writes, [(0, 128), (128 * 80, 64), (192 * 80, 64)]);
        for _ in 1..MAX_FRAMES_IN_FLIGHT {
            lists.write_pending(|_, _| {});
        }
        lists.remove_sections(0, |s| s.section_index != 2);
        writes.clear();
        lists.write_pending(|offset, metas| writes.push((offset, metas.len())));
        assert_eq!(writes, [(192 * 80, 1)]);
        lists.write_pending(|offset, metas| writes.push((offset, metas.len())));
        assert_eq!(writes, [(192 * 80, 1), (192 * 80, 1)]);
    }

    fn need(vertices: u64) -> Need {
        let indices = vertices / 2 * 3;
        Need {
            vertices,
            indices,
            max_vertices: vertices as u32,
            max_indices: indices as u32,
        }
    }

    #[test]
    fn eviction_takes_the_farthest_until_the_need_is_met() {
        let loaded = [
            (1.0, ChunkPos::new(1, 0), need(100)),
            (9.0, ChunkPos::new(3, 0), need(100)),
            (4.0, ChunkPos::new(2, 0), need(100)),
            (16.0, ChunkPos::new(4, 0), need(100)),
        ];
        let far = vec![(16.0, ChunkPos::new(4, 0)), (9.0, ChunkPos::new(3, 0))];
        assert_eq!(
            eviction_victims(loaded.into_iter(), 2.0, need(150)),
            (far.clone(), true)
        );
        // Never one nearer than the column it's for, even when the farther
        // ones can't free enough.
        assert_eq!(
            eviction_victims(loaded.into_iter(), 8.0, need(1000)),
            (far, false)
        );
        assert_eq!(
            eviction_victims(loaded.into_iter(), 0.0, Need::default()),
            (vec![], true)
        );
    }

    fn waiting(dist: f64, x: i32, vertices: u64) -> Waiting {
        Waiting {
            dist,
            pos: ChunkPos::new(x, 0),
            need: need(vertices),
            fits_an_arena: true,
        }
    }

    #[test]
    fn starved_columns_inside_the_frontier_come_back_for_eviction() {
        let free = (100, 150);
        // The near one fits; the next doesn't, but a farther loaded column
        // can be evicted for it.
        let (retry, frontier) = columns_to_retry(
            [waiting(1.0, 1, 50), waiting(4.0, 2, 500)].into_iter(),
            free,
            100.0,
            9.0,
            ARENA_SIZE,
        );
        assert_eq!(retry, [ChunkPos::new(1, 0), ChunkPos::new(2, 0)]);
        assert_eq!(frontier, 100.0);
        // One coming back for eviction keeps the free room from a farther
        // one, past the frontier, that would fit it otherwise.
        let slack = u64::from(ARENA_SIZE.vertices) / FRONTIER_SLACK;
        let (retry, frontier) = columns_to_retry(
            [waiting(4.0, 2, slack + 11), waiting(200.0, 9, 1)].into_iter(),
            (slack + 10, u64::MAX),
            100.0,
            300.0,
            ARENA_SIZE,
        );
        assert_eq!(retry, [ChunkPos::new(2, 0)]);
        assert_eq!(frontier, 100.0);
        // Nothing loaded farther out, so it waits.
        let (retry, _) = columns_to_retry(
            [waiting(4.0, 2, 500)].into_iter(),
            free,
            100.0,
            1.0,
            ARENA_SIZE,
        );
        assert!(retry.is_empty());
    }

    #[test]
    fn starved_columns_past_the_frontier_need_slack() {
        let slack = u64::from(ARENA_SIZE.vertices) / FRONTIER_SLACK;
        let free = (slack + 100, u64::MAX);
        let (retry, frontier) = columns_to_retry(
            [
                waiting(25.0, 5, 100),
                waiting(36.0, 6, 100),
                waiting(49.0, 7, 1),
            ]
            .into_iter(),
            free,
            16.0,
            f64::INFINITY,
            ARENA_SIZE,
        );
        // The first fits with the slack left and moves the frontier past it;
        // the second doesn't, and nothing farther comes back.
        assert_eq!(retry, [ChunkPos::new(5, 0)]);
        assert_eq!(frontier, 25.0f64.next_up());
    }

    #[test]
    fn compaction_drains_the_emptiest_once_the_rest_hold_it() {
        let quarter = ARENA_SIZE.vertices / 4;
        let drain = |live: &[(u16, u32, u32)]| arena_to_drain(live.iter().copied(), ARENA_SIZE);
        assert_eq!(drain(&[(0, quarter, 0)]), None);
        assert_eq!(drain(&[(0, quarter * 2, 0), (3, quarter / 2, 0)]), Some(3));
        assert_eq!(drain(&[(0, quarter * 3, 0), (3, quarter, 0)]), None);
    }
}
