use super::*;

impl Ids766 {
    pub(super) fn rewrite(&self, id: u32) -> Option<FrameRewrite> {
        (id == self.projectile_power_id).then_some(translate_projectile_power_766 as FrameRewrite)
    }
}

/// Rewrites serverbound `use_item` for 1.20.6, dropping the rotation
/// floats 1.21 appended.
pub(super) fn translate_use_item(old_id: u32, payload: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::with_capacity(payload.len());
    wire::write_varint(&mut out, old_id);
    out.extend_from_slice(&payload[..payload.len().saturating_sub(8)]);
    vec![out]
}

/// Rewrites `projectile_power`: 1.21 collapsed the per-axis acceleration
/// vector into its magnitude.
pub(super) fn translate_projectile_power_766(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let entity = varint_span(&mut cur)?;
    let magnitude = DVec3::from_array(read_f64s(&mut cur)?).length();

    let mut out = Vec::with_capacity(16);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[entity]);
    out.extend_from_slice(&magnitude.to_be_bytes());
    Some(out)
}
