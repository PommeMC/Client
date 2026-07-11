use std::sync::OnceLock;

/// Maps all supported versions to their protocol version.
/// Snapshots encode as `(1 << 30) | base_protocol`.
/// KEEP IN SYNC WITH pomme-launcher/src-tauri/src/lib.rs
pub const VERSION_PROTOCOL_MAP: [(&str, i32); 4] = [
    ("26.2", 776),
    ("26.1", 775),
    ("26.1.1-rc-1", 0x40000130),
    ("26.1.1", 775),
];

static SELECTED_PROTOCOL: OnceLock<i32> = OnceLock::new();

pub fn protocol_for(version: &str) -> Option<i32> {
    VERSION_PROTOCOL_MAP
        .iter()
        .find(|(v, _)| *v == version)
        .map(|&(_, p)| p)
}

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
