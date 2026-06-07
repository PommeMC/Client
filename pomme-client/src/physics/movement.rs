// TODO: fall damage - track fall distance, reset on water entry, apply damage
// on ground impact

use glam::DVec3;
use winit::keyboard::KeyCode;

use super::aabb::Aabb;
use super::collision::resolve_collision;
use crate::app::input::InputState;
use crate::player::LocalPlayer;
use crate::world::chunk::ChunkStore;

const GRAVITY: f64 = 0.08;
const JUMP_VELOCITY: f64 = 0.42;
const VERTICAL_DRAG: f32 = 0.98;
const HORIZONTAL_DRAG: f64 = 0.91;
const BLOCK_FRICTION: f64 = 0.6;
const GROUND_FRICTION: f64 = BLOCK_FRICTION * HORIZONTAL_DRAG;
const GROUND_ACCEL_FACTOR: f64 = 0.216;
const MOVEMENT_SPEED: f64 = 0.1;
const SPRINT_SPEED_MODIFIER: f64 = 0.3;
const INPUT_DAMPING: f64 = 0.98;
const AIR_ACCELERATION: f64 = 0.02;
// TODO: WATER_MOVEMENT_EFFICIENCY attribute - scales drag toward 0.546 and
// accel toward land speed
const WATER_ACCELERATION: f64 = 0.02;
const WATER_HORIZONTAL_DRAG: f64 = 0.8;
const WATER_HORIZONTAL_DRAG_SPRINT: f64 = 0.9;
const WATER_VERTICAL_DRAG: f64 = 0.8;
const WATER_GRAVITY: f64 = 0.02;
const STEP_HEIGHT: f64 = 0.6;
const PLAYER_HALF_WIDTH: f64 = 0.3;
const PLAYER_HEIGHT: f64 = 1.8;
const SPRINT_JUMP_BOOST: f64 = 0.2;
const SPRINT_HUNGER_THRESHOLD: u32 = 6;
const DEFAULT_SPRINT_WINDOW: u32 = 7;
const MINOR_COLLISION_ANGLE: f64 = 0.13962634;

pub fn tick(player: &mut LocalPlayer, input: &InputState, chunk_store: &ChunkStore) {
    player.update_water_state(chunk_store);

    let (forward, strafe) = movement_input(input);
    let forward_pressed = input.key_pressed(KeyCode::KeyW);

    update_sprint_state(player, input, forward, forward_pressed);

    let (sin_y_rot, cos_y_rot) = (player.look_dir.y_rot_rad() as f64).sin_cos();

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

    player.tick_air_supply();
    player.was_forward_pressed = forward_pressed;
}

fn tick_land(
    player: &mut LocalPlayer,
    input: &InputState,
    chunk_store: &ChunkStore,
    forward: f64,
    strafe: f64,
    sin_y_rot: f64,
    cos_y_rot: f64,
) {
    if player.on_ground && input.key_pressed(KeyCode::Space) {
        player.velocity.y = JUMP_VELOCITY.max(player.velocity.y);

        if player.sprinting {
            player.velocity.x -= sin_y_rot * SPRINT_JUMP_BOOST;
            player.velocity.z += cos_y_rot * SPRINT_JUMP_BOOST;
        }
    }

    let speed = if player.sprinting {
        MOVEMENT_SPEED * (1.0 + SPRINT_SPEED_MODIFIER)
    } else {
        MOVEMENT_SPEED
    };

    let accel = friction_influenced_speed(speed, player.on_ground, BLOCK_FRICTION);
    let (move_x, move_z) = world_movement(forward, strafe, sin_y_rot, cos_y_rot);
    player.velocity.x += move_x * accel;
    player.velocity.z += move_z * accel;

    apply_collision(player, chunk_store, forward, strafe, sin_y_rot, cos_y_rot);

    player.velocity.y -= GRAVITY;
    player.velocity.y *= VERTICAL_DRAG as f64;

    let h_friction = if player.on_ground {
        GROUND_FRICTION
    } else {
        HORIZONTAL_DRAG
    };
    player.velocity.x *= h_friction;
    player.velocity.z *= h_friction;

    if player.on_ground && player.velocity.y < 0.0 {
        player.velocity.y = 0.0;
    }
}

fn tick_water(
    player: &mut LocalPlayer,
    input: &InputState,
    chunk_store: &ChunkStore,
    forward: f64,
    strafe: f64,
    sin_y_rot: f64,
    cos_y_rot: f64,
) {
    if input.key_pressed(KeyCode::Space) {
        player.velocity.y += 0.04;
    }
    if input.key_pressed(KeyCode::ShiftLeft) {
        player.velocity.y -= 0.04;
    }

    let (move_x, move_z) = world_movement(forward, strafe, sin_y_rot, cos_y_rot);
    player.velocity.x += move_x * WATER_ACCELERATION;
    player.velocity.z += move_z * WATER_ACCELERATION;

    if player.swimming {
        let sin_x = player.look_dir.x_rot_rad().sin() as f64;
        let target_vy = -sin_x;
        let boost = if target_vy < -0.2 { 0.085 } else { 0.06 };
        player.velocity.y += (target_vy - player.velocity.y) * boost;
    }

    apply_collision(player, chunk_store, forward, strafe, sin_y_rot, cos_y_rot);

    let h_drag = if player.sprinting {
        WATER_HORIZONTAL_DRAG_SPRINT
    } else {
        WATER_HORIZONTAL_DRAG
    };
    player.velocity.x *= h_drag;
    player.velocity.z *= h_drag;

    let gravity = if player.velocity.y <= 0.0 && !player.swimming {
        GRAVITY * 0.25
    } else {
        WATER_GRAVITY
    };
    player.velocity.y -= gravity;
    player.velocity.y *= WATER_VERTICAL_DRAG;
}

fn apply_collision(
    player: &mut LocalPlayer,
    chunk_store: &ChunkStore,
    forward: f64,
    strafe: f64,
    sin_y_rot: f64,
    cos_y_rot: f64,
) {
    let aabb = Aabb::from_center(
        player.position.into(),
        PLAYER_HALF_WIDTH,
        PLAYER_HEIGHT / 2.0,
    );
    let step_height = if player.on_ground { STEP_HEIGHT } else { 0.0 };
    let (resolved, on_ground) = resolve_collision(chunk_store, aabb, player.velocity, step_height);

    let collided_x = (resolved.x - player.velocity.x).abs() > 1.0e-5;
    let collided_z = (resolved.z - player.velocity.z).abs() > 1.0e-5;
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
    forward: f64,
    forward_pressed: bool,
) {
    if player.sprint_toggle_timer > 0 {
        player.sprint_toggle_timer -= 1;
    }

    let can_sprint = forward > 0.0 && player.food > SPRINT_HUNGER_THRESHOLD;

    if input.key_pressed(KeyCode::ControlLeft) && can_sprint {
        player.sprinting = true;
    }

    if !player.was_forward_pressed && forward_pressed && can_sprint {
        if player.sprint_toggle_timer > 0 {
            player.sprinting = true;
        }
        player.sprint_toggle_timer = DEFAULT_SPRINT_WINDOW;
    }

    if player.sprinting && (forward <= 0.0 || player.food <= SPRINT_HUNGER_THRESHOLD) {
        player.sprinting = false;
    }
}

fn world_movement(forward: f64, strafe: f64, sin_y_rot: f64, cos_y_rot: f64) -> (f64, f64) {
    (
        forward * -sin_y_rot + strafe * -cos_y_rot,
        forward * cos_y_rot + strafe * -sin_y_rot,
    )
}

fn friction_influenced_speed(speed: f64, on_ground: bool, block_friction: f64) -> f64 {
    if on_ground {
        if block_friction > BLOCK_FRICTION {
            speed * (GROUND_ACCEL_FACTOR / block_friction.powi(3))
        } else {
            speed
        }
    } else {
        AIR_ACCELERATION
    }
}

fn is_minor_horizontal_collision(
    forward: f64,
    strafe: f64,
    sin_y_rot: f64,
    cos_y_rot: f64,
    resolved: DVec3,
) -> bool {
    let (intent_x, intent_z) = world_movement(forward, strafe, sin_y_rot, cos_y_rot);
    let intent_len_sq = intent_x * intent_x + intent_z * intent_z;
    let resolved_len_sq = resolved.x.powi(2) + resolved.z.powi(2);
    if intent_len_sq < 1.0e-5 || resolved_len_sq < 1.0e-5 {
        return false;
    }
    let dot = intent_x * resolved.x + intent_z * resolved.z;
    let angle = (dot / (intent_len_sq * resolved_len_sq).sqrt()).acos();
    angle < MINOR_COLLISION_ANGLE
}

fn movement_input(input: &InputState) -> (f64, f64) {
    let mut forward = 0.0f64;
    let mut strafe = 0.0f64;

    if input.key_pressed(KeyCode::KeyW) {
        forward += 1.0;
    }
    if input.key_pressed(KeyCode::KeyS) {
        forward -= 1.0;
    }
    if input.key_pressed(KeyCode::KeyA) {
        strafe -= 1.0;
    }
    if input.key_pressed(KeyCode::KeyD) {
        strafe += 1.0;
    }

    forward *= INPUT_DAMPING;
    strafe *= INPUT_DAMPING;

    square_movement(forward, strafe)
}

// Assumes cardinal keyboard input (-1/0/+1 axes); analog input would need a
// normalize first.
fn square_movement(forward: f64, strafe: f64) -> (f64, f64) {
    let len = (forward * forward + strafe * strafe).sqrt();
    if len < 1.0e-7 {
        return (0.0, 0.0);
    }
    let max_axis = forward.abs().max(strafe.abs());
    let modified = (len * len / max_axis).min(1.0);
    let scale = modified / len;
    (forward * scale, strafe * scale)
}
