use std::sync::OnceLock;

static SELECTED_PROTOCOL: OnceLock<i32> = OnceLock::new();

/// Record the launched version's protocol id; called once from `main`.
pub fn set_selected_protocol(protocol: i32) {
    let _ = SELECTED_PROTOCOL.set(protocol);
}

/// The protocol id of the version the client was launched as, used for the
/// handshake and server-list compatibility checks.
pub fn selected_protocol() -> i32 {
    *SELECTED_PROTOCOL
        .get()
        .unwrap_or(&azalea_protocol::packets::PROTOCOL_VERSION)
}
