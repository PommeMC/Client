pub mod interaction;
pub mod inventory;
pub mod menu_click;
pub mod tab_list;

use glam::{dvec2, dvec3};
use inventory::Inventory;

use crate::entity::HURT_DURATION;
use crate::entity::components::{LookDirection, Position, Velocity};
use crate::world::block::{FluidKind, fluid};

pub const MAX_AIR_SUPPLY: i32 = 300;
pub const STANDING_HEIGHT: f64 = 1.8;
pub const CROUCH_HEIGHT: f64 = 1.5;
pub const STANDING_EYE_HEIGHT: f64 = 1.62;
pub const CROUCH_EYE_HEIGHT: f64 = 1.27;
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

pub struct LocalPlayer {
    pub position: Position,
    pub prev_position: Position,
    pub velocity: Velocity,
    pub look_dir: LookDirection,
    pub prev_look_dir: LookDirection,
    pub on_ground: bool,
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
    pub crouching: bool,
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
    pub eye_height: f64,
    pub prev_eye_height: f64,
    pub walk_dist: f32,
    pub prev_walk_dist: f32,
    pub bob: f32,
    pub prev_bob: f32,
    pub horizontal_collision: bool,
    pub sprint_toggle_timer: u32,
    pub was_forward_pressed: bool,
    pub in_water: bool,
    /// Vanilla `getFluidHeight(WATER)`: water surface height above the feet.
    pub fluid_height: f64,
    pub eyes_in_water: bool,
    pub swimming: bool,
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
            sprint_toggle_timer: 0,
            was_forward_pressed: false,
            in_water: false,
            fluid_height: 0.0,
            eyes_in_water: false,
            swimming: false,
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
        let kept_crouching = self.crouching;
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
        self.sprint_toggle_timer = 0;
        self.was_forward_pressed = false;
        self.in_water = false;
        self.fluid_height = 0.0;
        self.eyes_in_water = false;
        self.swimming = false;
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
            self.crouching = kept_crouching;
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
        if self.crouching {
            CROUCH_HEIGHT
        } else {
            STANDING_HEIGHT
        }
    }

    pub fn target_eye_height(&self) -> f64 {
        if self.crouching {
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
        self.prev_position + dvec3(0.0, self.prev_eye_height, 0.0)
    }

    pub fn eye_pos(&self) -> Position {
        self.position + dvec3(0.0, self.eye_height, 0.0)
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

    pub fn update_water_state(&mut self, chunks: &crate::world::chunk::ChunkStore) {
        self.update_water_state_for_dimensions(
            chunks,
            0.3,
            self.height(),
            self.target_eye_height(),
        );
    }

    pub(crate) fn update_water_state_for_dimensions(
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
                    if fluid_top >= feet_y + MARGIN {
                        fluid_height = fluid_height.max(fluid_top - feet_y);
                    }
                }
            }
        }

        let eye_y = (self.position.y + eye_height).floor() as i32;
        let eye_x = self.position.x.floor() as i32;
        let eye_z = self.position.z.floor() as i32;

        self.fluid_height = fluid_height;
        // Vanilla `wasTouchingWater` is exactly "fluid height > 0".
        self.in_water = fluid_height > 0.0;
        self.eyes_in_water = is_water_block(chunks.get_block_state(eye_x, eye_y, eye_z));
        self.swimming = self.sprinting && self.in_water && self.eyes_in_water;
    }

    /// Vanilla Entity.checkInsideBlocks: a nether portal counts as entered when
    /// the bounding box deflated by 1.0E-5 overlaps its (full) block cell.
    pub fn is_inside_nether_portal(&self, chunks: &crate::world::chunk::ChunkStore) -> bool {
        const MARGIN: f64 = 1.0e-5;
        let half_w = 0.3;
        let x0 = (self.position.x - half_w + MARGIN).floor() as i32;
        let x1 = (self.position.x + half_w - MARGIN).ceil() as i32 - 1;
        let y0 = (self.position.y + MARGIN).floor() as i32;
        let y1 = (self.position.y + self.height() - MARGIN).ceil() as i32 - 1;
        let z0 = (self.position.z - half_w + MARGIN).floor() as i32;
        let z1 = (self.position.z + half_w - MARGIN).ceil() as i32 - 1;

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
    use super::*;

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
        player.crouching = true;
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
        assert!(player.crouching);
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
