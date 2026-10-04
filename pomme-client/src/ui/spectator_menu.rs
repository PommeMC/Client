//! Spectator hotbar menu, vanilla `SpectatorGui` + `SpectatorMenu`: a digit
//! key opens a transient 9-slot command bar (teleport to player / team) that
//! fades out after five seconds.

use std::time::Instant;

use azalea_protocol::packets::game::ServerboundGamePacket;
use azalea_protocol::packets::game::s_teleport_to_entity::ServerboundTeleportToEntity;
use uuid::Uuid;

use super::common;
use crate::net::sender::PacketSender;
use crate::player::tab_list::{TabList, TabListPlayer};
use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId};
use crate::ui::hud::Scoreboard;
use crate::ui::text::TextSpan;

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
/// Vanilla `FADE_OUT_DELAY`: ms from the last interaction to fully faded.
const FADE_OUT_DELAY_MS: f32 = 5000.0;
/// Vanilla `FADE_OUT_TIME`: length of the linear fade at the end of the delay.
const FADE_OUT_TIME_MS: f32 = 2000.0;

#[derive(Default)]
pub struct SpectatorGuiState {
    menu: Option<SpectatorMenu>,
    /// Vanilla `lastSelectionTime`; `None` is its `0` (alpha reads 0).
    last_selection: Option<Instant>,
}

struct SpectatorMenu {
    category: Category,
    /// -1 = no selection (vanilla `SpectatorPage.NO_SELECTION`).
    selected_slot: i32,
    page: i32,
}

struct Category {
    prompt_key: &'static str,
    items: Vec<Item>,
}

#[derive(Clone)]
enum Item {
    /// Root "Teleport to Player" entry wrapping the player items.
    TeleportToPlayers(Vec<Item>),
    /// Root "Teleport to Team Member" entry wrapping the team items.
    TeleportToTeams(Vec<Item>),
    Player {
        uuid: Uuid,
        name: String,
        /// Game mode at menu open; `enabled` falls back to it once the player
        /// leaves, like vanilla holding onto the detached `PlayerInfo`.
        snapshot_game_mode: u8,
    },
    Team {
        display: Vec<TextSpan>,
        fill_color: Option<[f32; 4]>,
        /// Random member whose face becomes the icon, picked once at open.
        face_uuid: Uuid,
        players: Vec<Item>,
    },
}

/// A rendered slot; Close/Scroll/Empty are synthesized per slot by `get_item`
/// (vanilla's fixed CLOSE_ITEM / ScrollMenuItem / EMPTY_SLOT singletons).
enum SlotItem<'a> {
    Empty,
    Close,
    Scroll { direction: i32, enabled: bool },
    Item(&'a Item),
}

enum Activation {
    Select,
    Exit,
    Page(i32),
    OpenCategory {
        prompt_key: &'static str,
        items: Vec<Item>,
    },
    Teleport(Uuid),
}

impl SlotItem<'_> {
    fn is_enabled(&self, tab_list: &TabList) -> bool {
        match self {
            SlotItem::Empty => false,
            SlotItem::Close => true,
            SlotItem::Scroll { enabled, .. } => *enabled,
            SlotItem::Item(item) => item.is_enabled(tab_list),
        }
    }
}

impl Item {
    fn is_enabled(&self, tab_list: &TabList) -> bool {
        match self {
            Item::TeleportToPlayers(items) | Item::TeleportToTeams(items) => !items.is_empty(),
            // Vanilla PlayerMenuItem reads the live PlayerInfo game mode.
            Item::Player {
                uuid,
                snapshot_game_mode,
                ..
            } => tab_list
                .players
                .get(uuid)
                .map_or(*snapshot_game_mode != 3, |p| p.game_mode != 3),
            Item::Team { .. } => true,
        }
    }
}

/// Vanilla `TeleportToPlayerMenuCategory`: drop spectators, sort by UUID.
fn player_items(players: Vec<&TabListPlayer>) -> Vec<Item> {
    let mut players: Vec<_> = players.into_iter().filter(|p| p.game_mode != 3).collect();
    players.sort_by_key(|p| p.uuid);
    players
        .into_iter()
        .map(|p| Item::Player {
            uuid: p.uuid,
            name: p.name.clone(),
            snapshot_game_mode: p.game_mode,
        })
        .collect()
}

/// Vanilla `TeleportToTeamMenuCategory`: members resolve by name against the
/// full player-info map (no `listed` filter); spectators, offline names, and
/// then empty teams are dropped.
fn team_items(tab_list: &TabList, scoreboard: &Scoreboard) -> Vec<Item> {
    let mut items = Vec::new();
    for (name, team) in scoreboard.teams() {
        let members: Vec<&TabListPlayer> = team
            .members
            .iter()
            .filter_map(|member| tab_list.players.values().find(|p| &p.name == member))
            .filter(|p| p.game_mode != 3)
            .collect();
        if members.is_empty() {
            continue;
        }
        let face_uuid = members[fastrand::usize(..members.len())].uuid;
        let display = if team.display_name.is_empty() {
            // Vanilla's default team display name is the team name literal.
            vec![TextSpan::new(name.clone(), WHITE)]
        } else {
            team.display_name.clone()
        };
        items.push(Item::Team {
            display,
            fill_color: team.fill_color,
            face_uuid,
            players: player_items(members),
        });
    }
    items
}

impl SpectatorMenu {
    fn new(tab_list: &TabList, scoreboard: &Scoreboard) -> Self {
        // Vanilla TeleportToPlayerMenuCategory() snapshots the *listed*
        // players; the team category resolves against the full map.
        let players = player_items(tab_list.players.values().filter(|p| p.listed).collect());
        Self {
            category: Category {
                prompt_key: "spectatorMenu.root.prompt",
                items: vec![
                    Item::TeleportToPlayers(players),
                    Item::TeleportToTeams(team_items(tab_list, scoreboard)),
                ],
            },
            selected_slot: -1,
            page: 0,
        }
    }

    /// Vanilla `SpectatorMenu.getItem`, including the stride-6 quirk: page 0
    /// shows item indices 0..=6, page 1 shows 7..=12 via slots 1..=6 (index 6
    /// on page 0 is never reachable again after paging).
    fn get_item(&self, slot: i32) -> SlotItem<'_> {
        let index = slot + self.page * 6;
        if self.page > 0 && slot == 0 {
            return SlotItem::Scroll {
                direction: -1,
                enabled: true,
            };
        }
        if slot == 7 {
            return SlotItem::Scroll {
                direction: 1,
                enabled: (index as usize) < self.category.items.len(),
            };
        }
        if slot == 8 {
            return SlotItem::Close;
        }
        if index < 0 || index as usize >= self.category.items.len() {
            return SlotItem::Empty;
        }
        SlotItem::Item(&self.category.items[index as usize])
    }

    /// Vanilla `selectSlot`: the first press moves the selection, a second
    /// press on the same enabled slot activates it. Returns true when the
    /// activation closes the menu.
    fn select_slot(&mut self, slot: i32, tab_list: &TabList, sender: &PacketSender) -> bool {
        let activation = match self.get_item(slot) {
            SlotItem::Empty => return false,
            item if self.selected_slot != slot || !item.is_enabled(tab_list) => Activation::Select,
            SlotItem::Close => Activation::Exit,
            SlotItem::Scroll { direction, .. } => Activation::Page(direction),
            SlotItem::Item(Item::TeleportToPlayers(items)) => Activation::OpenCategory {
                prompt_key: "spectatorMenu.teleport.prompt",
                items: items.clone(),
            },
            SlotItem::Item(Item::TeleportToTeams(items)) => Activation::OpenCategory {
                prompt_key: "spectatorMenu.team_teleport.prompt",
                items: items.clone(),
            },
            SlotItem::Item(Item::Team { players, .. }) => Activation::OpenCategory {
                prompt_key: "spectatorMenu.teleport.prompt",
                items: players.clone(),
            },
            SlotItem::Item(Item::Player { uuid, .. }) => Activation::Teleport(*uuid),
        };
        match activation {
            Activation::Select => {
                self.selected_slot = slot;
                false
            }
            Activation::Exit => true,
            Activation::Page(direction) => {
                // Vanilla keeps the selection on the scroll slot so a repeat
                // press pages again.
                self.page += direction;
                false
            }
            Activation::OpenCategory { prompt_key, items } => {
                // Vanilla selectCategory: fresh selection and page.
                self.category = Category { prompt_key, items };
                self.selected_slot = -1;
                self.page = 0;
                false
            }
            Activation::Teleport(uuid) => {
                // Vanilla PlayerMenuItem: send and leave the menu open.
                sender.send(ServerboundGamePacket::TeleportToEntity(
                    ServerboundTeleportToEntity { uuid },
                ));
                false
            }
        }
    }
}

impl SpectatorGuiState {
    /// Digit key 1-9 in spectator mode. Vanilla `onHotbarSelected`: the press
    /// that opens the menu selects nothing.
    pub fn on_hotbar_selected(
        &mut self,
        slot: u8,
        tab_list: &TabList,
        scoreboard: &Scoreboard,
        sender: &PacketSender,
    ) {
        self.last_selection = Some(Instant::now());
        if self.menu.is_none() {
            self.menu = Some(SpectatorMenu::new(tab_list, scoreboard));
        } else {
            self.select(slot as i32, tab_list, sender);
        }
    }

    /// Middle click (vanilla `keySpectatorHotbar`): open the menu, or
    /// re-press the current selection to activate it.
    pub fn on_hotbar_action_key(
        &mut self,
        tab_list: &TabList,
        scoreboard: &Scoreboard,
        sender: &PacketSender,
    ) {
        self.last_selection = Some(Instant::now());
        match self.menu.as_ref().map(|m| m.selected_slot) {
            None => self.menu = Some(SpectatorMenu::new(tab_list, scoreboard)),
            Some(-1) => {}
            Some(slot) => self.select(slot, tab_list, sender),
        }
    }

    /// Vanilla `onMouseScrolled` (already sign-inverted by the caller): step
    /// in the scroll direction skipping empty and disabled slots.
    pub fn on_mouse_scrolled(&mut self, wheel: i32, tab_list: &TabList, sender: &PacketSender) {
        let Some(menu) = &self.menu else { return };
        let mut slot = menu.selected_slot + wheel;
        while (0..=8).contains(&slot) && !menu.get_item(slot).is_enabled(tab_list) {
            slot += wheel;
        }
        if (0..=8).contains(&slot) {
            self.select(slot, tab_list, sender);
            self.last_selection = Some(Instant::now());
        }
    }

    /// `selectSlot` plus the listener's close-on-exit.
    fn select(&mut self, slot: i32, tab_list: &TabList, sender: &PacketSender) {
        if let Some(menu) = &mut self.menu
            && menu.select_slot(slot, tab_list, sender)
        {
            self.close();
        }
    }

    pub fn is_menu_active(&self) -> bool {
        self.menu.is_some()
    }

    /// Vanilla `onSpectatorMenuClosed`: drop the menu and zero the timer so
    /// alpha reads 0 immediately.
    pub fn close(&mut self) {
        self.menu = None;
        self.last_selection = None;
    }

    /// Vanilla `getHotbarAlpha`: opaque for the first 3 s after the last
    /// interaction, then a 2 s linear fade.
    fn hotbar_alpha(&self) -> f32 {
        let Some(last) = self.last_selection else {
            return 0.0;
        };
        let elapsed = last.elapsed().as_millis() as f32;
        ((FADE_OUT_DELAY_MS - elapsed) / FADE_OUT_TIME_MS).clamp(0.0, 1.0)
    }
}

fn key_spans(key: &str) -> Vec<TextSpan> {
    let text = crate::lang::translate(key).unwrap_or(key).to_string();
    vec![TextSpan::new(text, WHITE)]
}

fn push_face(elements: &mut Vec<MenuElement>, x: f32, y: f32, gs: f32, uuid: Uuid, tint: [f32; 4]) {
    elements.push(MenuElement::SkinFace {
        x: x + 2.0 * gs,
        y: y + 2.0 * gs,
        size: 12.0 * gs,
        uuid: uuid.to_string(),
        tint,
    });
}

/// Vanilla `SpectatorGui.extractHotbar` + `extractAction`: the sliding,
/// fading command bar and the name/prompt line above it. Replaces the item
/// hotbar while in spectator mode.
pub fn build_spectator_menu(
    elements: &mut Vec<MenuElement>,
    state: &mut SpectatorGuiState,
    tab_list: &TabList,
    screen_w: f32,
    screen_h: f32,
    gs: f32,
    text_width: common::TextWidthFn,
) {
    if state.menu.is_none() {
        return;
    }
    let alpha = state.hotbar_alpha();
    if alpha <= 0.0 {
        // Vanilla extractHotbar: a fully faded menu closes itself.
        state.close();
        return;
    }
    let cx = screen_w / 2.0;
    // The bar slides down as it fades: y = floor(guiHeight - 22 * alpha).
    let y = (screen_h - 22.0 * alpha * gs).floor();
    let fade = [1.0, 1.0, 1.0, alpha];
    let menu = state.menu.as_ref().unwrap();

    elements.push(MenuElement::Image {
        x: (cx - 91.0 * gs).round(),
        y,
        w: 182.0 * gs,
        h: 22.0 * gs,
        sprite: SpriteId::Hotbar,
        tint: fade,
    });
    if menu.selected_slot >= 0 {
        elements.push(MenuElement::Image {
            x: (cx - 92.0 * gs + menu.selected_slot as f32 * 20.0 * gs).round(),
            y: y - 1.0 * gs,
            w: 24.0 * gs,
            h: 23.0 * gs,
            sprite: SpriteId::HotbarSelection,
            tint: fade,
        });
    }

    for slot in 0..9 {
        let item = menu.get_item(slot);
        if matches!(item, SlotItem::Empty) {
            continue;
        }
        let enabled = item.is_enabled(tab_list);
        // Disabled icons dim to 25% brightness; alpha fades separately.
        let b = if enabled { 1.0 } else { 0.25 };
        let icon_x = (cx - 88.0 * gs + slot as f32 * 20.0 * gs).round();
        let icon_y = y + 3.0 * gs;
        let sprite = match &item {
            SlotItem::Empty => None,
            SlotItem::Close => Some(SpriteId::SpectatorClose),
            SlotItem::Scroll { direction, .. } => Some(if *direction < 0 {
                SpriteId::SpectatorScrollLeft
            } else {
                SpriteId::SpectatorScrollRight
            }),
            SlotItem::Item(Item::TeleportToPlayers(_)) => Some(SpriteId::SpectatorTeleportToPlayer),
            SlotItem::Item(Item::TeleportToTeams(_)) => Some(SpriteId::SpectatorTeleportToTeam),
            SlotItem::Item(Item::Player { uuid, .. }) => {
                // Vanilla PlayerMenuItem: the face ignores the brightness dim.
                push_face(elements, icon_x, icon_y, gs, *uuid, fade);
                None
            }
            SlotItem::Item(Item::Team {
                fill_color,
                face_uuid,
                ..
            }) => {
                if let Some([r, g, bl, _]) = fill_color {
                    // Vanilla team icon: the color fill dims but never fades.
                    elements.push(MenuElement::Rect {
                        x: icon_x + 1.0 * gs,
                        y: icon_y + 1.0 * gs,
                        w: 14.0 * gs,
                        h: 14.0 * gs,
                        corner_radius: 0.0,
                        color: [r * b, g * b, bl * b, 1.0],
                    });
                }
                push_face(elements, icon_x, icon_y, gs, *face_uuid, [b, b, b, alpha]);
                None
            }
        };
        if let Some(sprite) = sprite {
            elements.push(MenuElement::Image {
                x: icon_x,
                y: icon_y,
                w: 16.0 * gs,
                h: 16.0 * gs,
                sprite,
                tint: [b, b, b, alpha],
            });
        }
        if enabled {
            let label = (slot + 1).to_string();
            let scale = common::FONT_SIZE * gs;
            elements.push(MenuElement::McText {
                x: icon_x + 17.0 * gs - text_width(&label, scale),
                y: y + 12.0 * gs,
                spans: vec![TextSpan::new(label, fade)],
                scale,
                centered: false,
                shadow: true,
            });
        }
    }

    // Vanilla extractAction: the selected item's name, or the category prompt
    // while nothing is selected. No backdrop: vanilla's textWithBackdrop
    // resolves to color 0 under the default text-background options.
    let mut spans = match menu.get_item(menu.selected_slot) {
        SlotItem::Empty => key_spans(menu.category.prompt_key),
        SlotItem::Close => key_spans("spectatorMenu.close"),
        SlotItem::Scroll { direction, .. } => key_spans(if direction < 0 {
            "spectatorMenu.previous_page"
        } else {
            "spectatorMenu.next_page"
        }),
        SlotItem::Item(Item::TeleportToPlayers(_)) => key_spans("spectatorMenu.teleport"),
        SlotItem::Item(Item::TeleportToTeams(_)) => key_spans("spectatorMenu.team_teleport"),
        SlotItem::Item(Item::Player { name, .. }) => vec![TextSpan::new(name.clone(), WHITE)],
        SlotItem::Item(Item::Team { display, .. }) => display.clone(),
    };
    for span in &mut spans {
        span.color[3] *= alpha;
    }
    elements.push(MenuElement::McText {
        x: cx,
        y: screen_h - 35.0 * gs,
        spans,
        scale: common::FONT_SIZE * gs,
        centered: true,
        shadow: true,
    });
}
