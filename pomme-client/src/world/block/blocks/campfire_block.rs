use azalea_block::BlockState;
use azalea_registry::builtin::BlockEntityKind;
use glam::DVec3;
use simdnbt::owned::NbtCompound;

use super::super::{Block, BlockData, PropMap};
use crate::particles::types::SimpleParticleType;
use crate::position::BlockPos;
use crate::world::block::model::Direction;
use crate::world::level::LevelContext;

pub(in crate::world::block) struct CampfireBlock {
    block_data: BlockData,
    spawn_particles: bool,
    lava: SimpleParticleType,
}

impl CampfireBlock {
    pub(in crate::world::block) fn new(
        block_data: BlockData,
        spawn_particles: bool,
        lava: SimpleParticleType,
    ) -> Self {
        Self {
            block_data,
            spawn_particles,
            lava,
        }
    }
}

impl Block for CampfireBlock {
    fn block_data(&self) -> &BlockData {
        &self.block_data
    }

    fn animate_tick(
        &self,
        state: BlockState,
        pos: BlockPos,
        level: &mut LevelContext<'_>,
        random: &mut fastrand::Rng,
    ) {
        if level
            .block_state_table()
            .block_state_data(state)
            .properties()
            .get("lit")
            != Some("true")
        {
            return;
        }
        if random.usize(..10) == 0 {
            // Audio is excluded; retain volume then pitch draws.
            let _ = 0.5_f32 + random.f32();
            let _ = random.f32() * 0.7_f32 + 0.6_f32;
        }
        if self.spawn_particles && random.usize(..5) == 0 {
            let mut i = 0;
            while i < random.usize(..1) + 1 {
                let position = DVec3::new(
                    f64::from(pos.x) + 0.5,
                    f64::from(pos.y) + 0.5,
                    f64::from(pos.z) + 0.5,
                );
                let auxiliary = DVec3::new(
                    f64::from(random.f32() / 2.0_f32),
                    5.0e-5,
                    f64::from(random.f32() / 2.0_f32),
                );
                level.add_particle(&self.lava, position, auxiliary);
                i += 1;
            }
        }
    }
}

pub(crate) fn tick_campfires(
    level: &mut LevelContext<'_>,
    cosy: &SimpleParticleType,
    signal: &SimpleParticleType,
    smoke: &SimpleParticleType,
) {
    let campfires: Vec<_> = level
        .chunks()
        .block_entities
        .iter()
        .filter(|(_, entity)| entity.kind == BlockEntityKind::Campfire)
        .map(|(&pos, entity)| (pos, campfire_occupied_slots(&entity.nbt)))
        .collect();
    for (pos, occupied) in campfires {
        let state = level.get_block_state(pos);
        let data = level.block_state_table().block_state_data(state);
        if matches!(data.id(), "campfire" | "soul_campfire")
            && data.properties().get("lit") == Some("true")
        {
            particle_tick(level, pos, data.properties(), occupied, cosy, signal, smoke);
        }
    }
}

// COMPAT: Vanilla CampfireBlockEntity owns cooking inventory and slot state.
// Pomme currently stores raw NBT generically, so occupancy decoding lives here
// with the campfire client particle implementation.
fn campfire_occupied_slots(nbt: &NbtCompound) -> [bool; 4] {
    let mut occupied = [false; 4];
    let Some(items) = nbt.list("Items").and_then(|list| list.compounds()) else {
        return occupied;
    };
    for item in items {
        let slot = if item.contains("Slot") {
            let Some(slot) = item.byte("Slot") else {
                continue;
            };
            usize::from(slot as u8)
        } else {
            0
        };
        if slot >= occupied.len() {
            continue;
        }
        let Some(id) = item.string("id") else {
            continue;
        };
        let id = id.to_str();
        if id.is_empty() || matches!(id.as_ref(), "air" | "minecraft:air") {
            continue;
        }
        let count = if item.contains("count") {
            item.int("count")
        } else if item.contains("Count") {
            item.byte("Count").map(i32::from)
        } else {
            Some(1)
        };
        let Some(_) = count.filter(|&count| count > 0) else {
            continue;
        };
        occupied[slot] = true;
    }
    occupied
}

fn particle_tick(
    level: &mut LevelContext<'_>,
    pos: BlockPos,
    properties: &PropMap,
    occupied: [bool; 4],
    cosy: &SimpleParticleType,
    signal: &SimpleParticleType,
    smoke: &SimpleParticleType,
) {
    let random = level.get_random().clone();
    if random.next_float() < 0.11_f32 {
        let options = if properties.get("signal_fire") == Some("true") {
            signal
        } else {
            cosy
        };
        let mut i = 0;
        while i < random.next_int(2) + 2 {
            make_particles(level, pos, options);
            i += 1;
        }
    }
    let rotation = match properties.get("facing") {
        Some("south") => 0,
        Some("west") => 1,
        Some("north") => 2,
        Some("east") => 3,
        _ => return,
    };
    let directions = [
        Direction::South,
        Direction::West,
        Direction::North,
        Direction::East,
    ];
    for (slot, occupied) in occupied.into_iter().enumerate() {
        if occupied && random.next_float() < 0.2_f32 {
            let direction = directions[(slot + rotation) % 4].offset();
            let clockwise = directions[(slot + rotation + 1) % 4].offset();
            let position = DVec3::new(
                f64::from(pos.x) + 0.5 - f64::from(direction[0] as f32 * 0.3125_f32)
                    + f64::from(clockwise[0] as f32 * 0.3125_f32),
                f64::from(pos.y) + 0.5,
                f64::from(pos.z) + 0.5 - f64::from(direction[2] as f32 * 0.3125_f32)
                    + f64::from(clockwise[2] as f32 * 0.3125_f32),
            );
            for _ in 0..4 {
                level.add_particle(smoke, position, DVec3::new(0.0, 0.0005, 0.0));
            }
        }
    }
}

fn make_particles(level: &mut LevelContext<'_>, pos: BlockPos, options: &SimpleParticleType) {
    let random = level.get_random().clone();
    let x = f64::from(pos.x)
        + 0.5
        + random.next_double() / 3.0 * if random.next_boolean() { 1.0 } else { -1.0 };
    let y = f64::from(pos.y) + random.next_double() + random.next_double();
    let z = f64::from(pos.z)
        + 0.5
        + random.next_double() / 3.0 * if random.next_boolean() { 1.0 } else { -1.0 };
    level.add_always_visible_particle_with_override(
        options,
        true,
        DVec3::new(x, y, z),
        DVec3::new(0.0, 0.07, 0.0),
    );
}
