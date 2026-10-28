use std::collections::HashMap;
use std::path::Path;

use azalea_registry::builtin::BlockEntityKind;

use crate::assets::{AssetIndex, resolve_asset_path};
use crate::position::BlockPos;
use crate::renderer::camera::CameraUniform;
use crate::renderer::chunk::block_lighter::LightLevels;
use crate::renderer::entity_model::{BakedEntityModel, EntityVertex, ModelConvention, PartAnim};
use crate::renderer::pipelines::entity_renderer::{
    BlendMode, ENTITY_ALPHA_CUTOFF, ModelInput, WHITE_TINT, create_pipeline, fallback_texture,
};
use crate::renderer::pipelines::lightmap::Lightmap;
use crate::renderer::{block_entity_model, rhi, util};

pub struct BlockEntityRenderInfo {
    pub pos: BlockPos,
    pub kind: BlockEntityKind,
    pub yaw: f32,
    /// Texture-variant index; the model index is `variant % models.len()`, so
    /// chest variants (material-major, [single, left, right] per material)
    /// fold to their type's model and single-model kinds always use model 0.
    pub variant: u32,
    /// Lid openness for chest/shulker, 0.0=closed to 1.0=open. Raw (un-eased);
    /// the pipeline applies a cubic ease at draw time.
    pub lid_open: f32,
    /// Vanilla `BlockEntityRenderState.lightCoords`.
    pub light: LightLevels,
}

struct TextureSlot {
    texture: rhi::backend::Texture,
    set: rhi::backend::BindGroup,
}

struct KindEntry {
    /// Model variants sharing one vertex buffer; their `part_ranges` are
    /// rebased to be buffer-absolute at build time.
    models: Vec<BakedEntityModel>,
    vertex_buffer: rhi::backend::Buffer,
    textures: Vec<TextureSlot>,
}

struct KindDef {
    kind: BlockEntityKind,
    models: Vec<BakedEntityModel>,
    tex_variants: &'static [&'static [&'static str]],
    tex_size: u32,
}

/// 16 dye colors in vanilla `DyeColor` ordinal order. Used both to build
/// texture-variant arrays and to map block names back to variant indices.
const DYE_COLOR_NAMES: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];

/// Wood/material variants for sign textures, in the order they appear in
/// `SIGN_TEXTURES`.
const SIGN_WOOD_NAMES: [&str; 12] = [
    "oak", "spruce", "birch", "jungle", "acacia", "dark_oak", "mangrove", "cherry", "pale_oak",
    "bamboo", "crimson", "warped",
];

const SIGN_TEXTURES: &[&[&str]] = &[
    &["minecraft/textures/block/oak_sign.png"],
    &["minecraft/textures/block/spruce_sign.png"],
    &["minecraft/textures/block/birch_sign.png"],
    &["minecraft/textures/block/jungle_sign.png"],
    &["minecraft/textures/block/acacia_sign.png"],
    &["minecraft/textures/block/dark_oak_sign.png"],
    &["minecraft/textures/block/mangrove_sign.png"],
    &["minecraft/textures/block/cherry_sign.png"],
    &["minecraft/textures/block/pale_oak_sign.png"],
    &["minecraft/textures/block/bamboo_sign.png"],
    &["minecraft/textures/block/crimson_sign.png"],
    &["minecraft/textures/block/warped_sign.png"],
];

/// Chest textures mirroring vanilla `Sheets.chooseSprite`: material-major with
/// [single, double-left, double-right] per material, so
/// `variant = material * 3 + type` and `variant % 3` is the model index.
/// Material order matches [`variant_for_block`]. Christmas only reskins the
/// normal material (vanilla `getChestMaterial` checks copper first), so copper
/// stages keep their look.
macro_rules! chest_textures {
    ($base:literal, copper) => {
        chest_textures!($base, "copper", "copper_exposed", "copper_weathered", "copper_oxidized")
    };
    ($($mat:literal),+ $(,)?) => {
        &[$(
            &[concat!("minecraft/textures/entity/chest/", $mat, ".png")],
            &[concat!("minecraft/textures/entity/chest/", $mat, "_left.png")],
            &[concat!("minecraft/textures/entity/chest/", $mat, "_right.png")],
        )+]
    };
}

const CHEST_TEXTURES: &[&[&str]] = chest_textures!("normal", copper);

const CHEST_XMAS_TEXTURES: &[&[&str]] = chest_textures!("christmas", copper);

const TRAPPED_CHEST_TEXTURES: &[&[&str]] = chest_textures!("trapped");

const ENDER_CHEST_TEXTURES: &[&[&str]] = &[&["minecraft/textures/entity/chest/ender.png"]];

const SHULKER_TEXTURES: &[&[&str]] = &[
    &["minecraft/textures/entity/shulker/shulker_white.png"],
    &["minecraft/textures/entity/shulker/shulker_orange.png"],
    &["minecraft/textures/entity/shulker/shulker_magenta.png"],
    &["minecraft/textures/entity/shulker/shulker_light_blue.png"],
    &["minecraft/textures/entity/shulker/shulker_yellow.png"],
    &["minecraft/textures/entity/shulker/shulker_lime.png"],
    &["minecraft/textures/entity/shulker/shulker_pink.png"],
    &["minecraft/textures/entity/shulker/shulker_gray.png"],
    &["minecraft/textures/entity/shulker/shulker_light_gray.png"],
    &["minecraft/textures/entity/shulker/shulker_cyan.png"],
    &["minecraft/textures/entity/shulker/shulker_purple.png"],
    &["minecraft/textures/entity/shulker/shulker_blue.png"],
    &["minecraft/textures/entity/shulker/shulker_brown.png"],
    &["minecraft/textures/entity/shulker/shulker_green.png"],
    &["minecraft/textures/entity/shulker/shulker_red.png"],
    &["minecraft/textures/entity/shulker/shulker_black.png"],
    &["minecraft/textures/entity/shulker/shulker.png"],
];

fn name_index(table: &[&str], name: &str) -> Option<u32> {
    table.iter().position(|&n| n == name).map(|i| i as u32)
}

/// Build a [`PartAnim`] applying chest/shulker lid motion. `openness` is the
/// raw [0, 1] value; vanilla applies cubic easing so the lid decelerates as it
/// approaches the open or closed extreme.
fn lid_anim(kind: BlockEntityKind, openness: f32) -> PartAnim {
    if openness <= 0.0 {
        return PartAnim::default();
    }
    let inv = 1.0 - openness;
    let eased = 1.0 - inv * inv * inv;
    match kind {
        BlockEntityKind::Chest | BlockEntityKind::TrappedChest | BlockEntityKind::EnderChest => {
            // Parts are [bottom, lid, lock]; lid and lock swing together.
            let rot = glam::Vec3::new(-eased * std::f32::consts::FRAC_PI_2, 0.0, 0.0);
            PartAnim {
                rotation: vec![(1, rot), (2, rot)],
                ..Default::default()
            }
        }
        BlockEntityKind::ShulkerBox => PartAnim {
            rotation: vec![(0, glam::Vec3::new(0.0, eased * 270.0f32.to_radians(), 0.0))],
            translation: vec![(0, glam::Vec3::new(0.0, -eased * 8.0, 0.0))],
        },
        _ => PartAnim::default(),
    }
}

pub fn variant_for_block(
    kind: BlockEntityKind,
    name: &str,
    props: &crate::world::block::PropMap,
) -> u32 {
    match kind {
        // Ender chests have no `type` property and fall through to 0.
        BlockEntityKind::Chest | BlockEntityKind::TrappedChest => {
            let ty = match props.get("type") {
                Some("left") => 1,
                Some("right") => 2,
                _ => 0,
            };
            // Copper weathering stage selects the material row (waxing keeps
            // the stage's texture); trapped chests have no copper form.
            let material = match name.strip_prefix("waxed_").unwrap_or(name) {
                "copper_chest" => 1,
                "exposed_copper_chest" => 2,
                "weathered_copper_chest" => 3,
                "oxidized_copper_chest" => 4,
                _ => 0,
            };
            material * 3 + ty
        }
        BlockEntityKind::ShulkerBox => name
            .strip_suffix("_shulker_box")
            .and_then(|s| name_index(&DYE_COLOR_NAMES, s))
            .unwrap_or(16),
        BlockEntityKind::Sign => name
            .strip_suffix("_sign")
            .or_else(|| name.strip_suffix("_wall_sign"))
            .and_then(|s| name_index(&SIGN_WOOD_NAMES, s))
            .unwrap_or(0),
        _ => 0,
    }
}

/// XZ offset from a double-chest half to its partner. `type=left` connects at
/// `facing.getClockWise()`, `type=right` at `getCounterClockWise()` (vanilla
/// `ChestBlock.getConnectedDirection`).
pub fn chest_partner_offset(facing: &str, chest_type: &str) -> Option<(i32, i32)> {
    let clockwise = match facing {
        "north" => (1, 0),
        "east" => (0, 1),
        "south" => (-1, 0),
        "west" => (0, -1),
        _ => return None,
    };
    match chest_type {
        "left" => Some(clockwise),
        "right" => Some((-clockwise.0, -clockwise.1)),
        _ => None,
    }
}

/// Values mirror vanilla's `direction.toYRot()`; the draw code applies
/// `rotY(180 - yaw)` for y-down models and `rotY(-yaw)` for y-up ones.
pub fn yaw_for_block(kind: BlockEntityKind, props: &crate::world::block::PropMap) -> f32 {
    match kind {
        BlockEntityKind::Chest
        | BlockEntityKind::TrappedChest
        | BlockEntityKind::EnderChest
        | BlockEntityKind::ShulkerBox => match props.get("facing") {
            Some("south") => 0.0,
            Some("west") => 90.0,
            Some("north") => 180.0,
            Some("east") => 270.0,
            _ => 0.0,
        },
        // TODO: wall signs have no `rotation` (they use `facing`) and vanilla
        // renders them with a postless model offset against the wall; they
        // currently fall back to the standing model facing south.
        BlockEntityKind::Sign => props
            .get("rotation")
            .and_then(|s| s.parse::<f32>().ok())
            .map(|r| r * 22.5)
            .unwrap_or(0.0),
        _ => 0.0,
    }
}

/// Vanilla swaps chest textures for the christmas set on Dec 24-26 (local
/// date), decided once at renderer construction. The check runs before the
/// trapped-chest one there, so trapped chests turn christmas too; ender and
/// copper chests never do.
fn is_christmas() -> bool {
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    now.month() == time::Month::December && (24..=26).contains(&now.day())
}

fn kind_definitions() -> Vec<KindDef> {
    let xmas = is_christmas();
    let chest_models = block_entity_model::bake_chest_models();
    // Ender chests have no double form; only the single model applies.
    let ender_models = vec![chest_models[0].clone()];
    vec![
        KindDef {
            kind: BlockEntityKind::Chest,
            models: chest_models.clone(),
            tex_variants: if xmas {
                CHEST_XMAS_TEXTURES
            } else {
                CHEST_TEXTURES
            },
            tex_size: 64,
        },
        KindDef {
            kind: BlockEntityKind::TrappedChest,
            models: chest_models,
            tex_variants: if xmas {
                CHEST_XMAS_TEXTURES
            } else {
                TRAPPED_CHEST_TEXTURES
            },
            tex_size: 64,
        },
        KindDef {
            kind: BlockEntityKind::EnderChest,
            models: ender_models,
            tex_variants: ENDER_CHEST_TEXTURES,
            tex_size: 64,
        },
        KindDef {
            kind: BlockEntityKind::ShulkerBox,
            models: vec![block_entity_model::bake_shulker_box_model()],
            tex_variants: SHULKER_TEXTURES,
            tex_size: 64,
        },
        KindDef {
            kind: BlockEntityKind::Sign,
            models: vec![block_entity_model::bake_sign_model()],
            tex_variants: SIGN_TEXTURES,
            tex_size: 32,
        },
    ]
}

pub struct BlockEntityPipeline {
    pipeline: rhi::backend::RenderPipeline,
    pipeline_layout: rhi::backend::PipelineLayout,
    camera_layout: rhi::backend::BindGroupLayout,
    texture_layout: rhi::backend::BindGroupLayout,
    camera_sets: Vec<rhi::backend::BindGroup>,
    camera_buffers: Vec<rhi::backend::Buffer>,
    /// The shared [`Lightmap`]'s group, set 2.
    lightmap_set: rhi::backend::BindGroup,
    /// A shader pack's `gbuffers_block`, drawn with while set; the texture
    /// binds at set 3.
    pub pack_binding: Option<super::shaderpack::PackBinding>,
    texture_sampler: rhi::backend::Sampler,
    entries: HashMap<BlockEntityKind, KindEntry>,
}

impl BlockEntityPipeline {
    pub fn new(
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
        jar_assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
        lightmap: &Lightmap,
    ) -> Self {
        let camera_layout = rhi::backend::create_bind_group_layout(device, util::VERTEX_UNIFORM);
        let texture_layout = rhi::backend::create_bind_group_layout(device, util::FRAGMENT_TEXTURE);
        let pipeline_layout = rhi::backend::create_pipeline_layout(
            device,
            &[camera_layout, texture_layout, lightmap.layout],
            Some(rhi::PushConstants {
                visibility: rhi::ShaderStages::VERTEX,
                size: 112,
            }),
        );

        let pipeline = create_pipeline(
            device,
            targets,
            pipeline_layout,
            BlendMode::Opaque,
            ModelInput::PushConstant,
        );

        let defs = kind_definitions();

        let (camera_sets, camera_buffers) = util::create_uniform_sets(
            device,
            camera_layout,
            size_of::<CameraUniform>() as u64,
            "block_entity_camera_uniform",
        );

        let texture_sampler = rhi::backend::create_sampler(device, &rhi::SamplerDesc::NEAREST);

        let mut entries = HashMap::new();
        let mut pending_uploads: Vec<util::PendingImageUpload> = Vec::new();
        let mut staging_to_free: Vec<rhi::backend::Buffer> = Vec::new();
        for def in defs {
            let entry = build_entry(
                device,
                texture_layout,
                texture_sampler,
                jar_assets_dir,
                asset_index,
                def.models,
                def.tex_variants,
                def.tex_size,
                &mut pending_uploads,
                &mut staging_to_free,
            );
            entries.insert(def.kind, entry);
        }

        util::upload_images_batched(device, &pending_uploads);

        util::destroy_buffers(device, &mut staging_to_free);

        Self {
            pipeline,
            pipeline_layout,
            camera_layout,
            texture_layout,
            camera_sets,
            camera_buffers,
            lightmap_set: lightmap.group,
            pack_binding: None,
            texture_sampler,
            entries,
        }
    }

    pub fn update_camera(&mut self, frame: usize, uniform: &CameraUniform) {
        let bytes = bytemuck::bytes_of(uniform);
        self.camera_buffers[frame].write(bytes);
    }

    pub fn draw(
        &self,
        pass: &mut rhi::backend::RenderPassEncoder,
        frame: usize,
        anchor: glam::DVec3,
        items: &[BlockEntityRenderInfo],
    ) {
        if items.is_empty() {
            return;
        }

        let pipeline = self
            .pack_binding
            .map_or(self.pipeline, |pack| pack.pipeline);
        pass.set_pipeline(&pipeline);

        let mut bound_entry: *const KindEntry = std::ptr::null();
        let mut bound_set = rhi::backend::BindGroup::default();

        for info in items {
            let Some(entry) = self.entries.get(&info.kind) else {
                continue;
            };
            let variant_idx = (info.variant as usize).min(entry.textures.len().saturating_sub(1));
            let slot = &entry.textures[variant_idx];

            let entry_ptr: *const KindEntry = entry;
            if bound_entry != entry_ptr {
                pass.set_vertex_buffer(0, entry.vertex_buffer.handle());
                bound_entry = entry_ptr;
                bound_set = rhi::backend::BindGroup::default();
            }
            if bound_set != slot.set {
                match &self.pack_binding {
                    Some(pack) => {
                        pass.set_bind_groups(0, &pack.sets);
                        pass.set_bind_group(3, slot.set);
                    }
                    None => pass.set_bind_groups(
                        0,
                        &[self.camera_sets[frame], slot.set, self.lightmap_set],
                    ),
                }
                bound_set = slot.set;
            }

            let model = &entry.models[info.variant as usize % entry.models.len()];

            let block_center = (glam::DVec3::new(
                info.pos.x as f64 + 0.5,
                info.pos.y as f64,
                info.pos.z as f64 + 0.5,
            ) - anchor)
                .as_vec3();
            let model_mat = match model.convention {
                // With the 180 yaw offset and the convention's baked-in flip
                // this reproduces vanilla's block-entity `scale(1,-1,-1)`.
                ModelConvention::EntityYDown => {
                    glam::Mat4::from_translation(block_center)
                        * glam::Mat4::from_rotation_y((180.0f32 - info.yaw).to_radians())
                }
                // Vanilla `ChestRenderer`: rotate by -facing.toYRot() about the
                // block center; coords are relative to the block's min corner.
                ModelConvention::BlockYUp => {
                    glam::Mat4::from_translation(block_center)
                        * glam::Mat4::from_rotation_y((-info.yaw).to_radians())
                        * glam::Mat4::from_translation(glam::Vec3::new(-0.5, 0.0, -0.5))
                }
            };

            let anim = lid_anim(info.kind, info.lid_open);
            let part_transforms = model.compute_part_transforms(&anim);
            for (i, (start, count)) in model.part_ranges.iter().enumerate() {
                if *count == 0 {
                    continue;
                }
                let part_mat = model_mat * part_transforms[i];
                let cols = part_mat.to_cols_array();
                // Shared entity shader push block: mat, tint, overlay_color, uv_params.
                // Block entities are opaque with no hurt flash or UV scroll.
                let no_overlay = [0.0f32, 0.0, 0.0, 1.0];
                let uv_params = [0.0f32, 0.0, ENTITY_ALPHA_CUTOFF, info.light.lane()];
                let mut bytes = [0u8; 112];
                bytes[..64].copy_from_slice(bytemuck::cast_slice(&cols));
                bytes[64..80].copy_from_slice(bytemuck::cast_slice(&WHITE_TINT));
                bytes[80..96].copy_from_slice(bytemuck::cast_slice(&no_overlay));
                bytes[96..112].copy_from_slice(bytemuck::cast_slice(&uv_params));
                pass.set_push_constants(rhi::ShaderStages::VERTEX, 0, &bytes);
                pass.draw(*count, 1, *start, 0);
            }
        }
    }

    pub fn recreate_pipeline(
        &mut self,
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
    ) {
        self.pipeline.destroy(device);
        self.pipeline = create_pipeline(
            device,
            targets,
            self.pipeline_layout,
            BlendMode::Opaque,
            ModelInput::PushConstant,
        );
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        util::destroy_buffers(device, &mut self.camera_buffers);
        self.texture_sampler.destroy(device);
        for entry in self.entries.values_mut() {
            std::mem::take(&mut entry.vertex_buffer).destroy(device);
            for slot in entry.textures.iter_mut() {
                std::mem::take(&mut slot.texture).destroy(device);
                slot.set.destroy(device);
            }
        }

        self.pipeline.destroy(device);
        self.pipeline_layout.destroy(device);
        util::destroy_bind_groups(device, self.camera_sets.drain(..));
        self.camera_layout.destroy(device);
        self.texture_layout.destroy(device);
    }
}

#[allow(clippy::too_many_arguments)]
fn build_entry(
    device: &rhi::backend::Device,
    texture_layout: rhi::backend::BindGroupLayout,
    texture_sampler: rhi::backend::Sampler,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    mut models: Vec<BakedEntityModel>,
    tex_variants: &[&[&str]],
    fallback_tex_size: u32,
    pending_uploads: &mut Vec<util::PendingImageUpload>,
    staging_to_free: &mut Vec<rhi::backend::Buffer>,
) -> KindEntry {
    let mut all_vertices: Vec<EntityVertex> = Vec::new();
    for model in &mut models {
        let base = all_vertices.len() as u32;
        all_vertices.append(&mut model.vertices);
        for range in &mut model.part_ranges {
            range.0 += base;
        }
    }
    let vertex_buffer = rhi::backend::Buffer::with_data(
        device,
        "block_entity_vertices",
        bytemuck::cast_slice(&all_vertices),
        rhi::BufferUsages::VERTEX,
    );

    let textures = tex_variants
        .iter()
        .map(|keys| {
            build_texture_slot(
                device,
                texture_layout,
                texture_sampler,
                jar_assets_dir,
                asset_index,
                keys,
                fallback_tex_size,
                pending_uploads,
                staging_to_free,
            )
        })
        .collect();

    KindEntry {
        models,
        vertex_buffer,
        textures,
    }
}

#[allow(clippy::too_many_arguments)]
fn build_texture_slot(
    device: &rhi::backend::Device,
    texture_layout: rhi::backend::BindGroupLayout,
    texture_sampler: rhi::backend::Sampler,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    keys: &[&str],
    fallback_tex_size: u32,
    pending_uploads: &mut Vec<util::PendingImageUpload>,
    staging_to_free: &mut Vec<rhi::backend::Buffer>,
) -> TextureSlot {
    let (pixels, width, height) = keys
        .iter()
        .find_map(|key| {
            let path = resolve_asset_path(jar_assets_dir, asset_index, key);
            util::load_png(&path)
        })
        .unwrap_or_else(|| {
            tracing::warn!("Failed to load BE texture {:?}, using fallback", keys);
            fallback_texture(fallback_tex_size)
        });

    let texture = rhi::backend::Texture::new(
        device,
        &util::sampled_texture_desc("block_entity_texture", width, height),
    );
    let staging = rhi::backend::Buffer::with_data(
        device,
        "block_entity_texture_staging",
        &pixels,
        rhi::BufferUsages::COPY_SRC,
    );
    pending_uploads.push(util::PendingImageUpload {
        staging_buffer: staging.handle(),
        staging_size: pixels.len() as u64,
        image: texture.handle(),
        width,
        height,
        mip_levels: 1,
    });
    staging_to_free.push(staging);

    let set = util::create_texture_group(device, texture_layout, texture.handle(), texture_sampler);

    TextureSlot { texture, set }
}
