//! The item stack and data-component wire grammar, ported from vanilla's
//! `ItemStack` / `ItemStackTemplate` / `DataComponentPatch` stream codecs.
//!
//! Component values are not length-prefixed on the wire, so framing a stack
//! means knowing every component's layout. Each one is described by a
//! [`Shape`] built from the `ByteBufCodecs` combinators; values are kept as
//! their exact bytes and only validated, leaving interpretation to callers.

use std::sync::OnceLock;

use crate::codec::{DecodeResult, MAX_STRING_LENGTH, Reader};
use crate::entity_data::{SERIALIZERS_26_2, SERIALIZERS_26_3, Serializer};
use crate::registries::{ClientRegistry, RegistryRemaps, RegistryTable};
use crate::version::EMBEDDED;
use crate::wire::write_varint;

/// Nesting limit across shapes, items and components, so hostile input
/// fails before the stack does.
const MAX_DEPTH: u32 = 512;

/// `ByteBufCodecs.list()` / `map()` with no explicit limit.
const UNBOUNDED: usize = usize::MAX;

/// A component value's wire layout, mirroring a `StreamCodec`.
#[derive(Debug)]
pub enum Shape {
    /// `StreamCodec.unit`: no bytes.
    Unit,
    Bool,
    /// `ByteBufCodecs.INT`.
    Int,
    Long,
    Float,
    Double,
    VarInt,
    /// `ByteBufCodecs.stringUtf8(max)`.
    Utf(usize),
    Uuid,
    /// `ByteBufCodecs.TAG` / `fromCodecWithRegistries`: any NBT but `End`.
    Nbt,
    /// `ByteBufCodecs.COMPOUND_TAG`.
    Compound,
    /// `ByteBufCodecs.optional`: a bool, then the value when true.
    Optional(&'static Shape),
    /// `ByteBufCodecs.list(max)`.
    List(usize, &'static Shape),
    /// `ByteBufCodecs.map`.
    Map(&'static Shape, &'static Shape),
    /// `ByteBufCodecs.either`: a bool, true for the left value.
    Either(&'static Shape, &'static Shape),
    /// `StreamCodec.composite`: the fields in order.
    Struct(&'static [Shape]),
    /// A shared shape, so composites can reuse (and recurse into) statics.
    Ref(&'static Shape),
    /// `ByteBufCodecs.holder`: varint 0 then an inline value, else id + 1.
    Holder(&'static Shape),
    /// [`Shape::Holder`] over a static registry, whose ids shift between
    /// versions.
    RegistryHolder(ClientRegistry, &'static Shape),
    /// `ByteBufCodecs.registry`: a static registry's id.
    RegistryId(ClientRegistry),
    /// `ByteBufCodecs.holderSet`: varint 0 then a tag id, else n + 1 ids.
    HolderSet,
    /// An `idMapper` enum dispatch with `OutOfBoundsStrategy.ZERO`.
    ById(&'static [Shape]),
    /// `ByteBufCodecs.registry(..).dispatch`, by entry name.
    ByRegistry(ClientRegistry, &'static [(&'static str, Shape)]),
    /// [`Shape::ByRegistry`] where every unlisted entry shares the last shape.
    ByRegistryOr(
        ClientRegistry,
        &'static [(&'static str, Shape)],
        &'static Shape,
    ),
    /// `ItemStackTemplate.STREAM_CODEC`.
    ItemTemplate,
    /// `ItemStack.STREAM_CODEC`, which 26.1 replaced with the template
    /// inside components: a positive count, the item, then the patch.
    Stack,
    /// `ItemStack.OPTIONAL_STREAM_CODEC`: a count, and the rest when it is
    /// positive.
    OptionalStack,
    /// `TagKey.streamCodec`: a tag's id, where 26.1 sends a `HolderSet`.
    TagKey,
    /// `EitherHolder.streamCodec`, gone in 26.1: `ByteBufCodecs.either` of
    /// the holder and a `ResourceKey`'s id.
    EitherHolder(&'static Shape),
    /// A field this version doesn't send, in a struct a later version
    /// extended: the bytes are what that later field holds by default.
    Absent(&'static [u8]),
    /// A field only this version sends, in a struct a later version cut
    /// down: its layout, and the bytes to send it when translating back.
    Extra(&'static Shape, &'static [u8]),
    /// `CustomData.STREAM_CODEC` holding an entity or block entity, whose
    /// type is the compound's `id` string. 1.21.9's `TypedEntityData` sends
    /// the type as a registry id ahead of the compound instead.
    IdCompound(ClientRegistry),
    /// `TypedDataComponent.STREAM_CODEC`: a component id and its value.
    TypedComponent,
}

use Shape::*;

const IDENTIFIER: Shape = Utf(MAX_STRING_LENGTH);
const STRING: Shape = Utf(MAX_STRING_LENGTH);
/// `ComponentSerialization.STREAM_CODEC`: text components travel as NBT.
const TEXT: Shape = Nbt;

/// `SoundEvent.STREAM_CODEC`: a holder over (location, fixed range).
pub static SOUND: Shape = RegistryHolder(
    ClientRegistry::SoundEvent,
    &Struct(&[IDENTIFIER, Optional(&Float)]),
);

/// `MobEffectInstance.Details.STREAM_CODEC`, recursive through its hidden
/// effect.
static MOB_EFFECT_DETAILS: Shape = Struct(&[
    VarInt,
    VarInt,
    Bool,
    Bool,
    Bool,
    Optional(&MOB_EFFECT_DETAILS),
]);
/// `MobEffectInstance.STREAM_CODEC`: `MobEffect.STREAM_CODEC`, then details.
pub static MOB_EFFECT_INSTANCE: Shape = Struct(&[VarInt, Ref(&MOB_EFFECT_DETAILS)]);

/// `ConsumeEffect.STREAM_CODEC` over `consume_effect_type`, given
/// `teleport_randomly`'s layout, the one arm that differs between versions.
macro_rules! consume_effect {
    ($teleport_randomly:expr) => {
        ByRegistry(
            ClientRegistry::ConsumeEffectType,
            &[
                // ApplyStatusEffectsConsumeEffect.java:31
                (
                    "apply_effects",
                    Struct(&[List(UNBOUNDED, &MOB_EFFECT_INSTANCE), Float]),
                ),
                // RemoveStatusEffectsConsumeEffect.java:32
                ("remove_effects", HolderSet),
                ("clear_all_effects", Unit),
                ("teleport_randomly", $teleport_randomly),
                // PlaySoundConsumeEffect.java:28
                ("play_sound", Ref(&SOUND)),
            ],
        )
    };
}

/// TeleportRandomlyConsumeEffect.java:37: a diameter.
static CONSUME_EFFECT: Shape = consume_effect!(Float);
/// 26.3 TeleportRandomlyConsumeEffect.java:42 added `directionalParticles`.
static CONSUME_EFFECT_26_3: Shape = consume_effect!(Struct(&[Float, Bool]));

/// `Consumable.STREAM_CODEC`'s fields before the effects (Consumable.java:49).
static CONSUMABLE_HEAD: Shape = Struct(&[Float, VarInt, Ref(&SOUND), Bool]);

/// `Consumable.STREAM_CODEC` over a version's effects.
macro_rules! consumable {
    ($effect:expr) => {
        Struct(&[Ref(&CONSUMABLE_HEAD), List(UNBOUNDED, $effect)])
    };
}

/// `SwingAnimation.STREAM_CODEC` (SwingAnimation.java:26): a type and a
/// duration. 26.2's `swing_animation` and the two components 26.3 split it
/// into all use it.
static SWING_ANIMATION: Shape = Struct(&[VarInt, VarInt]);

/// `TrimPattern.STREAM_CODEC` (TrimPattern.java:30).
static TRIM_PATTERN: Shape = Holder(&Struct(&[IDENTIFIER, TEXT, Bool]));

/// `ArmorTrim.STREAM_CODEC` (ArmorTrim.java:38) over a version's trim
/// material.
macro_rules! trim {
    ($material:expr) => {
        Struct(&[Ref($material), Ref(&TRIM_PATTERN)])
    };
}

/// `StatePropertiesPredicate.PropertyMatcher.STREAM_CODEC`: a name, then an
/// exact value or a range of optional bounds.
static PROPERTY_MATCHER: Shape = Struct(&[
    STRING,
    Either(&STRING, &Struct(&[Optional(&STRING), Optional(&STRING)])),
]);

/// `DataComponentMatchers.STREAM_CODEC`: exact components, then up to 64
/// partial predicates, each a `$kind` and an NBT body
/// (`DataComponentPredicate.TypeBase` encodes through its Codec).
macro_rules! component_matchers {
    ($kind:expr) => {
        Struct(&[
            List(UNBOUNDED, &TypedComponent),
            List(64, &Struct(&[$kind, Nbt])),
        ])
    };
}

/// A partial predicate's kind is a predicate type or a component type.
static COMPONENT_MATCHERS: Shape = component_matchers!(Either(&VarInt, &VarInt));

/// `advancements.predicates.BlockPredicate.STREAM_CODEC`, whose component
/// matchers arrived in 1.21.5.
macro_rules! block_predicate {
    ($matchers:expr) => {
        Struct(&[
            Optional(&HolderSet),
            Optional(&List(UNBOUNDED, &PROPERTY_MATCHER)),
            Optional(&Compound),
            $matchers,
        ])
    };
}

static BLOCK_PREDICATE: Shape = block_predicate!(Ref(&COMPONENT_MATCHERS));

/// `AdventureModePredicate.STREAM_CODEC` from 1.21.5 to 1.21.10, whose
/// partial predicates are typed by a predicate type's id alone
/// (DataComponentPredicate.java:31). 1.21.11 made that an either with a
/// component type.
static ADVENTURE_PREDICATE_1_21_10: Shape =
    List(UNBOUNDED, &block_predicate!(component_matchers!(VarInt)));

/// 1.21.10's components that differ from 1.21.11's, layered over
/// [`COMPONENTS_1_21_11`].
static COMPONENTS_1_21_10: &[(&str, Shape)] = &[
    ("can_place_on", Ref(&ADVENTURE_PREDICATE_1_21_10)),
    ("can_break", Ref(&ADVENTURE_PREDICATE_1_21_10)),
];

/// `ByteBufCodecs.GAME_PROFILE_PROPERTIES`.
static PROFILE_PROPERTIES: Shape = List(16, &Struct(&[Utf(64), Utf(32767), Optional(&Utf(1024))]));

/// `ResolvableProfile.STREAM_CODEC`: a full `GAME_PROFILE` or a `Partial`,
/// then `PlayerSkin.Patch.STREAM_CODEC`.
pub static RESOLVABLE_PROFILE: Shape = Struct(&[
    Either(
        &Struct(&[Uuid, Utf(16), Ref(&PROFILE_PROPERTIES)]),
        &Struct(&[
            Optional(&Utf(16)),
            Optional(&Uuid),
            Ref(&PROFILE_PROPERTIES),
        ]),
    ),
    Struct(&[
        Optional(&IDENTIFIER),
        Optional(&IDENTIFIER),
        Optional(&IDENTIFIER),
        Optional(&Bool),
    ]),
]);

/// `FireworkExplosion.STREAM_CODEC`.
static FIREWORK_EXPLOSION: Shape = Struct(&[
    VarInt,
    List(UNBOUNDED, &Int),
    List(UNBOUNDED, &Int),
    Bool,
    Bool,
]);

/// `TrimMaterial.DIRECT_STREAM_CODEC`: `MaterialAssetGroup` (a base suffix,
/// then equipment-asset overrides) and a description.
static TRIM_MATERIAL: Shape = Holder(&Struct(&[
    Struct(&[STRING, Map(&IDENTIFIER, &STRING)]),
    TEXT,
]));
/// 26.3 TrimMaterial.java:28 replaced the asset group with a palette id.
static TRIM_MATERIAL_26_3: Shape = Holder(&Struct(&[IDENTIFIER, TEXT]));

/// `TypedEntityData.streamCodec`: a registry id, then the compound.
/// `TypedEntityData.streamCodec` over `EntityType.STREAM_CODEC`.
static ENTITY_DATA: Shape = Struct(&[RegistryId(ClientRegistry::EntityType), Compound]);

/// `TypedEntityData.streamCodec` over the block entity type registry.
static BLOCK_ENTITY_DATA: Shape = Struct(&[RegistryId(ClientRegistry::BlockEntityType), Compound]);

/// `JukeboxSong.STREAM_CODEC` (JukeboxSong.java:34).
static JUKEBOX_SONG: Shape = Holder(&Struct(&[Ref(&SOUND), TEXT, Float, VarInt]));

/// `Instrument.STREAM_CODEC` through 26.2 (Instrument.java:29).
static INSTRUMENT: Shape = Holder(&Struct(&[Ref(&SOUND), Float, Float, TEXT]));

/// `BlocksAttacks.STREAM_CODEC` (BlocksAttacks.java:42), whose `bypassedBy`
/// damage types are `$bypassed_by`: a tag through 1.21.11, then a set.
macro_rules! blocks_attacks {
    ($bypassed_by:expr) => {
        Struct(&[
            Float,
            Float,
            List(
                UNBOUNDED,
                &Struct(&[Float, Optional(&HolderSet), Float, Float]),
            ),
            Struct(&[Float, Float, Float]),
            Optional(&$bypassed_by),
            Optional(&SOUND),
            Optional(&SOUND),
        ])
    };
}

/// `Equippable.STREAM_CODEC` (Equippable.java:48), whose last three fields
/// (`equipOnInteract`, `canBeSheared`, `shearingSound`) arrived in 1.21.5
/// and 1.21.6.
macro_rules! equippable {
    ($equip_on_interact:expr, $can_be_sheared:expr, $shearing_sound:expr) => {
        Struct(&[
            VarInt,
            Ref(&SOUND),
            Optional(&IDENTIFIER),
            Optional(&IDENTIFIER),
            Optional(&HolderSet),
            Bool,
            Bool,
            Bool,
            $equip_on_interact,
            $can_be_sheared,
            $shearing_sound,
        ])
    };
}

/// The default `shearingSound`, as a direct sound event.
const SHEARS_SNIP: Shape = Absent(b"\0\x1aminecraft:item.shears.snip\0");

/// `Tool.STREAM_CODEC` (Tool.java:30,50), whose
/// `canDestroyBlocksInCreative` arrived in 1.21.5.
macro_rules! tool {
    ($creative:expr) => {
        Struct(&[
            List(
                UNBOUNDED,
                &Struct(&[HolderSet, Optional(&Float), Optional(&Bool)]),
            ),
            Float,
            VarInt,
            $creative,
        ])
    };
}

/// `PotionContents.STREAM_CODEC` (PotionContents.java:66), whose
/// `customName` arrived in 1.21.2.
macro_rules! potion_contents {
    ($custom_name:expr) => {
        Struct(&[
            Optional(&VarInt),
            Optional(&Int),
            List(UNBOUNDED, &MOB_EFFECT_INSTANCE),
            $custom_name,
        ])
    };
}

/// `KineticWeapon.Condition.STREAM_CODEC`.
static KINETIC_CONDITION: Shape = Struct(&[VarInt, Float, Float]);

/// `BlockParticleOption.streamCodec`: a block state id.
const BLOCK_STATE: Shape = VarInt;

/// `ParticleTypes.STREAM_CODEC` in 26.2: the options of each particle type
/// that has any; the rest are `SimpleParticleType`s with none.
pub static PARTICLE: Shape = ByRegistryOr(
    ClientRegistry::ParticleType,
    &[
        ("block", BLOCK_STATE),
        ("block_marker", BLOCK_STATE),
        // GeyserParticleOptions.java:31
        ("geyser", Int),
        ("geyser_plume", Int),
        // GeyserBaseParticleOptions.java:33
        ("geyser_base", Struct(&[Int, Float])),
        ("geyser_poof", Struct(&[Int, Float])),
        // PowerParticleOption.java:29
        ("dragon_breath", Float),
        // DustParticleOptions.java:32
        ("dust", Struct(&[Int, Float])),
        // DustColorTransitionOptions.java:32
        ("dust_color_transition", Struct(&[Int, Int, Float])),
        // SpellParticleOption.java:38
        ("effect", Struct(&[Int, Float])),
        ("instant_effect", Struct(&[Int, Float])),
        // ColorParticleOption.java:29
        ("entity_effect", Int),
        ("tinted_leaves", Int),
        ("flash", Int),
        ("falling_dust", BLOCK_STATE),
        // SculkChargeParticleOptions.java:28
        ("sculk_charge", Float),
        // ItemParticleOption.java:27
        ("item", ItemTemplate),
        // VibrationParticleOption.java:33: a `PositionSource`, then ticks.
        (
            "vibration",
            Struct(&[
                ByRegistry(
                    ClientRegistry::PositionSourceType,
                    &[
                        // BlockPositionSource.java:29
                        ("block", Long),
                        // EntityPositionSource.java:38
                        ("entity", Struct(&[VarInt, Float])),
                    ],
                ),
                VarInt,
            ]),
        ),
        // TrailParticleOption.java:28: a `Vec3`, colour and duration.
        ("trail", Struct(&[Double, Double, Double, Int, VarInt])),
        // ShriekParticleOption.java:28
        ("shriek", VarInt),
        ("dust_pillar", BLOCK_STATE),
        ("block_crumble", BLOCK_STATE),
    ],
    &Unit,
);

/// The 26.2 value layout of each `DataComponents` entry, by registry name.
/// Components without a network codec fall back to their persistent Codec as
/// NBT (`DataComponentType.Builder.build`): a compound wherever that Codec
/// only parses one.
static COMPONENTS_26_2: &[(&str, Shape)] = &[
    ("custom_data", Compound),
    ("max_stack_size", VarInt),
    ("max_damage", VarInt),
    ("damage", VarInt),
    ("repair_cost", VarInt),
    ("enchantable", VarInt),
    ("additional_trade_cost", VarInt),
    ("map_id", VarInt),
    ("ominous_bottle_amplifier", VarInt),
    ("unbreakable", Unit),
    ("creative_slot_lock", Unit),
    ("glider", Unit),
    // UseEffects.java:24
    ("use_effects", Struct(&[Bool, Bool, Float])),
    ("custom_name", TEXT),
    ("item_name", TEXT),
    ("minimum_attack_charge", Float),
    ("potion_duration_scale", Float),
    ("damage_type", VarInt),
    ("item_model", IDENTIFIER),
    ("tooltip_style", IDENTIFIER),
    ("note_block_sound", IDENTIFIER),
    // ItemLore.java:34
    ("lore", List(256, &TEXT)),
    ("rarity", VarInt),
    ("dye", VarInt),
    ("map_post_processing", VarInt),
    ("base_color", VarInt),
    // ItemEnchantments.java:44
    ("enchantments", Map(&VarInt, &VarInt)),
    ("stored_enchantments", Map(&VarInt, &VarInt)),
    // AdventureModePredicate.java:37
    ("can_place_on", List(UNBOUNDED, &BLOCK_PREDICATE)),
    ("can_break", List(UNBOUNDED, &BLOCK_PREDICATE)),
    // ItemAttributeModifiers.java:56,132,145; AttributeModifier.java:30
    (
        "attribute_modifiers",
        List(
            UNBOUNDED,
            &Struct(&[
                VarInt,
                Struct(&[IDENTIFIER, Double, VarInt]),
                VarInt,
                ById(&[Unit, Unit, TEXT]),
            ]),
        ),
    ),
    // CustomModelData.java:28
    (
        "custom_model_data",
        Struct(&[
            List(UNBOUNDED, &Float),
            List(UNBOUNDED, &Bool),
            List(UNBOUNDED, &STRING),
            List(UNBOUNDED, &Int),
        ]),
    ),
    // TooltipDisplay.java:30
    ("tooltip_display", Struct(&[Bool, List(UNBOUNDED, &VarInt)])),
    ("enchantment_glint_override", Bool),
    ("intangible_projectile", Compound),
    // FoodProperties.java:35
    ("food", Struct(&[VarInt, Float, Bool])),
    ("consumable", consumable!(&CONSUME_EFFECT)),
    // UseRemainder.java:17
    ("use_remainder", ItemTemplate),
    ("sulfur_cube_content", ItemTemplate),
    // UseCooldown.java:28
    ("use_cooldown", Struct(&[Float, Optional(&IDENTIFIER)])),
    ("damage_resistant", HolderSet),
    ("repairable", HolderSet),
    ("provides_banner_patterns", HolderSet),
    // Tool.java:30,50
    ("tool", tool!(Bool)),
    // Weapon.java:24
    ("weapon", Struct(&[VarInt, Float])),
    // AttackRange.java:42
    (
        "attack_range",
        Struct(&[Float, Float, Float, Float, Float, Float]),
    ),
    // Equippable.java:48
    ("equippable", equippable!(Bool, Bool, Ref(&SOUND))),
    // DeathProtection.java:30
    ("death_protection", List(UNBOUNDED, &CONSUME_EFFECT)),
    // BlocksAttacks.java:42,96,109
    ("blocks_attacks", blocks_attacks!(HolderSet)),
    // PiercingWeapon.java:39
    (
        "piercing_weapon",
        Struct(&[Bool, Bool, Optional(&SOUND), Optional(&SOUND)]),
    ),
    // KineticWeapon.java:47,110
    (
        "kinetic_weapon",
        Struct(&[
            VarInt,
            VarInt,
            Optional(&KINETIC_CONDITION),
            Optional(&KINETIC_CONDITION),
            Optional(&KINETIC_CONDITION),
            Float,
            Float,
            Optional(&SOUND),
            Optional(&SOUND),
        ]),
    ),
    // SwingAnimation.java:26
    ("swing_animation", Ref(&SWING_ANIMATION)),
    ("dyed_color", Int),
    ("map_color", Int),
    ("map_decorations", Compound),
    ("debug_stick_state", Compound),
    ("recipes", Nbt),
    ("lock", Compound),
    ("container_loot", Compound),
    // ChargedProjectiles.java:38
    ("charged_projectiles", List(1024, &ItemTemplate)),
    // BundleContents.java:42
    ("bundle_contents", List(UNBOUNDED, &ItemTemplate)),
    // PotionContents.java:66
    ("potion_contents", potion_contents!(Optional(&STRING))),
    // SuspiciousStewEffects.java:44,70
    (
        "suspicious_stew_effects",
        List(UNBOUNDED, &Struct(&[VarInt, VarInt])),
    ),
    // WritableBookContent.java:33
    (
        "writable_book_content",
        List(100, &Struct(&[Utf(1024), Optional(&Utf(1024))])),
    ),
    // WrittenBookContent.java:65
    (
        "written_book_content",
        Struct(&[
            Struct(&[Utf(32), Optional(&Utf(32))]),
            STRING,
            VarInt,
            List(UNBOUNDED, &Struct(&[TEXT, Optional(&TEXT)])),
            Bool,
        ]),
    ),
    ("trim", trim!(&TRIM_MATERIAL)),
    ("provides_trim_material", Ref(&TRIM_MATERIAL)),
    ("entity_data", Ref(&ENTITY_DATA)),
    ("block_entity_data", Ref(&BLOCK_ENTITY_DATA)),
    // CustomData.java:26
    ("bucket_entity_data", Compound),
    // InstrumentComponent.java:27
    ("instrument", Ref(&INSTRUMENT)),
    // JukeboxPlayable.java:40
    ("jukebox_playable", Ref(&JUKEBOX_SONG)),
    // LodestoneTracker.java:28; GlobalPos.java:29
    (
        "lodestone_tracker",
        Struct(&[Optional(&Struct(&[IDENTIFIER, Long])), Bool]),
    ),
    ("firework_explosion", Ref(&FIREWORK_EXPLOSION)),
    // Fireworks.java:37
    (
        "fireworks",
        Struct(&[VarInt, List(256, &FIREWORK_EXPLOSION)]),
    ),
    ("profile", Ref(&RESOLVABLE_PROFILE)),
    // BannerPatternLayers.java:47,62; BannerPattern.java:26
    (
        "banner_patterns",
        List(
            UNBOUNDED,
            &Struct(&[Holder(&Struct(&[IDENTIFIER, STRING])), VarInt]),
        ),
    ),
    // PotDecorations.java:33
    ("pot_decorations", List(4, &VarInt)),
    // ItemContainerContents.java:42
    ("container", List(256, &Optional(&ItemTemplate))),
    // BlockItemStateProperties.java:37
    ("block_state", Map(&STRING, &STRING)),
    // Bees.java:26; BeehiveBlockEntity.java:310
    (
        "bees",
        List(UNBOUNDED, &Struct(&[Ref(&ENTITY_DATA), VarInt, VarInt])),
    ),
    ("break_sound", Ref(&SOUND)),
    // Registry-holder variants (`holderRegistry`) and `idMapper` enums.
    ("villager/variant", VarInt),
    ("wolf/variant", VarInt),
    ("wolf/sound_variant", VarInt),
    ("wolf/collar", VarInt),
    ("fox/variant", VarInt),
    ("salmon/size", VarInt),
    ("parrot/variant", VarInt),
    ("tropical_fish/pattern", VarInt),
    ("tropical_fish/base_color", VarInt),
    ("tropical_fish/pattern_color", VarInt),
    ("mooshroom/variant", VarInt),
    ("rabbit/variant", VarInt),
    ("pig/variant", VarInt),
    ("pig/sound_variant", VarInt),
    ("cow/variant", VarInt),
    ("cow/sound_variant", VarInt),
    ("chicken/variant", VarInt),
    ("chicken/sound_variant", VarInt),
    ("zombie_nautilus/variant", VarInt),
    ("frog/variant", VarInt),
    ("horse/variant", VarInt),
    ("llama/variant", VarInt),
    ("axolotl/variant", VarInt),
    ("cat/variant", VarInt),
    ("cat/sound_variant", VarInt),
    ("cat/collar", VarInt),
    ("sheep/color", VarInt),
    ("shulker/color", VarInt),
    // PaintingVariant.java:30,32
    (
        "painting/variant",
        Holder(&Struct(&[
            VarInt,
            VarInt,
            IDENTIFIER,
            Optional(&TEXT),
            Optional(&TEXT),
        ])),
    ),
];

/// 1.21.11's components that differ from 26.2's, layered over
/// [`COMPONENTS_26_2`]. 26.1 turned the stacks nested in components into
/// templates, the tags into holder sets, and dropped `EitherHolder`.
static COMPONENTS_1_21_11: &[(&str, Shape)] = &[
    // DataComponents.java: EitherHolder.streamCodec over a registry holder.
    ("damage_type", EitherHolder(&VarInt)),
    ("chicken/variant", EitherHolder(&VarInt)),
    ("zombie_nautilus/variant", EitherHolder(&VarInt)),
    // JukeboxPlayable.java:47
    ("jukebox_playable", EitherHolder(&JUKEBOX_SONG)),
    // InstrumentComponent.java:36
    ("instrument", EitherHolder(&INSTRUMENT)),
    // ProvidesTrimMaterial.java:26
    ("provides_trim_material", EitherHolder(&TRIM_MATERIAL)),
    // DamageResistant.java:29
    ("damage_resistant", TagKey),
    ("provides_banner_patterns", TagKey),
    // BlocksAttacks.java:53
    ("blocks_attacks", blocks_attacks!(TagKey)),
    // UseRemainder.java
    ("use_remainder", Stack),
    // ChargedProjectiles.java:31
    ("charged_projectiles", List(UNBOUNDED, &Stack)),
    // BundleContents.java:36
    ("bundle_contents", List(UNBOUNDED, &Stack)),
    // ItemContainerContents.java:43
    ("container", List(256, &OptionalStack)),
];

/// `ResolvableProfile.STREAM_CODEC` through 1.21.8 (ResolvableProfile.java:45),
/// which is the later layout's partial profile with no skin patch.
static PROFILE_1_21_8: Shape = Struct(&[
    Optional(&Utf(16)),
    Optional(&Uuid),
    Ref(&PROFILE_PROPERTIES),
]);

/// 1.21.8's and 1.21.6's components that differ from 1.21.11's, layered
/// over [`COMPONENTS_1_21_11`].
static COMPONENTS_1_21_8: &[(&str, Shape)] = &[
    ("profile", Ref(&PROFILE_1_21_8)),
    // DataComponents.java: CustomData.STREAM_CODEC for both.
    ("entity_data", IdCompound(ClientRegistry::EntityType)),
    (
        "block_entity_data",
        IdCompound(ClientRegistry::BlockEntityType),
    ),
    // BeehiveBlockEntity.java:325
    (
        "bees",
        List(
            UNBOUNDED,
            &Struct(&[IdCompound(ClientRegistry::EntityType), VarInt, VarInt]),
        ),
    ),
];

/// `ItemAttributeModifiers.Entry.STREAM_CODEC` before 1.21.6 added its
/// `Display`, which defaults to 0.
static ATTRIBUTE_ENTRIES_1_21_5: Shape = List(
    UNBOUNDED,
    &Struct(&[
        VarInt,
        Struct(&[IDENTIFIER, Double, VarInt]),
        VarInt,
        Absent(&[0]),
    ]),
);

/// A component's `showInTooltip` bool, which 1.21.5 moved to
/// `tooltip_display`.
// TODO: a false one should add the component to `tooltip_display`'s hidden
// list; it is dropped, so the component shows in the tooltip.
const SHOWN_IN_TOOLTIP: Shape = Extra(&Bool, &[1]);

/// 1.21.4 `TrimMaterial.STREAM_CODEC` (TrimMaterial.java): the asset name,
/// the ingredient item, per-equipment overrides, the description. 1.21.5
/// dropped the ingredient.
static TRIM_MATERIAL_1_21_4: Shape = Holder(&Struct(&[
    Struct(&[STRING, Extra(&VarInt, &[0]), Map(&IDENTIFIER, &STRING)]),
    TEXT,
]));

/// `ArmorTrim.STREAM_CODEC` through 1.21.4 over a version's trim material.
/// The named conversions carry it, so its `showInTooltip` is a plain bool.
macro_rules! old_trim {
    ($material:expr) => {
        Struct(&[Ref($material), Ref(&TRIM_PATTERN_1_21_4), Bool])
    };
}

/// `ItemEnchantments.STREAM_CODEC` through 1.21.4.
static ENCHANTMENTS_1_21_4: Shape = Struct(&[Map(&VarInt, &VarInt), SHOWN_IN_TOOLTIP]);

/// 1.21.4 `TrimPattern.STREAM_CODEC`, which named its template item.
static TRIM_PATTERN_1_21_4: Shape =
    Holder(&Struct(&[IDENTIFIER, Extra(&VarInt, &[0]), TEXT, Bool]));

/// 1.21.3 `TrimMaterial.STREAM_CODEC`: an item model index after the
/// ingredient, and overrides keyed by an armor material's id.
static TRIM_MATERIAL_1_21_3: Shape = Holder(&Struct(&[
    STRING,
    VarInt,
    Float,
    Map(&IDENTIFIER, &STRING),
    TEXT,
]));

/// 1.21.1 `TrimMaterial.STREAM_CODEC`: the overrides keyed by a holder.
static TRIM_MATERIAL_1_21_1: Shape = Holder(&Struct(&[
    STRING,
    VarInt,
    Float,
    Map(&VarInt, &STRING),
    TEXT,
]));

/// A `FoodProperties.PossibleEffect` list, which 1.21.2 moved to
/// `consumable`.
// TODO: carry the effects, eat time and conversion over as `consumable` and
// `use_remainder`; they are dropped.
static FOOD_EFFECTS: Shape = List(UNBOUNDED, &Struct(&[Ref(&MOB_EFFECT_INSTANCE), Float]));

/// `eatSeconds`, 1.6 by default.
const EAT_SECONDS: Shape = Extra(&Float, &[0x3F, 0xCC, 0xCC, 0xCD]);

/// 1.20.6's components that differ from 1.21.1's, layered over
/// [`COMPONENTS_1_21_1`].
static COMPONENTS_1_20_6: &[(&str, Shape)] = &[
    // FoodProperties.java, before `usingConvertsTo`.
    (
        "food",
        Struct(&[VarInt, Float, Bool, EAT_SECONDS, Extra(&FOOD_EFFECTS, &[0])]),
    ),
    // ItemAttributeModifiers.java; AttributeModifier.java: a UUID and a
    // name where 1.21 has an id.
    (
        "attribute_modifiers",
        Struct(&[
            List(
                UNBOUNDED,
                &Struct(&[VarInt, Struct(&[Uuid, STRING, Double, VarInt]), VarInt]),
            ),
            Bool,
        ]),
    ),
];

/// 1.21.1's components that differ from 1.21.3's, layered over
/// [`COMPONENTS_1_21_3`]. 1.21.2 split eating out of `food`.
static COMPONENTS_1_21_1: &[(&str, Shape)] = &[
    // FoodProperties.java
    (
        "food",
        Struct(&[
            VarInt,
            Float,
            Bool,
            EAT_SECONDS,
            Extra(&Optional(&Stack), &[0]),
            Extra(&FOOD_EFFECTS, &[0]),
        ]),
    ),
    // PotionContents.java, before `customName` (absent).
    ("potion_contents", potion_contents!(Absent(&[0]))),
    // Instrument.java: a use duration in ticks, no description.
    ("instrument", Holder(&Struct(&[Ref(&SOUND), VarInt, Float]))),
    ("trim", old_trim!(&TRIM_MATERIAL_1_21_1)),
    // 1.21.2 made it `damage_resistant` to the fire tag.
    // TODO: carry it over as `damage_resistant`; it is dropped.
    ("fire_resistant", Unit),
    // LockCode.java:21: the key as a string tag, where 1.21.2 has an item
    // predicate compound.
    // TODO: carry it over as a predicate on the key's name; it is dropped.
    ("lock", Nbt),
];

/// 1.21.3's components that differ from 1.21.4's, layered over
/// [`COMPONENTS_1_21_4`].
static COMPONENTS_1_21_3: &[(&str, Shape)] = &[
    // CustomModelData.java: one int.
    ("custom_model_data", VarInt),
    ("trim", old_trim!(&TRIM_MATERIAL_1_21_3)),
];

/// 1.21.4's components that differ from 1.21.5's, layered over
/// [`COMPONENTS_1_21_5`]. 1.21.5 moved every `showInTooltip` out, extended
/// three structs and wrapped the instrument in an `EitherHolder`.
static COMPONENTS_1_21_4: &[(&str, Shape)] = &[
    // Unbreakable.java
    ("unbreakable", Struct(&[SHOWN_IN_TOOLTIP, Unit])),
    // DyedItemColor.java
    ("dyed_color", Struct(&[Int, SHOWN_IN_TOOLTIP])),
    // ItemEnchantments.java
    ("enchantments", Ref(&ENCHANTMENTS_1_21_4)),
    ("stored_enchantments", Ref(&ENCHANTMENTS_1_21_4)),
    // ItemAttributeModifiers.java
    (
        "attribute_modifiers",
        Struct(&[Ref(&ATTRIBUTE_ENTRIES_1_21_5), SHOWN_IN_TOOLTIP]),
    ),
    // AdventureModePredicate.java
    ("can_place_on", Ref(&ADVENTURE_PREDICATE_1_21_4)),
    ("can_break", Ref(&ADVENTURE_PREDICATE_1_21_4)),
    // JukeboxPlayable.java
    (
        "jukebox_playable",
        Struct(&[EitherHolder(&JUKEBOX_SONG), SHOWN_IN_TOOLTIP]),
    ),
    // Tool.java, before `canDestroyBlocksInCreative` (true).
    ("tool", tool!(Absent(&[1]))),
    // Equippable.java, also before `equipOnInteract` (false).
    (
        "equippable",
        equippable!(Absent(&[0]), Absent(&[0]), SHEARS_SNIP),
    ),
    // ArmorTrim.java
    ("trim", old_trim!(&TRIM_MATERIAL_1_21_4)),
    // DataComponents.java: Instrument.STREAM_CODEC, a plain holder.
    ("instrument", Ref(&INSTRUMENT)),
    // 1.21.5 folded both into `tooltip_display`.
    // TODO: carry them over as `tooltip_display`; they are dropped.
    ("hide_additional_tooltip", Unit),
    ("hide_tooltip", Unit),
];

/// 1.21.4 `AdventureModePredicate.STREAM_CODEC`: block predicates without
/// 1.21.5's component matchers (empty by default), and `showInTooltip`.
static ADVENTURE_PREDICATE_1_21_4: Shape = Struct(&[
    List(UNBOUNDED, &block_predicate!(Absent(&[0, 0]))),
    SHOWN_IN_TOOLTIP,
]);

/// 1.21.5's components that differ from 1.21.6's, layered over
/// [`COMPONENTS_1_21_8`]: both structs were extended in 1.21.6.
static COMPONENTS_1_21_5: &[(&str, Shape)] = &[
    // ItemAttributeModifiers.java:134
    ("attribute_modifiers", Ref(&ATTRIBUTE_ENTRIES_1_21_5)),
    // Equippable.java:59, before `canBeSheared` (false) and
    // `shearingSound`.
    ("equippable", equippable!(Bool, Absent(&[0]), SHEARS_SNIP)),
];

/// 26.3's components that differ from 26.2's, layered over
/// [`COMPONENTS_26_2`]: the four whose layout changed, then the added ones.
static COMPONENTS_26_3: &[(&str, Shape)] = &[
    ("consumable", consumable!(&CONSUME_EFFECT_26_3)),
    ("death_protection", List(UNBOUNDED, &CONSUME_EFFECT_26_3)),
    ("trim", trim!(&TRIM_MATERIAL_26_3)),
    ("provides_trim_material", Ref(&TRIM_MATERIAL_26_3)),
    // Instrument.java:29 added `durabilityDamage`.
    (
        "instrument",
        Holder(&Struct(&[Ref(&SOUND), Float, Float, VarInt, TEXT])),
    ),
    // PotDecorations.java:37: back, left, right, front.
    (
        "pot_decorations",
        Struct(&[
            Optional(&ItemTemplate),
            Optional(&ItemTemplate),
            Optional(&ItemTemplate),
            Optional(&ItemTemplate),
        ]),
    ),
    // SwingAnimation.java:26
    ("attack_animation", Ref(&SWING_ANIMATION)),
    ("interact_animation", Ref(&SWING_ANIMATION)),
    // BlockTransformer.java:66, a registry holder.
    ("block_transformer", VarInt),
    // VillagerFood.java:24
    ("villager_food", VarInt),
    // Compostable.java:25; ResolvableInt: a constant or a reference.
    ("compostable", Either(&Int, &IDENTIFIER)),
    // CookingFuel.java:27, BrewingFuel.java:27: a ResolvableInt, then a
    // ResolvableFloat.
    (
        "cooking_fuel",
        Struct(&[Either(&Int, &IDENTIFIER), Either(&Float, &IDENTIFIER)]),
    ),
    (
        "brewing_fuel",
        Struct(&[Either(&Int, &IDENTIFIER), Either(&Float, &IDENTIFIER)]),
    ),
    // MobVisibility.java:29
    ("mob_visibility", Struct(&[HolderSet, Float])),
    // DecoratedPotPatterns.java:46, a registry holder.
    ("provides_pottery_pattern", VarInt),
    ("sign_text_front", Ref(&SIGN_TEXT)),
    ("sign_text_back", Ref(&SIGN_TEXT)),
    ("waxed", Unit),
    // DyeColor.java:172
    ("cushion/color", VarInt),
];

/// `SignText.STREAM_CODEC` (SignText.java:43): four lines, optional filtered
/// lines, a dye color and the glowing flag; fixed-size lists carry no count.
static SIGN_TEXT: Shape = Struct(&[
    Struct(&[TEXT, TEXT, TEXT, TEXT]),
    Optional(&Struct(&[TEXT, TEXT, TEXT, TEXT])),
    VarInt,
    Bool,
]);

/// A stack or template: an item registry id, a count and its patch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemStack {
    pub item: u32,
    pub count: i32,
    pub patch: ComponentPatch,
}

/// `DataComponentPatch`: components added (with their exact value bytes)
/// and removed, by component-type id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ComponentPatch {
    pub added: Vec<(u32, Box<[u8]>)>,
    pub removed: Vec<u32>,
}

impl ComponentPatch {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }
}

/// One version's item grammar: each component id's shape plus the
/// registries its dispatches resolve against.
pub struct ItemCodec {
    registries: &'static RegistryTable,
    components: Vec<&'static Shape>,
    entity_data: &'static [Serializer],
    /// This version's layer and every later one, whose conversions carry
    /// what the generic one can't, searched in order; empty for 26.3.
    layers: &'static [Layer],
    air: u32,
}

type Conversions = &'static [(&'static str, Conversion)];

/// What one or more versions before 26.3 lay out differently from the next
/// entry's: the shapes, and the conversions the generic one can't do. A
/// version's grammar is its layer and every later one, searched in order, so
/// a layer lists only what changed.
struct Layer {
    versions: &'static [&'static str],
    /// A reference to the table itself, never a copy of it: two grammars
    /// carry a component unchanged only when they reach the same shape.
    shapes: &'static &'static [(&'static str, Shape)],
    /// This version's components into the native layout, by its name for
    /// the component.
    up: Conversions,
    /// Native components into this version's layout, by the same name.
    down: Conversions,
    /// Its entity-data serializers, for the versions whose entity data is
    /// read through the grammar.
    entity_data: &'static [Serializer],
}

/// Oldest first.
static LAYERS: [Layer; 9] = [
    Layer {
        versions: &["1.20.6"],
        shapes: &COMPONENTS_1_20_6,
        up: FROM_1_20_6,
        down: TO_1_20_6,
        entity_data: &[],
    },
    Layer {
        versions: &["1.21.1"],
        shapes: &COMPONENTS_1_21_1,
        up: FROM_1_21_1,
        down: TO_1_21_1,
        entity_data: &[],
    },
    Layer {
        versions: &["1.21.3"],
        shapes: &COMPONENTS_1_21_3,
        up: FROM_1_21_3,
        down: TO_1_21_3,
        entity_data: &[],
    },
    Layer {
        versions: &["1.21.4"],
        shapes: &COMPONENTS_1_21_4,
        up: FROM_1_21_4,
        down: TO_1_21_4,
        entity_data: &[],
    },
    Layer {
        versions: &["1.21.5"],
        shapes: &COMPONENTS_1_21_5,
        up: &[],
        down: &[],
        entity_data: &[],
    },
    Layer {
        versions: &["1.21.6", "1.21.8"],
        shapes: &COMPONENTS_1_21_8,
        up: FROM_1_21_8,
        down: TO_1_21_8,
        entity_data: &[],
    },
    Layer {
        versions: &["1.21.10"],
        shapes: &COMPONENTS_1_21_10,
        up: &[],
        down: &[],
        entity_data: &[],
    },
    Layer {
        versions: &["1.21.11"],
        shapes: &COMPONENTS_1_21_11,
        up: FROM_1_21_11,
        down: TO_1_21_11,
        entity_data: &[],
    },
    // 26.1's layouts are 26.2's; the one component it lacks
    // (`sulfur_cube_content`) isn't in its table.
    Layer {
        versions: &["26.1", "26.2"],
        shapes: &COMPONENTS_26_2,
        up: TO_26_3,
        down: TO_26_2,
        entity_data: SERIALIZERS_26_2,
    },
];

impl ItemCodec {
    /// The native version's grammar.
    pub fn native() -> &'static ItemCodec {
        Self::for_protocol(crate::version::NATIVE.protocol).expect("native item grammar")
    }

    /// The grammar for `protocol`, when its component layouts are ported.
    pub fn for_protocol(protocol: i32) -> Option<&'static ItemCodec> {
        static CODECS: [OnceLock<Option<ItemCodec>>; EMBEDDED.len()] =
            [const { OnceLock::new() }; EMBEDDED.len()];
        let index = EMBEDDED
            .iter()
            .position(|e| e.version.protocol == protocol)?;
        CODECS[index]
            .get_or_init(|| {
                let name = EMBEDDED[index].version.name;
                let (shapes, entity_data, layers): (Vec<_>, &[Serializer], &[Layer]) =
                    if name == "26.3" {
                        (
                            vec![COMPONENTS_26_3, COMPONENTS_26_2],
                            SERIALIZERS_26_3,
                            &[],
                        )
                    } else {
                        let layer = LAYERS.iter().position(|l| l.versions.contains(&name))?;
                        let layers = &LAYERS[layer..];
                        (
                            layers.iter().map(|l| *l.shapes).collect(),
                            layers[0].entity_data,
                            layers,
                        )
                    };
                let mut codec =
                    Self::new(RegistryTable::for_protocol(protocol)?, &shapes, entity_data)?;
                codec.layers = layers;
                Some(codec)
            })
            .as_ref()
    }

    /// `layers` are shape tables searched in order, so a version can list
    /// only what changed over an older one.
    fn new(
        registries: &'static RegistryTable,
        layers: &[&'static [(&'static str, Shape)]],
        entity_data: &'static [Serializer],
    ) -> Option<Self> {
        let components = registries
            .names(ClientRegistry::DataComponentType)
            .iter()
            .map(|name| {
                layers
                    .iter()
                    .find_map(|shapes| shapes.iter().find(|(n, _)| n == name))
                    .map(|(_, s)| s)
            })
            .collect::<Option<_>>()?;
        let air = registries.id_of(ClientRegistry::Item, "air")?;
        Some(Self {
            registries,
            components,
            entity_data,
            layers: &[],
            air,
        })
    }

    /// Component type `id`'s value layout.
    pub fn component_shape(&self, id: u32) -> Option<&'static Shape> {
        self.components.get(id as usize).copied()
    }

    pub fn registries(&self) -> &'static RegistryTable {
        self.registries
    }

    pub fn entity_data_serializers(&self) -> &'static [Serializer] {
        self.entity_data
    }

    /// `ItemStack.OPTIONAL_STREAM_CODEC`: a count (empty when not positive),
    /// the item, then the patch.
    pub fn read_stack(&self, r: &mut Reader) -> DecodeResult<Option<ItemStack>> {
        let count = r.var_i32()?;
        if count <= 0 {
            return Ok(None);
        }
        let item = self.read_item(r)?;
        let patch = self.read_patch(r, 0)?;
        Ok(Some(ItemStack { item, count, patch }))
    }

    /// `ItemStackTemplate.STREAM_CODEC`: the item, a count, then the patch;
    /// never empty.
    pub fn read_template(&self, r: &mut Reader) -> DecodeResult<ItemStack> {
        self.read_template_at(r, 0)
    }

    fn read_template_at(&self, r: &mut Reader, depth: u32) -> DecodeResult<ItemStack> {
        let item = self.read_item(r)?;
        let count = r.var_i32()?;
        if count == 0 || item == self.air {
            return Err(r.error("empty item template"));
        }
        let patch = self.read_patch(r, depth + 1)?;
        Ok(ItemStack { item, count, patch })
    }

    fn read_item(&self, r: &mut Reader) -> DecodeResult<u32> {
        let item = r.varint()?;
        if self
            .registries
            .name_of(ClientRegistry::Item, item)
            .is_none()
        {
            return Err(r.error("unknown item id"));
        }
        Ok(item)
    }

    /// `DataComponentPatch.STREAM_CODEC`.
    pub fn read_patch(&self, r: &mut Reader, depth: u32) -> DecodeResult<ComponentPatch> {
        if depth > MAX_DEPTH {
            return Err(r.error("item nested too deeply"));
        }
        let added_count = r.len(UNBOUNDED)?;
        let removed_count = r.len(UNBOUNDED)?;
        let mut patch = ComponentPatch {
            added: Vec::with_capacity(added_count),
            removed: Vec::with_capacity(removed_count),
        };
        for _ in 0..added_count {
            let id = r.varint()?;
            let start = r.pos();
            self.walk(r, self.component(r, id)?, depth + 1)?;
            patch.added.push((id, r.since(start).into()));
        }
        for _ in 0..removed_count {
            let id = r.varint()?;
            self.component(r, id)?;
            patch.removed.push(id);
        }
        Ok(patch)
    }

    fn component(&self, r: &Reader, id: u32) -> DecodeResult<&'static Shape> {
        self.components
            .get(id as usize)
            .copied()
            .ok_or_else(|| r.error("unknown data component type"))
    }

    /// Validates one component value of type `id` and returns its bytes.
    pub fn read_component<'a>(&self, r: &mut Reader<'a>, id: u32) -> DecodeResult<&'a [u8]> {
        let start = r.pos();
        self.walk(r, self.component(r, id)?, 0)?;
        Ok(r.since(start))
    }

    /// Validates and skips one value of `shape`.
    pub fn skip(&self, r: &mut Reader, shape: &'static Shape) -> DecodeResult<()> {
        self.walk(r, shape, 0)
    }

    fn walk(&self, r: &mut Reader, shape: &'static Shape, depth: u32) -> DecodeResult<()> {
        if depth > MAX_DEPTH {
            return Err(r.error("component nested too deeply"));
        }
        let depth = depth + 1;
        match shape {
            Unit | Absent(_) => {}
            Bool => {
                r.bool()?;
            }
            Int | Float => {
                r.bytes(4)?;
            }
            Long | Double => {
                r.bytes(8)?;
            }
            VarInt => {
                r.varint()?;
            }
            Utf(max) => {
                r.utf(*max)?;
            }
            Uuid => {
                r.uuid()?;
            }
            Nbt => {
                if r.nbt()?[0] == 0 {
                    return Err(r.error("expected a non-empty NBT tag"));
                }
            }
            Compound | IdCompound(_) => {
                if r.nbt()?[0] != 10 {
                    return Err(r.error("expected an NBT compound"));
                }
            }
            Optional(inner) => {
                if r.bool()? {
                    self.walk(r, inner, depth)?;
                }
            }
            List(max, inner) => {
                for _ in 0..r.len(*max)? {
                    self.walk(r, inner, depth)?;
                }
            }
            Map(key, value) => {
                for _ in 0..r.len(UNBOUNDED)? {
                    self.walk(r, key, depth)?;
                    self.walk(r, value, depth)?;
                }
            }
            Either(left, right) => {
                let inner = if r.bool()? { left } else { right };
                self.walk(r, inner, depth)?;
            }
            Struct(fields) => {
                for field in *fields {
                    self.walk(r, field, depth)?;
                }
            }
            Ref(inner) => self.walk(r, inner, depth)?,
            Holder(inline) | RegistryHolder(_, inline) => {
                if r.varint()? == 0 {
                    self.walk(r, inline, depth)?;
                }
            }
            RegistryId(_) => {
                r.varint()?;
            }
            HolderSet => match r.varint()? {
                0 => {
                    r.identifier()?;
                }
                n => {
                    for _ in 0..r.len_of(n as usize - 1)? {
                        r.varint()?;
                    }
                }
            },
            ById(arms) => {
                let id = r.varint()? as usize;
                // OutOfBoundsStrategy.ZERO
                self.walk(r, arms.get(id).unwrap_or(&arms[0]), depth)?;
            }
            ByRegistry(registry, arms) => {
                let inner = self.dispatch(r, *registry, arms)?;
                self.walk(
                    r,
                    inner.ok_or_else(|| r.error("unported dispatch type"))?,
                    depth,
                )?;
            }
            ByRegistryOr(registry, arms, rest) => {
                let inner = self.dispatch(r, *registry, arms)?;
                self.walk(r, inner.unwrap_or(rest), depth)?;
            }
            ItemTemplate => {
                self.read_template_at(r, depth)?;
            }
            Stack => {
                if r.var_i32()? <= 0 {
                    return Err(r.error("empty item stack"));
                }
                self.read_item(r)?;
                self.read_patch(r, depth)?;
            }
            OptionalStack => {
                if r.var_i32()? > 0 {
                    self.read_item(r)?;
                    self.read_patch(r, depth)?;
                }
            }
            TagKey => {
                r.identifier()?;
            }
            EitherHolder(holder) => {
                if r.bool()? {
                    self.walk(r, holder, depth)?;
                } else {
                    r.identifier()?;
                }
            }
            Extra(inner, _) => self.walk(r, inner, depth)?,
            TypedComponent => {
                let id = r.varint()?;
                self.walk(r, self.component(r, id)?, depth)?;
            }
        }
        Ok(())
    }

    /// Reads a registry dispatch's type id and finds its arm, if listed.
    fn dispatch(
        &self,
        r: &mut Reader,
        registry: ClientRegistry,
        arms: &'static [(&'static str, Shape)],
    ) -> DecodeResult<Option<&'static Shape>> {
        let id = r.varint()?;
        let name = self
            .registries
            .name_of(registry, id)
            .ok_or_else(|| r.error("unknown dispatch type"))?;
        Ok(arms.iter().find(|(arm, _)| *arm == name).map(|(_, s)| s))
    }
}

/// Rewrites items read with one version's grammar into another's: the item,
/// component and dispatch ids are remapped, and a component the target lacks
/// or lays out differently is dropped (its counts rebuilt). A stack whose
/// item the target lacks becomes empty, and such a template is left out of
/// the list or optional holding it.
// TODO: static-registry ids typed only as `VarInt` or `HolderSet` (block
// predicates, attributes, effects, ...) pass through unremapped.
struct Translator<'c> {
    from: &'c ItemCodec,
    to: &'c ItemCodec,
    remaps: &'c RegistryRemaps,
}

impl ItemCodec {
    /// `ItemStack.OPTIONAL_STREAM_CODEC` read with this grammar, written in
    /// `to`'s ids.
    pub fn translate_stack(
        &self,
        r: &mut Reader,
        out: &mut Vec<u8>,
        to: &ItemCodec,
        remaps: &RegistryRemaps,
    ) -> DecodeResult<()> {
        self.translator(to, remaps).stack(r, out)
    }

    /// `ItemStack.OPTIONAL_UNTRUSTED_STREAM_CODEC` (each component value
    /// length-prefixed), read with this grammar and written in `to`'s ids.
    pub fn translate_untrusted_stack(
        &self,
        r: &mut Reader,
        out: &mut Vec<u8>,
        to: &ItemCodec,
        remaps: &RegistryRemaps,
    ) -> DecodeResult<()> {
        let t = self.translator(to, remaps);
        t.stack_with(r, out, |r, patch| t.patch_with(r, patch, 0, true))
    }

    /// `ItemStackTemplate.STREAM_CODEC`; `false` (nothing written) when `to`
    /// lacks the item.
    pub fn translate_template(
        &self,
        r: &mut Reader,
        out: &mut Vec<u8>,
        to: &ItemCodec,
        remaps: &RegistryRemaps,
    ) -> DecodeResult<bool> {
        self.translator(to, remaps).template(r, out, 0)
    }

    /// `DataComponentPatch.STREAM_CODEC`.
    pub fn translate_patch(
        &self,
        r: &mut Reader,
        out: &mut Vec<u8>,
        to: &ItemCodec,
        remaps: &RegistryRemaps,
    ) -> DecodeResult<()> {
        self.translator(to, remaps).patch(r, out, 0)
    }

    /// `TypedDataComponent.STREAM_CODEC`; `false` (nothing written) when
    /// `to` can't carry the component.
    pub fn translate_typed_component(
        &self,
        r: &mut Reader,
        out: &mut Vec<u8>,
        to: &ItemCodec,
        remaps: &RegistryRemaps,
    ) -> DecodeResult<bool> {
        self.translator(to, remaps).typed_component(r, out, 0)
    }

    fn translator<'c>(&'c self, to: &'c ItemCodec, remaps: &'c RegistryRemaps) -> Translator<'c> {
        Translator {
            from: self,
            to,
            remaps,
        }
    }
}

impl Translator<'_> {
    fn stack(&self, r: &mut Reader, out: &mut Vec<u8>) -> DecodeResult<()> {
        self.stack_with(r, out, |r, patch| self.patch(r, patch, 0))
    }

    /// A stack whose patch `patch` translates.
    fn stack_with(
        &self,
        r: &mut Reader,
        out: &mut Vec<u8>,
        patch_with: impl FnOnce(&mut Reader, &mut Vec<u8>) -> DecodeResult<()>,
    ) -> DecodeResult<()> {
        let count = r.var_i32()?;
        if count <= 0 {
            write_varint(out, 0);
            return Ok(());
        }
        let item = self.item(r)?;
        let mut patch = Vec::new();
        patch_with(r, &mut patch)?;
        match item {
            Some(item) => {
                write_varint(out, count as u32);
                write_varint(out, item);
                out.extend_from_slice(&patch);
            }
            None => write_varint(out, 0),
        }
        Ok(())
    }

    fn template(&self, r: &mut Reader, out: &mut Vec<u8>, depth: u32) -> DecodeResult<bool> {
        let item = self.item(r)?;
        let count = r.var_i32()?;
        let mut patch = Vec::new();
        self.patch(r, &mut patch, depth + 1)?;
        let Some(item) = item else {
            return Ok(false);
        };
        write_varint(out, item);
        write_varint(out, count as u32);
        out.extend_from_slice(&patch);
        Ok(true)
    }

    /// The item id in the target's space, `None` when it has no such item.
    fn item(&self, r: &mut Reader) -> DecodeResult<Option<u32>> {
        let item = self.from.read_item(r)?;
        Ok(self.remaps.remap(ClientRegistry::Item, item))
    }

    fn patch(&self, r: &mut Reader, out: &mut Vec<u8>, depth: u32) -> DecodeResult<()> {
        self.patch_with(r, out, depth, false)
    }

    /// `DataComponentPatch.STREAM_CODEC`, or with `delimited` its
    /// `DELIMITED_STREAM_CODEC`, which length-prefixes each value.
    fn patch_with(
        &self,
        r: &mut Reader,
        out: &mut Vec<u8>,
        depth: u32,
        delimited: bool,
    ) -> DecodeResult<()> {
        if depth > MAX_DEPTH {
            return Err(r.error("item nested too deeply"));
        }
        let added_count = r.len(UNBOUNDED)?;
        let removed_count = r.len(UNBOUNDED)?;
        let mut added = (0, Vec::new());
        for _ in 0..added_count {
            let id = r.varint()?;
            let translated = if delimited {
                let len = r.len(UNBOUNDED)?;
                let mut value = Reader::new(r.bytes(len)?);
                let translated = self.component(&mut value, id, depth)?;
                value.ensure_eof()?;
                translated
            } else {
                self.component(r, id, depth)?
            };
            if let Some((target, value)) = translated {
                for target in [Some(target), self.split_twin(id)].into_iter().flatten() {
                    write_varint(&mut added.1, target);
                    if delimited {
                        write_varint(&mut added.1, value.len() as u32);
                    }
                    added.1.extend_from_slice(&value);
                    added.0 += 1;
                }
            }
        }
        let mut removed = (0, Vec::new());
        for _ in 0..removed_count {
            let id = r.varint()?;
            self.from.component(r, id)?;
            let target = self.remaps.remap(ClientRegistry::DataComponentType, id);
            for target in [target, target.and(self.split_twin(id))]
                .into_iter()
                .flatten()
            {
                write_varint(&mut removed.1, target);
                removed.0 += 1;
            }
        }
        write_varint(out, added.0);
        write_varint(out, removed.0);
        out.extend_from_slice(&added.1);
        out.extend_from_slice(&removed.1);
        Ok(())
    }

    fn typed_component(&self, r: &mut Reader, out: &mut Vec<u8>, depth: u32) -> DecodeResult<bool> {
        let id = r.varint()?;
        let Some((target, value)) = self.component(r, id, depth)? else {
            return Ok(false);
        };
        write_varint(out, target);
        out.extend_from_slice(&value);
        Ok(true)
    }

    /// Component `id`'s target id and value in the target's layout, or `None`
    /// when it was dropped. The value is consumed either way.
    fn component(
        &self,
        r: &mut Reader,
        id: u32,
        depth: u32,
    ) -> DecodeResult<Option<(u32, Vec<u8>)>> {
        let shape = self.from.component(r, id)?;
        let target = self.remaps.remap(ClientRegistry::DataComponentType, id);
        let target_shape = target.and_then(|target| self.to.component_shape(target));
        let same_layout = target_shape.is_some_and(|other| std::ptr::eq(shape, other));
        let conversion = target
            .filter(|_| !same_layout)
            .and_then(|target| self.conversion(id, target));
        let mut value = Vec::new();
        let carried = match (conversion, target_shape) {
            (Some(convert), _) => convert(self, r, &mut value, depth + 1)?,
            (None, Some(to)) => self.convert(r, &mut value, shape, to, depth + 1)?,
            (None, None) => {
                self.from.walk(r, shape, depth + 1)?;
                false
            }
        };
        Ok(target.filter(|_| carried).map(|target| (target, value)))
    }

    /// The target's second component for source component `id`, when the
    /// target split it in two: 26.3 made `swing_animation` both
    /// `attack_animation`, which the remap gives, and `interact_animation`
    /// with the same value (`SwingAnimationComponentSplitFix`).
    fn split_twin(&self, id: u32) -> Option<u32> {
        let name = self
            .from
            .registries
            .name_of(ClientRegistry::DataComponentType, id)?;
        (name == "swing_animation")
            .then(|| {
                self.to
                    .registries
                    .id_of(ClientRegistry::DataComponentType, "interact_animation")
            })
            .flatten()
    }

    /// The named conversion for source component `id`, whose target id is
    /// `target`. The layers belong to the non-native side, and are keyed by
    /// its name for the component.
    fn conversion(&self, id: u32, target: u32) -> Option<Conversion> {
        let up = std::ptr::eq(self.to, ItemCodec::native());
        let (codec, id) = if up {
            (self.from, id)
        } else {
            (self.to, target)
        };
        let name = codec
            .registries
            .name_of(ClientRegistry::DataComponentType, id)?;
        codec
            .layers
            .iter()
            .flat_map(|layer| if up { layer.up } else { layer.down })
            .find(|(n, _)| *n == name)
            .map(|(_, convert)| *convert)
    }

    /// One value laid out as `from`, rewritten as `to`: the recurring
    /// differences between versions (a nested stack against a template, a
    /// tag against a holder set, an `EitherHolder` against its holder),
    /// found by walking both shapes together. `false` when the target can't
    /// represent the value, or the shapes differ some other way. The value
    /// is consumed either way.
    fn convert(
        &self,
        r: &mut Reader,
        out: &mut Vec<u8>,
        from: &'static Shape,
        to: &'static Shape,
        depth: u32,
    ) -> DecodeResult<bool> {
        if std::ptr::eq(from, to) {
            return self.value(r, out, from, depth);
        }
        if depth > MAX_DEPTH {
            return Err(r.error("component nested too deeply"));
        }
        let depth = depth + 1;
        Ok(match (from, to) {
            (Ref(from), _) => self.convert(r, out, from, to, depth)?,
            (_, Ref(to)) => self.convert(r, out, from, to, depth)?,
            (Absent(default), _) => {
                out.extend_from_slice(default);
                true
            }
            (_, Absent(_)) => {
                self.from.walk(r, from, depth)?;
                true
            }
            (Optional(from), Optional(to)) => {
                if !r.bool()? {
                    out.push(0);
                    return Ok(true);
                }
                let mut value = Vec::new();
                let carried = self.convert(r, &mut value, from, to, depth)?;
                out.push(u8::from(carried));
                if carried {
                    out.extend_from_slice(&value);
                }
                // A stack the target lacks the item of is an absent one.
                carried || to.holds_an_item()
            }
            (List(max, from), List(_, to)) => {
                let (mut kept, mut items) = (0, Vec::new());
                let mut carried = true;
                for _ in 0..r.len(*max)? {
                    let mut item = Vec::new();
                    if self.convert(r, &mut item, from, to, depth)? {
                        kept += 1;
                        items.extend_from_slice(&item);
                    } else if !to.holds_an_item() {
                        carried = false;
                    }
                }
                write_varint(out, kept);
                out.extend_from_slice(&items);
                carried
            }
            (Holder(from), Holder(to)) => {
                let id = r.varint()?;
                write_varint(out, id);
                id != 0 || self.convert(r, out, from, to, depth)?
            }
            (Map(from_key, from_value), Map(to_key, to_value)) => {
                let len = r.len(UNBOUNDED)?;
                write_varint(out, len as u32);
                let mut carried = true;
                for _ in 0..len {
                    carried &= self.convert(r, out, from_key, to_key, depth)?;
                    carried &= self.convert(r, out, from_value, to_value, depth)?;
                }
                carried
            }
            // The named form of a holder set is a zero ahead of the tag id.
            (TagKey, HolderSet) => {
                out.push(0);
                self.value(r, out, from, depth)?
            }
            (HolderSet, TagKey) => {
                let start = r.pos();
                self.from.walk(r, from, depth)?;
                let set = r.since(start);
                let named = set[0] == 0;
                if named {
                    out.extend_from_slice(&set[1..]);
                }
                named
            }
            (ItemTemplate, ItemTemplate) => self.template(r, out, depth)?,
            (TypedComponent, TypedComponent) => self.typed_component(r, out, depth)?,
            // A value that a later version made the left arm of an either.
            (Either(left, right), _) if !matches!(to, Either(..)) => {
                if r.bool()? {
                    self.convert(r, out, left, to, depth)?
                } else {
                    self.from.walk(r, right, depth)?;
                    false
                }
            }
            (_, Either(left, _)) if !matches!(from, Either(..)) => {
                out.push(1);
                self.convert(r, out, from, left, depth)?
            }
            (EitherHolder(holder), _) => {
                if r.bool()? {
                    self.convert(r, out, holder, to, depth)?
                } else {
                    // A bare key names an entry of a registry the server
                    // syncs, which the grammar can't resolve to a holder.
                    r.identifier()?;
                    false
                }
            }
            (_, EitherHolder(holder)) => {
                out.push(1);
                self.convert(r, out, from, holder, depth)?
            }
            (Stack, ItemTemplate) => {
                let count = r.var_i32()?;
                if count <= 0 {
                    return Err(r.error("empty item stack"));
                }
                self.item_then(r, out, depth, |out, item| {
                    write_varint(out, item);
                    write_varint(out, count as u32);
                })?
            }
            (ItemTemplate, Stack) => {
                let item = self.item(r)?;
                let count = r.varint()?;
                let mut patch = Vec::new();
                self.patch(r, &mut patch, depth)?;
                if let Some(item) = item {
                    write_varint(out, count);
                    write_varint(out, item);
                    out.extend_from_slice(&patch);
                }
                item.is_some()
            }
            (OptionalStack, Optional(ItemTemplate)) => {
                let count = r.var_i32()?;
                if count <= 0 {
                    out.push(0);
                    return Ok(true);
                }
                let mut template = vec![1];
                let carried = self.item_then(r, &mut template, depth, |out, item| {
                    write_varint(out, item);
                    write_varint(out, count as u32);
                })?;
                out.extend_from_slice(if carried { &template } else { &[0] });
                true
            }
            (Optional(ItemTemplate), OptionalStack) => {
                let mut stack = Vec::new();
                if r.bool()? && self.convert(r, &mut stack, &ItemTemplate, &Stack, depth)? {
                    out.extend_from_slice(&stack);
                } else {
                    write_varint(out, 0);
                }
                true
            }
            (IdCompound(registry), Struct([RegistryId(typed), Compound])) if registry == typed => {
                let start = r.pos();
                self.from.walk(r, from, depth)?;
                self.typed_from_id_compound(r.since(start), out, *registry)?
            }
            (Struct([RegistryId(typed), Compound]), IdCompound(registry)) if registry == typed => {
                let id = r.varint()?;
                let compound = r.nbt()?;
                let name = self
                    .remaps
                    .remap(*registry, id)
                    .and_then(|id| self.to.registries.name_of(*registry, id));
                if let Some(name) = name {
                    let name = format!("minecraft:{name}");
                    // The compound's tag, then `id` as its first entry.
                    out.extend([10, 8, 0, 2, b'i', b'd']);
                    out.extend((name.len() as u16).to_be_bytes());
                    out.extend_from_slice(name.as_bytes());
                    out.extend_from_slice(&compound[1..]);
                }
                name.is_some()
            }
            // Field by field. A struct against a bare value is one whose
            // other fields are all `Extra`.
            (Struct(_), _) | (_, Struct(_)) => {
                let (mut a, mut b) = (from.fields().iter(), to.fields().iter());
                let (mut from, mut to) = (a.next(), b.next());
                let mut carried = true;
                loop {
                    match (from, to) {
                        (Some(Extra(only_here, _)), _) => {
                            self.from.walk(r, only_here, depth)?;
                            from = a.next();
                        }
                        (_, Some(Extra(_, default))) => {
                            out.extend_from_slice(default);
                            to = b.next();
                        }
                        (Some(field), Some(target)) => {
                            carried &= self.convert(r, out, field, target, depth)?;
                            (from, to) = (a.next(), b.next());
                        }
                        (Some(field), None) => {
                            self.from.walk(r, field, depth)?;
                            carried = false;
                            from = a.next();
                        }
                        (None, Some(_)) => return Ok(false),
                        (None, None) => break,
                    }
                }
                carried
            }
            _ if from.same_leaf(to) => self.value(r, out, from, depth)?,
            _ => {
                self.from.walk(r, from, depth)?;
                false
            }
        })
    }

    /// `compound`, which names its type in an `id` string, as that type's
    /// registry id and the compound without the entry. `false` without a
    /// type the target knows.
    fn typed_from_id_compound(
        &self,
        compound: &[u8],
        out: &mut Vec<u8>,
        registry: ClientRegistry,
    ) -> DecodeResult<bool> {
        let mut entries = Reader::new(&compound[1..]);
        let mut rest = vec![10];
        let mut kind = None;
        loop {
            let start = entries.pos();
            let tag = entries.u8()?;
            if tag == 0 {
                break;
            }
            let name_len = entries.u16()?;
            let name = entries.bytes(name_len as usize)?;
            let value_at = entries.pos();
            entries.skip_nbt_payload(tag, 0)?;
            if tag == 8 && name == b"id" {
                // A string's own two length bytes lead its payload.
                let id = std::str::from_utf8(&entries.since(value_at)[2..]).ok();
                kind = id
                    .map(|id| id.strip_prefix("minecraft:").unwrap_or(id))
                    .and_then(|id| self.from.registries.id_of(registry, id))
                    .and_then(|id| self.remaps.remap(registry, id));
            } else {
                rest.extend_from_slice(entries.since(start));
            }
        }
        rest.push(0);
        if let Some(kind) = kind {
            write_varint(out, kind);
            out.extend_from_slice(&rest);
        }
        Ok(kind.is_some())
    }

    /// Reads a stack's item and patch after its count; with an item the
    /// target has, writes what `head` puts ahead of the patch, then the
    /// patch.
    fn item_then(
        &self,
        r: &mut Reader,
        out: &mut Vec<u8>,
        depth: u32,
        head: impl FnOnce(&mut Vec<u8>, u32),
    ) -> DecodeResult<bool> {
        let item = self.item(r)?;
        let mut patch = Vec::new();
        self.patch(r, &mut patch, depth)?;
        if let Some(item) = item {
            head(out, item);
            out.extend_from_slice(&patch);
        }
        Ok(item.is_some())
    }

    /// A static registry's `id` written in the target's ids, plus `offset`
    /// (1 in a holder); `false` when the target lacks the entry.
    fn registry_id(
        &self,
        out: &mut Vec<u8>,
        registry: ClientRegistry,
        id: u32,
        offset: u32,
    ) -> bool {
        let remapped = self.remaps.remap(registry, id);
        write_varint(out, remapped.map_or(0, |id| id + offset));
        remapped.is_some()
    }

    /// One value of `shape`, rewritten into `out`; `false` when the target
    /// can't represent it (its bytes are then meaningless). The value is
    /// consumed either way.
    fn value(
        &self,
        r: &mut Reader,
        out: &mut Vec<u8>,
        shape: &'static Shape,
        depth: u32,
    ) -> DecodeResult<bool> {
        if depth > MAX_DEPTH {
            return Err(r.error("component nested too deeply"));
        }
        let depth = depth + 1;
        Ok(match shape {
            Unit | Bool | Int | Long | Float | Double | VarInt | Utf(_) | Uuid | Nbt | Compound
            | HolderSet | TagKey => {
                let start = r.pos();
                self.from.walk(r, shape, depth)?;
                out.extend_from_slice(r.since(start));
                true
            }
            Optional(inner) => {
                if !r.bool()? {
                    out.push(0);
                    return Ok(true);
                }
                let mut value = Vec::new();
                let carried = self.value(r, &mut value, inner, depth)?;
                if carried {
                    out.push(1);
                    out.extend_from_slice(&value);
                    true
                } else {
                    // A template the target lacks is an absent one.
                    out.push(0);
                    matches!(inner, ItemTemplate)
                }
            }
            List(max, inner) => {
                let len = r.len(*max)?;
                let (mut kept, mut items) = (0, Vec::new());
                let mut carried = true;
                for _ in 0..len {
                    let mut item = Vec::new();
                    if self.value(r, &mut item, inner, depth)? {
                        kept += 1;
                        items.extend_from_slice(&item);
                    } else if !matches!(inner, ItemTemplate) {
                        carried = false;
                    }
                }
                write_varint(out, kept);
                out.extend_from_slice(&items);
                carried
            }
            Map(key, value) => {
                let len = r.len(UNBOUNDED)?;
                write_varint(out, len as u32);
                let mut carried = true;
                for _ in 0..len {
                    carried &= self.value(r, out, key, depth)?;
                    carried &= self.value(r, out, value, depth)?;
                }
                carried
            }
            Either(left, right) => {
                let is_left = r.bool()?;
                out.push(u8::from(is_left));
                self.value(r, out, if is_left { left } else { right }, depth)?
            }
            Struct(fields) => {
                let mut carried = true;
                for field in *fields {
                    carried &= self.value(r, out, field, depth)?;
                }
                carried
            }
            Ref(inner) => self.value(r, out, inner, depth)?,
            Holder(inline) => {
                let id = r.varint()?;
                write_varint(out, id);
                id != 0 || self.value(r, out, inline, depth)?
            }
            RegistryHolder(registry, inline) => match r.varint()? {
                0 => {
                    write_varint(out, 0);
                    self.value(r, out, inline, depth)?
                }
                id => self.registry_id(out, *registry, id - 1, 1),
            },
            RegistryId(registry) => {
                let id = r.varint()?;
                self.registry_id(out, *registry, id, 0)
            }
            ById(arms) => {
                let id = r.varint()?;
                write_varint(out, id);
                // OutOfBoundsStrategy.ZERO
                self.value(r, out, arms.get(id as usize).unwrap_or(&arms[0]), depth)?
            }
            ByRegistry(registry, arms) | ByRegistryOr(registry, arms, _) => {
                let id = r.varint()?;
                let name = self
                    .from
                    .registries
                    .name_of(*registry, id)
                    .ok_or_else(|| r.error("unknown dispatch type"))?;
                let inner = match (arms.iter().find(|(arm, _)| *arm == name), shape) {
                    (Some((_, inner)), _) => inner,
                    (None, ByRegistryOr(_, _, rest)) => rest,
                    (None, _) => return Err(r.error("unported dispatch type")),
                };
                let target = self.remaps.remap(*registry, id);
                write_varint(out, target.unwrap_or(0));
                self.value(r, out, inner, depth)? && target.is_some()
            }
            ItemTemplate => self.template(r, out, depth)?,
            // The older nested forms only ever meet a newer layout, through
            // `convert`.
            Stack | OptionalStack | EitherHolder(_) | IdCompound(_) | Absent(_) | Extra(..) => {
                self.from.walk(r, shape, depth)?;
                false
            }
            TypedComponent => self.typed_component(r, out, depth)?,
        })
    }
}

impl Shape {
    /// Whether a value is an item stack in some form, which a list or an
    /// optional leaves out when the target lacks the item.
    fn holds_an_item(&self) -> bool {
        matches!(self, ItemTemplate | Stack)
    }

    /// A struct's fields, or the value itself as its only field.
    fn fields(&'static self) -> &'static [Shape] {
        match self {
            Struct(fields) => fields,
            _ => std::slice::from_ref(self),
        }
    }

    /// Whether two shapes are the same fixed layout with nothing nested to
    /// compare.
    fn same_leaf(&self, other: &Shape) -> bool {
        match (self, other) {
            (Utf(a), Utf(b)) => a == b,
            (RegistryId(a), RegistryId(b)) => a == b,
            (
                Unit | Bool | Int | Long | Float | Double | VarInt | Uuid | Nbt | Compound
                | HolderSet | TagKey,
                _,
            ) => std::mem::discriminant(self) == std::mem::discriminant(other),
            _ => false,
        }
    }
}

/// Rewrites one component value from a version's layout into another's;
/// `false` when it can't be represented there. The value is consumed either
/// way.
type Conversion = fn(&Translator, &mut Reader, &mut Vec<u8>, u32) -> DecodeResult<bool>;

/// A trim through 1.21.4: its material in `layout`, its pattern naming a
/// template item, and `showInTooltip`.
fn trim_to_26_3(
    t: &Translator,
    r: &mut Reader,
    out: &mut Vec<u8>,
    depth: u32,
    layout: TrimMaterialLayout,
) -> DecodeResult<bool> {
    let material = trim_material_to_26_3(t, r, out, depth, layout)?;
    let pattern = t.convert(r, out, &TRIM_PATTERN_1_21_4, &TRIM_PATTERN, depth)?;
    r.bool()?;
    Ok(material && pattern)
}

/// The reverse; the template item isn't known, and is air.
fn trim_from_26_3(
    t: &Translator,
    r: &mut Reader,
    out: &mut Vec<u8>,
    depth: u32,
    layout: TrimMaterialLayout,
) -> DecodeResult<bool> {
    let material = trim_material_to_26_2(t, r, out, depth, layout)?;
    let pattern = t.convert(r, out, &TRIM_PATTERN, &TRIM_PATTERN_1_21_4, depth)?;
    out.push(1);
    Ok(material && pattern)
}

/// Through 1.21.4 the instrument is a plain holder, as in 26.2, ahead of
/// the `EitherHolder` the later layers convert.
const FROM_1_21_4: Conversions = &[
    ("trim", |t, r, out, depth| {
        trim_to_26_3(t, r, out, depth, TrimMaterialLayout::V1_21_4)
    }),
    ("instrument", instrument_26_3),
];

const TO_1_21_4: Conversions = &[
    ("trim", |t, r, out, depth| {
        trim_from_26_3(t, r, out, depth, TrimMaterialLayout::V1_21_4)
    }),
    ("instrument", instrument_26_2),
];

/// 1.21.3's trim material also carried a model index, and its
/// `custom_model_data` was one int, which 1.21.4 made the first float
/// (`CustomModelDataExpandFix`).
const FROM_1_21_3: Conversions = &[
    ("trim", |t, r, out, depth| {
        trim_to_26_3(t, r, out, depth, TrimMaterialLayout::V1_21_3)
    }),
    ("custom_model_data", |_, r, out, _| {
        let value = r.var_i32()?;
        out.push(1);
        out.extend((value as f32).to_be_bytes());
        out.extend([0, 0, 0]);
        Ok(true)
    }),
];

/// The reverse: the first float as the int, when there is one.
const TO_1_21_3: Conversions = &[
    ("trim", |t, r, out, depth| {
        trim_from_26_3(t, r, out, depth, TrimMaterialLayout::V1_21_3)
    }),
    ("custom_model_data", |t, r, out, depth| {
        let floats = r.len(UNBOUNDED)?;
        let mut first = None;
        for _ in 0..floats {
            let float = f32::from_be_bytes(r.bytes(4)?.try_into().expect("four bytes"));
            first.get_or_insert(float);
        }
        for rest in [&Bool, &STRING, &Int] {
            for _ in 0..r.len(UNBOUNDED)? {
                t.from.walk(r, rest, depth)?;
            }
        }
        if let Some(first) = first {
            write_varint(out, first as i32 as u32);
        }
        Ok(first.is_some())
    }),
];

/// 1.21.1's trim material keyed its overrides by holder, and its instrument
/// had a use duration in ticks and no description.
const FROM_1_21_1: Conversions = &[
    ("trim", |t, r, out, depth| {
        trim_to_26_3(t, r, out, depth, TrimMaterialLayout::V1_21_1)
    }),
    ("instrument", |t, r, out, depth| {
        let holder = r.varint()?;
        write_varint(out, holder);
        if holder != 0 {
            return Ok(true);
        }
        let carried = t.value(r, out, &SOUND, depth)?;
        // Ticks to seconds (Instrument.java), then the range.
        out.extend((r.var_i32()? as f32 / 20.0).to_be_bytes());
        out.extend_from_slice(r.bytes(4)?);
        // No durability damage, and an empty description.
        out.extend([0, 8, 0, 0]);
        Ok(carried)
    }),
];

const TO_1_21_1: Conversions = &[
    ("trim", |t, r, out, depth| {
        trim_from_26_3(t, r, out, depth, TrimMaterialLayout::V1_21_1)
    }),
    ("instrument", |t, r, out, depth| {
        let holder = r.varint()?;
        write_varint(out, holder);
        if holder != 0 {
            return Ok(true);
        }
        let carried = t.value(r, out, &SOUND, depth)?;
        let seconds = f32::from_be_bytes(r.bytes(4)?.try_into().expect("four bytes"));
        write_varint(out, (seconds * 20.0) as i32 as u32);
        out.extend_from_slice(r.bytes(4)?);
        r.varint()?; // durability damage
        t.from.walk(r, &TEXT, depth)?;
        Ok(carried)
    }),
];

/// 1.20.6's attribute modifiers were named by a UUID and a name; 1.21 made
/// that one id, here `minecraft:<uuid>` as `AttributeModifierIdFix` gives a
/// modifier it doesn't know.
const FROM_1_20_6: Conversions = &[("attribute_modifiers", |_, r, out, _| {
    let entries = r.len(UNBOUNDED)?;
    write_varint(out, entries as u32);
    for _ in 0..entries {
        write_varint(out, r.varint()?); // attribute
        let uuid = r.uuid()?;
        r.utf(MAX_STRING_LENGTH)?; // name
        let hex = format!("{uuid:032x}");
        let id = format!(
            "minecraft:{}-{}-{}-{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..]
        );
        crate::wire::write_utf(out, &id);
        out.extend_from_slice(r.bytes(8)?); // amount
        write_varint(out, r.varint()?); // operation
        write_varint(out, r.varint()?); // slot group
        out.push(0); // the default display
    }
    r.bool()?; // showInTooltip
    Ok(true)
})];

// TODO: a later modifier's id has no UUID to go back to; the component is
// left out of a stack sent to 1.20.6.
const TO_1_20_6: Conversions = &[("attribute_modifiers", |t, r, _, depth| {
    let native = t
        .from
        .registries
        .id_of(ClientRegistry::DataComponentType, "attribute_modifiers")
        .and_then(|id| t.from.component_shape(id))
        .ok_or_else(|| r.error("no attribute modifiers shape"))?;
    t.from.walk(r, native, depth)?;
    Ok(false)
})];

/// 1.21.8's profile as the later layout's partial arm with an empty skin
/// patch.
const FROM_1_21_8: Conversions = &[("profile", |t, r, out, depth| {
    out.push(0);
    let carried = t.value(r, out, &PROFILE_1_21_8, depth)?;
    out.extend([0; 4]);
    Ok(carried)
})];

/// A later profile as 1.21.8's: a full profile's id and name become the
/// optionals, and the skin patch has no place.
const TO_1_21_8: Conversions = &[("profile", |t, r, out, _| {
    let [profile, skin_patch] = match &RESOLVABLE_PROFILE {
        Struct([Either(full, partial), skin_patch]) => {
            [if r.bool()? { *full } else { *partial }, skin_patch]
        }
        _ => unreachable!("the profile is an either and a skin patch"),
    };
    if let Struct([Uuid, name, _]) = profile {
        let id = r.bytes(16)?;
        let start = r.pos();
        t.from.walk(r, name, 0)?;
        out.push(1);
        out.extend_from_slice(r.since(start));
        out.push(1);
        out.extend_from_slice(id);
        let start = r.pos();
        t.from.walk(r, &PROFILE_PROPERTIES, 0)?;
        out.extend_from_slice(r.since(start));
    } else {
        let start = r.pos();
        t.from.walk(r, profile, 0)?;
        out.extend_from_slice(r.since(start));
    }
    t.from.walk(r, skin_patch, 0)?;
    Ok(true)
})];

/// 1.21.11's components that need more than the generic conversion on the
/// way to 26.3: an `EitherHolder` around a layout 26.3 also changed.
const FROM_1_21_11: Conversions = &[
    ("instrument", |t, r, out, depth| {
        t.either_holder(r, |r| instrument_26_3(t, r, out, depth))
    }),
    ("provides_trim_material", |t, r, out, depth| {
        t.either_holder(r, |r| trim_material_26_3(t, r, out, depth))
    }),
];

/// The same two, from 26.3 into 1.21.11's `EitherHolder`.
const TO_1_21_11: Conversions = &[
    ("instrument", |t, r, out, depth| {
        out.push(1);
        instrument_26_2(t, r, out, depth)
    }),
    ("provides_trim_material", |t, r, out, depth| {
        out.push(1);
        trim_material_26_2(t, r, out, depth)
    }),
];

/// 26.3's changed components in 26.2's layouts.
const TO_26_2: Conversions = &[
    ("consumable", |t, r, out, depth| {
        consumable(t, r, out, depth, consume_effects_26_2)
    }),
    ("death_protection", consume_effects_26_2),
    ("trim", |t, r, out, depth| {
        trim(t, r, out, depth, trim_material_26_2)
    }),
    ("provides_trim_material", trim_material_26_2),
    ("instrument", instrument_26_2),
    ("pot_decorations", pot_decorations_26_2),
    ("swing_animation", swing_animation),
];

/// The same components of 26.2 and 26.1 in 26.3's layouts.
const TO_26_3: Conversions = &[
    ("consumable", |t, r, out, depth| {
        consumable(t, r, out, depth, consume_effects_26_3)
    }),
    ("death_protection", consume_effects_26_3),
    ("trim", |t, r, out, depth| {
        trim(t, r, out, depth, trim_material_26_3)
    }),
    ("provides_trim_material", trim_material_26_3),
    ("instrument", instrument_26_3),
    ("pot_decorations", pot_decorations_26_3),
    ("swing_animation", swing_animation),
];

impl Translator<'_> {
    /// An `EitherHolder`'s holder converted by `holder`; a bare key can't be
    /// resolved, and takes the component with it.
    fn either_holder(
        &self,
        r: &mut Reader,
        holder: impl FnOnce(&mut Reader) -> DecodeResult<bool>,
    ) -> DecodeResult<bool> {
        if r.bool()? {
            holder(r)
        } else {
            r.identifier()?;
            Ok(false)
        }
    }
}

/// 26.2's `swing_animation` and 26.3's `attack_animation`, one layout under
/// two names.
fn swing_animation(
    t: &Translator,
    r: &mut Reader,
    out: &mut Vec<u8>,
    depth: u32,
) -> DecodeResult<bool> {
    t.value(r, out, &SWING_ANIMATION, depth)
}

/// `consumable`: only its effects differ between versions, rewritten by
/// `effects`.
fn consumable(
    t: &Translator,
    r: &mut Reader,
    out: &mut Vec<u8>,
    depth: u32,
    effects: Conversion,
) -> DecodeResult<bool> {
    let head = t.value(r, out, &CONSUMABLE_HEAD, depth)?;
    Ok(effects(t, r, out, depth)? && head)
}

/// A list of consume effects, dropping 26.3's `directionalParticles` from
/// `teleport_randomly` (TeleportRandomlyConsumeEffect.java:42).
fn consume_effects_26_2(
    t: &Translator,
    r: &mut Reader,
    out: &mut Vec<u8>,
    depth: u32,
) -> DecodeResult<bool> {
    consume_effects(t, r, out, depth, |r, out| {
        out.extend_from_slice(r.bytes(4)?); // diameter
        r.bool()?;
        Ok(())
    })
}

/// A list of consume effects, giving `teleport_randomly` 26.3's
/// `directionalParticles` as false: a 26.2 server sends no particle event
/// for the teleport.
fn consume_effects_26_3(
    t: &Translator,
    r: &mut Reader,
    out: &mut Vec<u8>,
    depth: u32,
) -> DecodeResult<bool> {
    consume_effects(t, r, out, depth, |r, out| {
        out.extend_from_slice(r.bytes(4)?); // diameter
        out.push(0);
        Ok(())
    })
}

/// A list of consume effects with each `teleport_randomly` rewritten by
/// `teleport_randomly`, the one effect whose layout differs.
fn consume_effects(
    t: &Translator,
    r: &mut Reader,
    out: &mut Vec<u8>,
    depth: u32,
    teleport_randomly: fn(&mut Reader, &mut Vec<u8>) -> DecodeResult<()>,
) -> DecodeResult<bool> {
    let ByRegistry(registry, arms) = &CONSUME_EFFECT else {
        unreachable!("consume effects dispatch by type");
    };
    let effects = r.len(UNBOUNDED)?;
    write_varint(out, effects as u32);
    let mut carried = true;
    for _ in 0..effects {
        let id = r.varint()?;
        let name = t
            .from
            .registries
            .name_of(*registry, id)
            .ok_or_else(|| r.error("unknown dispatch type"))?;
        let target = t.remaps.remap(*registry, id);
        write_varint(out, target.unwrap_or(0));
        carried &= target.is_some();
        if name == "teleport_randomly" {
            teleport_randomly(r, out)?;
            continue;
        }
        let arm = arms
            .iter()
            .find(|(arm, _)| *arm == name)
            .map(|(_, shape)| shape)
            .ok_or_else(|| r.error("unported dispatch type"))?;
        carried &= t.value(r, out, arm, depth)?;
    }
    Ok(carried)
}

/// `trim`: the material, rewritten by `material`, then the pattern
/// (unchanged).
fn trim(
    t: &Translator,
    r: &mut Reader,
    out: &mut Vec<u8>,
    depth: u32,
    material: Conversion,
) -> DecodeResult<bool> {
    let material = material(t, r, out, depth)?;
    let pattern = t.value(r, out, &TRIM_PATTERN, depth)?;
    Ok(material && pattern)
}

/// A trim material holder; an inline one's asset group becomes the palette
/// id of its base suffix (`trim/<suffix>`, TrimMaterials.java:75). The
/// group's per-equipment overrides have no place in 26.3's layout.
fn trim_material_26_3(
    t: &Translator,
    r: &mut Reader,
    out: &mut Vec<u8>,
    depth: u32,
) -> DecodeResult<bool> {
    trim_material_to_26_3(t, r, out, depth, TrimMaterialLayout::LATER)
}

/// What an inline trim material carries besides its asset name, overrides
/// and description, by version.
#[derive(Clone, Copy)]
struct TrimMaterialLayout {
    /// Through 1.21.4, the ingredient item after the name.
    ingredient: bool,
    /// Through 1.21.3, the item model index after that.
    model_index: bool,
    /// Through 1.21.1, overrides keyed by an armor material holder where
    /// later versions key them by id.
    holder_keys: bool,
}

impl TrimMaterialLayout {
    const LATER: Self = Self {
        ingredient: false,
        model_index: false,
        holder_keys: false,
    };
    const V1_21_4: Self = Self {
        ingredient: true,
        ..Self::LATER
    };
    const V1_21_3: Self = Self {
        model_index: true,
        ..Self::V1_21_4
    };
    const V1_21_1: Self = Self {
        holder_keys: true,
        ..Self::V1_21_3
    };
}

/// [`trim_material_26_3`], from any older `layout`.
fn trim_material_to_26_3(
    t: &Translator,
    r: &mut Reader,
    out: &mut Vec<u8>,
    depth: u32,
    layout: TrimMaterialLayout,
) -> DecodeResult<bool> {
    let holder = r.varint()?;
    write_varint(out, holder);
    if holder != 0 {
        return Ok(true);
    }
    let suffix = r.utf(MAX_STRING_LENGTH)?;
    if layout.ingredient {
        r.varint()?;
    }
    if layout.model_index {
        r.bytes(4)?;
    }
    crate::wire::write_utf(out, &format!("minecraft:trim/{suffix}"));
    for _ in 0..r.len(UNBOUNDED)? {
        if layout.holder_keys {
            r.varint()?;
        } else {
            r.identifier()?;
        }
        r.utf(MAX_STRING_LENGTH)?;
    }
    t.value(r, out, &TEXT, depth)
}

/// The trim materials whose 26.2 asset group swaps in a darker texture on
/// armour of the same material (MaterialAssetGroup.java:38-44).
const DARKER_TRIMS: [&str; 5] = ["iron", "netherite", "copper", "gold", "diamond"];

/// A trim material holder; an inline one's palette id (`trim/<name>`,
/// TrimMaterials.java:75) becomes 26.2's asset group for `<name>`, with the
/// darker override 26.2 gives that material.
fn trim_material_26_2(
    t: &Translator,
    r: &mut Reader,
    out: &mut Vec<u8>,
    depth: u32,
) -> DecodeResult<bool> {
    trim_material_to_26_2(t, r, out, depth, TrimMaterialLayout::LATER)
}

/// [`trim_material_26_2`], into any older `layout`: the ingredient is air,
/// the model index zero, and overrides keyed by a holder are left out.
fn trim_material_to_26_2(
    t: &Translator,
    r: &mut Reader,
    out: &mut Vec<u8>,
    depth: u32,
    layout: TrimMaterialLayout,
) -> DecodeResult<bool> {
    let holder = r.varint()?;
    write_varint(out, holder);
    if holder != 0 {
        return Ok(true);
    }
    let palette = r.identifier()?;
    let name = palette.rsplit(['/', ':']).next().unwrap_or(palette);
    crate::wire::write_utf(out, name);
    if layout.ingredient {
        write_varint(out, 0);
    }
    if layout.model_index {
        out.extend([0; 4]);
    }
    if DARKER_TRIMS.contains(&name) && !layout.holder_keys {
        write_varint(out, 1);
        crate::wire::write_utf(out, &format!("minecraft:{name}"));
        crate::wire::write_utf(out, &format!("{name}_darker"));
    } else {
        write_varint(out, 0);
    }
    t.value(r, out, &TEXT, depth)
}

/// 26.2's instrument in 26.3's layout, whose `durabilityDamage`
/// (Instrument.java:29) is zero.
fn instrument_26_3(
    t: &Translator,
    r: &mut Reader,
    out: &mut Vec<u8>,
    depth: u32,
) -> DecodeResult<bool> {
    instrument(t, r, out, depth, |_, out| {
        write_varint(out, 0);
        Ok(())
    })
}

/// The reverse, dropping it.
fn instrument_26_2(
    t: &Translator,
    r: &mut Reader,
    out: &mut Vec<u8>,
    depth: u32,
) -> DecodeResult<bool> {
    instrument(t, r, out, depth, |r, _| r.varint().map(drop))
}

/// An instrument holder; `durability_damage` rewrites the one field an
/// inline one differs by between versions, which sits after the range.
fn instrument(
    t: &Translator,
    r: &mut Reader,
    out: &mut Vec<u8>,
    depth: u32,
    durability_damage: fn(&mut Reader, &mut Vec<u8>) -> DecodeResult<()>,
) -> DecodeResult<bool> {
    let holder = r.varint()?;
    write_varint(out, holder);
    if holder != 0 {
        return Ok(true);
    }
    let mut carried = t.value(r, out, &SOUND, depth)?;
    out.extend_from_slice(r.bytes(8)?); // use duration, range
    durability_damage(r, out)?;
    carried &= t.value(r, out, &TEXT, depth)?;
    Ok(carried)
}

/// `pot_decorations`: 26.2's list of up to four item ids (back, left, right,
/// front) as 26.3's four optional templates of one item each. A brick, a
/// missing side and an item 26.3 lacks are all absent, as in 26.2's
/// `PotDecorations.getItem`.
fn pot_decorations_26_3(
    t: &Translator,
    r: &mut Reader,
    out: &mut Vec<u8>,
    _depth: u32,
) -> DecodeResult<bool> {
    let brick = t.from.registries.id_of(ClientRegistry::Item, "brick");
    let sides = r.len(4)?;
    for side in 0..4 {
        let item = if side < sides {
            let id = r.varint()?;
            Some(id)
                .filter(|id| Some(*id) != brick)
                .and_then(|id| t.remaps.remap(ClientRegistry::Item, id))
        } else {
            None
        };
        match item {
            Some(item) => {
                out.push(1);
                write_varint(out, item);
                // One of the item, with an empty patch.
                out.extend([1, 0, 0]);
            }
            None => out.push(0),
        }
    }
    Ok(true)
}

/// `pot_decorations`: 26.3's four optional templates (back, left, right,
/// front) as 26.2's list of four item ids, an absent or unknown side being a
/// brick as in `PotDecorations.ordered`.
fn pot_decorations_26_2(
    t: &Translator,
    r: &mut Reader,
    out: &mut Vec<u8>,
    depth: u32,
) -> DecodeResult<bool> {
    let brick =
        t.to.registries
            .id_of(ClientRegistry::Item, "brick")
            .ok_or_else(|| r.error("no brick item"))?;
    write_varint(out, 4);
    for _ in 0..4 {
        let mut item = brick;
        if r.bool()? {
            if let Some(id) = t.item(r)? {
                item = id;
            }
            r.var_i32()?; // count
            t.patch(r, &mut Vec::new(), depth)?;
        }
        write_varint(out, item);
    }
    Ok(true)
}

/// Writes `ItemStack.OPTIONAL_STREAM_CODEC`; see [`write_patch`].
pub fn write_stack(buf: &mut Vec<u8>, stack: Option<&ItemStack>, delimited: bool) {
    let Some(stack) = stack else {
        write_varint(buf, 0);
        return;
    };
    write_varint(buf, stack.count as u32);
    write_varint(buf, stack.item);
    write_patch(buf, &stack.patch, delimited);
}

/// Writes `ItemStackTemplate.STREAM_CODEC`.
pub fn write_template(buf: &mut Vec<u8>, template: &ItemStack) {
    write_varint(buf, template.item);
    write_varint(buf, template.count as u32);
    write_patch(buf, &template.patch, false);
}

/// Writes `DataComponentPatch.STREAM_CODEC`, or with `delimited` its
/// `DELIMITED_STREAM_CODEC`, which length-prefixes each value.
pub fn write_patch(buf: &mut Vec<u8>, patch: &ComponentPatch, delimited: bool) {
    write_varint(buf, patch.added.len() as u32);
    write_varint(buf, patch.removed.len() as u32);
    for (id, value) in &patch.added {
        write_varint(buf, *id);
        if delimited {
            write_varint(buf, value.len() as u32);
        }
        buf.extend_from_slice(value);
    }
    for id in &patch.removed {
        write_varint(buf, *id);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const PROTOCOL_26_2: i32 = 776;

    pub(crate) fn codec() -> &'static ItemCodec {
        ItemCodec::for_protocol(PROTOCOL_26_2).expect("26.2 item grammar")
    }

    pub(crate) fn id(registry: ClientRegistry, name: &str) -> u32 {
        codec()
            .registries()
            .id_of(registry, name)
            .unwrap_or_else(|| panic!("{name} in 26.2"))
    }

    fn component(name: &str) -> u32 {
        id(ClientRegistry::DataComponentType, name)
    }

    fn item(name: &str) -> u32 {
        id(ClientRegistry::Item, name)
    }

    /// Wire bytes built field by field from the vanilla codecs.
    #[derive(Default)]
    struct Wire(Vec<u8>);

    impl Wire {
        fn var(mut self, v: u32) -> Self {
            write_varint(&mut self.0, v);
            self
        }
        fn bool(mut self, v: bool) -> Self {
            self.0.push(v as u8);
            self
        }
        fn f32(mut self, v: f32) -> Self {
            self.0.extend(v.to_be_bytes());
            self
        }
        fn f64(mut self, v: f64) -> Self {
            self.0.extend(v.to_be_bytes());
            self
        }
        fn i32(mut self, v: i32) -> Self {
            self.0.extend(v.to_be_bytes());
            self
        }
        fn str(mut self, s: &str) -> Self {
            crate::wire::write_utf(&mut self.0, s);
            self
        }
        fn bytes(mut self, b: &[u8]) -> Self {
            self.0.extend(b);
            self
        }
        /// An NBT string tag: the text component `"hi"`.
        fn text(self) -> Self {
            self.bytes(&[8, 0, 2, b'h', b'i'])
        }
        fn empty_compound(self) -> Self {
            self.bytes(&[10, 0])
        }
        /// A patch holding one component value.
        fn patch(self, name: &str, value: Wire) -> Self {
            self.var(1).var(0).var(component(name)).bytes(&value.0)
        }
        fn template(self, item_name: &str, count: u32) -> Self {
            self.var(item(item_name)).var(count).var(0).var(0)
        }
    }

    /// Decodes a one-component patch and checks it spans exactly `value`.
    fn assert_component(name: &str, value: Wire) {
        let bytes = Wire::default().patch(name, Wire(value.0.clone())).0;
        let mut r = Reader::new(&bytes);
        let patch = codec()
            .read_patch(&mut r, 0)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(r.ensure_eof(), Ok(()), "{name} left bytes");
        assert_eq!(patch.added, [(component(name), value.0.into())], "{name}");
    }

    #[test]
    fn every_26_2_component_has_a_shape() {
        let names = codec()
            .registries()
            .names(ClientRegistry::DataComponentType);
        assert_eq!(names.len(), 111);
        for (name, _) in COMPONENTS_26_2 {
            assert!(
                names.iter().any(|n| n == name),
                "{name} isn't a 26.2 component"
            );
        }
    }

    #[test]
    fn particle_options_name_26_2_particle_types() {
        let ByRegistryOr(registry, arms, _) = &PARTICLE else {
            unreachable!()
        };
        let names = codec().registries().names(*registry);
        assert_eq!(names.len(), 125);
        for (name, _) in *arms {
            assert!(
                names.iter().any(|n| n == name),
                "{name} isn't a 26.2 particle type"
            );
        }
    }

    #[test]
    fn item_particles_carry_a_template() {
        let mut bytes = Wire::default()
            .var(id(ClientRegistry::ParticleType, "item"))
            .var(item("stick"))
            .var(1)
            .var(0)
            .var(0)
            .0;
        bytes.push(0xEE);
        let mut r = Reader::new(&bytes);
        codec().skip(&mut r, &PARTICLE).expect("item particle");
        assert_eq!(r.u8(), Ok(0xEE));
    }

    #[test]
    fn optional_stacks_round_trip() {
        assert_eq!(codec().read_stack(&mut Reader::new(&[0])), Ok(None));

        let enchantments = Wire::default().var(1).var(5).var(3);
        let bytes = Wire::default()
            .var(2)
            .var(item("diamond_sword"))
            .var(2)
            .var(1)
            .var(component("enchantments"))
            .bytes(&enchantments.0)
            .var(component("custom_name"))
            .text()
            .var(component("damage"))
            .0;
        let stack = codec()
            .read_stack(&mut Reader::new(&bytes))
            .unwrap()
            .unwrap();
        assert_eq!((stack.item, stack.count), (item("diamond_sword"), 2));
        assert_eq!(stack.patch.removed, [component("damage")]);
        let mut out = Vec::new();
        write_stack(&mut out, Some(&stack), false);
        assert_eq!(out, bytes);
    }

    #[test]
    fn delimited_patches_length_prefix_each_value() {
        let patch = ComponentPatch {
            added: vec![(component("max_stack_size"), [16].into())],
            removed: vec![component("lore")],
        };
        let mut out = Vec::new();
        write_patch(&mut out, &patch, true);
        let expected = Wire::default()
            .var(1)
            .var(1)
            .var(component("max_stack_size"))
            .var(1)
            .var(16)
            .var(component("lore"))
            .0;
        assert_eq!(out, expected);
    }

    #[test]
    fn templates_are_item_then_count_and_never_empty() {
        let bytes = Wire::default().template("stone", 3).0;
        let t = codec().read_template(&mut Reader::new(&bytes)).unwrap();
        assert_eq!((t.item, t.count), (item("stone"), 3));
        let mut out = Vec::new();
        write_template(&mut out, &t);
        assert_eq!(out, bytes);

        for bad in [
            Wire::default().template("air", 1),
            Wire::default().template("stone", 0),
        ] {
            assert!(codec().read_template(&mut Reader::new(&bad.0)).is_err());
        }
    }

    #[test]
    fn nested_item_components() {
        // A container holding an empty slot and a bundle with one stone.
        let bundle = Wire::default().var(1).template("stone", 1);
        let nested = Wire::default()
            .var(item("bundle"))
            .var(1)
            .patch("bundle_contents", bundle);
        assert_component(
            "container",
            Wire::default()
                .var(2)
                .bool(false)
                .bool(true)
                .bytes(&nested.0),
        );
        assert_component("use_remainder", Wire::default().template("bowl", 1));
        assert_component(
            "charged_projectiles",
            Wire::default().var(1).template("arrow", 1),
        );
    }

    #[test]
    fn block_predicates_with_component_matchers() {
        let predicate = Wire::default()
            // blocks: a direct set of one block
            .bool(true)
            .var(2)
            .var(1)
            // properties: facing=north, then an empty range
            .bool(true)
            .var(2)
            .str("facing")
            .bool(true)
            .str("north")
            .str("age")
            .bool(false)
            .bool(true)
            .str("1")
            .bool(false)
            // no nbt
            .bool(false)
            // exact: damage = 3; partial: "has custom_data" by component type
            .var(1)
            .var(component("damage"))
            .var(3)
            .var(1)
            .bool(false)
            .var(component("custom_data"))
            .empty_compound();
        assert_component("can_place_on", Wire::default().var(1).bytes(&predicate.0));
    }

    #[test]
    fn attribute_modifier_displays() {
        let entry = |display: Wire| {
            Wire::default()
                .var(0)
                .str("minecraft:base")
                .f64(1.5)
                .var(0)
                .var(1)
                .bytes(&display.0)
        };
        let mut list = Wire::default().var(3);
        for display in [
            Wire::default().var(0),
            Wire::default().var(1),
            Wire::default().var(2).text(),
        ] {
            list = list.bytes(&entry(display).0);
        }
        assert_component("attribute_modifiers", list);
    }

    #[test]
    fn consumables_with_every_effect() {
        let effect = |name: &str| Wire::default().var(id(ClientRegistry::ConsumeEffectType, name));
        let details = |hidden: Option<Wire>| {
            let w = Wire::default()
                .var(1)
                .var(200)
                .bool(false)
                .bool(true)
                .bool(true);
            match hidden {
                Some(h) => w.bool(true).bytes(&h.0),
                None => w.bool(false),
            }
        };
        let effects = Wire::default()
            .var(5)
            .bytes(&effect("apply_effects").0)
            .var(1)
            .var(3)
            .bytes(&details(Some(details(None))).0)
            .f32(0.5)
            .bytes(&effect("remove_effects").0)
            .var(0)
            .str("minecraft:harmful")
            .bytes(&effect("clear_all_effects").0)
            .bytes(&effect("teleport_randomly").0)
            .f32(16.0)
            .bytes(&effect("play_sound").0)
            .var(0)
            .str("minecraft:custom")
            .bool(true)
            .f32(8.0);
        assert_component(
            "consumable",
            Wire::default()
                .f32(1.6)
                .var(1)
                .var(10)
                .bool(true)
                .bytes(&effects.0),
        );
    }

    #[test]
    fn written_books_and_profiles() {
        assert_component(
            "written_book_content",
            Wire::default()
                .str("title")
                .bool(false)
                .str("author")
                .var(0)
                .var(1)
                .text()
                .bool(true)
                .text()
                .bool(true),
        );
        let properties = Wire::default()
            .var(1)
            .str("textures")
            .str("e30=")
            .bool(false);
        assert_component(
            "profile",
            Wire::default()
                .bool(true)
                .bytes(&[0; 16])
                .str("Steve")
                .bytes(&properties.0)
                .bool(false)
                .bool(false)
                .bool(false)
                .bool(true)
                .bool(true),
        );
        assert_component(
            "profile",
            Wire::default()
                .bool(false)
                .bool(true)
                .str("Alex")
                .bool(false)
                .var(0)
                .bool(true)
                .str("minecraft:skin")
                .bool(false)
                .bool(false)
                .bool(false),
        );
    }

    #[test]
    fn equipment_and_combat_components() {
        assert_component(
            "equippable",
            Wire::default()
                .var(4)
                .var(12)
                .bool(true)
                .str("minecraft:diamond")
                .bool(false)
                .bool(true)
                .var(0)
                .str("minecraft:can_wear_horse_armor")
                .bool(true)
                .bool(true)
                .bool(false)
                .bool(false)
                .bool(true)
                .var(0)
                .str("minecraft:shear")
                .bool(false),
        );
        assert_component(
            "blocks_attacks",
            Wire::default()
                .f32(0.25)
                .f32(1.0)
                .var(1)
                .f32(90.0)
                .bool(false)
                .f32(0.0)
                .f32(1.0)
                .f32(3.0)
                .f32(1.0)
                .f32(1.0)
                .bool(false)
                .bool(true)
                .var(7)
                .bool(false),
        );
    }

    #[test]
    fn fireworks_and_banners() {
        let explosion = Wire::default()
            .var(1)
            .var(2)
            .i32(0xFF0000)
            .i32(0x00FF00)
            .var(0)
            .bool(true)
            .bool(false);
        assert_component(
            "fireworks",
            Wire::default().var(2).var(1).bytes(&explosion.0),
        );
        assert_component(
            "banner_patterns",
            Wire::default()
                .var(2)
                .var(3)
                .var(14)
                .var(0)
                .str("minecraft:custom")
                .str("block.minecraft.banner.custom")
                .var(1),
        );
    }

    #[test]
    fn hostile_input_fails_cleanly() {
        // Bundles nested past the depth cap.
        let mut bytes = Vec::new();
        for _ in 0..600 {
            write_varint(&mut bytes, 1);
            write_varint(&mut bytes, 0);
            write_varint(&mut bytes, component("bundle_contents"));
            write_varint(&mut bytes, 1);
            write_varint(&mut bytes, item("bundle"));
            write_varint(&mut bytes, 1);
        }
        bytes.extend([0, 0]);
        let err = codec().read_patch(&mut Reader::new(&bytes), 0).unwrap_err();
        assert!(err.what.contains("too deeply"), "{err}");

        let unknown = Wire::default().var(1).var(0).var(999).0;
        assert!(codec().read_patch(&mut Reader::new(&unknown), 0).is_err());

        let truncated = Wire::default()
            .var(1)
            .var(0)
            .var(component("custom_name"))
            .0;
        assert!(codec().read_patch(&mut Reader::new(&truncated), 0).is_err());

        let end_text = Wire::default()
            .patch("custom_name", Wire::default().bytes(&[0]))
            .0;
        assert!(codec().read_patch(&mut Reader::new(&end_text), 0).is_err());
    }

    const PROTOCOL_26_3: i32 = 777;

    pub(crate) fn codec_26_3() -> &'static ItemCodec {
        ItemCodec::for_protocol(PROTOCOL_26_3).expect("26.3 item grammar")
    }

    pub(crate) fn id_26_3(registry: ClientRegistry, name: &str) -> u32 {
        codec_26_3()
            .registries()
            .id_of(registry, name)
            .unwrap_or_else(|| panic!("{name} in 26.3"))
    }

    fn component_26_3(name: &str) -> u32 {
        id_26_3(ClientRegistry::DataComponentType, name)
    }

    fn item_26_3(name: &str) -> u32 {
        id_26_3(ClientRegistry::Item, name)
    }

    /// Decodes a one-component 26.3 patch and checks it spans exactly `value`.
    fn assert_component_26_3(name: &str, value: Wire) {
        let bytes = Wire::default()
            .var(1)
            .var(0)
            .var(component_26_3(name))
            .bytes(&value.0)
            .0;
        let mut r = Reader::new(&bytes);
        codec_26_3()
            .read_patch(&mut r, 0)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(r.ensure_eof(), Ok(()), "{name} left bytes");
    }

    /// The layouts of 26.3's added and changed components, from its
    /// decompiled codecs.
    #[test]
    fn grammar_26_3_reads_its_new_and_changed_components() {
        let animation = || Wire::default().var(1).var(10);
        let resolvable_fuel = || {
            Wire::default()
                .bool(true)
                .i32(200)
                .bool(false)
                .str("minecraft:x")
        };
        let sign = || {
            Wire::default()
                .text()
                .text()
                .text()
                .text()
                .bool(false)
                .var(3)
                .bool(true)
        };
        assert_component_26_3("attack_animation", animation());
        assert_component_26_3("interact_animation", animation());
        assert_component_26_3("block_transformer", Wire::default().var(3));
        assert_component_26_3("villager_food", Wire::default().var(4));
        assert_component_26_3("compostable", Wire::default().bool(true).i32(65));
        assert_component_26_3(
            "compostable",
            Wire::default().bool(false).str("minecraft:x"),
        );
        assert_component_26_3("cooking_fuel", resolvable_fuel());
        assert_component_26_3("brewing_fuel", resolvable_fuel());
        assert_component_26_3("mob_visibility", Wire::default().var(2).var(5).f32(0.5));
        assert_component_26_3("provides_pottery_pattern", Wire::default().var(1));
        assert_component_26_3("sign_text_front", sign());
        assert_component_26_3("sign_text_back", sign());
        assert_component_26_3("waxed", Wire::default());
        assert_component_26_3("cushion/color", Wire::default().var(2));
        // Four optional templates: back only.
        let pot = Wire::default()
            .bool(true)
            .var(item_26_3("brick"))
            .var(1)
            .var(0)
            .var(0)
            .bool(false)
            .bool(false)
            .bool(false);
        assert_component_26_3("pot_decorations", pot);
        // An inline instrument: sound (inline location, no range), use
        // duration, range, durability damage, description.
        let instrument = Wire::default()
            .var(0)
            .var(0)
            .str("minecraft:goat")
            .bool(false)
            .f32(7.0)
            .f32(256.0)
            .var(1)
            .text();
        assert_component_26_3("instrument", instrument);
        let teleport = id_26_3(ClientRegistry::ConsumeEffectType, "teleport_randomly");
        let consumable = Wire::default()
            .f32(1.6)
            .var(1)
            .var(1)
            .bool(true)
            .var(1)
            .var(teleport)
            .f32(16.0)
            .bool(true);
        assert_component_26_3("consumable", consumable);
        let trim = Wire::default().var(0).str("minecraft:gold").text().var(1);
        assert_component_26_3("trim", trim);
    }

    /// A 26.3 bundle comes out in 26.2's ids: its item and component ids
    /// remapped, a nested item 26.2 lacks left out, a component 26.2 lacks
    /// (`waxed`) dropped and one it lays out differently (`pot_decorations`)
    /// converted.
    #[test]
    fn translates_a_26_3_stack_into_26_2_ids() {
        let wire = Wire::default()
            .var(1)
            .var(item_26_3("bundle"))
            .var(4)
            .var(1)
            .var(component_26_3("dyed_color"))
            .i32(0x112233)
            .var(component_26_3("bundle_contents"))
            .var(2)
            .var(item_26_3("diamond"))
            .var(3)
            .var(0)
            .var(0)
            .var(item_26_3("poplar_planks"))
            .var(1)
            .var(0)
            .var(0)
            .var(component_26_3("waxed"))
            .var(component_26_3("pot_decorations"))
            .bool(false)
            .bool(false)
            .bool(false)
            .bool(false)
            .var(component_26_3("max_stack_size"))
            .0;
        let mut out = Vec::new();
        let mut r = Reader::new(&wire);
        let remaps = RegistryRemaps::from_native(PROTOCOL_26_2).unwrap();
        codec_26_3()
            .translate_stack(&mut r, &mut out, codec(), remaps)
            .unwrap();
        assert_eq!(r.ensure_eof(), Ok(()));
        let brick = item("brick");
        let native = Wire::default()
            .var(1)
            .var(item("bundle"))
            .var(3)
            .var(1)
            .var(component("dyed_color"))
            .i32(0x112233)
            .var(component("bundle_contents"))
            .var(1)
            .template("diamond", 3)
            .var(component("pot_decorations"))
            .var(4)
            .var(brick)
            .var(brick)
            .var(brick)
            .var(brick)
            .var(component("max_stack_size"))
            .0;
        assert_eq!(out, native);
        codec().read_stack(&mut Reader::new(&out)).unwrap();
    }

    /// A stack of an item 26.2 lacks arrives empty.
    #[test]
    fn a_26_3_only_item_translates_to_an_empty_stack() {
        let wire = Wire::default()
            .var(5)
            .var(item_26_3("poplar_planks"))
            .var(0)
            .var(0)
            .0;
        let mut out = Vec::new();
        let remaps = RegistryRemaps::from_native(PROTOCOL_26_2).unwrap();
        codec_26_3()
            .translate_stack(&mut Reader::new(&wire), &mut out, codec(), remaps)
            .unwrap();
        assert_eq!(out, [0]);
    }

    /// A native patch adding just `name` with `value`.
    fn one_component(name: &str, value: Wire) -> Vec<u8> {
        Wire::default().patch(name, value).0
    }

    /// Translates a one-component 26.3 patch and returns the 26.2 patch.
    fn translate_component_26_3(name: &str, value: Wire) -> Vec<u8> {
        let wire = one_component_26_3(name, value);
        let mut r = Reader::new(&wire);
        let mut out = Vec::new();
        let remaps = RegistryRemaps::from_native(PROTOCOL_26_2).unwrap();
        codec_26_3()
            .translate_patch(&mut r, &mut out, codec(), remaps)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(r.ensure_eof(), Ok(()), "{name} left bytes");
        codec()
            .read_patch(&mut Reader::new(&out), 0)
            .unwrap_or_else(|e| panic!("{name} as 26.2: {e}"));
        out
    }

    /// Sound and entity type ids remap like item ids; a sound 26.2 lacks
    /// takes its component with it.
    #[test]
    fn remaps_sound_and_entity_type_ids_in_values() {
        let sound = |id: u32| Wire::default().var(id + 1);
        assert_eq!(
            translate_component_26_3(
                "break_sound",
                sound(id_26_3(ClientRegistry::SoundEvent, "entity.generic.eat"))
            ),
            one_component(
                "break_sound",
                sound(id(ClientRegistry::SoundEvent, "entity.generic.eat"))
            )
        );
        let egg = |id: u32| Wire::default().var(id).empty_compound();
        assert_eq!(
            translate_component_26_3(
                "entity_data",
                egg(id_26_3(ClientRegistry::EntityType, "zombie"))
            ),
            one_component("entity_data", egg(id(ClientRegistry::EntityType, "zombie")))
        );
        assert_eq!(
            translate_component_26_3(
                "break_sound",
                sound(id_26_3(ClientRegistry::SoundEvent, "entity.cushion.sit"))
            ),
            Wire::default().var(0).var(0).0
        );
    }

    /// 26.3's changed layouts arrive in 26.2's: `teleport_randomly` without
    /// its particle flag, an inline trim material as an asset group, an
    /// inline instrument without its durability damage, and pot sides as
    /// four item ids.
    #[test]
    fn converts_26_3_changed_components_to_26_2_layouts() {
        let teleport = |codec_id: u32| {
            Wire::default()
                .f32(1.6)
                .var(1)
                .var(1)
                .bool(true)
                .var(1)
                .var(codec_id)
                .f32(16.0)
        };
        let teleport_26_3 = id_26_3(ClientRegistry::ConsumeEffectType, "teleport_randomly");
        let teleport_26_2 = id(ClientRegistry::ConsumeEffectType, "teleport_randomly");
        assert_eq!(
            translate_component_26_3("consumable", teleport(teleport_26_3).bool(true)),
            one_component("consumable", teleport(teleport_26_2))
        );

        let trim = |palette| Wire::default().var(0).str(palette).text().var(1);
        let gold_26_2 = Wire::default()
            .var(0)
            .str("gold")
            .var(1)
            .str("minecraft:gold")
            .str("gold_darker")
            .text()
            .var(1);
        assert_eq!(
            translate_component_26_3("trim", trim("minecraft:trim/gold")),
            one_component("trim", gold_26_2)
        );
        let quartz_26_2 = Wire::default().var(0).str("quartz").var(0).text().var(1);
        assert_eq!(
            translate_component_26_3("trim", trim("minecraft:trim/quartz")),
            one_component("trim", quartz_26_2)
        );

        let instrument = |damage: Option<u32>| {
            let w = Wire::default().var(0).var(1).f32(7.0).f32(256.0);
            match damage {
                Some(damage) => w.var(damage),
                None => w,
            }
            .text()
        };
        assert_eq!(
            translate_component_26_3("instrument", instrument(Some(1))),
            one_component("instrument", instrument(None))
        );

        let pot = Wire::default()
            .bool(true)
            .var(item_26_3("diamond"))
            .var(1)
            .var(0)
            .var(0)
            .bool(false)
            .bool(false)
            .bool(false);
        let brick = item("brick");
        assert_eq!(
            translate_component_26_3("pot_decorations", pot),
            one_component(
                "pot_decorations",
                Wire::default()
                    .var(4)
                    .var(item("diamond"))
                    .var(brick)
                    .var(brick)
                    .var(brick)
            )
        );
    }

    /// Translates a one-component 26.2 patch and returns the 26.3 patch,
    /// empty when the component is dropped.
    fn translate_component_26_2(name: &str, value: Wire) -> Vec<u8> {
        let wire = Wire::default().patch(name, value).0;
        let mut r = Reader::new(&wire);
        let mut out = Vec::new();
        let remaps = RegistryRemaps::to_native(PROTOCOL_26_2).unwrap();
        codec()
            .translate_patch(&mut r, &mut out, codec_26_3(), remaps)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(r.ensure_eof(), Ok(()), "{name} left bytes");
        codec_26_3()
            .read_patch(&mut Reader::new(&out), 0)
            .unwrap_or_else(|e| panic!("{name} as 26.3: {e}"));
        out
    }

    /// A 26.3 patch adding just `name` with `value`.
    fn one_component_26_3(name: &str, value: Wire) -> Vec<u8> {
        Wire::default()
            .var(1)
            .var(0)
            .var(component_26_3(name))
            .bytes(&value.0)
            .0
    }

    /// 26.2's layouts arrive in 26.3's: `teleport_randomly` with its particle
    /// flag off, an inline trim material as a palette id, an inline
    /// instrument with no durability damage, and pot sides as four optional
    /// templates. `swing_animation` becomes both components 26.3 split it
    /// into, and `map_color`, which 26.3 dropped, is left out.
    #[test]
    fn converts_26_2_changed_components_to_26_3_layouts() {
        let teleport = |codec_id: u32| {
            Wire::default()
                .f32(1.6)
                .var(1)
                .var(1)
                .bool(true)
                .var(1)
                .var(codec_id)
                .f32(16.0)
        };
        let teleport_26_2 = id(ClientRegistry::ConsumeEffectType, "teleport_randomly");
        let teleport_26_3 = id_26_3(ClientRegistry::ConsumeEffectType, "teleport_randomly");
        assert_eq!(
            translate_component_26_2("consumable", teleport(teleport_26_2)),
            one_component_26_3("consumable", teleport(teleport_26_3).bool(false))
        );

        let gold_26_2 = Wire::default()
            .var(0)
            .str("gold")
            .var(1)
            .str("minecraft:gold")
            .str("gold_darker")
            .text()
            .var(1);
        let gold_26_3 = Wire::default()
            .var(0)
            .str("minecraft:trim/gold")
            .text()
            .var(1);
        assert_eq!(
            translate_component_26_2("trim", gold_26_2),
            one_component_26_3("trim", gold_26_3)
        );

        let instrument = |damage: Option<u32>| {
            let w = Wire::default().var(0).var(1).f32(7.0).f32(256.0);
            match damage {
                Some(damage) => w.var(damage),
                None => w,
            }
            .text()
        };
        assert_eq!(
            translate_component_26_2("instrument", instrument(None)),
            one_component_26_3("instrument", instrument(Some(0)))
        );

        // A diamond at the back, a brick on the left and nothing after it.
        let pot = Wire::default()
            .var(2)
            .var(item("diamond"))
            .var(item("brick"));
        assert_eq!(
            translate_component_26_2("pot_decorations", pot),
            one_component_26_3(
                "pot_decorations",
                Wire::default()
                    .bool(true)
                    .var(item_26_3("diamond"))
                    .var(1)
                    .var(0)
                    .var(0)
                    .bool(false)
                    .bool(false)
                    .bool(false)
            )
        );

        // A stab of ten ticks.
        assert_eq!(
            translate_component_26_2("swing_animation", Wire::default().var(2).var(10)),
            Wire::default()
                .var(2)
                .var(0)
                .var(component_26_3("attack_animation"))
                .var(2)
                .var(10)
                .var(component_26_3("interact_animation"))
                .var(2)
                .var(10)
                .0
        );
        assert_eq!(
            translate_component_26_2("map_color", Wire::default().i32(0x46402e)),
            Wire::default().var(0).var(0).0
        );
    }

    const PROTOCOL_1_21_11: i32 = 774;

    /// An older version's grammar and its ids for a registry entry.
    fn older(protocol: i32) -> &'static ItemCodec {
        ItemCodec::for_protocol(protocol).expect("an item grammar")
    }

    fn older_id(protocol: i32, registry: ClientRegistry, name: &str) -> u32 {
        older(protocol)
            .registries()
            .id_of(registry, name)
            .unwrap_or_else(|| panic!("{name} in {protocol}"))
    }

    /// A patch of `protocol` adding just `name` with `value`.
    fn one_older_component(protocol: i32, name: &str, value: Wire) -> Vec<u8> {
        Wire::default()
            .var(1)
            .var(0)
            .var(older_id(protocol, ClientRegistry::DataComponentType, name))
            .bytes(&value.0)
            .0
    }

    /// Translates `patch` between `protocol`'s grammar and the native one,
    /// towards native with `up`, and checks the result reads back there.
    fn translate_older(protocol: i32, up: bool, patch: &[u8]) -> Vec<u8> {
        let (from, to, remaps) = if up {
            let remaps = RegistryRemaps::to_native(protocol).unwrap();
            (older(protocol), codec_26_3(), remaps)
        } else {
            let remaps = RegistryRemaps::from_native(protocol).unwrap();
            (codec_26_3(), older(protocol), remaps)
        };
        let mut r = Reader::new(patch);
        let mut out = Vec::new();
        from.translate_patch(&mut r, &mut out, to, remaps).unwrap();
        assert_eq!(r.ensure_eof(), Ok(()));
        to.read_patch(&mut Reader::new(&out), 0).unwrap();
        out
    }

    /// Checks that `name` laid out as `old` on `protocol` becomes `new` on
    /// 26.3, and back.
    fn assert_converts_both_ways(protocol: i32, name: &str, old: Wire, new: Wire) {
        let old = one_older_component(protocol, name, old);
        let new = one_component_26_3(name, new);
        assert_eq!(translate_older(protocol, true, &old), new, "{name} up");
        assert_eq!(translate_older(protocol, false, &new), old, "{name} down");
    }

    /// The dropped form of a one-component patch.
    const NO_COMPONENTS: [u8; 2] = [0, 0];

    #[test]
    fn every_older_version_has_a_grammar() {
        for version in [
            "1.20.6", "1.21.1", "1.21.3", "1.21.4", "1.21.5", "1.21.6", "1.21.8", "1.21.10",
            "1.21.11",
        ] {
            let protocol = EMBEDDED
                .iter()
                .find(|e| e.version.name == version)
                .unwrap()
                .version
                .protocol;
            assert!(
                ItemCodec::for_protocol(protocol).is_some(),
                "{version} has a component without a shape"
            );
        }
    }

    /// Through 1.21.11 a component nests whole stacks (count first), where
    /// 26.1 nests templates (item first).
    #[test]
    fn nested_stacks_become_templates_and_back() {
        let p = PROTOCOL_1_21_11;
        let diamond = older_id(p, ClientRegistry::Item, "diamond");
        let stack = |w: Wire| w.var(3).var(diamond).var(0).var(0);
        let template = |w: Wire| w.var(item_26_3("diamond")).var(3).var(0).var(0);

        assert_converts_both_ways(
            p,
            "bundle_contents",
            stack(Wire::default().var(1)),
            template(Wire::default().var(1)),
        );
        assert_converts_both_ways(
            p,
            "use_remainder",
            stack(Wire::default()),
            template(Wire::default()),
        );
        // A filled slot, then an empty one.
        assert_converts_both_ways(
            p,
            "container",
            stack(Wire::default().var(2)).var(0),
            template(Wire::default().var(2).bool(true)).bool(false),
        );
    }

    /// A tag id is a holder set's named form; a set that lists its entries
    /// has no older form.
    #[test]
    fn tags_become_holder_sets_and_back() {
        let p = PROTOCOL_1_21_11;
        let tag = "minecraft:is_fire";
        assert_converts_both_ways(
            p,
            "damage_resistant",
            Wire::default().str(tag),
            Wire::default().var(0).str(tag),
        );
        let listed = one_component_26_3("damage_resistant", Wire::default().var(2).var(4));
        assert_eq!(translate_older(p, false, &listed), NO_COMPONENTS);

        // `bypassedBy`, deep in a struct.
        let blocks = |bypassed_by: Wire| {
            Wire::default()
                .f32(0.25)
                .f32(1.0)
                .var(0)
                .f32(1.0)
                .f32(0.0)
                .f32(1.0)
                .bool(true)
                .bytes(&bypassed_by.0)
                .bool(false)
                .bool(false)
        };
        assert_converts_both_ways(
            p,
            "blocks_attacks",
            blocks(Wire::default().str(tag)),
            blocks(Wire::default().var(0).str(tag)),
        );
    }

    /// An `EitherHolder` holding a holder is that holder; one holding a bare
    /// key can't be resolved and takes its component with it.
    #[test]
    fn either_holders_become_their_holder_and_back() {
        let p = PROTOCOL_1_21_11;
        assert_converts_both_ways(
            p,
            "damage_type",
            Wire::default().bool(true).var(5),
            Wire::default().var(5),
        );
        assert_converts_both_ways(
            p,
            "jukebox_playable",
            Wire::default().bool(true).var(3),
            Wire::default().var(3),
        );
        let key = one_older_component(
            p,
            "damage_type",
            Wire::default().bool(false).str("minecraft:arrow"),
        );
        assert_eq!(translate_older(p, true, &key), NO_COMPONENTS);

        // An inline instrument also gains 26.3's durability damage.
        let instrument = |damage: Option<u32>| {
            let w = Wire::default().var(0).var(1).f32(7.0).f32(256.0);
            match damage {
                Some(damage) => w.var(damage),
                None => w,
            }
            .text()
        };
        assert_converts_both_ways(
            p,
            "instrument",
            Wire(Wire::default().bool(true).bytes(&instrument(None).0).0),
            instrument(Some(0)),
        );
    }

    const PROTOCOL_1_21_8: i32 = 772;

    /// A compound's entries: `id` as the string `kind` when given, then a
    /// byte `Age` of 3.
    fn entity_compound(kind: Option<&str>) -> Vec<u8> {
        let mut nbt = vec![10];
        if let Some(kind) = kind {
            nbt.extend([8, 0, 2, b'i', b'd', 0, kind.len() as u8]);
            nbt.extend_from_slice(kind.as_bytes());
        }
        nbt.extend([1, 0, 3, b'A', b'g', b'e', 3, 0]);
        nbt
    }

    /// Through 1.21.8 an entity's type is the `id` inside its compound;
    /// 1.21.9 sends it as a registry id ahead of the compound.
    #[test]
    fn id_compounds_become_typed_entity_data_and_back() {
        let p = PROTOCOL_1_21_8;
        let typed = |w: Wire| {
            w.var(id_26_3(ClientRegistry::EntityType, "bee"))
                .bytes(&entity_compound(None))
        };
        let untyped = |w: Wire| w.bytes(&entity_compound(Some("minecraft:bee")));
        assert_converts_both_ways(
            p,
            "entity_data",
            untyped(Wire::default()),
            typed(Wire::default()),
        );
        // One bee, 5 ticks in the hive of at least 20.
        assert_converts_both_ways(
            p,
            "bees",
            untyped(Wire::default().var(1)).var(5).var(20),
            typed(Wire::default().var(1)).var(5).var(20),
        );

        let untyped_at_all = one_older_component(
            p,
            "entity_data",
            Wire::default().bytes(&entity_compound(None)),
        );
        assert_eq!(translate_older(p, true, &untyped_at_all), NO_COMPONENTS);
    }

    /// 1.21.8's profile is the later partial profile without a skin patch;
    /// a later full profile gives it its name and id.
    #[test]
    fn profiles_gain_and_lose_their_skin_patch() {
        let p = PROTOCOL_1_21_8;
        let named = || Wire::default().bool(true).str("Steve").bool(false).var(0);
        assert_converts_both_ways(
            p,
            "profile",
            named(),
            Wire(Wire::default().bool(false).bytes(&named().0).0)
                .bool(false)
                .bool(false)
                .bool(false)
                .bool(false),
        );

        let full = Wire::default()
            .bool(true)
            .bytes(&[7; 16])
            .str("Alex")
            .var(0)
            // A skin patch with a body texture.
            .bool(true)
            .str("minecraft:skin")
            .bool(false)
            .bool(false)
            .bool(false);
        let old = Wire::default()
            .bool(true)
            .str("Alex")
            .bool(true)
            .bytes(&[7; 16])
            .var(0);
        assert_eq!(
            translate_older(p, false, &one_component_26_3("profile", full)),
            one_older_component(p, "profile", old)
        );
    }

    /// 1.21.6 extended two structs; 1.21.5's shorter ones take the later
    /// fields' defaults on the way up and lose them on the way down.
    #[test]
    fn absent_fields_take_their_defaults_and_back() {
        let p = 770;
        let entry = |display: bool| {
            let w = Wire::default()
                .var(1) // one entry
                .var(2)
                .str("minecraft:a")
                .f64(1.5)
                .var(0)
                .var(3);
            if display { w.var(0) } else { w }
        };
        assert_converts_both_ways(p, "attribute_modifiers", entry(false), entry(true));

        let equippable = |sheared: bool| {
            // Slot 4, a referenced sound, and nothing optional.
            let w = Wire::default()
                .var(4)
                .var(1)
                .bool(false)
                .bool(false)
                .bool(false)
                .bool(true)
                .bool(true)
                .bool(true)
                .bool(false);
            if sheared {
                w.bool(false)
                    .var(0)
                    .str("minecraft:item.shears.snip")
                    .bool(false)
            } else {
                w
            }
        };
        assert_converts_both_ways(p, "equippable", equippable(false), equippable(true));
    }

    /// 1.21.4 sends `showInTooltip` with several components, and an
    /// ingredient and template item inside a trim; 1.21.5 has none of them.
    #[test]
    fn fields_only_1_21_4_sends_are_dropped_and_restored() {
        let p = 769;
        assert_converts_both_ways(
            p,
            "dyed_color",
            Wire::default().i32(0x123456).bool(true),
            Wire::default().i32(0x123456),
        );
        assert_converts_both_ways(
            p,
            "unbreakable",
            Wire::default().bool(true),
            Wire::default(),
        );
        assert_converts_both_ways(
            p,
            "enchantments",
            Wire::default().var(1).var(4).var(2).bool(true),
            Wire::default().var(1).var(4).var(2),
        );
        // One predicate with nothing set, then `showInTooltip`.
        assert_converts_both_ways(
            p,
            "can_break",
            Wire::default()
                .var(1)
                .bool(false)
                .bool(false)
                .bool(false)
                .bool(true),
            Wire::default()
                .var(1)
                .bool(false)
                .bool(false)
                .bool(false)
                .var(0)
                .var(0),
        );

        // An inline material and pattern: the ingredient (item 9) and the
        // template item (item 7) come back as air.
        let trim = |ingredient: u32, template: u32| {
            Wire::default()
                .var(0)
                .str("gold")
                .var(ingredient)
                .var(1)
                .str("minecraft:gold")
                .str("gold_darker")
                .text()
                .var(0)
                .str("minecraft:coast")
                .var(template)
                .text()
                .bool(false)
                .bool(true)
        };
        let later = one_component_26_3(
            "trim",
            Wire::default()
                .var(0)
                .str("minecraft:trim/gold")
                .text()
                .var(0)
                .str("minecraft:coast")
                .text()
                .bool(false),
        );
        let old = one_older_component(p, "trim", trim(9, 7));
        assert_eq!(translate_older(p, true, &old), later);
        assert_eq!(
            translate_older(p, false, &later),
            one_older_component(p, "trim", trim(0, 0))
        );
    }

    /// 1.21.3's one-int model data is the later first float.
    #[test]
    fn custom_model_data_int_becomes_the_first_float_and_back() {
        assert_converts_both_ways(
            768,
            "custom_model_data",
            Wire::default().var(7),
            Wire::default().var(1).f32(7.0).var(0).var(0).var(0),
        );
    }

    /// 1.21.1's food still carried eating, its potion had no custom name,
    /// and its instrument counted its use in ticks.
    #[test]
    fn components_1_21_2_reshaped_convert() {
        let p = 767;
        let food = || Wire::default().var(3).f32(0.6).bool(false);
        assert_converts_both_ways(
            p,
            "food",
            // 1.6 seconds, nothing it converts to, no effects.
            food().f32(1.6).bool(false).var(0),
            food(),
        );
        let potion = || Wire::default().bool(false).bool(false).var(0);
        assert_converts_both_ways(p, "potion_contents", potion(), potion().bool(false));

        // An inline instrument: a referenced sound, 140 ticks, range 256.
        assert_converts_both_ways(
            p,
            "instrument",
            Wire::default().var(0).var(1).var(140).f32(256.0),
            Wire::default()
                .var(0)
                .var(1)
                .f32(7.0)
                .f32(256.0)
                .var(0)
                .bytes(&[8, 0, 0]),
        );
    }

    /// A 1.20.6 attribute modifier's UUID becomes its id.
    #[test]
    fn attribute_modifier_uuids_become_ids() {
        let p = 766;
        let uuid: Vec<u8> = (1..=16).collect();
        let old = Wire::default()
            .var(1)
            .var(2)
            .bytes(&uuid)
            .str("Tool modifier")
            .f64(1.5)
            .var(0)
            .var(3)
            .bool(true);
        let later = Wire::default()
            .var(1)
            .var(2)
            .str("minecraft:01020304-0506-0708-090a-0b0c0d0e0f10")
            .f64(1.5)
            .var(0)
            .var(3)
            .var(0);
        assert_eq!(
            translate_older(p, true, &one_older_component(p, "attribute_modifiers", old)),
            one_component_26_3("attribute_modifiers", later)
        );
    }

    /// A component two versions lay out alike is carried as it is: both
    /// grammars must reach the very same shape for it, whatever layers they
    /// are built from.
    #[test]
    fn unchanged_components_share_their_shape_across_grammars() {
        let shape = |codec: &ItemCodec, name: &str| {
            let id = codec
                .registries()
                .id_of(ClientRegistry::DataComponentType, name)
                .unwrap();
            codec.component_shape(id).unwrap() as *const Shape
        };
        for name in ["use_remainder", "bundle_contents", "can_break", "food"] {
            assert_eq!(shape(codec(), name), shape(codec_26_3(), name), "{name}");
        }
        // The same through a stack of layers, for one 1.21.11 left alone.
        assert_eq!(
            shape(older(PROTOCOL_1_21_11), "food"),
            shape(codec_26_3(), "food")
        );
        assert_eq!(
            translate_component_26_2("use_remainder", Wire::default().template("bowl", 1)),
            one_component_26_3(
                "use_remainder",
                Wire::default().var(item_26_3("bowl")).var(1).var(0).var(0)
            )
        );
    }

    /// From 1.21.5 to 1.21.10 a partial predicate is typed by a predicate
    /// type's id; 1.21.11 reads an either, whose left arm that is.
    #[test]
    fn partial_predicates_gain_their_either_and_back() {
        // One block predicate with nothing set but one partial predicate of
        // type 2, whose body is an empty compound.
        let predicate = |kind: Wire| {
            Wire::default()
                .var(1)
                .bool(false)
                .bool(false)
                .bool(false)
                .var(0)
                .var(1)
                .bytes(&kind.0)
                .empty_compound()
        };
        for protocol in [770, PROTOCOL_1_21_8, 773] {
            assert_converts_both_ways(
                protocol,
                "can_break",
                predicate(Wire::default().var(2)),
                predicate(Wire::default().bool(true).var(2)),
            );
        }
    }

    /// 1.21.1's lock is a string tag, which has no later form: the component
    /// is left out, where it used to fail the whole patch.
    #[test]
    fn a_string_lock_is_dropped_without_failing_the_stack() {
        let lock = one_older_component(767, "lock", Wire::default().bytes(&[8, 0, 1, b'k']));
        assert_eq!(translate_older(767, true, &lock), NO_COMPONENTS);
    }
}
