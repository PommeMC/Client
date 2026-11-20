// TODO: fall damage - track fall distance, reset on water entry, apply damage
// on ground impact; Player.causeFallDamage returns false when may_fly, and
// fall distance resets every tick while flying

use glam::{DVec3, dvec3};
use pomme_block::BlockState;

use super::aabb::Aabb;
use super::block_shape::CollisionContext;
use super::collision::{
    CollisionWorld, find_supporting_block, no_collision, resolve_collision, state_boxes,
};
use super::movement_sound;
use crate::app::input::{self, InputState};
use crate::attribute::AttributeKind;
use crate::entity::components::Velocity;
use crate::player::{CROUCH_HEIGHT, LocalPlayer, PLAYER_HALF_WIDTH, STANDING_HEIGHT, is_spectator};
use crate::position::BlockPos;
use crate::world::block::model::Direction;
use crate::world::block::{BlockStateData, BlockStateTable, BlockTags, FluidKind};
use crate::world::chunk::ChunkStore;
use crate::world::piston::{self, MovingPiston};

// Vanilla mixes float and double physics values. Keep float values as f32 until
// the exact point where vanilla widens them into Vec3/AABB doubles; an
// attribute is a double that its reader casts.
const VERTICAL_DRAG: f32 = 0.98;
const HORIZONTAL_DRAG: f32 = 0.91;
const GROUND_ACCEL_FACTOR: f32 = 0.216_000_02;
const INPUT_DAMPING: f32 = 0.98;
const AIR_ACCELERATION: f32 = 0.02;
const WATER_ACCELERATION: f32 = 0.02;
// `travelInWater`'s drag with full WATER_MOVEMENT_EFFICIENCY.
const WATER_EFFICIENT_DRAG: f32 = 0.546_000_06;
const WATER_HORIZONTAL_DRAG: f32 = 0.8;
const WATER_HORIZONTAL_DRAG_SPRINT: f32 = 0.9;
const WATER_VERTICAL_DRAG: f32 = 0.8;
const SPRINT_JUMP_BOOST: f64 = 0.2;
const FLYING_VERTICAL_FRICTION: f64 = 0.6;
// Vanilla Player.getFlyingSpeed values are floats.
const SPRINT_AIR_ACCELERATION: f32 = 0.025_999_999;
const SPRINT_HUNGER_THRESHOLD: u32 = 6;
const JUMP_DELAY_TICKS: u32 = 10;
// Vanilla `Entity.getFluidJumpThreshold`; always 0.4 for the player.
const FLUID_JUMP_THRESHOLD: f64 = 0.4;
// Vanilla `LivingEntity.jumpInLiquid` / `goDownInWater` add/subtract 0.04f.
const LIQUID_JUMP_ACCELERATION: f32 = 0.04;
const FLY_TOGGLE_WINDOW: u32 = 7;
// Mth.equal(double, double) widens the float EPSILON constant to double.
const MTH_EQUAL_EPSILON: f64 = 1.0e-5_f32 as f64;
const MINOR_COLLISION_ANGLE: f64 = 0.139_626_339_077_949_52;
const CLIMB_UP_VELOCITY: f64 = 0.2;
const CLIMB_MAX_FALL_SPEED: f32 = 0.15;
pub(crate) const DEG_TO_RAD: f32 = std::f32::consts::PI / 180.0_f32;
const SIN_SCALE: f64 = 10_430.378_350_470_453;

/// Vanilla `Mth.SIN`: 65536 samples of one turn, in its own division form.
static SIN: std::sync::LazyLock<Box<[f32]>> = std::sync::LazyLock::new(|| {
    (0..65536)
        .map(|i| (f64::from(i) / SIN_SCALE).sin() as f32)
        .collect()
});

/// The vehicle the player rides, as the sprint and flight-toggle gates see it.
#[derive(Clone, Copy)]
pub struct Vehicle {
    /// `jumpableVehicle() != null`. Pomme's controllable vehicles are saddled
    /// equines, so this also stands in for `vehicleCanSprint`.
    pub jumpable: bool,
}

pub fn tick(
    player: &mut LocalPlayer,
    input: &InputState,
    world: CollisionWorld<'_>,
    tags: &BlockTags,
    use_speed_multiplier: f32,
    slow_due_to_using_item: bool,
    vehicle: Option<Vehicle>,
) {
    let is_passenger = vehicle.is_some();
    // `Entity.rideTick` clears a passenger's motion before its tick; its
    // vehicle then seats it.
    if is_passenger {
        player.velocity = Velocity::new(0.0, 0.0, 0.0);
    }
    // `LocalPlayer.aiStep`: an auto-jump presses jump for its tick.
    player.auto_jumping = player.auto_jump_time > 0;
    if player.auto_jumping {
        player.auto_jump_time -= 1;
    }
    let raw_jump_held = input.performing_action(input::Action::Jump) || player.auto_jumping;
    movement_sound::advance_tick(&mut player.movement_sounds);

    // `Player.tick` snapshots `wasUnderwater` before `Entity.baseTick`
    // refreshes fluid contact (and applies its current).
    player.under_water = player.eyes_in_water;
    player.update_water_state(world.chunks, world.block_state_table, is_passenger);
    player.movement_sounds.first_tick = false;

    if player.flying {
        // Vanilla `Player.aiStep`.
        player.fall_distance = 0.0;
    }

    // Vanilla `LivingEntity.aiStep`.
    if player.no_jump_delay > 0 {
        player.no_jump_delay -= 1;
    }

    update_movement_crouching_state(player, world, is_passenger);
    player.tick_eye_height();

    // Vanilla `LocalPlayer.modifyInput` keeps the entire input pipeline in
    // float: damping, item-use slowdown, sneaking slowdown, then square remap.
    let moving_slowly = is_moving_slowly(player);
    let sneaking_speed = moving_slowly.then(|| sneaking_speed(player));
    let (mut forward, mut strafe) = movement_input(input, sneaking_speed, use_speed_multiplier);
    let forward_pressed = has_forward_impulse(input);

    update_sprint_state(
        player,
        input,
        forward_pressed,
        slow_due_to_using_item,
        vehicle,
    );

    let (sin_y_rot, cos_y_rot) = vanilla_yaw_sin_cos(player.look_dir.y_rot_deg());

    update_fly_state(
        player,
        raw_jump_held,
        world,
        tags,
        vehicle,
        sin_y_rot,
        cos_y_rot,
    );

    // `LocalPlayer.aiStep`'s `goDownInWater`, before the living jump.
    if player.in_water
        && input.performing_action(input::Action::Sneak)
        && player.is_affected_by_fluids()
    {
        player.velocity.y -= f64::from(LIQUID_JUMP_ACCELERATION);
    }

    if player.flying {
        let mut input_ya = 0.0f32;
        if input.performing_action(input::Action::Sneak) {
            input_ya -= 1.0;
        }
        if raw_jump_held {
            input_ya += 1.0;
        }
        if input_ya != 0.0 {
            // Vanilla does this math in f32 before widening.
            player.velocity.y += f64::from(input_ya * player.fly_speed * 3.0);
        }
    }

    // `LivingEntity.aiStep` starts here.
    normalize_tiny_velocity(&mut player.velocity);

    // `Player.isImmobile` (asleep) zeroes locomotion and jump input; travel
    // still runs.
    let mut jump_held = raw_jump_held;
    apply_living_immobility(player, &mut forward, &mut strafe, &mut jump_held);

    // Vanilla `LivingEntity.aiStep`: swim upward when submerged past the jump
    // threshold, otherwise a full jump off the ground or the shallow-fluid floor.
    // TODO: lava `jumpInLiquid`.
    if jump_held && player.is_affected_by_fluids() {
        let in_water = player.in_water && player.fluid_height > 0.0;
        if in_water && (!player.on_ground || player.fluid_height > FLUID_JUMP_THRESHOLD) {
            player.velocity.y += f64::from(LIQUID_JUMP_ACCELERATION);
        } else if (player.on_ground || (in_water && player.fluid_height <= FLUID_JUMP_THRESHOLD))
            && player.no_jump_delay == 0
        {
            jump_from_ground(player, world, tags, sin_y_rot, cos_y_rot);
            player.no_jump_delay = JUMP_DELAY_TICKS;
        }
    } else {
        player.no_jump_delay = 0;
    }

    player.move_forward = forward;
    player.move_strafe = strafe;
    travel(
        player,
        input,
        world,
        tags,
        is_passenger,
        forward,
        strafe,
        sin_y_rot,
        cos_y_rot,
    );
    step_on_block(
        player,
        world,
        tags,
        input.performing_action(input::Action::Sneak),
    );

    stop_flying_on_ground(player);

    // `Player.updatePlayerPose` runs after movement, so the new pose applies
    // from next tick.
    update_player_pose(player, input, world, is_passenger);

    player.was_forward_pressed = forward_pressed;
    player.was_shift_pressed = input.performing_action(input::Action::Sneak);
    player.was_jump_pressed = raw_jump_held;
}

/// `Entity.applyEffectsFromBlocks`' `Block.stepOn`, which only a slime block
/// uses to move the player: slow or still vertically, and not sneaking, it
/// drags the horizontal speed down.
fn step_on_block(
    player: &mut LocalPlayer,
    world: CollisionWorld<'_>,
    tags: &BlockTags,
    stepping_carefully: bool,
) {
    if !player.on_ground || stepping_carefully {
        return;
    }
    let abs_y = player.velocity.y.abs();
    if abs_y < 0.1 && world.block_data(on_pos(player, world, tags, 0.2)).id() == "slime_block" {
        let scale = 0.4 + abs_y * 0.2;
        player.velocity.x *= scale;
        player.velocity.z *= scale;
    }
}

fn apply_living_immobility(
    player: &LocalPlayer,
    forward: &mut f32,
    strafe: &mut f32,
    jumping: &mut bool,
) {
    if player.is_sleeping() {
        *forward = 0.0;
        *strafe = 0.0;
        *jumping = false;
    }
}

/// Vanilla `LivingEntity.travel`: fluid travel only when fluids affect the
/// player (not while flying).
// TODO: lava travel.
#[allow(clippy::too_many_arguments)]
fn travel(
    player: &mut LocalPlayer,
    input: &InputState,
    world: CollisionWorld<'_>,
    tags: &BlockTags,
    is_passenger: bool,
    forward: f32,
    strafe: f32,
    sin_y_rot: f32,
    cos_y_rot: f32,
) {
    if player.in_water && player.is_affected_by_fluids() {
        tick_water(
            player,
            input,
            world,
            tags,
            is_passenger,
            forward,
            strafe,
            sin_y_rot,
            cos_y_rot,
        );
    } else {
        tick_land(
            player,
            input,
            world,
            tags,
            is_passenger,
            forward,
            strafe,
            sin_y_rot,
            cos_y_rot,
        );
    }
}

/// Touching down cancels flight, even in creative.
fn stop_flying_on_ground(player: &mut LocalPlayer) {
    if player.on_ground && player.flying && player.game_mode != 3 {
        player.flying = false;
        player.abilities_dirty = true;
    }
}

/// Vanilla dead-player `LivingEntity.aiStep`: input is immobile, but travel
/// still applies existing velocity, gravity, collision, and drag until tick-20
/// removal.
pub fn tick_dead(
    player: &mut LocalPlayer,
    world: CollisionWorld<'_>,
    tags: &BlockTags,
    is_passenger: bool,
) {
    movement_sound::advance_tick(&mut player.movement_sounds);
    player.no_jump_delay = 0;

    // Local players enter death through SetHealth; entity event 3 intentionally
    // skips LivingEntity.die for players, so the current ordinary player pose
    // remains authoritative until Player.updatePlayerPose runs at tick end.
    let neutral = InputState::released();
    let vehicle = is_passenger.then_some(Vehicle { jumpable: false });
    update_sprint_state(player, &neutral, false, false, vehicle);
    player.update_water_state(world.chunks, world.block_state_table, is_passenger);
    player.tick_eye_height();

    let (sin_y_rot, cos_y_rot) = vanilla_yaw_sin_cos(player.look_dir.y_rot_deg());
    normalize_tiny_velocity(&mut player.velocity);
    player.move_forward = 0.0;
    player.move_strafe = 0.0;
    travel(
        player,
        &neutral,
        world,
        tags,
        is_passenger,
        0.0,
        0.0,
        sin_y_rot,
        cos_y_rot,
    );

    // Player.updatePlayerPose runs after LivingEntity.tick in vanilla. With
    // death-screen input released, this becomes standing unless clearance keeps
    // the player in the crouching pose for the following tick.
    update_player_pose(player, &neutral, world, is_passenger);

    stop_flying_on_ground(player);
    player.was_forward_pressed = false;
    player.was_shift_pressed = false;
    player.was_jump_pressed = false;
}

// Vanilla `LocalPlayer.aiStep`: a fresh jump press arms the toggle window;
// a second one inside it toggles flight.
fn update_fly_state(
    player: &mut LocalPlayer,
    jump_held: bool,
    world: CollisionWorld<'_>,
    tags: &BlockTags,
    vehicle: Option<Vehicle>,
    sin_y_rot: f32,
    cos_y_rot: f32,
) {
    if player.may_fly {
        if player.game_mode == 3 {
            // Spectator flight is forced on. TODO: spectator noclip
            if !player.flying {
                player.flying = true;
                player.abilities_dirty = true;
            }
        } else if !player.was_jump_pressed && jump_held && !player.auto_jumping {
            if player.jump_trigger_time == 0 {
                player.jump_trigger_time = FLY_TOGGLE_WINDOW;
            } else if !player.swimming && vehicle.is_none_or(|v| v.jumpable) {
                player.flying = !player.flying;
                if player.flying && player.on_ground {
                    jump_from_ground(player, world, tags, sin_y_rot, cos_y_rot);
                }
                player.abilities_dirty = true;
                player.jump_trigger_time = 0;
            }
        }
    }
    // Vanilla decrements after the toggle check (unlike sprint_toggle_timer).
    if player.jump_trigger_time > 0 {
        player.jump_trigger_time -= 1;
    }
}

fn jump_from_ground(
    player: &mut LocalPlayer,
    world: CollisionWorld<'_>,
    tags: &BlockTags,
    sin_y_rot: f32,
    cos_y_rot: f32,
) {
    // `LivingEntity.getJumpPower`.
    // TODO: `getJumpBoostPower` adds the jump boost effect's 0.1 a level.
    let jump_power = player.attribute(AttributeKind::JumpStrength) as f32
        * block_jump_factor(player, world, tags);
    if jump_power <= 1.0e-5 {
        return;
    }
    player.velocity.y = f64::from(jump_power).max(player.velocity.y);

    if player.sprinting {
        player.velocity.x += f64::from(-sin_y_rot) * SPRINT_JUMP_BOOST;
        player.velocity.z += f64::from(cos_y_rot) * SPRINT_JUMP_BOOST;
    }
}

pub(super) fn is_climbable_state(block_state_data: &BlockStateData, tags: &BlockTags) -> bool {
    tags.contains("minecraft:climbable", block_state_data.id())
}

fn trapdoor_usable_as_ladder(
    trapdoor_block_state_data: &BlockStateData,
    below_block_state_data: &BlockStateData,
) -> bool {
    let props = trapdoor_block_state_data.properties();
    trapdoor_block_state_data.id().ends_with("_trapdoor")
        && props.get("open") == Some("true")
        && below_block_state_data.id() == "ladder"
        && props.get("facing") == below_block_state_data.properties().get("facing")
}

fn on_climbable_state(
    player: &LocalPlayer,
    chunk_store: &ChunkStore,
    block_state_table: BlockStateTable,
    tags: &BlockTags,
) -> Option<pomme_block::BlockState> {
    // TODO: `onClimbable` also rejects fall-flying in `can_glide_through`.
    if player.flying || is_spectator(player.game_mode) {
        return None;
    }

    let x = player.position.x.floor() as i32;
    let y = player.position.y.floor() as i32;
    let z = player.position.z.floor() as i32;
    let state = chunk_store.get_block_state(x, y, z);
    if is_climbable_state(block_state_table.block_state_data(state), tags) {
        return Some(state);
    }

    let below = chunk_store.get_block_state(x, y - 1, z);
    trapdoor_usable_as_ladder(
        block_state_table.block_state_data(state),
        block_state_table.block_state_data(below),
    )
    .then_some(state)
}

fn apply_climbable_velocity(
    player: &mut LocalPlayer,
    block_state_data: &BlockStateData,
    suppress_slide: bool,
) {
    player.fall_distance = 0.0;
    let max = f64::from(CLIMB_MAX_FALL_SPEED);
    player.velocity.x = player.velocity.x.clamp(-max, max);
    player.velocity.z = player.velocity.z.clamp(-max, max);
    player.velocity.y = player.velocity.y.max(-max);

    if player.velocity.y < 0.0 && block_state_data.id() != "scaffolding" && suppress_slide {
        player.velocity.y = 0.0;
    }
}

fn handle_on_climbable(
    player: &mut LocalPlayer,
    world: CollisionWorld<'_>,
    tags: &BlockTags,
    suppress_slide: bool,
) {
    let Some(state) = on_climbable_state(player, world.chunks, world.block_state_table, tags)
    else {
        return;
    };
    apply_climbable_velocity(
        player,
        world.block_state_table.block_state_data(state),
        suppress_slide,
    );
}

/// `Player.onClimbable`.
pub(crate) fn on_climbable(
    player: &LocalPlayer,
    chunk_store: &ChunkStore,
    block_state_table: BlockStateTable,
    tags: &BlockTags,
) -> bool {
    on_climbable_state(player, chunk_store, block_state_table, tags).is_some()
}

#[allow(clippy::too_many_arguments)]
fn tick_land(
    player: &mut LocalPlayer,
    input: &InputState,
    world: CollisionWorld<'_>,
    tags: &BlockTags,
    is_passenger: bool,
    forward: f32,
    strafe: f32,
    sin_y_rot: f32,
    cos_y_rot: f32,
) {
    // Vanilla `travelInAir` samples on-ground once before the move and reuses
    // it for the end-of-tick drag, so a jump launches with ground friction.
    let on_ground_at_start = player.on_ground;

    let saved_vy = player.velocity.y;
    let block_friction = if on_ground_at_start {
        modified_friction(
            world
                .block_state_table
                .block_state_data(block_below_affecting_movement(player, world, tags))
                .movement_friction(),
            player.attribute(AttributeKind::FrictionModifier) as f32,
        )
    } else {
        1.0
    };
    let accel = friction_influenced_speed(movement_speed(player), player, block_friction);
    move_relative(player, forward, strafe, accel, sin_y_rot, cos_y_rot);

    handle_on_climbable(
        player,
        world,
        tags,
        input.performing_action(input::Action::Sneak),
    );

    apply_collision(
        player,
        input,
        world,
        tags,
        is_passenger,
        forward,
        strafe,
        sin_y_rot,
        cos_y_rot,
    );

    // TODO: `wasInPowderSnow && canEntityWalkOnPowderSnow` also takes the impulse.
    if (player.horizontal_collision || input.performing_action(input::Action::Jump))
        && on_climbable(player, world.chunks, world.block_state_table, tags)
    {
        player.velocity.y = CLIMB_UP_VELOCITY;
    }

    // `Entity.move`, before gravity and drag.
    let speed_factor = block_speed_factor(player, world, tags);
    player.velocity.x *= f64::from(speed_factor);
    player.velocity.z *= f64::from(speed_factor);

    // TODO: over an unloaded chunk vanilla sets vy to -0.1 instead.
    player.velocity.y -= gravity(player);
    player.velocity.y *= f64::from(air_drag(player, VERTICAL_DRAG));

    let h_friction = block_friction * air_drag(player, HORIZONTAL_DRAG);
    player.velocity.x *= f64::from(h_friction);
    player.velocity.z *= f64::from(h_friction);

    overwrite_flying_vy(player, saved_vy);
}

fn water_falling_adjusted_y(
    movement_y: f64,
    base_gravity: f64,
    is_falling: bool,
    sprinting: bool,
) -> f64 {
    if base_gravity == 0.0 || sprinting {
        return movement_y;
    }
    if is_falling
        && (movement_y - 0.005).abs() >= 0.003
        && (movement_y - base_gravity / 16.0).abs() < 0.003
    {
        -0.003
    } else {
        movement_y - base_gravity / 16.0
    }
}

#[allow(clippy::too_many_arguments)]
fn tick_water(
    player: &mut LocalPlayer,
    input: &InputState,
    world: CollisionWorld<'_>,
    tags: &BlockTags,
    is_passenger: bool,
    forward: f32,
    strafe: f32,
    sin_y_rot: f32,
    cos_y_rot: f32,
) {
    // `Player.travel`'s swim pitch steering; no upward pull at the surface
    // unless jumping.
    if player.swimming {
        let target_vy = vanilla_look_y(player.look_dir.x_rot_deg());
        let surface_y = (player.position.y + 1.0 - 0.1).floor() as i32;
        let surface_fluid = world
            .block_data(BlockPos::new(
                player.position.x.floor() as i32,
                surface_y,
                player.position.z.floor() as i32,
            ))
            .fluid();
        if target_vy <= 0.0
            || input.performing_action(input::Action::Jump)
            || surface_fluid.kind != FluidKind::Empty
        {
            let boost = if target_vy < -0.2 { 0.085 } else { 0.06 };
            player.velocity.y += (target_vy - player.velocity.y) * boost;
        }
    }

    let is_falling = player.velocity.y <= 0.0;
    let old_y = player.position.y;
    let saved_vy = player.velocity.y;

    // `LivingEntity.travelInWater`.
    let mut slow_down = if player.sprinting {
        WATER_HORIZONTAL_DRAG_SPRINT
    } else {
        WATER_HORIZONTAL_DRAG
    };
    let mut speed = WATER_ACCELERATION;
    let mut water_walker = player.attribute(AttributeKind::WaterMovementEfficiency) as f32;
    if !player.on_ground {
        water_walker *= 0.5;
    }
    if water_walker > 0.0 {
        slow_down += (WATER_EFFICIENT_DRAG - slow_down) * water_walker;
        speed += (movement_speed(player) - speed) * water_walker;
    }
    // TODO: dolphin's grace sets the drag to 0.96.
    move_relative(player, forward, strafe, speed, sin_y_rot, cos_y_rot);

    apply_collision(
        player,
        input,
        world,
        tags,
        is_passenger,
        forward,
        strafe,
        sin_y_rot,
        cos_y_rot,
    );
    // TODO: `Entity.move`'s block speed factor also applies in water.

    if player.horizontal_collision
        && on_climbable(player, world.chunks, world.block_state_table, tags)
    {
        player.velocity.y = CLIMB_UP_VELOCITY;
    }

    player.velocity.x *= f64::from(slow_down);
    player.velocity.y *= f64::from(WATER_VERTICAL_DRAG);
    player.velocity.z *= f64::from(slow_down);

    // `LivingEntity.getFluidFallingAdjustedMovement`.
    player.velocity.y = water_falling_adjusted_y(
        player.velocity.y,
        gravity(player),
        is_falling,
        player.sprinting,
    );

    // `LivingEntity.jumpOutOfFluid`.
    if player.horizontal_collision {
        let y_offset =
            player.velocity.y + f64::from(step_height(player)) - player.position.y + old_y;
        let ctx = collision_context(player, input.performing_action(input::Action::Sneak));
        let probe =
            player
                .bounding_box()
                .offset(dvec3(player.velocity.x, y_offset, player.velocity.z));
        if no_collision(world, &probe, &ctx) && !contains_any_liquid(world, &probe) {
            player.velocity.y = f64::from(0.3_f32);
        }
    }

    overwrite_flying_vy(player, saved_vy);
}

// Vanilla `Player.travel`: while flying the air travel step runs normally but
// its vertical result is discarded for the pre-travel vy decayed by 0.6.
fn overwrite_flying_vy(player: &mut LocalPlayer, saved_vy: f64) {
    if player.flying {
        player.velocity.y = saved_vy * FLYING_VERTICAL_FRICTION;
    }
}

/// Vanilla `LevelReader.containsAnyLiquid`: any fluid in an overlapped cell,
/// whatever its height.
fn contains_any_liquid(world: CollisionWorld<'_>, aabb: &Aabb) -> bool {
    let x0 = aabb.min.x.floor() as i32;
    let x1 = aabb.max.x.ceil() as i32;
    let y0 = aabb.min.y.floor() as i32;
    let y1 = aabb.max.y.ceil() as i32;
    let z0 = aabb.min.z.floor() as i32;
    let z1 = aabb.max.z.ceil() as i32;

    for x in x0..x1 {
        for y in y0..y1 {
            for z in z0..z1 {
                if world.block_data(BlockPos::new(x, y, z)).fluid().kind != FluidKind::Empty {
                    return true;
                }
            }
        }
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn apply_collision(
    player: &mut LocalPlayer,
    input: &InputState,
    world: CollisionWorld<'_>,
    tags: &BlockTags,
    is_passenger: bool,
    forward: f32,
    strafe: f32,
    sin_y_rot: f32,
    cos_y_rot: f32,
) {
    let aabb = player.bounding_box();
    let shift_down = input.performing_action(input::Action::Sneak);
    let ctx = collision_context(player, shift_down);
    let delta = back_off_from_edge(world, player, *player.velocity, shift_down, &ctx);
    let (resolved, on_ground) = resolve_collision(
        world,
        aabb,
        delta.into(),
        f64::from(step_height(player)),
        player.on_ground,
        &ctx,
    );

    let collided = collided_axes(delta, resolved);
    let horizontal_collision = collided[0] || collided[2];

    // TODO: `Entity.move`'s stuck speed (cobweb, berry bush, powder snow) and
    // its skip of sub-1e-7 moves.
    player.position += resolved;
    player.horizontal_collision = horizontal_collision;
    player.minor_horizontal_collision = horizontal_collision
        && is_minor_horizontal_collision(forward, strafe, sin_y_rot, cos_y_rot, resolved);
    let mv = settle_after_move(player, world, tags, resolved, on_ground, collided, &ctx);
    update_auto_jump(
        player,
        input,
        world,
        tags,
        is_passenger,
        (resolved.x as f32, resolved.z as f32),
        (sin_y_rot, cos_y_rot),
    );
    if !is_passenger {
        movement_sound::emit(player, world, tags, shift_down, mv);
    }
}

/// `LocalPlayer.updateAutoJump`, after each of the player's own moves: walking
/// into something between half a block and a jump tall, with room above,
/// starts a jump for the next tick. `(xa, za)` is the move just made.
fn update_auto_jump(
    player: &mut LocalPlayer,
    input: &InputState,
    world: CollisionWorld<'_>,
    tags: &BlockTags,
    is_passenger: bool,
    (xa, za): (f32, f32),
    (sin_y_rot, cos_y_rot): (f32, f32),
) {
    let shift_down = input.performing_action(input::Action::Sneak);
    let (move_x, move_y) = move_vector(input);
    // `canAutoJump`.
    if !player.auto_jump_enabled
        || player.auto_jump_time > 0
        || !player.on_ground
        || shift_down
        || is_passenger
        || move_x * move_x + move_y * move_y <= 0.0
        || f64::from(block_jump_factor(player, world, tags)) < 1.0
    {
        return;
    }

    let move_begin: DVec3 = player.position.into();
    let move_end = move_begin + dvec3(f64::from(xa), 0.0, f64::from(za));
    let mut move_diff = dvec3(f64::from(xa), 0.0, f64::from(za));
    let current_speed = movement_speed(player);
    let mut move_dist_sq = move_diff.length_squared() as f32;
    if move_dist_sq <= 0.001 {
        let input_xa = current_speed * move_x;
        let input_za = current_speed * move_y;
        move_diff = dvec3(
            f64::from(input_xa * cos_y_rot - input_za * sin_y_rot),
            0.0,
            f64::from(input_za * cos_y_rot + input_xa * sin_y_rot),
        );
        move_dist_sq = move_diff.length_squared() as f32;
        if move_dist_sq <= 0.001 {
            return;
        }
    }
    let move_dist_inverted = 1.0 / mth_sqrt(move_dist_sq);
    let move_dir = move_diff * f64::from(move_dist_inverted);
    let (facing_x, facing_z) = forward_xz(player);
    if ((facing_x * move_dir.x + facing_z * move_dir.z) as f32) < -0.15 {
        return;
    }

    let ctx = collision_context(player, shift_down);
    let has_collision = |pos: BlockPos| !world.block_boxes(pos, &ctx).is_empty();
    let mut ceiling_pos = BlockPos::containing(dvec3(
        move_begin.x,
        player.bounding_box().max.y,
        move_begin.z,
    ));
    if has_collision(ceiling_pos) {
        return;
    }
    ceiling_pos = ceiling_pos.relative(Direction::Up);
    if has_collision(ceiling_pos) {
        return;
    }

    let mut jump_height = 1.2_f32;
    if let Some(amplifier) = player.effects.amplifier(crate::mob_effect::JUMP_BOOST) {
        jump_height += (amplifier + 1) as f32 * 0.75;
    }
    let look_ahead_dist = (current_speed * 7.0).max(1.0 / move_dist_inverted);
    let seg_begin = move_begin;
    let seg_end = move_end + move_dir * f64::from(look_ahead_dist);
    let player_width = (PLAYER_HALF_WIDTH * 2.0) as f32;
    let player_height = player.height() as f32;
    let reach = seg_end + dvec3(0.0, f64::from(player_height), 0.0);
    let test_box = Aabb::new(seg_begin.min(reach), seg_begin.max(reach)).inflate(dvec3(
        f64::from(player_width),
        0.0,
        f64::from(player_width),
    ));
    let lift = dvec3(0.0, f64::from(0.51_f32), 0.0);
    let (seg_begin, seg_end) = (seg_begin + lift, seg_end + lift);
    let right_offset = dvec3(-move_dir.z, 0.0, move_dir.x) * f64::from(player_width * 0.5);
    // `AABB.intersects(Vec3, Vec3)`: against the box the two points span.
    let crosses = |aabb: &Aabb, a: DVec3, b: DVec3| aabb.intersects(&Aabb::new(a.min(b), a.max(b)));
    let collisions = world.collisions_in_order(&test_box, &ctx);
    let Some(obstacle) = collisions.iter().find(|aabb| {
        crosses(aabb, seg_begin - right_offset, seg_end - right_offset)
            || crosses(aabb, seg_begin + right_offset, seg_end + right_offset)
    }) else {
        return;
    };

    let mut obstacle_height = obstacle.max.y as f32;
    let obstacle_pos = BlockPos::containing((obstacle.min + obstacle.max) * 0.5);
    let mut steps = 1;
    while (steps as f32) < jump_height {
        let above_pos = BlockPos::new(obstacle_pos.x, obstacle_pos.y + steps, obstacle_pos.z);
        let top = world
            .block_boxes(above_pos, &ctx)
            .iter()
            .map(|aabb| aabb.max.y - f64::from(above_pos.y))
            .reduce(f64::max);
        if let Some(top) = top {
            obstacle_height = top as f32 + above_pos.y as f32;
            if f64::from(obstacle_height) - move_begin.y > f64::from(jump_height) {
                return;
            }
        }
        if steps > 1 {
            ceiling_pos = ceiling_pos.relative(Direction::Up);
            if has_collision(ceiling_pos) {
                return;
            }
        }
        steps += 1;
    }

    let y_delta = (f64::from(obstacle_height) - move_begin.y) as f32;
    if y_delta <= 0.5 || y_delta > jump_height {
        return;
    }
    player.auto_jump_time = 1;
}

/// The horizontal part of `Entity.getForward`, the look direction.
fn forward_xz(player: &LocalPlayer) -> (f64, f64) {
    let y_rot = -player.look_dir.y_rot_deg() * DEG_TO_RAD - std::f32::consts::PI;
    let x_cos = -mth_cos(-player.look_dir.x_rot_deg() * DEG_TO_RAD);
    (
        f64::from(mth_sin(y_rot) * x_cos),
        f64::from(mth_cos(y_rot) * x_cos),
    )
}

/// Which axes `Entity.move`'s collision shortened `delta` on.
pub(crate) fn collided_axes(delta: DVec3, resolved: DVec3) -> [bool; 3] {
    // Vanilla horizontal collision flags use Mth.equal(double, double), whose
    // epsilon is the widened float constant 1.0E-5f. Vertical collision is an
    // exact double comparison.
    [
        !mth_equal(delta.x, resolved.x),
        delta.y != resolved.y,
        !mth_equal(delta.z, resolved.z),
    ]
}

/// The rest of vanilla `Entity.move` once the move is applied: ground and
/// supporting block, fluid contact, fall distance, and stopping or bouncing
/// on the collided axes. Returns the move for its movement emission.
fn settle_after_move(
    player: &mut LocalPlayer,
    world: CollisionWorld<'_>,
    tags: &BlockTags,
    resolved: DVec3,
    on_ground: bool,
    collided: [bool; 3],
    ctx: &CollisionContext,
) -> movement_sound::Move {
    player.on_ground = on_ground;
    update_supporting_block(player, world, on_ground, resolved, ctx);
    let effect_pos = on_pos(player, world, tags, 0.2);

    // `LivingEntity.checkFallDamage` refreshes a dry player's fluid contact
    // (adding any current) before collided axes are zeroed; the fall distance
    // narrows the movement Y to float.
    if !player.in_water {
        player.refresh_water_interaction(world.chunks, world.block_state_table);
    }
    if !player.in_water && resolved.y < 0.0 {
        player.fall_distance -= f64::from(resolved.y as f32);
    }
    if on_ground {
        if player.fall_distance > 0.0 {
            movement_sound::land(
                player,
                world,
                world.block_data(effect_pos),
                ctx.descending(),
            );
        }
        player.fall_distance = 0.0;
    }

    if collided.contains(&true) {
        let landing =
            on_ground.then(|| block_bounce(world.block_data(effect_pos), tags).map(f64::from));
        let bouncer = Bouncer {
            bounciness: player.attribute(AttributeKind::Bounciness),
            gravity: gravity(player),
            air_drag: f64::from(air_drag(player, VERTICAL_DRAG)),
            suppressing: ctx.descending(),
        };
        *player.velocity =
            restitute_after_collisions(*player.velocity, collided, resolved, landing, bouncer);
    }
    movement_sound::Move {
        movement: resolved,
        velocity: *player.velocity,
    }
}

/// What `Entity.restituteMovementAfterCollisions` reads of the entity.
#[derive(Clone, Copy)]
pub(crate) struct Bouncer {
    /// `getEntityBounciness`.
    pub bounciness: f64,
    /// `getEffectiveGravity`.
    pub gravity: f64,
    /// `getAirDrag`.
    pub air_drag: f64,
    /// `isSuppressingBounce`: sneaking.
    pub suppressing: bool,
}

/// `block`'s `getBounceRestitution`, `None` when it suppresses bounces.
pub(crate) fn block_bounce(block: &BlockStateData, tags: &BlockTags) -> Option<f32> {
    (!tags.contains("minecraft:suppresses_bounce", block.id()))
        .then(|| block.movement_bounce_restitution())
}

/// Vanilla `Entity.restituteMovementAfterCollisions`, returning the new
/// velocity: a collided axis stops, or bounces back by the entity's
/// bounciness; a landing also bounces by the block's, unless it is slower
/// than gravity. `landing` is [`block_bounce`] of the block landed on, absent
/// when the collision wasn't from above.
// TODO: before 26.2 a slime block or bed bounced through
// `updateEntityMovementAfterFallOn`, with no gravity or drag correction.
pub(crate) fn restitute_after_collisions(
    current: DVec3,
    [x, y, z]: [bool; 3],
    movement: DVec3,
    landing: Option<Option<f64>>,
    bouncer: Bouncer,
) -> DVec3 {
    let mut restitution = if bouncer.suppressing {
        0.0
    } else {
        bouncer.bounciness
    };
    let mut after = current;
    if x {
        after.x = -current.x * restitution;
    }
    if z {
        after.z = -current.z * restitution;
    }
    if y {
        if let Some(block_bounce) = landing {
            // 26.3 (777) stopped bouncing a landing at exactly gravity's speed.
            let too_slow = if crate::version::session_protocol() >= 777 {
                -current.y <= bouncer.gravity
            } else {
                -current.y < bouncer.gravity
            };
            restitution = match block_bounce {
                Some(block) if !too_slow && !bouncer.suppressing => restitution.max(block),
                _ => 0.0,
            };
        }
        let (gravity_compensation, drag) = if restitution > 0.0 {
            let portion = movement.y / current.y;
            (
                portion * bouncer.gravity,
                1.0 + portion * (bouncer.air_drag - 1.0),
            )
        } else {
            (0.0, 1.0)
        };
        after.y = (gravity_compensation - current.y) * drag * restitution;
    }
    after
}

fn update_sprint_state(
    player: &mut LocalPlayer,
    input: &InputState,
    forward_pressed: bool,
    slow_due_to_using_item: bool,
    vehicle: Option<Vehicle>,
) {
    // Vanilla `LocalPlayer.aiStep`: `was_*` is the input before this tick's
    // `KeyboardInput.tick`.
    if player.sprint_toggle_timer > 0 {
        player.sprint_toggle_timer -= 1;
    }
    let current_shift = input.performing_action(input::Action::Sneak);
    let current_backward = input.action_held(input::Action::Backward)
        || input
            .analog_movement_axes()
            .map(|vec| vec.y < -input::STICK_MOVEMENT_THRESHOLD)
            .unwrap_or(false);
    if player.was_shift_pressed || slow_due_to_using_item && vehicle.is_none() || current_backward {
        player.sprint_toggle_timer = 0;
    }

    let in_shallow_water = player.in_water && !player.under_water;
    // `LocalPlayer.isSprintingPossible`.
    let may_sprint = !player.effects.has("blindness")
        && vehicle.map_or(
            player.food > SPRINT_HUNGER_THRESHOLD || player.may_fly,
            |v| v.jumpable,
        );
    let sprinting_possible = |allowed_in_shallow_water: bool| {
        may_sprint && (allowed_in_shallow_water || !in_shallow_water)
    };
    let can_start_sprinting = !player.sprinting
        && forward_pressed
        && sprinting_possible(player.flying)
        && !slow_due_to_using_item
        && (!is_moving_slowly(player) || player.under_water);

    if can_start_sprinting {
        if !player.was_forward_pressed {
            if player.sprint_toggle_timer > 0 {
                player.set_sprinting(true);
            } else {
                player.sprint_toggle_timer = player.sprint_window;
            }
        }
        if input.performing_action(input::Action::Sprint) {
            player.set_sprinting(true);
        }
    }

    if player.sprinting {
        if player.swimming {
            // `LocalPlayer.shouldStopSwimSprinting`.
            if !sprinting_possible(true)
                || !player.in_water
                || (!forward_pressed && !player.on_ground && !current_shift)
            {
                player.set_sprinting(false);
            }
        } else {
            let hard_run_collision =
                player.horizontal_collision && !player.minor_horizontal_collision;
            // `LocalPlayer.shouldStopRunSprinting`.
            if !sprinting_possible(player.flying) || !forward_pressed || hard_run_collision {
                player.set_sprinting(false);
            }
        }
    }
}

/// `LocalPlayer.aiStep`'s private `crouching` field, from last tick's Shift;
/// distinct from the physical pose.
fn update_movement_crouching_state(
    player: &mut LocalPlayer,
    world: CollisionWorld<'_>,
    is_passenger: bool,
) {
    let descending = player.was_shift_pressed;
    let crouch_fits = can_fit_with_height(world, player, CROUCH_HEIGHT, descending);
    let stand_fits = can_fit_with_height(world, player, STANDING_HEIGHT, descending);
    let shift_or_forced_crouch =
        player.was_shift_pressed || (player.sleeping_pos.is_none() && !stand_fits);

    player.movement_crouching = !player.flying
        && !player.swimming
        && !is_passenger
        && crouch_fits
        && shift_or_forced_crouch;
}

// Vanilla `Player.updatePlayerPose`: an unfitting desired pose falls back to
// crouching, then swimming; spectators and passengers take it directly.
// Sleeping/fall-flying aren't simulated.
fn update_player_pose(
    player: &mut LocalPlayer,
    input: &InputState,
    world: CollisionWorld<'_>,
    is_passenger: bool,
) {
    let descending = input.performing_action(input::Action::Sneak);
    if !can_fit_with_height(world, player, crate::player::SWIMMING_HEIGHT, descending) {
        return;
    }

    let wants_crouch = !player.flying && descending;
    let desired_height = if wants_crouch {
        CROUCH_HEIGHT
    } else {
        STANDING_HEIGHT
    };
    let (swimming_pose, crouching) = if player.swimming {
        (true, false)
    } else if is_passenger
        || player.game_mode == 3
        || can_fit_with_height(world, player, desired_height, descending)
    {
        (false, wants_crouch)
    } else if can_fit_with_height(world, player, CROUCH_HEIGHT, descending) {
        (false, true)
    } else {
        (true, false)
    };
    player.swimming_pose = swimming_pose;
    player.crouching = crouching;
}

fn can_fit_with_height(
    world: CollisionWorld<'_>,
    player: &LocalPlayer,
    height: f64,
    descending: bool,
) -> bool {
    no_collision(
        world,
        &Aabb::from_center(player.position.into(), PLAYER_HALF_WIDTH, height / 2.0).deflate(1.0e-7),
        &collision_context(player, descending),
    )
}

// While holding shift on the ground, clamp the horizontal move so the player
// can't fall further than the step height.
fn back_off_from_edge(
    world: CollisionWorld<'_>,
    player: &LocalPlayer,
    delta: DVec3,
    shift_down: bool,
    ctx: &CollisionContext,
) -> DVec3 {
    if !shift_down || player.flying || delta.y > 0.0 {
        return delta;
    }
    let bb = &player.bounding_box();
    let can_fall = |dx, dz, min_height| can_fall_at_least(world, bb, dx, dz, min_height, ctx);
    // `Player.isAboveGround`.
    let max_down_step = f64::from(step_height(player));
    let above_ground = player.on_ground
        || (player.fall_distance < max_down_step
            && !can_fall(0.0, 0.0, max_down_step - player.fall_distance));
    if !above_ground {
        return delta;
    }

    let mut dx = delta.x;
    let mut dz = delta.z;
    let step_x = dx.signum() * 0.05;
    let step_z = dz.signum() * 0.05;

    while dx != 0.0 && can_fall(dx, 0.0, max_down_step) {
        if dx.abs() <= 0.05 {
            dx = 0.0;
            break;
        }
        dx -= step_x;
    }
    while dz != 0.0 && can_fall(0.0, dz, max_down_step) {
        if dz.abs() <= 0.05 {
            dz = 0.0;
            break;
        }
        dz -= step_z;
    }
    while dx != 0.0 && dz != 0.0 && can_fall(dx, dz, max_down_step) {
        dx = if dx.abs() <= 0.05 { 0.0 } else { dx - step_x };
        if dz.abs() <= 0.05 {
            dz = 0.0;
            continue;
        }
        dz -= step_z;
    }

    dvec3(dx, delta.y, dz)
}

fn can_fall_at_least(
    world: CollisionWorld<'_>,
    bb: &Aabb,
    dx: f64,
    dz: f64,
    min_height: f64,
    ctx: &CollisionContext,
) -> bool {
    no_collision(
        world,
        &Aabb::new(
            dvec3(
                bb.min.x + 1.0e-7 + dx,
                bb.min.y - min_height - 1.0e-7,
                bb.min.z + 1.0e-7 + dz,
            ),
            dvec3(bb.max.x - 1.0e-7 + dx, bb.min.y, bb.max.z - 1.0e-7 + dz),
        ),
        ctx,
    )
}

/// `Player.getSpeed`; sprinting is a modifier on the attribute
/// (`LocalPlayer::set_sprinting`).
fn movement_speed(player: &LocalPlayer) -> f32 {
    player.attribute(AttributeKind::MovementSpeed) as f32
}

/// `LivingEntity.computeModifiedFriction`: a friction or drag scaled by its
/// attribute, which leaves it alone at 1.
fn modified_friction(friction: f32, modifier: f32) -> f32 {
    (1.0 - (1.0 - friction) * modifier).clamp(0.0, 1.0)
}

/// `drag` under the player's `air_drag_modifier`; with [`VERTICAL_DRAG`],
/// `LivingEntity.getAirDrag`.
fn air_drag(player: &LocalPlayer, drag: f32) -> f32 {
    modified_friction(
        drag,
        player.attribute(AttributeKind::AirDragModifier) as f32,
    )
}

/// `LivingEntity.getDefaultGravity`.
// TODO: `getEffectiveGravity` caps it at 0.01 while falling with slow falling.
fn gravity(player: &LocalPlayer) -> f64 {
    player.attribute(AttributeKind::Gravity)
}

/// `LivingEntity.maxUpStep`.
fn step_height(player: &LocalPlayer) -> f32 {
    player.attribute(AttributeKind::StepHeight) as f32
}

/// `LocalPlayer.modifyInput`'s sneaking factor.
fn sneaking_speed(player: &LocalPlayer) -> f32 {
    player.attribute(AttributeKind::SneakingSpeed) as f32
}

/// Vanilla `Entity.moveRelative`.
fn move_relative(
    player: &mut LocalPlayer,
    forward: f32,
    strafe: f32,
    speed: f32,
    sin_y_rot: f32,
    cos_y_rot: f32,
) {
    let (x, z) = movement_delta(forward, strafe, speed, sin_y_rot, cos_y_rot);
    player.velocity.x += x;
    player.velocity.z += z;
}

fn movement_delta(
    forward: f32,
    strafe: f32,
    speed: f32,
    sin_y_rot: f32,
    cos_y_rot: f32,
) -> (f64, f64) {
    // Vanilla constructs Vec3 from the float xxa/zza fields, then performs the
    // normalization and speed scaling in double precision.
    let mut x = f64::from(strafe);
    let mut z = f64::from(forward);
    let length_sq = x * x + z * z;
    if length_sq < 1.0e-7 {
        return (0.0, 0.0);
    }
    if length_sq > 1.0 {
        // `Vec3.normalize` divides; a reciprocal multiply lands an ULP off.
        let length = length_sq.sqrt();
        x /= length;
        z /= length;
    }
    let speed = f64::from(speed);
    x *= speed;
    z *= speed;

    let sin = f64::from(sin_y_rot);
    let cos = f64::from(cos_y_rot);
    (x * cos - z * sin, z * cos + x * sin)
}

fn world_input_direction(forward: f32, strafe: f32, sin_y_rot: f32, cos_y_rot: f32) -> (f64, f64) {
    let forward = f64::from(forward);
    let strafe = f64::from(strafe);
    let sin = f64::from(sin_y_rot);
    let cos = f64::from(cos_y_rot);
    (strafe * cos - forward * sin, forward * cos + strafe * sin)
}

/// `CollisionContext.of(player)`.
fn collision_context(player: &LocalPlayer, descending: bool) -> CollisionContext {
    CollisionContext::entity(
        player.position.y,
        descending,
        player.inventory.wears_leather_boots(),
    )
    .with_fall_distance(player.fall_distance)
}

fn block_state_at_player_position(
    player: &LocalPlayer,
    chunk_store: &ChunkStore,
) -> pomme_block::BlockState {
    chunk_store.get_block_state(
        player.position.x.floor() as i32,
        player.position.y.floor() as i32,
        player.position.z.floor() as i32,
    )
}

/// `Entity.getBlockPosBelowThatAffectsMyMovement`'s block.
fn block_below_affecting_movement(
    player: &LocalPlayer,
    world: CollisionWorld<'_>,
    tags: &BlockTags,
) -> pomme_block::BlockState {
    let pos = on_pos(player, world, tags, 0.500_001);
    world.chunks.get_block_state(pos.x, pos.y, pos.z)
}

pub(super) fn on_pos(
    player: &LocalPlayer,
    world: CollisionWorld<'_>,
    tags: &BlockTags,
    offset: f32,
) -> BlockPos {
    on_pos_of(
        player.position.into(),
        player.main_supporting_block_pos,
        world,
        tags,
        offset,
    )
}

/// Vanilla `Entity.getOnPos(offset)`: the supporting block's column `offset`
/// below the feet, or the support itself for a near-zero offset, a fence
/// (offsets up to 0.5), a wall or a fence gate.
pub(crate) fn on_pos_of(
    position: DVec3,
    support: Option<BlockPos>,
    world: CollisionWorld<'_>,
    tags: &BlockTags,
    offset: f32,
) -> BlockPos {
    let y = (position.y - f64::from(offset)).floor() as i32;
    let Some(support) = support else {
        return BlockPos::new(position.x.floor() as i32, y, position.z.floor() as i32);
    };
    if offset <= 1.0e-5 {
        return support;
    }
    let support_id = world.block_data(support).id();
    if (f64::from(offset) <= 0.5 && tags.contains("minecraft:fences", support_id))
        || tags.contains("minecraft:walls", support_id)
        || support_id.ends_with("_fence_gate")
    {
        return support;
    }
    BlockPos::new(support.x, y, support.z)
}

fn update_supporting_block(
    player: &mut LocalPlayer,
    world: CollisionWorld<'_>,
    on_ground: bool,
    movement: DVec3,
    ctx: &CollisionContext,
) {
    let bounding_box = player.bounding_box();
    check_supporting_block(
        &mut player.main_supporting_block_pos,
        &mut player.on_ground_no_blocks,
        world,
        player.position.into(),
        &bounding_box,
        on_ground,
        movement,
        ctx,
    );
}

/// Vanilla `Entity.checkSupportingBlock`: the block under `bounding_box`
/// that holds it up, kept in `support`, with `no_blocks` set when it stands
/// on no block (an entity, say).
#[allow(clippy::too_many_arguments)]
pub(crate) fn check_supporting_block(
    support: &mut Option<BlockPos>,
    no_blocks: &mut bool,
    world: CollisionWorld<'_>,
    position: DVec3,
    bounding_box: &Aabb,
    on_ground: bool,
    movement: DVec3,
    ctx: &CollisionContext,
) {
    if !on_ground {
        *no_blocks = false;
        *support = None;
        return;
    }

    let test_area = Aabb::new(
        dvec3(
            bounding_box.min.x,
            bounding_box.min.y - 1.0e-6,
            bounding_box.min.z,
        ),
        dvec3(bounding_box.max.x, bounding_box.min.y, bounding_box.max.z),
    );
    let mut found = find_supporting_block(world, &test_area, position, ctx);
    if found.is_none() && !*no_blocks {
        let previous_test_area = test_area.offset(dvec3(-movement.x, 0.0, -movement.z));
        found = find_supporting_block(world, &previous_test_area, position, ctx);
    }
    *support = found;
    *no_blocks = found.is_none();
}

fn block_jump_factor(player: &LocalPlayer, world: CollisionWorld<'_>, tags: &BlockTags) -> f32 {
    let here_factor = world
        .block_state_table
        .block_state_data(block_state_at_player_position(player, world.chunks))
        .movement_jump_factor();
    if f64::from(here_factor) == 1.0 {
        world
            .block_state_table
            .block_state_data(block_below_affecting_movement(player, world, tags))
            .movement_jump_factor()
    } else {
        here_factor
    }
}

/// `Player.getBlockSpeedFactor` over `LivingEntity`'s, which
/// MOVEMENT_EFFICIENCY lerps toward 1; pomme has no fall flying.
fn block_speed_factor(player: &LocalPlayer, world: CollisionWorld<'_>, tags: &BlockTags) -> f32 {
    if player.flying {
        return 1.0;
    }
    let factor = entity_block_speed_factor(
        player.position.into(),
        player.main_supporting_block_pos,
        world,
        tags,
    );
    let efficiency = player.attribute(AttributeKind::MovementEfficiency) as f32;
    factor + efficiency * (1.0 - factor)
}

/// `Entity.getBlockSpeedFactor`.
pub(crate) fn entity_block_speed_factor(
    position: DVec3,
    support: Option<BlockPos>,
    world: CollisionWorld<'_>,
    tags: &BlockTags,
) -> f32 {
    let here = BlockPos::containing(position);
    let block_state_data = world.block_data(here);
    let here_factor = block_state_data.movement_speed_factor();
    if matches!(block_state_data.id(), "water" | "bubble_column") || f64::from(here_factor) != 1.0 {
        here_factor
    } else {
        let below = on_pos_of(position, support, world, tags, 0.500_001);
        world.block_data(below).movement_speed_factor()
    }
}

fn friction_influenced_speed(speed: f32, player: &LocalPlayer, block_friction: f32) -> f32 {
    if player.on_ground {
        // Vanilla deliberately compares the widened float friction against the
        // double literal 0.6. Normal-block 0.6f therefore enters this branch.
        if f64::from(block_friction) > 0.6 {
            speed * (GROUND_ACCEL_FACTOR / (block_friction * block_friction * block_friction))
        } else {
            speed
        }
    } else if player.flying {
        // Vanilla Player.getFlyingSpeed.
        if player.sprinting {
            player.fly_speed * 2.0_f32
        } else {
            player.fly_speed
        }
    } else if player.sprinting {
        SPRINT_AIR_ACCELERATION
    } else {
        AIR_ACCELERATION
    }
}

fn is_minor_horizontal_collision(
    forward: f32,
    strafe: f32,
    sin_y_rot: f32,
    cos_y_rot: f32,
    resolved: DVec3,
) -> bool {
    let (intent_x, intent_z) = world_input_direction(forward, strafe, sin_y_rot, cos_y_rot);
    let intent_len_sq = intent_x * intent_x + intent_z * intent_z;
    let resolved_len_sq = resolved.x * resolved.x + resolved.z * resolved.z;
    if intent_len_sq < f64::from(1.0e-5_f32) || resolved_len_sq < f64::from(1.0e-5_f32) {
        return false;
    }
    let dot = intent_x * resolved.x + intent_z * resolved.z;
    let angle = (dot / (intent_len_sq * resolved_len_sq).sqrt()).acos();
    angle < MINOR_COLLISION_ANGLE
}

fn is_moving_slowly(player: &LocalPlayer) -> bool {
    // `LocalPlayer.isMovingSlowly`: the private crouching flag, or visually
    // crawling (swimming pose without the swimming flag).
    player.movement_crouching || (player.swimming_pose && !player.in_water)
}

/// `ClientInput.moveVector` as `(left, forward)`. Pomme's analog stick is
/// already clamped to unit length; keys normalize like `KeyboardInput.tick`.
fn move_vector(input: &InputState) -> (f32, f32) {
    if let Some(analog) = input.analog_movement_axes() {
        (analog.x, analog.y)
    } else {
        let mut forward = 0.0_f32;
        let mut strafe = 0.0_f32;
        if input.action_held(input::Action::Forward) {
            forward += 1.0;
        }
        if input.action_held(input::Action::Backward) {
            forward -= 1.0;
        }
        if input.action_held(input::Action::StrafeLeft) {
            strafe += 1.0;
        }
        if input.action_held(input::Action::StrafeRight) {
            strafe -= 1.0;
        }
        normalize_vec2(strafe, forward)
    }
}

/// `ClientInput.hasForwardImpulse`.
fn has_forward_impulse(input: &InputState) -> bool {
    move_vector(input).1 > 1.0e-5
}

/// `strafe` follows vanilla xxa: positive is left.
fn movement_input(
    input: &InputState,
    sneaking_speed: Option<f32>,
    use_speed_multiplier: f32,
) -> (f32, f32) {
    let (mut strafe, mut forward) = move_vector(input);
    if strafe == 0.0 && forward == 0.0 {
        return (forward, strafe);
    }

    strafe *= INPUT_DAMPING;
    forward *= INPUT_DAMPING;
    strafe *= use_speed_multiplier;
    forward *= use_speed_multiplier;

    if let Some(sneaking_speed) = sneaking_speed {
        strafe *= sneaking_speed;
        forward *= sneaking_speed;
    }

    let (strafe, forward) = square_movement(strafe, forward);
    (forward, strafe)
}

fn normalize_vec2(x: f32, y: f32) -> (f32, f32) {
    let length = mth_sqrt(x * x + y * y);
    if length < 1.0e-4_f32 {
        return (0.0, 0.0);
    }
    (x / length, y / length)
}

fn square_movement(x: f32, y: f32) -> (f32, f32) {
    let length = mth_sqrt(x * x + y * y);
    if length <= 0.0 {
        return (x, y);
    }
    let inv_len = 1.0_f32 / length;
    let dir_x = x * inv_len;
    let dir_y = y * inv_len;
    let abs_x = dir_x.abs();
    let abs_y = dir_y.abs();
    let tan = if abs_y > abs_x {
        abs_x / abs_y
    } else {
        abs_y / abs_x
    };
    let distance_to_square = mth_sqrt(1.0_f32 + tan * tan);
    let modified_length = (length * distance_to_square).min(1.0_f32);
    (dir_x * modified_length, dir_y * modified_length)
}

fn normalize_tiny_velocity(velocity: &mut Velocity) {
    if velocity.x * velocity.x + velocity.z * velocity.z < 9.0e-6 {
        velocity.x = 0.0;
        velocity.z = 0.0;
    }
    if velocity.y.abs() < 0.003 {
        velocity.y = 0.0;
    }
}

fn mth_equal(a: f64, b: f64) -> bool {
    (b - a).abs() < MTH_EQUAL_EPSILON
}

fn mth_sqrt(value: f32) -> f32 {
    f64::from(value).sqrt() as f32
}

pub(crate) fn mth_sin(angle: f32) -> f32 {
    let index = ((f64::from(angle) * SIN_SCALE) as i64 & 0xFFFF) as usize;
    SIN[index]
}

pub(crate) fn mth_cos(angle: f32) -> f32 {
    let index = ((f64::from(angle) * SIN_SCALE + 16_384.0) as i64 & 0xFFFF) as usize;
    SIN[index]
}

fn vanilla_yaw_sin_cos(yaw_degrees: f32) -> (f32, f32) {
    let angle = yaw_degrees * DEG_TO_RAD;
    (mth_sin(angle), mth_cos(angle))
}

fn vanilla_look_y(pitch_degrees: f32) -> f64 {
    let angle = pitch_degrees * DEG_TO_RAD;
    -f64::from(mth_sin(angle))
}

/// What a piston's move of the player reads: the world and its tags, whether
/// the player is descending (holding sneak), and the tick for the 0.51
/// budget.
#[derive(Clone, Copy)]
struct PistonMove<'a> {
    world: CollisionWorld<'a>,
    tags: &'a BlockTags,
    descending: bool,
    tick: u64,
}

/// Vanilla `Entity.limitPistonMovement`: a piston moves an entity along one
/// axis, at most 0.51 per axis per game tick however many pistons push it.
fn limit_piston_movement(player: &mut LocalPlayer, delta: DVec3, tick: u64) -> DVec3 {
    if delta.length_squared() <= 1.0e-7 {
        return delta;
    }
    // Not the game time, which pomme advances outside the tick loop.
    if tick != player.piston_deltas_tick {
        player.piston_deltas = [0.0; 3];
        player.piston_deltas_tick = tick;
    }
    let Some(axis) = (0..3).find(|&axis| delta[axis] != 0.0) else {
        return DVec3::ZERO;
    };
    let total = (delta[axis] + player.piston_deltas[axis]).clamp(-0.51, 0.51);
    let amount = total - player.piston_deltas[axis];
    player.piston_deltas[axis] = total;
    if amount.abs() <= f64::from(1.0e-5_f32) {
        return DVec3::ZERO;
    }
    let mut out = DVec3::ZERO;
    out[axis] = amount;
    out
}

/// Vanilla `Entity.move` with `MoverType.PISTON`: no edge back-off and no
/// stuck-speed slowdown, but the usual collision.
// TODO: `addMovementThisTick` for `applyEffectsFromBlocks`, which the
// player's own move doesn't do yet either.
fn move_by_piston(player: &mut LocalPlayer, mv: PistonMove<'_>, delta: DVec3) {
    let delta = limit_piston_movement(player, delta, mv.tick);
    if delta == DVec3::ZERO {
        return;
    }
    let ctx = collision_context(player, mv.descending);
    let (resolved, on_ground) = resolve_collision(
        mv.world,
        player.bounding_box(),
        delta.into(),
        f64::from(step_height(player)),
        player.on_ground,
        &ctx,
    );
    let collided = collided_axes(delta, resolved);
    let moved = resolved.length_squared();
    if moved > 1.0e-7 || delta.length_squared() - moved < 1.0e-7 {
        player.position += resolved;
    }
    player.horizontal_collision = collided[0] || collided[2];
    let (sin_y_rot, cos_y_rot) = vanilla_yaw_sin_cos(player.look_dir.y_rot_deg());
    player.minor_horizontal_collision = player.horizontal_collision
        && is_minor_horizontal_collision(
            player.move_forward,
            player.move_strafe,
            sin_y_rot,
            cos_y_rot,
            resolved,
        );
    let moved = settle_after_move(
        player, mv.world, mv.tags, resolved, on_ground, collided, &ctx,
    );
    movement_sound::emit(player, mv.world, mv.tags, mv.descending, moved);
    let speed_factor = f64::from(block_speed_factor(player, mv.world, mv.tags));
    player.velocity.x *= speed_factor;
    player.velocity.z *= speed_factor;
}

/// Vanilla `PistonMovingBlockEntity.moveEntityByPiston`: `piston_direction`
/// stops its own moving blocks colliding while the entity moves.
fn move_entity_by_piston(
    player: &mut LocalPlayer,
    mv: PistonMove<'_>,
    piston_direction: Direction,
    delta: f64,
    movement: Direction,
) {
    let world = CollisionWorld {
        piston_noclip: Some(piston_direction),
        ..mv.world
    };
    move_by_piston(player, PistonMove { world, ..mv }, movement.step() * delta);
}

/// Vanilla `PistonMovingBlockEntity.getMovement`: how far `aabb` must move
/// along `movement` to clear `outside`.
fn piston_movement(outside: &Aabb, movement: Direction, aabb: &Aabb) -> f64 {
    match movement {
        Direction::East => outside.max.x - aabb.min.x,
        Direction::West => aabb.max.x - outside.min.x,
        Direction::Up => outside.max.y - aabb.min.y,
        Direction::Down => aabb.max.y - outside.min.y,
        Direction::South => outside.max.z - aabb.min.z,
        Direction::North => aabb.max.z - outside.min.z,
    }
}

/// Vanilla `PistonMath.getMovementArea`: the slab `aabb`'s leading face
/// sweeps when it moves `amount` along `direction`.
fn movement_area(aabb: &Aabb, direction: Direction, amount: f64) -> Aabb {
    let step = direction.step();
    let axis = direction.axis();
    let delta = amount * step[axis];
    let face = if step[axis] > 0.0 {
        aabb.max[axis]
    } else {
        aabb.min[axis]
    };
    let (mut min, mut max) = (aabb.min, aabb.max);
    min[axis] = face + delta.min(0.0);
    max[axis] = face + delta.max(0.0);
    Aabb::new(min, max)
}

/// Vanilla `PistonMovingBlockEntity.tick`'s entity moves for the local player:
/// `moveCollidedEntities` (with its slime bounce and the source piston's
/// `fixEntityWithinPistonBase`), then `moveStuckEntities` for honey. Runs
/// before the moving block's progress reaches `new_progress`.
pub fn push_by_moving_piston(
    player: &mut LocalPlayer,
    world: CollisionWorld<'_>,
    tags: &BlockTags,
    pos: BlockPos,
    piston: &MovingPiston,
    new_progress: f32,
    tick: u64,
) {
    // `getEntities`' `NO_SPECTATORS`; honey's own filter needs `onGround`,
    // which `Player.tick` clears for spectators.
    if is_spectator(player.game_mode) {
        return;
    }
    let blocks = world.block_state_table;
    let mv = PistonMove {
        world,
        tags,
        descending: player.was_shift_pressed,
        tick,
    };
    let moved_id = blocks.block_state_data(piston.moved_state).id();
    let movement = piston.movement_direction();
    let delta_progress = f64::from(new_progress - piston.progress);
    let empty = &CollisionContext::EMPTY;

    // `moveCollidedEntities`.
    let mut shape = Vec::new();
    state_boxes(
        blocks,
        collision_related_state(blocks, piston),
        pos,
        empty,
        piston.extension(),
        &mut shape,
    );
    if let Some(aabb) = shape.iter().copied().reduce(|a, b| a.minmax(&b))
        && player
            .bounding_box()
            .intersects(&movement_area(&aabb, movement, delta_progress).minmax(&aabb))
    {
        if moved_id == "slime_block" {
            player.velocity[movement.axis()] = movement.step()[movement.axis()];
        }
        let mut delta = 0.0_f64;
        for part in &shape {
            let moving = movement_area(part, movement, delta_progress);
            let entity = player.bounding_box();
            if moving.intersects(&entity) {
                delta = delta.max(piston_movement(&moving, movement, &entity));
                if delta >= delta_progress {
                    break;
                }
            }
        }
        if delta > 0.0 {
            let delta = delta.min(delta_progress) + 0.01;
            move_entity_by_piston(player, mv, movement, delta, movement);
            if !piston.extending && piston.source {
                fix_entity_within_piston_base(player, mv, pos, movement, delta_progress);
            }
        }
    }

    // `moveStuckEntities`.
    if moved_id == "honey_block" && !matches!(movement, Direction::Up | Direction::Down) {
        let mut honey = Vec::new();
        state_boxes(
            blocks,
            piston.moved_state,
            pos,
            empty,
            DVec3::ZERO,
            &mut honey,
        );
        let cell = Aabb::block(pos.x, pos.y, pos.z);
        let top = honey.iter().map(|b| b.max.y).fold(cell.min.y, f64::max);
        let aabb = Aabb::new(
            dvec3(cell.min.x, top, cell.min.z),
            dvec3(cell.max.x, cell.min.y + 1.500_001, cell.max.z),
        )
        .offset(piston.extension());
        let p = player.position;
        let sticks = player.on_ground
            && (player.main_supporting_block_pos == Some(pos)
                || p.x >= aabb.min.x
                    && p.x <= aabb.max.x
                    && p.z >= aabb.min.z
                    && p.z <= aabb.max.z);
        if sticks && player.bounding_box().intersects(&aabb) {
            move_entity_by_piston(player, mv, movement, delta_progress, movement);
        }
    }
}

/// Vanilla `getCollisionRelatedBlockState`: a retracting piston's base
/// collides as its head.
fn collision_related_state(blocks: BlockStateTable, piston: &MovingPiston) -> BlockState {
    let moved = piston.moved_state;
    if !piston.extending && piston.source && piston::is_piston_base(blocks, moved) {
        return piston::head_of(blocks, moved, piston.progress > 0.25).unwrap_or(moved);
    }
    moved
}

/// Vanilla `PistonMovingBlockEntity.fixEntityWithinPistonBase`: an entity
/// caught in the base cell of a retracting piston is pushed out the front.
fn fix_entity_within_piston_base(
    player: &mut LocalPlayer,
    mv: PistonMove<'_>,
    pos: BlockPos,
    direction: Direction,
    delta_progress: f64,
) {
    let entity = player.bounding_box();
    let cell = Aabb::block(pos.x, pos.y, pos.z);
    if !entity.intersects(&cell) {
        return;
    }
    let opposite = direction.opposite();
    let delta = piston_movement(&cell, opposite, &entity) + 0.01;
    let delta_intersected = piston_movement(&cell, opposite, &entity.intersect(&cell)) + 0.01;
    if (delta - delta_intersected).abs() < 0.01 {
        let delta = delta.min(delta_progress) + 0.01;
        move_entity_by_piston(player, mv, direction, delta, opposite);
    }
}

#[cfg(test)]
mod tests {
    use winit::keyboard::KeyCode;

    use super::*;

    /// The player's defaults for the attributes the physics reads.
    const GRAVITY: f64 = 0.08;
    const JUMP_VELOCITY: f32 = 0.42;
    const SNEAKING_SPEED: f32 = 0.3_f64 as f32;
    use crate::player::{CROUCH_EYE_HEIGHT, STANDING_EYE_HEIGHT};
    use crate::world::block_entity_anim::BlockEntityAnimStore;

    fn test_world<'a>(
        chunks: &'a ChunkStore,
        anim: &'a BlockEntityAnimStore,
    ) -> CollisionWorld<'a> {
        CollisionWorld::with_block_entities(chunks, BlockStateTable::native(), anim)
    }

    #[test]
    fn a_modifier_of_one_leaves_friction_and_drag_alone() {
        for friction in [0.6_f32, 0.8, 0.98, 0.989, HORIZONTAL_DRAG, VERTICAL_DRAG] {
            assert_eq!(
                modified_friction(friction, 1.0).to_bits(),
                friction.to_bits()
            );
        }
        assert_eq!(modified_friction(0.6, 0.5), 0.8);
        assert_eq!(modified_friction(0.6, 0.0), 1.0);
        assert_eq!(modified_friction(0.6, 2048.0), 0.0);
    }

    #[test]
    fn a_landing_bounces_by_the_block_unless_slow_or_sneaking() {
        let land = |vy: f64, landing, suppressing| {
            let bouncer = Bouncer {
                bounciness: 0.0,
                gravity: GRAVITY,
                air_drag: f64::from(VERTICAL_DRAG),
                suppressing,
            };
            let moved = dvec3(0.0, vy * 0.4, 0.0);
            restitute_after_collisions(
                dvec3(0.3, vy, 0.0),
                [true, true, false],
                moved,
                landing,
                bouncer,
            )
        };
        // Stone: both collided axes stop.
        assert_eq!(land(-0.5, Some(Some(0.0)), false), DVec3::ZERO);
        // Slime: back up at the landing speed, with the gravity and drag of
        // the part of the tick spent falling given back.
        let bounce = (0.4 * GRAVITY + 0.5) * (1.0 + 0.4 * (f64::from(VERTICAL_DRAG) - 1.0));
        assert_eq!(land(-0.5, Some(Some(1.0)), false).y, bounce);
        assert_eq!(land(-0.5, Some(Some(0.75)), false).y, bounce * 0.75);
        // Slower than gravity, sneaking, on honey, or against a ceiling.
        assert_eq!(land(-0.07, Some(Some(1.0)), false).y, 0.0);
        assert_eq!(land(-0.5, Some(Some(1.0)), true).y, 0.0);
        assert_eq!(land(-0.5, Some(None), false).y, 0.0);
        assert_eq!(land(0.5, None, false).y, 0.0);
    }

    #[test]
    fn player_width_stops_at_negative_two_block_face() {
        let block = Aabb::block(0, 0, -2);
        let player = Aabb::from_center(
            dvec3(0.5, 0.0, block.min.z - PLAYER_HALF_WIDTH),
            PLAYER_HALF_WIDTH,
            STANDING_HEIGHT / 2.0,
        );

        assert_eq!(player.max.z, block.min.z);
        assert_eq!(block.clip_z_collide(&player, 0.1), 0.0);
    }

    #[test]
    fn float_derived_width_reconstructs_problem_block_faces_exactly() {
        // These are the sparse integer boundaries where the old direct f64
        // literal `0.3` could reconstruct one ULP inside the block face.
        for block_coord in [-2, -32, -512, -8192, -131_072, -2_097_152] {
            let face = f64::from(block_coord);
            assert_eq!((face - PLAYER_HALF_WIDTH) + PLAYER_HALF_WIDTH, face);
        }
        for block_coord in [2, 32, 512, 8192, 131_072, 2_097_152] {
            let face = f64::from(block_coord);
            assert_eq!((face + PLAYER_HALF_WIDTH) - PLAYER_HALF_WIDTH, face);
        }
    }

    #[test]
    fn player_dimensions_match_vanilla_float_widening() {
        assert_eq!(PLAYER_HALF_WIDTH.to_bits(), 0x3fd3333340000000);
        assert_eq!(STANDING_HEIGHT.to_bits(), 0x3ffcccccc0000000);
        assert_eq!(CROUCH_HEIGHT.to_bits(), 1.5_f64.to_bits());
        assert_eq!(STANDING_EYE_HEIGHT.to_bits(), 0x3fcf5c29);
        assert_eq!(CROUCH_EYE_HEIGHT.to_bits(), 0x3fa28f5c);
    }

    #[test]
    fn movement_constants_match_vanilla_value_types() {
        assert_eq!(JUMP_VELOCITY.to_bits(), 0x3ed70a3d);
        assert_eq!(HORIZONTAL_DRAG.to_bits(), 0x3f68f5c3);
        assert_eq!(0.6_f32.to_bits(), 0x3f19999a);
        assert_eq!((0.6_f32 * HORIZONTAL_DRAG).to_bits(), 0x3f0bc6a9);
        assert_eq!(GROUND_ACCEL_FACTOR.to_bits(), 0x3e5d2f1c);
        assert_eq!((GRAVITY / 16.0).to_bits(), 0x3f747ae147ae147b);
        assert_eq!(MTH_EQUAL_EPSILON.to_bits(), 0x3ee4f8b580000000);
        assert_eq!(MINOR_COLLISION_ANGLE.to_bits(), 0x3fc1df46a0000000);
    }

    /// An open north-facing trapdoor at (8, 64, 8) over a matching ladder.
    fn trapdoor_ladder_store(waterlogged: bool) -> ChunkStore {
        let chunks = ChunkStore::with_origin_chunk();
        let waterlogged = if waterlogged { "true" } else { "false" };
        let trapdoor = [
            ("open", "true"),
            ("facing", "north"),
            ("waterlogged", waterlogged),
        ];
        chunks.set_block_state(
            8,
            64,
            8,
            BlockStateTable::native().find_state("oak_trapdoor", &trapdoor),
            BlockStateTable::native(),
        );
        chunks.set_block_state(
            8,
            63,
            8,
            BlockStateTable::native().find_state("ladder", &[("facing", "north")]),
            BlockStateTable::native(),
        );
        chunks
    }

    #[test]
    fn climbable_membership_comes_from_synchronized_tags() {
        let tags = crate::world::block::block_tags_for_test(
            BlockStateTable::native(),
            &[("climbable", &["stone"])],
        );
        let stone = BlockStateTable::native().find_state("stone", &[]);
        let ladder = BlockStateTable::native().find_state("ladder", &[("facing", "north")]);

        assert!(is_climbable_state(
            BlockStateTable::native().block_state_data(stone),
            &tags
        ));
        assert!(!is_climbable_state(
            BlockStateTable::native().block_state_data(ladder),
            &tags
        ));
    }

    #[test]
    fn open_matching_trapdoor_continues_ladder() {
        let north_ladder = BlockStateTable::native().find_state("ladder", &[("facing", "north")]);
        let south_ladder = BlockStateTable::native().find_state("ladder", &[("facing", "south")]);
        let open_north = BlockStateTable::native()
            .find_state("oak_trapdoor", &[("open", "true"), ("facing", "north")]);
        let open_south = BlockStateTable::native()
            .find_state("oak_trapdoor", &[("open", "true"), ("facing", "south")]);
        let closed_north = BlockStateTable::native()
            .find_state("oak_trapdoor", &[("open", "false"), ("facing", "north")]);

        assert!(trapdoor_usable_as_ladder(
            BlockStateTable::native().block_state_data(open_north),
            BlockStateTable::native().block_state_data(north_ladder)
        ));
        assert!(!trapdoor_usable_as_ladder(
            BlockStateTable::native().block_state_data(open_north),
            BlockStateTable::native().block_state_data(south_ladder)
        ));
        assert!(!trapdoor_usable_as_ladder(
            BlockStateTable::native().block_state_data(open_south),
            BlockStateTable::native().block_state_data(north_ladder)
        ));
        assert!(!trapdoor_usable_as_ladder(
            BlockStateTable::native().block_state_data(closed_north),
            BlockStateTable::native().block_state_data(north_ladder)
        ));
        assert!(!trapdoor_usable_as_ladder(
            BlockStateTable::native().block_state_data(open_north),
            BlockStateTable::native()
                .block_state_data(BlockStateTable::native().find_state("stone", &[]))
        ));
    }

    #[test]
    fn climbable_handling_clamps_velocity_and_resets_fall_distance() {
        let chunks = trapdoor_ladder_store(false);

        let mut player = LocalPlayer::new();
        player.position = dvec3(8.5, 64.5, 8.5).into();
        player.velocity = Velocity::new(0.5, -0.7, -0.5);
        player.fall_distance = 12.0;

        handle_on_climbable(
            &mut player,
            CollisionWorld::new(&chunks, BlockStateTable::native()),
            &BlockTags::default(),
            false,
        );

        let max = f64::from(CLIMB_MAX_FALL_SPEED);
        assert_eq!(player.velocity, Velocity::new(max, -max, -max));
        assert_eq!(player.fall_distance, 0.0);

        player.velocity.y = -0.1;
        handle_on_climbable(
            &mut player,
            CollisionWorld::new(&chunks, BlockStateTable::native()),
            &BlockTags::default(),
            true,
        );
        assert_eq!(player.velocity.y, 0.0);
    }

    #[test]
    fn scaffolding_does_not_suppress_downward_climbable_motion() {
        let scaffolding =
            BlockStateTable::native().find_state("scaffolding", &[("bottom", "false")]);
        let mut player = LocalPlayer::new();
        player.velocity.y = -0.4;

        apply_climbable_velocity(
            &mut player,
            BlockStateTable::native().block_state_data(scaffolding),
            true,
        );

        assert_eq!(player.velocity.y, -f64::from(CLIMB_MAX_FALL_SPEED));
    }

    #[test]
    fn flying_and_spectator_players_do_not_use_trapdoor_ladder_continuation() {
        let chunks = trapdoor_ladder_store(false);
        let mut player = LocalPlayer::new();
        player.position = dvec3(8.5, 64.5, 8.5).into();

        player.flying = true;
        assert!(
            on_climbable_state(
                &player,
                &chunks,
                BlockStateTable::native(),
                &BlockTags::default()
            )
            .is_none()
        );

        player.flying = false;
        player.game_mode = 3;
        assert!(
            on_climbable_state(
                &player,
                &chunks,
                BlockStateTable::native(),
                &BlockTags::default()
            )
            .is_none()
        );

        player.game_mode = 0;
        assert!(
            on_climbable_state(
                &player,
                &chunks,
                BlockStateTable::native(),
                &BlockTags::default()
            )
            .is_some()
        );
    }

    #[test]
    fn land_travel_clamps_before_collision_then_applies_climb_impulse() {
        let chunks = trapdoor_ladder_store(false);
        chunks.set_block_state(
            9,
            64,
            8,
            BlockStateTable::native().find_state("stone", &[]),
            BlockStateTable::native(),
        );

        let mut player = LocalPlayer::new();
        player.position = dvec3(8.6, 64.5, 8.5).into();
        player.velocity = Velocity::new(0.5, -0.7, -0.5);
        player.fall_distance = 12.0;
        let neutral = InputState::released();

        tick_land(
            &mut player,
            &neutral,
            CollisionWorld::new(&chunks, BlockStateTable::native()),
            &BlockTags::default(),
            false,
            0.0,
            0.0,
            0.0,
            1.0,
        );

        assert!(player.horizontal_collision);
        assert!((player.position.z - (8.5 - f64::from(CLIMB_MAX_FALL_SPEED))).abs() < 1.0e-12);
        let expected_y = (CLIMB_UP_VELOCITY - GRAVITY) * f64::from(VERTICAL_DRAG);
        assert!((player.velocity.y - expected_y).abs() < 1.0e-12);
    }

    #[test]
    fn water_travel_applies_post_collision_climb_impulse() {
        let chunks = trapdoor_ladder_store(true);
        chunks.set_block_state(
            9,
            64,
            8,
            BlockStateTable::native().find_state("stone", &[]),
            BlockStateTable::native(),
        );

        let mut player = LocalPlayer::new();
        player.position = dvec3(8.6, 64.0, 8.5).into();
        player.velocity = Velocity::new(0.5, 0.0, 0.0);
        let neutral = InputState::released();

        tick_water(
            &mut player,
            &neutral,
            CollisionWorld::new(&chunks, BlockStateTable::native()),
            &BlockTags::default(),
            false,
            0.0,
            0.0,
            0.0,
            1.0,
        );

        assert!(player.horizontal_collision);
        let expected_y = CLIMB_UP_VELOCITY * f64::from(WATER_VERTICAL_DRAG) - GRAVITY / 16.0;
        assert!((player.velocity.y - expected_y).abs() < 1.0e-12);
    }

    #[test]
    fn tiny_velocity_cleanup_matches_living_entity_thresholds() {
        let mut below = Velocity::new(0.002, 0.002999, 0.002);
        normalize_tiny_velocity(&mut below);
        assert_eq!(below, Velocity::new(0.0, 0.0, 0.0));

        let mut horizontal_edge = Velocity::new(0.003, 0.003, 0.0);
        normalize_tiny_velocity(&mut horizontal_edge);
        assert_eq!(horizontal_edge.x, 0.003);
        assert_eq!(horizontal_edge.y, 0.003);

        let mut vertical_edge = Velocity::new(0.0, -0.003, 0.0);
        normalize_tiny_velocity(&mut vertical_edge);
        assert_eq!(vertical_edge.y, -0.003);
    }

    #[test]
    fn a_server_update_replaces_the_speed_and_the_sprint_modifier() {
        use crate::attribute::AttributeSnapshot;

        let mut player = LocalPlayer::new();
        player.set_sprinting(true);
        let update = AttributeSnapshot {
            attribute: AttributeKind::MovementSpeed,
            base: f64::from(0.2_f32),
            modifiers: Vec::new(),
        };
        assert!(player.attributes.apply_snapshot(&update));
        // Still flagged sprinting, at the walking speed the server sent,
        // until sprinting is set again.
        assert!(player.sprinting);
        assert_eq!(movement_speed(&player), 0.2);
        player.set_sprinting(true);
        assert_eq!(
            movement_speed(&player),
            (f64::from(0.2_f32) * (1.0 + f64::from(0.3_f32))) as f32
        );
        player.set_sprinting(false);
        assert_eq!(movement_speed(&player), 0.2);
    }

    #[test]
    fn movement_speed_matches_vanilla_attribute_rounding() {
        let mut player = LocalPlayer::new();
        player.on_ground = true;
        for (sprinting, bits) in [(false, 0x3dcccccd), (true, 0x3e051eb9)] {
            player.set_sprinting(sprinting);
            let speed = movement_speed(&player);
            assert_eq!(speed.to_bits(), bits);
            assert_eq!(
                friction_influenced_speed(speed, &player, 0.6_f32).to_bits(),
                bits
            );
        }
    }

    #[test]
    fn keyboard_input_math_matches_vanilla_float_pipeline() {
        let (left, forward) = normalize_vec2(1.0, 1.0);
        let (left, forward) = square_movement(left * INPUT_DAMPING, forward * INPUT_DAMPING);
        assert_eq!(left.to_bits(), 0x3f3504f2);
        assert_eq!(forward.to_bits(), 0x3f3504f2);

        let (left, forward) = normalize_vec2(0.0, 1.0);
        let (left, forward) = square_movement(left * INPUT_DAMPING, forward * INPUT_DAMPING);
        assert_eq!(left.to_bits(), 0x00000000);
        assert_eq!(forward.to_bits(), 0x3f7ae148);
    }

    #[test]
    fn crouch_press_changes_pose_after_current_ticks_unslowed_movement() {
        let chunks = ChunkStore::new(2);
        let block_entity_anim = BlockEntityAnimStore::default();

        let mut shifted = LocalPlayer::new();
        shifted.position = dvec3(0.0, 80.0, 0.0).into();
        let mut plain = LocalPlayer::new();
        plain.position = shifted.position;

        let mut shift_input = InputState::released();
        shift_input.set_key_pressed_for_test(KeyCode::KeyW, true);
        shift_input.set_key_pressed_for_test(KeyCode::ShiftLeft, true);
        let mut plain_input = InputState::released();
        plain_input.set_key_pressed_for_test(KeyCode::KeyW, true);

        tick(
            &mut shifted,
            &shift_input,
            test_world(&chunks, &block_entity_anim),
            &BlockTags::default(),
            1.0,
            false,
            None,
        );
        tick(
            &mut plain,
            &plain_input,
            test_world(&chunks, &block_entity_anim),
            &BlockTags::default(),
            1.0,
            false,
            None,
        );

        assert_eq!(shifted.position, plain.position);
        assert_eq!(shifted.velocity, plain.velocity);
        assert!(
            shifted.crouching,
            "the current Shift press must select the crouch pose at tick end"
        );
        assert!(!plain.crouching);

        let first_shifted_horizontal_speed =
            shifted.velocity.x * shifted.velocity.x + shifted.velocity.z * shifted.velocity.z;
        tick(
            &mut shifted,
            &shift_input,
            test_world(&chunks, &block_entity_anim),
            &BlockTags::default(),
            1.0,
            false,
            None,
        );
        let second_shifted_horizontal_speed =
            shifted.velocity.x * shifted.velocity.x + shifted.velocity.z * shifted.velocity.z;

        let first_plain_horizontal_speed =
            plain.velocity.x * plain.velocity.x + plain.velocity.z * plain.velocity.z;
        tick(
            &mut plain,
            &plain_input,
            test_world(&chunks, &block_entity_anim),
            &BlockTags::default(),
            1.0,
            false,
            None,
        );
        let second_plain_horizontal_speed =
            plain.velocity.x * plain.velocity.x + plain.velocity.z * plain.velocity.z;

        assert_eq!(first_shifted_horizontal_speed, first_plain_horizontal_speed);
        assert!(second_shifted_horizontal_speed < second_plain_horizontal_speed);
        assert!(shifted.movement_crouching);

        // Releasing Shift keeps this tick slowed (last tick's Shift) but
        // stands the pose at tick end.
        tick(
            &mut shifted,
            &plain_input,
            test_world(&chunks, &block_entity_anim),
            &BlockTags::default(),
            1.0,
            false,
            None,
        );
        assert!(shifted.movement_crouching);
        assert!(
            !shifted.crouching,
            "physical pose must stand at the end of the Shift-release tick"
        );

        tick(
            &mut shifted,
            &plain_input,
            test_world(&chunks, &block_entity_anim),
            &BlockTags::default(),
            1.0,
            false,
            None,
        );
        assert!(!shifted.movement_crouching);
    }

    #[test]
    fn passenger_clears_private_crouching_and_uses_desired_pose_directly() {
        let chunks = ChunkStore::with_origin_chunk();
        let mut player = LocalPlayer::new();
        player.position = dvec3(8.5, 64.0, 8.5).into();
        player.was_shift_pressed = true;

        update_movement_crouching_state(
            &mut player,
            CollisionWorld::new(&chunks, BlockStateTable::native()),
            true,
        );
        assert!(
            !player.movement_crouching,
            "LocalPlayer private crouching flag is forced false while passenger"
        );

        // One block of headroom: swimming fits, crouching doesn't.
        let stone = BlockStateTable::native().find_state("stone", &[]);
        chunks.set_block_state(8, 65, 8, stone, BlockStateTable::native());
        let mut input = InputState::released();
        input.set_key_pressed_for_test(KeyCode::ShiftLeft, true);
        update_player_pose(
            &mut player,
            &input,
            CollisionWorld::new(&chunks, BlockStateTable::native()),
            true,
        );
        assert!(player.crouching);
        assert!(!player.swimming_pose);

        player.crouching = false;
        player.swimming_pose = false;
        update_player_pose(
            &mut player,
            &input,
            CollisionWorld::new(&chunks, BlockStateTable::native()),
            false,
        );
        assert!(
            player.swimming_pose,
            "unmounted player falls back to SWIMMING when desired crouch pose does not fit"
        );
    }

    #[test]
    fn private_crouching_flag_drives_slow_input_not_physical_pose() {
        let mut player = LocalPlayer::new();
        player.crouching = true;
        player.movement_crouching = false;
        assert!(
            !is_moving_slowly(&player),
            "physical CROUCHING pose alone is not LocalPlayer.isCrouching()"
        );

        player.crouching = false;
        player.movement_crouching = true;
        assert!(is_moving_slowly(&player));

        player.movement_crouching = false;
        player.swimming_pose = true;
        player.in_water = false;
        assert!(
            is_moving_slowly(&player),
            "visual crawling remains the independent second isMovingSlowly branch"
        );
    }

    #[test]
    fn crouch_swim_cancels_near_point_zero_four_vertical_speed_before_pitch_steering() {
        let chunks = ChunkStore::with_origin_chunk();
        let source = BlockStateTable::native().find_state("water", &[("level", "0")]);
        for x in 7..=9 {
            for y in 64..=66 {
                for z in 7..=9 {
                    chunks.set_block_state(x, y, z, source, BlockStateTable::native());
                }
            }
        }

        let mut player = LocalPlayer::new();
        player.position = dvec3(8.5, 64.0, 8.5).into();
        player.velocity.y = f64::from(LIQUID_JUMP_ACCELERATION) + 0.001;
        player.look_dir = crate::entity::components::LookDirection::new(0.0, -38.0);
        player.set_sprinting(true);
        player.swimming = true;
        player.swimming_pose = true;
        player.eyes_in_water = true;
        player.food = 20;

        let mut input = InputState::released();
        input.set_key_pressed_for_test(KeyCode::KeyW, true);
        input.set_key_pressed_for_test(KeyCode::ShiftLeft, true);
        let anim = BlockEntityAnimStore::default();

        tick(
            &mut player,
            &input,
            test_world(&chunks, &anim),
            &BlockTags::default(),
            1.0,
            false,
            None,
        );

        let target_vy = vanilla_look_y(-38.0);
        let expected = target_vy * 0.06 * f64::from(WATER_VERTICAL_DRAG);
        assert!(
            (player.velocity.y - expected).abs() < 1.0e-12,
            "goDownInWater must run before LivingEntity tiny-motion cleanup: got {:.15}, expected {:.15}",
            player.velocity.y,
            expected
        );

        let old_order_residual = 0.001 * (1.0 - 0.06) * f64::from(WATER_VERTICAL_DRAG);
        assert!(old_order_residual > 0.0007);
    }

    #[test]
    fn falling_water_subthreshold_horizontal_current_is_cleaned_before_travel() {
        let chunks = ChunkStore::with_origin_chunk();
        let falling = BlockStateTable::native().find_state("water", &[("level", "8")]);
        let low = BlockStateTable::native().find_state("water", &[("level", "7")]);
        let stone = BlockStateTable::native().find_state("stone", &[]);
        chunks.set_block_state(8, 64, 8, falling, BlockStateTable::native());
        chunks.set_block_state(9, 64, 8, low, BlockStateTable::native());
        chunks.set_block_state(8, 64, 7, stone, BlockStateTable::native());

        let mut player = LocalPlayer::new();
        player.position = dvec3(8.5, 64.0, 8.5).into();
        player.refresh_water_interaction(&chunks, BlockStateTable::native());

        let expected_horizontal = 0.014_f64 / 37.0_f64.sqrt();
        assert!(
            (player.velocity.x - expected_horizontal).abs() < 1.0e-12,
            "falling-water +X/-6Y flow must produce the observed 0.014/sqrt(37) current: got {:.12}",
            player.velocity.x
        );
        assert!(player.velocity.x.abs() < 0.003);
        assert!(player.velocity.y.abs() >= 0.003);

        normalize_tiny_velocity(&mut player.velocity);
        assert_eq!(player.velocity.x, 0.0);
        assert_eq!(player.velocity.z, 0.0);
        assert_ne!(player.velocity.y, 0.0);
    }

    #[test]
    fn surface_exit_visual_crawl_applies_exact_vanilla_slow_input_impulse() {
        let mut player = LocalPlayer::new();
        player.swimming = false;
        player.swimming_pose = true;
        player.in_water = false;
        player.crouching = false;
        player.set_sprinting(true);

        assert!(
            is_moving_slowly(&player),
            "Pose.SWIMMING outside water is vanilla isVisuallyCrawling()"
        );

        let mut input = InputState::released();
        input.set_key_pressed_for_test(KeyCode::KeyW, true);
        let slowed = is_moving_slowly(&player).then(|| sneaking_speed(&player));
        let (slowed_forward, _) = movement_input(&input, slowed, 1.0);
        let (full_forward, _) = movement_input(&input, None, 1.0);

        assert_eq!(full_forward.to_bits(), INPUT_DAMPING.to_bits());
        assert_eq!(
            slowed_forward.to_bits(),
            (INPUT_DAMPING * SNEAKING_SPEED).to_bits()
        );

        // First dry tick after swimming: the kept swimming pose slows input.
        let observed_signature =
            f64::from((full_forward - slowed_forward) * SPRINT_AIR_ACCELERATION);
        assert!(
            (observed_signature - 0.017_835_998_88).abs() < 2.0e-9,
            "surface-exit Grim signature: {observed_signature:.12}"
        );
    }

    #[test]
    fn dry_move_into_partial_flowing_water_applies_current_before_ground_friction() {
        let wet = ChunkStore::with_origin_chunk();
        let dry = ChunkStore::with_origin_chunk();
        let stone = BlockStateTable::native().find_state("stone", &[]);
        for x in 6..=10 {
            for z in 7..=9 {
                wet.set_block_state(x, 63, z, stone, BlockStateTable::native());
                dry.set_block_state(x, 63, z, stone, BlockStateTable::native());
            }
        }

        // A source to the east makes this level-3 cell flow west.
        let partial = BlockStateTable::native().find_state("water", &[("level", "3")]);
        let source = BlockStateTable::native().find_state("water", &[("level", "0")]);
        wet.set_block_state(8, 64, 8, partial, BlockStateTable::native());
        wet.set_block_state(9, 64, 8, source, BlockStateTable::native());

        let mut wet_player = LocalPlayer::new();
        wet_player.position = dvec3(7.65, 64.0, 8.5).into();
        wet_player.velocity = Velocity::new(0.08, 0.0, 0.0);
        wet_player.on_ground = true;
        let mut dry_player = LocalPlayer::new();
        dry_player.position = wet_player.position;
        dry_player.velocity = Velocity::new(0.08, 0.0, 0.0);
        dry_player.on_ground = true;

        let neutral = InputState::released();
        tick_land(
            &mut wet_player,
            &neutral,
            CollisionWorld::new(&wet, BlockStateTable::native()),
            &BlockTags::default(),
            false,
            0.0,
            0.0,
            0.0,
            1.0,
        );
        tick_land(
            &mut dry_player,
            &neutral,
            CollisionWorld::new(&dry, BlockStateTable::native()),
            &BlockTags::default(),
            false,
            0.0,
            0.0,
            0.0,
            1.0,
        );

        assert!(
            wet_player.in_water,
            "post-move fluid refresh must detect entry"
        );
        assert!(!dry_player.in_water);
        let observed = dry_player.velocity.x - wet_player.velocity.x;
        let expected = 0.014_f64 * f64::from(0.6_f32 * HORIZONTAL_DRAG);
        assert!(
            (observed - expected).abs() < 1.0e-12,
            "missing-current Grim signature: observed={observed:.15} expected={expected:.15}"
        );
    }

    #[test]
    fn jump_out_of_fluid_rejects_collision_free_boxes_that_still_contain_liquid() {
        let chunks = ChunkStore::with_origin_chunk();
        let water = BlockStateTable::native().find_state("water", &[("level", "0")]);
        chunks.set_block_state(8, 64, 8, water, BlockStateTable::native());

        let wet_box = Aabb::new(dvec3(8.2, 64.2, 8.2), dvec3(8.8, 65.8, 8.8));
        assert!(
            !BlockStateTable::native()
                .block_state_data(water)
                .has_collision(),
            "water has no collision shape"
        );
        assert!(
            contains_any_liquid(
                CollisionWorld::new(&chunks, BlockStateTable::native()),
                &wet_box
            ),
            "vanilla Entity.isFree must still reject a collision-free box containing fluid"
        );

        let dry_box = wet_box.offset(dvec3(2.0, 0.0, 0.0));
        assert!(!contains_any_liquid(
            CollisionWorld::new(&chunks, BlockStateTable::native()),
            &dry_box
        ));
    }

    #[test]
    fn water_falling_adjustment_matches_vanilla_order_and_sprint_suppression() {
        let dragged = -0.002_f64;
        assert_eq!(
            water_falling_adjusted_y(dragged, 0.01, true, false),
            -0.003,
            "vanilla snaps the narrow falling-water band under reduced effective gravity"
        );
        assert_eq!(
            water_falling_adjusted_y(0.0, GRAVITY, false, false),
            -GRAVITY / 16.0
        );
        assert_eq!(
            water_falling_adjusted_y(dragged, GRAVITY, true, true),
            dragged,
            "sprint-swimming suppresses water gravity entirely"
        );
    }

    #[test]
    fn swim_sprint_uses_water_specific_stop_predicate() {
        let mut player = LocalPlayer::new();
        player.set_sprinting(true);
        player.swimming = true;
        player.in_water = true;
        player.food = 20;
        player.horizontal_collision = true;
        player.minor_horizontal_collision = false;

        let released = InputState::released();
        player.on_ground = true;
        update_sprint_state(&mut player, &released, false, false, None);
        assert!(
            player.sprinting,
            "vanilla swim sprint ignores hard-wall collision and may persist without forward input on ground"
        );

        player.on_ground = false;
        update_sprint_state(&mut player, &released, false, false, None);
        assert!(
            !player.sprinting,
            "airborne swim sprint stops when forward input is lost"
        );

        let mut descending = InputState::released();
        descending.set_key_pressed_for_test(KeyCode::ShiftLeft, true);
        player.set_sprinting(true);
        update_sprint_state(&mut player, &descending, false, false, None);
        assert!(
            player.sprinting,
            "descending is the vanilla exception to the no-forward swim-sprint stop"
        );
    }

    #[test]
    fn sprint_window_uses_previous_shift_and_current_backward_like_vanilla() {
        let mut player = LocalPlayer::new();
        player.food = 20;
        player.sprint_toggle_timer = 4;
        player.was_shift_pressed = true;

        let mut forward = InputState::released();
        forward.set_key_pressed_for_test(KeyCode::KeyW, true);
        update_sprint_state(&mut player, &forward, true, false, None);
        assert_eq!(
            player.sprint_toggle_timer, player.sprint_window,
            "previous shift clears the old window before the new forward edge arms a fresh one"
        );
        assert!(!player.sprinting);

        player.sprint_toggle_timer = 4;
        player.was_shift_pressed = false;
        player.was_forward_pressed = true;
        let mut backward = InputState::released();
        backward.set_key_pressed_for_test(KeyCode::KeyS, true);
        update_sprint_state(&mut player, &backward, false, false, None);
        assert_eq!(player.sprint_toggle_timer, 0);
    }

    #[test]
    fn the_double_tap_arms_for_the_sprint_window_and_never_at_zero() {
        let mut forward = InputState::released();
        forward.set_key_pressed_for_test(KeyCode::KeyW, true);
        for (window, sprints) in [(3, true), (0, false)] {
            let mut player = LocalPlayer::new();
            player.food = 20;
            player.sprint_window = window;
            update_sprint_state(&mut player, &forward, true, false, None);
            assert_eq!(player.sprint_toggle_timer, window);

            // Let go, then press again inside the window.
            player.was_forward_pressed = false;
            update_sprint_state(&mut player, &forward, true, false, None);
            assert_eq!(player.sprinting, sprints);
        }
    }

    /// A stone floor at y 63 around a player standing on it at (8.5, 8.5),
    /// facing and walking south, with auto-jump on.
    fn auto_jump_scene() -> (ChunkStore, LocalPlayer, InputState) {
        let chunks = ChunkStore::with_origin_chunk();
        for x in 5..=11 {
            for z in 5..=13 {
                place(&chunks, "stone", (x, 63, z));
            }
        }
        let mut player = LocalPlayer::new();
        player.position = dvec3(8.5, 64.0, 8.5).into();
        player.on_ground = true;
        let mut input = InputState::released();
        input.set_key_pressed_for_test(KeyCode::KeyW, true);
        (chunks, player, input)
    }

    fn place(chunks: &ChunkStore, block: &str, pos: (i32, i32, i32)) {
        place_state(chunks, block, &[], pos);
    }

    fn place_state(
        chunks: &ChunkStore,
        block: &str,
        properties: &[(&str, &str)],
        (x, y, z): (i32, i32, i32),
    ) {
        let state = BlockStateTable::native().find_state(block, properties);
        chunks.set_block_state(x, y, z, state, BlockStateTable::native());
    }

    /// Whether a step south of 0.1 starts an auto-jump.
    fn starts_auto_jump(chunks: &ChunkStore, player: &mut LocalPlayer, input: &InputState) -> bool {
        let (sin, cos) = vanilla_yaw_sin_cos(player.look_dir.y_rot_deg());
        update_auto_jump(
            player,
            input,
            CollisionWorld::new(chunks, BlockStateTable::native()),
            &BlockTags::default(),
            false,
            (0.0, 0.1),
            (sin, cos),
        );
        player.auto_jump_time == 1
    }

    #[test]
    fn walking_into_a_block_high_step_starts_an_auto_jump() {
        let (chunks, mut player, input) = auto_jump_scene();
        assert!(!starts_auto_jump(&chunks, &mut player, &input));

        place(&chunks, "stone", (8, 64, 9));
        assert!(starts_auto_jump(&chunks, &mut player, &input));
    }

    #[test]
    fn auto_jump_needs_the_option_ground_and_no_sneaking() {
        let (chunks, player, input) = auto_jump_scene();
        place(&chunks, "stone", (8, 64, 9));

        let mut off = LocalPlayer::new();
        off.position = player.position;
        off.on_ground = true;
        off.auto_jump_enabled = false;
        assert!(!starts_auto_jump(&chunks, &mut off, &input));

        let mut airborne = LocalPlayer::new();
        airborne.position = player.position;
        assert!(!starts_auto_jump(&chunks, &mut airborne, &input));

        let mut sneaking = input;
        sneaking.set_key_pressed_for_test(KeyCode::ShiftLeft, true);
        let mut player = player;
        assert!(!starts_auto_jump(&chunks, &mut player, &sneaking));

        let still = InputState::released();
        assert!(!starts_auto_jump(&chunks, &mut player, &still));
    }

    #[test]
    fn auto_jump_skips_what_is_too_low_too_tall_or_roofed_over() {
        let (slab_world, mut player, input) = auto_jump_scene();
        place_state(&slab_world, "stone_slab", &[("type", "bottom")], (8, 64, 9));
        assert!(!starts_auto_jump(&slab_world, &mut player, &input));
        place_state(&slab_world, "stone_slab", &[("type", "top")], (8, 64, 9));
        assert!(starts_auto_jump(&slab_world, &mut player, &input));

        let (wall, mut player, input) = auto_jump_scene();
        place(&wall, "stone", (8, 64, 9));
        place(&wall, "stone", (8, 65, 9));
        assert!(!starts_auto_jump(&wall, &mut player, &input));

        let (roofed, mut player, input) = auto_jump_scene();
        place(&roofed, "stone", (8, 64, 9));
        place(&roofed, "stone", (8, 66, 8));
        assert!(!starts_auto_jump(&roofed, &mut player, &input));
    }

    #[test]
    fn auto_jump_ignores_a_step_behind_the_way_the_player_faces() {
        let (chunks, mut player, input) = auto_jump_scene();
        place(&chunks, "stone", (8, 64, 9));
        player.look_dir = crate::entity::components::LookDirection::new(180.0, 0.0);
        assert!(!starts_auto_jump(&chunks, &mut player, &input));
    }

    #[test]
    fn an_auto_jump_jumps_the_next_tick_without_arming_the_flight_toggle() {
        let (chunks, mut player, _) = auto_jump_scene();
        let anim = BlockEntityAnimStore::default();
        player.may_fly = true;
        player.auto_jump_time = 1;
        tick(
            &mut player,
            &InputState::released(),
            test_world(&chunks, &anim),
            &BlockTags::default(),
            1.0,
            false,
            None,
        );
        assert!(player.auto_jumping);
        assert!(player.was_jump_pressed);
        assert!(player.velocity.y > 0.0);
        assert_eq!(player.auto_jump_time, 0);
        assert_eq!(player.jump_trigger_time, 0);
    }

    #[test]
    fn item_slowdown_blocks_start_but_does_not_cancel_existing_sprint() {
        let mut input = InputState::released();
        input.set_key_pressed_for_test(KeyCode::KeyW, true);
        input.set_key_pressed_for_test(KeyCode::ControlLeft, true);

        let mut player = LocalPlayer::new();
        player.food = 20;
        update_sprint_state(&mut player, &input, true, true, None);
        assert!(!player.sprinting, "item slowdown blocks canStartSprinting");

        player.set_sprinting(true);
        player.was_forward_pressed = true;
        update_sprint_state(&mut player, &input, true, true, None);
        assert!(
            player.sprinting,
            "vanilla shouldStopRunSprinting does not cancel an already-active sprint solely for item slowdown"
        );
    }

    #[test]
    fn shallow_water_stops_existing_run_sprint_even_while_sprint_key_is_held() {
        let mut input = InputState::released();
        input.set_key_pressed_for_test(KeyCode::KeyW, true);
        input.set_key_pressed_for_test(KeyCode::ControlLeft, true);

        let mut player = LocalPlayer::new();
        player.set_sprinting(true);
        player.swimming = false;
        player.in_water = true;
        player.under_water = false;
        player.food = 20;

        update_sprint_state(&mut player, &input, true, false, None);

        assert!(
            !player.sprinting,
            "vanilla shouldStopRunSprinting cancels sprint in shallow water even while Sprint remains held"
        );
    }

    #[test]
    fn previous_hard_wall_collision_stops_run_sprint_but_minor_collision_does_not() {
        let input = InputState::released();
        let mut player = LocalPlayer::new();
        player.set_sprinting(true);
        player.was_forward_pressed = true;
        player.horizontal_collision = true;
        player.minor_horizontal_collision = false;

        update_sprint_state(&mut player, &input, true, false, None);
        assert!(
            !player.sprinting,
            "vanilla shouldStopRunSprinting consumes the prior hard wall collision"
        );

        player.set_sprinting(true);
        player.minor_horizontal_collision = true;
        update_sprint_state(&mut player, &input, true, false, None);
        assert!(
            player.sprinting,
            "minorHorizontalCollision is the vanilla wall-sprint exemption"
        );
    }

    #[test]
    fn gamepad_horizontal_axis_matches_vanilla_strafe_sign() {
        let analog = input::gamepad_movement_axes(glam::vec2(-1.0, 0.0));
        assert_eq!(analog, glam::vec2(1.0, 0.0));
        let (sin, cos) = vanilla_yaw_sin_cos(0.0);
        let (dx, dz) = movement_delta(analog.y, analog.x, 1.0, sin, cos);
        assert_eq!((dx, dz), (1.0, 0.0));

        let analog = input::gamepad_movement_axes(glam::vec2(1.0, 0.0));
        assert_eq!(analog, glam::vec2(-1.0, 0.0));
        let (dx, dz) = movement_delta(analog.y, analog.x, 1.0, sin, cos);
        assert_eq!((dx, dz), (-1.0, 0.0));

        assert_eq!(
            input::gamepad_movement_axes(glam::vec2(-0.6, 0.8)),
            glam::vec2(0.6, 0.8)
        );
    }

    /// A float direction that widens to just over unit length takes the
    /// normalize branch, where a reciprocal multiply lands one ULP off.
    #[test]
    fn over_unit_input_normalizes_like_vanilla() {
        let (sin, cos) = vanilla_yaw_sin_cos(0.0);
        let strafe = f32::from_bits(0x3f7ffb1c);
        let forward = f32::from_bits(0x3c4829d1);
        let (dx, dz) = movement_delta(forward, strafe, 1.0, sin, cos);
        assert_eq!(dx.to_bits(), 0x3fefff637d23f861);
        assert_eq!(dz.to_bits(), 0x3f89053a1dc39789);
    }

    #[test]
    fn vanilla_mth_trig_and_movement_rotation_match_java_bits() {
        let (sin, cos) = vanilla_yaw_sin_cos(30.0);
        assert_eq!(sin.to_bits(), 0x3efffc5f);
        assert_eq!(cos.to_bits(), 0x3f5db4e3);

        let (sin, cos) = vanilla_yaw_sin_cos(45.0);
        assert_eq!(sin.to_bits(), 0x3f3504f3);
        assert_eq!(cos.to_bits(), 0x3f3504f3);

        let forward = f32::from_bits(0x3f3504f2);
        let speed = movement_speed(&LocalPlayer::new());
        let (dx, dz) = movement_delta(forward, 0.0, speed, sin, cos);
        assert_eq!(dx.to_bits(), 0xbfa999996d18578d);
        assert_eq!(dz.to_bits(), 0x3fa999996d18578d);

        assert_eq!(vanilla_look_y(30.0).to_bits(), 0xbfdfff8be0000000);
    }

    #[test]
    fn sleeping_player_is_immobile_without_freezing_travel() {
        let mut player = LocalPlayer::new();
        player.sleeping_pos = Some(crate::position::BlockPos::new(0, 64, 0));

        let mut forward = 0.75;
        let mut strafe = -0.25;
        let mut jumping = true;
        apply_living_immobility(&player, &mut forward, &mut strafe, &mut jumping);
        assert_eq!((forward, strafe, jumping), (0.0, 0.0, false));

        player.position = dvec3(0.0, 80.0, 0.0).into();
        player.velocity = Velocity::new(0.25, 0.0, -0.1);
        let chunks = ChunkStore::new(2);
        tick(
            &mut player,
            &InputState::released(),
            test_world(&chunks, &BlockEntityAnimStore::default()),
            &BlockTags::default(),
            1.0,
            false,
            None,
        );
        assert!(player.position.x > 0.0);
        assert!(player.position.z < 0.0);
        assert!(player.position.y <= 80.0);
    }

    #[test]
    fn dead_player_keeps_zero_input_air_travel() {
        let mut player = LocalPlayer::new();
        player.position = dvec3(0.0, 80.0, 0.0).into();
        player.velocity = crate::entity::components::Velocity::new(0.25, 0.0, -0.1);
        player.set_sprinting(true);
        player.crouching = true;
        player.eye_height = 1.27;
        player.prev_eye_height = 1.27;
        let chunks = ChunkStore::new(2);
        let block_entity_anim = BlockEntityAnimStore::default();

        player.death_time = 1;
        let starting_height = player.height();
        tick_dead(
            &mut player,
            test_world(&chunks, &block_entity_anim),
            &BlockTags::default(),
            false,
        );

        assert_eq!(
            starting_height, CROUCH_HEIGHT,
            "death must begin from the player's existing ordinary pose"
        );

        assert!(
            !player.crouching,
            "the first dead tick must end by selecting the neutral-input pose"
        );
        assert_eq!(
            player.eye_height, 1.27,
            "the first dead tick must still use the pre-tick crouching eye height"
        );
        player.death_time = 2;
        tick_dead(
            &mut player,
            test_world(&chunks, &block_entity_anim),
            &BlockTags::default(),
            false,
        );
        assert!(
            player.eye_height > 1.27,
            "the next dead tick must smooth toward the neutral standing pose"
        );
        assert!(
            player.position.x > 0.0,
            "dead-player momentum must still move the corpse"
        );
        assert!(
            player.position.z < 0.0,
            "dead-player momentum must still move the corpse"
        );
        assert!(
            player.position.y <= 80.0,
            "dead-player travel must continue applying gravity"
        );
        assert!(
            player.velocity.y < 0.0,
            "dead-player travel must retain downward gravity/drag"
        );
        assert!(
            !player.sprinting,
            "immobile dead-player input must stop sprinting"
        );
    }

    #[test]
    fn forward_and_backward_cancel_the_forward_impulse() {
        let mut input = InputState::released();
        input.set_key_pressed_for_test(KeyCode::KeyW, true);
        assert!(has_forward_impulse(&input));
        input.set_key_pressed_for_test(KeyCode::KeyS, true);
        assert!(!has_forward_impulse(&input));
    }

    #[test]
    fn rebound_movement_replaces_defaults_and_cancels_forward_impulse() {
        let mut input = InputState::released();
        input.key_bindings.set(
            input::Action::Forward,
            vec![input::InputKey::Keyboard(KeyCode::KeyI)],
        );
        input.key_bindings.set(
            input::Action::Backward,
            vec![input::InputKey::Keyboard(KeyCode::KeyK)],
        );
        input.set_key_pressed_for_test(KeyCode::KeyW, true);
        assert_eq!(movement_input(&input, None, 1.0), (0.0, 0.0));

        input.set_key_pressed_for_test(KeyCode::KeyI, true);
        assert_eq!(movement_input(&input, None, 1.0), (INPUT_DAMPING, 0.0));
        assert!(has_forward_impulse(&input));
        input.set_key_pressed_for_test(KeyCode::KeyK, true);
        assert_eq!(movement_input(&input, None, 1.0), (0.0, 0.0));
        assert!(!has_forward_impulse(&input));

        let mut player = LocalPlayer::new();
        player.sprint_toggle_timer = 4;
        player.was_forward_pressed = true;
        update_sprint_state(&mut player, &input, false, false, None);
        assert_eq!(player.sprint_toggle_timer, 0);
    }

    #[test]
    fn flight_toggle_off_the_ground_skips_the_living_jump() {
        let chunks = ChunkStore::with_origin_chunk();
        chunks.set_block_state(
            8,
            63,
            8,
            BlockStateTable::native().find_state("stone", &[]),
            BlockStateTable::native(),
        );
        let mut player = LocalPlayer::new();
        player.position = dvec3(8.5, 64.0, 8.5).into();
        player.on_ground = true;
        player.may_fly = true;
        player.jump_trigger_time = 5;
        let mut input = InputState::released();
        input.set_key_pressed_for_test(KeyCode::Space, true);

        tick(
            &mut player,
            &input,
            test_world(&chunks, &BlockEntityAnimStore::default()),
            &BlockTags::default(),
            1.0,
            false,
            None,
        );
        assert!(player.flying);
        assert_eq!(
            player.no_jump_delay, 0,
            "flying players skip the jump block"
        );
    }

    #[test]
    fn flying_through_water_uses_air_travel() {
        let chunks = ChunkStore::with_origin_chunk();
        chunks.set_block_state(
            8,
            64,
            8,
            BlockStateTable::native().find_state("water", &[]),
            BlockStateTable::native(),
        );
        let mut player = LocalPlayer::new();
        player.position = dvec3(8.5, 64.1, 8.5).into();
        player.may_fly = true;
        player.flying = true;
        player.velocity = Velocity::new(0.1, 0.0, 0.0);

        tick(
            &mut player,
            &InputState::released(),
            test_world(&chunks, &BlockEntityAnimStore::default()),
            &BlockTags::default(),
            1.0,
            false,
            None,
        );
        assert!(player.in_water);
        assert_eq!(player.velocity.x, 0.1 * f64::from(HORIZONTAL_DRAG));
    }

    #[test]
    fn flying_ignores_the_block_speed_factor() {
        let chunks = ChunkStore::with_origin_chunk();
        chunks.set_block_state(
            8,
            64,
            8,
            BlockStateTable::native().find_state("soul_sand", &[]),
            BlockStateTable::native(),
        );
        let mut player = LocalPlayer::new();
        player.position = dvec3(8.5, 65.0, 8.5).into();
        assert_eq!(
            block_speed_factor(
                &player,
                CollisionWorld::new(&chunks, BlockStateTable::native()),
                &BlockTags::default()
            ),
            0.4
        );
        player.flying = true;
        assert_eq!(
            block_speed_factor(
                &player,
                CollisionWorld::new(&chunks, BlockStateTable::native()),
                &BlockTags::default()
            ),
            1.0
        );
    }

    #[test]
    fn piston_movement_caps_at_0_51_per_axis_per_tick() {
        let mut player = LocalPlayer::new();
        let up = |player: &mut LocalPlayer, amount, time| {
            limit_piston_movement(player, dvec3(0.0, amount, 0.0), time).y
        };
        assert_eq!(up(&mut player, 0.3, 7), 0.3);
        assert!((up(&mut player, 0.3, 7) - 0.21).abs() < 1e-12);
        assert_eq!(up(&mut player, 0.3, 7), 0.0);
        // A new tick starts the budget over.
        assert_eq!(up(&mut player, 0.3, 8), 0.3);
        // Only the first non-zero axis moves.
        let both = limit_piston_movement(&mut player, dvec3(0.2, 0.2, 0.0), 9);
        assert_eq!(both, dvec3(0.2, 0.0, 0.0));
    }

    #[test]
    fn piston_movement_area_and_overlap_match_vanilla() {
        let block = Aabb::block(0, 0, 0);
        let up = movement_area(&block, Direction::Up, 0.5);
        assert_eq!(
            (up.min, up.max),
            (dvec3(0.0, 1.0, 0.0), dvec3(1.0, 1.5, 1.0))
        );
        let west = movement_area(&block, Direction::West, 0.5);
        assert_eq!(
            (west.min, west.max),
            (dvec3(-0.5, 0.0, 0.0), dvec3(0.0, 1.0, 1.0))
        );
        let entity = Aabb::new(dvec3(0.2, 1.2, 0.2), dvec3(0.8, 3.0, 0.8));
        assert!((piston_movement(&up, Direction::Up, &entity) - 0.3).abs() < 1e-12);
    }

    /// The player's y after a stone rising out of (8, 64, 8) moves from
    /// half to fully out, with them standing on its top face.
    fn y_after_rising_stone(game_mode: u8) -> f64 {
        let chunks = ChunkStore::new(2);
        let anim = BlockEntityAnimStore::default();
        let pos = BlockPos::new(8, 64, 8);
        let stone = BlockStateTable::native().find_state("stone", &[]);
        let mut piston = MovingPiston::new(stone, Direction::Up, true, false);
        piston.progress = 0.5;
        let mut player = LocalPlayer::new();
        player.game_mode = game_mode;
        player.position = dvec3(8.5, 64.5, 8.5).into();
        player.on_ground = true;
        let world = test_world(&chunks, &anim);
        push_by_moving_piston(
            &mut player,
            world,
            &BlockTags::default(),
            pos,
            &piston,
            1.0,
            1,
        );
        player.position.y
    }

    #[test]
    fn an_extending_piston_lifts_the_player_standing_on_it() {
        let y = y_after_rising_stone(0);
        assert!((y - 65.01).abs() < 1e-9, "{y}");
    }

    #[test]
    fn pistons_leave_spectators_alone() {
        assert_eq!(y_after_rising_stone(3), 64.5);
    }

    #[test]
    fn on_pos_keeps_a_tagged_fence_or_wall_as_the_block_underfoot() {
        let table = BlockStateTable::native();
        let tags = crate::world::block::block_tags_for_test(
            table,
            &[("fences", &["oak_fence"]), ("walls", &["cobblestone_wall"])],
        );
        let support = BlockPos::new(8, 63, 8);
        for block in ["oak_fence", "cobblestone_wall"] {
            let chunks = ChunkStore::with_origin_chunk();
            chunks.set_block_state(8, 63, 8, table.find_state(block, &[]), table);
            let world = CollisionWorld::new(&chunks, table);
            // Standing on the 1.5-tall top, 0.2 below the feet is y 64.
            let mut player = LocalPlayer::new();
            player.position = dvec3(8.5, 64.5, 8.5).into();
            player.main_supporting_block_pos = Some(support);
            assert_eq!(on_pos(&player, world, &tags, 0.2), support, "{block}");
            assert_eq!(
                on_pos(&player, world, &BlockTags::default(), 0.2),
                BlockPos::new(8, 64, 8),
                "{block} untagged"
            );
        }
    }

    #[test]
    fn piston_pushes_walk_toward_the_next_step() {
        let chunks = ChunkStore::with_origin_chunk();
        let table = BlockStateTable::native();
        for x in 0..16 {
            chunks.set_block_state(x, 63, 8, table.find_state("stone", &[]), table);
        }
        let anim = BlockEntityAnimStore::default();
        let tags = BlockTags::default();
        let world = test_world(&chunks, &anim);
        let mut player = LocalPlayer::new();
        player.position = dvec3(2.5, 64.0, 8.5).into();
        // 0.51 a tick walks 0.306 of `moveDist`; a sideways push leaves the
        // player off the ground, so it steps on nothing itself.
        for tick in 1..=4 {
            let mv = PistonMove {
                world,
                tags: &tags,
                descending: false,
                tick,
            };
            move_by_piston(&mut player, mv, dvec3(0.51, 0.0, 0.0));
        }
        assert!(player.movement_sounds.pending.is_empty());
        // The player's own next move, however short, passes the step at 1.
        player.on_ground = true;
        player.main_supporting_block_pos = Some(BlockPos::new(4, 63, 8));
        let mv = movement_sound::Move {
            movement: dvec3(0.01, 0.0, 0.0),
            velocity: DVec3::ZERO,
        };
        movement_sound::emit(&mut player, world, &tags, false, mv);
        assert_eq!(player.movement_sounds.pending.len(), 1);
        assert_eq!(player.movement_sounds.pending[0].event, "block.stone.step");
    }
}
