use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use azalea_inventory::ItemStack;
use azalea_inventory::components::{DataComponentTrait, DyedColor, EquipmentSlot, Equippable};
use azalea_registry::builtin::EntityKind;
use serde::Deserialize;

use crate::assets::{AssetIndex, resolve_asset_path, resolve_asset_path_with_packs};
use crate::entity::components::Position;
use crate::entity::dimensions;
use crate::renderer::camera::CameraUniform;
use crate::renderer::chunk::block_lighter::LightLevels;
use crate::renderer::entity_model::{BakedEntityModel, EntityVertex};
use crate::renderer::pipelines::lightmap::Lightmap;
use crate::renderer::rhi::{self, VertexFormat};
use crate::renderer::{entity_model, shader, util};
use crate::resource_pack::ResourcePackManager;
use crate::world::block::BedDirection;

pub const MAX_OVERLAYS: usize = 4;

fn flip_degrees(kind: EntityKind) -> f32 {
    match kind {
        EntityKind::Spider
        | EntityKind::CaveSpider
        | EntityKind::Endermite
        | EntityKind::Silverfish => 180.0,
        _ => 90.0,
    }
}

fn death_fall_degrees(death_time: f32, kind: EntityKind) -> f32 {
    if death_time <= 0.0 || matches!(kind, EntityKind::Squid | EntityKind::GlowSquid) {
        return 0.0;
    }
    (((death_time - 1.0) / 20.0 * 1.6).sqrt()).min(1.0) * flip_degrees(kind)
}

/// Per-frame instance buffer capacity, in (entity, part) draws. Far above any
/// realistic on-screen entity count; excess is dropped with a warning.
const MAX_INSTANCES: usize = 16384;
const MAX_PLAYER_SKINS: usize = 128;

/// Per-instance data for one (entity, part) draw, fed as instance-rate vertex
/// attributes (binding 1) — the four model-matrix columns, tint, overlay, and
/// uv scroll, alpha cutoff and light lane.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct EntityInstance {
    model: [[f32; 4]; 4],
    tint: [f32; 4],
    overlay_color: [f32; 4],
    uv_params: [f32; 4],
}

#[derive(Clone, Debug)]
pub struct ArmorRenderInfo {
    pub asset_id: Arc<str>,
    pub dyed_color: Option<u32>,
}

fn effective_item_component<'a, T: DataComponentTrait>(item: &'a ItemStack) -> Option<Cow<'a, T>> {
    // Azalea's `get_component` currently falls back to the item default when
    // the patch entry is an explicit removal (`kind -> None`). Vanilla's
    // PatchedDataComponentMap keeps that state distinct from an absent patch.
    if item
        .component_patch()
        .iter()
        .any(|(kind, value)| kind == T::KIND && value.is_none())
    {
        return None;
    }
    item.get_component::<T>()
}

pub fn armor_render_info(item: Option<&ItemStack>, slot: EquipmentSlot) -> Option<ArmorRenderInfo> {
    let item = item?;
    let equippable = effective_item_component::<Equippable>(item)?;
    if equippable.slot != slot {
        return None;
    }
    let asset_id = equippable.asset_id.as_ref()?.to_string().into();
    let dyed_color =
        effective_item_component::<DyedColor>(item).map(|color| color.rgb as u32 & 0x00ff_ffff);
    Some(ArmorRenderInfo {
        asset_id,
        dyed_color,
    })
}

/// Armor slots in `EntityRenderInfo::armor` order.
pub const ARMOR_SLOTS: [EquipmentSlot; 4] = [
    EquipmentSlot::Head,
    EquipmentSlot::Chest,
    EquipmentSlot::Legs,
    EquipmentSlot::Feet,
];
const LEGS_SLOT: usize = 2;
/// Vanilla `HumanoidArmorLayer.submit` order: chest, legs, feet, head.
pub(crate) const ARMOR_SUBMIT_ORDER: [usize; 4] = [1, LEGS_SLOT, 3, 0];

pub fn armor_render_infos(items: &[ItemStack]) -> [Option<ArmorRenderInfo>; 4] {
    std::array::from_fn(|i| armor_render_info(items.get(i), ARMOR_SLOTS[i]))
}

pub struct EntityRenderInfo {
    pub position: Position,
    pub head_x_rot_deg: f32,
    pub head_y_rot_deg: f32,
    pub body_y_rot_deg: f32,
    pub is_sleeping: bool,
    /// The bed's facing; `None` when no bed can be resolved.
    pub sleeping_direction: Option<BedDirection>,
    /// Standing-pose eye height used by LivingEntityRenderer's sleeping offset.
    pub sleeping_eye_height: f32,
    pub is_baby: bool,
    pub is_crouching: bool,
    pub walk_anim_pos: f32,
    pub walk_anim_speed: f32,
    pub entity_kind: EntityKind,
    pub player_uuid: Option<uuid::Uuid>,
    /// Indexed like [`ARMOR_SLOTS`].
    pub armor: [Option<ArmorRenderInfo>; 4],
    pub variant_index: u32,
    pub overlay_tints: [Option<[f32; 4]>; MAX_OVERLAYS],
    /// Per-slot overlay texture variant (villager type/profession/level).
    pub overlay_variants: [u32; MAX_OVERLAYS],
    /// Villager head-shake (unhappy counter > 0).
    pub is_unhappy: bool,
    pub head_y_offset: f32,
    pub head_x_rot_deg_override: Option<f32>,
    pub has_red_overlay: bool,
    pub death_time: f32,
    /// Mob is targeting/attacking — raises zombie/skeleton arms.
    pub aggressive: bool,
    /// Chicken wing-flap phase and 0..1 amplitude, interpolated.
    pub flap: f32,
    pub flap_speed: f32,
    /// Enderman screaming state — raises the head.
    pub is_creepy: bool,
    /// Zombie-family conversion — shakes the whole body.
    pub is_converting: bool,
    /// Witch main-hand occupancy; swings the nose down toward the held item.
    pub is_holding_item: bool,
    /// Witch per-entity nose-wobble rate, resolved from the entity id.
    pub nose_wobble_speed: f32,
    /// Tamable sitting pose (wolf/cat).
    pub is_sitting: bool,
    pub is_sprinting: bool,
    /// Wolf anger — angry face texture is picked upstream; this pins the tail.
    pub is_angry: bool,
    /// Wolf tail pitch (vanilla `getTailAngle`), radians.
    pub tail_angle: f32,
    /// Wolf beg head tilt, radians, interpolated.
    pub head_roll_angle: f32,
    /// Wolf wet-shake progress 0..2, interpolated.
    pub shake_anim: f32,
    /// Cat lie-down / relax springs, interpolated.
    pub lie_down_amount: f32,
    pub lie_down_amount_tail: f32,
    pub relax_state_one_amount: f32,
    /// Rabbit hop keyframe clock, seconds since the hop started.
    pub hop_elapsed_secs: Option<f32>,
    /// Equine grass-eat / rear-up / feeding springs, interpolated.
    pub eat_anim: f32,
    pub stand_anim: f32,
    pub feeding_anim: f32,
    /// Equine tail swish (client-local RNG counter).
    pub animate_tail: bool,
    /// Fish flop pose / squid body branch.
    pub is_in_water: bool,
    /// Squid tentacle stroke angle, interpolated.
    pub tentacle_angle: f32,
    /// Bat pose flag + its fly/rest animation clock.
    pub bat_resting: bool,
    pub bat_elapsed_secs: Option<f32>,
    /// Iron golem countdowns; the punch one is partial-tick adjusted.
    pub golem_attack_ticks: f32,
    pub golem_offer_flower_ticks: u32,
    /// Base-model tint (wolf wet-shade grayscale); white for everyone else.
    pub base_tint: [f32; 4],
    /// Extra scale applied after the entity rotation (slime size + squish),
    /// shared by base and overlay draws.
    pub body_transform: Option<glam::Mat4>,
    /// Interpolated entity age in ticks; drives the undead idle arm bob.
    pub age_in_ticks: f32,
    /// Arm-swing progress 0..1; drives the zombie attack swing.
    pub attack_time: f32,
    /// Skip frustum/distance culling (the 3rd-person self entity, which sits at
    /// the camera and must never blink out).
    pub skip_cull: bool,
    pub shadow: super::entity_shadow::ShadowCaster,
    /// The glowing outline's colour (vanilla `EntityRenderState.outlineColor`),
    /// when the entity draws one.
    pub outline_color: Option<u32>,
    pub visibility: Visibility,
    /// Vanilla `EntityRenderState.lightCoords`.
    pub light: LightLevels,
}

/// Everything inert: mob-family animation inputs zeroed, no overlays, white
/// tint. Construction sites spell out only the fields that apply to them.
impl Default for EntityRenderInfo {
    fn default() -> Self {
        Self {
            position: Position::new(0.0, 0.0, 0.0),
            head_x_rot_deg: 0.0,
            head_y_rot_deg: 0.0,
            body_y_rot_deg: 0.0,
            is_sleeping: false,
            sleeping_direction: None,
            sleeping_eye_height: crate::player::STANDING_EYE_HEIGHT,
            is_baby: false,
            is_crouching: false,
            walk_anim_pos: 0.0,
            walk_anim_speed: 0.0,
            entity_kind: EntityKind::Player,
            player_uuid: None,
            armor: Default::default(),
            variant_index: 0,
            overlay_tints: [None; MAX_OVERLAYS],
            overlay_variants: [0; MAX_OVERLAYS],
            is_unhappy: false,
            head_y_offset: 0.0,
            head_x_rot_deg_override: None,
            has_red_overlay: false,
            death_time: 0.0,
            aggressive: false,
            flap: 0.0,
            flap_speed: 0.0,
            is_creepy: false,
            is_converting: false,
            is_holding_item: false,
            nose_wobble_speed: 0.0,
            is_sitting: false,
            is_sprinting: false,
            is_angry: false,
            tail_angle: 0.0,
            head_roll_angle: 0.0,
            shake_anim: 0.0,
            lie_down_amount: 0.0,
            lie_down_amount_tail: 0.0,
            relax_state_one_amount: 0.0,
            hop_elapsed_secs: None,
            eat_anim: 0.0,
            stand_anim: 0.0,
            feeding_anim: 0.0,
            animate_tail: false,
            is_in_water: false,
            tentacle_angle: 0.0,
            bat_resting: false,
            bat_elapsed_secs: None,
            golem_attack_ticks: 0.0,
            golem_offer_flower_ticks: 0,
            base_tint: WHITE_TINT,
            body_transform: None,
            age_in_ticks: 0.0,
            attack_time: 0.0,
            skip_cull: false,
            outline_color: None,
            visibility: Visibility::Visible,
            shadow: Default::default(),
            light: LightLevels::FULL_BRIGHT,
        }
    }
}

/// Vanilla `isInvisible` with `isInvisibleToPlayer`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Visibility {
    #[default]
    Visible,
    /// Invisible, but the viewer sees through it: a spectator, or a teammate
    /// whose team sees friendly invisibles.
    Seen,
    /// Invisible to the viewer.
    Hidden,
}

/// Vanilla's `0x26` alpha on a `Seen` mob's body.
const SEEN_ALPHA: f32 = 38.0 / 255.0;

/// How an overlay layer is blended. Base/baby variants are always `Opaque`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OverlayKind {
    /// Cutout, depth-writing — sheep wool and all base models.
    Opaque,
    /// `Opaque` with backface culling (vanilla `entityCutoutCull`) — meshes
    /// with coplanar zero-depth quads (bat wings).
    OpaqueCulled,
    /// Translucent, depth-writing — the slime shell (vanilla
    /// `entityTranslucent`; the alpha lives in the texture).
    BodyTranslucent,
    /// Translucent, full-bright, depth-write off — spider glowing eyes.
    EyesTranslucent,
    /// Additive, full-bright, depth-writing, scrolling UV — charged creeper
    /// swirl.
    SwirlAdditive,
}

struct MobVariant {
    model: BakedEntityModel,
    vertex_buffer: rhi::backend::Buffer,
    texture: rhi::backend::Texture,
    texture_set: rhi::backend::BindGroup,
    overlay_kind: OverlayKind,
    /// A layer that leaves its outline while its mob is invisible and glows
    /// (vanilla `SheepWoolLayer`, `SlimeOuterLayer`).
    invisible_outline: bool,
    /// Overlay whose part poses (pivots/rotations/scales) differ from the
    /// base model's, so its part transforms can't be shared with the base
    /// (stray/bogged clothing: humanoid ±1.9 legs over skeleton ±2.0).
    own_pivots: bool,
}

struct MobEntry {
    adult_variants: Vec<MobVariant>,
    baby_variants: Option<Vec<MobVariant>>,
    /// Overlay slots, each with its own texture variants
    /// (`overlay_variants[slot]` picks one).
    adult_overlays: Vec<Vec<MobVariant>>,
    baby_overlays: Vec<Vec<MobVariant>>,
    anim: AnimationType,
}

struct PlayerSkinTexture {
    texture: rhi::backend::Texture,
    set: rhi::backend::BindGroup,
    slim: bool,
}

#[derive(Deserialize)]
struct EquipmentClientInfoJson {
    #[serde(default)]
    layers: HashMap<String, Vec<EquipmentLayerJson>>,
}

#[derive(Deserialize)]
struct EquipmentLayerJson {
    texture: String,
    #[serde(default)]
    dyeable: Option<EquipmentDyeableJson>,
}

#[derive(Deserialize)]
struct EquipmentDyeableJson {
    #[serde(default)]
    color_when_undyed: Option<i32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArmorDye {
    None,
    Dyeable { color_when_undyed: Option<u32> },
}

struct ArmorLayerDef {
    texture_key: String,
    dye: ArmorDye,
}

#[derive(Default)]
struct ArmorAssetDef {
    humanoid: Vec<ArmorLayerDef>,
    leggings: Vec<ArmorLayerDef>,
}

struct ArmorTextureLayer {
    texture: rhi::backend::Texture,
    set: rhi::backend::BindGroup,
    dye: ArmorDye,
}

impl ArmorAsset {
    fn layers_for(&self, slot: usize) -> &[ArmorTextureLayer] {
        if slot == LEGS_SLOT {
            &self.leggings
        } else {
            &self.humanoid
        }
    }
}

fn armor_layer_tint(layer: &ArmorTextureLayer, armor: &ArmorRenderInfo) -> Option<[f32; 4]> {
    match layer.dye {
        ArmorDye::None => Some([1.0, 1.0, 1.0, 1.0]),
        ArmorDye::Dyeable { color_when_undyed } => {
            Some(rgb_tint(armor.dyed_color.or(color_when_undyed)?))
        }
    }
}

/// An opaque `0xRRGGBB` as an instance tint.
fn rgb_tint(rgb: u32) -> [f32; 4] {
    [
        ((rgb >> 16) & 0xff) as f32 / 255.0,
        ((rgb >> 8) & 0xff) as f32 / 255.0,
        (rgb & 0xff) as f32 / 255.0,
        1.0,
    ]
}

#[derive(Default)]
struct ArmorAsset {
    humanoid: Vec<ArmorTextureLayer>,
    leggings: Vec<ArmorTextureLayer>,
}

struct ArmorMesh {
    model: BakedEntityModel,
    vertex_buffer: rhi::backend::Buffer,
}

struct ArmorMeshes {
    humanoid: [ArmorMesh; 4],
    husk: [ArmorMesh; 4],
    zombie_villager: [ArmorMesh; 4],
}

impl MobEntry {
    fn base_variant(&self, is_baby: bool, variant_index: u32) -> &MobVariant {
        let pool = if is_baby {
            self.baby_variants.as_ref().unwrap_or(&self.adult_variants)
        } else {
            &self.adult_variants
        };
        let idx = (variant_index as usize).min(pool.len().saturating_sub(1));
        &pool[idx]
    }

    fn overlays(&self, is_baby: bool) -> &[Vec<MobVariant>] {
        if is_baby {
            &self.baby_overlays
        } else {
            &self.adult_overlays
        }
    }

    fn overlay_variant(&self, is_baby: bool, slot: usize, variant_index: u32) -> &MobVariant {
        let pool = &self.overlays(is_baby)[slot];
        let idx = (variant_index as usize).min(pool.len().saturating_sub(1));
        &pool[idx]
    }
}

pub const WHITE_TINT: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// Each mob's flattened variant pool by registry path; the net handler
/// resolves synced registry entries by name against these same slices (so
/// their order is pomme's, not the protocol id's), and the renderer
/// constructor asserts the pools line up.
pub const CHICKEN_VARIANT_ORDER: &[&str] = &["temperate", "warm", "cold"];
pub const COW_VARIANT_ORDER: &[&str] = &["temperate", "cold", "warm"];
/// Wolf pool interleaves 3 state textures (wild/tame/angry) per variant.
pub const WOLF_VARIANT_ORDER: &[&str] = &[
    "pale", "spotted", "snowy", "black", "ashen", "rusty", "woods", "chestnut", "striped",
];
pub const CAT_VARIANT_ORDER: &[&str] = &[
    "all_black",
    "black",
    "british_shorthair",
    "calico",
    "jellie",
    "persian",
    "ragdoll",
    "red",
    "siamese",
    "tabby",
    "white",
];

/// Pool length the `*_VARIANT_ORDER` slice implies for mobs whose variant
/// index comes from a synced registry.
fn expected_variant_count(kind: EntityKind) -> Option<usize> {
    match kind {
        EntityKind::Chicken => Some(CHICKEN_VARIANT_ORDER.len()),
        EntityKind::Cow => Some(COW_VARIANT_ORDER.len()),
        EntityKind::Wolf => Some(WOLF_VARIANT_ORDER.len() * 3),
        EntityKind::Cat => Some(CAT_VARIANT_ORDER.len()),
        _ => None,
    }
}

/// Vanilla `OverlayTexture` hurt pixel (ARGB 0xB2FF0000): rgb is the overlay
/// color, `a` is how much of the base color survives the mix.
const HURT_OVERLAY: [f32; 4] = [1.0, 0.0, 0.0, 178.0 / 255.0];
const NO_OVERLAY: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
pub(super) const ENTITY_ALPHA_CUTOFF: f32 = 0.5;
pub(crate) const ARMOR_ALPHA_CUTOFF: f32 = 0.1;
/// The light lane `light_lane_color` leaves unlit.
const EMISSIVE_LANE: f32 = -1.0;

pub const WOOL_COLOR_RGBA: [[f32; 4]; 16] = [
    rgb(0xF0F0F0), // 0 white
    rgb(0xEB8844), // 1 orange
    rgb(0xC354CD), // 2 magenta
    rgb(0x6689D3), // 3 light_blue
    rgb(0xDECF2A), // 4 yellow
    rgb(0x41CD34), // 5 lime
    rgb(0xD88198), // 6 pink
    rgb(0x434343), // 7 gray
    rgb(0xABABAB), // 8 light_gray
    rgb(0x287697), // 9 cyan
    rgb(0x7B2FBE), // 10 purple
    rgb(0x253192), // 11 blue
    rgb(0x51301A), // 12 brown
    rgb(0x3B511A), // 13 green
    rgb(0xB3312C), // 14 red
    rgb(0x1E1B1B), // 15 black
];

const fn rgb(hex: u32) -> [f32; 4] {
    let r = ((hex >> 16) & 0xFF) as f32 / 255.0;
    let g = ((hex >> 8) & 0xFF) as f32 / 255.0;
    let b = (hex & 0xFF) as f32 / 255.0;
    [r, g, b, 1.0]
}

pub fn wool_color_tint(color: u8) -> [f32; 4] {
    WOOL_COLOR_RGBA[(color & 0x0F) as usize]
}

/// Vanilla `DyeColor.getTextureDiffuseColor` — the modern dye table used by
/// collar layers (`WOOL_COLOR_RGBA` above is the legacy wool table).
pub const DYE_COLOR_RGBA: [[f32; 4]; 16] = [
    rgb(0xF9FFFE), // 0 white
    rgb(0xF9801D), // 1 orange
    rgb(0xC74EBD), // 2 magenta
    rgb(0x3AB3DA), // 3 light_blue
    rgb(0xFED83D), // 4 yellow
    rgb(0x80C71F), // 5 lime
    rgb(0xF38BAA), // 6 pink
    rgb(0x474F52), // 7 gray
    rgb(0x9D9D97), // 8 light_gray
    rgb(0x169C9C), // 9 cyan
    rgb(0x8932B8), // 10 purple
    rgb(0x3C44AA), // 11 blue
    rgb(0x835432), // 12 brown
    rgb(0x5E7C16), // 13 green
    rgb(0xB02E26), // 14 red
    rgb(0x1D1D21), // 15 black
];

/// Out-of-range ids are white (vanilla `DyeColor.byId`).
pub fn dye_color_tint(color: u8) -> [f32; 4] {
    DYE_COLOR_RGBA
        .get(color as usize)
        .copied()
        .unwrap_or(DYE_COLOR_RGBA[0])
}

pub fn jeb_sheep_tint(entity_id: i32, age_in_ticks: u32) -> [f32; 4] {
    let base = (age_in_ticks / 25).wrapping_add(entity_id as u32);
    let c1 = (base % 16) as usize;
    let c2 = ((base + 1) % 16) as usize;
    let t = (age_in_ticks % 25) as f32 / 25.0;
    let a = WOOL_COLOR_RGBA[c1];
    let b = WOOL_COLOR_RGBA[c2];
    [
        a[0] * (1.0 - t) + b[0] * t,
        a[1] * (1.0 - t) + b[1] * t,
        a[2] * (1.0 - t) + b[2] * t,
        1.0,
    ]
}

fn equipment_texture_key(texture: &str, layer_type: &str) -> Option<String> {
    let (namespace, path) = texture.split_once(':').unwrap_or(("minecraft", texture));
    let key = format!("{namespace}/textures/entity/equipment/{layer_type}/{path}.png");
    if namespace.contains('/') || !crate::assets::valid_asset_key(&key) {
        tracing::warn!("Ignoring invalid equipment texture id {texture:?}");
        return None;
    }
    Some(key)
}

fn collect_equipment_json_dir(
    dir: &Path,
    base: &Path,
    namespace: &str,
    out: &mut HashMap<String, PathBuf>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_equipment_json_dir(&path, base, namespace, out);
            continue;
        }
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let Ok(relative) = path.strip_prefix(base) else {
            continue;
        };
        let mut resource_path = relative
            .with_extension("")
            .to_string_lossy()
            .replace('\\', "/");
        while resource_path.starts_with('/') {
            resource_path.remove(0);
        }
        if !resource_path.is_empty() {
            out.insert(format!("{namespace}:{resource_path}"), path);
        }
    }
}

fn collect_equipment_json_root(root: &Path, out: &mut HashMap<String, PathBuf>) {
    let Ok(namespaces) = std::fs::read_dir(root) else {
        return;
    };
    for namespace in namespaces.flatten() {
        let ns_path = namespace.path();
        if !ns_path.is_dir() {
            continue;
        }
        let ns = namespace.file_name().to_string_lossy().to_string();
        let equipment = ns_path.join("equipment");
        if equipment.is_dir() {
            collect_equipment_json_dir(&equipment, &equipment, &ns, out);
        }
    }
}

fn discover_equipment_json(
    jar_assets_dir: &Path,
    packs: Option<&ResourcePackManager>,
) -> HashMap<String, PathBuf> {
    let mut resources = HashMap::new();
    collect_equipment_json_root(jar_assets_dir, &mut resources);
    // Packs come low-to-high priority, so later inserts override.
    if let Some(packs) = packs {
        for pack in packs.active_pack_dirs() {
            collect_equipment_json_root(&pack.join("assets"), &mut resources);
        }
    }
    resources
}

fn read_equipment_json(path: &Path) -> Option<EquipmentClientInfoJson> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes)
        .map_err(|error| {
            tracing::warn!(
                "Failed to parse equipment definition {}: {error}",
                path.display()
            );
            error
        })
        .ok()
}

fn armor_layer_defs(layers: &[EquipmentLayerJson], layer_type: &str) -> Vec<ArmorLayerDef> {
    layers
        .iter()
        .filter_map(|layer| {
            let texture_key = equipment_texture_key(&layer.texture, layer_type)?;
            let dye = layer
                .dyeable
                .as_ref()
                .map_or(ArmorDye::None, |dyeable| ArmorDye::Dyeable {
                    color_when_undyed: dyeable
                        .color_when_undyed
                        .map(|color| color as u32 & 0x00ff_ffff),
                });
            Some(ArmorLayerDef { texture_key, dye })
        })
        .collect()
}

fn load_armor_asset_defs(
    jar_assets_dir: &Path,
    packs: Option<&ResourcePackManager>,
) -> HashMap<String, ArmorAssetDef> {
    discover_equipment_json(jar_assets_dir, packs)
        .into_iter()
        .filter_map(|(id, path)| {
            let info = read_equipment_json(&path)?;
            let layers = |ty| {
                info.layers
                    .get(ty)
                    .map_or_else(Vec::new, |layers| armor_layer_defs(layers, ty))
            };
            let humanoid = layers("humanoid");
            let leggings = layers("humanoid_leggings");
            (!humanoid.is_empty() || !leggings.is_empty())
                .then_some((id, ArmorAssetDef { humanoid, leggings }))
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn build_armor_texture_layer(
    device: &rhi::backend::Device,
    texture_layout: rhi::backend::BindGroupLayout,
    texture_sampler: rhi::backend::Sampler,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    packs: Option<&ResourcePackManager>,
    def: ArmorLayerDef,
) -> ArmorTextureLayer {
    let path = resolve_asset_path_with_packs(jar_assets_dir, asset_index, &def.texture_key, packs);
    let (pixels, width, height) = util::load_png(&path).unwrap_or_else(|| {
        tracing::warn!(
            "Failed to load equipment texture {}, using fallback",
            def.texture_key
        );
        fallback_texture(64)
    });
    let texture = util::create_sampled_texture(device, "armor_texture", &pixels, width, height);

    let set = util::create_texture_group(device, texture_layout, texture.handle(), texture_sampler);

    ArmorTextureLayer {
        texture,
        set,
        dye: def.dye,
    }
}

/// Loads every equipment definition with humanoid layers and uploads its
/// textures.
fn load_armor_assets(
    device: &rhi::backend::Device,
    texture_layout: rhi::backend::BindGroupLayout,
    texture_sampler: rhi::backend::Sampler,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    packs: &ResourcePackManager,
) -> HashMap<String, ArmorAsset> {
    let packs = Some(packs);
    let defs = load_armor_asset_defs(jar_assets_dir, packs);
    defs.into_iter()
        .map(|(id, def)| {
            let build = |layer| {
                build_armor_texture_layer(
                    device,
                    texture_layout,
                    texture_sampler,
                    jar_assets_dir,
                    asset_index,
                    packs,
                    layer,
                )
            };
            (
                id,
                ArmorAsset {
                    humanoid: def.humanoid.into_iter().map(build).collect(),
                    leggings: def.leggings.into_iter().map(build).collect(),
                },
            )
        })
        .collect()
}

fn build_armor_mesh(device: &rhi::backend::Device, model: BakedEntityModel) -> ArmorMesh {
    let vertex_buffer = rhi::backend::Buffer::with_data(
        device,
        "armor_vertices",
        bytemuck::cast_slice(&model.vertices),
        rhi::BufferUsages::VERTEX,
    );
    ArmorMesh {
        model,
        vertex_buffer,
    }
}

fn build_armor_meshes(device: &rhi::backend::Device) -> ArmorMeshes {
    let bake = |bake_slot: fn(entity_model::HumanoidArmorSlot) -> BakedEntityModel| {
        entity_model::HumanoidArmorSlot::ALL.map(|slot| build_armor_mesh(device, bake_slot(slot)))
    };
    ArmorMeshes {
        humanoid: bake(entity_model::bake_humanoid_armor_model),
        husk: bake(entity_model::bake_husk_armor_model),
        zombie_villager: bake(entity_model::bake_zombie_villager_armor_model),
    }
}

fn destroy_armor_assets(device: &rhi::backend::Device, assets: &mut HashMap<String, ArmorAsset>) {
    for layer in assets
        .drain()
        .flat_map(|(_, asset)| asset.humanoid.into_iter().chain(asset.leggings))
    {
        layer.texture.destroy(device);
        layer.set.destroy(device);
    }
}

impl ArmorMeshes {
    // TODO: the other vanilla `HumanoidArmorLayer` wearers once pomme renders
    // them: wither skeleton (1.2 scale), parched, piglin / piglin brute /
    // zombified piglin (`PiglinModel.createArmorMeshSet`), giant, armor stand.
    fn for_entity(&self, kind: EntityKind, slot: usize) -> Option<&ArmorMesh> {
        if slot >= 4 {
            return None;
        }
        match kind {
            EntityKind::ZombieVillager => Some(&self.zombie_villager[slot]),
            EntityKind::Husk => Some(&self.husk[slot]),
            EntityKind::Player
            | EntityKind::Zombie
            | EntityKind::Drowned
            | EntityKind::Skeleton
            | EntityKind::Stray
            | EntityKind::Bogged => Some(&self.humanoid[slot]),
            _ => None,
        }
    }
}

pub struct EntityRenderer {
    pipeline: rhi::backend::RenderPipeline,
    /// Opaque with backface culling — bat wings.
    culled_pipeline: rhi::backend::RenderPipeline,
    translucent_culled_pipeline: rhi::backend::RenderPipeline,
    /// Translucent, depth-writing — slime shell.
    body_translucent_pipeline: rhi::backend::RenderPipeline,
    /// Translucent, depth-write off — spider eyes.
    eyes_pipeline: rhi::backend::RenderPipeline,
    /// Additive, depth-writing — charged-creeper energy swirl.
    swirl_pipeline: rhi::backend::RenderPipeline,
    /// Glowing outlines into the outline target, unculled then culled.
    outline_pipelines: [rhi::backend::RenderPipeline; 2],
    /// This frame's outline draws, paired with `outline_pipelines`.
    outline_records: [Vec<DrawRecord>; 2],
    /// The instances this frame's buffer holds.
    instance_count: usize,
    pipeline_layout: rhi::backend::PipelineLayout,
    camera_layout: rhi::backend::BindGroupLayout,
    texture_layout: rhi::backend::BindGroupLayout,
    camera_sets: Vec<rhi::backend::BindGroup>,
    camera_buffers: Vec<rhi::backend::Buffer>,
    /// The shared [`Lightmap`]'s group, set 2.
    lightmap_set: rhi::backend::BindGroup,
    /// Per-instance vertex buffer (bound at binding 1), one per frame in
    /// flight.
    instance_buffers: Vec<rhi::backend::Buffer>,
    texture_sampler: rhi::backend::Sampler,
    /// REPEAT-wrap sampler for the scrolling swirl overlay.
    texture_sampler_repeat: rhi::backend::Sampler,
    mobs: HashMap<EntityKind, MobEntry>,
    armor_assets: HashMap<String, ArmorAsset>,
    armor_meshes: ArmorMeshes,
    player_skins: HashMap<uuid::Uuid, PlayerSkinTexture>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum BlendMode {
    Opaque,
    /// Opaque with backface culling (vanilla `entityCutoutCull`) — used by
    /// meshes with coplanar zero-depth quads (bat wings).
    OpaqueCulled,
    Translucent,
    /// Same blend as `Translucent` but keeps depth writes (vanilla
    /// `entityTranslucent` vs `EYES`).
    TranslucentDepthWrite,
    /// `TranslucentDepthWrite` with backface culling (vanilla
    /// `entityTranslucentCull`): an invisible mob the viewer sees through.
    TranslucentCulled,
    Additive,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AnimationType {
    Quadruped,
    Chicken,
    Humanoid,
    Enderman,
    Zombie,
    Skeleton,
    Spider,
    Villager,
    Witch,
    Wolf,
    /// Cat and ocelot (ocelots only drive the crouch/sprint inputs).
    Feline,
    Rabbit,
    /// Horse family; the hook set is derived from (entity kind, is_baby).
    Equine,
    Squid,
    Bat,
    /// Cod, salmon, tropical fish, pufferfish.
    Fish,
    Golem,
    /// No part animation (slime — size/squish live in the body transform).
    Static,
}

struct VariantDef {
    model: BakedEntityModel,
    /// Outer slice: one entry per texture variant (variant_index). Inner slice:
    /// fallback chain of asset keys.
    tex_variants: &'static [&'static [&'static str]],
    tex_size: u32,
    overlay_kind: OverlayKind,
    invisible_outline: bool,
}

struct MobDef {
    kind: EntityKind,
    anim: AnimationType,
    adult: Vec<VariantDef>,
    baby: Option<VariantDef>,
    adult_overlays: Vec<VariantDef>,
    baby_overlays: Vec<VariantDef>,
}

fn mob_definitions() -> Vec<MobDef> {
    // One single-fallback texture entry per name under an entity texture dir.
    macro_rules! tex_table {
        ($dir:expr => $($name:literal),+ $(,)?) => {
            &[$(&[concat!("minecraft/textures/entity/", $dir, "/", $name, ".png")]),+]
        };
    }
    // The villager and zombie-villager overlay dirs ship identical
    // registry-ordered file names; each list is written once here and both
    // mobs' tables expand from it. Types index by the builtin VillagerKind
    // registry order, professions by VillagerProfession order minus "none"
    // (which has no texture), levels by profession level 1-5 minus one.
    macro_rules! villager_type_table {
        ($dir:expr) => {
            tex_table!($dir => "desert", "jungle", "plains", "savanna", "snow", "swamp", "taiga")
        };
    }
    macro_rules! villager_profession_table {
        ($dir:expr) => {
            tex_table!($dir => "armorer", "butcher", "cartographer", "cleric", "farmer",
                "fisherman", "fletcher", "leatherworker", "librarian", "mason", "nitwit",
                "shepherd", "toolsmith", "weaponsmith")
        };
    }
    macro_rules! villager_level_table {
        ($dir:expr) => {
            tex_table!($dir => "stone", "iron", "gold", "emerald", "diamond")
        };
    }

    const PIG_ADULT_TEX: &[&[&str]] = &[&[
        "minecraft/textures/entity/pig/pig_temperate.png",
        "minecraft/textures/entity/pig/temperate_pig.png",
    ]];
    const PIG_BABY_TEX: &[&[&str]] = &[&["minecraft/textures/entity/pig/pig_temperate_baby.png"]];
    const COW_ADULT_TEX: &[&[&str]] = &[
        &[
            "minecraft/textures/entity/cow/cow_temperate.png",
            "minecraft/textures/entity/cow/cow.png",
        ],
        &["minecraft/textures/entity/cow/cow_cold.png"],
        &["minecraft/textures/entity/cow/cow_warm.png"],
    ];
    const COW_BABY_TEX: &[&[&str]] = &[
        &["minecraft/textures/entity/cow/cow_temperate_baby.png"],
        &["minecraft/textures/entity/cow/cow_cold_baby.png"],
        &["minecraft/textures/entity/cow/cow_warm_baby.png"],
    ];
    // The two normal-mesh variants share one VariantDef, the cold mesh gets
    // its own; the flattened pool follows CHICKEN_VARIANT_ORDER.
    const CHICKEN_NORMAL_TEX: &[&[&str]] = &[
        &[
            "minecraft/textures/entity/chicken/chicken_temperate.png",
            "minecraft/textures/entity/chicken.png",
        ],
        &["minecraft/textures/entity/chicken/chicken_warm.png"],
    ];
    const CHICKEN_COLD_TEX: &[&[&str]] = &[&["minecraft/textures/entity/chicken/chicken_cold.png"]];
    const CHICKEN_BABY_TEX: &[&[&str]] = &[
        &["minecraft/textures/entity/chicken/chicken_temperate_baby.png"],
        &["minecraft/textures/entity/chicken/chicken_warm_baby.png"],
        &["minecraft/textures/entity/chicken/chicken_cold_baby.png"],
    ];
    const SHEEP_ADULT_TEX: &[&[&str]] = tex_table!("sheep" => "sheep");
    const SHEEP_BABY_TEX: &[&[&str]] = tex_table!("sheep" => "sheep_baby");
    const SHEEP_WOOL_UNDERCOAT_TEX: &[&[&str]] = tex_table!("sheep" => "sheep_wool_undercoat");
    const SHEEP_WOOL_TEX: &[&[&str]] = tex_table!("sheep" => "sheep_wool");
    const SHEEP_BABY_WOOL_TEX: &[&[&str]] = tex_table!("sheep" => "sheep_wool_baby");
    const PLAYER_TEX: &[&[&str]] = tex_table!("player/wide" => "steve");
    const ZOMBIE_TEX: &[&[&str]] = tex_table!("zombie" => "zombie");
    const ZOMBIE_BABY_TEX: &[&[&str]] = tex_table!("zombie" => "zombie_baby");
    const HUSK_TEX: &[&[&str]] = tex_table!("zombie" => "husk");
    const HUSK_BABY_TEX: &[&[&str]] = tex_table!("zombie" => "husk_baby");
    const DROWNED_TEX: &[&[&str]] = tex_table!("zombie" => "drowned");
    const DROWNED_BABY_TEX: &[&[&str]] = tex_table!("zombie" => "drowned_baby");
    const DROWNED_OUTER_TEX: &[&[&str]] = tex_table!("zombie" => "drowned_outer_layer");
    const DROWNED_OUTER_BABY_TEX: &[&[&str]] = tex_table!("zombie" => "drowned_outer_layer_baby");
    const ZOMBIE_VILLAGER_TEX: &[&[&str]] = tex_table!("zombie_villager" => "zombie_villager");
    const ZOMBIE_VILLAGER_BABY_TEX: &[&[&str]] =
        tex_table!("zombie_villager" => "zombie_villager_baby");
    const ZOMBIE_VILLAGER_TYPE_TEX: &[&[&str]] = villager_type_table!("zombie_villager/type");
    const ZOMBIE_VILLAGER_BABY_TYPE_TEX: &[&[&str]] = villager_type_table!("zombie_villager/baby");
    const ZOMBIE_VILLAGER_PROFESSION_TEX: &[&[&str]] =
        villager_profession_table!("zombie_villager/profession");
    const ZOMBIE_VILLAGER_LEVEL_TEX: &[&[&str]] =
        villager_level_table!("zombie_villager/profession_level");
    // Wolf pool: variant_index = variant * 3 + state (0 wild, 1 tame,
    // 2 angry); variants follow WOLF_VARIANT_ORDER.
    const WOLF_TEX: &[&[&str]] = tex_table!("wolf" =>
        "wolf", "wolf_tame", "wolf_angry",
        "wolf_spotted", "wolf_spotted_tame", "wolf_spotted_angry",
        "wolf_snowy", "wolf_snowy_tame", "wolf_snowy_angry",
        "wolf_black", "wolf_black_tame", "wolf_black_angry",
        "wolf_ashen", "wolf_ashen_tame", "wolf_ashen_angry",
        "wolf_rusty", "wolf_rusty_tame", "wolf_rusty_angry",
        "wolf_woods", "wolf_woods_tame", "wolf_woods_angry",
        "wolf_chestnut", "wolf_chestnut_tame", "wolf_chestnut_angry",
        "wolf_striped", "wolf_striped_tame", "wolf_striped_angry");
    const WOLF_BABY_TEX: &[&[&str]] = tex_table!("wolf" =>
        "wolf_baby", "wolf_tame_baby", "wolf_angry_baby",
        "wolf_spotted_baby", "wolf_spotted_tame_baby", "wolf_spotted_angry_baby",
        "wolf_snowy_baby", "wolf_snowy_tame_baby", "wolf_snowy_angry_baby",
        "wolf_black_baby", "wolf_black_tame_baby", "wolf_black_angry_baby",
        "wolf_ashen_baby", "wolf_ashen_tame_baby", "wolf_ashen_angry_baby",
        "wolf_rusty_baby", "wolf_rusty_tame_baby", "wolf_rusty_angry_baby",
        "wolf_woods_baby", "wolf_woods_tame_baby", "wolf_woods_angry_baby",
        "wolf_chestnut_baby", "wolf_chestnut_tame_baby", "wolf_chestnut_angry_baby",
        "wolf_striped_baby", "wolf_striped_tame_baby", "wolf_striped_angry_baby");
    const WOLF_COLLAR_TEX: &[&[&str]] = tex_table!("wolf" => "wolf_collar");
    const WOLF_COLLAR_BABY_TEX: &[&[&str]] = tex_table!("wolf" => "wolf_collar_baby");
    // Cat pool follows CAT_VARIANT_ORDER.
    const CAT_TEX: &[&[&str]] = tex_table!("cat" =>
        "cat_all_black", "cat_black", "cat_british_shorthair", "cat_calico", "cat_jellie",
        "cat_persian", "cat_ragdoll", "cat_red", "cat_siamese", "cat_tabby", "cat_white");
    const CAT_BABY_TEX: &[&[&str]] = tex_table!("cat" =>
        "cat_all_black_baby", "cat_black_baby", "cat_british_shorthair_baby", "cat_calico_baby",
        "cat_jellie_baby", "cat_persian_baby", "cat_ragdoll_baby", "cat_red_baby",
        "cat_siamese_baby", "cat_tabby_baby", "cat_white_baby");
    const CAT_COLLAR_TEX: &[&[&str]] = tex_table!("cat" => "cat_collar");
    const CAT_COLLAR_BABY_TEX: &[&[&str]] = tex_table!("cat" => "cat_collar_baby");
    const OCELOT_TEX: &[&[&str]] = tex_table!("cat" => "ocelot");
    const OCELOT_BABY_TEX: &[&[&str]] = tex_table!("cat" => "ocelot_baby");
    // Rabbit: variant ids 0-6 in vanilla id order, slot 7 = the "Toast"
    // custom-name override.
    const RABBIT_TEX: &[&[&str]] = tex_table!("rabbit" =>
        "rabbit_brown", "rabbit_white", "rabbit_black", "rabbit_white_splotched",
        "rabbit_gold", "rabbit_salt", "rabbit_caerbannog", "rabbit_toast");
    const RABBIT_BABY_TEX: &[&[&str]] = tex_table!("rabbit" =>
        "rabbit_brown_baby", "rabbit_white_baby", "rabbit_black_baby",
        "rabbit_white_splotched_baby", "rabbit_gold_baby", "rabbit_salt_baby",
        "rabbit_caerbannog_baby", "rabbit_toast_baby");
    // Horse variant_index = color id 0-6; markings overlay variant = id - 1.
    const HORSE_TEX: &[&[&str]] = tex_table!("horse" =>
        "horse_white", "horse_creamy", "horse_chestnut", "horse_brown", "horse_black",
        "horse_gray", "horse_darkbrown");
    const HORSE_BABY_TEX: &[&[&str]] = tex_table!("horse" =>
        "horse_white_baby", "horse_creamy_baby", "horse_chestnut_baby", "horse_brown_baby",
        "horse_black_baby", "horse_gray_baby", "horse_darkbrown_baby");
    const HORSE_MARKINGS_TEX: &[&[&str]] = tex_table!("horse" =>
        "horse_markings_white", "horse_markings_whitefield", "horse_markings_whitedots",
        "horse_markings_blackdots");
    const HORSE_MARKINGS_BABY_TEX: &[&[&str]] = tex_table!("horse" =>
        "horse_markings_white_baby", "horse_markings_whitefield_baby",
        "horse_markings_whitedots_baby", "horse_markings_blackdots_baby");
    const DONKEY_TEX: &[&[&str]] = tex_table!("horse" => "donkey");
    const DONKEY_BABY_TEX: &[&[&str]] = tex_table!("horse" => "donkey_baby");
    const MULE_TEX: &[&[&str]] = tex_table!("horse" => "mule");
    const MULE_BABY_TEX: &[&[&str]] = tex_table!("horse" => "mule_baby");
    const SKELETON_HORSE_TEX: &[&[&str]] = tex_table!("horse" => "horse_skeleton");
    const SKELETON_HORSE_BABY_TEX: &[&[&str]] = tex_table!("horse" => "horse_skeleton_baby");
    const ZOMBIE_HORSE_TEX: &[&[&str]] = tex_table!("horse" => "horse_zombie");
    const ZOMBIE_HORSE_BABY_TEX: &[&[&str]] = tex_table!("horse" => "horse_zombie_baby");
    const SQUID_TEX: &[&[&str]] = tex_table!("squid" => "squid");
    const SQUID_BABY_TEX: &[&[&str]] = tex_table!("squid" => "squid_baby");
    const GLOW_SQUID_TEX: &[&[&str]] = tex_table!("squid" => "glow_squid");
    const GLOW_SQUID_BABY_TEX: &[&[&str]] = tex_table!("squid" => "glow_squid_baby");
    const BAT_TEX: &[&[&str]] = tex_table!("bat" => "bat");
    const COD_TEX: &[&[&str]] = tex_table!("fish" => "cod");
    const SALMON_TEX: &[&[&str]] = tex_table!("fish" => "salmon");
    const PUFFERFISH_TEX: &[&[&str]] = tex_table!("fish" => "pufferfish");
    const IRON_GOLEM_TEX: &[&[&str]] = tex_table!("iron_golem" => "iron_golem");
    // Indexed by crackiness level minus one (low, medium, high).
    const IRON_GOLEM_CRACKINESS_TEX: &[&[&str]] = tex_table!("iron_golem" =>
        "iron_golem_crackiness_low", "iron_golem_crackiness_medium",
        "iron_golem_crackiness_high");
    const TROPICAL_A_TEX: &[&[&str]] = tex_table!("fish" => "tropical_a");
    const TROPICAL_B_TEX: &[&[&str]] = tex_table!("fish" => "tropical_b");
    const TROPICAL_A_PATTERN_TEX: &[&[&str]] = tex_table!("fish" =>
        "tropical_a_pattern_1", "tropical_a_pattern_2", "tropical_a_pattern_3",
        "tropical_a_pattern_4", "tropical_a_pattern_5", "tropical_a_pattern_6");
    const TROPICAL_B_PATTERN_TEX: &[&[&str]] = tex_table!("fish" =>
        "tropical_b_pattern_1", "tropical_b_pattern_2", "tropical_b_pattern_3",
        "tropical_b_pattern_4", "tropical_b_pattern_5", "tropical_b_pattern_6");
    const SKELETON_TEX: &[&[&str]] = tex_table!("skeleton" => "skeleton");
    const STRAY_TEX: &[&[&str]] = tex_table!("skeleton" => "stray");
    const STRAY_OVERLAY_TEX: &[&[&str]] = tex_table!("skeleton" => "stray_overlay");
    const BOGGED_TEX: &[&[&str]] = tex_table!("skeleton" => "bogged");
    const BOGGED_OVERLAY_TEX: &[&[&str]] = tex_table!("skeleton" => "bogged_overlay");
    const CREEPER_TEX: &[&[&str]] = tex_table!("creeper" => "creeper");
    const CREEPER_ARMOR_TEX: &[&[&str]] = tex_table!("creeper" => "creeper_armor");
    const SPIDER_TEX: &[&[&str]] = tex_table!("spider" => "spider");
    const SPIDER_EYES_TEX: &[&[&str]] = tex_table!("spider" => "spider_eyes");
    const ENDERMAN_TEX: &[&[&str]] = tex_table!("enderman" => "enderman");
    const ENDERMAN_EYES_TEX: &[&[&str]] = tex_table!("enderman" => "enderman_eyes");
    const SLIME_TEX: &[&[&str]] = tex_table!("slime" => "slime");
    const WITCH_TEX: &[&[&str]] = &[&[
        "minecraft/textures/entity/witch/witch.png",
        "minecraft/textures/entity/witch.png",
    ]];
    const VILLAGER_TEX: &[&[&str]] = tex_table!("villager" => "villager");
    const VILLAGER_BABY_TEX: &[&[&str]] = tex_table!("villager" => "villager_baby");
    const VILLAGER_TYPE_TEX: &[&[&str]] = villager_type_table!("villager/type");
    const VILLAGER_BABY_TYPE_TEX: &[&[&str]] = villager_type_table!("villager/baby");
    const VILLAGER_PROFESSION_TEX: &[&[&str]] = villager_profession_table!("villager/profession");
    const VILLAGER_LEVEL_TEX: &[&[&str]] = villager_level_table!("villager/profession_level");

    // Base and baby models, plus opaque overlays (sheep wool), are all Opaque.
    fn opaque(
        model: BakedEntityModel,
        tex_variants: &'static [&'static [&'static str]],
        tex_size: u32,
    ) -> VariantDef {
        VariantDef {
            model,
            tex_variants,
            tex_size,
            overlay_kind: OverlayKind::Opaque,
            invisible_outline: false,
        }
    }

    fn outlined_when_invisible(def: VariantDef) -> VariantDef {
        VariantDef {
            invisible_outline: true,
            ..def
        }
    }

    // Cutout layers over a villager-like base skin (vanilla
    // `VillagerProfessionLayer`, shared by villager and zombie villager):
    // slot 0 = biome type, slot 1 = biome type on the no-hat model (used when
    // the profession texture brings its own hat), slot 2 = profession, slot 3
    // = profession level badge. entity_extras gates slot 0 xor 1 and picks
    // each slot's texture variant. The `bake` parameter takes `no_hat`.
    fn villager_like_overlays(
        bake: fn(bool) -> BakedEntityModel,
        type_tex: &'static [&'static [&'static str]],
        profession_tex: &'static [&'static [&'static str]],
        level_tex: &'static [&'static [&'static str]],
    ) -> Vec<VariantDef> {
        // Slots 0/2/3 share one bake of the hatted model.
        let hatted = bake(false);
        vec![
            opaque(hatted.clone(), type_tex, 64),
            opaque(bake(true), type_tex, 64),
            opaque(hatted.clone(), profession_tex, 64),
            opaque(hatted, level_tex, 64),
        ]
    }

    fn villager_like_baby_overlays(
        bake: fn(bool) -> BakedEntityModel,
        type_tex: &'static [&'static [&'static str]],
    ) -> Vec<VariantDef> {
        vec![
            opaque(bake(false), type_tex, 64),
            opaque(bake(true), type_tex, 64),
        ]
    }

    vec![
        MobDef {
            kind: EntityKind::Pig,
            anim: AnimationType::Quadruped,
            adult: vec![opaque(entity_model::bake_pig_model(), PIG_ADULT_TEX, 64)],
            baby: Some(opaque(
                entity_model::bake_baby_pig_model(),
                PIG_BABY_TEX,
                32,
            )),
            adult_overlays: vec![],
            baby_overlays: vec![],
        },
        MobDef {
            kind: EntityKind::Cow,
            anim: AnimationType::Quadruped,
            adult: vec![opaque(entity_model::bake_cow_model(), COW_ADULT_TEX, 64)],
            baby: Some(opaque(
                entity_model::bake_baby_cow_model(),
                COW_BABY_TEX,
                64,
            )),
            adult_overlays: vec![],
            baby_overlays: vec![],
        },
        MobDef {
            kind: EntityKind::Chicken,
            anim: AnimationType::Chicken,
            adult: vec![
                opaque(entity_model::bake_chicken_model(), CHICKEN_NORMAL_TEX, 64),
                opaque(
                    entity_model::bake_cold_chicken_model(),
                    CHICKEN_COLD_TEX,
                    64,
                ),
            ],
            baby: Some(opaque(
                entity_model::bake_baby_chicken_model(),
                CHICKEN_BABY_TEX,
                16,
            )),
            adult_overlays: vec![],
            baby_overlays: vec![],
        },
        MobDef {
            kind: EntityKind::Sheep,
            anim: AnimationType::Quadruped,
            adult: vec![opaque(
                entity_model::bake_sheep_model(),
                SHEEP_ADULT_TEX,
                64,
            )],
            baby: Some(opaque(
                entity_model::bake_baby_sheep_model(),
                SHEEP_BABY_TEX,
                64,
            )),
            adult_overlays: vec![
                opaque(
                    entity_model::bake_sheep_wool_undercoat_model(),
                    SHEEP_WOOL_UNDERCOAT_TEX,
                    64,
                ),
                outlined_when_invisible(opaque(
                    entity_model::bake_sheep_wool_model(),
                    SHEEP_WOOL_TEX,
                    64,
                )),
            ],
            baby_overlays: vec![outlined_when_invisible(opaque(
                entity_model::bake_baby_sheep_wool_model(),
                SHEEP_BABY_WOOL_TEX,
                64,
            ))],
        },
        MobDef {
            kind: EntityKind::Player,
            anim: AnimationType::Humanoid,
            // Variant 0 = classic (wide) arms, 1 = slim; picked per player from
            // the skin's model metadata (effective_variant_index).
            adult: vec![
                opaque(entity_model::bake_player_model(false), PLAYER_TEX, 64),
                opaque(entity_model::bake_player_model(true), PLAYER_TEX, 64),
            ],
            baby: None,
            adult_overlays: vec![],
            baby_overlays: vec![],
        },
        MobDef {
            kind: EntityKind::Zombie,
            anim: AnimationType::Zombie,
            adult: vec![opaque(entity_model::bake_zombie_model(), ZOMBIE_TEX, 64)],
            baby: Some(opaque(
                entity_model::bake_baby_zombie_model(),
                ZOMBIE_BABY_TEX,
                64,
            )),
            adult_overlays: vec![],
            baby_overlays: vec![],
        },
        MobDef {
            kind: EntityKind::Husk,
            anim: AnimationType::Zombie,
            adult: vec![opaque(entity_model::bake_husk_model(), HUSK_TEX, 64)],
            baby: Some(opaque(
                entity_model::bake_baby_zombie_model(),
                HUSK_BABY_TEX,
                64,
            )),
            adult_overlays: vec![],
            baby_overlays: vec![],
        },
        MobDef {
            kind: EntityKind::Drowned,
            anim: AnimationType::Zombie,
            adult: vec![opaque(
                entity_model::bake_drowned_model(0.0),
                DROWNED_TEX,
                64,
            )],
            baby: Some(opaque(
                entity_model::bake_baby_zombie_model(),
                DROWNED_BABY_TEX,
                64,
            )),
            adult_overlays: vec![opaque(
                entity_model::bake_drowned_model(0.25),
                DROWNED_OUTER_TEX,
                64,
            )],
            baby_overlays: vec![opaque(
                entity_model::bake_baby_drowned_outer_model(),
                DROWNED_OUTER_BABY_TEX,
                64,
            )],
        },
        MobDef {
            kind: EntityKind::ZombieVillager,
            anim: AnimationType::Zombie,
            adult: vec![opaque(
                entity_model::bake_zombie_villager_model(false),
                ZOMBIE_VILLAGER_TEX,
                64,
            )],
            baby: Some(opaque(
                entity_model::bake_baby_zombie_villager_model(false),
                ZOMBIE_VILLAGER_BABY_TEX,
                64,
            )),
            adult_overlays: villager_like_overlays(
                entity_model::bake_zombie_villager_model,
                ZOMBIE_VILLAGER_TYPE_TEX,
                ZOMBIE_VILLAGER_PROFESSION_TEX,
                ZOMBIE_VILLAGER_LEVEL_TEX,
            ),
            baby_overlays: villager_like_baby_overlays(
                entity_model::bake_baby_zombie_villager_model,
                ZOMBIE_VILLAGER_BABY_TYPE_TEX,
            ),
        },
        MobDef {
            kind: EntityKind::Skeleton,
            anim: AnimationType::Skeleton,
            adult: vec![opaque(
                entity_model::bake_skeleton_model(),
                SKELETON_TEX,
                64,
            )],
            baby: None,
            adult_overlays: vec![],
            baby_overlays: vec![],
        },
        MobDef {
            kind: EntityKind::Stray,
            anim: AnimationType::Skeleton,
            adult: vec![opaque(entity_model::bake_skeleton_model(), STRAY_TEX, 64)],
            baby: None,
            adult_overlays: vec![opaque(
                entity_model::bake_skeleton_clothing_model(0.25),
                STRAY_OVERLAY_TEX,
                64,
            )],
            baby_overlays: vec![],
        },
        MobDef {
            kind: EntityKind::Bogged,
            anim: AnimationType::Skeleton,
            // Variant 0 = mushrooms, 1 = sheared (empty mushroom parts).
            // TODO: replace with a per-part visibility mask (vanilla
            // `mushrooms.visible = !isSheared`) instead of a second baked
            // model; would also drop the cubeless overlay padding.
            adult: vec![
                opaque(entity_model::bake_bogged_model(false), BOGGED_TEX, 64),
                opaque(entity_model::bake_bogged_model(true), BOGGED_TEX, 64),
            ],
            baby: None,
            adult_overlays: vec![opaque(
                entity_model::bake_bogged_clothing_model(),
                BOGGED_OVERLAY_TEX,
                64,
            )],
            baby_overlays: vec![],
        },
        MobDef {
            kind: EntityKind::Creeper,
            anim: AnimationType::Quadruped,
            adult: vec![opaque(entity_model::bake_creeper_model(), CREEPER_TEX, 64)],
            baby: None,
            // Slot 0: charged-creeper energy swirl (additive, scrolling), shown only
            // when `powered` (gated via overlay_tints in entity_extras).
            adult_overlays: vec![VariantDef {
                model: entity_model::bake_creeper_model(),
                tex_variants: CREEPER_ARMOR_TEX,
                tex_size: 64,
                overlay_kind: OverlayKind::SwirlAdditive,
                invisible_outline: false,
            }],
            baby_overlays: vec![],
        },
        MobDef {
            kind: EntityKind::Villager,
            anim: AnimationType::Villager,
            adult: vec![opaque(
                entity_model::bake_villager_model(false),
                VILLAGER_TEX,
                64,
            )],
            baby: Some(opaque(
                entity_model::bake_baby_villager_model(false),
                VILLAGER_BABY_TEX,
                64,
            )),
            // TODO: CustomHeadLayer (worn head items) and CrossedArmsItemLayer
            // (held item) need a held-item layer first.
            adult_overlays: villager_like_overlays(
                entity_model::bake_villager_model,
                VILLAGER_TYPE_TEX,
                VILLAGER_PROFESSION_TEX,
                VILLAGER_LEVEL_TEX,
            ),
            baby_overlays: villager_like_baby_overlays(
                entity_model::bake_baby_villager_model,
                VILLAGER_BABY_TYPE_TEX,
            ),
        },
        MobDef {
            kind: EntityKind::Spider,
            anim: AnimationType::Spider,
            adult: vec![opaque(entity_model::bake_spider_model(), SPIDER_TEX, 64)],
            baby: None,
            // Slot 0: glowing eyes (translucent, full-bright), always visible.
            adult_overlays: vec![VariantDef {
                model: entity_model::bake_spider_model(),
                tex_variants: SPIDER_EYES_TEX,
                tex_size: 64,
                overlay_kind: OverlayKind::EyesTranslucent,
                invisible_outline: false,
            }],
            baby_overlays: vec![],
        },
        MobDef {
            kind: EntityKind::Enderman,
            anim: AnimationType::Enderman,
            adult: vec![opaque(
                entity_model::bake_enderman_model(),
                ENDERMAN_TEX,
                64,
            )],
            baby: None,
            adult_overlays: vec![VariantDef {
                model: entity_model::bake_enderman_model(),
                tex_variants: ENDERMAN_EYES_TEX,
                tex_size: 64,
                overlay_kind: OverlayKind::EyesTranslucent,
                invisible_outline: false,
            }],
            baby_overlays: vec![],
        },
        MobDef {
            kind: EntityKind::Slime,
            anim: AnimationType::Static,
            adult: vec![opaque(
                entity_model::bake_slime_inner_model(),
                SLIME_TEX,
                64,
            )],
            baby: None,
            adult_overlays: vec![VariantDef {
                model: entity_model::bake_slime_outer_model(),
                tex_variants: SLIME_TEX,
                tex_size: 64,
                overlay_kind: OverlayKind::BodyTranslucent,
                invisible_outline: true,
            }],
            baby_overlays: vec![],
        },
        MobDef {
            kind: EntityKind::Witch,
            anim: AnimationType::Witch,
            adult: vec![opaque(entity_model::bake_witch_model(), WITCH_TEX, 64)],
            baby: None,
            adult_overlays: vec![],
            baby_overlays: vec![],
        },
        // TODO: wolf armor layer (needs the equipment-asset pipeline).
        MobDef {
            kind: EntityKind::Wolf,
            anim: AnimationType::Wolf,
            adult: vec![opaque(entity_model::bake_wolf_model(), WOLF_TEX, 64)],
            baby: Some(opaque(
                entity_model::bake_baby_wolf_model(),
                WOLF_BABY_TEX,
                32,
            )),
            // Slot 0: dye-tinted collar, tame only.
            adult_overlays: vec![opaque(
                entity_model::bake_wolf_collar_model(),
                WOLF_COLLAR_TEX,
                64,
            )],
            baby_overlays: vec![opaque(
                entity_model::bake_baby_wolf_model(),
                WOLF_COLLAR_BABY_TEX,
                32,
            )],
        },
        MobDef {
            kind: EntityKind::Cat,
            anim: AnimationType::Feline,
            adult: vec![opaque(entity_model::bake_cat_model(), CAT_TEX, 64)],
            baby: Some(opaque(
                entity_model::bake_baby_cat_model(),
                CAT_BABY_TEX,
                32,
            )),
            // Slot 0: dye-tinted collar, tame only (its bake is inflated /
            // rescaled per vanilla's collar layers).
            adult_overlays: vec![opaque(
                entity_model::bake_cat_collar_model(),
                CAT_COLLAR_TEX,
                64,
            )],
            baby_overlays: vec![opaque(
                entity_model::bake_baby_cat_collar_model(),
                CAT_COLLAR_BABY_TEX,
                32,
            )],
        },
        MobDef {
            kind: EntityKind::Ocelot,
            anim: AnimationType::Feline,
            adult: vec![opaque(entity_model::bake_ocelot_model(), OCELOT_TEX, 64)],
            baby: Some(opaque(
                entity_model::bake_baby_ocelot_model(),
                OCELOT_BABY_TEX,
                32,
            )),
            adult_overlays: vec![],
            baby_overlays: vec![],
        },
        MobDef {
            kind: EntityKind::Rabbit,
            anim: AnimationType::Rabbit,
            adult: vec![opaque(entity_model::bake_rabbit_model(), RABBIT_TEX, 64)],
            baby: Some(opaque(
                entity_model::bake_baby_rabbit_model(),
                RABBIT_BABY_TEX,
                32,
            )),
            adult_overlays: vec![],
            baby_overlays: vec![],
        },
        // TODO: saddle and horse-armor equipment layers.
        MobDef {
            kind: EntityKind::Horse,
            anim: AnimationType::Equine,
            adult: vec![opaque(entity_model::bake_horse_model(), HORSE_TEX, 64)],
            baby: Some(opaque(
                entity_model::bake_baby_horse_model(),
                HORSE_BABY_TEX,
                64,
            )),
            // Slot 0: markings (vanilla `entityTranslucent`), gated on
            // markings != NONE.
            adult_overlays: vec![VariantDef {
                model: entity_model::bake_horse_model(),
                tex_variants: HORSE_MARKINGS_TEX,
                tex_size: 64,
                overlay_kind: OverlayKind::BodyTranslucent,
                invisible_outline: false,
            }],
            baby_overlays: vec![VariantDef {
                model: entity_model::bake_baby_horse_model(),
                tex_variants: HORSE_MARKINGS_BABY_TEX,
                tex_size: 64,
                overlay_kind: OverlayKind::BodyTranslucent,
                invisible_outline: false,
            }],
        },
        MobDef {
            kind: EntityKind::Donkey,
            anim: AnimationType::Equine,
            // Variant 0 = no chest, 1 = chest; the single baby bake absorbs
            // both through `base_variant`'s pool clamp.
            adult: vec![
                opaque(entity_model::bake_donkey_model(0.87, false), DONKEY_TEX, 64),
                opaque(entity_model::bake_donkey_model(0.87, true), DONKEY_TEX, 64),
            ],
            baby: Some(opaque(
                entity_model::bake_baby_donkey_model(),
                DONKEY_BABY_TEX,
                64,
            )),
            adult_overlays: vec![],
            baby_overlays: vec![],
        },
        MobDef {
            kind: EntityKind::Mule,
            anim: AnimationType::Equine,
            adult: vec![
                opaque(entity_model::bake_donkey_model(0.92, false), MULE_TEX, 64),
                opaque(entity_model::bake_donkey_model(0.92, true), MULE_TEX, 64),
            ],
            baby: Some(opaque(
                entity_model::bake_baby_donkey_model(),
                MULE_BABY_TEX,
                64,
            )),
            adult_overlays: vec![],
            baby_overlays: vec![],
        },
        MobDef {
            kind: EntityKind::SkeletonHorse,
            anim: AnimationType::Equine,
            adult: vec![opaque(
                entity_model::bake_undead_horse_model(),
                SKELETON_HORSE_TEX,
                64,
            )],
            baby: Some(opaque(
                entity_model::bake_baby_horse_model(),
                SKELETON_HORSE_BABY_TEX,
                64,
            )),
            adult_overlays: vec![],
            baby_overlays: vec![],
        },
        MobDef {
            kind: EntityKind::ZombieHorse,
            anim: AnimationType::Equine,
            adult: vec![opaque(
                entity_model::bake_undead_horse_model(),
                ZOMBIE_HORSE_TEX,
                64,
            )],
            baby: Some(opaque(
                entity_model::bake_baby_horse_model(),
                ZOMBIE_HORSE_BABY_TEX,
                64,
            )),
            adult_overlays: vec![],
            baby_overlays: vec![],
        },
        MobDef {
            kind: EntityKind::Squid,
            anim: AnimationType::Squid,
            adult: vec![opaque(entity_model::bake_squid_model(), SQUID_TEX, 64)],
            baby: Some(opaque(
                entity_model::bake_baby_squid_model(),
                SQUID_BABY_TEX,
                32,
            )),
            adult_overlays: vec![],
            baby_overlays: vec![],
        },
        // The glow itself is free (the entity pipeline is unlit/fullbright);
        // the post-hurt dimming rides base_tint.
        MobDef {
            kind: EntityKind::GlowSquid,
            anim: AnimationType::Squid,
            adult: vec![opaque(entity_model::bake_squid_model(), GLOW_SQUID_TEX, 64)],
            baby: Some(opaque(
                entity_model::bake_baby_squid_model(),
                GLOW_SQUID_BABY_TEX,
                32,
            )),
            adult_overlays: vec![],
            baby_overlays: vec![],
        },
        MobDef {
            kind: EntityKind::Bat,
            anim: AnimationType::Bat,
            // Backface-culled: the bat's zero-depth quads are coplanar
            // front/back pairs (vanilla `entityCutoutCull`).
            adult: vec![VariantDef {
                model: entity_model::bake_bat_model(),
                tex_variants: BAT_TEX,
                tex_size: 32,
                overlay_kind: OverlayKind::OpaqueCulled,
                invisible_outline: false,
            }],
            baby: None,
            adult_overlays: vec![],
            baby_overlays: vec![],
        },
        MobDef {
            kind: EntityKind::Cod,
            anim: AnimationType::Fish,
            adult: vec![opaque(entity_model::bake_cod_model(), COD_TEX, 32)],
            baby: None,
            adult_overlays: vec![],
            baby_overlays: vec![],
        },
        // Variant = size (small/medium/large), three root-scaled bakes.
        MobDef {
            kind: EntityKind::Salmon,
            anim: AnimationType::Fish,
            adult: vec![
                opaque(entity_model::bake_salmon_model(0.5), SALMON_TEX, 32),
                opaque(entity_model::bake_salmon_model(1.0), SALMON_TEX, 32),
                opaque(entity_model::bake_salmon_model(1.5), SALMON_TEX, 32),
            ],
            baby: None,
            adult_overlays: vec![],
            baby_overlays: vec![],
        },
        // Variant = shape; the dye-tinted pattern layer picks the matching
        // shape slot (0 small / 1 large, xor-gated in entity_extras).
        MobDef {
            kind: EntityKind::TropicalFish,
            anim: AnimationType::Fish,
            adult: vec![
                opaque(
                    entity_model::bake_tropical_fish_model(false, 0.0),
                    TROPICAL_A_TEX,
                    32,
                ),
                opaque(
                    entity_model::bake_tropical_fish_model(true, 0.0),
                    TROPICAL_B_TEX,
                    32,
                ),
            ],
            baby: None,
            adult_overlays: vec![
                opaque(
                    entity_model::bake_tropical_fish_model(false, 0.008),
                    TROPICAL_A_PATTERN_TEX,
                    32,
                ),
                opaque(
                    entity_model::bake_tropical_fish_model(true, 0.008),
                    TROPICAL_B_PATTERN_TEX,
                    32,
                ),
            ],
            baby_overlays: vec![],
        },
        // Variant = puff state (three meshes).
        MobDef {
            kind: EntityKind::Pufferfish,
            anim: AnimationType::Fish,
            adult: vec![
                opaque(entity_model::bake_pufferfish_model(0), PUFFERFISH_TEX, 32),
                opaque(entity_model::bake_pufferfish_model(1), PUFFERFISH_TEX, 32),
                opaque(entity_model::bake_pufferfish_model(2), PUFFERFISH_TEX, 32),
            ],
            baby: None,
            adult_overlays: vec![],
            baby_overlays: vec![],
        },
        // TODO: `IronGolemFlowerLayer` (the offered poppy) needs block models
        // rendered inside an entity pose.
        MobDef {
            kind: EntityKind::IronGolem,
            anim: AnimationType::Golem,
            adult: vec![opaque(
                entity_model::bake_iron_golem_model(),
                IRON_GOLEM_TEX,
                128,
            )],
            baby: None,
            // Slot 0: crack overlay, gated on health in entity_extras.
            adult_overlays: vec![opaque(
                entity_model::bake_iron_golem_model(),
                IRON_GOLEM_CRACKINESS_TEX,
                128,
            )],
            baby_overlays: vec![],
        },
    ]
}

impl EntityRenderer {
    pub fn new(
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
        jar_assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
        packs: &ResourcePackManager,
        lightmap: &Lightmap,
    ) -> Self {
        let camera_layout = rhi::backend::create_bind_group_layout(device, util::VERTEX_UNIFORM);
        let texture_layout = rhi::backend::create_bind_group_layout(device, util::FRAGMENT_TEXTURE);
        let pipeline_layout = rhi::backend::create_pipeline_layout(
            device,
            &[camera_layout, texture_layout, lightmap.layout],
            None,
        );

        let [
            translucent_culled_pipeline,
            pipeline,
            culled_pipeline,
            body_translucent_pipeline,
            eyes_pipeline,
            swirl_pipeline,
        ] = create_pipelines(device, targets, pipeline_layout);
        let outline_pipelines = [rhi::CullMode::None, rhi::CullMode::Back]
            .map(|cull_mode| create_outline_pipeline(device, pipeline_layout, cull_mode));

        let defs = mob_definitions();

        let (camera_sets, camera_buffers) = util::create_uniform_sets(
            device,
            camera_layout,
            size_of::<CameraUniform>() as u64,
            "entity_camera_uniform",
        );
        // Per-instance data is a vertex buffer (bound at binding 1), not an SSBO:
        // MoltenVK can't translate a storage-buffer read in a vertex shader.
        let instance_buffers = util::create_per_frame_host_buffers(
            device,
            (MAX_INSTANCES * size_of::<EntityInstance>()) as u64,
            rhi::BufferUsages::VERTEX,
            "entity_instances",
        );

        let texture_sampler = rhi::backend::create_sampler(device, &rhi::SamplerDesc::NEAREST);
        let texture_sampler_repeat =
            rhi::backend::create_sampler(device, &rhi::SamplerDesc::NEAREST_REPEAT);
        let armor_assets = load_armor_assets(
            device,
            texture_layout,
            texture_sampler,
            jar_assets_dir,
            asset_index,
            packs,
        );
        let armor_meshes = build_armor_meshes(device);

        let mut mobs = HashMap::new();

        for def in defs {
            let mut build = |v: VariantDef| {
                build_variants(
                    device,
                    texture_layout,
                    texture_sampler,
                    texture_sampler_repeat,
                    jar_assets_dir,
                    asset_index,
                    v,
                )
            };
            let adult_variants: Vec<MobVariant> =
                def.adult.into_iter().flat_map(&mut build).collect();
            let baby_variants = def.baby.map(&mut build);
            let mut adult_overlays: Vec<Vec<MobVariant>> =
                def.adult_overlays.into_iter().map(&mut build).collect();
            let mut baby_overlays: Vec<Vec<MobVariant>> =
                def.baby_overlays.into_iter().map(&mut build).collect();

            link_overlays(&adult_variants, &mut adult_overlays);
            if let Some(baby) = &baby_variants {
                link_overlays(baby, &mut baby_overlays);
            }

            if let Some(n) = expected_variant_count(def.kind) {
                assert_eq!(
                    adult_variants.len(),
                    n,
                    "{:?} adult variant pool != variant order length",
                    def.kind
                );
                if let Some(baby) = &baby_variants {
                    assert_eq!(
                        baby.len(),
                        n,
                        "{:?} baby variant pool != variant order length",
                        def.kind
                    );
                }
            }

            mobs.insert(
                def.kind,
                MobEntry {
                    adult_variants,
                    baby_variants,
                    adult_overlays,
                    baby_overlays,
                    anim: def.anim,
                },
            );
        }

        Self {
            pipeline,
            culled_pipeline,
            translucent_culled_pipeline,
            body_translucent_pipeline,
            eyes_pipeline,
            swirl_pipeline,
            outline_pipelines,
            outline_records: Default::default(),
            instance_count: 0,
            pipeline_layout,
            camera_layout,
            texture_layout,
            camera_sets,
            camera_buffers,
            lightmap_set: lightmap.group,
            instance_buffers,
            texture_sampler,
            texture_sampler_repeat,
            mobs,
            armor_assets,
            armor_meshes,
            player_skins: HashMap::new(),
        }
    }

    pub fn reload_armor_assets(
        &mut self,
        device: &rhi::backend::Device,
        jar_assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
        packs: &ResourcePackManager,
    ) {
        destroy_armor_assets(device, &mut self.armor_assets);
        self.armor_assets = load_armor_assets(
            device,
            self.texture_layout,
            self.texture_sampler,
            jar_assets_dir,
            asset_index,
            packs,
        );
    }

    pub fn update_camera(&mut self, frame: usize, uniform: &CameraUniform) {
        let bytes = bytemuck::bytes_of(uniform);
        self.camera_buffers[frame].write(bytes);
    }

    pub fn update_player_skin(
        &mut self,
        device: &rhi::backend::Device,
        uuid: &uuid::Uuid,
        skin: &crate::renderer::SkinData,
    ) {
        if !self.player_skins.contains_key(uuid) && self.player_skins.len() >= MAX_PLAYER_SKINS {
            tracing::warn!("Player skin cache full; keeping fallback texture for {uuid}");
            return;
        }

        let texture = util::create_sampled_texture(
            device,
            "player_skin_texture",
            &skin.pixels,
            skin.width,
            skin.height,
        );
        let set = util::create_texture_group(
            device,
            self.texture_layout,
            texture.handle(),
            self.texture_sampler,
        );

        if let Some(old) = self.player_skins.insert(
            uuid.to_owned(),
            PlayerSkinTexture {
                texture,
                set,
                slim: skin.slim,
            },
        ) {
            free_player_skin_texture(device, old);
        }

        tracing::debug!(
            "Player skin loaded for {uuid}: {}x{}",
            skin.width,
            skin.height
        );
    }

    pub fn remove_player_skin(&mut self, device: &rhi::backend::Device, uuid: &uuid::Uuid) {
        if let Some(skin) = self.player_skins.remove(uuid) {
            free_player_skin_texture(device, skin);
        }
    }

    pub fn clear_player_skins(&mut self, device: &rhi::backend::Device) {
        for (_, skin) in self.player_skins.drain() {
            free_player_skin_texture(device, skin);
        }
    }

    fn player_skin(&self, info: &EntityRenderInfo) -> Option<&PlayerSkinTexture> {
        if info.entity_kind != EntityKind::Player {
            return None;
        }
        self.player_skins.get(info.player_uuid.as_ref()?)
    }

    fn player_texture_set(
        &self,
        info: &EntityRenderInfo,
        fallback: rhi::backend::BindGroup,
    ) -> rhi::backend::BindGroup {
        self.player_skin(info).map_or(fallback, |skin| skin.set)
    }

    /// Players pick their model variant (0 = wide, 1 = slim) from the fetched
    /// skin's metadata rather than the caller-supplied index.
    fn effective_variant_index(&self, info: &EntityRenderInfo) -> u32 {
        self.player_skin(info)
            .map_or(info.variant_index, |skin| skin.slim as u32)
    }

    /// Texture set and tint of each drawn equipment layer, in submit order.
    pub(crate) fn armor_layers<'a>(
        &'a self,
        armor: &'a ArmorRenderInfo,
        slot: usize,
    ) -> impl Iterator<Item = (rhi::backend::BindGroup, [f32; 4])> + 'a {
        self.armor_assets
            .get(&*armor.asset_id)
            .into_iter()
            .flat_map(move |asset| asset.layers_for(slot))
            .filter_map(move |layer| armor_layer_tint(layer, armor).map(|tint| (layer.set, tint)))
    }

    fn collect_armor_draws(
        &self,
        vis: &[VisEntity<'_>],
        instances: &mut Instances,
    ) -> Vec<DrawRecord> {
        let mut records = Vec::new();
        for (vi, v) in vis.iter().enumerate() {
            // TODO: baby armor (vanilla `HUMANOID_BABY` layer type +
            // `HumanoidModel.createBabyArmorMeshSet`) needs anisotropic
            // `CubeDeformation`, which pomme's model format lacks.
            if v.info.is_baby {
                continue;
            }
            for slot in ARMOR_SUBMIT_ORDER {
                let Some(armor) = &v.info.armor[slot] else {
                    continue;
                };
                let Some(mesh) = self.armor_meshes.for_entity(v.info.entity_kind, slot) else {
                    continue;
                };
                // TODO: armor trims and foil (`EquipmentLayerRenderer`) need a
                // trim atlas and an entity glint pass.
                let mut layers = self.armor_layers(armor, slot).peekable();
                if layers.peek().is_none() {
                    continue;
                }

                let anim = self.compute_anim(v.entry.anim, &mesh.model, v.info);
                let part_transforms = mesh.model.compute_part_transforms(&anim);
                for (texture_set, tint) in layers {
                    for (part, (start, part_count)) in mesh.model.part_ranges.iter().enumerate() {
                        if *part_count == 0 {
                            continue;
                        }
                        let first_instance = instances.len() as u32;
                        let model = v.entity_mat * part_transforms[part];
                        instances.push(
                            vi,
                            EntityInstance {
                                model: model.to_cols_array_2d(),
                                tint,
                                // Vanilla armor is `NO_OVERLAY`: no hurt/death red.
                                overlay_color: NO_OVERLAY,
                                uv_params: [0.0, 0.0, ARMOR_ALPHA_CUTOFF, v.info.light.lane()],
                            },
                        );
                        records.push(DrawRecord {
                            texture_set,
                            vertex_buffer: mesh.vertex_buffer.handle(),
                            part_start: *start,
                            part_count: *part_count,
                            first_instance,
                            instance_count: 1,
                        });
                    }
                }
            }
        }
        records
    }

    fn compute_anim(
        &self,
        anim_type: AnimationType,
        model: &BakedEntityModel,
        info: &EntityRenderInfo,
    ) -> entity_model::PartAnim {
        // Vanilla `wrapDegrees(headRot - bodyRot)`; matters once a model
        // clamps it (equine +-20).
        let local_head_y = crate::entity::wrap_degrees(info.head_y_rot_deg - info.body_y_rot_deg);
        match anim_type {
            AnimationType::Quadruped => entity_model::compute_quadruped_anim(
                model,
                info.head_x_rot_deg,
                local_head_y,
                info.walk_anim_pos,
                info.walk_anim_speed,
                info.head_y_offset,
                info.head_x_rot_deg_override,
            ),
            AnimationType::Chicken => entity_model::compute_chicken_anim(
                model,
                info.head_x_rot_deg,
                local_head_y,
                info.walk_anim_pos,
                info.walk_anim_speed,
                info.flap,
                info.flap_speed,
            ),
            AnimationType::Humanoid => entity_model::compute_humanoid_anim(
                model,
                info.head_x_rot_deg,
                local_head_y,
                info.walk_anim_pos,
                info.walk_anim_speed,
                info.is_crouching,
            ),
            AnimationType::Enderman => entity_model::compute_enderman_anim(
                model,
                info.head_x_rot_deg,
                local_head_y,
                info.walk_anim_pos,
                info.walk_anim_speed,
                info.age_in_ticks,
                info.is_creepy,
            ),
            AnimationType::Zombie => entity_model::compute_zombie_anim(
                model,
                info.head_x_rot_deg,
                local_head_y,
                info.walk_anim_pos,
                info.walk_anim_speed,
                info.aggressive,
                info.age_in_ticks,
                info.attack_time,
            ),
            AnimationType::Skeleton => entity_model::compute_skeleton_anim(
                model,
                info.head_x_rot_deg,
                local_head_y,
                info.walk_anim_pos,
                info.walk_anim_speed,
                info.aggressive,
                info.age_in_ticks,
            ),
            AnimationType::Spider => entity_model::compute_spider_anim(
                model,
                info.head_x_rot_deg,
                local_head_y,
                info.walk_anim_pos,
                info.walk_anim_speed,
            ),
            AnimationType::Villager => entity_model::compute_villager_anim(
                model,
                info.head_x_rot_deg,
                local_head_y,
                info.walk_anim_pos,
                info.walk_anim_speed,
                info.is_unhappy,
                info.age_in_ticks,
            ),
            AnimationType::Witch => entity_model::compute_witch_anim(
                model,
                info.head_x_rot_deg,
                local_head_y,
                info.walk_anim_pos,
                info.walk_anim_speed,
                info.age_in_ticks,
                info.nose_wobble_speed,
                info.is_holding_item,
            ),
            AnimationType::Wolf => entity_model::compute_wolf_anim(
                model,
                info.head_x_rot_deg,
                local_head_y,
                info.walk_anim_pos,
                info.walk_anim_speed,
                &entity_model::WolfAnimInputs {
                    is_sitting: info.is_sitting,
                    is_angry: info.is_angry,
                    is_baby: info.is_baby,
                    tail_angle: info.tail_angle,
                    head_roll_angle: info.head_roll_angle,
                    shake_anim: info.shake_anim,
                },
            ),
            AnimationType::Feline => entity_model::compute_feline_anim(
                model,
                info.head_x_rot_deg,
                local_head_y,
                info.walk_anim_pos,
                info.walk_anim_speed,
                &entity_model::FelineAnimInputs {
                    is_crouching: info.is_crouching,
                    is_sprinting: info.is_sprinting,
                    is_sitting: info.is_sitting,
                    lie_down_amount: info.lie_down_amount,
                    lie_down_amount_tail: info.lie_down_amount_tail,
                    relax_state_one_amount: info.relax_state_one_amount,
                    is_baby: info.is_baby,
                },
            ),
            AnimationType::Rabbit => entity_model::compute_rabbit_anim(
                model,
                info.head_x_rot_deg,
                local_head_y,
                info.hop_elapsed_secs,
                info.is_baby,
            ),
            AnimationType::Equine => entity_model::compute_equine_anim(
                model,
                info.head_x_rot_deg,
                local_head_y,
                info.walk_anim_pos,
                info.walk_anim_speed,
                info.age_in_ticks,
                &entity_model::EquineAnimInputs {
                    kind: if !info.is_baby {
                        entity_model::EquineKind::Adult
                    } else if matches!(info.entity_kind, EntityKind::Donkey | EntityKind::Mule) {
                        entity_model::EquineKind::BabyDonkey
                    } else {
                        entity_model::EquineKind::BabyHorse
                    },
                    eat_anim: info.eat_anim,
                    stand_anim: info.stand_anim,
                    feeding_anim: info.feeding_anim,
                    animate_tail: info.animate_tail,
                },
            ),
            AnimationType::Squid => entity_model::compute_squid_anim(model, info.tentacle_angle),
            AnimationType::Bat => entity_model::compute_bat_anim(
                model,
                local_head_y,
                info.bat_elapsed_secs,
                info.bat_resting,
            ),
            AnimationType::Fish => entity_model::compute_fish_anim(
                model,
                info.age_in_ticks,
                info.is_in_water,
                info.entity_kind == EntityKind::Pufferfish,
            ),
            AnimationType::Golem => entity_model::compute_golem_anim(
                model,
                info.head_x_rot_deg,
                local_head_y,
                info.walk_anim_pos,
                info.walk_anim_speed,
                info.golem_attack_ticks,
                info.golem_offer_flower_ticks,
            ),
            AnimationType::Static => entity_model::PartAnim::default(),
        }
    }

    /// The translation is anchor-relative, subtracted in f64 (see
    /// `Camera::anchor`).
    fn entity_matrix(info: &EntityRenderInfo, anchor: glam::DVec3) -> glam::Mat4 {
        let mut body_y_rot_deg = info.body_y_rot_deg;
        if info.is_converting {
            // Vanilla `setupRotations` isShaking: a per-tick body-yaw jitter.
            // The addend is a radians-magnitude value applied to degrees —
            // vanilla's own unit mixing, ported literally (~±1.26 degrees).
            // Applied here, after the head-vs-body split, so the head shakes
            // with the body like vanilla.
            body_y_rot_deg += (info.age_in_ticks.floor() * 3.25).cos() * std::f32::consts::PI * 0.4;
        }
        let translation = glam::Mat4::from_translation((*info.position - anchor).as_vec3());
        // Vanilla `LivingEntityRenderer.submit`'s bed offset, then
        // `setupRotations`: death wins over the sleeping pose.
        // TODO: vanilla keys these on `Pose.SLEEPING`; pomme on SLEEPING_POS,
        // since the handler reduces the pose to crouching.
        let bed = info.sleeping_direction.filter(|_| info.is_sleeping);
        let mut base = translation;
        if let Some(direction) = bed {
            let head_offset = info.sleeping_eye_height - 0.1;
            let (step_x, step_z) = direction.step();
            base *= glam::Mat4::from_translation(glam::Vec3::new(
                -step_x * head_offset,
                0.0,
                -step_z * head_offset,
            ));
        }
        if !info.is_sleeping {
            base *= glam::Mat4::from_rotation_y((180.0 - body_y_rot_deg).to_radians());
        }
        if info.death_time > 0.0 {
            base *= glam::Mat4::from_rotation_z(
                death_fall_degrees(info.death_time, info.entity_kind).to_radians(),
            );
        } else if info.is_sleeping {
            let angle = bed.map_or(body_y_rot_deg, BedDirection::render_yaw_deg);
            base *= glam::Mat4::from_rotation_y(angle.to_radians())
                * glam::Mat4::from_rotation_z(flip_degrees(info.entity_kind).to_radians())
                * glam::Mat4::from_rotation_y(270.0_f32.to_radians());
        }
        // body_transform sits before the parts (whose root transforms carry
        // the convention's X flip), matching vanilla's setupRotations order.
        let base = info.body_transform.map_or(base, |m| base * m);
        // AvatarRenderer applies its fixed player scale after setupRotations and
        // before the model's -1.501 Y translation. Pomme's part transforms own
        // that rebase, so the scale belongs immediately before the parts here.
        if info.entity_kind == EntityKind::Player {
            base * glam::Mat4::from_scale(glam::Vec3::splat(PLAYER_MODEL_SCALE))
        } else {
            base
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        pass: &mut rhi::backend::RenderPassEncoder,
        frame: usize,
        entities: &[EntityRenderInfo],
        frustum: &[[f32; 4]; 6],
        anchor: glam::DVec3,
        eye: glam::DVec3,
        cull_dist: f32,
    ) {
        self.outline_records = Default::default();
        if entities.is_empty() {
            return;
        }

        // Build the per-frame instance buffer + draw records on the CPU
        // (immutable reads of self.mobs), grouped by variant so each (variant,
        // part) becomes a single instanced draw. `vis`/`groups` borrow self.mobs
        // and are dropped at the end of this block, before the buffer write below.
        let mut instances = Instances::default();
        let (opaque, armor, culled, translucent, body, eyes, swirl, outline) = {
            let mut vis: Vec<VisEntity> = Vec::new();
            for info in entities {
                let Some(entry) = self.mobs.get(&info.entity_kind) else {
                    continue;
                };
                if !info.skip_cull && !entity_visible(info, frustum, eye, cull_dist) {
                    continue;
                }
                let variant = entry.base_variant(info.is_baby, self.effective_variant_index(info));
                let entity_mat = Self::entity_matrix(info, anchor);
                let anim = self.compute_anim(entry.anim, &variant.model, info);
                // Shared with every overlay that isn't `own_pivots`.
                let part_transforms = variant.model.compute_part_transforms(&anim);
                vis.push(VisEntity {
                    info,
                    entry,
                    entity_mat,
                    anim,
                    part_transforms,
                });
            }
            if vis.is_empty() {
                return;
            }

            // Opaque pass: base model + opaque overlays (sheep wool, villager
            // clothing). Overlay layers are exactly coplanar with the base and
            // rely on LessOrEqual depth + draw order to win, so emit in layer
            // phases (all bases, then slot 0 across all entities, then slot
            // 1, ...) — interleaving per entity would let a shared group
            // created by an earlier entity draw a later entity's lower layer
            // after its upper one.
            let mut opaque = VariantGroups::default();
            let mut culled = VariantGroups::default();
            let mut translucent = VariantGroups::default();
            // An invisible glowing mob's parts that only leave their outline.
            let mut outline_only = VariantGroups::default();
            for (vi, v) in vis.iter().enumerate() {
                let base = v
                    .entry
                    .base_variant(v.info.is_baby, self.effective_variant_index(v.info));
                let texture_set = self.player_texture_set(v.info, base.texture_set);
                let member = (vi, v.info.base_tint, hurt_color(v.info), [0.0, 0.0]);
                match body_route(v.info) {
                    Route::Draw if base.overlay_kind == OverlayKind::OpaqueCulled => {
                        culled.add(base, texture_set, member);
                    }
                    Route::Draw => opaque.add(base, texture_set, member),
                    Route::Translucent => {
                        let [r, g, b, a] = v.info.base_tint;
                        translucent.add(
                            base,
                            texture_set,
                            (vi, [r, g, b, a * SEEN_ALPHA], member.2, member.3),
                        );
                    }
                    Route::OutlineOnly => outline_only.add(base, texture_set, member),
                    Route::Skip => {}
                }
            }
            for slot in 0..MAX_OVERLAYS {
                for (vi, v) in vis.iter().enumerate() {
                    if slot >= v.entry.overlays(v.info.is_baby).len() {
                        continue;
                    }
                    let overlay = v.entry.overlay_variant(
                        v.info.is_baby,
                        slot,
                        v.info.overlay_variants[slot],
                    );
                    let group = match overlay.overlay_kind {
                        OverlayKind::Opaque => &mut opaque,
                        OverlayKind::OpaqueCulled => &mut culled,
                        _ => continue,
                    };
                    if let Some(tint) = v.info.overlay_tints[slot] {
                        let member = (vi, tint, hurt_color(v.info), [0.0, 0.0]);
                        add_layer(v.info, overlay, member, group, &mut outline_only);
                    }
                }
            }

            let body = collect_overlays(&vis, OverlayKind::BodyTranslucent, &mut outline_only);
            let eyes = collect_overlays(&vis, OverlayKind::EyesTranslucent, &mut outline_only);
            let swirl = collect_overlays(&vis, OverlayKind::SwirlAdditive, &mut outline_only);

            let opaque = opaque.emit(&vis, &mut instances);
            let armor = self.collect_armor_draws(&vis, &mut instances);
            let culled = culled.emit(&vis, &mut instances);
            let translucent = translucent.emit(&vis, &mut instances);
            let body = body.emit(&vis, &mut instances);
            let eyes = eyes.emit(&vis, &mut instances);
            let swirl = swirl.emit(&vis, &mut instances);
            let outline_only = outline_only.emit(&vis, &mut instances);
            // Eyes and the energy swirl don't affect the outline; vanilla's
            // outline-only parts use the unculled `RenderTypes.outline`.
            let outline = [
                outline_records(
                    &[&opaque, &armor, &body, &outline_only],
                    &vis,
                    &mut instances,
                ),
                outline_records(&[&culled, &translucent], &vis, &mut instances),
            ];
            (
                opaque,
                armor,
                culled,
                translucent,
                body,
                eyes,
                swirl,
                outline,
            )
        };

        // Write the instance buffer (clamped to capacity; the cap is far above any
        // realistic entity count, so overflow only drops the tail with a warning).
        let count = instances.len().min(MAX_INSTANCES);
        if instances.len() > MAX_INSTANCES {
            tracing::warn!(
                "Entity instances ({}) exceed cap {}, dropping excess",
                instances.len(),
                MAX_INSTANCES
            );
        }
        let bytes = bytemuck::cast_slice(&instances.data[..count]);
        self.instance_buffers[frame].write(bytes);
        self.instance_count = count;
        self.outline_records = outline;

        self.record_pass(pass, frame, self.pipeline, &opaque, count);
        self.record_pass(pass, frame, self.pipeline, &armor, count);
        self.record_pass(pass, frame, self.culled_pipeline, &culled, count);
        self.record_pass(
            pass,
            frame,
            self.translucent_culled_pipeline,
            &translucent,
            count,
        );
        self.record_pass(pass, frame, self.body_translucent_pipeline, &body, count);
        self.record_pass(pass, frame, self.eyes_pipeline, &eyes, count);
        self.record_pass(pass, frame, self.swirl_pipeline, &swirl, count);
    }

    /// Whether the last `draw` left any glowing outline to draw.
    pub fn has_outlines(&self) -> bool {
        self.outline_records
            .iter()
            .any(|records| !records.is_empty())
    }

    /// The last `draw`'s glowing outlines, into the outline target's pass.
    pub fn draw_outlines(&self, pass: &mut rhi::backend::RenderPassEncoder, frame: usize) {
        for (pipeline, records) in self.outline_pipelines.iter().zip(&self.outline_records) {
            self.record_pass(pass, frame, *pipeline, records, self.instance_count);
        }
    }

    fn record_pass(
        &self,
        pass: &mut rhi::backend::RenderPassEncoder,
        frame: usize,
        pipeline: rhi::backend::RenderPipeline,
        records: &[DrawRecord],
        count: usize,
    ) {
        if records.is_empty() {
            return;
        }
        pass.set_pipeline(&pipeline);
        // Per-instance data (binding 1) is the same buffer for the whole pass;
        // gl_InstanceIndex (incl. firstInstance) indexes into it.
        pass.set_vertex_buffer(1, self.instance_buffers[frame].handle());
        let mut last_vb = rhi::backend::BufferHandle::default();
        let mut last_texture_set = rhi::backend::BindGroup::default();
        for r in records {
            if r.first_instance as usize + r.instance_count as usize > count {
                continue; // dropped by the capacity clamp above
            }
            if r.vertex_buffer != last_vb || r.texture_set != last_texture_set {
                pass.set_bind_groups(
                    0,
                    &[self.camera_sets[frame], r.texture_set, self.lightmap_set],
                );
                pass.set_vertex_buffer(0, r.vertex_buffer);
                last_vb = r.vertex_buffer;
                last_texture_set = r.texture_set;
            }
            pass.draw(
                r.part_count,
                r.instance_count,
                r.part_start,
                r.first_instance,
            );
        }
    }

    pub fn recreate_pipeline(
        &mut self,
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
    ) {
        self.destroy_pipelines(device);
        [
            self.translucent_culled_pipeline,
            self.pipeline,
            self.culled_pipeline,
            self.body_translucent_pipeline,
            self.eyes_pipeline,
            self.swirl_pipeline,
        ] = create_pipelines(device, targets, self.pipeline_layout);
    }

    fn destroy_pipelines(&self, device: &rhi::backend::Device) {
        for pipeline in [
            self.translucent_culled_pipeline,
            self.pipeline,
            self.culled_pipeline,
            self.body_translucent_pipeline,
            self.eyes_pipeline,
            self.swirl_pipeline,
        ] {
            pipeline.destroy(device);
        }
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        destroy_armor_assets(device, &mut self.armor_assets);

        util::destroy_buffers(device, &mut self.camera_buffers);
        util::destroy_buffers(device, &mut self.instance_buffers);

        for mesh in self
            .armor_meshes
            .humanoid
            .iter_mut()
            .chain(self.armor_meshes.husk.iter_mut())
            .chain(self.armor_meshes.zombie_villager.iter_mut())
        {
            std::mem::take(&mut mesh.vertex_buffer).destroy(device);
        }

        self.texture_sampler.destroy(device);
        self.texture_sampler_repeat.destroy(device);

        for entry in self.mobs.values_mut() {
            let variants: Vec<&mut MobVariant> = entry
                .adult_variants
                .iter_mut()
                .chain(entry.baby_variants.iter_mut().flatten())
                .chain(entry.adult_overlays.iter_mut().flatten())
                .chain(entry.baby_overlays.iter_mut().flatten())
                .collect();
            for v in variants {
                std::mem::take(&mut v.vertex_buffer).destroy(device);
                std::mem::take(&mut v.texture).destroy(device);
                v.texture_set.destroy(device);
            }
        }
        for (_, skin) in self.player_skins.drain() {
            free_player_skin_texture(device, skin);
        }

        self.destroy_pipelines(device);
        for pipeline in self.outline_pipelines {
            pipeline.destroy(device);
        }
        self.pipeline_layout.destroy(device);
        util::destroy_bind_groups(device, self.camera_sets.drain(..));
        self.camera_layout.destroy(device);
        self.texture_layout.destroy(device);
    }
}

/// A culled, drawable entity with its world transform, animation, and the
/// base model's per-part matrices precomputed.
struct VisEntity<'a> {
    info: &'a EntityRenderInfo,
    entry: &'a MobEntry,
    entity_mat: glam::Mat4,
    anim: entity_model::PartAnim,
    part_transforms: Vec<glam::Mat4>,
}

/// One instanced (variant, part) draw: a run of `instance_count` instances from
/// `first_instance` in the per-frame instance buffer.
#[derive(Clone, Copy)]
struct DrawRecord {
    texture_set: rhi::backend::BindGroup,
    vertex_buffer: rhi::backend::BufferHandle,
    part_start: u32,
    part_count: u32,
    first_instance: u32,
    instance_count: u32,
}

/// The frame's instances, each with the visible entity it belongs to.
#[derive(Default)]
struct Instances {
    data: Vec<EntityInstance>,
    owners: Vec<usize>,
}

impl Instances {
    fn push(&mut self, owner: usize, instance: EntityInstance) {
        self.data.push(instance);
        self.owners.push(owner);
    }

    fn len(&self) -> usize {
        self.data.len()
    }
}

/// Copies of `records`' instances whose entity glows, recoloured to its
/// outline colour: vanilla resubmits every outline-affecting render type as
/// its `outline()` type.
fn outline_records(
    records: &[&[DrawRecord]],
    vis: &[VisEntity],
    instances: &mut Instances,
) -> Vec<DrawRecord> {
    let mut outline = Vec::new();
    for record in records.iter().copied().flatten() {
        let first_instance = instances.len() as u32;
        for i in record.first_instance..record.first_instance + record.instance_count {
            let owner = instances.owners[i as usize];
            if let Some(color) = vis[owner].info.outline_color {
                let instance = EntityInstance {
                    tint: rgb_tint(color),
                    ..instances.data[i as usize]
                };
                instances.push(owner, instance);
            }
        }
        let instance_count = instances.len() as u32 - first_instance;
        if instance_count > 0 {
            outline.push(DrawRecord {
                first_instance,
                instance_count,
                ..*record
            });
        }
    }
    outline
}

/// (visible-entity index, tint, overlay color, uv scroll) for one instance.
type Member = (usize, [f32; 4], [f32; 4], [f32; 2]);

/// Visible entities grouped by variant (geometry), so each variant's parts emit
/// one instanced draw covering all its entities.
#[derive(Default)]
struct VariantGroups<'a> {
    groups: Vec<(&'a MobVariant, rhi::backend::BindGroup, Vec<Member>)>,
}

impl<'a> VariantGroups<'a> {
    fn add(
        &mut self,
        variant: &'a MobVariant,
        texture_set: rhi::backend::BindGroup,
        member: Member,
    ) {
        let key = variant as *const MobVariant as usize;
        let gi =
            match self.groups.iter().position(|(v, set, _)| {
                *v as *const MobVariant as usize == key && *set == texture_set
            }) {
                Some(gi) => gi,
                None => {
                    self.groups.push((variant, texture_set, Vec::new()));
                    self.groups.len() - 1
                }
            };
        self.groups[gi].2.push(member);
    }

    fn emit(&self, vis: &[VisEntity], instances: &mut Instances) -> Vec<DrawRecord> {
        let mut records = Vec::new();
        for (variant, texture_set, members) in &self.groups {
            // Vanilla's EMISSIVE eyes and swirl layers skip the lightmap.
            let emissive = matches!(
                variant.overlay_kind,
                OverlayKind::EyesTranslucent | OverlayKind::SwirlAdditive
            );
            let own: Option<Vec<Vec<glam::Mat4>>> = variant.own_pivots.then(|| {
                members
                    .iter()
                    .map(|(vi, ..)| variant.model.compute_part_transforms(&vis[*vi].anim))
                    .collect()
            });
            for (p, (start, part_count)) in variant.model.part_ranges.iter().enumerate() {
                if *part_count == 0 {
                    continue;
                }
                let first_instance = instances.len() as u32;
                for (k, (vi, tint, overlay, uv)) in members.iter().enumerate() {
                    let part = match &own {
                        Some(own) => own[k][p],
                        None => vis[*vi].part_transforms[p],
                    };
                    let model = vis[*vi].entity_mat * part;
                    let light = if emissive {
                        EMISSIVE_LANE
                    } else {
                        vis[*vi].info.light.lane()
                    };
                    instances.push(
                        *vi,
                        EntityInstance {
                            model: model.to_cols_array_2d(),
                            tint: *tint,
                            overlay_color: *overlay,
                            uv_params: [uv[0], uv[1], ENTITY_ALPHA_CUTOFF, light],
                        },
                    );
                }
                records.push(DrawRecord {
                    texture_set: *texture_set,
                    vertex_buffer: variant.vertex_buffer.handle(),
                    part_start: *start,
                    part_count: *part_count,
                    first_instance,
                    instance_count: members.len() as u32,
                });
            }
        }
        records
    }
}

/// Group the non-opaque overlays of one kind (body / eyes / swirl) by variant.
/// Where vanilla sends a part of an entity's model.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Route {
    Draw,
    /// The `entityTranslucentCull` body of an invisible mob the viewer sees.
    Translucent,
    /// Only into the outline, for an invisible glowing mob.
    OutlineOnly,
    Skip,
}

/// `LivingEntityRenderer.getRenderType` for the base model.
fn body_route(info: &EntityRenderInfo) -> Route {
    match info.visibility {
        Visibility::Visible => Route::Draw,
        Visibility::Seen => Route::Translucent,
        Visibility::Hidden if info.outline_color.is_some() => Route::OutlineOnly,
        Visibility::Hidden => Route::Skip,
    }
}

/// A layer of an invisible mob: most hide (`RenderLayer.
/// coloredCutoutModelCopyLayerRender`), eyes and the energy swirl ignore it,
/// and sheep wool and the slime shell leave their outline.
fn layer_route(info: &EntityRenderInfo, kind: OverlayKind, invisible_outline: bool) -> Route {
    if info.visibility == Visibility::Visible
        || matches!(
            kind,
            OverlayKind::EyesTranslucent | OverlayKind::SwirlAdditive
        )
    {
        Route::Draw
    } else if invisible_outline && info.outline_color.is_some() {
        Route::OutlineOnly
    } else {
        Route::Skip
    }
}

/// Adds `layer` to `group`, or only to `outline_only`, as `layer_route`
/// sends it.
fn add_layer<'a>(
    info: &EntityRenderInfo,
    layer: &'a MobVariant,
    member: Member,
    group: &mut VariantGroups<'a>,
    outline_only: &mut VariantGroups<'a>,
) {
    match layer_route(info, layer.overlay_kind, layer.invisible_outline) {
        Route::OutlineOnly => outline_only.add(layer, layer.texture_set, member),
        Route::Skip => {}
        _ => group.add(layer, layer.texture_set, member),
    }
}

fn collect_overlays<'a>(
    vis: &[VisEntity<'a>],
    kind: OverlayKind,
    outline_only: &mut VariantGroups<'a>,
) -> VariantGroups<'a> {
    let mut groups = VariantGroups::default();
    for (vi, v) in vis.iter().enumerate() {
        // Energy swirl scrolls its UVs over time (vanilla `EnergySwirlLayer`).
        let uv = if kind == OverlayKind::SwirlAdditive {
            let o = (v.info.age_in_ticks * 0.01).rem_euclid(1.0);
            [o, o]
        } else {
            [0.0, 0.0]
        };
        // The body layer flashes red with the entity (vanilla passes the hurt
        // overlay coords); the emissive eyes/swirl layers never do.
        let overlay_color = if kind == OverlayKind::BodyTranslucent {
            hurt_color(v.info)
        } else {
            NO_OVERLAY
        };
        for slot in 0..v.entry.overlays(v.info.is_baby).len() {
            let overlay =
                v.entry
                    .overlay_variant(v.info.is_baby, slot, v.info.overlay_variants[slot]);
            if overlay.overlay_kind != kind {
                continue;
            }
            if let Some(tint) = v.info.overlay_tints[slot] {
                let member = (vi, tint, overlay_color, uv);
                add_layer(v.info, overlay, member, &mut groups, outline_only);
            }
        }
    }
    groups
}

fn hurt_color(info: &EntityRenderInfo) -> [f32; 4] {
    if info.has_red_overlay {
        HURT_OVERLAY
    } else {
        NO_OVERLAY
    }
}

const ANIM_MARGIN: f32 = 0.5;
/// Vanilla `AvatarRenderer.scale`: players render at 15/16 model scale.
const PLAYER_MODEL_SCALE: f32 = 0.9375;

/// Standing eye height, which positions a sleeper on its bed and a
/// spectator's camera on the entity it spectates.
pub(crate) fn standing_eye_height(kind: EntityKind, is_baby: bool) -> f32 {
    match kind {
        EntityKind::Villager if is_baby => 0.63,
        _ if is_baby => entity_bounds(kind, is_baby).1 * dimensions::DEFAULT_EYE_HEIGHT_RATIO,
        _ => dimensions::EntityDimensions::eye_height(kind),
    }
}

/// Vanilla (width, height) hitbox per supported mob, scaled for babies; used to
/// build the cull bounding sphere.
fn entity_bounds(kind: EntityKind, is_baby: bool) -> (f32, f32) {
    // Vanilla babies declare explicit BABY_DIMENSIONS rather than a scale;
    // list kinds whose constant isn't the half-scale the fallback below
    // assumes. Every new baby mob must be checked against its class.
    if is_baby {
        match kind {
            EntityKind::Chicken => return (0.3, 0.4),
            EntityKind::Rabbit => return (0.24, 0.4),
            EntityKind::Zombie
            | EntityKind::Husk
            | EntityKind::Drowned
            | EntityKind::ZombieVillager
            | EntityKind::Villager => return (0.49, 0.98),
            _ => {}
        }
    }
    let (w, h) = match kind {
        EntityKind::Pig => (0.9, 0.9),
        EntityKind::Cow => (0.9, 1.4),
        EntityKind::Chicken => (0.4, 0.7),
        EntityKind::Sheep => (0.9, 1.3),
        EntityKind::Zombie
        | EntityKind::Husk
        | EntityKind::Drowned
        | EntityKind::ZombieVillager
        | EntityKind::Villager
        | EntityKind::Witch => (0.6, 1.95),
        EntityKind::Skeleton | EntityKind::Stray | EntityKind::Bogged => (0.6, 1.99),
        EntityKind::Creeper => (0.6, 1.7),
        EntityKind::Spider => (1.4, 0.9),
        EntityKind::Enderman => (0.6, 2.9),
        EntityKind::Slime => (0.52, 0.52),
        EntityKind::Wolf => (0.6, 0.85),
        EntityKind::Cat | EntityKind::Ocelot => (0.6, 0.7),
        EntityKind::Rabbit => (0.49, 0.6),
        // Horse babies scale 0.7 since 26.1 (`Horse.BABY_DIMENSIONS`; 1.21.x
        // halved, harmless for the cull sphere); donkey/mule babies are the
        // generic half scale.
        EntityKind::Horse | EntityKind::SkeletonHorse | EntityKind::ZombieHorse if is_baby => {
            return (1.3964844 * 0.7, 1.6 * 0.7);
        }
        EntityKind::Horse
        | EntityKind::Mule
        | EntityKind::SkeletonHorse
        | EntityKind::ZombieHorse => (1.3964844, 1.6),
        EntityKind::Donkey => (1.3964844, 1.5),
        // Baby squid dimensions are an explicit 0.5x0.5 in vanilla, not the
        // generic half scale.
        EntityKind::Squid | EntityKind::GlowSquid if is_baby => return (0.5, 0.5),
        EntityKind::Squid | EntityKind::GlowSquid => (0.8, 0.8),
        EntityKind::Bat => (0.5, 0.9),
        EntityKind::Cod => (0.5, 0.3),
        // Salmon/pufferfish scale with their variant; use the largest.
        EntityKind::Salmon => (1.05, 0.6),
        EntityKind::TropicalFish => (0.5, 0.4),
        EntityKind::Pufferfish => (0.7, 0.7),
        EntityKind::IronGolem => (1.4, 2.7),
        EntityKind::Player => (0.6, 1.8),
        _ => (1.0, 1.0),
    };
    let s = if is_baby { 0.5 } else { 1.0 };
    (w * s, h * s)
}

/// Bounding-sphere frustum + distance cull. The frustum planes operate on
/// camera-relative coords (like chunk cull), so the entity position is
/// rebased against the eye in f64 first.
fn entity_visible(
    info: &EntityRenderInfo,
    frustum: &[[f32; 4]; 6],
    eye: glam::DVec3,
    cull_dist: f32,
) -> bool {
    let (w, h) = entity_bounds(info.entity_kind, info.is_baby);
    // A body transform (slime size/squish) can grow the entity well past its
    // base bounds: scale the sphere and its center by the largest axis scale,
    // and pad the radius by the translation (pure-rotation transforms still
    // displace pivots — squid pitch, cat lie-down).
    let (scale, shift) = info.body_transform.map_or((1.0, 0.0), |m| {
        let s = m
            .x_axis
            .length_squared()
            .max(m.y_axis.length_squared())
            .max(m.z_axis.length_squared())
            .sqrt()
            .max(1.0);
        (s, m.w_axis.truncate().length())
    });
    let radius = (0.5 * (2.0 * w * w + h * h).sqrt() + ANIM_MARGIN) * scale + shift;
    let mut q = (*info.position - eye).as_vec3();
    q.y += h * 0.5 * scale;
    // Distance-cull with the radius as margin so an oversized entity stays
    // visible while any of its body is in range.
    let max_dist = cull_dist + radius;
    if q.length_squared() > max_dist * max_dist {
        return false;
    }
    for pl in frustum {
        if pl[0] * q.x + pl[1] * q.y + pl[2] * q.z + pl[3] < -radius {
            return false;
        }
    }
    true
}

/// Anim part-name indices are computed against the base variant's model and
/// reused for each overlay draw, so overlay part order must match the base
/// (asserted at construction rather than rendering wrong limbs). Overlays
/// whose part poses also match share the base's transforms; the rest are
/// flagged `own_pivots` and get their own.
fn link_overlays(base: &[MobVariant], overlays: &mut [Vec<MobVariant>]) {
    let Some(base_first) = base.first() else {
        return;
    };
    let base_names: Vec<&str> = base_first
        .model
        .parts
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    for overlay in overlays.iter_mut().flatten() {
        let overlay_names: Vec<&str> = overlay
            .model
            .parts
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(
            base_names, overlay_names,
            "overlay part order must match base; anim indices are shared across both"
        );
        overlay.own_pivots = !base_first.model.same_part_poses(&overlay.model);
    }
}

// TODO: share one vertex buffer + model per distinct mesh across texture
// variants (a zombie villager's 33 texture variants clone 2 meshes), and
// batch the per-texture one-time upload submits into one fence wait.
#[allow(clippy::too_many_arguments)]
fn build_variants(
    device: &rhi::backend::Device,
    texture_layout: rhi::backend::BindGroupLayout,
    texture_sampler: rhi::backend::Sampler,
    texture_sampler_repeat: rhi::backend::Sampler,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    variant: VariantDef,
) -> Vec<MobVariant> {
    let VariantDef {
        model,
        tex_variants,
        tex_size,
        overlay_kind,
        invisible_outline,
    } = variant;
    // The scrolling swirl needs REPEAT wrapping; everything else clamps.
    let sampler = match overlay_kind {
        OverlayKind::SwirlAdditive => texture_sampler_repeat,
        _ => texture_sampler,
    };
    let vert_bytes = bytemuck::cast_slice::<EntityVertex, u8>(&model.vertices);

    tex_variants
        .iter()
        .map(|tex_keys| {
            let vertex_buffer = rhi::backend::Buffer::with_data(
                device,
                "entity_vertices",
                vert_bytes,
                rhi::BufferUsages::VERTEX,
            );

            let texture =
                load_entity_texture(device, jar_assets_dir, asset_index, tex_keys, tex_size);

            let texture_set =
                util::create_texture_group(device, texture_layout, texture.handle(), sampler);

            MobVariant {
                model: model.clone(),
                vertex_buffer,
                texture,
                texture_set,
                overlay_kind,
                invisible_outline,
                own_pivots: false,
            }
        })
        .collect()
}

fn load_entity_texture(
    device: &rhi::backend::Device,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    asset_keys: &[&str],
    fallback_size: u32,
) -> rhi::backend::Texture {
    let (pixels, width, height) = asset_keys
        .iter()
        .find_map(|key| {
            let path = resolve_asset_path(jar_assets_dir, asset_index, key);
            util::load_png(&path)
        })
        .unwrap_or_else(|| {
            tracing::warn!(
                "Failed to load entity texture {:?}, using fallback",
                asset_keys
            );
            fallback_texture(fallback_size)
        });

    util::create_sampled_texture(device, "entity_texture", &pixels, width, height)
}

// TODO: frames in flight may still read a replaced, removed or cleared skin;
// free it once they finish.
fn free_player_skin_texture(device: &rhi::backend::Device, skin: PlayerSkinTexture) {
    skin.texture.destroy(device);
    skin.set.destroy(device);
}

pub(super) fn fallback_texture(size: u32) -> (Vec<u8>, u32, u32) {
    let pixels = [219u8, 148, 148, 255].repeat((size * size) as usize);
    (pixels, size, size)
}

/// The entity render pipelines, in draw order: opaque base, translucent eyes,
/// additive swirl.
fn create_pipelines(
    device: &rhi::backend::Device,
    targets: rhi::backend::PipelineTargets,
    layout: rhi::backend::PipelineLayout,
) -> [rhi::backend::RenderPipeline; 6] {
    [
        create_pipeline(
            device,
            targets,
            layout,
            BlendMode::TranslucentCulled,
            ModelInput::Instanced,
        ),
        create_pipeline(
            device,
            targets,
            layout,
            BlendMode::Opaque,
            ModelInput::Instanced,
        ),
        create_pipeline(
            device,
            targets,
            layout,
            BlendMode::OpaqueCulled,
            ModelInput::Instanced,
        ),
        create_pipeline(
            device,
            targets,
            layout,
            BlendMode::TranslucentDepthWrite,
            ModelInput::Instanced,
        ),
        create_pipeline(
            device,
            targets,
            layout,
            BlendMode::Translucent,
            ModelInput::Instanced,
        ),
        create_pipeline(
            device,
            targets,
            layout,
            BlendMode::Additive,
            ModelInput::Instanced,
        ),
    ]
}

/// Binding 0: per-vertex mesh data. Instanced pipelines add binding 1 with
/// per-instance data (model columns + tint + overlay + uv), one EntityInstance
/// per (entity, part); push-constant ones bind only the mesh.
const INSTANCED_BUFFERS: [rhi::VertexBufferLayout<'static>; 2] = [
    EntityVertex::BUFFER,
    rhi::VertexBufferLayout {
        stride: size_of::<EntityInstance>() as u32,
        step_mode: rhi::VertexStepMode::Instance,
        attributes: &rhi::packed_attributes(3, [VertexFormat::Float32x4; 7]),
    },
];

/// Vanilla's `OUTLINE_NO_CULL` / `OUTLINE_CULL`: the instanced model drawn flat
/// in its outline colour into the outline target, with no depth or blend.
fn create_outline_pipeline(
    device: &rhi::backend::Device,
    layout: rhi::backend::PipelineLayout,
    cull_mode: rhi::CullMode,
) -> rhi::backend::RenderPipeline {
    rhi::backend::create_render_pipeline(
        device,
        super::entity_outline::targets(),
        layout,
        &rhi::RenderPipelineDesc {
            cull_mode,
            ..rhi::RenderPipelineDesc::new(
                "entity outline",
                shader::include_shader!("entity.vert"),
                shader::include_shader!("entity_outline.frag"),
                &INSTANCED_BUFFERS,
            )
        },
    )
}

/// Source of a draw's model matrix: mobs are GPU-instanced (binding 1, a perf
/// divergence from vanilla); block entities keep vanilla's per-draw
/// push-constant transform (binding 0 only).
pub(super) enum ModelInput {
    Instanced,
    PushConstant,
}

pub(super) fn create_pipeline(
    device: &rhi::backend::Device,
    targets: rhi::backend::PipelineTargets,
    layout: rhi::backend::PipelineLayout,
    blend: BlendMode,
    model_input: ModelInput,
) -> rhi::backend::RenderPipeline {
    let vertex_shader: rhi::Shader = match model_input {
        ModelInput::Instanced => shader::include_shader!("entity.vert"),
        ModelInput::PushConstant => shader::include_shader!("block_entity.vert"),
    };
    let vertex_buffers = match model_input {
        ModelInput::Instanced => &INSTANCED_BUFFERS[..],
        ModelInput::PushConstant => &INSTANCED_BUFFERS[..1],
    };
    rhi::backend::create_render_pipeline(
        device,
        targets,
        layout,
        &rhi::RenderPipelineDesc {
            cull_mode: if matches!(
                blend,
                BlendMode::OpaqueCulled | BlendMode::TranslucentCulled
            ) {
                rhi::CullMode::Back
            } else {
                rhi::CullMode::None
            },
            // Only the translucent eyes overlay skips depth-write (vanilla
            // `EYES`); the opaque base, slime shell, and additive swirl write
            // depth.
            depth: Some(if blend == BlendMode::Translucent {
                rhi::DepthState::READ_ONLY
            } else {
                rhi::DepthState::WRITE
            }),
            blend: match blend {
                BlendMode::Opaque | BlendMode::OpaqueCulled => None,
                // Glowing eyes, slime shell.
                BlendMode::Translucent
                | BlendMode::TranslucentDepthWrite
                | BlendMode::TranslucentCulled => Some(rhi::BlendState::TRANSLUCENT),
                // Energy swirl glow.
                BlendMode::Additive => Some(rhi::BlendState {
                    color: rhi::BlendComponent {
                        src: rhi::BlendFactor::SrcAlpha,
                        dst: rhi::BlendFactor::One,
                    },
                    alpha: rhi::BlendComponent {
                        src: rhi::BlendFactor::One,
                        dst: rhi::BlendFactor::One,
                    },
                }),
            },
            ..rhi::RenderPipelineDesc::new(
                "entity",
                vertex_shader,
                shader::include_shader!("entity.frag"),
                vertex_buffers,
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use azalea_inventory::ItemStack;
    use azalea_inventory::components::{DyedColor, EquipmentSlot, Equippable};
    use azalea_registry::builtin::ItemKind;

    use super::{
        ArmorDye, EquipmentClientInfoJson, OverlayKind, Route, Visibility, armor_layer_defs,
        armor_render_info, body_route, equipment_texture_key, layer_route,
    };

    fn invisible(visibility: Visibility, glowing: bool) -> EntityRenderInfo {
        EntityRenderInfo {
            visibility,
            outline_color: glowing.then_some(0xFFFFFF),
            ..Default::default()
        }
    }

    #[test]
    fn invisible_bodies_route_like_get_render_type() {
        assert_eq!(
            body_route(&invisible(Visibility::Visible, false)),
            Route::Draw
        );
        assert_eq!(
            body_route(&invisible(Visibility::Seen, true)),
            Route::Translucent
        );
        assert_eq!(
            body_route(&invisible(Visibility::Hidden, true)),
            Route::OutlineOnly
        );
        assert_eq!(
            body_route(&invisible(Visibility::Hidden, false)),
            Route::Skip
        );
    }

    #[test]
    fn invisible_layers_hide_but_eyes_stay_and_wool_outlines() {
        let hidden = invisible(Visibility::Hidden, true);
        let seen = invisible(Visibility::Seen, false);
        assert_eq!(
            layer_route(&hidden, OverlayKind::Opaque, false),
            Route::Skip
        );
        assert_eq!(layer_route(&seen, OverlayKind::Opaque, false), Route::Skip);
        assert_eq!(
            layer_route(&hidden, OverlayKind::EyesTranslucent, false),
            Route::Draw
        );
        assert_eq!(
            layer_route(&hidden, OverlayKind::SwirlAdditive, false),
            Route::Draw
        );
        assert_eq!(
            layer_route(&hidden, OverlayKind::Opaque, true),
            Route::OutlineOnly
        );
        assert_eq!(
            layer_route(&seen, OverlayKind::BodyTranslucent, true),
            Route::Skip
        );
        assert_eq!(
            layer_route(
                &invisible(Visibility::Visible, false),
                OverlayKind::Opaque,
                false
            ),
            Route::Draw
        );
    }

    #[test]
    fn armor_render_info_uses_equippable_slot_asset_and_dye() {
        let diamond = ItemStack::new(ItemKind::DiamondHelmet, 1);
        let diamond_info = armor_render_info(Some(&diamond), EquipmentSlot::Head)
            .expect("diamond helmet should expose its default equippable asset");
        assert_eq!(&*diamond_info.asset_id, "minecraft:diamond");
        assert_eq!(diamond_info.dyed_color, None);
        assert!(armor_render_info(Some(&diamond), EquipmentSlot::Chest).is_none());

        let leather = ItemStack::new(ItemKind::LeatherChestplate, 1)
            .with_component(DyedColor { rgb: 0x12_34_56 });
        let leather_info = armor_render_info(Some(&leather), EquipmentSlot::Chest)
            .expect("leather chestplate should expose its default equippable asset");
        assert_eq!(&*leather_info.asset_id, "minecraft:leather");
        assert_eq!(leather_info.dyed_color, Some(0x12_34_56));
    }

    #[test]
    fn armor_render_info_respects_explicit_equippable_removal() {
        let stripped =
            ItemStack::new(ItemKind::DiamondHelmet, 1).with_component::<Equippable>(None);
        assert!(armor_render_info(Some(&stripped), EquipmentSlot::Head).is_none());
    }

    #[test]
    fn equipment_layers_preserve_order_dye_defaults_and_resource_ids() {
        let info: EquipmentClientInfoJson = serde_json::from_str(
            r#"{
                "layers": {
                    "humanoid": [
                        {"texture":"minecraft:leather","dyeable":{"color_when_undyed":10511680}},
                        {"texture":"example:overlay"}
                    ]
                }
            }"#,
        )
        .expect("valid equipment fixture");
        let layers = armor_layer_defs(&info.layers["humanoid"], "humanoid");
        assert_eq!(layers.len(), 2);
        assert_eq!(
            layers[0].texture_key,
            "minecraft/textures/entity/equipment/humanoid/leather.png"
        );
        assert_eq!(
            layers[0].dye,
            ArmorDye::Dyeable {
                color_when_undyed: Some(0xA0_65_40)
            }
        );
        assert_eq!(
            layers[1].texture_key,
            "example/textures/entity/equipment/humanoid/overlay.png"
        );
        assert_eq!(layers[1].dye, ArmorDye::None);
    }

    #[test]
    fn equipment_texture_ids_reject_path_traversal() {
        assert_eq!(
            equipment_texture_key("iron", "humanoid"),
            Some("minecraft/textures/entity/equipment/humanoid/iron.png".into())
        );
        assert!(equipment_texture_key("minecraft:../iron", "humanoid").is_none());
        assert!(equipment_texture_key(":iron", "humanoid").is_none());
    }

    use azalea_registry::builtin::EntityKind;

    use super::{EntityRenderInfo, EntityRenderer, PLAYER_MODEL_SCALE};

    #[test]
    fn player_matrix_applies_vanilla_avatar_scale() {
        let player = EntityRenderInfo {
            entity_kind: EntityKind::Player,
            ..Default::default()
        };
        let player_matrix = EntityRenderer::entity_matrix(&player, glam::DVec3::ZERO);
        assert!((player_matrix.x_axis.truncate().length() - PLAYER_MODEL_SCALE).abs() < 1e-6);
        assert!((player_matrix.y_axis.truncate().length() - PLAYER_MODEL_SCALE).abs() < 1e-6);
        assert!((player_matrix.z_axis.truncate().length() - PLAYER_MODEL_SCALE).abs() < 1e-6);

        let zombie = EntityRenderInfo {
            entity_kind: EntityKind::Zombie,
            ..Default::default()
        };
        let zombie_matrix = EntityRenderer::entity_matrix(&zombie, glam::DVec3::ZERO);
        assert!((zombie_matrix.x_axis.truncate().length() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn sleeping_player_matrix_uses_bed_facing_and_standing_eye_offset() {
        use crate::entity::components::Position;
        use crate::world::block::BedDirection;

        let cases = [
            BedDirection::North,
            BedDirection::South,
            BedDirection::West,
            BedDirection::East,
        ];
        for direction in cases {
            let info = EntityRenderInfo {
                position: Position::new(10.0, 64.6875, -4.5),
                is_sleeping: true,
                sleeping_direction: Some(direction),
                sleeping_eye_height: 1.62,
                entity_kind: EntityKind::Player,
                ..Default::default()
            };
            let matrix = EntityRenderer::entity_matrix(&info, glam::DVec3::ZERO);
            let (step_x, step_z) = direction.step();
            let translation = matrix.w_axis.truncate();
            let expected_translation =
                glam::Vec3::new(10.0 - step_x * 1.52, 64.6875, -4.5 - step_z * 1.52);
            assert!(
                translation.abs_diff_eq(expected_translation, 1e-5),
                "sleeping translation mismatch for {direction:?}: {translation:?} != {expected_translation:?}"
            );

            let model_up = matrix.y_axis.truncate().normalize();
            let expected_axis = glam::Vec3::new(step_x, 0.0, step_z);
            assert!(
                model_up.abs_diff_eq(expected_axis, 1e-5),
                "sleeping body axis mismatch for {direction:?}: {model_up:?} != {expected_axis:?}"
            );
        }

        let baby_villager = EntityRenderInfo {
            position: Position::new(0.5, 64.6875, 0.5),
            is_sleeping: true,
            sleeping_direction: Some(BedDirection::South),
            sleeping_eye_height: 0.63,
            entity_kind: EntityKind::Villager,
            is_baby: true,
            ..Default::default()
        };
        let matrix = EntityRenderer::entity_matrix(&baby_villager, glam::DVec3::ZERO);
        assert!(
            matrix
                .w_axis
                .truncate()
                .abs_diff_eq(glam::Vec3::new(0.5, 64.6875, 0.5 - 0.53), 1e-5)
        );
    }

    #[test]
    fn dying_sleeper_takes_the_death_fall_not_the_bed_rotation() {
        use crate::entity::components::Position;
        use crate::world::block::BedDirection;

        let info = EntityRenderInfo {
            position: Position::new(2.0, 3.0, 4.0),
            is_sleeping: true,
            sleeping_direction: Some(BedDirection::North),
            sleeping_eye_height: 1.62,
            death_time: 5.0,
            entity_kind: EntityKind::Player,
            ..Default::default()
        };
        let actual = EntityRenderer::entity_matrix(&info, glam::DVec3::ZERO);
        // The bed offset still applies (`submit`), then only the death fall.
        let expected = glam::Mat4::from_translation(glam::Vec3::new(2.0, 3.0, 4.0 + 1.52))
            * glam::Mat4::from_rotation_z(
                crate::renderer::pipelines::entity_renderer::death_fall_degrees(
                    5.0,
                    EntityKind::Player,
                )
                .to_radians(),
            )
            * glam::Mat4::from_scale(glam::Vec3::splat(PLAYER_MODEL_SCALE));
        for (a, b) in actual
            .to_cols_array()
            .into_iter()
            .zip(expected.to_cols_array())
        {
            assert!((a - b).abs() < 1e-5, "{a} != {b}");
        }
    }

    #[test]
    fn sleeping_matrix_falls_back_to_body_yaw_without_bed_direction() {
        use crate::entity::components::Position;

        let info = EntityRenderInfo {
            position: Position::new(2.0, 3.0, 4.0),
            body_y_rot_deg: 37.0,
            is_sleeping: true,
            sleeping_direction: None,
            entity_kind: EntityKind::Player,
            ..Default::default()
        };
        let actual = EntityRenderer::entity_matrix(&info, glam::DVec3::ZERO);
        let expected = glam::Mat4::from_translation(glam::Vec3::new(2.0, 3.0, 4.0))
            * glam::Mat4::from_rotation_y(37.0_f32.to_radians())
            * glam::Mat4::from_rotation_z(90.0_f32.to_radians())
            * glam::Mat4::from_rotation_y(270.0_f32.to_radians())
            * glam::Mat4::from_scale(glam::Vec3::splat(PLAYER_MODEL_SCALE));
        for (a, b) in actual
            .to_cols_array()
            .into_iter()
            .zip(expected.to_cols_array())
        {
            assert!((a - b).abs() < 1e-5, "{a} != {b}");
        }
    }

    #[test]
    fn death_fall_matches_vanilla_boundaries_and_flip_overrides() {
        use azalea_registry::builtin::EntityKind;

        assert_eq!(super::death_fall_degrees(0.0, EntityKind::Zombie), 0.0);
        assert_eq!(super::death_fall_degrees(1.0, EntityKind::Zombie), 0.0);
        assert!(
            (super::death_fall_degrees(6.0, EntityKind::Zombie) - (0.4_f32.sqrt() * 90.0)).abs()
                < 1e-5
        );
        assert_eq!(super::death_fall_degrees(20.0, EntityKind::Zombie), 90.0);
        assert_eq!(super::death_fall_degrees(200.0, EntityKind::Zombie), 90.0);

        for kind in [
            EntityKind::Spider,
            EntityKind::CaveSpider,
            EntityKind::Endermite,
            EntityKind::Silverfish,
        ] {
            assert_eq!(
                super::death_fall_degrees(20.0, kind),
                180.0,
                "vanilla renderer override must use a 180-degree death flip for {kind:?}"
            );
        }
        for kind in [EntityKind::Squid, EntityKind::GlowSquid] {
            assert_eq!(
                super::death_fall_degrees(20.0, kind),
                0.0,
                "Vanilla SquidRenderer bypasses LivingEntityRenderer.setupRotations for {kind:?}"
            );
        }
    }

    /// Bakes every mob model; `generate_cube_vertices`' UV seam
    /// `debug_assert!` fires for any mesh that straddles its sheet.
    #[test]
    fn all_mob_meshes_bake() {
        super::mob_definitions();
    }
}
