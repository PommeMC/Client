//! Component values pomme reads itself, each ported from its vanilla
//! `STREAM_CODEC`.

mod basic;
mod codec;
mod combat;
mod consume;
mod equipment;
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

/// A data component's value, read from and written to its wire bytes.
pub trait Component: Sized {
    const KIND: DataComponentKind;

    fn read(r: &mut Reader) -> DecodeResult<Self>;
    fn write(&self, buf: &mut Vec<u8>);

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
