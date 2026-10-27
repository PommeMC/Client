use std::path::Path;

use crate::assets::{AssetIndex, resolve_asset_path};
use crate::renderer::camera::{Camera, CameraUniform, CloudMode, MIN_FAR};
use crate::renderer::pipelines::sky::SkyState;
use crate::renderer::rhi::{self, VertexFormat};
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, shader, util};

/// Vertical thickness of the cloud layer in blocks.
const CLOUD_THICKNESS: f32 = 4.0;
/// Horizontal size of one cloud cell in blocks (one clouds.png texel).
const CELL_SIZE: f32 = 12.0;
/// Scroll speed factor: vanilla `cloudOffset * 0.030000001` blocks per tick.
const SCROLL_PER_TICK: f32 = 0.030_000_001;
/// Fixed Z bias vanilla adds to the cloud origin.
const Z_BIAS: f64 = 3.96;

/// Distance (blocks) by which cloud alpha fades to zero (vanilla
/// `FogCloudsEnd`). Kept just below the far-plane floor so the fade completes
/// before clouds would clip at it into a hard circle.
// TODO: vanilla reaches its full `cloudRange` (128 chunks / 2048 blocks). Here
// the far-plane floor (`camera::MIN_FAR`) caps the reach so the instance
// buffers stay a fixed size; matching vanilla means a cloud-range option that
// also raises the floor, like vanilla's `depthFar` does.
const CLOUD_FADE_END: f32 = MIN_FAR * 0.96;
/// Disc radius in cells: `ceil(CLOUD_FADE_END / CELL_SIZE)`.
const RADIUS_CELLS: i32 = (CLOUD_FADE_END as i32 + CELL_SIZE as i32 - 1) / CELL_SIZE as i32;

/// Upper bound on faces for the per-frame instance buffers, mirroring vanilla
/// `CloudRenderer.getSizeForCloudDistance` (4 faces per cell over the disc's
/// bounding diamond, plus the 9 interior cells' extra faces).
const MAX_FACES: usize = {
    let span = ((RADIUS_CELLS + 1) * 2) as usize;
    (span * span / 2) * 4 + 54
};

/// Face direction, matching vanilla `Direction.get3DDataValue()` and the corner
/// table in `clouds.vert`.
const DIR_DOWN: u8 = 0;
const DIR_UP: u8 = 1;
const DIR_NORTH: u8 = 2;
const DIR_SOUTH: u8 = 3;
const DIR_WEST: u8 = 4;
const DIR_EAST: u8 = 5;

/// Face flag bit: reverse winding so the face is visible from inside the cell.
const FLAG_INSIDE_FACE: u8 = 1 << 4;
/// Face flag bit: use the top (brightest) shade regardless of direction.
const FLAG_USE_TOP_COLOR: u8 = 1 << 5;

/// One cloud face, drawn as a single instance; the vertex shader expands it
/// into a quad. Layout must match the attributes in `clouds.vert`.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct CloudFace {
    /// Relative cell offset (rcx, rcz) from the centre cell.
    cell: [i16; 2],
    dir: u8,
    flags: u8,
}

/// Push constants shared by both stages: cloud tint (rgba) and the camera-
/// relative grid offset (xyz) plus the fog-fade end (w). Layout must match
/// `clouds.vert`/`clouds.frag`.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct CloudPush {
    tint: [f32; 4],
    offset: [f32; 4],
}

/// Where the camera sits relative to the cloud slab, used to cull the unseen
/// horizontal face (vanilla `RelativeCameraPos`). Drawing both top and bottom
/// makes the two 4-block-apart faces z-fight at distance.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RelativePos {
    Above,
    Inside,
    Below,
}

#[derive(Clone, Copy, Default)]
struct CloudCell {
    present: bool,
    /// Whether the neighbour in each direction is empty (cull the shared face).
    north_empty: bool,
    east_empty: bool,
    south_empty: bool,
    west_empty: bool,
}

struct CloudGrid {
    width: u32,
    height: u32,
    cells: Vec<CloudCell>,
}

impl CloudGrid {
    fn cell(&self, gx: usize, gz: usize) -> &CloudCell {
        &self.cells[gx + gz * self.width as usize]
    }
}

pub struct CloudPipeline {
    fancy_pipeline: rhi::backend::RenderPipeline,
    flat_pipeline: rhi::backend::RenderPipeline,
    pipeline_layout: rhi::backend::PipelineLayout,
    camera_layout: rhi::backend::BindGroupLayout,
    camera_sets: Vec<rhi::backend::BindGroup>,
    camera_buffers: Vec<rhi::backend::Buffer>,
    instance_buffers: Vec<rhi::backend::Buffer>,

    grid: Option<CloudGrid>,

    /// Cached CPU face list and the state it was built for.
    faces: Vec<CloudFace>,
    /// Per-frame flag: the frame's instance buffer still holds a stale mesh.
    dirty: Vec<bool>,
    prev_cell_x: i32,
    prev_cell_z: i32,
    prev_mode: CloudMode,
    prev_rel: RelativePos,
    have_mesh: bool,
}

impl CloudPipeline {
    pub fn new(
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
        jar_assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
    ) -> Self {
        let camera_layout =
            rhi::backend::create_bind_group_layout(device, util::VERTEX_FRAGMENT_UNIFORM);
        let pipeline_layout = rhi::backend::create_pipeline_layout(
            device,
            &[camera_layout],
            Some(rhi::PushConstants {
                visibility: rhi::ShaderStages::VERTEX_FRAGMENT,
                size: std::mem::size_of::<CloudPush>() as u32,
            }),
        );

        let (fancy_pipeline, flat_pipeline) = create_pipelines(device, targets, pipeline_layout);

        let (camera_sets, camera_buffers) = util::create_uniform_sets(
            device,
            camera_layout,
            std::mem::size_of::<CameraUniform>() as u64,
            "cloud_camera",
        );

        let instance_buffers = util::create_per_frame_host_buffers(
            device,
            (MAX_FACES * std::mem::size_of::<CloudFace>()) as u64,
            rhi::BufferUsages::VERTEX,
            "cloud_faces",
        );

        let grid = load_cloud_grid(jar_assets_dir, asset_index);

        Self {
            fancy_pipeline,
            flat_pipeline,
            pipeline_layout,
            camera_layout,
            camera_sets,
            camera_buffers,
            instance_buffers,
            grid,
            faces: Vec::new(),
            dirty: vec![false; MAX_FRAMES_IN_FLIGHT],
            prev_cell_x: i32::MIN,
            prev_cell_z: i32::MIN,
            prev_mode: CloudMode::Off,
            prev_rel: RelativePos::Inside,
            have_mesh: false,
        }
    }

    pub fn update_camera(&mut self, frame: usize, uniform: &CameraUniform) {
        let bytes = bytemuck::bytes_of(uniform);
        self.camera_buffers[frame].write(bytes);
    }

    pub fn update_and_draw(
        &mut self,
        pass: &mut rhi::backend::RenderPassEncoder,
        frame: usize,
        camera: &Camera,
        sky: &SkyState,
        mode: CloudMode,
    ) {
        if mode == CloudMode::Off {
            return;
        }
        let Some(grid) = self.grid.as_ref() else {
            return;
        };

        // Stay in f64 until the values are camera-relative so the cell phase
        // and layer height keep precision at extreme coordinates.
        let eye = *camera.position + camera.third_person_offset().as_dvec3();

        // Scroll the cloud field in +X over time (vanilla `cloudOffset`).
        let cycle = grid.width as i64 * 400;
        let game_time = sky.game_time as i64;
        let cloud_offset = game_time.rem_euclid(cycle) as f32 + sky.partial_tick;

        let tex_w_blocks = grid.width as f64 * CELL_SIZE as f64;
        let tex_h_blocks = grid.height as f64 * CELL_SIZE as f64;
        let mut cloud_x = eye.x + (cloud_offset * SCROLL_PER_TICK) as f64;
        let mut cloud_z = eye.z + Z_BIAS;
        cloud_x -= (cloud_x / tex_w_blocks).floor() * tex_w_blocks;
        cloud_z -= (cloud_z / tex_h_blocks).floor() * tex_h_blocks;

        let cell_x = (cloud_x / CELL_SIZE as f64).floor() as i32;
        let cell_z = (cloud_z / CELL_SIZE as f64).floor() as i32;
        let x_in_cell = (cloud_x - cell_x as f64 * CELL_SIZE as f64) as f32;
        let z_in_cell = (cloud_z - cell_z as f64 * CELL_SIZE as f64) as f32;
        let relative_bottom_y = (f64::from(sky.attributes.cloud_height) - eye.y) as f32;
        let relative_top_y = relative_bottom_y + CLOUD_THICKNESS;
        let rel = if relative_top_y < 0.0 {
            RelativePos::Above
        } else if relative_bottom_y > 0.0 {
            RelativePos::Below
        } else {
            RelativePos::Inside
        };

        // Rebuild the cached mesh only when the camera crosses a cell, the mode
        // changes, or the camera moves above/below the layer (which face is
        // visible changes); the smooth sub-cell scroll is applied via the offset.
        if !self.have_mesh
            || cell_x != self.prev_cell_x
            || cell_z != self.prev_cell_z
            || mode != self.prev_mode
            || rel != self.prev_rel
        {
            self.build_mesh(cell_x, cell_z, mode, rel);
            self.prev_cell_x = cell_x;
            self.prev_cell_z = cell_z;
            self.prev_mode = mode;
            self.prev_rel = rel;
            self.have_mesh = true;
            self.dirty.fill(true);
        }

        if self.faces.is_empty() {
            return;
        }

        if self.dirty[frame] {
            let bytes = bytemuck::cast_slice::<CloudFace, u8>(&self.faces);
            self.instance_buffers[frame].write(bytes);
            self.dirty[frame] = false;
        }

        let tint = sky.attributes.cloud_color.to_array();
        // offset.w carries the fog-fade end (blocks): the fragment shader fades
        // cloud alpha to zero by this distance so the field melts into the sky
        // instead of ending at a hard circle.
        let push = CloudPush {
            tint,
            offset: [-x_in_cell, relative_bottom_y, -z_in_cell, CLOUD_FADE_END],
        };

        let pipeline = if mode == CloudMode::Fancy {
            self.fancy_pipeline
        } else {
            self.flat_pipeline
        };
        pass.set_pipeline(&pipeline);
        pass.set_vertex_buffer(0, self.instance_buffers[frame].handle());
        pass.set_bind_group(0, self.camera_sets[frame]);
        pass.set_push_constants(
            rhi::ShaderStages::VERTEX_FRAGMENT,
            0,
            bytemuck::bytes_of(&push),
        );
        // Six vertices (two triangles) per face, one instance per face.
        pass.draw(6, self.faces.len() as u32, 0, 0);
    }

    fn build_mesh(&mut self, cell_x: i32, cell_z: i32, mode: CloudMode, rel: RelativePos) {
        self.faces.clear();
        let Some(grid) = self.grid.as_ref() else {
            return;
        };
        let faces = &mut self.faces;
        let fancy = mode == CloudMode::Fancy;
        let r2 = RADIUS_CELLS * RADIUS_CELLS;

        let mut emit = |rcx: i32, rcz: i32| {
            let gx = (cell_x + rcx).rem_euclid(grid.width as i32) as usize;
            let gz = (cell_z + rcz).rem_euclid(grid.height as i32) as usize;
            let cell = grid.cell(gx, gz);
            if !cell.present {
                return;
            }
            if fancy {
                build_extruded_cell(faces, rcx, rcz, cell, rel);
            } else {
                build_flat_cell(faces, rcx, rcz);
            }
        };

        // Vanilla `CloudRenderer.buildMesh`: walk rings outward from the centre so
        // nearer cells draw first (depth-write keeps the closest face).
        for ring in 0..=(2 * RADIUS_CELLS) {
            for rcx in -ring..=ring {
                let rcz = ring - rcx.abs();
                if !(0..=RADIUS_CELLS).contains(&rcz) || rcx * rcx + rcz * rcz > r2 {
                    continue;
                }
                if rcz != 0 {
                    emit(rcx, -rcz);
                }
                emit(rcx, rcz);
            }
        }

        if faces.len() > MAX_FACES {
            faces.truncate(MAX_FACES);
        }
    }

    pub fn recreate_pipeline(
        &mut self,
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
    ) {
        self.fancy_pipeline.destroy(device);
        self.flat_pipeline.destroy(device);
        (self.fancy_pipeline, self.flat_pipeline) =
            create_pipelines(device, targets, self.pipeline_layout);
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        util::destroy_buffers(device, &mut self.camera_buffers);
        util::destroy_buffers(device, &mut self.instance_buffers);

        self.fancy_pipeline.destroy(device);
        self.flat_pipeline.destroy(device);
        self.pipeline_layout.destroy(device);
        util::destroy_bind_groups(device, self.camera_sets.drain(..));
        self.camera_layout.destroy(device);
    }
}

fn emit_face(faces: &mut Vec<CloudFace>, rcx: i32, rcz: i32, dir: u8, flags: u8) {
    faces.push(CloudFace {
        cell: [rcx as i16, rcz as i16],
        dir,
        flags,
    });
}

/// Fast clouds: a single down-facing quad at the layer bottom using the top
/// (brightest) shade (vanilla `buildFlatCell` → `encodeFace(DOWN,
/// USE_TOP_COLOR)`).
fn build_flat_cell(faces: &mut Vec<CloudFace>, rcx: i32, rcz: i32) {
    emit_face(faces, rcx, rcz, DIR_DOWN, FLAG_USE_TOP_COLOR);
}

/// Fancy clouds: an extruded 3D box per cell, mirroring vanilla
/// `CloudRenderer.buildExtrudedCell`. Exterior faces use normal winding and
/// back-face culling; the centre 3x3 cells additionally get reversed-winding
/// inside faces so the cloud remains visible when the camera enters the volume.
fn build_extruded_cell(
    faces: &mut Vec<CloudFace>,
    rcx: i32,
    rcz: i32,
    cell: &CloudCell,
    rel: RelativePos,
) {
    if rel != RelativePos::Below {
        emit_face(faces, rcx, rcz, DIR_UP, 0);
    }
    if rel != RelativePos::Above {
        emit_face(faces, rcx, rcz, DIR_DOWN, 0);
    }
    if cell.north_empty && rcz > 0 {
        emit_face(faces, rcx, rcz, DIR_NORTH, 0);
    }
    if cell.south_empty && rcz < 0 {
        emit_face(faces, rcx, rcz, DIR_SOUTH, 0);
    }
    if cell.west_empty && rcx > 0 {
        emit_face(faces, rcx, rcz, DIR_WEST, 0);
    }
    if cell.east_empty && rcx < 0 {
        emit_face(faces, rcx, rcz, DIR_EAST, 0);
    }

    if rcx.abs() <= 1 && rcz.abs() <= 1 {
        for dir in [DIR_DOWN, DIR_UP, DIR_NORTH, DIR_SOUTH, DIR_WEST, DIR_EAST] {
            emit_face(faces, rcx, rcz, dir, FLAG_INSIDE_FACE);
        }
    }
}

/// Loads and parses `clouds.png` into a cell grid. A texel is a cloud cell when
/// its alpha is >= 10 (vanilla `isCellEmpty`). The texel colour is ignored —
/// vanilla colours clouds purely from the uniform tint times the face shade.
fn load_cloud_grid(jar_assets_dir: &Path, asset_index: &Option<AssetIndex>) -> Option<CloudGrid> {
    let key = "minecraft/textures/environment/clouds.png";
    let path = resolve_asset_path(jar_assets_dir, asset_index, key);
    let (pixels, w, h) = util::load_png(&path)?;
    let width = w as usize;
    let height = h as usize;

    let alpha = |x: i32, y: i32| -> u8 {
        let xx = x.rem_euclid(w as i32) as usize;
        let yy = y.rem_euclid(h as i32) as usize;
        pixels[(xx + yy * width) * 4 + 3]
    };
    let empty = |x: i32, y: i32| alpha(x, y) < 10;

    let mut cells = Vec::with_capacity(width * height);
    for y in 0..height as i32 {
        for x in 0..width as i32 {
            if empty(x, y) {
                cells.push(CloudCell::default());
                continue;
            }
            cells.push(CloudCell {
                present: true,
                north_empty: empty(x, y - 1),
                east_empty: empty(x + 1, y),
                south_empty: empty(x, y + 1),
                west_empty: empty(x - 1, y),
            });
        }
    }

    tracing::info!("Clouds: loaded {key} ({w}x{h})");
    Some(CloudGrid {
        width: w,
        height: h,
        cells,
    })
}

fn create_pipelines(
    device: &rhi::backend::Device,
    targets: rhi::backend::PipelineTargets,
    layout: rhi::backend::PipelineLayout,
) -> (rhi::backend::RenderPipeline, rhi::backend::RenderPipeline) {
    (
        create_pipeline(device, targets, layout, rhi::CullMode::Back),
        create_pipeline(device, targets, layout, rhi::CullMode::None),
    )
}

fn create_pipeline(
    device: &rhi::backend::Device,
    targets: rhi::backend::PipelineTargets,
    layout: rhi::backend::PipelineLayout,
    cull_mode: rhi::CullMode,
) -> rhi::backend::RenderPipeline {
    // Each instance is a packed cloud face, expanded to a quad by the vertex
    // shader via gl_VertexIndex.
    rhi::backend::create_render_pipeline(
        device,
        targets,
        layout,
        &rhi::RenderPipelineDesc {
            cull_mode,
            // Depth-test against the world so terrain occludes clouds. Write
            // depth too: clouds are near-opaque (0.8 alpha) and 3D boxes
            // overlap, so depth writes keep the nearest cloud face and avoid
            // double-blending the layer.
            depth: Some(rhi::DepthState {
                compare: rhi::CompareFunction::Less,
                ..rhi::DepthState::WRITE
            }),
            blend: Some(rhi::BlendState::TRANSLUCENT),
            ..rhi::RenderPipelineDesc::new(
                "clouds",
                shader::include_shader!("clouds.vert"),
                shader::include_shader!("clouds.frag"),
                &[rhi::VertexBufferLayout {
                    stride: size_of::<CloudFace>() as u32,
                    step_mode: rhi::VertexStepMode::Instance,
                    attributes: &rhi::packed_attributes(
                        0,
                        [VertexFormat::Sint16x2, VertexFormat::Uint8x2],
                    ),
                }],
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interior_fancy_cell_keeps_exterior_faces_and_adds_reversed_inside_faces() {
        let cell = CloudCell {
            south_empty: true,
            west_empty: true,
            ..Default::default()
        };
        let mut faces = Vec::new();

        build_extruded_cell(&mut faces, 1, -1, &cell, RelativePos::Inside);

        let exterior: Vec<_> = faces
            .iter()
            .filter(|face| face.flags & FLAG_INSIDE_FACE == 0)
            .map(|face| face.dir)
            .collect();
        let inside: Vec<_> = faces
            .iter()
            .filter(|face| face.flags & FLAG_INSIDE_FACE != 0)
            .map(|face| face.dir)
            .collect();

        assert_eq!(exterior, vec![DIR_UP, DIR_DOWN, DIR_SOUTH, DIR_WEST]);
        assert_eq!(
            inside,
            vec![DIR_DOWN, DIR_UP, DIR_NORTH, DIR_SOUTH, DIR_WEST, DIR_EAST]
        );
    }

    #[test]
    fn non_interior_fancy_cell_only_emits_visible_exterior_faces() {
        let cell = CloudCell {
            west_empty: true,
            ..Default::default()
        };
        let mut faces = Vec::new();

        build_extruded_cell(&mut faces, 2, 0, &cell, RelativePos::Above);

        assert_eq!(faces.len(), 2);
        assert_eq!(faces[0].dir, DIR_UP);
        assert_eq!(faces[1].dir, DIR_WEST);
        assert!(faces.iter().all(|face| face.flags == 0));
    }
}
