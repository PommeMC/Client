// Vendored from Inspirateur/binary-greedy-meshing under the MIT License;
// for the full license, see THIRD_PARTY_LICENSES.md
// Modified to use AO as a merge criterion.

use std::collections::BTreeSet;

use super::block_ao;

const MASK_6: u64 = 0b111111;

#[derive(Copy, Clone)]
pub struct Quad {
    packed: u64,
    pub ao: u8,
    /// Per-corner smooth light (quantised brightness 0..=255), ordered to match
    /// `ao_levels`.
    pub light: [u8; 4],
}

impl Quad {
    #[allow(clippy::too_many_arguments)]
    fn pack(
        x: usize,
        y: usize,
        z: usize,
        w: usize,
        h: usize,
        v_type: usize,
        ao: u8,
        light: [u8; 4],
    ) -> Self {
        Self {
            packed: ((v_type << 32) | (h << 24) | (w << 18) | (z << 12) | (y << 6) | x) as u64,
            ao,
            light,
        }
    }

    pub fn xyz(&self) -> [u32; 3] {
        [
            (self.packed & MASK_6) as u32,
            ((self.packed >> 6) & MASK_6) as u32,
            ((self.packed >> 12) & MASK_6) as u32,
        ]
    }

    pub fn width(&self) -> u32 {
        ((self.packed >> 18) & MASK_6) as u32
    }

    pub fn height(&self) -> u32 {
        ((self.packed >> 24) & MASK_6) as u32
    }

    pub fn voxel_id(&self) -> u16 {
        (self.packed >> 32) as u16
    }

    pub fn ao_levels(&self) -> [u8; 4] {
        [
            (self.ao >> 6) & 3,
            (self.ao >> 4) & 3,
            (self.ao >> 2) & 3,
            self.ao & 3,
        ]
    }
}

pub struct GreedyMesher<const CS: usize> {
    pub quads: [Vec<Quad>; 6],
    face_masks: Box<[u64]>,
    ao_faces: Box<[u8]>,
    light_faces: Box<[[u8; 4]]>,
    forward_merged: Box<[u8]>,
    right_merged: Box<[u8]>,
}

impl<const CS: usize> GreedyMesher<CS> {
    pub const CS_P: usize = CS + 2;
    pub const CS_P2: usize = Self::CS_P * Self::CS_P;
    pub const CS_P3: usize = Self::CS_P * Self::CS_P * Self::CS_P;
    pub const CS_2: usize = CS * CS;
    pub fn new() -> Self {
        Self {
            face_masks: vec![0; Self::CS_2 * 6].into_boxed_slice(),
            ao_faces: vec![0; Self::CS_2 * 6 * CS].into_boxed_slice(),
            light_faces: vec![[0u8; 4]; Self::CS_2 * 6 * CS].into_boxed_slice(),
            forward_merged: vec![0; Self::CS_2].into_boxed_slice(),
            right_merged: vec![0; CS].into_boxed_slice(),
            quads: core::array::from_fn(|_| Vec::new()),
        }
    }

    pub fn mesh(
        &mut self,
        voxels: &[u16],
        occluders: &[bool],
        light: &[f32],
        transparents: &BTreeSet<u16>,
    ) {
        self.face_culling(voxels, transparents);
        self.compute_ao_and_light(occluders, light);
        self.face_merging(voxels);
    }

    fn face_culling(&mut self, voxels: &[u16], transparents: &BTreeSet<u16>) {
        for a in 1..(Self::CS_P - 1) {
            let a_cs_p = a * Self::CS_P;
            for b in 1..(Self::CS_P - 1) {
                let ab = (a_cs_p + b) * Self::CS_P;
                let ba_index = (b - 1) + (a - 1) * CS;
                let ab_index = (a - 1) + (b - 1) * CS;
                for c in 1..(Self::CS_P - 1) {
                    let abc = ab + c;
                    let v1 = voxels[abc];
                    if v1 == 0 {
                        continue;
                    }
                    self.face_masks[ba_index] |=
                        face_value(v1, voxels[abc + Self::CS_P2], transparents) << (c - 1);
                    self.face_masks[ba_index + Self::CS_2] |=
                        face_value(v1, voxels[abc - Self::CS_P2], transparents) << (c - 1);
                    self.face_masks[ab_index + 2 * Self::CS_2] |=
                        face_value(v1, voxels[abc + Self::CS_P], transparents) << (c - 1);
                    self.face_masks[ab_index + 3 * Self::CS_2] |=
                        face_value(v1, voxels[abc - Self::CS_P], transparents) << (c - 1);
                    self.face_masks[ba_index + 4 * Self::CS_2] |=
                        face_value(v1, voxels[abc + 1], transparents) << c;
                    self.face_masks[ba_index + 5 * Self::CS_2] |=
                        face_value(v1, voxels[abc - 1], transparents) << c;
                }
            }
        }
    }

    /// Linear index into `ao_faces` / `light_faces` for one face cell.
    #[inline]
    fn cell_index(face: usize, layer: usize, forward: usize, right: usize) -> usize {
        face * Self::CS_2 * CS + (layer * CS + forward) * CS + right
    }

    /// Index into the padded `CS_P^3` voxel/occluder/light arrays, or `None`
    /// when out of range.
    #[inline]
    fn padded_index(x: i32, y: i32, z: i32) -> Option<usize> {
        if x < 0 || y < 0 || z < 0 {
            return None;
        }
        let (x, y, z) = (x as usize, y as usize, z as usize);
        if x >= Self::CS_P || y >= Self::CS_P || z >= Self::CS_P {
            return None;
        }
        Some(pad_linearize::<CS>(x, y, z))
    }

    /// Per-corner AO and smooth light for every visible face cell, written to
    /// the same `idx` so the light merge gate stays locked to the AO gate.
    fn compute_ao_and_light(&mut self, occluders: &[bool], light: &[f32]) {
        let occ = |x: i32, y: i32, z: i32| -> bool {
            Self::padded_index(x, y, z).is_some_and(|i| occluders[i])
        };
        // Out of range falls back to full bright (never reached for real face cells).
        let lit = |x: i32, y: i32, z: i32| -> f32 {
            Self::padded_index(x, y, z).map_or(1.0, |i| light[i])
        };

        for face in 0..=3u8 {
            let axis = face / 2;
            for layer in 0..CS {
                for forward in 0..CS {
                    let mask_idx = forward + layer * CS + face as usize * Self::CS_2;
                    let bits = self.face_masks[mask_idx];
                    if bits == 0 {
                        continue;
                    }
                    let mut remaining = bits;
                    while remaining != 0 {
                        let bit_pos = remaining.trailing_zeros() as usize;
                        remaining &= !(1u64 << bit_pos);

                        let (x, y, z) =
                            axis_to_xyz(axis as usize, forward + 1, bit_pos + 1, layer + 1);
                        let (nx, ny, nz) = face_normal(face);
                        let fx = x as i32 + nx;
                        let fy = y as i32 + ny;
                        let fz = z as i32 + nz;

                        let idx = Self::cell_index(face as usize, layer, forward, bit_pos);
                        self.ao_faces[idx] = compute_vertex_ao_packed(face, fx, fy, fz, &occ);
                        self.light_faces[idx] = compute_vertex_light_packed(face, fx, fy, fz, &lit);
                    }
                }
            }
        }

        for face in 4..=5u8 {
            let axis = face / 2;
            for forward in 0..CS {
                for right in 0..CS {
                    let mask_idx = right + forward * CS + face as usize * Self::CS_2;
                    let bits = self.face_masks[mask_idx];
                    if bits == 0 {
                        continue;
                    }
                    let mut remaining = bits;
                    while remaining != 0 {
                        let bit_pos = remaining.trailing_zeros() as usize;
                        remaining &= !(1u64 << bit_pos);

                        let (x, y, z) = axis_to_xyz(axis as usize, right + 1, forward + 1, bit_pos);
                        let (nx, ny, nz) = face_normal(face);
                        let fx = x as i32 + nx;
                        let fy = y as i32 + ny;
                        let fz = z as i32 + nz;

                        let idx = Self::cell_index(face as usize, forward, right, bit_pos - 1);
                        self.ao_faces[idx] = compute_vertex_ao_packed(face, fx, fy, fz, &occ);
                        self.light_faces[idx] = compute_vertex_light_packed(face, fx, fy, fz, &lit);
                    }
                }
            }
        }
    }

    fn get_ao(&self, face: usize, layer: usize, forward: usize, right: usize) -> u8 {
        self.ao_faces[Self::cell_index(face, layer, forward, right)]
    }

    fn get_light4(&self, face: usize, layer: usize, forward: usize, right: usize) -> [u8; 4] {
        self.light_faces[Self::cell_index(face, layer, forward, right)]
    }

    fn face_merging(&mut self, voxels: &[u16]) {
        for face in 0..=3usize {
            let axis = face / 2;
            for layer in 0..CS {
                let bits_location = layer * CS + face * Self::CS_2;
                for forward in 0..CS {
                    let mut bits_here = self.face_masks[forward + bits_location];
                    if bits_here == 0 {
                        continue;
                    }
                    let bits_next = if forward + 1 < CS {
                        self.face_masks[(forward + 1) + bits_location]
                    } else {
                        0
                    };
                    let mut right_merged = 1usize;
                    while bits_here != 0 {
                        let bit_pos = bits_here.trailing_zeros() as usize;
                        let v_type =
                            voxels[get_axis_index::<CS>(axis, forward + 1, bit_pos + 1, layer + 1)];
                        let ao_here = self.get_ao(face, layer, forward, bit_pos);
                        let light_here = self.get_light4(face, layer, forward, bit_pos);

                        if (bits_next >> bit_pos & 1) != 0
                            && ao_uniform(ao_here)
                            && light_uniform(light_here)
                            && v_type
                                == voxels[get_axis_index::<CS>(
                                    axis,
                                    forward + 2,
                                    bit_pos + 1,
                                    layer + 1,
                                )]
                            && ao_here == self.get_ao(face, layer, forward + 1, bit_pos)
                            && light_here == self.get_light4(face, layer, forward + 1, bit_pos)
                        {
                            self.forward_merged[bit_pos] += 1;
                            bits_here &= !(1 << bit_pos);
                            continue;
                        }

                        for right in (bit_pos + 1)..CS {
                            if (bits_here >> right & 1) == 0
                                || !ao_uniform(ao_here)
                                || !light_uniform(light_here)
                                || self.forward_merged[bit_pos] != self.forward_merged[right]
                                || v_type
                                    != voxels[get_axis_index::<CS>(
                                        axis,
                                        forward + 1,
                                        right + 1,
                                        layer + 1,
                                    )]
                                || ao_here != self.get_ao(face, layer, forward, right)
                                || light_here != self.get_light4(face, layer, forward, right)
                            {
                                break;
                            }
                            self.forward_merged[right] = 0;
                            right_merged += 1;
                        }
                        bits_here &= !((1u64 << (bit_pos + right_merged)) - 1);

                        let mesh_front = forward - self.forward_merged[bit_pos] as usize;
                        let mesh_left = bit_pos;
                        let mesh_up = layer + (!face & 1);
                        let mesh_width = right_merged;
                        let mesh_length = (self.forward_merged[bit_pos] + 1) as usize;

                        self.forward_merged[bit_pos] = 0;
                        right_merged = 1;

                        let quad = match face {
                            0 => Quad::pack(
                                mesh_front,
                                mesh_up,
                                mesh_left,
                                mesh_length,
                                mesh_width,
                                v_type as usize,
                                ao_here,
                                light_here,
                            ),
                            1 => Quad::pack(
                                mesh_front + mesh_length,
                                mesh_up,
                                mesh_left,
                                mesh_length,
                                mesh_width,
                                v_type as usize,
                                ao_here,
                                light_here,
                            ),
                            2 => Quad::pack(
                                mesh_up,
                                mesh_front + mesh_length,
                                mesh_left,
                                mesh_length,
                                mesh_width,
                                v_type as usize,
                                ao_here,
                                light_here,
                            ),
                            3 => Quad::pack(
                                mesh_up,
                                mesh_front,
                                mesh_left,
                                mesh_length,
                                mesh_width,
                                v_type as usize,
                                ao_here,
                                light_here,
                            ),
                            _ => unreachable!(),
                        };
                        self.quads[face].push(quad);
                    }
                }
            }
        }

        for face in 4..=5usize {
            let axis = face / 2;
            for forward in 0..CS {
                let bits_location = forward * CS + face * Self::CS_2;
                let bits_forward_location = (forward + 1) * CS + face * Self::CS_2;
                for right in 0..CS {
                    let mut bits_here = self.face_masks[right + bits_location];
                    if bits_here == 0 {
                        continue;
                    }
                    let bits_forward = if forward < CS - 1 {
                        self.face_masks[right + bits_forward_location]
                    } else {
                        0
                    };
                    let bits_right = if right < CS - 1 {
                        self.face_masks[right + 1 + bits_location]
                    } else {
                        0
                    };
                    let right_cs = right * CS;

                    while bits_here != 0 {
                        let bit_pos = bits_here.trailing_zeros() as usize;
                        bits_here &= !(1 << bit_pos);

                        let v_type =
                            voxels[get_axis_index::<CS>(axis, right + 1, forward + 1, bit_pos)];
                        let ao_here = self.get_ao(face, forward, right, bit_pos - 1);
                        let light_here = self.get_light4(face, forward, right, bit_pos - 1);
                        let forward_merge_i = right_cs + (bit_pos - 1);

                        let ao_forward = if (bits_forward >> bit_pos & 1) != 0 {
                            self.get_ao(face, forward + 1, right, bit_pos - 1)
                        } else {
                            255
                        };
                        let ao_right = if (bits_right >> bit_pos & 1) != 0 {
                            self.get_ao(face, forward, right + 1, bit_pos - 1)
                        } else {
                            255
                        };
                        // Precomputed before the `right_merged` borrow below to avoid aliasing
                        // `self`; sentinels are guarded by the `bits_*` checks, never compared.
                        let light_forward = if (bits_forward >> bit_pos & 1) != 0 {
                            self.get_light4(face, forward + 1, right, bit_pos - 1)
                        } else {
                            [255u8; 4]
                        };
                        let light_right = if (bits_right >> bit_pos & 1) != 0 {
                            self.get_light4(face, forward, right + 1, bit_pos - 1)
                        } else {
                            [255u8; 4]
                        };

                        let right_merged_ref = &mut self.right_merged[bit_pos - 1];

                        if *right_merged_ref == 0
                            && (bits_forward >> bit_pos & 1) != 0
                            && ao_uniform(ao_here)
                            && light_uniform(light_here)
                            && v_type
                                == voxels
                                    [get_axis_index::<CS>(axis, right + 1, forward + 2, bit_pos)]
                            && ao_here == ao_forward
                            && light_here == light_forward
                        {
                            self.forward_merged[forward_merge_i] += 1;
                            continue;
                        }

                        if (bits_right >> bit_pos & 1) != 0
                            && ao_uniform(ao_here)
                            && light_uniform(light_here)
                            && self.forward_merged[forward_merge_i]
                                == self.forward_merged[(right_cs + CS) + (bit_pos - 1)]
                            && v_type
                                == voxels
                                    [get_axis_index::<CS>(axis, right + 2, forward + 1, bit_pos)]
                            && ao_here == ao_right
                            && light_here == light_right
                        {
                            self.forward_merged[forward_merge_i] = 0;
                            *right_merged_ref += 1;
                            continue;
                        }

                        let mesh_left = right - *right_merged_ref as usize;
                        let mesh_front = forward - self.forward_merged[forward_merge_i] as usize;
                        let mesh_up = bit_pos - 1 + (!face & 1);
                        let mesh_width = 1 + *right_merged_ref;
                        let mesh_length = 1 + self.forward_merged[forward_merge_i];

                        self.forward_merged[forward_merge_i] = 0;
                        *right_merged_ref = 0;

                        let quad = Quad::pack(
                            mesh_left + (if face == 4 { mesh_width } else { 0 }) as usize,
                            mesh_front,
                            mesh_up,
                            mesh_width as usize,
                            mesh_length as usize,
                            v_type as usize,
                            ao_here,
                            light_here,
                        );
                        self.quads[face].push(quad);
                    }
                }
            }
        }
    }
}

#[inline]
fn face_value(v1: u16, v2: u16, transparents: &BTreeSet<u16>) -> u64 {
    (v2 == 0 || (v1 != v2 && transparents.contains(&v2))) as u64
}

#[inline]
fn get_axis_index<const CS: usize>(axis: usize, a: usize, b: usize, c: usize) -> usize {
    match axis {
        0 => b + (a * GreedyMesher::<CS>::CS_P) + (c * GreedyMesher::<CS>::CS_P2),
        1 => b + (c * GreedyMesher::<CS>::CS_P) + (a * GreedyMesher::<CS>::CS_P2),
        _ => c + (a * GreedyMesher::<CS>::CS_P) + (b * GreedyMesher::<CS>::CS_P2),
    }
}

fn face_normal(face: u8) -> (i32, i32, i32) {
    match face {
        0 => (0, 0, 1),  // Up (+Z in bgm's coordinate system)
        1 => (0, 0, -1), // Down
        2 => (0, 1, 0),  // Right (+Y)
        3 => (0, -1, 0), // Left
        4 => (1, 0, 0),  // Front (+X)
        5 => (-1, 0, 0), // Back
        _ => unreachable!(),
    }
}

fn axis_to_xyz(axis: usize, a: usize, b: usize, c: usize) -> (usize, usize, usize) {
    match axis {
        0 => (a, b, c),
        1 => (b, c, a),
        _ => (c, a, b),
    }
}

fn compute_vertex_ao_packed(
    face: u8,
    fx: i32,
    fy: i32,
    fz: i32,
    occ: &dyn Fn(i32, i32, i32) -> bool,
) -> u8 {
    let neighbors = face_ao_neighbors(face);
    // Vanilla's `shade0` fallback (corners[0]) for the both-sides-occlude case,
    // recovered from the neighbour table to stay in its coordinate frame.
    let c0 = shared_side(&neighbors[0], &neighbors[1]);
    let shade0_occ = occ(fx + c0[0], fy + c0[1], fz + c0[2]);
    let mut packed = 0u8;
    for (i, [s1, s2, c]) in neighbors.iter().enumerate() {
        let side1 = occ(fx + s1[0], fy + s1[1], fz + s1[2]);
        let side2 = occ(fx + s2[0], fy + s2[1], fz + s2[2]);
        let corner = occ(fx + c[0], fy + c[1], fz + c[2]);
        let ao = block_ao::vertex_ao_level(side1, side2, corner, shade0_occ);
        packed |= ao << (6 - i * 2);
    }
    packed
}

/// Per-corner smooth light: each corner averages the face-adjacent block (`(fx,
/// fy, fz)`, the face cell plus its normal) and the same three neighbours the
/// AO pass samples, packed in `face_ao_neighbors` order to match
/// `Quad::ao_levels`.
fn compute_vertex_light_packed(
    face: u8,
    fx: i32,
    fy: i32,
    fz: i32,
    lit: &dyn Fn(i32, i32, i32) -> f32,
) -> [u8; 4] {
    let neighbors = face_ao_neighbors(face);
    let center = lit(fx, fy, fz);
    let mut packed = [0u8; 4];
    for (i, [s1, s2, c]) in neighbors.iter().enumerate() {
        let side1 = lit(fx + s1[0], fy + s1[1], fz + s1[2]);
        let side2 = lit(fx + s2[0], fy + s2[1], fz + s2[2]);
        let corner = lit(fx + c[0], fy + c[1], fz + c[2]);
        let brightness = 0.25 * (center + side1 + side2 + corner);
        packed[i] = (brightness * 255.0).round() as u8;
    }
    packed
}

/// Whether all four packed AO corners are equal (a flat-lit face). Vanilla
/// renders every block face individually, so a merged quad only reproduces its
/// shading when the AO is uniform; gradient faces must stay 1x1.
#[inline]
fn ao_uniform(ao: u8) -> bool {
    let c = (ao >> 6) & 3;
    ((ao >> 4) & 3) == c && ((ao >> 2) & 3) == c && (ao & 3) == c
}

/// Whether all four packed smooth-light corners are equal. Mirrors
/// `ao_uniform`: a quad may only merge across cells whose light is uniform.
#[inline]
fn light_uniform(l: [u8; 4]) -> bool {
    l[0] == l[1] && l[1] == l[2] && l[2] == l[3]
}

/// The side offset common to both vertices' neighbour triples (their first two
/// entries are the two edge sides). This is vanilla's `corners[0]`.
fn shared_side(v0: &[[i32; 3]; 3], v1: &[[i32; 3]; 3]) -> [i32; 3] {
    if v0[0] == v1[0] || v0[0] == v1[1] {
        v0[0]
    } else {
        v0[1]
    }
}

fn face_ao_neighbors(face: u8) -> [[[i32; 3]; 3]; 4] {
    match face {
        0 => [
            [[-1, 0, 0], [0, -1, 0], [-1, -1, 0]],
            [[1, 0, 0], [0, -1, 0], [1, -1, 0]],
            [[1, 0, 0], [0, 1, 0], [1, 1, 0]],
            [[-1, 0, 0], [0, 1, 0], [-1, 1, 0]],
        ],
        1 => [
            [[-1, 0, 0], [0, -1, 0], [-1, -1, 0]],
            [[1, 0, 0], [0, -1, 0], [1, -1, 0]],
            [[1, 0, 0], [0, 1, 0], [1, 1, 0]],
            [[-1, 0, 0], [0, 1, 0], [-1, 1, 0]],
        ],
        2 => [
            [[-1, 0, 0], [0, 0, -1], [-1, 0, -1]],
            [[1, 0, 0], [0, 0, -1], [1, 0, -1]],
            [[1, 0, 0], [0, 0, 1], [1, 0, 1]],
            [[-1, 0, 0], [0, 0, 1], [-1, 0, 1]],
        ],
        3 => [
            [[-1, 0, 0], [0, 0, 1], [-1, 0, 1]],
            [[1, 0, 0], [0, 0, 1], [1, 0, 1]],
            [[1, 0, 0], [0, 0, -1], [1, 0, -1]],
            [[-1, 0, 0], [0, 0, -1], [-1, 0, -1]],
        ],
        4 => [
            [[0, -1, 0], [0, 0, -1], [0, -1, -1]],
            [[0, 1, 0], [0, 0, -1], [0, 1, -1]],
            [[0, 1, 0], [0, 0, 1], [0, 1, 1]],
            [[0, -1, 0], [0, 0, 1], [0, -1, 1]],
        ],
        5 => [
            [[0, 1, 0], [0, 0, -1], [0, 1, -1]],
            [[0, -1, 0], [0, 0, -1], [0, -1, -1]],
            [[0, -1, 0], [0, 0, 1], [0, -1, 1]],
            [[0, 1, 0], [0, 0, 1], [0, 1, 1]],
        ],
        _ => unreachable!(),
    }
}

pub fn pad_linearize<const CS: usize>(x: usize, y: usize, z: usize) -> usize {
    (y * GreedyMesher::<CS>::CS_P + x) * GreedyMesher::<CS>::CS_P + z
}

#[derive(Clone, Copy)]
pub enum Face {
    Up,
    Down,
    Right,
    Left,
    Front,
    Back,
}

impl From<usize> for Face {
    fn from(v: usize) -> Self {
        match v {
            0 => Self::Up,
            1 => Self::Down,
            2 => Self::Right,
            3 => Self::Left,
            4 => Self::Front,
            _ => Self::Back,
        }
    }
}

impl Face {
    pub fn shade_light(&self) -> f32 {
        match self {
            Self::Up => 1.0,
            Self::Down => 0.5,
            Self::Front | Self::Back => 0.8,
            Self::Right | Self::Left => 0.6,
        }
    }

    pub fn vertices(&self, quad: &Quad) -> [([f32; 3], [f32; 2]); 4] {
        let [x, y, z] = quad.xyz();
        let (x, y, z) = (x as f32, y as f32, z as f32);
        let w = quad.width() as f32;
        let h = quad.height() as f32;
        match self {
            Face::Up => [
                ([x + w, z, y + h], [w, h]),
                ([x + w, z, y], [w, 0.0]),
                ([x, z, y + h], [0.0, h]),
                ([x, z, y], [0.0, 0.0]),
            ],
            Face::Down => [
                ([x, z, y + h], [w, h]),
                ([x, z, y], [w, 0.0]),
                ([x + w, z, y + h], [0.0, h]),
                ([x + w, z, y], [0.0, 0.0]),
            ],
            Face::Right => [
                ([y, z + h, x], [0.0, 0.0]),
                ([y, z, x], [h, 0.0]),
                ([y + w, z + h, x], [0.0, w]),
                ([y + w, z, x], [h, w]),
            ],
            Face::Left => [
                ([y, z, x], [h, w]),
                ([y, z + h, x], [0.0, w]),
                ([y + w, z, x], [h, 0.0]),
                ([y + w, z + h, x], [0.0, 0.0]),
            ],
            Face::Front => [
                ([x, y + h, z], [0.0, 0.0]),
                ([x, y, z], [0.0, h]),
                ([x, y + h, z + w], [w, 0.0]),
                ([x, y, z + w], [w, h]),
            ],
            Face::Back => [
                ([x, y + h, z + w], [w, 0.0]),
                ([x, y, z + w], [w, h]),
                ([x, y + h, z], [0.0, 0.0]),
                ([x, y, z], [0.0, h]),
            ],
        }
    }
}
