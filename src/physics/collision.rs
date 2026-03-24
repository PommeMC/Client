use std::collections::HashSet;
use std::sync::LazyLock;

use azalea_registry::builtin::BlockKind;
use glam::Vec3;

use super::aabb::Aabb;
use crate::world::chunk::ChunkStore;

static NO_COLLISION: LazyLock<HashSet<BlockKind>> = LazyLock::new(|| {
    HashSet::from([
        BlockKind::AcaciaHangingSign,
        BlockKind::AcaciaPressurePlate,
        BlockKind::AcaciaSapling,
        BlockKind::AcaciaSign,
        BlockKind::AcaciaWallHangingSign,
        BlockKind::AcaciaWallSign,
        BlockKind::ActivatorRail,
        BlockKind::Air,
        BlockKind::Allium,
        BlockKind::AzureBluet,
        BlockKind::BambooHangingSign,
        BlockKind::BambooPressurePlate,
        BlockKind::BambooSapling,
        BlockKind::BambooSign,
        BlockKind::BambooWallHangingSign,
        BlockKind::BambooWallSign,
        BlockKind::Beetroots,
        BlockKind::BigDripleafStem,
        BlockKind::BirchHangingSign,
        BlockKind::BirchPressurePlate,
        BlockKind::BirchSapling,
        BlockKind::BirchSign,
        BlockKind::BirchWallHangingSign,
        BlockKind::BirchWallSign,
        BlockKind::BlackBanner,
        BlockKind::BlackWallBanner,
        BlockKind::BlueBanner,
        BlockKind::BlueOrchid,
        BlockKind::BlueWallBanner,
        BlockKind::BrainCoral,
        BlockKind::BrainCoralFan,
        BlockKind::BrainCoralWallFan,
        BlockKind::BrownBanner,
        BlockKind::BrownMushroom,
        BlockKind::BrownWallBanner,
        BlockKind::BubbleColumn,
        BlockKind::BubbleCoral,
        BlockKind::BubbleCoralFan,
        BlockKind::BubbleCoralWallFan,
        BlockKind::Bush,
        BlockKind::CactusFlower,
        BlockKind::Carrots,
        BlockKind::CaveAir,
        BlockKind::CaveVines,
        BlockKind::CaveVinesPlant,
        BlockKind::CherryHangingSign,
        BlockKind::CherryPressurePlate,
        BlockKind::CherrySapling,
        BlockKind::CherrySign,
        BlockKind::CherryWallHangingSign,
        BlockKind::CherryWallSign,
        BlockKind::ClosedEyeblossom,
        BlockKind::Cobweb,
        BlockKind::CopperTorch,
        BlockKind::CopperWallTorch,
        BlockKind::Cornflower,
        BlockKind::CrimsonFungus,
        BlockKind::CrimsonHangingSign,
        BlockKind::CrimsonPressurePlate,
        BlockKind::CrimsonRoots,
        BlockKind::CrimsonSign,
        BlockKind::CrimsonWallHangingSign,
        BlockKind::CrimsonWallSign,
        BlockKind::CyanBanner,
        BlockKind::CyanWallBanner,
        BlockKind::Dandelion,
        BlockKind::DarkOakHangingSign,
        BlockKind::DarkOakPressurePlate,
        BlockKind::DarkOakSapling,
        BlockKind::DarkOakSign,
        BlockKind::DarkOakWallHangingSign,
        BlockKind::DarkOakWallSign,
        BlockKind::DeadBrainCoral,
        BlockKind::DeadBrainCoralFan,
        BlockKind::DeadBrainCoralWallFan,
        BlockKind::DeadBubbleCoral,
        BlockKind::DeadBubbleCoralFan,
        BlockKind::DeadBubbleCoralWallFan,
        BlockKind::DeadBush,
        BlockKind::DeadFireCoral,
        BlockKind::DeadFireCoralFan,
        BlockKind::DeadFireCoralWallFan,
        BlockKind::DeadHornCoral,
        BlockKind::DeadHornCoralFan,
        BlockKind::DeadHornCoralWallFan,
        BlockKind::DeadTubeCoral,
        BlockKind::DeadTubeCoralFan,
        BlockKind::DeadTubeCoralWallFan,
        BlockKind::DetectorRail,
        BlockKind::EndGateway,
        BlockKind::EndPortal,
        BlockKind::Fern,
        BlockKind::Fire,
        BlockKind::FireCoral,
        BlockKind::FireCoralFan,
        BlockKind::FireCoralWallFan,
        BlockKind::FireflyBush,
        BlockKind::Frogspawn,
        BlockKind::GlowLichen,
        BlockKind::GrayBanner,
        BlockKind::GrayWallBanner,
        BlockKind::GreenBanner,
        BlockKind::GreenWallBanner,
        BlockKind::HangingRoots,
        BlockKind::HeavyWeightedPressurePlate,
        BlockKind::HornCoral,
        BlockKind::HornCoralFan,
        BlockKind::HornCoralWallFan,
        BlockKind::JungleHangingSign,
        BlockKind::JunglePressurePlate,
        BlockKind::JungleSapling,
        BlockKind::JungleSign,
        BlockKind::JungleWallHangingSign,
        BlockKind::JungleWallSign,
        BlockKind::Kelp,
        BlockKind::KelpPlant,
        BlockKind::LargeFern,
        BlockKind::Lava,
        BlockKind::LeafLitter,
        BlockKind::Lever,
        BlockKind::LightBlueBanner,
        BlockKind::LightBlueWallBanner,
        BlockKind::LightGrayBanner,
        BlockKind::LightGrayWallBanner,
        BlockKind::LightWeightedPressurePlate,
        BlockKind::Lilac,
        BlockKind::LilyOfTheValley,
        BlockKind::LimeBanner,
        BlockKind::LimeWallBanner,
        BlockKind::MagentaBanner,
        BlockKind::MagentaWallBanner,
        BlockKind::MangroveHangingSign,
        BlockKind::MangrovePressurePlate,
        BlockKind::MangrovePropagule,
        BlockKind::MangroveSign,
        BlockKind::MangroveWallHangingSign,
        BlockKind::MangroveWallSign,
        BlockKind::NetherPortal,
        BlockKind::NetherSprouts,
        BlockKind::NetherWart,
        BlockKind::OakHangingSign,
        BlockKind::OakPressurePlate,
        BlockKind::OakSapling,
        BlockKind::OakSign,
        BlockKind::OakWallHangingSign,
        BlockKind::OakWallSign,
        BlockKind::OpenEyeblossom,
        BlockKind::OrangeBanner,
        BlockKind::OrangeTulip,
        BlockKind::OrangeWallBanner,
        BlockKind::OxeyeDaisy,
        BlockKind::PaleHangingMoss,
        BlockKind::PaleOakHangingSign,
        BlockKind::PaleOakPressurePlate,
        BlockKind::PaleOakSapling,
        BlockKind::PaleOakSign,
        BlockKind::PaleOakWallHangingSign,
        BlockKind::PaleOakWallSign,
        BlockKind::Peony,
        BlockKind::PinkBanner,
        BlockKind::PinkPetals,
        BlockKind::PinkTulip,
        BlockKind::PinkWallBanner,
        BlockKind::PitcherCrop,
        BlockKind::PitcherPlant,
        BlockKind::PolishedBlackstonePressurePlate,
        BlockKind::Poppy,
        BlockKind::Potatoes,
        BlockKind::PoweredRail,
        BlockKind::PurpleBanner,
        BlockKind::PurpleWallBanner,
        BlockKind::Rail,
        BlockKind::RedBanner,
        BlockKind::RedMushroom,
        BlockKind::RedTulip,
        BlockKind::RedWallBanner,
        BlockKind::RedstoneTorch,
        BlockKind::RedstoneWallTorch,
        BlockKind::RedstoneWire,
        BlockKind::ResinClump,
        BlockKind::RoseBush,
        BlockKind::Scaffolding,
        BlockKind::SculkVein,
        BlockKind::Seagrass,
        BlockKind::ShortDryGrass,
        BlockKind::ShortGrass,
        BlockKind::SmallDripleaf,
        BlockKind::SoulFire,
        BlockKind::SoulTorch,
        BlockKind::SoulWallTorch,
        BlockKind::SporeBlossom,
        BlockKind::SpruceHangingSign,
        BlockKind::SprucePressurePlate,
        BlockKind::SpruceSapling,
        BlockKind::SpruceSign,
        BlockKind::SpruceWallHangingSign,
        BlockKind::SpruceWallSign,
        BlockKind::StonePressurePlate,
        BlockKind::StructureVoid,
        BlockKind::SugarCane,
        BlockKind::Sunflower,
        BlockKind::SweetBerryBush,
        BlockKind::TallDryGrass,
        BlockKind::TallGrass,
        BlockKind::TallSeagrass,
        BlockKind::Torch,
        BlockKind::Torchflower,
        BlockKind::TorchflowerCrop,
        BlockKind::Tripwire,
        BlockKind::TripwireHook,
        BlockKind::TubeCoral,
        BlockKind::TubeCoralFan,
        BlockKind::TubeCoralWallFan,
        BlockKind::TwistingVines,
        BlockKind::TwistingVinesPlant,
        BlockKind::Vine,
        BlockKind::VoidAir,
        BlockKind::WallTorch,
        BlockKind::WarpedFungus,
        BlockKind::WarpedHangingSign,
        BlockKind::WarpedPressurePlate,
        BlockKind::WarpedRoots,
        BlockKind::WarpedSign,
        BlockKind::WarpedWallHangingSign,
        BlockKind::WarpedWallSign,
        BlockKind::Water,
        BlockKind::WeepingVines,
        BlockKind::WeepingVinesPlant,
        BlockKind::Wheat,
        BlockKind::WhiteBanner,
        BlockKind::WhiteTulip,
        BlockKind::WhiteWallBanner,
        BlockKind::Wildflowers,
        BlockKind::WitherRose,
        BlockKind::YellowBanner,
        BlockKind::YellowWallBanner,
    ])
});

fn has_collision(state: azalea_block::BlockState) -> bool {
    if state.is_air() {
        return false;
    }
    let kind: BlockKind = state.into();
    !NO_COLLISION.contains(&kind)
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
                if has_collision(state) {
                    aabbs.push(Aabb::new(
                        Vec3::new(bx as f32, by as f32, bz as f32),
                        Vec3::new((bx + 1) as f32, (by + 1) as f32, (bz + 1) as f32),
                    ));
                }
            }
        }
    }

    aabbs
}
fn collide_along_axes(block_aabbs: &[Aabb], player_aabb: Aabb, mut velocity: Vec3) -> (Vec3, bool) {
    let original_y = velocity.y;

    for block in block_aabbs {
        velocity.y = block.clip_y_collide(&player_aabb, velocity.y);
    }
    let mut resolved = player_aabb.offset(Vec3::new(0.0, velocity.y, 0.0));

    let x_first = velocity.x.abs() >= velocity.z.abs();

    if x_first {
        for block in block_aabbs {
            velocity.x = block.clip_x_collide(&resolved, velocity.x);
        }
        resolved = resolved.offset(Vec3::new(velocity.x, 0.0, 0.0));

        for block in block_aabbs {
            velocity.z = block.clip_z_collide(&resolved, velocity.z);
        }
    } else {
        for block in block_aabbs {
            velocity.z = block.clip_z_collide(&resolved, velocity.z);
        }
        resolved = resolved.offset(Vec3::new(0.0, 0.0, velocity.z));

        for block in block_aabbs {
            velocity.x = block.clip_x_collide(&resolved, velocity.x);
        }
    }

    let on_ground = original_y < 0.0 && velocity.y != original_y;

    (velocity, on_ground)
}

pub fn resolve_collision(
    chunk_store: &ChunkStore,
    player_aabb: Aabb,
    velocity: Vec3,
    step_height: f32,
) -> (Vec3, bool) {
    let expanded = player_aabb.expand(velocity);
    let block_aabbs = collect_block_aabbs(chunk_store, &expanded);

    let (resolved, on_ground) = collide_along_axes(&block_aabbs, player_aabb, velocity);

    let horizontal_blocked = resolved.x != velocity.x || resolved.z != velocity.z;
    if step_height > 0.0 && on_ground && horizontal_blocked {
        let step_up = Vec3::new(velocity.x, step_height, velocity.z);
        let step_expanded = player_aabb
            .expand(step_up)
            .expand(Vec3::new(0.0, -step_height, 0.0));
        let step_aabbs = collect_block_aabbs(chunk_store, &step_expanded);

        let mut up_vel = step_height;
        for block in &step_aabbs {
            up_vel = block.clip_y_collide(&player_aabb, up_vel);
        }
        let raised = player_aabb.offset(Vec3::new(0.0, up_vel, 0.0));

        let (step_resolved, _) =
            collide_along_axes(&step_aabbs, raised, Vec3::new(velocity.x, 0.0, velocity.z));

        let after_move = raised.offset(Vec3::new(step_resolved.x, 0.0, step_resolved.z));
        let mut down_vel = -(up_vel - velocity.y);
        for block in &step_aabbs {
            down_vel = block.clip_y_collide(&after_move, down_vel);
        }

        let step_total = Vec3::new(step_resolved.x, up_vel + down_vel, step_resolved.z);

        let step_h_dist = step_total.x * step_total.x + step_total.z * step_total.z;
        let orig_h_dist = resolved.x * resolved.x + resolved.z * resolved.z;

        if step_h_dist > orig_h_dist {
            let step_on_ground = down_vel != -(up_vel - velocity.y);
            return (step_total, step_on_ground || on_ground);
        }
    }

    (resolved, on_ground)
}
