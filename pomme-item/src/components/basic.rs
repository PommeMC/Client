//! Components holding one plain value: durability, colours, names and what
//! a tooltip shows.

use pomme_protocol::codec::{DecodeResult, Reader};
use pomme_registry::builtin::DataComponentKind;

use super::Component;
use super::codec::{
    Nbt, Text, hash_compound, hash_entry, id_enum, registry, string, write_bool, write_f32,
    write_i32, write_list, write_registry, write_var_i32,
};
use crate::hash::{Hash, Names, Record};

/// Declares a component that is one var-int, a `ByteBufCodecs.VAR_INT`.
macro_rules! var_int_component {
    ($(#[$meta:meta])* $name:ident, $field:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct $name {
            pub $field: i32,
        }

        impl Component for $name {
            const KIND: DataComponentKind = DataComponentKind::$name;

            fn read(r: &mut Reader) -> DecodeResult<Self> {
                let $field = r.var_i32()?;
                Ok(Self { $field })
            }

            fn write(&self, buf: &mut Vec<u8>) {
                write_var_i32(buf, self.$field);
            }

            fn hash(&self, _: &dyn Names) -> Option<Hash> {
                Some(Hash::int(self.$field))
            }
        }
    };
}

var_int_component!(
    /// Vanilla `DataComponents.MAX_STACK_SIZE`: how many of the item a slot
    /// holds.
    MaxStackSize,
    count
);
var_int_component!(
    /// Vanilla `DataComponents.DAMAGE`: the durability used up.
    Damage,
    amount
);
var_int_component!(
    /// Vanilla `DataComponents.MAX_DAMAGE`: the durability an item has.
    MaxDamage,
    amount
);

/// Vanilla `DataComponents.UNBREAKABLE`: a unit, the item never takes damage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unbreakable;

impl Component for Unbreakable {
    const KIND: DataComponentKind = DataComponentKind::Unbreakable;

    fn read(_: &mut Reader) -> DecodeResult<Self> {
        Ok(Self)
    }

    fn write(&self, _: &mut Vec<u8>) {}

    /// `Unit.CODEC`: an empty map.
    fn hash(&self, _: &dyn Names) -> Option<Hash> {
        Some(Record::new().build())
    }
}

/// Declares a component that is one colour, a `ByteBufCodecs.INT`.
macro_rules! color_component {
    ($(#[$meta:meta])* $name:ident, $field:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct $name {
            pub $field: i32,
        }

        impl Component for $name {
            const KIND: DataComponentKind = DataComponentKind::$name;

            fn read(r: &mut Reader) -> DecodeResult<Self> {
                let $field = r.i32()?;
                Ok(Self { $field })
            }

            fn write(&self, buf: &mut Vec<u8>) {
                write_i32(buf, self.$field);
            }

            fn hash(&self, _: &dyn Names) -> Option<Hash> {
                Some(Hash::int(self.$field))
            }
        }
    };
}

color_component!(
    /// Vanilla `DyedItemColor`: the colour leather armour is dyed.
    DyedColor,
    rgb
);
color_component!(
    /// Vanilla `MapItemColor`: the tint of a filled map's markings.
    MapColor,
    color
);

id_enum!(
    /// Vanilla `Rarity`, which colours an item's name.
    Rarity {
        Common = 0, "common";
        Uncommon = 1, "uncommon";
        Rare = 2, "rare";
        Epic = 3, "epic";
    }
);

impl Component for Rarity {
    const KIND: DataComponentKind = DataComponentKind::Rarity;

    fn read(r: &mut Reader) -> DecodeResult<Self> {
        Self::read_id(r)
    }

    fn write(&self, buf: &mut Vec<u8>) {
        self.write_id(buf);
    }

    fn hash(&self, _: &dyn Names) -> Option<Hash> {
        Some((*self).hash_name())
    }
}

id_enum!(
    /// Vanilla `DyeColor`.
    DyeColor {
        White = 0, "white";
        Orange = 1, "orange";
        Magenta = 2, "magenta";
        LightBlue = 3, "light_blue";
        Yellow = 4, "yellow";
        Lime = 5, "lime";
        Pink = 6, "pink";
        Gray = 7, "gray";
        LightGray = 8, "light_gray";
        Cyan = 9, "cyan";
        Purple = 10, "purple";
        Blue = 11, "blue";
        Brown = 12, "brown";
        Green = 13, "green";
        Red = 14, "red";
        Black = 15, "black";
    }
);

/// Vanilla `DataComponents.DYE`: the colour an item dyes with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dye {
    pub color: DyeColor,
}

impl Component for Dye {
    const KIND: DataComponentKind = DataComponentKind::Dye;

    fn read(r: &mut Reader) -> DecodeResult<Self> {
        let color = DyeColor::read_id(r)?;
        Ok(Self { color })
    }

    fn write(&self, buf: &mut Vec<u8>) {
        self.color.write_id(buf);
    }

    fn hash(&self, _: &dyn Names) -> Option<Hash> {
        Some(self.color.hash_name())
    }
}

/// Declares a component that is one text.
macro_rules! text_component {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub struct $name {
            pub name: Text,
        }

        impl Component for $name {
            const KIND: DataComponentKind = DataComponentKind::$name;

            fn read(r: &mut Reader) -> DecodeResult<Self> {
                let name = Text::read(r)?;
                Ok(Self { name })
            }

            fn write(&self, buf: &mut Vec<u8>) {
                self.name.write(buf);
            }

            fn hash(&self, _: &dyn Names) -> Option<Hash> {
                self.name.hash()
            }
        }
    };
}

text_component!(
    /// Vanilla `DataComponents.CUSTOM_NAME`: the name a player gave the
    /// stack.
    CustomName
);
text_component!(
    /// Vanilla `DataComponents.ITEM_NAME`: the item's own name.
    ItemName
);

/// Vanilla `ItemLore`: the lines under an item's name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lore {
    pub lines: Vec<Text>,
}

impl Lore {
    /// `ItemLore.MAX_LINES`.
    const MAX_LINES: usize = 256;
}

impl Component for Lore {
    const KIND: DataComponentKind = DataComponentKind::Lore;

    fn read(r: &mut Reader) -> DecodeResult<Self> {
        let lines = r.list(Self::MAX_LINES, Text::read)?;
        Ok(Self { lines })
    }

    fn write(&self, buf: &mut Vec<u8>) {
        write_list(buf, &self.lines, |buf, line| line.write(buf));
    }

    fn hash(&self, _: &dyn Names) -> Option<Hash> {
        let lines = self.lines.iter().map(Text::hash);
        Hash::try_list(lines)
    }
}

/// Vanilla `CustomData`: a compound the game doesn't interpret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomData {
    pub nbt: Nbt,
}

impl Component for CustomData {
    const KIND: DataComponentKind = DataComponentKind::CustomData;

    fn read(r: &mut Reader) -> DecodeResult<Self> {
        let nbt = Nbt::read(r)?;
        Ok(Self { nbt })
    }

    fn write(&self, buf: &mut Vec<u8>) {
        self.nbt.write(buf);
    }

    fn hash(&self, _: &dyn Names) -> Option<Hash> {
        Some(hash_compound(&self.nbt.compound()?, None))
    }
}

/// Vanilla `TooltipDisplay`: whether the stack has a tooltip, and the
/// components left out of it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TooltipDisplay {
    pub hide_tooltip: bool,
    pub hidden_components: Vec<DataComponentKind>,
}

impl Component for TooltipDisplay {
    const KIND: DataComponentKind = DataComponentKind::TooltipDisplay;

    fn read(r: &mut Reader) -> DecodeResult<Self> {
        Ok(Self {
            hide_tooltip: r.bool()?,
            hidden_components: r.list(usize::MAX, registry)?,
        })
    }

    fn write(&self, buf: &mut Vec<u8>) {
        write_bool(buf, self.hide_tooltip);
        write_list(buf, &self.hidden_components, write_registry);
    }

    fn hash(&self, _: &dyn Names) -> Option<Hash> {
        let hidden = &self.hidden_components;
        Some(
            Record::new()
                .flag("hide_tooltip", self.hide_tooltip, false)
                .unless("hidden_components", hidden.is_empty(), || {
                    Hash::list(hidden.iter().map(hash_entry))
                })
                .build(),
        )
    }
}

/// Vanilla `CustomModelData`: the values an item model's selectors read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CustomModelData {
    pub floats: Vec<f32>,
    pub flags: Vec<bool>,
    pub strings: Vec<String>,
    pub colors: Vec<i32>,
}

impl Component for CustomModelData {
    const KIND: DataComponentKind = DataComponentKind::CustomModelData;

    fn read(r: &mut Reader) -> DecodeResult<Self> {
        Ok(Self {
            floats: r.list(usize::MAX, Reader::f32)?,
            flags: r.list(usize::MAX, Reader::bool)?,
            strings: r.list(usize::MAX, string)?,
            colors: r.list(usize::MAX, Reader::i32)?,
        })
    }

    fn write(&self, buf: &mut Vec<u8>) {
        write_list(buf, &self.floats, |buf, v| write_f32(buf, *v));
        write_list(buf, &self.flags, |buf, v| write_bool(buf, *v));
        write_list(buf, &self.strings, |buf, v| {
            pomme_protocol::wire::write_utf(buf, v);
        });
        write_list(buf, &self.colors, |buf, v| write_i32(buf, *v));
    }

    fn hash(&self, _: &dyn Names) -> Option<Hash> {
        let floats = || Hash::list(self.floats.iter().map(|v| Hash::float(*v)));
        let flags = || Hash::list(self.flags.iter().map(|v| Hash::bool(*v)));
        let strings = || Hash::list(self.strings.iter().map(|v| Hash::string(v)));
        let colors = || Hash::list(self.colors.iter().map(|v| Hash::int(*v)));
        Some(
            Record::new()
                .unless("floats", self.floats.is_empty(), floats)
                .unless("flags", self.flags.is_empty(), flags)
                .unless("strings", self.strings.is_empty(), strings)
                .unless("colors", self.colors.is_empty(), colors)
                .build(),
        )
    }
}
