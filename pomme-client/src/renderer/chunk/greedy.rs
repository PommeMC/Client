// Vendored from https://github.com/Inspirateur/binary-greedy-meshing (MIT)
// Modified to use AO as a merge criterion.

use std::collections::BTreeSet;

const MASK_6: u64 = 0b111111;

#[derive(Copy, Clone)]
pub struct Quad {
    packed: u64,
    pub ao: u8,
}

impl Quad {
    fn pack(x: usize, y: usize, z: usize, w: usize, h: usize, v_type: usize, ao: u8) -> Self {
        Self {
            packed: ((v_type << 32) | (h << 24) | (w << 18) | (z << 12) | (y << 6) | x) as u64,
            ao,
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
            forward_merged: vec![0; Self::CS_2].into_boxed_slice(),
            right_merged: vec![0; CS].into_boxed_slice(),
            quads: core::array::from_fn(|_| Vec::new()),
        }
    }

    pub fn mesh(&mut self, voxels: &[u16], occluders: &[bool], transparents: &BTreeSet<u16>) {
        self.face_culling(voxels, transparents);
        self.compute_ao(occluders);
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

    fn compute_ao(&mut self, occluders: &[bool]) {
        let occ = |x: i32, y: i32, z: i32| -> bool {
            if x < 0 || y < 0 || z < 0 {
                return false;
            }
            let (x, y, z) = (x as usize, y as usize, z as usize);
            if x >= Self::CS_P || y >= Self::CS_P || z >= Self::CS_P {
                return false;
            }
            occluders[(y * Self::CS_P + x) * Self::CS_P + z]
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

                        let ao = compute_vertex_ao_packed(face, fx, fy, fz, &occ);
                        let ao_idx = (face as usize * Self::CS_2 * CS)
                            + (layer * CS + forward) * CS
                            + bit_pos;
                        self.ao_faces[ao_idx] = ao;
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

                        let ao = compute_vertex_ao_packed(face, fx, fy, fz, &occ);
                        let ao_idx = (face as usize * Self::CS_2 * CS)
                            + (forward * CS + right) * CS
                            + (bit_pos - 1);
                        self.ao_faces[ao_idx] = ao;
                    }
                }
            }
        }
    }

    fn get_ao(&self, face: usize, layer: usize, forward: usize, right: usize) -> u8 {
        let idx = face * Self::CS_2 * CS + (layer * CS + forward) * CS + right;
        self.ao_faces[idx]
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

                        if (bits_next >> bit_pos & 1) != 0
                            && v_type
                                == voxels[get_axis_index::<CS>(
                                    axis,
                                    forward + 2,
                                    bit_pos + 1,
                                    layer + 1,
                                )]
                            && ao_here == self.get_ao(face, layer, forward + 1, bit_pos)
                        {
                            self.forward_merged[bit_pos] += 1;
                            bits_here &= !(1 << bit_pos);
                            continue;
                        }

                        for right in (bit_pos + 1)..CS {
                            if (bits_here >> right & 1) == 0
                                || self.forward_merged[bit_pos] != self.forward_merged[right]
                                || v_type
                                    != voxels[get_axis_index::<CS>(
                                        axis,
                                        forward + 1,
                                        right + 1,
                                        layer + 1,
                                    )]
                                || ao_here != self.get_ao(face, layer, forward, right)
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
                            ),
                            1 => Quad::pack(
                                mesh_front + mesh_length,
                                mesh_up,
                                mesh_left,
                                mesh_length,
                                mesh_width,
                                v_type as usize,
                                ao_here,
                            ),
                            2 => Quad::pack(
                                mesh_up,
                                mesh_front + mesh_length,
                                mesh_left,
                                mesh_length,
                                mesh_width,
                                v_type as usize,
                                ao_here,
                            ),
                            3 => Quad::pack(
                                mesh_up,
                                mesh_front,
                                mesh_left,
                                mesh_length,
                                mesh_width,
                                v_type as usize,
                                ao_here,
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

                        let right_merged_ref = &mut self.right_merged[bit_pos - 1];

                        if *right_merged_ref == 0
                            && (bits_forward >> bit_pos & 1) != 0
                            && v_type
                                == voxels
                                    [get_axis_index::<CS>(axis, right + 1, forward + 2, bit_pos)]
                            && ao_here == ao_forward
                        {
                            self.forward_merged[forward_merge_i] += 1;
                            continue;
                        }

                        if (bits_right >> bit_pos & 1) != 0
                            && self.forward_merged[forward_merge_i]
                                == self.forward_merged[(right_cs + CS) + (bit_pos - 1)]
                            && v_type
                                == voxels
                                    [get_axis_index::<CS>(axis, right + 2, forward + 1, bit_pos)]
                            && ao_here == ao_right
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
    let mut packed = 0u8;
    for (i, [s1, s2, c]) in neighbors.iter().enumerate() {
        let side1 = occ(fx + s1[0], fy + s1[1], fz + s1[2]) as u8;
        let side2 = occ(fx + s2[0], fy + s2[1], fz + s2[2]) as u8;
        let corner = occ(fx + c[0], fy + c[1], fz + c[2]) as u8;
        let ao = if side1 == 1 && side2 == 1 {
            0
        } else {
            3 - (side1 + side2 + corner)
        };
        packed |= ao << (6 - i * 2);
    }
    packed
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
    pub fn offset(&self) -> [i32; 3] {
        match self {
            Self::Up => [0, 1, 0],
            Self::Down => [0, -1, 0],
            Self::Right => [1, 0, 0],
            Self::Left => [-1, 0, 0],
            Self::Front => [0, 0, 1],
            Self::Back => [0, 0, -1],
        }
    }

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
