use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

use azalea_block::BlockState;
use azalea_inventory::ItemStackData;
use azalea_inventory::components::{
    ChargedProjectiles, CustomModelData, DyedColor, FireworkExplosion, MapColor, PotionContents,
};
use azalea_registry::Registry as AzaleaRegistry;
use azalea_registry::builtin::{ItemKind, MobEffect, Potion};
use serde::{Deserialize, Serialize};

const BLOCK_CACHE_FILE: &str = "block_cache_v4.json";

use super::block_state_table::BlockStateTable;
use super::json_bundle::JsonBundle;
use super::model;
use super::model::{BakedModel, WeightedModels};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tint {
    None,
    Grass,
    /// Tall grass and large fern sample the lower block's biome for their upper
    /// half.
    DoubleGrass,
    Foliage,
    DryFoliage,
    Water,
    /// Fixed vanilla block tint color (`0xRRGGBB`).
    Constant(u32),
    /// Power-level color, resolved at mesh time from the state's `power`.
    Redstone,
    /// Age-dependent melon/pumpkin stem color.
    Stem,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct FaceTextures {
    pub top: String,
    pub bottom: String,
    pub north: String,
    pub south: String,
    pub east: String,
    pub west: String,
    pub side_overlay: Option<String>,
    pub tint: Tint,
    /// The model's `particle` texture slot (vanilla `getParticleMaterial`),
    /// used for block-break particles.
    #[serde(default)]
    pub particle: Option<String>,
}

impl FaceTextures {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        top: &str,
        bottom: &str,
        north: &str,
        south: &str,
        east: &str,
        west: &str,
        side_overlay: Option<&str>,
        tint: Tint,
    ) -> Self {
        Self {
            top: top.into(),
            bottom: bottom.into(),
            north: north.into(),
            south: south.into(),
            east: east.into(),
            west: west.into(),
            side_overlay: side_overlay.map(Into::into),
            tint,
            particle: None,
        }
    }

    pub fn uniform(name: &str, tint: Tint) -> Self {
        Self::new(name, name, name, name, name, name, None, tint)
    }
}

/// A block state's model lookups, resolved once per state.
struct StateRender {
    model: Option<Arc<WeightedModels>>,
    opaque_full_cube: bool,
    occludes: bool,
    hidden_when_enclosed: bool,
}

#[derive(Default)]
pub struct BlockRenderAssets {
    textures: HashMap<String, FaceTextures>,
    baked: model::BakedModelMap,
    /// Per block-state table slot, every state's resolved model lookups,
    /// indexed by state id; built on first use of the slot. The mesher queries
    /// these for every block and neighbour, so they must not hash or parse.
    state_render: [OnceLock<Box<[StateRender]>>; super::TABLE_SLOTS],
    item_models: HashMap<String, BakedModel>,
    flat_item_textures: std::collections::HashSet<String>,
    flat_item_texture_keys: HashMap<String, Vec<Option<String>>>,
    item_gui_transforms: HashMap<String, model::DisplayTransform>,
    item_ground_transforms: HashMap<String, glam::Mat4>,
    item_tint_sources: HashMap<String, Vec<model::ItemTintSource>>,
}

/// Lightweight view of CPU assets paired with the session's explicitly selected
/// state table.
#[derive(Clone, Copy)]
pub(crate) struct BlockRenderView<'a> {
    assets: &'a BlockRenderAssets,
    block_state_table: BlockStateTable,
}

/// Owned, lightweight view of CPU-asset and state-table snapshot for deferred
/// rendering work. Cloning shares the assets; queries borrow the existing
/// render view.
#[derive(Clone)]
pub(crate) struct OwnedBlockRenderView {
    assets: Arc<BlockRenderAssets>,
    block_state_table: BlockStateTable,
}

impl OwnedBlockRenderView {
    pub(crate) fn block_state_table(&self) -> BlockStateTable {
        self.block_state_table
    }

    pub(crate) fn new(assets: Arc<BlockRenderAssets>, block_state_table: BlockStateTable) -> Self {
        Self {
            assets,
            block_state_table,
        }
    }

    pub(crate) fn as_view(&self) -> BlockRenderView<'_> {
        self.assets.for_version(self.block_state_table)
    }
}

/// The stages of loading the block models and their atlas, each an equal
/// share of the progress.
#[derive(Clone, Copy)]
pub enum LoadStage {
    Files,
    Blockstates,
    Items,
    Textures,
}

impl LoadStage {
    const ALL: [Self; 4] = [Self::Files, Self::Blockstates, Self::Items, Self::Textures];
}

/// How far loading the block models and their atlas has got, for the splash.
#[derive(Default)]
pub struct LoadProgress {
    stage: AtomicU32,
    done: AtomicU32,
    total: AtomicU32,
}

impl LoadProgress {
    pub(crate) fn begin_stage(progress: Option<&Self>, stage: LoadStage, total: usize) {
        if let Some(progress) = progress {
            progress.done.store(0, Ordering::Relaxed);
            progress.total.store(total as u32, Ordering::Relaxed);
            progress.stage.store(stage as u32, Ordering::Relaxed);
        }
    }

    pub(crate) fn step(progress: Option<&Self>) {
        if let Some(progress) = progress {
            progress.done.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn stage(&self) -> LoadStage {
        LoadStage::ALL[self.stage.load(Ordering::Relaxed) as usize]
    }

    pub fn fraction(&self) -> f32 {
        let total = self.total.load(Ordering::Relaxed).max(1);
        let done = self.done.load(Ordering::Relaxed).min(total);
        let stage = self.stage.load(Ordering::Relaxed);
        (stage as f32 + done as f32 / total as f32) / LoadStage::ALL.len() as f32
    }
}

impl BlockRenderAssets {
    pub fn load(
        inputs: &model::ModelInputs,
        cache_dir: Option<&Path>,
        use_vanilla_cache: bool,
        progress: Option<&LoadProgress>,
    ) -> Self {
        // Only vanilla textures are cached, so a cache shared between
        // profiles never carries one profile's resource packs into another.
        let cache_path = cache_dir
            .filter(|_| use_vanilla_cache)
            .map(|dir| dir.join(BLOCK_CACHE_FILE));

        // Parallel reads use selected contents even for custom packs. Never
        // read or write their data through the shared vanilla bundle cache.
        let bundle = JsonBundle::load(inputs, cache_dir.filter(|_| use_vanilla_cache), progress);

        let textures: Option<HashMap<String, FaceTextures>> =
            cache_path.as_deref().and_then(load_cache);
        if let Some(cached) = &textures {
            tracing::info!(
                "Block render assets: {} blocks (cached textures)",
                cached.len()
            );
        }

        let textures = textures.unwrap_or_else(|| {
            let mut textures = model::load_all_block_textures(inputs, &bundle);

            textures
                .entry("water".into())
                .or_insert_with(|| FaceTextures::uniform("water_still", Tint::None));
            textures
                .entry("lava".into())
                .or_insert_with(|| FaceTextures::uniform("lava_still", Tint::None));

            if let Some(path) = &cache_path {
                save_cache(path, &textures);
            }
            tracing::info!("Block render assets: {} blocks (built)", textures.len());
            textures
        });

        LoadProgress::begin_stage(progress, LoadStage::Blockstates, bundle.blockstates().len());
        let baked = model::bake_all_models(inputs, &bundle, progress);
        LoadProgress::begin_stage(progress, LoadStage::Items, bundle.item_names().len());
        let baked_items = model::bake_item_models(inputs, Some(&bundle), progress);
        let item_models = baked_items.models;
        let flat_item_textures = baked_items.generated_textures;
        let flat_item_texture_keys = baked_items.flat_texture_keys;
        let item_gui_transforms = baked_items.gui_transforms;
        let item_ground_transforms = baked_items.ground_transforms;
        let item_tint_sources = baked_items.tint_sources;

        Self {
            textures,
            baked,
            state_render: [const { OnceLock::new() }; super::TABLE_SLOTS],
            item_models,
            flat_item_textures,
            flat_item_texture_keys,
            item_gui_transforms,
            item_ground_transforms,
            item_tint_sources,
        }
    }

    pub(crate) fn for_version(&self, block_state_table: BlockStateTable) -> BlockRenderView<'_> {
        BlockRenderView {
            assets: self,
            block_state_table,
        }
    }

    pub fn get_item_model(&self, name: &str) -> Option<&BakedModel> {
        self.item_models.get(name)
    }

    /// The model `name` draws with for this stack: its loaded variant
    /// (vanilla `Charge` select property) if it bakes one, else `name`.
    pub fn stack_model_key(&self, name: &str, stack: &ItemStackData) -> String {
        let charge = stack
            .get_component::<ChargedProjectiles>()
            .filter(|c| !c.items.is_empty())
            .map(|c| {
                let rocket = c.items.iter().any(|i| i.kind() == ItemKind::FireworkRocket);
                if rocket { "rocket" } else { "arrow" }
            });
        charge
            .map(|charge| model::charge_model_key(name, charge))
            .filter(|key| {
                self.item_models.contains_key(key) || self.flat_item_texture_keys.contains_key(key)
            })
            .unwrap_or_else(|| name.to_string())
    }

    /// Every item with a baked 3D model or a generated flat sprite.
    pub fn item_names(&self) -> impl Iterator<Item = &str> + '_ {
        self.item_models
            .keys()
            .chain(self.flat_item_texture_keys.keys())
            .map(String::as_str)
    }

    pub fn flat_item_textures(&self) -> impl Iterator<Item = &str> + '_ {
        self.flat_item_textures.iter().map(String::as_str)
    }

    /// A flat item's layer textures indexed by layer (`None` = unresolvable),
    /// defaulting to the item's own sprite.
    pub fn flat_item_layers(&self, name: &str) -> Vec<Option<String>> {
        self.flat_item_texture_keys
            .get(name)
            .cloned()
            .unwrap_or_else(|| vec![Some(format!("item/{name}"))])
    }

    /// Evaluate the selected item model's tint-source list for this stack.
    /// Vanilla keeps the full list, and model-face `tintindex` values address
    /// it directly, so resource-pack entries must not be truncated or shifted.
    pub fn item_tint_palette(
        &self,
        name: &str,
        stack: Option<&ItemStackData>,
        team_color: Option<u32>,
    ) -> Vec<u32> {
        self.item_tint_sources
            .get(name)
            .map(|sources| {
                sources
                    .iter()
                    .map(|source| evaluate_item_tint(source, stack, team_color))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn get_item_gui_transform(&self, name: &str) -> model::DisplayTransform {
        self.item_gui_transforms
            .get(name)
            .copied()
            .unwrap_or(model::DisplayTransform::IDENTITY)
    }

    pub fn get_item_ground_transform(&self, name: &str) -> Option<glam::Mat4> {
        self.item_ground_transforms.get(name).copied()
    }

    fn state_render(
        &self,
        block_state_table: BlockStateTable,
        state: BlockState,
    ) -> Option<&StateRender> {
        let slot = block_state_table.table_slot();
        self.state_render[slot]
            .get_or_init(|| {
                block_state_table
                    .table()
                    .iter()
                    .map(|block_state_data| {
                        let model = self
                            .resolve_model(block_state_data.id, &block_state_data.properties)
                            .cloned();
                        // TODO: the occlusion graph reads this for vanilla's
                        // `isSolidRender` (`canOcclude` and a full occlusion shape),
                        // but it still counts the model's quads and ignores
                        // `canOcclude`, so glass counts as solid there.
                        let first = model.as_deref().map(WeightedModels::first);
                        let opaque_full_cube =
                            !block_state_data.is_air && first.is_some_and(|m| m.is_full_cube);
                        let occludes = block_state_data.occludes_neighbor_faces();
                        let all_quads_cull =
                            first.is_some_and(|m| m.quads.iter().all(|q| q.cullface.is_some()));
                        StateRender {
                            model,
                            opaque_full_cube,
                            occludes,
                            hidden_when_enclosed: occludes
                                && all_quads_cull
                                && matches!(block_state_data.fluid.kind, super::FluidKind::Empty),
                        }
                    })
                    .collect()
            })
            .get(u32::from(state) as usize)
    }

    fn resolve_model(&self, block: &str, props: &super::PropMap) -> Option<&Arc<WeightedModels>> {
        let variants = &self.blockstate_layer(block, props)?.variants;

        if variants.len() == 1 {
            return variants.values().next();
        }

        // Vanilla variant keys only list the properties that affect the model, so
        // match by subset rather than exact string equality (an empty key matches
        // any state, serving as the default variant).
        variants
            .iter()
            .find(|(key, _)| {
                constraints_match(props, key.split(',').filter_map(|p| p.split_once('=')))
            })
            .map(|(_, model)| model)
            .or_else(|| variants.values().next())
    }

    fn blockstate_layer(
        &self,
        block: &str,
        props: &super::PropMap,
    ) -> Option<&model::BakedBlockstate> {
        let layers = self.baked.get(block)?;
        // Vanilla merges instantiated states, not JSON keys: a higher partial
        // variant definition leaves unmatched states in the lower layer.
        layers
            .iter()
            .rev()
            .find(|layer| {
                !layer.multipart.is_empty()
                    || layer.variants.keys().any(|key| {
                        constraints_match(props, key.split(',').filter_map(|p| p.split_once('=')))
                    })
            })
            .or_else(|| layers.first())
    }

    pub fn texture_names(&self) -> impl Iterator<Item = &str> + '_ {
        let face_textures = self.textures.values().flat_map(|ft| {
            let base = [
                &ft.top, &ft.bottom, &ft.north, &ft.south, &ft.east, &ft.west,
            ];
            base.into_iter()
                .map(|s| s.as_str())
                .chain(ft.side_overlay.as_deref())
        });

        let models = self
            .baked
            .values()
            .flatten()
            .flat_map(|layer| {
                layer
                    .variants
                    .values()
                    .flat_map(|models| models.models())
                    .chain(
                        layer
                            .multipart
                            .iter()
                            .flat_map(|entry| entry.models.models()),
                    )
            })
            .chain(self.item_models.values());
        let quad_textures =
            models.flat_map(|model| model.quads.iter().map(|quad| quad.texture.name()));

        face_textures.chain(quad_textures)
    }
}

impl<'a> BlockRenderView<'a> {
    pub(crate) fn block_state_table(self) -> BlockStateTable {
        self.block_state_table
    }

    pub(crate) fn get_textures(self, state: BlockState) -> Option<&'a FaceTextures> {
        self.assets
            .textures
            .get(self.block_state_table.block_state_data(state).id)
    }

    /// The state's variant model as vanilla `ModelBlockRenderer` picks it for
    /// the block at `pos`, or the first of a weighted list without one.
    pub(crate) fn get_baked_model(
        self,
        state: BlockState,
        pos: Option<[i32; 3]>,
    ) -> Option<&'a BakedModel> {
        let models = self
            .assets
            .state_render(self.block_state_table, state)?
            .model
            .as_deref()?;
        Some(models.pick_at(pos, position_random))
    }

    /// Vanilla `MultiPartModel.collectParts`: every matching case reseeded
    /// from one draw, so each picks as if alone. The draw waits for the first
    /// weighted case, as a single model's pick takes none.
    pub(crate) fn get_multipart_quads(
        self,
        state: BlockState,
        pos: Option<[i32; 3]>,
    ) -> Option<Vec<&'a model::BakedQuad>> {
        let block_state_data = self.block_state_table.block_state_data(state);
        let entries = &self
            .assets
            .blockstate_layer(block_state_data.id, &block_state_data.properties)?
            .multipart;
        let mut seed = None;
        let mut quads = Vec::new();
        for entry in entries {
            if entry.when.matches(&block_state_data.properties) {
                let model = entry.models.pick_at(pos, |pos| {
                    let seed = *seed.get_or_insert_with(|| position_random(pos).next_long());
                    crate::util::JavaRandom::new(seed)
                });
                quads.extend(model.quads.iter());
            }
        }
        if quads.is_empty() { None } else { Some(quads) }
    }

    pub(crate) fn is_opaque_full_cube(self, state: BlockState) -> bool {
        self.assets
            .state_render(self.block_state_table, state)
            .is_some_and(|r| r.opaque_full_cube)
    }

    /// Full cubes that can occlude, excluding glass and other no-occlusion
    /// cubes.
    pub(crate) fn occludes_neighbor(self, state: BlockState) -> bool {
        self.assets
            .state_render(self.block_state_table, state)
            .is_some_and(|r| r.occludes)
    }

    /// Non-fluid occluders whose every quad has a cullface.
    pub(crate) fn hidden_when_enclosed(self, state: BlockState) -> bool {
        self.assets
            .state_render(self.block_state_table, state)
            .is_some_and(|r| r.hidden_when_enclosed)
    }
}

/// Vanilla `ModelBlockRenderer`'s random, seeded by `BlockState.getSeed`.
// TODO: doors, double plants and beds override `getSeed` to share one seed
// between their halves; no vanilla blockstate of theirs is weighted, so only
// a resource pack's would differ.
#[inline]
fn position_random([x, y, z]: [i32; 3]) -> crate::util::JavaRandom {
    crate::util::JavaRandom::new(super::position_seed(x, y, z))
}

fn opaque_rgb(color: u32) -> u32 {
    color | 0xFF00_0000
}

fn evaluate_item_tint(
    source: &model::ItemTintSource,
    stack: Option<&ItemStackData>,
    team_color: Option<u32>,
) -> u32 {
    match *source {
        model::ItemTintSource::Constant(color) | model::ItemTintSource::Grass { color } => color,
        // Vanilla preserves the configured default alpha for dye/firework, but
        // opacifies component colors.
        model::ItemTintSource::Dye { default } => stack
            .and_then(|stack| stack.get_component::<DyedColor>())
            .map(|color| opaque_rgb(color.rgb as u32))
            .unwrap_or(default),
        model::ItemTintSource::Firework { default } => stack
            .and_then(|stack| stack.get_component::<FireworkExplosion>())
            .and_then(|explosion| average_rgb(&explosion.colors))
            .unwrap_or(default),
        model::ItemTintSource::Potion { default } => stack
            .and_then(|stack| stack.get_component::<PotionContents>())
            .map(|contents| potion_contents_color(&contents, default))
            .unwrap_or_else(|| opaque_rgb(default)),
        model::ItemTintSource::MapColor { default } => stack
            .and_then(|stack| stack.get_component::<MapColor>())
            .map(|color| opaque_rgb(color.color as u32))
            .unwrap_or_else(|| opaque_rgb(default)),
        model::ItemTintSource::CustomModelData { index, default } => stack
            .and_then(|stack| stack.get_component::<CustomModelData>())
            .and_then(|data| data.colors.get(index).copied())
            .map(|color| opaque_rgb(color as u32))
            .unwrap_or_else(|| opaque_rgb(default)),
        model::ItemTintSource::Team { default } => opaque_rgb(team_color.unwrap_or(default)),
    }
}

fn average_rgb(colors: &[i32]) -> Option<u32> {
    if colors.is_empty() {
        return None;
    }
    if colors.len() == 1 {
        return Some(opaque_rgb(colors[0] as u32));
    }
    let mut red = 0_u32;
    let mut green = 0_u32;
    let mut blue = 0_u32;
    for &color in colors {
        let color = color as u32;
        red += (color >> 16) & 0xFF;
        green += (color >> 8) & 0xFF;
        blue += color & 0xFF;
    }
    let count = colors.len() as u32;
    Some(opaque_rgb(
        ((red / count) << 16) | ((green / count) << 8) | (blue / count),
    ))
}

fn potion_contents_color(contents: &PotionContents, default: u32) -> u32 {
    if let Some(color) = contents.custom_color {
        return opaque_rgb(color as u32);
    }

    let mut effects: Vec<(u32, u32)> = Vec::new();
    if let Some(potion) = contents.potion {
        potion_effect_colors(potion, &mut effects);
    }
    for effect in &contents.custom_effects {
        if effect.details.show_particles {
            effects.push((
                mob_effect_color(effect.id),
                (effect.details.amplifier.max(0) as u32) + 1,
            ));
        }
    }
    opaque_rgb(weighted_effect_color(&effects).unwrap_or(default))
}

fn weighted_effect_color(effects: &[(u32, u32)]) -> Option<u32> {
    let mut red = 0_u64;
    let mut green = 0_u64;
    let mut blue = 0_u64;
    let mut weight_sum = 0_u64;
    for &(color, weight) in effects {
        let weight = weight as u64;
        red += ((color >> 16) & 0xFF) as u64 * weight;
        green += ((color >> 8) & 0xFF) as u64 * weight;
        blue += (color & 0xFF) as u64 * weight;
        weight_sum += weight;
    }
    let red = red.checked_div(weight_sum)? as u32;
    let green = green.checked_div(weight_sum)? as u32;
    let blue = blue.checked_div(weight_sum)? as u32;
    Some((red << 16) | (green << 8) | blue)
}

fn potion_effect_colors(potion: Potion, out: &mut Vec<(u32, u32)>) {
    use Potion::*;
    let one = |out: &mut Vec<(u32, u32)>, effect, amplifier: u32| {
        out.push((mob_effect_color(effect), amplifier + 1));
    };
    match potion {
        Water | Mundane | Thick | Awkward => {}
        NightVision | LongNightVision => one(out, MobEffect::NightVision, 0),
        Invisibility | LongInvisibility => one(out, MobEffect::Invisibility, 0),
        Leaping | LongLeaping => one(out, MobEffect::JumpBoost, 0),
        StrongLeaping => one(out, MobEffect::JumpBoost, 1),
        FireResistance | LongFireResistance => one(out, MobEffect::FireResistance, 0),
        Swiftness | LongSwiftness => one(out, MobEffect::Speed, 0),
        StrongSwiftness => one(out, MobEffect::Speed, 1),
        Slowness | LongSlowness => one(out, MobEffect::Slowness, 0),
        StrongSlowness => one(out, MobEffect::Slowness, 3),
        TurtleMaster | LongTurtleMaster => {
            one(out, MobEffect::Slowness, 3);
            one(out, MobEffect::Resistance, 2);
        }
        StrongTurtleMaster => {
            one(out, MobEffect::Slowness, 5);
            one(out, MobEffect::Resistance, 3);
        }
        WaterBreathing | LongWaterBreathing => one(out, MobEffect::WaterBreathing, 0),
        Healing => one(out, MobEffect::InstantHealth, 0),
        StrongHealing => one(out, MobEffect::InstantHealth, 1),
        Harming => one(out, MobEffect::InstantDamage, 0),
        StrongHarming => one(out, MobEffect::InstantDamage, 1),
        Poison | LongPoison => one(out, MobEffect::Poison, 0),
        StrongPoison => one(out, MobEffect::Poison, 1),
        Regeneration | LongRegeneration => one(out, MobEffect::Regeneration, 0),
        StrongRegeneration => one(out, MobEffect::Regeneration, 1),
        Strength | LongStrength => one(out, MobEffect::Strength, 0),
        StrongStrength => one(out, MobEffect::Strength, 1),
        Weakness | LongWeakness => one(out, MobEffect::Weakness, 0),
        Luck => one(out, MobEffect::Luck, 0),
        SlowFalling | LongSlowFalling => one(out, MobEffect::SlowFalling, 0),
        WindCharged => one(out, MobEffect::WindCharged, 0),
        Weaving => one(out, MobEffect::Weaving, 0),
        Oozing => one(out, MobEffect::Oozing, 0),
        Infested => one(out, MobEffect::Infested, 0),
    }
}

fn mob_effect_color(effect: MobEffect) -> u32 {
    crate::mob_effect::info(effect.to_u32()).map_or(0xFFFFFF, |info| info.color)
}

/// Whether every `key=value` constraint holds for `props`. A value may list
/// alternatives separated by `|`, as vanilla multipart `when` clauses do.
fn constraints_match<'a>(
    props: &super::PropMap,
    mut constraints: impl Iterator<Item = (&'a str, &'a str)>,
) -> bool {
    constraints.all(|(k, v)| {
        props
            .get(k)
            .is_some_and(|pv| v.split('|').any(|opt| opt == pv))
    })
}

pub(super) fn load_cache<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let data = std::fs::read(path).ok()?;
    serde_json::from_slice(&data).ok()
}

/// Atomic, since every client of a version shares the one cache.
pub(super) fn save_cache<T: Serialize>(path: &Path, value: &T) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec(value)
        && let Err(e) = crate::util::write_atomic(path, &json)
    {
        tracing::warn!("Failed to write {}: {e}", path.display());
    }
}

#[cfg(test)]
mod load_progress_tests {
    use super::{LoadProgress, LoadStage};

    #[test]
    fn each_stage_fills_an_equal_share() {
        let progress = LoadProgress::default();
        assert_eq!(progress.fraction(), 0.0);
        LoadProgress::begin_stage(Some(&progress), LoadStage::Blockstates, 4);
        LoadProgress::step(Some(&progress));
        assert_eq!(progress.fraction(), (1.0 + 0.25) / 4.0);
        // Steps past the total don't run into the next stage.
        for _ in 0..10 {
            LoadProgress::step(Some(&progress));
        }
        assert_eq!(progress.fraction(), 2.0 / 4.0);
        LoadProgress::begin_stage(Some(&progress), LoadStage::Items, 0);
        assert_eq!(progress.fraction(), 2.0 / 4.0);
        assert!(matches!(progress.stage(), LoadStage::Items));
    }
}

#[cfg(test)]
mod tint_tests {
    use azalea_inventory::ItemStack;
    use azalea_inventory::components::CustomModelData;

    use super::*;

    #[test]
    fn custom_model_data_tint_uses_requested_color_index_and_default() {
        let stack = ItemStack::from(ItemKind::Stone).with_component(CustomModelData {
            floats: vec![],
            flags: vec![],
            strings: vec![],
            colors: vec![0x112233, 0x445566, 0x778899],
        });
        let stack = stack.as_present().unwrap();

        assert_eq!(
            evaluate_item_tint(
                &model::ItemTintSource::CustomModelData {
                    index: 1,
                    default: 0xABCDEF,
                },
                Some(stack),
                None,
            ),
            0xFF445566
        );
        assert_eq!(
            evaluate_item_tint(
                &model::ItemTintSource::CustomModelData {
                    index: 9,
                    default: 0xABCDEF,
                },
                Some(stack),
                None,
            ),
            0xFFABCDEF
        );
    }

    #[test]
    fn team_tint_uses_owner_team_color_or_default() {
        let source = model::ItemTintSource::Team { default: 0x123456 };
        assert_eq!(
            evaluate_item_tint(&source, None, Some(0xABCDEF)),
            0xFFABCDEF
        );
        assert_eq!(evaluate_item_tint(&source, None, None), 0xFF123456);
    }

    #[test]
    fn source_specific_alpha_matches_vanilla_fallback_rules() {
        let translucent = 0x80112233;
        assert_eq!(
            evaluate_item_tint(
                &model::ItemTintSource::Dye {
                    default: translucent
                },
                None,
                None,
            ),
            translucent
        );
        assert_eq!(
            evaluate_item_tint(
                &model::ItemTintSource::Firework {
                    default: translucent
                },
                None,
                None,
            ),
            translucent
        );
        for source in [
            model::ItemTintSource::Potion {
                default: translucent,
            },
            model::ItemTintSource::MapColor {
                default: translucent,
            },
            model::ItemTintSource::CustomModelData {
                index: 0,
                default: translucent,
            },
            model::ItemTintSource::Team {
                default: translucent,
            },
        ] {
            assert_eq!(evaluate_item_tint(&source, None, None), 0xFF112233);
        }
    }
}
