use std::collections::HashMap;
use std::sync::Arc;

use azalea_block::BlockState;
use azalea_core::position::ChunkPos;
use binary_greedy_meshing as bgm;

use crate::renderer::chunk::atlas::{AtlasRegion, AtlasUVMap};
use crate::world::block::model::{BakedModel, Direction};
use crate::world::block::registry::{BlockRegistry, FaceTextures, Tint};
use crate::world::chunk::{self, ChunkStore};

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ChunkVertex {
    pub position: [f32; 3],
    pub tex_coords: [f32; 2],
    pub light: f32,
    pub tint: [f32; 3],
}

pub struct ChunkMeshData {
    pub pos: ChunkPos,
    pub vertices: Vec<ChunkVertex>,
    pub indices: Vec<u32>,
}

const WHITE: [f32; 3] = [1.0, 1.0, 1.0];
const GRASS_TINT: [f32; 3] = [0.569, 0.741, 0.349];
const FOLIAGE_TINT: [f32; 3] = [0.467, 0.671, 0.184];

fn tint_color(tint: Tint) -> [f32; 3] {
    match tint {
        Tint::None => WHITE,
        Tint::Grass => GRASS_TINT,
        Tint::Foliage => FOLIAGE_TINT,
    }
}

const MAX_MESH_UPLOADS_PER_FRAME: usize = 16;

pub struct MeshDispatcher {
    result_rx: crossbeam_channel::Receiver<ChunkMeshData>,
    result_tx: crossbeam_channel::Sender<ChunkMeshData>,
    registry: Arc<BlockRegistry>,
    uv_map: Arc<AtlasUVMap>,
}

impl MeshDispatcher {
    pub fn new(registry: BlockRegistry, uv_map: AtlasUVMap) -> Self {
        let (result_tx, result_rx) = crossbeam_channel::unbounded();
        Self {
            result_rx,
            result_tx,
            registry: Arc::new(registry),
            uv_map: Arc::new(uv_map),
        }
    }

    pub fn enqueue(&self, chunk_store: &ChunkStore, pos: ChunkPos, lod: u32) {
        let registry = Arc::clone(&self.registry);
        let uv_map = Arc::clone(&self.uv_map);
        let tx = self.result_tx.clone();

        let chunks_needed = [
            pos,
            ChunkPos::new(pos.x - 1, pos.z),
            ChunkPos::new(pos.x + 1, pos.z),
            ChunkPos::new(pos.x, pos.z - 1),
            ChunkPos::new(pos.x, pos.z + 1),
        ];
        let chunk_arcs: Vec<_> = chunks_needed
            .iter()
            .map(|p| chunk_store.get_chunk(p))
            .collect();

        let min_y = chunk_store.min_y();
        let height = chunk_store.height();

        rayon::spawn(move || {
            let snapshot = ChunkStoreSnapshot {
                chunks: chunks_needed.into_iter().zip(chunk_arcs).collect(),
                min_y,
                height,
            };
            let mesh = mesh_chunk_snapshot(&snapshot, pos, &registry, &uv_map, lod);
            let _ = tx.send(mesh);
        });
    }

    pub fn drain_results(&self) -> impl Iterator<Item = ChunkMeshData> + '_ {
        self.result_rx.try_iter().take(MAX_MESH_UPLOADS_PER_FRAME)
    }
}

struct ChunkStoreSnapshot {
    chunks: Vec<(
        ChunkPos,
        Option<Arc<parking_lot::RwLock<azalea_world::chunk_storage::Chunk>>>,
    )>,
    min_y: i32,
    height: u32,
}

impl ChunkStoreSnapshot {
    fn get_block_state(&self, x: i32, y: i32, z: i32) -> azalea_block::BlockState {
        let chunk_pos = ChunkPos::new(x.div_euclid(16), z.div_euclid(16));
        let chunk_lock = self
            .chunks
            .iter()
            .find(|(p, _)| *p == chunk_pos)
            .and_then(|(_, c)| c.as_ref());

        let Some(chunk_lock) = chunk_lock else {
            return azalea_block::BlockState::AIR;
        };

        let c = chunk_lock.read();
        chunk::block_state_from_section(&c, x, y, z, self.min_y)
    }

    fn min_y(&self) -> i32 {
        self.min_y
    }

    fn height(&self) -> u32 {
        self.height
    }
}

struct GreedyBlockInfo {
    textures: FaceTextures,
}

struct BlockTypeMap {
    state_to_id: HashMap<BlockState, u16>,
    id_to_info: Vec<GreedyBlockInfo>,
}

impl BlockTypeMap {
    fn build(
        snapshot: &ChunkStoreSnapshot,
        registry: &BlockRegistry,
        world_x: i32,
        world_z: i32,
        min_y: i32,
        max_y: i32,
    ) -> Self {
        let mut state_to_id = HashMap::new();
        let mut id_to_info: Vec<GreedyBlockInfo> = Vec::new();
        let mut next_id = 1u16;

        for lz in -1..17i32 {
            for lx in -1..17i32 {
                let bx = world_x + lx;
                let bz = world_z + lz;
                for by in (min_y - 1)..=(max_y) {
                    let state = snapshot.get_block_state(bx, by, bz);
                    if state.is_air() || state_to_id.contains_key(&state) {
                        continue;
                    }
                    let has_baked = registry.get_baked_model(state).is_some();
                    let has_multipart = registry.get_multipart_quads(state).is_some();
                    if has_baked || has_multipart {
                        state_to_id.insert(state, 0);
                        continue;
                    }
                    if let Some(textures) = registry.get_textures(state) {
                        if textures.side_overlay.is_some() || !registry.is_opaque_full_cube(state) {
                            state_to_id.insert(state, 0);
                            continue;
                        }
                        state_to_id.insert(state, next_id);
                        id_to_info.push(GreedyBlockInfo {
                            textures: textures.clone(),
                        });
                        next_id += 1;
                    } else {
                        state_to_id.insert(state, 0);
                    }
                }
            }
        }

        Self {
            state_to_id,
            id_to_info,
        }
    }

    fn get_id(&self, state: BlockState) -> u16 {
        if state.is_air() {
            return 0;
        }
        self.state_to_id.get(&state).copied().unwrap_or(0)
    }

    fn get_info(&self, id: u16) -> Option<&GreedyBlockInfo> {
        if id == 0 {
            return None;
        }
        self.id_to_info.get((id - 1) as usize)
    }
}

const SECTION_SIZE: usize = 16;

fn greedy_face_light(face: bgm::Face) -> f32 {
    match face {
        bgm::Face::Up => 1.0,
        bgm::Face::Down => 0.5,
        bgm::Face::Right | bgm::Face::Left => 0.8,
        bgm::Face::Front | bgm::Face::Back => 0.7,
    }
}

fn face_texture_name(textures: &FaceTextures, face: bgm::Face) -> &str {
    match face {
        bgm::Face::Up => &textures.top,
        bgm::Face::Down => &textures.bottom,
        bgm::Face::Right => &textures.east,
        bgm::Face::Left => &textures.west,
        bgm::Face::Front => &textures.south,
        bgm::Face::Back => &textures.north,
    }
}

#[allow(clippy::too_many_arguments)]
fn greedy_mesh_section(
    vertices: &mut Vec<ChunkVertex>,
    indices: &mut Vec<u32>,
    snapshot: &ChunkStoreSnapshot,
    type_map: &BlockTypeMap,
    uv_map: &AtlasUVMap,
    world_x: i32,
    section_y: i32,
    world_z: i32,
) {
    let mut mesher = bgm::Mesher::<SECTION_SIZE>::new();
    let mut voxels = [0u16; bgm::Mesher::<SECTION_SIZE>::CS_P3];

    for lz in 0..18 {
        for lx in 0..18 {
            for ly in 0..18 {
                let bx = world_x + lx as i32 - 1;
                let by = section_y + ly as i32 - 1;
                let bz = world_z + lz as i32 - 1;
                let state = snapshot.get_block_state(bx, by, bz);
                let id = type_map.get_id(state);
                let cs_p = SECTION_SIZE + 2;
                let idx = (ly * cs_p + lx) * cs_p + lz;
                voxels[idx] = id;
            }
        }
    }

    let transparent_set = std::collections::BTreeSet::new();
    mesher.mesh(&voxels, &transparent_set);

    for face_idx in 0..6u8 {
        let face = bgm::Face::from(face_idx);
        let light = greedy_face_light(face);

        for quad in &mesher.quads[face_idx as usize] {
            let block_id = quad.voxel_id() as u16;

            let info = match type_map.get_info(block_id) {
                Some(i) => i,
                None => continue,
            };

            let tex_name = face_texture_name(&info.textures, face);
            let region = uv_map.get_region(tex_name);
            let tint = tint_color(info.textures.tint);

            let packed_verts = face.vertices_packed(*quad);
            let base = vertices.len() as u32;

            let u_span = region.u_max - region.u_min;
            let v_span = region.v_max - region.v_min;

            for pv in &packed_verts {
                let x = pv.x() as f32 + world_x as f32;
                let y = pv.y() as f32 + section_y as f32;
                let z = pv.z() as f32 + world_z as f32;
                let u = region.u_min + pv.u() as f32 * u_span;
                let v = region.v_min + pv.v() as f32 * v_span;

                vertices.push(ChunkVertex {
                    position: [x, y, z],
                    tex_coords: [u, v],
                    light,
                    tint,
                });
            }

            indices.extend_from_slice(&[base, base + 1, base + 2, base + 1, base + 3, base + 2]);
        }
    }
}

fn mesh_chunk_snapshot(
    snapshot: &ChunkStoreSnapshot,
    pos: ChunkPos,
    registry: &BlockRegistry,
    uv_map: &AtlasUVMap,
    lod: u32,
) -> ChunkMeshData {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut logged_missing: std::collections::HashSet<String> = std::collections::HashSet::new();

    let step = 1i32 << lod;
    let scale = step as f32;

    let min_y = snapshot.min_y();
    let max_y = min_y + snapshot.height() as i32;
    let world_x = pos.x * 16;
    let world_z = pos.z * 16;

    let type_map = if lod == 0 {
        Some(BlockTypeMap::build(
            snapshot, registry, world_x, world_z, min_y, max_y,
        ))
    } else {
        None
    };

    if let Some(ref tm) = type_map {
        let sections = (max_y - min_y) / 16;
        for section in 0..sections {
            let section_y = min_y + section * 16;
            greedy_mesh_section(
                &mut vertices,
                &mut indices,
                snapshot,
                tm,
                uv_map,
                world_x,
                section_y,
                world_z,
            );
        }
    }

    let mut local_z = 0i32;
    while local_z < 16 {
        let mut local_x = 0i32;
        while local_x < 16 {
            let bx = world_x + local_x;
            let bz = world_z + local_z;

            let mut by = min_y;
            while by < max_y {
                let state = snapshot.get_block_state(bx, by, bz);
                let kind = classify_block(state);
                if matches!(kind, BlockKind::Air) {
                    by += step;
                    continue;
                }

                if lod == 0 {
                    if let Some(ref tm) = type_map {
                        if tm.get_id(state) != 0 {
                            by += step;
                            continue;
                        }
                    }
                }

                let block_pos = [bx as f32, by as f32, bz as f32];

                if lod > 0 {
                    emit_lod_cube(
                        &mut vertices,
                        &mut indices,
                        block_pos,
                        scale,
                        state,
                        snapshot,
                        registry,
                        uv_map,
                        bx,
                        by,
                        bz,
                        step,
                    );
                } else if let BlockKind::Water | BlockKind::Lava = kind {
                    let fluid = if matches!(kind, BlockKind::Lava) {
                        "lava"
                    } else {
                        "water"
                    };
                    emit_fluid(
                        &mut vertices,
                        &mut indices,
                        block_pos,
                        fluid,
                        snapshot,
                        registry,
                        uv_map,
                        bx,
                        by,
                        bz,
                    );
                } else if let Some(baked) = registry.get_baked_model(state) {
                    emit_baked_model(
                        &mut vertices,
                        &mut indices,
                        block_pos,
                        baked,
                        snapshot,
                        registry,
                        uv_map,
                        bx,
                        by,
                        bz,
                    );
                } else if let Some(quads) = registry.get_multipart_quads(state) {
                    emit_multipart(
                        &mut vertices,
                        &mut indices,
                        block_pos,
                        &quads,
                        snapshot,
                        registry,
                        uv_map,
                        bx,
                        by,
                        bz,
                    );
                } else if let Some(textures) = registry.get_textures(state) {
                    emit_cube_faces(
                        &mut vertices,
                        &mut indices,
                        block_pos,
                        textures,
                        snapshot,
                        registry,
                        uv_map,
                        bx,
                        by,
                        bz,
                    );
                } else {
                    let block: Box<dyn azalea_block::BlockTrait> = state.into();
                    let id = block.id().to_string();
                    if logged_missing.insert(id.clone()) {
                        log::warn!("Missing model: {id}");
                    }
                    emit_missing_cube(
                        &mut vertices,
                        &mut indices,
                        block_pos,
                        snapshot,
                        registry,
                        bx,
                        by,
                        bz,
                    );
                }
                by += step;
            }
            local_x += step;
        }
        local_z += step;
    }

    ChunkMeshData {
        pos,
        vertices,
        indices,
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_baked_model(
    vertices: &mut Vec<ChunkVertex>,
    indices: &mut Vec<u32>,
    block_pos: [f32; 3],
    model: &BakedModel,
    snapshot: &ChunkStoreSnapshot,
    registry: &BlockRegistry,
    uv_map: &AtlasUVMap,
    bx: i32,
    by: i32,
    bz: i32,
) {
    for quad in &model.quads {
        if let Some(cullface) = quad.cullface {
            let offset = cullface.offset();
            let neighbor = snapshot.get_block_state(bx + offset[0], by + offset[1], bz + offset[2]);
            if registry.is_opaque_full_cube(neighbor) {
                continue;
            }
        }

        let region = uv_map.get_region(&quad.texture);
        let tint = if quad.tinted { GRASS_TINT } else { WHITE };
        emit_face(
            vertices,
            indices,
            block_pos,
            &quad.positions,
            &quad.uvs,
            quad.shade_light,
            region,
            tint,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_cube_faces(
    vertices: &mut Vec<ChunkVertex>,
    indices: &mut Vec<u32>,
    block_pos: [f32; 3],
    textures: &crate::world::block::registry::FaceTextures,
    snapshot: &ChunkStoreSnapshot,
    registry: &BlockRegistry,
    uv_map: &AtlasUVMap,
    bx: i32,
    by: i32,
    bz: i32,
) {
    let tint = tint_color(textures.tint);

    for (i, dir) in CUBE_FACE_DIRS.iter().enumerate() {
        let offset = dir.offset();
        let neighbor = snapshot.get_block_state(bx + offset[0], by + offset[1], bz + offset[2]);
        if registry.is_opaque_full_cube(neighbor) {
            continue;
        }

        let face_tex = match i {
            0 => &textures.top,
            1 => &textures.bottom,
            2 => &textures.north,
            3 => &textures.south,
            4 => &textures.east,
            _ => &textures.west,
        };
        let region = uv_map.get_region(face_tex);
        let (positions, uvs, light) = cube_face_geometry(*dir);

        let is_side = i >= 2;
        if let Some(overlay) = textures.side_overlay.as_deref().filter(|_| is_side) {
            emit_face(
                vertices, indices, block_pos, &positions, &uvs, light, region, WHITE,
            );
            let overlay_region = uv_map.get_region(overlay);
            emit_face(
                vertices,
                indices,
                block_pos,
                &positions,
                &uvs,
                light,
                overlay_region,
                tint,
            );
        } else {
            let is_tinted =
                !matches!(textures.tint, Tint::None) && (textures.side_overlay.is_none() || i == 0);
            let face_tint = if is_tinted { tint } else { WHITE };
            emit_face(
                vertices, indices, block_pos, &positions, &uvs, light, region, face_tint,
            );
        }
    }
}

enum BlockKind {
    Air,
    Water,
    Lava,
    Solid,
}

fn classify_block(state: azalea_block::BlockState) -> BlockKind {
    if state.is_air() {
        return BlockKind::Air;
    }
    let block: Box<dyn azalea_block::BlockTrait> = state.into();
    match block.id() {
        "cave_air" | "void_air" | "light" | "barrier" | "structure_void" | "moving_piston" => {
            BlockKind::Air
        }
        "water" | "bubble_column" => BlockKind::Water,
        "lava" => BlockKind::Lava,
        _ => BlockKind::Solid,
    }
}

// TODO: biome-based water color
// TODO: per-corner height averaging for smooth water surfaces
// TODO: flowing water texture (water_flow) with direction-based rotation
// TODO: per-level height for flowing water (level / 9.0 per corner)

const FLUID_MAX_HEIGHT: f32 = 8.0 / 9.0;

#[allow(clippy::too_many_arguments)]
fn emit_fluid(
    vertices: &mut Vec<ChunkVertex>,
    indices: &mut Vec<u32>,
    block_pos: [f32; 3],
    fluid: &str,
    snapshot: &ChunkStoreSnapshot,
    registry: &BlockRegistry,
    uv_map: &AtlasUVMap,
    bx: i32,
    by: i32,
    bz: i32,
) {
    let (tex_name, tint) = if fluid == "lava" {
        ("lava_still", [1.0, 1.0, 1.0])
    } else {
        ("water_still", [0.247, 0.463, 0.894])
    };
    let region = uv_map.get_region(tex_name);

    for dir in &CUBE_FACE_DIRS {
        let offset = dir.offset();
        let neighbor = snapshot.get_block_state(bx + offset[0], by + offset[1], bz + offset[2]);

        if matches!(classify_block(neighbor), BlockKind::Water | BlockKind::Lava)
            || registry.is_opaque_full_cube(neighbor)
        {
            continue;
        }

        let (mut positions, uvs, light) = cube_face_geometry(*dir);

        if matches!(dir, Direction::Up) {
            let above = snapshot.get_block_state(bx, by + 1, bz);
            let top = if matches!(classify_block(above), BlockKind::Water | BlockKind::Lava) {
                1.0
            } else {
                FLUID_MAX_HEIGHT
            };
            for p in &mut positions {
                p[1] = top;
            }
        }

        emit_face(
            vertices, indices, block_pos, &positions, &uvs, light, region, tint,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_multipart(
    vertices: &mut Vec<ChunkVertex>,
    indices: &mut Vec<u32>,
    block_pos: [f32; 3],
    quads: &[&crate::world::block::model::BakedQuad],
    snapshot: &ChunkStoreSnapshot,
    registry: &BlockRegistry,
    uv_map: &AtlasUVMap,
    bx: i32,
    by: i32,
    bz: i32,
) {
    for quad in quads {
        if let Some(cullface) = quad.cullface {
            let offset = cullface.offset();
            let neighbor = snapshot.get_block_state(bx + offset[0], by + offset[1], bz + offset[2]);
            if registry.is_opaque_full_cube(neighbor) {
                continue;
            }
        }

        let region = uv_map.get_region(&quad.texture);
        let tint = if quad.tinted { GRASS_TINT } else { WHITE };
        emit_face(
            vertices,
            indices,
            block_pos,
            &quad.positions,
            &quad.uvs,
            quad.shade_light,
            region,
            tint,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_lod_cube(
    vertices: &mut Vec<ChunkVertex>,
    indices: &mut Vec<u32>,
    block_pos: [f32; 3],
    scale: f32,
    state: azalea_block::BlockState,
    snapshot: &ChunkStoreSnapshot,
    registry: &BlockRegistry,
    uv_map: &AtlasUVMap,
    bx: i32,
    by: i32,
    bz: i32,
    step: i32,
) {
    let region = if let Some(textures) = registry.get_textures(state) {
        let tint = tint_color(textures.tint);
        let tex = uv_map.get_region(&textures.top);
        (tex, tint)
    } else {
        (uv_map.get_region(""), MISSING_TINT)
    };

    for dir in &CUBE_FACE_DIRS {
        let offset = dir.offset();
        let nx = bx + offset[0] * step;
        let ny = by + offset[1] * step;
        let nz = bz + offset[2] * step;
        let neighbor = snapshot.get_block_state(nx, ny, nz);
        if registry.is_opaque_full_cube(neighbor) {
            continue;
        }

        let (positions, uvs, light) = cube_face_geometry(*dir);
        let base = vertices.len() as u32;
        for i in 0..4 {
            vertices.push(ChunkVertex {
                position: [
                    block_pos[0] + positions[i][0] * scale,
                    block_pos[1] + positions[i][1] * scale,
                    block_pos[2] + positions[i][2] * scale,
                ],
                tex_coords: [
                    region.0.u_min + uvs[i][0] * (region.0.u_max - region.0.u_min),
                    region.0.v_min + uvs[i][1] * (region.0.v_max - region.0.v_min),
                ],
                light,
                tint: region.1,
            });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 3, base]);
    }
}

const MISSING_TINT: [f32; 3] = [1.0, 0.0, 1.0];

#[allow(clippy::too_many_arguments)]
fn emit_missing_cube(
    vertices: &mut Vec<ChunkVertex>,
    indices: &mut Vec<u32>,
    block_pos: [f32; 3],
    snapshot: &ChunkStoreSnapshot,
    registry: &BlockRegistry,
    bx: i32,
    by: i32,
    bz: i32,
) {
    for dir in &CUBE_FACE_DIRS {
        let offset = dir.offset();
        let neighbor = snapshot.get_block_state(bx + offset[0], by + offset[1], bz + offset[2]);
        if registry.is_opaque_full_cube(neighbor) {
            continue;
        }

        let (positions, _, light) = cube_face_geometry(*dir);
        let base = vertices.len() as u32;
        for pos in &positions {
            vertices.push(ChunkVertex {
                position: [
                    block_pos[0] + pos[0],
                    block_pos[1] + pos[1],
                    block_pos[2] + pos[2],
                ],
                tex_coords: [0.0, 0.0],
                light,
                tint: MISSING_TINT,
            });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 3, base]);
    }
}

const CUBE_FACE_DIRS: [Direction; 6] = [
    Direction::Up,
    Direction::Down,
    Direction::North,
    Direction::South,
    Direction::East,
    Direction::West,
];

#[allow(clippy::too_many_arguments)]
fn emit_face(
    vertices: &mut Vec<ChunkVertex>,
    indices: &mut Vec<u32>,
    block_pos: [f32; 3],
    positions: &[[f32; 3]; 4],
    uvs: &[[f32; 2]; 4],
    light: f32,
    region: AtlasRegion,
    tint: [f32; 3],
) {
    let base = vertices.len() as u32;
    let u_span = region.u_max - region.u_min;
    let v_span = region.v_max - region.v_min;

    for i in 0..4 {
        vertices.push(ChunkVertex {
            position: [
                block_pos[0] + positions[i][0],
                block_pos[1] + positions[i][1],
                block_pos[2] + positions[i][2],
            ],
            tex_coords: [
                region.u_min + uvs[i][0] * u_span,
                region.v_min + uvs[i][1] * v_span,
            ],
            light,
            tint,
        });
    }

    indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 3, base]);
}

fn cube_face_geometry(dir: Direction) -> ([[f32; 3]; 4], [[f32; 2]; 4], f32) {
    match dir {
        Direction::Up => (
            [
                [0.0, 1.0, 1.0],
                [1.0, 1.0, 1.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
            ],
            [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
            1.0,
        ),
        Direction::Down => (
            [
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 0.0, 1.0],
                [0.0, 0.0, 1.0],
            ],
            [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            0.5,
        ),
        Direction::North => (
            [
                [0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
            ],
            [[0.0, 1.0], [0.0, 0.0], [1.0, 0.0], [1.0, 1.0]],
            0.7,
        ),
        Direction::South => (
            [
                [1.0, 0.0, 1.0],
                [1.0, 1.0, 1.0],
                [0.0, 1.0, 1.0],
                [0.0, 0.0, 1.0],
            ],
            [[1.0, 1.0], [1.0, 0.0], [0.0, 0.0], [0.0, 1.0]],
            0.7,
        ),
        Direction::East => (
            [
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [1.0, 1.0, 1.0],
                [1.0, 0.0, 1.0],
            ],
            [[1.0, 1.0], [1.0, 0.0], [0.0, 0.0], [0.0, 1.0]],
            0.8,
        ),
        Direction::West => (
            [
                [0.0, 0.0, 1.0],
                [0.0, 1.0, 1.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0],
            ],
            [[1.0, 1.0], [1.0, 0.0], [0.0, 0.0], [0.0, 1.0]],
            0.8,
        ),
    }
}
