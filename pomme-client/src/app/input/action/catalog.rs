//! Logical game actions, display metadata, default inputs and UI ordering.

use std::borrow::Cow;

use gilrs::Button;
use winit::event::MouseButton;
use winit::keyboard::KeyCode;

use super::super::InputKey::{Controller, Keyboard, Mouse};
use super::{ActionCategoryMetadata, ActionId, ActionMetadata, DisplayText, StringId};

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
    Debug => {
        translation_id: Some(StringId::new("key.category.minecraft.debug")),
        fallback: "Debug",
        ui_order: 7,
    },
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
            $(ui_order: $ui_order:expr,)?
            $(fixed_inputs: [$($fixed:expr),* $(,)?],)?
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
                        ui_order: 0 $(+ $ui_order)?,
                        defaults: &[$($default,)*],
                        fixed_inputs: &[$($($fixed,)*)?],
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

actions! {
    Forward => {
        id: "key.forward",
        translation_id: Some(StringId::new("key.forward")),
        fallback: "Walk Forward",
        category: Movement,
        listed: true,
        defaults: [Keyboard(KeyCode::KeyW)],
    },
    Backward => {
        id: "key.back",
        translation_id: Some(StringId::new("key.back")),
        fallback: "Walk Backward",
        category: Movement,
        listed: true,
        defaults: [Keyboard(KeyCode::KeyS)],
    },
    StrafeLeft => {
        id: "key.left",
        translation_id: Some(StringId::new("key.left")),
        fallback: "Strafe Left",
        category: Movement,
        listed: true,
        defaults: [Keyboard(KeyCode::KeyA)],
    },
    StrafeRight => {
        id: "key.right",
        translation_id: Some(StringId::new("key.right")),
        fallback: "Strafe Right",
        category: Movement,
        listed: true,
        defaults: [Keyboard(KeyCode::KeyD)],
    },
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
        fallback: "Open Pause Menu",
        category: Miscellaneous,
        listed: true,
        fixed_inputs: [Keyboard(KeyCode::Escape)],
        defaults: [Controller(Button::Start)],
    },
    ViewPlayerList => {
        id: "key.playerlist",
        translation_id: Some(StringId::new("key.playerlist")),
        fallback: "List Players",
        category: Multiplayer,
        listed: true,
        defaults: [Keyboard(KeyCode::Tab), Controller(Button::Select)],
    },
    Screenshot => {
        id: "key.screenshot",
        translation_id: Some(StringId::new("key.screenshot")),
        fallback: "Take Screenshot",
        category: Miscellaneous,
        listed: true,
        defaults: [Keyboard(KeyCode::F2)],
    },
    ToggleFullscreen => {
        id: "key.fullscreen",
        translation_id: Some(StringId::new("key.fullscreen")),
        fallback: "Toggle Fullscreen",
        category: Miscellaneous,
        listed: true,
        defaults: [Keyboard(KeyCode::F11)],
    },
    ToggleGui => {
        id: "key.toggleGui",
        translation_id: Some(StringId::new("key.toggleGui")),
        fallback: "Toggle GUI",
        category: Miscellaneous,
        listed: true,
        defaults: [Keyboard(KeyCode::F1)],
    },
    ToggleOverlay => {
        id: "key.debug.overlay",
        translation_id: Some(StringId::new("key.debug.overlay")),
        fallback: "Toggle Overlay",
        category: Debug,
        listed: true,
        ui_order: -2,
        defaults: [Keyboard(KeyCode::F3)],
    },
    DebugModifier => {
        id: "key.debug.modifier",
        translation_id: Some(StringId::new("key.debug.modifier")),
        fallback: "Debug Modifier Key",
        category: Debug,
        listed: true,
        ui_order: -1,
        defaults: [Keyboard(KeyCode::F3)],
    },
    // Debug shortcuts have ordinary editable bindings; modifier qualification
    // is separate from their catalog metadata.
    DebugReloadChunks => {
        id: "key.debug.reloadChunk",
        translation_id: Some(StringId::new("key.debug.reloadChunk")),
        fallback: "Reload Chunks",
        category: Debug,
        listed: true,
        defaults: [Keyboard(KeyCode::KeyA)],
    },
    DebugCopyLocation => {
        id: "key.debug.copyLocation",
        translation_id: Some(StringId::new("key.debug.copyLocation")),
        fallback: "Copy Location",
        category: Debug,
        listed: true,
        defaults: [Keyboard(KeyCode::KeyC)],
    },
    DebugClearChat => {
        id: "key.debug.clearChat",
        translation_id: Some(StringId::new("key.debug.clearChat")),
        fallback: "Clear Chat",
        category: Debug,
        listed: true,
        defaults: [Keyboard(KeyCode::KeyD)],
    },
    DebugChunkBorders => {
        id: "key.debug.showChunkBorders",
        translation_id: Some(StringId::new("key.debug.showChunkBorders")),
        fallback: "Show Chunk Boundaries",
        category: Debug,
        listed: true,
        defaults: [Keyboard(KeyCode::KeyG)],
    },
    DebugAdvancedTooltips => {
        id: "key.debug.showAdvancedTooltips",
        translation_id: Some(StringId::new("key.debug.showAdvancedTooltips")),
        fallback: "Show Advanced Tooltips",
        category: Debug,
        listed: true,
        defaults: [Keyboard(KeyCode::KeyH)],
    },
    DebugCopyData => {
        id: "key.debug.copyRecreateCommand",
        translation_id: Some(StringId::new("key.debug.copyRecreateCommand")),
        fallback: "Copy Data",
        category: Debug,
        listed: true,
        defaults: [Keyboard(KeyCode::KeyI)],
    },
    DebugGameModeSwitcher => {
        id: "key.debug.switchGameMode",
        translation_id: Some(StringId::new("key.debug.switchGameMode")),
        fallback: "Game Mode Switcher",
        category: Debug,
        listed: true,
        defaults: [Keyboard(KeyCode::F4)],
    },
    DebugSpectate => {
        id: "key.debug.spectate",
        translation_id: Some(StringId::new("key.debug.spectate")),
        fallback: "Cycle Spectator",
        category: Debug,
        listed: true,
        defaults: [Keyboard(KeyCode::KeyN)],
    },
    DebugChunkOcclusion => {
        id: "key.debug.chunkOcclusion",
        translation_id: None,
        fallback: "Toggle Chunk Occlusion",
        category: Debug,
        listed: true,
        defaults: [Keyboard(KeyCode::KeyO)],
    },
    DebugDumpVersion => {
        id: "key.debug.dumpVersion",
        translation_id: Some(StringId::new("key.debug.dumpVersion")),
        fallback: "Dump Version Info",
        category: Debug,
        listed: true,
        defaults: [Keyboard(KeyCode::KeyV)],
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
    Close => {
        id: "key.close",
        translation_id: None,
        fallback: "Cancel/Close",
        category: Miscellaneous,
        listed: true,
        fixed_inputs: [Keyboard(KeyCode::Escape)],
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
    NextHotbarSlot => {
        id: "key.hotbar.next",
        translation_id: None,
        fallback: "Next Hotbar Slot",
        category: Inventory,
        listed: true,
        defaults: [Controller(Button::RightTrigger)],
    },
    PreviousHotbarSlot => {
        id: "key.hotbar.previous",
        translation_id: None,
        fallback: "Previous Hotbar Slot",
        category: Inventory,
        listed: true,
        defaults: [Controller(Button::LeftTrigger)],
    },
    Hotbar1 => {
        id: "key.hotbar.1",
        translation_id: Some(StringId::new("key.hotbar.1")),
        fallback: "Hotbar Slot 1",
        category: Inventory,
        listed: true,
        defaults: [Keyboard(KeyCode::Digit1)],
    },
    Hotbar2 => {
        id: "key.hotbar.2",
        translation_id: Some(StringId::new("key.hotbar.2")),
        fallback: "Hotbar Slot 2",
        category: Inventory,
        listed: true,
        defaults: [Keyboard(KeyCode::Digit2)],
    },
    Hotbar3 => {
        id: "key.hotbar.3",
        translation_id: Some(StringId::new("key.hotbar.3")),
        fallback: "Hotbar Slot 3",
        category: Inventory,
        listed: true,
        defaults: [Keyboard(KeyCode::Digit3)],
    },
    Hotbar4 => {
        id: "key.hotbar.4",
        translation_id: Some(StringId::new("key.hotbar.4")),
        fallback: "Hotbar Slot 4",
        category: Inventory,
        listed: true,
        defaults: [Keyboard(KeyCode::Digit4)],
    },
    Hotbar5 => {
        id: "key.hotbar.5",
        translation_id: Some(StringId::new("key.hotbar.5")),
        fallback: "Hotbar Slot 5",
        category: Inventory,
        listed: true,
        defaults: [Keyboard(KeyCode::Digit5)],
    },
    Hotbar6 => {
        id: "key.hotbar.6",
        translation_id: Some(StringId::new("key.hotbar.6")),
        fallback: "Hotbar Slot 6",
        category: Inventory,
        listed: true,
        defaults: [Keyboard(KeyCode::Digit6)],
    },
    Hotbar7 => {
        id: "key.hotbar.7",
        translation_id: Some(StringId::new("key.hotbar.7")),
        fallback: "Hotbar Slot 7",
        category: Inventory,
        listed: true,
        defaults: [Keyboard(KeyCode::Digit7)],
    },
    Hotbar8 => {
        id: "key.hotbar.8",
        translation_id: Some(StringId::new("key.hotbar.8")),
        fallback: "Hotbar Slot 8",
        category: Inventory,
        listed: true,
        defaults: [Keyboard(KeyCode::Digit8)],
    },
    Hotbar9 => {
        id: "key.hotbar.9",
        translation_id: Some(StringId::new("key.hotbar.9")),
        fallback: "Hotbar Slot 9",
        category: Inventory,
        listed: true,
        defaults: [Keyboard(KeyCode::Digit9)],
    },
    PickBlock => {
        id: "key.pickItem",
        translation_id: Some(StringId::new("key.pickItem")),
        fallback: "Pick Block",
        category: Gameplay,
        listed: true,
        defaults: [Mouse(MouseButton::Middle)],
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
