//! Entity hitbox sizes: vanilla `EntityType.Builder.sized` for every type
//! registered in `EntityTypes.java`.

use azalea_registry::builtin::EntityKind;
use glam::DVec3;

use crate::physics::aabb::Aabb;
use crate::physics::movement::{DEG_TO_RAD, mth_cos, mth_sin};

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

    /// The type's standing eye height: its builder's `eyeHeight`, else
    /// `EntityType.Builder`'s default of 0.85 of its height.
    pub fn eye_height(kind: EntityKind) -> f32 {
        EYE_HEIGHTS
            .binary_search_by(|(id, _)| id.cmp(&kind.to_str()))
            .map_or_else(
                |_| Self::of(kind).height * DEFAULT_EYE_HEIGHT_RATIO,
                |i| EYE_HEIGHTS[i].1,
            )
    }

    /// Vanilla `EntityAttachments.getClamped(PASSENGER, seat, yRot)`: where
    /// the `seat`th passenger rides (the last point serves any further
    /// seats), turned by the vehicle's `y_rot`.
    // TODO: the attachments of babies and scaled entities scale with them.
    pub fn passenger_attachment(kind: EntityKind, seat: usize, y_rot: f32) -> DVec3 {
        let point = PASSENGER_ATTACHMENTS
            .binary_search_by(|(id, _)| id.cmp(&kind.to_str()))
            .map_or([0.0, Self::of(kind).height, 0.0], |i| {
                let points = PASSENGER_ATTACHMENTS[i].1;
                points[seat.min(points.len() - 1)]
            });
        attachment_point(point, y_rot)
    }

    /// Vanilla `getVehicleAttachmentPoint`: the point of a rider of `kind`
    /// that its seat holds, turned by the rider's `y_rot`.
    pub fn vehicle_attachment(kind: EntityKind, y_rot: f32) -> DVec3 {
        let point = VEHICLE_ATTACHMENTS
            .binary_search_by(|(id, _)| id.cmp(&kind.to_str()))
            .map_or([0.0; 3], |i| VEHICLE_ATTACHMENTS[i].1);
        attachment_point(point, y_rot)
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

/// Vanilla `EntityAttachments.transformPoint`: `point.yRot(-y_rot)`.
pub fn attachment_point([x, y, z]: [f32; 3], y_rot: f32) -> DVec3 {
    let angle = -y_rot * DEG_TO_RAD;
    let (sin, cos) = (f64::from(mth_sin(angle)), f64::from(mth_cos(angle)));
    let (x, z) = (f64::from(x), f64::from(z));
    DVec3::new(x * cos + z * sin, f64::from(y), z * cos - x * sin)
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

/// `EntityType.Builder`'s eye height, as a fraction of the height, when it
/// sets none.
pub const DEFAULT_EYE_HEIGHT_RATIO: f32 = 0.85;

/// `(registry id, eye height)` for every type whose builder sets `eyeHeight`,
/// sorted by id.
#[rustfmt::skip]
const EYE_HEIGHTS: &[(&str, f32)] = &[
    ("minecraft:acacia_boat", 0.5625),
    ("minecraft:acacia_chest_boat", 0.5625),
    ("minecraft:allay", 0.36),
    ("minecraft:armadillo", 0.26),
    ("minecraft:armor_stand", 1.7775),
    ("minecraft:arrow", 0.13),
    ("minecraft:axolotl", 0.2751),
    ("minecraft:bamboo_chest_raft", 0.5625),
    ("minecraft:bamboo_raft", 0.5625),
    ("minecraft:bat", 0.45),
    ("minecraft:bee", 0.3),
    ("minecraft:birch_boat", 0.5625),
    ("minecraft:birch_chest_boat", 0.5625),
    ("minecraft:bogged", 1.74),
    ("minecraft:breeze", 1.3452),
    ("minecraft:breeze_wind_charge", 0.0),
    ("minecraft:camel", 2.275),
    ("minecraft:camel_husk", 2.275),
    ("minecraft:cat", 0.35),
    ("minecraft:cave_spider", 0.45),
    ("minecraft:cherry_boat", 0.5625),
    ("minecraft:cherry_chest_boat", 0.5625),
    ("minecraft:chicken", 0.644),
    ("minecraft:cod", 0.195),
    ("minecraft:copper_golem", 0.8125),
    ("minecraft:cow", 1.3),
    ("minecraft:creaking", 2.3),
    ("minecraft:dark_oak_boat", 0.5625),
    ("minecraft:dark_oak_chest_boat", 0.5625),
    ("minecraft:dolphin", 0.3),
    ("minecraft:donkey", 1.425),
    ("minecraft:drowned", 1.74),
    ("minecraft:elder_guardian", 0.99875),
    ("minecraft:enderman", 2.55),
    ("minecraft:endermite", 0.13),
    ("minecraft:fox", 0.4),
    ("minecraft:ghast", 2.6),
    ("minecraft:giant", 10.44),
    ("minecraft:glow_item_frame", 0.0),
    ("minecraft:glow_squid", 0.4),
    ("minecraft:guardian", 0.425),
    ("minecraft:happy_ghast", 2.6),
    ("minecraft:horse", 1.52),
    ("minecraft:husk", 1.74),
    ("minecraft:item", 0.2125),
    ("minecraft:item_frame", 0.0),
    ("minecraft:jungle_boat", 0.5625),
    ("minecraft:jungle_chest_boat", 0.5625),
    ("minecraft:leash_knot", 0.0625),
    ("minecraft:llama", 1.7765),
    ("minecraft:magma_cube", 0.325),
    ("minecraft:mangrove_boat", 0.5625),
    ("minecraft:mangrove_chest_boat", 0.5625),
    ("minecraft:mannequin", 1.62),
    ("minecraft:mooshroom", 1.3),
    ("minecraft:mule", 1.52),
    ("minecraft:nautilus", 0.2751),
    ("minecraft:oak_boat", 0.5625),
    ("minecraft:oak_chest_boat", 0.5625),
    ("minecraft:pale_oak_boat", 0.5625),
    ("minecraft:pale_oak_chest_boat", 0.5625),
    ("minecraft:parched", 1.74),
    ("minecraft:parrot", 0.54),
    ("minecraft:phantom", 0.175),
    ("minecraft:piglin", 1.79),
    ("minecraft:piglin_brute", 1.79),
    ("minecraft:player", 1.62),
    ("minecraft:poplar_boat", 0.5625),
    ("minecraft:poplar_chest_boat", 0.5625),
    ("minecraft:pufferfish", 0.455),
    ("minecraft:rabbit", 0.59),
    ("minecraft:salmon", 0.26),
    ("minecraft:sheep", 1.235),
    ("minecraft:shulker", 0.5),
    ("minecraft:silverfish", 0.13),
    ("minecraft:skeleton", 1.74),
    ("minecraft:skeleton_horse", 1.52),
    ("minecraft:slime", 0.325),
    ("minecraft:sniffer", 1.05),
    ("minecraft:snow_golem", 1.7),
    ("minecraft:spectral_arrow", 0.13),
    ("minecraft:spider", 0.65),
    ("minecraft:spruce_boat", 0.5625),
    ("minecraft:spruce_chest_boat", 0.5625),
    ("minecraft:squid", 0.4),
    ("minecraft:stray", 1.74),
    ("minecraft:sulfur_cube", 0.175),
    ("minecraft:tadpole", 0.19500001),
    ("minecraft:tnt", 0.15),
    ("minecraft:trader_llama", 1.7765),
    ("minecraft:trident", 0.13),
    ("minecraft:tropical_fish", 0.26),
    ("minecraft:vex", 0.51875),
    ("minecraft:villager", 1.62),
    ("minecraft:wandering_trader", 1.62),
    ("minecraft:wind_charge", 0.0),
    ("minecraft:witch", 1.62),
    ("minecraft:wither_skeleton", 2.1),
    ("minecraft:wolf", 0.68),
    ("minecraft:zombie", 1.74),
    ("minecraft:zombie_horse", 1.52),
    ("minecraft:zombie_nautilus", 0.2751),
    ("minecraft:zombie_villager", 1.74),
    ("minecraft:zombified_piglin", 1.79),
];

/// `(registry id, points)` from `EntityTypes`' `passengerAttachments`, sorted
/// by id; other types seat a passenger at their height.
#[rustfmt::skip]
const PASSENGER_ATTACHMENTS: &[(&str, &[[f32; 3]])] = &[
    ("minecraft:cat", &[[0.0, 0.5125, 0.0]]),
    ("minecraft:chest_minecart", &[[0.0, 0.1875, 0.0]]),
    ("minecraft:chicken", &[[0.0, 0.7, -0.1]]),
    ("minecraft:command_block_minecart", &[[0.0, 0.1875, 0.0]]),
    ("minecraft:cow", &[[0.0, 1.36875, 0.0]]),
    ("minecraft:donkey", &[[0.0, 1.1125, 0.0]]),
    ("minecraft:drowned", &[[0.0, 2.0125, 0.0]]),
    ("minecraft:elder_guardian", &[[0.0, 2.350625, 0.0]]),
    ("minecraft:ender_dragon", &[[0.0, 3.0, 0.0]]),
    ("minecraft:enderman", &[[0.0, 2.80625, 0.0]]),
    ("minecraft:endermite", &[[0.0, 0.2375, 0.0]]),
    ("minecraft:evoker", &[[0.0, 2.0, 0.0]]),
    ("minecraft:fox", &[[0.0, 0.6375, -0.25]]),
    ("minecraft:frog", &[[0.0, 0.375, -0.25]]),
    ("minecraft:furnace_minecart", &[[0.0, 0.1875, 0.0]]),
    ("minecraft:ghast", &[[0.0, 4.0625, 0.0]]),
    ("minecraft:goat", &[[0.0, 1.1125, 0.0]]),
    ("minecraft:guardian", &[[0.0, 0.975, 0.0]]),
    ("minecraft:happy_ghast", &[[0.0, 4.0, 1.7], [-1.7, 4.0, 0.0], [0.0, 4.0, -1.7], [1.7, 4.0, 0.0]]),
    ("minecraft:hoglin", &[[0.0, 1.49375, 0.0]]),
    ("minecraft:hopper_minecart", &[[0.0, 0.1875, 0.0]]),
    ("minecraft:horse", &[[0.0, 1.44375, 0.0]]),
    ("minecraft:husk", &[[0.0, 2.075, 0.0]]),
    ("minecraft:illusioner", &[[0.0, 2.0, 0.0]]),
    ("minecraft:llama", &[[0.0, 1.37, -0.3]]),
    ("minecraft:minecart", &[[0.0, 0.1875, 0.0]]),
    ("minecraft:mooshroom", &[[0.0, 1.36875, 0.0]]),
    ("minecraft:mule", &[[0.0, 1.2125, 0.0]]),
    ("minecraft:nautilus", &[[0.0, 1.1375, 0.0]]),
    ("minecraft:ocelot", &[[0.0, 0.6375, 0.0]]),
    ("minecraft:parrot", &[[0.0, 0.4625, 0.0]]),
    ("minecraft:phantom", &[[0.0, 0.3375, 0.0]]),
    ("minecraft:pig", &[[0.0, 0.86875, 0.0]]),
    ("minecraft:piglin", &[[0.0, 2.0125, 0.0]]),
    ("minecraft:piglin_brute", &[[0.0, 2.0125, 0.0]]),
    ("minecraft:pillager", &[[0.0, 2.0, 0.0]]),
    ("minecraft:ravager", &[[0.0, 2.2625, -0.0625]]),
    ("minecraft:sheep", &[[0.0, 1.2375, 0.0]]),
    ("minecraft:silverfish", &[[0.0, 0.2375, 0.0]]),
    ("minecraft:skeleton_horse", &[[0.0, 1.31875, 0.0]]),
    ("minecraft:sniffer", &[[0.0, 2.09375, 0.0]]),
    ("minecraft:spawner_minecart", &[[0.0, 0.1875, 0.0]]),
    ("minecraft:spider", &[[0.0, 0.765, 0.0]]),
    ("minecraft:tnt_minecart", &[[0.0, 0.1875, 0.0]]),
    ("minecraft:trader_llama", &[[0.0, 1.37, -0.3]]),
    ("minecraft:turtle", &[[0.0, 0.55625, -0.25]]),
    ("minecraft:vex", &[[0.0, 0.7375, 0.0]]),
    ("minecraft:vindicator", &[[0.0, 2.0, 0.0]]),
    ("minecraft:warden", &[[0.0, 3.15, 0.0]]),
    ("minecraft:witch", &[[0.0, 2.2625, 0.0]]),
    ("minecraft:wolf", &[[0.0, 0.81875, -0.0625]]),
    ("minecraft:zoglin", &[[0.0, 1.49375, 0.0]]),
    ("minecraft:zombie", &[[0.0, 2.0125, 0.0]]),
    ("minecraft:zombie_horse", &[[0.0, 1.31875, 0.0]]),
    ("minecraft:zombie_nautilus", &[[0.0, 1.1375, 0.0]]),
    ("minecraft:zombie_villager", &[[0.0, 2.125, 0.0]]),
    ("minecraft:zombified_piglin", &[[0.0, 2.0, 0.0]]),
];

/// `(registry id, point)` from `EntityTypes`' `ridingOffset` (negated) and
/// `vehicleAttachment`, sorted by id; other types are held at their feet.
#[rustfmt::skip]
const VEHICLE_ATTACHMENTS: &[(&str, [f32; 3])] = &[
    ("minecraft:allay", [0.0, -0.04, 0.0]),
    ("minecraft:bogged", [0.0, 0.7, 0.0]),
    ("minecraft:drowned", [0.0, 0.7, 0.0]),
    ("minecraft:evoker", [0.0, 0.6, 0.0]),
    ("minecraft:ghast", [0.0, -0.5, 0.0]),
    ("minecraft:giant", [0.0, 3.75, 0.0]),
    ("minecraft:happy_ghast", [0.0, -0.5, 0.0]),
    ("minecraft:husk", [0.0, 0.7, 0.0]),
    ("minecraft:illusioner", [0.0, 0.6, 0.0]),
    ("minecraft:mannequin", [0.0, 0.6, 0.0]),
    ("minecraft:parched", [0.0, 0.7, 0.0]),
    ("minecraft:phantom", [0.0, 0.125, 0.0]),
    ("minecraft:piglin", [0.0, 0.7, 0.0]),
    ("minecraft:piglin_brute", [0.0, 0.7, 0.0]),
    ("minecraft:pillager", [0.0, 0.6, 0.0]),
    ("minecraft:player", [0.0, 0.6, 0.0]),
    ("minecraft:skeleton", [0.0, 0.7, 0.0]),
    ("minecraft:stray", [0.0, 0.7, 0.0]),
    ("minecraft:vex", [0.0, -0.04, 0.0]),
    ("minecraft:vindicator", [0.0, 0.6, 0.0]),
    ("minecraft:wither_skeleton", [0.0, 0.875, 0.0]),
    ("minecraft:zombie", [0.0, 0.7, 0.0]),
    ("minecraft:zombie_villager", [0.0, 0.7, 0.0]),
    ("minecraft:zombified_piglin", [0.0, 0.7, 0.0]),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_sorted_for_lookup() {
        assert!(SIZES.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(EYE_HEIGHTS.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn eye_heights_take_the_override_or_the_default() {
        assert_eq!(EntityDimensions::eye_height(EntityKind::Spider), 0.65);
        assert_eq!(EntityDimensions::eye_height(EntityKind::Enderman), 2.55);
        assert_eq!(
            EntityDimensions::eye_height(EntityKind::Creeper),
            1.7 * 0.85
        );
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
