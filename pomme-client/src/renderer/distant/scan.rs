//! Voxy's `ScanMesher2D` and `RenderDataFactory.Mesher`: a 32-wide plane of
//! voxel faces fed one at a time, row by row, merged greedily into quads of
//! up to 16x16 and written out as Voxy quads.

/// A quad's bits (`quad_format.glsl`): face in 0-2 (`axis << 1 | side`,
/// axis 0 for y, 1 for z, 2 for x), width-1 in 3-6, height-1 in 7-10, z in
/// 11-15, y in 16-20, x in 21-25, model id in 26-45, biome in 46-54, light
/// in 55-62.
pub(crate) const QUAD_MODEL_SHIFT: u32 = 26;
pub(crate) const QUAD_BIOME_SHIFT: u32 = 46;
pub(crate) const QUAD_LIGHT_SHIFT: u32 = 55;
/// The quad's light byte: sky in the low nibble, block in the high.
pub(crate) const QUAD_LIGHT: u64 = 0xFF << QUAD_LIGHT_SHIFT;
/// The bits below the model id, which the mesher uses for the face's side
/// (bit 0) and its group (bits 1-2) until the quad is placed.
const AUX: u64 = (1 << QUAD_MODEL_SHIFT) - 1;

const MAX_SIZE: u8 = 16;

/// The groups a section's quads come in: translucent, double-sided, then
/// one per face.
pub(crate) const GROUPS: usize = 8;

/// Where a section's quads land, and the bounds they span.
pub(crate) struct Quads {
    pub groups: [Vec<u64>; GROUPS],
    pub min: [i32; 3],
    pub max: [i32; 3],
}

impl Quads {
    pub fn new() -> Self {
        Self {
            groups: Default::default(),
            min: [i32::MAX; 3],
            max: [i32::MIN; 3],
        }
    }

    pub fn clear(&mut self) {
        for group in &mut self.groups {
            group.clear();
        }
        self.min = [i32::MAX; 3];
        self.max = [i32::MIN; 3];
    }

    pub fn count(&self) -> usize {
        self.groups.iter().map(Vec::len).sum()
    }
}

/// One plane's mesher. `axis` is the plane's normal; `position` which layer
/// along it, and `face_offset` whether a face on the negative side belongs
/// to the next layer up (the inner passes) or to this one (the outer).
pub(crate) struct Mesher {
    row_data: [u64; 32],
    row_length: [u8; 32],
    row_depth: [u8; 32],
    row_bitset: u32,
    index: u32,
    sum: u8,
    data: u64,
    pub axis: u8,
    pub position: u8,
    pub face_offset: bool,
}

impl Mesher {
    pub fn new(axis: u8, position: u8, face_offset: bool) -> Self {
        Self {
            row_data: [0; 32],
            row_length: [0; 32],
            row_depth: [0; 32],
            row_bitset: 0,
            index: 0,
            sum: 0,
            data: 0,
            axis,
            position,
            face_offset,
        }
    }

    /// The next face along the row; 0 for none.
    pub fn put(&mut self, data: u64, out: &mut Quads) {
        let idx = (self.index & 31) as usize;
        self.index += 1;
        if idx == 0 {
            // A row starts: what the last row ended on couldn't merge
            // sideways, so it goes in slot 31.
            if self.data != 0 {
                if self.row_bitset & (1 << 31) != 0 {
                    self.emit(31, ((self.index - 1) >> 5) - 1, out);
                }
                self.row_bitset |= 1 << 31;
                self.row_length[31] = self.sum;
                self.row_depth[31] = 1;
                self.row_data[31] = self.data;
            }
            self.data = data;
            self.sum = 0;
        }
        if data != self.data || self.sum == MAX_SIZE {
            if self.data != 0 {
                let prev = idx - 1;
                self.row_depth[prev] = 1;
                self.row_length[prev] = self.sum;
                self.row_data[prev] = self.data;
                self.row_bitset |= 1 << prev;
            }
            self.data = data;
            self.sum = 0;
        }
        self.sum += 1;

        let is_set = self.row_bitset & (1 << idx) != 0;
        let mut by_depth_max = false;
        // The run above, ending here with the same data and length, takes
        // this one.
        if self.data != 0
            && is_set
            && self.row_length[idx] == self.sum
            && self.row_data[idx] == self.data
        {
            self.row_depth[idx] += 1;
            self.sum = 0;
            self.data = 0;
            if self.row_depth[idx] != MAX_SIZE {
                return;
            }
            by_depth_max = true;
        }
        if is_set {
            let row = ((self.index - 1) >> 5) - u32::from(!by_depth_max);
            self.emit(idx, row, out);
            self.row_bitset &= !(1 << idx);
        }
    }

    /// Writes out every pending run in `mask`'s slots.
    fn emit_ranged(&mut self, mask: u32, out: &mut Quads) {
        let mut set = self.row_bitset & mask;
        self.row_bitset &= !mask;
        while set != 0 {
            let idx = set.trailing_zeros() as usize;
            set &= set - 1;
            self.emit(idx, (self.index >> 5) - 1, out);
        }
    }

    /// `count` faces of nothing.
    pub fn skip(&mut self, mut count: u32, out: &mut Quads) {
        if count == 0 {
            return;
        }
        if self.data != 0 {
            self.put(0, out);
            count -= 1;
        }
        if count > 0 {
            let mask = (((1u64 << count.min(32)) - 1) << (self.index & 31)) as u32;
            self.emit_ranged(mask, out);
            self.index += count;
        }
    }

    pub fn end_row(&mut self, out: &mut Quads) {
        if self.index & 31 != 0 {
            self.skip(32 - (self.index & 31), out);
        }
    }

    /// Writes out whatever is pending and starts over.
    pub fn finish(&mut self, out: &mut Quads) {
        if self.index != 0 {
            self.skip(32 - (self.index & 31), out);
            self.emit_ranged(u32::MAX, out);
        }
        self.row_bitset = 0;
        self.sum = 0;
        self.data = 0;
        self.index = 0;
    }

    /// `RenderDataFactory.Mesher.emitQuad`: the run ending at column `x` of
    /// row `z`, `length` along the row and `width` rows deep, placed as a
    /// quad.
    fn emit(&self, x: usize, z: u32, out: &mut Quads) {
        let (mut length, mut width) = (u32::from(self.row_length[x]), u32::from(self.row_depth[x]));
        let data = self.row_data[x];
        let mut x = x as u32 + 1 - length;
        let mut z = z + 1 - width;
        if self.axis == 2 {
            // Along x, rows run up y and columns along z.
            std::mem::swap(&mut x, &mut z);
            std::mem::swap(&mut length, &mut width);
        }
        let aux = (data & AUX) as u32;
        let data = data & !AUX;
        let side = aux & 1;
        let group = aux >> 1 & 3;
        let position = u32::from(self.position) + 1 - if self.face_offset { side } else { 1 };
        let axis = u32::from(self.axis);
        let face = axis << 1 | side;
        let mut encoded = face | (width - 1) << 7 | (length - 1) << 3;
        encoded |= x << if axis == 2 { 16 } else { 21 };
        encoded |= z << if axis == 1 { 16 } else { 11 };
        encoded |= position
            << match axis {
                0 => 16,
                1 => 11,
                _ => 21,
            };
        let group = group as usize + if group == 2 { face as usize } else { 0 };
        out.groups[group].push(data | u64::from(encoded));

        let (position, x, z, length, width) = (
            position as i32,
            x as i32,
            z as i32,
            length as i32,
            width as i32,
        );
        // The bounds along the normal span only the layers, Voxy's quirk.
        let (lo, hi) = match axis {
            0 => ([x, position, z], [x + length, position, z + width]),
            1 => ([x, z, position], [x + length, z + width, position]),
            _ => ([position, x, z], [position, x + length, z + width]),
        };
        for i in 0..3 {
            out.min[i] = out.min[i].min(lo[i]);
            out.max[i] = out.max[i].max(hi[i]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds a 32x32 plane of face data, row by row, and returns each
    /// quad's `(x, z, length, width, data)`, the data's aux bits cleared.
    fn mesh(plane: &[u64; 1024]) -> Vec<(u32, u32, u32, u32, u64)> {
        let mut mesher = Mesher::new(0, 0, true);
        let mut out = Quads::new();
        for &data in plane {
            if data == 0 {
                mesher.skip(1, &mut out);
            } else {
                mesher.put(data, &mut out);
            }
        }
        mesher.finish(&mut out);
        out.groups
            .iter()
            .flatten()
            .map(|&quad| {
                let field = |shift: u32, bits: u32| (quad >> shift) as u32 & ((1 << bits) - 1);
                (
                    field(21, 5),
                    field(11, 5),
                    field(3, 4) + 1,
                    field(7, 4) + 1,
                    quad & !((1 << 26) - 1),
                )
            })
            .collect()
    }

    const A: u64 = 1 << 26;
    const B: u64 = 2 << 26;

    #[test]
    fn runs_merge_across_rows_up_to_sixteen() {
        let mut plane = [0u64; 1024];
        for z in 0..20 {
            for x in 0..20 {
                plane[z * 32 + x] = A;
            }
        }
        let mut quads = mesh(&plane);
        quads.sort_unstable();
        assert_eq!(
            quads,
            [
                (0, 0, 16, 16, A),
                (0, 16, 16, 4, A),
                (16, 0, 4, 16, A),
                (16, 16, 4, 4, A)
            ]
        );
    }

    #[test]
    fn every_face_is_covered_exactly_once() {
        // A fixed pseudo-random plane of two kinds of face and gaps.
        let mut seed = 12345u32;
        let plane: [u64; 1024] = std::array::from_fn(|_| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            match seed >> 28 {
                0..=5 => A,
                6..=12 => B,
                _ => 0,
            }
        });
        let mut covered = [0u64; 1024];
        for (x, z, length, width, data) in mesh(&plane) {
            assert!((1..=16).contains(&length) && (1..=16).contains(&width));
            for dz in 0..width {
                for dx in 0..length {
                    let at = ((z + dz) * 32 + x + dx) as usize;
                    assert_eq!(covered[at], 0, "face {at} covered twice");
                    covered[at] = data;
                }
            }
        }
        assert_eq!(covered, plane);
    }

    #[test]
    fn quads_land_on_their_face_plane() {
        // The negative face of an inner layer belongs to the layer above.
        let mut out = Quads::new();
        let mut mesher = Mesher::new(0, 5, true);
        mesher.put(A | 1, &mut out);
        mesher.finish(&mut out);
        mesher.put(A, &mut out);
        mesher.finish(&mut out);
        let positions: Vec<u64> = out.groups.iter().flatten().map(|q| q >> 16 & 31).collect();
        assert_eq!(positions, [5, 6]);
        assert_eq!(out.min, [0, 5, 0]);
        assert_eq!(out.max, [1, 6, 1]);
        // Along x the row runs up y and the column along z.
        let mut out = Quads::new();
        let mut mesher = Mesher::new(2, 7, false);
        mesher.skip(3, &mut out);
        mesher.put(B | 0b101, &mut out);
        mesher.finish(&mut out);
        let quad = out.groups[7][0];
        assert_eq!(
            (quad >> 21 & 31, quad >> 16 & 31, quad >> 11 & 31),
            (7, 0, 3)
        );
    }
}
