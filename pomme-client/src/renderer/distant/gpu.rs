//! Distant terrain on the GPU (Voxy's `BasicSectionGeometryManager`,
//! `MDICSectionRenderer` and the node graph `NodeManager` keeps): built
//! sections' quads in one arena with a metadata record each, the models
//! they index, the tree walked each frame to pick which to draw
//! (`traversal_dev.comp`, on the CPU until the GPU traversal lands), and the
//! draw list the GPU builds from that pick.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::{DVec3, Mat4, Vec3, Vec4Swizzles};

use super::build::BuiltSection;
use super::colors::TINT_KINDS;
use super::models::{
    ATLAS_COLUMNS, ATLAS_ROWS, BlockModel, CAPACITY, MODEL_MIP_LEVELS, ModelTile, TILE_HEIGHT,
    TILE_WIDTH,
};
use super::section::{LEVELS, SIDE, SectionKey};
use super::worker::DistantUpdate;
use crate::position::ChunkPos;
use crate::renderer::camera::Camera;
use crate::renderer::chunk::buffer::DrawCommand;
use crate::renderer::chunk::tlsf::Tlsf;
use crate::renderer::rhi::IndirectDrawFeatures;
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, rhi, shader, util};

/// The arena's size: an eighth of the device's memory, within these bounds.
const MAX_ARENA_BYTES: u64 = 512 << 20;
const MIN_ARENA_BYTES: u64 = 64 << 20;
/// Quads the shared index buffer covers, the most a group can hold.
const QUADS_PER_DRAW: u32 = 1 << 16;
/// Sections the metadata buffer holds (Voxy's `maxSectionCount`).
const MAX_SECTIONS: u32 = 1 << 20;
/// Sections a frame's render list may hold (Voxy's `MAX_QUEUE_SIZE`).
const MAX_LISTED: usize = 200_000;
/// The draw list's regions, in draws (`MDICSectionRenderer`).
const OPAQUE_DRAWS: u32 = 400_000;
const TRANSLUCENT_DRAWS: u32 = 100_000;
const TEMPORAL_DRAWS: u32 = 100_000;
const TRANSLUCENT_OFFSET: u32 = OPAQUE_DRAWS;
const TEMPORAL_OFFSET: u32 = OPAQUE_DRAWS + TRANSLUCENT_DRAWS;
/// Distance buckets the translucent draws sort by (`TRANSLUCENT_WRITE_BASE`).
pub(crate) const TRANSLUCENT_BUCKETS: u32 = 1024;
/// Quads a frame's staging holds for the arena.
const STAGING_BYTES: u64 = 16 << 20;
/// Section records a frame's scatter list holds (`distant_scatter.comp`:
/// chunks of four indices and four records).
const SCATTER_RECORDS: usize = 16384;
const SCATTER_BYTES: u64 = (SCATTER_RECORDS / 4 * 80) as u64;
const MODELS_BYTES: u64 = (CAPACITY * size_of::<BlockModel>()) as u64;
const TINTS_BYTES: u64 = (TINT_BIOMES * TINT_KINDS * size_of::<u32>()) as u64;
/// A model tile's bytes, every level.
const TILE_BYTES: usize = {
    let mut bytes = 0;
    let mut level = 0;
    while level < MODEL_MIP_LEVELS {
        bytes += ((TILE_WIDTH >> level) * (TILE_HEIGHT >> level) * 4) as usize;
        level += 1;
    }
    bytes
};
/// Model tiles uploaded a frame; more wait for the next.
const TILES_PER_FRAME: usize = 256;
/// Biome tints per kind in the colour buffer ([`super::colors`]).
pub(crate) const TINT_BIOMES: usize = 512;
/// Voxy's `subDivisionSize`: a node is split while it covers more than this
/// many pixels squared.
const SUBDIVISION_SIZE: f32 = 64.0;
/// Meshes asked of the worker at once (Voxy's `TARGET_COUNT`).
const MAX_REQUESTS: usize = 4000;
/// How long a request waits before it's asked again.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// Voxy's far plane: 3000 chunks.
pub(crate) const DISTANT_FAR: f32 = 3000.0 * 16.0;
/// Drawn chunk sections the bound store starts with room for.
const BOUND_CAPACITY: u32 = 1 << 15;
/// The box's 12 triangles as corner bits (x, y, z), every face wound the
/// same way, so culling one side keeps the near faces or the far ones;
/// after the quads' indices in the shared index buffer.
const CUBE: [u32; 36] = [
    0, 2, 6, 6, 4, 0, 1, 5, 7, 7, 3, 1, 0, 4, 5, 5, 1, 0, 2, 3, 7, 7, 6, 2, 0, 1, 3, 3, 2, 0, 4, 6,
    7, 7, 5, 4,
];
pub(crate) const CUBE_FIRST_INDEX: u32 = QUADS_PER_DRAW * 6;

/// Byte sizes of the buffers the command generation passes bind.
pub(crate) const SECTIONS_BYTES: u64 = MAX_SECTIONS as u64 * 32;
pub(crate) const RENDER_LIST_BYTES: u64 = ((1 + MAX_LISTED) * size_of::<u32>()) as u64;
pub(crate) const COUNTS_BYTES: u64 = 64;
pub(crate) const DRAWS_BYTES: u64 =
    (OPAQUE_DRAWS + TRANSLUCENT_DRAWS + TEMPORAL_DRAWS) as u64 * size_of::<DrawCommand>() as u64;
pub(crate) const POSITIONS_BYTES: u64 = (MAX_LISTED * 8) as u64;
pub(crate) const VISIBILITY_BYTES: u64 = MAX_SECTIONS as u64 * 4;
pub(crate) const TRANSLUCENT_BYTES: u64 = (TRANSLUCENT_BUCKETS + TRANSLUCENT_DRAWS) as u64 * 4;
/// Byte offsets within the draw-count buffer (`distant_section.glsl`).
pub(crate) const COUNTS_OPAQUE: u64 = 12;
pub(crate) const COUNTS_TRANSLUCENT: u64 = 16;
pub(crate) const COUNTS_TEMPORAL: u64 = 20;
pub(crate) const COUNTS_CULL_DRAW: u64 = 24;

/// A section's quads in the arena and its record (`SectionMeta`).
#[derive(Clone, Copy)]
struct Uploaded {
    /// Its record's slot.
    id: u32,
    /// Its first quad.
    ptr: u32,
    quads: u32,
    child_existence: u8,
    /// The last frame drawn, for evicting the least recent.
    last_drawn: u64,
}

/// A section placed in the arena, waiting for a frame's staging to carry
/// its quads and record up.
struct Pending {
    key: SectionKey,
    uploaded: Uploaded,
    quads: Vec<u64>,
    record: [[u32; 4]; 2],
}

/// The draw list's regions.
#[derive(Clone, Copy)]
pub(crate) enum Region {
    Opaque,
    /// Sections the occlusion pass saw this frame but not the frame before,
    /// which the opaque region (built last frame) missed.
    Temporal,
    Translucent,
}

/// Where the player is and how far distant terrain draws.
#[derive(Clone, PartialEq)]
pub(crate) struct View {
    pub dimension: Arc<str>,
    /// The dimension's block range, so the tree spans its height.
    pub min_y: i32,
    pub height: i32,
    /// In blocks.
    pub radius: f64,
}

/// The buffers the command generation passes read and write.
#[derive(Clone, Copy)]
pub(crate) struct DrawBuffers<'a> {
    pub sections: &'a rhi::backend::Buffer,
    pub counts: &'a rhi::backend::Buffer,
    pub draws: &'a rhi::backend::Buffer,
    pub positions: &'a rhi::backend::Buffer,
    pub visibility: &'a rhi::backend::Buffer,
    pub translucent: &'a rhi::backend::Buffer,
}

pub(crate) struct DistantBuffers {
    layout: rhi::backend::BindGroupLayout,
    /// Set 1: the quads, models, tints and the model atlas.
    group: rhi::backend::BindGroup,
    /// Every section's quads (`BufferArena`).
    arena: rhi::backend::Buffer,
    quads: Tlsf,
    /// Every section's record, by id.
    sections_buffer: rhi::backend::Buffer,
    ids: Tlsf,
    index_buffer: rhi::backend::Buffer,
    /// Every section the renderer knows, empty ones too (`EMPTY_MESH`).
    sections: HashMap<SectionKey, Uploaded>,
    pending: VecDeque<Pending>,
    /// Per frame in flight: the arena's staging, the scatter list, and the
    /// render list the traversal writes.
    quad_staging: Vec<rhi::backend::Buffer>,
    scatter_staging: Vec<rhi::backend::Buffer>,
    scatter: util::ComputePass,
    render_lists: Vec<rhi::backend::Buffer>,
    /// The draw-count buffer, draw list, draw positions, section visibility
    /// and translucent buckets the GPU passes keep between frames.
    counts: rhi::backend::Buffer,
    draws: rhi::backend::Buffer,
    positions: rhi::backend::Buffer,
    visibility: rhi::backend::Buffer,
    translucent: rhi::backend::Buffer,
    indirect: IndirectDrawFeatures,
    /// Sections asked of the worker, and when.
    requested: HashMap<SectionKey, Instant>,
    /// Requests since the last [`Self::take_requests`].
    requests: Vec<SectionKey>,
    /// Sections dropped since the last [`Self::take_requests`], for the
    /// worker to stop keeping up to date.
    dropped: Vec<SectionKey>,
    /// The sections to draw this frame, from the last traversal.
    visible: Vec<(SectionKey, Uploaded)>,
    /// The ring of top-level sections the last traversal walked: its
    /// centre, radius and height range, so sections outside it are swept
    /// only when it moves.
    ring: Option<(i32, i32, i32, i32, i32)>,
    view: Option<View>,
    bounds: BoundStore,
    frame_seq: u64,
    /// Sections whose quads and ids are freed at the frame from which no
    /// in-flight frame can still draw them.
    pending_free: VecDeque<(u64, Uploaded)>,
    /// Every model's six faces and their mips, a tile each.
    model_atlas: rhi::backend::Texture,
    model_sampler: rhi::backend::Sampler,
    /// Every model's face data ([`BlockModel`]).
    models: rhi::backend::Buffer,
    /// Biome tints by kind, then biome.
    tints: rhi::backend::Buffer,
    /// Tiles waiting for [`Self::record_model_upload`].
    pending_tiles: VecDeque<ModelTile>,
    tile_staging: Vec<rhi::backend::Buffer>,
}

/// Set 1: the arena's quads, the models, the biome tints, the model atlas
/// and its sampler.
const LAYOUT: &[rhi::BindGroupLayoutEntry] = &[
    rhi::BindGroupLayoutEntry::storage(rhi::ShaderStages::VERTEX),
    rhi::BindGroupLayoutEntry::storage(rhi::ShaderStages::VERTEX),
    rhi::BindGroupLayoutEntry::storage(rhi::ShaderStages::VERTEX),
    rhi::BindGroupLayoutEntry::texture(rhi::ShaderStages::FRAGMENT),
    rhi::BindGroupLayoutEntry::sampler(rhi::ShaderStages::FRAGMENT),
];

/// `distant_scatter.comp`'s set: the frame's list and the records.
const SCATTER_LAYOUT: &[rhi::BindGroupLayoutEntry] = &[
    rhi::BindGroupLayoutEntry::storage(rhi::ShaderStages::COMPUTE),
    rhi::BindGroupLayoutEntry::storage(rhi::ShaderStages::COMPUTE),
];

impl DistantBuffers {
    pub fn new(device: &rhi::backend::Device, indirect: IndirectDrawFeatures) -> Self {
        // Voxy's `SharedIndexBuffer`: corner 1 leads both triangles, so it's
        // the provoking vertex the flat attributes come from; the cube after.
        let indices: Vec<u32> = (0..QUADS_PER_DRAW)
            .flat_map(|quad| [1, 2, 0, 1, 3, 2].map(|i| quad * 4 + i))
            .chain(CUBE)
            .collect();
        let model_atlas = rhi::backend::Texture::new(
            device,
            &rhi::TextureDesc {
                mip_levels: MODEL_MIP_LEVELS,
                ..util::sampled_texture_desc(
                    "distant_models",
                    ATLAS_COLUMNS * TILE_WIDTH,
                    ATLAS_ROWS * TILE_HEIGHT,
                )
            },
        );
        let atlas = model_atlas.handle();
        let host_storage = |name, bytes| {
            rhi::backend::Buffer::new(
                device,
                name,
                bytes,
                rhi::BufferUsages::STORAGE,
                rhi::MemoryLocation::Host,
            )
        };
        let device_buffer = |name, bytes, usage| {
            rhi::backend::Buffer::new(
                device,
                name,
                bytes,
                usage | rhi::BufferUsages::COPY_DST,
                rhi::MemoryLocation::Device,
            )
        };
        let arena_bytes = MAX_ARENA_BYTES
            .min(device.device_local_bytes / 8)
            .max(MIN_ARENA_BYTES)
            / 8
            * 8;
        let arena = device_buffer("distant_quads", arena_bytes, rhi::BufferUsages::STORAGE);
        let sections_buffer = device_buffer(
            "distant_sections",
            SECTIONS_BYTES,
            rhi::BufferUsages::STORAGE,
        );
        let counts = device_buffer(
            "distant_draw_counts",
            COUNTS_BYTES,
            rhi::BufferUsages::STORAGE | rhi::BufferUsages::INDIRECT,
        );
        let draws = device_buffer(
            "distant_draws",
            DRAWS_BYTES,
            rhi::BufferUsages::STORAGE | rhi::BufferUsages::INDIRECT,
        );
        let positions = device_buffer(
            "distant_draw_positions",
            POSITIONS_BYTES,
            rhi::BufferUsages::STORAGE,
        );
        let visibility = device_buffer(
            "distant_visibility",
            VISIBILITY_BYTES,
            rhi::BufferUsages::STORAGE,
        );
        let translucent = device_buffer(
            "distant_translucent_draws",
            TRANSLUCENT_BYTES,
            rhi::BufferUsages::STORAGE,
        );
        let models = host_storage("distant_block_models", MODELS_BYTES);
        let tints = host_storage("distant_biome_tints", TINTS_BYTES);
        device.submit(|encoder| {
            encoder.transition_textures(&[atlas_transition(
                atlas,
                rhi::TextureUse::Undefined,
                rhi::TextureUse::CopyDst,
            )]);
            encoder.clear_texture(atlas, MODEL_ATLAS_RANGE, [0.0; 4]);
            encoder.transition_textures(&[atlas_transition(
                atlas,
                rhi::TextureUse::CopyDst,
                rhi::TextureUse::ShaderRead,
            )]);
            // Nothing seen, nothing to draw, before the first frame.
            encoder.fill_buffer(visibility.handle(), 0, VISIBILITY_BYTES, 0);
            encoder.fill_buffer(counts.handle(), 0, COUNTS_BYTES, 0);
        });
        // Voxy's `ModelStore` sampler.
        let model_sampler = rhi::backend::create_sampler(
            device,
            &rhi::SamplerDesc {
                mipmap_filter: rhi::FilterMode::Linear,
                lod_max_clamp: (MODEL_MIP_LEVELS - 1) as f32,
                ..rhi::SamplerDesc::NEAREST
            },
        );
        let layout = rhi::backend::create_bind_group_layout(device, LAYOUT);
        let group = device.create_bind_group(
            layout,
            &[
                (
                    0,
                    rhi::backend::BindingResource::storage(&arena, arena_bytes),
                ),
                (
                    1,
                    rhi::backend::BindingResource::storage(&models, MODELS_BYTES),
                ),
                (
                    2,
                    rhi::backend::BindingResource::storage(&tints, TINTS_BYTES),
                ),
                (3, rhi::backend::BindingResource::Texture(atlas)),
                (4, rhi::backend::BindingResource::Sampler(model_sampler)),
            ],
        );
        let scatter_staging = util::create_per_frame_host_buffers(
            device,
            SCATTER_BYTES,
            rhi::BufferUsages::STORAGE,
            "distant_scatter",
        );
        // The record count, as its push constant.
        let scatter = util::ComputePass::new(
            device,
            "distant scatter",
            shader::include_shader!("distant_scatter.comp"),
            SCATTER_LAYOUT,
            Some(rhi::PushConstants {
                visibility: rhi::ShaderStages::COMPUTE,
                size: 4,
            }),
            MAX_FRAMES_IN_FLIGHT,
            |frame| {
                vec![
                    (
                        0,
                        rhi::backend::BindingResource::storage(
                            &scatter_staging[frame],
                            SCATTER_BYTES,
                        ),
                    ),
                    (
                        1,
                        rhi::backend::BindingResource::storage(&sections_buffer, SECTIONS_BYTES),
                    ),
                ]
            },
        );
        Self {
            layout,
            group,
            quads: Tlsf::new((arena_bytes / 8) as u32),
            arena,
            sections_buffer,
            ids: Tlsf::new(MAX_SECTIONS),
            index_buffer: rhi::backend::Buffer::with_data(
                device,
                "distant_quad_indices",
                bytemuck::cast_slice(&indices),
                rhi::BufferUsages::INDEX,
            ),
            sections: HashMap::new(),
            pending: VecDeque::new(),
            quad_staging: util::create_per_frame_host_buffers(
                device,
                STAGING_BYTES,
                rhi::BufferUsages::COPY_SRC,
                "distant_quad_staging",
            ),
            scatter_staging,
            scatter,
            render_lists: util::create_per_frame_host_buffers(
                device,
                RENDER_LIST_BYTES,
                rhi::BufferUsages::STORAGE,
                "distant_render_list",
            ),
            counts,
            draws,
            positions,
            visibility,
            translucent,
            indirect,
            requested: HashMap::new(),
            requests: Vec::new(),
            dropped: Vec::new(),
            visible: Vec::new(),
            ring: None,
            view: None,
            bounds: BoundStore::new(device),
            frame_seq: 0,
            pending_free: VecDeque::new(),
            model_atlas,
            model_sampler,
            models,
            tints,
            pending_tiles: VecDeque::new(),
            tile_staging: util::create_per_frame_host_buffers(
                device,
                (TILE_BYTES * TILES_PER_FRAME) as u64,
                rhi::BufferUsages::COPY_SRC,
                "distant_model_staging",
            ),
        }
    }

    pub fn layout(&self) -> rhi::backend::BindGroupLayout {
        self.layout
    }

    /// Whether the GPU can build and issue the draw list.
    pub fn supported(&self) -> bool {
        self.indirect.count && self.indirect.multi_draw
    }

    pub fn draw_buffers(&self) -> DrawBuffers<'_> {
        DrawBuffers {
            sections: &self.sections_buffer,
            counts: &self.counts,
            draws: &self.draws,
            positions: &self.positions,
            visibility: &self.visibility,
            translucent: &self.translucent,
        }
    }

    pub fn render_list(&self, frame: usize) -> &rhi::backend::Buffer {
        &self.render_lists[frame]
    }

    /// The quads' indices, then the cube's from [`CUBE_FIRST_INDEX`].
    pub fn index_buffer(&self) -> rhi::backend::BufferHandle {
        self.index_buffer.handle()
    }

    /// The frame's id as the GPU passes see it.
    pub fn frame_id(&self) -> u32 {
        self.frame_seq as u32 & 0x7FFF_FFFF
    }

    /// Writes the waiting model tiles into the atlas, as many as fit this
    /// frame's staging.
    pub fn record_model_upload(&mut self, encoder: &rhi::backend::CommandEncoder, frame: usize) {
        if self.pending_tiles.is_empty() {
            return;
        }
        let staging = &mut self.tile_staging[frame];
        let mut copies = Vec::new();
        let mut offset = 0;
        for tile in self
            .pending_tiles
            .drain(..TILES_PER_FRAME.min(self.pending_tiles.len()))
        {
            let (x, y) = (
                u32::from(tile.id) % ATLAS_COLUMNS * TILE_WIDTH,
                u32::from(tile.id) / ATLAS_COLUMNS * TILE_HEIGHT,
            );
            for (level, pixels) in (0..).zip(&tile.levels) {
                staging.write_at(offset, pixels);
                copies.push(rhi::TexelCopy {
                    buffer_offset: offset as u64,
                    mip_level: level,
                    layer: 0,
                    origin: [x >> level, y >> level],
                    width: TILE_WIDTH >> level,
                    height: TILE_HEIGHT >> level,
                });
                offset += pixels.len();
            }
        }
        let atlas = self.model_atlas.handle();
        encoder.transition_textures(&[atlas_transition(
            atlas,
            rhi::TextureUse::ShaderRead,
            rhi::TextureUse::CopyDst,
        )]);
        encoder.copy_buffer_to_texture(staging.handle(), atlas, &copies);
        encoder.transition_textures(&[atlas_transition(
            atlas,
            rhi::TextureUse::CopyDst,
            rhi::TextureUse::ShaderRead,
        )]);
    }

    /// Carries the pending sections up: their quads into the arena through
    /// `frame`'s staging and their records through its scatter list
    /// (`BasicSectionGeometryManager.tick`); as many as fit, the rest next
    /// frame. A section is known to the traversal from here on.
    pub fn record_uploads(&mut self, encoder: &rhi::backend::CommandEncoder, frame: usize) {
        let mut copies = Vec::new();
        let mut records: Vec<(u32, [u32; 4])> = Vec::new();
        let mut used = 0;
        while let Some(next) = self.pending.front() {
            let bytes = next.quads.len() * 8;
            if used + bytes > STAGING_BYTES as usize || records.len() + 2 > SCATTER_RECORDS {
                break;
            }
            let Pending {
                key,
                uploaded,
                quads,
                record,
            } = self.pending.pop_front().expect("peeked");
            if bytes > 0 {
                self.quad_staging[frame].write_at(used, bytemuck::cast_slice(&quads));
                copies.push(rhi::BufferCopy {
                    src_offset: used as u64,
                    dst_offset: u64::from(uploaded.ptr) * 8,
                    size: bytes as u64,
                });
                used += bytes;
            }
            for (i, half) in (0..).zip(record) {
                records.push((uploaded.id * 2 + i, half));
            }
            self.requested.remove(&key);
            self.retire(key);
            self.sections.insert(key, uploaded);
        }
        if !copies.is_empty() {
            encoder.copy_buffer(
                self.quad_staging[frame].handle(),
                self.arena.handle(),
                &copies,
            );
            encoder.transition_buffers(rhi::BufferUse::CopyDst, rhi::BufferUse::ShaderRead);
        }
        if records.is_empty() {
            return;
        }
        // Chunks of four indices, then their four records; the count stops
        // the shader short of a last chunk's padding.
        let mut list: Vec<[u32; 4]> = Vec::with_capacity(records.len().div_ceil(4) * 5);
        for chunk in records.chunks(4) {
            let mut indices = [0; 4];
            let mut data = [[0; 4]; 4];
            for (i, &(index, record)) in chunk.iter().enumerate() {
                indices[i] = index;
                data[i] = record;
            }
            list.push(indices);
            list.extend(data);
        }
        self.scatter_staging[frame].write_at(0, bytemuck::cast_slice(&list));
        let count = records.len() as u32;
        encoder.dispatch(
            &self.scatter.pipeline,
            self.scatter.set(frame),
            &count.to_le_bytes(),
            [count.div_ceil(128), 1, 1],
        );
        encoder.transition_buffers(rhi::BufferUse::StorageWrite, rhi::BufferUse::Storage);
        encoder.transition_buffers(rhi::BufferUse::StorageWrite, rhi::BufferUse::ShaderRead);
    }

    /// Writes this frame's render list: the visible sections' ids.
    pub fn write_render_list(&mut self, frame: usize) {
        let mut list: Vec<u32> = Vec::with_capacity(self.visible.len() + 1);
        list.push(0);
        list.extend(
            self.visible
                .iter()
                .filter(|(_, section)| section.quads > 0)
                .map(|(_, section)| section.id)
                .take(MAX_LISTED),
        );
        list[0] = list.len() as u32 - 1;
        self.render_lists[frame].write_at(0, bytemuck::cast_slice(&list));
    }

    pub fn bounds(&self) -> &BoundStore {
        &self.bounds
    }

    /// Reclaims the ranges and buffers no in-flight frame reads any more.
    /// Call once a frame, after its fence wait.
    pub fn begin_frame(&mut self, device: &rhi::backend::Device) {
        self.frame_seq += 1;
        self.bounds.free_retired(device, self.frame_seq);
        while let Some(&(at, section)) = self.pending_free.front()
            && at <= self.frame_seq
        {
            self.pending_free.pop_front();
            self.ids.free(section.id);
            if section.quads > 0 {
                self.quads.free(section.ptr);
            }
        }
    }

    /// Frees `section`'s quads and id once no frame draws them.
    fn free_later(&mut self, section: Uploaded) {
        let at = self.frame_seq + MAX_FRAMES_IN_FLIGHT as u64;
        self.pending_free.push_back((at, section));
    }

    /// Forgets `key`'s section.
    fn retire(&mut self, key: SectionKey) {
        if let Some(old) = self.sections.remove(&key) {
            self.free_later(old);
        }
    }

    /// Drops every section, as on a dimension change. The bounds stay: they
    /// track the chunk store, which [`Self::clear_bounds`] follows.
    pub fn clear(&mut self) {
        let keys: Vec<SectionKey> = self.sections.keys().copied().collect();
        for key in keys {
            self.retire(key);
        }
        let pending: Vec<Uploaded> = self.pending.drain(..).map(|p| p.uploaded).collect();
        for section in pending {
            self.free_later(section);
        }
        self.requested.clear();
        self.requests.clear();
        self.dropped.clear();
        self.visible.clear();
        self.ring = None;
        self.view = None;
    }

    pub fn clear_bounds(&mut self) {
        self.bounds.clear();
    }

    /// Which of column `pos`'s sections draw (bit `i` for the `i`th from
    /// the dimension's bottom, `min_section`), bounding distant terrain
    /// there.
    pub fn set_column_sections(
        &mut self,
        device: &rhi::backend::Device,
        pos: ChunkPos,
        sections: u32,
        min_section: i32,
    ) {
        self.bounds
            .set_column(device, self.frame_seq, pos, sections, min_section);
    }

    /// Where the player is and how far to draw; `None` draws nothing.
    pub fn set_view(&mut self, view: Option<View>) {
        if self.view != view {
            if view.as_ref().is_none_or(|v| {
                self.view
                    .as_ref()
                    .is_some_and(|last| last.dimension != v.dimension)
            }) {
                self.clear();
            }
            self.view = view;
        }
    }

    /// Room for `count` quads, else made by dropping the sections drawn
    /// least recently, whose parents draw in their place.
    fn alloc(&mut self, count: u32) -> Option<u32> {
        loop {
            if let Some(ptr) = self.quads.alloc(count) {
                return Some(ptr);
            }
            // Voxy's `NodeCleaner`: the least recently drawn go, but never a
            // top-level section, which nothing stands in for.
            let mut stale: Vec<(u64, SectionKey)> = self
                .sections
                .iter()
                .filter(|(key, section)| section.quads > 0 && key.level != LEVELS - 1)
                .map(|(key, section)| (section.last_drawn, *key))
                .collect();
            if stale.is_empty() {
                return None;
            }
            stale.sort_unstable_by_key(|&(frame, _)| frame);
            let mut freed = 0;
            for &(_, key) in &stale {
                if freed >= count {
                    break;
                }
                freed += self.sections[&key].quads;
                self.retire(key);
                self.dropped.push(key);
            }
            // Freed ranges come back a few frames on; the mesh waits.
            if freed < count || self.pending_free.iter().any(|&(at, _)| at > self.frame_seq) {
                return None;
            }
        }
    }

    /// Takes the worker's updates. A section with no room yet is dropped
    /// and asked for again once there is.
    pub fn upload(&mut self, updates: impl IntoIterator<Item = DistantUpdate>) {
        for update in updates {
            if self
                .view
                .as_ref()
                .is_none_or(|view| view.dimension != update.dimension)
            {
                continue;
            }
            if let Some(tints) = update.tints {
                self.tints.write_at(0, bytemuck::cast_slice(&tints));
            }
            for tile in update.tiles {
                self.models.write_at(
                    usize::from(tile.id) * size_of::<BlockModel>(),
                    bytemuck::bytes_of(&tile.model),
                );
                self.pending_tiles.push_back(tile);
            }
            for section in update.sections {
                self.place(section);
            }
        }
    }

    /// Places `section` in the arena and queues its upload
    /// (`uploadReplaceSection`).
    fn place(&mut self, section: BuiltSection) {
        let count = section.quads.len() as u32;
        let ptr = if count == 0 {
            0
        } else if let Some(ptr) = self.alloc(count) {
            ptr
        } else {
            return;
        };
        let Some(id) = self.ids.alloc(1) else {
            if count > 0 {
                self.quads.free(ptr);
            }
            return;
        };
        let record = section_record(&section, ptr);
        self.pending.push_back(Pending {
            key: section.key,
            uploaded: Uploaded {
                id,
                ptr,
                quads: count,
                child_existence: section.child_existence,
                last_drawn: self.frame_seq,
            },
            quads: section.quads,
            record,
        });
    }

    /// The sections asked for since last time, and those dropped.
    pub fn take_requests(&mut self) -> (Vec<SectionKey>, Vec<SectionKey>) {
        (
            std::mem::take(&mut self.requests),
            std::mem::take(&mut self.dropped),
        )
    }

    fn request(&mut self, key: SectionKey, now: Instant) {
        if self.requested.len() >= MAX_REQUESTS {
            return;
        }
        let stale = self
            .requested
            .get(&key)
            .is_none_or(|&at| now.duration_since(at) > REQUEST_TIMEOUT);
        if stale {
            self.requested.insert(key, now);
            self.requests.push(key);
        }
    }

    /// Whether every child `key` has is known, so the traversal can step
    /// down into them (Voxy's child request finishing).
    fn children_ready(&self, key: SectionKey, existence: u8) -> bool {
        children(key, existence).all(|child| self.sections.contains_key(&child))
    }

    /// Draws `key` this frame.
    fn show(&mut self, key: SectionKey, section: Uploaded) {
        self.sections
            .get_mut(&key)
            .expect("a copy of a known section")
            .last_drawn = self.frame_seq;
        self.visible.push((key, section));
    }

    /// Whether the finest section holding block column `x`/`z` is uploaded
    /// with something in it, at any height.
    #[cfg(feature = "test-harness")]
    pub fn has_column(&self, x: i32, z: i32) -> bool {
        self.sections.iter().any(|(key, section)| {
            key.level == 0
                && key.x == x.div_euclid(SIDE as i32)
                && key.z == z.div_euclid(SIDE as i32)
                && section.quads > 0
        })
    }

    /// `traversal_dev.comp`: walks the tree from the top-level sections
    /// around `eye`, down into every section that covers more than
    /// [`SUBDIVISION_SIZE`]² pixels and whose children are all known, asking
    /// the worker for what's missing; collects what to draw into
    /// `visible` and drops what the ring left behind.
    pub fn traverse(&mut self, camera: &Camera, eye: DVec3, viewport: [u32; 2]) {
        self.visible.clear();
        let Some(view) = self.view.clone() else {
            return;
        };
        let now = Instant::now();
        let view_proj = camera.distant_view_projection(DISTANT_FAR);
        let planes = Camera::planes_from_view_projection(view_proj);
        let min_sss = SUBDIVISION_SIZE * SUBDIVISION_SIZE / (viewport[0] * viewport[1]) as f32;
        let render_distance = view.radius * view.radius;

        // `RenderDistanceTracker`: a ring of top-level sections around the
        // camera, over the dimension's height.
        let top = LEVELS - 1;
        let top_width = (SIDE as i32) << top;
        let ring = (view.radius / f64::from(top_width)).ceil() as i32 + 1;
        let (cx, cz) = (
            (eye.x.floor() as i32).div_euclid(top_width),
            (eye.z.floor() as i32).div_euclid(top_width),
        );
        let (min_sec, max_sec) = (
            view.min_y.div_euclid(top_width),
            (view.min_y + view.height - 1).div_euclid(top_width),
        );
        let mut queue: VecDeque<SectionKey> = VecDeque::new();
        for x in cx - ring..=cx + ring {
            for z in cz - ring..=cz + ring {
                for y in min_sec..=max_sec {
                    queue.push_back(SectionKey {
                        level: top,
                        x,
                        y,
                        z,
                    });
                }
            }
        }
        if self.ring != Some((cx, cz, ring, min_sec, max_sec)) {
            self.ring = Some((cx, cz, ring, min_sec, max_sec));
            let roots: HashSet<SectionKey> = queue.iter().copied().collect();
            let gone: Vec<SectionKey> = self
                .sections
                .keys()
                .copied()
                .filter(|key| !roots.contains(&key.ancestor(top)))
                .collect();
            for key in gone {
                self.retire(key);
                self.dropped.push(key);
            }
            self.requested
                .retain(|key, _| roots.contains(&key.ancestor(top)));
        }

        while let Some(key) = queue.pop_front() {
            let min = origin(&key) - eye;
            let size = f64::from(key.width());
            let max = min + size;
            // `isWithinRenderDistance`: the nearest point's distance along
            // x/z.
            let near = |lo: f64, hi: f64| lo.max(0.0) + hi.min(0.0);
            let (nx, nz) = (near(min.x, max.x), near(min.z, max.z));
            if nx * nx + nz * nz > render_distance {
                continue;
            }
            if !in_frustum(&planes, min.as_vec3(), max.as_vec3()) {
                continue;
            }
            let section = self.sections.get(&key).copied();
            if key.level != 0 && screen_size(view_proj, min.as_vec3(), size as f32) > min_sss {
                if let Some(section) = section
                    && self.children_ready(key, section.child_existence)
                {
                    queue.extend(children(key, section.child_existence));
                } else {
                    match section {
                        Some(section) => {
                            for child in children(key, section.child_existence) {
                                if !self.sections.contains_key(&child) {
                                    self.request(child, now);
                                }
                            }
                        }
                        None => self.request(key, now),
                    }
                    // Draw this one meanwhile, if it's wholly within reach
                    // (`furthestPointToCamera`, with Voxy's buffer room).
                    let far = |lo: f64, hi: f64| {
                        if lo > 0.0 {
                            hi
                        } else if hi < 0.0 {
                            lo
                        } else {
                            lo.abs().max(hi)
                        }
                    };
                    let (fx, fz) = (far(min.x, max.x), far(min.z, max.z));
                    if let Some(section) = section
                        && fx * fx + fz * fz + 16.0 * 16.0 * 16.0 <= render_distance
                    {
                        self.show(key, section);
                    }
                }
            } else if let Some(section) = section {
                self.show(key, section);
            } else {
                self.request(key, now);
            }
        }
    }

    /// Issues one region of the draw list the GPU built
    /// (`renderTerrain`): an indirect draw whose count the generation
    /// wrote. The caller has bound the matching distant pipeline and its
    /// sets.
    pub fn draw(&self, pass: &rhi::backend::RenderPassEncoder, region: Region) {
        let (first, count_offset, max_draws) = match region {
            Region::Opaque => (0, COUNTS_OPAQUE, OPAQUE_DRAWS),
            Region::Temporal => (TEMPORAL_OFFSET, COUNTS_TEMPORAL, TEMPORAL_DRAWS),
            Region::Translucent => (TRANSLUCENT_OFFSET, COUNTS_TRANSLUCENT, TRANSLUCENT_DRAWS),
        };
        pass.set_bind_group(1, self.group);
        pass.set_index_buffer(self.index_buffer.handle());
        let stride = size_of::<DrawCommand>() as u32;
        pass.draw_indexed_indirect_count(
            self.draws.handle(),
            u64::from(first * stride),
            self.counts.handle(),
            count_offset,
            max_draws,
            stride,
        );
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        self.group.destroy(device);
        for buffer in [
            &mut self.arena,
            &mut self.sections_buffer,
            &mut self.index_buffer,
            &mut self.counts,
            &mut self.draws,
            &mut self.positions,
            &mut self.visibility,
            &mut self.translucent,
            &mut self.models,
            &mut self.tints,
        ] {
            std::mem::take(buffer).destroy(device);
        }
        util::destroy_buffers(device, &mut self.quad_staging);
        util::destroy_buffers(device, &mut self.scatter_staging);
        util::destroy_buffers(device, &mut self.render_lists);
        self.scatter.destroy(device);
        self.bounds.destroy(device);
        util::destroy_buffers(device, &mut self.tile_staging);
        self.model_sampler.destroy(device);
        std::mem::take(&mut self.model_atlas).destroy(device);
        self.layout.destroy(device);
    }
}

/// Voxy's `ChunkBoundStore`: the drawn chunk sections' coordinates for the
/// bound pass, a slot each in a buffer the pass instances over, the last
/// slot moving into a freed one.
pub(crate) struct BoundStore {
    layout: rhi::backend::BindGroupLayout,
    buffer: rhi::backend::Buffer,
    group: rhi::backend::BindGroup,
    capacity: u32,
    slots: Vec<[i32; 3]>,
    index: HashMap<[i32; 3], u32>,
    /// Per column, which of its sections have a slot.
    columns: HashMap<ChunkPos, u32>,
    /// Buffers in-flight frames may still read, by the frame they're free
    /// from.
    retired: VecDeque<(u64, rhi::backend::Buffer, rhi::backend::BindGroup)>,
}

impl BoundStore {
    fn new(device: &rhi::backend::Device) -> Self {
        let layout = rhi::backend::create_bind_group_layout(
            device,
            &[rhi::BindGroupLayoutEntry::storage(
                rhi::ShaderStages::VERTEX,
            )],
        );
        let (buffer, group) = Self::make_buffer(device, layout, BOUND_CAPACITY);
        Self {
            layout,
            buffer,
            group,
            capacity: BOUND_CAPACITY,
            slots: Vec::new(),
            index: HashMap::new(),
            columns: HashMap::new(),
            retired: VecDeque::new(),
        }
    }

    fn make_buffer(
        device: &rhi::backend::Device,
        layout: rhi::backend::BindGroupLayout,
        capacity: u32,
    ) -> (rhi::backend::Buffer, rhi::backend::BindGroup) {
        let bytes = u64::from(capacity) * 16;
        let buffer = rhi::backend::Buffer::new(
            device,
            "distant_chunk_bounds",
            bytes,
            rhi::BufferUsages::STORAGE,
            rhi::MemoryLocation::Host,
        );
        let group = device.create_bind_group(
            layout,
            &[(0, rhi::backend::BindingResource::storage(&buffer, bytes))],
        );
        (buffer, group)
    }

    pub fn layout(&self) -> rhi::backend::BindGroupLayout {
        self.layout
    }

    pub fn group(&self) -> rhi::backend::BindGroup {
        self.group
    }

    pub fn count(&self) -> u32 {
        self.slots.len() as u32
    }

    fn write(&mut self, slot: u32) {
        let [x, y, z] = self.slots[slot as usize];
        self.buffer
            .write_at(slot as usize * 16, bytemuck::bytes_of(&[x, y, z, 0]));
    }

    fn set_column(
        &mut self,
        device: &rhi::backend::Device,
        frame_seq: u64,
        pos: ChunkPos,
        sections: u32,
        min_section: i32,
    ) {
        let was = self.columns.get(&pos).copied().unwrap_or(0);
        if was == sections {
            return;
        }
        for bit in 0..32 {
            let (had, has) = (was & 1 << bit != 0, sections & 1 << bit != 0);
            let key = [pos.x, min_section + bit, pos.z];
            if had && !has {
                self.remove(key);
            } else if has && !had {
                self.add(device, frame_seq, key);
            }
        }
        if sections == 0 {
            self.columns.remove(&pos);
        } else {
            self.columns.insert(pos, sections);
        }
    }

    fn add(&mut self, device: &rhi::backend::Device, frame_seq: u64, key: [i32; 3]) {
        if self.count() == self.capacity {
            self.capacity *= 2;
            let (buffer, group) = Self::make_buffer(device, self.layout, self.capacity);
            let old = (
                frame_seq + MAX_FRAMES_IN_FLIGHT as u64,
                std::mem::replace(&mut self.buffer, buffer),
                std::mem::replace(&mut self.group, group),
            );
            self.retired.push_back(old);
            for slot in 0..self.count() {
                self.write(slot);
            }
        }
        let slot = self.count();
        self.slots.push(key);
        self.index.insert(key, slot);
        self.write(slot);
    }

    fn remove(&mut self, key: [i32; 3]) {
        let Some(slot) = self.index.remove(&key) else {
            return;
        };
        let last = self.slots.pop().expect("a slot was indexed");
        if slot < self.count() {
            self.slots[slot as usize] = last;
            self.index.insert(last, slot);
            self.write(slot);
        }
    }

    fn clear(&mut self) {
        self.slots.clear();
        self.index.clear();
        self.columns.clear();
    }

    fn free_retired(&mut self, device: &rhi::backend::Device, frame_seq: u64) {
        while let Some(&(at, ..)) = self.retired.front()
            && at <= frame_seq
        {
            let (_, buffer, group) = self.retired.pop_front().expect("peeked");
            group.destroy(device);
            buffer.destroy(device);
        }
    }

    fn destroy(&mut self, device: &rhi::backend::Device) {
        self.free_retired(device, u64::MAX);
        self.group.destroy(device);
        std::mem::take(&mut self.buffer).destroy(device);
        self.layout.destroy(device);
    }
}

/// `SectionMeta.writeMetadata`: the key high word first, the bounds, the
/// first quad `ptr`; then the eight groups' quad counts in `u16` pairs.
fn section_record(section: &BuiltSection, ptr: u32) -> [[u32; 4]; 2] {
    let key = section.key.pack();
    let count = section.quads.len() as u32;
    let group_count = |group: usize| {
        let end = section.offsets.get(group + 1).copied().unwrap_or(count);
        end - section.offsets[group]
    };
    let pair = |group: usize| group_count(group) | group_count(group + 1) << 16;
    [
        [(key >> 32) as u32, key as u32, section.aabb, ptr],
        [pair(0), pair(2), pair(4), pair(6)],
    ]
}

/// The block position of `key`'s corner.
fn origin(key: &SectionKey) -> DVec3 {
    let [x, y, z] = key.origin();
    DVec3::new(f64::from(x), f64::from(y), f64::from(z))
}

/// `key`'s children that `existence` has something in.
fn children(key: SectionKey, existence: u8) -> impl Iterator<Item = SectionKey> {
    (0..8u8)
        .filter(move |&i| existence & 1 << i != 0)
        .map(move |i| SectionKey {
            level: key.level - 1,
            x: key.x * 2 + i32::from(i & 1),
            z: key.z * 2 + i32::from(i >> 1 & 1),
            y: key.y * 2 + i32::from(i >> 2),
        })
}

/// Every level of the model atlas.
const MODEL_ATLAS_RANGE: rhi::TextureRange = rhi::TextureRange {
    mips: MODEL_MIP_LEVELS,
    ..rhi::TextureRange::BASE
};

fn atlas_transition(
    atlas: rhi::backend::TextureHandle,
    from: rhi::TextureUse,
    to: rhi::TextureUse,
) -> rhi::backend::TextureTransition {
    rhi::backend::TextureTransition {
        texture: atlas,
        range: MODEL_ATLAS_RANGE,
        from,
        to,
    }
}

/// Whether the box between `min` and `max` (eye-relative) is at least
/// partly inside `planes`.
fn in_frustum(planes: &[[f32; 4]; 6], min: Vec3, max: Vec3) -> bool {
    planes.iter().all(|p| {
        let corner = |n: f32, lo: f32, hi: f32| if n >= 0.0 { hi } else { lo };
        p[0] * corner(p[0], min.x, max.x)
            + p[1] * corner(p[1], min.y, max.y)
            + p[2] * corner(p[2], min.z, max.z)
            + p[3]
            >= 0.0
    })
}

/// `screenspace.glsl`'s screen size: half the summed areas of the cube's
/// projected faces, in units of the viewport.
fn screen_size(view_proj: Mat4, min: Vec3, size: f32) -> f32 {
    let project = |p: Vec3| {
        let clip = view_proj * p.extend(1.0);
        // NDC to the viewport's 0..1.
        (clip.xy() / clip.w) * 0.5 + 0.5
    };
    let p = |dx: f32, dy: f32, dz: f32| project(min + Vec3::new(dx, dy, dz) * size);
    let cross = |a: glam::Vec2, b: glam::Vec2| (a.x * b.y - b.x * a.y).abs();
    let (p000, p100, p010, p001) = (
        p(0.0, 0.0, 0.0),
        p(1.0, 0.0, 0.0),
        p(0.0, 1.0, 0.0),
        p(0.0, 0.0, 1.0),
    );
    let (p111, p011, p101, p110) = (
        p(1.0, 1.0, 1.0),
        p(0.0, 1.0, 1.0),
        p(1.0, 0.0, 1.0),
        p(1.0, 1.0, 0.0),
    );
    let (a, b, c) = (p100 - p000, p010 - p000, p001 - p000);
    let (d, e, f) = (p011 - p111, p101 - p111, p110 - p111);
    0.5 * (cross(a, b) + cross(a, c) + cross(c, b) + cross(d, e) + cross(d, f) + cross(f, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_records_pack_the_key_bounds_and_count_pairs() {
        let key = SectionKey {
            level: 2,
            x: 5,
            y: -1,
            z: 9,
        };
        let mut section = BuiltSection::empty(key, 0b11);
        section.aabb = 0xABCD;
        section.quads = vec![0; 10];
        section.offsets = [0, 1, 1, 3, 6, 6, 6, 8];
        let packed = key.pack();
        assert_eq!(
            section_record(&section, 77),
            [
                [(packed >> 32) as u32, packed as u32, 0xABCD, 77],
                [1, 2 | 3 << 16, 0, 2 | 2 << 16]
            ]
        );
    }

    #[test]
    fn cube_faces_wind_the_same_way() {
        let corner = |i: u32| [i & 1, i >> 1 & 1, i >> 2].map(|c| c as f32 - 0.5);
        let facing: Vec<bool> = CUBE
            .chunks(3)
            .map(|tri| {
                let [a, b, c] = [corner(tri[0]), corner(tri[1]), corner(tri[2])];
                let edge = |p: [f32; 3], q: [f32; 3]| [q[0] - p[0], q[1] - p[1], q[2] - p[2]];
                let (u, v) = (edge(a, b), edge(a, c));
                let normal = [
                    u[1] * v[2] - u[2] * v[1],
                    u[2] * v[0] - u[0] * v[2],
                    u[0] * v[1] - u[1] * v[0],
                ];
                // The face's centre is along its normal, one way or the other.
                (0..3)
                    .map(|i| normal[i] * (a[i] + b[i] + c[i]))
                    .sum::<f32>()
                    > 0.0
            })
            .collect();
        assert!(facing.iter().all(|&f| f == facing[0]), "{facing:?}");
    }
}
