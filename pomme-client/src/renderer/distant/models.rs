//! Voxy's `ModelFactory`, `ModelQueries`, `TextureUtils` and `MipGen`:
//! block states baked into models, each six face textures with mips in a
//! tile of the model atlas, plus what the mesher and shaders need to know of
//! each face.

use std::collections::HashMap;

use azalea_block::BlockState;

use super::bakery::{Bakery, FaceTexture, MODEL_TEXTURE_SIZE, TEXELS};
use crate::renderer::chunk::atlas::AtlasUVMap;
use crate::renderer::chunk::mesher::int_to_rgb;
use crate::world::block::FluidKind;
use crate::world::block::render_assets::{BlockRenderView, Tint};

const SIZE: usize = MODEL_TEXTURE_SIZE;
/// Mip levels of a face texture: 16, 8, 4 and 2 texels.
pub(crate) const MODEL_MIP_LEVELS: u32 = SIZE.trailing_zeros();
/// A model's tile: its six faces two rows of three, Voxy face index `i` at
/// column `i / 2`, row `i % 2`.
pub(crate) const TILE_WIDTH: u32 = SIZE as u32 * 3;
pub(crate) const TILE_HEIGHT: u32 = SIZE as u32 * 2;
/// Tiles a row of the model atlas holds, and rows.
pub(crate) const ATLAS_COLUMNS: u32 = 64;
pub(crate) const ATLAS_ROWS: u32 = 32;
/// Models the atlas holds; the first is air, drawn as nothing.
pub(crate) const CAPACITY: usize = (ATLAS_COLUMNS * ATLAS_ROWS) as usize;

/// Which layer a model draws in (vanilla `ChunkSectionLayer`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Layer {
    Solid,
    Cutout,
    Translucent,
}

/// What the mesher asks of each model (`ModelQueries`): a byte per face,
/// bit 0 the face occludes what it touches, 1 it covers its whole side, 2 it
/// can be occluded, 3 it's lit by its own block, 0xFF for no face; in byte 6
/// whether the model is biome-coloured, translucent, double-sided, holds a
/// fluid, is a fluid, culls its own kind and is fully opaque; the light it
/// gives off in bits 55-58.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Metadata(pub u64);

impl Metadata {
    fn face(self, face: u8) -> u64 {
        self.0 >> (8 * face) & 0xFF
    }

    pub fn face_exists(self, face: u8) -> bool {
        self.face(face) != 0xFF
    }

    pub fn face_occludes(self, face: u8) -> bool {
        self.face_exists(face) && self.face(face) & 1 != 0
    }

    pub fn face_can_be_occluded(self, face: u8) -> bool {
        self.face(face) & 4 != 0
    }

    pub fn face_uses_self_lighting(self, face: u8) -> bool {
        self.face(face) & 8 != 0
    }

    fn flag(self, bit: u32) -> bool {
        self.0 >> 48 & 1 << bit != 0
    }

    pub fn is_biome_coloured(self) -> bool {
        self.flag(0)
    }

    pub fn is_translucent(self) -> bool {
        self.flag(1)
    }

    pub fn is_double_sided(self) -> bool {
        self.flag(2)
    }

    pub fn contains_fluid(self) -> bool {
        self.flag(3)
    }

    pub fn is_fluid(self) -> bool {
        self.flag(4)
    }

    pub fn culls_same(self) -> bool {
        self.flag(5)
    }

    pub fn is_fully_opaque(self) -> bool {
        self.flag(6)
    }

    pub fn light_emission(self) -> u64 {
        self.0 >> 55 & 0xF
    }
}

/// The models the mesher reads, baked as it asks.
pub(crate) trait Models {
    /// Block state `block`'s model, 0 for none.
    fn model(&mut self, block: u32) -> u16;
    fn metadata(&self, model: u16) -> Metadata;
    /// The model of the fluid a model's block holds, for a fluid its own.
    fn fluid_model(&self, model: u16) -> u16;
}

/// What tints a model's texture: a fixed colour, or the biome's (in the
/// order of [`super::colors::TINT_KINDS`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ModelTint {
    None,
    Constant([u8; 3]),
    Grass,
    Foliage,
    DryFoliage,
    Water,
}

impl ModelTint {
    fn biome_dependent(self) -> bool {
        !matches!(self, Self::None | Self::Constant(_))
    }

    /// [`BlockModel::tint`]: the colour, or the kind to look the biome's up
    /// by.
    fn packed(self) -> u32 {
        match self {
            Self::None => u32::MAX,
            Self::Constant([r, g, b]) => u32::from_le_bytes([r, g, b, 255]),
            Self::Grass => 0,
            Self::Foliage => 1,
            Self::DryFoliage => 2,
            Self::Water => 3,
        }
    }
}

/// A tint colour as the shaders unpack it.
pub(crate) fn pack_tint(rgb: [f32; 3]) -> u32 {
    let [r, g, b] = rgb.map(|c| (c * 255.0).round().clamp(0.0, 255.0) as u8);
    u32::from_le_bytes([r, g, b, 255])
}

/// A model as the shaders read it (`block_model.glsl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct BlockModel {
    /// Per face: the drawn texels' min x, max x, min y and max y in 4 bits
    /// each, how far in from its side the face sits in 64ths from bit 16,
    /// whether it cuts clear texels out (bit 22) and does so once a quad
    /// spans more than one block (23), and its tint state in bits 24-25: 1
    /// on some texels, 2 on all.
    pub face_data: [u32; 6],
    /// Tinted (1), by the biome (2), translucent (4), shaded per face as
    /// vanilla does blocks with ambient occlusion (8).
    pub flags: u32,
    /// [`ModelTint::packed`].
    pub tint: u32,
}

/// What a model is deduplicated by (`ModelEntry`): states that look alike
/// share one.
#[derive(PartialEq, Eq, Hash)]
struct ModelEntry {
    faces: Box<[FaceTexture; 6]>,
    /// The fluid a non-fluid block holds, by model.
    fluid: Option<u16>,
    tint: ModelTint,
}

/// A state's model is still to bake.
const UNBAKED: u16 = u16::MAX;

/// What the mesher needs of a model.
#[derive(Clone, Copy)]
pub(crate) struct ModelInfo {
    pub metadata: Metadata,
    /// [`Models::fluid_model`].
    pub fluid: u16,
}

/// Model 0's place: air.
const AIR: ModelInfo = ModelInfo {
    metadata: Metadata(0),
    fluid: 0,
};

/// A new model: its record and its tile with mips, to upload.
pub(crate) struct ModelTile {
    pub id: u16,
    pub model: BlockModel,
    /// RGBA per level, the level-0 tile first, row by row.
    pub levels: Vec<Vec<u8>>,
}

pub(crate) struct ModelFactory {
    bakery: Bakery,
    /// Each state's model by state id, once baked.
    id_mappings: Vec<u16>,
    /// Models by what they're made of, so states that look alike share one.
    entries: HashMap<ModelEntry, u16>,
    models: Vec<ModelInfo>,
    /// Tiles baked since the last [`Self::take_tiles`].
    tiles: Vec<ModelTile>,
}

impl ModelFactory {
    pub fn new() -> Self {
        Self {
            bakery: Bakery::new(),
            id_mappings: Vec::new(),
            entries: HashMap::new(),
            models: vec![AIR],
            tiles: Vec::new(),
        }
    }

    pub fn info(&self, model: u16) -> ModelInfo {
        self.models[usize::from(model)]
    }

    pub fn take_tiles(&mut self) -> Vec<ModelTile> {
        std::mem::take(&mut self.tiles)
    }

    /// `state`'s model, baked the first time; 0, air, when it draws nothing
    /// or the atlas is full.
    pub fn model(
        &mut self,
        view: BlockRenderView<'_>,
        uv_map: &AtlasUVMap,
        state: BlockState,
    ) -> u16 {
        let slot = usize::from(state.id());
        if slot >= self.id_mappings.len() {
            self.id_mappings.resize(slot + 1, UNBAKED);
        }
        if self.id_mappings[slot] != UNBAKED {
            return self.id_mappings[slot];
        }
        let id = self.bake(view, uv_map, state);
        self.id_mappings[slot] = id;
        id
    }

    /// Voxy's `processModelResult` and `processTextureBakeResult`.
    fn bake(&mut self, view: BlockRenderView<'_>, uv_map: &AtlasUVMap, state: BlockState) -> u16 {
        let Some((faces, flags)) = self.bakery.render(view, uv_map, state) else {
            return 0;
        };
        let table = view.block_state_table();
        let data = table.block_state_data(state);
        let tint = match flags.tint {
            Tint::None => ModelTint::None,
            Tint::Grass | Tint::DoubleGrass => ModelTint::Grass,
            Tint::Foliage => ModelTint::Foliage,
            Tint::DryFoliage => ModelTint::DryFoliage,
            Tint::Water => ModelTint::Water,
            Tint::Constant(rgb) => ModelTint::Constant(to_bytes(int_to_rgb(rgb as i32))),
            Tint::Redstone => ModelTint::Constant(to_bytes(data.redstone_wire_rgb())),
            Tint::Stem => ModelTint::Constant(to_bytes(data.stem_rgb())),
        };
        let is_fluid = data.is_fluid_block();
        // A block holding a fluid draws the fluid's model around it.
        let held_fluid = match data.fluid().kind {
            _ if is_fluid => None,
            FluidKind::Empty => Some(0),
            _ => Some(self.model(view, uv_map, table.fluid_block(data.fluid()))),
        };
        let entry = ModelEntry {
            faces: Box::new(faces),
            fluid: held_fluid,
            tint,
        };
        if let Some(&id) = self.entries.get(&entry) {
            return id;
        }
        if self.models.len() == CAPACITY {
            // TODO: a model atlas that grows past its capacity.
            return 0;
        }
        let faces = &entry.faces;
        let leaves = data.is_leaves();
        let mut layer = None;
        if flags.translucent {
            layer = Some(if faces.iter().any(has_translucent_pixel) {
                Layer::Translucent
            } else if faces.iter().all(is_solid_where_drawn) {
                Layer::Solid
            } else {
                Layer::Cutout
            });
        }
        if layer.is_none() && flags.discard {
            layer = Some(Layer::Cutout);
        }
        if leaves {
            layer = Some(Layer::Solid);
        }
        let layer = layer.unwrap_or(Layer::Solid);
        let check = if layer == Layer::Solid {
            WriteCheck::Stencil
        } else {
            WriteCheck::Alpha
        };
        let depth_mode = if layer == Layer::Solid {
            DepthMode::Avg
        } else {
            DepthMode::Min
        };
        let depths: [Option<f32>; 6] =
            std::array::from_fn(|i| compute_depth(&faces[i], depth_mode, check));
        // Nothing on either side of an axis: plants and the like, drawn from
        // both sides.
        let double_sided = [0, 2, 4]
            .iter()
            .any(|&a| depths[a].is_none() && depths[a + 1].is_none());
        let mut metadata = u64::from(tint.biome_dependent())
            | u64::from(layer == Layer::Translucent) << 1
            | u64::from(double_sided) << 2
            | u64::from(!is_fluid && data.fluid().kind != FluidKind::Empty) << 3
            | u64::from(is_fluid) << 4
            | u64::from(data.culls_same()) << 5;
        let mut fully_opaque = true;
        let mut face_data = [u32::MAX; 6];
        // Highest face first, so each shifts into place.
        for face in (0..6).rev() {
            metadata <<= 8;
            let Some(offset) = depths[face] else {
                metadata |= 0xFF;
                fully_opaque = false;
                continue;
            };
            let texture = &faces[face];
            let [min_x, max_x, min_y, max_y] = compute_bounds(texture, check);
            let written = written_pixel_count(texture, check) as f32;
            let covers_block = min_x == 0 && min_y == 0 && max_x == SIZE - 1 && max_y == SIZE - 1;
            metadata |= u64::from(covers_block) << 1;
            let occludes = layer != Layer::Translucent
                // A face drawn far in from its side doesn't hide the next block.
                && offset < 0.1
                && written / TEXELS as f32 > 0.9;
            metadata |= u64::from(occludes);
            fully_opaque &= occludes;
            metadata |= u64::from(offset < 0.3) << 2;
            metadata |= u64::from(offset > 0.01 || layer == Layer::Translucent) << 3;
            let area = ((max_x + 1 - min_x) * (max_y + 1 - min_y)) as f32;
            // Fences and the like are solid with gaps between their parts.
            let discard =
                (written / area < 0.9 || layer != Layer::Solid) && layer != Layer::Translucent;
            let discard_when_merged = !covers_block && layer != Layer::Translucent;
            let tint_state = match face_tint(texture, check) {
                _ if tint == ModelTint::None => 0,
                FaceTint::Some => 1,
                FaceTint::All => 2,
                FaceTint::Nothing | FaceTint::None => 0,
            };
            face_data[face] = (min_x | max_x << 4 | min_y << 8 | max_y << 12) as u32
                | ((offset * 64.0).round() as u32).min(62) << 16
                | u32::from(discard) << 22
                | u32::from(discard_when_merged) << 23
                | tint_state << 24;
        }
        metadata |= u64::from(fully_opaque) << 54;
        metadata |= u64::from(data.light_props().emission) << 55;
        let id = self.models.len() as u16;
        self.models.push(ModelInfo {
            metadata: Metadata(metadata),
            fluid: held_fluid.unwrap_or(id),
        });
        self.tiles.push(ModelTile {
            id,
            model: BlockModel {
                face_data,
                flags: u32::from(tint != ModelTint::None)
                    | u32::from(tint.biome_dependent()) << 1
                    | u32::from(layer == Layer::Translucent) << 2
                    | u32::from(flags.shaded) << 3,
                tint: tint.packed(),
            },
            levels: mip_tile(flags.darkened, leaves, faces),
        });
        self.entries.insert(entry, id);
        id
    }
}

fn to_bytes(rgb: [f32; 3]) -> [u8; 3] {
    pack_tint(rgb).to_le_bytes()[..3]
        .try_into()
        .expect("three channels")
}

/// How `TextureUtils` tells a texel was drawn.
#[derive(Clone, Copy)]
enum WriteCheck {
    /// Some quad covered it.
    Stencil,
    /// Its alpha isn't (near) zero.
    Alpha,
}

fn was_pixel_written(face: &FaceTexture, check: WriteCheck, i: usize) -> bool {
    match check {
        WriteCheck::Stencil => face.depth[i] & 0xFF != 0,
        // Voxy: some come out with an alpha of 1 where they're meant to be 0.
        WriteCheck::Alpha => (face.colour[i] >> 24) & 0xFF > 1,
    }
}

fn written_pixel_count(face: &FaceTexture, check: WriteCheck) -> usize {
    (0..TEXELS)
        .filter(|&i| was_pixel_written(face, check, i))
        .count()
}

fn has_translucent_pixel(face: &FaceTexture) -> bool {
    (0..TEXELS).any(|i| {
        let alpha = face.colour[i] >> 24;
        face.depth[i] & 0xFF != 0 && alpha != 0 && alpha != 255
    })
}

fn is_solid_where_drawn(face: &FaceTexture) -> bool {
    (0..TEXELS).all(|i| face.depth[i] & 0xFF == 0 || face.colour[i] >> 24 == 255)
}

enum FaceTint {
    Nothing,
    None,
    Some,
    All,
}

/// Voxy's `computeFaceTint`, over the drawn texels that aren't black or
/// clear.
fn face_tint(face: &FaceTexture, check: WriteCheck) -> FaceTint {
    let (mut all, mut some, mut written) = (true, false, false);
    for i in 0..TEXELS {
        let colour = face.colour[i];
        if !was_pixel_written(face, check, i) || colour & 0xFF_FFFF == 0 || colour >> 24 == 0 {
            continue;
        }
        let tinted = face.depth[i] & (1 << 7) != 0;
        written = true;
        all &= tinted;
        some |= tinted;
    }
    match (written, some, all) {
        (false, ..) => FaceTint::Nothing,
        (true, false, _) => FaceTint::None,
        (true, true, false) => FaceTint::Some,
        (true, true, true) => FaceTint::All,
    }
}

#[derive(Clone, Copy)]
enum DepthMode {
    Avg,
    Min,
}

/// Voxy's `computeDepth`: how far in from its side the face's drawn texels
/// sit, 0 to 1; `None` with nothing drawn.
fn compute_depth(face: &FaceTexture, mode: DepthMode, check: WriteCheck) -> Option<f32> {
    let depths = (0..TEXELS)
        .filter(|&i| was_pixel_written(face, check, i))
        .map(|i| u64::from(face.depth[i] >> 8));
    let depth = match mode {
        DepthMode::Avg => {
            let (count, sum) = depths.fold((0, 0), |(n, sum), d| (n + 1, sum + d));
            (count != 0).then(|| sum / count)?
        }
        DepthMode::Min => depths.min()?,
    };
    Some((depth as f64 / f64::from((1 << 24) - 1)) as f32)
}

/// Voxy's `computeBounds`: the drawn texels' `[min_x, max_x, min_y, max_y]`.
fn compute_bounds(face: &FaceTexture, check: WriteCheck) -> [usize; 4] {
    let written = |x: usize, y: usize| was_pixel_written(face, check, x + y * SIZE);
    let column = |x| (0..SIZE).any(|y| written(x, y));
    let row = |y| (0..SIZE).any(|x| written(x, y));
    let min_x = (0..SIZE).find(|&x| column(x)).unwrap_or(SIZE);
    let max_x = (0..SIZE).rev().find(|&x| column(x)).unwrap_or(0);
    let min_y = (0..SIZE).find(|&y| row(y)).unwrap_or(SIZE);
    let max_y = (0..SIZE).rev().find(|&y| row(y)).unwrap_or(0);
    [min_x, max_x, min_y, max_y]
}

/// Voxy's `MipGen.putTextures`: the faces laid out in a tile, transparent
/// texels taking the colour of the nearest drawn one, then halved down to
/// 2x2 faces. Voxy leaves sprites that mip dark black there; leaves fill
/// theirs anyway, since coarse distant leaves draw them solid.
fn mip_tile(darkened: bool, leaves: bool, faces: &[FaceTexture; 6]) -> Vec<Vec<u8>> {
    let width = TILE_WIDTH as usize;
    let mut tile = vec![0u32; width * TILE_HEIGHT as usize];
    for (i, face) in faces.iter().enumerate() {
        let (x, y) = ((i >> 1) * SIZE, (i & 1) * SIZE);
        let mut colours = *face.colour;
        if (!darkened || leaves) && colours.iter().any(|c| c >> 24 == 0) {
            solidify(&mut colours);
        }
        for (j, &colour) in colours.iter().enumerate() {
            tile[(y + j / SIZE) * width + x + j % SIZE] = colour;
        }
    }
    let mut levels = vec![tile];
    for level in 1..MODEL_MIP_LEVELS as usize {
        let source = &levels[level - 1];
        let source_width = width >> (level - 1);
        let (w, h) = (width >> level, TILE_HEIGHT as usize >> level);
        let next = (0..w * h)
            .map(|i| {
                let at = (i % w) * 2 + (i / w) * 2 * source_width;
                mip_colours(
                    darkened,
                    [
                        source[at],
                        source[at + source_width],
                        source[at + 1],
                        source[at + source_width + 1],
                    ],
                )
            })
            .collect();
        levels.push(next);
    }
    levels
        .into_iter()
        .map(|level| level.iter().flat_map(|c| c.to_le_bytes()).collect())
        .collect()
}

/// Voxy's `MipGen.solidify`: a breadth-first walk out from the drawn
/// texels, each clear one taking the nearest one's colour and keeping its
/// own zero alpha, so mips don't bleed black.
fn solidify(colours: &mut [u32; TEXELS]) {
    // Per texel, the distance walked (high byte) and the drawn texel reached.
    let mut nearest = [u16::MAX; TEXELS];
    let mut queue = std::collections::VecDeque::new();
    for (i, colour) in colours.iter().enumerate() {
        if colour & 0xFF00_0000 != 0 {
            nearest[i] = i as u16;
            queue.push_back(i);
        }
    }
    while let Some(at) = queue.pop_front() {
        let (x, y) = ((at % SIZE) as i32, (at / SIZE) as i32);
        let next = nearest[at] + 0x0100;
        for (dx, dy) in [(0, 1), (0, -1), (1, 0), (-1, 0)] {
            let (x2, y2) = (x + dx, y + dy);
            if !(0..SIZE as i32).contains(&x2) || !(0..SIZE as i32).contains(&y2) {
                continue;
            }
            let to = x2 as usize + y2 as usize * SIZE;
            if next & 0xFF00 < nearest[to] & 0xFF00 {
                nearest[to] = next;
                queue.push_back(to);
            }
        }
    }
    for i in 0..TEXELS {
        if nearest[i] & 0xFF00 != 0 {
            colours[i] = colours[usize::from(nearest[i] & 0xFF)] & 0x00FF_FFFF;
        }
    }
}

/// Voxy's `TextureUtils.mipColours`: the mean of four ABGR texels in linear
/// space. Dark-mipped sprites count only texels with alpha and still divide
/// by four, darkening their edges as vanilla's `dark_cutout` does.
fn mip_colours(darkened: bool, texels: [u32; 4]) -> u32 {
    let mut sum = [0.0f32; 4];
    for texel in texels {
        if darkened && texel >> 24 == 0 {
            continue;
        }
        for (channel, total) in sum.iter_mut().enumerate().take(3) {
            *total += srgb_to_linear((texel >> (channel * 8)) as u8);
        }
        let alpha = (texel >> 24) as u8;
        sum[3] += if darkened {
            srgb_to_linear(alpha)
        } else {
            f32::from(alpha)
        };
    }
    let [r, g, b, a] = sum.map(|total| total / 4.0);
    let alpha = if darkened {
        linear_to_srgb(a)
    } else {
        (sum[3] as u32 / 4) as u8
    };
    u32::from_le_bytes([
        linear_to_srgb(r),
        linear_to_srgb(g),
        linear_to_srgb(b),
        alpha,
    ])
}

fn srgb_to_linear(channel: u8) -> f32 {
    let c = f32::from(channel) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(linear: f32) -> u8 {
    let c = if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    };
    (c * 255.0).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Six faces of green with a clear texel at the corner of each.
    fn holey_faces() -> [FaceTexture; 6] {
        std::array::from_fn(|_| {
            let mut colour = Box::new([u32::from_le_bytes([0, 200, 0, 255]); TEXELS]);
            colour[0] = 0;
            FaceTexture {
                colour,
                depth: Box::new([1; TEXELS]),
            }
        })
    }

    #[test]
    fn leaves_fill_their_holes_where_dark_sprites_stay_black() {
        // The clear texel takes its neighbour's colour, keeping zero alpha so
        // full detail still cuts it out.
        let filled = [0, 200, 0, 0];
        for (darkened, leaves, corner) in [
            (true, true, filled),
            (true, false, [0; 4]),
            (false, false, filled),
        ] {
            let levels = mip_tile(darkened, leaves, &holey_faces());
            assert_eq!(
                levels[0][..4],
                corner,
                "darkened {darkened}, leaves {leaves}"
            );
        }
    }

    #[test]
    fn metadata_fields_unpack() {
        let faces: u64 = (0..6).fold(0, |m, f| m | 0b1011 << (8 * f)) & !(0xFF << 16) | 0xFF << 16;
        let meta = Metadata(faces | (2 | 32 | 64) << 48 | 7 << 55);
        assert!(meta.face_exists(0) && !meta.face_exists(2));
        assert!(meta.face_occludes(1) && !meta.face_occludes(2));
        assert!(!meta.face_can_be_occluded(0) && meta.face_uses_self_lighting(0));
        assert!(meta.is_translucent() && meta.culls_same() && meta.is_fully_opaque());
        assert!(!meta.is_fluid() && !meta.contains_fluid() && !meta.is_double_sided());
        assert_eq!(meta.light_emission(), 7);
        assert_eq!(ModelTint::Constant([1, 2, 3]).packed(), 0xFF03_0201);
        assert_eq!(pack_tint([1.0, 0.0, 0.5]), 0xFF80_00FF);
    }
}
