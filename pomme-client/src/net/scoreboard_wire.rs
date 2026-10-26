//! Scoreboard objective, score, display slot and team packets, decoded by
//! pomme for every session: translated frames already carry the native
//! layout.

use crossbeam_channel::Sender;
use pomme_protocol::codec::DecodeResult;
use pomme_protocol::scoreboard::{
    self, NumberFormat, ObjectiveMethod, ScoreboardPacket, TeamMethod,
};

use super::NetworkEvent;
use super::chat::component_spans;
use super::handler::send_ordered;
use crate::ui::hud::{CollisionRule, ScoreNumberFormat};

/// `DisplaySlot.SIDEBAR`'s id.
const SIDEBAR: u32 = 1;

/// Decodes a scoreboard packet; `None` for any other packet.
pub(super) fn handle_raw_scoreboard_packet(
    packet_id: u32,
    payload: &[u8],
    event_tx: &Sender<NetworkEvent>,
) -> Option<DecodeResult<()>> {
    Some(
        super::read_raw_packet(packet_id, payload, scoreboard::read)?.map(|packet| {
            if let Some(event) = event(packet) {
                send_ordered(event_tx, event);
            }
        }),
    )
}

/// The client event for a packet, if the client acts on it; `None` too when
/// its text is unreadable.
fn event(packet: ScoreboardPacket) -> Option<NetworkEvent> {
    Some(match packet {
        ScoreboardPacket::Objective { name, method } => {
            let (display, number_format) = match method {
                ObjectiveMethod::Add(parameters) | ObjectiveMethod::Change(parameters) => (
                    Some(component_spans(&parameters.display_name)?),
                    super::present(parameters.number_format, number_format)?,
                ),
                ObjectiveMethod::Remove => (None, None),
            };
            NetworkEvent::ScoreboardObjective {
                name,
                display,
                number_format,
            }
        }
        // TODO: track the other display slots too — vanilla prefers the
        // local team's `sidebar.team.<color>` slot over SIDEBAR, and LIST
        // drives the tab-overlay score column.
        ScoreboardPacket::Display { slot, objective } => {
            if slot != SIDEBAR {
                return None;
            }
            NetworkEvent::ScoreboardDisplay {
                name: (!objective.is_empty()).then_some(objective),
            }
        }
        ScoreboardPacket::Score {
            owner,
            objective,
            score,
            display,
            number_format,
        } => NetworkEvent::ScoreboardScore {
            owner,
            objective,
            score,
            display: super::present(display.as_deref(), component_spans)?,
            number_format: super::present(number_format, self::number_format)?,
        },
        ScoreboardPacket::ResetScore { owner, objective } => {
            NetworkEvent::ScoreboardReset { owner, objective }
        }
        ScoreboardPacket::Team { name, method } => match method {
            TeamMethod::Add {
                parameters,
                players,
            } => team(name, parameters, Some(players))?,
            TeamMethod::Change(parameters) => team(name, parameters, None)?,
            TeamMethod::Remove => NetworkEvent::ScoreboardTeamRemoved { name },
            TeamMethod::Join(members) => NetworkEvent::ScoreboardTeamMembers {
                name,
                members,
                join: true,
            },
            TeamMethod::Leave(members) => NetworkEvent::ScoreboardTeamMembers {
                name,
                members,
                join: false,
            },
        },
    })
}

/// A team's add or change; prefix and suffix draw over the team color.
fn team(
    name: String,
    parameters: scoreboard::TeamParameters,
    members: Option<Vec<String>>,
) -> Option<NetworkEvent> {
    let fill_color = parameters.color.map(team_color);
    let color = crate::ui::common::rgb(fill_color.unwrap_or(0xFFFFFF));
    Some(NetworkEvent::ScoreboardTeam {
        name,
        display_name: component_spans(&parameters.display_name)?,
        prefix: super::chat::component_spans_over(&parameters.player_prefix, color)?,
        suffix: super::chat::component_spans_over(&parameters.player_suffix, color)?,
        color,
        fill_color,
        collision_rule: collision_rule(parameters.collision_rule),
        // `PlayerTeam.unpackOptions`: bit 2.
        see_friendly_invisibles: parameters.options & 2 != 0,
        members,
    })
}

fn number_format(format: NumberFormat) -> Option<ScoreNumberFormat> {
    Some(match format {
        NumberFormat::Blank => ScoreNumberFormat::Blank,
        // Vanilla applies the style to the plain number; only the color
        // matters for rendering, an unstyled number stays white.
        NumberFormat::Styled(style) => ScoreNumberFormat::Styled(styled_color(&style)?),
        NumberFormat::Fixed(text) => ScoreNumberFormat::Fixed(component_spans(&text)?),
    })
}

fn styled_color(nbt: &[u8]) -> Option<[f32; 4]> {
    let style = super::chat::read_nbt_tag(nbt, &mut 0)
        .ok()
        .and_then(|tag| crate::chat_component::Style::from_nbt_tag(&tag).ok());
    let Some(style) = style else {
        tracing::debug!("Ignoring a scoreboard packet with an unreadable number style");
        return None;
    };
    Some(crate::ui::common::rgb(style.color.unwrap_or(0xFFFFFF)))
}

/// Vanilla `TeamColor` id N is the legacy formatting code for hex digit N;
/// out-of-range ids fall back to BLACK (`ByIdMap` ZERO).
fn team_color(id: u32) -> u32 {
    char::from_digit(id, 16)
        .and_then(crate::ui::text::legacy_rgb)
        .unwrap_or(0x000000)
}

/// `Team.CollisionRule` by id; out-of-range ids fall back to ALWAYS.
fn collision_rule(id: u32) -> CollisionRule {
    match id {
        1 => CollisionRule::Never,
        2 => CollisionRule::PushOtherTeams,
        3 => CollisionRule::PushOwnTeam,
        _ => CollisionRule::Always,
    }
}

#[cfg(test)]
mod tests {
    use pomme_protocol::scoreboard::TeamParameters;

    use super::*;

    /// The NBT string tag `"x"`.
    const TEXT: [u8; 4] = [8, 0, 1, b'x'];

    fn parameters(color: Option<u32>) -> TeamParameters {
        TeamParameters {
            display_name: TEXT.into(),
            player_prefix: TEXT.into(),
            player_suffix: TEXT.into(),
            name_tag_visibility: 0,
            collision_rule: 0,
            color,
            options: 0,
        }
    }

    #[test]
    fn team_color_fills_and_tints_the_affixes() {
        let Some(NetworkEvent::ScoreboardTeam {
            name,
            prefix,
            color,
            fill_color,
            members,
            ..
        }) = event(ScoreboardPacket::Team {
            name: "crew".into(),
            method: TeamMethod::Add {
                parameters: parameters(Some(12)),
                players: vec!["Player".into()],
            },
        })
        else {
            panic!("expected ScoreboardTeam");
        };
        assert_eq!(name, "crew");
        assert_eq!(color, crate::ui::common::rgb(0xFF5555));
        assert_eq!(prefix[0].color, color);
        assert_eq!(fill_color, Some(0xFF5555));
        assert_eq!(members, Some(vec!["Player".to_string()]));
    }

    #[test]
    fn team_without_a_color_has_no_fill() {
        let Some(NetworkEvent::ScoreboardTeam {
            color,
            fill_color,
            members,
            ..
        }) = event(ScoreboardPacket::Team {
            name: "crew".into(),
            method: TeamMethod::Change(parameters(None)),
        })
        else {
            panic!("expected ScoreboardTeam");
        };
        assert_eq!(color, [1.0; 4]);
        assert_eq!((fill_color, members), (None, None));
    }

    #[test]
    fn team_color_and_collision_rule_follow_vanilla_ids() {
        assert_eq!(team_color(0), 0x000000);
        assert_eq!(team_color(12), 0xFF5555);
        assert_eq!(team_color(15), 0xFFFFFF);
        assert_eq!(team_color(16), 0x000000);
        assert_eq!(collision_rule(1), CollisionRule::Never);
        assert_eq!(collision_rule(3), CollisionRule::PushOwnTeam);
        assert_eq!(collision_rule(4), CollisionRule::Always);
    }

    #[test]
    fn styled_number_format_takes_the_style_color() {
        // {color: "red"}
        let red = [&[10, 8, 0, 5][..], b"color", &[0, 3], b"red", &[0]].concat();
        let color = |style: &[u8]| match number_format(NumberFormat::Styled(style.into())) {
            Some(ScoreNumberFormat::Styled(color)) => color,
            _ => panic!("not a styled format"),
        };
        assert_eq!(color(&red), crate::ui::common::rgb(0xFF5555));
        assert_eq!(color(&[10, 0]), [1.0; 4]);
    }

    #[test]
    fn other_display_slots_are_ignored() {
        let display = |slot| ScoreboardPacket::Display {
            slot,
            objective: "kills".into(),
        };
        assert!(event(display(0)).is_none());
        assert!(matches!(
            event(display(SIDEBAR)),
            Some(NetworkEvent::ScoreboardDisplay { name: Some(name) }) if name == "kills"
        ));
    }
}
