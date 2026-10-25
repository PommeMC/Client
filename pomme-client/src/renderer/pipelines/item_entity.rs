use std::collections::HashMap;
use std::sync::Arc;

use glam::Mat4;

use crate::renderer::camera::CameraUniform;
use crate::renderer::chunk::atlas::{AtlasRegion, AtlasUVMap, SpriteAlphaMask, TextureAtlas};
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, rhi, shader, util};
use crate::world::block::model::{BakedModel, Direction, direction_from_positions};

/// Item-only vertex format. Vanilla's ENTITY item format keeps UV0 as floats
/// and carries a baked face normal; both matter here because generated sprite
/// texel boundaries must line up with extrusion geometry and dropped items use
/// the normal for two-direction diffuse lighting.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct ItemVertex {
    position: [f32; 3],
    tex_coords: [f32; 2],
    light_tint: u32,
    normal: [i8; 4],
    tint_index: u32,
}

impl ItemVertex {
    const BUFFER: rhi::VertexBufferLayout<'static> =
        rhi::vertex_buffer!(Self, [Float32x3, Float32x2, Unorm8x4, Snorm8x4, Uint32]);
}

fn pack_normal(normal: glam::Vec3) -> [i8; 4] {
    let normal = normal.normalize_or_zero();
    [
        (normal.x.clamp(-1.0, 1.0) * 127.0).round() as i8,
        (normal.y.clamp(-1.0, 1.0) * 127.0).round() as i8,
        (normal.z.clamp(-1.0, 1.0) * 127.0).round() as i8,
        0,
    ]
}

pub struct ItemRenderInfo {
    pub item_name: String,
    /// Evaluated tint palette, shared by every copy of one dropped stack.
    pub tints: Arc<[u32]>,
    pub model_matrix: Mat4,
    pub light: f32,
    pub nether_lighting: bool,
}

/// What the item-entity renderer needs to place a mesh: whether it baked from a
/// 3D (block) model, plus its local-space bounding box used for the hover
/// height and the 3D-vs-flat copy layout (vanilla reads these off
/// `getModelBoundingBox`).
#[derive(Clone, Copy)]
pub struct ItemMeshInfo {
    pub is_block_model: bool,
    pub bounds_min: glam::Vec3,
    pub bounds_max: glam::Vec3,
}

struct MeshEntry {
    buffer: rhi::backend::Buffer,
    vertex_count: u32,
    is_3d_model: bool,
    /// Vanilla `BakedQuad.MaterialInfo`: a sprite with partial alpha draws on
    /// the translucent item sheet, everything else on the cutout one.
    translucent: bool,
    bounds_min: glam::Vec3,
    bounds_max: glam::Vec3,
}

/// The per-frame tint palette a [`TintPaletteArena`] binds.
const ITEM_TINTS: &[rhi::BindGroupLayoutEntry] = &[rhi::BindGroupLayoutEntry::storage(
    rhi::ShaderStages::VERTEX,
)];

/// The item pipelines' bind groups: camera, atlas, tint palette.
const ITEM_BIND_GROUPS: [&[rhi::BindGroupLayoutEntry]; 3] =
    [util::VERTEX_UNIFORM, util::FRAGMENT_TEXTURE, ITEM_TINTS];
/// item_push.glsl's block. Vulkan guarantees at least 128 push bytes.
const ITEM_PUSH_CONSTANTS: rhi::PushConstants = rhi::PushConstants {
    visibility: rhi::ShaderStages::VERTEX_FRAGMENT,
    size: 128,
};

#[derive(Clone, Copy, Default)]
pub(super) struct ItemTintRange {
    pub base: u32,
    pub count: u32,
}

struct TintPaletteArena {
    layout: rhi::backend::BindGroupLayout,
    sets: Vec<rhi::backend::BindGroup>,
    buffers: Vec<Option<rhi::backend::Buffer>>,
    capacities: Vec<usize>,
    cursors: Vec<usize>,
    label: String,
}

impl TintPaletteArena {
    fn new(
        device: &rhi::backend::Device,
        layout: rhi::backend::BindGroupLayout,
        label: &str,
    ) -> Self {
        let mut arena = Self {
            layout,
            sets: vec![rhi::backend::BindGroup::default(); MAX_FRAMES_IN_FLIGHT],
            buffers: (0..MAX_FRAMES_IN_FLIGHT).map(|_| None).collect(),
            capacities: vec![0; MAX_FRAMES_IN_FLIGHT],
            cursors: vec![0; MAX_FRAMES_IN_FLIGHT],
            label: label.to_string(),
        };
        for frame in 0..MAX_FRAMES_IN_FLIGHT {
            arena.ensure_capacity(device, frame, 1);
        }
        arena
    }

    fn ensure_capacity(&mut self, device: &rhi::backend::Device, frame: usize, required: usize) {
        if self.capacities[frame] >= required.max(1) {
            return;
        }
        let capacity = required.max(1).next_power_of_two();
        if let Some(buffer) = self.buffers[frame].take() {
            buffer.destroy(device);
        }
        let buffer = rhi::backend::Buffer::new(
            device,
            &format!("{}_tints_{frame}", self.label),
            (capacity * size_of::<u32>()) as u64,
            rhi::BufferUsages::STORAGE,
            rhi::MemoryLocation::Host,
        );
        self.sets[frame].replace(
            device,
            self.layout,
            &[(
                0,
                rhi::backend::BindingResource::storage(
                    &buffer,
                    (capacity * size_of::<u32>()) as u64,
                ),
            )],
        );
        self.buffers[frame] = Some(buffer);
        self.capacities[frame] = capacity;
    }

    fn begin_frame(&mut self, device: &rhi::backend::Device, frame: usize, required_colors: usize) {
        self.ensure_capacity(device, frame, required_colors.max(1));
        self.cursors[frame] = 0;
    }

    fn push(&mut self, frame: usize, colors: &[u32]) -> ItemTintRange {
        if colors.is_empty() {
            return ItemTintRange::default();
        }
        let base = self.cursors[frame];
        let end = base + colors.len();
        debug_assert!(end <= self.capacities[frame]);
        let buffer = self.buffers[frame].as_mut().expect("tint buffer allocated");
        buffer.write_at(base * size_of::<u32>(), bytemuck::cast_slice(colors));
        self.cursors[frame] = end;
        ItemTintRange {
            base: base as u32,
            count: colors.len() as u32,
        }
    }

    fn set(&self, frame: usize) -> rhi::backend::BindGroup {
        self.sets[frame]
    }

    fn destroy(&mut self, device: &rhi::backend::Device) {
        for buffer in self.buffers.iter_mut().filter_map(Option::take) {
            buffer.destroy(device);
        }
        util::destroy_bind_groups(device, self.sets.drain(..));
    }
}

/// Bind group layouts, per-frame camera UBOs, the atlas group and tint palette
/// storage shared by pipelines that draw item meshes with the item shaders.
pub(super) struct ItemPipelineShared {
    pub pipeline_layout: rhi::backend::PipelineLayout,
    camera_layout: rhi::backend::BindGroupLayout,
    atlas_layout: rhi::backend::BindGroupLayout,
    tint_layout: rhi::backend::BindGroupLayout,
    camera_sets: Vec<rhi::backend::BindGroup>,
    atlas_set: rhi::backend::BindGroup,
    atlas_sampler: rhi::backend::Sampler,
    camera_buffers: Vec<rhi::backend::Buffer>,
    tint_arena: TintPaletteArena,
}

impl ItemPipelineShared {
    pub(super) fn new(device: &rhi::backend::Device, atlas: &TextureAtlas, label: &str) -> Self {
        let [camera_layout, atlas_layout, tint_layout] =
            ITEM_BIND_GROUPS.map(|entries| rhi::backend::create_bind_group_layout(device, entries));

        let pipeline_layout = rhi::backend::create_pipeline_layout(
            device,
            &[camera_layout, atlas_layout, tint_layout],
            Some(ITEM_PUSH_CONSTANTS),
        );

        let (camera_sets, camera_buffers) = util::create_uniform_sets(
            device,
            camera_layout,
            size_of::<CameraUniform>() as u64,
            &format!("{label}_camera"),
        );
        // Vanilla's items atlas has no mip chain, so generated items sample
        // level 0. TODO: block-model items draw from the mipmapped blocks atlas
        // in vanilla; pomme samples those at level 0 too.
        let atlas_sampler = rhi::backend::create_sampler(device, &rhi::SamplerDesc::NEAREST);

        let tint_arena = TintPaletteArena::new(device, tint_layout, label);
        let mut this = Self {
            pipeline_layout,
            camera_layout,
            atlas_layout,
            tint_layout,
            camera_sets,
            atlas_set: rhi::backend::BindGroup::default(),
            atlas_sampler,
            camera_buffers,
            tint_arena,
        };
        this.rebind_atlas(device, atlas);
        this
    }

    pub(super) fn rebind_atlas(&mut self, device: &rhi::backend::Device, atlas: &TextureAtlas) {
        self.atlas_set.replace(
            device,
            self.atlas_layout,
            &util::texture_binding(atlas.texture.handle(), self.atlas_sampler),
        );
    }

    pub(super) fn update_camera(&mut self, frame: usize, uniform: &CameraUniform) {
        let bytes = bytemuck::bytes_of(uniform);
        self.camera_buffers[frame].write(bytes);
    }

    pub(super) fn bind(
        &self,
        pass: &mut rhi::backend::RenderPassEncoder,
        frame: usize,
        pipeline: rhi::backend::RenderPipeline,
    ) {
        pass.set_pipeline(&pipeline);
        pass.set_bind_groups(
            0,
            &[
                self.camera_sets[frame],
                self.atlas_set,
                self.tint_arena.set(frame),
            ],
        );
    }

    pub(super) fn begin_tint_frame(
        &mut self,
        device: &rhi::backend::Device,
        frame: usize,
        required_colors: usize,
    ) {
        self.tint_arena.begin_frame(device, frame, required_colors);
    }

    pub(super) fn push_tints(&mut self, frame: usize, colors: &[u32]) -> ItemTintRange {
        self.tint_arena.push(frame, colors)
    }

    pub(super) fn destroy(&mut self, device: &rhi::backend::Device) {
        util::destroy_buffers(device, &mut self.camera_buffers);

        self.tint_arena.destroy(device);
        self.atlas_sampler.destroy(device);
        self.pipeline_layout.destroy(device);
        util::destroy_bind_groups(device, self.camera_sets.drain(..));
        self.atlas_set.destroy(device);
        self.camera_layout.destroy(device);
        self.atlas_layout.destroy(device);
        self.tint_layout.destroy(device);
    }
}

pub(super) fn push_model_light(
    pass: &rhi::backend::RenderPassEncoder,
    model: &Mat4,
    light: f32,
    item_tints: ItemTintRange,
) {
    let mvp_data = model.to_cols_array();
    pass.set_push_constants(
        rhi::ShaderStages::VERTEX_FRAGMENT,
        0,
        bytemuck::bytes_of(&mvp_data),
    );
    pass.set_push_constants(
        rhi::ShaderStages::VERTEX_FRAGMENT,
        64,
        bytemuck::bytes_of(&light),
    );
    let tint_range = [item_tints.base, item_tints.count];
    pass.set_push_constants(
        rhi::ShaderStages::VERTEX_FRAGMENT,
        72,
        bytemuck::bytes_of(&tint_range),
    );
}

fn push_world_lighting(pass: &rhi::backend::RenderPassEncoder, model: &Mat4, nether: bool) {
    let nether = if nether { 1.0_f32 } else { 0.0_f32 };
    pass.set_push_constants(
        rhi::ShaderStages::VERTEX_FRAGMENT,
        68,
        bytemuck::bytes_of(&nether),
    );

    let normal = glam::Mat3::from_mat4(*model).inverse().transpose();
    let cols = normal.to_cols_array();
    let padded_cols = [
        cols[0], cols[1], cols[2], 0.0, cols[3], cols[4], cols[5], 0.0, cols[6], cols[7], cols[8],
        0.0,
    ];
    pass.set_push_constants(
        rhi::ShaderStages::VERTEX_FRAGMENT,
        80,
        bytemuck::bytes_of(&padded_cols),
    );
}

pub struct ItemEntityPipeline {
    /// Vanilla `ITEM_CUTOUT`: alpha-tested, no blending.
    cutout: rhi::backend::RenderPipeline,
    /// Vanilla `ITEM_TRANSLUCENT`: alpha-tested and blended.
    translucent: rhi::backend::RenderPipeline,
    shared: ItemPipelineShared,
    meshes: HashMap<String, MeshEntry>,
}

impl ItemEntityPipeline {
    pub fn new(
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
        atlas: &TextureAtlas,
    ) -> Self {
        let shared = ItemPipelineShared::new(device, atlas, "item_entity");
        let (cutout, translucent) = create_world_pipelines(device, targets, shared.pipeline_layout);

        Self {
            cutout,
            translucent,
            shared,
            meshes: HashMap::new(),
        }
    }

    pub fn update_camera(&mut self, frame: usize, uniform: &CameraUniform) {
        self.shared.update_camera(frame, uniform);
    }

    fn insert_mesh(
        &mut self,
        device: &rhi::backend::Device,
        name: &str,
        vertices: &[ItemVertex],
        is_3d_model: bool,
        translucent: bool,
    ) {
        let bytes = bytemuck::cast_slice(vertices);
        let buffer = rhi::backend::Buffer::with_data(
            device,
            &format!("item_{name}"),
            bytes,
            rhi::BufferUsages::VERTEX,
        );
        let (bounds_min, bounds_max) = mesh_bounds(vertices);
        self.meshes.insert(
            name.to_string(),
            MeshEntry {
                buffer,
                vertex_count: vertices.len() as u32,
                is_3d_model,
                translucent,
                bounds_min,
                bounds_max,
            },
        );
    }

    /// `None` if no mesh is built yet, else its 3D-model flag and local bounds.
    pub fn mesh_info(&self, name: &str) -> Option<ItemMeshInfo> {
        self.meshes.get(name).map(|m| ItemMeshInfo {
            is_block_model: m.is_3d_model,
            bounds_min: m.bounds_min,
            bounds_max: m.bounds_max,
        })
    }

    pub fn mesh_handle(&self, name: &str) -> Option<(rhi::backend::BufferHandle, u32)> {
        self.meshes
            .get(name)
            .map(|m| (m.buffer.handle(), m.vertex_count))
    }

    pub fn ensure_mesh(
        &mut self,
        device: &rhi::backend::Device,
        name: &str,
        model: &BakedModel,
        uv_map: &AtlasUVMap,
    ) {
        if self.meshes.contains_key(name) {
            return;
        }
        let vertices = build_item_mesh(model, uv_map);
        if !vertices.is_empty() {
            let translucent = model
                .quads
                .iter()
                .any(|quad| uv_map.region(quad.texture).translucent);
            self.insert_mesh(device, name, &vertices, true, translucent);
        }
    }

    pub fn ensure_flat_mesh(
        &mut self,
        device: &rhi::backend::Device,
        name: &str,
        layers: &[Option<String>],
        uv_map: &AtlasUVMap,
    ) {
        if self.meshes.contains_key(name) {
            return;
        }

        let mut vertices = Vec::new();
        let mut translucent = false;
        for (layer, texture_key) in layers.iter().enumerate() {
            let Some(texture_key) = texture_key.as_deref() else {
                continue;
            };
            if !uv_map.has_region(texture_key) {
                continue;
            }
            let region = uv_map.get_region(texture_key);
            translucent |= region.translucent;
            // layerN is tintindex N; the shader leaves ones past the palette untinted.
            let tint_index = Some(layer as u32);
            let mut layer_vertices = uv_map
                .sprite_alpha_mask(texture_key)
                .map(|mask| build_extruded_item_mask(mask, region, tint_index))
                .unwrap_or_else(|| build_flat_quad(region, tint_index));
            vertices.append(&mut layer_vertices);
        }
        if !vertices.is_empty() {
            self.insert_mesh(device, name, &vertices, false, translucent);
        }
    }

    /// Cutout meshes first, then translucent ones, as vanilla's item sheets
    /// are ordered.
    pub fn draw(
        &mut self,
        device: &rhi::backend::Device,
        pass: &mut rhi::backend::RenderPassEncoder,
        frame: usize,
        items: &[ItemRenderInfo],
    ) {
        if items.is_empty() {
            return;
        }

        let required_colors = items.iter().map(|item| item.tints.len()).sum();
        self.shared.begin_tint_frame(device, frame, required_colors);
        let tint_ranges: Vec<_> = items
            .iter()
            .map(|item| self.shared.push_tints(frame, &item.tints))
            .collect();

        for (pipeline, translucent) in [(self.cutout, false), (self.translucent, true)] {
            let mut bound = false;
            for (item, tint_range) in items.iter().zip(tint_ranges.iter().copied()) {
                let Some(mesh) = self.meshes.get(&item.item_name) else {
                    continue;
                };
                if mesh.translucent != translucent {
                    continue;
                }
                if !bound {
                    self.shared.bind(pass, frame, pipeline);
                    bound = true;
                }
                pass.set_vertex_buffer(0, mesh.buffer.handle());
                push_model_light(pass, &item.model_matrix, item.light, tint_range);
                push_world_lighting(pass, &item.model_matrix, item.nether_lighting);
                pass.draw(mesh.vertex_count, 1, 0, 0);
            }
        }
    }

    pub fn recreate_pipeline(
        &mut self,
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
    ) {
        self.destroy_pipelines(device);
        (self.cutout, self.translucent) =
            create_world_pipelines(device, targets, self.shared.pipeline_layout);
    }

    fn destroy_pipelines(&self, device: &rhi::backend::Device) {
        self.cutout.destroy(device);
        self.translucent.destroy(device);
    }

    pub fn rebind_atlas(&mut self, device: &rhi::backend::Device, atlas: &TextureAtlas) {
        self.shared.rebind_atlas(device, atlas);
    }

    pub fn clear_meshes(&mut self, device: &rhi::backend::Device) {
        for (_, entry) in self.meshes.drain() {
            entry.buffer.destroy(device);
        }
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        self.clear_meshes(device);
        self.destroy_pipelines(device);
        self.shared.destroy(device);
    }
}

/// Local-space bounds of a baked item mesh before its display transform.
/// Empty meshes report a degenerate box at the origin.
fn mesh_bounds(vertices: &[ItemVertex]) -> (glam::Vec3, glam::Vec3) {
    if vertices.is_empty() {
        return (glam::Vec3::ZERO, glam::Vec3::ZERO);
    }

    let mut min = glam::Vec3::splat(f32::INFINITY);
    let mut max = glam::Vec3::splat(f32::NEG_INFINITY);
    for vertex in vertices {
        let position = glam::Vec3::from_array(vertex.position);
        min = min.min(position);
        max = max.max(position);
    }
    (min, max)
}

/// Vanilla stores a cardinal `BakedQuad.direction()` even when an element
/// rotation makes the geometric normal oblique, `UP` for a degenerate quad.
fn cardinal_normal(positions: &[[f32; 3]; 4]) -> glam::Vec3 {
    let direction = direction_from_positions(positions).unwrap_or(Direction::Up);
    glam::Vec3::from_array(direction.offset().map(|v| v as f32))
}

const NO_ITEM_TINT: u32 = u32::MAX;

fn tint_index_or_none(index: Option<u32>) -> u32 {
    index.unwrap_or(NO_ITEM_TINT)
}

fn build_item_mesh(model: &BakedModel, uv_map: &AtlasUVMap) -> Vec<ItemVertex> {
    let mut vertices = Vec::new();
    for quad in &model.quads {
        let region = uv_map.region(quad.texture);
        let u_span = region.u_max - region.u_min;
        let v_span = region.v_max - region.v_min;
        let tint_index = tint_index_or_none(quad.tint_index);
        let normal = pack_normal(cardinal_normal(&quad.positions));

        for i in [0, 1, 2, 2, 3, 0] {
            let p = quad.positions[i];
            vertices.push(ItemVertex {
                position: [p[0] - 0.5, p[1] - 0.5, p[2] - 0.5],
                tex_coords: [
                    region.u_min + quad.uvs[i][0] * u_span,
                    region.v_min + quad.uvs[i][1] * v_span,
                ],
                // Held/GUI item rendering still consumes the baked shade byte.
                // The dropped-item world shader ignores it and instead uses
                // the baked cardinal normal, matching vanilla's item shader.
                light_tint: crate::renderer::chunk::mesher::pack_light_tint(
                    quad.shade_light,
                    crate::renderer::chunk::mesher::PACKED_WHITE_SHIFTED,
                ),
                normal,
                tint_index,
            });
        }
    }
    vertices
}

#[cfg(test)]
fn build_extruded_item(img: &image::RgbaImage, region: AtlasRegion) -> Vec<ItemVertex> {
    let w = img.width();
    let h = img.height();
    let mask = SpriteAlphaMask {
        width: w,
        height: h,
        frames: vec![img.pixels().map(|pixel| pixel[3] != 0).collect()],
    };
    build_extruded_item_mask(&mask, region, None)
}

fn build_extruded_item_mask(
    mask: &SpriteAlphaMask,
    region: AtlasRegion,
    tint_index: Option<u32>,
) -> Vec<ItemVertex> {
    let w = mask.width as i32;
    let h = mask.height as i32;
    let mut vertices = Vec::new();

    let px = 1.0 / w as f32;
    let py = 1.0 / h as f32;
    let u_span = region.u_max - region.u_min;
    let v_span = region.v_max - region.v_min;
    let z_min = 7.5 / 16.0 - 0.5;
    let z_max = 8.5 / 16.0 - 0.5;
    let tint_index = tint_index_or_none(tint_index);

    let front = [
        [-0.5, -0.5, z_max],
        [0.5, -0.5, z_max],
        [0.5, 0.5, z_max],
        [-0.5, -0.5, z_max],
        [0.5, 0.5, z_max],
        [-0.5, 0.5, z_max],
    ];
    let front_uvs = [
        [region.u_min, region.v_max],
        [region.u_max, region.v_max],
        [region.u_max, region.v_min],
        [region.u_min, region.v_max],
        [region.u_max, region.v_min],
        [region.u_min, region.v_min],
    ];
    for i in 0..6 {
        vertices.push(ItemVertex {
            position: front[i],
            tex_coords: front_uvs[i],
            light_tint: crate::renderer::chunk::mesher::pack_light_tint(
                1.0,
                crate::renderer::chunk::mesher::PACKED_WHITE_SHIFTED,
            ),
            normal: pack_normal(glam::Vec3::Z),
            tint_index,
        });
    }

    let back = [
        [0.5, -0.5, z_min],
        [-0.5, -0.5, z_min],
        [-0.5, 0.5, z_min],
        [0.5, -0.5, z_min],
        [-0.5, 0.5, z_min],
        [0.5, 0.5, z_min],
    ];
    // Vanilla's generated-item NORTH face uses UVs [16, 0, 0, 16].
    // With the north-face winding below that preserves the sprite's model-space
    // orientation: x=+0.5 samples the right side of the sprite, just like the
    // SOUTH face. Mirroring these UVs makes an asymmetric back silhouette no
    // longer line up with the generated side faces.
    let back_uvs = [
        [region.u_max, region.v_max],
        [region.u_min, region.v_max],
        [region.u_min, region.v_min],
        [region.u_max, region.v_max],
        [region.u_min, region.v_min],
        [region.u_max, region.v_min],
    ];
    for i in 0..6 {
        vertices.push(ItemVertex {
            position: back[i],
            tex_coords: back_uvs[i],
            light_tint: crate::renderer::chunk::mesher::pack_light_tint(
                1.0,
                crate::renderer::chunk::mesher::PACKED_WHITE_SHIFTED,
            ),
            normal: pack_normal(glam::Vec3::NEG_Z),
            tint_index,
        });
    }

    for y in 0..h {
        for x in 0..w {
            let top_exposed = mask.has_exposed_edge(x, y, x, y - 1);
            let bottom_exposed = mask.has_exposed_edge(x, y, x, y + 1);
            let left_exposed = mask.has_exposed_edge(x, y, x - 1, y);
            let right_exposed = mask.has_exposed_edge(x, y, x + 1, y);
            if !(top_exposed || bottom_exposed || left_exposed || right_exposed) {
                continue;
            }
            let fx = x as f32 * px - 0.5;
            let fy = 0.5 - (y + 1) as f32 * py;
            let fx1 = fx + px;
            let fy1 = fy + py;
            // Vanilla ItemModelGenerator maps each side face from 0.1 to 0.9
            // inside its source pixel, measured across the exact sprite bounds.
            let u0 = region.u_min + (x as f32 + 0.1) * px * u_span;
            let u1 = region.u_min + (x as f32 + 0.9) * px * u_span;
            let v0 = region.v_min + (y as f32 + 0.1) * py * v_span;
            let v1 = region.v_min + (y as f32 + 0.9) * py * v_span;

            if top_exposed {
                push_side_quad(
                    &mut vertices,
                    fx,
                    fy1,
                    fx1,
                    fy1,
                    z_min,
                    z_max,
                    [[u0, v0], [u0, v1], [u1, v1], [u1, v0]],
                    0.8,
                    tint_index,
                );
            }
            if bottom_exposed {
                push_side_quad(
                    &mut vertices,
                    fx1,
                    fy,
                    fx,
                    fy,
                    z_min,
                    z_max,
                    [[u1, v1], [u1, v0], [u0, v0], [u0, v1]],
                    0.8,
                    tint_index,
                );
            }
            if left_exposed {
                push_side_quad(
                    &mut vertices,
                    fx,
                    fy,
                    fx,
                    fy1,
                    z_min,
                    z_max,
                    [[u1, v1], [u0, v1], [u0, v0], [u1, v0]],
                    0.8,
                    tint_index,
                );
            }
            if right_exposed {
                push_side_quad(
                    &mut vertices,
                    fx1,
                    fy1,
                    fx1,
                    fy,
                    z_min,
                    z_max,
                    [[u0, v0], [u1, v0], [u1, v1], [u0, v1]],
                    0.8,
                    tint_index,
                );
            }
        }
    }

    vertices
}

#[allow(clippy::too_many_arguments)]
fn push_side_quad(
    vertices: &mut Vec<ItemVertex>,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    z0: f32,
    z1: f32,
    uvs: [[f32; 2]; 4],
    light: f32,
    tint_index: u32,
) {
    let positions = [[x0, y0, z0], [x0, y0, z1], [x1, y1, z1], [x1, y1, z0]];
    let normal = pack_normal(cardinal_normal(&positions));
    for i in [0, 1, 2, 0, 2, 3] {
        vertices.push(ItemVertex {
            position: positions[i],
            tex_coords: uvs[i],
            light_tint: crate::renderer::chunk::mesher::pack_light_tint(
                light,
                crate::renderer::chunk::mesher::PACKED_WHITE_SHIFTED,
            ),
            normal,
            tint_index,
        });
    }
}

fn build_flat_quad(region: AtlasRegion, tint_index: Option<u32>) -> Vec<ItemVertex> {
    let h = 0.5;
    let positions = [
        [-h, -h, 0.0],
        [h, -h, 0.0],
        [h, h, 0.0],
        [-h, -h, 0.0],
        [h, h, 0.0],
        [-h, h, 0.0],
    ];
    let tint_index = tint_index_or_none(tint_index);
    let uvs = [
        [region.u_min, region.v_max],
        [region.u_max, region.v_max],
        [region.u_max, region.v_min],
        [region.u_min, region.v_max],
        [region.u_max, region.v_min],
        [region.u_min, region.v_min],
    ];
    positions
        .iter()
        .zip(uvs.iter())
        .map(|(p, uv)| ItemVertex {
            position: *p,
            tex_coords: *uv,
            light_tint: crate::renderer::chunk::mesher::pack_light_tint(
                1.0,
                crate::renderer::chunk::mesher::PACKED_WHITE_SHIFTED,
            ),
            normal: pack_normal(glam::Vec3::Z),
            tint_index,
        })
        .collect()
}

pub(super) fn create_pipeline(
    device: &rhi::backend::Device,
    targets: rhi::backend::PipelineTargets,
    layout: rhi::backend::PipelineLayout,
) -> rhi::backend::RenderPipeline {
    create_pipeline_impl(device, targets, layout, rhi::FrontFace::Ccw, false, true)
}

/// The dropped-item pipelines: `(cutout, translucent)`.
fn create_world_pipelines(
    device: &rhi::backend::Device,
    targets: rhi::backend::PipelineTargets,
    layout: rhi::backend::PipelineLayout,
) -> (rhi::backend::RenderPipeline, rhi::backend::RenderPipeline) {
    let world =
        |blend| create_pipeline_impl(device, targets, layout, rhi::FrontFace::Ccw, true, blend);
    (world(false), world(true))
}

pub(super) fn create_pipeline_with_front_face(
    device: &rhi::backend::Device,
    targets: rhi::backend::PipelineTargets,
    layout: rhi::backend::PipelineLayout,
    front_face: rhi::FrontFace,
) -> rhi::backend::RenderPipeline {
    create_pipeline_impl(device, targets, layout, front_face, false, true)
}

fn create_pipeline_impl(
    device: &rhi::backend::Device,
    targets: rhi::backend::PipelineTargets,
    layout: rhi::backend::PipelineLayout,
    front_face: rhi::FrontFace,
    world_lighting: bool,
    blend: bool,
) -> rhi::backend::RenderPipeline {
    let vertex_shader: rhi::Shader = if world_lighting {
        shader::include_shader!("item_entity_world.vert")
    } else {
        shader::include_shader!("item_entity.vert")
    };
    rhi::backend::create_render_pipeline(
        device,
        targets,
        layout,
        &rhi::RenderPipelineDesc {
            cull_mode: rhi::CullMode::Back,
            front_face,
            // Vanilla 26.2's DepthStencilState.DEFAULT is inclusive
            // GREATER_THAN_OR_EQUAL under reversed-Z. Pomme uses conventional
            // depth here, so the equivalent comparison is LESS_OR_EQUAL. The
            // equality case matters where generated front/back planes meet
            // their per-pixel extrusion walls at the exact same depth.
            depth: Some(rhi::DepthState::WRITE),
            blend: blend.then_some(rhi::BlendState::OVER_REPLACE_ALPHA),
            ..rhi::RenderPipelineDesc::new(
                "item entity",
                vertex_shader,
                shader::include_shader!("item_entity.frag"),
                &[ItemVertex::BUFFER],
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_region() -> AtlasRegion {
        AtlasRegion {
            u_min: 0.0,
            v_min: 0.0,
            u_max: 1.0,
            v_max: 1.0,
            pixel_rect: [0, 0, 1, 1],
            sprite: 0,
            opaque: true,
            translucent: false,
        }
    }

    fn unpack_uv(vertex: &ItemVertex) -> [f32; 2] {
        vertex.tex_coords
    }

    fn assert_uvs_near(vertices: &[ItemVertex], expected: &[[f32; 2]]) {
        assert_eq!(vertices.len(), expected.len());
        for (vertex, expected) in vertices.iter().zip(expected) {
            let actual = unpack_uv(vertex);
            assert!(
                (actual[0] - expected[0]).abs() <= f32::EPSILON
                    && (actual[1] - expected[1]).abs() <= f32::EPSILON,
                "UV {actual:?} differs from expected {expected:?}"
            );
        }
    }

    fn unpack_normal(vertex: &ItemVertex) -> glam::Vec3 {
        glam::Vec3::new(
            vertex.normal[0] as f32 / 127.0,
            vertex.normal[1] as f32 / 127.0,
            vertex.normal[2] as f32 / 127.0,
        )
    }

    #[test]
    fn extruded_item_side_faces_wind_outward() {
        let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
        let vertices = build_extruded_item(&image, unit_region());
        let expected_normals = [
            glam::Vec3::Y,
            glam::Vec3::NEG_Y,
            glam::Vec3::NEG_X,
            glam::Vec3::X,
        ];

        assert_eq!(vertices.len(), 36);
        for (face, expected) in vertices[12..]
            .as_chunks::<6>()
            .0
            .iter()
            .zip(expected_normals)
        {
            let p0 = glam::Vec3::from_array(face[0].position);
            let p1 = glam::Vec3::from_array(face[1].position);
            let p2 = glam::Vec3::from_array(face[2].position);
            let normal = (p1 - p0).cross(p2 - p0);
            assert!(
                normal.dot(expected) > 0.0,
                "side face normal {normal:?} points away from {expected:?}"
            );
        }
    }

    #[test]
    fn extruded_item_carries_vanilla_cardinal_normals() {
        let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
        let vertices = build_extruded_item(&image, unit_region());
        let expected = [
            glam::Vec3::Z,
            glam::Vec3::NEG_Z,
            glam::Vec3::Y,
            glam::Vec3::NEG_Y,
            glam::Vec3::NEG_X,
            glam::Vec3::X,
        ];

        for (face, expected) in vertices.as_chunks::<6>().0.iter().zip(expected) {
            for vertex in face {
                assert_eq!(unpack_normal(vertex), expected);
            }
        }
    }

    #[test]
    fn baked_quad_normal_matches_vanilla_closest_cardinal_direction() {
        let angle = 22.5_f32.to_radians();
        let normal = glam::Vec3::new(angle.sin(), 0.0, angle.cos());
        let tangent = glam::Vec3::X * angle.cos() - glam::Vec3::Z * angle.sin();
        let up = glam::Vec3::Y;
        let p0 = -tangent * 0.5 - up * 0.5;
        let p1 = tangent * 0.5 - up * 0.5;
        let p2 = tangent * 0.5 + up * 0.5;
        let p3 = -tangent * 0.5 + up * 0.5;
        let mut positions = [p0.to_array(), p1.to_array(), p2.to_array(), p3.to_array()];

        // Ensure the synthetic winding points along the intended oblique normal.
        if crate::world::block::model::quad_normal(&positions)
            .unwrap()
            .dot(normal)
            < 0.0
        {
            positions.reverse();
        }
        assert_eq!(cardinal_normal(&positions), glam::Vec3::Z);
    }

    #[test]
    fn generated_item_layer_preserves_arbitrary_tint_index() {
        let mask = SpriteAlphaMask {
            width: 1,
            height: 1,
            frames: vec![vec![true]],
        };
        let vertices = build_extruded_item_mask(&mask, unit_region(), Some(37));
        assert!(!vertices.is_empty());
        assert!(vertices.iter().all(|vertex| vertex.tint_index == 37));
        assert!(
            vertices
                .iter()
                .all(|vertex| { vertex.light_tint.to_le_bytes()[1..] == [255, 255, 255] })
        );
    }

    #[test]
    fn untinted_item_vertices_use_tint_index_sentinel() {
        let mask = SpriteAlphaMask {
            width: 1,
            height: 1,
            frames: vec![vec![true]],
        };
        let vertices = build_extruded_item_mask(&mask, unit_region(), None);
        assert!(
            vertices
                .iter()
                .all(|vertex| vertex.tint_index == NO_ITEM_TINT)
        );
    }

    #[test]
    fn animated_extrusion_unions_exposed_edges_per_frame() {
        let mask = SpriteAlphaMask {
            width: 2,
            height: 1,
            frames: vec![vec![true, false], vec![false, true]],
        };
        let vertices = build_extruded_item_mask(&mask, unit_region(), None);

        // Each frame exposes the shared edge from one side, so vanilla emits
        // eight side quads total. Collapsing the frames into one opacity bitmap
        // would incorrectly remove both shared-edge quads and yield 48 vertices.
        assert_eq!(vertices.len(), 12 + 8 * 6);
    }

    #[test]
    fn extruded_item_back_face_preserves_sprite_orientation() {
        let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
        let vertices = build_extruded_item(&image, unit_region());

        assert_uvs_near(
            &vertices[6..12],
            &[
                [1.0, 1.0],
                [0.0, 1.0],
                [0.0, 0.0],
                [1.0, 1.0],
                [0.0, 0.0],
                [1.0, 0.0],
            ],
        );
    }

    #[test]
    fn extruded_item_front_and_back_use_true_sprite_bounds() {
        let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
        let region = AtlasRegion {
            u_min: 16.0 / 64.0,
            v_min: 8.0 / 64.0,
            u_max: 32.0 / 64.0,
            v_max: 24.0 / 64.0,
            pixel_rect: [16, 8, 16, 16],
            sprite: 1,
            opaque: true,
            translucent: false,
        };
        let vertices = build_extruded_item(&image, region);

        assert_uvs_near(
            &vertices[..12],
            &[
                [region.u_min, region.v_max],
                [region.u_max, region.v_max],
                [region.u_max, region.v_min],
                [region.u_min, region.v_max],
                [region.u_max, region.v_min],
                [region.u_min, region.v_min],
                [region.u_max, region.v_max],
                [region.u_min, region.v_max],
                [region.u_min, region.v_min],
                [region.u_max, region.v_max],
                [region.u_min, region.v_min],
                [region.u_max, region.v_min],
            ],
        );
    }

    #[test]
    fn extruded_item_side_faces_match_vanilla_pixel_inset_uvs() {
        let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
        let vertices = build_extruded_item(&image, unit_region());
        let expected = [
            // UP
            [0.1, 0.1],
            [0.1, 0.9],
            [0.9, 0.9],
            [0.1, 0.1],
            [0.9, 0.9],
            [0.9, 0.1],
            // DOWN
            [0.9, 0.9],
            [0.9, 0.1],
            [0.1, 0.1],
            [0.9, 0.9],
            [0.1, 0.1],
            [0.1, 0.9],
            // LEFT / west-facing geometry
            [0.9, 0.9],
            [0.1, 0.9],
            [0.1, 0.1],
            [0.9, 0.9],
            [0.1, 0.1],
            [0.9, 0.1],
            // RIGHT / east-facing geometry
            [0.1, 0.1],
            [0.9, 0.1],
            [0.9, 0.9],
            [0.1, 0.1],
            [0.9, 0.9],
            [0.1, 0.9],
        ];

        assert_uvs_near(&vertices[12..], &expected);
    }
}
