//! Distant terrain on the GPU: built sections' quads in one arena with a
//! metadata record each, the models they index, the node tree the GPU walks
//! each frame to pick which to draw, the requests it sends back, and the
//! draw list the GPU builds from that pick.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;

use glam::DVec3;

use super::build::BuiltSection;
use super::colors::TINT_KINDS;
use super::models::{
    ATLAS_COLUMNS, ATLAS_ROWS, BlockModel, CAPACITY, MODEL_MIP_LEVELS, ModelTile, TILE_HEIGHT,
    TILE_WIDTH,
};
use super::nodes::{NODE_BYTES, NodeGraph};
use super::section::{LEVELS, SIDE, SectionKey};
use super::worker::DistantUpdate;
use crate::position::ChunkPos;
use crate::renderer::chunk::buffer::DrawCommand;
use crate::renderer::chunk::tlsf::Tlsf;
use crate::renderer::rhi::IndirectDrawFeatures;
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, rhi, shader, util};

/// The sizes and markers the shaders share.
pub(crate) mod consts {
    include!("shader_consts.rs");
}

use consts::{
    CLEANER_OUTPUT, CLEANER_SENTINEL, CLEANER_SORT_ELEMS, CLEANER_SORT_THREADS, MAX_NODES,
    NODE_SCATTER_BIT, OPAQUE_DRAWS, QUADS_PER_DRAW, SEEN_BEFORE_BIT, TEMPORAL_DRAWS,
    TEMPORAL_OFFSET, TRANSLUCENT_DRAWS, TRANSLUCENT_OFFSET,
};
pub(crate) use consts::{
    COUNTS_CULL_DRAW, COUNTS_OPAQUE, COUNTS_TEMPORAL, COUNTS_TRANSLUCENT, LIST_GROUP, MAX_LISTED,
    MAX_REQUESTS, TRANSLUCENT_BUCKETS,
};

const _: () = assert!(consts::LEVELS == LEVELS as u32);
/// The traversal's workgroup.
pub(crate) const TRAVERSAL_GROUP: u32 = 1 << consts::TRAVERSAL_GROUP_BITS;

/// The arena's size: an eighth of the device's memory, within these bounds.
const MAX_ARENA_BYTES: u64 = 512 << 20;
const MIN_ARENA_BYTES: u64 = 64 << 20;
/// Sections the metadata buffer holds.
const MAX_SECTIONS: u32 = 1 << 20;
/// Quads a frame's staging holds for the arena.
const STAGING_BYTES: u64 = 16 << 20;
/// Records a frame's scatter list holds (`distant_scatter.comp`: chunks of
/// four indices and four records).
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
/// The requests in flight at which the traversal stops asking for more.
const TARGET_REQUESTS: f64 = 4000.0;
/// Top-level nodes the ring may hold.
const MAX_ROOTS: usize = 1 << 14;
/// The distant projection's far plane: 3000 chunks.
pub(crate) const DISTANT_FAR: f32 = 3000.0 * 16.0;
/// Drawn chunk sections the bound store starts with room for.
const BOUND_CAPACITY: u32 = 1 << 15;
/// The box's 12 triangles as corner bits (x, y, z), every face wound
/// counter-clockwise seen from outside, so culling the back faces keeps
/// the near ones and culling the front faces the far ones; after the
/// quads' indices in the shared index buffer.
const CUBE: [u32; 36] = [
    0, 6, 2, 6, 0, 4, 1, 7, 5, 7, 1, 3, 0, 5, 4, 5, 0, 1, 2, 7, 3, 7, 2, 6, 0, 3, 1, 3, 0, 2, 4, 7,
    6, 7, 4, 5,
];
pub(crate) const CUBE_FIRST_INDEX: u32 = QUADS_PER_DRAW * 6;

/// Byte sizes of the buffers the GPU passes bind.
pub(crate) const SECTIONS_BYTES: u64 = MAX_SECTIONS as u64 * 32;
pub(crate) const RENDER_LIST_BYTES: u64 = (1 + MAX_LISTED as u64) * 4;
pub(crate) const COUNTS_BYTES: u64 = 64;
const COUNT_WORDS: usize = COUNTS_BYTES as usize / 4;
pub(crate) const DRAWS_BYTES: u64 =
    (TEMPORAL_OFFSET + TEMPORAL_DRAWS) as u64 * size_of::<DrawCommand>() as u64;
pub(crate) const POSITIONS_BYTES: u64 = MAX_LISTED as u64 * 8;
pub(crate) const VISIBILITY_BYTES: u64 = MAX_SECTIONS as u64 * 4;
pub(crate) const TRANSLUCENT_BYTES: u64 = (TRANSLUCENT_BUCKETS + TRANSLUCENT_DRAWS) as u64 * 4;
/// The request queue: a `uvec2` count, then the keys.
pub(crate) const REQUESTS_BYTES: u64 = 8 + MAX_REQUESTS as u64 * 8;
/// Per traversal iteration, a `uvec4`: its dispatch size and node count.
pub(crate) const QUEUE_META_BYTES: u64 = LEVELS as u64 * 16;
pub(crate) const SCRATCH_BYTES: u64 = MAX_LISTED as u64 * 4;
pub(crate) const TOP_IDS_BYTES: u64 = MAX_ROOTS as u64 * 4;
/// Per node, the frame the traversal last drew it, which the cleaner sorts by.
pub(crate) const NODE_VISIBILITY_BYTES: u64 = MAX_NODES as u64 * 4;
/// The cleaner's output: the ids it picks, then their keys.
pub(crate) const CLEANER_IDS_BYTES: u64 = CLEANER_OUTPUT as u64 * 4;
/// Per pick its key and its id (a `uvec4`), so a pick whose id now holds
/// another node is dropped (a departure: the reference reads back keys
/// alone, which after a dimension change can name a new node at the same
/// coordinates).
pub(crate) const CLEANER_KEYS_BYTES: u64 = CLEANER_OUTPUT as u64 * 16;
/// Nodes a workgroup of the cleaner's sort covers.
const CLEANER_SORT_GROUP: u32 = CLEANER_SORT_THREADS * CLEANER_SORT_ELEMS;
/// Visibility sets a frame's list holds.
const MAX_VISIBILITY_SETS: usize = 1 << 16;
pub(crate) const VISIBILITY_SETS_BYTES: u64 = MAX_VISIBILITY_SETS as u64 * 4;

/// A section's quads in the arena and its record (`SectionMeta`).
#[derive(Clone, Copy)]
struct Uploaded {
    /// Its record's slot.
    id: u32,
    /// Its first quad.
    ptr: u32,
}

/// A section placed in the arena, waiting for a frame's staging to carry
/// its quads and record up.
struct Pending {
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

impl Region {
    const ALL: [Self; 3] = [Self::Opaque, Self::Temporal, Self::Translucent];

    /// The region's first draw, the byte offset of its count in the
    /// draw-count buffer, and its size in draws.
    fn layout(self) -> (u32, u64, u32) {
        match self {
            Self::Opaque => (0, COUNTS_OPAQUE, OPAQUE_DRAWS),
            Self::Temporal => (TEMPORAL_OFFSET, COUNTS_TEMPORAL, TEMPORAL_DRAWS),
            Self::Translucent => (TRANSLUCENT_OFFSET, COUNTS_TRANSLUCENT, TRANSLUCENT_DRAWS),
        }
    }
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

/// The buffers the GPU passes read and write.
#[derive(Clone, Copy)]
pub(crate) struct DrawBuffers<'a> {
    pub sections: &'a rhi::backend::Buffer,
    pub counts: &'a rhi::backend::Buffer,
    pub draws: &'a rhi::backend::Buffer,
    pub positions: &'a rhi::backend::Buffer,
    pub visibility: &'a rhi::backend::Buffer,
    pub translucent: &'a rhi::backend::Buffer,
    pub render_list: &'a rhi::backend::Buffer,
    pub nodes: &'a rhi::backend::Buffer,
    pub requests: &'a rhi::backend::Buffer,
    pub queue_meta: &'a rhi::backend::Buffer,
    pub scratch: [&'a rhi::backend::Buffer; 2],
    pub node_visibility: &'a rhi::backend::Buffer,
    pub cleaner_ids: &'a rhi::backend::Buffer,
    pub cleaner_keys: &'a rhi::backend::Buffer,
}

pub(crate) struct DistantBuffers {
    layout: rhi::backend::BindGroupLayout,
    /// Set 1: the quads, models, tints and the model atlas.
    group: rhi::backend::BindGroup,
    /// Every section's quads.
    arena: rhi::backend::Buffer,
    quads: Tlsf,
    /// Every section's record, by id.
    sections: rhi::backend::Buffer,
    ids: Tlsf,
    /// Every uploaded section, by id.
    meshes: HashMap<u32, Uploaded>,
    index_buffer: rhi::backend::Buffer,
    pending: VecDeque<Pending>,
    /// Results held until the arena has room for their quads.
    waiting: HashMap<SectionKey, BuiltSection>,
    graph: NodeGraph,
    /// Per frame in flight: the arena's staging, the scatter list, the
    /// top-level node ids, the queue metadata's staging, the visibility
    /// sets, and the request queue's and the cleaner's readbacks.
    quad_staging: Vec<rhi::backend::Buffer>,
    scatter_staging: Vec<rhi::backend::Buffer>,
    scatter: util::ComputePass,
    top_ids: Vec<rhi::backend::Buffer>,
    queue_meta_staging: Vec<rhi::backend::Buffer>,
    visibility_sets: Vec<rhi::backend::Buffer>,
    request_readbacks: Vec<rhi::backend::Buffer>,
    cleaner_readbacks: Vec<rhi::backend::Buffer>,
    /// Without an indirect count: the draw counts read back per frame,
    /// the last ones read, and the slots each region draws this frame.
    count_readbacks: Vec<rhi::backend::Buffer>,
    last_counts: [u32; COUNT_WORDS],
    slots: [u32; 3],
    /// Frames recorded, which a frame ended at acquire is not.
    recorded_frames: u64,
    /// The recorded frame whose draw list the draw buffer holds, which the
    /// next one's opaque draws use; `None` when there is none to use.
    list_frame: Option<u64>,
    /// Per frame in flight, whether its cleaner readback holds a pass's
    /// keys.
    cleaner_pending: [bool; MAX_FRAMES_IN_FLIGHT],
    /// Per frame in flight, whether a traversal filled its request readback;
    /// a frame that skipped the pass leaves the last run's requests there.
    request_pending: [bool; MAX_FRAMES_IN_FLIGHT],
    /// The buffers the GPU passes keep between frames: the node tree, the
    /// traversal's request queue, queue metadata and scratch queues, the
    /// render list, the nodes' last-drawn frames and the cleaner's output,
    /// the draw-count buffer, draw list, draw positions, section visibility
    /// and translucent buckets.
    nodes: rhi::backend::Buffer,
    requests: rhi::backend::Buffer,
    queue_meta: rhi::backend::Buffer,
    scratch: [rhi::backend::Buffer; 2],
    render_list: rhi::backend::Buffer,
    node_visibility: rhi::backend::Buffer,
    cleaner_ids: rhi::backend::Buffer,
    cleaner_keys: rhi::backend::Buffer,
    counts: rhi::backend::Buffer,
    draws: rhi::backend::Buffer,
    positions: rhi::backend::Buffer,
    visibility: rhi::backend::Buffer,
    translucent: rhi::backend::Buffer,
    indirect: IndirectDrawFeatures,
    /// The ring of top-level sections: its centre, radius and height
    /// range, so the roots change only when it moves.
    ring: Option<(i32, i32, i32, i32, i32)>,
    view: Option<View>,
    bounds: BoundStore,
    frame_seq: u64,
    /// Sections whose quads and ids are freed at the frame from which no
    /// in-flight frame can still draw them.
    pending_free: VecDeque<(u64, Uploaded)>,
    /// Sections let go of, by the graph's dirty mark then; they join
    /// [`Self::pending_free`] once the records changed by then are written,
    /// so none on the GPU names them.
    unflushed_free: VecDeque<(u64, Uploaded)>,
    /// Every model's six faces and their mips, a tile each.
    model_atlas: rhi::backend::Texture,
    model_sampler: rhi::backend::Sampler,
    /// Every model's face data ([`BlockModel`]).
    models: rhi::backend::Buffer,
    /// Biome tints by kind, then biome.
    tints: rhi::backend::Buffer,
    /// Models and tints waiting for [`Self::record_model_upload`], which
    /// copies them in through `model_staging` so no frame in flight sees
    /// them change.
    pending_models: BTreeMap<u16, BlockModel>,
    pending_tints: Option<Vec<u32>>,
    model_staging: Vec<rhi::backend::Buffer>,
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

/// `distant_scatter.comp`'s set: the frame's list, the records and the
/// nodes.
const SCATTER_LAYOUT: &[rhi::BindGroupLayoutEntry] =
    &[rhi::BindGroupLayoutEntry::storage(rhi::ShaderStages::COMPUTE); 3];

impl DistantBuffers {
    pub fn new(device: &rhi::backend::Device, indirect: IndirectDrawFeatures) -> Self {
        // The shared index buffer: corner 1 leads both triangles, so it's the
        // provoking vertex the flat attributes come from; the cube after.
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
        let device_buffer = |name, bytes, usage| {
            rhi::backend::Buffer::new(
                device,
                name,
                bytes,
                usage | rhi::BufferUsages::COPY_DST,
                rhi::MemoryLocation::Device,
            )
        };
        let storage = rhi::BufferUsages::STORAGE;
        let arena_bytes = MAX_ARENA_BYTES
            .min(device.memory_bytes(rhi::MemoryLocation::Device) / 8)
            .max(MIN_ARENA_BYTES)
            / 8
            * 8;
        let arena = device_buffer("distant_quads", arena_bytes, storage);
        let sections = device_buffer("distant_sections", SECTIONS_BYTES, storage);
        let nodes = device_buffer("distant_nodes", NODE_BYTES, storage);
        let requests = device_buffer(
            "distant_requests",
            REQUESTS_BYTES,
            storage | rhi::BufferUsages::COPY_SRC,
        );
        let queue_meta = device_buffer(
            "distant_queue_meta",
            QUEUE_META_BYTES,
            storage | rhi::BufferUsages::INDIRECT,
        );
        let scratch = ["distant_queue_a", "distant_queue_b"]
            .map(|name| device_buffer(name, SCRATCH_BYTES, storage));
        let render_list = device_buffer("distant_render_list", RENDER_LIST_BYTES, storage);
        let node_visibility =
            device_buffer("distant_node_visibility", NODE_VISIBILITY_BYTES, storage);
        let cleaner_ids = device_buffer("distant_cleaner_ids", CLEANER_IDS_BYTES, storage);
        let cleaner_keys = device_buffer(
            "distant_cleaner_keys",
            CLEANER_KEYS_BYTES,
            storage | rhi::BufferUsages::COPY_SRC,
        );
        let counts = device_buffer(
            "distant_draw_counts",
            COUNTS_BYTES,
            storage | rhi::BufferUsages::INDIRECT | rhi::BufferUsages::COPY_SRC,
        );
        let draws = device_buffer(
            "distant_draws",
            DRAWS_BYTES,
            storage | rhi::BufferUsages::INDIRECT,
        );
        let positions = device_buffer("distant_draw_positions", POSITIONS_BYTES, storage);
        let visibility = device_buffer("distant_visibility", VISIBILITY_BYTES, storage);
        let translucent = device_buffer("distant_translucent_draws", TRANSLUCENT_BYTES, storage);
        let models = device_buffer("distant_block_models", MODELS_BYTES, storage);
        let tints = device_buffer("distant_biome_tints", TINTS_BYTES, storage);
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
            // No nodes, none drawn, nothing seen, nothing to draw before the
            // first frame.
            encoder.fill_buffer(nodes.handle(), 0, NODE_BYTES, u32::MAX);
            encoder.fill_buffer(node_visibility.handle(), 0, NODE_VISIBILITY_BYTES, u32::MAX);
            encoder.fill_buffer(visibility.handle(), 0, VISIBILITY_BYTES, 0);
            encoder.fill_buffer(counts.handle(), 0, COUNTS_BYTES, 0);
            encoder.fill_buffer(render_list.handle(), 0, 4, 0);
            encoder.fill_buffer(models.handle(), 0, MODELS_BYTES, 0);
            encoder.fill_buffer(tints.handle(), 0, TINTS_BYTES, 0);
        });
        // The model atlas's sampler: nearest within a level, linear between
        // the tiles' mip levels.
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
                util::storage_binding(0, &arena, arena_bytes),
                util::storage_binding(1, &models, MODELS_BYTES),
                util::storage_binding(2, &tints, TINTS_BYTES),
                (3, rhi::backend::BindingResource::Texture(atlas)),
                (4, rhi::backend::BindingResource::Sampler(model_sampler)),
            ],
        );
        let per_frame =
            |bytes, usage, name| util::create_per_frame_host_buffers(device, bytes, usage, name);
        let readbacks = |name, bytes| -> Vec<rhi::backend::Buffer> {
            (0..MAX_FRAMES_IN_FLIGHT)
                .map(|_| {
                    rhi::backend::Buffer::new(
                        device,
                        name,
                        bytes,
                        rhi::BufferUsages::COPY_DST,
                        rhi::MemoryLocation::Readback,
                    )
                })
                .collect()
        };
        let scatter_staging = per_frame(SCATTER_BYTES, storage, "distant_scatter");
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
                    util::storage_binding(0, &scatter_staging[frame], SCATTER_BYTES),
                    util::storage_binding(1, &sections, SECTIONS_BYTES),
                    util::storage_binding(2, &nodes, NODE_BYTES),
                ]
            },
        );
        Self {
            layout,
            group,
            quads: Tlsf::new((arena_bytes / 8) as u32),
            arena,
            sections,
            ids: Tlsf::new(MAX_SECTIONS),
            meshes: HashMap::new(),
            index_buffer: rhi::backend::Buffer::with_data(
                device,
                "distant_quad_indices",
                bytemuck::cast_slice(&indices),
                rhi::BufferUsages::INDEX,
            ),
            pending: VecDeque::new(),
            waiting: HashMap::new(),
            graph: NodeGraph::new(),
            quad_staging: per_frame(
                STAGING_BYTES,
                rhi::BufferUsages::COPY_SRC,
                "distant_quad_staging",
            ),
            scatter_staging,
            scatter,
            top_ids: per_frame(TOP_IDS_BYTES, storage, "distant_top_nodes"),
            queue_meta_staging: per_frame(
                QUEUE_META_BYTES,
                rhi::BufferUsages::COPY_SRC,
                "distant_queue_meta_staging",
            ),
            visibility_sets: per_frame(VISIBILITY_SETS_BYTES, storage, "distant_visibility_sets"),
            request_readbacks: readbacks("distant_request_readback", REQUESTS_BYTES),
            cleaner_readbacks: readbacks("distant_cleaner_readback", CLEANER_KEYS_BYTES),
            cleaner_pending: [false; MAX_FRAMES_IN_FLIGHT],
            request_pending: [false; MAX_FRAMES_IN_FLIGHT],
            count_readbacks: readbacks("distant_count_readback", COUNTS_BYTES),
            last_counts: [0; COUNT_WORDS],
            slots: [0; 3],
            recorded_frames: 0,
            list_frame: None,
            nodes,
            requests,
            queue_meta,
            scratch,
            render_list,
            node_visibility,
            cleaner_ids,
            cleaner_keys,
            counts,
            draws,
            positions,
            visibility,
            translucent,
            indirect,
            ring: None,
            view: None,
            bounds: BoundStore::new(device),
            frame_seq: 0,
            pending_free: VecDeque::new(),
            unflushed_free: VecDeque::new(),
            model_atlas,
            model_sampler,
            models,
            tints,
            pending_tiles: VecDeque::new(),
            pending_models: BTreeMap::new(),
            pending_tints: None,
            model_staging: per_frame(
                MODELS_BYTES + TINTS_BYTES,
                rhi::BufferUsages::COPY_SRC,
                "distant_model_data_staging",
            ),
            tile_staging: per_frame(
                (TILE_BYTES * TILES_PER_FRAME) as u64,
                rhi::BufferUsages::COPY_SRC,
                "distant_model_staging",
            ),
        }
    }

    pub fn layout(&self) -> rhi::backend::BindGroupLayout {
        self.layout
    }

    pub fn draw_buffers(&self) -> DrawBuffers<'_> {
        DrawBuffers {
            sections: &self.sections,
            counts: &self.counts,
            draws: &self.draws,
            positions: &self.positions,
            visibility: &self.visibility,
            translucent: &self.translucent,
            render_list: &self.render_list,
            nodes: &self.nodes,
            requests: &self.requests,
            queue_meta: &self.queue_meta,
            scratch: self.scratch.each_ref(),
            node_visibility: &self.node_visibility,
            cleaner_ids: &self.cleaner_ids,
            cleaner_keys: &self.cleaner_keys,
        }
    }

    /// The top-level nodes' ids, the traversal's first iteration's source.
    pub fn top_ids(&self, frame: usize) -> &rhi::backend::Buffer {
        &self.top_ids[frame]
    }

    pub fn top_count(&self) -> u32 {
        self.graph.top_ids().len() as u32
    }

    /// The ids whose last-drawn frame the visibility-set pass sets.
    pub fn visibility_sets(&self, frame: usize) -> &rhi::backend::Buffer {
        &self.visibility_sets[frame]
    }

    /// The quads' indices, then the cube's from [`CUBE_FIRST_INDEX`].
    pub fn index_buffer(&self) -> rhi::backend::BufferHandle {
        self.index_buffer.handle()
    }

    /// The frame's id as the GPU passes see it.
    pub fn frame_id(&self) -> u32 {
        self.frame_seq as u32 & !SEEN_BEFORE_BIT
    }

    /// The render distance squared, in blocks.
    pub fn render_distance_sq(&self) -> f32 {
        self.view
            .as_ref()
            .map_or(0.0, |view| (view.radius * view.radius) as f32)
    }

    /// Requests the traversal may make this frame: fewer the more wait on
    /// the worker.
    pub fn request_limit(&self) -> u32 {
        let room = ((TARGET_REQUESTS - self.graph.in_flight() as f64) / TARGET_REQUESTS).max(0.0);
        ((room * room * f64::from(MAX_REQUESTS)).ceil() as u32).min(MAX_REQUESTS)
    }

    /// Writes the changed models and tints, then the waiting model tiles
    /// into the atlas, as many as fit this frame's staging.
    pub fn record_model_upload(&mut self, encoder: &rhi::backend::CommandEncoder, frame: usize) {
        self.record_model_data(encoder, frame);
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

    /// The changed models, then the tints, through `frame`'s staging.
    fn record_model_data(&mut self, encoder: &rhi::backend::CommandEncoder, frame: usize) {
        let models = std::mem::take(&mut self.pending_models);
        let tints = self.pending_tints.take();
        if models.is_empty() && tints.is_none() {
            return;
        }
        let staging = &mut self.model_staging[frame];
        let model_bytes = size_of::<BlockModel>();
        let mut copies = Vec::with_capacity(models.len());
        for (i, (id, model)) in models.iter().enumerate() {
            staging.write_at(i * model_bytes, bytemuck::bytes_of(model));
            copies.push(rhi::BufferCopy {
                src_offset: (i * model_bytes) as u64,
                dst_offset: (usize::from(*id) * model_bytes) as u64,
                size: model_bytes as u64,
            });
        }
        encoder.transition_buffers(rhi::BufferUse::ShaderRead, rhi::BufferUse::CopyDst);
        if !copies.is_empty() {
            encoder.copy_buffer(staging.handle(), self.models.handle(), &copies);
        }
        if let Some(tints) = tints {
            staging.write_at(MODELS_BYTES as usize, bytemuck::cast_slice(&tints));
            encoder.copy_buffer(
                staging.handle(),
                self.tints.handle(),
                &[rhi::BufferCopy {
                    src_offset: MODELS_BYTES,
                    dst_offset: 0,
                    size: TINTS_BYTES,
                }],
            );
        }
        encoder.transition_buffers(rhi::BufferUse::CopyDst, rhi::BufferUse::ShaderRead);
    }

    /// Carries the pending sections up, their quads into the arena through
    /// `frame`'s staging and their records through its scatter list, as many
    /// as fit, the rest next frame; then the changed nodes through the same
    /// list, but those naming a section still pending, which wait with it;
    /// then the sections no node record names any more are freed. Call once
    /// a recorded frame.
    pub fn record_uploads(&mut self, encoder: &rhi::backend::CommandEncoder, frame: usize) {
        self.recorded_frames += 1;
        let mut copies = Vec::new();
        let mut records: Vec<(u32, [u32; 4])> = Vec::new();
        let mut used = 0;
        while let Some(next) = self.pending.front() {
            let bytes = next.quads.len() * 8;
            if used + bytes > STAGING_BYTES as usize || records.len() + 2 > SCATTER_RECORDS {
                break;
            }
            let Pending {
                uploaded,
                quads,
                record,
            } = self.pending.pop_front().expect("peeked");
            self.quad_staging[frame].write_at(used, bytemuck::cast_slice(&quads));
            copies.push(rhi::BufferCopy {
                src_offset: used as u64,
                dst_offset: u64::from(uploaded.ptr) * 8,
                size: bytes as u64,
            });
            used += bytes;
            for (i, half) in (0..).zip(record) {
                records.push((uploaded.id * 2 + i, half));
            }
            self.meshes.insert(uploaded.id, uploaded);
        }
        if !copies.is_empty() {
            encoder.copy_buffer(
                self.quad_staging[frame].handle(),
                self.arena.handle(),
                &copies,
            );
            encoder.transition_buffers(rhi::BufferUse::CopyDst, rhi::BufferUse::ShaderRead);
        }
        // What fits the list; the rest of the nodes wait for the next frame.
        let pending: HashSet<u32> = self.pending.iter().map(|p| p.uploaded.id).collect();
        records.extend(
            self.graph
                .take_dirty(SCATTER_RECORDS.saturating_sub(records.len()), |id| {
                    pending.contains(&id)
                })
                .into_iter()
                .map(|(id, record)| (id | NODE_SCATTER_BIT, record)),
        );
        let written = self.graph.written_through();
        while let Some(&(mark, section)) = self.unflushed_free.front()
            && mark <= written
        {
            self.unflushed_free.pop_front();
            self.free_later(section);
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
            [count.div_ceil(LIST_GROUP), 1, 1],
        );
        encoder.transition_buffers_many(&[
            (rhi::BufferUse::StorageWrite, rhi::BufferUse::Storage),
            (rhi::BufferUse::StorageWrite, rhi::BufferUse::ShaderRead),
        ]);
    }

    /// Before the traversal: this frame's top-level ids (those whose records
    /// are written) and queue metadata (the first iteration over the roots,
    /// the rest empty), the render list and request queue emptied; how many
    /// roots.
    pub fn record_traversal_setup(
        &mut self,
        encoder: &rhi::backend::CommandEncoder,
        frame: usize,
    ) -> u32 {
        let top = self.graph.written_top_ids();
        let count = top.len() as u32;
        self.top_ids[frame].write_at(0, bytemuck::cast_slice(&top));
        let mut meta = [[0u32, 1, 1, 0]; LEVELS as usize];
        meta[0] = [count.div_ceil(TRAVERSAL_GROUP), 1, 1, count];
        self.queue_meta_staging[frame].write_at(0, bytemuck::cast_slice(&meta));
        encoder.transition_buffers_many(&[
            (rhi::BufferUse::DrawInput, rhi::BufferUse::CopyDst),
            (rhi::BufferUse::Storage, rhi::BufferUse::CopyDst),
        ]);
        encoder.copy_buffer(
            self.queue_meta_staging[frame].handle(),
            self.queue_meta.handle(),
            &[rhi::BufferCopy::from_start(QUEUE_META_BYTES)],
        );
        encoder.fill_buffer(self.render_list.handle(), 0, 4, 0);
        encoder.fill_buffer(self.requests.handle(), 0, 8, 0);
        encoder.transition_buffers_many(&[
            (rhi::BufferUse::CopyDst, rhi::BufferUse::Storage),
            (rhi::BufferUse::CopyDst, rhi::BufferUse::DrawInput),
        ]);
        count
    }

    /// After the traversal: its requests into this frame's readback, read
    /// by [`Self::begin_frame`] once the frame is done.
    pub fn record_request_readback(
        &mut self,
        encoder: &rhi::backend::CommandEncoder,
        frame: usize,
    ) {
        self.request_pending[frame] = true;
        record_readback(
            encoder,
            &self.requests,
            &self.request_readbacks[frame],
            REQUESTS_BYTES,
        );
    }

    /// Writes this frame's visibility sets: the nodes allocated and freed
    /// since last time, as many as fit, the rest next frame; how many.
    pub fn write_visibility_sets(&mut self, frame: usize) -> u32 {
        let sets = &mut self.graph.visibility_sets;
        let ids: Vec<u32> = sets.iter().copied().take(MAX_VISIBILITY_SETS).collect();
        for id in &ids {
            sets.remove(id);
        }
        self.visibility_sets[frame].write_at(0, bytemuck::cast_slice(&ids));
        ids.len() as u32
    }

    /// Before the cleaner's sort, when it should run (less than an eighth of
    /// the arena free, a fraction rather than a fixed amount since the arena
    /// is sized to the device): its output reset to the sentinel, an id no
    /// node has; the sort's workgroups.
    pub fn record_cleaner_setup(
        &mut self,
        encoder: &rhi::backend::CommandEncoder,
        frame: usize,
    ) -> Option<u32> {
        let capacity = self.quads.capacity();
        let groups = self.graph.end_id().div_ceil(CLEANER_SORT_GROUP);
        if capacity - self.quads.used() >= capacity / 8 || groups == 0 {
            return None;
        }
        encoder.transition_buffers(rhi::BufferUse::Storage, rhi::BufferUse::CopyDst);
        encoder.fill_buffer(
            self.cleaner_ids.handle(),
            0,
            CLEANER_IDS_BYTES,
            CLEANER_SENTINEL,
        );
        encoder.transition_buffers(rhi::BufferUse::CopyDst, rhi::BufferUse::Storage);
        self.cleaner_pending[frame] = true;
        Some(groups)
    }

    /// Without an indirect count, before the command generation: zeroes
    /// the slots each region draws ([`draw_slots`]) so the unwritten ones
    /// draw nothing, as the chunk renderer's cull does.
    pub fn record_draw_slots(&mut self, encoder: &rhi::backend::CommandEncoder) {
        if self.indirect.count {
            return;
        }
        let stride = size_of::<DrawCommand>() as u64;
        encoder.transition_buffers(rhi::BufferUse::DrawInput, rhi::BufferUse::CopyDst);
        self.slots = draw_slots(&self.last_counts);
        for region in Region::ALL {
            let (first, ..) = region.layout();
            encoder.fill_buffer(
                self.draws.handle(),
                u64::from(first) * stride,
                u64::from(self.slots[region as usize]) * stride,
                0,
            );
        }
    }

    /// After the command generation: this frame's draw list is the one the
    /// next frame's opaque draws use; without an indirect count, the draw
    /// counts into this frame's readback.
    pub fn finish_list(&mut self, encoder: &rhi::backend::CommandEncoder, frame: usize) {
        self.list_frame = Some(self.recorded_frames);
        if !self.indirect.count {
            record_readback(
                encoder,
                &self.counts,
                &self.count_readbacks[frame],
                COUNTS_BYTES,
            );
        }
    }

    /// After the cleaner's results: the keys to drop into this frame's
    /// readback.
    pub fn record_cleaner_readback(&self, encoder: &rhi::backend::CommandEncoder, frame: usize) {
        record_readback(
            encoder,
            &self.cleaner_keys,
            &self.cleaner_readbacks[frame],
            CLEANER_KEYS_BYTES,
        );
    }

    pub fn bound_layout(&self) -> rhi::backend::BindGroupLayout {
        self.bounds.layout()
    }

    /// The bound store with the frame's buffer brought up to date.
    pub fn bounds(&mut self, frame: usize) -> &BoundStore {
        self.bounds.flush(frame);
        &self.bounds
    }

    /// Reclaims the ranges and buffers no in-flight frame reads any more,
    /// and takes up the requests the traversal made, and the meshes the
    /// cleaner picked, the last time this frame slot ran. Call once a
    /// frame, after its fence wait.
    pub fn begin_frame(&mut self, device: &rhi::backend::Device, frame: usize) {
        self.frame_seq += 1;
        self.bounds.free_retired(device, self.frame_seq);
        while let Some(&(at, section)) = self.pending_free.front()
            && at <= self.frame_seq
        {
            self.pending_free.pop_front();
            self.ids.free(section.id);
            self.quads.free(section.ptr);
        }
        self.place_waiting();
        if std::mem::take(&mut self.request_pending[frame]) {
            let keys: Vec<u64> = read_words(
                &mut self.request_readbacks[frame],
                REQUESTS_BYTES,
                |words| {
                    let count = (words[0] as usize).min(MAX_REQUESTS as usize);
                    packed_keys(&words[2..2 + count * 2]).collect()
                },
            );
            for key in keys {
                self.graph.process_request(SectionKey::unpack(key));
            }
        }
        if !self.indirect.count {
            self.last_counts =
                read_words(&mut self.count_readbacks[frame], COUNTS_BYTES, |words| {
                    words.try_into().expect("the whole draw-count buffer")
                });
        }
        if std::mem::take(&mut self.cleaner_pending[frame]) {
            let picks: Vec<(u32, u64)> = read_words(
                &mut self.cleaner_readbacks[frame],
                CLEANER_KEYS_BYTES,
                |words| {
                    words
                        .chunks(4)
                        .map(|pick| (pick[2], u64::from(pick[0]) << 32 | u64::from(pick[1])))
                        .collect()
                },
            );
            for (id, key) in picks {
                let key = SectionKey::unpack(key);
                if self.graph.holds(id, key) {
                    self.graph.remove_node_geometry(key);
                }
            }
            self.free_released();
        }
    }

    /// Frees `section`'s quads and id once no frame draws them.
    fn free_later(&mut self, section: Uploaded) {
        let at = self.frame_seq + MAX_FRAMES_IN_FLIGHT as u64;
        self.pending_free.push_back((at, section));
    }

    /// Frees the meshes the graph let go of once no node record names them
    /// and no frame draws them.
    fn free_released(&mut self) {
        let mark = self.graph.dirty_mark();
        for id in std::mem::take(&mut self.graph.freed) {
            let section = if let Some(section) = self.meshes.remove(&id) {
                section
            } else if let Some(at) = self.pending.iter().position(|p| p.uploaded.id == id) {
                self.pending.remove(at).expect("found").uploaded
            } else {
                continue;
            };
            self.unflushed_free.push_back((mark, section));
        }
    }

    /// Drops every section, as on a dimension change. The bounds stay: they
    /// track the chunk store, which [`Self::clear_bounds`] follows. Nothing
    /// draws until new roots' records are written, so the meshes go free.
    pub fn clear(&mut self) {
        self.list_frame = None;
        self.cleaner_pending = [false; MAX_FRAMES_IN_FLIGHT];
        self.waiting.clear();
        self.graph.clear();
        self.free_released();
        for (_, section) in std::mem::take(&mut self.unflushed_free) {
            self.free_later(section);
        }
        self.ring = None;
        self.view = None;
    }

    pub fn clear_bounds(&mut self) {
        self.bounds.clear();
    }

    /// Which of column `pos`'s sections are compiled (bit `i` for the `i`th
    /// from the dimension's bottom, `min_section`), bounding distant terrain
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

    /// Which sections the occlusion graph shows, per column, as the chunk
    /// cull draws them.
    pub fn set_visibility(&mut self, device: &rhi::backend::Device, vis: &HashMap<ChunkPos, u32>) {
        self.bounds.set_visibility(device, self.frame_seq, vis);
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

    /// Takes the worker's updates: the models and tints up, the sections
    /// into the graph and the arena as room allows.
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
                self.pending_tints = Some(tints);
            }
            for tile in update.tiles {
                self.pending_models.insert(tile.id, tile.model);
                self.pending_tiles.push_back(tile);
            }
            for section in update.sections {
                self.waiting.insert(section.key, section);
            }
        }
        self.place_waiting();
    }

    /// Takes the waiting results whose quads fit the arena into the graph.
    fn place_waiting(&mut self) {
        if self.waiting.is_empty() {
            return;
        }
        let keys: Vec<SectionKey> = self.waiting.keys().copied().collect();
        for key in keys {
            let quads = self.waiting[&key].quads.len() as u32;
            if quads > 0 && !self.quads.fits(quads) {
                continue;
            }
            let section = self.waiting.remove(&key).expect("listed");
            let (quads, ids, pending) = (&mut self.quads, &mut self.ids, &mut self.pending);
            self.graph.process_geometry_result(section, &mut |section| {
                place(quads, ids, pending, section)
            });
        }
        self.free_released();
    }

    /// The sections the graph asked the worker for since last time, and
    /// those it dropped.
    pub fn take_requests(&mut self) -> (Vec<SectionKey>, Vec<SectionKey>) {
        (
            std::mem::take(&mut self.graph.watch),
            std::mem::take(&mut self.graph.unwatch),
        )
    }

    /// Whether the finest section holding block column `x`/`z` is meshed
    /// with something in it, at any height.
    #[cfg(feature = "test-harness")]
    pub fn has_column(&self, x: i32, z: i32) -> bool {
        self.graph.has_column(x, z)
    }

    /// The top-level nodes are a ring of sections around `eye`, over the
    /// dimension's height, remade when it moves.
    pub fn update_roots(&mut self, eye: DVec3) {
        let Some(view) = self.view.clone() else {
            return;
        };
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
        if self.ring == Some((cx, cz, ring, min_sec, max_sec)) {
            return;
        }
        self.ring = Some((cx, cz, ring, min_sec, max_sec));
        let mut roots = HashSet::new();
        for x in cx - ring..=cx + ring {
            for z in cz - ring..=cz + ring {
                for y in min_sec..=max_sec {
                    roots.insert(SectionKey {
                        level: top,
                        x,
                        y,
                        z,
                    });
                }
            }
        }
        assert!(roots.len() <= MAX_ROOTS, "too many top-level sections");
        self.graph.set_roots(&roots);
        self.free_released();
    }

    /// Issues one region of the draw list the GPU built: an indirect draw
    /// whose count the generation wrote, or, without an indirect count, over
    /// the zeroed slots of [`Self::record_draw_slots`], one call each without
    /// multi-draw. The caller has bound the matching distant pipeline and its
    /// sets.
    pub fn draw(&self, pass: &rhi::backend::RenderPassEncoder, region: Region) {
        // The opaque region is last frame's list, of no use after a clear or
        // a frame that built none.
        if matches!(region, Region::Opaque)
            && self
                .list_frame
                .is_none_or(|built| built + 1 != self.recorded_frames)
        {
            return;
        }
        let (first, count_offset, max_draws) = region.layout();
        pass.set_bind_group(1, self.group);
        pass.set_index_buffer(self.index_buffer.handle());
        let stride = size_of::<DrawCommand>() as u32;
        let draws = self.draws.handle();
        let offset = u64::from(first * stride);
        let slots = self.slots[region as usize];
        if self.indirect.count {
            pass.draw_indexed_indirect_count(
                draws,
                offset,
                self.counts.handle(),
                count_offset,
                max_draws,
                stride,
            );
        } else if self.indirect.multi_draw {
            pass.draw_indexed_indirect(draws, offset, slots, stride);
        } else {
            for i in 0..slots {
                pass.draw_indexed_indirect(draws, offset + u64::from(i * stride), 1, stride);
            }
        }
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        self.group.destroy(device);
        let [scratch_a, scratch_b] = &mut self.scratch;
        for buffer in [
            &mut self.arena,
            &mut self.sections,
            &mut self.index_buffer,
            &mut self.nodes,
            &mut self.requests,
            &mut self.queue_meta,
            scratch_a,
            scratch_b,
            &mut self.render_list,
            &mut self.node_visibility,
            &mut self.cleaner_ids,
            &mut self.cleaner_keys,
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
        for buffers in [
            &mut self.quad_staging,
            &mut self.scatter_staging,
            &mut self.top_ids,
            &mut self.queue_meta_staging,
            &mut self.visibility_sets,
            &mut self.request_readbacks,
            &mut self.cleaner_readbacks,
            &mut self.count_readbacks,
            &mut self.tile_staging,
            &mut self.model_staging,
        ] {
            util::destroy_buffers(device, buffers);
        }
        self.scatter.destroy(device);
        self.bounds.destroy(device);
        self.model_sampler.destroy(device);
        std::mem::take(&mut self.model_atlas).destroy(device);
        self.layout.destroy(device);
    }
}

/// Copies `source` into `readback` for the CPU to read once the frame is
/// done.
fn record_readback(
    encoder: &rhi::backend::CommandEncoder,
    source: &rhi::backend::Buffer,
    readback: &rhi::backend::Buffer,
    bytes: u64,
) {
    encoder.transition_buffers(rhi::BufferUse::StorageWrite, rhi::BufferUse::CopySrc);
    encoder.copy_buffer(
        source.handle(),
        readback.handle(),
        &[rhi::BufferCopy::from_start(bytes)],
    );
    encoder.transition_buffers(rhi::BufferUse::CopyDst, rhi::BufferUse::HostRead);
}

/// The first `bytes` of `readback`, as `u32`s.
fn read_words<R>(
    readback: &mut rhi::backend::Buffer,
    bytes: u64,
    read: impl FnOnce(&[u32]) -> R,
) -> R {
    readback.read(|data| read(bytemuck::cast_slice(&data[..bytes as usize])))
}

/// The packed keys in `words`, a `uvec2` each, high word first.
fn packed_keys(words: &[u32]) -> impl Iterator<Item = u64> + '_ {
    words
        .chunks(2)
        .map(|key| u64::from(key[0]) << 32 | u64::from(key[1]))
}

/// Places `section`'s quads in the arena and queues their upload; its id, or
/// `None` when out of ids.
fn place(
    quads: &mut Tlsf,
    ids: &mut Tlsf,
    pending: &mut VecDeque<Pending>,
    section: BuiltSection,
) -> Option<u32> {
    let ptr = quads.alloc(section.quads.len() as u32)?;
    let Some(id) = ids.alloc(1) else {
        quads.free(ptr);
        return None;
    };
    pending.push_back(Pending {
        uploaded: Uploaded { id, ptr },
        record: section_record(&section, ptr),
        quads: section.quads,
    });
    Some(id)
}

/// The chunk sections the chunk renderer draws (compiled, and shown by the
/// occlusion graph) for the bound pass, as boxes in a buffer the pass
/// instances over, the last slot moving into a freed one. The columns are
/// kept across frames rather than streamed anew each frame, and each run of
/// stacked sections is one box, whose far faces are the run's.
pub(crate) struct BoundStore {
    layout: rhi::backend::BindGroupLayout,
    /// One per frame in flight, written as its frame records the bound
    /// pass, so a frame still drawing never reads a moved box.
    buffers: Vec<rhi::backend::Buffer>,
    groups: Vec<rhi::backend::BindGroup>,
    /// Per frame in flight, the slots changed since its buffer was written.
    dirty: [Option<(u32, u32)>; MAX_FRAMES_IN_FLIGHT],
    capacity: u32,
    /// Each box's section coordinates and its height in sections.
    slots: Vec<[i32; 4]>,
    /// The slot of the box starting at these section coordinates.
    index: HashMap<[i32; 3], u32>,
    /// Per column, which of its sections are compiled and which the graph
    /// shows (a bit per section from `min_section` up).
    columns: HashMap<ChunkPos, (u32, u32)>,
    min_section: i32,
    /// Buffers in-flight frames may still read, by the frame they're free
    /// from.
    retired: VecDeque<(u64, Vec<rhi::backend::Buffer>, Vec<rhi::backend::BindGroup>)>,
}

impl BoundStore {
    fn new(device: &rhi::backend::Device) -> Self {
        let layout = rhi::backend::create_bind_group_layout(
            device,
            &[rhi::BindGroupLayoutEntry::storage(
                rhi::ShaderStages::VERTEX,
            )],
        );
        let (buffers, groups) = Self::make_buffers(device, layout, BOUND_CAPACITY);
        Self {
            layout,
            buffers,
            groups,
            dirty: [None; MAX_FRAMES_IN_FLIGHT],
            capacity: BOUND_CAPACITY,
            slots: Vec::new(),
            index: HashMap::new(),
            columns: HashMap::new(),
            min_section: 0,
            retired: VecDeque::new(),
        }
    }

    fn make_buffers(
        device: &rhi::backend::Device,
        layout: rhi::backend::BindGroupLayout,
        capacity: u32,
    ) -> (Vec<rhi::backend::Buffer>, Vec<rhi::backend::BindGroup>) {
        let bytes = u64::from(capacity) * 16;
        (0..MAX_FRAMES_IN_FLIGHT)
            .map(|_| {
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
            })
            .unzip()
    }

    pub fn layout(&self) -> rhi::backend::BindGroupLayout {
        self.layout
    }

    pub fn group(&self, frame: usize) -> rhi::backend::BindGroup {
        self.groups[frame]
    }

    pub fn count(&self) -> u32 {
        self.slots.len() as u32
    }

    /// Writes the frame's buffer what changed since it last drew.
    fn flush(&mut self, frame: usize) {
        let Some((lo, hi)) = self.dirty[frame].take() else {
            return;
        };
        let (lo, hi) = (lo as usize, (hi as usize).min(self.slots.len()));
        if lo < hi {
            self.buffers[frame].write_at(lo * 16, bytemuck::cast_slice(&self.slots[lo..hi]));
        }
    }

    fn mark(&mut self, slot: u32) {
        for dirty in &mut self.dirty {
            *dirty = Some(match *dirty {
                Some((lo, hi)) => (lo.min(slot), hi.max(slot + 1)),
                None => (slot, slot + 1),
            });
        }
    }

    /// Which of column `pos`'s sections are compiled.
    fn set_column(
        &mut self,
        device: &rhi::backend::Device,
        frame_seq: u64,
        pos: ChunkPos,
        compiled: u32,
        min_section: i32,
    ) {
        self.min_section = min_section;
        let (_, visible) = self.column(pos);
        self.update(device, frame_seq, pos, compiled, visible);
    }

    /// Column `pos`'s compiled and shown sections; none and all for one not
    /// listed.
    fn column(&self, pos: ChunkPos) -> (u32, u32) {
        self.columns.get(&pos).copied().unwrap_or((0, u32::MAX))
    }

    /// Sets column `pos`'s compiled and shown sections, re-boxing what
    /// changed.
    fn update(
        &mut self,
        device: &rhi::backend::Device,
        frame_seq: u64,
        pos: ChunkPos,
        compiled: u32,
        visible: u32,
    ) {
        let (was_compiled, was_visible) = self.column(pos);
        self.rebox(
            device,
            frame_seq,
            pos,
            was_compiled & was_visible,
            compiled & visible,
        );
        if compiled == 0 {
            self.columns.remove(&pos);
        } else {
            self.columns.insert(pos, (compiled, visible));
        }
    }

    /// Which sections the occlusion graph shows, per column; a column it
    /// doesn't list shows every section, as the chunk cull takes it.
    fn set_visibility(
        &mut self,
        device: &rhi::backend::Device,
        frame_seq: u64,
        vis: &HashMap<ChunkPos, u32>,
    ) {
        let changed: Vec<(ChunkPos, u32, u32)> = self
            .columns
            .iter()
            .filter_map(|(&pos, &(compiled, was))| {
                let visible = vis.get(&pos).copied().unwrap_or(u32::MAX);
                (visible != was).then_some((pos, compiled, visible))
            })
            .collect();
        for (pos, compiled, visible) in changed {
            self.update(device, frame_seq, pos, compiled, visible);
        }
    }

    /// Replaces column `pos`'s boxes for the sections `was` drawn with those
    /// for `now`.
    fn rebox(
        &mut self,
        device: &rhi::backend::Device,
        frame_seq: u64,
        pos: ChunkPos,
        was: u32,
        now: u32,
    ) {
        if was == now {
            return;
        }
        let min_section = self.min_section;
        let key = |start| [pos.x, min_section + start, pos.z];
        for (start, _) in runs(was) {
            self.remove(key(start));
        }
        for (start, height) in runs(now) {
            self.add(device, frame_seq, key(start), height);
        }
    }

    /// A box of `height` sections from the section at `key` up.
    fn add(&mut self, device: &rhi::backend::Device, frame_seq: u64, key: [i32; 3], height: i32) {
        if self.count() == self.capacity {
            self.capacity *= 2;
            let (buffers, groups) = Self::make_buffers(device, self.layout, self.capacity);
            let old = (
                frame_seq + MAX_FRAMES_IN_FLIGHT as u64,
                std::mem::replace(&mut self.buffers, buffers),
                std::mem::replace(&mut self.groups, groups),
            );
            self.retired.push_back(old);
            self.dirty = [Some((0, self.count())); MAX_FRAMES_IN_FLIGHT];
        }
        let slot = self.count();
        let [x, y, z] = key;
        self.slots.push([x, y, z, height]);
        self.index.insert(key, slot);
        self.mark(slot);
    }

    fn remove(&mut self, key: [i32; 3]) {
        let Some(slot) = self.index.remove(&key) else {
            return;
        };
        let last = self.slots.pop().expect("a slot was indexed");
        if slot < self.count() {
            self.slots[slot as usize] = last;
            let [x, y, z, _] = last;
            self.index.insert([x, y, z], slot);
            self.mark(slot);
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
            let (_, buffers, groups) = self.retired.pop_front().expect("peeked");
            Self::destroy_buffers(device, buffers, groups);
        }
    }

    fn destroy_buffers(
        device: &rhi::backend::Device,
        buffers: Vec<rhi::backend::Buffer>,
        groups: Vec<rhi::backend::BindGroup>,
    ) {
        for group in groups {
            group.destroy(device);
        }
        for buffer in buffers {
            buffer.destroy(device);
        }
    }

    fn destroy(&mut self, device: &rhi::backend::Device) {
        self.free_retired(device, u64::MAX);
        Self::destroy_buffers(
            device,
            std::mem::take(&mut self.buffers),
            std::mem::take(&mut self.groups),
        );
        self.layout.destroy(device);
    }
}

/// The runs of set bits in `mask`, as (first bit, length).
fn runs(mut mask: u32) -> impl Iterator<Item = (i32, i32)> {
    std::iter::from_fn(move || {
        if mask == 0 {
            return None;
        }
        let start = mask.trailing_zeros();
        let len = (mask >> start).trailing_ones();
        mask &= !((((1u64 << len) - 1) << start) as u32);
        Some((start as i32, len as i32))
    })
}

/// Without an indirect count, the slots each region draws: an earlier
/// frame's count plus a quarter and a floor for the temporal region's
/// spikes, within the region.
fn draw_slots(last_counts: &[u32; COUNT_WORDS]) -> [u32; 3] {
    Region::ALL.map(|region| {
        let (_, count_offset, max_draws) = region.layout();
        let count = last_counts[count_offset as usize / 4];
        count.saturating_add(count / 4 + 256).min(max_draws)
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_split_a_mask_at_its_gaps() {
        assert_eq!(runs(0).collect::<Vec<_>>(), []);
        assert_eq!(runs(0b1).collect::<Vec<_>>(), [(0, 1)]);
        assert_eq!(runs(0b1110_0000_0110).collect::<Vec<_>>(), [(1, 2), (9, 3)]);
        assert_eq!(runs(u32::MAX).collect::<Vec<_>>(), [(0, 32)]);
        assert_eq!(runs(1 << 31 | 1).collect::<Vec<_>>(), [(0, 1), (31, 1)]);
    }

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
    fn regions_tile_the_draw_list_and_counts_fit_their_buffer() {
        let stride = size_of::<DrawCommand>() as u64;
        let mut regions: Vec<(u32, u64, u32)> = Region::ALL.map(Region::layout).to_vec();
        regions.sort_unstable_by_key(|&(first, ..)| first);
        let mut next = 0;
        for &(first, _, draws) in &regions {
            assert_eq!(first, next);
            next += draws;
        }
        assert_eq!(u64::from(next) * stride, DRAWS_BYTES);
        // The counts follow cmdgen's dispatch size, a word each, before the
        // occlusion pass's draw.
        let mut counts: Vec<u64> = regions.iter().map(|&(_, count, _)| count).collect();
        counts.sort_unstable();
        assert_eq!(counts, [12, 16, 20]);
        assert_eq!(COUNTS_CULL_DRAW, 24);
        assert!(COUNTS_CULL_DRAW + stride <= COUNTS_BYTES);
    }

    #[test]
    fn draw_slots_grow_on_the_last_counts_within_their_regions() {
        let mut counts = [0; COUNT_WORDS];
        assert_eq!(draw_slots(&counts), [256; 3]);
        counts[COUNTS_OPAQUE as usize / 4] = 1000;
        counts[COUNTS_TEMPORAL as usize / 4] = u32::MAX;
        counts[COUNTS_TRANSLUCENT as usize / 4] = TRANSLUCENT_DRAWS;
        let slots = draw_slots(&counts);
        assert_eq!(slots[Region::Opaque as usize], 1000 + 250 + 256);
        assert_eq!(slots[Region::Temporal as usize], TEMPORAL_DRAWS);
        assert_eq!(slots[Region::Translucent as usize], TRANSLUCENT_DRAWS);
    }

    #[test]
    fn cube_faces_wind_outward() {
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
                // The face's centre lies along its normal when it points out.
                (0..3)
                    .map(|i| normal[i] * (a[i] + b[i] + c[i]))
                    .sum::<f32>()
                    > 0.0
            })
            .collect();
        assert!(facing.iter().all(|&f| f), "{facing:?}");
    }
}
