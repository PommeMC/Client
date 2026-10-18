//! Title, tab list and boss bar packets, as vanilla 26.2 lays them out
//! (`net/minecraft/network/protocol/game/Clientbound*Packet`). Text
//! components stay as NBT.

use crate::codec::{DecodeResult, Reader};

#[derive(Clone, Debug, PartialEq)]
pub enum HudPacket {
    Title(Box<[u8]>),
    Subtitle(Box<[u8]>),
    /// In ticks; vanilla keeps the current value for a negative one.
    TitlesAnimation {
        fade_in: i32,
        stay: i32,
        fade_out: i32,
    },
    ClearTitles {
        reset_times: bool,
    },
    TabList {
        header: Box<[u8]>,
        footer: Box<[u8]>,
    },
    BossEvent {
        id: u128,
        op: BossOp,
    },
}

/// `ClientboundBossEventPacket.OperationType`, in ordinal order.
#[derive(Clone, Debug, PartialEq)]
pub enum BossOp {
    Add {
        name: Box<[u8]>,
        progress: f32,
        /// `BossEvent.BossBarColor` and `BossBarOverlay` ordinals.
        color: u32,
        overlay: u32,
        flags: BossFlags,
    },
    Remove,
    Progress(f32),
    Name(Box<[u8]>),
    Style {
        color: u32,
        overlay: u32,
    },
    Properties(BossFlags),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BossFlags {
    pub darken_screen: bool,
    pub play_music: bool,
    pub create_world_fog: bool,
}

/// Decodes packet `name`, or `None` if it isn't a HUD packet.
pub fn read(name: &str, r: &mut Reader) -> Option<DecodeResult<HudPacket>> {
    let read: fn(&mut Reader) -> DecodeResult<HudPacket> = match name {
        // ClientboundSetTitleTextPacket.java:17
        "set_title_text" => |r| Ok(HudPacket::Title(r.nbt()?.into())),
        // ClientboundSetSubtitleTextPacket.java:17
        "set_subtitle_text" => |r| Ok(HudPacket::Subtitle(r.nbt()?.into())),
        // ClientboundSetTitlesAnimationPacket.java:26-30
        "set_titles_animation" => |r| {
            Ok(HudPacket::TitlesAnimation {
                fade_in: r.i32()?,
                stay: r.i32()?,
                fade_out: r.i32()?,
            })
        },
        // ClientboundClearTitlesPacket.java:22-24
        "clear_titles" => |r| {
            Ok(HudPacket::ClearTitles {
                reset_times: r.bool()?,
            })
        },
        // ClientboundTabListPacket.java:17
        "tab_list" => |r| {
            Ok(HudPacket::TabList {
                header: r.nbt()?.into(),
                footer: r.nbt()?.into(),
            })
        },
        // ClientboundBossEventPacket.java:38-42
        "boss_event" => |r| {
            Ok(HudPacket::BossEvent {
                id: r.uuid()?,
                op: boss_op(r)?,
            })
        },
        _ => return None,
    };
    Some(read(r))
}

/// `OperationType.reader` for each ordinal.
fn boss_op(r: &mut Reader) -> DecodeResult<BossOp> {
    Ok(match r.varint()? {
        0 => BossOp::Add {
            name: r.nbt()?.into(),
            progress: r.f32()?,
            color: r.varint()?,
            overlay: r.varint()?,
            flags: boss_flags(r)?,
        },
        1 => BossOp::Remove,
        2 => BossOp::Progress(r.f32()?),
        3 => BossOp::Name(r.nbt()?.into()),
        4 => BossOp::Style {
            color: r.varint()?,
            overlay: r.varint()?,
        },
        5 => BossOp::Properties(boss_flags(r)?),
        _ => return Err(r.error("unknown boss event operation")),
    })
}

/// The properties byte Add and UpdateProperties share.
fn boss_flags(r: &mut Reader) -> DecodeResult<BossFlags> {
    let flags = r.u8()?;
    Ok(BossFlags {
        darken_screen: flags & 1 != 0,
        play_music: flags & 2 != 0,
        create_world_fog: flags & 4 != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The NBT string tag `"hi"`.
    const TEXT: [u8; 5] = [8, 0, 2, b'h', b'i'];

    fn decode(name: &str, bytes: &[u8]) -> HudPacket {
        let mut r = Reader::new(bytes);
        let packet = read(name, &mut r).expect("a HUD packet").unwrap();
        r.ensure_eof().unwrap();
        packet
    }

    fn boss(op: &[u8]) -> DecodeResult<HudPacket> {
        let bytes = [&7u128.to_be_bytes()[..], op].concat();
        read("boss_event", &mut Reader::new(&bytes)).unwrap()
    }

    #[test]
    fn other_packets_are_not_read() {
        assert!(read("block_update", &mut Reader::new(&[])).is_none());
    }

    #[test]
    fn titles() {
        assert_eq!(
            decode("set_title_text", &TEXT),
            HudPacket::Title(TEXT.into())
        );
        assert_eq!(
            decode("set_subtitle_text", &TEXT),
            HudPacket::Subtitle(TEXT.into())
        );
        let times = [10i32, -1, 20].map(i32::to_be_bytes).concat();
        assert_eq!(
            decode("set_titles_animation", &times),
            HudPacket::TitlesAnimation {
                fade_in: 10,
                stay: -1,
                fade_out: 20,
            }
        );
        assert_eq!(
            decode("clear_titles", &[1]),
            HudPacket::ClearTitles { reset_times: true }
        );
    }

    #[test]
    fn tab_list_header_then_footer() {
        let footer = [8, 0, 1, b'f'];
        assert_eq!(
            decode("tab_list", &[&TEXT[..], &footer].concat()),
            HudPacket::TabList {
                header: TEXT.into(),
                footer: footer.into(),
            }
        );
    }

    #[test]
    fn boss_event_ops() {
        let op = |op| HudPacket::BossEvent { id: 7, op };
        let add = [&[0][..], &TEXT, &0.5f32.to_be_bytes(), &[2, 1, 0b101]].concat();
        assert_eq!(
            boss(&add),
            Ok(op(BossOp::Add {
                name: TEXT.into(),
                progress: 0.5,
                color: 2,
                overlay: 1,
                flags: BossFlags {
                    darken_screen: true,
                    play_music: false,
                    create_world_fog: true,
                },
            }))
        );
        assert_eq!(boss(&[1]), Ok(op(BossOp::Remove)));
        let progress = [&[2][..], &0.25f32.to_be_bytes()].concat();
        assert_eq!(boss(&progress), Ok(op(BossOp::Progress(0.25))));
        assert_eq!(
            boss(&[&[3][..], &TEXT].concat()),
            Ok(op(BossOp::Name(TEXT.into())))
        );
        assert_eq!(
            boss(&[4, 6, 4]),
            Ok(op(BossOp::Style {
                color: 6,
                overlay: 4
            }))
        );
        assert_eq!(
            boss(&[5, 0b010]),
            Ok(op(BossOp::Properties(BossFlags {
                play_music: true,
                ..Default::default()
            })))
        );
        assert!(boss(&[6]).is_err());
    }
}
