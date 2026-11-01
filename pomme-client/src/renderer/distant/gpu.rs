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
use crate::renderer::chunk::tlsf::Tlsf;
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, rhi, util};
use crate::world::chunk::{RING_SIDE, ring_slot};

/// Quads an arena holds: 64 MB at 8 bytes each.
const ARENA_QUADS: u32 = 8 << 20;
/// Arenas there may be: 512 MB of quads.
const MAX_ARENAS: usize = 8;
/// Quads the shared index buffer covers, the most a group can hold.
const QUADS_PER_DRAW: u32 = 1 << 16;
/// `distant.frag` hard-codes the ring's side.
const _: () = assert!(RING_SIDE == 257);
const MASK_WORDS: usize = ((RING_SIDE * RING_SIDE) as usize).div_ceil(32);
const MASK_BYTES: u64 = (MASK_WORDS * size_of::<u32>()) as u64;
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
const FAR: f32 = 3000.0 * 16.0;

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
    /// Each section's last frame drawn, for evicting the least recent.
    last_drawn: HashMap<SectionKey, u64>,
    /// The sections to draw this frame, from the last traversal.
    visible: Vec<SectionKey>,
    view: Option<View>,
    /// Bit per [`ring_slot`]: a real chunk draws there.
    mask: rhi::backend::Buffer,
    mask_words: Vec<u32>,
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
/// and its sampler, then the drawn-column mask `distant.frag` discards
/// with.
const LAYOUT: &[rhi::BindGroupLayoutEntry] = &[
    rhi::BindGroupLayoutEntry::storage(rhi::ShaderStages::VERTEX),
    rhi::BindGroupLayoutEntry::storage(rhi::ShaderStages::VERTEX),
    rhi::BindGroupLayoutEntry::storage(rhi::ShaderStages::VERTEX),
    rhi::BindGroupLayoutEntry::texture(rhi::ShaderStages::FRAGMENT),
    rhi::BindGroupLayoutEntry::sampler(rhi::ShaderStages::FRAGMENT),
    rhi::BindGroupLayoutEntry::storage(rhi::ShaderStages::FRAGMENT),
];

impl DistantBuffers {
    pub fn new(device: &rhi::backend::Device) -> Self {
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
            last_drawn: HashMap::new(),
            visible: Vec::new(),
            view: None,
            mask: storage("distant_mask", MASK_BYTES),
            mask_words: vec![0; MASK_WORDS],
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
                (
                    5,
                    rhi::backend::BindingResource::storage(&self.mask, MASK_BYTES),
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

    /// Reclaims the ranges no in-flight frame reads any more. Call once a
    /// frame, after its fence wait.
    pub fn begin_frame(&mut self) {
        self.frame_seq += 1;
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
        self.last_drawn.remove(&key);
    }

    /// Drops every section, as on a dimension change. The mask stays: it
    /// tracks the chunk store, which [`Self::clear_mask`] follows.
    pub fn clear(&mut self) {
        let keys: Vec<SectionKey> = self.sections.keys().copied().collect();
        for key in keys {
            self.retire(key);
        }
        self.requested.clear();
        self.requests.clear();
        self.dropped.clear();
        self.visible.clear();
        self.view = None;
    }

    pub fn clear_mask(&mut self) {
        self.mask_words.fill(0);
        self.mask.zero();
    }

    pub fn set_column_drawn(&mut self, pos: ChunkPos, drawn: bool) {
        let slot = ring_slot(pos);
        let (word, bit) = (slot / 32, 1u32 << (slot % 32));
        let value = &mut self.mask_words[word];
        if drawn {
            *value |= bit;
        } else {
            *value &= !bit;
        }
        self.mask
            .write_at(word * size_of::<u32>(), &value.to_ne_bytes());
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
                .map(|(key, _)| (self.last_drawn.get(key).copied().unwrap_or(0), *key))
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
    fn children_ready(&self, key: SectionKey) -> bool {
        self.sections.get(&key).is_some_and(|section| {
            children(key, section.child_existence).all(|child| self.sections.contains_key(&child))
        })
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
        let view_proj = camera.distant_view_projection(FAR);
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
        let mut roots = HashSet::new();
        for x in cx - ring..=cx + ring {
            for z in cz - ring..=cz + ring {
                for y in min_sec..=max_sec {
                    let key = SectionKey {
                        level: top,
                        x,
                        y,
                        z,
                    };
                    roots.insert(key);
                    queue.push_back(key);
                }
            }
        }
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
            let has_mesh = self.sections.contains_key(&key);
            if key.level != 0 && screen_size(view_proj, min.as_vec3(), size as f32) > min_sss {
                if self.children_ready(key) {
                    let existence = self.sections[&key].child_existence;
                    queue.extend(children(key, existence));
                } else {
                    match self
                        .sections
                        .get(&key)
                        .map(|section| section.child_existence)
                    {
                        Some(existence) => {
                            for child in children(key, existence) {
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
                    if has_mesh && fx * fx + fz * fz + 16.0 * 16.0 * 16.0 <= render_distance {
                        self.visible.push(key);
                    }
                }
            } else if has_mesh {
                self.visible.push(key);
            } else {
                self.request(key, now);
            }
        }
        for key in &self.visible {
            self.last_drawn.insert(*key, self.frame_seq);
        }
    }

    /// Draws the opaque or see-through groups of the visible sections
    /// (`cmdgen.comp`, `buildtranslucents.comp`): face groups turned away
    /// from the camera are left out; translucent sections go far to near.
    /// The caller has bound the matching distant pipeline.
    pub fn draw(
        &self,
        pass: &rhi::backend::RenderPassEncoder,
        camera: &Camera,
        eye: DVec3,
        translucent: bool,
    ) {
        if self.visible.is_empty() {
            return;
        }
        let view_proj = camera.distant_view_projection(FAR);
        let planes = Camera::planes_from_view_projection(view_proj);
        let anchor = camera.anchor();
        pass.set_push_constants(
            rhi::ShaderStages::VERTEX,
            0,
            bytemuck::cast_slice(&view_proj.to_cols_array()),
        );
        pass.set_index_buffer(self.index_buffer.handle());
        let camera_section =
            [eye.x, eye.y, eye.z].map(|c| (c.floor() as i32).div_euclid(SIDE as i32));
        let relative = |key: &SectionKey| {
            [key.x, key.y, key.z]
                .into_iter()
                .zip(camera_section)
                .map(|(c, cam)| c - (cam >> key.level))
                .collect::<Vec<i32>>()
        };
        // Sections whose quads all lie outside the view go, as Voxy's raster
        // cull of their bounds drops them until the GPU traversal lands.
        let mut order: Vec<&SectionKey> = self
            .visible
            .iter()
            .filter(|key| {
                let section = &self.sections[key];
                if section.quads == 0 || (translucent && section.group(0).1 == 0) {
                    return false;
                }
                let corner = (origin(key) - eye).as_vec3();
                let (min, max) = section.bounds();
                let scale = (1 << key.level) as f32;
                in_frustum(&planes, corner + min * scale, corner + max * scale)
            })
            .collect();
        if translucent {
            order.sort_by_key(|key| {
                let rel = relative(key);
                std::cmp::Reverse(rel.iter().map(|c| c.abs()).sum::<i32>() << key.level)
            });
        } else {
            order.sort_unstable_by_key(|key| self.sections[key].arena);
        }
        let mut bound = None;
        for key in order {
            let section = &self.sections[key];
            if bound != Some(section.arena) {
                pass.set_bind_group(1, self.arenas[section.arena].group);
                bound = Some(section.arena);
            }
            let origin = (origin(key) - anchor).as_vec3();
            pass.set_push_constants(
                rhi::ShaderStages::VERTEX,
                64,
                bytemuck::cast_slice(&[origin.x, origin.y, origin.z, (1 << key.level) as f32]),
            );
            let groups: Vec<usize> = if translucent {
                vec![0]
            } else {
                let rel = relative(key);
                let facing = [
                    rel[1] > -1,
                    rel[1] < 1,
                    rel[2] > -1,
                    rel[2] < 1,
                    rel[0] > -1,
                    rel[0] < 1,
                ];
                std::iter::once(1)
                    .chain((0..6).filter(|&face| facing[face]).map(|face| face + 2))
                    .collect()
            };
            for group in groups {
                let (first, count) = section.group(group);
                if count == 0 {
                    continue;
                }
                pass.draw_indexed(count * 6, 1, 0, (first * 4) as i32, 0);
            }
        }
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        for arena in self.arenas.drain(..) {
            arena.group.destroy(device);
            arena.buffer.destroy(device);
        }
        for buffer in [
            &mut self.index_buffer,
            &mut self.mask,
            &mut self.models,
            &mut self.tints,
        ] {
            std::mem::take(buffer).destroy(device);
        }
        util::destroy_buffers(device, &mut self.tile_staging);
        self.model_sampler.destroy(device);
        std::mem::take(&mut self.model_atlas).destroy(device);
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
