//! Vanilla `LightEngine.QueueEntry`: propagation queue payloads packed into a
//! `u64`, plus the propagation `Dir` order the bit layout depends on.
//!
//! Layout (LightEngine.java `QueueEntry`): bits 0-3 = from-level, bits 4-9 =
//! one flag per direction (`1 << (ordinal + 4)`), bit 10 = FROM_EMPTY_SHAPE,
//! bit 11 = INCREASE_FROM_EMISSION.

/// Vanilla `Direction` in ordinal order; opposites pair as `ordinal ^ 1`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(usize)]
pub(crate) enum Dir {
    Down = 0,
    Up = 1,
    North = 2,
    South = 3,
    West = 4,
    East = 5,
}

impl Dir {
    pub const ALL: [Dir; 6] = [
        Dir::Down,
        Dir::Up,
        Dir::North,
        Dir::South,
        Dir::West,
        Dir::East,
    ];

    pub fn opposite(self) -> Dir {
        Self::ALL[self as usize ^ 1]
    }

    pub fn offset(self) -> (i32, i32, i32) {
        match self {
            Dir::Down => (0, -1, 0),
            Dir::Up => (0, 1, 0),
            Dir::North => (0, 0, -1),
            Dir::South => (0, 0, 1),
            Dir::West => (-1, 0, 0),
            Dir::East => (1, 0, 0),
        }
    }
}

const LEVEL_MASK: u64 = 15;
const DIRECTIONS_MASK: u64 = 1008;
const FLAG_FROM_EMPTY_SHAPE: u64 = 1024;
const FLAG_INCREASE_FROM_EMISSION: u64 = 2048;

/// `decreaseAllDirections(1)`: re-pull neighbor light into a node whose own
/// level didn't drop.
pub(crate) const PULL_LIGHT_IN_ENTRY: u64 = with_level(DIRECTIONS_MASK, 1);

pub(crate) const fn decrease_all_directions(old_from_level: u8) -> u64 {
    with_level(DIRECTIONS_MASK, old_from_level)
}

pub(crate) const fn decrease_skip_one_direction(old_from_level: u8, skip: Dir) -> u64 {
    with_level(without_direction(DIRECTIONS_MASK, skip), old_from_level)
}

pub(crate) const fn increase_light_from_emission(
    new_from_level: u8,
    from_empty_shape: bool,
) -> u64 {
    let mut data = DIRECTIONS_MASK | FLAG_INCREASE_FROM_EMISSION;
    if from_empty_shape {
        data |= FLAG_FROM_EMPTY_SHAPE;
    }
    with_level(data, new_from_level)
}

pub(crate) const fn increase_skip_one_direction(
    new_from_level: u8,
    from_empty_shape: bool,
    skip: Dir,
) -> u64 {
    let mut data = without_direction(DIRECTIONS_MASK, skip);
    if from_empty_shape {
        data |= FLAG_FROM_EMPTY_SHAPE;
    }
    with_level(data, new_from_level)
}

pub(crate) const fn increase_only_one_direction(
    new_from_level: u8,
    from_empty_shape: bool,
    dir: Dir,
) -> u64 {
    let mut data = 0;
    if from_empty_shape {
        data |= FLAG_FROM_EMPTY_SHAPE;
    }
    with_level(with_direction(data, dir), new_from_level)
}

pub(crate) fn get_from_level(entry: u64) -> u8 {
    (entry & LEVEL_MASK) as u8
}

pub(crate) fn is_from_empty_shape(entry: u64) -> bool {
    entry & FLAG_FROM_EMPTY_SHAPE != 0
}

pub(crate) fn is_increase_from_emission(entry: u64) -> bool {
    entry & FLAG_INCREASE_FROM_EMISSION != 0
}

pub(crate) fn should_propagate_in_direction(entry: u64, dir: Dir) -> bool {
    entry & (1 << (dir as usize + 4)) != 0
}

const fn with_level(entry: u64, level: u8) -> u64 {
    entry & !LEVEL_MASK | (level as u64 & LEVEL_MASK)
}

const fn with_direction(entry: u64, dir: Dir) -> u64 {
    entry | 1 << (dir as usize + 4)
}

const fn without_direction(entry: u64, dir: Dir) -> u64 {
    entry & !(1 << (dir as usize + 4))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_constants_are_bit_exact() {
        // Oracles computed from vanilla LightEngine.QueueEntry.
        assert_eq!(PULL_LIGHT_IN_ENTRY, 0x3F1);
        assert_eq!(decrease_all_directions(15), 0x3FF);
        assert_eq!(decrease_skip_one_direction(15, Dir::Up), 0x3DF);
        assert_eq!(increase_light_from_emission(14, true), 0xFFE);
        assert_eq!(increase_light_from_emission(14, false), 0xBFE);
        assert_eq!(increase_skip_one_direction(15, false, Dir::Up), 0x3DF);
        assert_eq!(increase_only_one_direction(7, false, Dir::West), 0x107);
    }

    #[test]
    fn entry_accessors() {
        let e = increase_skip_one_direction(9, true, Dir::North);
        assert_eq!(get_from_level(e), 9);
        assert!(is_from_empty_shape(e));
        assert!(!is_increase_from_emission(e));
        assert!(!should_propagate_in_direction(e, Dir::North));
        for dir in [Dir::Down, Dir::Up, Dir::South, Dir::West, Dir::East] {
            assert!(should_propagate_in_direction(e, dir));
        }
        assert_eq!(Dir::North.opposite(), Dir::South);
        assert_eq!(Dir::Down.opposite(), Dir::Up);
        assert_eq!(Dir::East.opposite(), Dir::West);
    }
}
