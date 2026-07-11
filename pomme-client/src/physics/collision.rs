use std::collections::HashSet;
use std::sync::LazyLock;

use glam::{DVec3, dvec3};

use super::aabb::Aabb;
use super::block_shape;
use crate::entity::components::Velocity;
use crate::world::block::{block_id, is_air};
use crate::world::chunk::ChunkStore;

static NO_COLLISION: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        "acacia_hanging_sign",
        "acacia_pressure_plate",
        "acacia_sapling",
        "acacia_sign",
        "acacia_wall_hanging_sign",
        "acacia_wall_sign",
        "activator_rail",
        "air",
        "allium",
        "azure_bluet",
        "bamboo_hanging_sign",
        "bamboo_pressure_plate",
        "bamboo_sapling",
        "bamboo_sign",
        "bamboo_wall_hanging_sign",
        "bamboo_wall_sign",
        "beetroots",
        "big_dripleaf_stem",
        "birch_hanging_sign",
        "birch_pressure_plate",
        "birch_sapling",
        "birch_sign",
        "birch_wall_hanging_sign",
        "birch_wall_sign",
        "black_banner",
        "black_wall_banner",
        "blue_banner",
        "blue_orchid",
        "blue_wall_banner",
        "brain_coral",
        "brain_coral_fan",
        "brain_coral_wall_fan",
        "brown_banner",
        "brown_mushroom",
        "brown_wall_banner",
        "bubble_column",
        "bubble_coral",
        "bubble_coral_fan",
        "bubble_coral_wall_fan",
        "bush",
        "cactus_flower",
        "carrots",
        "cave_air",
        "cave_vines",
        "cave_vines_plant",
        "cherry_hanging_sign",
        "cherry_pressure_plate",
        "cherry_sapling",
        "cherry_sign",
        "cherry_wall_hanging_sign",
        "cherry_wall_sign",
        "closed_eyeblossom",
        "cobweb",
        "copper_torch",
        "copper_wall_torch",
        "cornflower",
        "crimson_fungus",
        "crimson_hanging_sign",
        "crimson_pressure_plate",
        "crimson_roots",
        "crimson_sign",
        "crimson_wall_hanging_sign",
        "crimson_wall_sign",
        "cyan_banner",
        "cyan_wall_banner",
        "dandelion",
        "dark_oak_hanging_sign",
        "dark_oak_pressure_plate",
        "dark_oak_sapling",
        "dark_oak_sign",
        "dark_oak_wall_hanging_sign",
        "dark_oak_wall_sign",
        "dead_brain_coral",
        "dead_brain_coral_fan",
        "dead_brain_coral_wall_fan",
        "dead_bubble_coral",
        "dead_bubble_coral_fan",
        "dead_bubble_coral_wall_fan",
        "dead_bush",
        "dead_fire_coral",
        "dead_fire_coral_fan",
        "dead_fire_coral_wall_fan",
        "dead_horn_coral",
        "dead_horn_coral_fan",
        "dead_horn_coral_wall_fan",
        "dead_tube_coral",
        "dead_tube_coral_fan",
        "dead_tube_coral_wall_fan",
        "detector_rail",
        "end_gateway",
        "end_portal",
        "fern",
        "fire",
        "fire_coral",
        "fire_coral_fan",
        "fire_coral_wall_fan",
        "firefly_bush",
        "frogspawn",
        "glow_lichen",
        "gray_banner",
        "gray_wall_banner",
        "green_banner",
        "green_wall_banner",
        "hanging_roots",
        "heavy_weighted_pressure_plate",
        "horn_coral",
        "horn_coral_fan",
        "horn_coral_wall_fan",
        "jungle_hanging_sign",
        "jungle_pressure_plate",
        "jungle_sapling",
        "jungle_sign",
        "jungle_wall_hanging_sign",
        "jungle_wall_sign",
        "kelp",
        "kelp_plant",
        "large_fern",
        "lava",
        "leaf_litter",
        "lever",
        "light_blue_banner",
        "light_blue_wall_banner",
        "light_gray_banner",
        "light_gray_wall_banner",
        "light_weighted_pressure_plate",
        "lilac",
        "lily_of_the_valley",
        "lime_banner",
        "lime_wall_banner",
        "magenta_banner",
        "magenta_wall_banner",
        "mangrove_hanging_sign",
        "mangrove_pressure_plate",
        "mangrove_propagule",
        "mangrove_sign",
        "mangrove_wall_hanging_sign",
        "mangrove_wall_sign",
        "nether_portal",
        "nether_sprouts",
        "nether_wart",
        "oak_hanging_sign",
        "oak_pressure_plate",
        "oak_sapling",
        "oak_sign",
        "oak_wall_hanging_sign",
        "oak_wall_sign",
        "open_eyeblossom",
        "orange_banner",
        "orange_tulip",
        "orange_wall_banner",
        "oxeye_daisy",
        "pale_hanging_moss",
        "pale_oak_hanging_sign",
        "pale_oak_pressure_plate",
        "pale_oak_sapling",
        "pale_oak_sign",
        "pale_oak_wall_hanging_sign",
        "pale_oak_wall_sign",
        "peony",
        "pink_banner",
        "pink_petals",
        "pink_tulip",
        "pink_wall_banner",
        "pitcher_crop",
        "pitcher_plant",
        "polished_blackstone_pressure_plate",
        "poppy",
        "potatoes",
        "powered_rail",
        "purple_banner",
        "purple_wall_banner",
        "rail",
        "red_banner",
        "red_mushroom",
        "red_tulip",
        "red_wall_banner",
        "redstone_torch",
        "redstone_wall_torch",
        "redstone_wire",
        "resin_clump",
        "rose_bush",
        "scaffolding",
        "sculk_vein",
        "seagrass",
        "short_dry_grass",
        "short_grass",
        "small_dripleaf",
        "soul_fire",
        "soul_torch",
        "soul_wall_torch",
        "spore_blossom",
        "spruce_hanging_sign",
        "spruce_pressure_plate",
        "spruce_sapling",
        "spruce_sign",
        "spruce_wall_hanging_sign",
        "spruce_wall_sign",
        "stone_pressure_plate",
        "structure_void",
        "sugar_cane",
        "sunflower",
        "sweet_berry_bush",
        "tall_dry_grass",
        "tall_grass",
        "tall_seagrass",
        "torch",
        "torchflower",
        "torchflower_crop",
        "tripwire",
        "tripwire_hook",
        "tube_coral",
        "tube_coral_fan",
        "tube_coral_wall_fan",
        "twisting_vines",
        "twisting_vines_plant",
        "vine",
        "void_air",
        "wall_torch",
        "warped_fungus",
        "warped_hanging_sign",
        "warped_pressure_plate",
        "warped_roots",
        "warped_sign",
        "warped_wall_hanging_sign",
        "warped_wall_sign",
        "water",
        "weeping_vines",
        "weeping_vines_plant",
        "wheat",
        "white_banner",
        "white_tulip",
        "white_wall_banner",
        "wildflowers",
        "wither_rose",
        "yellow_banner",
        "yellow_wall_banner",
    ])
});

pub fn has_collision(state: azalea_block::BlockState) -> bool {
    if is_air(state) {
        return false;
    }
    !NO_COLLISION.contains(block_id(state))
}

pub fn collect_block_aabbs(chunk_store: &ChunkStore, region: &Aabb) -> Vec<Aabb> {
    let mut aabbs = Vec::new();

    let min_x = region.min.x.floor() as i32;
    let min_y = region.min.y.floor() as i32;
    let min_z = region.min.z.floor() as i32;
    let max_x = region.max.x.ceil() as i32;
    let max_y = region.max.y.ceil() as i32;
    let max_z = region.max.z.ceil() as i32;

    for by in min_y..max_y {
        for bz in min_z..max_z {
            for bx in min_x..max_x {
                let state = chunk_store.get_block_state(bx, by, bz);
                if !has_collision(state) {
                    continue;
                }
                match block_shape::partial_shape(state) {
                    Some(boxes) => {
                        for &[lx0, ly0, lz0, lx1, ly1, lz1] in boxes {
                            aabbs.push(Aabb::new(
                                dvec3(bx as f64 + lx0, by as f64 + ly0, bz as f64 + lz0),
                                dvec3(bx as f64 + lx1, by as f64 + ly1, bz as f64 + lz1),
                            ));
                        }
                    }
                    None => aabbs.push(Aabb::block(bx, by, bz)),
                }
            }
        }
    }

    aabbs
}

pub fn no_collision(chunk_store: &ChunkStore, aabb: &Aabb) -> bool {
    collect_block_aabbs(chunk_store, aabb)
        .iter()
        .all(|block| !block.intersects(aabb))
}

fn collide_along_axes(
    block_aabbs: &[Aabb],
    player_aabb: Aabb,
    mut velocity: Velocity,
) -> (DVec3, bool) {
    let original_y = velocity.y;

    for block in block_aabbs {
        velocity.y = block.clip_y_collide(&player_aabb, velocity.y);
    }
    let mut resolved = player_aabb.offset(dvec3(0.0, velocity.y, 0.0));

    let x_first = velocity.x.abs() >= velocity.z.abs();

    if x_first {
        for block in block_aabbs {
            velocity.x = block.clip_x_collide(&resolved, velocity.x);
        }
        resolved = resolved.offset(dvec3(velocity.x, 0.0, 0.0));

        for block in block_aabbs {
            velocity.z = block.clip_z_collide(&resolved, velocity.z);
        }
    } else {
        for block in block_aabbs {
            velocity.z = block.clip_z_collide(&resolved, velocity.z);
        }
        resolved = resolved.offset(dvec3(0.0, 0.0, velocity.z));

        for block in block_aabbs {
            velocity.x = block.clip_x_collide(&resolved, velocity.x);
        }
    }

    let on_ground = original_y < 0.0 && velocity.y != original_y;

    (*velocity, on_ground)
}

pub fn resolve_collision(
    chunk_store: &ChunkStore,
    player_aabb: Aabb,
    velocity: Velocity,
    step_height: f64,
) -> (DVec3, bool) {
    let expanded = player_aabb.expand(*velocity);
    let block_aabbs = collect_block_aabbs(chunk_store, &expanded);

    let (resolved, on_ground) = collide_along_axes(&block_aabbs, player_aabb, velocity);

    let horizontal_blocked = resolved.x != velocity.x || resolved.z != velocity.z;
    if step_height > 0.0 && on_ground && horizontal_blocked {
        let step_up = dvec3(velocity.x, step_height, velocity.z);
        let step_expanded = player_aabb
            .expand(step_up)
            .expand(dvec3(0.0, -step_height, 0.0));
        let step_aabbs = collect_block_aabbs(chunk_store, &step_expanded);

        let mut up_vel = step_height;
        for block in &step_aabbs {
            up_vel = block.clip_y_collide(&player_aabb, up_vel);
        }
        let raised = player_aabb.offset(dvec3(0.0, up_vel, 0.0));

        let (step_resolved, _) = collide_along_axes(
            &step_aabbs,
            raised,
            Velocity::new(velocity.x, 0.0, velocity.z),
        );

        let after_move = raised.offset(dvec3(step_resolved.x, 0.0, step_resolved.z));
        let mut down_vel = -(up_vel - velocity.y);
        for block in &step_aabbs {
            down_vel = block.clip_y_collide(&after_move, down_vel);
        }

        let step_total = dvec3(step_resolved.x, up_vel + down_vel, step_resolved.z);

        let step_h_dist = step_total.x * step_total.x + step_total.z * step_total.z;
        let orig_h_dist = resolved.x * resolved.x + resolved.z * resolved.z;

        if step_h_dist > orig_h_dist {
            let step_on_ground = down_vel != -(up_vel - velocity.y);
            return (step_total, step_on_ground || on_ground);
        }
    }

    (resolved, on_ground)
}
