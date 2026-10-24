//! Pomme-owned villager type and profession ids. Discriminant order matches
//! the vanilla registry bootstrap order, which the texture tables in
//! `entity_renderer` and the hat tables in `in_game` index by.

/// The enum plus `from_id`, which maps a registry id to its variant by
/// position.
macro_rules! registry_enum {
    ($bootstrap:literal, $name:ident { $($(#[$attr:meta])* $variant:ident,)* }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
        pub enum $name {
            $($(#[$attr])* $variant,)*
        }

        impl $name {
            const ALL: &[Self] = &[$(Self::$variant),*];

            #[doc = concat!("The registry id's entry (`", $bootstrap, "`).")]
            pub fn from_id(id: u32) -> Option<Self> {
                Self::ALL.get(id as usize).copied()
            }
        }
    };
}

registry_enum!(
    "VillagerType.bootstrap",
    VillagerKind {
        Desert,
        Jungle,
        #[default]
        Plains,
        Savanna,
        Snow,
        Swamp,
        Taiga,
    }
);

registry_enum!(
    "VillagerProfession.bootstrap",
    VillagerProfession {
        #[default]
        None,
        Armorer,
        Butcher,
        Cartographer,
        Cleric,
        Farmer,
        Fisherman,
        Fletcher,
        Leatherworker,
        Librarian,
        Mason,
        Nitwit,
        Shepherd,
        Toolsmith,
        Weaponsmith,
    }
);
