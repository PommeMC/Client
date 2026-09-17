// TODO: fall damage - track fall distance, reset on water entry, apply damage
// on ground impact; Player.causeFallDamage returns false when may_fly, and
// fall distance resets every tick while flying

use glam::{DVec3, dvec3};
use winit::keyboard::KeyCode;

use super::aabb::Aabb;
use super::collision::{
    PlayerCollisionContext, find_supporting_block, no_player_collision, resolve_player_collision,
};
use crate::app::input::{self, InputState};
use crate::entity::components::Velocity;
use crate::player::{CROUCH_HEIGHT, LocalPlayer, PLAYER_HALF_WIDTH, STANDING_HEIGHT};
use crate::world::block::{
    FluidKind, fluid, movement_friction, movement_jump_factor, movement_speed_factor,
};
use crate::world::block_entity_anim::BlockEntityAnimStore;
use crate::world::chunk::ChunkStore;

const GRAVITY: f64 = 0.08;
// Vanilla mixes float and double physics values. Keep float values as f32 until
// the exact point where vanilla widens them into Vec3/AABB doubles.
const JUMP_VELOCITY: f32 = 0.42;
const VERTICAL_DRAG: f32 = 0.98;
const HORIZONTAL_DRAG: f32 = 0.91;
const GROUND_ACCEL_FACTOR: f32 = 0.216_000_02;
// Player.createAttributes receives 0.1f; the sprint modifier amount is 0.3f.
// Both are widened into the double-backed attribute system before
// Player.getSpeed casts the final value back to float.
const MOVEMENT_SPEED_ATTRIBUTE: f64 = 0.1_f32 as f64;
const SPRINT_SPEED_MODIFIER: f64 = 0.3_f32 as f64;
// SNEAKING_SPEED is a double attribute (default 0.3), then LocalPlayer casts it
// to float before scaling its Vec2 input.
const SNEAKING_SPEED: f32 = 0.3_f64 as f32;
const INPUT_DAMPING: f32 = 0.98;
const AIR_ACCELERATION: f32 = 0.02;
// TODO: WATER_MOVEMENT_EFFICIENCY attribute - scales drag toward 0.54600006f
// and accel toward land speed.
const WATER_ACCELERATION: f32 = 0.02;
const WATER_HORIZONTAL_DRAG: f32 = 0.8;
const WATER_HORIZONTAL_DRAG_SPRINT: f32 = 0.9;
const WATER_VERTICAL_DRAG: f32 = 0.8;
// STEP_HEIGHT is a double attribute, but LivingEntity.maxUpStep casts it to
// float.
const STEP_HEIGHT: f32 = 0.6_f64 as f32;
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
const DEFAULT_SPRINT_WINDOW: u32 = 7;
const FLY_TOGGLE_WINDOW: u32 = 7;
// Mth.equal(double, double) widens the float EPSILON constant to double.
const MTH_EQUAL_EPSILON: f64 = 1.0e-5_f32 as f64;
const MINOR_COLLISION_ANGLE: f64 = 0.139_626_339_077_949_52;
const DEG_TO_RAD: f32 = std::f32::consts::PI / 180.0_f32;
const SIN_SCALE: f64 = 10_430.378_350_470_453;

#[derive(Clone, Copy)]
struct MovementWorld<'a> {
    chunks: &'a ChunkStore,
    block_entity_anim: &'a BlockEntityAnimStore,
}

pub fn tick(
    player: &mut LocalPlayer,
    input: &InputState,
    chunk_store: &ChunkStore,
    block_entity_anim: &BlockEntityAnimStore,
    use_speed_multiplier: f32,
    slow_due_to_using_item: bool,
    is_passenger: bool,
) {
    let jump_held = input.performing_action(input::Action::Jump);
    let world = MovementWorld {
        chunks: chunk_store,
        block_entity_anim,
    };

    // Vanilla Player.tick snapshots LocalPlayer.wasUnderwater from the
    // previous EntityFluidInteraction before Entity.baseTick refreshes fluid
    // contact for this tick. Keep that one-tick surface transition explicitly.
    player.under_water = player.eyes_in_water;
    // Entity.baseTick refreshes fluid interaction (and applies its current)
    // before LocalPlayer.aiStep. LivingEntity's tiny-motion cleanup does not
    // run until LocalPlayer has processed sprint/flight and vertical key input
    // (including goDownInWater) below.
    player.update_water_state(chunk_store, is_passenger);

    if player.flying {
        // Vanilla `Player.aiStep` resets this before LivingEntity movement, so
        // powder-snow collision sees zero fall distance while flying.
        player.fall_distance = 0.0;
    }

    // Vanilla `LivingEntity.aiStep`.
    if player.no_jump_delay > 0 {
        player.no_jump_delay -= 1;
    }

    if player.in_water {
        // Vanilla `Entity.updateFluidInteraction` resets fall distance as soon
        // as water contact is established.
        player.fall_distance = 0.0;
    }
    // LocalPlayer's private `crouching` movement flag is recomputed here,
    // before KeyboardInput.tick, and is distinct from the physical CROUCHING
    // pose selected by Player.updatePlayerPose at the previous tick end.
    // They differ during crouch/swim transitions.
    update_movement_crouching_state(player, chunk_store, block_entity_anim, is_passenger);
    player.tick_eye_height();

    // Vanilla `LocalPlayer.modifyInput` keeps the entire input pipeline in
    // float: damping, item-use slowdown, sneaking slowdown, then square remap.
    let moving_slowly = is_moving_slowly(player);
    let (forward, strafe) = movement_input(input, moving_slowly, use_speed_multiplier);
    let forward_pressed = input.key_pressed(KeyCode::KeyW)
        || input
            .get_gamepad_movement_axes()
            .map(|vec| vec.y > input::STICK_MOVEMENT_THRESHOLD)
            .unwrap_or(false);

    update_sprint_state(
        player,
        input,
        forward,
        forward_pressed,
        slow_due_to_using_item,
    );

    let (sin_y_rot, cos_y_rot) = vanilla_yaw_sin_cos(player.look_dir.y_rot_deg());

    update_fly_state(player, input, chunk_store, sin_y_rot, cos_y_rot);

    // Vanilla LocalPlayer.aiStep applies goDownInWater before super.aiStep's
    // liquid jump handling. This ordering matters when Sneak and Jump overlap.
    if player.in_water && input.performing_action(input::Action::Sneak) {
        player.velocity.y -= f64::from(LIQUID_JUMP_ACCELERATION);
    }

    if player.flying {
        let mut input_ya = 0.0f32;
        if input.performing_action(input::Action::Sneak) {
            input_ya -= 1.0;
        }
        if jump_held {
            input_ya += 1.0;
        }
        if input_ya != 0.0 {
            // Vanilla does this math in f32 before widening.
            player.velocity.y += f64::from(input_ya * player.fly_speed * 3.0);
        }
    }

    // Vanilla `LivingEntity.aiStep` begins only after the LocalPlayer-specific
    // aiStep work above. This is the exact point where tiny player motion is
    // discarded. In particular, `goDownInWater(-0.04)` has already run, so a
    // small residual produced by cancelling an upward swim velocity snaps to
    // zero before pitch steering/travel.
    normalize_tiny_velocity(&mut player.velocity);

    // Vanilla `LivingEntity.aiStep`: swim upward when submerged past the jump
    // threshold, otherwise a full jump off the ground or the shallow-fluid floor.
    if jump_held {
        let in_water = player.in_water && player.fluid_height > 0.0;
        if in_water && (!player.on_ground || player.fluid_height > FLUID_JUMP_THRESHOLD) {
            player.velocity.y += f64::from(LIQUID_JUMP_ACCELERATION);
        } else if (player.on_ground || (in_water && player.fluid_height <= FLUID_JUMP_THRESHOLD))
            && player.no_jump_delay == 0
        {
            jump_from_ground(player, chunk_store, sin_y_rot, cos_y_rot);
            player.no_jump_delay = JUMP_DELAY_TICKS;
        }
    } else {
        player.no_jump_delay = 0;
    }

    if player.in_water {
        tick_water(player, input, world, forward, strafe, sin_y_rot, cos_y_rot);
    } else {
        tick_land(player, input, world, forward, strafe, sin_y_rot, cos_y_rot);
    }

    player.tick_air_supply();
    stop_flying_on_ground(player);

    // Vanilla Player.updatePlayerPose runs after LocalPlayer/LivingEntity
    // movement (including the on-ground flight cancellation above). The newly
    // sampled shift state therefore changes the physical pose/bounding box for
    // the following tick; this tick's movement slowdown used the previous pose.
    update_crouch_state(player, input, chunk_store, block_entity_anim, is_passenger);

    player.was_forward_pressed = forward_pressed;
    player.was_shift_pressed = input.performing_action(input::Action::Sneak);
    player.was_jump_pressed = jump_held;
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
    chunk_store: &ChunkStore,
    block_entity_anim: &BlockEntityAnimStore,
    is_passenger: bool,
) {
    player.no_jump_delay = 0;
    player.sprinting = false;

    // Local players enter death through SetHealth; entity event 3 intentionally
    // skips LivingEntity.die for players, so the current ordinary player pose
    // remains authoritative until Player.updatePlayerPose runs at tick end.
    let neutral = InputState::released();
    player.update_water_state(chunk_store, is_passenger);
    player.tick_eye_height();

    let (sin_y_rot, cos_y_rot) = vanilla_yaw_sin_cos(player.look_dir.y_rot_deg());
    let world = MovementWorld {
        chunks: chunk_store,
        block_entity_anim,
    };
    if player.in_water {
        tick_water(player, &neutral, world, 0.0, 0.0, sin_y_rot, cos_y_rot);
    } else {
        tick_land(player, &neutral, world, 0.0, 0.0, sin_y_rot, cos_y_rot);
    }

    // Player.updatePlayerPose runs after LivingEntity.tick in vanilla. With
    // death-screen input released, this becomes standing unless clearance keeps
    // the player in the crouching pose for the following tick.
    update_crouch_state(
        player,
        &neutral,
        chunk_store,
        block_entity_anim,
        is_passenger,
    );

    stop_flying_on_ground(player);
    player.was_forward_pressed = false;
    player.was_shift_pressed = false;
    player.was_jump_pressed = false;
}

// Vanilla `LocalPlayer.aiStep`: a fresh jump press arms the toggle window;
// a second one inside it toggles flight.
fn update_fly_state(
    player: &mut LocalPlayer,
    input: &InputState,
    chunk_store: &ChunkStore,
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
        } else if !player.was_jump_pressed && input.performing_action(input::Action::Jump) {
            if player.jump_trigger_time == 0 {
                player.jump_trigger_time = FLY_TOGGLE_WINDOW;
            } else if !player.swimming {
                player.flying = !player.flying;
                if player.flying && player.on_ground {
                    jump_from_ground(player, chunk_store, sin_y_rot, cos_y_rot);
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
    chunk_store: &ChunkStore,
    sin_y_rot: f32,
    cos_y_rot: f32,
) {
    let jump_factor = block_jump_factor(player, chunk_store);
    let jump_velocity = JUMP_VELOCITY * jump_factor;
    player.velocity.y = f64::from(jump_velocity).max(player.velocity.y);

    if player.sprinting {
        player.velocity.x += f64::from(-sin_y_rot) * SPRINT_JUMP_BOOST;
        player.velocity.z += f64::from(cos_y_rot) * SPRINT_JUMP_BOOST;
    }
}

fn tick_land(
    player: &mut LocalPlayer,
    input: &InputState,
    world: MovementWorld<'_>,
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
        movement_friction(block_below_affecting_movement(player, world.chunks))
    } else {
        1.0
    };
    let speed = movement_speed(player.sprinting);
    let accel = friction_influenced_speed(speed, player, block_friction);
    let (move_x, move_z) = movement_delta(forward, strafe, accel, sin_y_rot, cos_y_rot);
    player.velocity.x += move_x;
    player.velocity.z += move_z;

    apply_collision(player, input, world, forward, strafe, sin_y_rot, cos_y_rot);

    // LivingEntity.checkFallDamage runs from Entity.move after the position and
    // collision flags have been updated. When travelInAir began dry, vanilla
    // refreshes fluid interaction here. Entering flowing water therefore adds
    // its current during this same move, before block speed factor and the
    // end-of-travel air/ground friction are applied.
    player.refresh_water_interaction(world.chunks);

    // `Entity.move` applies the block speed factor immediately after collision,
    // before LivingEntity applies gravity and air/ground drag.
    let speed_factor = block_speed_factor(player, world.chunks);
    player.velocity.x *= f64::from(speed_factor);
    player.velocity.z *= f64::from(speed_factor);

    player.velocity.y -= GRAVITY;
    player.velocity.y *= f64::from(VERTICAL_DRAG);

    let h_friction = if on_ground_at_start {
        block_friction * HORIZONTAL_DRAG
    } else {
        HORIZONTAL_DRAG
    };
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

fn tick_water(
    player: &mut LocalPlayer,
    input: &InputState,
    world: MovementWorld<'_>,
    forward: f32,
    strafe: f32,
    sin_y_rot: f32,
    cos_y_rot: f32,
) {
    // Player.travel applies the swimming look-vector correction before
    // LivingEntity.travelInFluid. Looking upward at the surface is suppressed
    // unless jump is held or there is still fluid at y + 0.9.
    if player.swimming {
        let target_vy = vanilla_look_y(player.look_dir.x_rot_deg());
        let surface_y = (player.position.y + 1.0 - 0.1).floor() as i32;
        let surface_fluid = fluid(world.chunks.get_block_state(
            player.position.x.floor() as i32,
            surface_y,
            player.position.z.floor() as i32,
        ));
        if target_vy <= 0.0
            || input.performing_action(input::Action::Jump)
            || surface_fluid.kind != FluidKind::Empty
        {
            let boost = if target_vy < -0.2 { 0.085 } else { 0.06 };
            player.velocity.y += (target_vy - player.velocity.y) * boost;
        }
    }

    // LivingEntity.travelInFluid samples falling state and old Y before the
    // water move; Player.travel also captures this post-look-adjustment Y
    // velocity for the flying override.
    let is_falling = player.velocity.y <= 0.0;
    let old_y = player.position.y;
    let saved_vy = player.velocity.y;

    let (move_x, move_z) =
        movement_delta(forward, strafe, WATER_ACCELERATION, sin_y_rot, cos_y_rot);
    player.velocity.x += move_x;
    player.velocity.z += move_z;

    apply_collision(player, input, world, forward, strafe, sin_y_rot, cos_y_rot);

    let slow_down = if player.sprinting {
        WATER_HORIZONTAL_DRAG_SPRINT
    } else {
        WATER_HORIZONTAL_DRAG
    };
    player.velocity.x *= f64::from(slow_down);
    player.velocity.y *= f64::from(WATER_VERTICAL_DRAG);
    player.velocity.z *= f64::from(slow_down);

    // LivingEntity.getFluidFallingAdjustedMovement: ordinary water gravity is
    // effective gravity / 16, after drag. Sprint-swimming suppresses it. The
    // special -0.003 snap is intentionally double-precision vanilla math.
    player.velocity.y =
        water_falling_adjusted_y(player.velocity.y, GRAVITY, is_falling, player.sprinting);

    // LivingEntity.jumpOutOfFluid: after the water move/drag/gravity, colliding
    // with a bank kicks vertical speed to 0.3f if the proposed step-out box is
    // free.
    if player.horizontal_collision {
        let y_offset = player.velocity.y + f64::from(STEP_HEIGHT) - player.position.y + old_y;
        let context = player_collision_context(
            player,
            input.performing_action(input::Action::Sneak),
            world.block_entity_anim,
        );
        let probe =
            player
                .bounding_box()
                .offset(dvec3(player.velocity.x, y_offset, player.velocity.z));
        if no_player_collision(world.chunks, &probe, &context)
            && !contains_any_liquid(world.chunks, &probe)
        {
            player.velocity.y = f64::from(0.3_f32);
        }
    }

    overwrite_flying_vy(player, saved_vy);
}

// Vanilla Player.travel: while flying the travel step runs normally (gravity
// and water physics included) but its vertical result is discarded, replaced
// with the pre-travel vy decayed by 0.6.
fn overwrite_flying_vy(player: &mut LocalPlayer, saved_vy: f64) {
    if player.flying {
        player.velocity.y = saved_vy * FLYING_VERTICAL_FRICTION;
    }
}

/// Vanilla `LevelReader.containsAnyLiquid(AABB)`: any non-empty fluid state in
/// an overlapped block cell makes the box non-free, regardless of the fluid's
/// actual surface height within that cell.
fn contains_any_liquid(chunk_store: &ChunkStore, aabb: &Aabb) -> bool {
    let x0 = aabb.min.x.floor() as i32;
    let x1 = aabb.max.x.ceil() as i32;
    let y0 = aabb.min.y.floor() as i32;
    let y1 = aabb.max.y.ceil() as i32;
    let z0 = aabb.min.z.floor() as i32;
    let z1 = aabb.max.z.ceil() as i32;

    for x in x0..x1 {
        for y in y0..y1 {
            for z in z0..z1 {
                if fluid(chunk_store.get_block_state(x, y, z)).kind != FluidKind::Empty {
                    return true;
                }
            }
        }
    }
    false
}

fn apply_collision(
    player: &mut LocalPlayer,
    input: &InputState,
    world: MovementWorld<'_>,
    forward: f32,
    strafe: f32,
    sin_y_rot: f32,
    cos_y_rot: f32,
) {
    let aabb = player.bounding_box();
    let delta = back_off_from_edge(
        world.chunks,
        &aabb,
        *player.velocity,
        input.performing_action(input::Action::Sneak),
        player.on_ground,
        player.flying,
        &player_collision_context(
            player,
            input.performing_action(input::Action::Sneak),
            world.block_entity_anim,
        ),
    );
    let collision_context = player_collision_context(
        player,
        input.performing_action(input::Action::Sneak),
        world.block_entity_anim,
    );
    let (resolved, on_ground) = resolve_player_collision(
        world.chunks,
        aabb,
        delta.into(),
        f64::from(STEP_HEIGHT),
        player.on_ground,
        &collision_context,
    );

    // Vanilla horizontal collision flags use Mth.equal(double, double), whose
    // epsilon is the widened float constant 1.0E-5f.
    let collided_x = !mth_equal(delta.x, resolved.x);
    // Vanilla only applies Mth.equal to the horizontal axes. Vertical
    // collision is an exact double comparison.
    let collided_y = delta.y != resolved.y;
    let collided_z = !mth_equal(delta.z, resolved.z);
    let horizontal_collision = collided_x || collided_z;

    player.position += resolved;
    player.on_ground = on_ground;
    player.horizontal_collision = horizontal_collision;
    player.minor_horizontal_collision = horizontal_collision
        && is_minor_horizontal_collision(forward, strafe, sin_y_rot, cos_y_rot, resolved);
    update_supporting_block(
        player,
        world.chunks,
        on_ground,
        resolved,
        &collision_context,
    );

    // Vanilla `Entity.checkFallDamage` receives the resolved movement Y. The
    // narrowing to f32 is intentional and observable by powder-snow collision
    // on the next tick.
    if !player.in_water && resolved.y < 0.0 {
        player.fall_distance -= f64::from(resolved.y as f32);
    }
    if on_ground {
        player.fall_distance = 0.0;
    }

    if collided_x {
        player.velocity.x = 0.0;
    }
    if collided_z {
        player.velocity.z = 0.0;
    }
    // Zero the vertical velocity on ground/ceiling contact (vanilla does this in
    // move()). Gravity is re-applied after the move, leaving vy slightly
    // negative so the next tick's move always probes downward and keeps
    // `on_ground` stable instead of flickering.
    if collided_y {
        player.velocity.y = 0.0;
    }
}

fn update_sprint_state(
    player: &mut LocalPlayer,
    input: &InputState,
    _forward: f32,
    forward_pressed: bool,
    slow_due_to_using_item: bool,
) {
    // Vanilla LocalPlayer.aiStep decrements first, then uses the *previous*
    // ClientInput shift/forward state captured before KeyboardInput.tick while
    // all predicates below read the newly sampled current key state.
    if player.sprint_toggle_timer > 0 {
        player.sprint_toggle_timer -= 1;
    }
    let current_shift = input.performing_action(input::Action::Sneak);
    let current_backward = input.key_pressed(KeyCode::KeyS)
        || input
            .get_gamepad_movement_axes()
            .map(|vec| vec.y < -input::STICK_MOVEMENT_THRESHOLD)
            .unwrap_or(false);
    if player.was_shift_pressed || slow_due_to_using_item || current_backward {
        player.sprint_toggle_timer = 0;
    }

    let in_shallow_water = player.in_water && !player.under_water;
    let sprinting_possible = |allowed_in_shallow_water: bool| {
        player.food > SPRINT_HUNGER_THRESHOLD && (allowed_in_shallow_water || !in_shallow_water)
    };
    let can_start_sprinting = !player.sprinting
        && forward_pressed
        && sprinting_possible(player.flying)
        && !slow_due_to_using_item
        // `LocalPlayer.isMovingSlowly()` is the crouching/visual-crawl gate;
        // vanilla permits that state while underwater.
        && (!is_moving_slowly(player) || player.under_water);

    if can_start_sprinting {
        if !player.was_forward_pressed {
            if player.sprint_toggle_timer > 0 {
                player.sprinting = true;
            } else {
                player.sprint_toggle_timer = DEFAULT_SPRINT_WINDOW;
            }
        }
        if input.performing_action(input::Action::Sprint) {
            player.sprinting = true;
        }
    }

    if player.sprinting {
        if player.swimming {
            // Vanilla LocalPlayer.shouldStopSwimSprinting: hard-wall collision
            // is irrelevant while swimming, and losing forward input only
            // stops sprint when airborne and not descending.
            if !sprinting_possible(true)
                || !player.in_water
                || (!forward_pressed && !player.on_ground && !current_shift)
            {
                player.sprinting = false;
            }
        } else {
            let hard_run_collision =
                player.horizontal_collision && !player.minor_horizontal_collision;
            // Vanilla shouldStopRunSprinting deliberately does not include
            // slowDueToUsingItem; that state blocks starting a new sprint but
            // does not itself cancel one already in progress.
            if !sprinting_possible(player.flying) || !forward_pressed || hard_run_collision {
                player.sprinting = false;
            }
        }
    }
}

/// Vanilla LocalPlayer.aiStep's private `crouching` field. This is evaluated
/// before KeyboardInput.tick from the previously sampled Shift state and can
/// differ for one tick from the physical Entity pose selected at tick end.
fn update_movement_crouching_state(
    player: &mut LocalPlayer,
    chunk_store: &ChunkStore,
    block_entity_anim: &BlockEntityAnimStore,
    is_passenger: bool,
) {
    let descending = player.was_shift_pressed;
    let crouch_fits = can_fit_with_height(
        chunk_store,
        player,
        CROUCH_HEIGHT,
        descending,
        block_entity_anim,
    );
    let stand_fits = can_fit_with_height(
        chunk_store,
        player,
        STANDING_HEIGHT,
        descending,
        block_entity_anim,
    );
    let shift_or_forced_crouch =
        player.was_shift_pressed || (player.sleeping_pos.is_none() && !stand_fits);

    player.movement_crouching = player.game_mode != 3
        && !player.flying
        && !player.swimming
        && !is_passenger
        && crouch_fits
        && shift_or_forced_crouch;
}

// Vanilla Player.updatePlayerPose: swimming is a separate 0.6-tall physical
// pose; if the desired standing/crouching pose will not fit, vanilla falls back
// to CROUCHING and then SWIMMING. Passengers take the desired pose directly.
// Sleeping/fall-flying aren't simulated.
fn update_crouch_state(
    player: &mut LocalPlayer,
    input: &InputState,
    chunk_store: &ChunkStore,
    block_entity_anim: &BlockEntityAnimStore,
    is_passenger: bool,
) {
    let descending = input.performing_action(input::Action::Sneak);
    if !can_fit_with_height(
        chunk_store,
        player,
        crate::player::SWIMMING_HEIGHT,
        descending,
        block_entity_anim,
    ) {
        return;
    }

    if player.swimming {
        player.swimming_pose = true;
        player.crouching = false;
        return;
    }

    let wants_crouch = player.game_mode != 3 && !player.flying && descending;
    if is_passenger {
        player.swimming_pose = false;
        player.crouching = wants_crouch;
        return;
    }

    let crouch_fits = can_fit_with_height(
        chunk_store,
        player,
        CROUCH_HEIGHT,
        descending,
        block_entity_anim,
    );
    let stand_fits = can_fit_with_height(
        chunk_store,
        player,
        STANDING_HEIGHT,
        descending,
        block_entity_anim,
    );

    if wants_crouch && crouch_fits {
        player.swimming_pose = false;
        player.crouching = true;
    } else if stand_fits {
        player.swimming_pose = false;
        player.crouching = false;
    } else if crouch_fits {
        player.swimming_pose = false;
        player.crouching = true;
    } else {
        player.swimming_pose = true;
        player.crouching = false;
    }
}

fn can_fit_with_height(
    chunk_store: &ChunkStore,
    player: &LocalPlayer,
    height: f64,
    descending: bool,
    block_entity_anim: &BlockEntityAnimStore,
) -> bool {
    let context = player_collision_context(player, descending, block_entity_anim);
    no_player_collision(
        chunk_store,
        &Aabb::from_center(player.position.into(), PLAYER_HALF_WIDTH, height / 2.0).deflate(1.0e-7),
        &context,
    )
}

// While holding shift on the ground, clamp the horizontal move so the player
// can't fall further than the step height.
fn back_off_from_edge(
    chunk_store: &ChunkStore,
    bb: &Aabb,
    delta: DVec3,
    shift_down: bool,
    on_ground: bool,
    flying: bool,
    context: &PlayerCollisionContext<'_>,
) -> DVec3 {
    if !shift_down || flying || delta.y > 0.0 {
        return delta;
    }
    // Vanilla `Player.isAboveGround`: an airborne sneaking player only keeps
    // edge-clamping while the accumulated fall distance is still within the
    // remaining max-down-step distance.
    let max_down_step = f64::from(STEP_HEIGHT);
    let above_ground = on_ground
        || (context.fall_distance < max_down_step
            && !can_fall_at_least(
                chunk_store,
                bb,
                0.0,
                0.0,
                max_down_step - context.fall_distance,
                context,
            ));
    if !above_ground {
        return delta;
    }

    let mut dx = delta.x;
    let mut dz = delta.z;
    let step_x = dx.signum() * 0.05;
    let step_z = dz.signum() * 0.05;

    while dx != 0.0 && can_fall_at_least(chunk_store, bb, dx, 0.0, f64::from(STEP_HEIGHT), context)
    {
        if dx.abs() <= 0.05 {
            dx = 0.0;
            break;
        }
        dx -= step_x;
    }
    while dz != 0.0 && can_fall_at_least(chunk_store, bb, 0.0, dz, f64::from(STEP_HEIGHT), context)
    {
        if dz.abs() <= 0.05 {
            dz = 0.0;
            break;
        }
        dz -= step_z;
    }
    while dx != 0.0
        && dz != 0.0
        && can_fall_at_least(chunk_store, bb, dx, dz, f64::from(STEP_HEIGHT), context)
    {
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
    chunk_store: &ChunkStore,
    bb: &Aabb,
    dx: f64,
    dz: f64,
    min_height: f64,
    context: &PlayerCollisionContext<'_>,
) -> bool {
    no_player_collision(
        chunk_store,
        &Aabb::new(
            dvec3(
                bb.min.x + 1.0e-7 + dx,
                bb.min.y - min_height - 1.0e-7,
                bb.min.z + 1.0e-7 + dz,
            ),
            dvec3(bb.max.x - 1.0e-7 + dx, bb.min.y, bb.max.z - 1.0e-7 + dz),
        ),
        context,
    )
}

fn movement_speed(sprinting: bool) -> f32 {
    let mut speed = MOVEMENT_SPEED_ATTRIBUTE;
    if sprinting {
        speed *= 1.0 + SPRINT_SPEED_MODIFIER;
    }
    speed as f32
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

fn player_collision_context<'a>(
    player: &LocalPlayer,
    descending: bool,
    block_entity_anim: &'a BlockEntityAnimStore,
) -> PlayerCollisionContext<'a> {
    let leather_boots = matches!(
        player.inventory.slot(crate::player::inventory::ARMOR_START + 3),
        azalea_inventory::ItemStack::Present(data)
            if data.kind == azalea_registry::builtin::ItemKind::LeatherBoots
    );
    PlayerCollisionContext {
        feet_y: player.position.y,
        descending,
        fall_distance: player.fall_distance,
        leather_boots,
        block_entity_anim: Some(block_entity_anim),
    }
}

fn block_state_at_player_position(
    player: &LocalPlayer,
    chunk_store: &ChunkStore,
) -> azalea_block::BlockState {
    chunk_store.get_block_state(
        player.position.x.floor() as i32,
        player.position.y.floor() as i32,
        player.position.z.floor() as i32,
    )
}

fn block_below_affecting_movement(
    player: &LocalPlayer,
    chunk_store: &ChunkStore,
) -> azalea_block::BlockState {
    let offset = f64::from(0.500_001_f32);
    let y = (player.position.y - offset).floor() as i32;

    if let Some(support) = player.main_supporting_block_pos {
        let support_state = chunk_store.get_block_state(support.x, support.y, support.z);
        let support_id = crate::world::block::block_id(support_state);
        // Preserve vanilla's special supporting-block Y for walls and gates;
        // other blocks retain the support's X/Z but use the offset-derived Y.
        if support_id.ends_with("_wall") || support_id.ends_with("_fence_gate") {
            return support_state;
        }
        return chunk_store.get_block_state(support.x, y, support.z);
    }

    chunk_store.get_block_state(
        player.position.x.floor() as i32,
        y,
        player.position.z.floor() as i32,
    )
}

fn update_supporting_block(
    player: &mut LocalPlayer,
    chunk_store: &ChunkStore,
    on_ground: bool,
    movement: DVec3,
    context: &PlayerCollisionContext<'_>,
) {
    if !on_ground {
        player.on_ground_no_blocks = false;
        player.main_supporting_block_pos = None;
        return;
    }

    let bounding_box = player.bounding_box();
    let test_area = Aabb::new(
        dvec3(
            bounding_box.min.x,
            bounding_box.min.y - 1.0e-6,
            bounding_box.min.z,
        ),
        dvec3(bounding_box.max.x, bounding_box.min.y, bounding_box.max.z),
    );
    let entity_position: DVec3 = player.position.into();
    let mut support =
        find_supporting_block(chunk_store, &test_area, entity_position, Some(context));

    if support.is_some() || player.on_ground_no_blocks {
        player.main_supporting_block_pos = support;
    } else {
        let previous_test_area = test_area.offset(dvec3(-movement.x, 0.0, -movement.z));
        support = find_supporting_block(
            chunk_store,
            &previous_test_area,
            entity_position,
            Some(context),
        );
        player.main_supporting_block_pos = support;
    }
    player.on_ground_no_blocks = support.is_none();
}

fn block_jump_factor(player: &LocalPlayer, chunk_store: &ChunkStore) -> f32 {
    let here_factor = movement_jump_factor(block_state_at_player_position(player, chunk_store));
    if f64::from(here_factor) == 1.0 {
        movement_jump_factor(block_below_affecting_movement(player, chunk_store))
    } else {
        here_factor
    }
}

fn block_speed_factor(player: &LocalPlayer, chunk_store: &ChunkStore) -> f32 {
    let here = block_state_at_player_position(player, chunk_store);
    let here_factor = movement_speed_factor(here);
    if crate::world::block::block_id(here) == "water"
        || crate::world::block::block_id(here) == "bubble_column"
        || f64::from(here_factor) != 1.0
    {
        here_factor
    } else {
        movement_speed_factor(block_below_affecting_movement(player, chunk_store))
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
    // Vanilla LocalPlayer.isMovingSlowly = isCrouching || isVisuallyCrawling.
    // LocalPlayer.isCrouching() returns its private aiStep movement flag, not
    // the physical Entity pose selected later by Player.updatePlayerPose.
    // Visual crawling is the one-tick surface-exit state where the shared
    // swimming flag has cleared but the physical SWIMMING pose remains.
    player.movement_crouching || (player.swimming_pose && !player.in_water)
}

fn movement_input(
    input: &InputState,
    moving_slowly: bool,
    use_speed_multiplier: f32,
) -> (f32, f32) {
    // Keep the LocalPlayer input pipeline in float exactly like vanilla. Pomme's
    // analog stick is already clamped to unit length; keyboard input is first
    // normalized just like KeyboardInput.tick(). `strafe` follows vanilla xxa:
    // positive is left, negative is right.
    let (mut strafe, mut forward) = if let Some(analog) = input.get_gamepad_movement_axes() {
        (analog.x, analog.y)
    } else {
        let mut forward = 0.0_f32;
        let mut strafe = 0.0_f32;
        if input.key_pressed(KeyCode::KeyW) {
            forward += 1.0;
        }
        if input.key_pressed(KeyCode::KeyS) {
            forward -= 1.0;
        }
        if input.key_pressed(KeyCode::KeyA) {
            strafe += 1.0;
        }
        if input.key_pressed(KeyCode::KeyD) {
            strafe -= 1.0;
        }
        normalize_vec2(strafe, forward)
    };

    if strafe == 0.0 && forward == 0.0 {
        return (forward, strafe);
    }

    strafe *= INPUT_DAMPING;
    forward *= INPUT_DAMPING;
    strafe *= use_speed_multiplier;
    forward *= use_speed_multiplier;

    if moving_slowly {
        strafe *= SNEAKING_SPEED;
        forward *= SNEAKING_SPEED;
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

fn mth_sin(angle: f32) -> f32 {
    let index = ((f64::from(angle) * SIN_SCALE) as i64 & 0xFFFF) as usize;
    azalea_core::math::SIN[index]
}

fn mth_cos(angle: f32) -> f32 {
    let index = ((f64::from(angle) * SIN_SCALE + 16_384.0) as i64 & 0xFFFF) as usize;
    azalea_core::math::SIN[index]
}

fn vanilla_yaw_sin_cos(yaw_degrees: f32) -> (f32, f32) {
    let angle = yaw_degrees * DEG_TO_RAD;
    (mth_sin(angle), mth_cos(angle))
}

fn vanilla_look_y(pitch_degrees: f32) -> f64 {
    let angle = pitch_degrees * DEG_TO_RAD;
    -f64::from(mth_sin(angle))
}

#[cfg(test)]
mod tests {
    use azalea_core::position::ChunkPos;
    use azalea_world::chunk::Chunk;

    use super::*;
    use crate::player::{CROUCH_EYE_HEIGHT, STANDING_EYE_HEIGHT};

    fn loaded_test_store() -> ChunkStore {
        crate::world::block::init("26.2");
        let mut chunks = ChunkStore::new(2);
        chunks.partial_storage.set(
            &ChunkPos::new(0, 0),
            Some(Chunk::default()),
            &mut chunks.chunk_storage,
        );
        chunks
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
    fn movement_speed_matches_vanilla_attribute_rounding() {
        assert_eq!(movement_speed(false).to_bits(), 0x3dcccccd);
        assert_eq!(movement_speed(true).to_bits(), 0x3e051eb9);

        let mut player = LocalPlayer::new();
        player.on_ground = true;
        assert_eq!(
            friction_influenced_speed(movement_speed(false), &player, 0.6_f32).to_bits(),
            0x3dcccccd
        );
        assert_eq!(
            friction_influenced_speed(movement_speed(true), &player, 0.6_f32).to_bits(),
            0x3e051eb9
        );
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
        crate::world::block::init("26.2");
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
            &chunks,
            &block_entity_anim,
            1.0,
            false,
            false,
        );
        tick(
            &mut plain,
            &plain_input,
            &chunks,
            &block_entity_anim,
            1.0,
            false,
            false,
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
            &chunks,
            &block_entity_anim,
            1.0,
            false,
            false,
        );
        let second_shifted_horizontal_speed =
            shifted.velocity.x * shifted.velocity.x + shifted.velocity.z * shifted.velocity.z;

        let first_plain_horizontal_speed =
            plain.velocity.x * plain.velocity.x + plain.velocity.z * plain.velocity.z;
        tick(
            &mut plain,
            &plain_input,
            &chunks,
            &block_entity_anim,
            1.0,
            false,
            false,
        );
        let second_plain_horizontal_speed =
            plain.velocity.x * plain.velocity.x + plain.velocity.z * plain.velocity.z;

        assert_eq!(first_shifted_horizontal_speed, first_plain_horizontal_speed);
        assert!(second_shifted_horizontal_speed < second_plain_horizontal_speed);
        assert!(shifted.movement_crouching);

        // Release Shift. Vanilla LocalPlayer.aiStep still computes its private
        // crouching flag from the previous sampled Shift=true before
        // KeyboardInput.tick observes the release, so this movement tick stays
        // slowed. Player.updatePlayerPose then sees the new release and returns
        // the physical pose to standing at tick end.
        tick(
            &mut shifted,
            &plain_input,
            &chunks,
            &block_entity_anim,
            1.0,
            false,
            false,
        );
        assert!(shifted.movement_crouching);
        assert!(
            !shifted.crouching,
            "physical pose must stand at the end of the Shift-release tick"
        );

        // The following tick's private movement flag observes the previously
        // sampled release and no longer slows input.
        tick(
            &mut shifted,
            &plain_input,
            &chunks,
            &block_entity_anim,
            1.0,
            false,
            false,
        );
        assert!(!shifted.movement_crouching);
    }

    #[test]
    fn passenger_clears_private_crouching_and_uses_desired_pose_directly() {
        let chunks = loaded_test_store();
        let anim = BlockEntityAnimStore::default();
        let mut player = LocalPlayer::new();
        player.position = dvec3(8.5, 64.0, 8.5).into();
        player.was_shift_pressed = true;

        update_movement_crouching_state(&mut player, &chunks, &anim, true);
        assert!(
            !player.movement_crouching,
            "LocalPlayer private crouching flag is forced false while passenger"
        );

        // Leave only 1 block of headroom. SWIMMING (0.6 tall) fits, CROUCHING
        // (1.5 tall) does not. Vanilla passengers still take desired CROUCHING
        // directly after the initial SWIMMING-fit guard.
        let stone = crate::world::block::find_state("stone", &[]);
        chunks.set_block_state(8, 65, 8, stone);
        let mut input = InputState::released();
        input.set_key_pressed_for_test(KeyCode::ShiftLeft, true);
        update_crouch_state(&mut player, &input, &chunks, &anim, true);
        assert!(player.crouching);
        assert!(!player.swimming_pose);

        player.crouching = false;
        player.swimming_pose = false;
        update_crouch_state(&mut player, &input, &chunks, &anim, false);
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
        let chunks = loaded_test_store();
        let source = crate::world::block::find_state("water", &[("level", "0")]);
        for x in 7..=9 {
            for y in 64..=66 {
                for z in 7..=9 {
                    chunks.set_block_state(x, y, z, source);
                }
            }
        }

        let mut player = LocalPlayer::new();
        player.position = dvec3(8.5, 64.0, 8.5).into();
        player.velocity.y = f64::from(LIQUID_JUMP_ACCELERATION) + 0.001;
        player.look_dir = crate::entity::components::LookDirection::new(0.0, -38.0);
        player.sprinting = true;
        player.swimming = true;
        player.swimming_pose = true;
        player.eyes_in_water = true;
        player.food = 20;

        let mut input = InputState::released();
        input.set_key_pressed_for_test(KeyCode::KeyW, true);
        input.set_key_pressed_for_test(KeyCode::ShiftLeft, true);
        let anim = BlockEntityAnimStore::default();

        tick(&mut player, &input, &chunks, &anim, 1.0, false, false);

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
        let chunks = loaded_test_store();
        let falling = crate::world::block::find_state("water", &[("level", "8")]);
        let low = crate::world::block::find_state("water", &[("level", "7")]);
        let stone = crate::world::block::find_state("stone", &[]);
        chunks.set_block_state(8, 64, 8, falling);
        chunks.set_block_state(9, 64, 8, low);
        chunks.set_block_state(8, 64, 7, stone);

        let mut player = LocalPlayer::new();
        player.position = dvec3(8.5, 64.0, 8.5).into();
        player.refresh_water_interaction(&chunks);

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
        player.sprinting = true;

        assert!(
            is_moving_slowly(&player),
            "Pose.SWIMMING outside water is vanilla isVisuallyCrawling()"
        );

        let mut input = InputState::released();
        input.set_key_pressed_for_test(KeyCode::KeyW, true);
        let (slowed_forward, _) = movement_input(&input, is_moving_slowly(&player), 1.0);
        let (full_forward, _) = movement_input(&input, false, 1.0);

        assert_eq!(full_forward.to_bits(), INPUT_DAMPING.to_bits());
        assert_eq!(
            slowed_forward.to_bits(),
            (INPUT_DAMPING * SNEAKING_SPEED).to_bits()
        );

        // On the first dry tick after swimming, sprinting is still active but
        // the retained SWIMMING pose makes LocalPlayer.modifyInput slow the
        // current forward input. The exact positional impulse Pomme used to
        // over-predict is the recurring Grim surface-exit signature.
        let observed_signature =
            f64::from((full_forward - slowed_forward) * SPRINT_AIR_ACCELERATION);
        assert!(
            (observed_signature - 0.017_835_998_88).abs() < 2.0e-9,
            "surface-exit Grim signature: {observed_signature:.12}"
        );
    }

    #[test]
    fn dry_move_into_partial_flowing_water_applies_current_before_ground_friction() {
        let wet = loaded_test_store();
        let dry = loaded_test_store();
        let stone = crate::world::block::find_state("stone", &[]);
        for x in 6..=10 {
            for z in 7..=9 {
                wet.set_block_state(x, 63, z, stone);
                dry.set_block_state(x, 63, z, stone);
            }
        }

        // Level 3 has own height 5/9 (> 0.4), so EntityFluidInteraction does
        // not attenuate its current by submerged height. A source-water
        // neighbor to the east makes this cell's normalized flow point west.
        let partial = crate::world::block::find_state("water", &[("level", "3")]);
        let source = crate::world::block::find_state("water", &[("level", "0")]);
        wet.set_block_state(8, 64, 8, partial);
        wet.set_block_state(9, 64, 8, source);

        let mut wet_player = LocalPlayer::new();
        wet_player.position = dvec3(7.65, 64.0, 8.5).into();
        wet_player.velocity = Velocity::new(0.08, 0.0, 0.0);
        wet_player.on_ground = true;
        let mut dry_player = LocalPlayer::new();
        dry_player.position = wet_player.position;
        dry_player.velocity = Velocity::new(0.08, 0.0, 0.0);
        dry_player.on_ground = true;

        let neutral = InputState::released();
        let anim = BlockEntityAnimStore::default();
        tick_land(
            &mut wet_player,
            &neutral,
            MovementWorld {
                chunks: &wet,
                block_entity_anim: &anim,
            },
            0.0,
            0.0,
            0.0,
            1.0,
        );
        tick_land(
            &mut dry_player,
            &neutral,
            MovementWorld {
                chunks: &dry,
                block_entity_anim: &anim,
            },
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
        let chunks = loaded_test_store();
        let water = crate::world::block::find_state("water", &[("level", "0")]);
        chunks.set_block_state(8, 64, 8, water);

        let wet_box = Aabb::new(dvec3(8.2, 64.2, 8.2), dvec3(8.8, 65.8, 8.8));
        assert!(
            !crate::world::block::has_collision(water),
            "water has no collision shape"
        );
        assert!(
            contains_any_liquid(&chunks, &wet_box),
            "vanilla Entity.isFree must still reject a collision-free box containing fluid"
        );

        let dry_box = wet_box.offset(dvec3(2.0, 0.0, 0.0));
        assert!(!contains_any_liquid(&chunks, &dry_box));
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
        player.sprinting = true;
        player.swimming = true;
        player.in_water = true;
        player.food = 20;
        player.horizontal_collision = true;
        player.minor_horizontal_collision = false;

        let released = InputState::released();
        player.on_ground = true;
        update_sprint_state(&mut player, &released, 0.0, false, false);
        assert!(
            player.sprinting,
            "vanilla swim sprint ignores hard-wall collision and may persist without forward input on ground"
        );

        player.on_ground = false;
        update_sprint_state(&mut player, &released, 0.0, false, false);
        assert!(
            !player.sprinting,
            "airborne swim sprint stops when forward input is lost"
        );

        let mut descending = InputState::released();
        descending.set_key_pressed_for_test(KeyCode::ShiftLeft, true);
        player.sprinting = true;
        update_sprint_state(&mut player, &descending, 0.0, false, false);
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
        update_sprint_state(&mut player, &forward, 0.98, true, false);
        assert_eq!(
            player.sprint_toggle_timer, DEFAULT_SPRINT_WINDOW,
            "previous shift clears the old window before the new forward edge arms a fresh one"
        );
        assert!(!player.sprinting);

        player.sprint_toggle_timer = 4;
        player.was_shift_pressed = false;
        player.was_forward_pressed = true;
        let mut backward = InputState::released();
        backward.set_key_pressed_for_test(KeyCode::KeyS, true);
        update_sprint_state(&mut player, &backward, -0.98, false, false);
        assert_eq!(player.sprint_toggle_timer, 0);
    }

    #[test]
    fn item_slowdown_blocks_start_but_does_not_cancel_existing_sprint() {
        let mut input = InputState::released();
        input.set_key_pressed_for_test(KeyCode::KeyW, true);
        input.set_key_pressed_for_test(KeyCode::ControlLeft, true);

        let mut player = LocalPlayer::new();
        player.food = 20;
        update_sprint_state(&mut player, &input, 0.196, true, true);
        assert!(!player.sprinting, "item slowdown blocks canStartSprinting");

        player.sprinting = true;
        player.was_forward_pressed = true;
        update_sprint_state(&mut player, &input, 0.196, true, true);
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
        player.sprinting = true;
        player.swimming = false;
        player.in_water = true;
        player.under_water = false;
        player.food = 20;

        update_sprint_state(&mut player, &input, 0.98, true, false);

        assert!(
            !player.sprinting,
            "vanilla shouldStopRunSprinting cancels sprint in shallow water even while Sprint remains held"
        );
    }

    #[test]
    fn previous_hard_wall_collision_stops_run_sprint_but_minor_collision_does_not() {
        let input = InputState::released();
        let mut player = LocalPlayer::new();
        player.sprinting = true;
        player.was_forward_pressed = true;
        player.horizontal_collision = true;
        player.minor_horizontal_collision = false;

        update_sprint_state(&mut player, &input, 0.98, true, false);
        assert!(
            !player.sprinting,
            "vanilla shouldStopRunSprinting consumes the prior hard wall collision"
        );

        player.sprinting = true;
        player.minor_horizontal_collision = true;
        update_sprint_state(&mut player, &input, 0.98, true, false);
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
        let (dx, dz) = movement_delta(forward, 0.0, movement_speed(false), sin, cos);
        assert_eq!(dx.to_bits(), 0xbfa999996d18578d);
        assert_eq!(dz.to_bits(), 0x3fa999996d18578d);

        assert_eq!(vanilla_look_y(30.0).to_bits(), 0xbfdfff8be0000000);
    }

    #[test]
    fn dead_player_keeps_zero_input_air_travel() {
        crate::world::block::init("26.2");
        let mut player = LocalPlayer::new();
        player.position = dvec3(0.0, 80.0, 0.0).into();
        player.velocity = crate::entity::components::Velocity::new(0.25, 0.0, -0.1);
        player.sprinting = true;
        player.crouching = true;
        player.eye_height = 1.27;
        player.prev_eye_height = 1.27;
        let chunks = ChunkStore::new(2);
        let block_entity_anim = BlockEntityAnimStore::default();

        player.death_time = 1;
        let starting_height = player.height();
        tick_dead(&mut player, &chunks, &block_entity_anim, false);

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
        tick_dead(&mut player, &chunks, &block_entity_anim, false);
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
}
