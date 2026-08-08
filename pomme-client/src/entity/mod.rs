pub mod components;
pub mod villager;

use std::collections::HashMap;

use azalea_core::position::ChunkPos;
use azalea_registry::builtin::EntityKind;
use glam::DVec3;

use crate::entity::components::{LookDirection, Position};
use crate::entity::villager::{VillagerKind, VillagerProfession};
use crate::physics::aabb::Aabb;
use crate::physics::collision::resolve_collision;
use crate::world::block::{FluidKind, fluid};
use crate::world::chunk::ChunkStore;

/// Kind-gated boolean mob states; each flag belongs to one mob kind and
/// [`EntityStore::set_mob_flag`] drops writes for a mismatched entity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MobFlag {
    CreeperPowered,
    EndermanCreepy,
    WitchDrinking,
    /// Zombie-family underwater conversion.
    ZombieConverting,
    /// Zombie villager curing.
    ZombieVillagerConverting,
    /// Wolf head-tilt beg state.
    WolfInterested,
    CatLying,
    CatRelaxed,
    /// Bat hanging state; a flip restarts the fly/rest animation clock.
    BatResting,
}

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
    /// Sheep wool shorn / bogged mushrooms shorn.
    pub is_sheared: bool,
    /// Registry/wire variant slot; meaning is per-kind (pool index for
    /// cow/chicken/wolf/cat, packed `color | markings << 8` for horse, raw
    /// vanilla int for rabbit/salmon/tropical fish). Normalized in
    /// `EntityStore::set_variant`.
    pub variant: u32,
    /// Chicken wing-flap state (vanilla `Chicken.aiStep`): `flap` is the
    /// unbounded wing-cycle phase, `flap_speed` the 0..1 amplitude.
    pub flap: f32,
    pub prev_flap: f32,
    pub flap_speed: f32,
    pub prev_flap_speed: f32,
    /// Slime squish spring (vanilla `AbstractCubeMob`): negative = squashed
    /// on landing, positive = stretched in the air.
    pub squish: f32,
    pub prev_squish: f32,
    pub slime_size: u8,
    /// Enderman screaming flag — raises the head and jitters the render
    /// position.
    pub is_creepy: bool,
    /// Zombie-family conversion (drowning / villager cure) — body-yaw shake.
    pub is_converting: bool,
    /// Witch drinking flag — swings the nose down toward the potion.
    pub witch_drinking: bool,
    /// Tamable (wolf/cat) state from the flags byte.
    pub is_sitting: bool,
    pub is_tame: bool,
    pub is_sprinting: bool,
    /// Dye id, wolf/cat collars (vanilla default red).
    pub collar_color: u8,
    pub is_interested: bool,
    /// Vanilla persistent-anger end time; angry while > current game time.
    pub anger_end_time: i64,
    /// Metadata health; drives the tame wolf's tail angle.
    pub health: f32,
    pub interested_angle: f32,
    pub prev_interested_angle: f32,
    pub shake_anim: f32,
    pub prev_shake_anim: f32,
    pub is_lying: bool,
    pub relax_state_one: bool,
    pub lie_down_amount: f32,
    pub prev_lie_down_amount: f32,
    pub lie_down_amount_tail: f32,
    pub prev_lie_down_amount_tail: f32,
    pub relax_state_one_amount: f32,
    pub prev_relax_state_one_amount: f32,
    /// Tick the rabbit hop keyframe clock started at, while hopping.
    pub hop_anim_start: Option<u32>,
    /// Equine flag-byte state (grass eating, rearing, open mouth).
    pub is_eating: bool,
    pub is_standing: bool,
    pub is_open_mouth: bool,
    pub eat_anim: f32,
    pub prev_eat_anim: f32,
    pub stand_anim: f32,
    pub prev_stand_anim: f32,
    pub mouth_anim: f32,
    pub prev_mouth_anim: f32,
    pub has_chest: bool,
    /// Packet-driven velocity (vanilla remote entities never integrate their
    /// own); feeds the squid body-rotation sim.
    pub velocity: DVec3,
    /// Vanilla `wasTouchingWater`, probed per tick for aquatic kinds.
    pub is_in_water: bool,
    /// Squid client sim (vanilla `Squid.aiStep`), degrees.
    pub x_body_rot: f32,
    pub prev_x_body_rot: f32,
    pub z_body_rot: f32,
    pub prev_z_body_rot: f32,
    pub tentacle_angle: f32,
    pub prev_tentacle_angle: f32,
    pub bat_resting: bool,
    /// Tick the bat's current fly/rest animation started at.
    pub bat_anim_start: Option<u32>,
    pub puff_state: u8,
    /// Glow squid post-hurt dim timer, synced then decremented client-side.
    pub dark_ticks: i32,
    pub villager_kind: VillagerKind,
    pub villager_profession: VillagerProfession,
    pub villager_level: u32,
    /// Villager head-shake timer; shakes while > 0 (vanilla unhappy counter,
    /// synched then decremented client-side each tick like vanilla does).
    pub unhappy_counter: i32,
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
    /// Chicken `flapping` decay factor.
    flapping: f32,
    target_squish: f32,
    prev_on_ground: bool,
    is_shaking: bool,
    jump_ticks: i32,
    jump_duration: i32,
    tail_counter: u8,
    tentacle_movement: f32,
    tentacle_speed: f32,
    rotate_speed: f32,
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
            // Spawn grounded: on_ground is packet-driven and a stationary
            // entity gets no movement packet for up to 60 ticks.
            on_ground: true,
            wool_color: None,
            is_sheared: false,
            // Vanilla salmon default is MEDIUM (id 1); non-default-only
            // metadata means the size may never be synced.
            variant: if entity_type == EntityKind::Salmon {
                1
            } else {
                0
            },
            flap: 0.0,
            prev_flap: 0.0,
            flap_speed: 0.0,
            prev_flap_speed: 0.0,
            squish: 0.0,
            prev_squish: 0.0,
            slime_size: 1,
            is_creepy: false,
            is_converting: false,
            witch_drinking: false,
            is_sitting: false,
            is_tame: false,
            is_sprinting: false,
            collar_color: 14,
            is_interested: false,
            anger_end_time: -1,
            health: 20.0,
            interested_angle: 0.0,
            prev_interested_angle: 0.0,
            shake_anim: 0.0,
            prev_shake_anim: 0.0,
            is_lying: false,
            relax_state_one: false,
            lie_down_amount: 0.0,
            prev_lie_down_amount: 0.0,
            lie_down_amount_tail: 0.0,
            prev_lie_down_amount_tail: 0.0,
            relax_state_one_amount: 0.0,
            prev_relax_state_one_amount: 0.0,
            hop_anim_start: None,
            is_eating: false,
            is_standing: false,
            is_open_mouth: false,
            eat_anim: 0.0,
            prev_eat_anim: 0.0,
            stand_anim: 0.0,
            prev_stand_anim: 0.0,
            mouth_anim: 0.0,
            prev_mouth_anim: 0.0,
            has_chest: false,
            velocity: DVec3::ZERO,
            is_in_water: false,
            x_body_rot: 0.0,
            prev_x_body_rot: 0.0,
            z_body_rot: 0.0,
            prev_z_body_rot: 0.0,
            tentacle_angle: 0.0,
            prev_tentacle_angle: 0.0,
            bat_resting: false,
            bat_anim_start: None,
            puff_state: 0,
            dark_ticks: 0,
            villager_kind: VillagerKind::default(),
            villager_profession: VillagerProfession::default(),
            villager_level: 0,
            unhappy_counter: 0,
            eat_anim_tick: 0,
            prev_eat_anim_tick: 0,
            hurt_time: 0,
            age_in_ticks: 0,
            custom_name: None,
            aggressive: false,
            powered: false,
            swing_time: 0,
            flapping: 1.0,
            target_squish: 0.0,
            // Vanilla `AbstractCubeMob.wasOnGround` starts false; with the
            // grounded spawn above this reproduces vanilla's first-track
            // landing squash and skips the airborne-spawn stretch.
            prev_on_ground: false,
            is_shaking: false,
            jump_ticks: 0,
            jump_duration: 0,
            tail_counter: 0,
            tentacle_movement: 0.0,
            tentacle_speed: 1.0 / (fastrand::f32() + 1.0) * 0.2,
            rotate_speed: 0.0,
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

    /// Vanilla `Chicken.aiStep` wing flap; the update order matters.
    fn tick_flap(&mut self) {
        self.prev_flap = self.flap;
        self.prev_flap_speed = self.flap_speed;
        let delta = if self.on_ground { -0.3 } else { 1.2 };
        self.flap_speed = (self.flap_speed + delta).clamp(0.0, 1.0);
        if !self.on_ground && self.flapping < 1.0 {
            self.flapping = 1.0;
        }
        self.flapping *= 0.9;
        self.flap += self.flapping * 2.0;
    }

    /// Client-side springs for the wolf beg tilt / shake ramp, cat lie-down
    /// and relax, and the rabbit hop clock (vanilla ticks these on both
    /// sides; only the driving flags are synced). Inert for other mobs.
    fn tick_tamable_anims(&mut self) {
        let spring = |cur: f32, on: bool, up: f32, down: f32| {
            if on {
                (cur + up).min(1.0)
            } else {
                (cur - down).max(0.0)
            }
        };
        self.prev_interested_angle = self.interested_angle;
        self.interested_angle +=
            (if self.is_interested { 1.0 } else { 0.0 } - self.interested_angle) * 0.4;

        if self.is_shaking {
            self.prev_shake_anim = self.shake_anim;
            self.shake_anim += 0.05;
            if self.prev_shake_anim >= 2.0 {
                self.is_shaking = false;
                self.shake_anim = 0.0;
                self.prev_shake_anim = 0.0;
            }
        }

        self.prev_lie_down_amount = self.lie_down_amount;
        self.lie_down_amount = spring(self.lie_down_amount, self.is_lying, 0.15, 0.22);
        self.prev_lie_down_amount_tail = self.lie_down_amount_tail;
        self.lie_down_amount_tail = spring(self.lie_down_amount_tail, self.is_lying, 0.08, 0.13);
        self.prev_relax_state_one_amount = self.relax_state_one_amount;
        self.relax_state_one_amount =
            spring(self.relax_state_one_amount, self.relax_state_one, 0.1, 0.13);

        // Vanilla `Rabbit.aiStep` jump counter + `setupAnimationStates`.
        if self.jump_ticks != self.jump_duration {
            self.jump_ticks += 1;
        } else if self.jump_duration != 0 {
            self.jump_ticks = 0;
            self.jump_duration = 0;
        }
        if self.jump_ticks > 0 {
            if self.hop_anim_start.is_none() {
                self.hop_anim_start = Some(self.age_in_ticks);
            }
        } else {
            self.hop_anim_start = None;
        }
    }

    /// Vanilla `Wolf.getWetShade` grayscale, with wetness approximated by the
    /// shake run (rain wetness isn't sampled).
    // TODO: true isInWaterOrRain wetness for the pre-shake 0.75 darkening.
    pub fn wet_shade(&self, alpha: f32) -> f32 {
        if !self.is_shaking {
            return 1.0;
        }
        let shake = self.prev_shake_anim + (self.shake_anim - self.prev_shake_anim) * alpha;
        (0.75 + shake / 2.0 * 0.25).min(1.0)
    }

    pub fn tail_swishing(&self) -> bool {
        self.tail_counter > 0
    }

    /// Vanilla `AbstractHorse.tick`/`aiStep` springs for the grass-eat,
    /// rear-up and feeding-mouth animations, plus the client-local tail-swish
    /// counter. Gated on the equine kinds (the tail RNG isn't free).
    fn tick_equine_anims(&mut self) {
        if fastrand::u32(0..200) == 0 {
            self.tail_counter = 1;
        }
        if self.tail_counter > 0 {
            self.tail_counter += 1;
            if self.tail_counter > 8 {
                self.tail_counter = 0;
            }
        }
        self.prev_eat_anim = self.eat_anim;
        if self.is_eating {
            self.eat_anim = (self.eat_anim + (1.0 - self.eat_anim) * 0.4 + 0.05).min(1.0);
        } else {
            self.eat_anim = (self.eat_anim - self.eat_anim * 0.4 - 0.05).max(0.0);
        }
        self.prev_stand_anim = self.stand_anim;
        if self.is_standing {
            self.prev_eat_anim = 0.0;
            self.eat_anim = 0.0;
            self.stand_anim = (self.stand_anim + (1.0 - self.stand_anim) * 0.4 + 0.05).min(1.0);
        } else {
            self.stand_anim = (self.stand_anim
                + (0.8 * self.stand_anim * self.stand_anim * self.stand_anim - self.stand_anim)
                    * 0.6
                - 0.05)
                .max(0.0);
        }
        self.prev_mouth_anim = self.mouth_anim;
        if self.is_open_mouth {
            self.mouth_anim = (self.mouth_anim + (1.0 - self.mouth_anim) * 0.7 + 0.05).min(1.0);
        } else {
            self.mouth_anim = (self.mouth_anim - self.mouth_anim * 0.7 - 0.05).max(0.0);
        }
    }

    /// Vanilla `Entity.updateFluidInteraction`: true when any water column in
    /// the (slightly deflated) AABB's block range reaches above the box
    /// bottom. The AABB matches the entity's actual dimensions, including the
    /// baby-squid override and the salmon/pufferfish variant scale.
    fn probe_water(&self, chunks: &ChunkStore) -> bool {
        let (w, h): (f64, f64) = match self.entity_type {
            // `Squid.BABY_DIMENSIONS` is an explicit 0.5x0.5.
            EntityKind::Squid | EntityKind::GlowSquid if self.is_baby => (0.5, 0.5),
            EntityKind::Squid | EntityKind::GlowSquid => (0.8, 0.8),
            EntityKind::Cod => (0.5, 0.3),
            // `Salmon.getSalmonScale`: small 0.5, medium 1.0, large 1.5.
            EntityKind::Salmon => {
                let scale = match self.variant {
                    0 => 0.5,
                    2 => 1.5,
                    _ => 1.0,
                };
                (0.7 * scale, 0.4 * scale)
            }
            EntityKind::TropicalFish => (0.5, 0.4),
            // `Pufferfish.getScale`: states 0/1/2 = 0.5/0.7/1.0.
            EntityKind::Pufferfish => {
                let scale = match self.puff_state {
                    0 => 0.5,
                    1 => 0.7,
                    _ => 1.0,
                };
                (0.7 * scale, 0.7 * scale)
            }
            _ => (0.6, 0.6),
        };
        let min_x = self.position.x - w / 2.0 + 0.001;
        let min_y = self.position.y + 0.001;
        let min_z = self.position.z - w / 2.0 + 0.001;
        let max_x = self.position.x + w / 2.0 - 0.001;
        let max_y = self.position.y + h - 0.001;
        let max_z = self.position.z + w / 2.0 - 0.001;
        for bx in (min_x.floor() as i32)..=(max_x.ceil() as i32 - 1) {
            for by in (min_y.floor() as i32)..=(max_y.ceil() as i32 - 1) {
                for bz in (min_z.floor() as i32)..=(max_z.ceil() as i32 - 1) {
                    let f = fluid(chunks.get_block_state(bx, by, bz));
                    if f.kind != FluidKind::Water {
                        continue;
                    }
                    // Full height when the block above is also water.
                    let above = fluid(chunks.get_block_state(bx, by + 1, bz));
                    let top = by as f64
                        + if above.kind == FluidKind::Water {
                            1.0
                        } else {
                            f.height() as f64
                        };
                    if top > min_y {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Vanilla `Squid.aiStep` body/tentacle sim. The tentacle stroke clock
    /// clamps at 2*pi on the client and only entity event 19 resets it; the
    /// body pitch follows the packet-driven velocity. Vanilla's yaw steering
    /// is skipped (pomme body yaw stays packet-driven).
    fn tick_squid(&mut self) {
        use std::f32::consts::PI;
        self.prev_x_body_rot = self.x_body_rot;
        self.prev_z_body_rot = self.z_body_rot;
        self.prev_tentacle_angle = self.tentacle_angle;
        self.tentacle_movement += self.tentacle_speed;
        if self.tentacle_movement > PI * 2.0 {
            self.tentacle_movement = PI * 2.0;
        }
        if self.is_in_water {
            if self.tentacle_movement < PI {
                let scale = self.tentacle_movement / PI;
                self.tentacle_angle = (scale * scale * PI).sin() * PI * 0.25;
                if scale > 0.75 {
                    self.rotate_speed = 1.0;
                } else {
                    self.rotate_speed *= 0.8;
                }
            } else {
                self.tentacle_angle = 0.0;
                self.rotate_speed *= 0.99;
            }
            let horiz =
                (self.velocity.x * self.velocity.x + self.velocity.z * self.velocity.z).sqrt();
            self.z_body_rot += PI * self.rotate_speed * 1.5;
            self.x_body_rot +=
                (-(horiz.atan2(self.velocity.y) as f32).to_degrees() - self.x_body_rot) * 0.1;
        } else {
            self.tentacle_angle = self.tentacle_movement.sin().abs() * PI * 0.25;
            self.x_body_rot += (-90.0 - self.x_body_rot) * 0.02;
        }
    }

    /// Vanilla `AbstractCubeMob.tick` squish spring; the update order matters.
    fn tick_squish(&mut self) {
        self.prev_squish = self.squish;
        self.squish += (self.target_squish - self.squish) * 0.5;
        if self.on_ground && !self.prev_on_ground {
            self.target_squish = -0.5;
        } else if !self.on_ground && self.prev_on_ground {
            self.target_squish = 1.0;
        }
        self.prev_on_ground = self.on_ground;
        self.target_squish *= 0.6;
    }

    /// Per-kind per-tick animation state (the kind-specific tail of vanilla
    /// `aiStep`); arms accrue as mobs land.
    fn tick_kind_anims(&mut self) {
        match self.entity_type {
            EntityKind::Chicken => self.tick_flap(),
            EntityKind::Slime => self.tick_squish(),
            EntityKind::Squid | EntityKind::GlowSquid => {
                self.tick_squid();
                if self.dark_ticks > 0 {
                    self.dark_ticks -= 1;
                }
            }
            // One animation clock, restarted whenever the resting flag
            // flips (the setter clears it).
            EntityKind::Bat if self.bat_anim_start.is_none() => {
                self.bat_anim_start = Some(self.age_in_ticks);
            }
            k if is_equine(&k) => self.tick_equine_anims(),
            _ => {}
        }
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
    velocity: DVec3,
    on_ground: bool,
    /// Server-authoritative position, tracked from move/teleport packets.
    server_pos: Position,
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

    pub fn spawn_item(&mut self, id: i32, position: Position, velocity: DVec3) {
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
                velocity,
                on_ground: false,
                server_pos: position,
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

    /// Apply a server position delta: advance the authoritative base and
    /// snap to it. Items have no interpolation handler in vanilla
    /// (`moveOrInterpolateTo` -> `setPos`); local physics predicts between
    /// packets and `prev_position` smooths the render lerp.
    pub fn move_delta(&mut self, id: i32, dx: f64, dy: f64, dz: f64, on_ground: bool) {
        if let Some(entity) = self.items.get_mut(&id) {
            entity.server_pos += DVec3::new(dx, dy, dz);
            entity.position = entity.server_pos;
            entity.on_ground = on_ground;
        }
    }

    pub fn teleport(
        &mut self,
        id: i32,
        position: Position,
        velocity: Option<DVec3>,
        on_ground: bool,
    ) {
        if let Some(entity) = self.items.get_mut(&id) {
            // Vanilla suppresses the render lerp on jumps over 64 blocks
            // (`tooBigToInterpolate`).
            if entity.position.distance_squared(*position) > 4096.0 {
                entity.prev_position = position;
            }
            entity.server_pos = position;
            entity.position = position;
            if let Some(velocity) = velocity {
                entity.velocity = velocity;
            }
            entity.on_ground = on_ground;
        }
    }

    /// Vanilla `handleSetEntityMotion`.
    pub fn set_motion(&mut self, id: i32, velocity: DVec3) {
        if let Some(entity) = self.items.get_mut(&id) {
            entity.velocity = velocity;
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

    pub fn tick(&mut self, chunk_store: &ChunkStore) {
        for (&id, entity) in self.items.iter_mut() {
            entity.prev_position = entity.position;
            tick_item_physics(id, entity, chunk_store);
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

/// Vanilla `ItemEntity.getDefaultGravity`.
const ITEM_GRAVITY: f64 = 0.04;
/// Vanilla `Entity.getAirDrag`.
const ITEM_AIR_DRAG: f64 = 0.98;
/// Item hitbox is 0.25 cubed (`EntityType.ITEM` dimensions).
const ITEM_HALF_WIDTH: f64 = 0.125;

/// Client-side port of `ItemEntity.tick` movement: gravity or fluid drift,
/// collide-and-slide, friction, then the half-speed landing bounce.
/// Server-only parts (merging, despawn, pickup delay) are omitted.
fn tick_item_physics(id: i32, entity: &mut ItemEntity, chunk_store: &ChunkStore) {
    let block_x = entity.position.x.floor() as i32;
    let block_y = entity.position.y.floor() as i32;
    let block_z = entity.position.z.floor() as i32;
    let chunk_pos = ChunkPos::new(block_x.div_euclid(16), block_z.div_euclid(16));
    if chunk_store.get_chunk(&chunk_pos).is_none() {
        // Don't simulate (and fall) through unloaded terrain.
        return;
    }

    let state = chunk_store.get_block_state(block_x, block_y, block_z);
    let fluid = fluid(state);
    // Vanilla `getFluidHeight(...) > 0.1`: how far the fluid surface sits
    // above the item's feet, sampled at the position block.
    let fluid_height = block_y as f64 + fluid.height() as f64 - entity.position.y;
    match fluid.kind {
        FluidKind::Water if fluid_height > 0.1 => apply_item_fluid_movement(entity, 0.99),
        FluidKind::Lava if fluid_height > 0.1 => apply_item_fluid_movement(entity, 0.95),
        _ => entity.velocity.y -= ITEM_GRAVITY,
    }

    // Vanilla rest throttle: a settled item only re-runs collision every
    // 4th tick. `age` increments after this runs; vanilla's `tickCount`
    // increments before, hence the +1.
    let horizontal_sq =
        entity.velocity.x * entity.velocity.x + entity.velocity.z * entity.velocity.z;
    if entity.on_ground && horizontal_sq <= 1e-5 && (entity.age as i64 + 1 + id as i64) % 4 != 0 {
        return;
    }

    let aabb = Aabb::from_center(entity.position.into(), ITEM_HALF_WIDTH, ITEM_HALF_WIDTH);
    let (delta, on_ground) = resolve_collision(chunk_store, aabb, entity.velocity.into(), 0.0);
    entity.position += delta;
    entity.on_ground = on_ground;

    // TODO: per-block slipperiness (ice/slime); vanilla multiplies by the
    // friction of the block below, default 0.6.
    let ground_friction = if on_ground {
        ITEM_AIR_DRAG * 0.6
    } else {
        ITEM_AIR_DRAG
    };
    entity.velocity.x *= ground_friction;
    entity.velocity.y *= ITEM_AIR_DRAG;
    entity.velocity.z *= ground_friction;
    if on_ground && entity.velocity.y < 0.0 {
        entity.velocity.y *= -0.5;
    }
}

/// Vanilla `ItemEntity.setFluidMovement`: horizontal drag plus a slow
/// upward drift toward the surface.
fn apply_item_fluid_movement(entity: &mut ItemEntity, multiplier: f64) {
    entity.velocity.x *= multiplier;
    entity.velocity.z *= multiplier;
    if entity.velocity.y < 0.06 {
        entity.velocity.y += 5.0e-4;
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

    pub fn move_living_delta(&mut self, id: i32, dx: f64, dy: f64, dz: f64, on_ground: bool) {
        if let Some(entity) = self.living.get_mut(&id) {
            let target = entity.interp_target + DVec3::new(dx, dy, dz);
            entity.interpolate_to_pos(target);
            entity.on_ground = on_ground;
        }
    }

    pub fn teleport_living(&mut self, id: i32, position: Position, on_ground: bool) {
        if let Some(entity) = self.living.get_mut(&id) {
            entity.interpolate_to_pos(position);
            entity.on_ground = on_ground;
        }
    }

    pub fn set_baby(&mut self, id: i32, is_baby: bool) {
        if let Some(entity) = self.living.get_mut(&id)
            // On bogged, entity-data index 16 is the sheared flag, not baby;
            // on fish (not ageable) it's `AbstractFish.FROM_BUCKET`.
            && !matches!(
                entity.entity_type,
                EntityKind::Bogged
                    | EntityKind::Cod
                    | EntityKind::Salmon
                    | EntityKind::TropicalFish
                    | EntityKind::Pufferfish
            )
        {
            entity.is_baby = is_baby;
        }
    }

    pub fn set_bogged_sheared(&mut self, id: i32, sheared: bool) {
        if let Some(entity) = self.living.get_mut(&id)
            && entity.entity_type == EntityKind::Bogged
        {
            entity.is_sheared = sheared;
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

    /// `kind` is the mob the emitting handler arm resolved the value for;
    /// metadata indices are overloaded across kinds, so a mismatched entity
    /// ignores the write.
    pub fn set_variant(&mut self, id: i32, kind: EntityKind, raw: u32) {
        if let Some(entity) = self.living.get_mut(&id)
            && entity.entity_type == kind
        {
            entity.variant = match entity.entity_type {
                // Vanilla sparse rabbit id map: 99 = evil, unknown ids fall
                // back to brown.
                EntityKind::Rabbit => match raw {
                    0..=5 => raw,
                    99 => 6,
                    _ => 0,
                },
                // Horse packed variant: `color | markings << 8`, both wrapping
                // their id ranges (vanilla `ByIdMap` WRAP).
                EntityKind::Horse => ((raw & 0xFF) % 7) | ((((raw >> 8) & 0xFF) % 5) << 8),
                // Salmon size ids stop at LARGE (2).
                EntityKind::Salmon => raw.min(2),
                // Holder-backed indices (cow/chicken/wolf/cat) are
                // pre-resolved by the net handler.
                _ => raw,
            };
        }
    }

    /// Applies a [`MobFlag`] write, dropping it when the entity isn't the
    /// flag's mob (metadata indices are overloaded across kinds, so the net
    /// handler emits every candidate flag for an ambiguous boolean).
    pub fn set_mob_flag(&mut self, id: i32, flag: MobFlag, value: bool) {
        let Some(entity) = self.living.get_mut(&id) else {
            return;
        };
        match (flag, entity.entity_type) {
            (MobFlag::CreeperPowered, EntityKind::Creeper) => entity.powered = value,
            (MobFlag::EndermanCreepy, EntityKind::Enderman) => entity.is_creepy = value,
            (MobFlag::WitchDrinking, EntityKind::Witch) => entity.witch_drinking = value,
            (
                MobFlag::ZombieConverting,
                EntityKind::Zombie | EntityKind::Husk | EntityKind::Drowned,
            ) => entity.is_converting = value,
            (MobFlag::ZombieVillagerConverting, EntityKind::ZombieVillager) => {
                entity.is_converting = value
            }
            (MobFlag::WolfInterested, EntityKind::Wolf) => entity.is_interested = value,
            (MobFlag::CatLying, EntityKind::Cat) => entity.is_lying = value,
            (MobFlag::CatRelaxed, EntityKind::Cat) => entity.relax_state_one = value,
            (MobFlag::BatResting, EntityKind::Bat) => {
                if entity.bat_resting != value {
                    entity.bat_resting = value;
                    entity.bat_anim_start = None;
                }
            }
            _ => {}
        }
    }

    pub fn set_slime_size(&mut self, id: i32, size: i32) {
        if let Some(entity) = self.living.get_mut(&id)
            && entity.entity_type == EntityKind::Slime
        {
            entity.slime_size = size.clamp(1, 127) as u8;
        }
    }

    pub fn set_villager_data(
        &mut self,
        id: i32,
        kind: VillagerKind,
        profession: VillagerProfession,
        level: u32,
    ) {
        if let Some(entity) = self.living.get_mut(&id)
            && matches!(
                entity.entity_type,
                EntityKind::Villager | EntityKind::ZombieVillager
            )
        {
            entity.villager_kind = kind;
            entity.villager_profession = profession;
            entity.villager_level = level;
        }
    }

    pub fn set_villager_unhappy(&mut self, id: i32, counter: i32) {
        if let Some(entity) = self.living.get_mut(&id)
            && entity.entity_type == EntityKind::Villager
        {
            entity.unhappy_counter = counter;
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

    pub fn set_tamable_flags(&mut self, id: i32, sitting: bool, tame: bool) {
        if let Some(entity) = self.living.get_mut(&id)
            && matches!(entity.entity_type, EntityKind::Wolf | EntityKind::Cat)
        {
            entity.is_sitting = sitting;
            entity.is_tame = tame;
        }
    }

    pub fn set_collar_color(&mut self, id: i32, color: u8) {
        if let Some(entity) = self.living.get_mut(&id)
            && matches!(entity.entity_type, EntityKind::Wolf | EntityKind::Cat)
        {
            entity.collar_color = color & 0x0F;
        }
    }

    pub fn set_wolf_anger(&mut self, id: i32, end_time: i64) {
        if let Some(entity) = self.living.get_mut(&id)
            && entity.entity_type == EntityKind::Wolf
        {
            entity.anger_end_time = end_time;
        }
    }

    /// Wolf wet-shake start / cancel (entity events 8 / 56).
    pub fn set_wolf_shaking(&mut self, id: i32, shaking: bool) {
        if let Some(entity) = self.living.get_mut(&id)
            && entity.entity_type == EntityKind::Wolf
        {
            entity.is_shaking = shaking;
            entity.shake_anim = 0.0;
            entity.prev_shake_anim = 0.0;
        }
    }

    /// Rabbit hop (entity event 1): vanilla sets a 15-tick jump run.
    pub fn start_rabbit_jump(&mut self, id: i32) {
        if let Some(entity) = self.living.get_mut(&id)
            && entity.entity_type == EntityKind::Rabbit
        {
            entity.jump_duration = 15;
            entity.jump_ticks = 0;
        }
    }

    pub fn set_equine_flags(&mut self, id: i32, eating: bool, standing: bool, open_mouth: bool) {
        if let Some(entity) = self.living.get_mut(&id)
            && is_equine(&entity.entity_type)
        {
            entity.is_eating = eating;
            entity.is_standing = standing;
            entity.is_open_mouth = open_mouth;
        }
    }

    pub fn set_chested(&mut self, id: i32, chest: bool) {
        if let Some(entity) = self.living.get_mut(&id)
            && matches!(entity.entity_type, EntityKind::Donkey | EntityKind::Mule)
        {
            entity.has_chest = chest;
        }
    }

    pub fn set_living_motion(&mut self, id: i32, velocity: DVec3) {
        if let Some(entity) = self.living.get_mut(&id) {
            entity.velocity = velocity;
        }
    }

    pub fn set_puff_state(&mut self, id: i32, state: i32) {
        if let Some(entity) = self.living.get_mut(&id)
            && entity.entity_type == EntityKind::Pufferfish
        {
            entity.puff_state = state.clamp(0, 2) as u8;
        }
    }

    pub fn set_glow_squid_dark_ticks(&mut self, id: i32, ticks: i32) {
        if let Some(entity) = self.living.get_mut(&id)
            && entity.entity_type == EntityKind::GlowSquid
        {
            entity.dark_ticks = ticks;
        }
    }

    /// Entity event 19: the server rolled the tentacle clock over.
    pub fn squid_tentacle_reset(&mut self, id: i32) {
        if let Some(entity) = self.living.get_mut(&id)
            && matches!(
                entity.entity_type,
                EntityKind::Squid | EntityKind::GlowSquid
            )
        {
            entity.tentacle_movement = 0.0;
        }
    }

    pub fn set_health(&mut self, id: i32, health: f32) {
        if let Some(entity) = self.living.get_mut(&id) {
            entity.health = health;
        }
    }

    pub fn set_sprinting(&mut self, id: i32, sprinting: bool) {
        if let Some(entity) = self.living.get_mut(&id) {
            entity.is_sprinting = sprinting;
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

    /// Rotation half of any movement packet: rotation plus onGround. Extends
    /// any in-flight position lerp instead of re-targeting it (vanilla
    /// `moveOrInterpolateTo` rotation overloads).
    pub fn rotate_living(&mut self, id: i32, y_rot_deg: f32, x_rot_deg: f32, on_ground: bool) {
        if let Some(entity) = self.living.get_mut(&id) {
            entity.interp_look_dir = LookDirection::new(y_rot_deg, x_rot_deg);
            entity.interp_steps = entity.interp_steps.max(INTERPOLATION_STEPS);
            entity.on_ground = on_ground;
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
        self.player_by_uuid(uuid).is_some()
    }

    pub fn player_by_uuid(&self, uuid: &uuid::Uuid) -> Option<&LivingEntity> {
        self.living
            .values()
            .find(|entity| entity.player_uuid == Some(*uuid))
    }

    pub fn tick_living(&mut self, chunks: &ChunkStore) {
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
            entity.tick_kind_anims();
            entity.tick_tamable_anims();
            if probes_water(&entity.entity_type) {
                entity.is_in_water = entity.probe_water(chunks);
            }
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
            if entity.unhappy_counter > 0 {
                entity.unhappy_counter -= 1;
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

/// The horse family (vanilla `AbstractHorse` subclasses pomme renders).
pub fn is_equine(kind: &EntityKind) -> bool {
    matches!(
        kind,
        EntityKind::Horse
            | EntityKind::Donkey
            | EntityKind::Mule
            | EntityKind::SkeletonHorse
            | EntityKind::ZombieHorse
    )
}

pub fn is_living_mob(kind: &EntityKind) -> bool {
    is_equine(kind)
        || matches!(
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
                | EntityKind::Villager
                | EntityKind::Enderman
                | EntityKind::Slime
                | EntityKind::Witch
                | EntityKind::Husk
                | EntityKind::Drowned
                | EntityKind::ZombieVillager
                | EntityKind::Stray
                | EntityKind::Bogged
                | EntityKind::Wolf
                | EntityKind::Cat
                | EntityKind::Ocelot
                | EntityKind::Rabbit
                | EntityKind::Squid
                | EntityKind::GlowSquid
                | EntityKind::Bat
                | EntityKind::Cod
                | EntityKind::Salmon
                | EntityKind::TropicalFish
                | EntityKind::Pufferfish
        )
}

/// Kinds whose `wasTouchingWater` matters for rendering (fish flop pose,
/// squid body rotation).
fn probes_water(kind: &EntityKind) -> bool {
    matches!(
        kind,
        EntityKind::Squid
            | EntityKind::GlowSquid
            | EntityKind::Cod
            | EntityKind::Salmon
            | EntityKind::TropicalFish
            | EntityKind::Pufferfish
    )
}
