//! The sounds the local player's own movement makes: footsteps, the amethyst
//! chime and swimming (vanilla `Entity.applyMovementEmissionAndPlaySound` and
//! `Player.playStepSound`), landing (`Entity.checkFallDamage`) and splashing
//! into water (`Entity.doWaterSplashEffect`). Vanilla's client plays these
//! itself (`LocalPlayer.playSound`) and the server sends them only to other
//! players.
//!
//! Pitch jitter uses `fastrand`, as Pomme's other sounds do, where vanilla
//! draws from the entity's random.
//!
//! TODO: the client's other sounds for its own player: the elytra wind
//! (`ElytraOnPlayerSoundInstance`), underwater ambience and the honey block's
//! wall slide.

use glam::DVec3;

use super::collision::CollisionWorld;
use super::movement::on_pos;
use crate::attribute::AttributeKind;
use crate::player::LocalPlayer;
use crate::position::BlockPos;
use crate::world::block::sound::block_sounds_for_id;
use crate::world::block::{BlockStateData, BlockTags};

/// `Player.getFallSounds`.
const SMALL_FALL: &str = "entity.player.small_fall";
const BIG_FALL: &str = "entity.player.big_fall";

/// A sound for the caller to play where the player stands, in the `players`
/// category.
pub struct PlayerSound {
    pub event: &'static str,
    pub volume: f32,
    pub pitch: f32,
    /// Skipped while the player is silent, as
    /// `LivingEntity.playBlockFallSound` is; `LocalPlayer.playSound` isn't.
    pub unless_silent: bool,
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
    /// Vanilla `Entity.firstTick`: no splash for spawning in water.
    pub(crate) first_tick: bool,
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
            first_tick: true,
            last_move: None,
            pending: Vec::new(),
        }
    }
}

/// `Entity.move`'s `applyMovementEmissionAndPlaySound` for this tick's move.
/// `Player.getMovementEmission` emits nothing while flying or sneaking on
/// the ground, and `Entity.move` skips it for a passenger.
pub(super) fn tick(
    player: &mut LocalPlayer,
    world: CollisionWorld<'_>,
    tags: &BlockTags,
    shift_down: bool,
    is_passenger: bool,
) {
    let sounds = &mut player.movement_sounds;
    sounds.ticks_since_chime = sounds.ticks_since_chime.saturating_add(1);
    let Some(mv) = sounds.last_move.take() else {
        return;
    };
    if player.flying || (player.on_ground && shift_down) || is_passenger {
        return;
    }

    let effect_pos = on_pos(player, world, 0.2);
    let support_pos = on_pos(player, world, 1.0e-5);
    let support = world.block_data(support_pos);
    let moved = if is_state_climbable(support, tags) {
        mv.movement.length()
    } else {
        mv.movement.x.hypot(mv.movement.z)
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
#[allow(clippy::too_many_arguments)]
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
    if player.in_water {
        water_swim_sound(player, mv.velocity);
        muffled_step(&mut player.movement_sounds, on);
        return;
    }
    let sounds = &mut player.movement_sounds;
    // `Entity.getPrimaryStepSoundBlockPos`.
    let above = world.block_data(BlockPos::new(pos.x, pos.y + 1, pos.z));
    if tags.contains("minecraft:combination_step_sound_blocks", above.id()) {
        step(sounds, above, 0.15, 1.0);
        muffled_step(sounds, on);
    } else if tags.contains("minecraft:inside_step_sound_blocks", above.id()) {
        step(sounds, above, 0.15, 1.0);
    } else {
        step(sounds, on, 0.15, 1.0);
    }
}

/// `Entity.playMuffledStepSound`.
fn muffled_step(sounds: &mut MovementSounds, state: &BlockStateData) {
    step(sounds, state, 0.05, 0.8);
}

/// `state`'s step sound, its `SoundType` volume and pitch scaled.
fn step(sounds: &mut MovementSounds, state: &BlockStateData, volume: f32, pitch: f32) {
    let sound_type = block_sounds_for_id(state.id());
    play(
        sounds,
        &sound_type.step_event,
        sound_type.volume * volume,
        sound_type.pitch * pitch,
    );
}

/// `Entity.waterSwimSound` for the player itself (no controlling passenger).
fn water_swim_sound(player: &mut LocalPlayer, velocity: DVec3) {
    let volume = (fluid_speed(velocity) * 0.35).min(1.0);
    play(
        &mut player.movement_sounds,
        "entity.player.swim",
        volume,
        jittered_pitch(),
    );
}

/// `Entity.doWaterSplashEffect`'s sound for the player itself, at the
/// velocity it entered the water with.
pub(crate) fn splash(sounds: &mut MovementSounds, velocity: DVec3) {
    let speed = (fluid_speed(velocity) * 0.2).min(1.0);
    let event = if speed < 0.25 {
        "entity.player.splash"
    } else {
        "entity.player.splash.high_speed"
    };
    play(sounds, event, speed, jittered_pitch());
}

/// The speed swimming and splashing scale by, horizontal motion damped.
fn fluid_speed(velocity: DVec3) -> f32 {
    (velocity.x * velocity.x * f64::from(0.2_f32)
        + velocity.y * velocity.y
        + velocity.z * velocity.z * f64::from(0.2_f32))
    .sqrt() as f32
}

/// `1 + (nextFloat() - nextFloat()) * 0.4`.
fn jittered_pitch() -> f32 {
    1.0 + (fastrand::f32() - fastrand::f32()) * 0.4
}

/// `Entity.checkFallDamage`'s landing on `on`: the block's `fallOn`.
pub(super) fn land(
    player: &mut LocalPlayer,
    world: CollisionWorld<'_>,
    on: &BlockStateData,
    descending: bool,
) {
    let fall = player.fall_distance;
    match on.id() {
        "hay_block" => {
            cause_fall_damage(player, world, fall, 0.2);
        }
        "honey_block" => {
            play(
                &mut player.movement_sounds,
                "block.honey_block.slide",
                1.0,
                1.0,
            );
            if cause_fall_damage(player, world, fall, 0.2) {
                block_fall_sound(&mut player.movement_sounds, "honey_block", false);
            }
        }
        "pointed_dripstone"
            if on.properties().get("vertical_direction") == Some("up")
                && on.properties().get("thickness") == Some("tip") =>
        {
            cause_fall_damage(player, world, fall + 2.5, 2.0);
        }
        "powder_snow" => {
            if fall >= 4.0 {
                let event = if fall < 7.0 { SMALL_FALL } else { BIG_FALL };
                play(&mut player.movement_sounds, event, 1.0, 1.0);
            }
        }
        // Bouncing takes no fall damage.
        "slime_block" if !descending => {}
        id => {
            // `getFallDistanceReduction`: beds and the shelf mushroom halve
            // the fall.
            let reduction = if id.ends_with("_bed") || id == "shelf_mushroom" {
                0.5_f32
            } else {
                0.0
            };
            cause_fall_damage(player, world, fall * f64::from(1.0 - reduction), 1.0);
        }
    }
}

/// `Player.causeFallDamage`'s sounds: whether the fall hurts, playing the
/// player's fall sound and the landed-on block's when it does.
// TODO: `isIgnoringFallDamageFromCurrentImpulse` (wind charge and mace
// launches).
fn cause_fall_damage(
    player: &mut LocalPlayer,
    world: CollisionWorld<'_>,
    fall: f64,
    modifier: f32,
) -> bool {
    if player.may_fly {
        return false;
    }
    let attribute = |kind: AttributeKind| {
        player
            .attributes
            .value(kind)
            .unwrap_or(kind.definition().default_value)
    };
    let power = fall + 1.0e-6 - attribute(AttributeKind::SafeFallDistance);
    let damage = (power * f64::from(modifier) * attribute(AttributeKind::FallDamageMultiplier))
        .floor() as i32;
    if damage <= 0 {
        return false;
    }
    let event = if damage > 4 { BIG_FALL } else { SMALL_FALL };
    play(&mut player.movement_sounds, event, 1.0, 1.0);
    // `LivingEntity.playBlockFallSound`.
    let below = world.block_data(BlockPos::new(
        player.position.x.floor() as i32,
        (player.position.y - f64::from(0.2_f32)).floor() as i32,
        player.position.z.floor() as i32,
    ));
    if !below.is_air() {
        block_fall_sound(&mut player.movement_sounds, below.id(), true);
    }
    true
}

/// Block `id`'s fall sound, at half its volume and three quarters its pitch.
fn block_fall_sound(sounds: &mut MovementSounds, id: &str, unless_silent: bool) {
    let sound_type = block_sounds_for_id(id);
    queue(
        sounds,
        &sound_type.fall_event,
        sound_type.volume * 0.5,
        sound_type.pitch * 0.75,
        unless_silent,
    );
}

/// `Entity.playAmethystStepSound`.
fn amethyst_chime(sounds: &mut MovementSounds) {
    let decay = 0.997_f64.powi(sounds.ticks_since_chime as i32) as f32;
    let intensity = (sounds.crystal_sound_intensity * decay + 0.07).min(1.0);
    sounds.crystal_sound_intensity = intensity;
    let pitch = 0.5 + intensity * fastrand::f32() * 1.2;
    let volume = 0.1 + intensity * 1.2;
    sounds.ticks_since_chime = 0;
    play(sounds, "block.amethyst_block.chime", volume, pitch);
}

/// [`queue`]s a sound `LocalPlayer.playSound` plays even while silent.
fn play(sounds: &mut MovementSounds, event: &'static str, volume: f32, pitch: f32) {
    queue(sounds, event, volume, pitch, false);
}

/// Queues `event` unless it's a silent `SoundType` slot.
fn queue(
    sounds: &mut MovementSounds,
    event: &'static str,
    volume: f32,
    pitch: f32,
    unless_silent: bool,
) {
    if !event.is_empty() {
        sounds.pending.push(PlayerSound {
            event,
            volume,
            pitch,
            unless_silent,
        });
    }
}

/// `Entity.isStateClimbable`.
fn is_state_climbable(state: &BlockStateData, tags: &BlockTags) -> bool {
    tags.contains("minecraft:climbable", state.id()) || state.id() == "powder_snow"
}

#[cfg(test)]
mod tests {
    use glam::dvec3;

    use super::*;
    use crate::world::block::{BlockStateTable, block_tags_for_test};
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

    /// The sounds landing on `ground` at (8, 63, 8) after falling `fall`.
    fn landing(
        ground: &str,
        props: &[(&str, &str)],
        fall: f64,
        may_fly: bool,
    ) -> Vec<(&'static str, bool)> {
        let table = BlockStateTable::native();
        let chunks = ChunkStore::with_origin_chunk();
        chunks.set_block_state(8, 63, 8, table.find_state(ground, props), table);
        let mut player = walker(DVec3::ZERO);
        player.fall_distance = fall;
        player.may_fly = may_fly;
        let world = CollisionWorld::new(&chunks, table);
        land(
            &mut player,
            world,
            world.block_data(BlockPos::new(8, 63, 8)),
            false,
        );
        player
            .movement_sounds
            .pending
            .iter()
            .map(|s| (s.event, s.unless_silent))
            .collect()
    }

    #[test]
    fn a_hurting_fall_plays_the_player_and_block_fall_sounds() {
        // Three blocks are safe; 5 deal 2 and 10 deal 7, past the big fall's 4.
        assert!(landing("stone", &[], 3.0, false).is_empty());
        assert_eq!(
            landing("stone", &[], 5.0, false),
            [(SMALL_FALL, false), ("block.stone.fall", true)]
        );
        assert_eq!(landing("stone", &[], 10.0, false)[0].0, BIG_FALL);
        // Creative players take no fall damage.
        assert!(landing("stone", &[], 10.0, true).is_empty());
    }

    #[test]
    fn landing_blocks_change_the_fall() {
        // A bed halves the fall, so 6 blocks hurt no more than 3.
        assert!(landing("red_bed", &[], 6.0, false).is_empty());
        // Hay takes a fifth: 10 blocks deal 1.
        assert_eq!(landing("hay_block", &[], 10.0, false)[0].0, SMALL_FALL);
        // Powder snow plays only the player's fall sound, from 4 blocks.
        assert!(landing("powder_snow", &[], 3.0, false).is_empty());
        assert_eq!(
            landing("powder_snow", &[], 5.0, false),
            [(SMALL_FALL, false)]
        );
        // Bouncing on slime hurts nothing.
        assert!(landing("slime_block", &[], 20.0, false).is_empty());
        // A stalagmite tip adds 2.5 blocks and doubles the damage.
        let tip = [("vertical_direction", "up"), ("thickness", "tip")];
        assert_eq!(
            landing("pointed_dripstone", &tip, 1.0, false)[0].0,
            SMALL_FALL
        );
    }

    #[test]
    fn splashes_get_louder_with_speed() {
        let mut sounds = MovementSounds::default();
        splash(&mut sounds, dvec3(0.0, -1.0, 0.0));
        splash(&mut sounds, dvec3(0.0, -2.0, 0.0));
        let events: Vec<_> = sounds.pending.iter().map(|s| (s.event, s.volume)).collect();
        assert_eq!(
            events,
            [
                ("entity.player.splash", 0.2),
                ("entity.player.splash.high_speed", 0.4)
            ]
        );
    }
}
