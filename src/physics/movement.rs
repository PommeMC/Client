use winit::keyboard::KeyCode;

use super::aabb::Aabb;
use super::collision::resolve_collision;
use crate::player::LocalPlayer;
use crate::window::input::InputState;
use crate::world::chunk::ChunkStore;

const GRAVITY: f32 = 0.08;
const JUMP_VELOCITY: f32 = 0.42;
const VERTICAL_DRAG: f32 = 0.98;
const HORIZONTAL_DRAG: f32 = 0.91;
const BLOCK_FRICTION: f32 = 0.6;
const GROUND_FRICTION: f32 = BLOCK_FRICTION * HORIZONTAL_DRAG;
const GROUND_ACCEL_FACTOR: f32 = 0.216;
const MOVEMENT_SPEED: f32 = 0.1;
const SPRINT_SPEED_MODIFIER: f32 = 0.3;
const AIR_ACCELERATION: f32 = 0.02;
const STEP_HEIGHT: f32 = 0.6;
const PLAYER_HALF_WIDTH: f32 = 0.3;
const PLAYER_HEIGHT: f32 = 1.8;
const SPRINT_JUMP_BOOST: f32 = 0.2;
const SPRINT_HUNGER_THRESHOLD: u32 = 6;
const DEFAULT_SPRINT_WINDOW: u32 = 7;
const MINOR_COLLISION_ANGLE: f32 = 0.13962634;

pub fn tick(player: &mut LocalPlayer, input: &InputState, chunk_store: &ChunkStore) {
    let (forward, strafe) = movement_input(input);
    let forward_pressed = input.key_pressed(KeyCode::KeyW);

    update_sprint_state(player, input, forward, forward_pressed);

    let (sin_yaw, cos_yaw) = player.yaw.sin_cos();

    if player.on_ground && input.key_pressed(KeyCode::Space) {
        player.velocity.y = JUMP_VELOCITY;

        if player.sprinting {
            player.velocity.x -= sin_yaw * SPRINT_JUMP_BOOST;
            player.velocity.z -= cos_yaw * SPRINT_JUMP_BOOST;
        }
    }

    let speed = if player.sprinting {
        MOVEMENT_SPEED * (1.0 + SPRINT_SPEED_MODIFIER)
    } else {
        MOVEMENT_SPEED
    };

    let accel = if player.on_ground {
        let friction_cubed = GROUND_FRICTION * GROUND_FRICTION * GROUND_FRICTION;
        speed * (GROUND_ACCEL_FACTOR / friction_cubed)
    } else {
        AIR_ACCELERATION
    };
    let (move_x, move_z) = world_movement(forward, strafe, sin_yaw, cos_yaw);
    player.velocity.x += move_x * accel;
    player.velocity.z += move_z * accel;

    let aabb = Aabb::from_center(player.position, PLAYER_HALF_WIDTH, PLAYER_HEIGHT / 2.0);
    let step_height = if player.on_ground { STEP_HEIGHT } else { 0.0 };
    let (resolved, on_ground) = resolve_collision(chunk_store, aabb, player.velocity, step_height);

    let horizontal_collision = (resolved.x - player.velocity.x).abs() > 1.0e-5
        || (resolved.z - player.velocity.z).abs() > 1.0e-5;

    player.position += resolved;
    player.on_ground = on_ground;
    player.horizontal_collision = horizontal_collision;

    if player.sprinting && horizontal_collision && forward > 0.0 {
        if !is_minor_horizontal_collision(forward, strafe, sin_yaw, cos_yaw, &resolved) {
            player.sprinting = false;
        }
    }

    player.velocity.y -= GRAVITY;
    player.velocity.y *= VERTICAL_DRAG;

    let h_friction = if player.on_ground {
        GROUND_FRICTION
    } else {
        HORIZONTAL_DRAG
    };
    player.velocity.x *= h_friction;
    player.velocity.z *= h_friction;

    if on_ground && player.velocity.y < 0.0 {
        player.velocity.y = 0.0;
    }

    player.was_forward_pressed = forward_pressed;
}

fn update_sprint_state(player: &mut LocalPlayer, input: &InputState, forward: f32, forward_pressed: bool) {
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

    if player.sprinting {
        if forward <= 0.0 || player.food <= SPRINT_HUNGER_THRESHOLD {
            player.sprinting = false;
        }
    }
}

fn world_movement(forward: f32, strafe: f32, sin_yaw: f32, cos_yaw: f32) -> (f32, f32) {
    (
        forward * -sin_yaw + strafe * cos_yaw,
        forward * -cos_yaw + strafe * -sin_yaw,
    )
}

fn is_minor_horizontal_collision(
    forward: f32, strafe: f32,
    sin_yaw: f32, cos_yaw: f32,
    resolved: &glam::Vec3,
) -> bool {
    let (intent_x, intent_z) = world_movement(forward, strafe, sin_yaw, cos_yaw);
    let (ix, iz) = (intent_x as f64, intent_z as f64);
    let intent_len_sq = ix * ix + iz * iz;
    let resolved_len_sq = (resolved.x as f64).powi(2) + (resolved.z as f64).powi(2);
    if intent_len_sq < 1.0e-5 || resolved_len_sq < 1.0e-5 {
        return false;
    }
    let dot = ix * resolved.x as f64 + iz * resolved.z as f64;
    let angle = (dot / (intent_len_sq * resolved_len_sq).sqrt()).acos();
    angle < MINOR_COLLISION_ANGLE as f64
}

fn movement_input(input: &InputState) -> (f32, f32) {
    let mut forward: f32 = 0.0;
    let mut strafe: f32 = 0.0;

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

    let len_sq = forward * forward + strafe * strafe;
    if len_sq > 1.0 {
        let len = len_sq.sqrt();
        forward /= len;
        strafe /= len;
    }

    (forward, strafe)
}
