use std::collections::HashMap;
use std::sync::Arc;

use azalea_block::BlockState;
use azalea_core::position::ChunkSectionPos;
use bytemuck::Zeroable;

use super::greedy_face::{block_index, GreedyFaceRecord};
use super::greedy_merge::{greedy_merge, pack_solid_terrain, RawGreedyFace};
use crate::renderer::chunk::atlas::AtlasUVMap;
use crate::renderer::chunk::section::LocalSection;
use crate::world::block::model::{BakedCuboid, BakedModel, Direction, face_positions, face_uvs};
use crate::world::block::registry::{BlockRegistry, Tint};
use crate::world::block::{block_id, is_air};
use crate::world::chunk::ChunkStore;

/// Pack biome tint into the greedy terrain tint-table layout (`greedyTint`).
pub fn pack_tint_rgb(rgb: [f32; 3]) -> u32 {
    let r = (rgb[0].clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    let g = (rgb[1].clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    let b = (rgb[2].clamp(0.0, 1.0) * 127.0 + 0.5) as u32;
    r | (g << 8) | (b << 16)
}

/// White tint in [`pack_tint_rgb`] layout; index 0 in every tint table.
pub const PACKED_WHITE_RGB: u32 = 0x007F_FFFF;

include!("packing_consts.rs");

/// Legacy fluid face: low 12 bits are window-local quad ID, upper 20 bits are
/// four u5 combined-shade values.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct FaceRecord {
    pub packed: u32,
}

impl FaceRecord {
    pub fn new(quad_id: u16, shades: [u8; 4]) -> Self {
        assert!(u32::from(quad_id) <= MAX_QUAD_ID);
        let mut packed = u32::from(quad_id);
        for (i, shade) in shades.into_iter().enumerate() {
            packed |= u32::from(shade.min(31)) << (12 + i * 5);
        }
        Self { packed }
    }

    #[cfg(test)]
    pub fn fields(self) -> (u16, [u8; 4]) {
        (
            (self.packed & 0xfff) as u16,
            std::array::from_fn(|i| ((self.packed >> (12 + i * 5)) & 31) as u8),
        )
    }
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CuboidFaceData {
    pub positions: [[f32; 4]; 4],
    pub uvs: [[f32; 4]; 4],
    /// x=face present, y=apply section tint. Remaining lanes are reserved.
    pub material: [u32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CuboidData {
    pub faces: [CuboidFaceData; 6],
}

#[derive(Clone)]
pub struct GlobalCuboidTable {
    pub data: Vec<CuboidData>,
    ids: HashMap<u32, u16>,
    missing_id: u16,
    water_id: u16,
    lava_id: u16,
}

impl GlobalCuboidTable {
    fn id(&self, uid: u32) -> u16 {
        *self
            .ids
            .get(&uid)
            .expect("baked cuboid missing from global table")
    }
}

pub fn build_global_cuboid_table(
    registry: &crate::world::block::registry::BlockRegistry,
    uv_map: &AtlasUVMap,
) -> GlobalCuboidTable {
    fn intern(
        data: &mut Vec<CuboidData>,
        dedup: &mut HashMap<Vec<u8>, u16>,
        cuboid: CuboidData,
    ) -> u16 {
        let key = bytemuck::bytes_of(&cuboid).to_vec();
        if let Some(&id) = dedup.get(&key) {
            return id;
        }
        let id = u16::try_from(data.len()).expect("global cuboid table exceeds u16 ID space");
        data.push(cuboid);
        dedup.insert(key, id);
        id
    }

    let mut data = Vec::new();
    let mut dedup = HashMap::new();
    let mut ids = HashMap::new();
    for baked in registry.all_baked_cuboids() {
        let mut cuboid = CuboidData::zeroed();
        for face in &baked.faces {
            let region = uv_map.get_region(&face.texture);
            let u_span = region.u_max - region.u_min;
            let v_span = region.v_max - region.v_min;
            cuboid.faces[face.direction.index()] = CuboidFaceData {
                positions: std::array::from_fn(|i| {
                    [
                        face.positions[i][0],
                        face.positions[i][1],
                        face.positions[i][2],
                        1.0,
                    ]
                }),
                uvs: std::array::from_fn(|i| {
                    [
                        region.u_min + face.uvs[i][0] * u_span,
                        region.v_min + face.uvs[i][1] * v_span,
                        if i == 0 {
                            region.u_min
                        } else if i == 1 {
                            region.u_max
                        } else {
                            0.0
                        },
                        if i == 0 {
                            region.v_min
                        } else if i == 1 {
                            region.v_max
                        } else {
                            0.0
                        },
                    ]
                }),
                material: [1, u32::from(!matches!(face.tint, Tint::None)), 0, 0],
            };
        }
        let id = intern(&mut data, &mut dedup, cuboid);
        ids.insert(baked.uid, id);
    }
    let mut append_cube = |texture: &str, tinted: bool| {
        let region = uv_map.get_region(texture);
        let mut cuboid = CuboidData::zeroed();
        for direction in CUBE_FACE_DIRS {
            let (positions, uvs, _) = cube_face_geometry(direction);
            let u_span = region.u_max - region.u_min;
            let v_span = region.v_max - region.v_min;
            cuboid.faces[direction.index()] = CuboidFaceData {
                positions: positions.map(|p| [p[0], p[1], p[2], 1.0]),
                uvs: std::array::from_fn(|i| {
                    [
                        region.u_min + uvs[i][0] * u_span,
                        region.v_min + uvs[i][1] * v_span,
                        if i == 0 {
                            region.u_min
                        } else if i == 1 {
                            region.u_max
                        } else {
                            0.0
                        },
                        if i == 0 {
                            region.v_min
                        } else if i == 1 {
                            region.v_max
                        } else {
                            0.0
                        },
                    ]
                }),
                material: [1, u32::from(tinted), 0, 0],
            };
        }
        intern(&mut data, &mut dedup, cuboid)
    };
    let missing_id = append_cube("", false);
    let water_id = append_cube("water_still", true);
    let lava_id = append_cube("lava_still", false);
    GlobalCuboidTable {
        data,
        ids,
        missing_id,
        water_id,
        lava_id,
    }
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SectionCuboid {
    pub packed: u64,
}

impl SectionCuboid {
    pub fn new(pos: [u8; 3], global_id: u16, tint: [f32; 3]) -> Self {
        let r = (tint[0].clamp(0.0, 1.0) * 255.0 + 0.5) as u64;
        let g = (tint[1].clamp(0.0, 1.0) * 255.0 + 0.5) as u64;
        let b = (tint[2].clamp(0.0, 1.0) * 127.0 + 0.5) as u64;
        Self {
            packed: u64::from(pos[0])
                | (u64::from(pos[1]) << 4)
                | (u64::from(pos[2]) << 8)
                | (u64::from(global_id) << 12)
                | (r << 28)
                | (g << 36)
                | (b << 44),
        }
    }
}

pub type PackedFace = FaceRecord;
/// Number of cuboids addressable by one face-record window. Integer division
/// intentionally leaves quad IDs 4092..4095 unused.
pub const MAX_CUBOIDS_PER_WINDOW: u32 = (1 << 12) / 6;
pub const MAX_QUAD_ID: u32 = (MAX_CUBOIDS_PER_WINDOW - 1) * 6 + 5;

/// Window formula shared by batching and tests:
/// `window = cuboid / 682`, `base = window * 682`, `local = cuboid - base`.
pub const fn cuboid_window(cuboid: u32) -> (u32, u32, u32) {
    let window = cuboid / MAX_CUBOIDS_PER_WINDOW;
    let base = window * MAX_CUBOIDS_PER_WINDOW;
    (window, base, cuboid - base)
}

#[derive(Clone, Copy, Debug)]
pub struct FaceBatch {
    pub face_offset: u32,
    pub face_count: u32,
    /// Solid batches: u32 index into the section tint table. Fluid batches:
    /// cuboid index within the fluid cuboid subrange.
    pub table_offset: u32,
    pub cull: BatchCull,
    pub aabb_min: [f32; 3],
    pub aabb_max: [f32; 3],
}

/// Conservative whole-batch backface classification. The numeric values are
/// shared with `batch_cull.glsl`; the six axis values match `Direction::index`.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatchCull {
    Down = 0,
    Up = 1,
    North = 2,
    South = 3,
    West = 4,
    East = 5,
    Uncullable = 6,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BatchGranularity {
    Coarse,
    Directional,
}

impl BatchCull {
    const fn from_direction(direction: Direction) -> Self {
        match direction {
            Direction::Down => Self::Down,
            Direction::Up => Self::Up,
            Direction::North => Self::North,
            Direction::South => Self::South,
            Direction::West => Self::West,
            Direction::East => Self::East,
        }
    }
}

pub const BATCH_CULL_SHIFT: u32 = 29;
pub const BATCH_FACE_COUNT_MASK: u32 = (1 << BATCH_CULL_SHIFT) - 1;

impl FaceBatch {
    pub const fn packed_face_count(self) -> u32 {
        assert!(self.face_count <= BATCH_FACE_COUNT_MASK);
        self.face_count | ((self.cull as u32) << BATCH_CULL_SHIFT)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FaceBatchCounts {
    pub regular_solid: u32,
    pub opaque_fluid: u32,
    pub translucent_fluid: u32,
}

impl FaceBatchCounts {
    pub const fn solid_draws(self) -> u32 {
        self.regular_solid + self.opaque_fluid
    }
}

/// Sixteen u32 words embedded in the section allocation. `firstInstance` is the
/// absolute word offset of this record in the mesh buffer.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuFaceBatch {
    pub face_word_offset: u32,
    pub face_count_and_cull: u32,
    /// Solid batches: tint-table word offset. Fluid batches: section-cuboid
    /// word offset.
    pub table_word_offset: u32,
    pub fluid_height_word_offset: u32,
    pub origin: [i32; 3],
    pub uploaded_ms: u32,
    pub aabb_min: [f32; 4],
    pub aabb_max: [f32; 4],
}

#[derive(Clone, Copy)]
pub struct SectionMeshLayout {
    pub regular_faces: usize,
    pub tint_table: usize,
    pub fluid_faces: usize,
    pub fluid_cuboids: usize,
    pub fluid_heights: usize,
    pub batches: usize,
    pub size: usize,
}

#[cfg(test)]
mod face_record_tests {
    use super::*;

    #[test]
    fn roundtrips_fields() {
        let f = FaceRecord::new(0xab, [0, 1, 30, 31]);
        assert_eq!(f.fields(), (0xab, [0, 1, 30, 31]));
        assert_eq!(size_of::<FaceRecord>(), 4);
    }

    #[test]
    fn cuboid_windows_keep_quad_ids_in_twelve_bits() {
        assert_eq!(MAX_CUBOIDS_PER_WINDOW, 682);
        assert_eq!(MAX_QUAD_ID, 4091);
        assert_eq!(cuboid_window(681), (0, 0, 681));
        assert_eq!(cuboid_window(682), (1, 682, 0));
        assert_eq!(cuboid_window(683), (1, 682, 1));
        assert_eq!(cuboid_window(681).2 * 6 + 5, MAX_QUAD_ID);
    }

    #[test]
    fn mesh_sink_tracks_fluid_batches() {
        let mut sink = MeshSink::default();
        let face = |cuboid, direction| PendingFace {
            cuboid,
            direction,
            shades: [31; 4],
        };
        let positions = [
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [1.0, 1.0, 1.0],
            [1.0, 0.0, 1.0],
        ];
        sink.push_fluid(
            MeshClass::TranslucentFluid,
            BatchCull::Up,
            positions,
            face(0, Direction::Up),
        );
        assert_eq!(sink.solid_faces.len(), 0);
        assert_eq!(sink.translucent_fluid_batches.len(), 1);
        assert_eq!(
            sink.translucent_fluid_batches[0].cull,
            BatchCull::Uncullable
        );
    }

    #[test]
    fn startup_granularity_selects_coarse_or_directional_batches() {
        fn make_sink() -> MeshSink {
            let mut sink = MeshSink::default();
            for (x, direction) in [(0, Direction::East), (1, Direction::West)] {
                sink.push_solid(RawGreedyFace {
                    block_index: block_index(x, 0, 0),
                    direction,
                    global_id: 1,
                    shades: [31; 4],
                    packed_tint: PACKED_WHITE_RGB,
                    tinted: false,
                    mergeable: false,
                    cull: BatchCull::from_direction(direction),
                    aabb_min: [x as f32, 0.0, 0.0],
                    aabb_max: [x as f32 + 1.0, 1.0, 1.0],
                });
            }
            sink
        }

        let finish = |granularity| {
            finalize_section(
                make_sink(),
                ChunkSectionPos::new(0, 0, 0),
                0,
                0,
                0,
                granularity,
            )
        };
        let coarse = finish(BatchGranularity::Coarse);
        let directional = finish(BatchGranularity::Directional);
        assert_eq!(coarse.batch_counts.regular_solid, 1);
        assert_eq!(directional.batch_counts.regular_solid, 2);
        assert!(
            coarse
                .batches
                .iter()
                .all(|batch| batch.cull == BatchCull::Uncullable)
        );
    }

    #[test]
    fn gpu_batch_packs_count_and_cull_tag() {
        let batch = FaceBatch {
            face_offset: 0,
            face_count: 123,
            table_offset: 0,
            cull: BatchCull::North,
            aabb_min: [0.0; 3],
            aabb_max: [1.0; 3],
        };
        assert_eq!(batch.packed_face_count() & BATCH_FACE_COUNT_MASK, 123);
        assert_eq!(batch.packed_face_count() >> BATCH_CULL_SHIFT, 2);
    }

    #[test]
    fn section_cuboid_roundtrips_rgb887() {
        let c = SectionCuboid::new([15, 7, 3], 0xabcd, [1.0, 128.0 / 255.0, 64.0 / 127.0]);
        assert_eq!(c.packed & 0xfff, 0x37f);
        assert_eq!((c.packed >> 12) & 0xffff, 0xabcd);
        assert_eq!((c.packed >> 28) & 0xff, 255);
        assert_eq!((c.packed >> 36) & 0xff, 128);
        assert_eq!((c.packed >> 44) & 0x7f, 64);
        assert_eq!(c.packed >> 51, 0);
        assert_eq!(size_of::<SectionCuboid>(), 8);
        assert_eq!(size_of::<GpuFaceBatch>(), 64);
    }

    #[test]
    fn fluid_heights_use_four_nibbles() {
        assert_eq!(
            pack_fluid_corner_heights([0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0]),
            0xfa50
        );
        assert_eq!(fluid_corner_height([1.0, 0.0, 0.0, 0.0]), 1.0);
        assert_eq!(fluid_corner_height([-1.0, -1.0, 0.5, -1.0]), 0.5);
    }

    #[test]
    fn section_layout_is_one_aligned_non_overlapping_allocation() {
        let mesh = SectionMeshData {
            spos: ChunkSectionPos::new(0, 0, 0),
            relative_si: 0,
            faces: vec![GreedyFaceRecord::new(0, 0, 1, 1, 0, [0; 4], 0); 3],
            tint_table: vec![0, 0x00ff8040],
            fluid_faces: vec![FaceRecord::new(0, [0; 4]); 5],
            fluid_cuboids: vec![SectionCuboid { packed: 0 }; 4],
            fluid_heights: vec![0; 4],
            batches: vec![FaceBatch {
                face_offset: 0,
                face_count: 3,
                table_offset: 0,
                cull: BatchCull::East,
                aabb_min: [0.0, 1.0, 2.0],
                aabb_max: [3.0, 4.0, 5.0],
            }],
            batch_counts: FaceBatchCounts {
                regular_solid: 1,
                ..Default::default()
            },
            aabb: ChunkAABB::zeroed(),
            content_gen: 0,
            upload_epoch: 0,
            queue_ms: 0.0,
            mesh_ms: 0.0,
        };
        let layout = mesh.layout();
        assert_eq!(layout.regular_faces, 0);
        assert_eq!(layout.tint_table % 8, 0);
        assert_eq!(layout.fluid_cuboids % 8, 0);
        assert_eq!(layout.batches % 8, 0);
        assert_eq!(layout.size % 8, 0);
        assert!(layout.tint_table >= mesh.faces.len() * 8);
        assert!(layout.fluid_faces >= layout.tint_table + mesh.tint_table.len() * 4);
        assert!(layout.fluid_cuboids >= layout.fluid_faces + mesh.fluid_faces.len() * 4);
        assert!(layout.fluid_heights >= layout.fluid_cuboids + mesh.fluid_cuboids.len() * 8);
        assert!(layout.batches >= layout.fluid_heights + mesh.fluid_heights.len() * 4);
        assert!(layout.size >= layout.batches + size_of::<GpuFaceBatch>());
    }
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ChunkAABB {
    pub min: [f32; 4],
    pub max: [f32; 4],
}

fn unorm_to_u16(x: f32) -> u16 {
    (x.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16
}

pub fn pack_uv(u: f32, v: f32) -> [u16; 2] {
    [unorm_to_u16(u), unorm_to_u16(v)]
}
pub fn pack_light_tint(light: f32, tint: u32) -> u32 {
    let l = (light.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    tint | (l << 24)
}

/// One 16³ section's packed faces, section cuboids, fluid heights, and batch
/// descriptors. All subranges are uploaded through one 8-byte-aligned pool
/// allocation and rendered as non-indexed six-vertex faces.
pub struct SectionMeshData {
    pub spos: ChunkSectionPos,
    pub relative_si: i32,
    pub faces: Vec<GreedyFaceRecord>,
    pub tint_table: Vec<u32>,
    pub fluid_faces: Vec<PackedFace>,
    pub fluid_cuboids: Vec<SectionCuboid>,
    /// Fluid-only parallel array. Each u32 stores four u4 corner heights in
    /// bits 0..15; the upper half is reserved.
    pub fluid_heights: Vec<u32>,
    pub batches: Vec<FaceBatch>,
    pub batch_counts: FaceBatchCounts,
    /// Section-local bounds of the un-quantized vertices, for culling.
    pub aabb: ChunkAABB,
    /// Content generation this mesh was built from (see
    /// `GameState::content_gen`). Lets the drain drop a stale result whose
    /// column has since been edited.
    pub content_gen: u64,
    /// Globally monotonic stamp assigned at enqueue. The buffer keeps the
    /// highest epoch uploaded per section and rejects any older upload, so an
    /// in-flight bulk mesh can never clobber a section a newer edit already
    /// uploaded (the edit always enqueues a higher epoch after its write).
    pub upload_epoch: u64,
    /// Worker-side stamps: time waiting in the mesh queue and time meshing.
    /// Aggregated by the chunk-load benchmark.
    pub queue_ms: f32,
    pub mesh_ms: f32,
}
impl SectionMeshData {
    pub fn is_empty(&self) -> bool {
        self.faces.is_empty() && self.fluid_faces.is_empty()
    }

    pub fn layout(&self) -> SectionMeshLayout {
        const fn align(value: usize, alignment: usize) -> usize {
            (value + alignment - 1) & !(alignment - 1)
        }
        let regular_faces = 0;
        let tint_table = align(self.faces.len() * 8, 8);
        let fluid_faces = tint_table + self.tint_table.len() * 4;
        let fluid_cuboids = align(fluid_faces + self.fluid_faces.len() * 4, 8);
        let fluid_heights = fluid_cuboids + self.fluid_cuboids.len() * 8;
        let batches = align(fluid_heights + self.fluid_heights.len() * 4, 8);
        let size = align(batches + self.batches.len() * size_of::<GpuFaceBatch>(), 8);
        SectionMeshLayout {
            regular_faces,
            tint_table,
            fluid_faces,
            fluid_cuboids,
            fluid_heights,
            batches,
            size,
        }
    }
}

/// Per-section meshing accumulator. Faces are separated immediately by render
/// class (terrain vs fluid), cuboid window, and coarse backface direction.
/// Finalization only serializes these already-built batches.
struct MeshSink {
    solid_faces: Vec<RawGreedyFace>,
    opaque_fluid_batches: Vec<PendingBatch>,
    translucent_fluid_batches: Vec<PendingBatch>,
    fluid_cuboids: Vec<SectionCuboid>,
    fluid_heights: Vec<u32>,
    aabb_min: [f32; 3],
    aabb_max: [f32; 3],
}

impl Default for MeshSink {
    fn default() -> Self {
        Self {
            solid_faces: Vec::new(),
            opaque_fluid_batches: Vec::new(),
            translucent_fluid_batches: Vec::new(),
            fluid_cuboids: Vec::new(),
            fluid_heights: Vec::new(),
            aabb_min: [f32::MAX; 3],
            aabb_max: [f32::MIN; 3],
        }
    }
}

#[derive(Clone, Copy)]
struct PendingFace {
    cuboid: u32,
    direction: Direction,
    shades: [u8; 4],
}

struct PendingBatch {
    cuboid_base: u32,
    cull: BatchCull,
    faces: Vec<PendingFace>,
    aabb_min: [f32; 3],
    aabb_max: [f32; 3],
}

impl PendingBatch {
    fn new(cuboid_base: u32, cull: BatchCull) -> Self {
        Self {
            cuboid_base,
            cull,
            faces: Vec::new(),
            aabb_min: [f32::MAX; 3],
            aabb_max: [f32::MIN; 3],
        }
    }

    fn push(&mut self, positions: &[[f32; 3]; 4], face: PendingFace) {
        for position in positions {
            for (axis, &value) in position.iter().enumerate() {
                self.aabb_min[axis] = self.aabb_min[axis].min(value);
                self.aabb_max[axis] = self.aabb_max[axis].max(value);
            }
        }
        self.faces.push(face);
    }
}

#[derive(Clone, Copy)]
enum MeshClass {
    OpaqueFluid,
    TranslucentFluid,
}

impl MeshSink {
    fn add_fluid_cuboid(&mut self, cuboid: SectionCuboid, heights: u32) -> u32 {
        let index = self.fluid_cuboids.len() as u32;
        self.fluid_cuboids.push(cuboid);
        self.fluid_heights.push(heights);
        index
    }

    fn push_solid(&mut self, face: RawGreedyFace) {
        for axis in 0..3 {
            self.aabb_min[axis] = self.aabb_min[axis].min(face.aabb_min[axis]);
            self.aabb_max[axis] = self.aabb_max[axis].max(face.aabb_max[axis]);
        }
        self.solid_faces.push(face);
    }

    fn push_fluid(
        &mut self,
        class: MeshClass,
        mut cull: BatchCull,
        positions: [[f32; 3]; 4],
        face: PendingFace,
    ) {
        for position in positions {
            for (axis, value) in position.into_iter().enumerate() {
                self.aabb_min[axis] = self.aabb_min[axis].min(value);
                self.aabb_max[axis] = self.aabb_max[axis].max(value);
            }
        }
        if matches!(class, MeshClass::TranslucentFluid) {
            cull = BatchCull::Uncullable;
        }
        let batches = match class {
            MeshClass::OpaqueFluid => &mut self.opaque_fluid_batches,
            MeshClass::TranslucentFluid => &mut self.translucent_fluid_batches,
        };
        let (_, cuboid_base, _) = cuboid_window(face.cuboid);
        let batch = if let Some(index) = batches
            .iter()
            .position(|batch| batch.cuboid_base == cuboid_base && batch.cull == cull)
        {
            &mut batches[index]
        } else {
            batches.push(PendingBatch::new(cuboid_base, cull));
            batches.last_mut().unwrap()
        };
        batch.push(&positions, face);
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub enum GrassColorModifier {
    #[default]
    None,
    DarkForest,
    Swamp,
}
#[derive(Clone, Copy, Debug)]
pub struct BiomeClimate {
    pub temperature: f32,
    pub downfall: f32,
    pub grass_color_override: Option<[f32; 3]>,
    pub grass_color_modifier: GrassColorModifier,
    pub foliage_color_override: Option<[f32; 3]>,
    pub dry_foliage_color_override: Option<[f32; 3]>,
    pub water_color: [f32; 3],
}
impl Default for BiomeClimate {
    fn default() -> Self {
        Self {
            temperature: 0.8,
            downfall: 0.4,
            grass_color_override: None,
            grass_color_modifier: GrassColorModifier::None,
            foliage_color_override: None,
            dry_foliage_color_override: None,
            water_color: [0.247, 0.463, 0.894],
        }
    }
}
fn tint_color(snapshot: &SectionStoreSnapshot, tint: Tint, lx: i32, ly: i32, lz: i32) -> u32 {
    match tint {
        Tint::None => PACKED_WHITE_RGB,
        Tint::Grass => pack_tint_rgb(snapshot.grass_tint(lx, ly, lz)),
        Tint::Foliage => pack_tint_rgb(snapshot.foliage_tint(lx, ly, lz)),
        Tint::DryFoliage => pack_tint_rgb(snapshot.dry_foliage_tint(lx, ly, lz)),
    }
}
#[derive(Clone)]
pub struct Colormap {
    pixels: Vec<[u8; 3]>,
}
impl Colormap {
    pub fn load(
        jar_assets_dir: &std::path::Path,
        asset_index: &Option<crate::assets::AssetIndex>,
        colormap_path: &str,
        packs: Option<&crate::resource_pack::ResourcePackManager>,
    ) -> Self {
        let path = crate::assets::resolve_asset_path_with_packs(
            jar_assets_dir,
            asset_index,
            colormap_path,
            packs,
        );
        let pixels = crate::renderer::util::load_png(&path)
            .map(|(data, _w, _h)| {
                data.chunks(4)
                    .take(256 * 256)
                    .map(|c| [c[0], c[1], c[2]])
                    .collect()
            })
            .unwrap_or_else(|| vec![[145, 189, 89]; 256 * 256]);
        Self { pixels }
    }
    fn lookup(&self, temperature: f32, downfall: f32) -> [f32; 3] {
        let t = temperature.clamp(0.0, 1.0);
        let d = (downfall.clamp(0.0, 1.0)) * t;
        let x = ((1.0 - t) * 255.0) as usize;
        let y = ((1.0 - d) * 255.0) as usize;
        let idx = (y * 256 + x).min(256 * 256 - 1);
        let [r, g, b] = self.pixels[idx];
        [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]
    }
}

pub fn grass_color(climate: &BiomeClimate, colormap: &Colormap, x: i32, z: i32) -> [f32; 3] {
    let base = climate
        .grass_color_override
        .unwrap_or_else(|| colormap.lookup(climate.temperature, climate.downfall));
    apply_grass_modifier(climate.grass_color_modifier, base, x, z)
}

pub fn foliage_color(climate: &BiomeClimate, colormap: &Colormap) -> [f32; 3] {
    climate
        .foliage_color_override
        .unwrap_or_else(|| colormap.lookup(climate.temperature, climate.downfall))
}

pub fn dry_foliage_color(climate: &BiomeClimate, colormap: &Colormap) -> [f32; 3] {
    climate
        .dry_foliage_color_override
        .unwrap_or_else(|| colormap.lookup(climate.temperature, climate.downfall))
}

/// Average a biome color over the vanilla 5x5 horizontal blend
/// (`BiomeColors` with the default blend radius of 2).
pub fn blend_color(x: i32, z: i32, mut color_at: impl FnMut(i32, i32) -> [f32; 3]) -> [f32; 3] {
    const RADIUS: i32 = 2;
    const COUNT: f32 = ((RADIUS * 2 + 1) * (RADIUS * 2 + 1)) as f32;
    let mut sum = [0.0f32; 3];
    for dz in -RADIUS..=RADIUS {
        for dx in -RADIUS..=RADIUS {
            let c = color_at(x + dx, z + dz);
            for (s, v) in sum.iter_mut().zip(c) {
                *s += v;
            }
        }
    }
    sum.map(|s| s / COUNT)
}

/// Brightness at a block position from the chunk store's light data:
/// `LIGHT_TABLE[max(sky, block)]`.
pub fn world_brightness(chunks: &ChunkStore, x: i32, y: i32, z: i32) -> f32 {
    let level = chunks
        .get_sky_light(x, y, z)
        .max(chunks.get_block_light(x, y, z));
    LIGHT_TABLE[level as usize]
}

fn apply_grass_modifier(modifier: GrassColorModifier, base: [f32; 3], x: i32, z: i32) -> [f32; 3] {
    match modifier {
        GrassColorModifier::None => base,
        GrassColorModifier::DarkForest => {
            let r = ((to_u8(base[0]) & 0xFE) as u32 + 0x28) >> 1;
            let g = ((to_u8(base[1]) & 0xFE) as u32 + 0x34) >> 1;
            let b = ((to_u8(base[2]) & 0xFE) as u32 + 0x0A) >> 1;
            [
                r.min(255) as f32 / 255.0,
                g.min(255) as f32 / 255.0,
                b.min(255) as f32 / 255.0,
            ]
        }
        GrassColorModifier::Swamp => {
            use std::sync::LazyLock;
            static BIOME_NOISE: LazyLock<SimplexNoise> =
                LazyLock::new(SimplexNoise::new_biome_info);
            let noise = BIOME_NOISE.value_2d(x as f64 * 0.0225, z as f64 * 0.0225);
            if noise < -0.1 {
                [
                    0x4C as f32 / 255.0,
                    0x76 as f32 / 255.0,
                    0x3C as f32 / 255.0,
                ]
            } else {
                [
                    0x6A as f32 / 255.0,
                    0x70 as f32 / 255.0,
                    0x39 as f32 / 255.0,
                ]
            }
        }
    }
}
fn to_u8(f: f32) -> u8 {
    (f * 255.0).round() as u8
}
struct SimplexNoise {
    perm: [u8; 256],
    #[allow(dead_code)]
    xo: f64,
    #[allow(dead_code)]
    yo: f64,
}
const GRADIENT: [[i32; 3]; 16] = [
    [1, 1, 0],
    [-1, 1, 0],
    [1, -1, 0],
    [-1, -1, 0],
    [1, 0, 1],
    [-1, 0, 1],
    [1, 0, -1],
    [-1, 0, -1],
    [0, 1, 1],
    [0, -1, 1],
    [0, 1, -1],
    [0, -1, -1],
    [1, 1, 0],
    [0, -1, 1],
    [-1, 1, 0],
    [0, -1, -1],
];
impl SimplexNoise {
    fn new_biome_info() -> Self {
        let mut rng = JavaRng::new(2345);
        let xo = rng.next_double() * 256.0;
        let yo = rng.next_double() * 256.0;
        let _zo = rng.next_double() * 256.0;
        let mut perm = [0u8; 256];
        for (i, p) in perm.iter_mut().enumerate() {
            *p = i as u8;
        }
        for i in 0..256 {
            let j = rng.next_int((256 - i) as i32) as usize + i;
            perm.swap(i, j);
        }
        Self { perm, xo, yo }
    }
    fn p(&self, i: i32) -> i32 {
        self.perm[(i & 0xFF) as usize] as i32
    }
    fn value_2d(&self, x: f64, y: f64) -> f64 {
        let sqrt3: f64 = 3.0_f64.sqrt();
        let f2 = 0.5 * (sqrt3 - 1.0);
        let g2 = (3.0 - sqrt3) / 6.0;
        let s = (x + y) * f2;
        let i = (x + s).floor() as i32;
        let j = (y + s).floor() as i32;
        let t = (i + j) as f64 * g2;
        let x0 = x - (i as f64 - t);
        let y0 = y - (j as f64 - t);
        let (i1, j1) = if x0 > y0 { (1, 0) } else { (0, 1) };
        let x1 = x0 - i1 as f64 + g2;
        let y1 = y0 - j1 as f64 + g2;
        let x2 = x0 - 1.0 + 2.0 * g2;
        let y2 = y0 - 1.0 + 2.0 * g2;
        let gi0 = (self.p(i + self.p(j)) % 12) as usize;
        let gi1 = (self.p(i + i1 + self.p(j + j1)) % 12) as usize;
        let gi2 = (self.p(i + 1 + self.p(j + 1)) % 12) as usize;
        let n0 = corner_noise(gi0, x0, y0, 0.0, 0.5);
        let n1 = corner_noise(gi1, x1, y1, 0.0, 0.5);
        let n2 = corner_noise(gi2, x2, y2, 0.0, 0.5);
        70.0 * (n0 + n1 + n2)
    }
}
fn corner_noise(gi: usize, x: f64, y: f64, z: f64, falloff: f64) -> f64 {
    let t = falloff - x * x - y * y - z * z;
    if t < 0.0 {
        0.0
    } else {
        let t2 = t * t;
        let g = &GRADIENT[gi];
        t2 * t2 * (g[0] as f64 * x + g[1] as f64 * y + g[2] as f64 * z)
    }
}
struct JavaRng {
    seed: i64,
}
impl JavaRng {
    fn new(seed: i64) -> Self {
        Self {
            seed: (seed ^ 0x5DEECE66D) & ((1i64 << 48) - 1),
        }
    }
    fn next(&mut self, bits: u32) -> i32 {
        self.seed = (self.seed.wrapping_mul(0x5DEECE66D).wrapping_add(0xB)) & ((1i64 << 48) - 1);
        (self.seed >> (48 - bits)) as i32
    }
    fn next_int(&mut self, bound: i32) -> i32 {
        if bound & (bound - 1) == 0 {
            return ((bound as i64 * self.next(31) as i64) >> 31) as i32;
        }
        loop {
            let bits = self.next(31);
            let val = bits % bound;
            if bits - val + (bound - 1) >= 0 {
                return val;
            }
        }
    }
    fn next_double(&mut self) -> f64 {
        let hi = self.next(26) as i64;
        let lo = self.next(27) as i64;
        ((hi << 27) + lo) as f64 / ((1i64 << 53) as f64)
    }
}
pub fn int_to_rgb(color: i32) -> [f32; 3] {
    let r = ((color >> 16) & 0xFF) as f32 / 255.0;
    let g = ((color >> 8) & 0xFF) as f32 / 255.0;
    let b = (color & 0xFF) as f32 / 255.0;
    [r, g, b]
}
pub(crate) struct SectionStoreSnapshot {
    pub(crate) section: Box<LocalSection>,
    pub(crate) grass_colormap: Arc<Colormap>,
    pub(crate) foliage_colormap: Arc<Colormap>,
    pub(crate) dry_foliage_colormap: Arc<Colormap>,
    pub(crate) biome_climate: Arc<HashMap<u32, BiomeClimate>>,
    pub(crate) min_y: i32,
    /// Section position, so section-local sample coords convert to world for
    /// spatial lookups (the swamp grass noise).
    pub(crate) spos: ChunkSectionPos,
    pub(crate) global_cuboids: Arc<GlobalCuboidTable>,
}
impl SectionStoreSnapshot {
    fn climate_at(&self, x: i32, y: i32, z: i32) -> BiomeClimate {
        let biome = self.section.get_biome(x, y, z);
        self.biome_climate
            .get(&u32::from(biome))
            .copied()
            .unwrap_or_default()
    }
    fn grass_color_at(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        let climate = self.climate_at(x, y, z);
        let base = climate.grass_color_override.unwrap_or_else(|| {
            self.grass_colormap
                .lookup(climate.temperature, climate.downfall)
        });
        // The swamp noise is world-space, so rebase the section-local coords.
        let wx = self.spos.x * 16 + x;
        let wz = self.spos.z * 16 + z;
        apply_grass_modifier(climate.grass_color_modifier, base, wx, wz)
    }
    fn foliage_color_at(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        let climate = self.climate_at(x, y, z);
        climate.foliage_color_override.unwrap_or_else(|| {
            self.foliage_colormap
                .lookup(climate.temperature, climate.downfall)
        })
    }
    fn dry_foliage_color_at(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        let climate = self.climate_at(x, y, z);
        climate.dry_foliage_color_override.unwrap_or_else(|| {
            self.dry_foliage_colormap
                .lookup(climate.temperature, climate.downfall)
        })
    }
    fn water_color_at(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        self.climate_at(x, y, z).water_color
    }
    fn grass_tint(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        self.blend_color(x, y, z, Self::grass_color_at)
    }
    fn foliage_tint(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        self.blend_color(x, y, z, Self::foliage_color_at)
    }
    fn dry_foliage_tint(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        self.blend_color(x, y, z, Self::dry_foliage_color_at)
    }
    fn water_tint(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        self.blend_color(x, y, z, Self::water_color_at)
    }
    fn blend_color(
        &self,
        x: i32,
        y: i32,
        z: i32,
        color_fn: fn(&Self, i32, i32, i32) -> [f32; 3],
    ) -> [f32; 3] {
        blend_color(x, z, |bx, bz| color_fn(self, bx, y, bz))
    }
    fn get_light(&self, x: i32, y: i32, z: i32) -> f32 {
        let light = self.section.get_light(x, y, z);
        LIGHT_TABLE[light as usize]
    }
}
pub const LIGHT_TABLE: [f32; 16] = [
    0.05, 0.067, 0.085, 0.106, 0.129, 0.156, 0.188, 0.227, 0.272, 0.328, 0.393, 0.472, 0.566,
    0.679, 0.815, 1.0,
];
pub(crate) fn mesh_section(
    snapshot: &SectionStoreSnapshot,
    spos: ChunkSectionPos,
    registry: &BlockRegistry,
    _uv_map: &AtlasUVMap,
    content_gen: u64,
    upload_epoch: u64,
    batch_granularity: BatchGranularity,
) -> SectionMeshData {
    let relative_si = spos.y - snapshot.min_y.div_euclid(16);
    let mut sink = MeshSink::default();
    let mut logged_missing: std::collections::HashSet<&'static str> =
        std::collections::HashSet::new();
    for local_z in 0..16 {
        for local_x in 0..16 {
            for local_y in 0..16 {
                let state = snapshot.section.get_block_state(local_x, local_y, local_z);
                let kind = classify_block(state);
                if matches!(kind, BlockKind::Air) {
                    continue;
                }
                // Section-local vertex base (matching the origin the buffer derives),
                // so positions never pass through absolute f32 world space.
                let block_pos = [local_x as f32, local_y as f32, local_z as f32];
                if let BlockKind::Water | BlockKind::Lava = kind {
                    emit_fluid(
                        &mut sink, kind, block_pos, state, snapshot, registry, local_x, local_y,
                        local_z,
                    );
                } else if let Some(baked) = registry.get_baked_model(state) {
                    emit_baked_model(
                        &mut sink, block_pos, baked, snapshot, registry, local_x, local_y, local_z,
                    );
                } else if let Some(cuboids) = registry.get_multipart_cuboids(state) {
                    emit_baked_cuboids(
                        &mut sink, block_pos, cuboids, false, snapshot, registry, local_x, local_y,
                        local_z,
                    );
                } else {
                    let id = block_id(state);
                    if logged_missing.insert(id) {
                        tracing::debug!("Missing model: {id}");
                    }
                    emit_missing_cube(
                        &mut sink, block_pos, snapshot, registry, local_x, local_y, local_z,
                    );
                }
            }
        }
    }

    finalize_section(
        sink,
        spos,
        relative_si,
        content_gen,
        upload_epoch,
        batch_granularity,
    )
}

/// Pack descriptors in the fixed ABI order: terrain, opaque fluid,
/// translucent fluid. The ordering replaces a stored per-batch tag.
fn finalize_section(
    sink: MeshSink,
    spos: ChunkSectionPos,
    relative_si: i32,
    content_gen: u64,
    upload_epoch: u64,
    batch_granularity: BatchGranularity,
) -> SectionMeshData {
    let MeshSink {
        solid_faces,
        opaque_fluid_batches,
        translucent_fluid_batches,
        fluid_cuboids,
        fluid_heights,
        aabb_min,
        aabb_max,
    } = sink;
    let count_faces =
        |batches: &[PendingBatch]| batches.iter().map(|batch| batch.faces.len()).sum::<usize>();
    let opaque_fluid_faces = count_faces(&opaque_fluid_batches);
    let translucent_fluid_faces = count_faces(&translucent_fluid_batches);
    let face_count = solid_faces.len() + opaque_fluid_faces + translucent_fluid_faces;
    let aabb = if face_count == 0 {
        ChunkAABB::zeroed()
    } else {
        ChunkAABB {
            min: [aabb_min[0], aabb_min[1], aabb_min[2], 0.0],
            max: [aabb_max[0], aabb_max[1], aabb_max[2], 0.0],
        }
    };

    let directional = batch_granularity == BatchGranularity::Directional;
    let solid = pack_solid_terrain(greedy_merge(solid_faces), directional);
    let faces = solid.faces;
    let tint_table = solid.tint_table;
    let mut batches: Vec<FaceBatch> = solid
        .batches
        .into_iter()
        .map(|batch| FaceBatch {
            face_offset: batch.face_offset,
            face_count: batch.face_count,
            table_offset: batch.tint_table_offset,
            cull: batch.cull,
            aabb_min: batch.aabb_min,
            aabb_max: batch.aabb_max,
        })
        .collect();
    let regular_solid = batches.len() as u32;

    fn append_fluid_batches(
        out: &mut Vec<FaceRecord>,
        batches: &mut Vec<FaceBatch>,
        mut pending: Vec<PendingBatch>,
        directional: bool,
    ) {
        pending.sort_by_key(|batch| (batch.cuboid_base, batch.cull as u8));
        let mut pending = pending.into_iter().peekable();
        while let Some(first) = pending.peek() {
            let cuboid_base = first.cuboid_base;
            let group_cull = first.cull;
            let face_offset = out.len() as u32;
            let mut aabb_min = [f32::MAX; 3];
            let mut aabb_max = [f32::MIN; 3];
            while pending.peek().is_some_and(|batch| {
                batch.cuboid_base == cuboid_base && (!directional || batch.cull == group_cull)
            }) {
                let batch = pending.next().unwrap();
                for face in batch.faces {
                    let (_, face_base, local) = cuboid_window(face.cuboid);
                    debug_assert_eq!(face_base, cuboid_base);
                    let quad_id = local * 6 + face.direction.index() as u32;
                    out.push(FaceRecord::new(quad_id as u16, face.shades));
                }
                for axis in 0..3 {
                    aabb_min[axis] = aabb_min[axis].min(batch.aabb_min[axis]);
                    aabb_max[axis] = aabb_max[axis].max(batch.aabb_max[axis]);
                }
            }
            let face_count = out.len() as u32 - face_offset;
            debug_assert_ne!(face_count, 0);
            batches.push(FaceBatch {
                face_offset,
                face_count,
                table_offset: cuboid_base,
                cull: if directional {
                    group_cull
                } else {
                    BatchCull::Uncullable
                },
                aabb_min,
                aabb_max,
            });
        }
    }

    let mut fluid_faces = Vec::with_capacity(opaque_fluid_faces + translucent_fluid_faces);
    let batch_start = batches.len();
    append_fluid_batches(
        &mut fluid_faces,
        &mut batches,
        opaque_fluid_batches,
        directional,
    );
    let opaque_fluid = (batches.len() - batch_start) as u32;
    let batch_start = batches.len();
    append_fluid_batches(
        &mut fluid_faces,
        &mut batches,
        translucent_fluid_batches,
        false,
    );
    let translucent_fluid = (batches.len() - batch_start) as u32;

    SectionMeshData {
        spos,
        relative_si,
        faces,
        tint_table,
        fluid_faces,
        fluid_cuboids,
        fluid_heights,
        batches,
        batch_counts: FaceBatchCounts {
            regular_solid,
            opaque_fluid,
            translucent_fluid,
        },
        aabb,
        content_gen,
        upload_epoch,
        queue_ms: 0.0,
        mesh_ms: 0.0,
    }
}
#[allow(clippy::too_many_arguments)]
fn emit_baked_model(
    sink: &mut MeshSink,
    block_pos: [f32; 3],
    model: &BakedModel,
    snapshot: &SectionStoreSnapshot,
    registry: &BlockRegistry,
    lx: i32,
    ly: i32,
    lz: i32,
) {
    emit_baked_cuboids(
        sink,
        block_pos,
        &model.cuboids,
        model.is_full_cube,
        snapshot,
        registry,
        lx,
        ly,
        lz,
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_baked_cuboids(
    sink: &mut MeshSink,
    block_pos: [f32; 3],
    cuboids: &[BakedCuboid],
    block_is_full_cube: bool,
    snapshot: &SectionStoreSnapshot,
    registry: &BlockRegistry,
    lx: i32,
    ly: i32,
    lz: i32,
) {
    for baked in cuboids {
        let visible: Vec<_> = baked
            .faces
            .iter()
            .filter(|quad| {
                if let Some(cullface) = quad.cullface {
                    let offset = cullface.offset();
                    let nx = lx + offset[0];
                    let ny = ly + offset[1];
                    let nz = lz + offset[2];
                    let neighbor = snapshot.section.get_block_state(nx, ny, nz);
                    let state = snapshot.section.get_block_state(lx, ly, lz);
                    face_visible_against_neighbor(
                        registry, state, neighbor, lx, ly, lz, nx, ny, nz,
                    )
                } else {
                    true
                }
            })
            .collect();
        if visible.is_empty() {
            continue;
        }
        let mergeable = block_is_full_cube && cuboids.len() == 1;
        let global_id = snapshot.global_cuboids.id(baked.uid);
        for quad in visible {
            let tinted = !matches!(quad.tint, Tint::None);
            let packed_tint = tint_color(snapshot, quad.tint, lx, ly, lz);
            let lights = if let Some(dir) = quad.cullface {
                compute_face_ao(snapshot, registry, lx, ly, lz, dir)
            } else {
                [quad.shade_light; 4]
            };
            let positions = quad.positions.map(|position| {
                [
                    block_pos[0] + position[0],
                    block_pos[1] + position[1],
                    block_pos[2] + position[2],
                ]
            });
            let (aabb_min, aabb_max) = positions_aabb(&positions);
            let cull = quad
                .batch_direction
                .map_or(BatchCull::Uncullable, BatchCull::from_direction);
            sink.push_solid(RawGreedyFace {
                block_index: block_index(lx as u8, ly as u8, lz as u8),
                direction: quad.direction,
                global_id,
                shades: lights.map(|light| (light.clamp(0.0, 1.0) * 31.0 + 0.5) as u8),
                packed_tint,
                tinted,
                mergeable,
                cull,
                aabb_min,
                aabb_max,
            });
        }
    }
}

fn positions_aabb(positions: &[[f32; 3]; 4]) -> ([f32; 3], [f32; 3]) {
    let mut aabb_min = [f32::MAX; 3];
    let mut aabb_max = [f32::MIN; 3];
    for position in positions {
        for (axis, &value) in position.iter().enumerate() {
            aabb_min[axis] = aabb_min[axis].min(value);
            aabb_max[axis] = aabb_max[axis].max(value);
        }
    }
    (aabb_min, aabb_max)
}
enum BlockKind {
    Air,
    Water,
    Lava,
    Solid,
}
/// Whether a face toward `neighbor` should be meshed for `state` at `(lx, ly, lz)`.
fn face_visible_against_neighbor(
    registry: &BlockRegistry,
    state: BlockState,
    neighbor: BlockState,
    lx: i32,
    ly: i32,
    lz: i32,
    nx: i32,
    ny: i32,
    nz: i32,
) -> bool {
    if registry.occludes_neighbor(neighbor) {
        return false;
    }
    // Two non-occluding solids (leaves, etc.) otherwise emit the same plane
    // twice and z-fight even with a static camera.
    if !registry.occludes_neighbor(state) && !is_air(neighbor) {
        return [lx, ly, lz] <= [nx, ny, nz];
    }
    true
}

fn classify_block(state: azalea_block::BlockState) -> BlockKind {
    if is_air(state) {
        return BlockKind::Air;
    }
    match block_id(state) {
        "cave_air" | "void_air" | "light" | "barrier" | "structure_void" | "moving_piston" => {
            BlockKind::Air
        }
        "water" | "bubble_column" => BlockKind::Water,
        "lava" => BlockKind::Lava,
        // Drawn by the block-entity pipeline; nothing to mesh.
        id if crate::world::block_entity::rendered_kind(id).is_some() => BlockKind::Air,
        _ => BlockKind::Solid,
    }
}
// TODO: flowing water texture (water_flow) with direction-based rotation

fn fluid_sample_height(
    snapshot: &SectionStoreSnapshot,
    registry: &BlockRegistry,
    kind: crate::world::block::FluidKind,
    x: i32,
    y: i32,
    z: i32,
) -> f32 {
    let state = snapshot.section.get_block_state(x, y, z);
    let fluid = crate::world::block::fluid(state);
    if fluid.kind == kind {
        let above = crate::world::block::fluid(snapshot.section.get_block_state(x, y + 1, z));
        if above.kind == kind || fluid.falling {
            1.0
        } else {
            f32::from(fluid.amount) / 9.0
        }
    } else if registry.is_opaque_full_cube(state) {
        -1.0
    } else {
        0.0
    }
}

/// Vanilla-style weighted corner height. Nearly-full samples receive ten
/// votes so a source surface stays flat next to a shallow flowing sample;
/// solid samples (-1) do not participate.
fn fluid_corner_height(samples: [f32; 4]) -> f32 {
    if samples.iter().any(|&height| height >= 1.0) {
        return 1.0;
    }
    let mut total = 0.0;
    let mut weight = 0.0;
    for height in samples {
        if height < 0.0 {
            continue;
        }
        let sample_weight = if height >= 0.8 { 10.0 } else { 1.0 };
        total += height * sample_weight;
        weight += sample_weight;
    }
    if weight == 0.0 { 0.0 } else { total / weight }
}

fn fluid_corner_heights(
    snapshot: &SectionStoreSnapshot,
    registry: &BlockRegistry,
    kind: crate::world::block::FluidKind,
    x: i32,
    y: i32,
    z: i32,
) -> [f32; 4] {
    let height = |dx, dz| fluid_sample_height(snapshot, registry, kind, x + dx, y, z + dz);
    let center = height(0, 0);
    [
        fluid_corner_height([center, height(-1, 0), height(0, -1), height(-1, -1)]),
        fluid_corner_height([center, height(-1, 0), height(0, 1), height(-1, 1)]),
        fluid_corner_height([center, height(1, 0), height(0, 1), height(1, 1)]),
        fluid_corner_height([center, height(1, 0), height(0, -1), height(1, -1)]),
    ]
}

fn pack_fluid_corner_heights(heights: [f32; 4]) -> u32 {
    heights
        .into_iter()
        .enumerate()
        .fold(0u32, |packed, (i, height)| {
            packed | (((height.clamp(0.0, 1.0) * 15.0 + 0.5) as u32) << (i * 4))
        })
}
#[allow(clippy::too_many_arguments)]
fn emit_fluid(
    sink: &mut MeshSink,
    kind: BlockKind,
    block_pos: [f32; 3],
    state: azalea_block::BlockState,
    snapshot: &SectionStoreSnapshot,
    registry: &BlockRegistry,
    lx: i32,
    ly: i32,
    lz: i32,
) {
    let tint_rgb = if matches!(kind, BlockKind::Water) {
        snapshot.water_tint(lx, ly, lz)
    } else {
        [1.0, 1.0, 1.0]
    };

    let global_id = if matches!(kind, BlockKind::Water) {
        snapshot.global_cuboids.water_id
    } else {
        snapshot.global_cuboids.lava_id
    };
    let fluid_kind = crate::world::block::fluid(state).kind;
    let heights = fluid_corner_heights(snapshot, registry, fluid_kind, lx, ly, lz);
    let packed_heights = pack_fluid_corner_heights(heights);
    let render_heights: [f32; 4] =
        std::array::from_fn(|i| ((packed_heights >> (i * 4)) & 0xf) as f32 / 15.0);
    let cuboid = sink.add_fluid_cuboid(
        SectionCuboid::new([lx as u8, ly as u8, lz as u8], global_id, tint_rgb),
        packed_heights,
    );

    let class = if matches!(kind, BlockKind::Water) {
        MeshClass::TranslucentFluid
    } else {
        MeshClass::OpaqueFluid
    };

    for dir in &CUBE_FACE_DIRS {
        let offset = dir.offset();
        let neighbor =
            snapshot
                .section
                .get_block_state(lx + offset[0], ly + offset[1], lz + offset[2]);
        let nx = lx + offset[0];
        let ny = ly + offset[1];
        let nz = lz + offset[2];
        if crate::world::block::fluid(neighbor).kind == fluid_kind
            || !face_visible_against_neighbor(
                registry, state, neighbor, lx, ly, lz, nx, ny, nz,
            )
        {
            continue;
        }
        let (mut positions, _, light) = cube_face_geometry(*dir);
        for p in &mut positions {
            if p[1] > 0.5 {
                let high_x = p[0] >= 0.5;
                let high_z = p[2] >= 0.5;
                let corner = if high_x {
                    if high_z { 2 } else { 3 }
                } else if high_z {
                    1
                } else {
                    0
                };
                p[1] = render_heights[corner];
            }
        }
        emit_quad_into(
            sink,
            class,
            cuboid,
            *dir,
            BatchCull::from_direction(*dir),
            block_pos,
            &positions,
            [light; 4],
        );
    }
}
#[allow(clippy::too_many_arguments)]
fn emit_missing_cube(
    sink: &mut MeshSink,
    block_pos: [f32; 3],
    snapshot: &SectionStoreSnapshot,
    registry: &BlockRegistry,
    lx: i32,
    ly: i32,
    lz: i32,
) {
    let global_id = snapshot.global_cuboids.missing_id;
    for dir in &CUBE_FACE_DIRS {
        let offset = dir.offset();
        let neighbor =
            snapshot
                .section
                .get_block_state(lx + offset[0], ly + offset[1], lz + offset[2]);
        let nx = lx + offset[0];
        let ny = ly + offset[1];
        let nz = lz + offset[2];
        let state = snapshot.section.get_block_state(lx, ly, lz);
        if !face_visible_against_neighbor(
            registry, state, neighbor, lx, ly, lz, nx, ny, nz,
        ) {
            continue;
        }
        let (positions, _, light) = cube_face_geometry(*dir);
        let positions = positions.map(|position| {
            [
                block_pos[0] + position[0],
                block_pos[1] + position[1],
                block_pos[2] + position[2],
            ]
        });
        let (aabb_min, aabb_max) = positions_aabb(&positions);
        let shades = [((light * 31.0) + 0.5) as u8; 4];
        sink.push_solid(RawGreedyFace {
            block_index: block_index(lx as u8, ly as u8, lz as u8),
            direction: *dir,
            global_id,
            shades,
            packed_tint: PACKED_WHITE_RGB,
            tinted: false,
            mergeable: true,
            cull: BatchCull::from_direction(*dir),
            aabb_min,
            aabb_max,
        });
    }
}
pub(crate) const CUBE_FACE_DIRS: [Direction; 6] = [
    Direction::Up,
    Direction::Down,
    Direction::North,
    Direction::South,
    Direction::East,
    Direction::West,
];
/// Fluids route through the legacy cuboid path via [`emit_quad_into`].
#[allow(clippy::too_many_arguments)]
fn emit_quad_into(
    sink: &mut MeshSink,
    class: MeshClass,
    cuboid: u32,
    direction: Direction,
    cull: BatchCull,
    block_pos: [f32; 3],
    positions: &[[f32; 3]; 4],
    lights: [f32; 4],
) {
    let positions = positions.map(|position| {
        [
            block_pos[0] + position[0],
            block_pos[1] + position[1],
            block_pos[2] + position[2],
        ]
    });
    let shades = lights.map(|light| (light.clamp(0.0, 1.0) * 31.0 + 0.5) as u8);
    sink.push_fluid(
        class,
        cull,
        positions,
        PendingFace {
            cuboid,
            direction,
            shades,
        },
    );
}
fn shade_brightness(state: azalea_block::BlockState, registry: &BlockRegistry) -> f32 {
    // TODO: non-occluding full cubes (leaves, glass, ice) still darken adjacent
    // faces here. Vanilla's are `isViewBlocking=never` and don't contribute AO.
    if registry.is_opaque_full_cube(state) {
        0.2
    } else {
        1.0
    }
}
/// Centre-relative offset of vanilla's `AdjacencyInfo.corners[0]` neighbour
/// (`centre + dir + corners[0]`), the `shade0` occlusion fallback.
fn corners0_offset(dir: Direction) -> [i32; 3] {
    match dir {
        // corners[0] = EAST(+x)
        Direction::Up => [1, 1, 0],
        // corners[0] = WEST(-x)
        Direction::Down => [-1, -1, 0],
        // corners[0] = UP(+y)
        Direction::North => [0, 1, -1],
        // corners[0] = WEST(-x)
        Direction::South => [-1, 0, 1],
        // corners[0] = UP(+y)
        Direction::West => [-1, 1, 0],
        // corners[0] = DOWN(-y)
        Direction::East => [1, -1, 0],
    }
}
fn compute_face_ao(
    snapshot: &SectionStoreSnapshot,
    registry: &BlockRegistry,
    lx: i32,
    ly: i32,
    lz: i32,
    dir: Direction,
) -> [f32; 4] {
    let s = |[dx, dy, dz]: [i32; 3]| -> f32 {
        shade_brightness(
            snapshot.section.get_block_state(lx + dx, ly + dy, lz + dz),
            registry,
        )
    };
    let l = |[dx, dy, dz]: [i32; 3]| -> f32 { snapshot.get_light(lx + dx, ly + dy, lz + dz) };

    let shade0 = s(corners0_offset(dir));

    // Each vertex's (side1, side2, corner) neighbour offsets, in
    // `face_positions`' vertex order.
    let rows: [[[i32; 3]; 3]; 4] = match dir {
        Direction::Up => [
            [[0, 1, -1], [-1, 1, 0], [-1, 1, -1]],
            [[0, 1, 1], [-1, 1, 0], [-1, 1, 1]],
            [[0, 1, 1], [1, 1, 0], [1, 1, 1]],
            [[0, 1, -1], [1, 1, 0], [1, 1, -1]],
        ],
        Direction::Down => [
            [[0, -1, 1], [-1, -1, 0], [-1, -1, 1]],
            [[0, -1, -1], [-1, -1, 0], [-1, -1, -1]],
            [[0, -1, -1], [1, -1, 0], [1, -1, -1]],
            [[0, -1, 1], [1, -1, 0], [1, -1, 1]],
        ],
        Direction::North => [
            [[1, 0, -1], [0, 1, -1], [1, 1, -1]],
            [[1, 0, -1], [0, -1, -1], [1, -1, -1]],
            [[-1, 0, -1], [0, -1, -1], [-1, -1, -1]],
            [[-1, 0, -1], [0, 1, -1], [-1, 1, -1]],
        ],
        Direction::South => [
            [[-1, 0, 1], [0, 1, 1], [-1, 1, 1]],
            [[-1, 0, 1], [0, -1, 1], [-1, -1, 1]],
            [[1, 0, 1], [0, -1, 1], [1, -1, 1]],
            [[1, 0, 1], [0, 1, 1], [1, 1, 1]],
        ],
        Direction::West => [
            [[-1, 0, -1], [-1, 1, 0], [-1, 1, -1]],
            [[-1, 0, -1], [-1, -1, 0], [-1, -1, -1]],
            [[-1, 0, 1], [-1, -1, 0], [-1, -1, 1]],
            [[-1, 0, 1], [-1, 1, 0], [-1, 1, 1]],
        ],
        Direction::East => [
            [[1, 0, 1], [1, 1, 0], [1, 1, 1]],
            [[1, 0, 1], [1, -1, 0], [1, -1, 1]],
            [[1, 0, -1], [1, -1, 0], [1, -1, -1]],
            [[1, 0, -1], [1, 1, 0], [1, 1, -1]],
        ],
    };

    let n = dir.offset();
    let dir_shade = dir.shade_light();
    rows.map(|[side1, side2, corner]| {
        let ao = super::block_ao::vertex_brightness(s(side1), s(side2), s(corner), shade0);
        let light = avg4(l(n), l(side1), l(side2), l(corner));
        ao * light * dir_shade
    })
}
fn avg4(a: f32, b: f32, c: f32, d: f32) -> f32 {
    (a + b + c + d) * 0.25
}
pub(crate) fn cube_face_geometry(dir: Direction) -> ([[f32; 3]; 4], [[f32; 2]; 4], f32) {
    let (from, to) = ([0.0; 3], [1.0; 3]);
    (
        face_positions(dir, from, to),
        face_uvs(dir, from, to, None, None),
        dir.shade_light(),
    )
}

