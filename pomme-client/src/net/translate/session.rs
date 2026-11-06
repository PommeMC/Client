use super::*;

/// Mutable translation values scoped to one connection and its reconfiguration.
#[derive(Default)]
pub(super) struct SessionState {
    /// Legacy dimension keys in wire registry-id order.
    ///
    /// TODO: a reconfiguration whose registry data omits dimension_type keeps
    /// the previous list (as `config_sequence` keeps the previous
    /// `RegistryHolder`).
    pub(super) dimension_types: Vec<String>,
    /// A 26.3 accept held until the following position-and-rotation packet.
    pub(super) pending_accept: Option<u32>,
}

/// Connection-owned state paired with shared immutable protocol tables.
pub struct TranslationSession {
    tables: &'static Translation,
    // Inbound and outbound paths borrow the session across async operations.
    // Guards protect only synchronous rewrites and never cross an await.
    state: Mutex<SessionState>,
}

impl std::ops::Deref for TranslationSession {
    type Target = Translation;

    /// Exposes stateless operations and immutable registry remaps.
    fn deref(&self) -> &Self::Target {
        self.tables
    }
}

impl TranslationSession {
    /// Starts a connection with empty state and cached protocol tables.
    pub(crate) fn for_protocol(protocol: i32) -> Option<Self> {
        Some(Self {
            tables: cached(protocol)?,
            state: Mutex::default(),
        })
    }

    /// Splits a legacy login's registries into native configuration packets.
    pub fn split_login_registries(&self, raw: &[u8]) -> Option<Vec<Box<[u8]>>> {
        self.tables
            .split_login_registries_with_state(raw, &mut self.state.lock().unwrap())
    }

    /// Translates configuration frames and captures this connection's
    /// dimensions.
    pub fn translate_config_frame(&self, raw: Box<[u8]>) -> Vec<Box<[u8]>> {
        self.tables
            .translate_config_frame_with_state(raw, &mut self.state.lock().unwrap())
    }

    /// Translates a game frame using this connection's mutable state.
    pub fn translate_game_frame(&self, raw: Box<[u8]>) -> Option<Box<[u8]>> {
        self.tables
            .translate_game_frame_with_state(raw, &mut self.state.lock().unwrap())
    }

    /// Translates an outbound frame and retains any deferred teleport
    /// acceptance.
    pub fn translate_outbound_game_frame(&self, frame: Vec<u8>) -> Vec<Vec<u8>> {
        self.tables
            .translate_outbound_game_frame_with_state(frame, &mut self.state.lock().unwrap())
    }
}
