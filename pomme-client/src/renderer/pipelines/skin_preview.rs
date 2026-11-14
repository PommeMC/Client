use std::f32::consts::{FRAC_PI_2, PI};

use glam::camera::rh::{proj, view};
use glam::{Mat3, Mat4, Vec2, Vec3};

use crate::renderer::entity_model::{
    HumanoidArmPose, HumanoidArmorSlot, ModelCube, cube_face_positions, cube_face_uvs,
    humanoid_armor_parts,
};
use crate::renderer::pipelines::entity_renderer::{
    ARMOR_ALPHA_CUTOFF, ARMOR_SUBMIT_ORDER, EntityRenderer, WHITE_TINT,
};
use crate::renderer::{PlayerPreview, rhi, shader, util};

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Vertex {
    pub(crate) position: [f32; 3],
    pub(crate) uv: [f32; 2],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Uniform {
    mvp: [[f32; 4]; 4],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct PreviewPush {
    tint: [f32; 4],
    alpha_cutoff: f32,
}

pub(crate) const PREVIEW_ALPHA_CUTOFF: f32 = 0.5;

pub(crate) fn preview_push_constant_range() -> rhi::PushConstants {
    rhi::PushConstants {
        visibility: rhi::ShaderStages::FRAGMENT,
        size: std::mem::size_of::<PreviewPush>() as u32,
    }
}

pub(crate) fn push_preview_style(
    pass: &rhi::backend::RenderPassEncoder,
    tint: [f32; 4],
    alpha_cutoff: f32,
) {
    let push = PreviewPush { tint, alpha_cutoff };
    pass.set_push_constants(rhi::ShaderStages::FRAGMENT, 0, bytemuck::bytes_of(&push));
}

/// `(first, count)` vertex ranges drawn with the body, head, right arm and
/// left arm MVPs.
type ArmorRanges = [(u32, u32); 4];

/// Where the preview's held items draw, from `draw_in_box`.
pub struct PreviewHands {
    pub view_proj: Mat4,
    /// The preview's space to the GUI view space vanilla lights in.
    pub normal_basis: Mat3,
    /// Each arm's (right, left) transform up to the item's display.
    pub hands: [Mat4; 2],
}

/// The arms' pivots in vanilla model space, right then left.
const ARM_PIVOTS: [Vec3; 2] = [Vec3::new(-5.0, 2.0, 0.0), Vec3::new(5.0, 2.0, 0.0)];

struct PreviewArmorMesh {
    buffer: rhi::backend::Buffer,
    ranges: ArmorRanges,
}

pub struct SkinPreviewPipeline {
    pipeline: rhi::backend::RenderPipeline,
    armor_pipeline: rhi::backend::RenderPipeline,
    pipeline_layout: rhi::backend::PipelineLayout,
    mvp_layout: rhi::backend::BindGroupLayout,
    tex_layout: rhi::backend::BindGroupLayout,
    mvp_sets: Vec<rhi::backend::BindGroup>,
    head_mvp_sets: Vec<rhi::backend::BindGroup>,
    tex_set: rhi::backend::BindGroup,
    mvp_buffers: Vec<rhi::backend::Buffer>,
    head_mvp_buffers: Vec<rhi::backend::Buffer>,
    body_buffer: rhi::backend::Buffer,
    body_count: u32,
    head_buffer: rhi::backend::Buffer,
    head_count: u32,
    /// Right then left.
    arm_buffers: [rhi::backend::Buffer; 2],
    arm_count: u32,
    arm_mvp_sets: [Vec<rhi::backend::BindGroup>; 2],
    arm_mvp_buffers: [Vec<rhi::backend::Buffer>; 2],
    armor_meshes: [PreviewArmorMesh; 4],
    swing_start: Option<std::time::Instant>,
    slim: bool,
}

impl SkinPreviewPipeline {
    pub fn new(
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
        skin_texture: rhi::backend::TextureHandle,
        skin_sampler: rhi::backend::Sampler,
        slim: bool,
    ) -> Self {
        let mvp_layout = rhi::backend::create_bind_group_layout(device, util::VERTEX_UNIFORM);
        let tex_layout = rhi::backend::create_bind_group_layout(device, util::FRAGMENT_TEXTURE);
        let pipeline_layout = rhi::backend::create_pipeline_layout(
            device,
            &[mvp_layout, tex_layout],
            Some(preview_push_constant_range()),
        );

        let pipeline = create_pipeline(device, targets, pipeline_layout);
        let armor_pipeline = create_armor_pipeline(device, targets, pipeline_layout);

        let uniform_size = std::mem::size_of::<Uniform>() as u64;
        let (mvp_sets, mvp_buffers) =
            util::create_uniform_sets(device, mvp_layout, uniform_size, "skin_body_mvp");
        let (head_mvp_sets, head_mvp_buffers) =
            util::create_uniform_sets(device, mvp_layout, uniform_size, "skin_head_mvp");
        let (right_arm_sets, right_arm_buffers) =
            util::create_uniform_sets(device, mvp_layout, uniform_size, "skin_right_arm_mvp");
        let (left_arm_sets, left_arm_buffers) =
            util::create_uniform_sets(device, mvp_layout, uniform_size, "skin_left_arm_mvp");
        let tex_set = util::create_texture_group(device, tex_layout, skin_texture, skin_sampler);

        let body_verts = build_body_mesh();
        let body_buffer = rhi::backend::Buffer::with_data(
            device,
            "skin_body",
            bytemuck::cast_slice(&body_verts),
            rhi::BufferUsages::VERTEX,
        );

        let arm_verts = [build_arm_mesh(slim, true), build_arm_mesh(slim, false)];
        let arm_buffers = arm_verts.each_ref().map(|verts| {
            rhi::backend::Buffer::with_data(
                device,
                "skin_arm",
                bytemuck::cast_slice(verts),
                rhi::BufferUsages::VERTEX,
            )
        });

        let head_verts = build_head_mesh();
        let head_buffer = rhi::backend::Buffer::with_data(
            device,
            "skin_head",
            bytemuck::cast_slice(&head_verts),
            rhi::BufferUsages::VERTEX,
        );

        let armor_meshes = std::array::from_fn(|i| {
            let (vertices, ranges) = build_armor_preview_mesh(HumanoidArmorSlot::ALL[i]);
            let buffer = rhi::backend::Buffer::with_data(
                device,
                "skin_preview_armor",
                bytemuck::cast_slice(&vertices),
                rhi::BufferUsages::VERTEX,
            );
            PreviewArmorMesh { buffer, ranges }
        });

        Self {
            pipeline,
            armor_pipeline,
            pipeline_layout,
            mvp_layout,
            tex_layout,
            mvp_sets,
            head_mvp_sets,
            tex_set,
            mvp_buffers,
            head_mvp_buffers,
            arm_buffers,
            arm_count: arm_verts[0].len() as u32,
            arm_mvp_sets: [right_arm_sets, left_arm_sets],
            arm_mvp_buffers: [right_arm_buffers, left_arm_buffers],
            armor_meshes,
            swing_start: None,
            slim,
            body_buffer,
            body_count: body_verts.len() as u32,
            head_buffer,
            head_count: head_verts.len() as u32,
        }
    }

    pub fn trigger_swing(&mut self) {
        self.swing_start = Some(std::time::Instant::now());
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        pass: &mut rhi::backend::RenderPassEncoder,
        frame: usize,
        aspect: f32,
        screen_x: f32,
        screen_y: f32,
        scale: f32,
        mouse_px_x: f32,
        mouse_px_y: f32,
        screen_w: f32,
        screen_h: f32,
    ) {
        let center_px_x = screen_x * screen_w;
        let center_px_y = screen_y * screen_h;
        let body_y_rot_raw = ((mouse_px_x - center_px_x) / 40.0).atan();
        let head_x_rot_raw = ((mouse_px_y - center_px_y) / 40.0).atan();

        let head_y_rot_deg = body_y_rot_raw * 40.0;
        let body_y_rot_deg = head_y_rot_deg * 0.3;
        let body_rot_rad = PI + body_y_rot_deg.to_radians();
        let head_y_rot_rad = head_y_rot_deg.to_radians();
        let head_x_rot_rad = head_x_rot_raw * 20.0f32.to_radians();

        let mut proj = proj::directx::perspective(MENU_FOV, aspect, 0.1, 100.0);
        proj.y_axis.y *= -1.0;

        let ndc_x = screen_x * 2.0 - 1.0;
        let ndc_y = screen_y * 2.0 - 1.0;
        let clip_offset = Mat4::from_translation(Vec3::new(ndc_x, ndc_y, 0.0));
        let size = Mat4::from_scale(Vec3::new(scale, scale, 1.0));

        let vp = clip_offset * size * proj * camera_view();
        let arm_rots = self.swung([Vec3::ZERO; 2]);
        self.record(
            pass,
            frame,
            vp,
            body_rot_rad,
            body_rot_rad + head_y_rot_rad,
            head_x_rot_rad,
            arm_rots,
        );
    }

    /// Draws the player in a GUI box like vanilla's inventory preview:
    /// orthographic, 30 GUI px per model unit, mouse-follow rotation, the
    /// arms in `arm_poses` (right, left). Returns where its held items go.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_in_box(
        &mut self,
        pass: &mut rhi::backend::RenderPassEncoder,
        frame: usize,
        p: &PlayerPreview,
        sw: f32,
        sh: f32,
        entity_renderer: &EntityRenderer,
        arm_poses: [HumanoidArmPose; 2],
    ) -> PreviewHands {
        let cx = p.rect[0] + p.rect[2] / 2.0;
        let cy = p.rect[1] + p.rect[3] / 2.0;
        let xa = ((p.cursor.0 - cx) / (40.0 * p.gui_scale)).atan();
        let ya = ((p.cursor.1 - cy) / (40.0 * p.gui_scale)).atan();
        let body_rot_rad = PI + (xa * 20.0).to_radians();
        let head_yaw_rad = PI + (xa * 40.0).to_radians();
        let head_pitch_rad = (ya * 20.0).to_radians();

        let units_to_px = p.model_scale * p.gui_scale;
        let half_w = sw / (2.0 * units_to_px);
        let half_h = sh / (2.0 * units_to_px);
        let mut proj = proj::directx::orthographic(-half_w, half_w, -half_h, half_h, 0.1, 100.0);
        proj.y_axis.y *= -1.0;

        let clip_offset =
            Mat4::from_translation(Vec3::new(cx / sw * 2.0 - 1.0, cy / sh * 2.0 - 1.0, 0.0));
        let tilt = Mat4::from_rotation_x(-head_pitch_rad);
        // Vanilla lifts the entity by bbHeight/2 + 0.0625 = 0.9625 from its
        // feet; mesh feet are at y = -1.5, so +0.5375 centers it in the box.
        let center = Mat4::from_translation(Vec3::new(0.0, 0.5375, 0.0));
        let view_proj = clip_offset * proj * camera_view() * tilt * center;

        // Vanilla's head angles: the half turn about X between its model
        // space and this mesh's keeps the pitch and flips the yaw.
        let head = Vec2::new(head_pitch_rad, body_rot_rad - head_yaw_rad);
        let arm_rots = self.swung([0, 1].map(|i| arm_poses[i].pose(Vec3::ZERO, i == 0, head)));
        self.record(
            pass,
            frame,
            view_proj,
            body_rot_rad,
            head_yaw_rad,
            head_pitch_rad,
            arm_rots,
        );
        self.draw_armor(pass, frame, p, entity_renderer);

        let body_rot_mat = Mat4::from_rotation_y(body_rot_rad);
        let hands = [0, 1].map(|i| {
            let right = i == 0;
            let side = if right { 1.0 } else { -1.0 };
            // Vanilla `PlayerModel.translateToHand`'s slim-arm nudge.
            let slim_nudge = if self.slim { side * 0.5 } else { 0.0 };
            let pivot = ARM_PIVOTS[i] + Vec3::new(slim_nudge, 0.0, 0.0);
            // Vanilla `ItemInHandLayer.submitArmWithItem`.
            body_rot_mat
                * arm_part(pivot, arm_rots[i])
                * Mat4::from_rotation_x(-FRAC_PI_2)
                * Mat4::from_rotation_y(PI)
                * Mat4::from_translation(Vec3::new(side, 2.0, -10.0) / 16.0)
        });
        PreviewHands {
            view_proj,
            normal_basis: gui_normal_basis(body_rot_rad, head_pitch_rad),
            hands,
        }
    }

    /// `arm_rots` with the right arm's click swing added.
    fn swung(&mut self, mut arm_rots: [Vec3; 2]) -> [Vec3; 2] {
        if let Some(start) = self.swing_start {
            let t = start.elapsed().as_secs_f32();
            if t > 0.4 {
                self.swing_start = None;
            } else {
                arm_rots[0].x += (t / 0.4 * PI).sin() * -1.5;
            }
        }
        arm_rots
    }

    fn draw_armor(
        &self,
        pass: &mut rhi::backend::RenderPassEncoder,
        frame: usize,
        p: &PlayerPreview,
        entity_renderer: &EntityRenderer,
    ) {
        pass.set_pipeline(&self.armor_pipeline);
        let mvp_sets = [
            self.mvp_sets[frame],
            self.head_mvp_sets[frame],
            self.arm_mvp_sets[0][frame],
            self.arm_mvp_sets[1][frame],
        ];
        for slot in ARMOR_SUBMIT_ORDER {
            let Some(armor) = &p.armor[slot] else {
                continue;
            };
            let mesh = &self.armor_meshes[slot];
            for (texture_set, tint) in entity_renderer.armor_layers(armor, slot) {
                push_preview_style(pass, tint, ARMOR_ALPHA_CUTOFF);
                pass.set_vertex_buffer(0, mesh.buffer.handle());
                for (mvp_set, (first, count)) in mvp_sets.into_iter().zip(mesh.ranges) {
                    if count == 0 {
                        continue;
                    }
                    // Entity texture sets are layout-compatible with set 1 here.
                    pass.set_bind_groups(0, &[mvp_set, texture_set]);
                    pass.draw(count, 1, first, 0);
                }
            }
        }
    }

    /// `arm_rots` are the arms' (right, left) vanilla `(xRot, yRot, zRot)`.
    #[allow(clippy::too_many_arguments)]
    fn record(
        &mut self,
        pass: &mut rhi::backend::RenderPassEncoder,
        frame: usize,
        vp: Mat4,
        body_rot_rad: f32,
        head_yaw_rad: f32,
        head_pitch_rad: f32,
        arm_rots: [Vec3; 2],
    ) {
        let body_rot_mat = Mat4::from_rotation_y(body_rot_rad);
        let body_mvp = vp * body_rot_mat;
        let head_rot_mat =
            Mat4::from_rotation_y(head_yaw_rad) * Mat4::from_rotation_x(head_pitch_rad);
        let head_mvp = vp * head_rot_mat;

        write_uniform(&mut self.mvp_buffers[frame], &body_mvp);
        write_uniform(&mut self.head_mvp_buffers[frame], &head_mvp);
        for (i, &pivot) in ARM_PIVOTS.iter().enumerate() {
            let arm = arm_mesh_transform(pivot, arm_rots[i]);
            write_uniform(&mut self.arm_mvp_buffers[i][frame], &(body_mvp * arm));
        }

        pass.set_pipeline(&self.pipeline);
        push_preview_style(pass, WHITE_TINT, PREVIEW_ALPHA_CUTOFF);
        pass.set_bind_group(1, self.tex_set);
        for (mvp_set, buffer, count) in [
            (self.mvp_sets[frame], &self.body_buffer, self.body_count),
            (
                self.head_mvp_sets[frame],
                &self.head_buffer,
                self.head_count,
            ),
        ]
        .into_iter()
        .chain((0..2).map(|i| {
            (
                self.arm_mvp_sets[i][frame],
                &self.arm_buffers[i],
                self.arm_count,
            )
        })) {
            pass.set_bind_group(0, mvp_set);
            pass.set_vertex_buffer(0, buffer.handle());
            pass.draw(count, 1, 0, 0);
        }
    }

    pub fn recreate_pipeline(
        &mut self,
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
    ) {
        self.pipeline.destroy(device);
        self.armor_pipeline.destroy(device);
        self.pipeline = create_pipeline(device, targets, self.pipeline_layout);
        self.armor_pipeline = create_armor_pipeline(device, targets, self.pipeline_layout);
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        util::destroy_buffers(device, &mut self.mvp_buffers);
        util::destroy_buffers(device, &mut self.head_mvp_buffers);
        for buffers in &mut self.arm_mvp_buffers {
            util::destroy_buffers(device, buffers);
        }

        let destroy = |buffer: &mut rhi::backend::Buffer| {
            std::mem::take(buffer).destroy(device);
        };
        destroy(&mut self.body_buffer);
        destroy(&mut self.head_buffer);
        for buffer in &mut self.arm_buffers {
            destroy(buffer);
        }
        for mesh in &mut self.armor_meshes {
            destroy(&mut mesh.buffer);
        }

        self.pipeline.destroy(device);
        self.armor_pipeline.destroy(device);
        self.pipeline_layout.destroy(device);
        util::destroy_bind_groups(
            device,
            self.mvp_sets
                .drain(..)
                .chain(self.head_mvp_sets.drain(..))
                .chain(self.arm_mvp_sets.iter_mut().flat_map(|sets| sets.drain(..))),
        );
        self.tex_set.destroy(device);
        self.mvp_layout.destroy(device);
        self.tex_layout.destroy(device);
    }
}

const CAMERA_DISTANCE: f32 = 4.5;
const MENU_FOV: f32 = 0.6;
/// The player mesh runs from its feet at y = -1.5 to the top of its head at
/// 0.5, so `draw`'s anchor sits a quarter of its height below the head.
const MODEL_HEIGHT: f32 = 2.0;
pub const MODEL_ANCHOR_BELOW_HEAD: f32 = 0.25;

/// The model's height as a fraction of the screen's at `draw`'s scale 1.
pub fn menu_model_height() -> f32 {
    MODEL_HEIGHT / (2.0 * CAMERA_DISTANCE * (MENU_FOV / 2.0).tan())
}

fn camera_view() -> Mat4 {
    view::look_at_mat4(Vec3::new(0.0, 0.0, -CAMERA_DISTANCE), Vec3::ZERO, Vec3::Y)
}

/// Vanilla model space (y down, the face on -Z, 16 units a block) to the
/// preview mesh's (y up, the face on +Z; see `PX`): a half turn about X.
const MODEL_TO_PREVIEW: Mat4 = Mat4::from_cols(
    glam::Vec4::X,
    glam::Vec4::NEG_Y,
    glam::Vec4::NEG_Z,
    glam::Vec4::W,
);

/// Vanilla `ModelPart.translateAndRotate` for an arm at `pivot` turned by
/// `rot` (`(xRot, yRot, zRot)`), from vanilla model space into the preview's.
fn arm_part(pivot: Vec3, rot: Vec3) -> Mat4 {
    MODEL_TO_PREVIEW
        * Mat4::from_translation(pivot / 16.0)
        * Mat4::from_rotation_z(rot.z)
        * Mat4::from_rotation_y(rot.y)
        * Mat4::from_rotation_x(rot.x)
}

/// `arm_part` for the arm mesh, which is built already at its pivot in the
/// preview's space, so the pivot is unwound first.
fn arm_mesh_transform(pivot: Vec3, rot: Vec3) -> Mat4 {
    arm_part(pivot, rot) * Mat4::from_translation(-pivot / 16.0) * MODEL_TO_PREVIEW
}

/// The preview's space (after the body turn) to the GUI view space vanilla's
/// `ENTITY_IN_UI` lights are in: `GuiEntityRenderer`'s pose (the box's
/// `(s, s, -s)` scale and its Z half turn and pitch) over
/// `LivingEntityRenderer`'s body turn and `(-1, -1, 1)` flip, undoing the
/// preview's own turn.
fn gui_normal_basis(body_rot_rad: f32, pitch_rad: f32) -> Mat3 {
    let vanilla_body = body_rot_rad - PI;
    let view = Mat3::from_diagonal(Vec3::new(1.0, 1.0, -1.0))
        * Mat3::from_rotation_z(PI)
        * Mat3::from_rotation_x(-pitch_rad)
        * Mat3::from_rotation_y(vanilla_body)
        * Mat3::from_diagonal(Vec3::new(-1.0, -1.0, 1.0));
    view * Mat3::from_mat4(MODEL_TO_PREVIEW) * Mat3::from_rotation_y(body_rot_rad).transpose()
}

pub(crate) fn write_uniform(buffer: &mut rhi::backend::Buffer, mvp: &Mat4) {
    let uniform = Uniform {
        mvp: mvp.to_cols_array_2d(),
    };
    let bytes = bytemuck::bytes_of(&uniform);
    buffer.write(bytes);
}

#[derive(Clone, Copy)]
struct PreviewPipelineState {
    cull_mode: rhi::CullMode,
    blend: Option<rhi::BlendState>,
    depth_compare: rhi::CompareFunction,
}

const TEXTURED_PREVIEW_STATE: PreviewPipelineState = PreviewPipelineState {
    cull_mode: rhi::CullMode::Front,
    blend: Some(rhi::BlendState::TRANSLUCENT),
    depth_compare: rhi::CompareFunction::Less,
};
// Vanilla `ARMOR_CUTOUT_NO_CULL`; LEQUAL lets stacked layers (leather, then
// leather_overlay) draw over each other.
const ARMOR_PREVIEW_STATE: PreviewPipelineState = PreviewPipelineState {
    cull_mode: rhi::CullMode::None,
    blend: None,
    depth_compare: rhi::CompareFunction::LessEqual,
};
// Vanilla `ENTITY_SOLID` for the enchanting book; LEQUAL keeps the seam from
// winning over the lids it touches. Its mesh keeps vanilla's winding, unlike
// the Z-reflected skin mesh, so it culls the opposite set.
const BOOK_PREVIEW_STATE: PreviewPipelineState = PreviewPipelineState {
    cull_mode: rhi::CullMode::Back,
    blend: None,
    depth_compare: rhi::CompareFunction::LessEqual,
};

/// The textured-model preview pipeline. Vanilla's GL-CCW front faces come
/// out clockwise in Vulkan's y-down framebuffer coords, so this culls the
/// CCW set.
fn create_pipeline(
    device: &rhi::backend::Device,
    targets: rhi::backend::PipelineTargets,
    layout: rhi::backend::PipelineLayout,
) -> rhi::backend::RenderPipeline {
    create_pipeline_with_state(device, targets, layout, TEXTURED_PREVIEW_STATE)
}

pub(crate) fn create_book_pipeline(
    device: &rhi::backend::Device,
    targets: rhi::backend::PipelineTargets,
    layout: rhi::backend::PipelineLayout,
) -> rhi::backend::RenderPipeline {
    create_pipeline_with_state(device, targets, layout, BOOK_PREVIEW_STATE)
}

fn create_armor_pipeline(
    device: &rhi::backend::Device,
    targets: rhi::backend::PipelineTargets,
    layout: rhi::backend::PipelineLayout,
) -> rhi::backend::RenderPipeline {
    create_pipeline_with_state(device, targets, layout, ARMOR_PREVIEW_STATE)
}

fn create_pipeline_with_state(
    device: &rhi::backend::Device,
    targets: rhi::backend::PipelineTargets,
    layout: rhi::backend::PipelineLayout,
    state: PreviewPipelineState,
) -> rhi::backend::RenderPipeline {
    rhi::backend::create_render_pipeline(
        device,
        targets,
        layout,
        &rhi::RenderPipelineDesc {
            cull_mode: state.cull_mode,
            depth: Some(rhi::DepthState {
                compare: state.depth_compare,
                ..rhi::DepthState::WRITE
            }),
            blend: state.blend,
            ..rhi::RenderPipelineDesc::new(
                "preview",
                shader::include_shader!("preview.vert"),
                shader::include_shader!("preview.frag"),
                &[rhi::vertex_buffer!(Vertex, [Float32x3, Float32x2])],
            )
        },
    )
}

// Vanilla model coordinates: Y-down, 1 unit = 1 pixel; converted to Y-up by
// negating Y, then scaled by PX. Unlike the entity renderer (X flip + face
// rect on -Z), this mesh is built Z-reflected — face rect on +Z, viewed from
// -Z — which yields the identical image; don't "fix" one convention without
// the other.
const PX: f32 = 1.0 / 16.0;

fn uv(x: u32, y: u32, w: u32, h: u32) -> [[f32; 2]; 4] {
    let s = 64.0;
    [
        [x as f32 / s, y as f32 / s],
        [(x + w) as f32 / s, y as f32 / s],
        [(x + w) as f32 / s, (y + h) as f32 / s],
        [x as f32 / s, (y + h) as f32 / s],
    ]
}

fn quad(verts: &mut Vec<Vertex>, pos: [[f32; 3]; 4], uvs: [[f32; 2]; 4]) {
    for &i in &[0u32, 1, 2, 2, 3, 0] {
        verts.push(Vertex {
            position: pos[i as usize],
            uv: uvs[i as usize],
        });
    }
}

fn add_model_cube(verts: &mut Vec<Vertex>, part_offset: Vec3, cube: &ModelCube) {
    let mut translated = *cube;
    translated.origin += part_offset;
    let positions = cube_face_positions(&translated, true);
    let face_uvs = cube_face_uvs(&translated);

    for (positions, uv) in positions.iter().zip(face_uvs) {
        let reflect = |p: [f32; 3]| [p[0], p[1], -p[2]];
        let mut corners = [
            (reflect(positions[0]), [uv[2] / 64.0, uv[1] / 32.0]),
            (reflect(positions[1]), [uv[0] / 64.0, uv[1] / 32.0]),
            (reflect(positions[2]), [uv[0] / 64.0, uv[3] / 32.0]),
            (reflect(positions[3]), [uv[2] / 64.0, uv[3] / 32.0]),
        ];
        // `cube_face_positions` + this corner order already reproduce vanilla
        // mirror semantics. Z reflection reverses winding once more; cancel it
        // here so the preview pipeline's front-face culling matches the skin.
        if !cube.mirror {
            corners.reverse();
        }
        for &i in &[0usize, 1, 2, 0, 2, 3] {
            verts.push(Vertex {
                position: corners[i].0,
                uv: corners[i].1,
            });
        }
    }
}

/// Splits the slot's armor cubes by the preview transform they ride: body
/// (also the legs), head, right arm, left arm.
fn build_armor_preview_mesh(slot: HumanoidArmorSlot) -> (Vec<Vertex>, ArmorRanges) {
    let mut groups: [Vec<Vertex>; 4] = Default::default();
    for part in humanoid_armor_parts(slot) {
        let group = match part.name.as_str() {
            "head" => 1,
            "right_arm" => 2,
            "left_arm" => 3,
            _ => 0,
        };
        for cube in &part.cubes {
            add_model_cube(&mut groups[group], part.offset, cube);
        }
    }
    let mut ranges = ArmorRanges::default();
    let mut vertices = Vec::new();
    for (range, group) in ranges.iter_mut().zip(groups) {
        *range = (vertices.len() as u32, group.len() as u32);
        vertices.extend(group);
    }
    (vertices, ranges)
}

// addBox(ox, oy, oz, w, h, d) at pivot (px, py, pz) with UV origin (tx, ty)
// Vanilla Y-down, we flip to Y-up
#[allow(clippy::too_many_arguments)]
fn add_box(
    verts: &mut Vec<Vertex>,
    px: f32,
    py: f32,
    _pz: f32,
    ox: f32,
    oy: f32,
    oz: f32,
    w: f32,
    h: f32,
    d: f32,
    tx: u32,
    ty: u32,
    tw: u32,
    th: u32,
    td: u32,
) {
    let x0 = (px + ox) * PX;
    let x1 = (px + ox + w) * PX;
    // Flip Y: vanilla Y-down, we want Y-up
    let y0 = -(py + oy + h) * PX;
    let y1 = -(py + oy) * PX;
    let z0 = (_pz + oz) * PX;
    let z1 = (_pz + oz + d) * PX;

    // Front (+Z)
    quad(
        verts,
        [[x0, y1, z1], [x1, y1, z1], [x1, y0, z1], [x0, y0, z1]],
        uv(tx + td, ty + td, tw, th),
    );
    // Back (-Z)
    quad(
        verts,
        [[x1, y1, z0], [x0, y1, z0], [x0, y0, z0], [x1, y0, z0]],
        uv(tx + td + tw + td, ty + td, tw, th),
    );
    // Right (+X)
    quad(
        verts,
        [[x1, y1, z1], [x1, y1, z0], [x1, y0, z0], [x1, y0, z1]],
        uv(tx + td + tw, ty + td, td, th),
    );
    // Left (-X)
    quad(
        verts,
        [[x0, y1, z0], [x0, y1, z1], [x0, y0, z1], [x0, y0, z0]],
        uv(tx, ty + td, td, th),
    );
    // Top (+Y in our space = -Y in vanilla = top of head)
    quad(
        verts,
        [[x0, y1, z0], [x1, y1, z0], [x1, y1, z1], [x0, y1, z1]],
        uv(tx + td, ty, tw, td),
    );
    // Bottom (-Y in our space = +Y in vanilla = bottom). Vanilla reads the
    // maxY face's rect with V reversed (`ModelPart.Cube`'s UP polygon).
    let mut bottom_uv = uv(tx + td + tw, ty, tw, td);
    bottom_uv.reverse();
    quad(
        verts,
        [[x0, y0, z1], [x1, y0, z1], [x1, y0, z0], [x0, y0, z0]],
        bottom_uv,
    );
}

fn build_head_mesh() -> Vec<Vertex> {
    let mut v = Vec::new();
    let e = 0.5; // hat overlay inflation (1 pixel bigger in vanilla)
    // Head: addBox(-4, -8, -4, 8, 8, 8) @ (0, 0, 0), UV (0, 0)
    add_box(
        &mut v, 0.0, 0.0, 0.0, -4.0, -8.0, -4.0, 8.0, 8.0, 8.0, 0, 0, 8, 8, 8,
    );
    // Hat overlay: same box inflated by 0.5, UV (32, 0)
    add_box(
        &mut v,
        0.0,
        0.0,
        0.0,
        -4.0 - e,
        -8.0 - e,
        -4.0 - e,
        8.0 + e * 2.0,
        8.0 + e * 2.0,
        8.0 + e * 2.0,
        32,
        0,
        8,
        8,
        8,
    );
    v
}

fn build_body_mesh() -> Vec<Vertex> {
    let mut v = Vec::new();
    let e = 0.25; // overlay inflation for body/limbs

    // Body: addBox(-4, 0, -2, 8, 12, 4) @ (0, 0, 0), UV (16, 16)
    add_box(
        &mut v, 0.0, 0.0, 0.0, -4.0, 0.0, -2.0, 8.0, 12.0, 4.0, 16, 16, 8, 12, 4,
    );
    // Jacket overlay: UV (16, 32)
    add_box(
        &mut v,
        0.0,
        0.0,
        0.0,
        -4.0 - e,
        0.0 - e,
        -2.0 - e,
        8.0 + e * 2.0,
        12.0 + e * 2.0,
        4.0 + e * 2.0,
        16,
        32,
        8,
        12,
        4,
    );

    // Right leg: addBox(-2, 0, -2, 4, 12, 4) @ (-1.9, 12, 0), UV (0, 16)
    add_box(
        &mut v, -1.9, 12.0, 0.0, -2.0, 0.0, -2.0, 4.0, 12.0, 4.0, 0, 16, 4, 12, 4,
    );
    // Right pants overlay: UV (0, 32)
    add_box(
        &mut v,
        -1.9,
        12.0,
        0.0,
        -2.0 - e,
        0.0 - e,
        -2.0 - e,
        4.0 + e * 2.0,
        12.0 + e * 2.0,
        4.0 + e * 2.0,
        0,
        32,
        4,
        12,
        4,
    );

    // Left leg: addBox(-2, 0, -2, 4, 12, 4) @ (1.9, 12, 0), UV (16, 48)
    add_box(
        &mut v, 1.9, 12.0, 0.0, -2.0, 0.0, -2.0, 4.0, 12.0, 4.0, 16, 48, 4, 12, 4,
    );
    // Left pants overlay: UV (0, 48)
    add_box(
        &mut v,
        1.9,
        12.0,
        0.0,
        -2.0 - e,
        0.0 - e,
        -2.0 - e,
        4.0 + e * 2.0,
        12.0 + e * 2.0,
        4.0 + e * 2.0,
        0,
        48,
        4,
        12,
        4,
    );
    v
}

/// Right arm: addBox(-3|-2, -2, -2, 4|3, 12, 4) @ (-5, 2, 0), UV (40, 16),
/// sleeve (40, 32). Left arm: addBox(-1, -2, -2, 4|3, 12, 4) @ (5, 2, 0),
/// UV (32, 48), sleeve (48, 48).
fn build_arm_mesh(slim: bool, right: bool) -> Vec<Vertex> {
    let mut v = Vec::new();
    let e = 0.25;
    let arm_tw: u32 = if slim { 3 } else { 4 };
    let arm_w = arm_tw as f32;
    let (px, ox, skin, sleeve) = if right {
        (-5.0, if slim { -2.0 } else { -3.0 }, (40, 16), (40, 32))
    } else {
        (5.0, -1.0, (32, 48), (48, 48))
    };
    add_box(
        &mut v, px, 2.0, 0.0, ox, -2.0, -2.0, arm_w, 12.0, 4.0, skin.0, skin.1, arm_tw, 12, 4,
    );
    add_box(
        &mut v,
        px,
        2.0,
        0.0,
        ox - e,
        -2.0 - e,
        -2.0 - e,
        arm_w + e * 2.0,
        12.0 + e * 2.0,
        4.0 + e * 2.0,
        sleeve.0,
        sleeve.1,
        arm_tw,
        12,
        4,
    );
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_cube_bridge_matches_existing_reflected_preview_box() {
        let cube = ModelCube {
            origin: Vec3::new(-4.0, 0.0, -2.0),
            size: Vec3::new(8.0, 12.0, 4.0),
            tex_offset: (16, 16),
            deformation: 0.0,
            mirror: false,
        };
        let mut actual = Vec::new();
        add_model_cube(&mut actual, Vec3::ZERO, &cube);

        let mut expected = Vec::new();
        add_box(
            &mut expected,
            0.0,
            0.0,
            0.0,
            -4.0,
            0.0,
            -2.0,
            8.0,
            12.0,
            4.0,
            16,
            16,
            8,
            12,
            4,
        );
        // `add_box` targets a 64x64 skin while equipment textures are 64x32.
        for vertex in &mut expected {
            vertex.uv[1] *= 2.0;
        }

        assert_eq!(actual.len(), expected.len());
        for vertex in &actual {
            assert_eq!(
                actual
                    .iter()
                    .filter(|candidate| *candidate == vertex)
                    .count(),
                expected
                    .iter()
                    .filter(|candidate| *candidate == vertex)
                    .count(),
                "preview bridge changed vertex/UV mapping for {vertex:?}"
            );
        }
    }

    #[test]
    fn armor_preview_pipeline_matches_cutout_no_cull_state() {
        assert!(matches!(ARMOR_PREVIEW_STATE.cull_mode, rhi::CullMode::None));
        assert!(ARMOR_PREVIEW_STATE.blend.is_none());
        assert!(matches!(
            ARMOR_PREVIEW_STATE.depth_compare,
            rhi::CompareFunction::LessEqual
        ));
        assert!(matches!(
            TEXTURED_PREVIEW_STATE.cull_mode,
            rhi::CullMode::Front
        ));
        assert!(TEXTURED_PREVIEW_STATE.blend.is_some());
    }

    #[test]
    fn armor_preview_meshes_split_slot_parts_by_preview_transform() {
        let drawn = |slot| build_armor_preview_mesh(slot).1.map(|(_, count)| count > 0);
        assert_eq!(drawn(HumanoidArmorSlot::Head), [false, true, false, false]);
        assert_eq!(drawn(HumanoidArmorSlot::Chest), [true, false, true, true]);
        assert_eq!(drawn(HumanoidArmorSlot::Legs), [true, false, false, false]);
        assert_eq!(drawn(HumanoidArmorSlot::Feet), [true, false, false, false]);
    }

    #[test]
    fn a_still_arm_draws_where_its_mesh_was_built() {
        for pivot in ARM_PIVOTS {
            assert!(arm_mesh_transform(pivot, Vec3::ZERO).abs_diff_eq(Mat4::IDENTITY, 1e-6));
        }
    }

    #[test]
    fn the_gui_light_sees_the_front_facing_the_viewer_and_up_as_up() {
        let basis = gui_normal_basis(PI, 0.0);
        // The preview's camera sits on -Z; GUI view space has +Z toward the
        // viewer and +Y down.
        assert!((basis * Vec3::NEG_Z).abs_diff_eq(Vec3::Z, 1e-6));
        assert!((basis * Vec3::Y).abs_diff_eq(Vec3::NEG_Y, 1e-6));
    }
}
