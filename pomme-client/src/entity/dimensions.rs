//! Entity hitbox sizes: vanilla `EntityType.Builder.sized` for every type
//! registered in `EntityTypes.java`.

use azalea_registry::builtin::EntityKind;
use glam::DVec3;

use crate::physics::aabb::Aabb;

/// Vanilla `EntityDimensions`' width and height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EntityDimensions {
    pub width: f32,
    pub height: f32,
}

impl EntityDimensions {
    /// The type's standing size; types not in the table take
    /// `EntityType.Builder`'s default.
    pub fn of(kind: EntityKind) -> Self {
        let (width, height) = SIZES
            .binary_search_by(|(id, ..)| id.cmp(&kind.to_str()))
            .map_or((0.6, 1.8), |i| (SIZES[i].1, SIZES[i].2));
        Self { width, height }
    }

    /// Vanilla `EntityDimensions.makeBoundingBox`: centred on `pos` in x and
    /// z, rising `height` from it.
    pub fn make_bounding_box(&self, pos: DVec3) -> Aabb {
        let radius = f64::from(self.width) / 2.0;
        Aabb::new(
            DVec3::new(pos.x - radius, pos.y, pos.z - radius),
            DVec3::new(
                pos.x + radius,
                pos.y + f64::from(self.height),
                pos.z + radius,
            ),
        )
    }
}

/// `(registry id, width, height)`, sorted by id.
#[rustfmt::skip]
const SIZES: &[(&str, f32, f32)] = &[
    ("minecraft:acacia_boat", 1.375, 0.5625),
    ("minecraft:acacia_chest_boat", 1.375, 0.5625),
    ("minecraft:allay", 0.35, 0.6),
    ("minecraft:area_effect_cloud", 6.0, 0.5),
    ("minecraft:armadillo", 0.7, 0.65),
    ("minecraft:armor_stand", 0.5, 1.975),
    ("minecraft:arrow", 0.5, 0.5),
    ("minecraft:axolotl", 0.75, 0.42),
    ("minecraft:bamboo_chest_raft", 1.375, 0.5625),
    ("minecraft:bamboo_raft", 1.375, 0.5625),
    ("minecraft:bat", 0.5, 0.9),
    ("minecraft:bee", 0.55, 0.5),
    ("minecraft:birch_boat", 1.375, 0.5625),
    ("minecraft:birch_chest_boat", 1.375, 0.5625),
    ("minecraft:blaze", 0.6, 1.8),
    ("minecraft:block_display", 0.0, 0.0),
    ("minecraft:bogged", 0.6, 1.99),
    ("minecraft:breeze", 0.6, 1.77),
    ("minecraft:breeze_wind_charge", 0.3125, 0.3125),
    ("minecraft:camel", 1.7, 2.375),
    ("minecraft:camel_husk", 1.7, 2.375),
    ("minecraft:cat", 0.6, 0.7),
    ("minecraft:cave_spider", 0.7, 0.5),
    ("minecraft:cherry_boat", 1.375, 0.5625),
    ("minecraft:cherry_chest_boat", 1.375, 0.5625),
    ("minecraft:chest_minecart", 0.98, 0.7),
    ("minecraft:chicken", 0.4, 0.7),
    ("minecraft:cod", 0.5, 0.3),
    ("minecraft:command_block_minecart", 0.98, 0.7),
    ("minecraft:copper_golem", 0.49, 0.98),
    ("minecraft:cow", 0.9, 1.4),
    ("minecraft:creaking", 0.9, 2.7),
    ("minecraft:creeper", 0.6, 1.7),
    ("minecraft:dark_oak_boat", 1.375, 0.5625),
    ("minecraft:dark_oak_chest_boat", 1.375, 0.5625),
    ("minecraft:dolphin", 0.9, 0.6),
    ("minecraft:donkey", 1.3964844, 1.5),
    ("minecraft:dragon_fireball", 1.0, 1.0),
    ("minecraft:drowned", 0.6, 1.95),
    ("minecraft:egg", 0.25, 0.25),
    ("minecraft:elder_guardian", 1.9975, 1.9975),
    ("minecraft:end_crystal", 2.0, 2.0),
    ("minecraft:ender_dragon", 16.0, 8.0),
    ("minecraft:ender_pearl", 0.25, 0.25),
    ("minecraft:enderman", 0.6, 2.9),
    ("minecraft:endermite", 0.4, 0.3),
    ("minecraft:evoker", 0.6, 1.95),
    ("minecraft:evoker_fangs", 0.5, 0.8),
    ("minecraft:experience_bottle", 0.25, 0.25),
    ("minecraft:experience_orb", 0.5, 0.5),
    ("minecraft:eye_of_ender", 0.25, 0.25),
    ("minecraft:falling_block", 0.98, 0.98),
    ("minecraft:fireball", 1.0, 1.0),
    ("minecraft:firework_rocket", 0.25, 0.25),
    ("minecraft:fishing_bobber", 0.25, 0.25),
    ("minecraft:fox", 0.6, 0.7),
    ("minecraft:frog", 0.5, 0.5),
    ("minecraft:furnace_minecart", 0.98, 0.7),
    ("minecraft:ghast", 4.0, 4.0),
    ("minecraft:giant", 3.6, 12.0),
    ("minecraft:glow_item_frame", 0.5, 0.5),
    ("minecraft:glow_squid", 0.8, 0.8),
    ("minecraft:goat", 0.9, 1.3),
    ("minecraft:guardian", 0.85, 0.85),
    ("minecraft:happy_ghast", 4.0, 4.0),
    ("minecraft:hoglin", 1.3964844, 1.4),
    ("minecraft:hopper_minecart", 0.98, 0.7),
    ("minecraft:horse", 1.3964844, 1.6),
    ("minecraft:husk", 0.6, 1.95),
    ("minecraft:illusioner", 0.6, 1.95),
    ("minecraft:interaction", 0.0, 0.0),
    ("minecraft:iron_golem", 1.4, 2.7),
    ("minecraft:item", 0.25, 0.25),
    ("minecraft:item_display", 0.0, 0.0),
    ("minecraft:item_frame", 0.5, 0.5),
    ("minecraft:jungle_boat", 1.375, 0.5625),
    ("minecraft:jungle_chest_boat", 1.375, 0.5625),
    ("minecraft:leash_knot", 0.375, 0.5),
    ("minecraft:lightning_bolt", 0.0, 0.0),
    ("minecraft:lingering_potion", 0.25, 0.25),
    ("minecraft:llama", 0.9, 1.87),
    ("minecraft:llama_spit", 0.25, 0.25),
    ("minecraft:magma_cube", 0.52, 0.52),
    ("minecraft:mangrove_boat", 1.375, 0.5625),
    ("minecraft:mangrove_chest_boat", 1.375, 0.5625),
    ("minecraft:mannequin", 0.6, 1.8),
    ("minecraft:marker", 0.0, 0.0),
    ("minecraft:minecart", 0.98, 0.7),
    ("minecraft:mooshroom", 0.9, 1.4),
    ("minecraft:mule", 1.3964844, 1.6),
    ("minecraft:nautilus", 0.875, 0.95),
    ("minecraft:oak_boat", 1.375, 0.5625),
    ("minecraft:oak_chest_boat", 1.375, 0.5625),
    ("minecraft:ocelot", 0.6, 0.7),
    ("minecraft:ominous_item_spawner", 0.25, 0.25),
    ("minecraft:painting", 0.5, 0.5),
    ("minecraft:pale_oak_boat", 1.375, 0.5625),
    ("minecraft:pale_oak_chest_boat", 1.375, 0.5625),
    ("minecraft:panda", 1.3, 1.25),
    ("minecraft:parched", 0.6, 1.99),
    ("minecraft:parrot", 0.5, 0.9),
    ("minecraft:phantom", 0.9, 0.5),
    ("minecraft:pig", 0.9, 0.9),
    ("minecraft:piglin", 0.6, 1.95),
    ("minecraft:piglin_brute", 0.6, 1.95),
    ("minecraft:pillager", 0.6, 1.95),
    ("minecraft:player", 0.6, 1.8),
    ("minecraft:polar_bear", 1.4, 1.4),
    ("minecraft:pufferfish", 0.7, 0.7),
    ("minecraft:rabbit", 0.49, 0.6),
    ("minecraft:ravager", 1.95, 2.2),
    ("minecraft:salmon", 0.7, 0.4),
    ("minecraft:sheep", 0.9, 1.3),
    ("minecraft:shulker", 1.0, 1.0),
    ("minecraft:shulker_bullet", 0.3125, 0.3125),
    ("minecraft:silverfish", 0.4, 0.3),
    ("minecraft:skeleton", 0.6, 1.99),
    ("minecraft:skeleton_horse", 1.3964844, 1.6),
    ("minecraft:slime", 0.52, 0.52),
    ("minecraft:small_fireball", 0.3125, 0.3125),
    ("minecraft:sniffer", 1.9, 1.75),
    ("minecraft:snow_golem", 0.7, 1.9),
    ("minecraft:snowball", 0.25, 0.25),
    ("minecraft:spawner_minecart", 0.98, 0.7),
    ("minecraft:spectral_arrow", 0.5, 0.5),
    ("minecraft:spider", 1.4, 0.9),
    ("minecraft:splash_potion", 0.25, 0.25),
    ("minecraft:spruce_boat", 1.375, 0.5625),
    ("minecraft:spruce_chest_boat", 1.375, 0.5625),
    ("minecraft:squid", 0.8, 0.8),
    ("minecraft:stray", 0.6, 1.99),
    ("minecraft:strider", 0.9, 1.7),
    ("minecraft:sulfur_cube", 0.49, 0.49),
    ("minecraft:tadpole", 0.4, 0.3),
    ("minecraft:text_display", 0.0, 0.0),
    ("minecraft:tnt", 0.98, 0.98),
    ("minecraft:tnt_minecart", 0.98, 0.7),
    ("minecraft:trader_llama", 0.9, 1.87),
    ("minecraft:trident", 0.5, 0.5),
    ("minecraft:tropical_fish", 0.5, 0.4),
    ("minecraft:turtle", 1.2, 0.4),
    ("minecraft:vex", 0.4, 0.8),
    ("minecraft:villager", 0.6, 1.95),
    ("minecraft:vindicator", 0.6, 1.95),
    ("minecraft:wandering_trader", 0.6, 1.95),
    ("minecraft:warden", 0.9, 2.9),
    ("minecraft:wind_charge", 0.3125, 0.3125),
    ("minecraft:witch", 0.6, 1.95),
    ("minecraft:wither", 0.9, 3.5),
    ("minecraft:wither_skeleton", 0.7, 2.4),
    ("minecraft:wither_skull", 0.3125, 0.3125),
    ("minecraft:wolf", 0.6, 0.85),
    ("minecraft:zoglin", 1.3964844, 1.4),
    ("minecraft:zombie", 0.6, 1.95),
    ("minecraft:zombie_horse", 1.3964844, 1.6),
    ("minecraft:zombie_nautilus", 0.875, 0.95),
    ("minecraft:zombie_villager", 0.6, 1.95),
    ("minecraft:zombified_piglin", 0.6, 1.95),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_sorted_for_lookup() {
        assert!(SIZES.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn looks_up_vanilla_sizes() {
        assert_eq!(
            EntityDimensions::of(EntityKind::Zombie),
            EntityDimensions {
                width: 0.6,
                height: 1.95
            }
        );
        assert_eq!(
            EntityDimensions::of(EntityKind::AcaciaBoat),
            EntityDimensions {
                width: 1.375,
                height: 0.5625
            }
        );
    }
}
