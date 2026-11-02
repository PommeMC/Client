use super::*;

impl Ids772 {
    pub(super) fn rewrite(&self, id: u32) -> Option<FrameRewrite> {
        Some(match id {
            i if i == self.add_entity_id => translate_add_entity,
            i if i == self.set_entity_motion_id => translate_set_entity_motion,
            i if i == self.player_rotation_id => translate_player_rotation,
            i if i == self.set_default_spawn_id => translate_set_default_spawn,
            _ => return None,
        })
    }
}

/// An `add_entity`/`set_entity_motion` velocity: three shorts in 1/8000ths
/// of a block per tick.
pub(super) fn read_velocity(cur: &mut Cursor<&[u8]>) -> Option<DVec3> {
    let mut v = [0.0; 3];
    for c in &mut v {
        *c = f64::from(read_u16(cur)? as i16) / 8000.0;
    }
    Some(DVec3::from_array(v))
}

/// Rewrites `add_entity`: 1.21.9 moved the velocity from three trailing
/// shorts to an `LpVec3` between the position and the rotation bytes
/// (`ClientboundAddEntityPacket` read/write in both references). The entity
/// type id stays in the wire version's space; `remap_inbound` remaps it on
/// the decoded packet.
pub(super) fn translate_add_entity(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    varint_span(&mut cur)?; // entity id
    advance(&mut cur, 16)?; // uuid
    varint_span(&mut cur)?; // entity type
    advance(&mut cur, 24)?; // position doubles
    let rot_at = cur.position() as usize;
    advance(&mut cur, 3)?; // x/y/head rotation bytes
    varint_span(&mut cur)?; // data
    let rot_end = cur.position() as usize;
    let velocity = read_velocity(&mut cur)?;

    let mut out = Vec::with_capacity(payload.len() + 4);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..rot_at]);
    wire::write_lp_vec3(&mut out, velocity);
    out.extend_from_slice(&payload[rot_at..rot_end]);
    Some(out)
}

/// Rewrites `set_entity_motion` from `entityId` + three velocity shorts to
/// `entityId` + `LpVec3`.
pub(super) fn translate_set_entity_motion(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let entity = varint_span(&mut cur)?;
    let velocity = read_velocity(&mut cur)?;

    let mut out = Vec::with_capacity(payload.len() + 4);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[entity]);
    wire::write_lp_vec3(&mut out, velocity);
    Some(out)
}

/// Rewrites `player_rotation`: 1.21.9 added a relative-rotation bool after
/// each angle, absolute here matching the old packet's semantics.
pub(super) fn translate_player_rotation(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let y_rot = payload.get(..4)?;
    let x_rot = payload.get(4..8)?;
    let mut out = Vec::with_capacity(12);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(y_rot);
    out.push(0);
    out.extend_from_slice(x_rot);
    out.push(0);
    Some(out)
}

/// Rewrites `set_default_spawn_position` from `BlockPos + angle` to 26.2's
/// `RespawnData` (`GlobalPos`, yaw, pitch). The old packet carries no
/// dimension or pitch: the overworld and zero are synthesized, and the
/// angle becomes the yaw.
/// TODO: carry the login/respawn dimension instead of synthesizing the
/// overworld, so compasses point right in other dimensions.
pub(super) fn translate_set_default_spawn(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let pos = payload.get(..8)?;
    let angle = payload.get(8..12)?;
    const DIMENSION: &str = "minecraft:overworld";
    let mut out = Vec::with_capacity(40);
    wire::write_varint(&mut out, id);
    wire::write_varint(&mut out, DIMENSION.len() as u32);
    out.extend_from_slice(DIMENSION.as_bytes());
    out.extend_from_slice(pos);
    out.extend_from_slice(angle); // yaw
    out.extend_from_slice(&0f32.to_be_bytes()); // pitch
    Some(out)
}

/// Rewrites `explode`: 1.21.9 inserted `radius` and `blockCount` after the
/// center and appended a weighted block-particle list (zero/empty here),
/// and the particle and sound registry ids between the knockback and the
/// end are remapped into 26.2's space (vanilla sends
/// `explosion`/`explosion_emitter` and `entity.generic.explode`, all of
/// which shift).
pub(super) fn translate_explode(
    id: u32,
    payload: &[u8],
    ids: &GameIds,
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    advance(&mut cur, 24)?; // center doubles
    skip_optional(&mut cur, |c| advance(c, 24))?; // player knockback
    let knockback_end = cur.position() as usize;

    let mut out = Vec::with_capacity(payload.len() + 10);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..24]);
    out.extend_from_slice(&0f32.to_be_bytes()); // radius
    out.extend_from_slice(&0i32.to_be_bytes()); // block count
    out.extend_from_slice(&payload[24..knockback_end]);
    translate_particles(&mut cur, &mut out, ids, remaps, false)?;
    translate_sound_holder(&mut cur, &mut out, remaps)?;
    out.push(0); // no block particles
    Some(out)
}
