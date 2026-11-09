//! Distant terrain's model texture bakery: a block state's model drawn from
//! each of its six sides into a 16x16 face texture on the CPU, colour, depth
//! and which texels are tinted.

use azalea_block::BlockState;
use glam::{Mat4, Quat, Vec3, Vec4};

use crate::renderer::chunk::atlas::{AtlasUVMap, SpritePixels};
use crate::world::block::FluidKind;
use crate::world::block::model::{BakedQuad, Direction, face_positions, face_uvs};
use crate::world::block::render_assets::{BlockRenderView, Tint};

/// Each face texture's side, in texels.
pub(crate) const MODEL_TEXTURE_SIZE: usize = 16;
pub(crate) const TEXELS: usize = MODEL_TEXTURE_SIZE * MODEL_TEXTURE_SIZE;

const INTEGER_BITS: u32 = 13;
const TOTAL_INTEGER_BITS: u32 = INTEGER_BITS + 1;
const FIXED_POINT_BITS: u32 = 32 - TOTAL_INTEGER_BITS;
const FIXED_POINT_BIT_SCALE: i64 = (1 << FIXED_POINT_BITS) - 1;

const DEPTH_MASK: u64 = ((1 << 24) - 1) << (64 - 24);
/// Depth at its farthest, the rest zero.
const CLEAR_VALUE: u64 = DEPTH_MASK;

/// Quad meta: texels with no alpha are left out.
const META_DISCARD: u32 = 1;
/// Quad meta: the texels take the block's tint.
const META_TINTED: u32 = 4;

/// One face's bake: per texel, bottom row first, its ABGR colour and its
/// depth word (24-bit depth high, bit 7 tinted, the low bits how many quads
/// covered it).
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct FaceTexture {
    pub colour: Box<[u32; TEXELS]>,
    pub depth: Box<[u32; TEXELS]>,
}

/// A model quad as the rasterizer takes it: block-local corners, sprite
/// UVs and the sprite they sample.
struct Quad<'a> {
    positions: [Vec3; 4],
    uvs: [[f32; 2]; 4],
    meta: u32,
    sprite: &'a SpritePixels,
}

/// A software rasterizer: a 16x16 target of 64-bit texels, colour in the
/// low 32 bits, a stencil count, the tint bit and a 24-bit depth in the
/// high.
struct Rasterizer {
    framebuffer: [u64; TEXELS],
    cull_back_face: bool,
    blending: bool,
}

/// A transformed corner: screen position (fixed point) and its attributes.
#[derive(Clone, Copy, Default)]
struct Corner {
    x: i32,
    y: i32,
    z: i32,
    meta: u32,
    u: f32,
    v: f32,
}

impl Rasterizer {
    fn new() -> Self {
        Self {
            framebuffer: [CLEAR_VALUE; TEXELS],
            cull_back_face: false,
            blending: false,
        }
    }

    fn clear(&mut self) {
        self.framebuffer.fill(CLEAR_VALUE);
    }

    fn raster(&mut self, mvp: &Mat4, quads: &[Quad]) {
        for quad in quads {
            self.raster_quad(mvp, quad);
        }
    }

    fn raster_quad(&mut self, transform: &Mat4, quad: &Quad) {
        let corners: [Corner; 4] = std::array::from_fn(|i| load_transform_pos(transform, quad, i));
        // 0, 1, 2 | 2, 3, 0
        self.raster_triangle(quad.sprite, [corners[0], corners[1], corners[2]], false);
        self.raster_triangle(quad.sprite, [corners[2], corners[3], corners[0]], true);
    }

    fn raster_triangle(&mut self, sprite: &SpritePixels, [v1, v2, v3]: [Corner; 3], or_zero: bool) {
        let area = edge(&v1, &v2, v3.x, v3.y);
        // A negative area winds counterclockwise.
        if (area < 0) == self.cull_back_face {
            return;
        }
        if from_fixed(area).abs() < 0.001 {
            return;
        }
        let last = to_fixed_int(MODEL_TEXTURE_SIZE as i32 - 1);
        let min_x = from_fixed_to_int(v1.x.min(v2.x).min(v3.x).max(0));
        let max_x = from_fixed_to_int(v1.x.max(v2.x).max(v3.x).min(last));
        let min_y = from_fixed_to_int(v1.y.min(v2.y).min(v3.y).max(0));
        let max_y = from_fixed_to_int(v1.y.max(v2.y).max(v3.y).min(last));
        for py in min_y..=max_y {
            for px in min_x..=max_x {
                let cx = to_fixed_int(px) + to_fixed(0.5);
                let cy = to_fixed_int(py) + to_fixed(0.5);
                let w1 = fixed_div(edge(&v2, &v3, cx, cy), area);
                let w2 = fixed_div(edge(&v3, &v1, cx, cy), area);
                let w3 = to_fixed(1.0) - w1 - w2;
                let inside = w1 > 0 && w2 > 0 && w3 > 0;
                if inside || (or_zero && w1 >= 0 && w2 >= 0 && w3 >= 0) {
                    let (b1, b2, b3) = (from_fixed(w1), from_fixed(w2), from_fixed(w3));
                    let z = b1.mul_add(
                        from_fixed(v1.z),
                        b2.mul_add(from_fixed(v2.z), b3 * from_fixed(v3.z)),
                    );
                    let index = (px + py * MODEL_TEXTURE_SIZE as i32) as usize;
                    self.raster_pixel(sprite, index, [b1, b2, b3], [v1, v2, v3], z);
                }
            }
        }
    }

    fn raster_pixel(
        &mut self,
        sprite: &SpritePixels,
        index: usize,
        [b1, b2, b3]: [f32; 3],
        [a1, a2, a3]: [Corner; 3],
        z: f32,
    ) {
        let mut z = z.mul_add(0.5, 0.5);
        if (-0.000_001..0.0).contains(&z) {
            z = 0.0;
        }
        if !(0.0..=1.0).contains(&z) {
            return;
        }
        let meta = a1.meta;
        let u = b1.mul_add(a1.u, b2.mul_add(a2.u, b3 * a3.u));
        let v = b1.mul_add(a1.v, b2.mul_add(a2.v, b3 * a3.v));
        let mut colour = sample(sprite, u, v);
        if meta & META_DISCARD != 0 && colour >> 24 == 0 {
            return;
        }
        let texel = &mut self.framebuffer[index];
        // The stencil counts first.
        *texel = texel.wrapping_add(1 << 32);
        let mut depth = ((f64::from(z) * f64::from((1 << 24) - 1)) as u64) << (64 - 24);
        if depth == DEPTH_MASK {
            // Something shows even at the far plane.
            depth -= 1;
        }
        // Strictly less than.
        if *texel <= depth {
            return;
        }
        *texel = (*texel & !DEPTH_MASK) | depth;
        *texel = (*texel & !(1 << 39)) | u64::from(meta & META_TINTED) << 37;
        let below = *texel as u32;
        *texel &= !u64::from(u32::MAX);
        if self.blending {
            colour = blend(below, colour);
        }
        *texel |= u64::from(colour);
    }
}

/// The nearest texel of `sprite` at `u`, `v`, as ABGR.
fn sample(sprite: &SpritePixels, u: f32, v: f32) -> u32 {
    let (w, h) = (sprite.width as i32, sprite.height as i32);
    // Java's `Math.round(u * w - 0.5)`.
    let pu = ((u * w as f32).floor() as i32).clamp(0, w - 1);
    let pv = ((v * h as f32).floor() as i32).clamp(0, h - 1);
    let at = (pv * w + pu) as usize * 4;
    u32::from_le_bytes(sprite.rgba[at..at + 4].try_into().expect("four bytes"))
}

/// `top` blended over `below`, both ABGR.
fn blend(below: u32, top: u32) -> u32 {
    let below_alpha = below >> 24;
    if below_alpha == 0 {
        return top;
    }
    let top_alpha = top >> 24;
    let alpha = (below_alpha + ((top_alpha * (255 - below_alpha)) >> 8)).min(0xFF);
    mix(top & 0xFF_FFFF, below & 0xFF_FFFF, top_alpha) | alpha << 24
}

/// Sodium's `ColorMixer.mix`: `start * weight + end * (255 - weight)` per
/// channel, rounded.
fn mix(start: u32, end: u32, weight: u32) -> u32 {
    (0..4).fold(0, |out, i| {
        let (a, b) = ((start >> (i * 8)) & 0xFF, (end >> (i * 8)) & 0xFF);
        out | ((a * weight + b * (255 - weight) + 127) / 255) << (i * 8)
    })
}

fn load_transform_pos(transform: &Mat4, quad: &Quad, vert: usize) -> Corner {
    let pos = *transform * quad.positions[vert].extend(1.0);
    assert!((pos.w - 1.0).abs() <= 0.000_001, "the views are affine");
    let size = MODEL_TEXTURE_SIZE as f32;
    let [u, v] = quad.uvs[vert];
    Corner {
        x: to_fixed(pos.x.mul_add(0.5, 0.5) * size),
        y: to_fixed(pos.y.mul_add(0.5, 0.5) * size),
        z: to_fixed(pos.z),
        meta: quad.meta,
        u,
        v,
    }
}

fn edge(a: &Corner, b: &Corner, cx: i32, cy: i32) -> i32 {
    fixed_mul(cx - a.x, b.y - a.y) - fixed_mul(cy - a.y, b.x - a.x)
}

fn to_fixed(a: f32) -> i32 {
    (f64::from(a) * FIXED_POINT_BIT_SCALE as f64) as i32
}

fn to_fixed_int(a: i32) -> i32 {
    (i64::from(a) * FIXED_POINT_BIT_SCALE) as i32
}

fn from_fixed(a: i32) -> f32 {
    (f64::from(a) / FIXED_POINT_BIT_SCALE as f64) as f32
}

fn from_fixed_to_int(a: i32) -> i32 {
    (i64::from(a) / FIXED_POINT_BIT_SCALE) as i32
}

fn fixed_mul(a: i32, b: i32) -> i32 {
    (i64::from(a) * i64::from(b) / FIXED_POINT_BIT_SCALE) as i32
}

fn fixed_div(a: i32, b: i32) -> i32 {
    (i64::from(a) * FIXED_POINT_BIT_SCALE / i64::from(b)) as i32
}

/// The six views, by face index ([`Direction::VALUES`]): each looks at the
/// block from that side and maps it to clip space, the face's texels at `x`
/// and `y`.
fn views() -> [Mat4; 6] {
    [
        view(-90.0, 0.0, 0.0, 0),
        view(90.0, 0.0, 0.0, 0b100),
        view(0.0, 180.0, 0.0, 0b001),
        view(0.0, 0.0, 0.0, 0),
        view(0.0, 90.0, 270.0, 0b100),
        view(0.0, 270.0, 270.0, 0),
    ]
}

/// One view: the pose (about the block's centre, flipped per `flip`), then
/// the orthographic projection of the unit cube.
fn view(pitch: f32, yaw: f32, rotation: f32, flip: i32) -> Mat4 {
    let scale = Vec3::new(
        (1 - 2 * (flip & 1)) as f32,
        (1 - (flip & 2)) as f32,
        (1 - ((flip >> 1) & 2)) as f32,
    );
    let pose = Mat4::from_translation(Vec3::splat(0.5))
        * Mat4::from_quat(quat(Vec3::Z, rotation))
        * Mat4::from_quat(quat(Vec3::X, pitch))
        * Mat4::from_quat(quat(Vec3::Y, yaw))
        * Mat4::from_scale(scale)
        * Mat4::from_translation(Vec3::splat(-0.5));
    let ortho = Mat4::from_cols(
        Vec4::new(2.0, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 2.0, 0.0, 0.0),
        Vec4::new(0.0, 0.0, -2.0, 0.0),
        Vec4::new(-1.0, -1.0, 1.0, 1.0),
    );
    ortho * pose
}

/// A rotation of `angle` degrees about `axis`, its sine and cosine taken in
/// `f64`.
fn quat(axis: Vec3, angle: f32) -> Quat {
    let half = angle.to_radians() / 2.0;
    let sin = f64::from(half).sin() as f32;
    let axis = axis / axis.length();
    Quat::from_xyzw(
        axis.x * sin,
        axis.y * sin,
        axis.z * sin,
        f64::from(half).cos() as f32,
    )
}

/// What a bake found across the model's quads.
#[derive(Clone, Copy)]
pub(crate) struct BakeFlags {
    /// Some quad's sprite mips as `dark_cutout`.
    pub darkened: bool,
    /// Some quad is in the translucent layer.
    pub translucent: bool,
    /// Some opaque-layer quad leaves texels out.
    pub discard: bool,
    /// Some quad takes its face's directional shade, so the model shades
    /// per face. Fluids do too, as vanilla's `FluidRenderer` shades them,
    /// where the reference leaves them unshaded.
    pub shaded: bool,
    /// The first tinted quad's tint.
    pub tint: Tint,
}

/// Whether `quad` takes directional shade.
fn shades(quad: &BakedQuad) -> bool {
    quad.shade_face.is_some()
}

/// The rasterizer and the six views every block state is drawn through.
pub(crate) struct Bakery {
    rasterizer: Rasterizer,
    views: [Mat4; 6],
}

impl Bakery {
    pub fn new() -> Self {
        Self {
            rasterizer: Rasterizer::new(),
            views: views(),
        }
    }

    /// `state`'s six faces, by face index, or `None` when it draws nothing.
    pub fn render<'a>(
        &mut self,
        view: BlockRenderView<'a>,
        uv_map: &'a AtlasUVMap,
        state: BlockState,
    ) -> Option<([FaceTexture; 6], BakeFlags)> {
        let data = view.block_state_table().block_state_data(state);
        // Fluids draw through vanilla's `FluidRenderer`, their block being
        // invisible.
        if data.is_invisible() && !data.is_fluid_block() {
            return None;
        }
        // Leaves go in the solid layer, their clear texels kept.
        let leaves = data.is_leaves();
        let mut flags = BakeFlags {
            darkened: false,
            translucent: false,
            discard: false,
            shaded: false,
            tint: Tint::None,
        };
        let (mut opaque, mut translucent): (Vec<Quad<'a>>, Vec<Quad<'a>>) =
            (Vec::new(), Vec::new());
        let mut add = |quad: Quad<'a>, layer_translucent: bool, opaque_texels: bool| {
            flags.darkened |= quad.sprite.dark_cutout;
            if layer_translucent {
                flags.translucent = true;
                translucent.push(Quad {
                    meta: quad.meta | META_DISCARD,
                    ..quad
                });
            } else {
                let discard = !leaves && !opaque_texels;
                flags.discard |= discard;
                opaque.push(Quad {
                    meta: quad.meta | if discard { META_DISCARD } else { 0 },
                    ..quad
                });
            }
        };
        if data.is_fluid_block() {
            let fluid = data.fluid().kind;
            flags.shaded = true;
            if fluid == FluidKind::Water {
                flags.tint = Tint::Water;
            }
            for quad in fluid_quads(uv_map, fluid) {
                add(quad, fluid == FluidKind::Water, true);
            }
        } else {
            let quads: Vec<&BakedQuad> = match view.get_baked_model(state, None) {
                Some(model) => model.quads.iter().collect(),
                None => view.get_multipart_quads(state, None).unwrap_or_default(),
            };
            for quad in quads {
                let name = quad.texture.name();
                let Some(sprite) = uv_map.sprite_pixels(name) else {
                    continue;
                };
                flags.shaded |= shades(quad);
                if flags.tint == Tint::None {
                    flags.tint = quad.tint;
                }
                let region = uv_map.region(quad.texture);
                let meta = if quad.tint == Tint::None {
                    0
                } else {
                    META_TINTED
                };
                add(
                    Quad {
                        positions: quad.positions.map(Vec3::from),
                        uvs: quad.uvs,
                        meta,
                        sprite,
                    },
                    region.translucent,
                    region.opaque,
                );
            }
        }
        if opaque.is_empty() && translucent.is_empty() {
            return None;
        }
        let faces = std::array::from_fn(|i| {
            let r = &mut self.rasterizer;
            r.cull_back_face = i == 1 || i == 2 || i == 4;
            r.clear();
            r.blending = false;
            r.raster(&self.views[i], &opaque);
            r.blending = true;
            r.raster(&self.views[i], &translucent);
            FaceTexture {
                colour: Box::new(r.framebuffer.map(|texel| texel as u32)),
                depth: Box::new(r.framebuffer.map(|texel| (texel >> 32) as u32)),
            }
        });
        Some((faces, flags))
    }
}

/// A full block of `fluid`: still on top and bottom, flowing on the sides
/// over vanilla's half of the sprite, as vanilla's `FluidRenderer` draws a
/// source with nothing around it. Departure: the top stays at the block's
/// top where `FluidRenderer` lowers it, so a run of water voxels meets
/// without a step; the surface's own height comes from the voxel's gap
/// (`ModelFactory::fluid_surface`).
fn fluid_quads(uv_map: &AtlasUVMap, fluid: FluidKind) -> Vec<Quad<'_>> {
    let (still, flow, meta) = match fluid {
        FluidKind::Water => ("water_still", "water_flow", META_TINTED),
        _ => ("lava_still", "lava_flow", 0),
    };
    let (from, to) = ([0.0; 3], [1.0; 3]);
    Direction::VALUES
        .into_iter()
        .filter_map(|dir| {
            let (name, uv) = match dir {
                Direction::Up | Direction::Down => (still, [0.0, 0.0, 16.0, 16.0]),
                _ => (flow, [0.0, 0.0, 8.0, 8.0]),
            };
            Some(Quad {
                positions: face_positions(dir, from, to).map(Vec3::from),
                uvs: face_uvs(dir, from, to, Some(&uv), None),
                meta,
                sprite: uv_map.sprite_pixels(name)?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn sprite(rgba: [u8; 4]) -> SpritePixels {
        SpritePixels {
            width: 1,
            height: 1,
            rgba: Arc::from(rgba.as_slice()),
            dark_cutout: false,
        }
    }

    /// The unit cube's `dir` face.
    fn face(dir: Direction, sprite: &SpritePixels) -> Quad<'_> {
        let (from, to) = ([0.0; 3], [1.0; 3]);
        Quad {
            positions: face_positions(dir, from, to).map(Vec3::from),
            uvs: face_uvs(dir, from, to, None, None),
            meta: 0,
            sprite,
        }
    }

    fn bake(quads: &[Quad], view: usize) -> [u64; TEXELS] {
        let mut r = Rasterizer::new();
        r.cull_back_face = view == 1 || view == 2 || view == 4;
        r.raster(&views()[view], quads);
        r.framebuffer
    }

    #[test]
    fn each_view_sees_its_face_of_a_cube_and_nothing_else() {
        let colours = [[1, 0, 0, 255], [2, 0, 0, 255], [3, 0, 0, 255]];
        let sprites = colours.map(sprite);
        let dirs = Direction::VALUES;
        for (view, &dir) in dirs.iter().enumerate() {
            let quads: Vec<Quad> = dirs
                .iter()
                .map(|&other| face(other, &sprites[usize::from(other == dir)]))
                .collect();
            let texels = bake(&quads, view);
            assert!(
                texels
                    .iter()
                    .all(|&t| t as u32 == u32::from_le_bytes([2, 0, 0, 255])),
                "view {view} shows its own face over the whole texture"
            );
        }
    }

    #[test]
    fn the_nearer_quad_wins_and_blending_puts_the_top_over() {
        let (red, blue) = (sprite([255, 0, 0, 255]), sprite([0, 0, 255, 128]));
        let mut back = face(Direction::North, &red);
        let mut front = face(Direction::North, &blue);
        // The north view looks at z = 0 from outside: nearer is lower z.
        for p in &mut back.positions {
            p.z = 0.5;
        }
        for p in &mut front.positions {
            p.z = 0.25;
        }
        let texels = bake(&[back, front], 2);
        assert_eq!(texels[0] as u32, u32::from_le_bytes([0, 0, 255, 128]));
        assert_eq!(blend(0xFF00_00FF, 0x8000_00FF) >> 24, 255);
        assert_eq!(blend(0, 0x8012_3456), 0x8012_3456);
    }

    /// Per view, the block axes a face texture's `x` and `y` run along:
    /// every view maps them one to one, unflipped. `distant.vert` mirrors
    /// this.
    const TILE_AXES: [[usize; 2]; 6] = [[0, 2], [0, 2], [0, 1], [0, 1], [1, 2], [1, 2]];

    #[test]
    fn tile_axes_follow_the_views() {
        for (view, axes) in views().iter().zip(TILE_AXES) {
            let rows = view.transpose();
            for (row, axis) in [rows.x_axis, rows.y_axis].into_iter().zip(axes) {
                // Clip space to texels, in blocks.
                let texel = row * 0.5 + Vec4::W * 0.5;
                assert!(texel.abs_diff_eq(Vec3::AXES[axis].extend(0.0), 1e-6));
            }
        }
    }

    #[test]
    fn shade_follows_the_element_not_ambient_occlusion() {
        let quad = |shade_face, ambient_occlusion| BakedQuad {
            positions: face_positions(Direction::Up, [0.0; 3], [1.0; 3]),
            uvs: [[0.0; 2]; 4],
            texture: crate::world::block::model::TextureId::intern("block/stone"),
            cullface: None,
            tint: Tint::None,
            tint_index: None,
            shade_light: 1.0,
            shade_face,
            emission: 0,
            ambient_occlusion,
        };
        assert!(shades(&quad(Some(Direction::Up), false)));
        assert!(!shades(&quad(None, true)));
    }

    #[test]
    fn discarded_texels_leave_nothing() {
        let clear = sprite([9, 9, 9, 0]);
        let quad = Quad {
            meta: META_DISCARD,
            ..face(Direction::Up, &clear)
        };
        assert!(bake(&[quad], 1).iter().all(|&t| t == CLEAR_VALUE));
    }
}
