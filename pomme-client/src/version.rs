use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, Ordering};

static SELECTED_PROTOCOL: OnceLock<i32> = OnceLock::new();
/// 0 until a connection negotiates a wire version.
static SESSION_PROTOCOL: AtomicI32 = AtomicI32::new(0);

/// Record the launched version's protocol id; called once from `main`.
pub fn set_selected_protocol(protocol: i32) {
    let _ = SELECTED_PROTOCOL.set(protocol);
}

/// The protocol id of the version the client was launched as.
pub fn selected_protocol() -> i32 {
    *SELECTED_PROTOCOL
        .get()
        .unwrap_or(&azalea_protocol::packets::PROTOCOL_VERSION)
}

/// Record the wire version negotiated for the current connection (see
/// `net::connection::negotiate_wire_version`); set on every join, before any
/// world state loads.
pub fn set_session_protocol(protocol: i32) {
    SESSION_PROTOCOL.store(protocol, Ordering::Release);
}

/// Forget the negotiated wire version; called when a connection ends so no
/// stale protocol leaks into the next session.
pub fn clear_session_protocol() {
    SESSION_PROTOCOL.store(0, Ordering::Release);
}

/// The protocol id spoken on the wire right now: the negotiated one, or the
/// launched version's outside a connection.
pub fn session_protocol() -> i32 {
    match SESSION_PROTOCOL.load(Ordering::Acquire) {
        0 => selected_protocol(),
        p => p,
    }
}
