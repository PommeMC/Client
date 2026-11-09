//! Pure recipe-display resolution; no rendering primitives.
use std::collections::HashSet;

use azalea_inventory::components::{
    self, DataComponentTrait, Dye, DyeColor, DyedColor, EncodableDataComponent, PotionContents,
    ProvidesTrimMaterial, Trim,
};
use azalea_inventory::default_components::get_default_component;
use azalea_inventory::{ItemStack, ItemStackData};
use azalea_registry::builtin::{DataComponentKind, ItemKind, Potion};
use azalea_registry::{DataRegistry, Holder, Registry};

use crate::recipe::{
    Ingredient, ItemTags, RecipeBookState, RecipeDisplay, SlotDisplay, TrimPatternHolder,
};
pub(crate) fn ingredient_items<'a>(
    ingredient: &'a Ingredient,
    tags: &'a ItemTags,
) -> Box<dyn Iterator<Item = u32> + 'a> {
    match ingredient {
        Ingredient::Items(items) => Box::new(items.iter().copied()),
        Ingredient::Tag(tag) => Box::new(
            tags.get(tag)
                .into_iter()
                .flat_map(|items| items.iter().copied()),
        ),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ResolvedDirectTrim {
    pub(crate) material:
        Holder<azalea_registry::data::TrimMaterial, components::DirectTrimMaterial>,
    pub(crate) pattern: TrimPatternHolder,
}

#[derive(Clone, Debug)]
pub(crate) struct ResolvedSlotStack {
    pub(crate) stack: ItemStackData,
    pub(crate) direct_trim: Option<ResolvedDirectTrim>,
}

impl ResolvedSlotStack {
    pub(crate) fn plain(stack: ItemStackData) -> Self {
        Self {
            stack,
            direct_trim: None,
        }
    }

    pub(crate) fn same_item_and_components(&self, other: &Self) -> bool {
        self.stack.is_same_item_and_components(&other.stack)
            && self.direct_trim == other.direct_trim
    }
}

pub(crate) fn display_result_stacks(
    display: &RecipeDisplay,
    book: &RecipeBookState,
) -> Vec<ResolvedSlotStack> {
    let result = match display {
        RecipeDisplay::Shapeless { result, .. }
        | RecipeDisplay::Shaped { result, .. }
        | RecipeDisplay::Furnace { result, .. }
        | RecipeDisplay::Stonecutter { result, .. }
        | RecipeDisplay::Smithing { result, .. } => result,
    };
    slot_stacks(result, book)
}

fn with_component<T>(stack: ItemStackData, component: T) -> ItemStackData
where
    T: DataComponentTrait + EncodableDataComponent,
{
    match ItemStack::Present(stack).with_component(component) {
        ItemStack::Present(stack) => stack,
        ItemStack::Empty => unreachable!("a positive recipe-display stack cannot become empty"),
    }
}

fn without_component<T: DataComponentTrait>(mut stack: ItemStackData) -> ItemStackData {
    // SAFETY: `None` represents an explicit removal, so there is no union value
    // whose runtime type needs to match the component kind.
    unsafe {
        stack
            .component_patch
            .unchecked_insert_component(T::KIND, None)
    };
    stack
}

fn stack_has_component_kind(stack: &ItemStackData, kind: DataComponentKind) -> bool {
    if let Some((_, value)) = stack
        .component_patch
        .iter()
        .find(|(patched_kind, _)| *patched_kind == kind)
    {
        // An explicit removal overrides an item default, while an explicit
        // value always satisfies OnlyWithComponent.
        return value.is_some();
    }

    match kind {
        DataComponentKind::CustomData => {
            get_default_component::<components::CustomData>(stack.kind).is_some()
        }
        DataComponentKind::MaxStackSize => {
            get_default_component::<components::MaxStackSize>(stack.kind).is_some()
        }
        DataComponentKind::MaxDamage => {
            get_default_component::<components::MaxDamage>(stack.kind).is_some()
        }
        DataComponentKind::Damage => {
            get_default_component::<components::Damage>(stack.kind).is_some()
        }
        DataComponentKind::Unbreakable => {
            get_default_component::<components::Unbreakable>(stack.kind).is_some()
        }
        DataComponentKind::CustomName => {
            get_default_component::<components::CustomName>(stack.kind).is_some()
        }
        DataComponentKind::ItemName => {
            get_default_component::<components::ItemName>(stack.kind).is_some()
        }
        DataComponentKind::ItemModel => {
            get_default_component::<components::ItemModel>(stack.kind).is_some()
        }
        DataComponentKind::Lore => get_default_component::<components::Lore>(stack.kind).is_some(),
        DataComponentKind::Rarity => {
            get_default_component::<components::Rarity>(stack.kind).is_some()
        }
        DataComponentKind::Enchantments => {
            get_default_component::<components::Enchantments>(stack.kind).is_some()
        }
        DataComponentKind::CanPlaceOn => {
            get_default_component::<components::CanPlaceOn>(stack.kind).is_some()
        }
        DataComponentKind::CanBreak => {
            get_default_component::<components::CanBreak>(stack.kind).is_some()
        }
        DataComponentKind::AttributeModifiers => {
            get_default_component::<components::AttributeModifiers>(stack.kind).is_some()
        }
        DataComponentKind::CustomModelData => {
            get_default_component::<components::CustomModelData>(stack.kind).is_some()
        }
        DataComponentKind::TooltipDisplay => {
            get_default_component::<components::TooltipDisplay>(stack.kind).is_some()
        }
        DataComponentKind::RepairCost => {
            get_default_component::<components::RepairCost>(stack.kind).is_some()
        }
        DataComponentKind::CreativeSlotLock => {
            get_default_component::<components::CreativeSlotLock>(stack.kind).is_some()
        }
        DataComponentKind::EnchantmentGlintOverride => {
            get_default_component::<components::EnchantmentGlintOverride>(stack.kind).is_some()
        }
        DataComponentKind::IntangibleProjectile => {
            get_default_component::<components::IntangibleProjectile>(stack.kind).is_some()
        }
        DataComponentKind::Food => get_default_component::<components::Food>(stack.kind).is_some(),
        DataComponentKind::Consumable => {
            get_default_component::<components::Consumable>(stack.kind).is_some()
        }
        DataComponentKind::UseRemainder => {
            get_default_component::<components::UseRemainder>(stack.kind).is_some()
        }
        DataComponentKind::UseCooldown => {
            get_default_component::<components::UseCooldown>(stack.kind).is_some()
        }
        DataComponentKind::DamageResistant => {
            get_default_component::<components::DamageResistant>(stack.kind).is_some()
        }
        DataComponentKind::Tool => get_default_component::<components::Tool>(stack.kind).is_some(),
        DataComponentKind::Weapon => {
            get_default_component::<components::Weapon>(stack.kind).is_some()
        }
        DataComponentKind::Enchantable => {
            get_default_component::<components::Enchantable>(stack.kind).is_some()
        }
        DataComponentKind::Equippable => {
            get_default_component::<components::Equippable>(stack.kind).is_some()
        }
        DataComponentKind::Repairable => {
            get_default_component::<components::Repairable>(stack.kind).is_some()
        }
        DataComponentKind::Glider => {
            get_default_component::<components::Glider>(stack.kind).is_some()
        }
        DataComponentKind::TooltipStyle => {
            get_default_component::<components::TooltipStyle>(stack.kind).is_some()
        }
        DataComponentKind::DeathProtection => {
            get_default_component::<components::DeathProtection>(stack.kind).is_some()
        }
        DataComponentKind::BlocksAttacks => {
            get_default_component::<components::BlocksAttacks>(stack.kind).is_some()
        }
        DataComponentKind::StoredEnchantments => {
            get_default_component::<components::StoredEnchantments>(stack.kind).is_some()
        }
        DataComponentKind::DyedColor => {
            get_default_component::<components::DyedColor>(stack.kind).is_some()
        }
        DataComponentKind::MapColor => {
            get_default_component::<components::MapColor>(stack.kind).is_some()
        }
        DataComponentKind::MapId => {
            get_default_component::<components::MapId>(stack.kind).is_some()
        }
        DataComponentKind::MapDecorations => {
            get_default_component::<components::MapDecorations>(stack.kind).is_some()
        }
        DataComponentKind::MapPostProcessing => {
            get_default_component::<components::MapPostProcessing>(stack.kind).is_some()
        }
        DataComponentKind::ChargedProjectiles => {
            get_default_component::<components::ChargedProjectiles>(stack.kind).is_some()
        }
        DataComponentKind::BundleContents => {
            get_default_component::<components::BundleContents>(stack.kind).is_some()
        }
        DataComponentKind::PotionContents => {
            get_default_component::<components::PotionContents>(stack.kind).is_some()
        }
        DataComponentKind::PotionDurationScale => {
            get_default_component::<components::PotionDurationScale>(stack.kind).is_some()
        }
        DataComponentKind::SuspiciousStewEffects => {
            get_default_component::<components::SuspiciousStewEffects>(stack.kind).is_some()
        }
        DataComponentKind::WritableBookContent => {
            get_default_component::<components::WritableBookContent>(stack.kind).is_some()
        }
        DataComponentKind::WrittenBookContent => {
            get_default_component::<components::WrittenBookContent>(stack.kind).is_some()
        }
        DataComponentKind::Trim => get_default_component::<components::Trim>(stack.kind).is_some(),
        DataComponentKind::DebugStickState => {
            get_default_component::<components::DebugStickState>(stack.kind).is_some()
        }
        DataComponentKind::EntityData => {
            get_default_component::<components::EntityData>(stack.kind).is_some()
        }
        DataComponentKind::BucketEntityData => {
            get_default_component::<components::BucketEntityData>(stack.kind).is_some()
        }
        DataComponentKind::BlockEntityData => {
            get_default_component::<components::BlockEntityData>(stack.kind).is_some()
        }
        DataComponentKind::Instrument => {
            get_default_component::<components::Instrument>(stack.kind).is_some()
        }
        DataComponentKind::ProvidesTrimMaterial => {
            get_default_component::<components::ProvidesTrimMaterial>(stack.kind).is_some()
        }
        DataComponentKind::OminousBottleAmplifier => {
            get_default_component::<components::OminousBottleAmplifier>(stack.kind).is_some()
        }
        DataComponentKind::JukeboxPlayable => {
            get_default_component::<components::JukeboxPlayable>(stack.kind).is_some()
        }
        DataComponentKind::ProvidesBannerPatterns => {
            get_default_component::<components::ProvidesBannerPatterns>(stack.kind).is_some()
        }
        DataComponentKind::Recipes => {
            get_default_component::<components::Recipes>(stack.kind).is_some()
        }
        DataComponentKind::LodestoneTracker => {
            get_default_component::<components::LodestoneTracker>(stack.kind).is_some()
        }
        DataComponentKind::FireworkExplosion => {
            get_default_component::<components::FireworkExplosion>(stack.kind).is_some()
        }
        DataComponentKind::Fireworks => {
            get_default_component::<components::Fireworks>(stack.kind).is_some()
        }
        DataComponentKind::Profile => {
            get_default_component::<components::Profile>(stack.kind).is_some()
        }
        DataComponentKind::NoteBlockSound => {
            get_default_component::<components::NoteBlockSound>(stack.kind).is_some()
        }
        DataComponentKind::BannerPatterns => {
            get_default_component::<components::BannerPatterns>(stack.kind).is_some()
        }
        DataComponentKind::BaseColor => {
            get_default_component::<components::BaseColor>(stack.kind).is_some()
        }
        DataComponentKind::PotDecorations => {
            get_default_component::<components::PotDecorations>(stack.kind).is_some()
        }
        DataComponentKind::Container => {
            get_default_component::<components::Container>(stack.kind).is_some()
        }
        DataComponentKind::BlockState => {
            get_default_component::<components::BlockState>(stack.kind).is_some()
        }
        DataComponentKind::Bees => get_default_component::<components::Bees>(stack.kind).is_some(),
        DataComponentKind::Lock => get_default_component::<components::Lock>(stack.kind).is_some(),
        DataComponentKind::ContainerLoot => {
            get_default_component::<components::ContainerLoot>(stack.kind).is_some()
        }
        DataComponentKind::BreakSound => {
            get_default_component::<components::BreakSound>(stack.kind).is_some()
        }
        DataComponentKind::VillagerVariant => {
            get_default_component::<components::VillagerVariant>(stack.kind).is_some()
        }
        DataComponentKind::WolfVariant => {
            get_default_component::<components::WolfVariant>(stack.kind).is_some()
        }
        DataComponentKind::WolfSoundVariant => {
            get_default_component::<components::WolfSoundVariant>(stack.kind).is_some()
        }
        DataComponentKind::WolfCollar => {
            get_default_component::<components::WolfCollar>(stack.kind).is_some()
        }
        DataComponentKind::FoxVariant => {
            get_default_component::<components::FoxVariant>(stack.kind).is_some()
        }
        DataComponentKind::SalmonSize => {
            get_default_component::<components::SalmonSize>(stack.kind).is_some()
        }
        DataComponentKind::ParrotVariant => {
            get_default_component::<components::ParrotVariant>(stack.kind).is_some()
        }
        DataComponentKind::TropicalFishPattern => {
            get_default_component::<components::TropicalFishPattern>(stack.kind).is_some()
        }
        DataComponentKind::TropicalFishBaseColor => {
            get_default_component::<components::TropicalFishBaseColor>(stack.kind).is_some()
        }
        DataComponentKind::TropicalFishPatternColor => {
            get_default_component::<components::TropicalFishPatternColor>(stack.kind).is_some()
        }
        DataComponentKind::MooshroomVariant => {
            get_default_component::<components::MooshroomVariant>(stack.kind).is_some()
        }
        DataComponentKind::RabbitVariant => {
            get_default_component::<components::RabbitVariant>(stack.kind).is_some()
        }
        DataComponentKind::PigVariant => {
            get_default_component::<components::PigVariant>(stack.kind).is_some()
        }
        DataComponentKind::CowVariant => {
            get_default_component::<components::CowVariant>(stack.kind).is_some()
        }
        DataComponentKind::ChickenVariant => {
            get_default_component::<components::ChickenVariant>(stack.kind).is_some()
        }
        DataComponentKind::FrogVariant => {
            get_default_component::<components::FrogVariant>(stack.kind).is_some()
        }
        DataComponentKind::HorseVariant => {
            get_default_component::<components::HorseVariant>(stack.kind).is_some()
        }
        DataComponentKind::PaintingVariant => {
            get_default_component::<components::PaintingVariant>(stack.kind).is_some()
        }
        DataComponentKind::LlamaVariant => {
            get_default_component::<components::LlamaVariant>(stack.kind).is_some()
        }
        DataComponentKind::AxolotlVariant => {
            get_default_component::<components::AxolotlVariant>(stack.kind).is_some()
        }
        DataComponentKind::CatVariant => {
            get_default_component::<components::CatVariant>(stack.kind).is_some()
        }
        DataComponentKind::CatCollar => {
            get_default_component::<components::CatCollar>(stack.kind).is_some()
        }
        DataComponentKind::SheepColor => {
            get_default_component::<components::SheepColor>(stack.kind).is_some()
        }
        DataComponentKind::ShulkerColor => {
            get_default_component::<components::ShulkerColor>(stack.kind).is_some()
        }
        DataComponentKind::UseEffects => {
            get_default_component::<components::UseEffects>(stack.kind).is_some()
        }
        DataComponentKind::MinimumAttackCharge => {
            get_default_component::<components::MinimumAttackCharge>(stack.kind).is_some()
        }
        DataComponentKind::DamageType => {
            get_default_component::<components::DamageType>(stack.kind).is_some()
        }
        DataComponentKind::PiercingWeapon => {
            get_default_component::<components::PiercingWeapon>(stack.kind).is_some()
        }
        DataComponentKind::KineticWeapon => {
            get_default_component::<components::KineticWeapon>(stack.kind).is_some()
        }
        DataComponentKind::SwingAnimation => {
            get_default_component::<components::SwingAnimation>(stack.kind).is_some()
        }
        DataComponentKind::ZombieNautilusVariant => {
            get_default_component::<components::ZombieNautilusVariant>(stack.kind).is_some()
        }
        DataComponentKind::AttackRange => {
            get_default_component::<components::AttackRange>(stack.kind).is_some()
        }
        DataComponentKind::AdditionalTradeCost => {
            get_default_component::<components::AdditionalTradeCost>(stack.kind).is_some()
        }
        DataComponentKind::Dye => get_default_component::<components::Dye>(stack.kind).is_some(),
        DataComponentKind::PigSoundVariant => {
            get_default_component::<components::PigSoundVariant>(stack.kind).is_some()
        }
        DataComponentKind::CowSoundVariant => {
            get_default_component::<components::CowSoundVariant>(stack.kind).is_some()
        }
        DataComponentKind::ChickenSoundVariant => {
            get_default_component::<components::ChickenSoundVariant>(stack.kind).is_some()
        }
        DataComponentKind::CatSoundVariant => {
            get_default_component::<components::CatSoundVariant>(stack.kind).is_some()
        }
        DataComponentKind::SulfurCubeContent => {
            get_default_component::<components::SulfurCubeContent>(stack.kind).is_some()
        }
    }
}

fn dye_texture_rgb(color: DyeColor) -> i32 {
    match color {
        DyeColor::White => 0xF9FFFE,
        DyeColor::Orange => 16_351_261,
        DyeColor::Magenta => 13_061_821,
        DyeColor::LightBlue => 3_847_130,
        DyeColor::Yellow => 16_701_501,
        DyeColor::Lime => 8_439_583,
        DyeColor::Pink => 15_961_002,
        DyeColor::Gray => 4_673_362,
        DyeColor::LightGray => 0x9D9D97,
        DyeColor::Cyan => 1_481_884,
        DyeColor::Purple => 8_991_416,
        DyeColor::Blue => 3_949_738,
        DyeColor::Brown => 8_606_770,
        DyeColor::Green => 6_192_150,
        DyeColor::Red => 11_546_150,
        DyeColor::Black => 0x1D1D21,
    }
}

fn rgb_channels(rgb: i32) -> (i32, i32, i32) {
    ((rgb >> 16) & 0xff, (rgb >> 8) & 0xff, rgb & 0xff)
}

fn apply_dye(mut target: ResolvedSlotStack, dye: DyeColor) -> ResolvedSlotStack {
    let mut red_total = 0;
    let mut green_total = 0;
    let mut blue_total = 0;
    let mut intensity_total = 0;
    let mut color_count = 0;

    if let Some(existing) = target.stack.get_component::<DyedColor>() {
        let (red, green, blue) = rgb_channels(existing.rgb);
        intensity_total += red.max(green).max(blue);
        red_total += red;
        green_total += green;
        blue_total += blue;
        color_count += 1;
    }

    let (red, green, blue) = rgb_channels(dye_texture_rgb(dye));
    intensity_total += red.max(green).max(blue);
    red_total += red;
    green_total += green;
    blue_total += blue;
    color_count += 1;

    let mut red = red_total / color_count;
    let mut green = green_total / color_count;
    let mut blue = blue_total / color_count;
    let average_intensity = intensity_total as f32 / color_count as f32;
    let result_intensity = red.max(green).max(blue) as f32;
    if result_intensity > 0.0 {
        red = (red as f32 * average_intensity / result_intensity) as i32;
        green = (green as f32 * average_intensity / result_intensity) as i32;
        blue = (blue as f32 * average_intensity / result_intensity) as i32;
    }

    target.stack.count = 1;
    target.stack = with_component(
        target.stack,
        DyedColor {
            rgb: (red << 16) | (green << 8) | blue,
        },
    );
    target
}

fn apply_trim(
    mut base: ResolvedSlotStack,
    material: &ResolvedSlotStack,
    pattern: &TrimPatternHolder,
) -> Option<ResolvedSlotStack> {
    let material = material.stack.get_component::<ProvidesTrimMaterial>()?;
    let resolved = ResolvedDirectTrim {
        material: material.value.clone(),
        pattern: pattern.clone(),
    };

    if let (Holder::Reference(material), TrimPatternHolder::Reference(pattern)) =
        (&resolved.material, &resolved.pattern)
    {
        let trim = Trim {
            material: *material,
            pattern: azalea_registry::data::TrimPattern::new_raw(*pattern),
        };
        if base.direct_trim.is_none()
            && base.stack.get_component::<Trim>().as_deref() == Some(&trim)
        {
            return None;
        }
        base.stack.count = 1;
        base.stack = with_component(base.stack, trim);
        base.direct_trim = None;
        return Some(base);
    }

    if base.direct_trim.as_ref() == Some(&resolved) {
        return None;
    }
    base.stack.count = 1;
    base.stack = without_component::<Trim>(base.stack);
    base.direct_trim = Some(resolved);
    Some(base)
}

#[derive(Clone, Copy)]
struct VanillaDemoRandom {
    seed: u64,
}

impl VanillaDemoRandom {
    fn from_identity(identity: usize) -> Self {
        let identity = identity as u32 as i32 as i64 as u64;
        Self {
            seed: (identity ^ 0x5DEE_CE66D) & 0xFFFF_FFFF_FFFF,
        }
    }

    fn next_bits(&mut self, bits: u32) -> u32 {
        self.seed = self.seed.wrapping_mul(25_214_903_917).wrapping_add(11) & 0xFFFF_FFFF_FFFF;
        (self.seed >> (48 - bits)) as u32
    }

    fn next_int(&mut self, bound: usize) -> usize {
        debug_assert!(bound > 0);
        let bound = bound as i32;
        if bound & (bound - 1) == 0 {
            return (((bound as i64) * (self.next_bits(31) as i64)) >> 31) as usize;
        }
        loop {
            let sample = self.next_bits(31) as i32;
            let modulo = sample % bound;
            if sample
                .wrapping_sub(modulo)
                .wrapping_add(bound.wrapping_sub(1))
                >= 0
            {
                return modulo as usize;
            }
        }
    }
}

/// Most stacks one slot display resolves to. Vanilla has no limit, but its
/// data never comes close; nested `with_any_potion` multiplies by every
/// potion per level, so hostile data would otherwise exhaust memory.
const MAX_RESOLVED_STACKS: usize = 4096;

pub(crate) fn slot_stacks(slot: &SlotDisplay, book: &RecipeBookState) -> Vec<ResolvedSlotStack> {
    let basic = |id: u32| {
        ItemKind::from_u32(id).map(|kind| ResolvedSlotStack::plain(ItemStackData::new(kind, 1)))
    };
    match slot {
        SlotDisplay::Empty => Vec::new(),
        SlotDisplay::AnyFuel => vanilla_fuel_items(book.item_tags())
            .into_iter()
            .filter_map(basic)
            .collect(),
        SlotDisplay::Item(id) => basic(*id).into_iter().collect(),
        SlotDisplay::ItemStack(template) => ItemKind::from_u32(template.item)
            .map(|kind| {
                ResolvedSlotStack::plain(ItemStackData {
                    kind,
                    count: template.count,
                    component_patch: template.components.clone(),
                })
            })
            .into_iter()
            .collect(),
        SlotDisplay::Tag(tag) => book
            .item_tags()
            .ordered(tag)
            .into_iter()
            .flatten()
            .copied()
            .filter_map(basic)
            .collect(),
        SlotDisplay::Composite(parts) => parts
            .iter()
            .flat_map(|part| slot_stacks(part, book))
            .take(MAX_RESOLVED_STACKS)
            .collect(),
        SlotDisplay::WithRemainder { input, .. } => slot_stacks(input, book),
        SlotDisplay::WithAnyPotion(inner) => {
            let base = slot_stacks(inner, book);
            let mut out = Vec::new();
            'potions: for potion in (0..).map_while(Potion::from_u32) {
                for stack in &base {
                    if out.len() == MAX_RESOLVED_STACKS {
                        break 'potions;
                    }
                    let mut stack = stack.clone();
                    stack.stack = with_component(
                        stack.stack,
                        PotionContents {
                            potion: Some(potion),
                            ..PotionContents::default()
                        },
                    );
                    out.push(stack);
                }
            }
            out
        }
        SlotDisplay::OnlyWithComponent {
            contents,
            component,
        } => {
            let Some(kind) = DataComponentKind::from_u32(*component) else {
                return Vec::new();
            };
            slot_stacks(contents, book)
                .into_iter()
                .filter(|stack| stack_has_component_kind(&stack.stack, kind))
                .collect()
        }
        SlotDisplay::Dyed { dye, target } => {
            let targets = slot_stacks(target, book);
            let dyes = slot_stacks(dye, book);
            let mut out = Vec::new();
            for index in 0..targets.len().saturating_mul(dyes.len()) {
                if out.len() == 16 {
                    break;
                }
                let target = targets[index % targets.len()].clone();
                let dye = &dyes[index / targets.len()];
                let dye = dye
                    .stack
                    .get_component::<Dye>()
                    .map_or(DyeColor::White, |component| component.color);
                out.push(apply_dye(target, dye));
            }
            out
        }
        SlotDisplay::SmithingTrim {
            base,
            material,
            trim_pattern,
        } => {
            let bases = slot_stacks(base, book);
            let materials = slot_stacks(material, book);
            if bases.is_empty() || materials.is_empty() {
                return Vec::new();
            }
            let mut random = VanillaDemoRandom::from_identity(slot as *const SlotDisplay as usize);
            let mut out = Vec::new();
            for _ in 0..256 {
                if out.len() == 16 {
                    break;
                }
                let base = &bases[random.next_int(bases.len())];
                let material = &materials[random.next_int(materials.len())];
                if let Some(trimmed) = apply_trim(base.clone(), material, trim_pattern) {
                    out.push(trimmed);
                }
            }
            out
        }
    }
}

pub(crate) fn vanilla_fuel_items(tags: &ItemTags) -> Vec<u32> {
    use azalea_registry::builtin::ItemKind as I;

    fn push_item(out: &mut Vec<u32>, seen: &mut HashSet<u32>, item: I) {
        let id = item.to_u32();
        if seen.insert(id) {
            out.push(id);
        }
    }

    fn push_tag(out: &mut Vec<u32>, seen: &mut HashSet<u32>, tags: &ItemTags, tag: &str) {
        if let Some(items) = tags.ordered(tag) {
            for &id in items {
                if seen.insert(id) {
                    out.push(id);
                }
            }
        }
    }

    let mut out = Vec::new();
    let mut seen = HashSet::new();

    push_item(&mut out, &mut seen, I::LavaBucket);
    push_item(&mut out, &mut seen, I::CoalBlock);
    push_item(&mut out, &mut seen, I::BlazeRod);
    push_item(&mut out, &mut seen, I::Coal);
    push_item(&mut out, &mut seen, I::Charcoal);
    push_tag(&mut out, &mut seen, tags, "minecraft:logs");
    push_tag(&mut out, &mut seen, tags, "minecraft:bamboo_blocks");
    push_tag(&mut out, &mut seen, tags, "minecraft:planks");
    push_item(&mut out, &mut seen, I::BambooMosaic);
    push_tag(&mut out, &mut seen, tags, "minecraft:wooden_stairs");
    push_item(&mut out, &mut seen, I::BambooMosaicStairs);
    push_tag(&mut out, &mut seen, tags, "minecraft:wooden_slabs");
    push_item(&mut out, &mut seen, I::BambooMosaicSlab);
    push_tag(&mut out, &mut seen, tags, "minecraft:wooden_trapdoors");
    push_tag(
        &mut out,
        &mut seen,
        tags,
        "minecraft:wooden_pressure_plates",
    );
    push_tag(&mut out, &mut seen, tags, "minecraft:wooden_shelves");
    push_tag(&mut out, &mut seen, tags, "minecraft:wooden_fences");
    push_tag(&mut out, &mut seen, tags, "minecraft:fence_gates");
    push_item(&mut out, &mut seen, I::NoteBlock);
    push_item(&mut out, &mut seen, I::Bookshelf);
    push_item(&mut out, &mut seen, I::ChiseledBookshelf);
    push_item(&mut out, &mut seen, I::Lectern);
    push_item(&mut out, &mut seen, I::Jukebox);
    push_item(&mut out, &mut seen, I::Chest);
    push_item(&mut out, &mut seen, I::TrappedChest);
    push_item(&mut out, &mut seen, I::CraftingTable);
    push_item(&mut out, &mut seen, I::DaylightDetector);
    push_tag(&mut out, &mut seen, tags, "minecraft:banners");
    push_item(&mut out, &mut seen, I::Bow);
    push_item(&mut out, &mut seen, I::FishingRod);
    push_item(&mut out, &mut seen, I::Ladder);
    push_tag(&mut out, &mut seen, tags, "minecraft:signs");
    push_tag(&mut out, &mut seen, tags, "minecraft:hanging_signs");
    push_item(&mut out, &mut seen, I::WoodenShovel);
    push_item(&mut out, &mut seen, I::WoodenSword);
    push_item(&mut out, &mut seen, I::WoodenSpear);
    push_item(&mut out, &mut seen, I::WoodenHoe);
    push_item(&mut out, &mut seen, I::WoodenAxe);
    push_item(&mut out, &mut seen, I::WoodenPickaxe);
    push_tag(&mut out, &mut seen, tags, "minecraft:wooden_doors");
    push_tag(&mut out, &mut seen, tags, "minecraft:boats");
    push_tag(&mut out, &mut seen, tags, "minecraft:wool");
    push_tag(&mut out, &mut seen, tags, "minecraft:wooden_buttons");
    push_item(&mut out, &mut seen, I::Stick);
    push_tag(&mut out, &mut seen, tags, "minecraft:saplings");
    push_item(&mut out, &mut seen, I::Bowl);
    push_tag(&mut out, &mut seen, tags, "minecraft:wool_carpets");
    push_item(&mut out, &mut seen, I::DriedKelpBlock);
    push_item(&mut out, &mut seen, I::Crossbow);
    push_item(&mut out, &mut seen, I::Bamboo);
    push_item(&mut out, &mut seen, I::DeadBush);
    push_item(&mut out, &mut seen, I::ShortDryGrass);
    push_item(&mut out, &mut seen, I::TallDryGrass);
    push_item(&mut out, &mut seen, I::Scaffolding);
    push_item(&mut out, &mut seen, I::Loom);
    push_item(&mut out, &mut seen, I::Barrel);
    push_item(&mut out, &mut seen, I::CartographyTable);
    push_item(&mut out, &mut seen, I::FletchingTable);
    push_item(&mut out, &mut seen, I::SmithingTable);
    push_item(&mut out, &mut seen, I::Composter);
    push_item(&mut out, &mut seen, I::Azalea);
    push_item(&mut out, &mut seen, I::FloweringAzalea);
    push_item(&mut out, &mut seen, I::MangroveRoots);
    push_item(&mut out, &mut seen, I::LeafLitter);

    if let Some(non_flammable) = tags.get("minecraft:non_flammable_wood") {
        out.retain(|id| !non_flammable.contains(id));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_potion_fan_out_stops_at_the_resolved_stack_cap() {
        let book = RecipeBookState::default();
        let mut slot = SlotDisplay::Item(ItemKind::Potion.to_u32());
        for _ in 0..4 {
            slot = SlotDisplay::WithAnyPotion(Box::new(slot));
        }
        assert_eq!(slot_stacks(&slot, &book).len(), MAX_RESOLVED_STACKS);
    }

    #[test]
    fn ghost_alternatives_preserve_slot_display_order_and_duplicates() {
        let book = RecipeBookState::default();
        let slot = SlotDisplay::Composite(vec![
            SlotDisplay::Item(3),
            SlotDisplay::Item(1),
            SlotDisplay::Item(3),
        ]);
        let ids = slot_stacks(&slot, &book)
            .into_iter()
            .map(|stack| stack.stack.kind.to_u32())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec![3, 1, 3]);
    }

    #[test]
    fn with_any_potion_resolves_component_bearing_stacks() {
        use azalea_registry::builtin::ItemKind;

        let book = RecipeBookState::default();
        let slot =
            SlotDisplay::WithAnyPotion(Box::new(SlotDisplay::Item(ItemKind::Potion.to_u32())));
        let stacks = slot_stacks(&slot, &book);
        assert!(stacks.len() > 1);
        assert!(stacks.iter().all(|stack| {
            stack.stack.kind == ItemKind::Potion
                && stack
                    .stack
                    .get_component::<PotionContents>()
                    .is_some_and(|contents| contents.potion.is_some())
        }));
    }

    #[test]
    fn only_with_component_uses_effective_dye_defaults() {
        use azalea_registry::builtin::{DataComponentKind, ItemKind};

        let book = RecipeBookState::default();
        let slot = SlotDisplay::OnlyWithComponent {
            contents: Box::new(SlotDisplay::Composite(vec![
                SlotDisplay::Item(ItemKind::WhiteDye.to_u32()),
                SlotDisplay::Item(ItemKind::Diamond.to_u32()),
            ])),
            component: DataComponentKind::Dye.to_u32(),
        };
        let stacks = slot_stacks(&slot, &book);
        assert_eq!(stacks.len(), 1);
        assert_eq!(stacks[0].stack.kind, ItemKind::WhiteDye);
        assert!(stacks[0].stack.get_component::<Dye>().is_some());
    }

    #[test]
    fn only_with_component_supports_non_dye_default_components() {
        use azalea_registry::builtin::{DataComponentKind, ItemKind};

        let book = RecipeBookState::default();
        let slot = SlotDisplay::OnlyWithComponent {
            contents: Box::new(SlotDisplay::Composite(vec![
                SlotDisplay::Item(ItemKind::IronChestplate.to_u32()),
                SlotDisplay::Item(ItemKind::Diamond.to_u32()),
            ])),
            component: DataComponentKind::MaxDamage.to_u32(),
        };
        let stacks = slot_stacks(&slot, &book);
        assert_eq!(stacks.len(), 1);
        assert_eq!(stacks[0].stack.kind, ItemKind::IronChestplate);
        assert!(
            stacks[0]
                .stack
                .get_component::<components::MaxDamage>()
                .is_some()
        );
    }

    #[test]
    fn dyed_demo_writes_vanilla_dyed_color_component() {
        use azalea_registry::builtin::ItemKind;

        let book = RecipeBookState::default();
        let slot = SlotDisplay::Dyed {
            dye: Box::new(SlotDisplay::Item(ItemKind::RedDye.to_u32())),
            target: Box::new(SlotDisplay::Item(ItemKind::LeatherChestplate.to_u32())),
        };
        let stacks = slot_stacks(&slot, &book);
        assert_eq!(stacks.len(), 1);
        assert_eq!(stacks[0].stack.kind, ItemKind::LeatherChestplate);
        assert_eq!(stacks[0].stack.count, 1);
        assert_eq!(
            stacks[0].stack.get_component::<DyedColor>().unwrap().rgb,
            0xB02E26
        );
    }

    #[test]
    fn smithing_trim_demo_writes_trim_component() {
        use azalea_registry::builtin::ItemKind;

        let book = RecipeBookState::default();
        let slot = SlotDisplay::SmithingTrim {
            base: Box::new(SlotDisplay::Item(ItemKind::IronChestplate.to_u32())),
            material: Box::new(SlotDisplay::Item(ItemKind::Diamond.to_u32())),
            trim_pattern: TrimPatternHolder::Reference(0),
        };
        let stacks = slot_stacks(&slot, &book);
        assert_eq!(stacks.len(), 16);
        assert!(stacks.iter().all(|stack| {
            stack.stack.kind == ItemKind::IronChestplate
                && stack.stack.count == 1
                && stack.stack.get_component::<Trim>().is_some()
                && stack.direct_trim.is_none()
        }));
    }

    #[test]
    fn vanilla_fuel_items_preserve_tag_order_and_exclude_non_flammable_wood() {
        use azalea_registry::builtin::ItemKind as I;

        let oak = I::OakLog.to_u32();
        let mangrove = I::MangroveLog.to_u32();
        let crimson = I::CrimsonStem.to_u32();
        let tags = ItemTags::from_entries([
            ("minecraft:logs".into(), vec![mangrove, oak, crimson]),
            ("minecraft:non_flammable_wood".into(), vec![crimson]),
        ]);

        let fuels = vanilla_fuel_items(&tags);
        let mangrove_pos = fuels.iter().position(|id| *id == mangrove).unwrap();
        let oak_pos = fuels.iter().position(|id| *id == oak).unwrap();
        assert!(mangrove_pos < oak_pos);
        assert!(!fuels.contains(&crimson));
    }
}
