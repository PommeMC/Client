pub mod interaction;
pub mod inventory;
pub mod menu_click;
pub mod tab_list;

use glam::{dvec2, dvec3};
use inventory::Inventory;

use crate::entity::HURT_DURATION;
use crate::entity::components::{LookDirection, Position, Velocity};
use crate::physics::aabb::Aabb;
use crate::world::block::{Fluid, FluidKind, block_id, blocks_motion, fluid, is_full_face_sturdy};

pub const MAX_AIR_SUPPLY: i32 = 300;
// Vanilla stores player dimensions/eye heights as floats and only widens them
// when combining them with double-precision positions and AABBs.
pub const PLAYER_HALF_WIDTH: f64 = (0.6_f32 / 2.0_f32) as f64;
pub const STANDING_HEIGHT: f64 = 1.8_f32 as f64;
pub const CROUCH_HEIGHT: f64 = 1.5_f32 as f64;
pub const SWIMMING_HEIGHT: f64 = 0.6_f32 as f64;
pub const STANDING_EYE_HEIGHT: f32 = 1.62;
pub const CROUCH_EYE_HEIGHT: f32 = 1.27;
pub const SWIMMING_EYE_HEIGHT: f32 = 0.4;
// Entity.checkInsideBlocks passes the float literal through AABB's double API.
const INSIDE_BLOCK_MARGIN: f64 = 1.0e-5_f32 as f64;
const DROWN_DAMAGE_THRESHOLD: i32 = -20;
const DROWN_DAMAGE: f32 = 2.0;
const AIR_RECOVERY_RATE: i32 = 4;

// TODO: migrate the remaining raw `game_mode == N` checks to shared constants
// or an enum.
/// Matches vanilla GameType.isSurvival(): Survival (0) or Adventure (2).
pub fn is_survival(game_mode: u8) -> bool {
    game_mode == 0 || game_mode == 2
}

/// Matches vanilla GameType.isCreative(): Creative (1).
pub fn is_creative(game_mode: u8) -> bool {
    game_mode == 1
}

/// Spectator (3).
pub fn is_spectator(game_mode: u8) -> bool {
    game_mode == 3
}

fn is_water_block(state: azalea_block::BlockState) -> bool {
    fluid(state).kind == FluidKind::Water
}

#[inline]
fn vanilla_vec3_normalize(v: glam::DVec3) -> glam::DVec3 {
    let length = v.length();
    if length < f64::from(1.0e-5_f32) {
        glam::DVec3::ZERO
    } else {
        v / length
    }
}

#[inline]
fn affects_water_flow(f: Fluid) -> bool {
    matches!(f.kind, FluidKind::Empty | FluidKind::Water)
}

fn water_flow_at(
    chunks: &crate::world::chunk::ChunkStore,
    x: i32,
    y: i32,
    z: i32,
    state_fluid: Fluid,
) -> glam::DVec3 {
    debug_assert_eq!(state_fluid.kind, FluidKind::Water);
    let own_height = state_fluid.height();
    let mut flow_x = 0.0_f64;
    let mut flow_z = 0.0_f64;

    // Direction.Plane.HORIZONTAL iteration order in vanilla is
    // NORTH, EAST, SOUTH, WEST.
    for (dx, dz) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
        let nx = x + dx;
        let nz = z + dz;
        let neighbor_state = chunks.get_block_state(nx, y, nz);
        let neighbor_fluid = fluid(neighbor_state);
        if !affects_water_flow(neighbor_fluid) {
            continue;
        }

        let mut neighbor_height = neighbor_fluid.height();
        let mut distance = 0.0_f32;
        if neighbor_height == 0.0 {
            let below_fluid = fluid(chunks.get_block_state(nx, y - 1, nz));
            if !blocks_motion(neighbor_state) && affects_water_flow(below_fluid) && {
                neighbor_height = below_fluid.height();
                neighbor_height > 0.0
            } {
                distance = own_height - (neighbor_height - 0.888_888_9_f32);
            }
        } else if neighbor_height > 0.0 {
            distance = own_height - neighbor_height;
        }
        if distance != 0.0 {
            flow_x += f64::from((dx as f32) * distance);
            flow_z += f64::from((dz as f32) * distance);
        }
    }

    let mut flow = dvec3(flow_x, 0.0, flow_z);
    if state_fluid.falling {
        for (dx, dz, direction) in [(0, -1, 2_usize), (1, 0, 5), (0, 1, 3), (-1, 0, 4)] {
            let nx = x + dx;
            let nz = z + dz;
            let solid_face = |by| {
                let state = chunks.get_block_state(nx, by, nz);
                let same_water = fluid(state).kind == FluidKind::Water;
                !same_water
                    && !matches!(block_id(state), "ice" | "frosted_ice")
                    && is_full_face_sturdy(state, direction)
            };
            if solid_face(y) || solid_face(y + 1) {
                flow = vanilla_vec3_normalize(flow) + dvec3(0.0, -6.0, 0.0);
                break;
            }
        }
    }
    vanilla_vec3_normalize(flow)
}

pub struct LocalPlayer {
    pub position: Position,
    pub prev_position: Position,
    pub velocity: Velocity,
    pub look_dir: LookDirection,
    pub prev_look_dir: LookDirection,
    pub on_ground: bool,
    /// Vanilla `Entity.fallDistance`, used by powder-snow collision and fall
    /// handling. Stored as f64 in 26.2.
    pub fall_distance: f64,
    pub health: f32,
    pub death_time: u32,
    pub absorption: f32,
    pub max_health: f32,
    pub hurt_time: u8,
    pub hurt_dir: f32,
    flash_on_set_health: bool,
    pub food: u32,
    pub armor: u32,
    pub saturation: f32,
    pub inventory: Inventory,
    pub sprinting: bool,
    /// Physical Entity pose: true when the current pose is CROUCHING. This is
    /// selected by Player.updatePlayerPose at tick end and controls dimensions.
    pub crouching: bool,
    /// Vanilla LocalPlayer's private `crouching` movement flag. aiStep updates
    /// this before KeyboardInput.tick; it can differ from the physical pose on
    /// crouch/swim transitions.
    pub movement_crouching: bool,
    // TODO: remaining Abilities fields - invulnerable, instabuild, may_build
    pub flying: bool,
    pub may_fly: bool,
    pub fly_speed: f32,
    pub walk_speed: f32,
    pub jump_trigger_time: u32,
    pub no_jump_delay: u32,
    pub was_jump_pressed: bool,
    /// Vanilla `jumpRidingTicks`: ride-jump charge ticks; negative counts up
    /// through the post-jump refractory window.
    pub jump_riding_ticks: i32,
    /// Vanilla `jumpRidingScale`: jump-bar fill, 0..=1.
    pub jump_riding_scale: f32,
    /// Vanilla `onUpdateAbilities`: a locally toggled `flying` still has to be
    /// reported to the server via `ServerboundPlayerAbilities`.
    pub abilities_dirty: bool,
    pub eye_height: f32,
    pub prev_eye_height: f32,
    pub walk_dist: f32,
    pub prev_walk_dist: f32,
    pub bob: f32,
    pub prev_bob: f32,
    pub horizontal_collision: bool,
    /// Vanilla `Entity.minorHorizontalCollision`, produced by `Entity.move` and
    /// consumed by the following `LocalPlayer.aiStep` sprint-stop decision.
    pub minor_horizontal_collision: bool,
    /// Vanilla `Entity.mainSupportingBlockPos`, retained across ticks so block
    /// friction/jump/speed properties use the actual supporting collider rather
    /// than whichever block happens to contain the player's center point.
    pub main_supporting_block_pos: Option<azalea_core::position::BlockPos>,
    /// Vanilla `Entity.onGroundNoBlocks`; controls the one-tick fallback search
    /// at the previous horizontal position when support changes underfoot.
    pub on_ground_no_blocks: bool,
    pub sprint_toggle_timer: u32,
    /// Vanilla LocalPlayer.aiStep samples these from ClientInput before
    /// KeyboardInput.tick refreshes the current physical keys.
    pub was_forward_pressed: bool,
    pub was_shift_pressed: bool,
    pub in_water: bool,
    /// Vanilla `getFluidHeight(WATER)`: water surface height above the feet.
    pub fluid_height: f64,
    pub eyes_in_water: bool,
    /// Vanilla `LocalPlayer.wasUnderwater`. `Player.tick` snapshots this from
    /// the previous EntityFluidInteraction before `Entity.baseTick` refreshes
    /// the current tick's eye-fluid state, so surface swim transitions lag raw
    /// eye contact by one tick.
    pub under_water: bool,
    /// Vanilla shared swimming flag, updated during Entity.baseTick.
    pub swimming: bool,
    /// Physical Pose.SWIMMING selected later by Player.updatePlayerPose. This
    /// intentionally lags `swimming` by the remainder of the current tick.
    pub swimming_pose: bool,
    pub air_supply: i32,
    /// Vanilla LocalPlayer.portalEffectIntensity: drives the full-screen
    /// portal overlay while standing in a nether portal.
    pub portal_effect_intensity: f32,
    pub prev_portal_effect_intensity: f32,
    /// Vanilla LivingEntity SLEEPING_POS metadata: Some while in a bed.
    pub sleeping_pos: Option<azalea_core::position::BlockPos>,
    /// Vanilla Player.sleepCounter: drives the sleep overlay fade.
    pub sleep_counter: u32,
    pub game_mode: u8,
    pub score: i32,
    pub entity_id: i32,
    pub experience_level: i32,
    pub experience_progress: f32,
    pub effects: crate::mob_effect::ActiveMobEffects,
}

impl LocalPlayer {
    pub fn new() -> Self {
        Self {
            position: Position::default(),
            prev_position: Position::default(),
            velocity: Velocity::default(),
            look_dir: LookDirection::default(),
            prev_look_dir: LookDirection::default(),
            on_ground: false,
            fall_distance: 0.0,
            health: 20.0,
            death_time: 0,
            absorption: 0.0,
            max_health: 20.0,
            hurt_time: 0,
            hurt_dir: 0.0,
            flash_on_set_health: false,
            food: 20,
            armor: 0,
            saturation: 5.0,
            inventory: Inventory::new(),
            sprinting: false,
            crouching: false,
            movement_crouching: false,
            flying: false,
            may_fly: false,
            fly_speed: 0.05,
            walk_speed: 0.1,
            jump_trigger_time: 0,
            no_jump_delay: 0,
            was_jump_pressed: false,
            jump_riding_ticks: 0,
            jump_riding_scale: 0.0,
            abilities_dirty: false,
            eye_height: STANDING_EYE_HEIGHT,
            prev_eye_height: STANDING_EYE_HEIGHT,
            walk_dist: 0.0,
            prev_walk_dist: 0.0,
            bob: 0.0,
            prev_bob: 0.0,
            horizontal_collision: false,
            minor_horizontal_collision: false,
            main_supporting_block_pos: None,
            on_ground_no_blocks: false,
            sprint_toggle_timer: 0,
            was_forward_pressed: false,
            was_shift_pressed: false,
            in_water: false,
            fluid_height: 0.0,
            eyes_in_water: false,
            under_water: false,
            swimming: false,
            swimming_pose: false,
            air_supply: MAX_AIR_SUPPLY,
            portal_effect_intensity: 0.0,
            prev_portal_effect_intensity: 0.0,
            sleeping_pos: None,
            sleep_counter: 0,
            game_mode: 0,
            score: 0,
            entity_id: -1,
            experience_level: 0,
            experience_progress: 0.0,
            effects: crate::mob_effect::ActiveMobEffects::default(),
        }
    }

    pub fn reset_death_time(&mut self) {
        self.death_time = 0;
    }

    /// ClientboundRespawn always constructs a fresh LocalPlayer. Keep bit 2
    /// then restores only synced entity data plus velocity/rotation; ordinary
    /// player state such as inventory, food and XP comes from fresh defaults
    /// until the server synchronizes it again.
    pub fn reset_for_respawn(&mut self, keep_entity_data: bool) {
        let kept_health = self.health;
        let kept_absorption = self.absorption;
        let kept_score = self.score;
        let kept_velocity = self.velocity;
        let kept_look = self.look_dir;
        let kept_sprinting = self.sprinting;
        let kept_swimming = self.swimming;
        let kept_crouching = self.crouching;
        let kept_swimming_pose = self.swimming_pose;
        let kept_air_supply = self.air_supply;
        let kept_sleeping_pos = self.sleeping_pos;

        self.death_time = 0;
        self.reset_hurt_state();
        self.effects = crate::mob_effect::ActiveMobEffects::default();
        self.inventory = Inventory::new();
        self.food = 20;
        self.armor = 0;
        self.saturation = 5.0;
        self.experience_level = 0;
        self.experience_progress = 0.0;
        self.flying = false;
        self.may_fly = false;
        self.fly_speed = 0.05;
        self.walk_speed = 0.1;
        self.jump_trigger_time = 0;
        self.no_jump_delay = 0;
        self.was_jump_pressed = false;
        self.jump_riding_ticks = 0;
        self.jump_riding_scale = 0.0;
        self.abilities_dirty = false;
        self.eye_height = STANDING_EYE_HEIGHT;
        self.prev_eye_height = STANDING_EYE_HEIGHT;
        self.walk_dist = 0.0;
        self.prev_walk_dist = 0.0;
        self.bob = 0.0;
        self.prev_bob = 0.0;
        self.horizontal_collision = false;
        self.minor_horizontal_collision = false;
        self.main_supporting_block_pos = None;
        self.on_ground_no_blocks = false;
        self.fall_distance = 0.0;
        self.sprint_toggle_timer = 0;
        self.was_forward_pressed = false;
        self.was_shift_pressed = false;
        self.movement_crouching = false;
        self.in_water = false;
        self.fluid_height = 0.0;
        self.eyes_in_water = false;
        self.under_water = false;
        self.swimming = false;
        self.swimming_pose = false;
        self.sleep_counter = 0;
        self.on_ground = false;

        if keep_entity_data {
            // SynchedEntityData copied by vanilla includes these Pomme-modeled
            // values; movement and rotation are copied explicitly as well.
            self.health = kept_health;
            self.absorption = kept_absorption;
            self.score = kept_score;
            self.velocity = kept_velocity;
            self.look_dir = kept_look;
            self.prev_look_dir = kept_look;
            self.sprinting = kept_sprinting;
            self.swimming = kept_swimming;
            self.crouching = kept_crouching;
            self.swimming_pose = kept_swimming_pose;
            self.air_supply = kept_air_supply;
            self.sleeping_pos = kept_sleeping_pos;
        } else {
            self.health = 20.0;
            self.absorption = 0.0;
            self.score = 0;
            self.velocity = Velocity::default();
            self.look_dir = LookDirection::new(-180.0, 0.0);
            self.prev_look_dir = self.look_dir;
            self.sprinting = false;
            self.crouching = false;
            self.air_supply = MAX_AIR_SUPPLY;
            self.sleeping_pos = None;
        }
    }

    pub fn sync_shared_flags(&mut self, flags: u8) {
        // Entity shared flags: bit 3 = sprinting, bit 4 = swimming. Vanilla
        // applies ClientboundSetEntityData to LocalPlayer just like any other
        // entity; Pomme keeps the local player outside EntityStore, so these
        // prediction-critical bits must be mirrored explicitly.
        self.sprinting = flags & 0x08 != 0;
        self.swimming = flags & 0x10 != 0;
    }

    pub fn snapshot_render_state(&mut self) {
        self.prev_position = self.position;
        self.prev_look_dir = self.look_dir;
        self.prev_eye_height = self.eye_height;
    }

    pub fn tick_death(&mut self) {
        self.death_time = (self.death_time + 1).min(20);
    }

    pub fn death_animation_finished(&self) -> bool {
        self.death_time >= 20
    }

    pub fn apply_server_health(&mut self, health: f32) {
        if self.flash_on_set_health && health < self.health {
            self.mark_hurt();
        }
        self.health = health;
        self.flash_on_set_health = true;
    }

    pub fn mark_hurt(&mut self) {
        self.hurt_time = HURT_DURATION;
    }

    pub fn animate_hurt(&mut self, yaw: f32) {
        self.mark_hurt();
        self.hurt_dir = yaw;
    }

    pub fn tick_hurt(&mut self) {
        if self.hurt_time > 0 {
            self.hurt_time -= 1;
        }
    }

    /// Login and respawn build a fresh vanilla `LocalPlayer`, so hurt state
    /// does not survive a life.
    pub fn reset_hurt_state(&mut self) {
        self.hurt_time = 0;
        self.hurt_dir = 0.0;
        self.flash_on_set_health = false;
    }

    pub fn height(&self) -> f64 {
        if self.swimming_pose {
            SWIMMING_HEIGHT
        } else if self.crouching {
            CROUCH_HEIGHT
        } else {
            STANDING_HEIGHT
        }
    }

    pub fn bounding_box(&self) -> Aabb {
        Aabb::from_center(self.position.into(), PLAYER_HALF_WIDTH, self.height() / 2.0)
    }

    pub fn target_eye_height(&self) -> f32 {
        if self.swimming_pose {
            SWIMMING_EYE_HEIGHT
        } else if self.crouching {
            CROUCH_EYE_HEIGHT
        } else {
            STANDING_EYE_HEIGHT
        }
    }

    pub fn tick_eye_height(&mut self) {
        self.prev_eye_height = self.eye_height;
        self.eye_height += (self.target_eye_height() - self.eye_height) * 0.5;
    }

    /// Accumulates walk distance and a smoothed bob amplitude for view bobbing,
    /// mirroring vanilla `AbstractClientPlayer.updateBob`.
    pub fn tick_bob(&mut self, dx: f64, dz: f64, dead: bool) {
        self.prev_walk_dist = self.walk_dist;
        // Vanilla LocalPlayer.move: addWalkedDistance(len * 0.6).
        self.walk_dist += dvec2(dx, dz).length() as f32 * 0.6;
        // updateBob's target is horizontal speed, not the walk delta.
        let target = if !dead && self.on_ground && !self.swimming {
            (dvec2(self.velocity.x, self.velocity.z).length() as f32).min(0.1)
        } else {
            0.0
        };
        self.prev_bob = self.bob;
        self.bob += (target - self.bob) * 0.4;
    }

    pub fn prev_eye_pos(&self) -> Position {
        self.prev_position + dvec3(0.0, f64::from(self.prev_eye_height), 0.0)
    }

    pub fn eye_pos(&self) -> Position {
        self.position + dvec3(0.0, f64::from(self.eye_height), 0.0)
    }

    // TODO: OXYGEN_BONUS attribute - chance to skip air loss per tick
    pub fn tick_air_supply(&mut self) {
        if self.eyes_in_water {
            self.air_supply -= 1;
            if self.air_supply <= DROWN_DAMAGE_THRESHOLD {
                self.air_supply = 0;
                self.health = (self.health - DROWN_DAMAGE).max(0.0);
            }
        } else if self.air_supply < MAX_AIR_SUPPLY {
            self.air_supply = (self.air_supply + AIR_RECOVERY_RATE).min(MAX_AIR_SUPPLY);
        }
    }

    pub fn update_water_state(
        &mut self,
        chunks: &crate::world::chunk::ChunkStore,
        is_passenger: bool,
    ) {
        self.refresh_water_interaction(chunks);
        self.update_swimming_state(chunks, is_passenger);
    }

    /// Vanilla `Entity.updateFluidInteraction()` without the subsequent
    /// `Entity.updateSwimming()`. `LivingEntity.checkFallDamage()` calls this
    /// again after a dry movement enters fluid, so movement code needs this
    /// operation independently from swimming-state selection.
    pub(crate) fn refresh_water_interaction(&mut self, chunks: &crate::world::chunk::ChunkStore) {
        self.update_water_interaction_for_dimensions(
            chunks,
            PLAYER_HALF_WIDTH,
            self.height(),
            f64::from(self.target_eye_height()),
        );
    }

    fn update_water_interaction_for_dimensions(
        &mut self,
        chunks: &crate::world::chunk::ChunkStore,
        half_w: f64,
        height: f64,
        eye_height: f64,
    ) {
        // Vanilla `EntityFluidInteraction.update`: scan the bounding box
        // deflated by 0.001; a block's fluid column is `amount / 9` of a
        // block, or a full block when more water sits directly above.
        const MARGIN: f64 = 0.001;
        let feet_y = self.position.y;
        let x0 = (self.position.x - half_w + MARGIN).floor() as i32;
        let x1 = (self.position.x + half_w - MARGIN).ceil() as i32 - 1;
        let y0 = (feet_y + MARGIN).floor() as i32;
        let y1 = (feet_y + height - MARGIN).ceil() as i32 - 1;
        let z0 = (self.position.z - half_w + MARGIN).floor() as i32;
        let z1 = (self.position.z + half_w - MARGIN).ceil() as i32 - 1;

        let mut fluid_height = 0.0f64;
        let mut accumulated_current = glam::DVec3::ZERO;
        let mut current_count = 0_u32;
        for bx in x0..=x1 {
            for by in y0..=y1 {
                for bz in z0..=z1 {
                    let f = fluid(chunks.get_block_state(bx, by, bz));
                    if f.kind != FluidKind::Water {
                        continue;
                    }
                    let block_height = if is_water_block(chunks.get_block_state(bx, by + 1, bz)) {
                        1.0
                    } else {
                        // f32 to match vanilla's float math.
                        f64::from(f.amount as f32 / 9.0)
                    };
                    let fluid_top = f64::from(by) + block_height;
                    if fluid_top < feet_y + MARGIN {
                        continue;
                    }

                    fluid_height = fluid_height.max(fluid_top - feet_y);
                    let mut flow = water_flow_at(chunks, bx, by, bz, f);
                    // EntityFluidInteraction scales each sampled flow by the
                    // tracker's current max submerged height when below 0.4.
                    if fluid_height < 0.4 {
                        flow *= fluid_height;
                    }
                    accumulated_current += flow;
                    current_count += 1;
                }
            }
        }

        if !self.flying
            && current_count != 0
            && accumulated_current.length_squared() >= f64::from(1.0e-5_f32)
        {
            // Players average intersecting fluid currents instead of
            // normalizing the accumulated vector like non-player entities.
            let mut impulse = accumulated_current / f64::from(current_count);
            impulse *= 0.014;
            if self.velocity.x.abs() < 0.003
                && self.velocity.z.abs() < 0.003
                && impulse.length() < 0.004_500_000_000_000_000_5
            {
                impulse = vanilla_vec3_normalize(impulse) * 0.004_500_000_000_000_000_5;
            }
            self.velocity.x += impulse.x;
            self.velocity.y += impulse.y;
            self.velocity.z += impulse.z;
        }

        let eye_y = self.position.y + eye_height;
        let eye_block_x = self.position.x.floor() as i32;
        let eye_block_y = eye_y.floor() as i32;
        let eye_block_z = self.position.z.floor() as i32;
        let eye_fluid = fluid(chunks.get_block_state(eye_block_x, eye_block_y, eye_block_z));
        let eye_fluid_top = if eye_fluid.kind == FluidKind::Water {
            let above = chunks.get_block_state(eye_block_x, eye_block_y + 1, eye_block_z);
            f64::from(eye_block_y)
                + if is_water_block(above) {
                    1.0
                } else {
                    f64::from(eye_fluid.height())
                }
        } else {
            f64::NEG_INFINITY
        };

        self.fluid_height = fluid_height;
        // Vanilla `wasTouchingWater` is exactly "fluid height > 0".
        self.in_water = fluid_height > 0.0;
        if self.in_water {
            // Entity.updateFluidInteraction resets fall distance immediately,
            // including the mid-move refresh from LivingEntity.checkFallDamage.
            self.fall_distance = 0.0;
        }
        // EntityFluidInteraction marks eyes-inside only when the eye point is
        // actually below the fluid surface, not merely when its block contains water.
        self.eyes_in_water = eye_fluid.kind == FluidKind::Water && eye_y <= eye_fluid_top;
    }

    fn update_swimming_state(
        &mut self,
        chunks: &crate::world::chunk::ChunkStore,
        is_passenger: bool,
    ) {
        // Entity.baseTick updates swimming after updateFluidInteraction and
        // before LocalPlayer.aiStep mutates sprinting for the current input
        // tick. Starting swimming requires the eyes and feet block to be in
        // water; once started it latches until sprinting/water ends. Entity
        // updateSwimming also clears the shared flag while riding.
        self.swimming = if self.flying || is_passenger {
            false
        } else if self.swimming {
            self.sprinting && self.in_water
        } else {
            let feet_state = chunks.get_block_state(
                self.position.x.floor() as i32,
                self.position.y.floor() as i32,
                self.position.z.floor() as i32,
            );
            self.sprinting && self.under_water && self.in_water && is_water_block(feet_state)
        };
    }

    /// Vanilla Entity.checkInsideBlocks: a nether portal counts as entered when
    /// the bounding box deflated by the widened float `1.0E-5f` overlaps its
    /// (full) block cell.
    pub fn is_inside_nether_portal(&self, chunks: &crate::world::chunk::ChunkStore) -> bool {
        let half_w = PLAYER_HALF_WIDTH;
        let x0 = (self.position.x - half_w + INSIDE_BLOCK_MARGIN).floor() as i32;
        let x1 = (self.position.x + half_w - INSIDE_BLOCK_MARGIN).ceil() as i32 - 1;
        let y0 = (self.position.y + INSIDE_BLOCK_MARGIN).floor() as i32;
        let y1 = (self.position.y + self.height() - INSIDE_BLOCK_MARGIN).ceil() as i32 - 1;
        let z0 = (self.position.z - half_w + INSIDE_BLOCK_MARGIN).floor() as i32;
        let z1 = (self.position.z + half_w - INSIDE_BLOCK_MARGIN).ceil() as i32 - 1;

        for bx in x0..=x1 {
            for by in y0..=y1 {
                for bz in z0..=z1 {
                    if crate::world::block::block_id(chunks.get_block_state(bx, by, bz))
                        == "nether_portal"
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Vanilla LocalPlayer.handlePortalTransitionEffect: the overlay ramps up
    /// over 80 ticks inside a portal and decays 4x as fast outside. Returns
    /// true when the rise starts from zero (vanilla plays PORTAL_TRIGGER).
    /// TODO: vanilla also force-closes portal-disallowed screens while rising.
    pub fn tick_portal_effect(&mut self, inside_portal: bool) -> bool {
        self.prev_portal_effect_intensity = self.portal_effect_intensity;
        let mut step = 0.0;
        let mut triggered = false;
        if inside_portal {
            triggered = self.portal_effect_intensity == 0.0;
            step = 0.0125;
        } else if self.portal_effect_intensity > 0.0 {
            step = -0.05;
        }
        self.portal_effect_intensity = (self.portal_effect_intensity + step).clamp(0.0, 1.0);
        triggered
    }

    /// Vanilla LivingEntity.isSleeping: getSleepingPos().isPresent().
    pub fn is_sleeping(&self) -> bool {
        self.sleeping_pos.is_some()
    }

    /// Vanilla Player.tick sleep-counter branch: ramps to 100 while sleeping,
    /// then runs 100..110 after waking and resets to 0.
    pub fn tick_sleep(&mut self) {
        if self.is_sleeping() {
            self.sleep_counter = (self.sleep_counter + 1).min(100);
        } else if self.sleep_counter > 0 {
            self.sleep_counter += 1;
            if self.sleep_counter >= 110 {
                self.sleep_counter = 0;
            }
        }
    }

    /// Vanilla client stopSleepInBed(false, _): the counter jumps to 100 so
    /// the fade-out runs from full even on an early wake-up.
    pub fn wake_up(&mut self) {
        self.sleeping_pos = None;
        self.sleep_counter = 100;
    }
}

#[cfg(test)]
mod tests {
    use azalea_core::position::ChunkPos;
    use azalea_world::chunk::Chunk;

    use super::*;

    fn flow_test_store() -> crate::world::chunk::ChunkStore {
        crate::world::block::init("26.2");
        let mut chunks = crate::world::chunk::ChunkStore::new(2);
        chunks.partial_storage.set(
            &ChunkPos::new(0, 0),
            Some(Chunk::default()),
            &mut chunks.chunk_storage,
        );
        chunks
    }

    #[test]
    fn local_player_is_removed_at_vanilla_death_tick() {
        let mut player = LocalPlayer::new();
        player.death_time = 18;

        player.tick_death();
        assert_eq!(player.death_time, 19);
        assert!(
            !player.death_animation_finished(),
            "vanilla keeps the local player renderable through death tick 19"
        );

        player.tick_death();
        assert_eq!(player.death_time, 20);
        assert!(
            player.death_animation_finished(),
            "vanilla removes LocalPlayer exactly when deathTime reaches 20"
        );

        player.tick_death();
        assert_eq!(
            player.death_time, 20,
            "removed local players no longer advance their death clock"
        );
    }

    #[test]
    fn death_respawn_resets_local_corpse_state() {
        let mut player = LocalPlayer::new();
        player.death_time = 20;
        player.max_health = 40.0;
        player.health = 0.0;
        player.score = 42;
        player.experience_level = 17;
        player.experience_progress = 0.75;
        player.inventory.set_slot(
            crate::player::inventory::HOTBAR_START,
            azalea_inventory::ItemStack::Present(azalea_inventory::ItemStackData::new(
                azalea_registry::builtin::ItemKind::Stone,
                4,
            )),
        );
        player.food = 3;
        player.saturation = 0.0;
        player.velocity = Velocity::new(0.3, -0.4, 0.2);
        player.sprinting = true;
        player.crouching = true;
        player.swimming_pose = true;
        player.flying = true;
        player.eye_height = CROUCH_EYE_HEIGHT;
        player.prev_eye_height = CROUCH_EYE_HEIGHT;
        player.walk_dist = 5.0;
        player.prev_walk_dist = 4.5;
        player.bob = 0.08;
        player.prev_bob = 0.04;
        player.in_water = true;
        player.fluid_height = 0.8;
        player.eyes_in_water = true;
        player.swimming = true;
        player.air_supply = 12;
        player.sleeping_pos = Some(azalea_core::position::BlockPos::new(1, 64, 1));
        player.sleep_counter = 100;

        player.reset_for_respawn(false);

        assert_eq!(player.death_time, 0);
        assert_eq!(
            player.health, 20.0,
            "fresh LocalPlayer health is initialized before kept attributes are copied"
        );
        assert_eq!(player.score, 0);
        assert_eq!(player.experience_level, 0);
        assert_eq!(player.experience_progress, 0.0);
        assert!(matches!(
            player
                .inventory
                .slot(crate::player::inventory::HOTBAR_START),
            azalea_inventory::ItemStack::Empty
        ));
        assert_eq!(player.food, 20);
        assert_eq!(player.saturation, 5.0);
        assert_eq!(player.velocity, Velocity::default());
        assert!(!player.sprinting);
        assert!(!player.crouching);
        assert!(!player.flying);
        assert_eq!(player.eye_height, STANDING_EYE_HEIGHT);
        assert_eq!(player.prev_eye_height, STANDING_EYE_HEIGHT);
        assert_eq!(player.walk_dist, 0.0);
        assert_eq!(player.prev_walk_dist, 0.0);
        assert_eq!(player.bob, 0.0);
        assert_eq!(player.prev_bob, 0.0);
        assert!(!player.in_water);
        assert_eq!(player.fluid_height, 0.0);
        assert!(!player.eyes_in_water);
        assert!(!player.swimming);
        assert!(!player.swimming_pose);
        assert_eq!(player.air_supply, MAX_AIR_SUPPLY);
        assert!(player.sleeping_pos.is_none());
        assert_eq!(player.sleep_counter, 0);
    }

    #[test]
    fn keep_entity_data_respawn_preserves_only_synced_player_state() {
        let mut player = LocalPlayer::new();
        player.health = 7.0;
        player.absorption = 3.0;
        player.score = 42;
        player.velocity = Velocity::new(0.3, -0.4, 0.2);
        player.look_dir = LookDirection::new(35.0, -12.0);
        player.sprinting = true;
        player.swimming = true;
        player.crouching = true;
        player.swimming_pose = true;
        player.air_supply = 87;
        player.sleeping_pos = Some(azalea_core::position::BlockPos::new(1, 64, 1));
        player.food = 4;
        player.experience_level = 17;
        player.experience_progress = 0.75;
        player.inventory.set_slot(
            crate::player::inventory::HOTBAR_START,
            azalea_inventory::ItemStack::Present(azalea_inventory::ItemStackData::new(
                azalea_registry::builtin::ItemKind::Stone,
                4,
            )),
        );
        player.flying = true;
        player.in_water = true;

        player.reset_for_respawn(true);

        assert_eq!(player.health, 7.0);
        assert_eq!(player.absorption, 3.0);
        assert_eq!(player.score, 42);
        assert_eq!(player.velocity, Velocity::new(0.3, -0.4, 0.2));
        assert_eq!(player.look_dir, LookDirection::new(35.0, -12.0));
        assert!(player.sprinting);
        assert!(player.swimming);
        assert!(player.crouching);
        assert!(player.swimming_pose);
        assert_eq!(player.air_supply, 87);
        assert_eq!(
            player.sleeping_pos,
            Some(azalea_core::position::BlockPos::new(1, 64, 1))
        );
        assert_eq!(player.food, 20);
        assert_eq!(player.experience_level, 0);
        assert_eq!(player.experience_progress, 0.0);
        assert!(matches!(
            player
                .inventory
                .slot(crate::player::inventory::HOTBAR_START),
            azalea_inventory::ItemStack::Empty
        ));
        assert!(!player.flying);
        assert!(!player.in_water);
    }

    #[test]
    fn dead_bob_advances_previous_state_and_decays() {
        let mut player = LocalPlayer::new();
        player.walk_dist = 3.25;
        player.prev_walk_dist = 2.75;
        player.bob = 0.1;
        player.prev_bob = 0.04;
        player.velocity = crate::entity::components::Velocity::new(0.2, 0.0, 0.0);
        player.on_ground = true;

        player.tick_bob(0.0, 0.0, true);

        assert_eq!(
            player.prev_walk_dist, 3.25,
            "dead ticks must advance the walk interpolation endpoint"
        );
        assert_eq!(
            player.walk_dist, 3.25,
            "dead ticks must not add walked distance"
        );
        assert_eq!(
            player.prev_bob, 0.1,
            "dead ticks must advance the bob interpolation endpoint"
        );
        assert!(
            (player.bob - 0.06).abs() < 1e-6,
            "vanilla dead-player bob decays 40% toward zero per tick"
        );
    }

    #[test]
    fn snapshot_render_state_clears_stale_interpolation_endpoints() {
        let mut player = LocalPlayer::new();
        player.position = dvec3(10.0, 64.0, -3.0).into();
        player.prev_position = dvec3(9.5, 63.8, -3.0).into();
        player.look_dir = LookDirection::new(35.0, -12.0);
        player.prev_look_dir = LookDirection::new(5.0, 8.0);
        player.eye_height = 1.4;
        player.prev_eye_height = 1.62;

        player.snapshot_render_state();

        assert_eq!(
            player.prev_position, player.position,
            "position interpolation must not replay a prior tick"
        );
        assert_eq!(
            player.prev_look_dir, player.look_dir,
            "look interpolation must start from the current tick state"
        );
        assert_eq!(
            player.prev_eye_height, player.eye_height,
            "eye interpolation must not reuse stale state"
        );
    }

    #[test]
    fn shallow_water_contact_uses_actual_fluid_surface_height() {
        let chunks = flow_test_store();
        let shallow = crate::world::block::find_state("water", &[("level", "7")]);
        chunks.set_block_state(8, 64, 8, shallow);

        let mut player = LocalPlayer::new();
        player.position = Position::new(8.5, 64.2, 8.5);
        player.update_water_state(&chunks, false);
        assert!(
            !player.in_water,
            "level-7 water is only 1/9 block high; feet above its surface are not in water"
        );

        player.position = Position::new(8.5, 64.05, 8.5);
        player.update_water_state(&chunks, false);
        assert!(
            player.in_water,
            "feet below the level-7 surface must still register water contact"
        );
    }

    #[test]
    fn passenger_state_clears_shared_swimming_flag() {
        let chunks = flow_test_store();
        let source = crate::world::block::find_state("water", &[("level", "0")]);
        for y in 64..=66 {
            chunks.set_block_state(8, y, 8, source);
        }

        let mut unmounted = LocalPlayer::new();
        unmounted.position = Position::new(8.5, 64.0, 8.5);
        unmounted.sprinting = true;
        unmounted.swimming = true;
        unmounted.update_water_state(&chunks, false);
        assert!(unmounted.swimming);

        let mut mounted = LocalPlayer::new();
        mounted.position = Position::new(8.5, 64.0, 8.5);
        mounted.sprinting = true;
        mounted.swimming = true;
        mounted.update_water_state(&chunks, true);
        assert!(
            !mounted.swimming,
            "Entity.updateSwimming clears the shared swimming flag while passenger"
        );
    }

    #[test]
    fn water_flow_is_zero_across_equal_source_levels() {
        let chunks = flow_test_store();
        let source = crate::world::block::find_state("water", &[("level", "0")]);
        for (x, z) in [(8, 8), (8, 7), (9, 8), (8, 9), (7, 8)] {
            chunks.set_block_state(x, 64, z, source);
        }

        let flow = water_flow_at(&chunks, 8, 64, 8, fluid(source));
        assert_eq!(flow, glam::DVec3::ZERO);
    }

    #[test]
    fn water_flow_points_toward_lower_neighbor() {
        let chunks = flow_test_store();
        let source = crate::world::block::find_state("water", &[("level", "0")]);
        let lower = crate::world::block::find_state("water", &[("level", "1")]);
        chunks.set_block_state(8, 64, 8, source);
        chunks.set_block_state(8, 64, 7, source);
        chunks.set_block_state(9, 64, 8, lower);
        chunks.set_block_state(8, 64, 9, source);
        chunks.set_block_state(7, 64, 8, source);

        let flow = water_flow_at(&chunks, 8, 64, 8, fluid(source));
        assert_eq!(flow, glam::DVec3::X);
    }

    #[test]
    fn falling_water_next_to_sturdy_wall_pulls_downward() {
        let chunks = flow_test_store();
        let falling = crate::world::block::find_state("water", &[("level", "8")]);
        let stone = crate::world::block::find_state("stone", &[]);
        chunks.set_block_state(8, 64, 8, falling);
        chunks.set_block_state(9, 64, 8, stone);

        let flow = water_flow_at(&chunks, 8, 64, 8, fluid(falling));
        assert_eq!(flow, glam::DVec3::NEG_Y);
    }

    #[test]
    fn local_shared_flags_can_clear_swimming_without_clearing_sprint() {
        let mut player = LocalPlayer::new();
        player.sprinting = true;
        player.swimming = true;

        player.sync_shared_flags(0x08);

        assert!(
            player.sprinting,
            "server shared flag bit 3 keeps sprinting set"
        );
        assert!(
            !player.swimming,
            "server shared flag bit 4 must clear the stale local swim latch"
        );
    }

    #[test]
    fn swimming_pose_uses_vanilla_dimensions_and_eye_height() {
        let mut player = LocalPlayer::new();
        player.position = Position::new(0.5, 64.0, 0.5);
        player.swimming_pose = true;
        player.crouching = false;

        assert_eq!(player.height(), SWIMMING_HEIGHT);
        assert_eq!(
            player.bounding_box().max.y - player.bounding_box().min.y,
            SWIMMING_HEIGHT
        );
        assert_eq!(player.target_eye_height(), SWIMMING_EYE_HEIGHT);
    }

    #[test]
    fn bounding_box_tracks_current_crouching_pose() {
        let mut player = LocalPlayer::new();
        player.position = Position::new(0.5, 0.0, 0.5);
        let ceiling = Aabb::new(dvec3(0.0, 1.6, 0.0), dvec3(1.0, 2.0, 1.0));

        player.crouching = false;
        assert!(player.bounding_box().intersects(&ceiling));

        player.crouching = true;
        assert!(!player.bounding_box().intersects(&ceiling));
        assert_eq!(player.bounding_box().max.y, CROUCH_HEIGHT);
    }

    #[test]
    fn hurt_state_matches_vanilla_duration_direction_and_expiry() {
        let mut player = LocalPlayer::new();

        player.apply_server_health(16.0);
        assert_eq!(
            player.hurt_time, 0,
            "first server health sync should not trigger hurt feedback"
        );
        player.apply_server_health(18.0);
        assert_eq!(
            player.hurt_time, 0,
            "health increases should not trigger hurt feedback"
        );
        player.apply_server_health(17.0);
        assert_eq!(
            player.hurt_time, HURT_DURATION,
            "later health decreases should trigger hurt feedback"
        );
        player.hurt_time = 0;

        player.mark_hurt();
        assert_eq!(
            player.hurt_time, HURT_DURATION,
            "damage should start the full hurt timer"
        );
        assert_eq!(
            player.hurt_dir, 0.0,
            "damage events alone must not invent a direction"
        );

        player.animate_hurt(-37.5);
        assert_eq!(
            player.hurt_time, HURT_DURATION,
            "hurt animation should refresh the timer"
        );
        assert_eq!(
            player.hurt_dir, -37.5,
            "hurt animation yaw should be preserved exactly"
        );

        for remaining in (0..HURT_DURATION).rev() {
            player.tick_hurt();
            assert_eq!(
                player.hurt_time, remaining,
                "hurt timer should decrement once per client tick"
            );
        }
        player.tick_hurt();
        assert_eq!(player.hurt_time, 0, "expired hurt timer must not underflow");
    }

    #[test]
    fn hurt_state_is_scoped_to_one_life() {
        let mut player = LocalPlayer::new();
        player.apply_server_health(20.0);
        player.animate_hurt(-37.5);

        player.reset_hurt_state();
        assert_eq!(
            player.hurt_time, 0,
            "a new life should clear the hurt timer"
        );
        assert_eq!(
            player.hurt_dir, 0.0,
            "a new life should clear the hurt direction"
        );

        player.apply_server_health(6.0);
        assert_eq!(
            player.hurt_time, 0,
            "the first health sync of a new life should not trigger hurt feedback"
        );
        player.apply_server_health(5.0);
        assert_eq!(
            player.hurt_time, HURT_DURATION,
            "later decreases in the new life should trigger hurt feedback"
        );
    }
}
