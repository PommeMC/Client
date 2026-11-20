//! What an item does when worn or held: its slot, trim, attribute modifiers
//! and enchantments.

use std::fmt;

use pomme_protocol::codec::{DecodeResult, Reader};
use pomme_protocol::wire::{write_utf, write_varint};
use pomme_registry::builtin::{Attribute, DataComponentKind, EntityKind, SoundEvent};
use pomme_registry::data::{Enchantment, TrimMaterial, TrimPattern};
use pomme_registry::{Holder, HolderSet, Identifier};

use super::Component;
use super::codec::{
    Sound, Text, hash_data_entry, hash_entries, hash_entry, hash_sound, holder, holder_set,
    id_enum, identifier, registry, sound, string, write_bool, write_holder, write_holder_set,
    write_identifier, write_list, write_optional, write_registry, write_sound, write_var_i32,
};
use crate::hash::{Hash, Names, Record};

id_enum!(
    /// Vanilla `EquipmentSlot`, in declaration order with each slot's
    /// network id.
    EquipmentSlot {
        Mainhand = 0, "mainhand";
        Offhand = 5, "offhand";
        Feet = 1, "feet";
        Legs = 2, "legs";
        Chest = 3, "chest";
        Head = 4, "head";
        Body = 6, "body";
        Saddle = 7, "saddle";
    }
);

impl EquipmentSlot {
    /// `EquipmentSlot.VALUES.get`: a slot by its place in the declaration,
    /// which is how the equipment packet numbers them.
    pub fn from_byte(ordinal: u8) -> Option<Self> {
        Self::ALL.get(usize::from(ordinal)).copied()
    }
}

impl fmt::Display for EquipmentSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

id_enum!(
    /// Vanilla `EquipmentSlotGroup`: the slots an attribute modifier applies
    /// in.
    EquipmentSlotGroup {
        Any = 0, "any";
        Mainhand = 1, "mainhand";
        Offhand = 2, "offhand";
        Hand = 3, "hand";
        Feet = 4, "feet";
        Legs = 5, "legs";
        Chest = 6, "chest";
        Head = 7, "head";
        Armor = 8, "armor";
        Body = 9, "body";
        Saddle = 10, "saddle";
    }
);

/// Vanilla `Equippable`: the slot an item is worn in and how it behaves
/// there.
#[derive(Clone, Debug, PartialEq)]
pub struct Equippable {
    pub slot: EquipmentSlot,
    pub equip_sound: Sound,
    pub asset_id: Option<Identifier>,
    pub camera_overlay: Option<Identifier>,
    pub allowed_entities: Option<HolderSet<EntityKind, Identifier>>,
    pub dispensable: bool,
    pub swappable: bool,
    pub damage_on_hurt: bool,
    pub equip_on_interact: bool,
    pub can_be_sheared: bool,
    pub shearing_sound: Sound,
}

impl Component for Equippable {
    const KIND: DataComponentKind = DataComponentKind::Equippable;

    fn read(r: &mut Reader) -> DecodeResult<Self> {
        Ok(Self {
            slot: EquipmentSlot::read_id(r)?,
            equip_sound: sound(r)?,
            asset_id: r.optional(identifier)?,
            camera_overlay: r.optional(identifier)?,
            allowed_entities: r.optional(holder_set)?,
            dispensable: r.bool()?,
            swappable: r.bool()?,
            damage_on_hurt: r.bool()?,
            equip_on_interact: r.bool()?,
            can_be_sheared: r.bool()?,
            shearing_sound: sound(r)?,
        })
    }

    fn write(&self, buf: &mut Vec<u8>) {
        self.slot.write_id(buf);
        write_sound(buf, &self.equip_sound);
        write_optional(buf, &self.asset_id, write_identifier);
        write_optional(buf, &self.camera_overlay, write_identifier);
        write_optional(buf, &self.allowed_entities, write_holder_set);
        for flag in [
            self.dispensable,
            self.swappable,
            self.damage_on_hurt,
            self.equip_on_interact,
            self.can_be_sheared,
        ] {
            write_bool(buf, flag);
        }
        write_sound(buf, &self.shearing_sound);
    }

    fn hash(&self, _: &dyn Names) -> Option<Hash> {
        let sound = |sound: &Sound, default| *sound == Holder::Reference(default);
        let entities = self.allowed_entities.as_ref().map(hash_entries);
        Some(
            Record::new()
                .field("slot", self.slot.hash_name())
                .unless(
                    "equip_sound",
                    sound(&self.equip_sound, SoundEvent::ItemArmorEquipGeneric),
                    || hash_sound(&self.equip_sound),
                )
                .optional("asset_id", self.asset_id.as_ref().map(Hash::identifier))
                .optional(
                    "camera_overlay",
                    self.camera_overlay.as_ref().map(Hash::identifier),
                )
                .optional("allowed_entities", entities)
                .flag("dispensable", self.dispensable, true)
                .flag("swappable", self.swappable, true)
                .flag("damage_on_hurt", self.damage_on_hurt, true)
                .flag("equip_on_interact", self.equip_on_interact, false)
                .flag("can_be_sheared", self.can_be_sheared, false)
                .unless(
                    "shearing_sound",
                    sound(&self.shearing_sound, SoundEvent::ItemShearsSnip),
                    || hash_sound(&self.shearing_sound),
                )
                .build(),
        )
    }
}

/// Vanilla `MaterialAssetGroup`: the texture suffix a trim material uses,
/// with the equipment assets that take another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaterialAssetGroup {
    pub base_suffix: String,
    pub overrides: Vec<(Identifier, String)>,
}

/// Vanilla `TrimMaterial` sent in place of a registry entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectTrimMaterial {
    pub assets: MaterialAssetGroup,
    pub description: Text,
}

/// `TrimMaterial.STREAM_CODEC`.
pub type TrimMaterialHolder = Holder<TrimMaterial, DirectTrimMaterial>;

fn trim_material(r: &mut Reader) -> DecodeResult<TrimMaterialHolder> {
    holder(r, |r| {
        let base_suffix = string(r)?;
        let overrides = r.list(usize::MAX, |r| Ok((identifier(r)?, string(r)?)))?;
        Ok(DirectTrimMaterial {
            assets: MaterialAssetGroup {
                base_suffix,
                overrides,
            },
            description: Text::read(r)?,
        })
    })
}

/// `TrimMaterial.CODEC`: a registered material's name, or the one sent in
/// its place as a record.
fn hash_trim_material(material: &TrimMaterialHolder, names: &dyn Names) -> Option<Hash> {
    let direct = match material {
        Holder::Reference(entry) => return hash_data_entry(entry, names),
        Holder::Direct(direct) => direct,
    };
    let overrides = &direct.assets.overrides;
    Some(
        Record::new()
            .field("asset_name", Hash::string(&direct.assets.base_suffix))
            .unless("override_armor_assets", overrides.is_empty(), || {
                let entries = overrides
                    .iter()
                    .map(|(asset, suffix)| (Hash::identifier(asset), Hash::string(suffix)));
                Hash::map(entries.collect())
            })
            .field("description", direct.description.hash()?)
            .build(),
    )
}

fn write_trim_material(buf: &mut Vec<u8>, material: &TrimMaterialHolder) {
    write_holder(buf, material, |buf, direct| {
        write_utf(buf, &direct.assets.base_suffix);
        write_list(buf, &direct.assets.overrides, |buf, (asset, suffix)| {
            write_identifier(buf, asset);
            write_utf(buf, suffix);
        });
        direct.description.write(buf);
    });
}

/// Vanilla `TrimPattern` sent in place of a registry entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectTrimPattern {
    pub asset_id: Identifier,
    pub description: Text,
    pub decal: bool,
}

/// `TrimPattern.STREAM_CODEC`.
pub type TrimPatternHolder = Holder<TrimPattern, DirectTrimPattern>;

/// Vanilla `ArmorTrim`: the material and pattern armour is trimmed with.
#[derive(Clone, Debug, PartialEq)]
pub struct Trim {
    pub material: TrimMaterialHolder,
    pub pattern: TrimPatternHolder,
}

impl Component for Trim {
    const KIND: DataComponentKind = DataComponentKind::Trim;

    fn read(r: &mut Reader) -> DecodeResult<Self> {
        let material = trim_material(r)?;
        let pattern = holder(r, |r| {
            Ok(DirectTrimPattern {
                asset_id: identifier(r)?,
                description: Text::read(r)?,
                decal: r.bool()?,
            })
        })?;
        Ok(Self { material, pattern })
    }

    fn write(&self, buf: &mut Vec<u8>) {
        write_trim_material(buf, &self.material);
        write_holder(buf, &self.pattern, |buf, direct| {
            write_identifier(buf, &direct.asset_id);
            direct.description.write(buf);
            write_bool(buf, direct.decal);
        });
    }

    fn hash(&self, names: &dyn Names) -> Option<Hash> {
        let pattern = match &self.pattern {
            Holder::Reference(entry) => hash_data_entry(entry, names)?,
            Holder::Direct(direct) => Record::new()
                .field("asset_id", Hash::identifier(&direct.asset_id))
                .field("description", direct.description.hash()?)
                .field("decal", Hash::bool(direct.decal))
                .build(),
        };
        Some(
            Record::new()
                .field("material", hash_trim_material(&self.material, names)?)
                .field("pattern", pattern)
                .build(),
        )
    }
}

/// Vanilla `ProvidesTrimMaterial`: the material an item trims armour with.
#[derive(Clone, Debug, PartialEq)]
pub struct ProvidesTrimMaterial {
    pub value: TrimMaterialHolder,
}

impl Component for ProvidesTrimMaterial {
    const KIND: DataComponentKind = DataComponentKind::ProvidesTrimMaterial;

    fn read(r: &mut Reader) -> DecodeResult<Self> {
        let value = trim_material(r)?;
        Ok(Self { value })
    }

    fn write(&self, buf: &mut Vec<u8>) {
        write_trim_material(buf, &self.value);
    }

    fn hash(&self, names: &dyn Names) -> Option<Hash> {
        hash_trim_material(&self.value, names)
    }
}

id_enum!(
    /// Vanilla `AttributeModifier.Operation`.
    AttributeModifierOperation {
        AddValue = 0, "add_value";
        AddMultipliedBase = 1, "add_multiplied_base";
        AddMultipliedTotal = 2, "add_multiplied_total";
    }
);

/// Vanilla `AttributeModifier`.
#[derive(Clone, Debug, PartialEq)]
pub struct AttributeModifier {
    pub id: Identifier,
    pub amount: f64,
    pub operation: AttributeModifierOperation,
}

/// Vanilla `ItemAttributeModifiers.Display`: how a modifier's tooltip line
/// is shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttributeModifierDisplay {
    Default,
    Hidden,
    Override { text: Text },
}

/// Vanilla `ItemAttributeModifiers.Entry`.
#[derive(Clone, Debug, PartialEq)]
pub struct AttributeModifiersEntry {
    pub kind: Attribute,
    pub modifier: AttributeModifier,
    pub slot: EquipmentSlotGroup,
    pub display: AttributeModifierDisplay,
}

/// Vanilla `ItemAttributeModifiers`: the attribute modifiers an item grants
/// in its slots.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AttributeModifiers {
    pub modifiers: Vec<AttributeModifiersEntry>,
}

impl Component for AttributeModifiers {
    const KIND: DataComponentKind = DataComponentKind::AttributeModifiers;

    fn read(r: &mut Reader) -> DecodeResult<Self> {
        let modifiers = r.list(usize::MAX, |r| {
            Ok(AttributeModifiersEntry {
                kind: registry(r)?,
                modifier: AttributeModifier {
                    id: identifier(r)?,
                    amount: r.f64()?,
                    operation: AttributeModifierOperation::read_id(r)?,
                },
                slot: EquipmentSlotGroup::read_id(r)?,
                // `Display.Type` by id, out of range reading as the first.
                display: match r.varint()? {
                    1 => AttributeModifierDisplay::Hidden,
                    2 => AttributeModifierDisplay::Override {
                        text: Text::read(r)?,
                    },
                    _ => AttributeModifierDisplay::Default,
                },
            })
        })?;
        Ok(Self { modifiers })
    }

    fn write(&self, buf: &mut Vec<u8>) {
        write_list(buf, &self.modifiers, |buf, entry| {
            write_registry(buf, &entry.kind);
            write_identifier(buf, &entry.modifier.id);
            buf.extend_from_slice(&entry.modifier.amount.to_be_bytes());
            entry.modifier.operation.write_id(buf);
            entry.slot.write_id(buf);
            match &entry.display {
                AttributeModifierDisplay::Default => write_varint(buf, 0),
                AttributeModifierDisplay::Hidden => write_varint(buf, 1),
                AttributeModifierDisplay::Override { text } => {
                    write_varint(buf, 2);
                    text.write(buf);
                }
            }
        });
    }

    fn hash(&self, _: &dyn Names) -> Option<Hash> {
        let entries = self.modifiers.iter().map(|entry| {
            let display = match &entry.display {
                AttributeModifierDisplay::Default => None,
                AttributeModifierDisplay::Hidden => {
                    Some(Record::new().field("type", Hash::string("hidden")))
                }
                AttributeModifierDisplay::Override { text } => Some(
                    Record::new()
                        .field("type", Hash::string("override"))
                        .field("value", text.hash()?),
                ),
            };
            Some(
                Record::new()
                    .field("type", hash_entry(&entry.kind))
                    .field("id", Hash::identifier(&entry.modifier.id))
                    .field("amount", Hash::double(entry.modifier.amount))
                    .field("operation", entry.modifier.operation.hash_name())
                    .unless("slot", entry.slot == EquipmentSlotGroup::Any, || {
                        entry.slot.hash_name()
                    })
                    .optional("display", display.map(Record::build))
                    .build(),
            )
        });
        Hash::try_list(entries)
    }
}

/// Declares a component that is `ItemEnchantments`: each enchantment with
/// its level, in the order sent.
macro_rules! enchantments_component {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Debug, Default, PartialEq, Eq)]
        pub struct $name {
            pub levels: Vec<(Enchantment, i32)>,
        }

        impl $name {
            /// `ItemEnchantments.getLevel`: zero for one that isn't there.
            pub fn level(&self, enchantment: Enchantment) -> i32 {
                self.levels
                    .iter()
                    .find(|(kind, _)| *kind == enchantment)
                    .map_or(0, |(_, level)| *level)
            }

            pub fn is_empty(&self) -> bool {
                self.levels.is_empty()
            }
        }

        impl Component for $name {
            const KIND: DataComponentKind = DataComponentKind::$name;

            fn read(r: &mut Reader) -> DecodeResult<Self> {
                let levels = r.list(usize::MAX, |r| Ok((registry(r)?, r.var_i32()?)))?;
                Ok(Self { levels })
            }

            fn write(&self, buf: &mut Vec<u8>) {
                write_list(buf, &self.levels, |buf, (enchantment, level)| {
                    write_registry(buf, enchantment);
                    write_var_i32(buf, *level);
                });
            }

            fn hash(&self, names: &dyn Names) -> Option<Hash> {
                let levels = self.levels.iter().map(|(enchantment, level)| {
                    Some((hash_data_entry(enchantment, names)?, Hash::int(*level)))
                });
                Some(Hash::map(levels.collect::<Option<_>>()?))
            }
        }
    };
}

enchantments_component!(
    /// Vanilla `DataComponents.ENCHANTMENTS`: the enchantments on an item.
    Enchantments
);
enchantments_component!(
    /// Vanilla `DataComponents.STORED_ENCHANTMENTS`: the enchantments an
    /// enchanted book holds.
    StoredEnchantments
);
