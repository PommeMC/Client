//! The harness commands and the state snapshots they reply with, shared by
//! scripts and the socket (one JSON object per line).

use std::path::PathBuf;

use azalea_inventory::ItemStack;
use serde::{Deserialize, Serialize};

use crate::app::phases::in_game::GameState;
use crate::renderer::pipelines::entity_renderer::{
    CAT_VARIANT_ORDER, CHICKEN_VARIANT_ORDER, COW_VARIANT_ORDER, WOLF_VARIANT_ORDER,
};
use crate::ui::pause::PauseScreen;

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
    Click(Click),
    CreativeSetSlot(CreativeSetSlot),
    Inventory,
    Entities,
    Advancements,
    Player,
    /// The title, boss bars and tab list text on screen.
    Hud,
    /// The sound events most recently played, oldest first.
    Sounds,
    /// The block state the client holds at `at`.
    Block {
        at: [i32; 3],
    },
    /// Turns the player to face `at`, as a mouse look would.
    LookAt {
        at: [f64; 3],
    },
    MouseDown {
        button: Mouse,
    },
    /// Presses a key once: a key binding's default key, or a physical key.
    Press(Press),
    /// Assigns inputs to a key binding for this session, unsaved.
    Bind(Bind),
    /// Moves the cursor over a creative-inventory cell.
    Hover(Hover),
    MouseUp {
        button: Mouse,
    },
    ChatLog,
    /// A PNG of the next presented frame, written to `path`.
    Screenshot {
        path: PathBuf,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mouse {
    Left,
    Right,
    Middle,
}

impl From<Mouse> for winit::event::MouseButton {
    fn from(button: Mouse) -> Self {
        match button {
            Mouse::Left => Self::Left,
            Mouse::Right => Self::Right,
            Mouse::Middle => Self::Middle,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
pub struct Click {
    pub slot: u16,
    pub op: ClickKind,
}

/// One of a key binding's default key (`action`, e.g. `key.drop`) or a
/// physical key (`key`, an input id such as `key.keyboard.j`), pressed once,
/// optionally with Ctrl held.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Press {
    pub action: Option<String>,
    pub key: Option<String>,
    #[serde(default)]
    pub ctrl: bool,
}

/// Key binding `action` set to `keys` (input ids), or back to its defaults
/// when `keys` is absent.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Bind {
    pub action: String,
    pub keys: Option<Vec<String>>,
}

/// A creative item-tab cell: one of `slot` (hotbar, 36-44) or `catalog`.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Hover {
    pub slot: Option<u16>,
    pub catalog: Option<usize>,
}

/// Sets inventory slot `slot` to a copy of slot `from`'s stack, as the
/// creative screen places a cloned stack. Creative mode only.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
pub struct CreativeSetSlot {
    pub slot: u16,
    pub from: u16,
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

impl Click {
    pub fn operation(self) -> azalea_inventory::operations::ClickOperation {
        use azalea_inventory::operations::{
            ClickOperation, PickupClick, QuickMoveClick, SwapClick,
        };
        let slot = self.slot;
        match self.op {
            ClickKind::PickupLeft => ClickOperation::Pickup(PickupClick::Left { slot: Some(slot) }),
            ClickKind::PickupRight => {
                ClickOperation::Pickup(PickupClick::Right { slot: Some(slot) })
            }
            ClickKind::QuickMove => QuickMoveClick::Left { slot }.into(),
            ClickKind::Swap { hotbar } => SwapClick {
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
    /// `inventory`, `creative`, a container id, `pause`, or `none`.
    pub menu: String,
    /// The open container's vanilla menu type, e.g. `generic_9x3`.
    pub menu_type: Option<&'static str>,
    pub title: Option<String>,
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
            None if game.paused && !matches!(game.pause_screen, PauseScreen::Hidden) => {
                "pause".into()
            }
            None => "none".into(),
        }
    };
    let container = game.open_container.as_ref();
    InventoryInfo {
        menu,
        menu_type: container.map(|c| c.menu.to_str().trim_start_matches("minecraft:")),
        title: container.map(|c| c.title.clone()),
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
    pub position: [f64; 3],
    /// Body yaw in degrees.
    pub y_rot: f32,
    pub variant: u32,
    /// The variant's asset name, for the kinds with a texture pool.
    pub variant_name: Option<&'static str>,
    pub baby: bool,
    pub crouching: bool,
    pub sleeping: bool,
    pub custom_name: Option<String>,
    /// Villagers' and zombie villagers' type, profession and level.
    pub villager: Option<VillagerInfo>,
    pub equipment: Vec<(String, StackInfo)>,
}

#[derive(Debug, Serialize)]
pub struct VillagerInfo {
    pub kind: String,
    pub profession: String,
    pub level: u32,
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
                EntityKind::Wolf => WOLF_VARIANT_ORDER,
                _ => &[],
            };
            let villager = matches!(
                entity.entity_type,
                EntityKind::Villager | EntityKind::ZombieVillager
            )
            .then(|| VillagerInfo {
                kind: format!("{:?}", entity.villager_kind).to_lowercase(),
                profession: format!("{:?}", entity.villager_profession).to_lowercase(),
                level: entity.villager_level,
            });
            LivingInfo {
                id: *id,
                kind: bare(&entity.entity_type.to_string()),
                position: glam::DVec3::from(entity.position).to_array(),
                y_rot: entity.look_dir.y_rot_deg(),
                variant: entity.variant,
                variant_name: order.get(entity.variant as usize).copied(),
                baby: entity.is_baby,
                crouching: entity.is_crouching,
                sleeping: entity.sleeping_pos.is_some(),
                custom_name: entity.custom_name.clone(),
                villager,
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

#[derive(Debug, Serialize)]
pub struct PlayerInfo {
    pub health: f32,
    pub food: u32,
    pub level: i32,
    pub game_mode: u8,
    pub effects: Vec<&'static str>,
    pub max_health: Option<f64>,
    pub raining: bool,
    /// Whether the first-person hand is drawn.
    pub hand: bool,
    /// The dimension, e.g. `overworld`.
    pub dimension: String,
    pub day_time: u64,
    pub position: [f64; 3],
    /// Yaw and pitch, in degrees.
    pub rotation: [f32; 2],
}

pub fn player(game: &GameState) -> PlayerInfo {
    let player = &game.player;
    PlayerInfo {
        health: player.health,
        food: player.food,
        level: player.experience_level,
        game_mode: player.game_mode,
        effects: player.effects.names().collect(),
        max_health: player
            .attributes
            .value(crate::attribute::AttributeKind::MaxHealth),
        raining: game.sky_state.rain_level > 0.0,
        hand: crate::app::phases::in_game::renders_first_person_hand(game),
        dimension: bare(&game.dimension),
        day_time: game.sky_state.day_time,
        position: [player.position.x, player.position.y, player.position.z],
        rotation: [player.look_dir.y_rot_deg(), player.look_dir.x_rot_deg()],
    }
}

#[derive(Debug, Serialize)]
pub struct HudInfo {
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub boss_bars: Vec<BossBarInfo>,
    pub tab_header: Option<String>,
    pub tab_footer: Option<String>,
    pub sidebar_title: Option<String>,
    /// Each row's name, then its number text after a space when it has one.
    pub sidebar: Vec<String>,
    /// The listed players, in the tab overlay's order.
    pub tab_list: Vec<TabEntryInfo>,
}

#[derive(Debug, Serialize)]
pub struct TabEntryInfo {
    pub name: String,
    pub game_mode: u8,
    pub latency: i32,
}

#[derive(Debug, Serialize)]
pub struct BossBarInfo {
    pub name: String,
    pub progress: f32,
}

pub fn hud(game: &GameState) -> HudInfo {
    use crate::ui::text::span_text;

    let shown = game.title.shown();
    let sidebar = game.scoreboard.sidebar();
    HudInfo {
        title: shown.map(|(title, _)| span_text(title)),
        subtitle: shown.and_then(|(_, subtitle)| subtitle).map(span_text),
        boss_bars: game
            .boss_bars
            .iter()
            .map(|bar| BossBarInfo {
                name: span_text(&bar.name),
                progress: bar.progress(),
            })
            .collect(),
        tab_header: game.tab_list.header.as_deref().map(span_text),
        tab_footer: game.tab_list.footer.as_deref().map(span_text),
        sidebar_title: sidebar.as_ref().map(|(title, _)| span_text(title)),
        sidebar: sidebar
            .into_iter()
            .flat_map(|(_, rows)| rows)
            .map(|(name, number)| {
                let (name, number) = (span_text(&name), span_text(&number));
                if number.is_empty() {
                    name
                } else {
                    format!("{name} {number}")
                }
            })
            .collect(),
        tab_list: game
            .tab_list
            .sorted_listed(&game.scoreboard)
            .into_iter()
            .map(|player| TabEntryInfo {
                name: player.name.clone(),
                game_mode: player.game_mode,
                latency: player.latency,
            })
            .collect(),
    }
}

/// The block state name at `at`, e.g. `stone`; air when its chunk isn't
/// loaded.
pub fn block(game: &GameState, [x, y, z]: [i32; 3]) -> String {
    bare(
        game.block_registry
            .block_state_data(game.chunk_store.get_block_state(x, y, z))
            .id(),
    )
}

/// The type of the block entity the client stores at `at`, e.g. `sign`.
pub fn block_entity(game: &GameState, [x, y, z]: [i32; 3]) -> Option<String> {
    let stored = game
        .chunk_store
        .block_entities
        .get(&crate::position::BlockPos { x, y, z })?;
    Some(bare(&stored.kind.to_string()))
}

/// Vanilla `Entity.lookAt`'s yaw and pitch from `eye` to `at`, in degrees.
pub fn look_angles(eye: glam::DVec3, at: glam::DVec3) -> (f32, f32) {
    let d = at - eye;
    let horizontal = d.x.hypot(d.z);
    let y_rot = d.z.atan2(d.x).to_degrees() as f32 - 90.0;
    let x_rot = -d.y.atan2(horizontal).to_degrees() as f32;
    (y_rot, x_rot)
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
            Command::Click(Click {
                slot: 9,
                op: ClickKind::Swap { hotbar: 40 }
            })
        );
        assert!(serde_json::from_str::<Command>(r#"{"cmd":"click","slot":9}"#).is_err());
    }
}
