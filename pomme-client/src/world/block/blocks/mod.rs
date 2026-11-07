mod campfire_block;
mod candle_block;
mod crying_obsidian_block;
mod end_rod_block;
mod fire_block;
mod furnace_block;
mod redstone_wire_block;
mod torch_block;

pub(super) use campfire_block::CampfireBlock;
pub(crate) use campfire_block::tick_campfires;
pub(super) use candle_block::{CandleBlock, CandleCakeBlock};
pub(super) use crying_obsidian_block::CryingObsidianBlock;
pub(super) use end_rod_block::EndRodBlock;
pub(super) use fire_block::{FireBlock, SoulFireBlock};
pub(super) use furnace_block::{BlastFurnaceBlock, FurnaceBlock, SmokerBlock};
pub(super) use redstone_wire_block::RedstoneWireBlock;
pub(super) use torch_block::{
    RedstoneTorchBlock, RedstoneWallTorchBlock, TorchBlock, WallTorchBlock,
};
