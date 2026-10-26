//! Vanilla's glowing outline: which entities draw one, and in what colour.

use uuid::Uuid;

use crate::player::tab_list::TabList;
use crate::ui::hud::Scoreboard;

/// Vanilla `Minecraft.shouldEntityAppearGlowing`: the server's glowing flag,
/// or a player while a spectating local player holds `key.spectatorOutlines`.
pub fn appears_glowing(glowing: bool, is_player: bool, spectator_outlines: bool) -> bool {
    glowing || spectator_outlines && is_player
}

/// Vanilla `getScoreboardName`: a player's name, anyone else's UUID string.
pub fn scoreboard_name(tab_list: &TabList, uuid: Uuid, player_uuid: Option<Uuid>) -> String {
    match player_uuid.and_then(|id| tab_list.players.get(&id)) {
        Some(player) => player.name.clone(),
        None => uuid.to_string(),
    }
}

/// Vanilla `Entity.getTeamColor`: the colour of the team the entity's
/// scoreboard name belongs to, white without one.
pub fn outline_color(scoreboard: &Scoreboard, name: &str) -> u32 {
    scoreboard.member_team_color(name).unwrap_or(0xFFFFFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spectator_outlines_only_light_players() {
        assert!(appears_glowing(true, false, false));
        assert!(appears_glowing(false, true, true));
        assert!(!appears_glowing(false, false, true));
        assert!(!appears_glowing(false, true, false));
    }

    #[test]
    fn outline_color_falls_back_to_white() {
        assert_eq!(outline_color(&Scoreboard::default(), "anyone"), 0xFFFFFF);
    }

    #[test]
    fn non_players_score_by_uuid() {
        let uuid = Uuid::from_u128(7);
        assert_eq!(
            scoreboard_name(&TabList::default(), uuid, None),
            uuid.to_string()
        );
    }
}
