//! The section mesher: a world section meshed into quads, a pass per axis
//! over its opaque blocks, its fluids and its other blocks, each face culled
//! against what it touches, inside the section and in the six around it.

use super::models::{Metadata, Models};
use super::scan::{
    GROUPS, Mesher, QUAD_BIOME_SHIFT, QUAD_LIGHT, QUAD_LIGHT_SHIFT, QUAD_MODEL_SHIFT, Quads,
};
use super::section::{self, SectionKey, UNSEEN, VOLUME, WorldSection};
use super::voxel;

/// Model ids in a quad, where the mesher compares them.
const MODEL: u64 = 0xFFFF << QUAD_MODEL_SHIFT;
/// A quad's block light.
const BLOCK_LIGHT: u64 = 0xF << (QUAD_LIGHT_SHIFT + 4);

pub(crate) struct BuiltSection {
    pub key: SectionKey,
    /// [`WorldSection::non_empty_children`] when it was meshed.
    pub child_existence: u8,
    /// Its quads' bounds: min x, y, z in 5 bits each, then the extents less
    /// one in 5 bits each from bit 15.
    pub aabb: u32,
    /// Every group's quads in turn ([`GROUPS`]); empty for a section with
    /// nothing to draw.
    pub quads: Vec<u64>,
    /// Where each group starts in `quads`.
    pub offsets: [u32; GROUPS],
}

impl BuiltSection {
    pub fn empty(key: SectionKey, child_existence: u8) -> Self {
        Self {
            key,
            child_existence,
            aabb: 0,
            quads: Vec::new(),
            offsets: [0; GROUPS],
        }
    }
}

/// The neighbouring sections' slices the outer passes read: -x, +x, -y, +y,
/// -z, +z, each 32x32.
const SIDES: [[i32; 3]; 6] = [
    [-1, 0, 0],
    [1, 0, 0],
    [0, -1, 0],
    [0, 1, 0],
    [0, 0, -1],
    [0, 0, 1],
];

pub(crate) struct Factory {
    /// Per voxel, its quad without a position, then its model's metadata;
    /// air keeps only its light.
    section: Box<[u64]>,
    /// The six neighbours' facing slices ([`SIDES`]).
    neighbours: Box<[u64]>,
    /// A bit per x of each row `y * 32 + z`: fully opaque blocks, blocks
    /// neither opaque nor fluid, and fluids or blocks holding one.
    opaque: Box<[u32; 1024]>,
    non_opaque: Box<[u32; 1024]>,
    fluid: Box<[u32; 1024]>,
    /// The y and z passes' meshers: positive faces, then negative ones of
    /// the blocks that aren't opaque.
    mesher: Mesher,
    secondary: Mesher,
    /// The x pass's, one per layer, the same way.
    x_meshers: Box<[Mesher; 32]>,
    x_secondary: Box<[Mesher; 32]>,
    out: Quads,
}

impl Factory {
    pub fn new() -> Self {
        Self {
            section: vec![0; VOLUME * 2].into(),
            neighbours: vec![UNSEEN; 6 * 1024].into(),
            opaque: Box::new([0; 1024]),
            non_opaque: Box::new([0; 1024]),
            fluid: Box::new([0; 1024]),
            mesher: Mesher::new(0, 0, true),
            secondary: Mesher::new(0, 0, false),
            x_meshers: Box::new(std::array::from_fn(|i| Mesher::new(2, i as u8, true))),
            x_secondary: Box::new(std::array::from_fn(|i| Mesher::new(2, i as u8, false))),
            out: Quads::new(),
        }
    }

    /// `section` at `key` meshed, `neighbour` giving the sections around it
    /// (missing ones are unseen air).
    pub fn build<'a>(
        &mut self,
        key: SectionKey,
        section: &'a WorldSection,
        neighbour: impl Fn(SectionKey) -> Option<&'a WorldSection>,
        models: &mut dyn Models,
    ) -> BuiltSection {
        self.out.clear();
        let needed = self.prepare(&section.data, models);
        for (side, offset) in SIDES.iter().enumerate() {
            if needed & 1 << side == 0 {
                continue;
            }
            let slice = &mut self.neighbours[side * 1024..][..1024];
            let next = SectionKey {
                level: key.level,
                x: key.x + offset[0],
                y: key.y + offset[1],
                z: key.z + offset[2],
            };
            match neighbour(next) {
                Some(next) => {
                    for (i, voxel) in slice.iter_mut().enumerate() {
                        *voxel = next.data[neighbour_index(side, i)];
                    }
                }
                None => slice.fill(UNSEEN),
            }
        }
        for axis in 0..2 {
            self.mesher.axis = axis;
            self.secondary.axis = axis;
            self.opaque_inner(axis);
            self.opaque_outer(axis, models);
            self.fluid_inner(axis, models);
            self.fluid_outer(axis, models);
            self.non_opaque_inner(axis);
            self.non_opaque_outer(axis, models);
        }
        self.x_opaque_inner();
        self.x_opaque_outer(models);
        self.finish_x();
        self.x_fluid_inner(models);
        self.x_fluid_outer(models);
        self.finish_x();
        self.x_non_opaque_inner();
        self.x_non_opaque_outer(models);
        self.finish_x();

        if self.out.count() == 0 {
            return BuiltSection::empty(key, section.non_empty_children);
        }
        let mut offsets = [0; GROUPS];
        let mut quads = Vec::with_capacity(self.out.count());
        for (group, offset) in self.out.groups.iter().zip(&mut offsets) {
            *offset = quads.len() as u32;
            quads.extend_from_slice(group);
        }
        let [min_x, min_y, min_z] = self.out.min.map(|c| c as u32);
        let extent = |axis: usize| (self.out.max[axis] - self.out.min[axis] - 1).max(0) as u32;
        let aabb =
            min_x | min_y << 5 | min_z << 10 | extent(0) << 15 | extent(1) << 20 | extent(2) << 25;
        BuiltSection {
            key,
            child_existence: section.non_empty_children,
            aabb,
            quads,
            offsets,
        }
    }

    /// Each voxel's quad and metadata and the row masks; returns which sides
    /// have blocks against them ([`SIDES`]).
    fn prepare(&mut self, data: &[u64], models: &mut dyn Models) -> u8 {
        let mut needed = 0;
        for y in 0..32 {
            for z in 0..32 {
                let (mut not_empty, mut opaque, mut pure_fluid, mut partial_fluid) =
                    (0u32, 0, 0, 0);
                for x in 0..32 {
                    let i = section::index(x, y, z);
                    let voxel = data[i];
                    let model = if voxel::is_air(voxel) {
                        0
                    } else {
                        models.model(voxel::block_id(voxel))
                    };
                    let (quad, meta) = if model == 0 {
                        ((voxel & voxel::LIGHT_MASK) >> 1, Metadata(0))
                    } else {
                        let meta = models.metadata(model);
                        not_empty |= 1 << x;
                        opaque |= u32::from(meta.is_fully_opaque()) << x;
                        pure_fluid |= u32::from(meta.is_fluid()) << x;
                        partial_fluid |= u32::from(meta.contains_fluid()) << x;
                        (pack_partial(model, voxel, meta), meta)
                    };
                    self.section[i * 2] = quad;
                    self.section[i * 2 + 1] = meta.0;
                }
                let row = y * 32 + z;
                self.opaque[row] = opaque;
                self.non_opaque[row] = (not_empty ^ opaque) & !pure_fluid;
                self.fluid[row] = pure_fluid | partial_fluid;
                if not_empty != 0 {
                    needed |= (not_empty & 1) as u8
                        | ((not_empty >> 31) << 1) as u8
                        | u8::from(y == 0) << 2
                        | u8::from(y == 31) << 3
                        | u8::from(z == 0) << 4
                        | u8::from(z == 31) << 5;
                }
            }
        }
        needed
    }

    fn quad(&self, i: usize) -> u64 {
        self.section[i * 2]
    }

    fn meta(&self, i: usize) -> Metadata {
        Metadata(self.section[i * 2 + 1])
    }

    fn neighbour(&self, side: usize, i: usize) -> u64 {
        self.neighbours[side * 1024 + i]
    }

    /// The neighbouring section's voxel against row `other`, x `index` of
    /// `axis`'s outer layer on `side`.
    fn across(&self, axis: u8, side: usize, other: usize, index: i32) -> u64 {
        self.neighbour((axis as usize + 1) * 2 + side, other * 32 + index as usize)
    }

    /// The neighbouring block's model and metadata, 0 for air.
    fn neighbour_model(models: &mut dyn Models, voxel: u64) -> (u16, Metadata) {
        if voxel::is_air(voxel) {
            return (0, Metadata(0));
        }
        let model = models.model(voxel::block_id(voxel));
        (model, models.metadata(model))
    }

    /// Voxel `index` of the y or z pass's row `pidx` in `axis`'s layer.
    fn row_index(axis: u8, layer: usize, other: usize) -> usize {
        if axis == 0 {
            layer * 32 + other
        } else {
            other * 32 + layer
        }
    }

    /// The voxel `idx` a face belongs to and the one across it, `step`
    /// voxels on: `idx` faces forward when it's the solid one.
    fn pair(idx: usize, step: usize, forward: u32) -> (usize, usize) {
        if forward == 1 {
            (idx, idx + step)
        } else {
            (idx + step, idx)
        }
    }

    /// The y and z passes' walk of `axis`'s layer `layer`, row by row:
    /// `mask` gives a row's faces by x, `each` meshes one, and the meshers
    /// skip the rest (the secondary too when `both`).
    fn walk(
        &mut self,
        axis: u8,
        layer: usize,
        both: bool,
        mask: impl Fn(&Self, usize) -> u32,
        mut each: impl FnMut(&mut Self, usize, usize, i32),
    ) {
        self.mesher.position = layer as u8;
        self.secondary.position = layer as u8;
        let mut skips = 0;
        for other in 0..32 {
            let pidx = Self::row_index(axis, layer, other);
            let mask = mask(self, pidx);
            if mask == 0 {
                skips += 32;
                continue;
            }
            self.mesher.skip(skips, &mut self.out);
            if both {
                self.secondary.skip(skips, &mut self.out);
            }
            skips = 0;
            for (index, gap) in faces(mask) {
                self.mesher.skip(gap, &mut self.out);
                if both {
                    self.secondary.skip(gap, &mut self.out);
                }
                each(self, pidx, other, index);
            }
            self.mesher.end_row(&mut self.out);
            if both {
                self.secondary.end_row(&mut self.out);
            }
        }
        self.mesher.finish(&mut self.out);
        if both {
            self.secondary.finish(&mut self.out);
        }
    }

    /// Mask rows between the layers along `axis`.
    fn step(axis: u8) -> usize {
        if axis == 0 { 32 } else { 1 }
    }

    /// The faces between opaque blocks and everything else, between the
    /// layers along `axis`.
    fn opaque_inner(&mut self, axis: u8) {
        self.mesher.face_offset = true;
        let step = Self::step(axis);
        for layer in 0..31 {
            self.walk(
                axis,
                layer,
                false,
                |s, pidx| s.opaque[pidx] ^ s.opaque[pidx + step],
                |s, pidx, _, index| {
                    let forward = s.opaque[pidx] >> index & 1;
                    let (own, next) = Self::pair(index as usize + pidx * 32, step * 32, forward);
                    let (own_quad, next_quad, next_meta) =
                        (s.quad(own), s.quad(next), s.meta(next));
                    let culls = (own_quad ^ next_quad) & MODEL == 0 && next_meta.culls_same()
                        || next_meta.face_occludes(axis << 1 | (1 - forward) as u8);
                    let quad = (!culls).then(|| {
                        apply_light(
                            u64::from(forward) | own_quad & !QUAD_LIGHT | next_quad & QUAD_LIGHT,
                            s.meta(own),
                        )
                    });
                    put_or_skip(&mut s.mesher, quad, &mut s.out);
                },
            );
        }
    }

    /// The opaque faces on the section's two sides along `axis`, against the
    /// neighbouring sections.
    fn opaque_outer(&mut self, axis: u8, models: &mut dyn Models) {
        self.mesher.face_offset = false;
        for side in 0..2usize {
            self.walk(
                axis,
                side * 31,
                false,
                |s, pidx| s.opaque[pidx],
                |s, pidx, other, index| {
                    let idx = index as usize + pidx * 32;
                    let voxel = s.across(axis, side, other, index);
                    let own_quad = s.quad(idx);
                    let (model, meta) = Self::neighbour_model(models, voxel);
                    let culls = model != 0
                        && (meta.is_fully_opaque()
                            || u64::from(model) << QUAD_MODEL_SHIFT == own_quad & MODEL
                                && meta.culls_same()
                            || meta.face_occludes(axis << 1 | (1 - side) as u8));
                    let quad = (!culls).then(|| {
                        apply_light(
                            side as u64 | own_quad & !QUAD_LIGHT | neighbour_light(voxel),
                            s.meta(idx),
                        )
                    });
                    put_or_skip(&mut s.mesher, quad, &mut s.out);
                },
            );
        }
        self.mesher.face_offset = true;
    }

    /// Fluid faces between layers, a block holding a fluid drawing the
    /// fluid's model instead.
    fn fluid_inner(&mut self, axis: u8, models: &mut dyn Models) {
        let step = Self::step(axis);
        for layer in 0..31 {
            self.walk(
                axis,
                layer,
                false,
                |s, pidx| {
                    let (current, next) = (s.fluid[pidx], s.fluid[pidx + step]);
                    ((current | s.opaque[pidx]) ^ (next | s.opaque[pidx + step])) & (current | next)
                },
                |s, pidx, _, index| {
                    let forward = s.fluid[pidx] >> index & 1;
                    let (own, next) = Self::pair(index as usize + pidx * 32, step * 32, forward);
                    let quad =
                        (!s.meta(next).face_occludes(axis << 1 | (1 - forward) as u8)).then(|| {
                            let (quad, meta) = as_fluid(models, s.quad(own), s.meta(own));
                            apply_light(
                                u64::from(forward) | quad & !QUAD_LIGHT | s.quad(next) & QUAD_LIGHT,
                                meta,
                            )
                        });
                    put_or_skip(&mut s.mesher, quad, &mut s.out);
                },
            );
        }
    }

    /// The fluid faces on the section's two sides along `axis`, against the
    /// neighbouring sections; a neighbour holding a fluid culls as that fluid.
    fn fluid_outer(&mut self, axis: u8, models: &mut dyn Models) {
        self.mesher.face_offset = false;
        for side in 0..2usize {
            self.walk(
                axis,
                side * 31,
                false,
                |s, pidx| s.fluid[pidx],
                |s, pidx, other, index| {
                    let idx = index as usize + pidx * 32;
                    let voxel = s.across(axis, side, other, index);
                    let (quad, meta) = as_fluid(models, s.quad(idx), s.meta(idx));
                    let (mut model, next_meta) = Self::neighbour_model(models, voxel);
                    if model != 0 && next_meta.contains_fluid() {
                        model = models.fluid_model(model);
                    }
                    let culls = model != 0
                        && (meta.culls_same()
                            && u64::from(model) << QUAD_MODEL_SHIFT == quad & MODEL
                            || next_meta.face_occludes(axis << 1 | (1 - side) as u8));
                    let quad = (!culls).then(|| {
                        apply_light(
                            side as u64 | quad & !QUAD_LIGHT | neighbour_light(voxel),
                            meta,
                        )
                    });
                    put_or_skip(&mut s.mesher, quad, &mut s.out);
                },
            );
        }
        self.mesher.face_offset = true;
    }

    /// Both faces of every block that's neither opaque nor fluid, in the
    /// inner layers.
    fn non_opaque_inner(&mut self, axis: u8) {
        let step = Self::step(axis) * 32;
        for layer in 1..31 {
            self.walk(
                axis,
                layer,
                true,
                |s, pidx| s.non_opaque[pidx],
                |s, pidx, _, index| {
                    let idx = index as usize + pidx * 32;
                    let (quad, meta) = (s.quad(idx), s.meta(idx));
                    let below = non_opaque_face(
                        axis << 1,
                        quad,
                        meta,
                        s.quad(idx - step),
                        s.meta(idx - step),
                    );
                    let above = non_opaque_face(
                        axis << 1 | 1,
                        quad,
                        meta,
                        s.quad(idx + step),
                        s.meta(idx + step),
                    );
                    put_or_skip(&mut s.secondary, below, &mut s.out);
                    put_or_skip(&mut s.mesher, above, &mut s.out);
                },
            );
        }
    }

    /// The same on the two outer layers, the outward face against the
    /// neighbouring section.
    fn non_opaque_outer(&mut self, axis: u8, models: &mut dyn Models) {
        for side in 0..2usize {
            let step = Self::step(axis) as isize * 32 * (1 - side as isize * 2);
            self.walk(
                axis,
                side * 31,
                true,
                |s, pidx| s.non_opaque[pidx],
                |s, pidx, other, index| {
                    let idx = index as usize + pidx * 32;
                    let voxel = s.across(axis, side, other, index);
                    let (quad, meta) = (s.quad(idx), s.meta(idx));
                    // Outward, against the neighbouring section's block.
                    let (model, next_meta) = Self::neighbour_model(models, voxel);
                    let fail = model != 0
                        && (meta.culls_same()
                            && u64::from(model) << QUAD_MODEL_SHIFT == quad & MODEL
                            || next_meta.face_occludes(axis << 1 | (1 - side) as u8));
                    // Inward, against the next layer in.
                    let inner = (idx as isize + step) as usize;
                    let (inner_quad, inner_meta) = (s.quad(inner), s.meta(inner));
                    let fail_inner = inner_meta.culls_same() && inner_quad & MODEL == quad & MODEL
                        || inner_meta.face_occludes(axis << 1 | side as u8);
                    // These faces keep the block's own light, not a
                    // neighbour's.
                    let positive = meta.face_exists(axis << 1 | 1)
                        && ((side == 1 && !fail) || (side == 0 && !fail_inner));
                    let negative = meta.face_exists(axis << 1)
                        && ((side == 0 && !fail) || (side == 1 && !fail_inner));
                    put_or_skip(
                        &mut s.mesher,
                        positive.then(|| apply_light(1 | quad, meta)),
                        &mut s.out,
                    );
                    put_or_skip(
                        &mut s.secondary,
                        negative.then(|| apply_light(quad, meta)),
                        &mut s.out,
                    );
                },
            );
        }
    }

    fn finish_x(&mut self) {
        for mesher in self.x_meshers.iter_mut().chain(self.x_secondary.iter_mut()) {
            mesher.finish(&mut self.out);
        }
    }

    /// The x pass's walk: every row `y * 32 + z` once, a mesher per x layer
    /// (the secondary too when `both`); `mask` gives a row's faces, `each`
    /// meshes one. Each layer's pending skips are counted up and flushed to
    /// its meshers before a face lands in it and at the end of every row.
    fn walk_x(
        &mut self,
        both: bool,
        mask: impl Fn(&Self, usize) -> u32,
        mut each: impl FnMut(&mut Self, usize, usize),
    ) {
        for y in 0..32 {
            let mut pending = [0u32; 32];
            for z in 0..32 {
                let i = y * 32 + z;
                let mask = mask(self, i);
                add_skips(&mut pending, mask);
                for (index, _) in faces(mask) {
                    let index = index as usize;
                    let skips = std::mem::take(&mut pending[index]);
                    self.x_meshers[index].skip(skips, &mut self.out);
                    if both {
                        self.x_secondary[index].skip(skips, &mut self.out);
                    }
                    each(self, i, index);
                }
            }
            for (index, skips) in pending.into_iter().enumerate() {
                self.x_meshers[index].skip(skips, &mut self.out);
                if both {
                    self.x_secondary[index].skip(skips, &mut self.out);
                }
            }
        }
    }

    /// [`Self::opaque_inner`] along x.
    fn x_opaque_inner(&mut self) {
        self.walk_x(
            false,
            |s, i| {
                let row = s.opaque[i];
                (row ^ row >> 1) & !(1 << 31)
            },
            |s, i, index| {
                let forward = s.opaque[i] >> index & 1;
                let (own, next) = Self::pair(index + i * 32, 1, forward);
                let quad = (!s.meta(next).face_occludes(2 << 1 | (1 - forward) as u8)).then(|| {
                    apply_light(
                        u64::from(forward) | s.quad(own) & !QUAD_LIGHT | s.quad(next) & QUAD_LIGHT,
                        s.meta(own),
                    )
                });
                put_or_skip(&mut s.x_meshers[index], quad, &mut s.out);
            },
        );
    }

    /// [`Self::fluid_inner`] along x.
    fn x_fluid_inner(&mut self, models: &mut dyn Models) {
        self.walk_x(
            false,
            |s, i| {
                let fluid = s.fluid[i];
                let row = s.opaque[i] | fluid;
                (row ^ row >> 1) & !(1 << 31) & (fluid | fluid >> 1)
            },
            |s, i, index| {
                let forward = (s.opaque[i] | s.fluid[i]) >> index & 1;
                let (own, next) = Self::pair(index + i * 32, 1, forward);
                let quad = (!s.meta(next).face_occludes(2 << 1 | (1 - forward) as u8)).then(|| {
                    let (quad, meta) = as_fluid(models, s.quad(own), s.meta(own));
                    apply_light(
                        u64::from(forward) | quad & !QUAD_LIGHT | s.quad(next) & QUAD_LIGHT,
                        meta,
                    )
                });
                put_or_skip(&mut s.x_meshers[index], quad, &mut s.out);
            },
        );
    }

    /// [`Self::non_opaque_inner`] along x.
    fn x_non_opaque_inner(&mut self) {
        self.walk_x(
            true,
            |s, i| s.non_opaque[i] & !0x8000_0001,
            |s, i, index| {
                let idx = index + i * 32;
                let (quad, meta) = (s.quad(idx), s.meta(idx));
                let below = non_opaque_face(2 << 1, quad, meta, s.quad(idx - 1), s.meta(idx - 1));
                let above =
                    non_opaque_face(2 << 1 | 1, quad, meta, s.quad(idx + 1), s.meta(idx + 1));
                put_or_skip(&mut s.x_secondary[index], below, &mut s.out);
                put_or_skip(&mut s.x_meshers[index], above, &mut s.out);
            },
        );
    }

    /// The x outer passes' walk: at x 0 and 31 of every row `mask` marks,
    /// `each` gives the quads for the layer's mesher and its secondary (or
    /// none to skip), against the neighbouring section's voxel. The
    /// layers' meshers start afresh, with `face_offset` for the pass.
    fn walk_x_outer(
        &mut self,
        models: &mut dyn Models,
        face_offset: bool,
        mask: impl Fn(&Self, usize) -> u32,
        mut each: impl FnMut(&Self, &mut dyn Models, usize, usize) -> [Option<u64>; 2],
    ) {
        for layer in [0, 31] {
            self.x_meshers[layer].finish(&mut self.out);
            self.x_secondary[layer].finish(&mut self.out);
            self.x_meshers[layer].face_offset = face_offset;
        }
        for y in 0..32 {
            let mut skips = [[0u32; 2]; 2];
            for z in 0..32 {
                let i = y * 32 + z;
                let row = mask(self, i);
                for side in 0..2usize {
                    if row & 1 << (side * 31) == 0 {
                        skips[side] = skips[side].map(|n| n + 1);
                        continue;
                    }
                    let quads = each(self, models, side, i);
                    let layer = side * 31;
                    let [own_skips, secondary_skips] = &mut skips[side];
                    for (mesher, quad, skips) in [
                        (&mut self.x_meshers[layer], quads[0], own_skips),
                        (&mut self.x_secondary[layer], quads[1], secondary_skips),
                    ] {
                        match quad {
                            Some(quad) => {
                                mesher.skip(std::mem::take(skips), &mut self.out);
                                mesher.put(quad, &mut self.out);
                            }
                            None => *skips += 1,
                        }
                    }
                }
            }
            for (side, [own_skips, secondary_skips]) in skips.into_iter().enumerate() {
                let layer = side * 31;
                self.x_meshers[layer].skip(own_skips, &mut self.out);
                self.x_secondary[layer].skip(secondary_skips, &mut self.out);
            }
        }
        for layer in [0, 31] {
            self.x_meshers[layer].finish(&mut self.out);
            self.x_secondary[layer].finish(&mut self.out);
            self.x_meshers[layer].face_offset = true;
        }
    }

    /// [`Self::opaque_outer`] along x.
    fn x_opaque_outer(&mut self, models: &mut dyn Models) {
        self.walk_x_outer(
            models,
            false,
            |s, i| s.opaque[i],
            |s, models, side, i| {
                let voxel = s.neighbour(side, i);
                let (model, meta) = Self::neighbour_model(models, voxel);
                let culls = model != 0
                    && (meta.is_fully_opaque() || meta.face_occludes(2 << 1 | (1 - side) as u8));
                let idx = i * 32 + side * 31;
                let quad = (!culls).then(|| {
                    apply_light(
                        side as u64 | s.quad(idx) & !QUAD_LIGHT | neighbour_light(voxel),
                        s.meta(idx),
                    )
                });
                [quad, None]
            },
        );
    }

    /// [`Self::fluid_outer`] along x.
    fn x_fluid_outer(&mut self, models: &mut dyn Models) {
        self.walk_x_outer(
            models,
            false,
            |s, i| s.fluid[i],
            |s, models, side, i| {
                let idx = i * 32 + side * 31;
                let (quad, meta) = as_fluid(models, s.quad(idx), s.meta(idx));
                let voxel = s.neighbour(side, i);
                let (mut model, next_meta) = Self::neighbour_model(models, voxel);
                let mut culls = model != 0
                    && (next_meta.is_fully_opaque()
                        || next_meta.face_occludes(2 << 1 | (1 - side) as u8));
                if model != 0 && next_meta.contains_fluid() {
                    model = models.fluid_model(model);
                }
                culls |= model != 0
                    && meta.culls_same()
                    && u64::from(model) << QUAD_MODEL_SHIFT == quad & MODEL;
                let quad = (!culls).then(|| {
                    apply_light(
                        side as u64 | quad & !QUAD_LIGHT | neighbour_light(voxel),
                        meta,
                    )
                });
                [quad, None]
            },
        );
    }

    /// At x 0 and 31, the outward face against the neighbouring section and
    /// the inward one against the next block in. At x 0 the outward face is
    /// -x, the secondary mesher's; at 31 it's +x, the layer's own.
    fn x_non_opaque_outer(&mut self, models: &mut dyn Models) {
        self.walk_x_outer(
            models,
            true,
            |s, i| s.non_opaque[i],
            |s, models, side, i| {
                let idx = i * 32 + side * 31;
                let (quad, meta) = (s.quad(idx), s.meta(idx));
                let voxel = s.neighbour(side, i);
                let (model, next_meta) = Self::neighbour_model(models, voxel);
                let out_face = (2 << 1) ^ side as u8;
                let draw_out = if model == 0 {
                    meta.face_exists(out_face)
                } else {
                    should_mesh_non_opaque(
                        out_face,
                        quad,
                        meta,
                        u64::from(model) << QUAD_MODEL_SHIFT,
                        next_meta,
                    )
                };
                let light = if meta.face_uses_self_lighting(out_face) {
                    quad & QUAD_LIGHT
                } else {
                    neighbour_light(voxel)
                };
                let outward =
                    draw_out.then(|| apply_light(side as u64 | quad & !QUAD_LIGHT | light, meta));
                let in_idx = if side == 0 { idx + 1 } else { idx - 1 };
                let inward = non_opaque_face(
                    (2 << 1 | 1) ^ side as u8,
                    quad,
                    meta,
                    s.quad(in_idx),
                    s.meta(in_idx),
                );
                if side == 0 {
                    [inward, outward]
                } else {
                    [outward, inward]
                }
            },
        );
    }
}

/// The set bits of a row's `mask`: each face's x and the gap of nothing
/// since the one before.
fn faces(mask: u32) -> impl Iterator<Item = (i32, u32)> {
    let mut rest = mask;
    let mut last = -1;
    std::iter::from_fn(move || {
        if rest == 0 {
            return None;
        }
        let index = rest.trailing_zeros() as i32;
        rest &= rest - 1;
        let gap = (index - last - 1) as u32;
        last = index;
        Some((index, gap))
    })
}

/// One more face of nothing for every x layer `faces` has no face in.
fn add_skips(pending: &mut [u32; 32], faces: u32) {
    for (index, skips) in pending.iter_mut().enumerate() {
        *skips += u32::from(faces & 1 << index == 0);
    }
}

/// The index in neighbour `side`'s data of the voxel facing this section at
/// slice position `i`.
fn neighbour_index(side: usize, i: usize) -> usize {
    let (a, b) = (i & 31, i >> 5);
    match side {
        0 => section::index(31, b, a),
        1 => section::index(0, b, a),
        2 => section::index(a, 31, b),
        3 => section::index(a, 0, b),
        4 => section::index(a, b, 31),
        _ => section::index(a, b, 0),
    }
}

/// The voxel's biome and light moved down a bit into their quad fields, the
/// biome kept only for biome-coloured models, the light dropped for fully
/// opaque ones (they take the neighbour's); then the model and its group.
fn pack_partial(model: u16, voxel: u64, meta: Metadata) -> u64 {
    let mut light_biome = (voxel & (voxel::BIOME_MASK | voxel::LIGHT_MASK)) >> 1;
    if !meta.is_biome_coloured() {
        light_biome &= !(0x1FF << QUAD_BIOME_SHIFT);
    }
    if meta.is_fully_opaque() {
        light_biome &= !QUAD_LIGHT;
    }
    light_biome | u64::from(model) << QUAD_MODEL_SHIFT | quad_group(meta)
}

/// The group in bits 1-2: translucent 0, double-sided 1, else 2 for the
/// face's own.
fn quad_group(meta: Metadata) -> u64 {
    0b111
        & (0b000_000_010_100
            >> (u64::from(meta.is_translucent()) * 6 + u64::from(meta.is_double_sided()) * 3))
}

/// A neighbouring section's voxel's light, in a quad's field.
fn neighbour_light(voxel: u64) -> u64 {
    (voxel & voxel::LIGHT_MASK) >> 1
}

/// The block light at least the model's emission.
fn apply_light(quad: u64, meta: Metadata) -> u64 {
    let light = (quad & BLOCK_LIGHT).max(meta.light_emission() << (QUAD_LIGHT_SHIFT + 4));
    quad & !BLOCK_LIGHT | light
}

/// A block holding a fluid takes the fluid's model and group for the fluid
/// passes.
fn as_fluid(models: &mut dyn Models, quad: u64, meta: Metadata) -> (u64, Metadata) {
    if !meta.contains_fluid() {
        return (quad, meta);
    }
    let fluid = models.fluid_model(((quad & MODEL) >> QUAD_MODEL_SHIFT) as u16);
    let meta = models.metadata(fluid);
    (
        quad & !MODEL & !0b110 | u64::from(fluid) << QUAD_MODEL_SHIFT | quad_group(meta),
        meta,
    )
}

/// Whether `face` of a block that's neither opaque nor fluid is drawn against
/// `next`: not against its own kind when the model culls same, not when the
/// model has no such face, and not when the face can be occluded and `next`
/// occludes it.
fn should_mesh_non_opaque(
    face: u8,
    quad: u64,
    meta: Metadata,
    next_quad: u64,
    next_meta: Metadata,
) -> bool {
    if (quad ^ next_quad) & MODEL == 0 && meta.culls_same() {
        return false;
    }
    if !meta.face_exists(face) {
        return false;
    }
    !(meta.face_can_be_occluded(face) && next_meta.face_occludes(face ^ 1))
}

/// The quad to draw for `face` against `next`, if any.
fn non_opaque_face(
    face: u8,
    quad: u64,
    meta: Metadata,
    next_quad: u64,
    next_meta: Metadata,
) -> Option<u64> {
    should_mesh_non_opaque(face, quad, meta, next_quad, next_meta).then(|| {
        let light = if meta.face_uses_self_lighting(face) {
            quad
        } else {
            next_quad
        } & QUAD_LIGHT;
        apply_light(u64::from(face & 1) | quad & !QUAD_LIGHT | light, meta)
    })
}

fn put_or_skip(mesher: &mut Mesher, quad: Option<u64>, out: &mut Quads) {
    match quad {
        Some(quad) => mesher.put(quad, out),
        None => mesher.skip(1, out),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Models: 1 stone (opaque), 2 glass (sees through, culls its own
    /// kind), 3 water (translucent fluid), 4 a waterlogged slab-like block
    /// (holds water, open on top), 5 a snow layer (a full occluder below,
    /// thin faces that can be occluded elsewhere).
    struct Stub;

    const OPAQUE_FACE: u64 = 0b0111;
    const CLEAR_FACE: u64 = 0b1110;

    fn faces(byte: u64) -> u64 {
        (0..6).fold(0, |m, f| m | byte << (8 * f))
    }

    impl Models for Stub {
        fn model(&mut self, block: u32) -> u16 {
            block as u16
        }

        fn metadata(&self, model: u16) -> Metadata {
            Metadata(match model {
                1 => faces(OPAQUE_FACE) | 1 << 54,
                2 => faces(CLEAR_FACE) | 32 << 48,
                3 => faces(CLEAR_FACE) | (2 | 16 | 32) << 48,
                4 => faces(OPAQUE_FACE) & !(0xFF << 8) | 0xFF << 8 | 8 << 48,
                5 => faces(0b0100) & !0xFF | OPAQUE_FACE,
                _ => 0,
            })
        }

        fn fluid_model(&self, model: u16) -> u16 {
            if model == 4 { 3 } else { model }
        }
    }

    fn key() -> SectionKey {
        SectionKey {
            level: 0,
            x: 0,
            y: 0,
            z: 0,
        }
    }

    fn build(section: &WorldSection) -> BuiltSection {
        Factory::new().build(key(), section, |_| None, &mut Stub)
    }

    fn with(blocks: &[((usize, usize, usize), u32)]) -> WorldSection {
        let mut section = WorldSection::unseen();
        for &((x, y, z), block) in blocks {
            section.data[section::index(x, y, z)] = voxel::compose(0x0F, block, 0);
        }
        section
    }

    fn face(quad: u64) -> u8 {
        (quad & 7) as u8
    }

    fn position(quad: u64) -> (u32, u32, u32) {
        (
            (quad >> 21 & 31) as u32,
            (quad >> 16 & 31) as u32,
            (quad >> 11 & 31) as u32,
        )
    }

    fn model(quad: u64) -> u16 {
        ((quad & MODEL) >> QUAD_MODEL_SHIFT) as u16
    }

    #[test]
    fn a_snow_layer_on_a_block_draws_its_top() {
        let built = build(&with(&[((5, 6, 7), 1), ((5, 7, 7), 5)]));
        let described: Vec<(u8, (u32, u32, u32), u16)> = built
            .quads
            .iter()
            .map(|&q| (face(q), position(q), model(q)))
            .collect();
        assert!(
            described.contains(&(1, (5, 7, 7), 5)),
            "no snow top in {described:?}"
        );
        assert!(!described.contains(&(1, (5, 6, 7), 1)), "{described:?}");
    }

    #[test]
    fn a_lone_cube_draws_its_six_faces_where_it_stands() {
        let built = build(&with(&[((5, 6, 7), 1)]));
        assert_eq!(built.quads.len(), 6);
        let mut seen: Vec<u8> = built.quads.iter().map(|&q| face(q)).collect();
        seen.sort_unstable();
        assert_eq!(seen, [0, 1, 2, 3, 4, 5]);
        for &quad in &built.quads {
            assert_eq!(position(quad), (5, 6, 7));
            assert_eq!(model(quad), 1);
            assert_eq!(quad >> 3 & 0xFF, 0, "one block wide and high");
            assert_eq!(
                quad >> QUAD_LIGHT_SHIFT & 0xFF,
                0x0F,
                "lit by the air around"
            );
        }
        // Each face in its own group, after the empty translucent and
        // double-sided ones.
        assert_eq!(built.offsets, [0, 0, 0, 1, 2, 3, 4, 5]);
        assert_eq!(built.aabb & 0x7FFF, 5 | 6 << 5 | 7 << 10);
    }

    #[test]
    fn touching_cubes_hide_their_shared_faces_and_merge() {
        let pair = build(&with(&[((5, 6, 7), 1), ((6, 6, 7), 1)]));
        // Their faces merge into one quad per side.
        assert_eq!(pair.quads.len(), 6);
        let tops: Vec<u64> = pair
            .quads
            .iter()
            .copied()
            .filter(|&q| face(q) == 1)
            .collect();
        assert_eq!(tops.len(), 1);
        assert_eq!(tops[0] >> 3 & 0xF, 1, "two wide");
        // A full section shows only its outside, each side in 16x16 quads.
        let mut full = WorldSection::unseen();
        full.data.fill(voxel::compose(0, 1, 0));
        assert_eq!(build(&full).quads.len(), 6 * 4);
    }

    #[test]
    fn glass_culls_against_glass_but_not_stone() {
        let glass = build(&with(&[((5, 6, 7), 2), ((6, 6, 7), 2)]));
        assert_eq!(glass.quads.len(), 6);
        // Beside stone, glass keeps its face (stone's hides behind glass).
        let mixed = build(&with(&[((5, 6, 7), 2), ((6, 6, 7), 1)]));
        assert_eq!(mixed.quads.iter().filter(|&&q| model(q) == 2).count(), 5);
        assert_eq!(mixed.quads.iter().filter(|&&q| model(q) == 1).count(), 6);
    }

    #[test]
    fn a_waterlogged_block_draws_its_water_around_it() {
        let built = build(&with(&[((5, 6, 7), 4)]));
        // The water fills the block, so it shows on every open side; the
        // block itself has no top face.
        let water: Vec<u64> = built
            .quads
            .iter()
            .copied()
            .filter(|&q| model(q) == 3)
            .collect();
        assert_eq!(water.len(), 6);
        assert_eq!(
            built.offsets[1], 6,
            "the water quads are the translucent group"
        );
        let block: Vec<u64> = built
            .quads
            .iter()
            .copied()
            .filter(|&q| model(q) == 4)
            .collect();
        assert_eq!(block.len(), 5);
        assert!(block.iter().all(|&q| face(q) != 1));
    }

    #[test]
    fn section_borders_face_the_neighbouring_section() {
        let mut stone = WorldSection::unseen();
        stone.data.fill(voxel::compose(0, 1, 0));
        let edge = with(&[((0, 6, 7), 1)]);
        // A stone neighbour on -x hides the face against it.
        let hidden =
            Factory::new().build(key(), &edge, |k| (k.x == -1).then_some(&stone), &mut Stub);
        assert_eq!(hidden.quads.len(), 5);
        assert!(hidden.quads.iter().all(|&q| face(q) != 4));
        // Nothing there: drawn, lit by the unseen sky.
        let open = build(&edge);
        let west = open.quads.iter().find(|&&q| face(q) == 4).unwrap();
        assert_eq!(west >> QUAD_LIGHT_SHIFT & 0xFF, 0x0F);
    }

    #[test]
    fn empty_sections_build_nothing() {
        let mut empty = WorldSection::unseen();
        empty.non_empty_children = 0b101;
        let built = build(&empty);
        assert!(built.quads.is_empty());
        assert_eq!(built.child_existence, 0b101);
    }
}
