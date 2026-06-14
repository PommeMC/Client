pub mod components;

use std::collections::HashMap;

use azalea_registry::builtin::EntityKind;
use glam::DVec3;

use crate::entity::components::{LookDirection, Position, Velocity};

fn item_move(
    pos: &mut Position,
    vel: Velocity,
    half_w: f64,
    height: f64,
    is_solid: &impl Fn(i32, i32, i32) -> bool,
) -> Velocity {
    let mut remaining = vel;

    if remaining.y != 0.0 {
        remaining.y = sweep_axis_y(pos, remaining.y, half_w, height, is_solid);
    }
    if remaining.x.abs() >= remaining.z.abs() {
        if remaining.x != 0.0 {
            remaining.x = sweep_axis_x(pos, remaining.x, half_w, height, is_solid);
        }
        if remaining.z != 0.0 {
            remaining.z = sweep_axis_z(pos, remaining.z, half_w, height, is_solid);
        }
    } else {
        if remaining.z != 0.0 {
            remaining.z = sweep_axis_z(pos, remaining.z, half_w, height, is_solid);
        }
        if remaining.x != 0.0 {
            remaining.x = sweep_axis_x(pos, remaining.x, half_w, height, is_solid);
        }
    }

    remaining
}

fn sweep_axis_y(
    pos: &mut Position,
    dy: f64,
    half_w: f64,
    height: f64,
    is_solid: &impl Fn(i32, i32, i32) -> bool,
) -> f64 {
    let min_x = (pos.x - half_w).floor() as i32;
    let max_x = (pos.x + half_w).floor() as i32;
    let min_z = (pos.z - half_w).floor() as i32;
    let max_z = (pos.z + half_w).floor() as i32;

    if dy < 0.0 {
        let target_y = pos.y + dy;
        let by = target_y.floor() as i32;
        for bx in min_x..=max_x {
            for bz in min_z..=max_z {
                if is_solid(bx, by, bz) {
                    let floor = by as f64 + 1.0;
                    if floor > target_y && floor <= pos.y {
                        pos.y = floor;
                        return 0.0;
                    }
                }
            }
        }
        pos.y = target_y;
    } else if dy > 0.0 {
        let target_y = pos.y + height + dy;
        let by = target_y.floor() as i32;
        for bx in min_x..=max_x {
            for bz in min_z..=max_z {
                if is_solid(bx, by, bz) {
                    let ceil = by as f64;
                    pos.y = ceil - height;
                    return 0.0;
                }
            }
        }
        pos.y += dy;
    }
    dy
}

fn sweep_axis_x(
    pos: &mut Position,
    dx: f64,
    half_w: f64,
    height: f64,
    is_solid: &impl Fn(i32, i32, i32) -> bool,
) -> f64 {
    let min_y = pos.y.floor() as i32;
    let max_y = (pos.y + height).floor() as i32;
    let min_z = (pos.z - half_w).floor() as i32;
    let max_z = (pos.z + half_w).floor() as i32;

    let edge = if dx > 0.0 {
        pos.x + half_w + dx
    } else {
        pos.x - half_w + dx
    };
    let bx = edge.floor() as i32;

    for by in min_y..=max_y {
        for bz in min_z..=max_z {
            if is_solid(bx, by, bz) {
                if dx > 0.0 {
                    pos.x = bx as f64 - half_w;
                } else {
                    pos.x = bx as f64 + 1.0 + half_w;
                }
                return 0.0;
            }
        }
    }
    pos.x += dx;
    dx
}

fn sweep_axis_z(
    pos: &mut Position,
    dz: f64,
    half_w: f64,
    height: f64,
    is_solid: &impl Fn(i32, i32, i32) -> bool,
) -> f64 {
    let min_y = pos.y.floor() as i32;
    let max_y = (pos.y + height).floor() as i32;
    let min_x = (pos.x - half_w).floor() as i32;
    let max_x = (pos.x + half_w).floor() as i32;

    let edge = if dz > 0.0 {
        pos.z + half_w + dz
    } else {
        pos.z - half_w + dz
    };
    let bz = edge.floor() as i32;

    for by in min_y..=max_y {
        for bx in min_x..=max_x {
            if is_solid(bx, by, bz) {
                if dz > 0.0 {
                    pos.z = bz as f64 - half_w;
                } else {
                    pos.z = bz as f64 + 1.0 + half_w;
                }
                return 0.0;
            }
        }
    }
    pos.z += dz;
    dz
}

const INTERPOLATION_STEPS: i32 = 3;
const HURT_DURATION: u8 = 10;

#[allow(dead_code)]
pub struct LivingEntity {
    pub position: Position,
    pub prev_position: Position,
    pub look_dir: LookDirection,
    pub prev_look_dir: LookDirection,
    pub head_y_rot_deg: f32,
    pub prev_head_y_rot_deg: f32,
    pub body_y_rot_deg: f32,
    pub prev_body_y_rot_deg: f32,
    pub entity_type: EntityKind,
    pub walk_anim_pos: f32,
    pub walk_anim_speed: f32,
    pub prev_walk_anim_speed: f32,
    pub is_baby: bool,
    pub is_crouching: bool,
    pub on_ground: bool,
    pub wool_color: Option<u8>,
    pub is_sheared: bool,
    pub cow_variant: u8,
    pub eat_anim_tick: u8,
    pub prev_eat_anim_tick: u8,
    pub hurt_time: u8,
    pub age_in_ticks: u32,
    pub custom_name: Option<String>,
    interp_target: Position,
    interp_look_dir: LookDirection,
    interp_steps: i32,
    interp_head_y_rot_deg: f32,
    interp_head_y_rot_steps: i32,
}

impl LivingEntity {
    pub fn new(
        entity_type: EntityKind,
        position: Position,
        look_dir: LookDirection,
        head_y_rot_deg: f32,
        body_y_rot_deg: f32,
    ) -> Self {
        Self {
            position,
            prev_position: position,
            look_dir,
            prev_look_dir: look_dir,
            head_y_rot_deg,
            prev_head_y_rot_deg: head_y_rot_deg,
            body_y_rot_deg,
            prev_body_y_rot_deg: body_y_rot_deg,
            entity_type,
            walk_anim_pos: 0.0,
            walk_anim_speed: 0.0,
            prev_walk_anim_speed: 0.0,
            is_baby: false,
            is_crouching: false,
            on_ground: false,
            wool_color: None,
            is_sheared: false,
            cow_variant: 0,
            eat_anim_tick: 0,
            prev_eat_anim_tick: 0,
            hurt_time: 0,
            age_in_ticks: 0,
            custom_name: None,
            interp_target: position,
            interp_look_dir: look_dir,
            interp_steps: 0,
            interp_head_y_rot_deg: head_y_rot_deg,
            interp_head_y_rot_steps: 0,
        }
    }

    fn interpolate_to_pos(&mut self, pos: Position) {
        self.interp_target = pos;
        self.interp_steps = INTERPOLATION_STEPS;
    }

    pub fn tick_interpolation(&mut self) {
        self.prev_position = self.position;
        self.prev_look_dir = self.look_dir;

        if self.interp_steps > 0 {
            let alpha = 1.0 / self.interp_steps as f64;
            self.position = self.position.lerp(self.interp_target, alpha);
            let y_rot = lerp_angle(
                self.look_dir.y_rot_deg(),
                self.interp_look_dir.y_rot_deg(),
                1.0 / self.interp_steps as f32,
            );
            let x_rot = self.look_dir.x_rot_deg()
                + (self.interp_look_dir.x_rot_deg() - self.look_dir.x_rot_deg())
                    / self.interp_steps as f32;
            self.look_dir = LookDirection::new(y_rot, x_rot);
            self.interp_steps -= 1;
        }

        self.prev_head_y_rot_deg = self.head_y_rot_deg;
        if self.interp_head_y_rot_steps > 0 {
            self.head_y_rot_deg = lerp_angle(
                self.head_y_rot_deg,
                self.interp_head_y_rot_deg,
                1.0 / self.interp_head_y_rot_steps as f32,
            );
            self.interp_head_y_rot_steps -= 1;
        }

        self.prev_body_y_rot_deg = self.body_y_rot_deg;
    }

    pub fn tick_body_rotation(&mut self) {
        let dx = self.position.x - self.prev_position.x;
        let dz = self.position.z - self.prev_position.z;
        let dist_sq = (dx * dx + dz * dz) as f32;

        if dist_sq > 0.0025 {
            let walk_dir = -(dx as f32).atan2(dz as f32).to_degrees();
            let diff_from_look = wrap_degrees(self.look_dir.y_rot_deg() - walk_dir).abs();
            let body_target = if diff_from_look > 95.0 && diff_from_look < 265.0 {
                walk_dir - 180.0
            } else {
                walk_dir
            };
            let diff = wrap_degrees(body_target - self.body_y_rot_deg);
            self.body_y_rot_deg += diff * 0.3;
        }

        let head_diff = wrap_degrees(self.head_y_rot_deg - self.body_y_rot_deg);
        if head_diff.abs() > 50.0 {
            self.body_y_rot_deg += head_diff - head_diff.signum() * 50.0;
        }
    }
}

pub struct ItemEntity {
    pub position: Position,
    pub prev_position: Position,
    pub velocity: Velocity,
    pub on_ground: bool,
    pub item_name: String,
    pub count: i32,
    pub age: u32,
    pub bob_offset: f32,
    pub is_block_model: bool,
}

struct PickupAnimation {
    item_name: String,
    start_pos: Position,
    target_pos: Position,
    bob_offset: f32,
    age: u32,
    life: u32,
    is_block_model: bool,
}

pub struct PickupRenderInfo {
    pub item_name: String,
    pub position: Position,
    pub bob_offset: f32,
    pub age: u32,
    pub is_block_model: bool,
}

const PICKUP_LIFE: u32 = 3;

pub struct ItemEntityStore {
    items: HashMap<i32, ItemEntity>,
    pickups: Vec<PickupAnimation>,
}

impl ItemEntityStore {
    pub fn new() -> Self {
        Self {
            items: HashMap::new(),
            pickups: Vec::new(),
        }
    }

    pub fn spawn_item(&mut self, id: i32, position: Position, velocity: Velocity) {
        let bob_offset =
            ((id as u32).wrapping_mul(2654435761)) as f32 / u32::MAX as f32 * std::f32::consts::TAU;
        self.items.insert(
            id,
            ItemEntity {
                position,
                prev_position: position,
                velocity,
                on_ground: false,
                item_name: String::new(),
                count: 1,
                age: 0,
                bob_offset,
                is_block_model: false,
            },
        );
    }

    pub fn set_item_data(&mut self, id: i32, item_name: String, count: i32, is_block_model: bool) {
        if let Some(entity) = self.items.get_mut(&id) {
            entity.item_name = item_name;
            entity.count = count;
            entity.is_block_model = is_block_model;
        }
    }

    pub fn move_delta(&mut self, id: i32, dx: f64, dy: f64, dz: f64) {
        if let Some(entity) = self.items.get_mut(&id) {
            entity.prev_position = entity.position;
            entity.position.x += dx;
            entity.position.y += dy;
            entity.position.z += dz;
        }
    }

    pub fn teleport(&mut self, id: i32, position: Position) {
        if let Some(entity) = self.items.get_mut(&id) {
            entity.prev_position = entity.position;
            entity.position = position;
        }
    }

    pub fn pickup(&mut self, item_id: i32, target_pos: Position) {
        if let Some(entity) = self.items.remove(&item_id)
            && !entity.item_name.is_empty()
        {
            self.pickups.push(PickupAnimation {
                item_name: entity.item_name,
                start_pos: entity.position,
                target_pos,
                bob_offset: entity.bob_offset,
                age: entity.age,
                life: 0,
                is_block_model: entity.is_block_model,
            });
        }
    }

    pub fn remove(&mut self, ids: &[i32]) {
        for &id in ids {
            self.items.remove(&id);
        }
    }

    pub fn tick(
        &mut self,
        is_solid: impl Fn(i32, i32, i32) -> bool,
        get_friction: impl Fn(i32, i32, i32) -> f32,
    ) {
        const GRAVITY: f64 = -0.04;
        const HALF_W: f64 = 0.125;
        const HEIGHT: f64 = 0.25;

        for entity in self.items.values_mut() {
            entity.prev_position = entity.position;
            entity.age += 1;

            entity.velocity.y += GRAVITY;

            let moved = item_move(
                &mut entity.position,
                entity.velocity,
                HALF_W,
                HEIGHT,
                &is_solid,
            );

            entity.on_ground = entity.velocity.y < 0.0 && moved.y != entity.velocity.y;

            if moved.x != entity.velocity.x {
                entity.velocity.x = 0.0;
            }
            if moved.z != entity.velocity.z {
                entity.velocity.z = 0.0;
            }

            let friction = if entity.on_ground {
                let bx = entity.position.x.floor() as i32;
                let by = (entity.position.y - 0.5001).floor() as i32;
                let bz = entity.position.z.floor() as i32;
                get_friction(bx, by, bz) * 0.98
            } else {
                0.98
            };

            if entity.on_ground && entity.velocity.y < 0.0 {
                entity.velocity.y *= -0.5;
            }

            entity.velocity.x *= friction as f64;
            entity.velocity.y *= 0.98;
            entity.velocity.z *= friction as f64;
        }
        for pickup in &mut self.pickups {
            pickup.life += 1;
        }
        self.pickups.retain(|p| p.life < PICKUP_LIFE);
    }

    pub fn visible_items(&self, camera_pos: DVec3, max_dist: f64) -> Vec<&ItemEntity> {
        let max_dist_sq = max_dist * max_dist;
        self.items
            .values()
            .filter(|e| {
                !e.item_name.is_empty() && e.position.distance_squared(camera_pos) < max_dist_sq
            })
            .collect()
    }

    pub fn active_pickups(&self, partial_tick: f32) -> Vec<PickupRenderInfo> {
        self.pickups
            .iter()
            .map(|p| {
                let t = (p.life as f32 + partial_tick) / PICKUP_LIFE as f32;
                let t = t * t;
                let pos = p.start_pos.lerp(p.target_pos, t as f64);
                PickupRenderInfo {
                    item_name: p.item_name.clone(),
                    position: pos,
                    bob_offset: p.bob_offset,
                    age: p.age,
                    is_block_model: p.is_block_model,
                }
            })
            .collect()
    }
}

pub struct EntityStore {
    pub living: HashMap<i32, LivingEntity>,
}

impl EntityStore {
    pub fn new() -> Self {
        Self {
            living: HashMap::new(),
        }
    }

    pub fn spawn_living(
        &mut self,
        id: i32,
        entity_type: EntityKind,
        position: Position,
        look_dir: LookDirection,
        body_y_rot_deg: f32,
        head_y_rot_deg: f32,
    ) {
        self.living.insert(
            id,
            LivingEntity::new(
                entity_type,
                position,
                look_dir,
                head_y_rot_deg,
                body_y_rot_deg,
            ),
        );
    }

    pub fn move_living_delta(&mut self, id: i32, dx: f64, dy: f64, dz: f64) {
        if let Some(entity) = self.living.get_mut(&id) {
            let target = entity.interp_target + DVec3::new(dx, dy, dz);
            entity.interpolate_to_pos(target);
        }
    }

    pub fn teleport_living(&mut self, id: i32, position: Position) {
        if let Some(entity) = self.living.get_mut(&id) {
            entity.interpolate_to_pos(position);
        }
    }

    pub fn set_baby(&mut self, id: i32, is_baby: bool) {
        if let Some(entity) = self.living.get_mut(&id) {
            entity.is_baby = is_baby;
        }
    }

    pub fn set_crouching(&mut self, id: i32, is_crouching: bool) {
        if let Some(entity) = self.living.get_mut(&id) {
            entity.is_crouching = is_crouching;
        }
    }

    pub fn set_sheep_wool(&mut self, id: i32, color: u8, sheared: bool) {
        if let Some(entity) = self.living.get_mut(&id)
            && entity.entity_type == EntityKind::Sheep
        {
            entity.wool_color = Some(color);
            entity.is_sheared = sheared;
        }
    }

    pub fn set_cow_variant(&mut self, id: i32, variant: u8) {
        if let Some(entity) = self.living.get_mut(&id)
            && entity.entity_type == EntityKind::Cow
        {
            entity.cow_variant = variant;
        }
    }

    pub fn start_sheep_eat(&mut self, id: i32) {
        if let Some(entity) = self.living.get_mut(&id)
            && entity.entity_type == EntityKind::Sheep
        {
            entity.eat_anim_tick = 40;
            entity.prev_eat_anim_tick = 40;
        }
    }

    /// Mirrors vanilla `LivingEntity.handleDamageEvent`: `hurtTime = 10`.
    pub fn mark_hurt(&mut self, id: i32) {
        if let Some(entity) = self.living.get_mut(&id) {
            entity.hurt_time = HURT_DURATION;
        }
    }

    pub fn set_custom_name(&mut self, id: i32, name: Option<String>) {
        if let Some(entity) = self.living.get_mut(&id) {
            entity.custom_name = name;
        }
    }

    pub fn update_living_rotation(&mut self, id: i32, y_rot_deg: f32, x_rot_deg: f32) {
        if let Some(entity) = self.living.get_mut(&id) {
            entity.interp_look_dir = LookDirection::new(y_rot_deg, x_rot_deg);
            entity.interp_steps = entity.interp_steps.max(INTERPOLATION_STEPS);
        }
    }

    pub fn update_head_rotation(&mut self, id: i32, head_y_rot_deg: f32) {
        if let Some(entity) = self.living.get_mut(&id) {
            entity.interp_head_y_rot_deg = head_y_rot_deg;
            entity.interp_head_y_rot_steps = INTERPOLATION_STEPS;
        }
    }

    pub fn remove_living(&mut self, id: i32) {
        self.living.remove(&id);
    }

    pub fn tick_living(&mut self) {
        for entity in self.living.values_mut() {
            entity.tick_interpolation();
            entity.tick_body_rotation();
            let dx = entity.position.x - entity.prev_position.x;
            let dz = entity.position.z - entity.prev_position.z;
            update_walk_animation(
                dx,
                dz,
                &mut entity.walk_anim_pos,
                &mut entity.walk_anim_speed,
                &mut entity.prev_walk_anim_speed,
            );
            entity.prev_eat_anim_tick = entity.eat_anim_tick;
            if entity.eat_anim_tick > 0 {
                entity.eat_anim_tick -= 1;
            }
            if entity.hurt_time > 0 {
                entity.hurt_time -= 1;
            }
            entity.age_in_ticks = entity.age_in_ticks.wrapping_add(1);
        }
    }
}

pub fn update_walk_animation(
    dx: f64,
    dz: f64,
    walk_pos: &mut f32,
    walk_speed: &mut f32,
    prev_walk_speed: &mut f32,
) {
    let distance = ((dx * dx + dz * dz) as f32).sqrt();
    let target_speed = (distance * 4.0).min(1.0);
    *prev_walk_speed = *walk_speed;
    *walk_speed += (target_speed - *walk_speed) * 0.4;
    *walk_pos += *walk_speed;
}

pub fn wrap_degrees(deg: f32) -> f32 {
    let mut d = deg % 360.0;
    if d >= 180.0 {
        d -= 360.0;
    }
    if d < -180.0 {
        d += 360.0;
    }
    d
}

pub fn lerp_angle(from: f32, to: f32, alpha: f32) -> f32 {
    from + wrap_degrees(to - from) * alpha
}

pub fn is_living_mob(kind: &EntityKind) -> bool {
    matches!(
        kind,
        EntityKind::Player
            | EntityKind::Pig
            | EntityKind::Cow
            | EntityKind::Sheep
            | EntityKind::Chicken
            | EntityKind::Zombie
            | EntityKind::Skeleton
            | EntityKind::Creeper
            | EntityKind::Spider
    )
}
