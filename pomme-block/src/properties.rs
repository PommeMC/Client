//! The property types azalea's crates name at compile time; every
//! block-state lookup returns `None`.

use std::str::FromStr;

use crate::{BlockState, Property};

macro_rules! bool_property {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub struct $name(pub bool);

        impl Property for $name {
            type Value = bool;

            fn try_from_block_state(_state: BlockState) -> Option<bool> {
                None
            }

            fn to_static_str(&self) -> &'static str {
                if self.0 { "true" } else { "false" }
            }
        }

        impl FromStr for $name {
            type Err = ();

            fn from_str(s: &str) -> Result<Self, ()> {
                match s {
                    "true" => Ok(Self(true)),
                    "false" => Ok(Self(false)),
                    _ => Err(()),
                }
            }
        }
    };
}

bool_property!(Waterlogged);
bool_property!(Open);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FacingCardinal {
    North,
    South,
    West,
    East,
}

impl Property for FacingCardinal {
    type Value = Self;

    fn try_from_block_state(_state: BlockState) -> Option<Self> {
        None
    }

    fn to_static_str(&self) -> &'static str {
        match self {
            Self::North => "north",
            Self::South => "south",
            Self::West => "west",
            Self::East => "east",
        }
    }
}

impl FromStr for FacingCardinal {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "north" => Ok(Self::North),
            "south" => Ok(Self::South),
            "west" => Ok(Self::West),
            "east" => Ok(Self::East),
            _ => Err(()),
        }
    }
}
