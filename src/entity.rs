use std::collections::HashMap;

use azalea_registry::builtin::EntityKind;
use glam::DVec3;

const INTERPOLATION_STEPS: i32 = 3;

#[allow(dead_code)]
pub struct LivingEntity {
    pub position: DVec3,
    pub prev_position: DVec3,
    pub yaw: f32,
    pub prev_yaw: f32,
    pub pitch: f32,
    pub prev_pitch: f32,
    pub body_yaw: f32,
    pub prev_body_yaw: f32,
    pub head_yaw: f32,
    pub prev_head_yaw: f32,
    pub entity_type: EntityKind,
    pub walk_anim_pos: f32,
    pub walk_anim_speed: f32,
    pub is_baby: bool,
    pub on_ground: bool,
    interp_target: DVec3,
    interp_yaw: f32,
    interp_pitch: f32,
    interp_steps: i32,
    interp_head_yaw: f32,
    interp_head_steps: i32,
}

impl LivingEntity {
    pub fn new(
        entity_type: EntityKind,
        position: DVec3,
        yaw: f32,
        pitch: f32,
        head_yaw: f32,
    ) -> Self {
        Self {
            position,
            prev_position: position,
            yaw,
            prev_yaw: yaw,
            pitch,
            prev_pitch: pitch,
            body_yaw: yaw,
            prev_body_yaw: yaw,
            head_yaw,
            prev_head_yaw: head_yaw,
            entity_type,
            walk_anim_pos: 0.0,
            walk_anim_speed: 0.0,
            is_baby: false,
            on_ground: false,
            interp_target: position,
            interp_yaw: yaw,
            interp_pitch: pitch,
            interp_steps: 0,
            interp_head_yaw: head_yaw,
            interp_head_steps: 0,
        }
    }

    fn interpolate_to_pos(&mut self, pos: DVec3) {
        self.interp_target = pos;
        self.interp_steps = INTERPOLATION_STEPS;
    }

    pub fn tick_interpolation(&mut self) {
        self.prev_position = self.position;
        self.prev_yaw = self.yaw;
        self.prev_pitch = self.pitch;

        if self.interp_steps > 0 {
            let alpha = 1.0 / self.interp_steps as f64;
            self.position = self.position.lerp(self.interp_target, alpha);
            self.yaw = lerp_angle(self.yaw, self.interp_yaw, 1.0 / self.interp_steps as f32);
            self.pitch += (self.interp_pitch - self.pitch) / self.interp_steps as f32;
            self.interp_steps -= 1;
        }

        self.prev_head_yaw = self.head_yaw;
        if self.interp_head_steps > 0 {
            self.head_yaw = lerp_angle(
                self.head_yaw,
                self.interp_head_yaw,
                1.0 / self.interp_head_steps as f32,
            );
            self.interp_head_steps -= 1;
        }
    }

    pub fn tick_body_rotation(&mut self) {
        self.prev_body_yaw = self.body_yaw;

        let dx = self.position.x - self.prev_position.x;
        let dz = self.position.z - self.prev_position.z;
        let dist_sq = (dx * dx + dz * dz) as f32;

        let body_target = if dist_sq > 0.0025 {
            -(dx as f32).atan2(dz as f32).to_degrees()
        } else {
            self.yaw
        };

        let diff = wrap_degrees(body_target - self.body_yaw);
        self.body_yaw += diff * 0.3;

        let head_diff = wrap_degrees(self.yaw - self.body_yaw);
        if head_diff.abs() > 50.0 {
            self.body_yaw += head_diff - head_diff.signum() * 50.0;
        }
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
        position: DVec3,
        yaw: f32,
        pitch: f32,
        head_yaw: f32,
    ) {
        self.living
            .insert(id, LivingEntity::new(entity_type, position, yaw, pitch, head_yaw));
    }

    pub fn move_living_delta(&mut self, id: i32, dx: f64, dy: f64, dz: f64) {
        if let Some(entity) = self.living.get_mut(&id) {
            let target = entity.interp_target + DVec3::new(dx, dy, dz);
            entity.interpolate_to_pos(target);
        }
    }

    pub fn teleport_living(&mut self, id: i32, x: f64, y: f64, z: f64) {
        if let Some(entity) = self.living.get_mut(&id) {
            let pos = DVec3::new(x, y, z);
            entity.interpolate_to_pos(pos);
        }
    }

    pub fn set_baby(&mut self, id: i32, is_baby: bool) {
        if let Some(entity) = self.living.get_mut(&id) {
            entity.is_baby = is_baby;
        }
    }

    pub fn update_living_rotation(&mut self, id: i32, yaw: f32, pitch: f32) {
        if let Some(entity) = self.living.get_mut(&id) {
            entity.interp_yaw = yaw;
            entity.interp_pitch = pitch;
            entity.interp_steps = entity.interp_steps.max(INTERPOLATION_STEPS);
        }
    }

    pub fn update_head_rotation(&mut self, id: i32, head_yaw: f32) {
        if let Some(entity) = self.living.get_mut(&id) {
            entity.interp_head_yaw = head_yaw;
            entity.interp_head_steps = INTERPOLATION_STEPS;
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
            let speed = ((dx * dx + dz * dz) as f32).sqrt();
            let target_speed = speed.min(1.0);
            entity.walk_anim_speed += (target_speed - entity.walk_anim_speed) * 0.4;
            entity.walk_anim_pos += entity.walk_anim_speed;
        }
    }

    pub fn clear(&mut self) {
        self.living.clear();
    }
}

fn wrap_degrees(deg: f32) -> f32 {
    let mut d = deg % 360.0;
    if d >= 180.0 {
        d -= 360.0;
    }
    if d < -180.0 {
        d += 360.0;
    }
    d
}

fn lerp_angle(from: f32, to: f32, alpha: f32) -> f32 {
    from + wrap_degrees(to - from) * alpha
}

pub fn is_living_mob(kind: &EntityKind) -> bool {
    matches!(
        kind,
        EntityKind::Pig
            | EntityKind::Cow
            | EntityKind::Sheep
            | EntityKind::Chicken
            | EntityKind::Zombie
            | EntityKind::Skeleton
            | EntityKind::Creeper
            | EntityKind::Spider
    )
}
