pub mod components;

use std::collections::HashMap;

use azalea_registry::builtin::EntityKind;
use glam::DVec3;

use crate::entity::components::{LookDirection, Position};

const INTERPOLATION_STEPS: i32 = 3;
const HURT_DURATION: u8 = 10;
/// Vanilla default arm-swing duration in ticks
/// (`LivingEntity.getCurrentSwingDuration`).
const SWING_DURATION: u8 = 6;

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
    pub player_uuid: Option<uuid::Uuid>,
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
    /// Mob is targeting/attacking (metadata mob-flags bit 0x04). Raises
    /// zombie/skeleton arms.
    pub aggressive: bool,
    /// Creeper charged/powered flag — shows the blue aura overlay.
    pub powered: bool,
    /// Arm-swing animation timer, counts down from `SWING_DURATION` to 0
    /// (driven by the server `Animate` packet). Drives the zombie attack
    /// swing.
    pub swing_time: u8,
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
        player_uuid: Option<uuid::Uuid>,
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
            player_uuid,
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
            aggressive: false,
            powered: false,
            swing_time: 0,
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

    /// Arm-swing progress 0..1 for the current frame. `swing_time` counts down,
    /// so progress rises 0→1 over the swing; idle clamps to 1 (where the
    /// attack pose contribution is zero, like vanilla's attackTime
    /// endpoints).
    pub fn swing_progress(&self, partial: f32) -> f32 {
        ((SWING_DURATION as f32 - self.swing_time as f32 + partial) / SWING_DURATION as f32)
            .clamp(0.0, 1.0)
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
    pub item_name: String,
    /// Registry id (vanilla `Item.getId`) — seeds the copy-scatter RNG.
    pub item_id: u32,
    pub count: i32,
    pub age: u32,
    pub bob_offset: f32,
    pub is_block_model: bool,
    /// Local-space model bounds (pre per-entity scale) from the baked mesh,
    /// used for hover height and the 3D-vs-flat copy layout.
    pub min_y: f32,
    pub z_size: f32,
    interp_target: Position,
    interp_steps: i32,
}

struct PickupAnimation {
    item_name: String,
    item_id: u32,
    count: i32,
    start_pos: Position,
    target_pos: Position,
    bob_offset: f32,
    age: u32,
    life: u32,
    is_block_model: bool,
    min_y: f32,
    z_size: f32,
}

pub struct PickupRenderInfo {
    pub item_name: String,
    pub item_id: u32,
    pub count: i32,
    pub position: Position,
    pub bob_offset: f32,
    pub age: u32,
    pub is_block_model: bool,
    pub min_y: f32,
    pub z_size: f32,
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

    pub fn spawn_item(&mut self, id: i32, position: Position) {
        let bob_offset =
            ((id as u32).wrapping_mul(2654435761)) as f32 / u32::MAX as f32 * std::f32::consts::TAU;
        self.items.insert(
            id,
            ItemEntity {
                position,
                prev_position: position,
                item_name: String::new(),
                item_id: 0,
                count: 1,
                age: 0,
                bob_offset,
                is_block_model: false,
                min_y: -0.5,
                z_size: 1.0,
                interp_target: position,
                interp_steps: 0,
            },
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn set_item_data(
        &mut self,
        id: i32,
        item_name: String,
        item_id: u32,
        count: i32,
        is_block_model: bool,
        min_y: f32,
        z_size: f32,
    ) {
        if let Some(entity) = self.items.get_mut(&id) {
            entity.item_name = item_name;
            entity.item_id = item_id;
            entity.count = count;
            entity.is_block_model = is_block_model;
            entity.min_y = min_y;
            entity.z_size = z_size;
        }
    }

    /// Apply a server position delta via 3-step interpolation, mirroring
    /// `move_living_delta`. Items are not simulated locally.
    pub fn move_delta(&mut self, id: i32, dx: f64, dy: f64, dz: f64) {
        if let Some(entity) = self.items.get_mut(&id) {
            entity.interp_target += DVec3::new(dx, dy, dz);
            entity.interp_steps = INTERPOLATION_STEPS;
        }
    }

    pub fn teleport(&mut self, id: i32, position: Position) {
        if let Some(entity) = self.items.get_mut(&id) {
            entity.interp_target = position;
            entity.interp_steps = INTERPOLATION_STEPS;
        }
    }

    /// Handle a take-item packet: animate the (pre-shrink) cluster flying to
    /// the collector, then shrink the stack by `amount`, removing it only
    /// when empty (vanilla `handleTakeItemEntity`). Returns the item's
    /// position for the pickup sound, or `None` if there's nothing to pick
    /// up.
    pub fn pickup(&mut self, item_id: i32, target_pos: Position, amount: i32) -> Option<Position> {
        let entity = self.items.get_mut(&item_id)?;
        if entity.item_name.is_empty() {
            return None;
        }
        let start_pos = entity.position;
        let anim = PickupAnimation {
            item_name: entity.item_name.clone(),
            item_id: entity.item_id,
            count: entity.count,
            start_pos,
            target_pos,
            bob_offset: entity.bob_offset,
            age: entity.age,
            life: 0,
            is_block_model: entity.is_block_model,
            min_y: entity.min_y,
            z_size: entity.z_size,
        };
        entity.count -= amount;
        let empty = entity.count <= 0;
        self.pickups.push(anim);
        if empty {
            self.items.remove(&item_id);
        }
        Some(start_pos)
    }

    pub fn remove(&mut self, ids: &[i32]) {
        for &id in ids {
            self.items.remove(&id);
        }
    }

    pub fn tick(&mut self) {
        for entity in self.items.values_mut() {
            entity.prev_position = entity.position;
            if entity.interp_steps > 0 {
                let alpha = 1.0 / entity.interp_steps as f64;
                entity.position = entity.position.lerp(entity.interp_target, alpha);
                entity.interp_steps -= 1;
            }
            entity.age += 1;
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
                    item_id: p.item_id,
                    count: p.count,
                    position: pos,
                    bob_offset: p.bob_offset,
                    age: p.age,
                    is_block_model: p.is_block_model,
                    min_y: p.min_y,
                    z_size: p.z_size,
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
        player_uuid: Option<uuid::Uuid>,
    ) {
        self.living.insert(
            id,
            LivingEntity::new(
                entity_type,
                position,
                look_dir,
                look_dir.y_rot_deg(),
                body_y_rot_deg,
                player_uuid,
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

    pub fn set_aggressive(&mut self, id: i32, aggressive: bool) {
        if let Some(entity) = self.living.get_mut(&id) {
            entity.aggressive = aggressive;
        }
    }

    pub fn set_powered(&mut self, id: i32, powered: bool) {
        if let Some(entity) = self.living.get_mut(&id)
            && entity.entity_type == EntityKind::Creeper
        {
            entity.powered = powered;
        }
    }

    /// Begins an arm swing (server `Animate` packet). Restarts when idle or
    /// past the halfway point (vanilla `LivingEntity.swing`); `swing_time`
    /// counts down, so that is `swing_time <= SWING_DURATION / 2`.
    pub fn start_swing(&mut self, id: i32) {
        if let Some(entity) = self.living.get_mut(&id)
            && entity.swing_time <= SWING_DURATION / 2
        {
            entity.swing_time = SWING_DURATION;
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

    pub fn remove_living(&mut self, id: i32) -> Option<LivingEntity> {
        self.living.remove(&id)
    }

    pub fn has_player_uuid(&self, uuid: &uuid::Uuid) -> bool {
        self.living
            .values()
            .any(|entity| entity.player_uuid == Some(*uuid))
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
            if entity.swing_time > 0 {
                entity.swing_time -= 1;
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
