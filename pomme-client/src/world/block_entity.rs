use azalea_registry::builtin::BlockEntityKind;
use simdnbt::owned::NbtCompound;

#[derive(Clone)]
pub struct StoredBlockEntity {
    #[allow(dead_code)]
    pub kind: BlockEntityKind,
    #[allow(dead_code)]
    pub nbt: NbtCompound,
}

/// Blocks vanilla draws via a `BlockEntityRenderer` instead of a baked model.
/// The chunk mesher skips these positions; the BE renderer fills them in.
pub fn is_block_entity_block(name: &str) -> bool {
    matches!(
        name,
        // Chests / containers
        "chest" | "trapped_chest" | "ender_chest"
        | "shulker_box"
        | "white_shulker_box" | "orange_shulker_box" | "magenta_shulker_box" | "light_blue_shulker_box"
        | "yellow_shulker_box" | "lime_shulker_box" | "pink_shulker_box" | "gray_shulker_box"
        | "light_gray_shulker_box" | "cyan_shulker_box" | "purple_shulker_box" | "blue_shulker_box"
        | "brown_shulker_box" | "green_shulker_box" | "red_shulker_box" | "black_shulker_box"
        // Signs
        | "oak_sign" | "spruce_sign" | "birch_sign" | "jungle_sign" | "acacia_sign" | "dark_oak_sign"
        | "mangrove_sign" | "cherry_sign" | "pale_oak_sign" | "bamboo_sign"
        | "crimson_sign" | "warped_sign"
        | "oak_wall_sign" | "spruce_wall_sign" | "birch_wall_sign" | "jungle_wall_sign"
        | "acacia_wall_sign" | "dark_oak_wall_sign" | "mangrove_wall_sign" | "cherry_wall_sign"
        | "pale_oak_wall_sign" | "bamboo_wall_sign"
        | "crimson_wall_sign" | "warped_wall_sign"
        | "oak_hanging_sign" | "spruce_hanging_sign" | "birch_hanging_sign" | "jungle_hanging_sign"
        | "acacia_hanging_sign" | "dark_oak_hanging_sign" | "mangrove_hanging_sign"
        | "cherry_hanging_sign" | "pale_oak_hanging_sign" | "bamboo_hanging_sign"
        | "crimson_hanging_sign" | "warped_hanging_sign"
        | "oak_wall_hanging_sign" | "spruce_wall_hanging_sign" | "birch_wall_hanging_sign"
        | "jungle_wall_hanging_sign" | "acacia_wall_hanging_sign" | "dark_oak_wall_hanging_sign"
        | "mangrove_wall_hanging_sign" | "cherry_wall_hanging_sign" | "pale_oak_wall_hanging_sign"
        | "bamboo_wall_hanging_sign" | "crimson_wall_hanging_sign" | "warped_wall_hanging_sign"
        // Banners
        | "white_banner" | "orange_banner" | "magenta_banner" | "light_blue_banner"
        | "yellow_banner" | "lime_banner" | "pink_banner" | "gray_banner"
        | "light_gray_banner" | "cyan_banner" | "purple_banner" | "blue_banner"
        | "brown_banner" | "green_banner" | "red_banner" | "black_banner"
        | "white_wall_banner" | "orange_wall_banner" | "magenta_wall_banner" | "light_blue_wall_banner"
        | "yellow_wall_banner" | "lime_wall_banner" | "pink_wall_banner" | "gray_wall_banner"
        | "light_gray_wall_banner" | "cyan_wall_banner" | "purple_wall_banner" | "blue_wall_banner"
        | "brown_wall_banner" | "green_wall_banner" | "red_wall_banner" | "black_wall_banner"
        // Beds
        | "white_bed" | "orange_bed" | "magenta_bed" | "light_blue_bed"
        | "yellow_bed" | "lime_bed" | "pink_bed" | "gray_bed"
        | "light_gray_bed" | "cyan_bed" | "purple_bed" | "blue_bed"
        | "brown_bed" | "green_bed" | "red_bed" | "black_bed"
        // Skulls / heads
        | "skeleton_skull" | "skeleton_wall_skull"
        | "wither_skeleton_skull" | "wither_skeleton_wall_skull"
        | "zombie_head" | "zombie_wall_head"
        | "player_head" | "player_wall_head"
        | "creeper_head" | "creeper_wall_head"
        | "dragon_head" | "dragon_wall_head"
        | "piglin_head" | "piglin_wall_head"
        // Misc block entities
        | "conduit" | "decorated_pot" | "end_portal" | "end_gateway"
        | "beacon" | "spawner" | "trial_spawner" | "vault"
        | "brewing_stand" | "lectern" | "campfire" | "soul_campfire"
        | "beehive" | "bee_nest" | "bell" | "suspicious_sand" | "suspicious_gravel"
        | "crafter"
    )
}

pub fn is_invisible_block(name: &str) -> bool {
    matches!(
        name,
        "air"
            | "cave_air"
            | "void_air"
            | "barrier"
            | "light"
            | "structure_void"
            | "moving_piston"
            | "heavy_core"
    )
}

pub fn is_fluid_block(name: &str) -> bool {
    matches!(name, "water" | "lava" | "bubble_column")
}
