use std::collections::HashMap;

use azalea_block::BlockState;
use azalea_core::direction::Direction;
use azalea_core::position::BlockPos;
use azalea_protocol::packets::game::ServerboundGamePacket;
use azalea_protocol::packets::game::s_interact::InteractionHand;
use azalea_protocol::packets::game::s_player_action::{Action, ServerboundPlayerAction};
use azalea_protocol::packets::game::s_use_item_on::{BlockHit, ServerboundUseItemOn};
use glam::{DVec3, Vec3, dvec3};

use crate::app::input::InputState;
use crate::audio::{AudioEngine, CATEGORY_BLOCKS, SoundRef};
use crate::entity::components::{LookDirection, Position};
use crate::net::sender::PacketSender;
use crate::world::block::sound::block_sounds;
use crate::world::chunk::ChunkStore;

const REACH: f32 = 4.5;
const DESTROY_COOLDOWN: u32 = 5;
const MISS_COOLDOWN: u32 = 10;
const RIGHT_CLICK_DELAY: u32 = 4;
const SWING_DURATION: i32 = 6;

#[derive(Debug, Clone, Copy)]
pub struct HitResult {
    pub block_pos: BlockPos,
    pub face: Direction,
    pub hit_point: DVec3,
}

pub struct InteractionState {
    pub target: Option<HitResult>,
    seq: u32,
    pending_predictions: HashMap<BlockPos, u32>,
    is_destroying: bool,
    destroy_pos: BlockPos,
    destroy_progress: f32,
    destroy_ticks: f32,
    destroy_delay: u32,
    miss_time: u32,
    right_click_delay: u32,
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
            right_click_delay: 0,
            swinging: false,
            swing_time: 0,
            attack_anim: 0.0,
            o_attack_anim: 0.0,
        }
    }

    pub fn has_pending_prediction(&self, pos: &BlockPos) -> bool {
        self.pending_predictions.contains_key(pos)
    }

    pub fn acknowledge(&mut self, seq: u32) {
        self.pending_predictions.retain(|_, &mut s| s > seq);
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

    pub fn update_target(
        &mut self,
        eye_pos: Position,
        look_dir: LookDirection,
        chunks: &ChunkStore,
    ) {
        self.target = raycast(eye_pos.into(), look_dir.as_vec(), REACH, chunks);
    }

    pub fn tick(
        &mut self,
        input: &InputState,
        chunks: &ChunkStore,
        sender: &PacketSender,
        audio: &AudioEngine,
        on_ground: bool,
        creative: bool,
    ) -> Vec<azalea_core::position::ChunkPos> {
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

        if input.left_just_pressed() {
            self.start_attack(
                chunks,
                sender,
                audio,
                on_ground,
                creative,
                &mut dirty_chunks,
            );
        }

        if input.left_held() {
            self.continue_attack(
                chunks,
                sender,
                audio,
                on_ground,
                creative,
                &mut dirty_chunks,
            );
        } else {
            self.miss_time = 0;
            self.stop_destroying(sender);
        }

        if input.right_just_pressed() || (input.right_held() && self.right_click_delay == 0) {
            self.use_item_on(sender);
        }

        if self.miss_time > 0 {
            self.miss_time -= 1;
        }
        if self.right_click_delay > 0 {
            self.right_click_delay -= 1;
        }
        self.update_swing();

        dirty_chunks
    }

    fn start_attack(
        &mut self,
        chunks: &ChunkStore,
        sender: &PacketSender,
        audio: &AudioEngine,
        on_ground: bool,
        creative: bool,
        dirty_chunks: &mut Vec<azalea_core::position::ChunkPos>,
    ) {
        if self.miss_time > 0 {
            return;
        }

        let Some(hit) = self.target else {
            self.miss_time = MISS_COOLDOWN;
            self.swing(sender);
            return;
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
            on_ground,
            creative,
            dirty_chunks,
        );
        self.swing(sender);
    }

    fn continue_attack(
        &mut self,
        chunks: &ChunkStore,
        sender: &PacketSender,
        audio: &AudioEngine,
        on_ground: bool,
        creative: bool,
        dirty_chunks: &mut Vec<azalea_core::position::ChunkPos>,
    ) {
        if self.miss_time > 0 {
            return;
        }

        let Some(hit) = self.target else {
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
            on_ground,
            creative,
            dirty_chunks,
        );
        self.swing(sender);
    }

    fn use_item_on(&mut self, sender: &PacketSender) {
        if self.is_destroying {
            return;
        }

        self.right_click_delay = RIGHT_CLICK_DELAY;

        let Some(hit) = self.target else {
            return;
        };

        self.swing(sender);
        self.seq += 1;

        sender.send(ServerboundGamePacket::UseItemOn(ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            block_hit: BlockHit {
                block_pos: hit.block_pos,
                direction: hit.face,
                location: azalea_core::position::Vec3 {
                    x: hit.hit_point.x,
                    y: hit.hit_point.y,
                    z: hit.hit_point.z,
                },
                inside: false,
                world_border: false,
            },
            seq: self.seq,
        }));
    }

    #[allow(clippy::too_many_arguments)]
    fn start_destroy_block(
        &mut self,
        hit: HitResult,
        chunks: &ChunkStore,
        sender: &PacketSender,
        audio: &AudioEngine,
        on_ground: bool,
        creative: bool,
        dirty_chunks: &mut Vec<azalea_core::position::ChunkPos>,
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
            chunks.set_block_state(
                hit.block_pos.x,
                hit.block_pos.y,
                hit.block_pos.z,
                BlockState::AIR,
            );
            self.pending_predictions.insert(hit.block_pos, seq);
            mark_dirty(&hit.block_pos, dirty_chunks);
            play_break_sound(audio, state, hit.block_pos);
            self.destroy_delay = DESTROY_COOLDOWN;
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
        hit: HitResult,
        chunks: &ChunkStore,
        sender: &PacketSender,
        audio: &AudioEngine,
        on_ground: bool,
        creative: bool,
        dirty_chunks: &mut Vec<azalea_core::position::ChunkPos>,
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
            chunks.set_block_state(
                hit.block_pos.x,
                hit.block_pos.y,
                hit.block_pos.z,
                BlockState::AIR,
            );
            self.pending_predictions.insert(hit.block_pos, seq);
            mark_dirty(&hit.block_pos, dirty_chunks);
            play_break_sound(audio, state, hit.block_pos);
            self.is_destroying = false;
            self.destroy_progress = 0.0;
            self.destroy_ticks = 0.0;
            self.destroy_delay = DESTROY_COOLDOWN;
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

fn mark_dirty(pos: &BlockPos, dirty: &mut Vec<azalea_core::position::ChunkPos>) {
    let chunk_pos =
        azalea_core::position::ChunkPos::new(pos.x.div_euclid(16), pos.z.div_euclid(16));
    if !dirty.contains(&chunk_pos) {
        dirty.push(chunk_pos);
    }

    let local_x = pos.x.rem_euclid(16);
    let local_z = pos.z.rem_euclid(16);
    let neighbors = [
        (local_x == 0, -1, 0),
        (local_x == 15, 1, 0),
        (local_z == 0, 0, -1),
        (local_z == 15, 0, 1),
    ];
    for (on_edge, dx, dz) in neighbors {
        if on_edge {
            let np = azalea_core::position::ChunkPos::new(chunk_pos.x + dx, chunk_pos.z + dz);
            if !dirty.contains(&np) {
                dirty.push(np);
            }
        }
    }
}

pub fn raycast(origin: DVec3, dir: Vec3, max_dist: f32, chunks: &ChunkStore) -> Option<HitResult> {
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
            return Some(HitResult {
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
