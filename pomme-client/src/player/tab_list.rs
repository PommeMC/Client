use std::collections::HashMap;

use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct PlayerInfoEntry {
    pub uuid: Uuid,
    pub name: String,
    /// 0 survival, 1 creative, 2 adventure, 3 spectator.
    pub game_mode: u8,
    pub listed: bool,
    pub latency: i32,
    pub display_name: Option<String>,
    pub list_order: i32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PlayerInfoActions {
    pub add_player: bool,
    pub update_game_mode: bool,
    pub update_listed: bool,
    pub update_latency: bool,
    pub update_display_name: bool,
    pub update_list_order: bool,
}

#[derive(Clone, Debug)]
pub struct TabListPlayer {
    #[allow(dead_code)]
    pub uuid: Uuid,
    pub name: String,
    pub display_name: Option<String>,
    pub game_mode: u8,
    pub latency: i32,
    pub listed: bool,
    pub list_order: i32,
}

#[derive(Default)]
pub struct TabList {
    pub players: HashMap<Uuid, TabListPlayer>,
    pub header: Option<String>,
    pub footer: Option<String>,
}

impl TabList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.players.clear();
        self.header = None;
        self.footer = None;
    }

    pub fn apply_update(&mut self, actions: &PlayerInfoActions, entries: &[PlayerInfoEntry]) {
        for e in entries {
            if actions.add_player {
                self.players.insert(
                    e.uuid,
                    TabListPlayer {
                        uuid: e.uuid,
                        name: e.name.clone(),
                        display_name: e.display_name.clone(),
                        game_mode: e.game_mode,
                        latency: e.latency,
                        listed: e.listed,
                        list_order: e.list_order,
                    },
                );
            } else if let Some(p) = self.players.get_mut(&e.uuid) {
                if actions.update_game_mode {
                    p.game_mode = e.game_mode;
                }
                if actions.update_listed {
                    p.listed = e.listed;
                }
                if actions.update_latency {
                    p.latency = e.latency;
                }
                if actions.update_display_name {
                    p.display_name = e.display_name.clone();
                }
                if actions.update_list_order {
                    p.list_order = e.list_order;
                }
            }
        }
    }

    pub fn remove(&mut self, uuids: &[Uuid]) {
        for id in uuids {
            self.players.remove(id);
        }
    }

    pub fn set_header_footer(&mut self, header: String, footer: String) {
        self.header = (!header.is_empty()).then_some(header);
        self.footer = (!footer.is_empty()).then_some(footer);
    }

    /// Vanilla PlayerTabOverlay PLAYER_COMPARATOR (PlayerTabOverlay.java:63).
    /// Teams aren't tracked, so the team-name tiebreaker is skipped.
    pub fn sorted_listed(&self) -> Vec<&TabListPlayer> {
        let mut out: Vec<&TabListPlayer> = self.players.values().filter(|p| p.listed).collect();
        out.sort_by(|a, b| {
            a.list_order
                .cmp(&b.list_order)
                .then_with(|| (a.game_mode == 3).cmp(&(b.game_mode == 3)))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        out.truncate(80);
        out
    }
}
