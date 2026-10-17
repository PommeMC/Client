//! The item stack and data-component wire grammar, ported from vanilla's
//! `ItemStack` / `ItemStackTemplate` / `DataComponentPatch` stream codecs.
//!
//! Component values are not length-prefixed on the wire, so framing a stack
//! means knowing every component's layout. Each one is described by a
//! [`Shape`] built from the `ByteBufCodecs` combinators; values are kept as
//! their exact bytes and only validated, leaving interpretation to callers.

use std::sync::OnceLock;

use crate::codec::{DecodeResult, MAX_STRING_LENGTH, Reader};
use crate::entity_data::{SERIALIZERS_26_2, Serializer};
use crate::registries::{ClientRegistry, RegistryTable};
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
    /// `TypedDataComponent.STREAM_CODEC`: a component id and its value.
    TypedComponent,
}

use Shape::*;

const IDENTIFIER: Shape = Utf(MAX_STRING_LENGTH);
const STRING: Shape = Utf(MAX_STRING_LENGTH);
/// `ComponentSerialization.STREAM_CODEC`: text components travel as NBT.
const TEXT: Shape = Nbt;

/// `SoundEvent.STREAM_CODEC`: a holder over (location, fixed range).
pub static SOUND: Shape = Holder(&Struct(&[IDENTIFIER, Optional(&Float)]));

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

/// `ConsumeEffect.STREAM_CODEC` over `consume_effect_type`.
static CONSUME_EFFECT: Shape = ByRegistry(
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
        // TeleportRandomlyConsumeEffect.java:37
        ("teleport_randomly", Float),
        // PlaySoundConsumeEffect.java:28
        ("play_sound", Ref(&SOUND)),
    ],
);

/// `StatePropertiesPredicate.PropertyMatcher.STREAM_CODEC`: a name, then an
/// exact value or a range of optional bounds.
static PROPERTY_MATCHER: Shape = Struct(&[
    STRING,
    Either(&STRING, &Struct(&[Optional(&STRING), Optional(&STRING)])),
]);

/// `DataComponentMatchers.STREAM_CODEC`: exact components, then up to 64
/// partial predicates, each a predicate or component type and an NBT body
/// (`DataComponentPredicate.TypeBase` encodes through its Codec).
static COMPONENT_MATCHERS: Shape = Struct(&[
    List(UNBOUNDED, &TypedComponent),
    List(64, &Struct(&[Either(&VarInt, &VarInt), Nbt])),
]);

/// `advancements.predicates.BlockPredicate.STREAM_CODEC`.
static BLOCK_PREDICATE: Shape = Struct(&[
    Optional(&HolderSet),
    Optional(&List(UNBOUNDED, &PROPERTY_MATCHER)),
    Optional(&Compound),
    Ref(&COMPONENT_MATCHERS),
]);

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

/// `TypedEntityData.streamCodec`: a registry id, then the compound.
static TYPED_ENTITY_DATA: Shape = Struct(&[VarInt, Compound]);

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
    // Consumable.java:49
    (
        "consumable",
        Struct(&[
            Float,
            VarInt,
            Ref(&SOUND),
            Bool,
            List(UNBOUNDED, &CONSUME_EFFECT),
        ]),
    ),
    // UseRemainder.java:17
    ("use_remainder", ItemTemplate),
    ("sulfur_cube_content", ItemTemplate),
    // UseCooldown.java:28
    ("use_cooldown", Struct(&[Float, Optional(&IDENTIFIER)])),
    ("damage_resistant", HolderSet),
    ("repairable", HolderSet),
    ("provides_banner_patterns", HolderSet),
    // Tool.java:30,50
    (
        "tool",
        Struct(&[
            List(
                UNBOUNDED,
                &Struct(&[HolderSet, Optional(&Float), Optional(&Bool)]),
            ),
            Float,
            VarInt,
            Bool,
        ]),
    ),
    // Weapon.java:24
    ("weapon", Struct(&[VarInt, Float])),
    // AttackRange.java:42
    (
        "attack_range",
        Struct(&[Float, Float, Float, Float, Float, Float]),
    ),
    // Equippable.java:48
    (
        "equippable",
        Struct(&[
            VarInt,
            Ref(&SOUND),
            Optional(&IDENTIFIER),
            Optional(&IDENTIFIER),
            Optional(&HolderSet),
            Bool,
            Bool,
            Bool,
            Bool,
            Bool,
            Ref(&SOUND),
        ]),
    ),
    // DeathProtection.java:30
    ("death_protection", List(UNBOUNDED, &CONSUME_EFFECT)),
    // BlocksAttacks.java:42,96,109
    (
        "blocks_attacks",
        Struct(&[
            Float,
            Float,
            List(
                UNBOUNDED,
                &Struct(&[Float, Optional(&HolderSet), Float, Float]),
            ),
            Struct(&[Float, Float, Float]),
            Optional(&HolderSet),
            Optional(&SOUND),
            Optional(&SOUND),
        ]),
    ),
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
    ("swing_animation", Struct(&[VarInt, VarInt])),
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
    (
        "potion_contents",
        Struct(&[
            Optional(&VarInt),
            Optional(&Int),
            List(UNBOUNDED, &MOB_EFFECT_INSTANCE),
            Optional(&STRING),
        ]),
    ),
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
    // ArmorTrim.java:38; TrimPattern.java:30
    (
        "trim",
        Struct(&[
            Ref(&TRIM_MATERIAL),
            Holder(&Struct(&[IDENTIFIER, TEXT, Bool])),
        ]),
    ),
    ("provides_trim_material", Ref(&TRIM_MATERIAL)),
    ("entity_data", Ref(&TYPED_ENTITY_DATA)),
    ("block_entity_data", Ref(&TYPED_ENTITY_DATA)),
    // CustomData.java:26
    ("bucket_entity_data", Compound),
    // InstrumentComponent.java:27; Instrument.java:29
    (
        "instrument",
        Holder(&Struct(&[Ref(&SOUND), Float, Float, TEXT])),
    ),
    // JukeboxPlayable.java:40; JukeboxSong.java:34
    (
        "jukebox_playable",
        Holder(&Struct(&[Ref(&SOUND), TEXT, Float, VarInt])),
    ),
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
        List(
            UNBOUNDED,
            &Struct(&[Ref(&TYPED_ENTITY_DATA), VarInt, VarInt]),
        ),
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
    air: u32,
}

impl ItemCodec {
    /// The grammar for `protocol`, when its component layouts are ported.
    pub fn for_protocol(protocol: i32) -> Option<&'static ItemCodec> {
        static CODECS: [OnceLock<Option<ItemCodec>>; EMBEDDED.len()] =
            [const { OnceLock::new() }; EMBEDDED.len()];
        let index = EMBEDDED
            .iter()
            .position(|e| e.version.protocol == protocol)?;
        CODECS[index]
            .get_or_init(|| {
                let (shapes, entity_data) = match EMBEDDED[index].version.name {
                    "26.2" => (COMPONENTS_26_2, SERIALIZERS_26_2),
                    _ => return None,
                };
                Self::new(RegistryTable::for_protocol(protocol)?, shapes, entity_data)
            })
            .as_ref()
    }

    fn new(
        registries: &'static RegistryTable,
        shapes: &'static [(&str, Shape)],
        entity_data: &'static [Serializer],
    ) -> Option<Self> {
        let components = registries
            .names(ClientRegistry::DataComponentType)
            .iter()
            .map(|name| shapes.iter().find(|(n, _)| n == name).map(|(_, s)| s))
            .collect::<Option<_>>()?;
        let air = registries.id_of(ClientRegistry::Item, "air")?;
        Some(Self {
            registries,
            components,
            entity_data,
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

    pub(crate) fn entity_data_serializers(&self) -> &'static [Serializer] {
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
            Unit => {}
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
            Compound => {
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
            Holder(inline) => {
                if r.varint()? == 0 {
                    self.walk(r, inline, depth)?;
                }
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
}
