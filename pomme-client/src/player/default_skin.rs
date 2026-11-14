//! Vanilla `DefaultPlayerSkin`: the skin a player without one of their own
//! draws, picked by profile id.

use crate::resources::{ResourceId, resource_id};

macro_rules! default_skins {
    ($($model:literal / $name:literal),+ $(,)?) => {
        /// `DEFAULT_SKINS`, in vanilla's order, as texture identifiers.
        pub const DEFAULT_SKINS: [&str; 18] =
            [$(concat!("minecraft:entity/player/", $model, "/", $name)),+];
        /// [`DEFAULT_SKINS`] as resources, each a one-entry fallback chain.
        pub static TEXTURES: [&[ResourceId]; 18] = [$(&[resource_id!(concat!(
            "minecraft:textures/entity/player/", $model, "/", $name, ".png"
        ))]),+];
    };
}

default_skins!(
    "slim" / "alex",
    "slim" / "ari",
    "slim" / "efe",
    "slim" / "kai",
    "slim" / "makena",
    "slim" / "noor",
    "slim" / "steve",
    "slim" / "sunny",
    "slim" / "zuri",
    "wide" / "alex",
    "wide" / "ari",
    "wide" / "efe",
    "wide" / "kai",
    "wide" / "makena",
    "wide" / "noor",
    "wide" / "steve",
    "wide" / "sunny",
    "wide" / "zuri",
);

/// The slim (3px-arm) skins come first.
pub const SLIM_COUNT: usize = 9;

/// `getDefaultSkin`: wide Steve, for no profile at all.
pub const STEVE: usize = 15;

/// `get(UUID)`: the [`DEFAULT_SKINS`] index a profile id draws.
pub fn index(id: &uuid::Uuid) -> usize {
    crate::world::waypoints::java_uuid_hash(id).rem_euclid(DEFAULT_SKINS.len() as i32) as usize
}

/// `get(partialProfile())`: by the profile's id, else the offline id of its
/// name, else the nil id.
pub fn for_profile(id: Option<uuid::Uuid>, name: Option<&str>) -> usize {
    index(
        &id.or_else(|| name.map(crate::user::UserData::offline_uuid))
            .unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_by_java_uuid_hash() {
        // `UUIDUtil.createOfflinePlayerUUID("Tester")` hashes to -375254014,
        // and floorMod(-375254014, 18) = 14.
        let tester = crate::user::UserData::offline_uuid("Tester");
        assert_eq!(
            DEFAULT_SKINS[index(&tester)],
            "minecraft:entity/player/wide/noor"
        );
        assert_eq!(DEFAULT_SKINS[STEVE], "minecraft:entity/player/wide/steve");
        assert!(DEFAULT_SKINS[SLIM_COUNT - 1].contains("/slim/"));
        assert!(DEFAULT_SKINS[SLIM_COUNT].contains("/wide/"));
    }
}
