use std::collections::HashMap;

use azalea_block::BlockState;
use azalea_core::direction::Direction;
use azalea_core::position::BlockPos;
use azalea_protocol::packets::game::ServerboundGamePacket;
use azalea_protocol::packets::game::s_interact::InteractionHand;
use azalea_protocol::packets::game::s_player_action::{Action, ServerboundPlayerAction};
use azalea_protocol::packets::game::s_use_item_on::{BlockHit, ServerboundUseItemOn};
use glam::Vec3;

use crate::net::sender::PacketSender;
use crate::window::input::InputState;
use crate::world::chunk::ChunkStore;

const REACH: f32 = 4.5;
const STEP: f32 = 0.01;
const DESTROY_COOLDOWN: u32 = 5;
const MISS_COOLDOWN: u32 = 10;
const RIGHT_CLICK_DELAY: u32 = 4;
const SWING_DURATION: i32 = 6;

#[derive(Debug, Clone, Copy)]
pub struct HitResult {
    pub block_pos: BlockPos,
    pub face: Direction,
    pub hit_point: Vec3,
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

    fn swing(&mut self, sender: Option<&PacketSender>) {
        if !self.swinging || self.swing_time >= SWING_DURATION / 2 || self.swing_time < 0 {
            self.swing_time = -1;
            self.swinging = true;
        }
        if let Some(sender) = sender {
            send_swing(sender);
        }
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

    pub fn update_target(&mut self, eye: Vec3, yaw: f32, pitch: f32, chunks: &ChunkStore) {
        let dir = look_direction(yaw, pitch);
        self.target = raycast(eye, dir, REACH, chunks);
    }

    pub fn tick(
        &mut self,
        input: &InputState,
        chunks: &ChunkStore,
        sender: Option<&PacketSender>,
        on_ground: bool,
        creative: bool,
    ) -> Vec<azalea_core::position::ChunkPos> {
        let mut dirty_chunks = Vec::new();
        self.update_swing();

        if self.miss_time > 0 {
            self.miss_time -= 1;
        }
        if self.right_click_delay > 0 {
            self.right_click_delay -= 1;
        }

        if !input.is_cursor_captured() {
            self.stop_destroying(sender);
            return dirty_chunks;
        }

        if input.left_just_pressed() {
            self.start_attack(chunks, sender, on_ground, creative, &mut dirty_chunks);
        }

        if input.left_held() {
            self.continue_attack(chunks, sender, on_ground, creative, &mut dirty_chunks);
        } else {
            self.miss_time = 0;
            self.stop_destroying(sender);
        }

        if input.right_just_pressed() || (input.right_held() && self.right_click_delay == 0) {
            self.use_item_on(sender);
        }

        dirty_chunks
    }

    fn start_attack(
        &mut self,
        chunks: &ChunkStore,
        sender: Option<&PacketSender>,
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

        self.start_destroy_block(hit, chunks, sender, on_ground, creative, dirty_chunks);
        self.swing(sender);
    }

    fn continue_attack(
        &mut self,
        chunks: &ChunkStore,
        sender: Option<&PacketSender>,
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

        self.continue_destroy_block(hit, chunks, sender, on_ground, creative, dirty_chunks);
        self.swing(sender);
    }

    fn use_item_on(&mut self, sender: Option<&PacketSender>) {
        if self.is_destroying {
            return;
        }

        self.right_click_delay = RIGHT_CLICK_DELAY;

        let Some(hit) = self.target else {
            return;
        };

        self.swing(sender);
        self.seq += 1;
        if let Some(sender) = sender {
            sender.send(ServerboundGamePacket::UseItemOn(ServerboundUseItemOn {
                hand: InteractionHand::MainHand,
                block_hit: BlockHit {
                    block_pos: hit.block_pos,
                    direction: hit.face,
                    location: azalea_core::position::Vec3 {
                        x: hit.hit_point.x as f64,
                        y: hit.hit_point.y as f64,
                        z: hit.hit_point.z as f64,
                    },
                    inside: false,
                    world_border: false,
                },
                seq: self.seq,
            }));
        }
    }

    fn start_destroy_block(
        &mut self,
        hit: HitResult,
        chunks: &ChunkStore,
        sender: Option<&PacketSender>,
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

    fn continue_destroy_block(
        &mut self,
        hit: HitResult,
        chunks: &ChunkStore,
        sender: Option<&PacketSender>,
        on_ground: bool,
        creative: bool,
        dirty_chunks: &mut Vec<azalea_core::position::ChunkPos>,
    ) {
        if self.destroy_delay > 0 {
            self.destroy_delay -= 1;
            return;
        }

        if self.destroy_pos != hit.block_pos {
            self.start_destroy_block(hit, chunks, sender, on_ground, creative, dirty_chunks);
            return;
        }

        let state = chunks.get_block_state(hit.block_pos.x, hit.block_pos.y, hit.block_pos.z);
        if state.is_air() {
            self.is_destroying = false;
            return;
        }

        self.destroy_progress += destroy_progress(state, on_ground, creative);
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
            self.is_destroying = false;
            self.destroy_progress = 0.0;
            self.destroy_ticks = 0.0;
            self.destroy_delay = DESTROY_COOLDOWN;
        }
    }

    fn stop_destroying(&mut self, sender: Option<&PacketSender>) {
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

fn look_direction(yaw: f32, pitch: f32) -> Vec3 {
    Vec3::new(
        -yaw.sin() * pitch.cos(),
        pitch.sin(),
        -yaw.cos() * pitch.cos(),
    )
}

fn raycast(origin: Vec3, dir: Vec3, max_dist: f32, chunks: &ChunkStore) -> Option<HitResult> {
    let mut t = 0.0;
    let mut prev_block = BlockPos {
        x: i32::MAX,
        y: i32::MAX,
        z: i32::MAX,
    };

    while t <= max_dist {
        let point = origin + dir * t;
        let bx = point.x.floor() as i32;
        let by = point.y.floor() as i32;
        let bz = point.z.floor() as i32;
        let block_pos = BlockPos {
            x: bx,
            y: by,
            z: bz,
        };

        if block_pos != prev_block {
            let state = chunks.get_block_state(bx, by, bz);
            if !state.is_air() {
                let face = hit_face(origin, dir, &block_pos);
                return Some(HitResult {
                    block_pos,
                    face,
                    hit_point: point,
                });
            }
            prev_block = block_pos;
        }

        t += STEP;
    }
    None
}

fn hit_face(origin: Vec3, dir: Vec3, pos: &BlockPos) -> Direction {
    let min = Vec3::new(pos.x as f32, pos.y as f32, pos.z as f32);
    let max = min + Vec3::ONE;

    let mut best_t = f32::MAX;
    let mut best_face = Direction::Up;

    let faces: [(f32, f32, f32, Direction); 6] = [
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
    sender: Option<&PacketSender>,
    action: Action,
    pos: BlockPos,
    direction: Direction,
    seq: u32,
) {
    if let Some(sender) = sender {
        sender.send(ServerboundGamePacket::PlayerAction(
            ServerboundPlayerAction {
                action,
                pos,
                direction,
                seq,
            },
        ));
    }
}

fn send_swing(sender: &PacketSender) {
    use azalea_protocol::packets::game::s_swing::ServerboundSwing;
    sender.send(ServerboundGamePacket::Swing(ServerboundSwing {
        hand: InteractionHand::MainHand,
    }));
}
