//! The harness commands and the state snapshots they reply with, shared by
//! scripts and the socket (one JSON object per line).

use std::path::PathBuf;

use azalea_inventory::ItemStack;
use serde::{Deserialize, Serialize};

use crate::app::phases::in_game::GameState;
use crate::renderer::pipelines::entity_renderer::{
    CAT_VARIANT_ORDER, CHICKEN_VARIANT_ORDER, COW_VARIANT_ORDER,
};

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// The app phase: `menu`, `connecting` or `in_game`.
    Status,
    /// Chat input; a leading `/` sends a command.
    Chat {
        text: String,
    },
    /// The player inventory, as the inventory key opens it.
    OpenInventory,
    /// Closes the open menu, as the inventory key does.
    CloseMenu,
    Click {
        slot: u16,
        op: ClickKind,
    },
    Inventory,
    Entities,
    Advancements,
    ChatLog,
    /// A PNG of the next presented frame, written to `path`.
    Screenshot {
        path: PathBuf,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClickKind {
    PickupLeft,
    PickupRight,
    QuickMove,
    /// The number key for hotbar slot `hotbar` (0-8; 40 is the offhand).
    Swap {
        hotbar: u8,
    },
}

impl ClickKind {
    pub fn operation(self, slot: u16) -> azalea_inventory::operations::ClickOperation {
        use azalea_inventory::operations::{
            ClickOperation, PickupClick, QuickMoveClick, SwapClick,
        };
        match self {
            Self::PickupLeft => ClickOperation::Pickup(PickupClick::Left { slot: Some(slot) }),
            Self::PickupRight => ClickOperation::Pickup(PickupClick::Right { slot: Some(slot) }),
            Self::QuickMove => QuickMoveClick::Left { slot }.into(),
            Self::Swap { hotbar } => SwapClick {
                source_slot: slot,
                target_slot: hotbar,
            }
            .into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StackInfo {
    pub item: String,
    pub count: i32,
    /// The component types the stack's patch adds.
    pub components: Vec<String>,
}

impl StackInfo {
    pub fn of(stack: &ItemStack) -> Option<Self> {
        let data = stack.as_present()?;
        Some(Self {
            item: crate::player::inventory::item_resource_name(data.kind),
            count: data.count,
            components: data
                .component_patch
                .iter()
                .filter(|(_, value)| value.is_some())
                .map(|(kind, _)| bare(&kind.to_string()))
                .collect(),
        })
    }
}

#[derive(Debug, Serialize)]
pub struct SlotInfo {
    pub slot: usize,
    #[serde(flatten)]
    pub stack: StackInfo,
}

#[derive(Debug, Serialize)]
pub struct InventoryInfo {
    /// `inventory`, `creative`, a container id, or `none`.
    pub menu: String,
    pub slots: Vec<SlotInfo>,
    pub cursor: Option<StackInfo>,
}

pub fn inventory(game: &GameState) -> InventoryInfo {
    let menu = if game.creative_inventory_open {
        "creative".into()
    } else {
        match game.open_menu_id() {
            Some(0) => "inventory".into(),
            Some(id) => id.to_string(),
            None => "none".into(),
        }
    };
    InventoryInfo {
        menu,
        slots: game
            .menu_slots()
            .iter()
            .enumerate()
            .filter_map(|(slot, stack)| {
                Some(SlotInfo {
                    slot,
                    stack: StackInfo::of(stack)?,
                })
            })
            .collect(),
        cursor: StackInfo::of(&game.cursor_item),
    }
}

#[derive(Debug, Serialize)]
pub struct LivingInfo {
    pub id: i32,
    pub kind: String,
    pub variant: u32,
    /// The variant's asset name, for the kinds with a texture pool.
    pub variant_name: Option<&'static str>,
    pub baby: bool,
    pub crouching: bool,
    pub sleeping: bool,
    pub equipment: Vec<(String, StackInfo)>,
}

#[derive(Debug, Serialize)]
pub struct ItemEntityInfo {
    pub id: i32,
    pub item: String,
    pub count: i32,
}

#[derive(Debug, Serialize)]
pub struct EntitiesInfo {
    pub living: Vec<LivingInfo>,
    pub items: Vec<ItemEntityInfo>,
}

pub fn entities(game: &GameState) -> EntitiesInfo {
    use azalea_registry::builtin::EntityKind;

    let living = game
        .entity_store
        .living
        .iter()
        .map(|(id, entity)| {
            let order = match entity.entity_type {
                EntityKind::Cow => COW_VARIANT_ORDER,
                EntityKind::Chicken => CHICKEN_VARIANT_ORDER,
                EntityKind::Cat => CAT_VARIANT_ORDER,
                _ => &[],
            };
            LivingInfo {
                id: *id,
                kind: bare(&entity.entity_type.to_string()),
                variant: entity.variant,
                variant_name: order.get(entity.variant as usize).copied(),
                baby: entity.is_baby,
                crouching: entity.is_crouching,
                sleeping: entity.sleeping_pos.is_some(),
                equipment: entity
                    .equipment
                    .iter()
                    .filter_map(|(slot, stack)| {
                        Some((format!("{slot:?}").to_lowercase(), StackInfo::of(stack)?))
                    })
                    .collect(),
            }
        })
        .collect();
    let items = game
        .item_entity_store
        .items()
        .map(|(id, item)| ItemEntityInfo {
            id,
            item: item.item_name.clone(),
            count: item.count,
        })
        .collect();
    EntitiesInfo { living, items }
}

/// A registry name without the `minecraft:` namespace.
fn bare(name: &str) -> String {
    name.strip_prefix("minecraft:").unwrap_or(name).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_parse_from_socket_lines() {
        let parse = |line| serde_json::from_str::<Command>(line).unwrap();
        assert_eq!(parse(r#"{"cmd":"status"}"#), Command::Status);
        assert_eq!(
            parse(r#"{"cmd":"chat","text":"/give @s stone"}"#),
            Command::Chat {
                text: "/give @s stone".into()
            }
        );
        assert_eq!(
            parse(r#"{"cmd":"click","slot":9,"op":{"swap":{"hotbar":40}}}"#),
            Command::Click {
                slot: 9,
                op: ClickKind::Swap { hotbar: 40 }
            }
        );
        assert!(serde_json::from_str::<Command>(r#"{"cmd":"click","slot":9}"#).is_err());
    }
}
