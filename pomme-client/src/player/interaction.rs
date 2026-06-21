use std::collections::HashMap;

use azalea_block::BlockState;
use azalea_core::direction::Direction;
use azalea_core::entity_id::MinecraftEntityId;
use azalea_core::position::BlockPos;
use azalea_entity::dimensions::EntityDimensions;
use azalea_protocol::packets::game::ServerboundGamePacket;
use azalea_protocol::packets::game::s_attack::ServerboundAttack;
use azalea_protocol::packets::game::s_interact::InteractionHand;
use azalea_protocol::packets::game::s_player_action::{Action, ServerboundPlayerAction};
use azalea_protocol::packets::game::s_use_item_on::{BlockHit, ServerboundUseItemOn};
use glam::{DVec3, Vec3, dvec3};

use crate::app::input::{self, InputState};
use crate::audio::{AudioEngine, CATEGORY_BLOCKS, SoundRef};
use crate::entity::EntityStore;
use crate::entity::components::{LookDirection, Position};
use crate::net::sender::PacketSender;
use crate::physics::aabb::Aabb;
use crate::physics::collision::has_collision;
use crate::physics::movement::{PLAYER_HALF_WIDTH, PLAYER_HEIGHT};
use crate::world::block::sound::block_sounds;
use crate::world::chunk::ChunkStore;

const REACH: f32 = 4.5;
const ENTITY_REACH: f64 = 3.0;
const CREATIVE_ENTITY_REACH_BONUS: f64 = 2.0;
const DESTROY_COOLDOWN: u32 = 5;
const MISS_COOLDOWN: u32 = 10;
const USE_DELAY: u32 = 4;
const SWING_DURATION: i32 = 6;

#[derive(Debug, Clone, Copy)]
pub struct BlockHitResult {
    pub block_pos: BlockPos,
    pub face: Direction,
    pub hit_point: DVec3,
}

#[derive(Debug, Clone, Copy)]
pub struct EntityHitResult {
    pub entity_id: i32,
    pub location: DVec3,
}

#[derive(Debug, Clone, Copy)]
pub enum HitResult {
    Block(BlockHitResult),
    Entity(EntityHitResult),
}

/// Last server-known state for a locally-predicted block change, matching
/// vanilla `BlockStatePredictionHandler.ServerVerifiedState`.
struct ServerVerifiedState {
    seq: u32,
    state: BlockState,
    player_pos: DVec3,
}

pub struct InteractionState {
    pub target: Option<HitResult>,
    seq: u32,
    last_teleport_seq: u32,
    pending_predictions: HashMap<BlockPos, ServerVerifiedState>,
    is_destroying: bool,
    destroy_pos: BlockPos,
    destroy_progress: f32,
    destroy_ticks: f32,
    destroy_delay: u32,
    miss_time: u32,
    use_delay: u32,
    swinging: bool,
    swing_time: i32,
    attack_anim: f32,
    o_attack_anim: f32,
}

impl InteractionState {
    pub fn new() -> Self {
        Self {
            target: None,
            seq: 0,
            last_teleport_seq: 0,
            pending_predictions: HashMap::new(),
            is_destroying: false,
            destroy_pos: BlockPos {
                x: -1,
                y: -1,
                z: -1,
            },
            destroy_progress: 0.0,
            destroy_ticks: 0.0,
            destroy_delay: 0,
            miss_time: 0,
            use_delay: 0,
            swinging: false,
            swing_time: 0,
            attack_anim: 0.0,
            o_attack_anim: 0.0,
        }
    }

    /// Vanilla `retainKnownServerState`: an existing entry only gets its
    /// sequence bumped, since its stored state is already the server's.
    fn retain_known_server_state(&mut self, pos: BlockPos, state: BlockState, player_pos: DVec3) {
        self.pending_predictions
            .entry(pos)
            .and_modify(|v| v.seq = self.seq)
            .or_insert(ServerVerifiedState {
                seq: self.seq,
                state,
                player_pos,
            });
    }

    /// Vanilla `updateKnownServerState`: a server block update for a predicted
    /// position only refreshes the stored state. Returns true if absorbed, in
    /// which case the caller must not apply the update to the world.
    pub fn update_known_server_state(&mut self, pos: &BlockPos, state: BlockState) -> bool {
        if let Some(v) = self.pending_predictions.get_mut(pos) {
            v.state = state;
            true
        } else {
            false
        }
    }

    pub fn on_teleport(&mut self) {
        self.last_teleport_seq = self.seq;
    }

    /// Applies a predicted break locally: remembers the server state for
    /// rollback, clears the block, and plays the break effects.
    fn predict_destroy(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        player_pos: DVec3,
        chunks: &ChunkStore,
        audio: &AudioEngine,
        dirty_chunks: &mut Vec<BlockPos>,
    ) {
        self.retain_known_server_state(pos, state, player_pos);
        chunks.set_block_state(pos.x, pos.y, pos.z, BlockState::AIR);
        mark_dirty(&pos, dirty_chunks);
        play_break_sound(audio, state, pos);
        self.destroy_delay = DESTROY_COOLDOWN;
    }

    /// Vanilla `endPredictionsUpTo` + `ClientLevel.syncBlockState`: resolves
    /// every prediction up to `seq` to the server-verified state, so rejected
    /// breaks pop back instead of desyncing the world. Returns the position to
    /// snap the player back to when a restored block overlaps them.
    pub fn acknowledge(
        &mut self,
        seq: u32,
        chunks: &ChunkStore,
        player_pos: DVec3,
        dirty_chunks: &mut Vec<BlockPos>,
    ) -> Option<DVec3> {
        let snap_allowed = self.last_teleport_seq < seq;
        let player = Aabb::from_center(player_pos, PLAYER_HALF_WIDTH, PLAYER_HEIGHT / 2.0);
        // Keep the lowest block pos among overlapping reverts so the chosen snap
        // is deterministic (HashMap iteration order is not).
        let mut snap_to: Option<((i32, i32, i32), DVec3)> = None;
        self.pending_predictions.retain(|pos, verified| {
            if verified.seq > seq {
                return true;
            }
            let current = chunks.get_block_state(pos.x, pos.y, pos.z);
            if current != verified.state {
                tracing::debug!(
                    "Server did not confirm block change at {pos:?}, reverting to {:?}",
                    verified.state
                );
                chunks.set_block_state(pos.x, pos.y, pos.z, verified.state);
                mark_dirty(pos, dirty_chunks);
                // Full-cube collision, as the engine has no per-shape voxels.
                if snap_allowed && has_collision(verified.state) {
                    let block = Aabb::block(pos.x, pos.y, pos.z);
                    let key = (pos.x, pos.y, pos.z);
                    if block.intersects(&player) && snap_to.is_none_or(|(best, _)| key < best) {
                        snap_to = Some((key, verified.player_pos));
                    }
                }
            }
            false
        });
        snap_to.map(|(_, pos)| pos)
    }

    pub fn destroy_stage(&self) -> Option<(BlockPos, u32)> {
        if !self.is_destroying || self.destroy_progress <= 0.0 {
            return None;
        }
        let stage = (self.destroy_progress * 10.0) as u32;
        Some((self.destroy_pos, stage.min(9)))
    }

    pub fn get_swing_progress(&self, partial_tick: f32) -> f32 {
        let mut diff = self.attack_anim - self.o_attack_anim;
        if diff < 0.0 {
            diff += 1.0;
        }
        self.o_attack_anim + diff * partial_tick
    }

    fn swing(&mut self, sender: &PacketSender) {
        if !self.swinging || self.swing_time >= SWING_DURATION / 2 || self.swing_time < 0 {
            self.swing_time = -1;
            self.swinging = true;
        }
        send_swing(sender);
    }

    fn update_swing(&mut self) {
        self.o_attack_anim = self.attack_anim;
        if self.swinging {
            self.swing_time += 1;
            if self.swing_time >= SWING_DURATION {
                self.swing_time = 0;
                self.swinging = false;
            }
        } else {
            self.swing_time = 0;
        }
        self.attack_anim = self.swing_time as f32 / SWING_DURATION as f32;
    }

    /// Ports vanilla `LocalPlayer.pick`: block raycast first, entity ray
    /// truncated at the block hit, the entity wins only if strictly closer.
    /// An entity hit beyond entity reach is a miss, not a block fallback.
    pub fn update_target(
        &mut self,
        eye_pos: Position,
        look_dir: LookDirection,
        chunks: &ChunkStore,
        entities: &EntityStore,
        creative: bool,
    ) {
        let entity_reach = ENTITY_REACH
            + if creative {
                CREATIVE_ENTITY_REACH_BONUS
            } else {
                0.0
            };
        let max_dist = (REACH as f64).max(entity_reach);

        let from: DVec3 = eye_pos.into();
        let dir = look_dir.as_vec();
        let block_hit = raycast(from, dir, REACH, chunks);

        let block_dist_sq = block_hit
            .map(|h| h.hit_point.distance_squared(from))
            .unwrap_or(max_dist * max_dist);
        let to = from + dir.as_dvec3() * block_dist_sq.sqrt();

        if let Some(hit) = nearest_entity_hit(from, to, entities) {
            let dist_sq = hit.location.distance_squared(from);
            if dist_sq < block_dist_sq {
                self.target =
                    (dist_sq < entity_reach * entity_reach).then_some(HitResult::Entity(hit));
                return;
            }
        }

        self.target = block_hit.map(HitResult::Block);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn tick(
        &mut self,
        input: &InputState,
        chunks: &ChunkStore,
        sender: &PacketSender,
        audio: &AudioEngine,
        player_pos: DVec3,
        on_ground: bool,
        creative: bool,
        place_block: Option<BlockState>,
    ) -> Vec<BlockPos> {
        let mut dirty_chunks = Vec::new();

        // Vanilla `Minecraft.tick` order: attack/use input (which triggers the
        // swing) runs first, then `--missTime`, then the player entity advances
        // `updateSwingTime`. Running `update_swing` last keeps the swing
        // animation cadence in lockstep with vanilla.
        if !input.is_cursor_captured() {
            self.stop_destroying(sender);
            self.update_swing();
            return dirty_chunks;
        }

        if input.action_just_pressed(input::Action::Destroy) {
            self.start_attack(
                chunks,
                sender,
                audio,
                player_pos,
                on_ground,
                creative,
                &mut dirty_chunks,
            );
        }

        if input.performing_action(input::Action::Destroy) {
            self.continue_attack(
                chunks,
                sender,
                audio,
                player_pos,
                on_ground,
                creative,
                &mut dirty_chunks,
            );
        } else {
            self.miss_time = 0;
            self.stop_destroying(sender);
        }

        if input.action_just_pressed(input::Action::Use)
            || (input.performing_action(input::Action::Use) && self.use_delay == 0)
        {
            self.use_item_on(sender, chunks, player_pos, place_block, &mut dirty_chunks);
        }

        if self.miss_time > 0 {
            self.miss_time -= 1;
        }
        if self.use_delay > 0 {
            self.use_delay -= 1;
        }
        self.update_swing();

        dirty_chunks
    }

    #[allow(clippy::too_many_arguments)]
    fn start_attack(
        &mut self,
        chunks: &ChunkStore,
        sender: &PacketSender,
        audio: &AudioEngine,
        player_pos: DVec3,
        on_ground: bool,
        creative: bool,
        dirty_chunks: &mut Vec<BlockPos>,
    ) {
        if self.miss_time > 0 {
            return;
        }

        let hit = match self.target {
            None => {
                self.miss_time = MISS_COOLDOWN;
                self.swing(sender);
                return;
            }
            Some(HitResult::Entity(hit)) => {
                sender.send(ServerboundGamePacket::Attack(ServerboundAttack {
                    entity_id: MinecraftEntityId(hit.entity_id),
                }));
                self.swing(sender);
                return;
            }
            Some(HitResult::Block(hit)) => hit,
        };

        let state = chunks.get_block_state(hit.block_pos.x, hit.block_pos.y, hit.block_pos.z);
        if state.is_air() {
            self.miss_time = MISS_COOLDOWN;
            self.swing(sender);
            return;
        }

        self.start_destroy_block(
            hit,
            chunks,
            sender,
            audio,
            player_pos,
            on_ground,
            creative,
            dirty_chunks,
        );
        self.swing(sender);
    }

    #[allow(clippy::too_many_arguments)]
    fn continue_attack(
        &mut self,
        chunks: &ChunkStore,
        sender: &PacketSender,
        audio: &AudioEngine,
        player_pos: DVec3,
        on_ground: bool,
        creative: bool,
        dirty_chunks: &mut Vec<BlockPos>,
    ) {
        if self.miss_time > 0 {
            return;
        }

        // Vanilla `continueAttack` only mines blocks; holding the button over
        // an entity does not re-attack it.
        let Some(HitResult::Block(hit)) = self.target else {
            self.stop_destroying(sender);
            return;
        };

        let state = chunks.get_block_state(hit.block_pos.x, hit.block_pos.y, hit.block_pos.z);
        if state.is_air() {
            self.stop_destroying(sender);
            return;
        }

        self.continue_destroy_block(
            hit,
            chunks,
            sender,
            audio,
            player_pos,
            on_ground,
            creative,
            dirty_chunks,
        );
        self.swing(sender);
    }

    fn use_item_on(
        &mut self,
        sender: &PacketSender,
        chunks: &ChunkStore,
        player_pos: DVec3,
        place_block: Option<BlockState>,
        dirty_chunks: &mut Vec<BlockPos>,
    ) {
        if self.is_destroying {
            return;
        }

        self.use_delay = USE_DELAY;

        let Some(HitResult::Block(hit)) = self.target else {
            return;
        };

        self.swing(sender);
        self.seq += 1;

        sender.send(ServerboundGamePacket::UseItemOn(ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            block_hit: BlockHit {
                block_pos: hit.block_pos,
                direction: hit.face,
                location: azalea_vec3(hit.hit_point),
                inside: false,
                world_border: false,
            },
            seq: self.seq,
        }));

        self.predict_place(hit, place_block, chunks, player_pos, dirty_chunks);
    }

    /// Predicts placement locally for unambiguous single-state blocks,
    /// mirroring `predict_destroy`: stores air for rollback, writes the
    /// block, and marks it for remesh. `acknowledge` reverts it if the
    /// server doesn't confirm. Skips anything not clearly placeable so the
    /// worst case is just no prediction.
    fn predict_place(
        &mut self,
        hit: BlockHitResult,
        place_block: Option<BlockState>,
        chunks: &ChunkStore,
        player_pos: DVec3,
        dirty_chunks: &mut Vec<BlockPos>,
    ) {
        let Some(state) = place_block else {
            return;
        };
        let pos = hit.block_pos.offset_with_direction(hit.face);

        // Only predict into an empty cell; replacing grass/water isn't handled yet.
        if !chunks.get_block_state(pos.x, pos.y, pos.z).is_air() {
            return;
        }

        // Don't predict a solid block overlapping the player; the server denies it.
        if has_collision(state) {
            let player = Aabb::from_center(player_pos, PLAYER_HALF_WIDTH, PLAYER_HEIGHT / 2.0);
            if Aabb::block(pos.x, pos.y, pos.z).intersects(&player) {
                return;
            }
        }

        self.retain_known_server_state(pos, BlockState::AIR, player_pos);
        chunks.set_block_state(pos.x, pos.y, pos.z, state);
        mark_dirty(&pos, dirty_chunks);
    }

    #[allow(clippy::too_many_arguments)]
    fn start_destroy_block(
        &mut self,
        hit: BlockHitResult,
        chunks: &ChunkStore,
        sender: &PacketSender,
        audio: &AudioEngine,
        player_pos: DVec3,
        on_ground: bool,
        creative: bool,
        dirty_chunks: &mut Vec<BlockPos>,
    ) {
        let state = chunks.get_block_state(hit.block_pos.x, hit.block_pos.y, hit.block_pos.z);

        if state.is_air() {
            return;
        }

        let progress = destroy_progress(state, on_ground, creative);

        if progress >= 1.0 {
            if self.is_destroying {
                send_action(
                    sender,
                    Action::AbortDestroyBlock,
                    self.destroy_pos,
                    Direction::Down,
                    0,
                );
                self.is_destroying = false;
            }
            self.seq += 1;
            let seq = self.seq;
            send_action(
                sender,
                Action::StartDestroyBlock,
                hit.block_pos,
                hit.face,
                seq,
            );
            self.predict_destroy(
                hit.block_pos,
                state,
                player_pos,
                chunks,
                audio,
                dirty_chunks,
            );
            return;
        }

        if self.is_destroying && self.destroy_pos == hit.block_pos {
            return;
        }

        if self.is_destroying {
            send_action(
                sender,
                Action::AbortDestroyBlock,
                self.destroy_pos,
                hit.face,
                0,
            );
        }

        self.seq += 1;
        let seq = self.seq;
        send_action(
            sender,
            Action::StartDestroyBlock,
            hit.block_pos,
            hit.face,
            seq,
        );

        self.is_destroying = true;
        self.destroy_pos = hit.block_pos;
        self.destroy_progress = 0.0;
        self.destroy_ticks = 0.0;
    }

    #[allow(clippy::too_many_arguments)]
    fn continue_destroy_block(
        &mut self,
        hit: BlockHitResult,
        chunks: &ChunkStore,
        sender: &PacketSender,
        audio: &AudioEngine,
        player_pos: DVec3,
        on_ground: bool,
        creative: bool,
        dirty_chunks: &mut Vec<BlockPos>,
    ) {
        if self.destroy_delay > 0 {
            self.destroy_delay -= 1;
            return;
        }

        if self.destroy_pos != hit.block_pos {
            self.start_destroy_block(
                hit,
                chunks,
                sender,
                audio,
                player_pos,
                on_ground,
                creative,
                dirty_chunks,
            );
            return;
        }

        let state = chunks.get_block_state(hit.block_pos.x, hit.block_pos.y, hit.block_pos.z);
        if state.is_air() {
            self.is_destroying = false;
            return;
        }

        self.destroy_progress += destroy_progress(state, on_ground, creative);
        if self.destroy_ticks % 4.0 == 0.0 {
            play_hit_sound(audio, state, hit.block_pos);
        }
        self.destroy_ticks += 1.0;

        if self.destroy_progress >= 1.0 {
            self.seq += 1;
            let seq = self.seq;
            send_action(
                sender,
                Action::StopDestroyBlock,
                hit.block_pos,
                hit.face,
                seq,
            );
            self.predict_destroy(
                hit.block_pos,
                state,
                player_pos,
                chunks,
                audio,
                dirty_chunks,
            );
            self.is_destroying = false;
            self.destroy_progress = 0.0;
            self.destroy_ticks = 0.0;
        }
    }

    fn stop_destroying(&mut self, sender: &PacketSender) {
        if self.is_destroying {
            send_action(
                sender,
                Action::AbortDestroyBlock,
                self.destroy_pos,
                Direction::Down,
                0,
            );
            self.is_destroying = false;
            self.destroy_progress = 0.0;
        }
    }
}

fn destroy_progress(state: BlockState, on_ground: bool, creative: bool) -> f32 {
    if creative {
        return 1.0;
    }
    use azalea_block::BlockTrait;
    let behavior = Box::<dyn BlockTrait>::from(state).behavior();
    let hardness = behavior.destroy_time;

    if hardness < 0.0 {
        return 0.0;
    }
    if hardness == 0.0 {
        return 1.0;
    }

    let mut speed = 1.0_f32;
    if !on_ground {
        speed /= 5.0;
    }

    let divisor = if behavior.requires_correct_tool_for_drops {
        100.0
    } else {
        30.0
    };
    speed / hardness / divisor
}

/// Plays a block's mining hit sound, matching vanilla
/// `MultiPlayerGameMode.continueDestroyBlock`: volume `(volume + 1) / 8`, pitch
/// `pitch * 0.5`.
fn play_hit_sound(audio: &AudioEngine, state: BlockState, pos: BlockPos) {
    let s = block_sounds(state);
    play_block_sound(
        audio,
        &s.hit_event,
        pos,
        (s.volume + 1.0) / 8.0,
        s.pitch * 0.5,
    );
}

/// Plays a block's break sound, matching vanilla `LevelEventHandler` event
/// 2001: volume `(volume + 1) / 2`, pitch `pitch * 0.8`.
fn play_break_sound(audio: &AudioEngine, state: BlockState, pos: BlockPos) {
    let s = block_sounds(state);
    play_block_sound(
        audio,
        &s.break_event,
        pos,
        (s.volume + 1.0) / 2.0,
        s.pitch * 0.8,
    );
}

/// Plays a block sound event at the block centre in the BLOCKS category with a
/// random variant. No-op for an empty event (a silent `SoundType` slot).
fn play_block_sound(audio: &AudioEngine, event: &str, pos: BlockPos, volume: f32, pitch: f32) {
    if event.is_empty() {
        return;
    }
    audio.play_world_sound(
        &SoundRef::Event(event.to_string()),
        CATEGORY_BLOCKS,
        Position::new(pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5),
        volume,
        pitch,
        fastrand::u64(..),
    );
}

/// Record an edited block. The caller (`core::dirty_sections_for_block`)
/// expands it into the affected 16³ sections, including neighbour
/// sections/columns when the block is on a boundary.
fn mark_dirty(pos: &BlockPos, dirty: &mut Vec<BlockPos>) {
    if !dirty.contains(pos) {
        dirty.push(*pos);
    }
}

pub fn raycast(
    origin: DVec3,
    dir: Vec3,
    max_dist: f32,
    chunks: &ChunkStore,
) -> Option<BlockHitResult> {
    let dir = dir.as_dvec3();
    let mut bx = origin.x.floor() as i32;
    let mut by = origin.y.floor() as i32;
    let mut bz = origin.z.floor() as i32;

    let step_x = if dir.x > 0.0 { 1 } else { -1 };
    let step_y = if dir.y > 0.0 { 1 } else { -1 };
    let step_z = if dir.z > 0.0 { 1 } else { -1 };

    let t_delta_x = if dir.x != 0.0 {
        (1.0 / dir.x).abs()
    } else {
        f64::INFINITY
    };
    let t_delta_y = if dir.y != 0.0 {
        (1.0 / dir.y).abs()
    } else {
        f64::INFINITY
    };
    let t_delta_z = if dir.z != 0.0 {
        (1.0 / dir.z).abs()
    } else {
        f64::INFINITY
    };

    let mut t_max_x = if dir.x > 0.0 {
        (bx as f64 + 1.0 - origin.x) * t_delta_x
    } else {
        (origin.x - bx as f64) * t_delta_x
    };
    let mut t_max_y = if dir.y > 0.0 {
        (by as f64 + 1.0 - origin.y) * t_delta_y
    } else {
        (origin.y - by as f64) * t_delta_y
    };
    let mut t_max_z = if dir.z > 0.0 {
        (bz as f64 + 1.0 - origin.z) * t_delta_z
    } else {
        (origin.z - bz as f64) * t_delta_z
    };

    let mut t = 0.0_f64;
    while t <= max_dist as f64 {
        let state = chunks.get_block_state(bx, by, bz);
        if !state.is_air() {
            let block_pos = BlockPos {
                x: bx,
                y: by,
                z: bz,
            };
            let hit_point = origin + dir * t;
            let face = hit_face(origin, dir.as_vec3(), &block_pos);
            return Some(BlockHitResult {
                block_pos,
                face,
                hit_point,
            });
        }
        if t_max_x < t_max_y && t_max_x < t_max_z {
            t = t_max_x;
            t_max_x += t_delta_x;
            bx += step_x;
        } else if t_max_y < t_max_z {
            t = t_max_y;
            t_max_y += t_delta_y;
            by += step_y;
        } else {
            t = t_max_z;
            t_max_z += t_delta_z;
            bz += step_z;
        }
    }
    None
}

/// Ports vanilla `ProjectileUtil.getEntityHitResult`: clips the ray against
/// each entity's bounding box and keeps the nearest hit. A box containing the
/// ray origin counts as distance zero.
fn nearest_entity_hit(from: DVec3, to: DVec3, entities: &EntityStore) -> Option<EntityHitResult> {
    let from_v = azalea_vec3(from);
    let to_v = azalea_vec3(to);

    let mut nearest_dist_sq = f64::MAX;
    let mut nearest = None;
    for (&entity_id, entity) in &entities.living {
        let mut dims = EntityDimensions::from(entity.entity_type);
        if entity.is_baby {
            dims.width *= 0.5;
            dims.height *= 0.5;
        }
        let aabb = dims.make_bounding_box(entity.position.into());

        let (location, dist_sq) = if aabb.contains(from_v) {
            (from, 0.0)
        } else if let Some(clip) = aabb.clip(from_v, to_v) {
            let clip = DVec3::new(clip.x, clip.y, clip.z);
            (clip, clip.distance_squared(from))
        } else {
            continue;
        };

        if dist_sq < nearest_dist_sq {
            nearest_dist_sq = dist_sq;
            nearest = Some(EntityHitResult {
                entity_id,
                location,
            });
        }
    }
    nearest
}

fn azalea_vec3(v: DVec3) -> azalea_core::position::Vec3 {
    azalea_core::position::Vec3::new(v.x, v.y, v.z)
}

fn hit_face(origin: DVec3, dir: Vec3, pos: &BlockPos) -> Direction {
    let dir = dir.as_dvec3();
    let min = dvec3(pos.x as f64, pos.y as f64, pos.z as f64);
    let max = min + DVec3::ONE;

    let mut best_t = f64::MAX;
    let mut best_face = Direction::Up;

    let faces = [
        (min.x, dir.x, origin.x, Direction::West),
        (max.x, dir.x, origin.x, Direction::East),
        (min.y, dir.y, origin.y, Direction::Down),
        (max.y, dir.y, origin.y, Direction::Up),
        (min.z, dir.z, origin.z, Direction::North),
        (max.z, dir.z, origin.z, Direction::South),
    ];

    for &(plane, d_comp, o_comp, face) in &faces {
        if d_comp.abs() < 1e-8 {
            continue;
        }
        let t = (plane - o_comp) / d_comp;
        if t < 0.0 || t >= best_t {
            continue;
        }
        let hit = origin + dir * t;
        let (c1, c2, c1_min, c1_max, c2_min, c2_max) = match face {
            Direction::West | Direction::East => (hit.y, hit.z, min.y, max.y, min.z, max.z),
            Direction::Down | Direction::Up => (hit.x, hit.z, min.x, max.x, min.z, max.z),
            Direction::North | Direction::South => (hit.x, hit.y, min.x, max.x, min.y, max.y),
        };
        if c1 >= c1_min && c1 <= c1_max && c2 >= c2_min && c2 <= c2_max {
            best_t = t;
            best_face = face;
        }
    }

    best_face
}

fn send_action(
    sender: &PacketSender,
    action: Action,
    pos: BlockPos,
    direction: Direction,
    seq: u32,
) {
    sender.send(ServerboundGamePacket::PlayerAction(
        ServerboundPlayerAction {
            action,
            pos,
            direction,
            seq,
        },
    ));
}

fn send_swing(sender: &PacketSender) {
    use azalea_protocol::packets::game::s_swing::ServerboundSwing;
    sender.send(ServerboundGamePacket::Swing(ServerboundSwing {
        hand: InteractionHand::MainHand,
    }));
}
