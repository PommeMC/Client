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
use pomme_registry::tags;

use super::MetaValue;
use super::components::Position;
use super::dimensions::EntityDimensions;
use crate::physics::aabb::Aabb;
use crate::physics::block_shape::CollisionContext;
use crate::physics::collision::{CollisionWorld, resolve_collision};
use crate::physics::movement::mth_equal;
use crate::position::BlockPos;
use crate::world::block::BlockStateTable;
pub use crate::world::block::model::Direction;
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

/// `AbstractArrow.SHAKE_TIME`.
const SHAKE_TIME: i32 = 7;
/// `AbstractArrow.ID_FLAGS`' crit and no-physics bits.
const FLAG_CRIT: u8 = 1;
const FLAG_NO_PHYSICS: u8 = 2;
/// `LinearInterpolationHandler`'s default step count.
const INTERPOLATION_STEPS: u32 = 3;
/// `ItemPickupParticle.LIFE_TIME`.
const PICKUP_LIFE: u32 = 3;

#[derive(Clone)]
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
    /// `Projectile.getOwner`: the add-entity packet's data, kept only when
    /// that entity was loaded at spawn (`recreateFromPacket`).
    owner: Option<i32>,
    /// `Entity.DATA_SHARED_FLAGS_ID`.
    pub shared_flags: u8,
    no_gravity: bool,
    on_ground: bool,
    /// `Entity.verticalCollision` and `verticalCollisionBelow`.
    vertical_collision: bool,
    vertical_collision_below: bool,
    /// `Entity.wasTouchingWater`, as the last tick's end left it.
    in_water: bool,
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
    /// `Projectile.leftOwner`: it has cleared its owner's vehicle and riders.
    left_owner: bool,
    /// `AbstractArrow.PIERCE_LEVEL` and the ids it has pierced
    /// (`piercingIgnoreEntityIds`).
    pierce_level: u8,
    pierced: Vec<i32>,
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
                ItemStackData::new(kind, 1),
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
            owner: None,
            shared_flags: 0,
            no_gravity: false,
            on_ground: false,
            vertical_collision: false,
            vertical_collision_below: false,
            in_water: false,
            horizontal_collision: false,
            first_tick: true,
            item,
            arrow_flags: 0,
            in_ground: false,
            shake_time: 0,
            in_ground_time: 0,
            dealt_damage: false,
            left_owner: false,
            pierce_level: 0,
            pierced: Vec::new(),
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
            let center = aabb.center();
            self.position = Position::from(center);
            self.prev_position = self.position;
            // `ItemFrame.recalculateBoundingBox` decodes later moves from
            // the centre; a painting keeps the packet's position.
            if is_item_frame(self.kind) {
                self.server_pos = center;
            }
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

    /// `HangingEntity.setDirection` / `ItemFrame.setDirection`; only a frame
    /// sets its pitch.
    fn set_direction(&mut self, direction: Direction) {
        // A painting only hangs on walls (`Validate.isTrue(horizontal)`).
        if self.kind == EntityKind::Painting && !direction.is_horizontal() {
            return;
        }
        self.direction = direction;
        if direction.is_horizontal() {
            if is_item_frame(self.kind) {
                self.x_rot = 0.0;
            }
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

    pub fn is_hanging(&self) -> bool {
        self.kind == EntityKind::Painting || is_item_frame(self.kind)
    }

    /// `AbstractArrow.setInGround`: the client's own `set`, which fires
    /// `onSyncedDataUpdated` only for a change.
    fn set_in_ground(&mut self, in_ground: bool) {
        if in_ground != self.in_ground {
            self.sync_in_ground(in_ground);
        }
    }

    /// `IN_GROUND` from the server, which fires `onSyncedDataUpdated` changed
    /// or not: a landing quivers.
    fn sync_in_ground(&mut self, in_ground: bool) {
        self.in_ground = in_ground;
        if in_ground && !self.first_tick && self.shake_time <= 0 {
            self.shake_time = SHAKE_TIME;
        }
    }

    /// `Entity.isPickable`: `BlockAttachedEntity`, and `Projectile` for a
    /// `#redirectable_projectile`.
    pub fn is_pickable(&self) -> bool {
        self.is_hanging() || tags::entities::REDIRECTABLE_PROJECTILE.contains(&self.kind)
    }

    /// The box a pick ray tests: the bounding box grown by `getPickRadius`,
    /// 1 for a pickable projectile.
    pub fn pick_box(&self) -> Option<Aabb> {
        if self.is_hanging() {
            self.bounding_box()
        } else {
            Some(self.dimensions_box(*self.position).inflate(DVec3::ONE))
        }
    }

    /// The kind's `EntityDimensions` box standing at `position`.
    pub fn dimensions_box(&self, position: DVec3) -> Aabb {
        let size = EntityDimensions::of(self.kind);
        Aabb::from_center(
            position,
            f64::from(size.width) / 2.0,
            f64::from(size.height) / 2.0,
        )
    }

    /// `Entity.setPos` after a server move; a hanging entity's `setPos`
    /// (`BlockAttachedEntity`) re-derives it from the block the point is in.
    /// `may_snap` for a position sync or teleport, which snap a jump over 64
    /// blocks where a move packet always eases.
    fn set_position(&mut self, position: Position, may_snap: bool) {
        if self.is_hanging() {
            self.block_pos = BlockPos::containing(*position);
            self.recalculate_bounding_box();
        } else if may_snap && self.position.distance_squared(*position) > 4096.0 {
            // `snapTo`.
            self.interpolation = None;
            self.prev_position = position;
            self.position = position;
        } else if self.kind == EntityKind::ExperienceOrb {
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
    /// What a projectile can hit (`canBeHitByProjectile`: alive and
    /// pickable), as the tick began.
    pub targets: &'a [HitTarget],
    /// `Entity.doTeamsAllowDamage` between two players, by id.
    pub teams_allow_damage: &'a dyn Fn(i32, i32) -> bool,
}

/// An entity a projectile can hit.
#[derive(Clone, Copy, Debug)]
pub struct HitTarget {
    pub id: i32,
    pub position: DVec3,
    pub aabb: Aabb,
    /// `getRootVehicle`'s id.
    pub root: i32,
    pub is_player: bool,
    /// Its `hurtClient` takes an arrow's hit: a hanging entity's or a
    /// vehicle's, so a non-piercing arrow goes.
    pub takes_arrow: bool,
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
        }
        self.entities.insert(id, entity);
    }

    /// `Projectile.recreateFromPacket`: the add-entity data names the owner,
    /// which the caller passes only if that entity is loaded.
    pub fn set_owner(&mut self, id: i32, owner: i32) {
        if let Some(e) = self.entities.get_mut(&id)
            && !e.is_hanging()
            && e.kind != EntityKind::ExperienceOrb
        {
            e.owner = Some(owner);
        }
    }

    pub fn contains(&self, id: i32) -> bool {
        self.entities.contains_key(&id)
    }

    /// A move packet, decoded against the server's last position
    /// (`VecDeltaCodec.decode`), not where the client has simulated it to.
    pub fn move_delta(&mut self, id: i32, delta: DVec3, on_ground: bool) {
        if let Some(e) = self.entities.get_mut(&id) {
            e.server_pos += delta;
            e.set_position(Position::from(e.server_pos), false);
            e.on_ground = on_ground;
        }
    }

    /// `handleEntityPositionSync` (no velocity) or `handleTeleportEntity`;
    /// [`Self::rotate`] follows with the rest.
    pub fn teleport(&mut self, id: i32, position: Position, velocity: Option<DVec3>) {
        if let Some(e) = self.entities.get_mut(&id) {
            e.server_pos = *position;
            e.set_position(position, true);
            if let Some(velocity) = velocity {
                e.velocity = velocity;
            }
        }
    }

    /// A move packet's rotation, which these entities snap to
    /// (`InterpolationHandler.NO_OP`), and its `setOnGround`.
    pub fn rotate(&mut self, id: i32, y_rot: f32, x_rot: f32, on_ground: bool) {
        if let Some(e) = self.entities.get_mut(&id) {
            e.on_ground = on_ground;
            if !e.is_hanging() {
                e.y_rot = y_rot;
                e.x_rot = x_rot;
            }
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
        match (index, value) {
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
            (9, MetaValue::Byte(level)) if is_arrow(e.kind) => e.pierce_level = level,
            (10, MetaValue::Bool(in_ground)) if is_arrow(e.kind) => e.sync_in_ground(in_ground),
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
        if index != slot || !holds_item {
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
    /// has one. Returns where it was, for the sound; the caller removes an
    /// arrow, while an orb waits for the server's removal, since one orb can
    /// be picked up several times.
    pub fn pickup(&mut self, id: i32, collector: Option<(i32, Position)>) -> Option<Position> {
        let entity = self.entities.get(&id)?;
        let start = entity.position;
        if let Some((collector, target)) = collector {
            self.pickups.push(NonLivingPickup {
                entity: entity.clone(),
                start,
                collector,
                prev_target: target,
                target,
                life: 0,
            });
        }
        Some(start)
    }

    /// The client tick; returns the ids it discarded (a returned trident).
    pub fn tick(&mut self, world: &TickWorld<'_>) -> Vec<i32> {
        let mut discarded = Vec::new();
        self.entities.retain(|&id, e| {
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
            let alive = !is_arrow(e.kind) || tick_arrow(e, id, world);
            match e.kind {
                kind if thrown_item_style(kind).is_some() => tick_thrown(e, id, world),
                EntityKind::FireworkRocket => tick_firework(e, world),
                EntityKind::ExperienceOrb => tick_orb(e, world),
                _ => {}
            }
            // An arrow clears it in `tick_arrow`, which a stuck one skips.
            if !is_arrow(e.kind) {
                e.first_tick = false;
            }
            if !alive {
                discarded.push(id);
            }
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
        discarded
    }
}

impl TickWorld<'_> {
    fn collision(&self) -> CollisionWorld<'_> {
        CollisionWorld::new(self.chunks, self.block_state_table)
    }
}

/// `Level.clip` with `ClipContext.Block.COLLIDER`: where the segment first
/// enters a block's collision shape, or the start if it's already inside.
fn clip_blocks(
    world: &TickWorld<'_>,
    from: DVec3,
    to: DVec3,
    ctx: &CollisionContext,
) -> Option<DVec3> {
    let collision = world.collision();
    crate::physics::collision::traverse_blocks(from, to, |pos| {
        // World-space boxes, as local ones at no offset.
        let boxes: Vec<_> = collision
            .block_boxes(pos, ctx)
            .iter()
            .map(|b| [b.min.x, b.min.y, b.min.z, b.max.x, b.max.y, b.max.z])
            .collect();
        crate::player::interaction::clip_shape(from, to, DVec3::ZERO, &boxes).map(|(hit, _)| hit)
    })
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
    let Some(owner) = e.owner.and_then(world.owner_of) else {
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
    e.velocity = e.velocity * 0.95 + crate::util::java_normalize(to_owner) * (0.05 * loyalty);
    true
}

/// `AbstractArrow.tick` as the client runs it; hits, despawning and falling
/// are server-side. Whether it lives on.
// TODO: a crit arrow's CRIT trail and the underwater bubbles.
fn tick_arrow(e: &mut NonLivingEntity, id: i32, world: &TickWorld<'_>) -> bool {
    if e.kind == EntityKind::Trident && !tick_trident_return(e, world) {
        return false;
    }
    let physics = !e.is_no_physics();
    let movement = e.velocity;
    if physics && inside_collision(world, *e.position) {
        e.velocity = DVec3::ZERO;
        e.set_in_ground(true);
    }
    if e.shake_time > 0 {
        e.shake_time -= 1;
    }
    if e.in_ground && physics {
        e.in_ground_time += 1;
        return true;
    }
    e.in_ground_time = 0;
    // `getWaterInertia`.
    let water_inertia = f64::from(if e.kind == EntityKind::Trident {
        0.99f32
    } else {
        0.6
    });
    if e.in_water {
        e.velocity *= water_inertia;
    }
    let (x_rot, mut y_rot) = movement_angles(movement);
    if !physics {
        y_rot = movement_angles(-movement).1;
    }
    e.x_rot = lerp_rotation(e.x_rot, x_rot);
    e.y_rot = lerp_rotation(e.y_rot, y_rot);
    check_left_owner(e, id, world);
    if physics {
        if !step_move_and_hit(e, id, world, movement) {
            return false;
        }
    } else {
        e.position += movement;
    }
    if !e.in_water {
        e.velocity *= f64::from(0.99f32);
    }
    if physics && !e.in_ground && !e.no_gravity {
        e.velocity.y -= 0.05;
    }
    // `super.tick()`: `baseTick`, with `AbstractArrow.isPushedByFluid`.
    update_in_water(e, world, !e.in_ground);
    e.first_tick = false;
    true
}

/// Where `movement` first enters a block's collision shape, as the
/// projectile asks (`ClipContext` with its entity).
fn block_hit(e: &NonLivingEntity, world: &TickWorld<'_>, movement: DVec3) -> Option<DVec3> {
    clip_blocks(
        world,
        *e.position,
        *e.position + movement,
        &projectile_context(e),
    )
}

/// `AbstractArrow.stepMoveAndHit` as the client runs it: to the first
/// entity on the way, or to the block it hits. Whether it lives on.
fn step_move_and_hit(
    e: &mut NonLivingEntity,
    id: i32,
    world: &TickWorld<'_>,
    movement: DVec3,
) -> bool {
    let block = block_hit(e, world, movement);
    let to = block.unwrap_or(*e.position + movement);
    loop {
        let from = *e.position;
        let mut hits = arrow_hits(e, id, world, from, to);
        hits.sort_by(|a, b| {
            let distance = |hit: &(HitTarget, DVec3)| from.distance_squared(hit.0.position);
            distance(a).total_cmp(&distance(b))
        });
        let Some(&(_, location)) = hits.first() else {
            e.position = Position::from(to);
            if block.is_some() {
                // `onHitBlock`: back off the face, stop and quiver.
                e.position -= crate::util::java_signum(movement) * f64::from(0.05f32);
                e.velocity = DVec3::ZERO;
                e.set_in_ground(true);
                e.shake_time = SHAKE_TIME;
                e.arrow_flags &= !FLAG_CRIT;
                e.pierce_level = 0;
                e.pierced.clear();
            }
            return true;
        };
        e.position = Position::from(location);
        // `hitTargetsOrDeflectSelf`.
        // TODO: an entity's own `deflection` (a breeze's, a shield's), which
        // ends the step.
        for (target, _) in &hits {
            if !arrow_hit_entity(e, target) {
                return false;
            }
        }
        if e.pierce_level == 0 {
            return true;
        }
    }
}

/// `onHitEntity` as the client runs it: damage and knockback are the
/// server's. Whether the arrow lives on.
fn arrow_hit_entity(e: &mut NonLivingEntity, target: &HitTarget) -> bool {
    if e.kind == EntityKind::Trident {
        // `ThrownTrident.onHitEntity`.
        e.dealt_damage = true;
        return true;
    }
    if e.pierce_level > 0 {
        if e.pierced.len() > usize::from(e.pierce_level) {
            return false;
        }
        e.pierced.push(target.id);
    }
    !(target.takes_arrow && e.pierce_level == 0)
}

/// `findHitEntities`: every entity the segment passes within the margin of;
/// a trident's (`ThrownTrident.findHitEntities`) is the nearest alone, and
/// none once it has dealt damage.
fn arrow_hits(
    e: &NonLivingEntity,
    id: i32,
    world: &TickWorld<'_>,
    from: DVec3,
    to: DVec3,
) -> Vec<(HitTarget, DVec3)> {
    if e.kind == EntityKind::Trident {
        return if e.dealt_damage {
            Vec::new()
        } else {
            nearest_entity_hit(e, id, world, from, to)
                .into_iter()
                .collect()
        };
    }
    // `ProjectileUtil.getManyEntityHitResult`, without the hits from inside.
    let margin = hit_margin(e);
    hit_candidates(e, id, world)
        .filter_map(|target| {
            if let Some(hit) = target.aabb.clip(from, to) {
                return Some((target, hit));
            }
            if margin <= 0.0 {
                return None;
            }
            let outside = target.aabb.inflate(DVec3::splat(margin)).clip(from, to)?;
            // Only if no block stands between the grown box and the real one.
            let center = target.aabb.center();
            let towards =
                clip_blocks(world, outside, center, &projectile_context(e)).unwrap_or(center);
            target
                .aabb
                .clip(outside, towards)
                .map(|_| (target, outside))
        })
        .collect()
}

/// `ProjectileUtil.getEntityHitResult` with the projectile's margin: the
/// nearest entity the segment passes within it of.
fn nearest_entity_hit(
    e: &NonLivingEntity,
    id: i32,
    world: &TickWorld<'_>,
    from: DVec3,
    to: DVec3,
) -> Option<(HitTarget, DVec3)> {
    let margin = DVec3::splat(hit_margin(e));
    hit_candidates(e, id, world)
        .filter_map(|target| Some((target, target.aabb.inflate(margin).clip(from, to)?)))
        .min_by(|a, b| {
            from.distance_squared(a.1)
                .total_cmp(&from.distance_squared(b.1))
        })
}

/// `ProjectileUtil.computeMargin`: a young projectile's targets don't grow.
fn hit_margin(e: &NonLivingEntity) -> f64 {
    f64::from(((e.tick_count as f32 - 2.0) / 20.0).clamp(0.0, 0.3))
}

/// The targets in the projectile's search box (its box swept by its
/// velocity, grown by 1) that `Projectile.canHitEntity` lets through: not
/// itself, nor its owner's vehicle and riders until it has left them; for an
/// arrow, not what it has pierced, nor a player its owning player's team
/// spares (`AbstractArrow.canHitEntity`).
fn hit_candidates<'w>(
    e: &'w NonLivingEntity,
    id: i32,
    world: &'w TickWorld<'_>,
) -> impl Iterator<Item = HitTarget> + 'w {
    let search = search_box(e);
    let owner_root = owner_root(e, world).filter(|_| !e.left_owner);
    let owning_player = e
        .owner
        .filter(|&owner| world.targets.iter().any(|t| t.id == owner && t.is_player));
    world.targets.iter().copied().filter(move |target| {
        let spared = is_arrow(e.kind)
            && target.is_player
            && owning_player.is_some_and(|owner| !(world.teams_allow_damage)(owner, target.id));
        target.id != id
            && target.aabb.intersects(&search)
            && Some(target.root) != owner_root
            && !e.pierced.contains(&target.id)
            && !spared
    })
}

/// Its box swept by its velocity and grown by 1: where it looks for
/// targets, and for its owner while it leaves them.
fn search_box(e: &NonLivingEntity) -> Aabb {
    e.dimensions_box(*e.position)
        .expand(e.velocity)
        .inflate(DVec3::ONE)
}

/// The root vehicle of the projectile's owner, or the owner alone when the
/// client has it unpickable.
fn owner_root(e: &NonLivingEntity, world: &TickWorld<'_>) -> Option<i32> {
    e.owner.map(|owner| {
        world
            .targets
            .iter()
            .find(|t| t.id == owner)
            .map_or(owner, |t| t.root)
    })
}

/// `Projectile.checkLeftOwner`: once nothing riding with its owner touches
/// its swept box grown by 1, it may hit them.
fn check_left_owner(e: &mut NonLivingEntity, id: i32, world: &TickWorld<'_>) {
    if e.left_owner {
        return;
    }
    let reach = search_box(e);
    e.left_owner = match owner_root(e, world) {
        None => true,
        Some(root) => !world
            .targets
            .iter()
            .any(|t| t.id != id && t.root == root && t.aabb.intersects(&reach)),
    };
}

/// `Entity.updateFluidInteraction` for water, from `baseTick`: whether the
/// box touches it, and the current that pushes a `pushed` entity.
// TODO: lava's current.
fn update_in_water(e: &mut NonLivingEntity, world: &TickWorld<'_>, pushed: bool) {
    let aabb = e.dimensions_box(*e.position);
    let water = crate::player::water_contact(world.chunks, world.block_state_table, &aabb);
    e.in_water = water.height > 0.0;
    if pushed && e.in_water {
        e.velocity += water.current_impulse(e.velocity, false);
    }
}

/// `ThrowableProjectile.tick`, `AbstractHurtingProjectile.tick` and
/// `EyeOfEnder.tick` movement; hits are server-side.
// TODO: the particles they spawn: underwater bubbles, a fireball's
// `createParticleTrail` smoke and an eye of ender's PORTAL trail.
fn tick_thrown(e: &mut NonLivingEntity, id: i32, world: &TickWorld<'_>) {
    let in_water = e.in_water;
    match e.kind {
        // `EyeOfEnder.tick` runs `baseTick` before it moves.
        EntityKind::EyeOfEnder => update_in_water(e, world, true),
        EntityKind::Fireball | EntityKind::SmallFireball => {
            // `accelerationPower` stays its default on the client.
            let inertia = f64::from(if in_water { 0.8f32 } else { 0.95 });
            e.velocity = (e.velocity + crate::util::java_normalize(e.velocity) * 0.1) * inertia;
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
            e.velocity *= f64::from(if in_water { 0.8f32 } else { 0.99 });
        }
    }
    if e.kind == EntityKind::EyeOfEnder {
        e.position += e.velocity;
        return;
    }
    // `ProjectileUtil.getHitResultOnMoveVector`: the block on the way, then
    // the nearest entity before it.
    let from = *e.position;
    let to = block_hit(e, world, e.velocity).unwrap_or(from + e.velocity);
    let to = nearest_entity_hit(e, id, world, from, to).map_or(to, |(_, hit)| hit);
    e.position = Position::from(to);
    // `super.tick()`: `Projectile.checkLeftOwner`, then `baseTick`.
    check_left_owner(e, id, world);
    update_in_water(e, world, true);
}

/// `FireworkRocketEntity.tick`'s free flight; an attached rocket isn't drawn.
// TODO: its FIREWORK spark trail.
fn tick_firework(e: &mut NonLivingEntity, world: &TickWorld<'_>) {
    // `baseTick` runs first.
    update_in_water(e, world, true);
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
    // `Entity.move` updates it only for an asked vertical move.
    if movement.y.abs() > 0.0 {
        e.on_ground = on_ground;
    }
    let (x_collision, z_collision) = horizontal_collisions(movement, delta);
    e.horizontal_collision = x_collision || z_collision;
    e.velocity = movement;
    // `Projectile.updateRotation`.
    let (x_rot, y_rot) = movement_angles(movement);
    e.x_rot = lerp_rotation(e.prev_x_rot, x_rot);
    e.y_rot = lerp_rotation(e.prev_y_rot, y_rot);
}

/// `Entity.move`'s `xCollision` and `zCollision`: the asked `movement` was
/// cut short on that axis.
fn horizontal_collisions(movement: DVec3, delta: DVec3) -> (bool, bool) {
    (
        !mth_equal(delta.x, movement.x),
        !mth_equal(delta.z, movement.z),
    )
}

/// `CollisionContext.of` for one of these entities, which never descends.
fn projectile_context(e: &NonLivingEntity) -> CollisionContext {
    CollisionContext::entity(e.position.y, false, false)
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
    let ctx = projectile_context(e);
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
    // `super.tick()`: `baseTick`.
    update_in_water(e, world, true);
    let eye = *e.position + DVec3::Y * f64::from(EntityDimensions::eye_height(e.kind));
    if crate::player::eye_in_water(world.chunks, world.block_state_table, eye) {
        // `setUnderwaterMovement`.
        e.velocity = DVec3::new(
            e.velocity.x * f64::from(0.99f32),
            (e.velocity.y + f64::from(5.0e-4f32)).min(f64::from(0.06f32)),
            e.velocity.z * f64::from(0.99f32),
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
        e.velocity += crate::util::java_normalize(delta) * (power * power * 0.1);
    }
    let fall_speed = e.velocity.y;
    let movement = e.velocity;
    let (delta, _) = move_with_collision(e, world, SIZE, movement);
    e.position += delta;
    if let Some((target, steps)) = e.interpolation {
        // `AbstractInterpolationHandler.applyPredictedMovement`.
        e.interpolation = Some((target + delta, steps));
    }
    // `Entity.move`, whose `delta` is the asked movement (`movement` here):
    // the vertical state only updates when it asked to move vertically, as
    // the client isn't authoritative for an orb.
    let moved_vertically = movement.y.abs() > 0.0;
    if moved_vertically {
        e.vertical_collision = delta.y != movement.y;
        e.vertical_collision_below = e.vertical_collision && movement.y < 0.0;
        e.on_ground = e.vertical_collision_below;
    }
    let (x_collision, z_collision) = horizontal_collisions(movement, delta);
    // `restituteMovementAfterCollisions`: an orb doesn't bounce off walls.
    // TODO: a bouncy block's restitution (`getBlockBounciness`) under it.
    if moved_vertically && e.vertical_collision || x_collision || z_collision {
        if x_collision {
            e.velocity.x = 0.0;
        }
        if z_collision {
            e.velocity.z = 0.0;
        }
        if e.vertical_collision {
            e.velocity.y = 0.0;
        }
    }
    // TODO: per-block friction (ice, slime); vanilla multiplies by the block
    // below's, 0.6 by default.
    let friction = f64::from(if e.on_ground { 0.98f32 * 0.6 } else { 0.98 });
    e.velocity *= friction;
    // `getGravity()` is 0 for a weightless orb.
    let gravity = if e.no_gravity { 0.0 } else { GRAVITY };
    if e.vertical_collision_below && fall_speed < -gravity {
        e.velocity.y = -fall_speed * 0.4;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The origin chunk with one stone block.
    fn stone_at(x: i32, y: i32, z: i32) -> ChunkStore {
        let chunks = ChunkStore::with_origin_chunk();
        let stone = BlockStateTable::native().find_state("stone", &[]);
        chunks.set_block_state(x, y, z, stone, BlockStateTable::native());
        chunks
    }

    /// A tick world on `chunks` with no players or owners.
    fn quiet_world(chunks: &ChunkStore) -> TickWorld<'_> {
        TickWorld {
            chunks,
            block_state_table: BlockStateTable::native(),
            players: &[],
            owner_of: &|_| None,
            pickup_target_of: &|_| None,
            targets: &[],
            teams_allow_damage: &|_, _| true,
        }
    }

    /// A pig-sized target standing at `x`, 64, 0.5.
    fn pig_at(id: i32, x: f64) -> HitTarget {
        let position = DVec3::new(x, 64.0, 0.5);
        HitTarget {
            id,
            position,
            aabb: Aabb::from_center(position, 0.45, 0.45),
            root: id,
            is_player: false,
            takes_arrow: false,
        }
    }

    /// A projectile `kind` at 0.5, 64.5, 0.5 heading +x at 2 blocks a
    /// tick, owned by entity 7, on its `tick`th tick.
    fn flying(kind: EntityKind, tick: u32) -> NonLivingStore {
        let mut store = spawn(kind, DVec3::new(0.5, 64.5, 0.5), 0);
        let e = store.entities.get_mut(&1).unwrap();
        e.velocity = DVec3::new(2.0, 0.0, 0.0);
        e.no_gravity = true;
        e.first_tick = false;
        e.tick_count = tick;
        e.owner = Some(7);
        store
    }

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

    /// `HangingEntity.setDirection` leaves a painting's pitch alone.
    #[test]
    fn paintings_keep_their_pitch() {
        let mut store = NonLivingStore::new();
        store.spawn(
            1,
            EntityKind::Painting,
            uuid::Uuid::nil(),
            Position::from(DVec3::ZERO),
            DVec3::ZERO,
            0.0,
            30.0,
            3,
        );
        assert_eq!(store.get(1).unwrap().x_rot, 30.0);
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
        // The server's value quivers it again, unchanged or not.
        store.entities.get_mut(&1).unwrap().shake_time = 0;
        store.apply_entity_data(1, 10, MetaValue::Bool(true));
        assert_eq!(store.get(1).unwrap().shake_time, SHAKE_TIME);
    }

    /// A block appearing around a flying arrow lands it, quivering.
    #[test]
    fn an_arrow_inside_a_block_lands_quivering() {
        let mut e = NonLivingEntity::new(
            EntityKind::Arrow,
            uuid::Uuid::nil(),
            Position::default(),
            DVec3::ZERO,
            0.0,
            0.0,
        );
        e.first_tick = false;
        e.set_in_ground(true);
        assert_eq!(e.shake_time, SHAKE_TIME);
        // Already in the ground, it doesn't quiver again.
        e.shake_time = 0;
        e.set_in_ground(true);
        assert_eq!(e.shake_time, 0);
    }

    /// `LinearInterpolationHandler` eases an orb even before its first tick.
    #[test]
    fn a_new_orb_eases_to_its_first_move() {
        let mut store = spawn(EntityKind::ExperienceOrb, DVec3::ZERO, 1);
        store.teleport(1, Position::from(DVec3::X), None);
        let orb = store.get(1).unwrap();
        assert_eq!(*orb.position, DVec3::ZERO);
        assert_eq!(orb.interpolation, Some((DVec3::X, INTERPOLATION_STEPS)));
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
                ItemStackData::new(kind, 1),
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
        store.move_delta(1, DVec3::new(0.0, 0.0, 1.5), false);
        assert_eq!(*store.get(1).unwrap().position, DVec3::new(0.0, 64.0, 1.5));
        store.teleport(1, Position::new(5.0, 64.0, 5.0), None);
        store.move_delta(1, DVec3::new(1.0, 0.0, 0.0), false);
        assert_eq!(*store.get(1).unwrap().position, DVec3::new(6.0, 64.0, 5.0));
    }

    /// `ItemFrame.recalculateBoundingBox` moves the decoding base to the
    /// frame's centre, so a move lands from there.
    #[test]
    fn item_frames_decode_moves_from_their_centre() {
        // Facing north on block (4, 70, -3): centred at x 4.5.
        let mut store = spawn(EntityKind::ItemFrame, DVec3::new(4.0, 70.0, -3.0), 2);
        store.move_delta(1, DVec3::new(0.6, 0.0, 0.0), false);
        // 4.5 + 0.6 is in block 5, where the corner's 4.6 isn't.
        assert_eq!(store.get(1).unwrap().position.x, 5.5);
    }

    /// `Projectile.recreateFromPacket` keeps an owner only for a projectile.
    #[test]
    fn only_projectiles_take_an_owner() {
        let mut store = spawn(EntityKind::Trident, DVec3::ZERO, 7);
        assert_eq!(store.get(1).unwrap().owner, None);
        store.set_owner(1, 7);
        assert_eq!(store.get(1).unwrap().owner, Some(7));
        let mut orbs = spawn(EntityKind::ExperienceOrb, DVec3::ZERO, 7);
        orbs.set_owner(1, 7);
        assert_eq!(orbs.get(1).unwrap().owner, None);
    }

    /// An orb lying on the ground stays there: its asked fall collides and
    /// zeroes every tick.
    #[test]
    fn a_resting_orb_stays_put() {
        let chunks = stone_at(8, 64, 8);
        let mut store = spawn(EntityKind::ExperienceOrb, DVec3::new(8.5, 65.0, 8.5), 1);
        store.entities.get_mut(&1).unwrap().first_tick = false;
        let world = quiet_world(&chunks);
        for _ in 0..20 {
            store.tick(&world);
            let orb = store.get(1).unwrap();
            assert_eq!(orb.position.y, 65.0);
            assert_eq!(orb.velocity.y, 0.0);
        }
    }

    /// A jump over 64 blocks snaps instead of easing.
    #[test]
    fn a_far_teleport_snaps() {
        let mut store = spawn(EntityKind::ExperienceOrb, DVec3::ZERO, 1);
        store.teleport(1, Position::new(100.0, 0.0, 0.0), None);
        let orb = store.get(1).unwrap();
        assert_eq!(orb.position.x, 100.0);
        assert_eq!(orb.prev_position.x, 100.0);
        assert_eq!(orb.interpolation, None);
    }

    /// `handleMoveEntity` eases however far the move goes.
    #[test]
    fn a_far_move_packet_still_eases() {
        let mut store = spawn(EntityKind::ExperienceOrb, DVec3::ZERO, 1);
        store.move_delta(1, DVec3::new(100.0, 0.0, 0.0), true);
        let orb = store.get(1).unwrap();
        assert_eq!(orb.position.x, 0.0);
        assert!(orb.interpolation.is_some());
        assert!(orb.on_ground);
    }

    /// The clip asks as the projectile: scaffolding is solid only from above.
    #[test]
    fn projectiles_rise_through_scaffolding() {
        let chunks = ChunkStore::with_origin_chunk();
        let table = BlockStateTable::native();
        let scaffolding =
            table.find_state("scaffolding", &[("bottom", "false"), ("distance", "0")]);
        chunks.set_block_state(8, 65, 8, scaffolding, table);
        let world = quiet_world(&chunks);
        let arrow = |y| {
            NonLivingEntity::new(
                EntityKind::Arrow,
                uuid::Uuid::nil(),
                Position::new(8.5, y, 8.5),
                DVec3::ZERO,
                0.0,
                0.0,
            )
        };
        let clip = |e: &NonLivingEntity, to: DVec3| {
            clip_blocks(&world, *e.position, to, &projectile_context(e))
        };
        // From below, it flies through.
        assert_eq!(clip(&arrow(64.0), DVec3::new(8.5, 67.0, 8.5)), None);
        // From above, it lands on top.
        assert_eq!(
            clip(&arrow(67.0), DVec3::new(8.5, 64.5, 8.5)),
            Some(DVec3::new(8.5, 66.0, 8.5))
        );
    }

    /// `ThrowableProjectile.tick` stops at the first entity on its way.
    #[test]
    fn a_snowball_stops_at_a_pig() {
        let chunks = ChunkStore::with_origin_chunk();
        let targets = [pig_at(2, 2.0)];
        let world = TickWorld {
            targets: &targets,
            ..quiet_world(&chunks)
        };
        let mut store = flying(EntityKind::Snowball, 1);
        store.tick(&world);
        // The pig's box starts at x 1.55.
        assert_eq!(store.get(1).unwrap().position.x, 1.55);
    }

    /// An arrow stops at a mob it can't hurt on the client, and is gone
    /// after hitting a frame's `hurtClient`; a piercing one passes both.
    #[test]
    fn arrows_hit_mobs_and_frames() {
        let chunks = ChunkStore::with_origin_chunk();
        let mob = [pig_at(2, 2.0)];
        let world = TickWorld {
            targets: &mob,
            ..quiet_world(&chunks)
        };
        let mut store = flying(EntityKind::Arrow, 1);
        assert!(store.tick(&world).is_empty());
        assert_eq!(store.get(1).unwrap().position.x, 1.55);

        let frame = [HitTarget {
            takes_arrow: true,
            ..pig_at(3, 2.0)
        }];
        let world = TickWorld {
            targets: &frame,
            ..quiet_world(&chunks)
        };
        let mut store = flying(EntityKind::Arrow, 1);
        assert_eq!(store.tick(&world), vec![1]);

        let mut store = flying(EntityKind::Arrow, 1);
        store.entities.get_mut(&1).unwrap().pierce_level = 1;
        assert!(store.tick(&world).is_empty());
        assert_eq!(store.get(1).unwrap().position.x, 2.5);
    }

    /// `AbstractArrow.onHitBlock` ends piercing and the crit.
    #[test]
    fn landing_ends_piercing() {
        let chunks = stone_at(1, 64, 0);
        let mut store = flying(EntityKind::Arrow, 1);
        let arrow = store.entities.get_mut(&1).unwrap();
        arrow.pierce_level = 2;
        arrow.arrow_flags = FLAG_CRIT;
        store.tick(&quiet_world(&chunks));
        let arrow = store.get(1).unwrap();
        assert!(arrow.in_ground);
        assert_eq!((arrow.pierce_level, arrow.arrow_flags), (0, 0));
    }

    /// `AbstractArrow.canHitEntity`: a teammate of the owning player is
    /// spared; a target past the swept search box isn't looked at.
    #[test]
    fn arrows_spare_teammates_and_look_only_nearby() {
        let chunks = ChunkStore::with_origin_chunk();
        let owner = HitTarget {
            is_player: true,
            ..pig_at(7, -20.0)
        };
        let mate = HitTarget {
            is_player: true,
            takes_arrow: true,
            ..pig_at(2, 2.0)
        };
        let targets = [owner, mate];
        let world = TickWorld {
            targets: &targets,
            teams_allow_damage: &|_, _| false,
            ..quiet_world(&chunks)
        };
        let mut store = flying(EntityKind::Arrow, 1);
        assert!(store.tick(&world).is_empty());
        assert_eq!(store.get(1).unwrap().position.x, 2.5);

        // Outside the box swept by its velocity and grown by 1.
        let far = [pig_at(2, 4.0)];
        let world = TickWorld {
            targets: &far,
            ..quiet_world(&chunks)
        };
        let mut store = flying(EntityKind::Arrow, 1);
        store.entities.get_mut(&1).unwrap().velocity = DVec3::new(0.5, 0.0, 0.0);
        let e = store.get(1).unwrap();
        assert!(hit_candidates(e, 1, &world).next().is_none());
    }

    /// `Projectile.canHitEntity`: the owner is passed until the projectile
    /// has left it.
    #[test]
    fn projectiles_pass_their_owner_until_they_leave_it() {
        let chunks = ChunkStore::with_origin_chunk();
        let owner = [pig_at(7, 2.0)];
        let world = TickWorld {
            targets: &owner,
            ..quiet_world(&chunks)
        };
        let mut store = flying(EntityKind::Snowball, 1);
        store.tick(&world);
        let snowball = store.get(1).unwrap();
        assert_eq!(snowball.position.x, 0.5 + 2.0 * f64::from(0.99f32));
        assert!(!snowball.left_owner);
    }

    /// `Entity.move` stops an orb along the axis it hit.
    #[test]
    fn an_orb_stops_at_a_wall() {
        let chunks = stone_at(9, 64, 8);
        let mut store = spawn(EntityKind::ExperienceOrb, DVec3::new(8.6, 64.0, 8.5), 1);
        let orb = store.entities.get_mut(&1).unwrap();
        orb.first_tick = false;
        orb.velocity = DVec3::new(0.5, 0.0, 0.0);
        let world = quiet_world(&chunks);
        store.tick(&world);
        let orb = store.get(1).unwrap();
        assert_eq!(orb.position.x, 8.75);
        assert_eq!(orb.velocity.x, 0.0);
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
        let world = quiet_world(&chunks);
        store.tick(&world);
        let arrow = store.get(1).unwrap();
        assert_eq!(arrow.tick_count, 0);
        assert!(arrow.first_tick);
    }

    /// A stuck arrow's tick returns before `baseTick`, so it stays on its
    /// first tick, and a later landing doesn't quiver.
    #[test]
    fn a_stuck_arrow_keeps_its_first_tick() {
        let chunks = stone_at(8, 64, 8);
        let mut store = spawn(EntityKind::Arrow, DVec3::new(8.5, 64.5, 8.5), 0);
        let world = quiet_world(&chunks);
        store.tick(&world);
        let arrow = store.get(1).unwrap();
        assert!(arrow.in_ground);
        assert!(arrow.first_tick);
        assert_eq!(arrow.shake_time, 0);
    }

    #[test]
    fn rotation_lerps_a_fifth_along_the_short_arc() {
        assert_eq!(lerp_rotation(0.0, 10.0), 2.0);
        // Unwrapped, as vanilla leaves it: -186 is 174.
        assert_eq!(lerp_rotation(170.0, -170.0), -186.0);
    }
}
