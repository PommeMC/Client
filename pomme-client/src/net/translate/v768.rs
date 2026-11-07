use super::*;

impl Ids768 {
    pub(super) fn rewrite(&self, id: u32) -> Option<FrameRewrite> {
        (id == self.level_particles_id).then_some(translate_level_particles_768 as FrameRewrite)
    }
}

/// Rewrites `level_particles`: 1.21.4 inserted the `alwaysShow` bool after
/// `overrideLimiter`; false matches the older client's behavior.
pub(super) fn translate_level_particles_768(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let first = *payload.first()?;
    let mut out = Vec::with_capacity(payload.len() + 2);
    wire::write_varint(&mut out, id);
    out.push(first);
    out.push(0);
    out.extend_from_slice(&payload[1..]);
    Some(out)
}
