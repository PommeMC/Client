//! Vanilla pistons on the client (26.2): `PistonBaseBlock.triggerEvent` and
//! `moveBlocks`, `PistonStructureResolver`, and `PistonMovingBlockEntity`.
//! Vanilla's client runs these itself when a block event arrives: the server
//! places its `moving_piston` blocks with flags that skip clients (276/324),
//! so only the finished blocks ever come over the network.

use std::collections::HashMap;

use azalea_block::BlockState;
use simdnbt::owned::NbtCompound;

use crate::position::BlockPos;
use crate::world::block::model::Direction;
use crate::world::block::{self, PushReaction};

/// Vanilla `PistonStructureResolver.MAX_PUSH_DEPTH`.
const MAX_PUSH_DEPTH: usize = 12;
/// Client-side ticks a finished moving block lingers (`deathTicks`).
const DEATH_TICKS: u8 = 5;

/// The world the piston code reads and writes.
pub trait PistonLevel {
    fn block(&self, pos: BlockPos) -> BlockState;
    fn set_block(&mut self, pos: BlockPos, state: BlockState);
    /// Whether `pos`'s chunk is loaded, so it can hold a block entity.
    fn is_loaded(&self, pos: BlockPos) -> bool;
    fn min_y(&self) -> i32;
    fn max_y(&self) -> i32;
    fn pistons(&mut self) -> &mut MovingPistons;
    /// Vanilla level event 2001, raised client-side for a destroyed block:
    /// its break particles and sound.
    fn destroy_effect(&mut self, pos: BlockPos, state: BlockState);
}

/// Vanilla `PistonMovingBlockEntity`: a block on its way between two cells.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MovingPiston {
    pub moved_state: BlockState,
    pub direction: Direction,
    pub extending: bool,
    pub source: bool,
    pub progress: f32,
    pub progress_o: f32,
    death_ticks: u8,
}

impl MovingPiston {
    pub(crate) fn new(
        moved_state: BlockState,
        direction: Direction,
        extending: bool,
        source: bool,
    ) -> Self {
        Self {
            moved_state,
            direction,
            extending,
            source,
            progress: 0.0,
            progress_o: 0.0,
            death_ticks: 0,
        }
    }

    /// Vanilla `loadAdditional`, for moving blocks a chunk arrives with.
    pub fn from_nbt(nbt: &NbtCompound) -> Self {
        let moved_state = nbt
            .compound("blockState")
            .and_then(state_from_nbt)
            .unwrap_or(BlockState::AIR);
        let direction = nbt
            .byte("facing")
            .map(|id| from_3d_data_value(id.into()))
            .unwrap_or(Direction::Down);
        let progress = nbt.float("progress").unwrap_or(0.0);
        let flag = |key| nbt.byte(key).is_some_and(|b| b != 0);
        Self {
            progress,
            progress_o: progress,
            ..Self::new(moved_state, direction, flag("extending"), flag("source"))
        }
    }

    /// Vanilla `getProgress`.
    pub fn progress(&self, partial_tick: f32) -> f32 {
        let a = partial_tick.min(1.0);
        self.progress_o + (self.progress - self.progress_o) * a
    }

    /// Vanilla `getExtendedProgress`.
    pub fn extended_progress(&self, progress: f32) -> f32 {
        if self.extending {
            progress - 1.0
        } else {
            1.0 - progress
        }
    }

    /// Vanilla `getXOff`/`getYOff`/`getZOff`.
    pub fn offset(&self, partial_tick: f32) -> [f32; 3] {
        let amount = self.extended_progress(self.progress(partial_tick));
        self.direction.offset().map(|step| step as f32 * amount)
    }

    /// Vanilla `moveByPositionAndProgress`'s offset: where the block has got
    /// to, relative to its cell.
    pub fn extension(&self) -> glam::DVec3 {
        glam::Vec3::from(self.offset(1.0)).as_dvec3()
    }

    /// Vanilla `getMovementDirection`.
    pub fn movement_direction(&self) -> Direction {
        if self.extending {
            self.direction
        } else {
            self.direction.opposite()
        }
    }
}

/// The client's moving blocks, by position. Each keeps the order it was
/// added in, which they tick in (vanilla's `blockEntityTickers` list).
#[derive(Default)]
pub struct MovingPistons {
    entries: HashMap<BlockPos, (u64, MovingPiston)>,
    next_order: u64,
}

impl MovingPistons {
    pub fn get(&self, pos: &BlockPos) -> Option<&MovingPiston> {
        self.entries.get(pos).map(|(_, piston)| piston)
    }

    fn get_mut(&mut self, pos: &BlockPos) -> Option<&mut MovingPiston> {
        self.entries.get_mut(pos).map(|(_, piston)| piston)
    }

    pub fn insert(&mut self, pos: BlockPos, piston: MovingPiston) {
        self.entries.insert(pos, (self.next_order, piston));
        self.next_order += 1;
    }

    pub fn remove(&mut self, pos: &BlockPos) -> Option<MovingPiston> {
        self.entries.remove(pos).map(|(_, piston)| piston)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&BlockPos, &MovingPiston)> {
        self.entries.iter().map(|(pos, (_, piston))| (pos, piston))
    }

    /// Positions in the order they were added.
    fn tick_order(&self) -> Vec<BlockPos> {
        let mut order: Vec<_> = self
            .entries
            .iter()
            .map(|(pos, (n, _))| (*n, *pos))
            .collect();
        order.sort_unstable_by_key(|&(n, _)| n);
        order.into_iter().map(|(_, pos)| pos).collect()
    }

    pub fn drop_chunk(&mut self, chunk_x: i32, chunk_z: i32) {
        self.entries
            .retain(|p, _| p.x.div_euclid(16) != chunk_x || p.z.div_euclid(16) != chunk_z);
    }
}

/// Vanilla `PistonBaseBlock.triggerEvent`, client side: `b0` 0 extends, 1
/// retracts and pulls, 2 retracts without pulling; `b1` is the facing's 3D
/// data value.
pub fn trigger_event(level: &mut impl PistonLevel, pos: BlockPos, b0: u8, b1: u8) -> bool {
    let state = level.block(pos);
    let sticky = match block::block_id(state) {
        "piston" => false,
        "sticky_piston" => true,
        _ => return false,
    };
    let Some(direction) = facing(state) else {
        return false;
    };
    if b0 == 0 {
        if !move_blocks(level, pos, direction, true, sticky) {
            return false;
        }
        let extended = block::with_property(state, "extended", "true").unwrap_or(state);
        level.set_block(pos, extended);
        return true;
    }
    if b0 != 1 && b0 != 2 {
        return true;
    }
    let arm = relative(pos, direction, 1);
    if level.pistons().get(&arm).is_some() {
        final_tick(level, arm);
    }
    let base = piston_base(sticky, from_3d_data_value((b1 & 7).into()));
    set_moving(
        level,
        pos,
        moving_piston(direction, sticky),
        MovingPiston::new(base, direction, false, true),
    );
    if sticky {
        let two = relative(pos, direction, 2);
        let moving = level.block(two);
        let mut piston_piece = false;
        if is(moving, "moving_piston")
            && let Some(entity) = level.pistons().get(&two)
            && entity.direction == direction
            && entity.extending
        {
            final_tick(level, two);
            piston_piece = true;
        }
        if !piston_piece {
            if b0 == 1
                && !block::is_air(moving)
                && is_pushable(level, moving, two, direction.opposite(), false, direction)
                && (block::block_behavior(moving).push_reaction == PushReaction::Normal
                    || is_piston_base(moving))
            {
                move_blocks(level, pos, direction, false, sticky);
            } else {
                remove_block(level, arm);
            }
        }
    } else {
        remove_block(level, arm);
    }
    true
}

/// Vanilla `PistonBaseBlock.moveBlocks`, minus the server-side drops, the
/// neighbour shape updates and the redstone updates, whose results the
/// server sends.
fn move_blocks(
    level: &mut impl PistonLevel,
    piston_pos: BlockPos,
    direction: Direction,
    extending: bool,
    sticky: bool,
) -> bool {
    let arm = relative(piston_pos, direction, 1);
    if !extending && is(level.block(arm), "piston_head") {
        level.set_block(arm, BlockState::AIR);
    }
    let mut resolver = Resolver::new(piston_pos, direction, extending);
    if !resolver.resolve(level) {
        return false;
    }
    let mut delete_after_move: HashMap<BlockPos, BlockState> = HashMap::new();
    let to_push_shapes: Vec<BlockState> = resolver
        .to_push
        .iter()
        .map(|&pos| {
            let state = level.block(pos);
            delete_after_move.insert(pos, state);
            state
        })
        .collect();
    let push_direction = resolver.push_direction;
    for &pos in resolver.to_destroy.iter().rev() {
        let state = level.block(pos);
        if !block::block_tags().contains("minecraft:fire", block::block_id(state)) {
            level.destroy_effect(pos, state);
        }
        level.set_block(pos, BlockState::AIR);
    }
    for (i, &from) in resolver.to_push.iter().enumerate().rev() {
        let pos = relative(from, push_direction, 1);
        delete_after_move.remove(&pos);
        let piston = MovingPiston::new(to_push_shapes[i], direction, extending, false);
        set_moving(level, pos, moving_piston(direction, false), piston);
    }
    if extending {
        let head = piston_head(direction, sticky, false);
        delete_after_move.remove(&arm);
        set_moving(
            level,
            arm,
            moving_piston(direction, sticky),
            MovingPiston::new(head, direction, true, true),
        );
    }
    for pos in delete_after_move.into_keys() {
        level.set_block(pos, BlockState::AIR);
    }
    true
}

/// Vanilla `Level.removeBlock`: what's left is the cell's fluid.
fn remove_block(level: &mut impl PistonLevel, pos: BlockPos) {
    let fluid = block::fluid_block(block::fluid(level.block(pos)));
    level.set_block(pos, fluid);
}

/// Vanilla `setBlock` then `setBlockEntity`: an unloaded chunk takes neither.
fn set_moving(
    level: &mut impl PistonLevel,
    pos: BlockPos,
    block_state: BlockState,
    piston: MovingPiston,
) {
    level.set_block(pos, block_state);
    if level.is_loaded(pos) {
        level.pistons().insert(pos, piston);
    }
}

/// Vanilla `PistonMovingBlockEntity.finalTick`: lands the block at once.
pub fn final_tick(level: &mut impl PistonLevel, pos: BlockPos) {
    let Some(piston) = level.pistons().remove(&pos) else {
        return;
    };
    if is(level.block(pos), "moving_piston") {
        // TODO: `Block.updateFromNeighbourShapes`; the server sends the
        // shape-updated block.
        let state = if piston.source {
            BlockState::AIR
        } else {
            piston.moved_state
        };
        level.set_block(pos, state);
    }
}

/// Vanilla `PistonMovingBlockEntity.tick` for every moving block. `on_move`
/// runs where vanilla moves the entities in the way, with the progress the
/// block is about to reach.
pub fn tick<L: PistonLevel>(
    level: &mut L,
    mut on_move: impl FnMut(&mut L, BlockPos, &MovingPiston, f32),
) {
    let positions = level.pistons().tick_order();
    for pos in positions {
        let Some(piston) = level.pistons().get_mut(&pos) else {
            continue;
        };
        piston.progress_o = piston.progress;
        if piston.progress_o >= 1.0 {
            if piston.death_ticks < DEATH_TICKS {
                piston.death_ticks += 1;
                continue;
            }
            let piston = level.pistons().remove(&pos).expect("entry checked above");
            if is(level.block(pos), "moving_piston") {
                // TODO: `Block.updateFromNeighbourShapes` and its `updateOrDestroy`
                // when the shape update yields air; the server sends the result.
                let state = block::with_property(piston.moved_state, "waterlogged", "false")
                    .unwrap_or(piston.moved_state);
                level.set_block(pos, state);
            }
            continue;
        }
        let new_progress = piston.progress + 0.5;
        let snapshot = *piston;
        on_move(level, pos, &snapshot, new_progress);
        if let Some(piston) = level.pistons().get_mut(&pos) {
            piston.progress = new_progress.min(1.0);
        }
    }
}

/// Vanilla `PistonBaseBlock.isPushable`.
fn is_pushable(
    level: &impl PistonLevel,
    state: BlockState,
    pos: BlockPos,
    direction: Direction,
    allow_destroyable: bool,
    connection_direction: Direction,
) -> bool {
    // TODO: the world border, which the client doesn't track yet.
    if pos.y < level.min_y() || pos.y > level.max_y() {
        return false;
    }
    if block::is_air(state) {
        return true;
    }
    if matches!(
        block::block_id(state),
        "obsidian" | "crying_obsidian" | "respawn_anchor" | "reinforced_deepslate"
    ) {
        return false;
    }
    if direction == Direction::Down && pos.y == level.min_y() {
        return false;
    }
    if direction == Direction::Up && pos.y == level.max_y() {
        return false;
    }
    let behavior = block::block_behavior(state);
    if is_piston_base(state) {
        if block::block_properties(state).get("extended") == Some("true") {
            return false;
        }
    } else {
        if behavior.destroy_time == -1.0 {
            return false;
        }
        match behavior.push_reaction {
            PushReaction::Block => return false,
            PushReaction::Destroy => return allow_destroyable,
            PushReaction::PushOnly => return direction == connection_direction,
            PushReaction::Normal | PushReaction::Ignore => {}
        }
    }
    !behavior.has_block_entity
}

/// Vanilla `PistonStructureResolver`.
struct Resolver {
    piston_pos: BlockPos,
    extending: bool,
    start_pos: BlockPos,
    push_direction: Direction,
    to_push: Vec<BlockPos>,
    to_destroy: Vec<BlockPos>,
    piston_direction: Direction,
}

impl Resolver {
    fn new(piston_pos: BlockPos, direction: Direction, extending: bool) -> Self {
        let (push_direction, start_pos) = if extending {
            (direction, relative(piston_pos, direction, 1))
        } else {
            (direction.opposite(), relative(piston_pos, direction, 2))
        };
        Self {
            piston_pos,
            extending,
            start_pos,
            push_direction,
            to_push: Vec::new(),
            to_destroy: Vec::new(),
            piston_direction: direction,
        }
    }

    fn resolve(&mut self, level: &impl PistonLevel) -> bool {
        self.to_push.clear();
        self.to_destroy.clear();
        let next_state = level.block(self.start_pos);
        if !is_pushable(
            level,
            next_state,
            self.start_pos,
            self.push_direction,
            false,
            self.piston_direction,
        ) {
            if self.extending
                && block::block_behavior(next_state).push_reaction == PushReaction::Destroy
            {
                self.to_destroy.push(self.start_pos);
                return true;
            }
            return false;
        }
        if !self.add_block_line(level, self.start_pos, self.push_direction) {
            return false;
        }
        let mut i = 0;
        while i < self.to_push.len() {
            let pos = self.to_push[i];
            if is_sticky(level.block(pos)) && !self.add_branching_blocks(level, pos) {
                return false;
            }
            i += 1;
        }
        true
    }

    fn add_block_line(
        &mut self,
        level: &impl PistonLevel,
        start: BlockPos,
        direction: Direction,
    ) -> bool {
        let mut next_state = level.block(start);
        if block::is_air(next_state) {
            return true;
        }
        if !is_pushable(
            level,
            next_state,
            start,
            self.push_direction,
            false,
            direction,
        ) {
            return true;
        }
        if start == self.piston_pos {
            return true;
        }
        if self.to_push.contains(&start) {
            return true;
        }
        let mut block_count = 1;
        if block_count + self.to_push.len() > MAX_PUSH_DEPTH {
            return false;
        }
        let back = self.push_direction.opposite();
        while is_sticky(next_state) {
            let pos = relative(start, back, block_count as i32);
            let previous_state = next_state;
            next_state = level.block(pos);
            if block::is_air(next_state)
                || !can_stick_to_each_other(previous_state, next_state)
                || !is_pushable(level, next_state, pos, self.push_direction, false, back)
                || pos == self.piston_pos
            {
                break;
            }
            block_count += 1;
            if block_count + self.to_push.len() > MAX_PUSH_DEPTH {
                return false;
            }
        }
        let mut blocks_added = 0;
        for i in (0..block_count).rev() {
            self.to_push.push(relative(start, back, i as i32));
            blocks_added += 1;
        }
        let mut i = 1;
        loop {
            let pos = relative(start, self.push_direction, i);
            if let Some(collision_pos) = self.to_push.iter().position(|&p| p == pos) {
                self.reorder_list_at_collision(blocks_added, collision_pos);
                for j in 0..=collision_pos + blocks_added {
                    let block_pos = self.to_push[j];
                    if is_sticky(level.block(block_pos))
                        && !self.add_branching_blocks(level, block_pos)
                    {
                        return false;
                    }
                }
                return true;
            }
            next_state = level.block(pos);
            if block::is_air(next_state) {
                return true;
            }
            if !is_pushable(
                level,
                next_state,
                pos,
                self.push_direction,
                true,
                self.push_direction,
            ) || pos == self.piston_pos
            {
                return false;
            }
            if block::block_behavior(next_state).push_reaction == PushReaction::Destroy {
                self.to_destroy.push(pos);
                return true;
            }
            if self.to_push.len() >= MAX_PUSH_DEPTH {
                return false;
            }
            self.to_push.push(pos);
            blocks_added += 1;
            i += 1;
        }
    }

    fn reorder_list_at_collision(&mut self, blocks_added: usize, collision_pos: usize) {
        let len = self.to_push.len();
        let head = &self.to_push[..collision_pos];
        let last_line_added = &self.to_push[len - blocks_added..];
        let collision_to_line = &self.to_push[collision_pos..len - blocks_added];
        self.to_push = [head, last_line_added, collision_to_line].concat();
    }

    fn add_branching_blocks(&mut self, level: &impl PistonLevel, from_pos: BlockPos) -> bool {
        let from_state = level.block(from_pos);
        for direction in Direction::VALUES {
            if direction.axis() == self.push_direction.axis() {
                continue;
            }
            let neighbour_pos = relative(from_pos, direction, 1);
            let neighbour_state = level.block(neighbour_pos);
            if can_stick_to_each_other(neighbour_state, from_state)
                && !self.add_block_line(level, neighbour_pos, direction)
            {
                return false;
            }
        }
        true
    }
}

fn is_sticky(state: BlockState) -> bool {
    is(state, "slime_block") || is(state, "honey_block")
}

fn can_stick_to_each_other(state1: BlockState, state2: BlockState) -> bool {
    if is(state1, "honey_block") && is(state2, "slime_block") {
        return false;
    }
    if is(state1, "slime_block") && is(state2, "honey_block") {
        return false;
    }
    is_sticky(state1) || is_sticky(state2)
}

fn is(state: BlockState, name: &str) -> bool {
    block::block_id(state) == name
}

pub(crate) fn relative(pos: BlockPos, direction: Direction, steps: i32) -> BlockPos {
    let [x, y, z] = direction.offset();
    BlockPos::new(pos.x + x * steps, pos.y + y * steps, pos.z + z * steps)
}

/// Vanilla `Direction.from3DDataValue`.
fn from_3d_data_value(id: i32) -> Direction {
    Direction::VALUES[(id % 6).unsigned_abs() as usize]
}

/// Whether `state` is a piston base, vanilla `instanceof PistonBaseBlock`.
pub fn is_piston_base(state: BlockState) -> bool {
    is(state, "piston") || is(state, "sticky_piston")
}

/// A piston-family block's `facing`.
pub fn facing(state: BlockState) -> Option<Direction> {
    Direction::from_str(block::block_properties(state).get("facing")?)
}

fn piston_type(sticky: bool) -> &'static str {
    if sticky { "sticky" } else { "normal" }
}

fn moving_piston(direction: Direction, sticky: bool) -> BlockState {
    block::state_with(
        "moving_piston",
        &[("facing", direction.name()), ("type", piston_type(sticky))],
    )
    .unwrap_or(BlockState::AIR)
}

/// A retracted piston base, vanilla `defaultBlockState().setValue(FACING, ..)`.
fn piston_base(sticky: bool, direction: Direction) -> BlockState {
    let name = if sticky { "sticky_piston" } else { "piston" };
    block::state_with(name, &[("facing", direction.name()), ("extended", "false")])
        .unwrap_or(BlockState::AIR)
}

pub fn piston_head(direction: Direction, sticky: bool, short: bool) -> BlockState {
    let head = block::state_with(
        "piston_head",
        &[("facing", direction.name()), ("type", piston_type(sticky))],
    )
    .unwrap_or(BlockState::AIR);
    with_short(head, short)
}

/// `head` with its `short` property (the shorter arm) set.
pub fn with_short(head: BlockState, short: bool) -> BlockState {
    block::with_property(head, "short", if short { "true" } else { "false" }).unwrap_or(head)
}

/// The head of piston base `base`: its type and facing.
pub fn head_of(base: BlockState, short: bool) -> Option<BlockState> {
    Some(piston_head(facing(base)?, is(base, "sticky_piston"), short))
}

/// Vanilla `BlockState.CODEC`: `{Name, Properties}`.
fn state_from_nbt(nbt: &NbtCompound) -> Option<BlockState> {
    let name = nbt.string("Name")?.to_str();
    let name = name.strip_prefix("minecraft:").unwrap_or(&name);
    let mut state = block::first_state_of(name)?;
    if let Some(props) = nbt.compound("Properties") {
        for (key, value) in props.iter() {
            if let Some(value) = value.string() {
                state = block::with_property(state, &key.to_str(), &value.to_str())?;
            }
        }
    }
    Some(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat test world: everything unset is air, y in -64..320.
    #[derive(Default)]
    struct TestLevel {
        blocks: HashMap<BlockPos, BlockState>,
        pistons: MovingPistons,
        destroyed: Vec<BlockPos>,
    }

    impl PistonLevel for TestLevel {
        fn block(&self, pos: BlockPos) -> BlockState {
            self.blocks.get(&pos).copied().unwrap_or(BlockState::AIR)
        }

        fn set_block(&mut self, pos: BlockPos, state: BlockState) {
            if !is(state, "moving_piston") {
                self.pistons.remove(&pos);
            }
            self.blocks.insert(pos, state);
        }

        fn is_loaded(&self, _pos: BlockPos) -> bool {
            true
        }

        fn min_y(&self) -> i32 {
            -64
        }

        fn max_y(&self) -> i32 {
            319
        }

        fn pistons(&mut self) -> &mut MovingPistons {
            &mut self.pistons
        }

        fn destroy_effect(&mut self, pos: BlockPos, _state: BlockState) {
            self.destroyed.push(pos);
        }
    }

    fn state(name: &str) -> BlockState {
        block::first_state_of(name).unwrap()
    }

    fn at(x: i32) -> BlockPos {
        BlockPos::new(x, 0, 0)
    }

    /// A piston at the origin facing east, with `blocks` from x = 1 on.
    fn level(sticky: bool, blocks: &[&str]) -> TestLevel {
        block::init("26.2");
        let mut level = TestLevel::default();
        level.set_block(at(0), piston_base(sticky, Direction::East));
        for (i, name) in blocks.iter().enumerate() {
            level.set_block(at(i as i32 + 1), state(name));
        }
        level
    }

    fn name(level: &TestLevel, pos: BlockPos) -> &'static str {
        block::block_id(level.block(pos))
    }

    #[test]
    fn extending_pushes_the_whole_line() {
        let mut level = level(false, &["stone", "dirt"]);
        assert!(trigger_event(&mut level, at(0), 0, 5));
        assert_eq!(
            block::block_properties(level.block(at(0))).get("extended"),
            Some("true")
        );
        for x in 1..=3 {
            assert_eq!(name(&level, at(x)), "moving_piston", "x = {x}");
        }
        let head = level.pistons.get(&at(1)).unwrap();
        assert!(head.source && head.extending);
        assert_eq!(block::block_id(head.moved_state), "piston_head");
        assert_eq!(
            block::block_id(level.pistons.get(&at(2)).unwrap().moved_state),
            "stone"
        );
        assert_eq!(
            block::block_id(level.pistons.get(&at(3)).unwrap().moved_state),
            "dirt"
        );
    }

    #[test]
    fn twelve_blocks_push_but_thirteen_dont() {
        let mut twelve = level(false, &["stone"; 12]);
        assert!(trigger_event(&mut twelve, at(0), 0, 5));
        assert_eq!(name(&twelve, at(13)), "moving_piston");

        let mut too_long = level(false, &["stone"; 13]);
        assert!(!trigger_event(&mut too_long, at(0), 0, 5));
        assert_eq!(name(&too_long, at(1)), "stone");
        assert!(too_long.pistons.iter().next().is_none());
    }

    #[test]
    fn obsidian_stops_the_push() {
        let mut level = level(false, &["stone", "obsidian"]);
        assert!(!trigger_event(&mut level, at(0), 0, 5));
        assert_eq!(name(&level, at(1)), "stone");
        assert_eq!(name(&level, at(2)), "obsidian");
    }

    #[test]
    fn flowers_are_destroyed_not_pushed() {
        let mut level = level(false, &["stone", "poppy"]);
        assert!(trigger_event(&mut level, at(0), 0, 5));
        assert_eq!(level.destroyed, vec![at(2)]);
        assert_eq!(
            block::block_id(level.pistons.get(&at(2)).unwrap().moved_state),
            "stone"
        );
    }

    #[test]
    fn sticky_retract_leaves_immovable_blocks() {
        let mut level = level(true, &["piston_head", "obsidian"]);
        let extended = block::with_property(level.block(at(0)), "extended", "true").unwrap();
        level.set_block(at(0), extended);
        level.set_block(at(1), piston_head(Direction::East, true, false));
        assert!(trigger_event(&mut level, at(0), 1, 5));
        assert_eq!(name(&level, at(0)), "moving_piston");
        assert_eq!(name(&level, at(1)), "air");
        assert_eq!(name(&level, at(2)), "obsidian");
    }

    #[test]
    fn sticky_retract_pulls_a_normal_block() {
        let mut level = level(true, &["piston_head", "stone"]);
        let extended = block::with_property(level.block(at(0)), "extended", "true").unwrap();
        level.set_block(at(0), extended);
        level.set_block(at(1), piston_head(Direction::East, true, false));
        assert!(trigger_event(&mut level, at(0), 1, 5));
        assert_eq!(name(&level, at(1)), "moving_piston");
        assert_eq!(name(&level, at(2)), "air");
        let pulled = level.pistons.get(&at(1)).unwrap();
        assert!(!pulled.extending && !pulled.source);
        assert_eq!(block::block_id(pulled.moved_state), "stone");
    }

    #[test]
    fn slime_drags_its_neighbours_but_not_honey() {
        let mut level = level(false, &["slime_block"]);
        level.set_block(BlockPos::new(1, 1, 0), state("stone"));
        level.set_block(BlockPos::new(1, -1, 0), state("honey_block"));
        assert!(trigger_event(&mut level, at(0), 0, 5));
        assert_eq!(name(&level, BlockPos::new(2, 1, 0)), "moving_piston");
        assert_eq!(name(&level, BlockPos::new(1, 1, 0)), "air");
        assert_eq!(name(&level, BlockPos::new(1, -1, 0)), "honey_block");
    }

    #[test]
    fn moving_blocks_land_after_the_death_ticks() {
        let mut level = level(false, &["stone"]);
        trigger_event(&mut level, at(0), 0, 5);
        let mut progress = Vec::new();
        for _ in 0..2 {
            tick(&mut level, |_, _, _, _| {});
            progress.push(level.pistons.get(&at(2)).unwrap().progress);
        }
        assert_eq!(progress, [0.5, 1.0]);
        for _ in 0..=DEATH_TICKS {
            assert_eq!(name(&level, at(2)), "moving_piston");
            tick(&mut level, |_, _, _, _| {});
        }
        assert_eq!(name(&level, at(2)), "stone");
        assert_eq!(name(&level, at(1)), "piston_head");
    }

    #[test]
    fn chunk_data_facing_is_a_byte() {
        let mut nbt = NbtCompound::new();
        nbt.insert("facing", 5i8);
        assert_eq!(MovingPiston::from_nbt(&nbt).direction, Direction::East);
    }

    #[test]
    fn retracting_mid_extension_lands_the_head_first() {
        let mut level = level(false, &[]);
        trigger_event(&mut level, at(0), 0, 5);
        tick(&mut level, |_, _, _, _| {});
        assert!(trigger_event(&mut level, at(0), 1, 5));
        // `finalTick` ends the arm's moving head early; a source piece lands
        // as air.
        assert_eq!(name(&level, at(1)), "air");
        let base = level.pistons.get(&at(0)).unwrap();
        assert!(base.source && !base.extending && base.progress == 0.0);
    }
}
