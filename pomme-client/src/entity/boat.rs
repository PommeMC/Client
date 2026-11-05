//! Vanilla 26.2 `AbstractBoat` tick, decoded from the unobfuscated jar. The
//! client is the authoritative instance for the boat it steers (boat physics
//! run locally and `MoveVehicle` syncs the server, like a vanilla client).

use crate::entity::{LivingEntity, is_boat};
use crate::physics::aabb::Aabb;
use crate::physics::block_shape::{CollisionContext, collision_shape};
use crate::physics::collision::resolve_collision;
use crate::world::block::{BlockStateTable, FluidKind};
use crate::world::chunk::ChunkStore;

/// Vanilla `AbstractBoat.getDefaultGravity`.
const BOAT_GRAVITY: f64 = 0.04;
/// Vanilla `AbstractBoat.PADDLE_SPEED`: each stroking paddle advances π/8
/// radians per tick.
const PADDLE_SPEED: f32 = std::f32::consts::FRAC_PI_8;

/// Vanilla `AbstractBoat$Status`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum BoatStatus {
    #[default]
    InAir,
    OnLand,
    InWater,
    UnderWater,
    UnderFlowingWater,
}

/// Per-boat control state, an owned field of every `LivingEntity` that is a
/// `*_boat`/`*_raft` kind.
#[derive(Clone, Debug)]
pub struct BoatSim {
    pub input_left: bool,
    pub input_right: bool,
    pub input_up: bool,
    pub input_down: bool,
    pub delta_rotation: f32,
    pub out_of_control_ticks: f32,
    pub land_friction: f32,
    pub status: BoatStatus,
    pub old_status: BoatStatus,
    /// Vanilla `waterLevel`: the highest water surface overlapping the hull,
    /// or the hull top while submerged.
    pub water_level: f64,
    /// Vanilla `lastYd`, kept from the splash-down snap.
    pub last_yd: f64,
    /// Vanilla `paddlePositions`, radians.
    pub paddle_positions: [f32; 2],
    /// Vanilla `getPaddleState(i)`, synced remotely via boat metadata.
    pub paddle_state: [bool; 2],
}

impl BoatSim {
    pub const fn new() -> Self {
        Self {
            input_left: false,
            input_right: false,
            input_up: false,
            input_down: false,
            delta_rotation: 0.0,
            out_of_control_ticks: 0.0,
            land_friction: 0.0,
            status: BoatStatus::InAir,
            old_status: BoatStatus::InAir,
            water_level: 0.0,
            last_yd: 0.0,
            paddle_positions: [0.0; 2],
            paddle_state: [false; 2],
        }
    }

    /// Vanilla `getRowingTime(paddle, partialTick)`: the renderer feeds this
    /// straight into `animatePaddle`.
    pub fn rowing_time(&self, paddle: usize, partial_tick: f32) -> f32 {
        if self.paddle_state[paddle] {
            let pos = self.paddle_positions[paddle];
            clamped_lerp(pos - PADDLE_SPEED, pos, partial_tick)
        } else {
            0.0
        }
    }
}

impl Default for BoatSim {
    fn default() -> Self {
        Self::new()
    }
}

/// Vanilla `Mth.clampedLerp`.
pub fn clamped_lerp(start: f32, end: f32, t: f32) -> f32 {
    if t <= 0.0 {
        start
    } else if t >= 1.0 {
        end
    } else {
        start + t * (end - start)
    }
}

/// The `(kind, height, is_source)` of the block position's fluid, through
/// vanilla `FluidState.getHeight`: the same fluid above makes the column read
/// full height, otherwise the state's own `amount / 9`.
fn fluid_of(
    chunk_store: &ChunkStore,
    table: BlockStateTable,
    x: i32,
    y: i32,
    z: i32,
) -> Option<(FluidKind, f32, bool)> {
    let (kind, height, source) = fluid_state(chunk_store, table, x, y, z)?;
    let full = fluid_state(chunk_store, table, x, y + 1, z)
        .is_some_and(|(above_kind, _, above_source)| above_kind == kind && above_source == source);
    Some((kind, if full { 1.0 } else { height }, source))
}

fn fluid_state(
    chunk_store: &ChunkStore,
    table: BlockStateTable,
    x: i32,
    y: i32,
    z: i32,
) -> Option<(FluidKind, f32, bool)> {
    let state = chunk_store.get_block_state(x, y, z);
    let fluid = table.block_state_data(state).fluid();
    (fluid.kind != FluidKind::Empty).then(|| {
        (
            fluid.kind,
            fluid.height(),
            fluid.amount == 8 && !fluid.falling,
        )
    })
}

/// The boat's collision box: full-width square, `height` tall from the feet.
pub fn boat_aabb(boat: &LivingEntity) -> Aabb {
    let dims = crate::entity::dimensions::EntityDimensions::of(boat.entity_type);
    let half = dims.width as f64 * 0.5;
    Aabb::new(
        glam::dvec3(
            boat.position.x - half,
            boat.position.y,
            boat.position.z - half,
        ),
        glam::dvec3(
            boat.position.x + half,
            boat.position.y + dims.height as f64,
            boat.position.z + half,
        ),
    )
}

/// Vanilla `AbstractBoat.isUnderwater`: any water in the slab just above the
/// hull top (`floor(maxY) .. ceil(maxY + 0.001)`). A non-source fluid there
/// wins immediately (`UnderFlowingWater`); any source yields `UnderWater`.
fn underwater_status(
    boat: &LivingEntity,
    chunk_store: &ChunkStore,
    table: BlockStateTable,
) -> Option<BoatStatus> {
    let aabb = boat_aabb(boat);
    let top = aabb.max.y + 0.001;
    let x0 = aabb.min.x.floor() as i32;
    let x1 = aabb.max.x.ceil() as i32;
    let y0 = aabb.max.y.floor() as i32;
    let y1 = top.ceil() as i32;
    let z0 = aabb.min.z.floor() as i32;
    let z1 = aabb.max.z.ceil() as i32;
    let mut source_found = false;
    for x in x0..x1 {
        for z in z0..z1 {
            for y in y0..y1 {
                let Some((kind, height, is_source)) = fluid_of(chunk_store, table, x, y, z) else {
                    continue;
                };
                if kind != FluidKind::Water {
                    continue;
                }
                if top < (y as f32 + height) as f64 {
                    if is_source {
                        source_found = true;
                    } else {
                        return Some(BoatStatus::UnderFlowingWater);
                    }
                }
            }
        }
    }
    source_found.then_some(BoatStatus::UnderWater)
}

/// Vanilla `checkInWater`: a one-block-high strip at the hull bottom
/// (`floor(minY) .. ceil(minY + 0.001)`). Returns whether any water crosses
/// the bottom face, plus the highest water surface found (`waterLevel`).
fn check_in_water(
    boat: &LivingEntity,
    chunk_store: &ChunkStore,
    table: BlockStateTable,
) -> (bool, f64) {
    let aabb = boat_aabb(boat);
    let mut water_level = f64::NEG_INFINITY;
    let mut found = false;
    let x0 = aabb.min.x.floor() as i32;
    let x1 = aabb.max.x.ceil() as i32;
    let y0 = aabb.min.y.floor() as i32;
    let y1 = (aabb.min.y + 0.001).ceil() as i32;
    let z0 = aabb.min.z.floor() as i32;
    let z1 = aabb.max.z.ceil() as i32;
    for x in x0..x1 {
        for z in z0..z1 {
            for y in y0..y1 {
                let Some((kind, height, _)) = fluid_of(chunk_store, table, x, y, z) else {
                    continue;
                };
                if kind != FluidKind::Water {
                    continue;
                }
                let surface = y as f64 + height as f64;
                water_level = water_level.max(surface);
                found = found || (aabb.min.y < surface);
            }
        }
    }
    (found, water_level)
}

/// Vanilla `getGroundFriction`: the average `movement_friction` of every
/// colliding block around the one-block strip just under the hull (the strip
/// expanded by one cell). Lily pads are excluded, as are cells that can only
/// touch the hull from outside the region: two borders, or one border on the
/// region's end layers.
fn ground_friction(boat: &LivingEntity, chunk_store: &ChunkStore, table: BlockStateTable) -> f32 {
    let hull = boat_aabb(boat);
    let hull_bottom = Aabb::new(
        glam::dvec3(hull.min.x, hull.min.y - 0.001, hull.min.z),
        glam::dvec3(hull.max.x, hull.min.y, hull.max.z),
    );
    // `AABB(minX, minY - 0.001, minZ, maxX, minY, maxZ)` expanded by one in
    // every axis drives the loop bounds.
    let x0 = (hull.min.x - 1.0).floor() as i32;
    let x1 = (hull.max.x + 1.0).ceil() as i32;
    let y0 = (hull.min.y - 1.001).floor() as i32;
    let y1 = (hull.min.y + 1.0).ceil() as i32;
    let z0 = (hull.min.z - 1.0).floor() as i32;
    let z1 = (hull.max.z + 1.0).ceil() as i32;
    let mut sum = 0.0f32;
    let mut count = 0u32;
    for x in x0..x1 {
        for z in z0..z1 {
            let border_x = (x == x0) as u8 + (x == x1 - 1) as u8;
            let border_z = (z == z0) as u8 + (z == z1 - 1) as u8;
            for y in y0..y1 {
                let border_y = (y == y0) as u8 + (y == y1 - 1) as u8;
                if border_x + border_z >= 2 || (border_x + border_z >= 1 && border_y >= 1) {
                    continue;
                }
                let state = chunk_store.get_block_state(x, y, z);
                let data = table.block_state_data(state);
                if data.id() == "lily_pad" {
                    continue;
                }
                static FULL_CUBE: &[crate::physics::block_shape::LocalBox] =
                    &[[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]];
                let shape = match collision_shape(data, y, &CollisionContext::EMPTY) {
                    Some(s) => s,
                    None => FULL_CUBE,
                };
                if shape.is_empty() {
                    continue;
                }
                for &box_local in shape {
                    let box_aabb =
                        Aabb::from_local(box_local, glam::dvec3(x as f64, y as f64, z as f64));
                    if box_aabb.intersects(&hull_bottom) {
                        sum += data.movement_friction();
                        count += 1;
                        break;
                    }
                }
            }
        }
    }
    if count > 0 { sum / count as f32 } else { 0.0 }
}

/// Vanilla `getStatus`: submerged raises the water level to the hull top, the
/// bottom strip's water sets `waterLevel`, and the ground friction result is
/// stored on the side (`getStatus` keeps `landFriction` fresh per tick).
/// The side effects come back as the `(water_level, land_friction)` payload.
fn boat_status(
    boat: &LivingEntity,
    chunk_store: &ChunkStore,
    table: BlockStateTable,
) -> (BoatStatus, Option<f64>, Option<f32>) {
    if let Some(status) = underwater_status(boat, chunk_store, table) {
        return (status, Some(boat_aabb(boat).max.y), None);
    }
    let (in_water, water_level) = check_in_water(boat, chunk_store, table);
    if in_water {
        return (BoatStatus::InWater, Some(water_level), None);
    }
    let friction = ground_friction(boat, chunk_store, table);
    if friction > 0.0 {
        return (BoatStatus::OnLand, None, Some(friction));
    }
    (BoatStatus::InAir, None, None)
}

/// Vanilla `getWaterLevelAbove`: sample the layers the hull swept through on
/// its last descent (`maxY - lastYd`), and return the first with no full
/// block of water.
fn water_level_above(boat: &LivingEntity, chunk_store: &ChunkStore, table: BlockStateTable) -> f64 {
    let aabb = boat_aabb(boat);
    let x0 = aabb.min.x.floor() as i32;
    let x1 = aabb.max.x.ceil() as i32;
    let z0 = aabb.min.z.floor() as i32;
    let z1 = aabb.max.z.ceil() as i32;
    let y0 = aabb.max.y.floor() as i32;
    let y1 = (aabb.max.y - boat.boat_sim.last_yd).ceil() as i32;
    for y in y0..y1 {
        let mut level = 0.0f32;
        for x in x0..x1 {
            for z in z0..z1 {
                let Some((kind, height, _)) = fluid_of(chunk_store, table, x, y, z) else {
                    continue;
                };
                if kind == FluidKind::Water {
                    level = level.max(height);
                }
            }
        }
        if level < 1.0 {
            return y as f64 + level as f64;
        }
    }
    y1 as f64 + 1.0
}

/// Vanilla `floatBoat`: per-status gravity/spring/drag. On the splash-down
/// path (`oldStatus == IN_AIR`, new one submerged) the boat instead snaps
/// onto the water surface, resetting its velocity.
fn float_boat(
    boat: &mut LivingEntity,
    sim: &mut BoatSim,
    chunk_store: &ChunkStore,
    table: BlockStateTable,
    controlling_player: bool,
) {
    let dims = crate::entity::dimensions::EntityDimensions::of(boat.entity_type);
    let bb_height = dims.height as f64;
    let mut gravity_delta = -BOAT_GRAVITY;
    let mut spring = 0.0f64;
    // The initial `0.05` only applies to statusless ticks; every branch below
    // assigns its own drag before the velocity scale.
    let drag;

    if sim.old_status == BoatStatus::InAir
        && sim.status != BoatStatus::InAir
        && sim.status != BoatStatus::OnLand
    {
        sim.water_level = boat.position.y + bb_height;
        // `f = (float)(wla - bbHeight) + 0.101`; the hull snaps there when free.
        let water_level =
            (water_level_above(boat, chunk_store, table) as f32 - dims.height) as f64 + 0.101;
        let aabb = boat_aabb(boat).offset(glam::dvec3(0.0, water_level - boat.position.y, 0.0));
        let world = crate::physics::collision::CollisionWorld::new(chunk_store, table);
        if crate::physics::collision::no_collision(
            world,
            &aabb,
            &CollisionContext::position(boat.position.y),
        ) {
            boat.position.y = water_level;
            // `setDeltaMovement(multiply(1.0, 0.0, 1.0))`.
            boat.velocity.y = 0.0;
            sim.last_yd = 0.0;
        }
        sim.status = BoatStatus::InWater;
        return;
    }

    match sim.status {
        BoatStatus::InWater => {
            spring = (sim.water_level - boat.position.y) / bb_height;
            drag = 0.9;
        }
        BoatStatus::UnderFlowingWater => {
            gravity_delta = -0.0007;
            drag = 0.9;
        }
        BoatStatus::UnderWater => {
            // Vanilla's float 0.01 widened to double.
            spring = 0.01_f32 as f64;
            drag = 0.45;
        }
        BoatStatus::InAir => {
            drag = 0.9;
        }
        BoatStatus::OnLand => {
            drag = sim.land_friction;
            // A controlling player halves the stored friction every tick.
            if controlling_player {
                sim.land_friction /= 2.0;
            }
        }
    }

    boat.velocity.x *= drag as f64;
    boat.velocity.y += gravity_delta;
    boat.velocity.z *= drag as f64;
    sim.delta_rotation *= drag;
    if spring > 0.0 {
        boat.velocity.y = (boat.velocity.y + spring * 0.04 / 0.65) * 0.75;
    }
}

/// Vanilla `controlBoat`: delta rotation from left/right keystrokes, thrust
/// from the stroke combinations, and the paddle stroke states.
fn control_boat(boat: &mut LivingEntity, sim: &mut BoatSim) {
    let mut control = 0.0f32;
    if sim.input_left {
        sim.delta_rotation -= 1.0;
    }
    if sim.input_right {
        sim.delta_rotation += 1.0;
    }
    if (sim.input_right != sim.input_left) && !sim.input_up && !sim.input_down {
        control += 0.005;
    }
    // The hull turns on the delta the previous tick left behind, before this
    // tick's keystrokes add to it.
    let yaw = boat.body_y_rot_deg + sim.delta_rotation;
    boat.body_y_rot_deg = yaw;
    boat.head_y_rot_deg = yaw;
    boat.look_dir = crate::entity::components::LookDirection::new(yaw, boat.look_dir.x_rot_deg());
    if sim.input_up {
        control += 0.04;
    }
    if sim.input_down {
        control -= 0.005;
    }
    // `velocity += (-sin(yaw) * control, 0, cos(yaw) * control)`.
    let yaw_rad = (yaw as f64).to_radians();
    boat.velocity.x -= yaw_rad.sin() * control as f64;
    boat.velocity.z += yaw_rad.cos() * control as f64;
    sim.paddle_state[0] = (sim.input_right && !sim.input_left) || sim.input_up;
    sim.paddle_state[1] = (sim.input_left && !sim.input_right) || sim.input_up;
}

/// Vanilla's paddle clock from `tick`, which every client runs on every boat:
/// a stroking paddle advances π/8 radians wrapped to one turn, an idle one
/// resets its phase.
pub fn tick_paddles(sim: &mut BoatSim) {
    for i in 0..2 {
        if sim.paddle_state[i] {
            sim.paddle_positions[i] =
                (sim.paddle_positions[i] + PADDLE_SPEED) % std::f32::consts::TAU;
        } else {
            sim.paddle_positions[i] = 0.0;
        }
    }
}

/// One client tick for a boat the local player steers (vanilla `tick` on the
/// authoritative client instance): statuses, `floatBoat`, `controlBoat`, the
/// collision move, then the paddle clock. The steering inputs live in
/// `boat.boat_sim`; callers must follow with the `MoveVehicle` sync.
pub fn tick_controlled_boat(
    boat: &mut LivingEntity,
    chunk_store: &ChunkStore,
    table: BlockStateTable,
) {
    if !is_boat(boat.entity_type) {
        return;
    }

    boat.prev_position = boat.position;
    boat.prev_body_y_rot_deg = boat.body_y_rot_deg;
    boat.prev_head_y_rot_deg = boat.head_y_rot_deg;
    boat.prev_look_dir = boat.look_dir;

    let mut sim = std::mem::take(&mut boat.boat_sim);
    sim.old_status = sim.status;
    let (status, water_level, ground_friction) = boat_status(&*boat, chunk_store, table);
    sim.status = status;
    if let Some(water_level) = water_level {
        sim.water_level = water_level;
    }
    if let Some(friction) = ground_friction {
        sim.land_friction = friction;
    }
    if matches!(
        status,
        BoatStatus::UnderWater | BoatStatus::UnderFlowingWater
    ) {
        sim.out_of_control_ticks += 1.0;
    } else {
        sim.out_of_control_ticks = 0.0;
    }

    // 2. Buoyancy.
    float_boat(boat, &mut sim, chunk_store, table, true);

    // 3. Steering.
    control_boat(boat, &mut sim);

    // 4. Vanilla `move(MoverType.SELF, velocity)`.
    let aabb = boat_aabb(boat);
    let world = crate::physics::collision::CollisionWorld::new(chunk_store, table);
    let ctx = CollisionContext::position(boat.position.y);
    let (movement_step, on_ground) =
        resolve_collision(world, aabb, boat.velocity.into(), 0.0, boat.on_ground, &ctx);
    boat.position.x += movement_step.x;
    boat.position.y += movement_step.y;
    boat.position.z += movement_step.z;
    boat.on_ground = on_ground;
    // Vanilla records lastYd inside `move` before restitution may zero y.
    sim.last_yd = boat.velocity.y;
    if on_ground {
        boat.velocity.y = 0.0;
    }

    // 5. Paddle animation clock.
    tick_paddles(&mut sim);
    boat.boat_sim = sim;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::EntityKind;
    use crate::entity::components::{LookDirection, Position};
    use crate::world::chunk::ChunkStore;

    fn pool() -> (ChunkStore, BlockStateTable) {
        let table = BlockStateTable::native();
        let store = ChunkStore::with_origin_chunk();
        for x in -8..24 {
            for z in -8..24 {
                store.set_block_state(x, 59, z, table.find_state("stone", &[]), table);
                store.set_block_state(x, 60, z, table.find_state("water", &[]), table);
                store.set_block_state(x, 61, z, table.find_state("water", &[]), table);
            }
        }
        (store, table)
    }

    #[test]
    fn boat_drives_forward_in_water() {
        let (store, table) = pool();
        let mut boat = LivingEntity::new(
            EntityKind::OakBoat,
            Position::new(0.5, 60.0, 0.5),
            LookDirection::new(0.0, 0.0),
            uuid::Uuid::from_u128(1),
        );
        for _ in 0..40 {
            tick_controlled_boat(&mut boat, &store, table);
        }
        assert!(
            (61.0..62.0).contains(&boat.position.y),
            "boat failed to float at the column surface: y={}",
            boat.position.y
        );
        boat.boat_sim.input_up = true;
        for _ in 0..20 {
            tick_controlled_boat(&mut boat, &store, table);
            assert_eq!(
                boat.boat_sim.status,
                BoatStatus::InWater,
                "status flipped while floating"
            );
            assert!(
                boat.position.y > 61.0,
                "boat sank below the surface: y={}",
                boat.position.y
            );
        }
        assert!(
            boat.position.z > 4.0,
            "boat barely moved: z={}",
            boat.position.z
        );
        assert!(
            boat.body_y_rot_deg.abs() < 1.0,
            "yaw drifted to {}",
            boat.body_y_rot_deg
        );
    }

    #[test]
    fn boat_does_not_spin_without_turning() {
        let (store, table) = pool();
        let mut boat = LivingEntity::new(
            EntityKind::OakBoat,
            Position::new(0.5, 61.0, 0.5),
            LookDirection::new(0.0, 0.0),
            uuid::Uuid::from_u128(1),
        );
        boat.boat_sim.input_up = true;
        for _ in 0..60 {
            tick_controlled_boat(&mut boat, &store, table);
        }
        assert!(
            boat.body_y_rot_deg.abs() < 1.0,
            "yaw drifted to {}",
            boat.body_y_rot_deg
        );
        assert!(
            boat.position.z > 5.0,
            "boat barely moved: z={}",
            boat.position.z
        );
    }

    #[test]
    fn boat_status_and_speed_on_land_and_ice() {
        let table = BlockStateTable::native();
        let store = ChunkStore::with_origin_chunk();
        for x in -8..24 {
            for z in -8..24 {
                store.set_block_state(x, 59, z, table.find_state("stone", &[]), table);
            }
        }
        let mut boat = LivingEntity::new(
            EntityKind::OakBoat,
            Position::new(0.5, 60.0, 0.5),
            LookDirection::new(0.0, 0.0),
            uuid::Uuid::from_u128(1),
        );
        boat.boat_sim.input_up = true;
        for _ in 0..20 {
            tick_controlled_boat(&mut boat, &store, table);
            assert_eq!(boat.boat_sim.status, BoatStatus::OnLand);
        }
        let land_z = boat.position.z;

        let store_ice = ChunkStore::with_origin_chunk();
        for x in -8..24 {
            for z in -8..24 {
                store_ice.set_block_state(x, 59, z, table.find_state("ice", &[]), table);
            }
        }
        let mut boat_ice = LivingEntity::new(
            EntityKind::OakBoat,
            Position::new(0.5, 60.0, 0.5),
            LookDirection::new(0.0, 0.0),
            uuid::Uuid::from_u128(1),
        );
        boat_ice.boat_sim.input_up = true;
        for _ in 0..20 {
            tick_controlled_boat(&mut boat_ice, &store_ice, table);
            assert_eq!(boat_ice.boat_sim.status, BoatStatus::OnLand);
        }
        assert!(boat_ice.position.z > land_z * 3.0);
    }
}
