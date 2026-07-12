//! Pomme-owned villager type and profession ids. Discriminant order matches
//! the vanilla registry bootstrap order, which the texture tables in
//! `entity_renderer` and the hat tables in `in_game` index by.

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum VillagerKind {
    Desert,
    Jungle,
    #[default]
    Plains,
    Savanna,
    Snow,
    Swamp,
    Taiga,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum VillagerProfession {
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

// The azalea boundary; drop these once the netcode decodes villager data
// itself.

impl From<azalea_registry::builtin::VillagerKind> for VillagerKind {
    fn from(kind: azalea_registry::builtin::VillagerKind) -> Self {
        use azalea_registry::builtin::VillagerKind as Az;
        match kind {
            Az::Desert => Self::Desert,
            Az::Jungle => Self::Jungle,
            Az::Plains => Self::Plains,
            Az::Savanna => Self::Savanna,
            Az::Snow => Self::Snow,
            Az::Swamp => Self::Swamp,
            Az::Taiga => Self::Taiga,
        }
    }
}

impl From<azalea_registry::builtin::VillagerProfession> for VillagerProfession {
    fn from(profession: azalea_registry::builtin::VillagerProfession) -> Self {
        use azalea_registry::builtin::VillagerProfession as Az;
        match profession {
            Az::None => Self::None,
            Az::Armorer => Self::Armorer,
            Az::Butcher => Self::Butcher,
            Az::Cartographer => Self::Cartographer,
            Az::Cleric => Self::Cleric,
            Az::Farmer => Self::Farmer,
            Az::Fisherman => Self::Fisherman,
            Az::Fletcher => Self::Fletcher,
            Az::Leatherworker => Self::Leatherworker,
            Az::Librarian => Self::Librarian,
            Az::Mason => Self::Mason,
            Az::Nitwit => Self::Nitwit,
            Az::Shepherd => Self::Shepherd,
            Az::Toolsmith => Self::Toolsmith,
            Az::Weaponsmith => Self::Weaponsmith,
        }
    }
}
