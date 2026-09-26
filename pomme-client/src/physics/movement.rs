// TODO: fall damage - track fall distance, reset on water entry, apply damage
// on ground impact; Player.causeFallDamage returns false when may_fly, and
// fall distance resets every tick while flying

use glam::{DVec3, dvec3};
use winit::keyboard::KeyCode;

use super::aabb::Aabb;
use super::block_shape::CollisionContext;
use super::collision::{no_collision, resolve_collision};
use crate::app::input::{self, InputState};
use crate::player::{CROUCH_HEIGHT, LocalPlayer, PLAYER_HALF_WIDTH, STANDING_HEIGHT};
use crate::world::chunk::ChunkStore;

const GRAVITY: f64 = 0.08;
// Vanilla mixes float and double physics values. Keep float values as f32 until
// the exact point where vanilla widens them into Vec3/AABB doubles.
const JUMP_VELOCITY: f32 = 0.42;
const VERTICAL_DRAG: f32 = 0.98;
const HORIZONTAL_DRAG: f32 = 0.91;
const BLOCK_FRICTION: f32 = 0.6;
const GROUND_FRICTION: f32 = BLOCK_FRICTION * HORIZONTAL_DRAG;
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
// TODO: vanilla `travelInWater` applies the 0.8 drag first and then
// `getFluidFallingAdjustedMovement` (gravity / 16 with the -0.003 falling
// clamp); pomme still subtracts this placeholder before the drag.
const WATER_GRAVITY: f64 = 0.02;
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

pub fn tick(
    player: &mut LocalPlayer,
    input: &InputState,
    chunk_store: &ChunkStore,
    use_speed_multiplier: f32,
    slow_due_to_using_item: bool,
) {
    let raw_jump_held = input.performing_action(input::Action::Jump);

    // Vanilla `LivingEntity.aiStep`.
    if player.no_jump_delay > 0 {
        player.no_jump_delay -= 1;
    }

    player.update_water_state(chunk_store);
    update_crouch_state(player, input, chunk_store);
    player.tick_eye_height();

    // Vanilla `LocalPlayer.modifyInput` keeps the entire input pipeline in
    // float: damping, item-use slowdown, sneaking slowdown, then square remap.
    let (mut forward, mut strafe) = movement_input(input, player.crouching, use_speed_multiplier);
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

    update_fly_state(player, input, sin_y_rot, cos_y_rot);

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

    // `Player.isImmobile` (asleep) zeroes locomotion and jump input; travel
    // still runs.
    let mut jump_held = raw_jump_held;
    apply_living_immobility(player, &mut forward, &mut strafe, &mut jump_held);

    // Vanilla `LivingEntity.aiStep`: swim upward when submerged past the jump
    // threshold, otherwise a full jump off the ground or the shallow-fluid floor.
    if jump_held {
        let in_water = player.in_water && player.fluid_height > 0.0;
        if in_water && (!player.on_ground || player.fluid_height > FLUID_JUMP_THRESHOLD) {
            player.velocity.y += f64::from(LIQUID_JUMP_ACCELERATION);
        } else if (player.on_ground || (in_water && player.fluid_height <= FLUID_JUMP_THRESHOLD))
            && player.no_jump_delay == 0
        {
            jump_from_ground(player, sin_y_rot, cos_y_rot);
            player.no_jump_delay = JUMP_DELAY_TICKS;
        }
    } else {
        player.no_jump_delay = 0;
    }

    travel(
        player,
        input,
        chunk_store,
        forward,
        strafe,
        sin_y_rot,
        cos_y_rot,
    );

    player.tick_air_supply();
    stop_flying_on_ground(player);

    player.was_forward_pressed = forward_pressed;
    player.was_jump_pressed = raw_jump_held;
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

/// Vanilla `LivingEntity.travel`: the water or air routine for this tick.
fn travel(
    player: &mut LocalPlayer,
    input: &InputState,
    chunk_store: &ChunkStore,
    forward: f32,
    strafe: f32,
    sin_y_rot: f32,
    cos_y_rot: f32,
) {
    if player.in_water {
        tick_water(
            player,
            input,
            chunk_store,
            forward,
            strafe,
            sin_y_rot,
            cos_y_rot,
        );
    } else {
        tick_land(
            player,
            input,
            chunk_store,
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
pub fn tick_dead(player: &mut LocalPlayer, chunk_store: &ChunkStore) {
    player.no_jump_delay = 0;
    player.sprinting = false;

    // Local players enter death through SetHealth; entity event 3 intentionally
    // skips LivingEntity.die for players, so the current ordinary player pose
    // remains authoritative until Player.updatePlayerPose runs at tick end.
    let neutral = InputState::released();
    player.update_water_state(chunk_store);
    player.tick_eye_height();

    let (sin_y_rot, cos_y_rot) = vanilla_yaw_sin_cos(player.look_dir.y_rot_deg());
    travel(
        player,
        &neutral,
        chunk_store,
        0.0,
        0.0,
        sin_y_rot,
        cos_y_rot,
    );

    // Player.updatePlayerPose runs after LivingEntity.tick in vanilla. With
    // death-screen input released, this becomes standing unless clearance keeps
    // the player in the crouching pose for the following tick.
    update_crouch_state(player, &neutral, chunk_store);

    stop_flying_on_ground(player);
    player.was_forward_pressed = false;
    player.was_jump_pressed = false;
}

// Vanilla `LocalPlayer.aiStep`: a fresh jump press arms the toggle window;
// a second one inside it toggles flight.
fn update_fly_state(player: &mut LocalPlayer, input: &InputState, sin_y_rot: f32, cos_y_rot: f32) {
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
                    jump_from_ground(player, sin_y_rot, cos_y_rot);
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

fn jump_from_ground(player: &mut LocalPlayer, sin_y_rot: f32, cos_y_rot: f32) {
    player.velocity.y = f64::from(JUMP_VELOCITY).max(player.velocity.y);

    if player.sprinting {
        player.velocity.x += f64::from(-sin_y_rot) * SPRINT_JUMP_BOOST;
        player.velocity.z += f64::from(cos_y_rot) * SPRINT_JUMP_BOOST;
    }
}

fn tick_land(
    player: &mut LocalPlayer,
    input: &InputState,
    chunk_store: &ChunkStore,
    forward: f32,
    strafe: f32,
    sin_y_rot: f32,
    cos_y_rot: f32,
) {
    // Vanilla `travelInAir` samples on-ground once before the move and reuses
    // it for the end-of-tick drag, so a jump launches with ground friction.
    let on_ground_at_start = player.on_ground;

    let saved_vy = player.velocity.y;
    let speed = movement_speed(player.sprinting);
    let accel = friction_influenced_speed(speed, player, BLOCK_FRICTION);
    let (move_x, move_z) = movement_delta(forward, strafe, accel, sin_y_rot, cos_y_rot);
    player.velocity.x += move_x;
    player.velocity.z += move_z;

    apply_collision(
        player,
        input,
        chunk_store,
        forward,
        strafe,
        sin_y_rot,
        cos_y_rot,
    );

    player.velocity.y -= GRAVITY;
    player.velocity.y *= f64::from(VERTICAL_DRAG);

    let h_friction = if on_ground_at_start {
        GROUND_FRICTION
    } else {
        HORIZONTAL_DRAG
    };
    player.velocity.x *= f64::from(h_friction);
    player.velocity.z *= f64::from(h_friction);

    overwrite_flying_vy(player, saved_vy);
}

fn tick_water(
    player: &mut LocalPlayer,
    input: &InputState,
    chunk_store: &ChunkStore,
    forward: f32,
    strafe: f32,
    sin_y_rot: f32,
    cos_y_rot: f32,
) {
    if input.performing_action(input::Action::Sneak) {
        player.velocity.y -= f64::from(LIQUID_JUMP_ACCELERATION);
    }

    let (move_x, move_z) =
        movement_delta(forward, strafe, WATER_ACCELERATION, sin_y_rot, cos_y_rot);
    player.velocity.x += move_x;
    player.velocity.z += move_z;

    if player.swimming {
        let target_vy = vanilla_look_y(player.look_dir.x_rot_deg());
        let boost = if target_vy < -0.2 { 0.085 } else { 0.06 };
        player.velocity.y += (target_vy - player.velocity.y) * boost;
    }

    let saved_vy = player.velocity.y;

    apply_collision(
        player,
        input,
        chunk_store,
        forward,
        strafe,
        sin_y_rot,
        cos_y_rot,
    );

    let h_drag = if player.sprinting {
        WATER_HORIZONTAL_DRAG_SPRINT
    } else {
        WATER_HORIZONTAL_DRAG
    };
    player.velocity.x *= f64::from(h_drag);
    player.velocity.z *= f64::from(h_drag);

    let gravity = if player.velocity.y <= 0.0 && !player.swimming {
        GRAVITY * 0.25
    } else {
        WATER_GRAVITY
    };
    player.velocity.y -= gravity;
    player.velocity.y *= f64::from(WATER_VERTICAL_DRAG);

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

fn apply_collision(
    player: &mut LocalPlayer,
    input: &InputState,
    chunk_store: &ChunkStore,
    forward: f32,
    strafe: f32,
    sin_y_rot: f32,
    cos_y_rot: f32,
) {
    let aabb = player.bounding_box();
    let ctx = collision_context(player, input);
    let delta = back_off_from_edge(
        chunk_store,
        &aabb,
        *player.velocity,
        input.performing_action(input::Action::Sneak),
        player.on_ground,
        player.flying,
        &ctx,
    );
    let step_height = if player.on_ground {
        f64::from(STEP_HEIGHT)
    } else {
        0.0
    };
    let (resolved, on_ground) =
        resolve_collision(chunk_store, aabb, delta.into(), step_height, &ctx);

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

    if player.sprinting
        && horizontal_collision
        && forward > 0.0
        && !is_minor_horizontal_collision(forward, strafe, sin_y_rot, cos_y_rot, resolved)
    {
        player.sprinting = false;
    }
}

fn update_sprint_state(
    player: &mut LocalPlayer,
    input: &InputState,
    forward: f32,
    forward_pressed: bool,
    slow_due_to_using_item: bool,
) {
    if player.sprint_toggle_timer > 0 {
        player.sprint_toggle_timer -= 1;
    }
    if input.performing_action(input::Action::Sneak) || slow_due_to_using_item {
        player.sprint_toggle_timer = 0;
    }

    // Crouching blocks starting a sprint but doesn't stop one in progress.
    // Vanilla `canStartSprinting` also denies it while slowed by an item use,
    // and the slowed input impulse (< 0.8) stops a sprint in progress.
    let can_sprint = forward > 0.0
        && player.food > SPRINT_HUNGER_THRESHOLD
        && !player.crouching
        && !slow_due_to_using_item;

    if input.performing_action(input::Action::Sprint) && can_sprint {
        player.sprinting = true;
    }

    if !player.was_forward_pressed && forward_pressed && can_sprint {
        if player.sprint_toggle_timer > 0 {
            player.sprinting = true;
        }
        player.sprint_toggle_timer = DEFAULT_SPRINT_WINDOW;
    }

    if player.sprinting
        && (forward <= 0.0 || player.food <= SPRINT_HUNGER_THRESHOLD || slow_due_to_using_item)
    {
        player.sprinting = false;
    }
}

// `LocalPlayer.aiStep`: forces the crouch pose under ceilings too low to
// stand in, unless asleep. Riding isn't simulated.
fn update_crouch_state(player: &mut LocalPlayer, input: &InputState, chunk_store: &ChunkStore) {
    let ctx = collision_context(player, input);
    let pos = player.position.into();
    player.crouching = player.game_mode != 3
        && !player.flying
        && !player.swimming
        && can_fit_with_height(chunk_store, pos, CROUCH_HEIGHT, &ctx)
        && (input.performing_action(input::Action::Sneak)
            || !player.is_sleeping()
                && !can_fit_with_height(chunk_store, pos, STANDING_HEIGHT, &ctx));
}

/// `CollisionContext.of(player)`.
fn collision_context(player: &LocalPlayer, input: &InputState) -> CollisionContext {
    CollisionContext::entity(
        player.position.y,
        input.performing_action(input::Action::Sneak),
        player.inventory.wears_leather_boots(),
    )
}

fn can_fit_with_height(
    chunk_store: &ChunkStore,
    pos: DVec3,
    height: f64,
    ctx: &CollisionContext,
) -> bool {
    no_collision(
        chunk_store,
        &Aabb::from_center(pos, PLAYER_HALF_WIDTH, height / 2.0).deflate(1.0e-7),
        ctx,
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
    ctx: &CollisionContext,
) -> DVec3 {
    if !shift_down || flying || delta.y > 0.0 {
        return delta;
    }
    let can_fall = |dx, dz| can_fall_at_least(chunk_store, bb, dx, dz, f64::from(STEP_HEIGHT), ctx);
    // TODO: fall distance - falling less than the step height still counts
    // as above ground
    let above_ground = on_ground || !can_fall(0.0, 0.0);
    if !above_ground {
        return delta;
    }

    let mut dx = delta.x;
    let mut dz = delta.z;
    let step_x = dx.signum() * 0.05;
    let step_z = dz.signum() * 0.05;

    while dx != 0.0 && can_fall(dx, 0.0) {
        if dx.abs() <= 0.05 {
            dx = 0.0;
            break;
        }
        dx -= step_x;
    }
    while dz != 0.0 && can_fall(0.0, dz) {
        if dz.abs() <= 0.05 {
            dz = 0.0;
            break;
        }
        dz -= step_z;
    }
    while dx != 0.0 && dz != 0.0 && can_fall(dx, dz) {
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
    ctx: &CollisionContext,
) -> bool {
    no_collision(
        chunk_store,
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

fn movement_input(input: &InputState, crouching: bool, use_speed_multiplier: f32) -> (f32, f32) {
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

    if crouching {
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
    use super::*;
    use crate::player::{CROUCH_EYE_HEIGHT, STANDING_EYE_HEIGHT};

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
        assert_eq!(BLOCK_FRICTION.to_bits(), 0x3f19999a);
        assert_eq!(GROUND_FRICTION.to_bits(), 0x3f0bc6a9);
        assert_eq!(GROUND_ACCEL_FACTOR.to_bits(), 0x3e5d2f1c);
        assert_eq!(WATER_GRAVITY.to_bits(), 0x3f947ae147ae147b);
        assert_eq!(MTH_EQUAL_EPSILON.to_bits(), 0x3ee4f8b580000000);
        assert_eq!(MINOR_COLLISION_ANGLE.to_bits(), 0x3fc1df46a0000000);
    }

    #[test]
    fn movement_speed_matches_vanilla_attribute_rounding() {
        assert_eq!(movement_speed(false).to_bits(), 0x3dcccccd);
        assert_eq!(movement_speed(true).to_bits(), 0x3e051eb9);

        let mut player = LocalPlayer::new();
        player.on_ground = true;
        assert_eq!(
            friction_influenced_speed(movement_speed(false), &player, BLOCK_FRICTION).to_bits(),
            0x3dcccccd
        );
        assert_eq!(
            friction_influenced_speed(movement_speed(true), &player, BLOCK_FRICTION).to_bits(),
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
    fn sleeping_player_is_immobile_without_freezing_travel() {
        let mut player = LocalPlayer::new();
        player.sleeping_pos = Some(azalea_core::position::BlockPos::new(0, 64, 0));

        let mut forward = 0.75;
        let mut strafe = -0.25;
        let mut jumping = true;
        apply_living_immobility(&player, &mut forward, &mut strafe, &mut jumping);
        assert_eq!((forward, strafe, jumping), (0.0, 0.0, false));

        crate::world::block::init("26.2");
        player.position = dvec3(0.0, 80.0, 0.0).into();
        player.velocity = crate::entity::components::Velocity::new(0.25, 0.0, -0.1);
        let chunks = ChunkStore::new(2);
        tick(&mut player, &InputState::released(), &chunks, 1.0, false);
        assert!(player.position.x > 0.0);
        assert!(player.position.z < 0.0);
        assert!(player.position.y <= 80.0);
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

        player.death_time = 1;
        let starting_height = player.height();
        tick_dead(&mut player, &chunks);

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
        tick_dead(&mut player, &chunks);
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
