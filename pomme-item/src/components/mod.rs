//! Component values pomme reads itself, each ported from its vanilla
//! `STREAM_CODEC`.

mod basic;
mod codec;
mod combat;
mod consume;
mod equipment;
mod forms;
mod misc;

pub use basic::*;
pub use codec::{CustomSound, Nbt, Sound, Text};
pub use combat::*;
pub use consume::*;
pub use equipment::*;
pub use misc::*;
use pomme_protocol::codec::{DecodeResult, Reader};
use pomme_protocol::item::{self as wire, ItemCodec};
use pomme_protocol::wire::{write_utf, write_varint};
use pomme_registry::builtin::DataComponentKind;

use crate::ItemStack;
use crate::hash::{Names, Record, Value};

/// A data component's value, read from and written to its wire bytes.
pub trait Component: Sized {
    const KIND: DataComponentKind;

    fn read(r: &mut Reader) -> DecodeResult<Self>;
    fn write(&self, buf: &mut Vec<u8>);

    /// The value's `CODEC` form, whose hash a click sends in the value's
    /// place; `None` for a value holding something whose form pomme doesn't
    /// write yet, or a registry entry `names` doesn't have.
    fn value(&self, names: &dyn Names) -> Option<Value>;

    /// The value `bytes` hold; `None` when they don't read as exactly one.
    fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let mut r = Reader::new(bytes);
        let value = Self::read(&mut r).ok()?;
        r.ensure_eof().ok()?;
        Some(value)
    }

    /// The value as a patch holds it.
    fn to_bytes(&self) -> Box<[u8]> {
        let mut buf = Vec::new();
        self.write(&mut buf);
        buf.into()
    }
}

/// `ItemStackTemplate.STREAM_CODEC`: a stack that is never empty.
fn read_template(r: &mut Reader) -> DecodeResult<ItemStack> {
    let template = ItemCodec::native().read_template(r)?;
    Ok(ItemStack::from_wire(&template))
}

/// Writes each stack as a template after their count; an empty stack has no
/// template and is left out.
fn write_templates(buf: &mut Vec<u8>, items: &[ItemStack]) {
    let templates: Vec<_> = items.iter().filter_map(ItemStack::to_wire).collect();
    write_varint(buf, templates.len() as u32);
    for template in &templates {
        wire::write_template(buf, template);
    }
}

/// The `CODEC` form of the `kind` value `bytes` hold, for the kinds pomme
/// reads.
pub fn value_of(kind: DataComponentKind, bytes: &[u8], names: &dyn Names) -> Option<Value> {
    macro_rules! of {
        ($($component:ty,)*) => {
            $(if kind == <$component>::KIND {
                return <$component>::from_bytes(bytes)?.value(names);
            })*
        };
    }
    of!(
        MaxStackSize,
        MaxDamage,
        Damage,
        Unbreakable,
        DyedColor,
        MapColor,
        Rarity,
        Dye,
        CustomName,
        ItemName,
        Lore,
        CustomData,
        TooltipDisplay,
        CustomModelData,
        Equippable,
        Trim,
        ProvidesTrimMaterial,
        AttributeModifiers,
        Enchantments,
        StoredEnchantments,
        Food,
        Consumable,
        UseEffects,
        UseCooldown,
        PotionContents,
        Tool,
        MinimumAttackCharge,
        KineticWeapon,
        BlocksAttacks,
        FireworkExplosion,
        Bees,
        Profile,
        BundleContents,
        ChargedProjectiles,
        BlockItemStateProperties,
    );
    forms::value_of(kind, bytes, names)
}

/// `ItemStackTemplate.CODEC`: the item, with its count and patch when they
/// aren't one and empty.
fn template_value(stack: &ItemStack, names: &dyn Names) -> Option<Value> {
    let stack = stack.as_present()?;
    // `DataComponentPatch.CODEC`: each value under its type's name, and a
    // removed type under `!` and its name as an empty map.
    let mut patch = Vec::new();
    for (kind, value) in stack.component_patch.iter() {
        let name = pomme_registry::Identifier::from(kind);
        let entry = match value {
            Some(_) if TRANSIENT.contains(&kind) => continue,
            Some(bytes) => (Value::identifier(&name), value_of(kind, bytes, names)?),
            None => (Value::String(format!("!{name}")), Record::new().build()),
        };
        patch.push(entry);
    }
    Some(
        Record::new()
            .field("id", codec::entry_value(&stack.kind))
            .int("count", stack.count, 1)
            .unless("components", stack.component_patch.is_empty(), || {
                Value::Map(patch)
            })
            .build(),
    )
}

/// Each stack's template, as a list.
fn templates_value(items: &[ItemStack], names: &dyn Names) -> Option<Value> {
    Value::try_list(items.iter().map(|stack| template_value(stack, names)))
}

/// The components with no `CODEC`, which a patch's own leaves out.
const TRANSIENT: [DataComponentKind; 3] = [
    DataComponentKind::CreativeSlotLock,
    DataComponentKind::AdditionalTradeCost,
    DataComponentKind::MapPostProcessing,
];

/// Vanilla `BundleContents`: the stacks in a bundle, newest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BundleContents {
    pub items: Vec<ItemStack>,
}

impl Component for BundleContents {
    const KIND: DataComponentKind = DataComponentKind::BundleContents;

    fn read(r: &mut Reader) -> DecodeResult<Self> {
        let items = r.list(usize::MAX, read_template)?;
        Ok(Self { items })
    }

    fn write(&self, buf: &mut Vec<u8>) {
        write_templates(buf, &self.items);
    }

    fn value(&self, names: &dyn Names) -> Option<Value> {
        templates_value(&self.items, names)
    }
}

/// Vanilla `ChargedProjectiles`: what a crossbow is loaded with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChargedProjectiles {
    pub items: Vec<ItemStack>,
}

impl ChargedProjectiles {
    /// `ByteBufCodecs.list(1024)`.
    const MAX_ITEMS: usize = 1024;
}

impl Component for ChargedProjectiles {
    const KIND: DataComponentKind = DataComponentKind::ChargedProjectiles;

    fn read(r: &mut Reader) -> DecodeResult<Self> {
        let items = r.list(Self::MAX_ITEMS, read_template)?;
        Ok(Self { items })
    }

    fn write(&self, buf: &mut Vec<u8>) {
        write_templates(buf, &self.items);
    }

    fn value(&self, names: &dyn Names) -> Option<Value> {
        templates_value(&self.items, names)
    }
}

/// Vanilla `BlockItemStateProperties`: block state properties the placed
/// block takes from its item, each a name and a value.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockItemStateProperties {
    pub properties: Vec<(String, String)>,
}

impl Component for BlockItemStateProperties {
    const KIND: DataComponentKind = DataComponentKind::BlockState;

    fn read(r: &mut Reader) -> DecodeResult<Self> {
        let properties = r.list(usize::MAX, |r| Ok((codec::string(r)?, codec::string(r)?)))?;
        Ok(Self { properties })
    }

    fn write(&self, buf: &mut Vec<u8>) {
        write_varint(buf, self.properties.len() as u32);
        for (name, value) in &self.properties {
            write_utf(buf, name);
            write_utf(buf, value);
        }
    }

    fn value(&self, _: &dyn Names) -> Option<Value> {
        let properties = self
            .properties
            .iter()
            .map(|(name, value)| (Value::string(name), Value::string(value)));
        Some(Value::Map(properties.collect()))
    }
}

#[cfg(test)]
mod tests {
    use pomme_registry::Registry;
    use pomme_registry::builtin::ItemKind;

    use super::*;

    fn read<T: Component>(bytes: &[u8]) -> Option<T> {
        T::from_bytes(bytes)
    }

    /// How many items have a default `T`, each read from the game's own
    /// bytes and written back to exactly them.
    fn defaults_round_trip<T: Component>() -> usize {
        let mut seen = 0;
        for &item in ItemKind::ALL {
            let Some(bytes) = crate::defaults::get(item, T::KIND) else {
                continue;
            };
            let value = T::from_bytes(bytes)
                .unwrap_or_else(|| panic!("{item}'s {:?} doesn't read", T::KIND));
            assert_eq!(*value.to_bytes(), *bytes, "{item}'s {:?}", T::KIND);
            seen += 1;
        }
        seen
    }

    #[test]
    fn every_items_default_reads_and_writes_back_as_the_game_wrote_it() {
        macro_rules! check {
            ($($component:ty,)*) => {
                $(assert!(
                    defaults_round_trip::<$component>() > 0,
                    "no item has a default {}",
                    stringify!($component)
                );)*
            };
        }
        check!(
            MaxStackSize,
            MaxDamage,
            Damage,
            Rarity,
            Dye,
            ItemName,
            Lore,
            TooltipDisplay,
            Equippable,
            ProvidesTrimMaterial,
            AttributeModifiers,
            Enchantments,
            StoredEnchantments,
            Food,
            Consumable,
            UseEffects,
            UseCooldown,
            PotionContents,
            Tool,
            MinimumAttackCharge,
            KineticWeapon,
            BlocksAttacks,
            Bees,
            BundleContents,
            ChargedProjectiles,
        );
    }

    /// The game's own data pack, whose registries the default components
    /// number in identifier order.
    struct DataPack;

    impl Names for DataPack {
        fn name(&self, registry: &str, id: u32) -> Option<pomme_registry::Identifier> {
            use pomme_registry::data::*;
            use pomme_registry::{DataRegistry, DataRegistryKey};
            macro_rules! nth {
                ($($registry:ty,)*) => {
                    $(if registry == <$registry>::NAME {
                        let all = <<$registry as DataRegistry>::Key>::ALL;
                        return Some(all.get(id as usize)?.clone().into_ident());
                    })*
                };
            }
            nth!(
                BannerPatternKind,
                CatSoundVariant,
                CatVariant,
                ChickenSoundVariant,
                ChickenVariant,
                CowSoundVariant,
                CowVariant,
                DamageKind,
                Enchantment,
                FrogVariant,
                Instrument,
                JukeboxSong,
                PaintingVariant,
                PigSoundVariant,
                PigVariant,
                TrimMaterial,
                TrimPattern,
                WolfSoundVariant,
                WolfVariant,
                ZombieNautilusVariant,
            );
            None
        }
    }

    #[test]
    fn every_items_defaults_hash_as_the_game_hashes_them() {
        let mut kinds = std::collections::BTreeSet::new();
        for (item, kind, expected) in crate::defaults::hashes() {
            let bytes = crate::defaults::get(item, kind).unwrap();
            let value = value_of(kind, bytes, &DataPack)
                .unwrap_or_else(|| panic!("{item}'s {kind} has no form"));
            assert_eq!(value.hash().as_i32(), expected, "{item}'s {kind}");
            kinds.insert(kind);
        }
        assert_eq!(kinds.len(), 59);
    }

    /// The codec oracle's registries, of which the fixtures use an entry each.
    struct Oracle;

    impl Names for Oracle {
        fn name(&self, registry: &str, id: u32) -> Option<pomme_registry::Identifier> {
            let entry = match registry {
                "enchantment" => "minecraft:sharpness",
                "banner_pattern" => "minecraft:creeper",
                _ => return None,
            };
            let known = id == pomme_protocol::vanilla_fixtures::registry_id(registry, entry);
            known.then(|| entry.into())
        }
    }

    #[test]
    fn fixture_values_hash_as_the_game_hashes_them() {
        use pomme_protocol::vanilla_fixtures::case;

        // Each fixture with the component it is a value of.
        let fixtures = [
            ("attribute_modifiers", "attribute_modifiers"),
            ("bees", "bees"),
            ("custom_data", "custom_data"),
            ("custom_model_data", "custom_model_data"),
            ("custom_name", "custom_name"),
            ("dyed_color", "dyed_color"),
            ("enchantments", "enchantments"),
            ("equippable", "equippable"),
            ("firework_explosion", "firework_explosion"),
            ("lore", "lore"),
            ("map_color", "map_color"),
            ("potion_contents", "potion_contents"),
            ("profile", "profile"),
            ("tooltip_display", "tooltip_display"),
            ("trim", "trim"),
            ("unbreakable", "unbreakable"),
            ("use_cooldown", "use_cooldown"),
            ("item_model", "item_model"),
            ("tooltip_style", "tooltip_style"),
            ("note_block_sound", "note_block_sound"),
            ("repair_cost", "repair_cost"),
            ("map_id", "map_id"),
            ("ominous_bottle_amplifier", "ominous_bottle_amplifier"),
            ("enchantable", "enchantable"),
            ("enchantment_glint_override", "enchantment_glint_override"),
            ("potion_duration_scale", "potion_duration_scale"),
            ("base_color", "base_color"),
            ("break_sound", "break_sound"),
            ("weapon", "weapon"),
            ("attack_range", "attack_range"),
            ("piercing_weapon", "piercing_weapon"),
            ("swing_animation", "swing_animation"),
            ("repairable", "repairable"),
            ("use_remainder", "use_remainder"),
            ("death_protection", "death_protection"),
            ("suspicious_stew_effects", "suspicious_stew_effects"),
            ("writable_book_content", "writable_book_content"),
            ("written_book_content", "written_book_content"),
            ("fireworks", "fireworks"),
            ("container", "container"),
            ("lodestone_tracker", "lodestone_tracker"),
            ("entity_data", "entity_data"),
            ("bucket_entity_data", "bucket_entity_data"),
            ("container_loot", "container_loot"),
            ("item_name", "item_name"),
            ("bundle_of_components", "bundle_contents"),
            ("fox_variant", "fox/variant"),
            ("rabbit_variant", "rabbit/variant"),
            ("tropical_fish_pattern", "tropical_fish/pattern"),
            ("horse_variant", "horse/variant"),
            ("axolotl_variant", "axolotl/variant"),
            ("salmon_size", "salmon/size"),
            ("parrot_variant", "parrot/variant"),
            ("llama_variant", "llama/variant"),
            ("mooshroom_variant", "mooshroom/variant"),
            ("block_entity_data", "block_entity_data"),
            ("villager_variant", "villager/variant"),
            ("map_decorations", "map_decorations"),
            ("kinetic_negative_zero", "kinetic_weapon"),
            ("sprite_name", "custom_name"),
            ("banner_patterns", "banner_patterns"),
            ("sulfur_cube_content", "sulfur_cube_content"),
        ];
        for (fixture, component) in fixtures {
            let kind: DataComponentKind = component.parse().unwrap();
            let bytes = case(&format!("component/{fixture}"));
            let expected = case(&format!("component_hash/{fixture}"));
            let value =
                value_of(kind, &bytes, &Oracle).unwrap_or_else(|| panic!("{fixture} has no form"));
            assert_eq!(
                value.hash().0.to_be_bytes(),
                expected[..],
                "{fixture}: {value:?}"
            );
        }
    }

    /// `component/<name>` as vanilla's own stream codec wrote it, read and
    /// written back to the same bytes.
    fn fixture<T: Component>(name: &str) -> T {
        let bytes = pomme_protocol::vanilla_fixtures::case(&format!("component/{name}"));
        let value = T::from_bytes(&bytes).unwrap_or_else(|| panic!("{name} doesn't read"));
        assert_eq!(*value.to_bytes(), bytes[..], "{name}");
        value
    }

    #[test]
    fn matches_vanilla_encoder() {
        use pomme_registry::builtin::{MobEffect, Potion, SoundEvent};
        use pomme_registry::{Holder, Identifier};

        assert_eq!(fixture::<DyedColor>("dyed_color").rgb, 0x12_34_56);
        assert_eq!(fixture::<MapColor>("map_color").color, 0x46_40_2e);
        fixture::<Unbreakable>("unbreakable");

        let name = fixture::<CustomName>("custom_name")
            .name
            .component()
            .unwrap();
        assert_eq!(name.content, pomme_text::Content::Text("Excalibur".into()));
        assert_eq!(name.style.color, Some(0xffaa00));

        assert_eq!(
            fixture::<CustomModelData>("custom_model_data"),
            CustomModelData {
                floats: vec![1.5],
                flags: vec![true, false],
                strings: vec!["a".into()],
                colors: vec![0xff_00_00],
            }
        );
        assert_eq!(
            fixture::<FireworkExplosion>("firework_explosion"),
            FireworkExplosion {
                shape: FireworkExplosionShape::Star,
                colors: vec![1, 2],
                fade_colors: vec![3],
                has_trail: true,
                has_twinkle: false,
            }
        );
        assert_eq!(
            fixture::<Profile>("profile").contents,
            ProfileContents::Partial {
                name: Some("Steve".into()),
                id: None,
                properties: Vec::new(),
            }
        );

        let data = fixture::<CustomData>("custom_data").nbt.compound().unwrap();
        assert_eq!(data.int("level"), Some(3));

        let sharpness =
            pomme_protocol::vanilla_fixtures::registry_id("enchantment", "minecraft:sharpness");
        let enchantments = fixture::<Enchantments>("enchantments");
        assert_eq!(enchantments.levels.len(), 1);
        assert_eq!(
            enchantments.level(Registry::from_u32(sharpness).unwrap()),
            5
        );

        let potion = fixture::<PotionContents>("potion_contents");
        assert_eq!(potion.potion, Some(Potion::StrongSwiftness));
        assert_eq!(potion.custom_color, Some(0x12_34_56));
        assert_eq!(potion.custom_name.as_deref(), Some("custom"));
        let [effect] = potion.custom_effects.as_slice() else {
            panic!("one effect, got {:?}", potion.custom_effects);
        };
        assert_eq!(effect.id, MobEffect::Speed);
        assert_eq!(
            (effect.details.amplifier, effect.details.duration),
            (1, 200)
        );

        assert_eq!(
            fixture::<TooltipDisplay>("tooltip_display"),
            TooltipDisplay {
                hide_tooltip: true,
                hidden_components: vec![DataComponentKind::Damage, DataComponentKind::Lore],
            }
        );
        assert_eq!(
            fixture::<UseCooldown>("use_cooldown"),
            UseCooldown {
                seconds: 1.5,
                cooldown_group: Some(Identifier::new("minecraft:pearl")),
            }
        );

        let equippable = fixture::<Equippable>("equippable");
        assert_eq!(equippable.slot, EquipmentSlot::Head);
        assert_eq!(
            equippable.equip_sound,
            Holder::Direct(CustomSound {
                location: Identifier::new("minecraft:custom"),
                fixed_range: Some(16.0),
            })
        );
        assert_eq!(
            equippable.shearing_sound,
            Holder::Reference(SoundEvent::ItemShearsSnip)
        );
    }

    fn plain(text: &Text) -> String {
        match text.component().unwrap().content {
            pomme_text::Content::Text(text) => text,
            other => panic!("plain text, got {other:?}"),
        }
    }

    #[test]
    fn texts_and_entity_data_match_vanilla_encoder() {
        use pomme_registry::Holder;
        use pomme_registry::builtin::{Attribute, EntityKind};

        let lore = fixture::<Lore>("lore");
        assert_eq!(
            lore.lines.iter().map(plain).collect::<Vec<_>>(),
            ["First", "Second"]
        );

        let trim = fixture::<Trim>("trim");
        let (Holder::Direct(material), Holder::Direct(pattern)) = (&trim.material, &trim.pattern)
        else {
            panic!("an inline material and pattern, got {trim:?}");
        };
        assert_eq!(material.assets.base_suffix, "test");
        assert!(material.assets.overrides.is_empty());
        assert_eq!(plain(&material.description), "Test Material");
        assert_eq!(pattern.asset_id.to_string(), "minecraft:test");
        assert_eq!(plain(&pattern.description), "Test Trim");
        assert!(pattern.decal);

        let bees = fixture::<Bees>("bees");
        let [bee] = bees.occupants.as_slice() else {
            panic!("one bee, got {:?}", bees.occupants);
        };
        assert_eq!(bee.entity, EntityKind::Bee);
        assert_eq!(bee.entity_data.compound().unwrap().int("Age"), Some(7));
        assert_eq!((bee.ticks_in_hive, bee.min_ticks_in_hive), (5, 10));

        let modifiers = fixture::<AttributeModifiers>("attribute_modifiers");
        let [entry] = modifiers.modifiers.as_slice() else {
            panic!("one modifier, got {:?}", modifiers.modifiers);
        };
        assert_eq!(entry.kind, Attribute::Armor);
        assert_eq!(entry.modifier.id.to_string(), "minecraft:a");
        assert_eq!(entry.modifier.amount, 2.5);
        assert_eq!(
            entry.modifier.operation,
            AttributeModifierOperation::AddMultipliedBase
        );
        assert_eq!(entry.slot, EquipmentSlotGroup::Chest);
        let AttributeModifierDisplay::Override { text } = &entry.display else {
            panic!("an override, got {:?}", entry.display);
        };
        assert_eq!(plain(text), "Sturdy");
    }

    /// A value no item has by default: written, then read back the same.
    fn round_trips<T: Component + PartialEq + std::fmt::Debug>(value: &T) {
        assert_eq!(T::from_bytes(&value.to_bytes()).as_ref(), Some(value));
    }

    #[test]
    fn values_no_item_defaults_to_round_trip() {
        use pomme_registry::builtin::{EntityKind, SoundEvent};
        use pomme_registry::{Holder, HolderSet, Identifier};

        round_trips(&Unbreakable);
        round_trips(&DyedColor { rgb: 0x12_34_56 });
        round_trips(&MapColor { color: -1 });
        round_trips(&CustomName {
            name: Text::literal("Excalibur"),
        });
        round_trips(&CustomModelData {
            floats: vec![1.5],
            flags: vec![true, false],
            strings: vec!["a".into()],
            colors: vec![0xff_00_00],
        });
        round_trips(&FireworkExplosion {
            shape: FireworkExplosionShape::Star,
            colors: vec![1, 2],
            fade_colors: vec![3],
            has_trail: true,
            has_twinkle: false,
        });
        round_trips(&Trim {
            material: Holder::Reference(Registry::from_u32(2).unwrap()),
            pattern: Holder::Direct(DirectTrimPattern {
                asset_id: Identifier::new("minecraft:coast"),
                description: Text::literal("Coast"),
                decal: true,
            }),
        });
        round_trips(&Equippable {
            slot: EquipmentSlot::Head,
            equip_sound: Holder::Direct(CustomSound {
                location: Identifier::new("minecraft:custom"),
                fixed_range: Some(16.0),
            }),
            asset_id: Some(Identifier::new("minecraft:diamond")),
            camera_overlay: None,
            allowed_entities: Some(HolderSet::Direct {
                contents: vec![EntityKind::Zombie, EntityKind::Skeleton],
            }),
            dispensable: true,
            swappable: false,
            damage_on_hurt: true,
            equip_on_interact: false,
            can_be_sheared: true,
            shearing_sound: Holder::Reference(SoundEvent::ItemShearsSnip),
        });
        round_trips(&Profile {
            contents: ProfileContents::Partial {
                name: Some("Steve".into()),
                id: Some(7),
                properties: vec![ProfileProperty {
                    name: "textures".into(),
                    value: "abc".into(),
                    signature: None,
                }],
            },
            skin_patch: PlayerSkinPatch {
                body: Some(Identifier::new("minecraft:skin")),
                slim: Some(true),
                ..PlayerSkinPatch::default()
            },
        });
    }

    #[test]
    fn an_equipment_slot_goes_by_its_id_and_an_unknown_one_is_the_main_hand() {
        // `EquipmentSlot`'s ids aren't its declaration order.
        assert_eq!(EquipmentSlot::from_id(5), EquipmentSlot::Offhand);
        assert_eq!(EquipmentSlot::from_id(4), EquipmentSlot::Head);
        assert_eq!(EquipmentSlot::from_id(99), EquipmentSlot::Mainhand);
        assert_eq!(EquipmentSlot::from_byte(1), Some(EquipmentSlot::Offhand));
        assert_eq!(EquipmentSlot::from_byte(8), None);
    }

    #[test]
    fn a_text_is_its_nbt_and_plain_text_is_a_string_tag() {
        let name = Text::literal("Hi");
        let mut bytes = Vec::new();
        name.write(&mut bytes);
        assert_eq!(bytes, [8, 0, 2, b'H', b'i']);
        assert!(name.component().is_some());
    }

    /// Two templates: item, count, then an empty patch's two zero counts.
    fn two_stacks() -> Vec<u8> {
        let (stone, arrow) = (ItemKind::Stone.to_u32(), ItemKind::Arrow.to_u32());
        let mut bytes = vec![2];
        for (item, count) in [(stone, 3), (arrow, 1)] {
            write_varint(&mut bytes, item);
            bytes.extend([count, 0, 0]);
        }
        bytes
    }

    #[test]
    fn block_state_properties_are_a_map_of_strings() {
        let bytes = [1, 5, b'l', b'e', b'v', b'e', b'l', 2, b'1', b'5'];
        let state = BlockItemStateProperties {
            properties: vec![("level".into(), "15".into())],
        };
        assert_eq!(read::<BlockItemStateProperties>(&bytes).unwrap(), state);
        assert_eq!(*state.to_bytes(), bytes);
    }

    #[test]
    fn a_stack_size_is_a_varint() {
        assert_eq!(
            read::<MaxStackSize>(&[64]).unwrap(),
            MaxStackSize { count: 64 }
        );
        assert_eq!(*MaxStackSize { count: 16 }.to_bytes(), [16]);
    }

    #[test]
    fn bundle_contents_are_a_counted_list_of_templates() {
        let bytes = two_stacks();
        let contents: BundleContents = read(&bytes).unwrap();
        assert_eq!(
            contents.items,
            [
                ItemStack::new(ItemKind::Stone, 3),
                ItemStack::new(ItemKind::Arrow, 1)
            ]
        );
        assert_eq!(*contents.to_bytes(), bytes[..]);
        assert_eq!(
            read::<BundleContents>(&[0]).unwrap(),
            BundleContents::default()
        );
    }

    #[test]
    fn an_empty_stack_is_left_out_of_what_is_written() {
        let contents = BundleContents {
            items: vec![ItemStack::Empty, ItemStack::new(ItemKind::Stone, 1)],
        };
        let written: BundleContents = read(&contents.to_bytes()).unwrap();
        assert_eq!(written.items, [ItemStack::new(ItemKind::Stone, 1)]);
    }

    #[test]
    fn charged_projectiles_share_the_layout_and_cap_the_count() {
        let bytes = two_stacks();
        let charged: ChargedProjectiles = read(&bytes).unwrap();
        assert_eq!(charged.items.len(), 2);
        assert_eq!(*charged.to_bytes(), bytes[..]);
        // 1025 as a varint.
        assert!(read::<ChargedProjectiles>(&[0x81, 0x08]).is_none());
    }
}
