pub mod interaction;
pub mod inventory;

use glam::Vec3;

use inventory::Inventory;

pub struct LocalPlayer {
    pub position: Vec3,
    pub velocity: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub health: f32,
    pub food: u32,
    pub saturation: f32,
    pub inventory: Inventory,
    pub sprinting: bool,
    pub horizontal_collision: bool,
    pub sprint_toggle_timer: u32,
    pub was_forward_pressed: bool,
}

impl LocalPlayer {
    pub fn new() -> Self {
        Self {
            position: Vec3::ZERO,
            velocity: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            health: 20.0,
            food: 20,
            saturation: 5.0,
            inventory: Inventory::new(),
            sprinting: false,
            horizontal_collision: false,
            sprint_toggle_timer: 0,
            was_forward_pressed: false,
        }
    }
}
