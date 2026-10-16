//! Logical game actions, display metadata, default inputs and UI ordering.

use std::borrow::Cow;

use gilrs::Button;
use winit::event::MouseButton;
use winit::keyboard::KeyCode;

use super::InputKey::{self, Controller, Keyboard, Mouse};
use super::{ActionId, DisplayText, StringId};

/// Display metadata and explicit UI ordering for an action category.
#[derive(Clone, Debug)]
pub struct ActionCategoryMetadata {
    pub display: DisplayText<'static>,
    pub ui_order: i32,
}

macro_rules! action_categories {
    ($($(#[$category_attr:meta])* $category:ident => {
        translation_id: $translation_id:expr,
        fallback: $fallback:literal,
        ui_order: $ui_order:expr $(,)?
    }),* $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum ActionCategory {
            $($(#[$category_attr])* $category,)*
        }

        impl ActionCategory {
            pub const fn metadata(self) -> ActionCategoryMetadata {
                match self {
                    $(Self::$category => ActionCategoryMetadata {
                        display: DisplayText {
                            translation_id: $translation_id,
                            fallback: Cow::Borrowed($fallback),
                        },
                        ui_order: $ui_order,
                    },)*
                }
            }

            pub fn display_name(self) -> Cow<'static, str> {
                self.metadata().display.resolve()
            }

            /// Explicit category priority; lower values sort first.
            pub fn ui_order(self) -> i32 {
                self.metadata().ui_order
            }
        }
    };
}

// Preserve vanilla's category registration order independently of enum order.
action_categories! {
    Movement => {
        translation_id: Some(StringId::new("key.category.minecraft.movement")),
        fallback: "Movement",
        ui_order: 0,
    },
    Miscellaneous => {
        translation_id: Some(StringId::new("key.category.minecraft.misc")),
        fallback: "Miscellaneous",
        ui_order: 1,
    },
    Multiplayer => {
        translation_id: Some(StringId::new("key.category.minecraft.multiplayer")),
        fallback: "Multiplayer",
        ui_order: 2,
    },
    Gameplay => {
        translation_id: Some(StringId::new("key.category.minecraft.gameplay")),
        fallback: "Gameplay",
        ui_order: 3,
    },
    Inventory => {
        translation_id: Some(StringId::new("key.category.minecraft.inventory")),
        fallback: "Inventory",
        ui_order: 4,
    },
    // TODO: Remove this allowance once the remaining Creative key bindings are added.
    #[allow(dead_code)]
    Creative => {
        translation_id: Some(StringId::new("key.category.minecraft.creative")),
        fallback: "Creative Mode",
        ui_order: 5,
    },
    Spectator => {
        translation_id: Some(StringId::new("key.category.minecraft.spectator")),
        fallback: "Spectator",
        ui_order: 6,
    },
    // TODO: Remove this allowance once the remaining Debug key bindings are added.
    #[allow(dead_code)]
    Debug => {
        translation_id: Some(StringId::new("key.category.minecraft.debug")),
        fallback: "Debug",
        ui_order: 7,
    },
}

/// Persistent identity, display metadata and default inputs of an action.
#[derive(Clone, Debug)]
pub struct ActionMetadata {
    pub id: ActionId,
    pub display: DisplayText<'static>,
    pub category: ActionCategory,
    /// Shown on the Key Binds screen. Pomme-only actions vanilla handles
    /// outside its key mappings (Escape) stay off it.
    pub listed: bool,
    pub defaults: &'static [InputKey],
}

// Keep persistent IDs, display labels, categories and default inputs in one
// catalog.
macro_rules! actions {
    ($($(#[$action_attr:meta])* $action:ident => {
            id: $name:literal,
            translation_id: $translation_id:expr,
            fallback: $fallback:literal,
            category: $category:ident,
            listed: $listed:literal,
            defaults: [$($default:expr),* $(,)?] $(,)?
        }),* $(,)?) => {
        // TODO: Reconcile Action catalog coverage with vanilla's configurable
        // key mappings.
        #[derive(Hash, PartialEq, Eq, Clone, Copy, Debug)]
        pub enum Action {
            $($(#[$action_attr])* $action,)*
        }

        impl Action {
            pub const ALL: &[Action] = &[$(Self::$action,)*];

            pub const fn metadata(self) -> ActionMetadata {
                match self {
                    $(Self::$action => ActionMetadata {
                        id: ActionId::new($name),
                        display: DisplayText {
                            translation_id: $translation_id,
                            fallback: Cow::Borrowed($fallback),
                        },
                        category: ActionCategory::$category,
                        listed: $listed,
                        defaults: &[$($default,)*],
                    },)*
                }
            }

            /// The action whose stable id (as in options.json) is `id`.
            pub fn from_id(id: &str) -> Option<Self> {
                match id {
                    $($name => Some(Self::$action),)*
                    _ => None,
                }
            }
        }
    };
}

impl Action {
    /// Stable identifier used in options.json and key mapping lookups.
    pub fn id(self) -> ActionId {
        self.metadata().id
    }

    /// Translated display label, or the catalog's English fallback.
    pub fn display_name(self) -> Cow<'static, str> {
        self.metadata().display.resolve()
    }

    pub fn category(self) -> ActionCategory {
        self.metadata().category
    }

    pub fn listed(self) -> bool {
        self.metadata().listed
    }

    pub fn defaults(self) -> &'static [InputKey] {
        self.metadata().defaults
    }

    /// The first default keyboard key, for the live input paths that still
    /// read keys directly.
    pub fn default_key(self) -> Option<KeyCode> {
        self.defaults().iter().find_map(|input| match input {
            Keyboard(code) => Some(*code),
            _ => None,
        })
    }

    /// `KeyMapping.compareTo`: category order, then localized name.
    pub fn compare_ui(self, other: Self) -> std::cmp::Ordering {
        self.category()
            .ui_order()
            .cmp(&other.category().ui_order())
            .then_with(|| self.display_name().cmp(&other.display_name()))
    }
}

actions! {
    Jump => {
        id: "key.jump",
        translation_id: Some(StringId::new("key.jump")),
        fallback: "Jump",
        category: Movement,
        listed: true,
        defaults: [Keyboard(KeyCode::Space), Controller(Button::South)],
    },
    Sneak => {
        id: "key.sneak",
        translation_id: Some(StringId::new("key.sneak")),
        fallback: "Sneak",
        category: Movement,
        listed: true,
        defaults: [Keyboard(KeyCode::ShiftLeft), Controller(Button::LeftThumb)],
    },
    Sprint => {
        id: "key.sprint",
        translation_id: Some(StringId::new("key.sprint")),
        fallback: "Sprint",
        category: Movement,
        listed: true,
        defaults: [Keyboard(KeyCode::ControlLeft), Controller(Button::West)],
    },
    Destroy => {
        id: "key.attack",
        translation_id: Some(StringId::new("key.attack")),
        fallback: "Attack/Destroy",
        category: Gameplay,
        listed: true,
        defaults: [Mouse(MouseButton::Left), Controller(Button::RightTrigger2)],
    },
    Use => {
        id: "key.use",
        translation_id: Some(StringId::new("key.use")),
        fallback: "Use Item/Place Block",
        category: Gameplay,
        listed: true,
        defaults: [Mouse(MouseButton::Right), Controller(Button::LeftTrigger2)],
    },
    ToggleInventory => {
        id: "key.inventory",
        translation_id: Some(StringId::new("key.inventory")),
        fallback: "Open/Close Inventory",
        category: Inventory,
        listed: true,
        defaults: [Keyboard(KeyCode::KeyE), Controller(Button::North)],
    },
    OpenMenu => {
        id: "key.menu",
        translation_id: None,
        fallback: "Open Menu",
        category: Miscellaneous,
        listed: false,
        defaults: [Keyboard(KeyCode::Escape), Controller(Button::Start)],
    },
    ViewPlayerList => {
        id: "key.playerlist",
        translation_id: Some(StringId::new("key.playerlist")),
        fallback: "List Players",
        category: Multiplayer,
        listed: true,
        defaults: [Keyboard(KeyCode::Tab), Controller(Button::Select)],
    },
    ChangePerspective => {
        id: "key.togglePerspective",
        translation_id: Some(StringId::new("key.togglePerspective")),
        fallback: "Toggle Perspective",
        category: Miscellaneous,
        listed: true,
        defaults: [Keyboard(KeyCode::F5), Controller(Button::DPadUp)],
    },
    OpenChat => {
        id: "key.chat",
        translation_id: Some(StringId::new("key.chat")),
        fallback: "Open Chat",
        category: Multiplayer,
        listed: true,
        defaults: [Keyboard(KeyCode::KeyT), Controller(Button::DPadRight)],
    },
    OpenCommands => {
        id: "key.command",
        translation_id: Some(StringId::new("key.command")),
        fallback: "Open Command",
        category: Multiplayer,
        listed: true,
        defaults: [Keyboard(KeyCode::Slash)],
    },
    // Keyboard Escape closes through OpenMenu, not the Close action.
    Close => {
        id: "key.close",
        translation_id: None,
        fallback: "Close",
        category: Miscellaneous,
        listed: false,
        defaults: [Controller(Button::East)],
    },
    DropItem => {
        id: "key.drop",
        translation_id: Some(StringId::new("key.drop")),
        fallback: "Drop Selected Item",
        category: Inventory,
        listed: true,
        defaults: [Keyboard(KeyCode::KeyQ)],
    },
    SwapOffhand => {
        id: "key.swapOffhand",
        translation_id: Some(StringId::new("key.swapOffhand")),
        fallback: "Swap Item With Off Hand",
        category: Inventory,
        listed: true,
        defaults: [Keyboard(KeyCode::KeyF)],
    },
    SpectatorHotbar => {
        id: "key.spectatorHotbar",
        translation_id: Some(StringId::new("key.spectatorHotbar")),
        fallback: "Select On Hotbar",
        category: Spectator,
        listed: true,
        defaults: [Mouse(MouseButton::Middle)],
    },
}
