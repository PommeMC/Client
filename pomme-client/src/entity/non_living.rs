//! The entities that aren't living and that the client draws itself:
//! arrows and tridents, thrown items, fireworks, experience orbs, paintings
//! and item frames. Item entities keep their own store
//! ([`super::ItemEntityStore`]), and boats ride the living one.
//!
//! Projectiles and orbs simulate their movement client-side between server
//! updates, as vanilla's `MoveSimulationType.SERVER_AND_CLIENT` entities do;
//! the hanging ones don't move at all.

use std::collections::HashMap;

use glam::DVec3;
use pomme_item::ItemStackData;
use pomme_registry::builtin::{EntityKind, ItemKind};

use super::MetaValue;
use super::components::Position;
use crate::physics::aabb::Aabb;
use crate::physics::block_shape::CollisionContext;
use crate::physics::collision::{CollisionWorld, resolve_collision};
use crate::position::BlockPos;
pub use crate::world::block::model::Direction;
use crate::world::block::{BlockStateTable, FluidKind};
use crate::world::chunk::ChunkStore;

/// A resolved painting variant: its size in blocks and its sprite.
#[derive(Clone, Debug, PartialEq)]
pub struct PaintingShape {
    pub width: i32,
    pub height: i32,
    /// `PaintingVariant.assetId`, e.g. `minecraft:kebab`.
    pub asset_id: String,
}

/// How [`ThrownItemRenderer`] is built for a kind (`EntityRenderers`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThrownItemStyle {
    pub scale: f32,
    pub full_bright: bool,
}

/// The kinds `ThrownItemRenderer` draws, with their constructor arguments.
pub fn thrown_item_style(kind: EntityKind) -> Option<ThrownItemStyle> {
    let style = |scale, full_bright| Some(ThrownItemStyle { scale, full_bright });
    match kind {
        EntityKind::Egg
        | EntityKind::EnderPearl
        | EntityKind::ExperienceBottle
        | EntityKind::LingeringPotion
        | EntityKind::Snowball
        | EntityKind::SplashPotion => style(1.0, false),
        EntityKind::EyeOfEnder => style(1.0, true),
        EntityKind::Fireball => style(3.0, true),
        EntityKind::SmallFireball => style(0.75, true),
        _ => None,
    }
}

/// The item a thrown entity shows until its `DATA_ITEM_STACK` arrives: each
/// class's `getDefaultItem`.
fn default_item(kind: EntityKind) -> Option<ItemKind> {
    Some(match kind {
        EntityKind::Snowball => ItemKind::Snowball,
        EntityKind::Egg => ItemKind::Egg,
        EntityKind::EnderPearl => ItemKind::EnderPearl,
        EntityKind::ExperienceBottle => ItemKind::ExperienceBottle,
        EntityKind::SplashPotion => ItemKind::SplashPotion,
        EntityKind::LingeringPotion => ItemKind::LingeringPotion,
        EntityKind::EyeOfEnder => ItemKind::EnderEye,
        EntityKind::Fireball | EntityKind::SmallFireball => ItemKind::FireCharge,
        EntityKind::FireworkRocket => ItemKind::FireworkRocket,
        _ => return None,
    })
}

pub fn is_arrow(kind: EntityKind) -> bool {
    matches!(
        kind,
        EntityKind::Arrow | EntityKind::SpectralArrow | EntityKind::Trident
    )
}

pub fn is_item_frame(kind: EntityKind) -> bool {
    matches!(kind, EntityKind::ItemFrame | EntityKind::GlowItemFrame)
}

/// Whether this store tracks `kind`.
pub fn is_non_living(kind: EntityKind) -> bool {
    is_arrow(kind)
        || thrown_item_style(kind).is_some()
        || is_item_frame(kind)
        || matches!(
            kind,
            EntityKind::FireworkRocket | EntityKind::ExperienceOrb | EntityKind::Painting
        )
}

/// The 26.3 index of a value the session's wire sent at `index`.
fn native_index(kind: EntityKind, index: u8) -> u8 {
    native_index_at(kind, index, crate::version::session_protocol())
}

/// [`native_index`] for `protocol`: 1.21.6 synced a hanging entity's facing
/// at 8 and 1.21.2 an arrow's `IN_GROUND` at 10, each moving what followed up
/// one.
fn native_index_at(kind: EntityKind, index: u8, protocol: i32) -> u8 {
    let inserted_at = if is_item_frame(kind) || kind == EntityKind::Painting {
        (protocol < 771).then_some(8)
    } else if is_arrow(kind) {
        (protocol < 768).then_some(10)
    } else {
        None
    };
    match inserted_at {
        Some(at) if index >= at => index + 1,
        _ => index,
    }
}

/// `AbstractArrow.SHAKE_TIME`.
const SHAKE_TIME: i32 = 7;
/// `AbstractArrow.ID_FLAGS`' no-physics bit.
const FLAG_NO_PHYSICS: u8 = 2;
/// `LinearInterpolationHandler`'s default step count.
const INTERPOLATION_STEPS: u32 = 3;
/// `ItemPickupParticle.LIFE_TIME`.
const PICKUP_LIFE: u32 = 3;

pub struct NonLivingEntity {
    pub kind: EntityKind,
    pub uuid: uuid::Uuid,
    pub position: Position,
    pub prev_position: Position,
    /// `Entity.getPositionCodec`'s base: the last position the server sent,
    /// which its move deltas are relative to.
    server_pos: DVec3,
    pub velocity: DVec3,
    pub y_rot: f32,
    pub x_rot: f32,
    pub prev_y_rot: f32,
    pub prev_x_rot: f32,
    /// `Entity.tickCount`.
    pub tick_count: u32,
    /// The add-entity packet's data: a projectile's owner id.
    pub owner: i32,
    /// `Entity.DATA_SHARED_FLAGS_ID`.
    pub shared_flags: u8,
    no_gravity: bool,
    on_ground: bool,
    horizontal_collision: bool,
    first_tick: bool,
    /// The item it shows: a thrown item's or firework's stack, or what an
    /// item frame holds.
    pub item: Option<(String, ItemStackData)>,
    // Arrows and tridents.
    arrow_flags: u8,
    pub in_ground: bool,
    /// `AbstractArrow.shakeTime`.
    pub shake_time: i32,
    /// `AbstractArrow.inGroundTime`.
    in_ground_time: u32,
    /// `ThrownTrident.dealtDamage`, as far as the client sets it.
    dealt_damage: bool,
    /// `Arrow.ID_EFFECT_COLOR`.
    pub arrow_color: i32,
    pub trident_loyalty: u8,
    // Fireworks.
    pub attached_to_entity: bool,
    pub shot_at_angle: bool,
    // Experience orbs.
    pub xp_value: i32,
    /// `ExperienceOrb.followingPlayer`'s id.
    following_player: Option<i32>,
    /// `LinearInterpolationHandler`'s target and remaining steps.
    interpolation: Option<(DVec3, u32)>,
    // Paintings and item frames.
    block_pos: BlockPos,
    pub direction: Direction,
    pub painting: Option<PaintingShape>,
    /// `ItemFrame.DATA_ROTATION`, 0..7.
    pub frame_rotation: i32,
}

impl NonLivingEntity {
    fn new(
        kind: EntityKind,
        uuid: uuid::Uuid,
        position: Position,
        velocity: DVec3,
        y_rot: f32,
        x_rot: f32,
    ) -> Self {
        let item = default_item(kind).map(|kind| {
            (
                crate::player::inventory::item_resource_name(kind),
                ItemStackData {
                    kind,
                    count: 1,
                    component_patch: Default::default(),
                },
            )
        });
        Self {
            kind,
            uuid,
            position,
            prev_position: position,
            server_pos: *position,
            velocity,
            y_rot,
            x_rot,
            prev_y_rot: y_rot,
            prev_x_rot: x_rot,
            tick_count: 0,
            owner: 0,
            shared_flags: 0,
            no_gravity: false,
            on_ground: false,
            horizontal_collision: false,
            first_tick: true,
            item,
            arrow_flags: 0,
            in_ground: false,
            shake_time: 0,
            in_ground_time: 0,
            dealt_damage: false,
            arrow_color: -1,
            trident_loyalty: 0,
            attached_to_entity: false,
            shot_at_angle: false,
            xp_value: 0,
            following_player: None,
            interpolation: None,
            block_pos: BlockPos::new(0, 0, 0),
            direction: Direction::South,
            painting: None,
            frame_rotation: 0,
        }
    }

    pub fn is_invisible(&self) -> bool {
        super::shared_flags_invisible(self.shared_flags)
    }

    pub fn is_glowing(&self) -> bool {
        super::shared_flags_glowing(self.shared_flags)
    }

    pub fn is_on_fire(&self) -> bool {
        super::shared_flags_on_fire(self.shared_flags)
    }

    /// `AbstractArrow.isNoPhysics` on the client: the synced flag.
    pub fn is_no_physics(&self) -> bool {
        self.arrow_flags & FLAG_NO_PHYSICS != 0
    }

    pub fn lerped_position(&self, partial: f32) -> Position {
        self.prev_position.lerp(self.position, partial as f64)
    }

    /// `Entity.getYRot(partial)`: a shortest-way lerp.
    pub fn lerped_y_rot(&self, partial: f32) -> f32 {
        super::lerp_angle(self.prev_y_rot, self.y_rot, partial)
    }

    /// `Entity.getXRot(partial)`: a plain lerp, unlike the yaw's.
    pub fn lerped_x_rot(&self, partial: f32) -> f32 {
        glam::FloatExt::lerp(self.prev_x_rot, self.x_rot, partial)
    }

    /// `ExperienceOrb.getIcon`.
    pub fn xp_icon(&self) -> u32 {
        const STEPS: [i32; 10] = [3, 7, 17, 37, 73, 149, 307, 617, 1237, 2477];
        STEPS.iter().filter(|&&step| self.xp_value >= step).count() as u32
    }

    /// `ItemFrame.hasFramedMap`.
    pub fn has_framed_map(&self) -> bool {
        self.item.as_ref().is_some_and(|(_, stack)| {
            stack
                .component_patch
                .get(pomme_registry::builtin::DataComponentKind::MapId)
                .is_some()
        })
    }

    /// `HangingEntity.recalculateBoundingBox`: the entity sits at the centre
    /// of the box `calculateBoundingBox` gives for its block and facing.
    fn recalculate_bounding_box(&mut self) {
        if let Some(aabb) = self.bounding_box() {
            let center = (aabb.min + aabb.max) / 2.0;
            self.position = Position::from(center);
            self.prev_position = self.position;
        }
    }

    /// `Painting.calculateBoundingBox` and `ItemFrame.createBoundingBox`.
    pub fn bounding_box(&self) -> Option<Aabb> {
        let direction = self.direction;
        let center = self.block_pos.center() - direction.step() * 0.46875;
        let (center, width, height) = match self.kind {
            EntityKind::Painting => {
                let shape = self.painting.as_ref()?;
                let offset = |size: i32| if size % 2 == 0 { 0.5 } else { 0.0 };
                let center = center
                    + direction.counter_clockwise().step() * offset(shape.width)
                    + DVec3::Y * offset(shape.height);
                (center, shape.width as f64, shape.height as f64)
            }
            _ if is_item_frame(self.kind) => {
                let size = if self.has_framed_map() { 1.0 } else { 0.75 };
                (center, size, size)
            }
            _ => return None,
        };
        let step = direction.step().abs();
        let size = DVec3::new(
            if step.x > 0.0 { 0.0625 } else { width },
            if step.y > 0.0 { 0.0625 } else { height },
            if step.z > 0.0 { 0.0625 } else { width },
        );
        Some(Aabb::new(center - size / 2.0, center + size / 2.0))
    }

    /// `HangingEntity.setDirection` / `ItemFrame.setDirection`.
    fn set_direction(&mut self, direction: Direction) {
        // A painting only hangs on walls (`Validate.isTrue(horizontal)`).
        if self.kind == EntityKind::Painting && !direction.is_horizontal() {
            return;
        }
        self.direction = direction;
        if direction.is_horizontal() {
            self.x_rot = 0.0;
            self.y_rot = (direction.data_2d() * 90) as f32;
        } else {
            self.x_rot = if direction == Direction::Up {
                -90.0
            } else {
                90.0
            };
            self.y_rot = 0.0;
        }
        self.prev_x_rot = self.x_rot;
        self.prev_y_rot = self.y_rot;
        self.recalculate_bounding_box();
    }

    fn is_hanging(&self) -> bool {
        self.kind == EntityKind::Painting || is_item_frame(self.kind)
    }

    /// `Entity.setPos` after a server move; a hanging entity's `setPos`
    /// (`BlockAttachedEntity`) re-derives it from the block the point is in.
    fn set_position(&mut self, position: Position) {
        if self.is_hanging() {
            self.block_pos = BlockPos::containing(*position);
            self.recalculate_bounding_box();
        } else if self.kind == EntityKind::ExperienceOrb && !self.first_tick {
            // `LinearInterpolationHandler.interpolateTo`.
            self.interpolation = Some((*position, INTERPOLATION_STEPS));
        } else {
            self.position = position;
        }
    }
}

/// An entity being picked up (`ItemPickupParticle`): its state frozen at the
/// pickup, drawn flying to the collector over three ticks.
pub struct NonLivingPickup {
    pub entity: NonLivingEntity,
    start: Position,
    collector: i32,
    /// Where the collector was last tick and is now (`targetXOld`, `targetX`).
    prev_target: Position,
    target: Position,
    life: u32,
}

impl NonLivingPickup {
    /// `ItemPickupParticleGroup.fromParticle`: toward the collector's lerped
    /// position, on squared time.
    pub fn position(&self, partial: f32) -> Position {
        let t = (self.life as f32 + partial) / PICKUP_LIFE as f32;
        let target = self.prev_target.lerp(self.target, partial as f64);
        self.start.lerp(target, (t * t) as f64)
    }
}

/// A player an orb can follow.
pub struct OrbTarget {
    pub id: i32,
    pub position: DVec3,
    pub eye_height: f64,
    /// `isDeadOrDying`.
    pub dead: bool,
}

/// The client's view of the world the tick reads: chunks for collision and
/// the players an orb follows.
pub struct TickWorld<'a> {
    pub chunks: &'a ChunkStore,
    pub block_state_table: BlockStateTable,
    /// Every non-spectator player, the local one included, for
    /// `ExperienceOrb.followNearbyPlayer`.
    pub players: &'a [OrbTarget],
    /// A returning trident's owner, by id, if the client has it.
    pub owner_of: &'a dyn Fn(i32) -> Option<TridentOwner>,
    /// Where a pickup flies to on a collector, by id (`ItemPickupParticle`).
    pub pickup_target_of: &'a dyn Fn(i32) -> Option<Position>,
}

/// What `ThrownTrident.tick` reads off its owner.
#[derive(Clone, Copy)]
pub struct TridentOwner {
    pub eye: DVec3,
    pub is_player: bool,
    /// `isAlive`.
    pub alive: bool,
    pub width: f64,
}

#[derive(Default)]
pub struct NonLivingStore {
    entities: HashMap<i32, NonLivingEntity>,
    pickups: Vec<NonLivingPickup>,
}

impl NonLivingStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn entities(&self) -> impl Iterator<Item = (i32, &NonLivingEntity)> {
        self.entities.iter().map(|(&id, e)| (id, e))
    }

    pub fn get(&self, id: i32) -> Option<&NonLivingEntity> {
        self.entities.get(&id)
    }

    pub fn pickups(&self) -> &[NonLivingPickup] {
        &self.pickups
    }

    /// `ClientPacketListener.handleAddEntity` for a kind this store tracks:
    /// `recreateFromPacket`, with each class's use of `data`.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        &mut self,
        id: i32,
        kind: EntityKind,
        uuid: uuid::Uuid,
        position: Position,
        velocity: DVec3,
        y_rot: f32,
        x_rot: f32,
        data: i32,
    ) {
        let mut entity = NonLivingEntity::new(kind, uuid, position, velocity, y_rot, x_rot);
        if entity.is_hanging() {
            entity.block_pos = BlockPos::containing(*position);
            entity.set_direction(Direction::from_3d(data));
        } else if kind == EntityKind::ExperienceOrb {
            // Native servers send 0 here and the value as data; the
            // translation carries an older wire's `add_experience_orb`
            // value here instead.
            entity.xp_value = data;
        } else {
            entity.owner = data;
        }
        self.entities.insert(id, entity);
    }

    pub fn contains(&self, id: i32) -> bool {
        self.entities.contains_key(&id)
    }

    /// A move packet, decoded against the server's last position
    /// (`VecDeltaCodec.decode`), not where the client has simulated it to.
    pub fn move_delta(&mut self, id: i32, delta: DVec3) {
        if let Some(e) = self.entities.get_mut(&id) {
            e.server_pos += delta;
            e.set_position(Position::from(e.server_pos));
        }
    }

    pub fn teleport(&mut self, id: i32, position: Position, velocity: Option<DVec3>) {
        if let Some(e) = self.entities.get_mut(&id) {
            e.server_pos = *position;
            e.set_position(position);
            if let Some(velocity) = velocity {
                e.velocity = velocity;
            }
        }
    }

    /// A move packet's rotation: these entities snap
    /// (`InterpolationHandler.NO_OP`).
    pub fn rotate(&mut self, id: i32, y_rot: f32, x_rot: f32) {
        if let Some(e) = self.entities.get_mut(&id)
            && !e.is_hanging()
        {
            e.y_rot = y_rot;
            e.x_rot = x_rot;
        }
    }

    /// `Entity.lerpMotion`; an arrow also leaves the ground once it moves
    /// (`AbstractArrow.lerpMotion`).
    pub fn set_motion(&mut self, id: i32, velocity: DVec3) {
        if let Some(e) = self.entities.get_mut(&id) {
            e.velocity = velocity;
            if is_arrow(e.kind) && velocity.length_squared() > 0.0 {
                e.in_ground = false;
            }
        }
    }

    pub fn remove(&mut self, ids: &[i32]) {
        for id in ids {
            self.entities.remove(id);
        }
    }

    pub fn clear(&mut self) {
        self.entities.clear();
        self.pickups.clear();
    }

    /// `onSyncedDataUpdated` for the scalar values these classes sync.
    pub fn apply_entity_data(&mut self, id: i32, index: u8, value: MetaValue) {
        let Some(e) = self.entities.get_mut(&id) else {
            return;
        };
        match (native_index(e.kind, index), value) {
            (0, MetaValue::Byte(flags)) => e.shared_flags = flags,
            (5, MetaValue::Bool(no_gravity)) => e.no_gravity = no_gravity,
            // `HangingEntity.DATA_DIRECTION`.
            (8, MetaValue::Direction(direction)) if e.is_hanging() => {
                e.set_direction(Direction::from_3d(direction as i32));
            }
            // `ExperienceOrb.DATA_VALUE`.
            (8, MetaValue::Int(value)) if e.kind == EntityKind::ExperienceOrb => {
                e.xp_value = value;
            }
            // `AbstractArrow.ID_FLAGS` and `IN_GROUND`.
            (8, MetaValue::Byte(flags)) if is_arrow(e.kind) => e.arrow_flags = flags,
            (10, MetaValue::Bool(in_ground)) if is_arrow(e.kind) => {
                e.in_ground = in_ground;
                if !e.first_tick && e.shake_time <= 0 && in_ground {
                    e.shake_time = SHAKE_TIME;
                }
            }
            // `Arrow.ID_EFFECT_COLOR`.
            (11, MetaValue::Int(color)) if e.kind == EntityKind::Arrow => e.arrow_color = color,
            // `ThrownTrident.ID_LOYALTY`.
            (11, MetaValue::Byte(loyalty)) if e.kind == EntityKind::Trident => {
                e.trident_loyalty = loyalty;
            }
            // `FireworkRocketEntity.DATA_ATTACHED_TO_TARGET` and
            // `DATA_SHOT_AT_ANGLE`.
            (9, MetaValue::OptionalUnsignedInt(attached))
                if e.kind == EntityKind::FireworkRocket =>
            {
                e.attached_to_entity = attached.is_some();
            }
            (10, MetaValue::Bool(angled)) if e.kind == EntityKind::FireworkRocket => {
                e.shot_at_angle = angled;
            }
            // `ItemFrame.DATA_ROTATION`.
            (10, MetaValue::Int(rotation)) if is_item_frame(e.kind) => e.frame_rotation = rotation,
            _ => {}
        }
    }

    /// The item stack value at `index`: `DATA_ITEM_STACK` (8) on the thrown
    /// items and fireworks, `ItemFrame.DATA_ITEM` (9).
    pub fn set_item(&mut self, id: i32, index: u8, item: Option<(String, ItemStackData)>) {
        let Some(e) = self.entities.get_mut(&id) else {
            return;
        };
        let slot = if is_item_frame(e.kind) { 9 } else { 8 };
        let holds_item = is_item_frame(e.kind) || default_item(e.kind).is_some();
        if native_index(e.kind, index) != slot || !holds_item {
            return;
        }
        e.item = item;
        if is_item_frame(e.kind) {
            // `ItemFrame.onItemChanged`: a map widens the box.
            e.recalculate_bounding_box();
        }
    }

    pub fn set_painting(&mut self, id: i32, shape: Option<PaintingShape>) {
        if let Some(e) = self.entities.get_mut(&id)
            && e.kind == EntityKind::Painting
        {
            e.painting = shape;
            e.recalculate_bounding_box();
        }
    }

    /// `ClientPacketListener.handleTakeItemEntity` for an orb or arrow: a
    /// pickup particle flies a frozen copy to the collector, when the client
    /// has one. An arrow is removed with it; an orb waits for the server's
    /// removal, since one orb can be picked up several times. Returns where
    /// it was, for the sound.
    pub fn pickup(&mut self, id: i32, collector: Option<(i32, Position)>) -> Option<Position> {
        let entity = self.entities.get(&id)?;
        let start = entity.position;
        let frozen = if entity.kind == EntityKind::ExperienceOrb {
            collector.is_some().then(|| {
                let mut copy = NonLivingEntity::new(
                    entity.kind,
                    entity.uuid,
                    entity.position,
                    DVec3::ZERO,
                    entity.y_rot,
                    entity.x_rot,
                );
                copy.xp_value = entity.xp_value;
                copy.tick_count = entity.tick_count;
                copy
            })
        } else {
            Some(self.entities.remove(&id)?)
        };
        if let (Some(entity), Some((collector, target))) = (frozen, collector) {
            self.pickups.push(NonLivingPickup {
                entity,
                start,
                collector,
                prev_target: target,
                target,
                life: 0,
            });
        }
        Some(start)
    }

    pub fn tick(&mut self, world: &TickWorld<'_>) {
        self.entities.retain(|_, e| {
            // Entities outside loaded chunks don't tick.
            if !super::chunk_loaded(world.chunks, *e.position) {
                return true;
            }
            // `Entity.commonTick`.
            e.prev_position = e.position;
            e.prev_y_rot = e.y_rot;
            e.prev_x_rot = e.x_rot;
            if let Some((target, steps)) = e.interpolation {
                // `LinearInterpolationHandler.doInterpolate`.
                e.position = e.position.lerp(Position::from(target), 1.0 / steps as f64);
                e.interpolation = (steps > 1).then_some((target, steps - 1));
            }
            e.tick_count += 1;
            let alive = !is_arrow(e.kind) || tick_arrow(e, world);
            match e.kind {
                kind if thrown_item_style(kind).is_some() => tick_thrown(e, world),
                EntityKind::FireworkRocket => tick_firework(e, world),
                EntityKind::ExperienceOrb => tick_orb(e, world),
                _ => {}
            }
            e.first_tick = false;
            alive
        });
        // `ItemPickupParticle.tick`.
        self.pickups.retain_mut(|p| {
            p.life += 1;
            p.prev_target = p.target;
            if let Some(target) = (world.pickup_target_of)(p.collector) {
                p.target = target;
            }
            p.life < PICKUP_LIFE
        });
    }
}

impl TickWorld<'_> {
    fn collision(&self) -> CollisionWorld<'_> {
        CollisionWorld::new(self.chunks, self.block_state_table)
    }

    fn fluid_at(&self, pos: DVec3) -> FluidKind {
        let block = BlockPos::containing(pos);
        let state = self.chunks.get_block_state(block.x, block.y, block.z);
        self.block_state_table.block_state_data(state).fluid().kind
    }
}

/// `Level.clip` with `ClipContext.Block.COLLIDER`: where the segment first
/// enters a block's collision shape.
fn clip_blocks(world: &TickWorld<'_>, from: DVec3, to: DVec3) -> Option<DVec3> {
    let collision = world.collision();
    let ctx = CollisionContext::EMPTY;
    let (min, max) = (from.min(to).floor(), from.max(to).floor());
    let mut nearest: Option<(f64, DVec3)> = None;
    for x in min.x as i32..=max.x as i32 {
        for y in min.y as i32..=max.y as i32 {
            for z in min.z as i32..=max.z as i32 {
                for aabb in collision.block_boxes(BlockPos::new(x, y, z), &ctx) {
                    if let Some(hit) = aabb.clip(from, to) {
                        let distance = hit.distance_squared(from);
                        if nearest.is_none_or(|(d, _)| distance < d) {
                            nearest = Some((distance, hit));
                        }
                    }
                }
            }
        }
    }
    nearest.map(|(_, hit)| hit)
}

/// Whether `pos` lies inside a collision box of the block it's in
/// (`AbstractArrow.tick`'s opening check).
fn inside_collision(world: &TickWorld<'_>, pos: DVec3) -> bool {
    world
        .collision()
        .block_boxes(BlockPos::containing(pos), &CollisionContext::EMPTY)
        .iter()
        .any(|aabb| aabb.contains(pos))
}

/// `Projectile.lerpRotation`: a fifth of the way along the short arc.
fn lerp_rotation(from: f32, to: f32) -> f32 {
    let mut from = from;
    while to - from < -180.0 {
        from -= 360.0;
    }
    while to - from >= 180.0 {
        from += 360.0;
    }
    from + (to - from) * 0.2
}

/// The pitch and yaw `movement` points along (`Projectile.updateRotation`'s
/// targets), with vanilla's radians-to-degrees constant.
fn movement_angles(movement: DVec3) -> (f32, f32) {
    let degrees = |y: f64, x: f64| (y.atan2(x) * 57.295_776_367_187_5) as f32;
    (
        degrees(movement.y, movement.x.hypot(movement.z)),
        degrees(movement.x, movement.z),
    )
}

/// Moves by `movement`, stopping where it first enters a block when `clip`;
/// whether it hit one.
fn move_clipped(
    e: &mut NonLivingEntity,
    world: &TickWorld<'_>,
    movement: DVec3,
    clip: bool,
) -> bool {
    let to = *e.position + movement;
    let hit = clip.then(|| clip_blocks(world, *e.position, to)).flatten();
    e.position = Position::from(hit.unwrap_or(to));
    hit.is_some()
}

/// `ThrownTrident.tick`'s return ahead of the arrow tick: after a few ticks
/// in the ground a loyal trident flies back to its owner, and is discarded
/// once its owner is gone or, for a mob, reached. Whether it lives on.
fn tick_trident_return(e: &mut NonLivingEntity, world: &TickWorld<'_>) -> bool {
    if e.in_ground_time > 4 {
        e.dealt_damage = true;
    }
    if e.trident_loyalty == 0 || !(e.dealt_damage || e.is_no_physics()) {
        return true;
    }
    let Some(owner) = (world.owner_of)(e.owner) else {
        return true;
    };
    // `isAcceptibleReturnOwner`: a client never sees a `ServerPlayer`.
    if !owner.alive {
        return false;
    }
    let to_owner = owner.eye - *e.position;
    if !owner.is_player && to_owner.length() < owner.width + 1.0 {
        return false;
    }
    e.arrow_flags |= FLAG_NO_PHYSICS;
    let loyalty = f64::from(e.trident_loyalty);
    e.position.y += to_owner.y * 0.015 * loyalty;
    e.velocity = e.velocity * 0.95 + to_owner.normalize_or_zero() * (0.05 * loyalty);
    true
}

/// `AbstractArrow.tick` as the client runs it; hits, despawning and falling
/// are server-side. Whether it lives on.
fn tick_arrow(e: &mut NonLivingEntity, world: &TickWorld<'_>) -> bool {
    if e.kind == EntityKind::Trident && !tick_trident_return(e, world) {
        return false;
    }
    let physics = !e.is_no_physics();
    let movement = e.velocity;
    if physics && inside_collision(world, *e.position) {
        e.velocity = DVec3::ZERO;
        e.in_ground = true;
    }
    if e.shake_time > 0 {
        e.shake_time -= 1;
    }
    if e.in_ground && physics {
        e.in_ground_time += 1;
        return true;
    }
    e.in_ground_time = 0;
    let in_water = world.fluid_at(*e.position) == FluidKind::Water;
    let water_inertia = if e.kind == EntityKind::Trident {
        0.99
    } else {
        0.6
    };
    if in_water {
        e.velocity *= water_inertia;
    }
    let (x_rot, mut y_rot) = movement_angles(movement);
    if !physics {
        y_rot = movement_angles(-movement).1;
    }
    e.x_rot = lerp_rotation(e.x_rot, x_rot);
    e.y_rot = lerp_rotation(e.y_rot, y_rot);
    if move_clipped(e, world, movement, physics) {
        // `AbstractArrow.onHitBlock`: back off the face, stop and quiver.
        let signum = movement.map(|c| if c == 0.0 { 0.0 } else { c.signum() });
        e.position -= signum * f64::from(0.05f32);
        e.velocity = DVec3::ZERO;
        e.in_ground = true;
        e.shake_time = SHAKE_TIME;
        return true;
    }
    if !in_water {
        e.velocity *= 0.99;
    }
    if physics && !e.no_gravity {
        e.velocity.y -= 0.05;
    }
    true
}

/// `ThrowableProjectile.tick`, `AbstractHurtingProjectile.tick` and
/// `EyeOfEnder.tick` movement; hits are server-side.
fn tick_thrown(e: &mut NonLivingEntity, world: &TickWorld<'_>) {
    let in_water = world.fluid_at(*e.position) == FluidKind::Water;
    match e.kind {
        EntityKind::EyeOfEnder => {}
        EntityKind::Fireball | EntityKind::SmallFireball => {
            // `accelerationPower` stays its default on the client.
            let inertia = if in_water { 0.8 } else { 0.95 };
            e.velocity = (e.velocity + e.velocity.normalize_or_zero() * 0.1) * inertia;
        }
        kind => {
            let gravity = match kind {
                EntityKind::SplashPotion | EntityKind::LingeringPotion => 0.05,
                EntityKind::ExperienceBottle => 0.07,
                _ => 0.03,
            };
            if !e.no_gravity {
                e.velocity.y -= gravity;
            }
            e.velocity *= if in_water { 0.8 } else { 0.99 };
        }
    }
    move_clipped(e, world, e.velocity, e.kind != EntityKind::EyeOfEnder);
}

/// `FireworkRocketEntity.tick`'s free flight; an attached rocket isn't drawn.
fn tick_firework(e: &mut NonLivingEntity, world: &TickWorld<'_>) {
    if e.attached_to_entity {
        return;
    }
    if !e.shot_at_angle {
        let acceleration = if e.horizontal_collision { 1.0 } else { 1.15 };
        e.velocity = DVec3::new(
            e.velocity.x * acceleration,
            e.velocity.y + 0.04,
            e.velocity.z * acceleration,
        );
    }
    let movement = e.velocity;
    let (delta, on_ground) = move_with_collision(e, world, 0.25, movement);
    e.position += delta;
    e.on_ground = on_ground;
    e.horizontal_collision = delta.x != movement.x || delta.z != movement.z;
    e.velocity = movement;
    // `Projectile.updateRotation`.
    let (x_rot, y_rot) = movement_angles(movement);
    e.x_rot = lerp_rotation(e.prev_x_rot, x_rot);
    e.y_rot = lerp_rotation(e.prev_y_rot, y_rot);
}

/// `Entity.move(SELF)`'s block collision for a box `size` wide and tall,
/// standing on the entity's position.
fn move_with_collision(
    e: &NonLivingEntity,
    world: &TickWorld<'_>,
    size: f64,
    movement: DVec3,
) -> (DVec3, bool) {
    let aabb = Aabb::from_center(*e.position, size / 2.0, size / 2.0);
    let ctx = CollisionContext::entity(e.position.y, false, false);
    resolve_collision(
        world.collision(),
        aabb,
        movement.into(),
        0.0,
        e.on_ground,
        &ctx,
    )
}

/// `ExperienceOrb.tick` as the client runs it: merging, unsticking and
/// pickup are server-side.
fn tick_orb(e: &mut NonLivingEntity, world: &TickWorld<'_>) {
    const GRAVITY: f64 = 0.03;
    const SIZE: f64 = 0.5;
    if e.first_tick {
        return;
    }
    let aabb = Aabb::from_center(*e.position, SIZE / 2.0, SIZE / 2.0);
    let colliding = !crate::physics::collision::no_collision(
        world.collision(),
        &aabb,
        &CollisionContext::EMPTY,
    );
    // `isEyeInFluid(WATER)`, the eye at 0.85 of the orb's height.
    let eye = *e.position + DVec3::Y * (SIZE * 0.85);
    if world.fluid_at(eye) == FluidKind::Water {
        // `setUnderwaterMovement`.
        e.velocity = DVec3::new(
            e.velocity.x * 0.99,
            (e.velocity.y + 5.0e-4).min(0.06),
            e.velocity.z * 0.99,
        );
    } else if !colliding && !e.no_gravity {
        e.velocity.y -= GRAVITY;
    }
    // TODO: the lava hop (`random.nextFloat() - random.nextFloat()`).
    // `followNearbyPlayer`: keep the followed player until it turns
    // spectator or leaves 8 blocks, then take the nearest living one.
    let distance = |p: &OrbTarget| p.position.distance_squared(*e.position);
    let followed = e
        .following_player
        .and_then(|id| world.players.iter().find(|p| p.id == id))
        .filter(|p| distance(p) <= 64.0)
        .or_else(|| {
            world
                .players
                .iter()
                .filter(|p| distance(p) < 64.0)
                .min_by(|a, b| distance(a).total_cmp(&distance(b)))
                .filter(|p| !p.dead)
        });
    e.following_player = followed.map(|p| p.id);
    if let Some(player) = followed {
        let delta = player.position + DVec3::Y * (player.eye_height / 2.0) - *e.position;
        let power = 1.0 - delta.length() / 8.0;
        e.velocity += delta.normalize_or_zero() * (power * power * 0.1);
    }
    let fall_speed = e.velocity.y;
    let movement = e.velocity;
    let (delta, on_ground) = move_with_collision(e, world, SIZE, movement);
    e.position += delta;
    if let Some((target, steps)) = e.interpolation {
        // `AbstractInterpolationHandler.applyPredictedMovement`.
        e.interpolation = Some((target + delta, steps));
    }
    e.on_ground = on_ground;
    // TODO: per-block friction (ice, slime); vanilla multiplies by the block
    // below's, 0.6 by default.
    let friction = if on_ground { 0.98 * 0.6 } else { 0.98 };
    e.velocity *= friction;
    let vertical_collision_below = movement.y < 0.0 && delta.y != movement.y;
    // `getGravity()` is 0 for a weightless orb.
    let gravity = if e.no_gravity { 0.0 } else { GRAVITY };
    if vertical_collision_below && fall_speed < -gravity {
        e.velocity.y = -fall_speed * 0.4;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spawn(kind: EntityKind, at: DVec3, data: i32) -> NonLivingStore {
        let mut store = NonLivingStore::new();
        store.spawn(
            1,
            kind,
            uuid::Uuid::nil(),
            Position::from(at),
            DVec3::ZERO,
            0.0,
            0.0,
            data,
        );
        store
    }

    #[test]
    fn xp_icons_follow_value_thresholds() {
        let mut store = spawn(EntityKind::ExperienceOrb, DVec3::ZERO, 0);
        for (value, icon) in [
            (0, 0),
            (2, 0),
            (3, 1),
            (16, 2),
            (17, 3),
            (2476, 9),
            (2477, 10),
        ] {
            store.apply_entity_data(1, 8, MetaValue::Int(value));
            assert_eq!(store.get(1).unwrap().xp_icon(), icon, "value {value}");
        }
    }

    /// `ItemFrame.createBoundingBox`: the block centre pushed 0.46875 toward
    /// the wall, 0.75 square and 1/16 deep; a map widens it to a block.
    #[test]
    fn item_frame_sits_against_its_wall() {
        // Facing north (3D id 2): hung on the block to its south.
        let mut store = spawn(EntityKind::ItemFrame, DVec3::new(4.0, 70.0, -3.0), 2);
        let frame = store.get(1).unwrap();
        assert_eq!(*frame.position, DVec3::new(4.5, 70.5, -3.0 + 0.5 + 0.46875));
        let aabb = frame.bounding_box().unwrap();
        assert_eq!(aabb.max - aabb.min, DVec3::new(0.75, 0.75, 0.0625));
        assert_eq!(frame.y_rot, 180.0);

        // Facing up: lying on the block below.
        store.apply_entity_data(1, 8, MetaValue::Direction(1));
        let frame = store.get(1).unwrap();
        assert_eq!(*frame.position, DVec3::new(4.5, 70.5 - 0.46875, -2.5));
        assert_eq!(frame.x_rot, -90.0);
    }

    /// `Painting.calculateBoundingBox`: an even side shifts half a block
    /// counter-clockwise (left of the facing) and up.
    #[test]
    fn even_paintings_shift_left_and_up() {
        let mut store = spawn(EntityKind::Painting, DVec3::new(0.0, 64.0, 0.0), 3);
        store.set_painting(
            1,
            Some(PaintingShape {
                width: 2,
                height: 2,
                asset_id: "minecraft:bust".into(),
            }),
        );
        let painting = store.get(1).unwrap();
        // Facing south: counter-clockwise is east.
        assert_eq!(*painting.position, DVec3::new(1.0, 65.0, 0.5 - 0.46875));
        let aabb = painting.bounding_box().unwrap();
        assert_eq!(aabb.max - aabb.min, DVec3::new(2.0, 2.0, 0.0625));
    }

    /// A painting can't face up or down; the update is ignored.
    #[test]
    fn paintings_stay_on_walls() {
        let mut store = spawn(EntityKind::Painting, DVec3::ZERO, 2);
        store.apply_entity_data(1, 8, MetaValue::Direction(0));
        assert_eq!(store.get(1).unwrap().direction, Direction::North);
    }

    /// `AbstractArrow.onSyncedDataUpdated`: landing starts the shake, but not
    /// on the first tick (a spawn already in the ground).
    #[test]
    fn landing_shakes_after_the_first_tick() {
        let mut store = spawn(EntityKind::Arrow, DVec3::ZERO, 0);
        store.apply_entity_data(1, 10, MetaValue::Bool(true));
        assert_eq!(store.get(1).unwrap().shake_time, 0);
        store.entities.get_mut(&1).unwrap().first_tick = false;
        store.apply_entity_data(1, 10, MetaValue::Bool(false));
        store.apply_entity_data(1, 10, MetaValue::Bool(true));
        assert_eq!(store.get(1).unwrap().shake_time, SHAKE_TIME);
    }

    #[test]
    fn thrown_items_default_to_their_item() {
        let mut store = spawn(EntityKind::Snowball, DVec3::ZERO, 0);
        assert_eq!(store.get(1).unwrap().item.as_ref().unwrap().0, "snowball");
        // An item frame's item lives at 9, not 8.
        store.spawn(
            2,
            EntityKind::ItemFrame,
            uuid::Uuid::nil(),
            Position::from(DVec3::ZERO),
            DVec3::ZERO,
            0.0,
            0.0,
            2,
        );
        let stack = |kind| {
            Some((
                crate::player::inventory::item_resource_name(kind),
                ItemStackData {
                    kind,
                    count: 1,
                    component_patch: Default::default(),
                },
            ))
        };
        store.set_item(2, 8, stack(ItemKind::Diamond));
        assert!(store.get(2).unwrap().item.is_none());
        store.set_item(2, 9, stack(ItemKind::Diamond));
        assert_eq!(store.get(2).unwrap().item.as_ref().unwrap().0, "diamond");
    }

    /// Move deltas build on the server's last position, whatever the client
    /// simulated meanwhile.
    #[test]
    fn moves_decode_against_the_server_position() {
        let mut store = spawn(EntityKind::Snowball, DVec3::new(0.0, 64.0, 0.0), 0);
        store.entities.get_mut(&1).unwrap().position = Position::new(0.0, 64.0, 3.0);
        store.move_delta(1, DVec3::new(0.0, 0.0, 1.5));
        assert_eq!(*store.get(1).unwrap().position, DVec3::new(0.0, 64.0, 1.5));
        store.teleport(1, Position::new(5.0, 64.0, 5.0), None);
        store.move_delta(1, DVec3::new(1.0, 0.0, 0.0));
        assert_eq!(*store.get(1).unwrap().position, DVec3::new(6.0, 64.0, 5.0));
    }

    #[test]
    fn older_wires_lift_inserted_indices() {
        // A 1.21.5 frame's item at 8, a 1.21.1 arrow's colour at 10.
        assert_eq!(native_index_at(EntityKind::ItemFrame, 8, 770), 9);
        assert_eq!(native_index_at(EntityKind::ItemFrame, 8, 771), 8);
        assert_eq!(native_index_at(EntityKind::Arrow, 10, 767), 11);
        assert_eq!(native_index_at(EntityKind::Arrow, 8, 767), 8);
        assert_eq!(native_index_at(EntityKind::Snowball, 8, 763), 8);
    }

    #[test]
    fn attached_fireworks_hide() {
        let mut store = spawn(EntityKind::FireworkRocket, DVec3::ZERO, 0);
        store.apply_entity_data(1, 9, MetaValue::OptionalUnsignedInt(Some(5)));
        assert!(store.get(1).unwrap().attached_to_entity);
        store.apply_entity_data(1, 9, MetaValue::OptionalUnsignedInt(None));
        assert!(!store.get(1).unwrap().attached_to_entity);
    }

    /// Entities in unloaded chunks don't age or end their first tick.
    #[test]
    fn unloaded_entities_hold_still() {
        let mut store = spawn(EntityKind::Arrow, DVec3::ZERO, 0);
        let chunks = ChunkStore::new(2);
        let world = TickWorld {
            chunks: &chunks,
            block_state_table: BlockStateTable::native(),
            players: &[],
            owner_of: &|_| None,
            pickup_target_of: &|_| None,
        };
        store.tick(&world);
        let arrow = store.get(1).unwrap();
        assert_eq!(arrow.tick_count, 0);
        assert!(arrow.first_tick);
    }

    #[test]
    fn rotation_lerps_a_fifth_along_the_short_arc() {
        assert_eq!(lerp_rotation(0.0, 10.0), 2.0);
        // Unwrapped, as vanilla leaves it: -186 is 174.
        assert_eq!(lerp_rotation(170.0, -170.0), -186.0);
    }
}
