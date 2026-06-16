use std::path::Path;
use std::sync::{Arc, Mutex};

use pomme_gpu_allocator::vulkan::{Allocation, Allocator};
use pyronyx::vk;

use crate::assets::{AssetIndex, resolve_asset_path};
use crate::renderer::camera::{Camera, CameraUniform, CloudMode};
use crate::renderer::pipelines::sky::SkyState;
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, shader, util};

/// World-space Y of the cloud layer bottom (vanilla overworld `cloud_height`).
const CLOUD_HEIGHT: f32 = 192.0;
/// Vertical thickness of the cloud layer in blocks.
const CLOUD_THICKNESS: f32 = 4.0;
/// Horizontal size of one cloud cell in blocks (one clouds.png texel).
const CELL_SIZE: f32 = 12.0;
/// Scroll speed factor: vanilla `cloudOffset * 0.030000001` blocks per tick.
const SCROLL_PER_TICK: f32 = 0.030_000_001;
/// Fixed Z bias vanilla adds to the cloud origin.
const Z_BIAS: f64 = 3.96;

/// Per-face brightness, matching the classic Minecraft cloud shading.
const SHADE_TOP: f32 = 1.0;
const SHADE_BOTTOM: f32 = 0.7;
const SHADE_NS: f32 = 0.8; // north / south faces (Z)
const SHADE_EW: f32 = 0.9; // east / west faces (X)

/// How far the cloud layer extends, in chunks. Fixed (not tied to the view
/// distance) so the layer reaches toward the horizon like vanilla regardless of
/// the player's render-distance setting.
const CLOUD_RENDER_DISTANCE_CHUNKS: i32 = 32;
/// Disc radius in cells. `ceil(CLOUD_RENDER_DISTANCE_CHUNKS * 16 / CELL_SIZE)`.
const RADIUS_CELLS: i32 =
    (CLOUD_RENDER_DISTANCE_CHUNKS * 16 + CELL_SIZE as i32 - 1) / CELL_SIZE as i32;

/// Upper bound on vertices for the per-frame buffers: each cell emits at most 6
/// faces (top, bottom, four sides) of 6 vertices.
const MAX_CELLS: usize = ((2 * RADIUS_CELLS + 1) * (2 * RADIUS_CELLS + 1)) as usize;
const MAX_CLOUD_VERTS: usize = MAX_CELLS * 6 * 6;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct CloudVertex {
    position: [f32; 3],
    /// Per-cell texel colour pre-multiplied by the face shade; alpha unused
    /// (the cloud tint alpha is applied in the shader).
    color: [f32; 4],
}

/// Push constants shared by both stages: cloud tint (rgba) and the camera-
/// relative grid offset (xyz). Layout must match `clouds.vert`/`clouds.frag`.
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
    rgb: [f32; 3],
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
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    camera_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    camera_sets: Vec<vk::DescriptorSet>,
    camera_buffers: Vec<vk::Buffer>,
    camera_allocations: Vec<Option<Allocation>>,
    vertex_buffers: Vec<vk::Buffer>,
    vertex_allocations: Vec<Option<Allocation>>,

    grid: Option<CloudGrid>,

    /// Cached CPU mesh and the state it was built for.
    verts: Vec<CloudVertex>,
    /// Per-frame flag: the frame's vertex buffer still holds a stale mesh.
    dirty: Vec<bool>,
    prev_cell_x: i32,
    prev_cell_z: i32,
    prev_mode: CloudMode,
    prev_rel: RelativePos,
    have_mesh: bool,
}

impl CloudPipeline {
    pub fn new(
        device: &vk::Device,
        render_pass: vk::RenderPass,
        allocator: &Arc<Mutex<Allocator>>,
        jar_assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
    ) -> Self {
        let camera_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::UniformBuffer,
            vk::ShaderStageFlags::Vertex | vk::ShaderStageFlags::Fragment,
        );

        let push_range = vk::PushConstantRange {
            stage_flags: vk::ShaderStageFlags::Vertex | vk::ShaderStageFlags::Fragment,
            offset: 0,
            size: std::mem::size_of::<CloudPush>() as u32,
        };
        let layouts = [camera_layout];
        let layout_info = vk::PipelineLayoutCreateInfo {
            set_layout_count: layouts.len() as u32,
            set_layouts: layouts.as_ptr(),
            push_constant_range_count: 1,
            push_constant_ranges: &push_range,
            ..Default::default()
        };
        let pipeline_layout = device
            .create_pipeline_layout(&layout_info, None)
            .expect("failed to create cloud pipeline layout");

        let pipeline = create_pipeline(device, render_pass, pipeline_layout);

        let pool_sizes = [vk::DescriptorPoolSize {
            ty: vk::DescriptorType::UniformBuffer,
            descriptor_count: MAX_FRAMES_IN_FLIGHT as u32,
        }];
        let pool_info = vk::DescriptorPoolCreateInfo {
            max_sets: MAX_FRAMES_IN_FLIGHT as u32,
            pool_size_count: pool_sizes.len() as u32,
            pool_sizes: pool_sizes.as_ptr(),
            ..Default::default()
        };
        let descriptor_pool = device
            .create_descriptor_pool(&pool_info, None)
            .expect("failed to create cloud descriptor pool");

        let camera_layouts: Vec<_> = (0..MAX_FRAMES_IN_FLIGHT).map(|_| camera_layout).collect();
        let camera_alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: camera_layouts.len() as u32,
            set_layouts: camera_layouts.as_ptr(),
            ..Default::default()
        };
        let mut camera_sets = vec![vk::DescriptorSet::null(); camera_layouts.len()];
        device
            .allocate_descriptor_sets(&camera_alloc_info, &mut camera_sets)
            .expect("failed to allocate cloud camera sets");

        let mut camera_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut camera_allocations: Vec<Option<Allocation>> =
            Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        for &set in &camera_sets {
            let (buf, alloc) = util::create_uniform_buffer(
                device,
                allocator,
                std::mem::size_of::<CameraUniform>() as u64,
                "cloud_camera",
            );
            let buffer_info = vk::DescriptorBufferInfo {
                buffer: buf,
                offset: 0,
                range: std::mem::size_of::<CameraUniform>() as u64,
            };
            let write = vk::WriteDescriptorSet {
                dst_set: set,
                dst_binding: 0,
                descriptor_type: vk::DescriptorType::UniformBuffer,
                descriptor_count: 1,
                buffer_info: &buffer_info,
                ..Default::default()
            };
            device.update_descriptor_sets(&[write], &[]);
            camera_buffers.push(buf);
            camera_allocations.push(Some(alloc));
        }

        let mut vertex_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut vertex_allocations: Vec<Option<Allocation>> =
            Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let vertex_bytes = (MAX_CLOUD_VERTS * std::mem::size_of::<CloudVertex>()) as u64;
        for _ in 0..MAX_FRAMES_IN_FLIGHT {
            let (buf, alloc) = util::create_host_buffer(
                device,
                allocator,
                vertex_bytes,
                vk::BufferUsageFlags::VertexBuffer,
                "cloud_vertices",
            );
            vertex_buffers.push(buf);
            vertex_allocations.push(Some(alloc));
        }

        let grid = load_cloud_grid(jar_assets_dir, asset_index);

        Self {
            pipeline,
            pipeline_layout,
            camera_layout,
            descriptor_pool,
            camera_sets,
            camera_buffers,
            camera_allocations,
            vertex_buffers,
            vertex_allocations,
            grid,
            verts: Vec::new(),
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
        if let Some(alloc) = self.camera_allocations[frame].as_mut() {
            alloc.mapped_slice_mut().unwrap()[..bytes.len()].copy_from_slice(bytes);
        }
    }

    pub fn update_and_draw(
        &mut self,
        cmd: vk::CommandBuffer,
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

        let cam = camera.position.as_vec3() + camera.third_person_offset();

        // Scroll the cloud field in +X over time (vanilla `cloudOffset`).
        let cycle = grid.width as i64 * 400;
        let game_time = sky.game_time as i64;
        let cloud_offset = game_time.rem_euclid(cycle) as f32 + sky.partial_tick;

        let tex_w_blocks = grid.width as f64 * CELL_SIZE as f64;
        let tex_h_blocks = grid.height as f64 * CELL_SIZE as f64;
        let mut cloud_x = cam.x as f64 + (cloud_offset * SCROLL_PER_TICK) as f64;
        let mut cloud_z = cam.z as f64 + Z_BIAS;
        cloud_x -= (cloud_x / tex_w_blocks).floor() * tex_w_blocks;
        cloud_z -= (cloud_z / tex_h_blocks).floor() * tex_h_blocks;

        let cell_x = (cloud_x / CELL_SIZE as f64).floor() as i32;
        let cell_z = (cloud_z / CELL_SIZE as f64).floor() as i32;
        let x_in_cell = (cloud_x - cell_x as f64 * CELL_SIZE as f64) as f32;
        let z_in_cell = (cloud_z - cell_z as f64 * CELL_SIZE as f64) as f32;
        let relative_bottom_y = CLOUD_HEIGHT - cam.y;
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
            for d in &mut self.dirty {
                *d = true;
            }
        }

        if self.verts.is_empty() {
            return;
        }

        if self.dirty[frame] {
            let bytes = bytemuck::cast_slice::<CloudVertex, u8>(&self.verts);
            if let Some(alloc) = self.vertex_allocations[frame].as_mut() {
                alloc.mapped_slice_mut().unwrap()[..bytes.len()].copy_from_slice(bytes);
            }
            self.dirty[frame] = false;
        }

        let tint = sky.cloud_color();
        // offset.w carries the disc edge distance (blocks) so the fragment
        // shader can fade clouds out instead of cutting off at a hard circle.
        let edge = RADIUS_CELLS as f32 * CELL_SIZE;
        let push = CloudPush {
            tint,
            offset: [-x_in_cell, relative_bottom_y, -z_in_cell, edge],
        };

        cmd.bind_pipeline(vk::PipelineBindPoint::Graphics, self.pipeline);
        cmd.bind_vertex_buffers(0, &[self.vertex_buffers[frame]], &[0]);
        cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Graphics,
            self.pipeline_layout,
            0,
            &[self.camera_sets[frame]],
            &[],
        );
        cmd.push_constants(
            self.pipeline_layout,
            vk::ShaderStageFlags::Vertex | vk::ShaderStageFlags::Fragment,
            0,
            bytemuck::bytes_of(&push),
        );
        cmd.draw(self.verts.len() as u32, 1, 0, 0);
    }

    fn build_mesh(&mut self, cell_x: i32, cell_z: i32, mode: CloudMode, rel: RelativePos) {
        self.verts.clear();
        let Some(grid) = self.grid.as_ref() else {
            return;
        };
        let r2 = RADIUS_CELLS * RADIUS_CELLS;
        let fancy = mode == CloudMode::Fancy;

        for rcz in -RADIUS_CELLS..=RADIUS_CELLS {
            for rcx in -RADIUS_CELLS..=RADIUS_CELLS {
                if rcx * rcx + rcz * rcz > r2 {
                    continue;
                }
                let gx = (cell_x + rcx).rem_euclid(grid.width as i32) as usize;
                let gz = (cell_z + rcz).rem_euclid(grid.height as i32) as usize;
                let cell = *grid.cell(gx, gz);
                if !cell.present {
                    continue;
                }
                if fancy {
                    build_extruded_cell(&mut self.verts, rcx, rcz, &cell, rel);
                } else {
                    build_flat_cell(&mut self.verts, rcx, rcz, &cell);
                }
            }
        }

        if self.verts.len() > MAX_CLOUD_VERTS {
            self.verts.truncate(MAX_CLOUD_VERTS);
        }
    }

    pub fn recreate_pipeline(&mut self, device: &vk::Device, render_pass: vk::RenderPass) {
        device.destroy_pipeline(self.pipeline, None);
        self.pipeline = create_pipeline(device, render_pass, self.pipeline_layout);
    }

    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        let mut alloc = allocator.lock().unwrap();
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            device.destroy_buffer(self.camera_buffers[i], None);
            if let Some(a) = self.camera_allocations[i].take() {
                alloc.free(a).ok();
            }
            device.destroy_buffer(self.vertex_buffers[i], None);
            if let Some(a) = self.vertex_allocations[i].take() {
                alloc.free(a).ok();
            }
        }
        drop(alloc);

        device.destroy_pipeline(self.pipeline, None);
        device.destroy_pipeline_layout(self.pipeline_layout, None);
        device.destroy_descriptor_pool(self.descriptor_pool, None);
        device.destroy_descriptor_set_layout(self.camera_layout, None);
    }
}

/// Pushes a quad (two triangles) given its four corners in CCW order.
fn push_quad(
    verts: &mut Vec<CloudVertex>,
    a: [f32; 3],
    b: [f32; 3],
    c: [f32; 3],
    d: [f32; 3],
    rgb: [f32; 3],
    shade: f32,
) {
    let color = [rgb[0] * shade, rgb[1] * shade, rgb[2] * shade, 1.0];
    for p in [a, b, c, a, c, d] {
        verts.push(CloudVertex { position: p, color });
    }
}

/// Fast clouds: a single flat quad at the layer bottom, using the top colour.
fn build_flat_cell(verts: &mut Vec<CloudVertex>, rcx: i32, rcz: i32, cell: &CloudCell) {
    let bx = rcx as f32 * CELL_SIZE;
    let bz = rcz as f32 * CELL_SIZE;
    let x1 = bx + CELL_SIZE;
    let z1 = bz + CELL_SIZE;
    push_quad(
        verts,
        [bx, 0.0, bz],
        [bx, 0.0, z1],
        [x1, 0.0, z1],
        [x1, 0.0, bz],
        cell.rgb,
        SHADE_TOP,
    );
}

/// Fancy clouds: an extruded 3D box per cell, mirroring vanilla
/// `CloudRenderer.buildExtrudedCell`. The visible horizontal face is gated by
/// the camera position (`rel`); a side is drawn only when its neighbour is
/// empty and it faces back toward the centre, so translucent back-faces don't
/// show through. The centre cells get every face so the layer reads solid when
/// the camera is near or inside the clouds.
fn build_extruded_cell(
    verts: &mut Vec<CloudVertex>,
    rcx: i32,
    rcz: i32,
    cell: &CloudCell,
    rel: RelativePos,
) {
    let bx = rcx as f32 * CELL_SIZE;
    let bz = rcz as f32 * CELL_SIZE;
    let x1 = bx + CELL_SIZE;
    let z1 = bz + CELL_SIZE;
    let y0 = 0.0;
    let y1 = CLOUD_THICKNESS;
    let interior = rcx.abs() <= 1 && rcz.abs() <= 1;

    if interior || rel != RelativePos::Below {
        push_quad(
            verts,
            [bx, y1, bz],
            [bx, y1, z1],
            [x1, y1, z1],
            [x1, y1, bz],
            cell.rgb,
            SHADE_TOP,
        );
    }
    if interior || rel != RelativePos::Above {
        push_quad(
            verts,
            [bx, y0, z1],
            [bx, y0, bz],
            [x1, y0, bz],
            [x1, y0, z1],
            cell.rgb,
            SHADE_BOTTOM,
        );
    }
    if interior || (cell.north_empty && rcz > 0) {
        push_quad(
            verts,
            [bx, y0, bz],
            [x1, y0, bz],
            [x1, y1, bz],
            [bx, y1, bz],
            cell.rgb,
            SHADE_NS,
        );
    }
    if interior || (cell.south_empty && rcz < 0) {
        push_quad(
            verts,
            [x1, y0, z1],
            [bx, y0, z1],
            [bx, y1, z1],
            [x1, y1, z1],
            cell.rgb,
            SHADE_NS,
        );
    }
    if interior || (cell.west_empty && rcx > 0) {
        push_quad(
            verts,
            [bx, y0, z1],
            [bx, y0, bz],
            [bx, y1, bz],
            [bx, y1, z1],
            cell.rgb,
            SHADE_EW,
        );
    }
    if interior || (cell.east_empty && rcx < 0) {
        push_quad(
            verts,
            [x1, y0, bz],
            [x1, y0, z1],
            [x1, y1, z1],
            [x1, y1, bz],
            cell.rgb,
            SHADE_EW,
        );
    }
}

/// Loads and parses `clouds.png` into a cell grid. A texel is a cloud cell when
/// its alpha is >= 10 (vanilla `isCellEmpty`).
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
            let p = (x as usize + y as usize * width) * 4;
            cells.push(CloudCell {
                present: true,
                rgb: [
                    pixels[p] as f32 / 255.0,
                    pixels[p + 1] as f32 / 255.0,
                    pixels[p + 2] as f32 / 255.0,
                ],
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

fn create_pipeline(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> vk::Pipeline {
    let vert_spv = shader::include_spirv!("clouds.vert.spv");
    let frag_spv = shader::include_spirv!("clouds.frag.spv");
    let vert_mod = shader::create_shader_module(device, vert_spv);
    let frag_mod = shader::create_shader_module(device, frag_spv);

    let stages = [
        vk::PipelineShaderStageCreateInfo {
            stage: vk::ShaderStageFlags::Vertex,
            module: vert_mod,
            name: c"main".as_ptr(),
            ..Default::default()
        },
        vk::PipelineShaderStageCreateInfo {
            stage: vk::ShaderStageFlags::Fragment,
            module: frag_mod,
            name: c"main".as_ptr(),
            ..Default::default()
        },
    ];

    let binding_descs = [vk::VertexInputBindingDescription {
        binding: 0,
        stride: std::mem::size_of::<CloudVertex>() as u32,
        input_rate: vk::VertexInputRate::Vertex,
    }];
    let attr_descs = [
        vk::VertexInputAttributeDescription {
            location: 0,
            binding: 0,
            format: vk::Format::R32G32B32Sfloat,
            offset: 0,
        },
        vk::VertexInputAttributeDescription {
            location: 1,
            binding: 0,
            format: vk::Format::R32G32B32A32Sfloat,
            offset: 12,
        },
    ];
    let vertex_input = vk::PipelineVertexInputStateCreateInfo {
        vertex_binding_description_count: binding_descs.len() as u32,
        vertex_binding_descriptions: binding_descs.as_ptr(),
        vertex_attribute_description_count: attr_descs.len() as u32,
        vertex_attribute_descriptions: attr_descs.as_ptr(),
        ..Default::default()
    };
    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo {
        topology: vk::PrimitiveTopology::TriangleList,
        ..Default::default()
    };
    let viewport_state = vk::PipelineViewportStateCreateInfo {
        viewport_count: 1,
        scissor_count: 1,
        ..Default::default()
    };
    let rasterizer = vk::PipelineRasterizationStateCreateInfo {
        polygon_mode: vk::PolygonMode::Fill,
        cull_mode: vk::CullModeFlags::None,
        front_face: vk::FrontFace::CounterClockwise,
        line_width: 1.0,
        ..Default::default()
    };
    let multisampling = vk::PipelineMultisampleStateCreateInfo {
        rasterization_samples: vk::SampleCountFlags::Type1,
        ..Default::default()
    };
    // Depth-test against the world so terrain occludes clouds. Write depth too:
    // clouds are near-opaque (0.8 alpha) and 3D boxes overlap, so depth writes
    // keep the nearest cloud face and avoid double-blending the layer.
    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo {
        depth_test_enable: vk::TRUE,
        depth_write_enable: vk::TRUE,
        depth_compare_op: vk::CompareOp::Less,
        ..Default::default()
    };
    let blend_attachment = vk::PipelineColorBlendAttachmentState {
        blend_enable: vk::TRUE,
        src_color_blend_factor: vk::BlendFactor::SrcAlpha,
        dst_color_blend_factor: vk::BlendFactor::OneMinusSrcAlpha,
        color_blend_op: vk::BlendOp::Add,
        src_alpha_blend_factor: vk::BlendFactor::One,
        dst_alpha_blend_factor: vk::BlendFactor::OneMinusSrcAlpha,
        alpha_blend_op: vk::BlendOp::Add,
        color_write_mask: vk::ColorComponentFlags::RGBA,
    };
    let color_blending = vk::PipelineColorBlendStateCreateInfo {
        attachment_count: 1,
        attachments: &blend_attachment,
        ..Default::default()
    };
    let dynamic_states = [vk::DynamicState::Viewport, vk::DynamicState::Scissor];
    let dynamic_state = vk::PipelineDynamicStateCreateInfo {
        dynamic_state_count: dynamic_states.len() as u32,
        dynamic_states: dynamic_states.as_ptr(),
        ..Default::default()
    };

    let info = [vk::GraphicsPipelineCreateInfo {
        stage_count: stages.len() as u32,
        stages: stages.as_ptr(),
        vertex_input_state: &vertex_input,
        input_assembly_state: &input_assembly,
        viewport_state: &viewport_state,
        rasterization_state: &rasterizer,
        multisample_state: &multisampling,
        depth_stencil_state: &depth_stencil,
        color_blend_state: &color_blending,
        dynamic_state: &dynamic_state,
        layout,
        render_pass,
        subpass: 0,
        ..Default::default()
    }];

    let mut pipeline = [vk::Pipeline::null()];
    device
        .create_graphics_pipelines(vk::PipelineCache::null(), &info, None, &mut pipeline)
        .expect("failed to create cloud pipeline");

    device.destroy_shader_module(vert_mod, None);
    device.destroy_shader_module(frag_mod, None);

    pipeline[0]
}
