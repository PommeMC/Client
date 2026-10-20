//! Scoreboard objective, score, display slot and team packets, as vanilla
//! 26.2 lays them out
//! (`net/minecraft/network/protocol/game/Clientbound*Packet`). Text components
//! and styles stay as NBT.

use crate::ClientRegistry;
use crate::codec::{DecodeResult, MAX_STRING_LENGTH, Reader};
use crate::registries::RegistryTable;

#[derive(Clone, Debug, PartialEq)]
pub enum ScoreboardPacket {
    Objective {
        name: String,
        method: ObjectiveMethod,
    },
    Display {
        /// A `DisplaySlot` id.
        slot: u32,
        /// Empty clears the slot.
        objective: String,
    },
    Score {
        owner: String,
        objective: String,
        score: i32,
        display: Option<Box<[u8]>>,
        number_format: Option<NumberFormat>,
    },
    ResetScore {
        owner: String,
        /// `None` resets every objective.
        objective: Option<String>,
    },
    Team {
        name: String,
        method: TeamMethod,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum ObjectiveMethod {
    Add(ObjectiveParameters),
    Remove,
    Change(ObjectiveParameters),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObjectiveParameters {
    pub display_name: Box<[u8]>,
    /// `ObjectiveCriteria.RenderType` ordinal.
    pub render_type: u32,
    pub number_format: Option<NumberFormat>,
}

/// `NumberFormatTypes`' formats.
#[derive(Clone, Debug, PartialEq)]
pub enum NumberFormat {
    Blank,
    /// A text style as NBT.
    Styled(Box<[u8]>),
    Fixed(Box<[u8]>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum TeamMethod {
    Add {
        parameters: TeamParameters,
        players: Vec<String>,
    },
    Remove,
    Change(TeamParameters),
    Join(Vec<String>),
    Leave(Vec<String>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct TeamParameters {
    pub display_name: Box<[u8]>,
    pub player_prefix: Box<[u8]>,
    pub player_suffix: Box<[u8]>,
    /// `Team.Visibility` and `Team.CollisionRule` ids.
    pub name_tag_visibility: u32,
    pub collision_rule: u32,
    /// A `TeamColor` id.
    pub color: Option<u32>,
    pub options: u8,
}

/// Decodes packet `name`, or `None` if it isn't a scoreboard packet.
pub fn read(name: &str, r: &mut Reader) -> Option<DecodeResult<ScoreboardPacket>> {
    let read: fn(&mut Reader) -> DecodeResult<ScoreboardPacket> = match name {
        // ClientboundSetObjectivePacket.java:41-50
        "set_objective" => |r| {
            let name = string(r)?;
            let method = match r.u8()? {
                0 => ObjectiveMethod::Add(objective_parameters(r)?),
                1 => ObjectiveMethod::Remove,
                2 => ObjectiveMethod::Change(objective_parameters(r)?),
                _ => return Err(r.error("unknown set_objective method")),
            };
            Ok(ScoreboardPacket::Objective { name, method })
        },
        // ClientboundSetDisplayObjectivePacket.java:31-34
        "set_display_objective" => |r| {
            Ok(ScoreboardPacket::Display {
                slot: r.varint()?,
                objective: string(r)?,
            })
        },
        // ClientboundSetScorePacket.java:21
        "set_score" => |r| {
            Ok(ScoreboardPacket::Score {
                owner: string(r)?,
                objective: string(r)?,
                score: r.var_i32()?,
                display: r.optional(component)?,
                number_format: r.optional(number_format)?,
            })
        },
        // ClientboundResetScorePacket.java:21-23
        "reset_score" => |r| {
            Ok(ScoreboardPacket::ResetScore {
                owner: string(r)?,
                objective: r.optional(string)?,
            })
        },
        // ClientboundSetPlayerTeamPacket.java:60-65
        "set_player_team" => |r| {
            let name = string(r)?;
            let players = |r: &mut Reader| r.list(usize::MAX, string);
            let method = match r.u8()? {
                0 => TeamMethod::Add {
                    parameters: team_parameters(r)?,
                    players: players(r)?,
                },
                1 => TeamMethod::Remove,
                2 => TeamMethod::Change(team_parameters(r)?),
                3 => TeamMethod::Join(players(r)?),
                4 => TeamMethod::Leave(players(r)?),
                _ => return Err(r.error("unknown set_player_team method")),
            };
            Ok(ScoreboardPacket::Team { name, method })
        },
        _ => return None,
    };
    Some(read(r))
}

fn string(r: &mut Reader) -> DecodeResult<String> {
    Ok(r.utf(MAX_STRING_LENGTH)?.to_owned())
}

/// `ComponentSerialization.TRUSTED_STREAM_CODEC`.
fn component(r: &mut Reader) -> DecodeResult<Box<[u8]>> {
    Ok(r.nbt()?.into())
}

fn objective_parameters(r: &mut Reader) -> DecodeResult<ObjectiveParameters> {
    Ok(ObjectiveParameters {
        display_name: component(r)?,
        render_type: r.varint()?,
        number_format: r.optional(number_format)?,
    })
}

/// `NumberFormatTypes.STREAM_CODEC`: a `number_format_type` registry id,
/// then that type's codec.
fn number_format(r: &mut Reader) -> DecodeResult<NumberFormat> {
    let id = r.varint()?;
    match RegistryTable::native().name_of(ClientRegistry::NumberFormatType, id) {
        Some("blank") => Ok(NumberFormat::Blank),
        // StyledFormat.java:23
        Some("styled") => Ok(NumberFormat::Styled(r.nbt()?.into())),
        // FixedFormat.java:22
        Some("fixed") => Ok(NumberFormat::Fixed(component(r)?)),
        _ => Err(r.error("unknown number format type")),
    }
}

/// `ClientboundSetPlayerTeamPacket.Parameters.STREAM_CODEC` (:125).
fn team_parameters(r: &mut Reader) -> DecodeResult<TeamParameters> {
    Ok(TeamParameters {
        display_name: component(r)?,
        player_prefix: component(r)?,
        player_suffix: component(r)?,
        name_tag_visibility: r.varint()?,
        collision_rule: r.varint()?,
        color: r.optional(Reader::varint)?,
        options: r.u8()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{write_utf, write_varint};

    /// The NBT string tag `"hi"`.
    const TEXT: [u8; 5] = [8, 0, 2, b'h', b'i'];
    /// An empty compound: a style with nothing set.
    const STYLE: [u8; 2] = [10, 0];

    fn decode(name: &str, bytes: &[u8]) -> ScoreboardPacket {
        let mut r = Reader::new(bytes);
        let packet = read(name, &mut r).expect("a scoreboard packet").unwrap();
        r.ensure_eof().unwrap();
        packet
    }

    fn rejects(name: &str, bytes: &[u8]) {
        assert!(read(name, &mut Reader::new(bytes)).unwrap().is_err());
    }

    fn format_id(name: &str) -> u32 {
        RegistryTable::native()
            .id_of(ClientRegistry::NumberFormatType, name)
            .unwrap()
    }

    fn utf(s: &str) -> Vec<u8> {
        let mut out = Vec::new();
        write_utf(&mut out, s);
        out
    }

    #[test]
    fn other_packets_are_not_read() {
        assert!(read("block_update", &mut Reader::new(&[])).is_none());
    }

    #[test]
    fn objective_methods_and_number_formats() {
        let add = |format: &[u8]| [&utf("kills")[..], &[0], &TEXT, &[1], format].concat();
        let objective = |number_format| ScoreboardPacket::Objective {
            name: "kills".into(),
            method: ObjectiveMethod::Add(ObjectiveParameters {
                display_name: TEXT.into(),
                render_type: 1,
                number_format,
            }),
        };
        assert_eq!(decode("set_objective", &add(&[0])), objective(None));
        let blank = [1, format_id("blank") as u8];
        assert_eq!(
            decode("set_objective", &add(&blank)),
            objective(Some(NumberFormat::Blank))
        );
        let styled = [&[1, format_id("styled") as u8][..], &STYLE].concat();
        assert_eq!(
            decode("set_objective", &add(&styled)),
            objective(Some(NumberFormat::Styled(STYLE.into())))
        );
        let fixed = [&[1, format_id("fixed") as u8][..], &TEXT].concat();
        assert_eq!(
            decode("set_objective", &add(&fixed)),
            objective(Some(NumberFormat::Fixed(TEXT.into())))
        );
        rejects("set_objective", &add(&[1, 3]));

        assert_eq!(
            decode("set_objective", &[&utf("kills")[..], &[1]].concat()),
            ScoreboardPacket::Objective {
                name: "kills".into(),
                method: ObjectiveMethod::Remove,
            }
        );
        let change = [&utf("kills")[..], &[2], &TEXT, &[0, 0]].concat();
        assert_eq!(
            decode("set_objective", &change),
            ScoreboardPacket::Objective {
                name: "kills".into(),
                method: ObjectiveMethod::Change(ObjectiveParameters {
                    display_name: TEXT.into(),
                    render_type: 0,
                    number_format: None,
                }),
            }
        );
        rejects("set_objective", &[&utf("kills")[..], &[3]].concat());
    }

    #[test]
    fn display_slot() {
        assert_eq!(
            decode("set_display_objective", &[&[1][..], &utf("kills")].concat()),
            ScoreboardPacket::Display {
                slot: 1,
                objective: "kills".into(),
            }
        );
    }

    #[test]
    fn scores_and_resets() {
        let mut bare = [utf("Bob"), utf("kills")].concat();
        write_varint(&mut bare, -3i32 as u32);
        bare.extend([0, 0]);
        assert_eq!(
            decode("set_score", &bare),
            ScoreboardPacket::Score {
                owner: "Bob".into(),
                objective: "kills".into(),
                score: -3,
                display: None,
                number_format: None,
            }
        );
        let full = [
            &utf("Bob")[..],
            &utf("kills"),
            &[9, 1],
            &TEXT,
            &[1, format_id("fixed") as u8],
            &TEXT,
        ]
        .concat();
        assert_eq!(
            decode("set_score", &full),
            ScoreboardPacket::Score {
                owner: "Bob".into(),
                objective: "kills".into(),
                score: 9,
                display: Some(TEXT.into()),
                number_format: Some(NumberFormat::Fixed(TEXT.into())),
            }
        );

        assert_eq!(
            decode(
                "reset_score",
                &[&utf("Bob")[..], &[1], &utf("kills")].concat()
            ),
            ScoreboardPacket::ResetScore {
                owner: "Bob".into(),
                objective: Some("kills".into()),
            }
        );
        assert_eq!(
            decode("reset_score", &[&utf("Bob")[..], &[0]].concat()),
            ScoreboardPacket::ResetScore {
                owner: "Bob".into(),
                objective: None,
            }
        );
    }

    #[test]
    fn team_methods() {
        let name = utf("reds");
        let players = [&[1][..], &utf("Bob")].concat();
        let parameters = |color: &[u8]| [&TEXT[..], &TEXT, &TEXT, &[0, 1], color, &[3]].concat();
        let decoded = |color| TeamParameters {
            display_name: TEXT.into(),
            player_prefix: TEXT.into(),
            player_suffix: TEXT.into(),
            name_tag_visibility: 0,
            collision_rule: 1,
            color,
            options: 3,
        };
        let team = |method| ScoreboardPacket::Team {
            name: "reds".into(),
            method,
        };

        let add = [&name[..], &[0], &parameters(&[1, 12]), &players].concat();
        assert_eq!(
            decode("set_player_team", &add),
            team(TeamMethod::Add {
                parameters: decoded(Some(12)),
                players: vec!["Bob".into()],
            })
        );
        assert_eq!(
            decode("set_player_team", &[&name[..], &[1]].concat()),
            team(TeamMethod::Remove)
        );
        let change = [&name[..], &[2], &parameters(&[0])].concat();
        assert_eq!(
            decode("set_player_team", &change),
            team(TeamMethod::Change(decoded(None)))
        );
        assert_eq!(
            decode("set_player_team", &[&name[..], &[3], &players].concat()),
            team(TeamMethod::Join(vec!["Bob".into()]))
        );
        assert_eq!(
            decode("set_player_team", &[&name[..], &[4], &players].concat()),
            team(TeamMethod::Leave(vec!["Bob".into()]))
        );
        rejects("set_player_team", &[&name[..], &[5]].concat());
    }
}
