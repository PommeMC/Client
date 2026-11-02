use super::*;

impl Ids767 {
    pub(super) fn rewrite(&self, id: u32) -> Option<FrameRewrite> {
        Some(match id {
            i if i == self.player_position_id => translate_player_position_767,
            i if i == self.respawn_id => translate_respawn_767,
            _ => return None,
        })
    }
}

/// Rewrites `player_position` from the 1.21.1 layout (`x/y/z, yRot, xRot,
/// u8 relative bits, teleport id`) to 26.2's (`id first, then
/// PositionMoveRotation with a zero delta, i32 relative bits`); the five
/// old bits keep their positions, and each position bit is mirrored into its
/// `DELTA_*` bit so 1.21.1's "keep the current velocity on a relative axis"
/// survives 26.2's `calculateDelta`.
pub(super) fn translate_player_position_767(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let pos = payload.get(..24)?;
    let y_rot = f32::from_be_bytes(payload.get(24..28)?.try_into().ok()?);
    let x_rot = f32::from_be_bytes(payload.get(28..32)?.try_into().ok()?);
    let relatives = i32::from(*payload.get(32)?);
    let mut p = 33;
    let teleport_id = wire::read_varint(payload, &mut p)?;
    // X/Y/Z (0-2) -> DELTA_X/Y/Z (5-7); ROTATE_DELTA (8) stays clear.
    let relatives = relatives | ((relatives & 0b111) << 5);

    let mut out = Vec::with_capacity(64);
    wire::write_varint(&mut out, id);
    wire::write_varint(&mut out, teleport_id);
    write_position_move_rotation(&mut out, pos, y_rot, x_rot);
    out.extend_from_slice(&relatives.to_be_bytes());
    Some(out)
}

/// Rewrites 1.21.1's `teleport_entity` (`id, x/y/z, packed-degree rotation
/// bytes, onGround`) into 26.2's `entity_position_sync` — the packet whose
/// handler, like 1.21.1's, syncs position and rotation without touching the
/// entity's velocity. 26.2's own `teleport_entity` resolves the delta through
/// `calculateAbsolute`, so the synthesized zero would zero the velocity on
/// every sync. Same layout minus the relative-bit set.
pub(super) fn translate_teleport_entity_767(v: &Ids767, payload: &[u8]) -> Option<Vec<u8>> {
    let mut p = 0;
    let entity = wire::read_varint(payload, &mut p)?;
    let pos = payload.get(p..p + 24)?;
    let y_rot = *payload.get(p + 24)? as i8;
    let x_rot = *payload.get(p + 25)? as i8;
    let on_ground = *payload.get(p + 26)?;

    let mut out = Vec::with_capacity(72);
    wire::write_varint(&mut out, v.entity_position_sync_id);
    wire::write_varint(&mut out, entity);
    write_position_move_rotation(
        &mut out,
        pos,
        pomme_protocol::entity::unpack_degrees(y_rot),
        pomme_protocol::entity::unpack_degrees(x_rot),
    );
    out.push(on_ground);
    Some(out)
}

pub(super) fn translate_respawn_767(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(payload.len() + 2);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&insert_sea_level(payload)?);
    Some(out)
}

/// Rewrites 1.21.1's two-field `set_time` (`gameTime, dayTime` with a
/// negated dayTime marking a frozen clock, -1 for frozen zero) into the
/// triple the shared rewrite consumes.
pub(super) fn translate_set_time_767(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let game_time = payload.get(..8)?;
    let day_time = i64::from_be_bytes(payload.get(8..16)?.try_into().ok()?);
    let (day_time, tick) = if day_time < 0 {
        (if day_time == -1 { 0 } else { -day_time }, 0)
    } else {
        (day_time, 1)
    };
    let mut tmp = Vec::with_capacity(17);
    tmp.extend_from_slice(game_time);
    tmp.extend_from_slice(&day_time.to_be_bytes());
    tmp.push(tick);
    translate_set_time(id, &tmp)
}

/// Rewrites `container_set_slot`: 1.21.2 widened the container id from a
/// signed byte to a varint and split its -1 (cursor) and -2 (player
/// inventory) sentinels into `set_cursor_item`/`set_player_inventory`,
/// which drop the state id.
pub(super) fn translate_container_set_slot_767(v: &Ids767, payload: &[u8]) -> Option<Vec<u8>> {
    let container = *payload.first()? as i8;
    let mut p = 1;
    wire::read_varint(payload, &mut p)?; // state id
    let slot = i16::from_be_bytes(payload.get(p..p + 2)?.try_into().ok()?);
    let stack = payload.get(p + 2..)?;

    let mut out = Vec::with_capacity(payload.len() + 4);
    match container {
        -1 => {
            wire::write_varint(&mut out, v.set_cursor_item_id);
            out.extend_from_slice(stack);
        }
        -2 => {
            wire::write_varint(&mut out, v.set_player_inventory_id);
            wire::write_varint(&mut out, slot as u32);
            out.extend_from_slice(stack);
        }
        _ => {
            wire::write_varint(&mut out, v.container_set_slot_id);
            wire::write_varint(&mut out, container as u32);
            out.extend_from_slice(&payload[1..]);
        }
    }
    Some(out)
}

/// Rewrites `cooldown`'s item id into the native registry space. Vanilla
/// 26.2 names a cooldown group instead, but azalea still decodes the item
/// registry id and these frames feed azalea (like the team-color ordinal).
/// TODO: write the cooldown group once pomme owns the decoder (see the
/// azalea-divergence list).
pub(super) fn translate_cooldown_767(
    remaps: &RegistryRemaps,
    id: u32,
    payload: &[u8],
) -> Option<Vec<u8>> {
    let mut p = 0;
    let item = wire::read_varint(payload, &mut p)?;
    let mut out = Vec::with_capacity(payload.len() + 2);
    wire::write_varint(&mut out, id);
    wire::write_varint(&mut out, remaps.remap(ClientRegistry::Item, item)?);
    out.extend_from_slice(&payload[p..]);
    Some(out)
}

/// Rewrites serverbound `player_input` for 1.21.1, where it was the vehicle
/// steering packet (`xxa, zza floats + jump/shift flags`) rather than the
/// key bitfield 1.21.2 introduced; axes are synthesized at full strength.
/// Unmounted frames need no suppression: the server's `setPlayerInput` only
/// applies them to a passenger.
pub(super) fn translate_player_input(old_id: u32, payload: &[u8]) -> Vec<Vec<u8>> {
    let Some(&bits) = payload.first() else {
        return Vec::new();
    };
    let axis = |pos, neg| match (bits & pos != 0, bits & neg != 0) {
        (true, false) => 1.0f32,
        (false, true) => -1.0f32,
        _ => 0.0f32,
    };
    let mut out = Vec::with_capacity(12);
    wire::write_varint(&mut out, old_id);
    out.extend_from_slice(&axis(4, 8).to_be_bytes()); // left / right -> xxa
    out.extend_from_slice(&axis(1, 2).to_be_bytes()); // forward / backward -> zza
    out.push((bits >> 4) & 3); // jump, shift
    vec![out]
}
