#![doc = include_str!("../README.md")]

mod behavior;
pub mod block_state;
pub mod blocks;
pub mod fluid_state;
pub mod properties;
mod range;

use core::fmt::Debug;
use std::any::Any;
use std::collections::HashMap;
use std::str::FromStr;

use azalea_registry::builtin::BlockKind;
pub use behavior::BlockBehavior;
// re-exported for convenience
pub use block_state::BlockState;
pub use range::BlockStates;

/// A trait that's implemented on block structs.
///
/// See the [azalea_block documentation](crate) for details.
pub trait BlockTrait: Debug + Any {
    fn behavior(&self) -> BlockBehavior;
    /// Get the Minecraft string ID for this block.
    ///
    /// For example, `stone` or `grass_block`.
    fn id(&self) -> &'static str;
    /// Convert the block struct to a [`BlockState`].
    ///
    /// This is a lossless conversion, as [`BlockState`] also contains state
    /// data.
    fn as_block_state(&self) -> BlockState;
    /// Convert the block struct to a [`BlockKind`].
    ///
    /// This is a lossy conversion, as [`BlockKind`] doesn't contain any state
    /// data.
    fn as_block_kind(&self) -> BlockKind;
    #[deprecated = "renamed to as_block_kind"]
    #[doc(hidden)]
    fn as_registry_block(&self) -> BlockKind {
        self.as_block_kind()
    }

    /// Returns a map of property names on this block to their values as
    /// strings.
    ///
    /// Consider using [`Self::get_property`] if you only need a single
    /// property.
    fn property_map(&self) -> HashMap<&'static str, &'static str>;
    /// Get a property's value as a string by its name, or `None` if the block
    /// has no property with that name.
    ///
    /// To get all properties, you may use [`Self::property_map`].
    ///
    /// To set a property, use [`Self::set_property`].
    fn get_property(&self, name: &str) -> Option<&'static str>;
    /// Update a property on this block, with the name and value being strings.
    ///
    /// Returns `Ok(())`, if the property name and value are valid, otherwise it
    /// returns `Err(InvalidPropertyError)`.
    ///
    /// To get a property, use [`Self::get_property`].
    fn set_property(&mut self, name: &str, new_value: &str) -> Result<(), InvalidPropertyError>;
}

#[derive(Debug)]
pub struct InvalidPropertyError;

impl dyn BlockTrait {
    pub fn downcast_ref<T: BlockTrait>(&self) -> Option<&T> {
        (self as &dyn Any).downcast_ref::<T>()
    }
}

pub trait Property: FromStr {
    type Value;

    fn try_from_block_state(state: BlockState) -> Option<Self::Value>;

    /// Convert the value of the property to a string, like "x" or "true".
    fn to_static_str(&self) -> &'static str;
}
