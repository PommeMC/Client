//! Vanilla ambient occlusion for cubic block faces, ported from
//! `BlockModelLighter.prepareQuadAmbientOcclusion` (snapshot 26.2).
//!
//! Each vertex brightness is `(side1 + side2 + corner + center) * 0.25`, where
//! every term is `0.2` (full-collision block) or `1.0`. `corner` is the
//! in-plane diagonal, except when both sides occlude: vanilla then substitutes
//! `shade0`, the brightness of `AdjacencyInfo.corners[0]` (the face's first
//! edge neighbour), which gives concave corners their asymmetric brightness.
//! With an exposed face (`center = 1.0`) the value is one of four brightnesses.

/// Vertex brightness by AO level; index 0 (three occluders) is darkest.
pub const AO_BRIGHTNESS: [f32; 4] = [0.4, 0.6, 0.8, 1.0];

#[inline]
fn occludes(shade: f32) -> bool {
    shade < 0.5
}

/// Vanilla per-vertex AO level (`0`..=`3`, `0` darkest) for an exposed face.
///
/// `shade0_occ` is whether the face's `corners[0]` neighbour occludes, used as
/// the diagonal fallback when both sides occlude.
#[inline]
pub fn vertex_ao_level(side1_occ: bool, side2_occ: bool, diag_occ: bool, shade0_occ: bool) -> u8 {
    let corner_occ = if side1_occ && side2_occ {
        shade0_occ
    } else {
        diag_occ
    };
    3 - (side1_occ as u8 + side2_occ as u8 + corner_occ as u8)
}

/// [`vertex_ao_level`] as a brightness, for callers working in shade floats.
#[inline]
pub fn vertex_brightness(side1: f32, side2: f32, diag: f32, shade0: f32) -> f32 {
    let level = vertex_ao_level(
        occludes(side1),
        occludes(side2),
        occludes(diag),
        occludes(shade0),
    );
    AO_BRIGHTNESS[level as usize]
}
