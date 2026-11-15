//! The registries a server sends on join.

mod generated;

pub use generated::*;

use crate::DataRegistry;

// A chunk palette's entries have a default and convert to and from their ids.
impl Default for Biome {
    fn default() -> Self {
        Self::new_raw(0)
    }
}

impl From<u32> for Biome {
    fn from(id: u32) -> Self {
        Self::new_raw(id)
    }
}

impl From<Biome> for u32 {
    fn from(biome: Biome) -> Self {
        biome.protocol_id()
    }
}

impl<O> ChatKindKey<O> {
    /// The translation a vanilla chat type decorates its message with
    /// (`ChatType.bootstrap`); empty for one a server added.
    #[must_use]
    pub fn chat_translation_key(self) -> &'static str {
        match self {
            Self::Chat => "chat.type.text",
            Self::SayCommand => "chat.type.announcement",
            Self::MsgCommandIncoming => "commands.message.display.incoming",
            Self::MsgCommandOutgoing => "commands.message.display.outgoing",
            Self::TeamMsgCommandIncoming => "chat.type.team.text",
            Self::TeamMsgCommandOutgoing => "chat.type.team.sent",
            Self::EmoteCommand => "chat.type.emote",
            Self::Other(_) => "",
        }
    }

    /// The translation the narrator reads a vanilla chat type with.
    #[must_use]
    pub fn narrator_translation_key(self) -> &'static str {
        match self {
            Self::EmoteCommand => "chat.type.emote",
            _ => "chat.type.text.narrate",
        }
    }
}
