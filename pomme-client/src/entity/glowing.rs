//! Vanilla's glowing outline: which entities draw one, and in what colour.

use uuid::Uuid;

use crate::player::tab_list::TabList;
use crate::ui::hud::Scoreboard;

/// Vanilla `Minecraft.shouldEntityAppearGlowing`: the server's glowing flag,
/// or a player while a spectating local player holds `key.spectatorOutlines`.
pub fn appears_glowing(glowing: bool, is_player: bool, spectator_outlines: bool) -> bool {
    glowing || spectator_outlines && is_player
}

/// Vanilla `Entity.getTeamColor`: the colour of the team the entity's
/// scoreboard name belongs to (a player's name, anyone else's UUID string,
/// `getScoreboardName`), white without one.
pub fn outline_color(
    scoreboard: &Scoreboard,
    tab_list: &TabList,
    uuid: Uuid,
    player_uuid: Option<Uuid>,
) -> u32 {
    let name = match player_uuid.and_then(|id| tab_list.players.get(&id)) {
        Some(player) => player.name.clone(),
        None => uuid.to_string(),
    };
    scoreboard.member_team_color(&name).unwrap_or(0xFFFFFF)
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
        let color = outline_color(
            &Scoreboard::default(),
            &TabList::default(),
            Uuid::nil(),
            None,
        );
        assert_eq!(color, 0xFFFFFF);
    }
}
