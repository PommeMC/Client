//! The sounds the local player's own movement makes: footsteps, the amethyst
//! chime and swimming (vanilla `Entity.applyMovementEmissionAndPlaySound` and
//! `Player.playStepSound`). Vanilla's client plays these itself
//! (`LocalPlayer.playSound`) and the server sends them only to other players.
//!
//! Pitch jitter uses `fastrand`, as Pomme's other sounds do, where vanilla
//! draws from the entity's random.

use glam::DVec3;

use super::collision::CollisionWorld;
use super::movement::{is_climbable_state, on_pos};
use crate::player::LocalPlayer;
use crate::position::BlockPos;
use crate::world::block::sound::block_sounds_for_id;
use crate::world::block::{BlockStateData, BlockStateTable, BlockTags};

/// A sound for the caller to play where the player stands, in the `players`
/// category.
pub struct PlayerSound {
    pub event: &'static str,
    pub volume: f32,
    pub pitch: f32,
}

/// The movement `Entity.move` applied this tick, with the velocity left once
/// the collided axes stopped.
#[derive(Clone, Copy)]
pub(super) struct Move {
    pub movement: DVec3,
    pub velocity: DVec3,
}

pub struct MovementSounds {
    /// Vanilla `Entity.moveDist`.
    move_dist: f32,
    /// Vanilla `Entity.nextStep`.
    next_step: f32,
    /// Vanilla `Entity.crystalSoundIntensity`.
    crystal_sound_intensity: f32,
    /// `tickCount - lastCrystalSoundPlayTick`.
    ticks_since_chime: u32,
    /// The player's own move this tick, emitted by [`tick`] once `travel`
    /// is done.
    pub(super) last_move: Option<Move>,
    /// This tick's sounds, for the caller to play.
    pub pending: Vec<PlayerSound>,
}

impl Default for MovementSounds {
    fn default() -> Self {
        Self {
            move_dist: 0.0,
            next_step: 1.0,
            crystal_sound_intensity: 0.0,
            ticks_since_chime: 0,
            last_move: None,
            pending: Vec::new(),
        }
    }
}

/// [`emit`]s the player's own move this tick; `Entity.move` skips a
/// passenger's.
pub(super) fn tick(
    player: &mut LocalPlayer,
    world: CollisionWorld<'_>,
    tags: &BlockTags,
    shift_down: bool,
    is_passenger: bool,
) {
    let sounds = &mut player.movement_sounds;
    sounds.ticks_since_chime = sounds.ticks_since_chime.saturating_add(1);
    if let Some(mv) = sounds.last_move.take()
        && !is_passenger
    {
        emit(player, world, tags, shift_down, mv);
    }
}

/// `Entity.move`'s `applyMovementEmissionAndPlaySound` for one move.
/// `Player.getMovementEmission` emits nothing while flying or sneaking on
/// the ground.
pub(super) fn emit(
    player: &mut LocalPlayer,
    world: CollisionWorld<'_>,
    tags: &BlockTags,
    shift_down: bool,
    mv: Move,
) {
    if player.flying || (player.on_ground && shift_down) {
        return;
    }

    let effect_pos = on_pos(player, world, tags, 0.2);
    let support_pos = on_pos(player, world, tags, 1.0e-5);
    let support = world.block_data(support_pos);
    let moved = if is_state_climbable(support, tags) {
        mv.movement.length()
    } else {
        (mv.movement.x * mv.movement.x + mv.movement.z * mv.movement.z).sqrt()
    };
    player.movement_sounds.move_dist += (moved * f64::from(0.6_f32)) as f32;

    let sounds = &player.movement_sounds;
    if sounds.move_dist <= sounds.next_step || support.is_air() {
        return;
    }
    let mut stepped = step_on(
        player,
        world,
        tags,
        effect_pos,
        world.block_data(effect_pos),
        true,
        mv,
    );
    if support_pos != effect_pos {
        stepped |= step_on(player, world, tags, support_pos, support, false, mv);
    }
    if stepped || player.in_water {
        let sounds = &mut player.movement_sounds;
        sounds.next_step = (sounds.move_dist as i32 + 1) as f32;
    }
    if !stepped && player.in_water {
        water_swim_sound(player, mv.velocity);
    }
}

/// `Entity.vibrationAndSoundEffectsFromBlock`: whether stepping on the block
/// counts, playing its step sound when `with_sound`.
fn step_on(
    player: &mut LocalPlayer,
    world: CollisionWorld<'_>,
    tags: &BlockTags,
    pos: BlockPos,
    data: &BlockStateData,
    with_sound: bool,
    mv: Move,
) -> bool {
    if data.is_air() {
        return false;
    }
    let grounded = player.on_ground
        || is_state_climbable(data, tags)
        || (player.crouching && mv.movement.y == 0.0);
    if !grounded || player.swimming {
        return false;
    }
    if with_sound {
        play_step_sound(player, world, tags, pos, data, mv);
        // `walkingStepSound`'s amethyst chime.
        if tags.contains("minecraft:crystal_sound_blocks", data.id())
            && player.movement_sounds.ticks_since_chime >= 20
        {
            amethyst_chime(&mut player.movement_sounds);
        }
    }
    true
}

/// `Player.playStepSound`: in water a swim stroke and a muffled step, else
/// the step sound of what's underfoot, or of the carpet, snow or plant on it.
fn play_step_sound(
    player: &mut LocalPlayer,
    world: CollisionWorld<'_>,
    tags: &BlockTags,
    pos: BlockPos,
    on: &BlockStateData,
    mv: Move,
) {
    let table = world.block_state_table;
    if player.in_water {
        water_swim_sound(player, mv.velocity);
        muffled_step(&mut player.movement_sounds, table, on);
        return;
    }
    let sounds = &mut player.movement_sounds;
    // `Entity.getPrimaryStepSoundBlockPos`.
    let above = world.block_data(BlockPos::new(pos.x, pos.y + 1, pos.z));
    if tags.contains("minecraft:combination_step_sound_blocks", above.id()) {
        step(sounds, table, above, 0.15, 1.0);
        muffled_step(sounds, table, on);
    } else if tags.contains("minecraft:inside_step_sound_blocks", above.id()) {
        step(sounds, table, above, 0.15, 1.0);
    } else {
        step(sounds, table, on, 0.15, 1.0);
    }
}

/// `Entity.playMuffledStepSound`.
fn muffled_step(sounds: &mut MovementSounds, table: BlockStateTable, state: &BlockStateData) {
    step(sounds, table, state, 0.05, 0.8);
}

/// `state`'s step sound, its `SoundType` volume and pitch scaled.
fn step(
    sounds: &mut MovementSounds,
    table: BlockStateTable,
    state: &BlockStateData,
    volume: f32,
    pitch: f32,
) {
    let sound_type = block_sounds_for_id(table, state.id());
    play(
        sounds,
        &sound_type.step_event,
        sound_type.volume * volume,
        sound_type.pitch * pitch,
    );
}

/// `Entity.waterSwimSound` for the player itself (no controlling passenger).
fn water_swim_sound(player: &mut LocalPlayer, velocity: DVec3) {
    let speed = (velocity.x * velocity.x * f64::from(0.2_f32)
        + velocity.y * velocity.y
        + velocity.z * velocity.z * f64::from(0.2_f32))
    .sqrt() as f32;
    let volume = (speed * 0.35).min(1.0);
    let pitch = 1.0 + (fastrand::f32() - fastrand::f32()) * 0.4;
    play(
        &mut player.movement_sounds,
        "entity.player.swim",
        volume,
        pitch,
    );
}

/// `Entity.playAmethystStepSound`.
fn amethyst_chime(sounds: &mut MovementSounds) {
    let decay = 0.997_f64.powf(f64::from(sounds.ticks_since_chime)) as f32;
    let intensity = (sounds.crystal_sound_intensity * decay + 0.07).min(1.0);
    sounds.crystal_sound_intensity = intensity;
    let pitch = 0.5 + intensity * fastrand::f32() * 1.2;
    let volume = 0.1 + intensity * 1.2;
    sounds.ticks_since_chime = 0;
    play(sounds, "block.amethyst_block.chime", volume, pitch);
}

/// Queues `event` unless it's a silent `SoundType` slot.
fn play(sounds: &mut MovementSounds, event: &'static str, volume: f32, pitch: f32) {
    if !event.is_empty() {
        sounds.pending.push(PlayerSound {
            event,
            volume,
            pitch,
        });
    }
}

/// `Entity.isStateClimbable`.
fn is_state_climbable(state: &BlockStateData, tags: &BlockTags) -> bool {
    is_climbable_state(state, tags) || state.id() == "powder_snow"
}

#[cfg(test)]
mod tests {
    use glam::dvec3;

    use super::*;
    use crate::world::block::block_tags_for_test;
    use crate::world::chunk::ChunkStore;

    fn tags() -> BlockTags {
        block_tags_for_test(
            BlockStateTable::native(),
            &[
                ("combination_step_sound_blocks", &["snow"]),
                ("crystal_sound_blocks", &["amethyst_block"]),
            ],
        )
    }

    /// `ground` at (8, 63, 8), and `on` above it when given.
    fn store(ground: &str, on: Option<(&str, &[(&str, &str)])>) -> ChunkStore {
        let table = BlockStateTable::native();
        let chunks = ChunkStore::with_origin_chunk();
        chunks.set_block_state(8, 63, 8, table.find_state(ground, &[]), table);
        if let Some((name, props)) = on {
            chunks.set_block_state(8, 64, 8, table.find_state(name, props), table);
        }
        chunks
    }

    /// A player standing on (8, 63, 8) that just moved `movement`.
    fn walker(movement: DVec3) -> LocalPlayer {
        let mut player = LocalPlayer::new();
        player.position = dvec3(8.5, 64.0, 8.5).into();
        player.on_ground = true;
        player.main_supporting_block_pos = Some(BlockPos::new(8, 63, 8));
        player.movement_sounds.last_move = Some(Move {
            movement,
            velocity: movement,
        });
        player
    }

    fn step_once(
        player: &mut LocalPlayer,
        chunks: &ChunkStore,
        shift_down: bool,
    ) -> Vec<(&'static str, f32, f32)> {
        let world = CollisionWorld::new(chunks, BlockStateTable::native());
        tick(player, world, &tags(), shift_down, false);
        std::mem::take(&mut player.movement_sounds.pending)
            .into_iter()
            .map(|s| (s.event, s.volume, s.pitch))
            .collect()
    }

    #[test]
    fn a_step_sounds_once_the_walked_distance_passes_the_next_step() {
        let chunks = store("stone", None);
        // 1.5 blocks walk 0.9 of `moveDist`: short of the first step at 1.
        let mut player = walker(dvec3(1.5, 0.0, 0.0));
        assert!(step_once(&mut player, &chunks, false).is_empty());
        player.movement_sounds.last_move = Some(Move {
            movement: dvec3(0.5, 0.0, 0.0),
            velocity: DVec3::ZERO,
        });
        assert_eq!(
            step_once(&mut player, &chunks, false),
            vec![("block.stone.step", 0.15, 1.0)]
        );
        assert_eq!(player.movement_sounds.next_step, 2.0);
    }

    #[test]
    fn sneaking_on_the_ground_or_flying_is_silent() {
        let chunks = store("stone", None);
        let mut player = walker(dvec3(3.0, 0.0, 0.0));
        assert!(step_once(&mut player, &chunks, true).is_empty());
        let mut player = walker(dvec3(3.0, 0.0, 0.0));
        player.flying = true;
        assert!(step_once(&mut player, &chunks, false).is_empty());
    }

    #[test]
    fn a_snow_layer_plays_over_a_muffled_step_on_the_block_below() {
        let chunks = store("stone", Some(("snow", &[("layers", "1")])));
        let mut player = walker(dvec3(3.0, 0.0, 0.0));
        assert_eq!(
            step_once(&mut player, &chunks, false),
            vec![
                ("block.snow.step", 0.15, 1.0),
                ("block.stone.step", 0.05, 0.8)
            ]
        );
    }

    #[test]
    fn in_water_a_step_is_a_swim_stroke_and_a_muffled_step() {
        let chunks = store("stone", None);
        let mut player = walker(dvec3(3.0, 0.0, 0.0));
        player.in_water = true;
        let sounds = step_once(&mut player, &chunks, false);
        assert_eq!(sounds.len(), 2);
        assert_eq!(sounds[0].0, "entity.player.swim");
        assert_eq!(sounds[1], ("block.stone.step", 0.05, 0.8));
    }

    #[test]
    fn amethyst_chimes_at_most_once_a_second() {
        let chunks = store("amethyst_block", None);
        let mut player = walker(dvec3(3.0, 0.0, 0.0));
        player.movement_sounds.ticks_since_chime = 19;
        let events = |sounds: Vec<(&'static str, f32, f32)>| {
            sounds.into_iter().map(|s| s.0).collect::<Vec<_>>()
        };
        assert_eq!(
            events(step_once(&mut player, &chunks, false)),
            ["block.amethyst_block.step", "block.amethyst_block.chime"]
        );
        player.movement_sounds.last_move = Some(Move {
            movement: dvec3(3.0, 0.0, 0.0),
            velocity: DVec3::ZERO,
        });
        assert_eq!(
            events(step_once(&mut player, &chunks, false)),
            ["block.amethyst_block.step"]
        );
    }
}
