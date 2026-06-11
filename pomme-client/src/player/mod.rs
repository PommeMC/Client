pub mod interaction;
pub mod inventory;
pub mod tab_list;

use glam::dvec3;
use inventory::Inventory;

use crate::entity::components::{LookDirection, Position, Velocity};

pub const MAX_AIR_SUPPLY: i32 = 300;
pub const STANDING_HEIGHT: f64 = 1.8;
pub const CROUCH_HEIGHT: f64 = 1.5;
const STANDING_EYE_HEIGHT: f64 = 1.62;
const CROUCH_EYE_HEIGHT: f64 = 1.27;
const DROWN_DAMAGE_THRESHOLD: i32 = -20;
const DROWN_DAMAGE: f32 = 2.0;
const AIR_RECOVERY_RATE: i32 = 4;

fn is_water_block(state: azalea_block::BlockState) -> bool {
    if state.is_air() {
        return false;
    }
    let block: Box<dyn azalea_block::BlockTrait> = state.into();
    let id = block.id();
    if id == "water" || id == "bubble_column" {
        return true;
    }
    block
        .property_map()
        .get("waterlogged")
        .is_some_and(|v| *v == "true")
}

pub struct LocalPlayer {
    pub position: Position,
    pub prev_position: Position,
    pub velocity: Velocity,
    pub look_dir: LookDirection,
    pub prev_look_dir: LookDirection,
    pub on_ground: bool,
    pub health: f32,
    pub food: u32,
    pub armor: u32,
    pub saturation: f32,
    pub inventory: Inventory,
    pub sprinting: bool,
    pub crouching: bool,
    pub eye_height: f64,
    pub prev_eye_height: f64,
    pub horizontal_collision: bool,
    pub sprint_toggle_timer: u32,
    pub was_forward_pressed: bool,
    pub in_water: bool,
    pub eyes_in_water: bool,
    pub swimming: bool,
    pub air_supply: i32,
    pub game_mode: u8,
    pub score: i32,
    pub entity_id: i32,
    pub experience_level: i32,
    pub experience_progress: f32,
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
            food: 20,
            armor: 0,
            saturation: 5.0,
            inventory: Inventory::new(),
            sprinting: false,
            crouching: false,
            eye_height: STANDING_EYE_HEIGHT,
            prev_eye_height: STANDING_EYE_HEIGHT,
            horizontal_collision: false,
            sprint_toggle_timer: 0,
            was_forward_pressed: false,
            in_water: false,
            eyes_in_water: false,
            swimming: false,
            air_supply: MAX_AIR_SUPPLY,
            game_mode: 0,
            score: 0,
            entity_id: -1,
            experience_level: 0,
            experience_progress: 0.0,
        }
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
        let half_w = 0.3;
        let height = self.height();
        let eye_height = self.target_eye_height();

        let min_x = (self.position.x - half_w).floor() as i32;
        let max_x = (self.position.x + half_w).floor() as i32;
        let min_y = self.position.y.floor() as i32;
        let max_y = (self.position.y + height).floor() as i32;
        let min_z = (self.position.z - half_w).floor() as i32;
        let max_z = (self.position.z + half_w).floor() as i32;

        let mut touching_water = false;
        'water_check: for bx in min_x..=max_x {
            for by in min_y..=max_y {
                for bz in min_z..=max_z {
                    if is_water_block(chunks.get_block_state(bx, by, bz)) {
                        touching_water = true;
                        break 'water_check;
                    }
                }
            }
        }

        let eye_y = (self.position.y + eye_height).floor() as i32;
        let eye_x = self.position.x.floor() as i32;
        let eye_z = self.position.z.floor() as i32;

        self.in_water = touching_water;
        self.eyes_in_water = is_water_block(chunks.get_block_state(eye_x, eye_y, eye_z));
        self.swimming = self.sprinting && self.in_water && self.eyes_in_water;
    }
}
