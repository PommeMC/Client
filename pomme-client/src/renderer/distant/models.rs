//! Voxy's `ModelFactory`, `TextureUtils` and `MipGen`: block states baked
//! into models, each six face textures with mips in a tile of the model
//! atlas, plus what the mesher and shaders need to know of each face.

use std::collections::HashMap;

use azalea_block::BlockState;

use super::bakery::{Bakery, FaceTexture, MODEL_TEXTURE_SIZE, TEXELS};
use crate::renderer::chunk::atlas::AtlasUVMap;
use crate::world::block::render_assets::BlockRenderView;

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
/// Models the atlas holds; the first is the untextured one.
const CAPACITY: usize = (ATLAS_COLUMNS * ATLAS_ROWS) as usize;

/// A face's flags as `distant.frag` reads them.
pub(crate) const FACE_DISCARD: u8 = 1;
/// Discards too once a quad spans more than one model: the face doesn't
/// cover its whole side.
pub(crate) const FACE_DISCARD_WHEN_MERGED: u8 = 2;
/// The tint, in bits 2-3: 1 only on grey texels, 2 everywhere.
pub(crate) const FACE_TINT_SHIFT: u8 = 2;
/// Set by the mesher on the faces it blends.
pub(crate) const FACE_TRANSLUCENT: u8 = 0x10;
/// Nothing drawn on that side.
pub(crate) const FACE_EMPTY: u8 = 0x80;

/// Which layer a model draws in (vanilla `ChunkSectionLayer`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Layer {
    Solid,
    Cutout,
    Translucent,
}

/// What the mesher needs of a model.
#[derive(Clone, Copy)]
pub(crate) struct ModelInfo {
    /// Blended over what's behind it, as water.
    pub translucent: bool,
    /// Every face fills its side, so it hides what it touches.
    pub occludes: bool,
    /// By Voxy face index ([`crate::world::block::model::Direction::VALUES`]).
    pub faces: [u8; 6],
    /// Solid only as Voxy draws leaves; their texture has holes.
    pub holes: bool,
}

/// Model 0's place: states without a model draw in their colours, never
/// asking for its info.
const UNTEXTURED: ModelInfo = ModelInfo {
    translucent: false,
    occludes: true,
    faces: [0; 6],
    holes: false,
};

/// A new model's tile and its mips, to upload to the atlas.
pub(crate) struct ModelTile {
    pub id: u16,
    /// RGBA per level, the level-0 tile first, row by row.
    pub levels: Vec<Vec<u8>>,
}

pub(crate) struct ModelFactory {
    bakery: Bakery,
    /// Each state's model, once baked.
    id_mappings: HashMap<BlockState, u16>,
    /// Models by their faces, so states that look alike share one.
    by_faces: HashMap<Box<[FaceTexture; 6]>, u16>,
    models: Vec<ModelInfo>,
    /// Tiles baked since the last [`Self::take_tiles`].
    tiles: Vec<ModelTile>,
}

impl ModelFactory {
    pub fn new() -> Self {
        Self {
            bakery: Bakery::new(),
            id_mappings: HashMap::new(),
            by_faces: HashMap::new(),
            models: vec![UNTEXTURED],
            tiles: Vec::new(),
        }
    }

    pub fn info(&self, model: u16) -> ModelInfo {
        self.models[usize::from(model)]
    }

    pub fn take_tiles(&mut self) -> Vec<ModelTile> {
        std::mem::take(&mut self.tiles)
    }

    /// `state`'s model, baked the first time; 0, untextured, when it draws
    /// nothing or the atlas is full.
    pub fn model(
        &mut self,
        view: BlockRenderView<'_>,
        uv_map: &AtlasUVMap,
        state: BlockState,
    ) -> u16 {
        if let Some(&id) = self.id_mappings.get(&state) {
            return id;
        }
        let id = self.bake(view, uv_map, state);
        self.id_mappings.insert(state, id);
        id
    }

    /// Voxy's `processModelResult` and `processTextureBakeResult`.
    fn bake(&mut self, view: BlockRenderView<'_>, uv_map: &AtlasUVMap, state: BlockState) -> u16 {
        let Some((faces, flags)) = self.bakery.render(view, uv_map, state) else {
            return 0;
        };
        let faces = Box::new(faces);
        if let Some(&id) = self.by_faces.get(&faces) {
            return id;
        }
        if self.models.len() == CAPACITY {
            // TODO: a model atlas that grows past its capacity.
            return 0;
        }
        let leaves = view
            .block_state_table()
            .block_state_data(state)
            .id()
            .ends_with("_leaves");
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
        let mut fully_opaque = true;
        let face_flags = std::array::from_fn(|i| {
            let face = &faces[i];
            let Some(offset) = compute_depth(face, depth_mode, check) else {
                fully_opaque = false;
                return FACE_EMPTY;
            };
            let [min_x, max_x, min_y, max_y] = compute_bounds(face, check);
            let written = written_pixel_count(face, check) as f32;
            let covers_block = min_x == 0 && min_y == 0 && max_x == SIZE - 1 && max_y == SIZE - 1;
            let occludes = layer != Layer::Translucent
                // A face drawn far in from its side doesn't hide the next block.
                && offset < 0.1
                && written / TEXELS as f32 > 0.9;
            fully_opaque &= occludes;
            let area =
                ((max_x + 1).saturating_sub(min_x) * (max_y + 1).saturating_sub(min_y)) as f32;
            // Fences and the like are solid with gaps between their parts.
            let discard =
                (written / area < 0.9 || layer != Layer::Solid) && layer != Layer::Translucent;
            let mut flags = u8::from(discard) * FACE_DISCARD;
            if !covers_block && layer != Layer::Translucent {
                flags |= FACE_DISCARD_WHEN_MERGED;
            }
            // Voxy asks only of tinted blocks; untinted quads mark no texel.
            flags |= match face_tint(face, check) {
                FaceTint::Some => 1,
                FaceTint::All => 2,
                FaceTint::Nothing | FaceTint::None => 0,
            } << FACE_TINT_SHIFT;
            flags
        });
        let id = self.models.len() as u16;
        self.models.push(ModelInfo {
            translucent: layer == Layer::Translucent,
            occludes: fully_opaque,
            faces: face_flags,
            holes: leaves,
        });
        self.tiles.push(ModelTile {
            id,
            levels: mip_tile(flags.darkened, &faces),
        });
        self.by_faces.insert(faces, id);
        id
    }
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
/// texels taking the colour of the nearest drawn one (unless the sprites
/// mip dark), then halved down to 2x2 faces.
fn mip_tile(darkened: bool, faces: &[FaceTexture; 6]) -> Vec<Vec<u8>> {
    let width = TILE_WIDTH as usize;
    let mut tile = vec![0u32; width * TILE_HEIGHT as usize];
    for (i, face) in faces.iter().enumerate() {
        let (x, y) = ((i >> 1) * SIZE, (i & 1) * SIZE);
        let mut colours = *face.colour;
        if !darkened && colours.iter().any(|c| c >> 24 == 0) {
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
