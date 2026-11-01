//! Distant terrain on the GPU (Voxy's `BasicSectionGeometryManager`,
//! `MDICSectionRenderer` and the node graph `NodeManager` keeps): built
//! sections' quads in arenas, the models they index, the tree walked each
//! frame to pick which to draw (`traversal_dev.comp`, on the CPU until the
//! GPU traversal lands), and the draws.

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
use super::scan::GROUPS;
use super::section::{LEVELS, SIDE, SectionKey};
use super::worker::DistantUpdate;
use crate::position::ChunkPos;
use crate::renderer::camera::Camera;
use crate::renderer::chunk::buffer::DrawCommand;
use crate::renderer::chunk::tlsf::Tlsf;
use crate::renderer::rhi::IndirectDrawFeatures;
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, rhi, util};

/// Quads an arena holds: 64 MB at 8 bytes each.
const ARENA_QUADS: u32 = 8 << 20;
/// Arenas there may be: 512 MB of quads.
const MAX_ARENAS: usize = 8;
/// Quads the shared index buffer covers, the most a group can hold.
const QUADS_PER_DRAW: u32 = 1 << 16;
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
/// Sections a frame may draw, and the draws they may take (one per group).
const MAX_DRAW_SECTIONS: usize = 1 << 15;
const MAX_DRAWS: usize = MAX_DRAW_SECTIONS * GROUPS;
pub(crate) const POSITIONS_BYTES: u64 = (MAX_DRAW_SECTIONS * 16) as u64;

/// A quad buffer, sub-allocated, with the bind group that reads it.
struct Arena {
    buffer: rhi::backend::Buffer,
    quads: Tlsf,
    group: rhi::backend::BindGroup,
}

/// A section's quads in an arena (`SectionMeta`).
#[derive(Clone, Copy)]
struct Uploaded {
    arena: usize,
    /// Its first quad.
    ptr: u32,
    quads: u32,
    offsets: [u32; GROUPS],
    /// [`BuiltSection::aabb`].
    aabb: u32,
    child_existence: u8,
    /// The last frame drawn, for evicting the least recent.
    last_drawn: u64,
}

/// One arena's share of a frame's draw list: where its opaque and its
/// translucent commands start, and how many.
#[derive(Clone, Copy, Default)]
struct ArenaDraws {
    opaque: (u32, u32),
    translucent: (u32, u32),
}

impl Uploaded {
    /// Where group `group` starts and how many quads it has.
    fn group(&self, group: usize) -> (u32, u32) {
        let start = self.offsets[group];
        let end = self.offsets.get(group + 1).copied().unwrap_or(self.quads);
        (self.ptr + start, end - start)
    }

    /// Its quads' bounds in the section, in voxels of its level.
    fn bounds(&self) -> (Vec3, Vec3) {
        let field = |shift: u32| (self.aabb >> shift & 31) as f32;
        let min = Vec3::new(field(0), field(5), field(10));
        let size = Vec3::new(field(15), field(20), field(25)) + 1.0;
        (min, min + size)
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

pub(crate) struct DistantBuffers {
    layout: rhi::backend::BindGroupLayout,
    /// Grown one at a time as sections fill them.
    arenas: Vec<Arena>,
    index_buffer: rhi::backend::Buffer,
    /// Every section the renderer knows, empty ones too (`EMPTY_MESH`).
    sections: HashMap<SectionKey, Uploaded>,
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
    indirect: IndirectDrawFeatures,
    /// Per frame in flight, the draw list (`cmdBuffer`) and each draw's
    /// section origin and scale (`positionBuffer`), by `first_instance`.
    commands: Vec<rhi::backend::Buffer>,
    positions: Vec<rhi::backend::Buffer>,
    /// Each arena's draws in the current frame's list.
    draws: Vec<ArenaDraws>,
    view: Option<View>,
    bounds: BoundStore,
    frame_seq: u64,
    /// Freed quad ranges as (arena, first quad), by the frame from which no
    /// in-flight frame can still draw them.
    pending_free: VecDeque<(u64, usize, u32)>,
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

impl DistantBuffers {
    pub fn new(device: &rhi::backend::Device, indirect: IndirectDrawFeatures) -> Self {
        // Voxy's `SharedIndexBuffer`: corner 1 leads both triangles, so it's
        // the provoking vertex the flat attributes come from.
        let quad_indices: Vec<u32> = (0..QUADS_PER_DRAW)
            .flat_map(|quad| [1, 2, 0, 1, 3, 2].map(|i| quad * 4 + i))
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
        });
        let storage = |name, bytes| {
            rhi::backend::Buffer::new(
                device,
                name,
                bytes,
                rhi::BufferUsages::STORAGE,
                rhi::MemoryLocation::Host,
            )
        };
        let mut buffers = Self {
            layout: rhi::backend::create_bind_group_layout(device, LAYOUT),
            arenas: Vec::new(),
            index_buffer: rhi::backend::Buffer::with_data(
                device,
                "distant_quad_indices",
                bytemuck::cast_slice(&quad_indices),
                rhi::BufferUsages::INDEX,
            ),
            sections: HashMap::new(),
            requested: HashMap::new(),
            requests: Vec::new(),
            dropped: Vec::new(),
            visible: Vec::new(),
            ring: None,
            indirect,
            commands: util::create_per_frame_host_buffers(
                device,
                (MAX_DRAWS * size_of::<DrawCommand>()) as u64,
                rhi::BufferUsages::INDIRECT,
                "distant_draws",
            ),
            positions: util::create_per_frame_host_buffers(
                device,
                POSITIONS_BYTES,
                rhi::BufferUsages::STORAGE,
                "distant_draw_positions",
            ),
            draws: Vec::new(),
            view: None,
            bounds: BoundStore::new(device),
            frame_seq: 0,
            pending_free: VecDeque::new(),
            model_atlas,
            // Voxy's `ModelStore` sampler.
            model_sampler: rhi::backend::create_sampler(
                device,
                &rhi::SamplerDesc {
                    mipmap_filter: rhi::FilterMode::Linear,
                    lod_max_clamp: (MODEL_MIP_LEVELS - 1) as f32,
                    ..rhi::SamplerDesc::NEAREST
                },
            ),
            models: storage("distant_block_models", MODELS_BYTES),
            tints: storage("distant_biome_tints", TINTS_BYTES),
            pending_tiles: VecDeque::new(),
            tile_staging: util::create_per_frame_host_buffers(
                device,
                (TILE_BYTES * TILES_PER_FRAME) as u64,
                rhi::BufferUsages::COPY_SRC,
                "distant_model_staging",
            ),
        };
        buffers.add_arena(device);
        buffers
    }

    pub fn layout(&self) -> rhi::backend::BindGroupLayout {
        self.layout
    }

    fn add_arena(&mut self, device: &rhi::backend::Device) {
        let buffer = rhi::backend::Buffer::new(
            device,
            "distant_quads",
            u64::from(ARENA_QUADS) * 8,
            rhi::BufferUsages::STORAGE,
            rhi::MemoryLocation::Host,
        );
        let group = device.create_bind_group(
            self.layout,
            &[
                (
                    0,
                    rhi::backend::BindingResource::storage(&buffer, u64::from(ARENA_QUADS) * 8),
                ),
                (
                    1,
                    rhi::backend::BindingResource::storage(&self.models, MODELS_BYTES),
                ),
                (
                    2,
                    rhi::backend::BindingResource::storage(&self.tints, TINTS_BYTES),
                ),
                (
                    3,
                    rhi::backend::BindingResource::Texture(self.model_atlas.handle()),
                ),
                (
                    4,
                    rhi::backend::BindingResource::Sampler(self.model_sampler),
                ),
            ],
        );
        self.arenas.push(Arena {
            buffer,
            quads: Tlsf::new(ARENA_QUADS),
            group,
        });
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

    pub fn bounds(&self) -> &BoundStore {
        &self.bounds
    }

    /// Reclaims the ranges and buffers no in-flight frame reads any more.
    /// Call once a frame, after its fence wait.
    pub fn begin_frame(&mut self, device: &rhi::backend::Device) {
        self.frame_seq += 1;
        self.bounds.free_retired(device, self.frame_seq);
        while let Some(&(at, arena, ptr)) = self.pending_free.front()
            && at <= self.frame_seq
        {
            self.pending_free.pop_front();
            self.arenas[arena].quads.free(ptr);
        }
    }

    /// Forgets `key`'s section, freeing its quads once no frame draws them.
    fn retire(&mut self, key: SectionKey) {
        if let Some(old) = self.sections.remove(&key)
            && old.quads > 0
        {
            let at = self.frame_seq + MAX_FRAMES_IN_FLIGHT as u64;
            self.pending_free.push_back((at, old.arena, old.ptr));
        }
    }

    /// Drops every section, as on a dimension change. The bounds stay: they
    /// track the chunk store, which [`Self::clear_bounds`] follows.
    pub fn clear(&mut self) {
        let keys: Vec<SectionKey> = self.sections.keys().copied().collect();
        for key in keys {
            self.retire(key);
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

    /// Room for `count` quads: in an arena with it, in a new one while
    /// there may be more, else by dropping the sections drawn least
    /// recently, whose parents draw in their place.
    fn alloc(&mut self, device: &rhi::backend::Device, count: u32) -> Option<(usize, u32)> {
        loop {
            if let Some(found) = self
                .arenas
                .iter_mut()
                .enumerate()
                .find_map(|(i, arena)| Some((i, arena.quads.alloc(count)?)))
            {
                return Some(found);
            }
            if self.arenas.len() < MAX_ARENAS {
                self.add_arena(device);
                continue;
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
            if freed < count
                || self
                    .pending_free
                    .iter()
                    .any(|&(at, ..)| at > self.frame_seq)
            {
                return None;
            }
        }
    }

    /// Takes the worker's updates. A section with no room yet is dropped
    /// and asked for again once there is.
    pub fn upload(
        &mut self,
        device: &rhi::backend::Device,
        updates: impl IntoIterator<Item = DistantUpdate>,
    ) {
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
                self.requested.remove(&section.key);
                self.place(device, section);
            }
        }
    }

    /// Puts `section` in an arena in place of its last.
    fn place(&mut self, device: &rhi::backend::Device, section: BuiltSection) {
        let count = section.quads.len() as u32;
        let (arena, ptr) = if count == 0 {
            (0, 0)
        } else {
            let Some((arena, ptr)) = self.alloc(device, count) else {
                return;
            };
            self.arenas[arena]
                .buffer
                .write_at(ptr as usize * 8, bytemuck::cast_slice(&section.quads));
            (arena, ptr)
        };
        self.retire(section.key);
        self.sections.insert(
            section.key,
            Uploaded {
                arena,
                ptr,
                quads: count,
                offsets: section.offsets,
                aabb: section.aabb,
                child_existence: section.child_existence,
                last_drawn: self.frame_seq,
            },
        );
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

    pub fn position_buffer(&self, frame: usize) -> &rhi::backend::Buffer {
        &self.positions[frame]
    }

    /// `cmdgen.comp` and `buildtranslucents.comp` on the CPU, until the GPU
    /// traversal: the visible sections' draws into `frame`'s list, an arena
    /// at a time, face groups turned away from the camera left out, each
    /// draw's section in the position list by its `first_instance`; the
    /// translucent draws after, far to near within each arena.
    pub fn build_draws(&mut self, frame: usize, camera: &Camera, eye: DVec3) {
        let eye_offset = (eye - camera.anchor()).as_vec3();
        let camera_section =
            [eye.x, eye.y, eye.z].map(|c| (c.floor() as i32).div_euclid(SIDE as i32));
        let view_proj = camera.distant_view_projection(DISTANT_FAR);
        let planes = Camera::planes_from_view_projection(view_proj);
        // Sections whose quads all lie outside the view go, as Voxy's raster
        // cull of their bounds drops them until the GPU traversal lands.
        // The rest by arena, each with its anchor-relative origin and scale
        // (its position entry) and where it stands from the camera's
        // section at its level.
        let mut visible: Vec<(Uploaded, [f32; 4], [i32; 3])> = self
            .visible
            .iter()
            .filter_map(|&(key, section)| {
                if section.quads == 0 {
                    return None;
                }
                let origin = (origin(&key) - camera.anchor()).as_vec3();
                let corner = origin - eye_offset;
                let (min, max) = section.bounds();
                let scale = (1 << key.level) as f32;
                let cam = camera_section.map(|c| c >> key.level);
                in_frustum(&planes, corner + min * scale, corner + max * scale).then(|| {
                    (
                        section,
                        [origin.x, origin.y, origin.z, scale],
                        [key.x - cam[0], key.y - cam[1], key.z - cam[2]],
                    )
                })
            })
            .take(MAX_DRAW_SECTIONS)
            .collect();
        visible.sort_unstable_by_key(|(section, ..)| section.arena);

        let mut commands: Vec<DrawCommand> = Vec::with_capacity(visible.len() * 4);
        let push =
            |commands: &mut Vec<DrawCommand>, section: &Uploaded, group: usize, id: usize| {
                let (first, count) = section.group(group);
                if count > 0 {
                    commands.push(DrawCommand {
                        index_count: count * 6,
                        instance_count: 1,
                        first_index: 0,
                        vertex_offset: (first * 4) as i32,
                        first_instance: id as u32,
                    });
                }
            };
        let span = |commands: &[DrawCommand], first: u32| (first, commands.len() as u32 - first);
        self.draws = vec![ArenaDraws::default(); self.arenas.len()];
        let mut start = 0;
        for run in visible.chunk_by(|a, b| a.0.arena == b.0.arena) {
            let ids = start..start + run.len();
            start = ids.end;
            let first = commands.len() as u32;
            for (id, (section, _, rel)) in ids.clone().zip(run) {
                push(&mut commands, section, 1, id);
                let facing = [
                    rel[1] > -1,
                    rel[1] < 1,
                    rel[2] > -1,
                    rel[2] < 1,
                    rel[0] > -1,
                    rel[0] < 1,
                ];
                for (face, _) in facing.iter().enumerate().filter(|&(_, &faces)| faces) {
                    push(&mut commands, section, face + 2, id);
                }
            }
            let opaque = span(&commands, first);
            let first = commands.len() as u32;
            let mut order: Vec<(i32, usize)> = ids
                .zip(run)
                .filter(|(_, (section, ..))| section.group(0).1 > 0)
                .map(|(id, (_, position, rel))| {
                    // The section's distance in its level's voxels.
                    (
                        rel.iter().map(|c| c.abs()).sum::<i32>() * position[3] as i32,
                        id,
                    )
                })
                .collect();
            order.sort_unstable_by_key(|&(distance, _)| std::cmp::Reverse(distance));
            for (_, id) in order {
                push(&mut commands, &visible[id].0, 0, id);
            }
            self.draws[run[0].0.arena] = ArenaDraws {
                opaque,
                translucent: span(&commands, first),
            };
        }
        let positions: Vec<[f32; 4]> = visible.iter().map(|(_, position, _)| *position).collect();
        self.commands[frame].write_at(0, bytemuck::cast_slice(&commands));
        self.positions[frame].write_at(0, bytemuck::cast_slice(&positions));
    }

    /// Issues `frame`'s draw list ([`Self::build_draws`]), the opaque or
    /// the translucent draws, an indirect draw per arena. The caller has
    /// bound the matching distant pipeline and its sets.
    pub fn draw(&self, pass: &rhi::backend::RenderPassEncoder, frame: usize, translucent: bool) {
        pass.set_index_buffer(self.index_buffer.handle());
        let stride = size_of::<DrawCommand>() as u32;
        for (arena, draws) in self.arenas.iter().zip(&self.draws) {
            let (first, count) = if translucent {
                draws.translucent
            } else {
                draws.opaque
            };
            if count == 0 {
                continue;
            }
            pass.set_bind_group(1, arena.group);
            let buffer = self.commands[frame].handle();
            let offset = u64::from(first * stride);
            if self.indirect.multi_draw {
                pass.draw_indexed_indirect(buffer, offset, count, stride);
            } else {
                for i in 0..count {
                    pass.draw_indexed_indirect(buffer, offset + u64::from(i * stride), 1, stride);
                }
            }
        }
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        for arena in self.arenas.drain(..) {
            arena.group.destroy(device);
            arena.buffer.destroy(device);
        }
        for buffer in [&mut self.index_buffer, &mut self.models, &mut self.tints] {
            std::mem::take(buffer).destroy(device);
        }
        util::destroy_buffers(device, &mut self.commands);
        util::destroy_buffers(device, &mut self.positions);
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
