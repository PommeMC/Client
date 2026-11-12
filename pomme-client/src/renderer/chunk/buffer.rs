use std::collections::{HashMap, HashSet, VecDeque};
use std::mem::offset_of;
use std::sync::Arc;

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
/// Frames before compaction, or adding an arena, tries again after it
/// failed or finished.
const BACKOFF_FRAMES: u64 = 600;
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
/// A draw count per arena, then an index count per arena.
const COUNTS_SIZE: u64 = (2 * MAX_ARENAS * size_of::<u32>()) as u64;
const FRUSTUM_SIZE: u64 = size_of::<FrustumData>() as u64;
/// Column slots the visibility masks start with room for.
const INITIAL_COLUMNS: usize = 1 << 14;
/// The least meta slots an arena's segment holds; segments grow by doubling
/// so a section landing in one rarely moves the rest.
const MIN_SEGMENT: u32 = 64;
/// Staging buffers in flight: one per frame that may still read one, and one
/// written while they do.
const STAGING_SLOTS: usize = MAX_FRAMES_IN_FLIGHT + 1;

/// Vanilla `LevelRenderer.compileSections`' `isNearby` distance, squared.
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

/// Bit for one section index, 0 outside a column's 32 addressable sections (a
/// camera outside build height resolves to such an index).
pub(crate) fn section_bit(si: i32) -> u32 {
    if (0..32).contains(&si) { 1u32 << si } else { 0 }
}

/// A section's `uploaded_ms` as it uploads at `now_ms`: 0 (no fade) when
/// `shown`, else from its first upload without geometry, if it had one.
fn fade_from(shown: bool, bare_ms: Option<u32>, now_ms: u32) -> u32 {
    if shown { 0 } else { bare_ms.unwrap_or(now_ms) }
}

/// Whether a section uploaded at `uploaded_ms` has faded in to vanilla's 0.3
/// (`LevelExtractor.CHUNK_VISIBILITY_THRESHOLD`) by `now_ms`, over `fade_ms`.
fn block_entities_faded_in(uploaded_ms: u32, now_ms: u32, fade_ms: u32) -> bool {
    uploaded_ms == 0 || now_ms.saturating_sub(uploaded_ms) * 10 >= fade_ms * 3
}

/// The column `eye` is in.
fn eye_column(eye: DVec3) -> ChunkPos {
    ChunkPos::containing(BlockPos::containing(eye))
}

/// How far the farthest of `columns` is from `eye`, squared.
fn farthest_from(columns: impl Iterator<Item = ChunkPos>, eye: Option<DVec3>) -> f64 {
    columns
        .map(|pos| eye_dist_sq(pos, eye))
        .fold(f64::NEG_INFINITY, f64::max)
}

/// [`column_dist_sq`], or 0 before there's an eye.
fn eye_dist_sq(pos: ChunkPos, eye: Option<DVec3>) -> f64 {
    eye.map_or(0.0, |eye| column_dist_sq(pos, eye))
}

/// The squared X/Z distance from `eye` to a column's center.
fn column_dist_sq(pos: ChunkPos, eye: DVec3) -> f64 {
    let dx = pos.x as f64 * 16.0 + 8.0 - eye.x;
    let dz = pos.z as f64 * 16.0 + 8.0 - eye.z;
    dx * dx + dz * dz
}

/// A section's entry in the draw lists (`cull.comp`'s `ChunkMeta`): what the
/// cull needs to test and draw it. The cull writes `visibility` each frame;
/// compaction rewrites where the geometry sits.
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
    /// An instance entry for a draw outside the chunk arenas: just the
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

/// `VkDrawIndexedIndirectCommand`.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct DrawCommand {
    pub index_count: u32,
    pub instance_count: u32,
    pub first_index: u32,
    pub vertex_offset: i32,
    pub first_instance: u32,
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
    /// How long a section fades in.
    fade_ms: u32,
    _pad: [u32; 2],
    /// Per arena, where its segment starts and how many sections it holds,
    /// four to a row as std140 lays `uint` arrays out.
    arena_base: [[u32; 4]; MAX_ARENAS / 4],
    arena_len: [[u32; 4]; MAX_ARENAS / 4],
}

// The sizes `cull.comp` lays these out at.
const _: () = assert!(size_of::<ChunkMeta>() == 80);
const _: () = assert!(size_of::<FrustumData>() == 2192);

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
    /// What the column takes, for eviction to weigh; the largest single
    /// slice doesn't matter there, so it's left at 0.
    fn need(&self) -> Need {
        Need {
            vertices: self.vertices,
            indices: self.indices,
            ..Need::default()
        }
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
    /// Eye position at the last front-to-back sort, if one ran.
    last_sort_cam: Option<DVec3>,
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
    /// enough since the last, then sorts one list per call until done; one
    /// under way finishes first, so a camera that keeps moving still reaches
    /// every list.
    pub(crate) fn sort_step(&mut self, eye: DVec3) {
        let moved = self
            .last_sort_cam
            .is_none_or(|last| (eye - last).length_squared() > SORT_RECAM_SQ);
        if self.sort_at.is_none() && moved {
            self.last_sort_cam = Some(eye);
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
        let eye = self.last_sort_cam.unwrap_or(eye);
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
/// slice length, for what the column needs and for compaction.
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
    /// Opaque index count; water's follow.
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
    /// The section with its vertex and index slice lengths.
    fn lens(&self) -> (i32, SliceLens) {
        (self.section_index, (self.vtx_len, self.idx_len))
    }

    fn slice(&self) -> Slice {
        Slice::new(
            self.arena,
            (self.vertex_offset as u32, self.vtx_len),
            (self.first_index, self.idx_len),
        )
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
        self.last_sort_cam = Some(eye);
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

impl Slice {
    /// A slice of `arena`, its vertices and indices each as (offset, length).
    #[cfg_attr(not(feature = "test-harness"), allow(unused_variables))]
    fn new(
        arena: u16,
        (vertex_offset, vertex_len): (u32, u32),
        (first_index, index_len): (u32, u32),
    ) -> Self {
        Self {
            arena,
            vertex_offset,
            #[cfg(feature = "test-harness")]
            vertex_len,
            first_index,
            #[cfg(feature = "test-harness")]
            index_len,
        }
    }
}

/// A section's vertex and index slice lengths.
type SliceLens = (u32, u32);

/// A copy into arena `.0`, of vertices then indices.
type ArenaCopy = (u16, rhi::BufferCopy, rhi::BufferCopy);

/// Sections an upload batch left without a mesh, per column.
pub struct ChunkUploads {
    /// Starved sections that may fit now, for re-meshing.
    pub remesh: Vec<(ChunkPos, Vec<i32>)>,
    /// Sections that found no room, or were evicted for nearer ones.
    pub starved: Vec<(ChunkPos, Vec<i32>)>,
    /// How many meshes were taken, from the front; the rest didn't fit the
    /// frame's staging room.
    pub taken: usize,
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
}

impl FromIterator<SliceLens> for Need {
    fn from_iter<T: IntoIterator<Item = SliceLens>>(slices: T) -> Self {
        let mut need = Self::default();
        for (vertices, indices) in slices {
            need.add(vertices, indices);
        }
        need
    }
}

/// A column's sections that found no room, each with what its newest mesh
/// takes.
#[derive(Default)]
struct Starved {
    sections: HashMap<i32, SliceLens>,
}

impl Starved {
    fn need(&self) -> Need {
        self.sections.values().copied().collect()
    }
}

/// The columns whose sections found no room, or were evicted for nearer
/// ones, and the eviction frontier they wait behind.
struct Waitlist {
    columns: HashMap<ChunkPos, Starved>,
    /// The sections of columns handed back to evict farther ones for, until
    /// a mesh of each reaches [`ChunkBufferStore::upload_batch`]. Until then
    /// the room is theirs: nothing past the frontier comes back to take it,
    /// or the two evict each other for good when the room is free in total
    /// but split into gaps too small for the nearer one's sections. Per
    /// section, as a mesh already on its way may cover only some of them.
    claims: HashMap<ChunkPos, HashSet<i32>>,
    /// Columns this far from the eye, squared, or farther were evicted for
    /// nearer ones, or are past those that were, and wait for room. It moves
    /// back out as room frees, and is gone once nothing waits, so a column
    /// arriving after an unload or a teleport tries for room first.
    frontier_sq: f64,
    /// Waiting columns with room in total but no gaps that hold their
    /// sections, and nothing farther to evict: only compaction, packing an
    /// arena's sections into the others' gaps, makes them room. A drain
    /// spends them: one still short is flagged again when it next fails.
    fragmented: HashSet<ChunkPos>,
}

impl Default for Waitlist {
    fn default() -> Self {
        Self {
            columns: HashMap::new(),
            claims: HashMap::new(),
            frontier_sq: f64::INFINITY,
            fragmented: HashSet::new(),
        }
    }
}

impl Waitlist {
    /// Nothing waits for room or holds a claim to it.
    fn is_empty(&self) -> bool {
        self.columns.is_empty() && self.claims.is_empty()
    }

    fn is_fragmented(&self) -> bool {
        !self.fragmented.is_empty()
    }

    /// Lists `pos`'s `sections`, each with what its newest mesh takes.
    fn starve(&mut self, pos: ChunkPos, sections: impl IntoIterator<Item = (i32, SliceLens)>) {
        self.columns
            .entry(pos)
            .or_default()
            .sections
            .extend(sections);
    }

    /// Lists `pos`'s evicted `sections` with what their drawn meshes take,
    /// keeping a newer mesh's size for one already waiting.
    fn evicted(&mut self, pos: ChunkPos, sections: impl IntoIterator<Item = (i32, SliceLens)>) {
        let waiting = &mut self.columns.entry(pos).or_default().sections;
        for (section, lens) in sections {
            waiting.entry(section).or_insert(lens);
        }
    }

    /// Hands `pos`'s sections back for re-meshing, holding a claim on the
    /// room until its mesh is uploaded when `claim`.
    fn hand_back(&mut self, pos: ChunkPos, claim: bool) -> Option<Vec<i32>> {
        let starved = self.columns.remove(&pos)?;
        self.fragmented.remove(&pos);
        if claim {
            self.claims
                .entry(pos)
                .or_default()
                .extend(starved.sections.keys());
        }
        self.drop_frontier_if_empty();
        Some(starved.sections.into_keys().collect())
    }

    /// A mesh of `pos`'s `sections` reached upload: their claims are
    /// settled. Returns whether that was the last claim while columns still
    /// wait, which the claims held back from the room.
    fn settle(&mut self, pos: ChunkPos, sections: &HashSet<i32>) -> bool {
        let Some(claim) = self.claims.get_mut(&pos) else {
            return false;
        };
        claim.retain(|s| !sections.contains(s));
        if !claim.is_empty() {
            return false;
        }
        self.claims.remove(&pos);
        self.drop_frontier_if_empty();
        self.claims.is_empty() && !self.columns.is_empty()
    }

    /// `pos`'s `sections` were placed, or dropped for good: they wait no
    /// longer.
    fn stop_waiting(&mut self, pos: ChunkPos, sections: &HashSet<i32>) {
        let Some(starved) = self.columns.get_mut(&pos) else {
            return;
        };
        starved.sections.retain(|s, _| !sections.contains(s));
        if starved.sections.is_empty() {
            self.unlist(pos);
        }
    }

    /// Forgets `pos`, which unloaded.
    fn forget(&mut self, pos: ChunkPos) {
        self.claims.remove(&pos);
        self.unlist(pos);
    }

    fn unlist(&mut self, pos: ChunkPos) {
        self.columns.remove(&pos);
        self.fragmented.remove(&pos);
        self.drop_frontier_if_empty();
    }

    fn drop_frontier_if_empty(&mut self) {
        if self.is_empty() {
            self.frontier_sq = f64::INFINITY;
        }
    }

    /// Pulls the frontier in for `pos`, at `dist` and starved, as
    /// [`eviction_victims`] gave `victims` and whether they and the free
    /// room hold it (`freed`): to the nearest victim; or with none but the
    /// room there and split, just past `pos`, flagged for compaction to make
    /// it room when that `can_defragment`; else to `pos`.
    fn evicting_for(
        &mut self,
        pos: ChunkPos,
        dist: f64,
        victims: &[(f64, ChunkPos)],
        freed: bool,
        can_defragment: bool,
    ) {
        let frontier = match victims.last() {
            Some(&(nearest, _)) => nearest,
            None if freed && can_defragment => {
                self.fragmented.insert(pos);
                dist.next_up()
            }
            None => dist,
        };
        self.pull_frontier(frontier);
    }

    /// Takes the columns [`columns_to_retry`] picks off the list, with the
    /// sections to re-mesh, measuring from `eye`: the room is the arenas'
    /// `heaps` and `to_add` more arenas of `arena` the budget fits, and
    /// `farthest_loaded` is for eviction.
    fn retry(
        &mut self,
        eye: Option<DVec3>,
        heaps: &[(&Tlsf, &Tlsf)],
        to_add: u64,
        arena: ArenaSize,
        farthest_loaded: impl FnOnce() -> f64,
    ) -> Vec<(ChunkPos, Vec<i32>)> {
        let free = with_new_arenas(heaps_free(heaps.iter().copied()), arena, to_add);
        let new_arena = (to_add > 0).then_some(arena);
        let waiting = self.columns.iter().map(|(&pos, s)| {
            let need = s.need();
            Waiting {
                dist: eye_dist_sq(pos, eye),
                pos,
                need,
                fits_an_arena: fits_an_arena(need, new_arena, heaps.iter().copied()),
            }
        });
        let (retry, frontier_sq, fragmented) = columns_to_retry(
            waiting,
            free,
            self.frontier_sq,
            !self.claims.is_empty(),
            farthest_loaded,
            arena,
        );
        self.frontier_sq = frontier_sq;
        if heaps_can_defragment(heaps, arena) {
            self.fragmented.extend(fragmented);
        }
        retry
            .into_iter()
            .filter_map(|(pos, claim)| Some((pos, self.hand_back(pos, claim)?)))
            .collect()
    }

    /// Moves the frontier in to `dist_sq` if that's nearer.
    fn pull_frontier(&mut self, dist_sq: f64) {
        self.frontier_sq = self.frontier_sq.min(dist_sq);
    }

    /// Whether `pos` is at or past the frontier, seen from `eye`.
    fn beyond(&self, pos: ChunkPos, eye: Option<DVec3>) -> bool {
        eye.is_some_and(|eye| column_dist_sq(pos, eye) >= self.frontier_sq)
    }
}

/// One vertex and one index heap, sub-allocated by [`Tlsf`]: a heap of
/// vanilla's `UberGpuBuffer`, its vertex and index halves paired so each
/// arena draws in one call. Device-local behind the staging buffer, in
/// memory of its own so destroying it hands the VRAM back; host-visible,
/// from the shared allocator, on integrated GPUs.
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
    /// A new arena, or the driver's error when it has no memory for one.
    fn new(
        device: &rhi::backend::Device,
        use_staging: bool,
        format: TerrainFormat,
        size: ArenaSize,
    ) -> Result<Self, String> {
        let vertex_buffer = create_pool(
            device,
            use_staging,
            "vertex_arena",
            u64::from(size.vertices) * format.vertex_size(),
            rhi::BufferUsages::VERTEX,
        )?;
        let index_buffer = match create_pool(
            device,
            use_staging,
            "index_arena",
            u64::from(size.indices) * INDEX_SIZE,
            rhi::BufferUsages::INDEX,
        ) {
            Ok(buffer) => buffer,
            Err(error) => {
                vertex_buffer.destroy(device);
                return Err(error);
            }
        };
        Ok(Self {
            vertex_buffer,
            index_buffer,
            vertices: Tlsf::new(size.vertices),
            indices: Tlsf::new(size.indices),
            doomed: None,
        })
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

/// The `live` arenas, as (id, vertices used, indices used), fullest first,
/// for [`alloc_in`].
fn alloc_order(live: impl Iterator<Item = (u16, u32, u32)>) -> Vec<u16> {
    let mut order: Vec<_> = live.collect();
    order.sort_unstable_by_key(|&(_, vertices, _)| std::cmp::Reverse(vertices));
    order.into_iter().map(|(id, ..)| id).collect()
}

/// A vertex and an index slice from one of the arenas in `order` other than
/// `exclude`, trying them in that order: the fullest first, so the emptiest
/// can drain.
fn alloc_in(
    arenas: &mut [Option<Arena>],
    order: &[u16],
    exclude: Option<u16>,
    vertices: u32,
    indices: u32,
) -> Option<(u16, u32, u32)> {
    for &id in order {
        if exclude == Some(id) {
            continue;
        }
        let Some(arena) = arenas[id as usize].as_mut() else {
            continue;
        };
        if let Some((vertex, index)) =
            alloc_pair(&mut arena.vertices, &mut arena.indices, vertices, indices)
        {
            return Some((id, vertex, index));
        }
    }
    None
}

/// A vertex and an index slice from one arena's heaps, or neither.
fn alloc_pair(
    vertex_heap: &mut Tlsf,
    index_heap: &mut Tlsf,
    vertices: u32,
    indices: u32,
) -> Option<(u32, u32)> {
    let vertex = vertex_heap.alloc(vertices)?;
    let Some(index) = index_heap.alloc(indices) else {
        vertex_heap.free(vertex);
        return None;
    };
    Some((vertex, index))
}

/// The live arenas' (vertex, index) heaps.
fn live_heaps(arenas: &[Option<Arena>]) -> Vec<(&Tlsf, &Tlsf)> {
    arenas
        .iter()
        .filter_map(live)
        .map(|a| (&a.vertices, &a.indices))
        .collect()
}

/// [`compaction_can_help`] arenas of `size` with these (vertex, index)
/// `heaps`.
fn heaps_can_defragment(heaps: &[(&Tlsf, &Tlsf)], size: ArenaSize) -> bool {
    let used: Vec<(u32, u32)> = heaps.iter().map(|(v, i)| (v.used(), i.used())).collect();
    compaction_can_help(&used, size)
}

/// The free vertices and indices of the (vertex, index) `heaps`.
fn heaps_free<'a>(heaps: impl Iterator<Item = (&'a Tlsf, &'a Tlsf)>) -> (u64, u64) {
    let free = |heap: &Tlsf| u64::from(heap.capacity() - heap.used());
    heaps.fold((0, 0), |(v, i), (vertices, indices)| {
        (v + free(vertices), i + free(indices))
    })
}

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
    /// The slices `pending_moves` copy out of, retired once they're
    /// recorded so nothing reuses them before.
    moved_from: Vec<Slice>,
    /// A frame began since the pending copies and moves were recorded.
    unrecorded: bool,
    use_staging: bool,
    format: TerrainFormat,
    arena_size: ArenaSize,
    /// The most arenas there may be, whatever the budget.
    max_arenas: usize,
    waiting: Waitlist,
    /// The starved columns are worth retrying in the next
    /// [`Self::upload_batch`]: a slice was reclaimed, the last claim settled,
    /// a defragment drain ended, the eye reached another column, an arena
    /// went, or the budget grew.
    retry_due: bool,
    /// [`ChunkUploads::starved`] for the batch being uploaded.
    starved: Vec<(ChunkPos, Vec<i32>)>,
    /// The live arenas, fullest first, as of the last arena change or
    /// frame; the order allocation tries them in.
    alloc_order: Vec<u16>,
    /// The arena compaction is emptying, kept out of allocation.
    draining: Option<u16>,
    /// The drain started for [`Waitlist::fragmented`] room, not to shrink
    /// the total.
    defragmenting: bool,
    /// The `frame_seq` before which compaction doesn't start.
    compact_after: u64,
    /// The frame before which no arena is added, after the driver had no
    /// memory for one.
    add_arena_after: u64,
    /// Arena adds the driver failed in a row, each doubling the wait.
    add_arena_failures: u32,

    lists: Lists,
    /// Per-column bitmask of occlusion-visible section indices (bit `si`), from
    /// the CPU visibility graph. A column absent here defaults to fully
    /// visible, so freshly-loaded-but-not-yet-graphed columns still draw.
    chunk_visibility: Arc<HashMap<ChunkPos, u32>>,
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
    /// The camera, which eviction measures distance from (see
    /// [`Self::set_eye`]).
    eye: Option<DVec3>,
    /// What `uploaded_ms` counts from.
    epoch: std::time::Instant,
    /// The `uploaded_ms` of sections meshed without geometry, by column:
    /// vanilla stamps every compiled section, and fades its block entities
    /// in from then.
    bare_uploads: HashMap<ChunkPos, Vec<(i32, u32)>>,

    compute_pipeline: rhi::backend::ComputePipeline,
    compute_layout: rhi::backend::PipelineLayout,
    cull_layout: rhi::backend::BindGroupLayout,

    /// Sections uploaded before the first cull appear at once; later ones
    /// fade in.
    fade_enabled: bool,
    /// How long a section fades in (vanilla `chunkSectionFadeInTime`).
    fade_ms: u32,
    /// Post-cull section draw count read back from the GPU (lags a few frames);
    /// exposed for the debug overlay so occlusion's effect is visible.
    last_draw_count: u32,
    /// Indices the last cull's draws of every layer covered.
    last_index_count: u32,

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
            moved_from: Vec::new(),
            unrecorded: false,
            use_staging,
            format,
            arena_size: ARENA_SIZE,
            max_arenas: MAX_ARENAS,
            waiting: Waitlist::default(),
            retry_due: false,
            starved: Vec::new(),
            alloc_order: Vec::new(),
            draining: None,
            defragmenting: false,
            compact_after: 0,
            add_arena_after: 0,
            add_arena_failures: 0,
            lists: Lists::default(),
            chunk_visibility: Arc::default(),
            column_visibility: Vec::new(),
            visibility_pending: 0,
            bare_uploads: HashMap::new(),
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
                        3 * COUNTS_SIZE,
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
            fade_ms: crate::ui::menu::DEFAULT_CHUNK_FADE_MS,
            last_draw_count: 0,
            last_index_count: 0,
            frame_seq: 0,
            pending_free: VecDeque::new(),
        }
    }

    /// Sections drawn last time this frame slot ran (post frustum + occlusion
    /// cull). Read back from the GPU count buffer, so it lags a few frames.
    pub fn sections_drawn(&self) -> u32 {
        self.last_draw_count
    }

    /// Triangles the last cull drew, every layer.
    pub fn triangles_drawn(&self) -> u32 {
        self.last_index_count / 3
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
        self.unrecorded = false;
        if !self.has_pending() {
            return;
        }
        self.record_pending(encoder, self.frame_seq + MAX_FRAMES_IN_FLIGHT as u64);
        encoder.transition_buffers(rhi::BufferUse::CopyDst, rhi::BufferUse::DrawInput);
    }

    fn has_pending(&self) -> bool {
        !(self.pending_copies.is_empty() && self.pending_moves.is_empty())
    }

    /// Submits the pending staging copies and moves now and waits for them,
    /// when a batch outgrows its staging buffer, or a frame that never drew
    /// left them.
    fn flush_now(&mut self, device: &rhi::backend::Device) {
        device.submit(|encoder| self.record_pending(encoder, 0));
        self.staging_used = [(0, 0); STAGING_SLOTS];
        self.staging_busy = [0; STAGING_SLOTS];
    }

    /// Records the pending staging copies, the staging buffers they read
    /// busy until `busy_until`, then compaction's moves, which may read what
    /// the copies wrote, retiring the slices they moved out of.
    fn record_pending(&mut self, encoder: &rhi::backend::CommandEncoder, busy_until: u64) {
        record_grouped(encoder, &self.arenas, &mut self.pending_copies, |slot| {
            self.staging_used[slot] = (0, 0);
            self.staging_busy[slot] = busy_until;
            let staging = self.staging[slot].handle();
            Some((staging, staging))
        });
        self.pending_copies.clear();
        if self.pending_moves.is_empty() {
            return;
        }
        encoder.transition_buffers(rhi::BufferUse::CopyDst, rhi::BufferUse::CopySrc);
        record_grouped(encoder, &self.arenas, &mut self.pending_moves, |from| {
            let src = self.arenas[from as usize].as_ref()?;
            Some((src.vertex_buffer.handle(), src.index_buffer.handle()))
        });
        self.pending_moves.clear();
        let moved = std::mem::take(&mut self.moved_from);
        self.retire_slices(moved);
    }

    /// Upload a batch of mesh results, each replacing the sections in its
    /// `mesh.replaced` range. The vertices go to this frame's staging buffer
    /// and the copies into the arenas are recorded as the frame starts, so a
    /// streaming frame never waits for the GPU: a batch outgrowing the
    /// staging buffer stops there, and the rest is the caller's to send next
    /// frame. Also returns the starved sections that may fit now, per column,
    /// for re-meshing.
    pub fn upload_batch(
        &mut self,
        device: &rhi::backend::Device,
        meshes: &[ChunkMeshData],
    ) -> ChunkUploads {
        if meshes.is_empty() {
            return self.uploads(device, 0);
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
            index_len: u32,
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

            fn slice(&self) -> Slice {
                Slice::new(
                    self.arena,
                    (self.vtx_off, self.vertex_count),
                    (self.idx_off, self.index_len),
                )
            }
        }

        // Retired slices only reclaim in `begin_frame`; if rendering is paused
        // while meshing continues (e.g. minimized window) the backlog grows
        // unbounded. Past a sane bound, force a GPU wait and reclaim it all.
        const PENDING_FREE_DRAIN_THRESHOLD: usize = 8192;
        if self.pending_free.len() > PENDING_FREE_DRAIN_THRESHOLD {
            device.wait_idle();
            while let Some((_, slice)) = self.pending_free.pop_front() {
                self.reclaim(slice);
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

        let mut taken = meshes.len();
        for (i, mesh) in meshes.iter().enumerate() {
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
            let v_bytes: usize = upload_secs.iter().map(|s| s.vertices.bytes().len()).sum();
            let i_bytes: usize = upload_secs
                .iter()
                .map(|s| s.index_len() as usize * INDEX_SIZE as usize)
                .sum();
            let (stg_v, stg_i) = self.staging_used[slot];
            if self.use_staging
                && i > 0
                && (stg_v + v_bytes > staging_half || stg_i + i_bytes > staging_half)
            {
                // The rest go next frame, their waitlist claims still held.
                taken = i;
                break;
            }
            // Placed or starved again below, they no longer hold the room.
            let unheld = self.waiting.settle(mesh.pos, &accepted);

            if self.use_staging && !upload_secs.is_empty() {
                // Verts and indices share the staging buffer (two halves). A chunk
                // too large for one half is dropped rather than overflowing the
                // buffer. This is permanent, so it's not reported for retry.
                if v_bytes > staging_half || i_bytes > staging_half {
                    tracing::warn!(
                        "Chunk {:?} too large for staging ({} v / {} i bytes), skipping",
                        mesh.pos,
                        v_bytes,
                        i_bytes,
                    );
                    self.retire_sections(mesh.pos, &accepted);
                    self.drop_if_empty(mesh.pos);
                    self.waiting.stop_waiting(mesh.pos, &accepted);
                    continue;
                }
            }

            // Allocate the new slices before retiring the old: no room then
            // keeps the old mesh drawn inside the frontier instead of
            // dropping it.
            let mut plans: Vec<Plan> = Vec::with_capacity(upload_secs.len());
            let mut lens: HashMap<i32, SliceLens> = HashMap::new();
            // Past the eviction frontier a column waits for room instead.
            let beyond = self.waiting.beyond(mesh.pos, self.eye);
            let mut no_room = beyond;
            for sec in &upload_secs {
                let vcount = sec.vertices.len() as u32;
                let icount = sec.index_len();
                if vcount == 0 || icount == 0 {
                    continue;
                }
                lens.insert(sec.section_index, (vcount, icount));
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
                    index_len: icount,
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
            let need: Need = lens.values().copied().collect();
            // A column with nothing to place doesn't wait.
            if no_room && need.vertices > 0 {
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
                self.report_starved(mesh.pos, accepted.iter().copied());
                let sections = accepted
                    .iter()
                    .map(|&si| (si, lens.get(&si).copied().unwrap_or_default()));
                self.waiting.starve(mesh.pos, sections);
                continue;
            }

            self.waiting.stop_waiting(mesh.pos, &accepted);
            // Placed, its room is taken and the rest may try what's left.
            self.retry_due |= unheld;
            // Remember which sections were present so a re-meshed one swaps
            // instantly while a freshly revealed one still fades in.
            let was_present = self.retire_sections(mesh.pos, &accepted);
            for &si in &accepted {
                if !plans.iter().any(|p| p.section_index == si)
                    && self.bare_upload_ms(mesh.pos, si).is_none()
                {
                    let ms = self.upload_ms(mesh.pos, si, &was_present, now_ms);
                    self.bare_uploads
                        .entry(mesh.pos)
                        .or_default()
                        .push((si, ms));
                }
            }
            if plans.is_empty() {
                // Every accepted section is now empty.
                self.drop_if_empty(mesh.pos);
                continue;
            }

            if self.use_staging {
                let mv: usize = plans.iter().map(|p| p.verts.len()).sum();
                let mi: usize = plans
                    .iter()
                    .map(|p| p.index_len as usize * INDEX_SIZE as usize)
                    .sum();
                // Only the batch's first mesh gets here with the half short
                // (an edit uploaded earlier in the frame); it alone fits a half
                // (checked above), so a flush always makes room.
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
                let uploaded_ms = self.upload_ms(mesh.pos, p.section_index, &was_present, now_ms);
                let section = SectionAlloc {
                    section_index: p.section_index,
                    aabb: p.aabb,
                    origin: p.origin,
                    arena: p.arena,
                    first_index: p.idx_off,
                    index_count: p.indices.len() as u32,
                    solid_index_count: p.solid_index_count,
                    water_index_count: p.water_indices.len() as u32,
                    idx_len: p.index_len,
                    vertex_offset: p.vtx_off as i32,
                    vtx_len: p.vertex_count,
                    uploaded_ms,
                    epoch: mesh.upload_epoch,
                    meta_index: 0,
                };
                self.lists.insert(column, section);
            }
        }

        self.ensure_meta_capacity(device);

        self.uploads(device, taken)
    }

    /// The batch's starved sections, and those to retry: after the batch, so
    /// the retry sees the room it took and nothing it hands back is placed
    /// in the same batch.
    fn uploads(&mut self, device: &rhi::backend::Device, taken: usize) -> ChunkUploads {
        let mut remesh = Vec::new();
        // Until something changes, the arenas are as full as before and the
        // retry would fail too.
        if std::mem::take(&mut self.retry_due) {
            self.retry_starved(device, &mut remesh);
        }
        ChunkUploads {
            remesh,
            starved: std::mem::take(&mut self.starved),
            taken,
        }
    }

    /// [`fade_from`] for `pos`'s section `si`: shown at once when re-meshed
    /// or uploaded before the first cull.
    fn upload_ms(&self, pos: ChunkPos, si: i32, was_present: &HashSet<i32>, now_ms: u32) -> u32 {
        fade_from(
            was_present.contains(&si) || !self.fade_enabled,
            self.bare_upload_ms(pos, si),
            now_ms,
        )
    }

    fn bare_upload_ms(&self, pos: ChunkPos, si: i32) -> Option<u32> {
        let sections = self.bare_uploads.get(&pos)?;
        sections.iter().find(|&&(s, _)| s == si).map(|&(_, ms)| ms)
    }

    /// `pos`'s column slot, made with its visibility mask if it has none.
    fn column_slot(&mut self, pos: ChunkPos) -> u32 {
        let slot = self.lists.columns.slot_or_insert(pos) as usize;
        if self.column_visibility.len() <= slot {
            // The buffers may still hold a mask from before a clear.
            self.column_visibility.resize(slot + 1, u32::MAX);
            self.visibility_pending = MAX_FRAMES_IN_FLIGHT as u32;
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

    /// A vertex and an index slice in one arena: stopping a drain to take its
    /// arena's room, then taking back a doomed arena or adding one when
    /// they're all full; `None` once that would pass the budget.
    fn alloc_slice(
        &mut self,
        device: &rhi::backend::Device,
        vertices: u32,
        indices: u32,
    ) -> Option<(u16, u32, u32)> {
        let alloc = |this: &mut Self| {
            alloc_in(
                &mut this.arenas,
                &this.alloc_order,
                this.draining,
                vertices,
                indices,
            )
        };
        if let Some(found) = alloc(self) {
            return Some(found);
        }
        // Before evicting or growing, take the room compaction is draining.
        if self.draining.is_some() {
            self.back_off_compaction();
            if let Some(found) = alloc(self) {
                return Some(found);
            }
        }
        if self.revive_arena() || (self.can_add_arena(device) && self.add_arena(device)) {
            return alloc(self);
        }
        None
    }

    /// Takes back an arena doomed for being empty, which still holds its
    /// share of the budget, returning whether there was one.
    fn revive_arena(&mut self) -> bool {
        let Some(arena) = self
            .arenas
            .iter_mut()
            .flatten()
            .find(|a| a.doomed.is_some())
        else {
            return false;
        };
        arena.doomed = None;
        self.refresh_alloc_order();
        true
    }

    fn can_add_arena(&self, device: &rhi::backend::Device) -> bool {
        self.arenas_to_add(device) > 0
    }

    /// How many more arenas fit the budget of the heaps arenas go in: what
    /// `VK_EXT_memory_budget` says this process may use, less headroom for
    /// everything else, or without it a share of them. The first always
    /// fits.
    fn arenas_to_add(&self, device: &rhi::backend::Device) -> u64 {
        let arenas = self.arena_count();
        let cap = (self.max_arenas as u64).saturating_sub(arenas);
        if cap == 0 || self.frame_seq < self.add_arena_after {
            return 0;
        }
        let bytes = self.arena_size.bytes(self.format);
        let location = arena_location(self.use_staging);
        let fit = match device.memory_budget(location) {
            Some(budget) => {
                let headroom = MIN_BUDGET_HEADROOM.max(budget.budget / 8);
                budget.budget.saturating_sub(budget.usage + headroom) / bytes
            }
            None => {
                let share = (device.memory_bytes(location) as f64 * BUDGET_FRACTION) as u64;
                (share / bytes).saturating_sub(arenas)
            }
        };
        let fit = if arenas == 0 { fit.max(1) } else { fit };
        cap.min(fit)
    }

    /// Adds an arena, returning whether it did. When the driver has no
    /// memory for one, none is added for a while, longer each time, so
    /// columns starve and evict instead.
    fn add_arena(&mut self, device: &rhi::backend::Device) -> bool {
        let arena = match Arena::new(device, self.use_staging, self.format, self.arena_size) {
            Ok(arena) => {
                self.add_arena_failures = 0;
                arena
            }
            Err(error) => {
                self.add_arena_after =
                    self.frame_seq + (BACKOFF_FRAMES << self.add_arena_failures.min(6));
                self.add_arena_failures += 1;
                tracing::warn!("{error}; no chunk arena added for a while");
                return false;
            }
        };
        match self.arenas.iter().position(Option::is_none) {
            Some(id) => self.arenas[id] = Some(arena),
            None => self.arenas.push(Some(arena)),
        }
        self.refresh_alloc_order();
        self.log_arenas("added");
        true
    }

    fn refresh_alloc_order(&mut self) {
        self.alloc_order = alloc_order(self.live_use());
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
    /// none goes and `pos` waits past the frontier. When the room is there
    /// but split, `pos` stays inside for compaction to make it room.
    fn evict_beyond(&mut self, pos: ChunkPos, need: Need) {
        let Some(eye) = self.eye else {
            return;
        };
        let dist = column_dist_sq(pos, eye);
        let loaded = self
            .lists
            .columns
            .iter()
            .map(|(_, column)| (column_dist_sq(column.pos, eye), column.pos, column.need()));
        let (victims, freed) = eviction_victims(loaded, dist, need, self.free_room());
        let can_defragment = self.can_defragment();
        self.waiting
            .evicting_for(pos, dist, &victims, freed, can_defragment);
        for (_, victim) in victims {
            self.evict(victim);
            tracing::debug!("Evicted {victim:?} for {pos:?}");
        }
    }

    /// Adds `pos`'s `sections` to the batch's [`ChunkUploads::starved`].
    fn report_starved(&mut self, pos: ChunkPos, sections: impl Iterator<Item = i32>) {
        self.starved.push((pos, sections.collect()));
    }

    /// Drops `pos`'s mesh, starving the column until there's room for it.
    fn evict(&mut self, pos: ChunkPos) {
        let sections: Vec<_> = self
            .take_column(pos)
            .iter()
            .map(SectionAlloc::lens)
            .collect();
        if !sections.is_empty() {
            self.report_starved(pos, sections.iter().map(|&(si, _)| si));
            self.waiting.evicted(pos, sections);
        }
    }

    /// Drops `pos`'s column from the draw lists and retires its slices,
    /// returning its sections.
    fn take_column(&mut self, pos: ChunkPos) -> Vec<SectionAlloc> {
        let Some(slot) = self.lists.columns.slot(&pos) else {
            return Vec::new();
        };
        let dropped = self.lists.remove_sections(slot, |_| false);
        self.retire_slices(dropped.iter().map(SectionAlloc::slice));
        self.lists.columns.remove(&pos);
        dropped
    }

    /// Hands the starved columns that could fit now back for re-meshing, as
    /// [`columns_to_retry`] picks them, counting the arenas the budget still
    /// fits as room.
    fn retry_starved(
        &mut self,
        device: &rhi::backend::Device,
        out: &mut Vec<(ChunkPos, Vec<i32>)>,
    ) {
        if self.waiting.columns.is_empty() {
            return;
        }
        let to_add = self.arenas_to_add(device);
        // The draining arena too: allocation stops draining to use it.
        let heaps = live_heaps(&self.arenas);
        let eye = self.eye;
        let columns = &self.lists.columns;
        out.extend(
            self.waiting
                .retry(eye, &heaps, to_add, self.arena_size, || {
                    farthest_from(columns.iter().map(|(_, column)| column.pos), eye)
                }),
        );
    }

    /// A retired slice no frame reads any more goes back, and may make room
    /// for a starved column.
    fn reclaim(&mut self, slice: Slice) {
        self.free_slice(slice);
        self.retry_due = true;
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
        // The last frame never drew, so nothing recorded what it left.
        if std::mem::replace(&mut self.unrecorded, true) && self.has_pending() {
            self.flush_now(device);
        }
        self.frame_seq += 1;
        while self
            .pending_free
            .front()
            .is_some_and(|&(retire_at, _)| retire_at <= self.frame_seq)
        {
            let (_, slice) = self.pending_free.pop_front().unwrap();
            self.reclaim(slice);
        }
        if !self.waiting.is_empty()
            && self.frame_seq.is_multiple_of(BUDGET_RECHECK_FRAMES)
            && self.can_add_arena(device)
        {
            self.retry_due = true;
        }
        self.free_empty_arenas(device);
        self.refresh_alloc_order();
        self.compact(device);
    }

    /// Destroys doomed arenas no frame reads any more, and dooms the empty
    /// ones [`arenas_to_doom`] picks, as vanilla destroys an empty heap.
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
            // The budget it held may take another arena now.
            self.retry_due = true;
            self.log_arenas("freed");
        }
        let live: Vec<(u16, bool)> = self
            .arenas
            .iter()
            .enumerate()
            .filter_map(|(id, slot)| live(slot).map(|arena| (id as u16, arena.is_empty())))
            .collect();
        for id in arenas_to_doom(&live, !self.waiting.is_empty()) {
            if let Some(arena) = self.arenas[id as usize].as_mut() {
                // Frames from here on skip it; earlier ones are done by then.
                arena.doomed = Some(self.frame_seq + MAX_FRAMES_IN_FLIGHT as u64);
            }
            if self.draining == Some(id) {
                self.stop_draining();
            }
        }
    }

    /// Leaves the draining arena to allocation again.
    fn stop_draining(&mut self) {
        self.draining = None;
        self.defragmenting = false;
    }

    /// Stops draining and waits a while before compacting again, as the
    /// room is wanted.
    fn back_off_compaction(&mut self) {
        self.stop_draining();
        self.compact_after = self.frame_seq + BACKOFF_FRAMES;
    }

    /// Once the arenas' contents would fit one fewer, drains the emptiest
    /// into the rest, a bounded amount per frame, so freeing it can shrink
    /// the total. The copies are recorded as the frame starts.
    ///
    /// Not while columns are starved: they want the room, and the arena it
    /// frees would only be added back for them. Unless the room is
    /// [`Waitlist::fragmented`]: then the emptiest drains at any fill into
    /// the others' gaps, and stays whole for them, once per flagging.
    fn compact(&mut self, device: &rhi::backend::Device) {
        if !self.waiting.is_fragmented() {
            if !self.waiting.is_empty() {
                self.stop_draining();
                return;
            }
            if self.defragmenting {
                self.stop_draining();
            }
        }
        if self.frame_seq < self.compact_after {
            return;
        }
        let from = match self.draining {
            Some(id) => id,
            None => {
                let Some(id) = self.arena_to_drain() else {
                    return;
                };
                tracing::debug!("Compacting chunk arena {id}");
                self.draining = Some(id);
                self.defragmenting = self.waiting.is_fragmented();
                id
            }
        };

        let vertex_size = self.format.vertex_size();
        let mut moved = 0;
        let mut no_room = false;
        // The arena's list is the sections in it; nothing lands in it while
        // it drains.
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
            let Some((to, vertex, index)) = alloc_in(
                &mut self.arenas,
                &self.alloc_order,
                Some(from),
                vtx_len,
                idx_len,
            ) else {
                tracing::debug!("No room to compact chunk arena {from}");
                no_room = true;
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
                self.moved_from.push(sec.slice());
                sec.arena = to;
                sec.first_index = index;
                sec.vertex_offset = vertex as i32;
            });
            if relaid {
                self.ensure_meta_capacity(device);
            }
            moved += vtx_len;
        }
        let drained = self.defragmenting
            && self
                .lists
                .segments
                .get(from as usize)
                .is_none_or(|segment| segment.metas.is_empty());
        if no_room || drained {
            if self.defragmenting {
                // Retried, the columns it was for take the room or flag it
                // again for the next drain.
                self.waiting.fragmented.clear();
                self.retry_due = true;
            }
            self.back_off_compaction();
        }
    }

    /// Free vertices and indices across the live arenas.
    fn free_room(&self) -> (u64, u64) {
        heaps_free(live_heaps(&self.arenas).into_iter())
    }

    fn arena_to_drain(&self) -> Option<u16> {
        arena_to_drain(
            self.live_use(),
            self.arena_size,
            self.waiting.is_fragmented(),
        )
    }

    fn can_defragment(&self) -> bool {
        heaps_can_defragment(&live_heaps(&self.arenas), self.arena_size)
    }

    /// Each live arena as (id, vertices used, indices used).
    fn live_use(&self) -> impl Iterator<Item = (u16, u32, u32)> + '_ {
        self.arenas.iter().enumerate().filter_map(|(id, slot)| {
            live(slot).map(|a| (id as u16, a.vertices.used(), a.indices.used()))
        })
    }

    pub fn remove(&mut self, pos: &ChunkPos) {
        self.waiting.forget(*pos);
        self.take_column(*pos);
        self.bare_uploads.remove(pos);
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
        self.alloc_order.clear();
        self.lists.segments.clear();
    }

    /// Arenas of `vertices`, at most `max` of them, so the harness fills
    /// them at a small render distance. A new size, or a cap under the
    /// arenas there are, drops every mesh as [`Self::set_format`] does;
    /// returns whether it did. The caller waits for in-flight frames first.
    #[cfg(feature = "test-harness")]
    pub fn set_arena_limits(
        &mut self,
        device: &rhi::backend::Device,
        vertices: u32,
        max: usize,
    ) -> bool {
        self.max_arenas = max.clamp(1, MAX_ARENAS);
        let size = ArenaSize::new(vertices);
        if size == self.arena_size && self.arena_count() as usize <= self.max_arenas {
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
        self.bare_uploads.clear();
        for arena in self.arenas.iter_mut().flatten() {
            arena.vertices.reset();
            arena.indices.reset();
        }
        self.pending_free.clear();
        self.pending_copies.clear();
        self.pending_moves.clear();
        self.moved_from.clear();
        self.staging_used = [(0, 0); STAGING_SLOTS];
        self.waiting = Waitlist::default();
        self.eye = None;
        self.stop_draining();
        self.fade_enabled = false;
    }

    /// A column not drawing its newest mesh, or none, for lack of room,
    /// nearer than the eviction frontier or with an arena to add for it: one
    /// that should fit.
    #[cfg(feature = "test-harness")]
    pub fn starved_column(&self, device: &rhi::backend::Device) -> Option<ChunkPos> {
        let can_add = self.can_add_arena(device);
        self.waiting
            .columns
            .keys()
            .find(|&&pos| can_add || !self.waiting.beyond(pos, self.eye))
            .copied()
    }

    /// Compaction has an arena to drain, or a drained one to free.
    #[cfg(feature = "test-harness")]
    pub fn compacting(&self) -> bool {
        self.draining.is_some()
            || self.arenas.iter().flatten().any(|a| a.doomed.is_some())
            || self.arena_to_drain().is_some()
    }

    /// Columns wait for room while the eye's own column draws inside the
    /// eviction frontier: too little room evicted far columns, not near ones.
    #[cfg(feature = "test-harness")]
    pub fn evicted(&self) -> Result<(), String> {
        if self.waiting.columns.is_empty() {
            return Err("no column waits for room".into());
        }
        let eye = self.eye.ok_or("nothing culled yet")?;
        let own = eye_column(eye);
        if self.lists.columns.get(&own).is_none() || self.waiting.beyond(own, self.eye) {
            return Err(format!(
                "the eye's column {own:?} isn't drawn inside the eviction frontier"
            ));
        }
        Ok(())
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
        let retired = self
            .pending_free
            .iter()
            .map(|&(_, slice)| slice)
            .chain(self.moved_from.iter().copied());
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

    /// Whether `pos`'s section `section` shows its block entities: not
    /// occluded and faded in (see [`block_entities_faded_in`]). Unlike
    /// vanilla's `visibleSections`, off-screen sections pass too, and the
    /// GPU clips what they draw.
    pub fn shows_block_entities(&self, pos: ChunkPos, section: i32) -> bool {
        let visible = self
            .chunk_visibility
            .get(&pos)
            .is_none_or(|mask| mask & section_bit(section) != 0);
        let uploaded_ms = self
            .lists
            .columns
            .get(&pos)
            .and_then(|column| column.sections.iter().find(|s| s.section_index == section))
            .map(|s| s.uploaded_ms)
            .or_else(|| self.bare_upload_ms(pos, section));
        visible
            && uploaded_ms.is_none_or(|ms| block_entities_faded_in(ms, self.now_ms(), self.fade_ms))
    }

    pub fn set_fade_ms(&mut self, fade_ms: u32) {
        self.fade_ms = fade_ms;
    }

    /// Push the CPU visibility graph's per-column visible-section masks.
    /// Columns not present default to fully visible, so the cull only omits
    /// sections the graph proved occluded.
    pub fn set_chunk_visibility(&mut self, vis: Arc<HashMap<ChunkPos, u32>>) {
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
            fade_ms: self.fade_ms,
            _pad: [0; 2],
            arena_base: [[0; 4]; MAX_ARENAS / 4],
            arena_len: [[0; 4]; MAX_ARENAS / 4],
        };
        for (id, segment) in self.lists.segments.iter().enumerate() {
            data.arena_base[id / 4][id % 4] = segment.base;
            data.arena_len[id / 4][id % 4] = segment.metas.len() as u32;
        }
        data
    }

    /// Where eviction and the frontier measure from: the camera, which the
    /// loading screen moves before any cull.
    pub fn set_eye(&mut self, eye: DVec3) {
        if !self.waiting.is_empty()
            && self
                .eye
                .is_none_or(|last| eye_column(last) != eye_column(eye))
        {
            self.retry_due = true;
        }
        self.eye = Some(eye);
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
        self.set_eye(eye);
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
        // so its readback holds the previous cull's totals: per layer, the
        // draws per arena then the indices per arena.
        (self.last_draw_count, self.last_index_count) = self.count_readbacks[frame].read(|bytes| {
            let words: &[u32] = bytemuck::cast_slice(bytes);
            let (layers, _) = words.as_chunks::<{ 2 * MAX_ARENAS }>();
            let mut draws = 0;
            let mut indices = 0;
            for (layer, counts) in layers.iter().enumerate() {
                let (per_arena, per_arena_indices) = counts.split_at(MAX_ARENAS);
                if layer != Layer::Water as usize {
                    draws += per_arena.iter().sum::<u32>();
                }
                indices += per_arena_indices.iter().sum::<u32>();
            }
            (draws, indices)
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
        for (i, count) in self
            .camera
            .counts
            .iter()
            .map(|counts| &counts[frame])
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
        // Both culls write the sections' fade.
        encoder.transition_buffers(rhi::BufferUse::StorageWrite, rhi::BufferUse::StorageWrite);
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
    ///
    /// TODO: water isn't depth-sorted (the cull appends it in no set order,
    /// arena by arena), so overlapping translucent surfaces (oceans at
    /// grazing angles, water seen through water) can blend out of order.
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
/// and the `free` room together hold it, with none when they don't. The free
/// room doesn't cut the victims short, as it may be split into gaps too
/// small to use.
fn eviction_victims(
    loaded: impl Iterator<Item = (f64, ChunkPos, Need)>,
    limit: f64,
    need: Need,
    (free_vertices, free_indices): (u64, u64),
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
    let fits = need.fits_in(
        freed.vertices.saturating_add(free_vertices),
        freed.indices.saturating_add(free_indices),
    );
    if !fits {
        victims.clear();
    }
    (victims, fits)
}

/// Whether compaction, which only moves sections between arenas, can make
/// room: of two or more arenas of `size` with `used` (vertices, indices)
/// each, one's sections fit the others' free room.
fn compaction_can_help(used: &[(u32, u32)], size: ArenaSize) -> bool {
    let free = |&(v, i): &(u32, u32)| (u64::from(size.vertices - v), u64::from(size.indices - i));
    let (free_vertices, free_indices) = used
        .iter()
        .map(free)
        .fold((0, 0), |(v, i), (fv, fi)| (v + fv, i + fi));
    used.len() > 1
        && used.iter().any(|arena| {
            let (own_vertices, own_indices) = free(arena);
            u64::from(arena.0) <= free_vertices - own_vertices
                && u64::from(arena.1) <= free_indices - own_indices
        })
}

/// The `free` (vertices, indices), and `count` arenas of `size` more.
fn with_new_arenas((vertices, indices): (u64, u64), size: ArenaSize, count: u64) -> (u64, u64) {
    (
        vertices.saturating_add(u64::from(size.vertices).saturating_mul(count)),
        indices.saturating_add(u64::from(size.indices).saturating_mul(count)),
    )
}

/// Whether `need`'s largest section fits one of the (vertex, index)
/// `heaps`, or a `new_arena`.
fn fits_an_arena<'a>(
    need: Need,
    new_arena: Option<ArenaSize>,
    mut heaps: impl Iterator<Item = (&'a Tlsf, &'a Tlsf)>,
) -> bool {
    new_arena
        .is_some_and(|size| need.max_vertices <= size.vertices && need.max_indices <= size.indices)
        || heaps.any(|(vertices, indices)| {
            vertices.fits(need.max_vertices) && indices.fits(need.max_indices)
        })
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

/// The starved columns to hand back, nearest first, each with whether it
/// claims the room (see [`Waitlist::claims`]), and the eviction frontier
/// after. Inside the frontier a column comes back, claiming, when it fits
/// the free vertices and indices, or when a loaded column farther out (the
/// farthest is at `farthest_loaded()`, asked at most once and only then) can
/// be evicted for it; either way the free room is its, so no farther column
/// takes it first. Past the frontier, only while no column holds a claim
/// (`claimed`, or one nearer in this pass) or was left short, and
/// [`FRONTIER_SLACK`] of an arena stays free after, moving the frontier out
/// past it, so the frontier doesn't swing back and forth as columns come and
/// go.
fn columns_to_retry(
    waiting: impl Iterator<Item = Waiting>,
    (mut free_vertices, mut free_indices): (u64, u64),
    mut frontier_sq: f64,
    claimed: bool,
    farthest_loaded: impl FnOnce() -> f64,
    arena: ArenaSize,
) -> (Vec<(ChunkPos, bool)>, f64, Vec<ChunkPos>) {
    let mut waiting: Vec<Waiting> = waiting.collect();
    waiting.sort_unstable_by(|a, b| a.dist.total_cmp(&b.dist));
    let farthest_loaded = std::cell::LazyCell::new(farthest_loaded);
    let mut retry = Vec::new();
    let mut held = claimed;
    let mut fragmented = Vec::new();
    for column in waiting {
        // With nothing to place it takes no room, wherever it is.
        if column.need.vertices == 0 {
            retry.push((column.pos, false));
            continue;
        }
        let past = column.dist >= frontier_sq;
        if past && held {
            break;
        }
        let (keep_vertices, keep_indices) = if past {
            (
                u64::from(arena.vertices) / FRONTIER_SLACK,
                u64::from(arena.indices) / FRONTIER_SLACK,
            )
        } else {
            (0, 0)
        };
        let room = column.need.fits_in(
            free_vertices.saturating_sub(keep_vertices),
            free_indices.saturating_sub(keep_indices),
        );
        if !(room && column.fits_an_arena) {
            if past {
                break;
            }
            held = true;
            if column.dist >= *farthest_loaded {
                if room {
                    fragmented.push(column.pos);
                }
                continue;
            }
        }
        free_vertices = free_vertices.saturating_sub(column.need.vertices);
        free_indices = free_indices.saturating_sub(column.need.indices);
        if past {
            frontier_sq = column.dist.next_up();
        } else {
            held = true;
        }
        retry.push((column.pos, !past));
    }
    (retry, frontier_sq, fragmented)
}

/// The arena compaction should drain, of the `live` ones as (id, vertices
/// used, indices used): the emptiest, once the others could hold everything
/// filled to [`COMPACT_FILL`], or at any fill to `defragment`.
fn arena_to_drain(
    live: impl Iterator<Item = (u16, u32, u32)>,
    arena: ArenaSize,
    defragment: bool,
) -> Option<u16> {
    let live: Vec<_> = live.collect();
    if live.len() < 2 {
        return None;
    }
    let emptiest = live.iter().min_by_key(|&&(_, v, _)| v).map(|&(id, ..)| id);
    if defragment {
        return emptiest;
    }
    let used_vertices: u64 = live.iter().map(|&(_, v, _)| u64::from(v)).sum();
    let used_indices: u64 = live.iter().map(|&(.., i)| u64::from(i)).sum();
    let room = (live.len() - 1) as f64 * COMPACT_FILL;
    if used_vertices as f64 > room * f64::from(arena.vertices)
        || used_indices as f64 > room * f64::from(arena.indices)
    {
        return None;
    }
    emptiest
}

/// Of the `live` arenas as (id, empty), those to doom: every empty one but
/// the last arena left, and none while columns are `waiting` for room, as
/// the arena would only be added back for them.
fn arenas_to_doom(live: &[(u16, bool)], waiting: bool) -> Vec<u16> {
    if waiting {
        return Vec::new();
    }
    let mut left = live.len();
    let mut doomed = Vec::new();
    for &(id, empty) in live {
        if left > 1 && empty {
            doomed.push(id);
            left -= 1;
        }
    }
    doomed
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
) -> Result<rhi::backend::Buffer, String> {
    let usage = usage | rhi::BufferUsages::COPY_SRC | rhi::BufferUsages::COPY_DST;
    let location = arena_location(use_staging);
    if use_staging {
        rhi::backend::Buffer::try_dedicated(device, label, size, usage, location)
    } else {
        rhi::backend::Buffer::try_new(device, label, size, usage, location)
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
            &[],
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
    use std::collections::hash_map::Entry;

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
    fn block_entities_wait_for_their_section_to_fade_to_three_tenths() {
        assert!(block_entities_faded_in(0, 5, 1000));
        assert!(!block_entities_faded_in(1000, 1299, 1000));
        assert!(block_entities_faded_in(1000, 1300, 1000));
        assert!(block_entities_faded_in(1000, 1000, 0));
    }

    #[test]
    fn a_section_fades_from_its_first_upload() {
        // Re-meshed, or uploaded before the first cull: no fade.
        assert_eq!(fade_from(true, Some(500), 2000), 0);
        // Freshly revealed.
        assert_eq!(fade_from(false, None, 2000), 2000);
        // Meshed without geometry first: keeps fading from then.
        assert_eq!(fade_from(false, Some(500), 2000), 500);
    }

    #[test]
    fn a_sort_under_way_finishes_while_the_eye_moves() {
        let mut lists = Lists::default();
        let pos = ChunkPos::new(0, 0);
        for arena in 0..3 {
            lists.list(pos, -64, [arena * 2, arena * 2 + 1], arena as u16);
        }
        // The first sort runs even with the eye at the origin.
        lists.sort_step(DVec3::ZERO);
        assert_eq!(lists.sort_at, Some(1));
        // Moving on doesn't restart it before every list is sorted.
        lists.sort_step(DVec3::new(100.0, 0.0, 0.0));
        lists.sort_step(DVec3::new(200.0, 0.0, 0.0));
        assert_eq!(lists.sort_at, Some(3));
        lists.sort_step(DVec3::new(300.0, 0.0, 0.0));
        assert_eq!(lists.sort_at, None);
        // Then the next starts from where the eye is.
        lists.sort_step(DVec3::new(400.0, 0.0, 0.0));
        assert_eq!(lists.sort_at, Some(1));
        assert_eq!(lists.last_sort_cam, Some(DVec3::new(400.0, 0.0, 0.0)));
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
            eviction_victims(loaded.into_iter(), 2.0, need(150), (0, 0)),
            (far.clone(), true)
        );
        // Never one nearer than the column it's for, and none when the
        // farther ones can't free enough.
        assert_eq!(
            eviction_victims(loaded.into_iter(), 8.0, need(1000), (0, 0)),
            (vec![], false)
        );
        // Free room counts toward the need but evicts no fewer.
        assert_eq!(
            eviction_victims(loaded.into_iter(), 8.0, need(1000), (800, u64::MAX)),
            (far, true)
        );
        // Nothing farther, but room in total: what failed was the gaps.
        assert_eq!(
            eviction_victims(loaded.into_iter(), 16.0, need(100), (100, u64::MAX)),
            (vec![], true)
        );
        assert_eq!(
            eviction_victims(loaded.into_iter(), 0.0, Need::default(), (0, 0)),
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
        let (retry, frontier, _) = columns_to_retry(
            [waiting(1.0, 1, 50), waiting(4.0, 2, 500)].into_iter(),
            free,
            100.0,
            false,
            || 9.0,
            ARENA_SIZE,
        );
        assert_eq!(
            retry,
            [(ChunkPos::new(1, 0), true), (ChunkPos::new(2, 0), true)]
        );
        assert_eq!(frontier, 100.0);
        // One coming back for eviction keeps the free room from a farther
        // one, past the frontier, that would fit it otherwise.
        let slack = u64::from(ARENA_SIZE.vertices) / FRONTIER_SLACK;
        let (retry, frontier, _) = columns_to_retry(
            [waiting(4.0, 2, slack + 11), waiting(200.0, 9, 1)].into_iter(),
            (slack + 10, u64::MAX),
            100.0,
            false,
            || 300.0,
            ARENA_SIZE,
        );
        assert_eq!(retry, [(ChunkPos::new(2, 0), true)]);
        assert_eq!(frontier, 100.0);
        // Nothing loaded farther out, so it waits.
        let (retry, _, fragmented) = columns_to_retry(
            [waiting(4.0, 2, 500)].into_iter(),
            free,
            100.0,
            false,
            || 1.0,
            ARENA_SIZE,
        );
        assert!(retry.is_empty());
        assert!(fragmented.is_empty());
    }

    /// Room enough in total but no gap for the largest section, and nothing
    /// farther to evict: compaction has to make it room.
    #[test]
    fn a_column_with_room_split_too_small_asks_for_compaction() {
        let split = Waiting {
            fits_an_arena: false,
            ..waiting(4.0, 2, 50)
        };
        let (retry, _, fragmented) = columns_to_retry(
            [split].into_iter(),
            (100, 150),
            100.0,
            false,
            || 1.0,
            ARENA_SIZE,
        );
        assert!(retry.is_empty());
        assert_eq!(fragmented, [ChunkPos::new(2, 0)]);
    }

    /// The evicting column's room, free in total but split too small for it,
    /// isn't handed to the column it evicted while it holds a claim, or the
    /// two evict each other for good.
    #[test]
    fn columns_past_the_frontier_wait_for_nearer_claims() {
        let slack = u64::from(ARENA_SIZE.vertices) / FRONTIER_SLACK;
        let free = (slack + 1000, u64::MAX);
        let evicted = || [waiting(200.0, 9, 100)].into_iter();
        // A claim from an earlier pass holds it back.
        let (retry, frontier, _) =
            columns_to_retry(evicted(), free, 100.0, true, || 300.0, ARENA_SIZE);
        assert!(retry.is_empty());
        assert_eq!(frontier, 100.0);
        // So does a nearer column handed back in this pass, though both fit.
        let both = [waiting(4.0, 2, 100), waiting(200.0, 9, 100)].into_iter();
        let (retry, ..) = columns_to_retry(both, free, 100.0, false, || 300.0, ARENA_SIZE);
        assert_eq!(retry, [(ChunkPos::new(2, 0), true)]);
        // And one nearer left short, with nothing farther to evict.
        let short = [waiting(4.0, 2, u64::MAX / 4), waiting(200.0, 9, 100)].into_iter();
        let (retry, ..) = columns_to_retry(short, free, 100.0, false, || 1.0, ARENA_SIZE);
        assert!(retry.is_empty());
        // Once nothing nearer holds it, it comes back without a claim.
        let (retry, ..) = columns_to_retry(evicted(), free, 100.0, false, || 300.0, ARENA_SIZE);
        assert_eq!(retry, [(ChunkPos::new(9, 0), false)]);
    }

    #[test]
    fn compaction_helps_while_an_arena_fits_the_others_free_room() {
        let size = ArenaSize::new(1000);
        let used = |vertices: u32| (vertices, vertices / 2 * 3);
        assert!(!compaction_can_help(&[used(100)], size));
        assert!(compaction_can_help(&[used(500), used(500)], size));
        // The emptiest holds more than the others have free.
        assert!(!compaction_can_help(
            &[used(880), used(880), used(940)],
            size
        ));
        assert!(compaction_can_help(
            &[used(100), used(880), used(940)],
            size
        ));
    }

    #[test]
    fn columns_with_nothing_to_place_always_come_back() {
        let pos = ChunkPos::new(3, 0);
        let empty = Waiting {
            dist: 9.0,
            pos,
            need: Need::default(),
            fits_an_arena: false,
        };
        let (retry, frontier_sq, _) = columns_to_retry(
            [empty].into_iter(),
            (0, 0),
            4.0,
            true,
            || 1.0,
            ArenaSize::new(4096),
        );
        assert_eq!(retry, [(pos, false)]);
        assert_eq!(frontier_sq, 4.0);
    }

    #[test]
    fn starved_columns_past_the_frontier_need_slack() {
        let slack = u64::from(ARENA_SIZE.vertices) / FRONTIER_SLACK;
        let free = (slack + 100, u64::MAX);
        let (retry, frontier, _) = columns_to_retry(
            [
                waiting(25.0, 5, 100),
                waiting(36.0, 6, 100),
                waiting(49.0, 7, 1),
            ]
            .into_iter(),
            free,
            16.0,
            false,
            || f64::INFINITY,
            ARENA_SIZE,
        );
        // The first fits with the slack left and moves the frontier past it;
        // the second doesn't, and nothing farther comes back.
        assert_eq!(retry, [(ChunkPos::new(5, 0), false)]);
        assert_eq!(frontier, 25.0f64.next_up());
    }

    #[test]
    fn compaction_drains_the_emptiest_once_the_rest_hold_it() {
        let quarter = ARENA_SIZE.vertices / 4;
        let drain = |live: &[(u16, u32, u32)], defragment| {
            arena_to_drain(live.iter().copied(), ARENA_SIZE, defragment)
        };
        assert_eq!(drain(&[(0, quarter, 0)], false), None);
        assert_eq!(
            drain(&[(0, quarter * 2, 0), (3, quarter / 2, 0)], false),
            Some(3)
        );
        assert_eq!(drain(&[(0, quarter * 3, 0), (3, quarter, 0)], false), None);
        // Fragmented room drains the emptiest whatever the fill.
        assert_eq!(
            drain(&[(0, quarter * 3, 0), (3, quarter, 0)], true),
            Some(3)
        );
        assert_eq!(drain(&[(0, quarter, 0)], true), None);
    }

    #[test]
    fn retrying_columns_that_fit_never_walks_the_loaded_ones() {
        let (retry, ..) = columns_to_retry(
            [waiting(1.0, 1, 50)].into_iter(),
            (100, 150),
            100.0,
            false,
            || panic!("asked for the farthest loaded column"),
            ARENA_SIZE,
        );
        assert_eq!(retry, [(ChunkPos::new(1, 0), true)]);
    }

    #[test]
    fn empty_arenas_but_one_are_doomed_unless_columns_wait() {
        let live = [(0, true), (2, false), (5, true), (7, true)];
        assert_eq!(arenas_to_doom(&live, false), [0, 5, 7]);
        assert_eq!(arenas_to_doom(&[(0, true), (1, true)], false), [0]);
        assert!(arenas_to_doom(&[(0, true)], false).is_empty());
        assert!(arenas_to_doom(&live, true).is_empty());
    }

    /// Fragmentation lasts while the column it starves waits, not while
    /// any column does.
    #[test]
    fn fragmentation_goes_with_its_column() {
        let (split, other) = (ChunkPos::new(1, 0), ChunkPos::new(2, 0));
        let mut waiting = Waitlist::default();
        waiting.starve(split, sized([0]));
        waiting.starve(other, sized([0]));
        waiting.fragmented.insert(split);
        assert!(waiting.is_fragmented());
        waiting.hand_back(split, true);
        assert!(!waiting.is_fragmented());

        waiting.fragmented.insert(other);
        waiting.forget(other);
        assert!(!waiting.is_fragmented());
    }

    fn heap_refs(heaps: &[(Tlsf, Tlsf)]) -> Vec<(&Tlsf, &Tlsf)> {
        heaps.iter().map(|(v, i)| (v, i)).collect()
    }

    /// An arena, and a vertex and an index offset in it.
    type Slot = (usize, u32, u32);

    /// A drawn section's slot and lens.
    type Placed = (usize, u32, u32, SliceLens);

    /// The store's uploads, eviction and retries without the GPU: arenas
    /// are bare heaps, retired slices free at once, and a handed-back column
    /// re-meshes by the next frame. Its choices go through the waitlist,
    /// eviction and retry functions the store's do. Not modelled: arenas
    /// doomed or taken back, the backoff after a failed add, and compaction
    /// with its drains, which the arena scripts cover.
    struct Sim {
        size: ArenaSize,
        max_arenas: usize,
        heaps: Vec<(Tlsf, Tlsf)>,
        /// Each loaded column's newest mesh, per section.
        meshes: HashMap<ChunkPos, HashMap<i32, SliceLens>>,
        drawn: HashMap<ChunkPos, HashMap<i32, Placed>>,
        /// Meshes due next frame: handed back, or new.
        pending: Vec<(ChunkPos, HashSet<i32>)>,
        waiting: Waitlist,
        eye: DVec3,
        retry_due: bool,
        frame: u64,
        handed_back: usize,
    }

    impl Sim {
        fn new(vertices: u32) -> Self {
            Self {
                size: ArenaSize::new(vertices),
                max_arenas: 2,
                heaps: Vec::new(),
                meshes: HashMap::new(),
                drawn: HashMap::new(),
                pending: Vec::new(),
                waiting: Waitlist::default(),
                eye: DVec3::new(8.0, 0.0, 8.0),
                retry_due: false,
                frame: 0,
                handed_back: 0,
            }
        }

        fn free_room(&self) -> (u64, u64) {
            heaps_free(heap_refs(&self.heaps).into_iter())
        }

        fn alloc_in(heaps: &mut [(Tlsf, Tlsf)], (v, i): SliceLens) -> Option<Slot> {
            heaps
                .iter_mut()
                .enumerate()
                .find_map(|(id, (hv, hi))| alloc_pair(hv, hi, v, i).map(|(vo, io)| (id, vo, io)))
        }

        fn alloc(&mut self, lens: SliceLens) -> Option<Slot> {
            if let Some(found) = Self::alloc_in(&mut self.heaps, lens) {
                return Some(found);
            }
            if self.heaps.len() >= self.max_arenas {
                return None;
            }
            self.heaps
                .push((Tlsf::new(self.size.vertices), Tlsf::new(self.size.indices)));
            Self::alloc_in(&mut self.heaps, lens)
        }

        fn free(&mut self, (id, vo, io): Slot) {
            self.heaps[id].0.free(vo);
            self.heaps[id].1.free(io);
        }

        fn can_defragment(&self) -> bool {
            heaps_can_defragment(&heap_refs(&self.heaps), self.size)
        }

        fn reclaim(&mut self, at: Slot) {
            self.free(at);
            self.retry_due = true;
        }

        fn upload(&mut self, pos: ChunkPos, accepted: &HashSet<i32>) {
            let Some(mesh) = self.meshes.get(&pos) else {
                return;
            };
            let mut lens: Vec<(i32, SliceLens)> = accepted
                .iter()
                .filter_map(|&si| mesh.get(&si).map(|&l| (si, l)))
                .filter(|&(_, (v, i))| v > 0 && i > 0)
                .collect();
            lens.sort_unstable();
            let unheld = self.waiting.settle(pos, accepted);
            let beyond = self.waiting.beyond(pos, Some(self.eye));
            let mut no_room = beyond;
            let mut plans = Vec::new();
            for &(si, l) in &lens {
                if no_room {
                    continue;
                }
                match self.alloc(l) {
                    Some(at) => plans.push((si, at, l)),
                    None => no_room = true,
                }
            }
            let need: Need = lens.iter().map(|&(_, l)| l).collect();
            if no_room && need.vertices > 0 {
                for (_, at, _) in plans {
                    self.free(at);
                }
                if beyond {
                    self.evict(pos);
                } else {
                    self.evict_beyond(pos, need);
                }
                let lens: HashMap<i32, SliceLens> = lens.into_iter().collect();
                self.waiting.starve(
                    pos,
                    accepted
                        .iter()
                        .map(|&si| (si, lens.get(&si).copied().unwrap_or_default())),
                );
                return;
            }
            self.waiting.stop_waiting(pos, accepted);
            self.retry_due |= unheld;
            let column = self.drawn.entry(pos).or_default();
            let old: Vec<_> = accepted.iter().filter_map(|si| column.remove(si)).collect();
            for (si, (id, vo, io), l) in plans {
                column.insert(si, (id, vo, io, l));
            }
            if column.is_empty() {
                self.drawn.remove(&pos);
            }
            for (id, vo, io, _) in old {
                self.reclaim((id, vo, io));
            }
        }

        fn evict(&mut self, pos: ChunkPos) {
            if let Some(sections) = self.drawn.remove(&pos) {
                for &(id, vo, io, _) in sections.values() {
                    self.reclaim((id, vo, io));
                }
                self.waiting
                    .evicted(pos, sections.iter().map(|(&si, s)| (si, s.3)));
            }
        }

        fn evict_beyond(&mut self, pos: ChunkPos, need: Need) {
            let dist = column_dist_sq(pos, self.eye);
            let loaded = self.drawn.iter().map(|(&p, sections)| {
                let need: Need = sections.values().map(|s| s.3).collect();
                (column_dist_sq(p, self.eye), p, need)
            });
            let (victims, freed) = eviction_victims(loaded, dist, need, self.free_room());
            let can_defragment = self.can_defragment();
            self.waiting
                .evicting_for(pos, dist, &victims, freed, can_defragment);
            for (_, victim) in victims {
                self.evict(victim);
            }
        }

        fn retry(&mut self) {
            let to_add = self.max_arenas.saturating_sub(self.heaps.len()) as u64;
            let heaps = heap_refs(&self.heaps);
            let eye = Some(self.eye);
            let drawn = &self.drawn;
            let handed = self.waiting.retry(eye, &heaps, to_add, self.size, || {
                farthest_from(drawn.keys().copied(), eye)
            });
            self.handed_back += handed.len();
            self.pending.extend(
                handed
                    .into_iter()
                    .map(|(pos, sections)| (pos, sections.into_iter().collect())),
            );
        }

        fn unload(&mut self, pos: ChunkPos) {
            self.meshes.remove(&pos);
            self.waiting.forget(pos);
            if let Some(sections) = self.drawn.remove(&pos) {
                for (id, vo, io, _) in sections.into_values() {
                    self.reclaim((id, vo, io));
                }
            }
        }

        fn move_eye(&mut self, eye: DVec3) {
            if !self.waiting.is_empty() && eye_column(self.eye) != eye_column(eye) {
                self.retry_due = true;
            }
            self.eye = eye;
        }

        /// One frame: the budget recheck, last frame's hand-backs and
        /// `meshes` upload, then the retry.
        fn frame(&mut self, meshes: Vec<(ChunkPos, HashSet<i32>)>) {
            self.frame += 1;
            if !self.waiting.is_empty()
                && self.frame.is_multiple_of(BUDGET_RECHECK_FRAMES)
                && self.heaps.len() < self.max_arenas
            {
                self.retry_due = true;
            }
            let mut batch = std::mem::take(&mut self.pending);
            batch.extend(meshes);
            for (pos, sections) in batch {
                self.upload(pos, &sections);
            }
            if std::mem::take(&mut self.retry_due) {
                self.retry();
            }
        }

        /// Runs frames until nothing is due, or `limit` of them.
        fn settle(&mut self, limit: u64) {
            for _ in 0..limit {
                let growing = !self.waiting.is_empty() && self.heaps.len() < self.max_arenas;
                if self.pending.is_empty() && !self.retry_due && !growing {
                    return;
                }
                self.frame(Vec::new());
            }
        }

        fn check(&self) {
            for (id, (hv, hi)) in self.heaps.iter().enumerate() {
                let (v, i) = self
                    .drawn
                    .values()
                    .flat_map(HashMap::values)
                    .filter(|s| s.0 == id)
                    .fold((0, 0), |(v, i), s| (v + s.3.0, i + s.3.1));
                assert_eq!((hv.used(), hi.used()), (v, i), "arena {id} accounting");
            }
            for pos in self
                .waiting
                .columns
                .keys()
                .chain(self.waiting.claims.keys())
            {
                assert!(self.meshes.contains_key(pos), "{pos:?} waits but unloaded");
            }
            if self.waiting.is_empty() {
                assert_eq!(self.waiting.frontier_sq, f64::INFINITY);
            }
            if self.pending.is_empty() {
                assert!(self.waiting.claims.is_empty(), "a claim outlived its mesh");
            }
            for (pos, starved) in &self.waiting.columns {
                assert!(
                    starved.need().vertices > 0,
                    "{pos:?} waits with nothing to place"
                );
            }
            for (&pos, mesh) in &self.meshes {
                let drawn = self.drawn.get(&pos);
                let waits = |si: &i32| {
                    self.waiting
                        .columns
                        .get(&pos)
                        .is_some_and(|s| s.sections.contains_key(si))
                        || self
                            .pending
                            .iter()
                            .any(|(p, sections)| *p == pos && sections.contains(si))
                };
                for (si, &(v, i)) in mesh {
                    let current = drawn.and_then(|d| d.get(si)).is_some_and(|s| s.3 == (v, i));
                    let empty = v == 0 || i == 0;
                    let shown = if empty {
                        drawn.is_none_or(|d| !d.contains_key(si))
                    } else {
                        current
                    };
                    assert!(shown || waits(si), "{pos:?} section {si} lost");
                }
            }
        }
    }

    /// Random loads, re-meshes, unloads, moves and budget changes never lose
    /// a section, leak room or loop, and with room for everything nothing
    /// waits.
    #[test]
    fn random_streaming_keeps_every_column_drawn_or_waiting() {
        const STEPS: usize = 300;
        for seed in 0..24 {
            let mut rng = fastrand::Rng::with_seed(seed);
            let mut sim = Sim::new(4096);
            let mesh = |rng: &mut fastrand::Rng| -> SliceLens {
                if rng.bool() {
                    (0, 0)
                } else {
                    let quads = rng.u32(1..=170);
                    (quads * 4, quads * 6)
                }
            };
            for _ in 0..STEPS {
                let eye_col = eye_column(sim.eye);
                let near = |rng: &mut fastrand::Rng| {
                    ChunkPos::new(eye_col.x + rng.i32(-6..=6), eye_col.z + rng.i32(-6..=6))
                };
                let loaded: Vec<ChunkPos> = sim.meshes.keys().copied().collect();
                let mut uploads = Vec::new();
                match rng.u32(0..100) {
                    0..35 => {
                        if let Entry::Vacant(slot) = sim.meshes.entry(near(&mut rng)) {
                            let sections: HashMap<i32, SliceLens> =
                                (0..24).map(|si| (si, mesh(&mut rng))).collect();
                            uploads.push((*slot.key(), sections.keys().copied().collect()));
                            slot.insert(sections);
                        }
                    }
                    35..60 if !loaded.is_empty() => {
                        let pos = loaded[rng.usize(..loaded.len())];
                        let edited: HashSet<i32> =
                            (0..rng.usize(1..=24)).map(|_| rng.i32(0..24)).collect();
                        for &si in &edited {
                            let lens = mesh(&mut rng);
                            sim.meshes.get_mut(&pos).unwrap().insert(si, lens);
                        }
                        uploads.push((pos, edited));
                    }
                    60..75 if !loaded.is_empty() => sim.unload(loaded[rng.usize(..loaded.len())]),
                    75..85 => {
                        let to = sim.eye
                            + DVec3::new(
                                f64::from(rng.i32(-48..=48)),
                                0.0,
                                f64::from(rng.i32(-48..=48)),
                            );
                        sim.move_eye(to);
                    }
                    _ => sim.max_arenas = rng.usize(1..=5).max(sim.heaps.len()),
                }
                sim.frame(uploads);
                sim.settle(600);
                sim.check();
            }
            sim.max_arenas = usize::MAX;
            sim.retry_due = true;
            sim.settle(100_000);
            sim.check();
            assert!(
                sim.waiting.is_empty(),
                "seed {seed}: columns wait with room for all"
            );
            // A column coming back again and again would outrun the steps.
            assert!(
                sim.handed_back < STEPS,
                "seed {seed}: {} hand-backs in {STEPS} steps",
                sim.handed_back
            );
        }
    }

    /// Sections of 10 vertices each.
    fn sized<const N: usize>(sections: [i32; N]) -> [(i32, SliceLens); N] {
        sections.map(|s| (s, (10, 15)))
    }

    #[test]
    fn a_column_needs_its_waiting_sections_newest_meshes() {
        let pos = ChunkPos::new(1, 0);
        let mut waiting = Waitlist::default();
        waiting.starve(pos, [(0, (10, 15)), (1, (20, 30))]);
        waiting.starve(pos, [(1, (5, 6)), (2, (40, 60))]);
        let need = waiting.columns[&pos].need();
        assert_eq!((need.vertices, need.max_vertices), (55, 40));
        waiting.stop_waiting(pos, &HashSet::from([2]));
        let need = waiting.columns[&pos].need();
        assert_eq!((need.vertices, need.max_vertices), (15, 10));
    }

    #[test]
    fn an_eviction_keeps_newer_waiting_sizes() {
        let pos = ChunkPos::new(1, 0);
        let mut waiting = Waitlist::default();
        waiting.starve(pos, [(3, (500, 750))]);
        waiting.evicted(pos, [(3, (20, 30)), (4, (10, 15))]);
        let need = waiting.columns[&pos].need();
        assert_eq!((need.vertices, need.max_vertices), (510, 500));
    }

    #[test]
    fn the_last_claim_settling_lets_the_rest_retry() {
        let (claimed, other) = (ChunkPos::new(1, 0), ChunkPos::new(2, 0));
        let mut waiting = Waitlist::default();
        waiting.starve(claimed, sized([0, 1]));
        waiting.starve(other, sized([0]));
        waiting.hand_back(claimed, true);
        assert!(!waiting.settle(claimed, &HashSet::from([0])));
        assert!(waiting.settle(claimed, &HashSet::from([1])));
    }

    #[test]
    fn placed_sections_leave_the_waitlist() {
        let pos = ChunkPos::new(1, 0);
        let mut waiting = Waitlist::default();
        waiting.starve(pos, sized([0, 1]));
        waiting.pull_frontier(0.0);
        waiting.fragmented.insert(pos);
        waiting.stop_waiting(pos, &HashSet::from([0]));
        assert!(waiting.is_fragmented());
        waiting.stop_waiting(pos, &HashSet::from([1]));
        assert!(waiting.is_empty());
        assert!(!waiting.is_fragmented());
        assert_eq!(waiting.frontier_sq, f64::INFINITY);
    }

    #[test]
    fn the_frontier_goes_once_nothing_waits() {
        let at = DVec3::new(8.0, 0.0, 8.0);
        let eye = Some(at);
        let (near, far) = (ChunkPos::new(1, 0), ChunkPos::new(4, 0));
        let mut waiting = Waitlist::default();
        assert!(!waiting.beyond(far, eye));

        waiting.starve(near, sized([0, 1]));
        waiting.starve(far, sized([0, 1]));
        waiting.pull_frontier(column_dist_sq(far, at));
        assert!(waiting.beyond(far, eye));
        assert!(!waiting.beyond(near, eye));
        assert!(!waiting.beyond(far, None));

        // One still waits, so the frontier holds.
        assert_eq!(waiting.hand_back(far, false).map(|s| s.len()), Some(2));
        assert!(waiting.beyond(far, eye));
        // Handed back with a claim, the last one holds it until a mesh of
        // every section it was handed back for reaches upload.
        assert!(waiting.hand_back(near, true).is_some());
        assert!(!waiting.is_empty());
        assert!(waiting.beyond(far, eye));
        waiting.settle(near, &HashSet::from([0]));
        assert!(waiting.beyond(far, eye));
        waiting.settle(near, &HashSet::from([1]));
        assert!(waiting.is_empty());
        assert!(!waiting.beyond(far, eye));

        // An unload of the last one drops it too, claim and all.
        waiting.starve(near, sized([0]));
        waiting.pull_frontier(0.0);
        waiting.hand_back(near, true);
        waiting.forget(near);
        assert!(waiting.is_empty());
        assert!(!waiting.beyond(far, eye));
    }
}
