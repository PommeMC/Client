use azalea_protocol::packets::game::ServerboundGamePacket;
use tokio::sync::mpsc;

use crate::recipe::{RecipeBookType, RecipeDisplayId};

/// An outbound game packet: either an azalea-serialized packet or bytes
/// pre-encoded by `net::wire` (varint packet id + body), or chat work the
/// network loop signs and tracks. Chat shares the queue so it applies in
/// game-thread order, like vanilla's single-threaded `ClientPacketListener`.
pub enum Outbound {
    Packet(Box<ServerboundGamePacket>),
    Raw(Vec<u8>),
    /// Typed chat, or a command with its leading `/`.
    ChatInput(String),
    /// Vanilla `handleLogin`'s chat reset.
    ChatLogin {
        online_mode: bool,
    },
    ChatMark(Box<ChatMark>),
    /// A dialog or chat `custom` click, encoded for whichever phase the
    /// connection is in when it goes out.
    CustomClick {
        id: String,
        payload: Option<simdnbt::owned::NbtTag>,
    },
}

/// A `LastSeenMessagesTracker` update, recorded by the chat UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatMark {
    Processed { signature: [u8; 256], shown: bool },
    Deleted { signature: [u8; 256] },
}

pub struct PacketSender {
    tx: mpsc::UnboundedSender<Outbound>,
}

impl PacketSender {
    pub fn new(tx: mpsc::UnboundedSender<Outbound>) -> Self {
        Self { tx }
    }

    pub fn send(&self, packet: ServerboundGamePacket) {
        self.queue(Outbound::Packet(Box::new(packet)));
    }

    pub fn send_raw(&self, bytes: Vec<u8>) {
        self.queue(Outbound::Raw(bytes));
    }

    pub fn send_chat(&self, input: String) {
        self.queue(Outbound::ChatInput(input));
    }

    pub fn chat_login(&self, online_mode: bool) {
        self.queue(Outbound::ChatLogin { online_mode });
    }

    pub fn mark_chat(&self, mark: ChatMark) {
        self.queue(Outbound::ChatMark(Box::new(mark)));
    }

    pub fn send_custom_click(&self, id: String, payload: Option<simdnbt::owned::NbtTag>) {
        self.queue(Outbound::CustomClick { id, payload });
    }

    /// Queue a post-1.21.2 recipe placement using Pomme's native wire model.
    /// Older supported protocols use resource-location recipe ids instead of
    /// `RecipeDisplayId`; Pomme intentionally does not synthesize a mapping
    /// for recipe data it cannot receive yet.
    #[allow(dead_code)]
    pub fn place_recipe(
        &self,
        container_id: i32,
        recipe_display_id: RecipeDisplayId,
        use_max_items: bool,
    ) -> bool {
        if crate::version::session_protocol() < 768 {
            tracing::warn!(
                protocol = crate::version::session_protocol(),
                "recipe placement is unavailable on pre-1.21.2 recipe protocol"
            );
            return false;
        }
        self.send_raw(pomme_protocol::wire::encode_place_recipe(
            container_id,
            recipe_display_id,
            use_max_items,
        ));
        true
    }

    #[allow(dead_code)]
    pub fn recipe_book_change_settings(
        &self,
        kind: RecipeBookType,
        open: bool,
        filtering: bool,
    ) -> bool {
        let kind = match kind {
            RecipeBookType::Crafting => pomme_protocol::wire::RecipeBookType::Crafting,
            RecipeBookType::Furnace => pomme_protocol::wire::RecipeBookType::Furnace,
            RecipeBookType::BlastFurnace => pomme_protocol::wire::RecipeBookType::BlastFurnace,
            RecipeBookType::Smoker => pomme_protocol::wire::RecipeBookType::Smoker,
        };
        self.send_raw(pomme_protocol::wire::encode_recipe_book_change_settings(
            kind, open, filtering,
        ));
        true
    }

    #[allow(dead_code)]
    pub fn recipe_book_seen_recipe(&self, recipe_display_id: RecipeDisplayId) -> bool {
        if crate::version::session_protocol() < 768 {
            tracing::warn!(
                protocol = crate::version::session_protocol(),
                "recipe-book seen action is unavailable on pre-1.21.2 recipe protocol"
            );
            return false;
        }
        self.send_raw(pomme_protocol::wire::encode_recipe_book_seen_recipe(
            recipe_display_id,
        ));
        true
    }

    fn queue(&self, out: Outbound) {
        if let Err(e) = self.tx.send(out) {
            tracing::error!("Failed to queue outbound packet: {e}");
        }
    }
}
