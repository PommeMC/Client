//! Vanilla `AbstractBoat` and its `Boat`, `Raft` and chest subclasses: the
//! tick every boat runs on the client, and the buoyancy, steering and move a
//! boat runs on the client of the player steering it.

use glam::{DVec3, dvec3};

use crate::entity::LivingEntity;
use crate::entity::dimensions::EntityDimensions;
use crate::physics::aabb::Aabb;
use crate::physics::block_shape::{CollisionContext, collision_shape};
use crate::physics::collision::{CollisionWorld, no_collision, resolve_collision};
use crate::physics::movement::{
    DEG_TO_RAD, check_supporting_block, collided_axes, entity_block_speed_factor, mth_cos, mth_sin,
    on_pos_of,
};
use crate::player::{is_water_block, water_contact};
use crate::position::BlockPos;
use crate::util::clamped_lerp;
use crate::world::block::{BlockStateTable, BlockTags, FluidKind};
use crate::world::chunk::ChunkStore;

/// Vanilla `AbstractBoat.getDefaultGravity`.
const BOAT_GRAVITY: f64 = 0.04;
/// Vanilla `AbstractBoat.PADDLE_SPEED`.
const PADDLE_SPEED: f32 = std::f32::consts::FRAC_PI_8;

/// Vanilla `AbstractBoat$Status`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum BoatStatus {
    InWater,
    UnderWater,
    UnderFlowingWater,
    OnLand,
    #[default]
    InAir,
}

/// The keys its controlling player holds (`AbstractBoat.setInput`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BoatInput {
    pub left: bool,
    pub right: bool,
    pub up: bool,
    pub down: bool,
}

/// A boat's own state, beside the `LivingEntity` fields Pomme keeps boats in.
#[derive(Clone, Debug, Default)]
pub struct BoatSim {
    pub input: BoatInput,
    /// Vanilla `deltaRotation`: the turn this tick, which its passengers
    /// share.
    pub delta_rotation: f32,
    out_of_control_ticks: f32,
    land_friction: f32,
    pub status: BoatStatus,
    old_status: BoatStatus,
    water_level: f64,
    last_yd: f64,
    paddle_positions: [f32; 2],
    /// `DATA_ID_PADDLE_LEFT` / `RIGHT`.
    pub paddle_state: [bool; 2],
    /// `VehicleEntity.DATA_ID_HURT`, `HURTDIR` and `DAMAGE`.
    pub hurt_time: i32,
    pub hurt_dir: i32,
    pub damage: f32,
    /// `DATA_ID_BUBBLE_TIME`.
    pub bubble_time: i32,
    bubble_multiplier: f32,
    bubble_angle: f32,
    bubble_angle_o: f32,
    /// `Entity.DATA_NO_GRAVITY`.
    pub no_gravity: bool,
    tick_count: i32,
    supporting_block: Option<BlockPos>,
    on_ground_no_blocks: bool,
}

impl BoatSim {
    /// Vanilla `getRowingTime`.
    pub fn rowing_time(&self, paddle: usize, partial_tick: f32) -> f32 {
        if self.paddle_state[paddle] {
            let pos = self.paddle_positions[paddle];
            clamped_lerp(partial_tick, pos - PADDLE_SPEED, pos)
        } else {
            0.0
        }
    }

    /// Vanilla `getBubbleAngle`.
    pub fn bubble_angle(&self, partial_tick: f32) -> f32 {
        self.bubble_angle_o + (self.bubble_angle - self.bubble_angle_o) * partial_tick
    }

    /// Vanilla `isUnderWater`.
    pub fn is_under_water(&self) -> bool {
        matches!(
            self.status,
            BoatStatus::UnderWater | BoatStatus::UnderFlowingWater
        )
    }
}

/// Vanilla `AbstractBoat.getPassengerAttachmentPoint`: `rideHeight` up and the
/// seat's offset forward, turned by the boat's yaw. `seat` is the passenger's
/// index of `passengers`.
pub fn passenger_attachment(
    boat: &LivingEntity,
    seat: usize,
    passengers: usize,
    passenger_is_animal: bool,
) -> DVec3 {
    let height = EntityDimensions::of(boat.entity_type).height;
    let ride_height = if crate::entity::is_raft(boat.entity_type) {
        height * 0.888_888_9
    } else {
        height / 3.0
    };
    let mut offset = if crate::entity::is_chest_boat(boat.entity_type) {
        0.15
    } else {
        0.0
    };
    if passengers > 1 {
        offset = if seat == 0 { 0.2 } else { -0.6 };
        if passenger_is_animal {
            offset += 0.2;
        }
    }
    crate::entity::dimensions::attachment_point([0.0, ride_height, offset], boat.body_y_rot_deg)
}

/// Vanilla `AbstractBoat.getMaxPassengers`.
pub fn max_passengers(kind: azalea_registry::builtin::EntityKind) -> usize {
    if crate::entity::is_chest_boat(kind) {
        1
    } else {
        2
    }
}

pub fn boat_aabb(boat: &LivingEntity) -> Aabb {
    EntityDimensions::of(boat.entity_type).make_bounding_box(*boat.position)
}

/// The water at a block, through `FluidState.getHeight` (the same fluid
/// above makes it a full block): its height and whether it's a source.
fn water_at(
    chunk_store: &ChunkStore,
    table: BlockStateTable,
    x: i32,
    y: i32,
    z: i32,
) -> Option<(f32, bool)> {
    let fluid = table
        .block_state_data(chunk_store.get_block_state(x, y, z))
        .fluid();
    if fluid.kind != FluidKind::Water {
        return None;
    }
    let height = if is_water_block(chunk_store.get_block_state(x, y + 1, z), table) {
        1.0
    } else {
        fluid.height()
    };
    Some((height, fluid.amount == 8 && !fluid.falling))
}

/// The block coordinates from `min`'s cell up to the one `max` reaches
/// (`Mth.floor(min)` to `Mth.ceil(max)`, exclusive).
fn cells(min: f64, max: f64) -> std::ops::Range<i32> {
    min.floor() as i32..max.ceil() as i32
}

/// Vanilla `isUnderwater`.
fn underwater_status(
    hull: &Aabb,
    chunk_store: &ChunkStore,
    table: BlockStateTable,
) -> Option<BoatStatus> {
    let max_y = hull.max.y + 0.001;
    let mut under_water = false;
    for x in cells(hull.min.x, hull.max.x) {
        for y in cells(hull.max.y, max_y) {
            for z in cells(hull.min.z, hull.max.z) {
                let Some((height, source)) = water_at(chunk_store, table, x, y, z) else {
                    continue;
                };
                if max_y >= f64::from(y as f32 + height) {
                    continue;
                }
                if !source {
                    return Some(BoatStatus::UnderFlowingWater);
                }
                under_water = true;
            }
        }
    }
    under_water.then_some(BoatStatus::UnderWater)
}

/// Vanilla `checkInWater`, which also leaves `waterLevel` at the highest
/// surface under the hull's bottom strip.
fn check_in_water(
    hull: &Aabb,
    sim: &mut BoatSim,
    chunk_store: &ChunkStore,
    table: BlockStateTable,
) -> bool {
    let mut in_water = false;
    sim.water_level = f64::MIN;
    for x in cells(hull.min.x, hull.max.x) {
        for y in cells(hull.min.y, hull.min.y + 0.001) {
            for z in cells(hull.min.z, hull.max.z) {
                let Some((height, _)) = water_at(chunk_store, table, x, y, z) else {
                    continue;
                };
                let surface = y as f32 + height;
                sim.water_level = sim.water_level.max(f64::from(surface));
                in_water |= hull.min.y < f64::from(surface);
            }
        }
    }
    in_water
}

/// Vanilla `getGroundFriction`: the average friction of the blocks the
/// strip under the hull touches, lily pads aside.
fn ground_friction(hull: &Aabb, chunk_store: &ChunkStore, table: BlockStateTable) -> f32 {
    let strip = Aabb::new(
        dvec3(hull.min.x, hull.min.y - 0.001, hull.min.z),
        dvec3(hull.max.x, hull.min.y, hull.max.z),
    );
    let x0 = strip.min.x.floor() as i32 - 1;
    let x1 = strip.max.x.ceil() as i32 + 1;
    let y0 = strip.min.y.floor() as i32 - 1;
    let y1 = strip.max.y.ceil() as i32 + 1;
    let z0 = strip.min.z.floor() as i32 - 1;
    let z1 = strip.max.z.ceil() as i32 + 1;
    let mut friction = 0.0f32;
    let mut count = 0u32;
    for x in x0..x1 {
        for z in z0..z1 {
            let edges = u8::from(x == x0 || x == x1 - 1) + u8::from(z == z0 || z == z1 - 1);
            if edges == 2 {
                continue;
            }
            for y in y0..y1 {
                if edges > 0 && (y == y0 || y == y1 - 1) {
                    continue;
                }
                let data = table.block_state_data(chunk_store.get_block_state(x, y, z));
                if data.id() == "lily_pad" {
                    continue;
                }
                let origin = dvec3(f64::from(x), f64::from(y), f64::from(z));
                // No shape listed is a full block.
                let touches = collision_shape(data, y, &CollisionContext::EMPTY)
                    .unwrap_or(&[[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]])
                    .iter()
                    .any(|&local| Aabb::from_local(local, origin).intersects(&strip));
                if touches {
                    friction += data.movement_friction();
                    count += 1;
                }
            }
        }
    }
    // Vanilla divides by zero to NaN, which no status takes.
    if count == 0 {
        0.0
    } else {
        friction / count as f32
    }
}

/// Vanilla `getStatus`.
fn status(
    hull: &Aabb,
    sim: &mut BoatSim,
    chunk_store: &ChunkStore,
    table: BlockStateTable,
) -> BoatStatus {
    if let Some(status) = underwater_status(hull, chunk_store, table) {
        sim.water_level = hull.max.y;
        return status;
    }
    if check_in_water(hull, sim, chunk_store, table) {
        return BoatStatus::InWater;
    }
    let friction = ground_friction(hull, chunk_store, table);
    if friction > 0.0 {
        sim.land_friction = friction;
        return BoatStatus::OnLand;
    }
    BoatStatus::InAir
}

/// Vanilla `getWaterLevelAbove`.
fn water_level_above(
    hull: &Aabb,
    last_yd: f64,
    chunk_store: &ChunkStore,
    table: BlockStateTable,
) -> f32 {
    let max_y = (hull.max.y - last_yd).ceil() as i32;
    for y in hull.max.y.floor() as i32..max_y {
        let mut level = 0.0f32;
        for x in cells(hull.min.x, hull.max.x) {
            for z in cells(hull.min.z, hull.max.z) {
                if let Some((height, _)) = water_at(chunk_store, table, x, y, z) {
                    level = level.max(height);
                }
            }
        }
        if level < 1.0 {
            return y as f32 + level;
        }
    }
    (max_y + 1) as f32
}

/// `Entity.commonTick`, which every client entity runs before its tick:
/// the old pose kept, then a step toward a pose the server sent. A boat's
/// yaw is its body yaw.
fn interpolate(boat: &mut LivingEntity) {
    boat.prev_position = boat.position;
    boat.prev_look_dir = boat.look_dir;
    boat.prev_body_y_rot_deg = boat.body_y_rot_deg;
    if boat.interpolate() {
        boat.body_y_rot_deg = boat.look_dir.y_rot_deg();
    }
}

/// The head of vanilla `tick`, before `super.tick()`.
fn tick_status(boat: &mut LivingEntity, chunk_store: &ChunkStore, table: BlockStateTable) {
    let hull = boat_aabb(boat);
    let sim = &mut boat.boat_sim;
    sim.old_status = sim.status;
    sim.status = status(&hull, sim, chunk_store, table);
    sim.out_of_control_ticks = if sim.is_under_water() {
        sim.out_of_control_ticks + 1.0
    } else {
        0.0
    };
    if sim.hurt_time > 0 {
        sim.hurt_time -= 1;
    }
    if sim.damage > 0.0 {
        sim.damage -= 1.0;
    }
}

/// The tail of vanilla `tick`: the client's `tickBubbleColumn`, then the
/// paddle clocks. Paddle sounds come from the server: the client's
/// `playSound(null, ...)` plays nothing.
fn tick_bubbles_and_paddles(sim: &mut BoatSim) {
    sim.bubble_multiplier = if sim.bubble_time > 0 {
        sim.bubble_multiplier + 0.05
    } else {
        sim.bubble_multiplier - 0.1
    }
    .clamp(0.0, 1.0);
    sim.bubble_angle_o = sim.bubble_angle;
    sim.bubble_angle =
        10.0 * (0.5 * f64::from(sim.tick_count)).sin() as f32 * sim.bubble_multiplier;
    for i in 0..2 {
        sim.paddle_positions[i] = if sim.paddle_state[i] {
            sim.paddle_positions[i] + PADDLE_SPEED
        } else {
            0.0
        };
    }
    sim.tick_count += 1;
}

/// Vanilla `tick` for a boat this client doesn't steer: it follows the
/// server, so its own motion and turn are dropped.
pub fn tick_client(boat: &mut LivingEntity, chunk_store: &ChunkStore, table: BlockStateTable) {
    interpolate(boat);
    tick_status(boat, chunk_store, table);
    boat.velocity = DVec3::ZERO;
    boat.boat_sim.delta_rotation = 0.0;
    tick_bubbles_and_paddles(&mut boat.boat_sim);
}

/// Vanilla `tick` for the boat the local player steers: `floatBoat`,
/// `controlBoat` and the move, on the input its rider set last tick.
/// `colliders` are the boxes of the entities it collides with
/// (`canVehicleCollide`). Returns the paddle states, for `PaddleBoat`.
// TODO: `applyEffectsFromBlocks` (bubble columns, cobwebs, fire) and the
// boat pushing other entities.
pub fn tick_controlled(
    boat: &mut LivingEntity,
    colliders: &[Aabb],
    chunk_store: &ChunkStore,
    table: BlockStateTable,
    tags: &BlockTags,
) -> [bool; 2] {
    interpolate(boat);
    tick_status(boat, chunk_store, table);

    // `Entity.updateFluidInteraction` from `super.tick()`.
    let water = water_contact(chunk_store, table, &boat_aabb(boat));
    boat.velocity += water.current_impulse(boat.velocity, false);

    let world = CollisionWorld::new(chunk_store, table).with_extra_boxes(colliders);
    float_boat(boat, world);
    control_boat(boat);
    let paddles = boat.boat_sim.paddle_state;
    move_boat(boat, world, tags);
    tick_bubbles_and_paddles(&mut boat.boat_sim);
    paddles
}

/// Vanilla `floatBoat`.
fn float_boat(boat: &mut LivingEntity, world: CollisionWorld<'_>) {
    let height = EntityDimensions::of(boat.entity_type).height;
    let hull = boat_aabb(boat);
    let sim = &mut boat.boat_sim;
    let mut vspeed = if sim.no_gravity { 0.0 } else { -BOAT_GRAVITY };
    let mut buoyancy = 0.0f64;
    if sim.old_status == BoatStatus::InAir
        && sim.status != BoatStatus::InAir
        && sim.status != BoatStatus::OnLand
    {
        sim.water_level = boat.position.y + f64::from(height);
        let target_y = f64::from(
            water_level_above(&hull, sim.last_yd, world.chunks, world.block_state_table) - height,
        ) + 0.101;
        let moved = hull.offset(dvec3(0.0, target_y - boat.position.y, 0.0));
        if no_collision(world, &moved, &CollisionContext::position(boat.position.y)) {
            boat.position.y = target_y;
            boat.velocity.y = 0.0;
            sim.last_yd = 0.0;
        }
        sim.status = BoatStatus::InWater;
        return;
    }
    let inv_friction = match sim.status {
        BoatStatus::InWater => {
            buoyancy = (sim.water_level - boat.position.y) / f64::from(height);
            0.9
        }
        BoatStatus::UnderFlowingWater => {
            vspeed = -7.0e-4;
            0.9
        }
        BoatStatus::UnderWater => {
            buoyancy = f64::from(0.01_f32);
            0.45
        }
        BoatStatus::InAir => 0.9,
        BoatStatus::OnLand => {
            let friction = sim.land_friction;
            // The controlling passenger is a player.
            sim.land_friction /= 2.0;
            friction
        }
    };
    boat.velocity.x *= f64::from(inv_friction);
    boat.velocity.y += vspeed;
    boat.velocity.z *= f64::from(inv_friction);
    sim.delta_rotation *= inv_friction;
    if buoyancy > 0.0 {
        boat.velocity.y = (boat.velocity.y + buoyancy * (BOAT_GRAVITY / 0.65)) * 0.75;
    }
}

/// Vanilla `controlBoat`.
fn control_boat(boat: &mut LivingEntity) {
    let sim = &mut boat.boat_sim;
    let BoatInput {
        left,
        right,
        up,
        down,
    } = sim.input;
    let mut acceleration = 0.0f32;
    if left {
        sim.delta_rotation -= 1.0;
    }
    if right {
        sim.delta_rotation += 1.0;
    }
    if right != left && !up && !down {
        acceleration += 0.005;
    }
    let yaw = boat.body_y_rot_deg + sim.delta_rotation;
    boat.body_y_rot_deg = yaw;
    boat.head_y_rot_deg = yaw;
    boat.look_dir = crate::entity::components::LookDirection::new(yaw, boat.look_dir.x_rot_deg());
    if up {
        acceleration += 0.04;
    }
    if down {
        acceleration -= 0.005;
    }
    let angle = yaw * DEG_TO_RAD;
    boat.velocity.x += f64::from(mth_sin(-angle) * acceleration);
    boat.velocity.z += f64::from(mth_cos(angle) * acceleration);
    sim.paddle_state = [right && !left || up, left && !right || up];
}

/// Vanilla `Entity.move(SELF, deltaMovement)` for a boat: no step-up, the
/// collided axes stopped (or bounced off slime and beds), then the block
/// speed factor.
fn move_boat(boat: &mut LivingEntity, world: CollisionWorld<'_>, tags: &BlockTags) {
    let delta = boat.velocity;
    let ctx = CollisionContext::position(boat.position.y);
    let (movement, _) = resolve_collision(
        world,
        boat_aabb(boat),
        delta.into(),
        0.0,
        boat.on_ground,
        &ctx,
    );
    let length = movement.length_squared();
    if length > 1.0e-7 || delta.length_squared() - length < 1.0e-7 {
        *boat.position += movement;
    }
    let [x_collision, vertical_collision, z_collision] = collided_axes(delta, movement);
    let below = vertical_collision && delta.y < 0.0;
    boat.on_ground = below;
    let hull = boat_aabb(boat);
    let sim = &mut boat.boat_sim;
    check_supporting_block(
        &mut sim.supporting_block,
        &mut sim.on_ground_no_blocks,
        world,
        &hull,
        below,
        movement,
        &ctx,
    );
    // `AbstractBoat.checkFallDamage`.
    sim.last_yd = boat.velocity.y;
    let position = *boat.position;
    let effect_pos = on_pos_of(position, sim.supporting_block, world, tags, 0.2);
    let moved_vertically = delta.y.abs() > 0.0;
    if moved_vertically && vertical_collision || x_collision || z_collision {
        let collided = [x_collision, vertical_collision, z_collision];
        restitute_after_collisions(boat, world, tags, effect_pos, collided, movement);
    }
    let speed_factor = f64::from(entity_block_speed_factor(
        position,
        boat.boat_sim.supporting_block,
        world,
        tags,
    ));
    boat.velocity.x *= speed_factor;
    boat.velocity.z *= speed_factor;
}

/// Vanilla `Entity.restituteMovementAfterCollisions`: a boat (not living,
/// no bounce of its own) stops on the axes it hit, and only bounces off a
/// bouncy block it lands on, at 0.8 of its restitution.
fn restitute_after_collisions(
    boat: &mut LivingEntity,
    world: CollisionWorld<'_>,
    tags: &BlockTags,
    effect_pos: BlockPos,
    [x_collision, vertical_collision, z_collision]: [bool; 3],
    movement: DVec3,
) {
    let current = boat.velocity;
    if x_collision {
        boat.velocity.x = 0.0;
    }
    if z_collision {
        boat.velocity.z = 0.0;
    }
    if !vertical_collision {
        return;
    }
    let gravity = if boat.boat_sim.no_gravity {
        0.0
    } else {
        BOAT_GRAVITY
    };
    let effect = world.block_data(effect_pos);
    let restitution = if current.y < 0.0
        && -current.y > gravity
        && !tags.contains("minecraft:suppresses_bounce", effect.id())
    {
        f64::from(effect.movement_bounce_restitution() * 0.8)
    } else {
        0.0
    };
    // A boat's air drag is 1, so the drag term drops out.
    let gravity_compensation = if restitution > 0.0 {
        movement.y / current.y * gravity
    } else {
        0.0
    };
    boat.velocity.y = (gravity_compensation - current.y) * restitution;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::EntityKind;
    use crate::entity::components::{LookDirection, Position};

    fn floor(block: &str, water_layers: i32) -> (ChunkStore, BlockStateTable) {
        let table = BlockStateTable::native();
        let store = ChunkStore::with_origin_chunk();
        for x in -8..24 {
            for z in -8..24 {
                store.set_block_state(x, 59, z, table.find_state(block, &[]), table);
                for y in 60..60 + water_layers {
                    store.set_block_state(x, y, z, table.find_state("water", &[]), table);
                }
            }
        }
        (store, table)
    }

    fn boat_at(y: f64) -> LivingEntity {
        LivingEntity::new(
            EntityKind::OakBoat,
            Position::new(0.5, y, 0.5),
            LookDirection::new(0.0, 0.0),
            uuid::Uuid::from_u128(1),
        )
    }

    fn run(boat: &mut LivingEntity, store: &ChunkStore, table: BlockStateTable, ticks: usize) {
        for _ in 0..ticks {
            tick_controlled(boat, &[], store, table, &BlockTags::default());
        }
    }

    #[test]
    fn a_boat_floats_and_paddles_ahead_in_water() {
        let (store, table) = floor("stone", 2);
        let mut boat = boat_at(60.0);
        run(&mut boat, &store, table, 40);
        assert!(
            (61.0..62.0).contains(&boat.position.y),
            "y={}",
            boat.position.y
        );
        boat.boat_sim.input.up = true;
        for _ in 0..20 {
            run(&mut boat, &store, table, 1);
            assert_eq!(boat.boat_sim.status, BoatStatus::InWater);
            assert_eq!(boat.boat_sim.paddle_state, [true, true]);
        }
        assert!(boat.position.z > 4.0, "z={}", boat.position.z);
        assert!(boat.body_y_rot_deg.abs() < 1.0);
    }

    #[test]
    fn turning_keys_turn_the_hull_and_stroke_one_paddle() {
        let (store, table) = floor("stone", 2);
        let mut boat = boat_at(61.0);
        boat.boat_sim.input.left = true;
        run(&mut boat, &store, table, 10);
        assert!(boat.body_y_rot_deg < -5.0, "yaw={}", boat.body_y_rot_deg);
        assert_eq!(boat.boat_sim.paddle_state, [false, true]);
    }

    #[test]
    fn ice_carries_a_boat_further_than_stone() {
        let ride = |block| {
            let (store, table) = floor(block, 0);
            let mut boat = boat_at(60.0);
            boat.boat_sim.input.up = true;
            for _ in 0..20 {
                run(&mut boat, &store, table, 1);
                assert_eq!(boat.boat_sim.status, BoatStatus::OnLand);
            }
            boat.position.z
        };
        assert!(ride("ice") > ride("stone") * 3.0);
    }

    #[test]
    fn a_wall_stops_the_boat() {
        let (store, table) = floor("stone", 2);
        for x in -8..24 {
            for y in 60..64 {
                store.set_block_state(x, y, 4, table.find_state("stone", &[]), table);
            }
        }
        let mut boat = boat_at(61.0);
        boat.boat_sim.input.up = true;
        run(&mut boat, &store, table, 60);
        assert_eq!(boat.position.z, 4.0 - 0.6875);
        assert_eq!(boat.velocity.z, 0.0);
    }

    #[test]
    fn rowing_time_lerps_the_last_stroke() {
        let mut sim = BoatSim::default();
        sim.paddle_state[0] = true;
        tick_bubbles_and_paddles(&mut sim);
        tick_bubbles_and_paddles(&mut sim);
        assert_eq!(sim.rowing_time(0, 0.5), PADDLE_SPEED * 1.5);
        assert_eq!(sim.rowing_time(1, 0.5), 0.0);
    }

    #[test]
    fn two_passengers_sit_fore_and_aft() {
        let boat = boat_at(61.0);
        let front = passenger_attachment(&boat, 0, 2, false);
        let back = passenger_attachment(&boat, 1, 2, true);
        assert!((front.z - 0.2).abs() < 1e-6 && (back.z + 0.4).abs() < 1e-6);
        assert!((front.y - 0.5625 / 3.0).abs() < 1e-6);
    }
}
