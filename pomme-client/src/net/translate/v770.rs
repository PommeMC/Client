use super::*;

/// Rewrites a native `player_command` payload (`entityId, action, data`
/// varints) for pre-1.21.6 wires, where PRESS/RELEASE_SHIFT_KEY still head
/// the action enum: every newer ordinal shifts up two.
pub(super) fn translate_player_command(old_id: u32, payload: &[u8]) -> Vec<Vec<u8>> {
    let parse = || {
        let mut pos = 0;
        let entity_id = wire::read_varint(payload, &mut pos)?;
        let action = wire::read_varint(payload, &mut pos)?;
        Some((entity_id, action, pos))
    };
    let Some((entity_id, action, data_at)) = parse() else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(payload.len() + 3);
    wire::write_varint(&mut out, old_id);
    wire::write_varint(&mut out, entity_id);
    wire::write_varint(&mut out, action + 2);
    out.extend_from_slice(&payload[data_at..]);
    vec![out]
}
