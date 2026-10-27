use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::assets::{AssetId, AssetIndex};
use crate::renderer::{rhi, util};
use crate::world::block::model::TextureId;
use crate::world::block::render_assets::BlockRenderAssets;

#[derive(Debug, Clone, Copy)]
pub struct AtlasRegion {
    /// Exact normalized bounds of the sprite's level-0 content rectangle.
    /// Vanilla addresses the full sprite; shrinking these bounds distorts a
    /// block's first/last texel columns and makes adjacent faces disagree.
    pub u_min: f32,
    pub v_min: f32,
    pub u_max: f32,
    pub v_max: f32,
    /// Exact level-0 content rectangle `(x, y, width, height)` in atlas texels.
    /// The atlas is capped at 8192², so u16 is sufficient and avoids the
    /// boundary rounding that motivated the old terrain inset.
    pub pixel_rect: [u16; 4],
    /// Index into the atlas's sprite-rectangle buffer, which terrain vertices
    /// carry instead of the rectangle itself. Index 0 is the missing tile.
    pub sprite: u16,
    /// Every level-0 texel is fully opaque (alpha 255), so quads using this
    /// sprite can render in the no-discard solid pass (early-Z). Sprites with
    /// any transparent texel are cutout and stay in the discard pass.
    pub opaque: bool,
    /// Some texel has an alpha strictly between 0 and 255. Vanilla
    /// `NativeImage.computeTransparency` routes such sprites to the
    /// translucent item sheet rather than the cutout one.
    pub translucent: bool,
}

#[cfg(test)]
impl AtlasRegion {
    /// A one-texel, fully opaque sprite spanning the whole atlas.
    pub(crate) const UNIT: Self = Self {
        u_min: 0.0,
        v_min: 0.0,
        u_max: 1.0,
        v_max: 1.0,
        pixel_rect: [0, 0, 1, 1],
        sprite: 0,
        opaque: true,
        translucent: false,
    };
}

#[derive(Clone)]
pub struct AtlasUVMap {
    regions: HashMap<String, AtlasRegion>,
    /// `regions` by `TextureId`, for the ids interned before the atlas was
    /// built; later ones fall back to the name lookup.
    by_texture: Vec<AtlasRegion>,
    sprite_alpha_masks: HashMap<String, SpriteAlphaMask>,
    /// Level-0 rectangles by sprite index, as `(x, y, width, height)`.
    rects: Vec<[u32; 4]>,
    missing: AtlasRegion,
}

impl AtlasUVMap {
    fn new(
        regions: HashMap<String, AtlasRegion>,
        sprite_alpha_masks: HashMap<String, SpriteAlphaMask>,
        rects: Vec<[u32; 4]>,
        missing: AtlasRegion,
    ) -> Self {
        let by_texture = TextureId::all_names()
            .into_iter()
            .map(|name| regions.get(name).copied().unwrap_or(missing))
            .collect();
        Self {
            regions,
            by_texture,
            sprite_alpha_masks,
            rects,
            missing,
        }
    }

    pub fn get_region(&self, name: &str) -> AtlasRegion {
        self.regions.get(name).copied().unwrap_or(self.missing)
    }

    pub fn region(&self, texture: TextureId) -> AtlasRegion {
        self.by_texture
            .get(texture.index())
            .copied()
            .unwrap_or_else(|| self.get_region(texture.name()))
    }

    pub fn has_region(&self, name: &str) -> bool {
        self.regions.contains_key(name)
    }

    pub fn missing_region(&self) -> AtlasRegion {
        self.missing
    }

    pub(crate) fn sprite_alpha_mask(&self, name: &str) -> Option<&SpriteAlphaMask> {
        self.sprite_alpha_masks.get(name)
    }

    /// Each sprite's texture name, by sprite index.
    #[cfg(feature = "cpu-bench")]
    pub fn sprite_names(&self) -> HashMap<u16, &str> {
        self.regions
            .iter()
            .map(|(name, region)| (region.sprite, name.as_str()))
            .collect()
    }
}

/// Vanilla 26.2 `atlases/blocks.json`, the only atlas built with mipmaps.
/// Block keys are bare names here; the three entity sprites are listed as is.
fn uses_block_atlas_mip_chain(key: &str) -> bool {
    let path = AssetId::parse(key).path;
    !path.contains('/')
        || path.starts_with("block/")
        || path.starts_with("entity/conduit/")
        || path == "entity/bell/bell_body"
        || path == "entity/enchantment/enchanting_table_book"
}

pub fn atlas_asset_path(key: &str) -> String {
    if key.contains(':') {
        return AssetId::parse(key).asset_key("textures", ".png");
    }
    if key.starts_with("item/") || key.starts_with("entity/") || key.starts_with("particle/") {
        format!("minecraft/textures/{key}.png")
    } else {
        format!("minecraft/textures/block/{key}.png")
    }
}

pub struct TextureAtlas {
    pub texture: rhi::backend::Texture,
    pub sampler: rhi::backend::Sampler,
    /// Storage buffer of `uvec4` level-0 sprite rectangles, indexed by
    /// `AtlasRegion::sprite`; the terrain shaders resolve sprite UVs inside it.
    pub sprite_rects: rhi::backend::Buffer,
    pub uv_map: AtlasUVMap,
    mip_levels: u32,
    animated: Vec<AnimatedSprite>,
    /// A tick ran since the last upload; vanilla only draws after a tick.
    animation_ticked: bool,
    /// Per frame in flight, room for every animated sprite's levels at once;
    /// empty when nothing animates.
    animation_staging: Vec<rhi::backend::Buffer>,
}

const MISSING_TILE: u32 = 16;

/// Maximum block-atlas mip level. Vanilla requests four levels and then lowers
/// this globally when any stitched block-atlas sprite cannot support them.
const MAX_MIP_LEVEL: u32 = 4;

struct Source {
    name: String,
    data: Vec<u8>,
    w: u32,
    h: u32,
    /// Over every frame, as vanilla's `SpriteContents.transparency` is.
    opaque: bool,
    translucent: bool,
    alpha_mask: Option<SpriteAlphaMask>,
    mip_source: Option<MipSource>,
    animation: Option<SpriteAnimation>,
}

impl Source {
    /// A texture that failed to load; packs as the missing tile.
    fn empty(name: &str) -> Self {
        Self {
            name: name.to_string(),
            data: Vec::new(),
            w: 0,
            h: 0,
            opaque: false,
            translucent: false,
            alpha_mask: None,
            mip_source: None,
            animation: None,
        }
    }
}

/// Vanilla `SpriteContents.AnimatedTexture`: the frames a sprite plays and
/// each unique frame's pixels.
struct SpriteAnimation {
    /// `(slot in pixels, ticks shown)` in play order.
    frames: Vec<(usize, u32)>,
    interpolate: bool,
    /// Each unique frame's levels, level 0 first. Level 0 only until
    /// `build_mip_chain` cuts a mipped sprite's levels from its strip mips.
    pixels: Vec<Vec<Vec<u8>>>,
}

impl SpriteAnimation {
    /// `None` unless `layout` plays more than one frame.
    fn new(strip: &[u8], strip_width: u32, layout: &AnimationLayout) -> Option<Self> {
        if layout.frames.len() <= 1 {
            return None;
        }
        let slot = |index| {
            layout
                .unique_frames
                .iter()
                .position(|&frame| frame == index)
                .expect("unique_frames holds every played frame")
        };
        Some(Self {
            frames: layout
                .frames
                .iter()
                .map(|frame| (slot(frame.index), frame.time))
                .collect(),
            interpolate: layout.interpolate,
            pixels: layout
                .unique_frames
                .iter()
                .map(|&frame| vec![extract_frame_rgba(strip, strip_width, layout, frame)])
                .collect(),
        })
    }
}

/// Vanilla `SpriteContents.AnimationState`: a placed animated sprite and where
/// it is in its frames.
struct AnimatedSprite {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    animation: SpriteAnimation,
    frame: usize,
    sub_frame: u32,
    /// The shown frame changed since the last upload. Vanilla's flag only
    /// covers the last tick; several ticks can pass between two frames here.
    dirty: bool,
}

impl AnimatedSprite {
    /// Vanilla `AnimationState.tick`.
    fn tick(&mut self) {
        self.sub_frame += 1;
        let (slot, time) = self.animation.frames[self.frame];
        if self.sub_frame >= time {
            self.frame = (self.frame + 1) % self.animation.frames.len();
            self.sub_frame = 0;
            self.dirty |= self.animation.frames[self.frame].0 != slot;
        }
    }

    /// Vanilla `AnimationState.needsToDraw`.
    fn needs_upload(&self) -> bool {
        self.animation.interpolate || self.dirty
    }

    fn levels(&self) -> u32 {
        self.animation.pixels[0].len() as u32
    }

    /// Level `level` as vanilla's `drawToAtlas` writes it: the shown frame, or
    /// mixed toward the next one by the frame's progress.
    fn level_pixels<'a>(&'a self, level: usize, scratch: &'a mut Vec<u8>) -> &'a [u8] {
        let frames = &self.animation.frames;
        let (slot, time) = frames[self.frame];
        let current = &self.animation.pixels[slot][level];
        if !self.animation.interpolate {
            return current;
        }
        let next = &self.animation.pixels[frames[(self.frame + 1) % frames.len()].0][level];
        // Vanilla packs the progress into the vertex index as an int of
        // thousandths.
        let progress = ((self.sub_frame as f32 / time as f32) * 1000.0) as i32 as f32 / 1000.0;
        mix_rgba8(current, next, progress, scratch);
        scratch
    }
}

/// `animate_sprite_interpolate.fsh`: `mix` on the atlas's RGBA8 values.
fn mix_rgba8(current: &[u8], next: &[u8], progress: f32, out: &mut Vec<u8>) {
    out.clear();
    out.extend(current.iter().zip(next).map(|(&a, &b)| {
        let mixed = f32::from(a) / 255.0 * (1.0 - progress) + f32::from(b) / 255.0 * progress;
        (mixed * 255.0).round() as u8
    }));
}

struct MipSource {
    full_data: Vec<u8>,
    full_width: u32,
    full_height: u32,
    animation: AnimationLayout,
    strategy: MipmapStrategy,
    alpha_cutoff_bias: f32,
}

#[derive(serde::Deserialize, Default)]
struct TextureMetadataFile {
    animation: Option<AnimationMetadata>,
    texture: Option<TextureMetadata>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum MipmapStrategy {
    #[default]
    Auto,
    Mean,
    Cutout,
    StrictCutout,
    DarkCutout,
}

#[derive(serde::Deserialize)]
struct TextureMetadata {
    #[serde(default)]
    mipmap_strategy: MipmapStrategy,
    #[serde(default)]
    alpha_cutoff_bias: f32,
}

#[derive(serde::Deserialize)]
struct AnimationMetadata {
    width: Option<u32>,
    height: Option<u32>,
    frames: Option<Vec<AnimationFrameSpec>>,
    frametime: Option<u32>,
    interpolate: Option<bool>,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum AnimationFrameSpec {
    Index(u32),
    Timed { index: u32, time: Option<u32> },
}

impl AnimationFrameSpec {
    fn index(&self) -> u32 {
        match self {
            Self::Index(index) | Self::Timed { index, .. } => *index,
        }
    }

    fn has_valid_time(&self) -> bool {
        !matches!(self, Self::Timed { time: Some(0), .. })
    }

    fn time(&self) -> Option<u32> {
        match self {
            Self::Index(_) => None,
            Self::Timed { time, .. } => *time,
        }
    }
}

/// Vanilla `SpriteContents.FrameInfo`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AnimationFrame {
    index: u32,
    time: u32,
}

#[derive(Clone)]
struct AnimationLayout {
    frame_width: u32,
    frame_height: u32,
    frames_per_row: u32,
    unique_frames: Vec<u32>,
    initial_display_frame: u32,
    /// The frames played, empty for a still sprite.
    frames: Vec<AnimationFrame>,
    interpolate: bool,
}

fn static_animation_layout(width: u32, height: u32) -> AnimationLayout {
    AnimationLayout {
        frame_width: width,
        frame_height: height,
        frames_per_row: 1,
        unique_frames: vec![0],
        initial_display_frame: 0,
        frames: Vec::new(),
        interpolate: false,
    }
}

fn read_texture_metadata(metadata_path: Option<&Path>) -> Option<TextureMetadataFile> {
    let Some(metadata_path) = metadata_path else {
        return Some(TextureMetadataFile::default());
    };
    let json = std::fs::read_to_string(metadata_path).ok()?;
    serde_json::from_str(&json).ok()
}

fn animation_layout_from_metadata(
    metadata: Option<&AnimationMetadata>,
    width: u32,
    height: u32,
) -> Option<AnimationLayout> {
    let Some(metadata) = metadata else {
        return Some(static_animation_layout(width, height));
    };
    if metadata.width == Some(0)
        || metadata.height == Some(0)
        || metadata.frametime == Some(0)
        || metadata
            .frames
            .as_ref()
            .is_some_and(|frames| frames.iter().any(|frame| !frame.has_valid_time()))
    {
        return None;
    }

    let min_dimension = width.min(height);
    let frame_width = metadata.width.unwrap_or_else(|| {
        if metadata.height.is_some() {
            width
        } else {
            min_dimension
        }
    });
    let frame_height = metadata.height.unwrap_or_else(|| {
        if metadata.width.is_some() {
            height
        } else {
            min_dimension
        }
    });
    if !width.is_multiple_of(frame_width) || !height.is_multiple_of(frame_height) {
        return None;
    }

    let frames_per_row = width / frame_width;
    let total_frames = frames_per_row * (height / frame_height);
    let frame_time = metadata.frametime.unwrap_or(1);
    let frames: Vec<AnimationFrame> = match &metadata.frames {
        Some(frames) => frames
            .iter()
            .map(|frame| AnimationFrame {
                index: frame.index(),
                time: frame.time().unwrap_or(frame_time),
            })
            .filter(|frame| frame.index < total_frames)
            .collect(),
        None => (0..total_frames)
            .map(|index| AnimationFrame {
                index,
                time: frame_time,
            })
            .collect(),
    };

    // SpriteContents drops the AnimatedTexture wrapper when zero or one valid
    // frame remains. In that case frame lookups ignore the metadata index and
    // sample the top-left logical frame from the original image.
    if frames.len() <= 1 {
        return Some(AnimationLayout {
            frame_width,
            frame_height,
            frames_per_row,
            ..static_animation_layout(width, height)
        });
    }

    let initial_display_frame = frames[0].index;
    let mut seen = HashSet::new();
    let unique_frames: Vec<u32> = frames
        .iter()
        .map(|frame| frame.index)
        .filter(|&index| seen.insert(index))
        .collect();

    Some(AnimationLayout {
        frame_width,
        frame_height,
        frames_per_row,
        unique_frames,
        initial_display_frame,
        frames,
        interpolate: metadata.interpolate.unwrap_or(false),
    })
}

fn extract_frame_rgba(
    data: &[u8],
    full_width: u32,
    layout: &AnimationLayout,
    frame: u32,
) -> Vec<u8> {
    let frame_x = frame % layout.frames_per_row;
    let frame_y = frame / layout.frames_per_row;
    let row_bytes = full_width as usize * 4;
    let frame_row_bytes = layout.frame_width as usize * 4;
    let mut output =
        Vec::with_capacity(layout.frame_width as usize * layout.frame_height as usize * 4);
    for y in 0..layout.frame_height {
        let source_y = frame_y * layout.frame_height + y;
        let start = source_y as usize * row_bytes + frame_x as usize * frame_row_bytes;
        output.extend_from_slice(&data[start..start + frame_row_bytes]);
    }
    output
}

#[derive(Clone)]
pub(crate) struct SpriteAlphaMask {
    pub width: u32,
    pub height: u32,
    pub(crate) frames: Vec<Vec<bool>>,
}

impl SpriteAlphaMask {
    fn frame_is_opaque(&self, frame: &[bool], x: i32, y: i32) -> bool {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return false;
        }
        frame[y as usize * self.width as usize + x as usize]
    }

    pub fn has_exposed_edge(&self, x: i32, y: i32, neighbor_x: i32, neighbor_y: i32) -> bool {
        self.frames.iter().any(|frame| {
            self.frame_is_opaque(frame, x, y)
                && !self.frame_is_opaque(frame, neighbor_x, neighbor_y)
        })
    }

    #[cfg(test)]
    fn is_opaque_in_any_frame(&self, x: i32, y: i32) -> bool {
        self.frames
            .iter()
            .any(|frame| self.frame_is_opaque(frame, x, y))
    }
}

fn sprite_alpha_mask_from_rgba(
    data: &[u8],
    full_width: u32,
    layout: &AnimationLayout,
) -> SpriteAlphaMask {
    let frames = layout
        .unique_frames
        .iter()
        .map(|frame| {
            extract_frame_rgba(data, full_width, layout, *frame)
                .as_chunks::<4>()
                .0
                .iter()
                .map(|pixel| pixel[3] != 0)
                .collect()
        })
        .collect();
    SpriteAlphaMask {
        width: layout.frame_width,
        height: layout.frame_height,
        frames,
    }
}

fn resolve_texture_resource(
    asset_key: &str,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    packs: Option<&crate::resource_pack::ResourcePackManager>,
) -> (PathBuf, Option<PathBuf>) {
    if let Some((path, metadata)) =
        packs.and_then(|packs| packs.resolve_asset_with_metadata(asset_key))
    {
        return (path, metadata);
    }

    let path = asset_index
        .as_ref()
        .and_then(|index| index.resolve(asset_key))
        .unwrap_or_else(|| jar_assets_dir.join(asset_key));
    let metadata_key = format!("{asset_key}.mcmeta");
    let metadata = packs
        .and_then(|packs| packs.resolve_asset(&metadata_key))
        .or_else(|| {
            asset_index
                .as_ref()
                .and_then(|index| index.resolve(&metadata_key))
        })
        .or_else(|| {
            let path = jar_assets_dir.join(&metadata_key);
            path.exists().then_some(path)
        });
    (path, metadata)
}

fn load_source(
    name: &str,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    packs: Option<&crate::resource_pack::ResourcePackManager>,
    retain_alpha_mask: bool,
) -> Source {
    let asset_key = atlas_asset_path(name);
    let (file_path, metadata_path) =
        resolve_texture_resource(&asset_key, jar_assets_dir, asset_index, packs);
    match util::load_png(&file_path) {
        Some((data, width, height)) => {
            let Some(metadata) = read_texture_metadata(metadata_path.as_deref()) else {
                tracing::warn!("Invalid texture metadata: {name}");
                return Source::empty(name);
            };
            let Some(animation) =
                animation_layout_from_metadata(metadata.animation.as_ref(), width, height)
            else {
                tracing::warn!("Invalid texture animation metadata: {name}");
                return Source::empty(name);
            };
            let alpha_mask =
                retain_alpha_mask.then(|| sprite_alpha_mask_from_rgba(&data, width, &animation));
            let (opaque, translucent) = sprite_transparency(&data);
            let display =
                extract_frame_rgba(&data, width, &animation, animation.initial_display_frame);
            let sprite_animation = SpriteAnimation::new(&data, width, &animation);
            let texture = metadata.texture.as_ref();
            let mip_source = uses_block_atlas_mip_chain(name).then(|| MipSource {
                full_data: data,
                full_width: width,
                full_height: height,
                animation: animation.clone(),
                strategy: texture.map(|t| t.mipmap_strategy).unwrap_or_default(),
                alpha_cutoff_bias: texture.map(|t| t.alpha_cutoff_bias).unwrap_or(0.0),
            });
            Source {
                name: name.to_string(),
                data: display,
                w: animation.frame_width,
                h: animation.frame_height,
                opaque,
                translucent,
                alpha_mask,
                mip_source,
                animation: sprite_animation,
            }
        }
        None => {
            tracing::warn!("Missing texture: {name}");
            Source::empty(name)
        }
    }
}

/// Every texture the block atlas packs for `render_assets`, and the
/// flat-generated item textures among them.
pub(crate) fn atlas_texture_names(
    render_assets: &BlockRenderAssets,
) -> (HashSet<&str>, HashSet<&str>) {
    let generated_item_textures: HashSet<&str> = render_assets.flat_item_textures().collect();
    let texture_names = render_assets
        .texture_names()
        .chain(generated_item_textures.iter().copied())
        .chain(crate::particle::END_ROD_SPRITES)
        .collect();
    (texture_names, generated_item_textures)
}

/// [`TextureAtlas::build_cpu`]'s output: everything the GPU upload consumes.
pub(crate) struct AtlasCpu {
    pub uv_map: AtlasUVMap,
    atlas_pixels: Vec<u8>,
    atlas_size: u32,
    mip_level: u32,
    sources: Vec<Source>,
    placements: HashMap<String, Option<(u32, u32)>>,
}

impl TextureAtlas {
    pub fn build(
        device: &rhi::backend::Device,
        jar_assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
        texture_names: &HashSet<&str>,
        generated_item_textures: &HashSet<&str>,
        packs: Option<&crate::resource_pack::ResourcePackManager>,
    ) -> Result<Self, String> {
        let AtlasCpu {
            uv_map,
            atlas_pixels,
            atlas_size,
            mip_level,
            mut sources,
            placements,
        } = Self::build_cpu(
            jar_assets_dir,
            asset_index,
            texture_names,
            generated_item_textures,
            packs,
        );

        let sprite_rects = rhi::backend::Buffer::try_with_data(
            device,
            "atlas_sprite_rects",
            bytemuck::cast_slice(&uv_map.rects),
            rhi::BufferUsages::STORAGE,
        )?;

        let staging_pixels = build_mip_chain(
            atlas_pixels,
            atlas_size,
            mip_level,
            &mut sources,
            &placements,
        );

        let mip_levels = mip_level + 1;
        let texture = rhi::backend::Texture::try_new(
            device,
            &rhi::TextureDesc {
                mip_levels,
                ..util::sampled_texture_desc("atlas_image", atlas_size, atlas_size)
            },
        )?;
        let staging_buffer = rhi::backend::Buffer::try_with_data(
            device,
            "atlas_staging",
            &staging_pixels,
            rhi::BufferUsages::COPY_SRC,
        )?;

        let uploaded = util::upload_image_mipmapped(
            device,
            staging_buffer.handle(),
            staging_pixels.len() as u64,
            texture.handle(),
            atlas_size,
            atlas_size,
            mip_levels,
        );
        staging_buffer.destroy(device);
        uploaded?;

        let sampler = rhi::backend::try_create_sampler(
            device,
            &rhi::SamplerDesc::NEAREST.with_mips(mip_levels),
        )?;

        let animated: Vec<AnimatedSprite> = sources
            .into_iter()
            .filter_map(|source| {
                let (x, y) = (*placements.get(source.name.as_str())?)?;
                Some(AnimatedSprite {
                    x,
                    y,
                    width: source.w,
                    height: source.h,
                    animation: source.animation?,
                    frame: 0,
                    sub_frame: 0,
                    dirty: false,
                })
            })
            .collect();
        let staging_bytes: usize = animated
            .iter()
            .flat_map(|sprite| &sprite.animation.pixels[0])
            .map(Vec::len)
            .sum();
        let animation_staging = if animated.is_empty() {
            Vec::new()
        } else {
            util::create_per_frame_host_buffers(
                device,
                staging_bytes as u64,
                rhi::BufferUsages::COPY_SRC,
                "atlas_animation",
            )
        };

        tracing::info!(
            "Atlas built: {atlas_size}x{atlas_size}, mip level {mip_level}, {} regions, {} animated",
            uv_map.regions.len(),
            animated.len()
        );

        Ok(Self {
            texture,
            sampler,
            sprite_rects,
            uv_map,
            mip_levels,
            animated,
            animation_ticked: false,
            animation_staging,
        })
    }

    /// Vanilla `TextureAtlas.cycleAnimationFrames`' tick half; the upload
    /// waits for [`Self::record_animation_upload`].
    pub fn tick_animations(&mut self) {
        for sprite in &mut self.animated {
            sprite.tick();
        }
        self.animation_ticked = true;
    }

    /// Holds every animated sprite on its first frame.
    pub fn hold_animations(&mut self) {
        for sprite in &mut self.animated {
            sprite.dirty |= sprite.frame != 0;
            sprite.frame = 0;
            sprite.sub_frame = 0;
        }
        self.animation_ticked = true;
    }

    /// Vanilla `uploadAnimationFrames`: writes every sprite that needs it
    /// into its rectangle at each of its levels.
    pub fn record_animation_upload(
        &mut self,
        encoder: &rhi::backend::CommandEncoder,
        frame: usize,
    ) {
        if !std::mem::take(&mut self.animation_ticked) {
            return;
        }
        let Some(staging) = self.animation_staging.get_mut(frame) else {
            return;
        };
        let mut copies = Vec::new();
        let mut offset = 0;
        let mut scratch = Vec::new();
        for sprite in self
            .animated
            .iter_mut()
            .filter(|sprite| sprite.needs_upload())
        {
            sprite.dirty = false;
            for level in 0..sprite.levels() {
                let pixels = sprite.level_pixels(level as usize, &mut scratch);
                staging.write_at(offset, pixels);
                copies.push(rhi::TexelCopy {
                    buffer_offset: offset as u64,
                    mip_level: level,
                    layer: 0,
                    origin: [sprite.x >> level, sprite.y >> level],
                    width: sprite.width >> level,
                    height: sprite.height >> level,
                });
                offset += pixels.len();
            }
        }
        if copies.is_empty() {
            return;
        }
        let transition = |from, to| rhi::backend::TextureTransition {
            texture: self.texture.handle(),
            range: rhi::TextureRange {
                mips: self.mip_levels,
                ..rhi::TextureRange::BASE
            },
            from,
            to,
        };
        encoder.transition_textures(&[transition(
            rhi::TextureUse::ShaderRead,
            rhi::TextureUse::CopyDst,
        )]);
        encoder.copy_buffer_to_texture(staging.handle(), self.texture.handle(), &copies);
        encoder.transition_textures(&[transition(
            rhi::TextureUse::CopyDst,
            rhi::TextureUse::ShaderRead,
        )]);
    }

    /// The CPU half of [`Self::build`]: loads and packs every sprite into the
    /// level-0 atlas pixels and UV map, with no GPU work.
    pub(crate) fn build_cpu(
        jar_assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
        texture_names: &HashSet<&str>,
        generated_item_textures: &HashSet<&str>,
        packs: Option<&crate::resource_pack::ResourcePackManager>,
    ) -> AtlasCpu {
        let mut sources: Vec<Source> = Vec::with_capacity(texture_names.len());
        for &name in texture_names {
            sources.push(load_source(
                name,
                jar_assets_dir,
                asset_index,
                packs,
                generated_item_textures.contains(name),
            ));
        }

        let mip_level = effective_mip_level(&sources);
        let mip_align = 1 << mip_level;
        let mut total_area: u64 = (MISSING_TILE * MISSING_TILE) as u64;
        for source in &sources {
            if !source.data.is_empty() {
                total_area += u64::from(source.w.next_multiple_of(mip_align))
                    * u64::from(source.h.next_multiple_of(mip_align));
            }
        }
        sources.sort_by_key(|s| std::cmp::Reverse(s.h.max(MISSING_TILE)));

        const MAX_ATLAS_SIZE: u32 = 8192;
        let mut atlas_size = (((total_area as f64) * 1.4).sqrt().ceil() as u32).next_power_of_two();

        let (placements, missing_region) = loop {
            let (result, all_fit) = pack(&sources, atlas_size, mip_align);
            if all_fit || atlas_size >= MAX_ATLAS_SIZE {
                if !all_fit {
                    tracing::warn!(
                        "Atlas at {MAX_ATLAS_SIZE} cap; oversize sources fall back to missing tile"
                    );
                }
                break result;
            }
            atlas_size *= 2;
        };

        let mut atlas_pixels = vec![0u8; (atlas_size * atlas_size * 4) as usize];
        // Vanilla `MissingTextureAtlasSprite.generateMissingImage`.
        for py in 0..MISSING_TILE {
            for px in 0..MISSING_TILE {
                let pink = (py < MISSING_TILE / 2) ^ (px < MISSING_TILE / 2);
                let color: [u8; 4] = if pink {
                    [248, 0, 248, 255]
                } else {
                    [0, 0, 0, 255]
                };
                let idx = ((py * atlas_size + px) * 4) as usize;
                atlas_pixels[idx..idx + 4].copy_from_slice(&color);
            }
        }

        let mut regions = HashMap::new();
        let mut sprite_alpha_masks = HashMap::new();
        let mut rects = vec![missing_region.pixel_rect.map(u32::from)];
        for src in &sources {
            match placements.get(src.name.as_str()) {
                Some(Some((cx, cy))) => {
                    let mut region = pixel_region(*cx, *cy, src.w, src.h, atlas_size);
                    region.sprite = u16::try_from(rects.len())
                        .expect("an 8192² atlas holds fewer than 65536 sprites");
                    region.opaque = src.opaque;
                    region.translucent = src.translucent;
                    rects.push(region.pixel_rect.map(u32::from));
                    for py in 0..src.h {
                        for px in 0..src.w {
                            let s = ((py * src.w + px) * 4) as usize;
                            let d = (((cy + py) * atlas_size + cx + px) * 4) as usize;
                            atlas_pixels[d..d + 4].copy_from_slice(&src.data[s..s + 4]);
                        }
                    }
                    if let Some(mask) = &src.alpha_mask {
                        sprite_alpha_masks.insert(src.name.clone(), mask.clone());
                    }
                    regions.insert(src.name.clone(), region);
                }
                _ => {
                    regions.insert(src.name.clone(), missing_region);
                }
            }
        }

        let uv_map = AtlasUVMap::new(regions, sprite_alpha_masks, rects, missing_region);

        AtlasCpu {
            uv_map,
            atlas_pixels,
            atlas_size,
            mip_level,
            sources,
            placements,
        }
    }

    /// Byte size of `sprite_rects`, for its descriptor range.
    pub fn sprite_rects_bytes(&self) -> u64 {
        (self.uv_map.rects.len() * size_of::<[u32; 4]>()) as u64
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        util::destroy_buffers(device, &mut self.animation_staging);
        self.sampler.destroy(device);
        std::mem::take(&mut self.texture).destroy(device);
        std::mem::take(&mut self.sprite_rects).destroy(device);
    }
}

fn effective_mip_level(sources: &[Source]) -> u32 {
    sources
        .iter()
        .filter(|source| !source.data.is_empty() && source.mip_source.is_some())
        .fold(MAX_MIP_LEVEL, |level, source| {
            level.min(source.w.trailing_zeros().min(source.h.trailing_zeros()))
        })
}

fn build_mip_chain(
    atlas_pixels: Vec<u8>,
    atlas_size: u32,
    mip_level: u32,
    sources: &mut [Source],
    placements: &HashMap<String, Option<(u32, u32)>>,
) -> Vec<u8> {
    let mut levels = Vec::with_capacity((mip_level + 1) as usize);
    levels.push(atlas_pixels);
    for level in 1..=mip_level {
        let size = atlas_size >> level;
        levels.push(vec![0; (size * size * 4) as usize]);
    }

    let missing = extract_rect_rgba(&levels[0], atlas_size, 0, 0, MISSING_TILE, MISSING_TILE);
    let missing_mips = generate_rgba_mips(
        missing,
        MISSING_TILE,
        MISSING_TILE,
        mip_level,
        MipmapStrategy::Mean,
        0.0,
    );
    for level in 1..=mip_level {
        copy_rect_rgba(
            &missing_mips[level as usize],
            MISSING_TILE >> level,
            MISSING_TILE >> level,
            &mut levels[level as usize],
            atlas_size >> level,
            0,
            0,
        );
    }

    for source in sources {
        let Some(Some((x, y))) = placements.get(source.name.as_str()) else {
            continue;
        };
        let Some(mip_source) = &source.mip_source else {
            continue;
        };
        let layout = &mip_source.animation;
        let full_mips = generate_rgba_mips(
            mip_source.full_data.clone(),
            mip_source.full_width,
            mip_source.full_height,
            mip_level,
            mip_source.strategy,
            mip_source.alpha_cutoff_bias,
        );
        let frame_levels = |frame| -> Vec<Vec<u8>> {
            (0..=mip_level)
                .map(|level| {
                    extract_frame_rgba_at_mip(
                        &full_mips[level as usize],
                        mip_source.full_width >> level,
                        layout,
                        frame,
                        level,
                    )
                })
                .collect()
        };
        if let Some(animation) = &mut source.animation {
            animation.pixels = layout
                .unique_frames
                .iter()
                .map(|&frame| frame_levels(frame))
                .collect();
        }
        for (level, frame) in frame_levels(layout.initial_display_frame)
            .into_iter()
            .enumerate()
        {
            let level = level as u32;
            copy_rect_rgba(
                &frame,
                source.w >> level,
                source.h >> level,
                &mut levels[level as usize],
                atlas_size >> level,
                x >> level,
                y >> level,
            );
        }
    }

    levels.concat()
}

fn extract_frame_rgba_at_mip(
    data: &[u8],
    full_width: u32,
    layout: &AnimationLayout,
    frame: u32,
    level: u32,
) -> Vec<u8> {
    let frame_width = layout.frame_width >> level;
    let frame_height = layout.frame_height >> level;
    let frame_x = frame % layout.frames_per_row;
    let frame_y = frame / layout.frames_per_row;
    extract_rect_rgba(
        data,
        full_width,
        frame_x * frame_width,
        frame_y * frame_height,
        frame_width,
        frame_height,
    )
}

fn extract_rect_rgba(
    data: &[u8],
    full_width: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Vec<u8> {
    let row_bytes = full_width as usize * 4;
    let copy_bytes = width as usize * 4;
    let mut output = Vec::with_capacity(width as usize * height as usize * 4);
    for row in y..y + height {
        let start = row as usize * row_bytes + x as usize * 4;
        output.extend_from_slice(&data[start..start + copy_bytes]);
    }
    output
}

fn copy_rect_rgba(
    source: &[u8],
    width: u32,
    height: u32,
    destination: &mut [u8],
    destination_width: u32,
    x: u32,
    y: u32,
) {
    let row_bytes = width as usize * 4;
    for row in 0..height {
        let src = row as usize * row_bytes;
        let dst = (((y + row) * destination_width + x) * 4) as usize;
        destination[dst..dst + row_bytes].copy_from_slice(&source[src..src + row_bytes]);
    }
}

fn generate_rgba_mips(
    mut level_zero: Vec<u8>,
    width: u32,
    height: u32,
    mip_level: u32,
    requested_strategy: MipmapStrategy,
    alpha_cutoff_bias: f32,
) -> Vec<Vec<u8>> {
    let strategy = match requested_strategy {
        MipmapStrategy::Auto if has_fully_transparent_texel(&level_zero) => MipmapStrategy::Cutout,
        MipmapStrategy::Auto => MipmapStrategy::Mean,
        strategy => strategy,
    };
    match strategy {
        MipmapStrategy::Cutout | MipmapStrategy::StrictCutout => {
            solidify_transparent_rgb(&mut level_zero, width, height)
        }
        MipmapStrategy::DarkCutout => {
            fill_transparent_rgb_with_dark_color(&mut level_zero, width, height)
        }
        MipmapStrategy::Auto | MipmapStrategy::Mean => {}
    }

    let cutout_ref = match strategy {
        MipmapStrategy::StrictCutout => 0.3,
        MipmapStrategy::Cutout | MipmapStrategy::DarkCutout => 0.5,
        MipmapStrategy::Auto | MipmapStrategy::Mean => 0.0,
    };
    let is_cutout = matches!(
        strategy,
        MipmapStrategy::Cutout | MipmapStrategy::StrictCutout | MipmapStrategy::DarkCutout
    );
    let original_coverage = if is_cutout {
        alpha_test_coverage(&level_zero, width, height, cutout_ref, 1.0)
    } else {
        0.0
    };

    let mut levels = vec![level_zero];
    let mut current_width = width;
    let mut current_height = height;
    for _ in 1..=mip_level {
        let next_width = current_width >> 1;
        let next_height = current_height >> 1;
        let previous = levels.last().unwrap();
        let mut next = vec![0; (next_width * next_height * 4) as usize];
        for y in 0..next_height {
            for x in 0..next_width {
                let mut pixels = [[0u8; 4]; 4];
                for (index, (ox, oy)) in [(0, 0), (1, 0), (0, 1), (1, 1)].into_iter().enumerate() {
                    let src = (((y * 2 + oy) * current_width + x * 2 + ox) * 4) as usize;
                    pixels[index].copy_from_slice(&previous[src..src + 4]);
                }
                let color = if strategy == MipmapStrategy::DarkCutout {
                    darkened_alpha_blend(pixels)
                } else {
                    mean_linear(pixels)
                };
                let dst = ((y * next_width + x) * 4) as usize;
                next[dst..dst + 4].copy_from_slice(&color);
            }
        }
        if is_cutout {
            scale_alpha_to_coverage(
                &mut next,
                next_width,
                next_height,
                original_coverage,
                cutout_ref,
                alpha_cutoff_bias,
            );
        }
        levels.push(next);
        current_width = next_width;
        current_height = next_height;
    }
    levels
}

fn has_fully_transparent_texel(data: &[u8]) -> bool {
    data.as_chunks::<4>().0.iter().any(|pixel| pixel[3] == 0)
}

fn srgb_to_linear_10(channel: u8) -> u16 {
    static TABLE: std::sync::OnceLock<[u16; 256]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        std::array::from_fn(|index| {
            let x = index as f32 / 255.0;
            let linear = if x >= 0.04045 {
                (((f64::from(x) + 0.055) / 1.055).powf(2.4)) as f32
            } else {
                x / 12.92
            };
            (linear * 1023.0).round() as u16
        })
    })[channel as usize]
}

fn linear_10_to_srgb(channel: u16) -> u8 {
    static TABLE: std::sync::OnceLock<[u8; 1024]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        std::array::from_fn(|index| {
            let x = index as f32 / 1023.0;
            let srgb = if x >= 0.0031308 {
                (1.055 * f64::from(x).powf(1.0 / 2.4) - 0.055) as f32
            } else {
                12.92 * x
            };
            (srgb * 255.0).round().clamp(0.0, 255.0) as u8
        })
    })[usize::from(channel)]
}

fn mean_linear(pixels: [[u8; 4]; 4]) -> [u8; 4] {
    let mut output = [0u8; 4];
    for channel in 0..3 {
        let linear_sum: u32 = pixels
            .iter()
            .map(|pixel| u32::from(srgb_to_linear_10(pixel[channel])))
            .sum();
        output[channel] = linear_10_to_srgb((linear_sum / 4) as u16);
    }
    output[3] = (pixels.iter().map(|pixel| u32::from(pixel[3])).sum::<u32>() / 4) as u8;
    output
}

fn darkened_alpha_blend(pixels: [[u8; 4]; 4]) -> [u8; 4] {
    let mut totals = [0.0f32; 4];
    for pixel in pixels {
        if pixel[3] == 0 {
            continue;
        }
        for channel in 0..4 {
            totals[channel] += f32::from(srgb_to_linear_10(pixel[channel])) / 1023.0;
        }
    }
    std::array::from_fn(|channel| {
        let linear = totals[channel] / 4.0;
        linear_10_to_srgb((linear * 1023.0).floor() as u16)
    })
}

fn alpha_test_coverage(
    data: &[u8],
    width: u32,
    height: u32,
    alpha_ref: f32,
    alpha_scale: f32,
) -> f32 {
    if width <= 1 || height <= 1 {
        return f32::NAN;
    }
    let mut coverage = 0.0;
    for y in 0..height - 1 {
        for x in 0..width - 1 {
            let alpha_at = |px: u32, py: u32| {
                let index = ((py * width + px) * 4 + 3) as usize;
                (f32::from(data[index]) / 255.0 * alpha_scale).clamp(0.0, 1.0)
            };
            let a00 = alpha_at(x, y);
            let a10 = alpha_at(x + 1, y);
            let a01 = alpha_at(x, y + 1);
            let a11 = alpha_at(x + 1, y + 1);
            let mut texel_coverage = 0.0;
            for sy in 0..4 {
                let fy = (sy as f32 + 0.5) / 4.0;
                for sx in 0..4 {
                    let fx = (sx as f32 + 0.5) / 4.0;
                    let alpha = a00 * (1.0 - fx) * (1.0 - fy)
                        + a10 * fx * (1.0 - fy)
                        + a01 * (1.0 - fx) * fy
                        + a11 * fx * fy;
                    if alpha > alpha_ref {
                        texel_coverage += 1.0;
                    }
                }
            }
            coverage += texel_coverage / 16.0;
        }
    }
    coverage / ((width - 1) * (height - 1)) as f32
}

fn scale_alpha_to_coverage(
    data: &mut [u8],
    width: u32,
    height: u32,
    desired_coverage: f32,
    alpha_ref: f32,
    alpha_cutoff_bias: f32,
) {
    let mut min_scale = 0.0;
    let mut max_scale = 4.0;
    let mut scale = 1.0;
    let mut best_scale = 1.0;
    let mut best_error = f32::MAX;
    for _ in 0..5 {
        let current_coverage = alpha_test_coverage(data, width, height, alpha_ref, scale);
        let error = (current_coverage - desired_coverage).abs();
        if error < best_error {
            best_error = error;
            best_scale = scale;
        }
        if current_coverage < desired_coverage {
            min_scale = scale;
        } else if current_coverage > desired_coverage {
            max_scale = scale;
        } else {
            break;
        }
        scale = (min_scale + max_scale) * 0.5;
    }
    for pixel in data.as_chunks_mut::<4>().0 {
        let alpha =
            (f32::from(pixel[3]) / 255.0 * best_scale + alpha_cutoff_bias + 0.025).clamp(0.0, 1.0);
        pixel[3] = (alpha * 255.0).floor() as u8;
    }
}

fn solidify_transparent_rgb(data: &mut [u8], width: u32, height: u32) {
    use std::collections::VecDeque;

    let len = (width * height) as usize;
    let mut nearest = vec![[0u8; 3]; len];
    let mut distances = vec![u32::MAX; len];
    let mut queue = VecDeque::new();
    for x in 0..width {
        for y in 0..height {
            let index = (y * width + x) as usize;
            let pixel = &data[index * 4..index * 4 + 4];
            if pixel[3] != 0 {
                distances[index] = 0;
                nearest[index].copy_from_slice(&pixel[..3]);
                queue.push_back((x, y));
            }
        }
    }
    while let Some((x, y)) = queue.pop_front() {
        let index = (y * width + x) as usize;
        for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
            let nx = x as i32 + dx;
            let ny = y as i32 + dy;
            if nx < 0 || ny < 0 || nx >= width as i32 || ny >= height as i32 {
                continue;
            }
            let neighbor = (ny as u32 * width + nx as u32) as usize;
            if distances[neighbor] <= distances[index] + 1 {
                continue;
            }
            distances[neighbor] = distances[index] + 1;
            nearest[neighbor] = nearest[index];
            queue.push_back((nx as u32, ny as u32));
        }
    }
    for index in 0..len {
        if data[index * 4 + 3] == 0 {
            data[index * 4..index * 4 + 3].copy_from_slice(&nearest[index]);
        }
    }
}

fn fill_transparent_rgb_with_dark_color(data: &mut [u8], width: u32, height: u32) {
    let mut darkest = [255u8; 3];
    let mut min_brightness = u32::MAX;
    for x in 0..width {
        for y in 0..height {
            let index = ((y * width + x) * 4) as usize;
            let pixel = &data[index..index + 4];
            if pixel[3] == 0 {
                continue;
            }
            let brightness = u32::from(pixel[0]) + u32::from(pixel[1]) + u32::from(pixel[2]);
            if brightness < min_brightness {
                min_brightness = brightness;
                darkest.copy_from_slice(&pixel[..3]);
            }
        }
    }
    let dark = [
        (u16::from(darkest[0]) * 3 / 4) as u8,
        (u16::from(darkest[1]) * 3 / 4) as u8,
        (u16::from(darkest[2]) * 3 / 4) as u8,
    ];
    for pixel in data.as_chunks_mut::<4>().0 {
        if pixel[3] == 0 {
            pixel[..3].copy_from_slice(&dark);
        }
    }
}

fn pixel_region(x: u32, y: u32, w: u32, h: u32, atlas_size: u32) -> AtlasRegion {
    let s = atlas_size as f32;
    let u_min = x as f32 / s;
    let v_min = y as f32 / s;
    let u_max = (x + w) as f32 / s;
    let v_max = (y + h) as f32 / s;
    AtlasRegion {
        // Vanilla maps the complete sprite rectangle. Terrain no longer needs
        // a synthetic inset because it carries the integer rectangle through
        // to the shader instead of quantising atlas-boundary UVs to u16.
        u_min,
        v_min,
        u_max,
        v_max,
        pixel_rect: [x as u16, y as u16, w as u16, h as u16],
        // Filled in by the caller from the sprite's texels; the missing tile is a
        // solid checker at index 0, so these are its values.
        sprite: 0,
        opaque: true,
        translucent: false,
    }
}

/// `(opaque, translucent)` over every texel of an RGBA image: opaque when all
/// alphas are 255, translucent when any alpha lies strictly between 0 and 255.
/// Conservative for the solid pass: any transparency routes the sprite to the
/// cutout pass, so a hole never renders solid.
fn sprite_transparency(data: &[u8]) -> (bool, bool) {
    data.as_chunks::<4>()
        .0
        .iter()
        .fold((true, false), |(opaque, translucent), px| {
            (
                opaque && px[3] == 255,
                translucent || (px[3] != 0 && px[3] != 255),
            )
        })
}

type PackResult = (HashMap<String, Option<(u32, u32)>>, AtlasRegion);

fn pack(sources: &[Source], atlas_size: u32, mip_align: u32) -> (PackResult, bool) {
    let mut placements: HashMap<String, Option<(u32, u32)>> = HashMap::new();
    let missing_region = pixel_region(0, 0, MISSING_TILE, MISSING_TILE, atlas_size);
    let mut cursor_x = MISSING_TILE;
    let mut cursor_y = 0;
    let mut shelf_h = MISSING_TILE;
    let mut all_fit = true;
    for src in sources {
        if src.data.is_empty() {
            placements.insert(src.name.clone(), None);
            continue;
        }
        if cursor_x + src.w > atlas_size {
            cursor_y = (cursor_y + shelf_h).next_multiple_of(mip_align);
            cursor_x = 0;
            shelf_h = 0;
        }
        if cursor_y + src.h > atlas_size {
            all_fit = false;
            placements.insert(src.name.clone(), None);
            continue;
        }
        placements.insert(src.name.clone(), Some((cursor_x, cursor_y)));
        cursor_x = (cursor_x + src.w).next_multiple_of(mip_align);
        shelf_h = shelf_h.max(src.h);
    }
    ((placements, missing_region), all_fit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::test_temp_dir;

    #[test]
    fn texture_ids_find_the_region_their_name_does() {
        let built = TextureId::intern("test/region_before_build");
        assert_eq!(TextureId::intern("test/region_before_build"), built);
        assert_eq!(built.name(), "test/region_before_build");

        let missing = pixel_region(0, 0, 1, 1, 16);
        let named = pixel_region(4, 4, 4, 4, 16);
        let late_named = pixel_region(8, 8, 4, 4, 16);
        let regions = HashMap::from([
            ("test/region_before_build".to_owned(), named),
            ("test/region_after_build".to_owned(), late_named),
        ]);
        let uv_map = AtlasUVMap::new(regions, HashMap::new(), Vec::new(), missing);
        // Interned after the map was built: found through the name instead.
        let late = TextureId::intern("test/region_after_build");
        assert_eq!(uv_map.region(built).pixel_rect, named.pixel_rect);
        assert_eq!(uv_map.region(late).pixel_rect, late_named.pixel_rect);
    }
    fn alpha_index(x: usize, y: usize, width: usize) -> usize {
        (y * width + x) * 4 + 3
    }

    #[test]
    fn pixel_region_uses_exact_sprite_bounds() {
        let region = pixel_region(32, 48, 16, 16, 256);
        assert_eq!(region.pixel_rect, [32, 48, 16, 16]);
        assert_eq!(region.u_min, 32.0 / 256.0);
        assert_eq!(region.v_min, 48.0 / 256.0);
        assert_eq!(region.u_max, 48.0 / 256.0);
        assert_eq!(region.v_max, 64.0 / 256.0);
    }

    fn solid_source(name: &str, width: u32, height: u32, color: [u8; 4]) -> Source {
        let mut data = vec![0; (width * height * 4) as usize];
        for pixel in data.as_chunks_mut::<4>().0 {
            pixel.copy_from_slice(&color);
        }
        let (opaque, translucent) = sprite_transparency(&data);
        Source {
            name: name.to_string(),
            data: data.clone(),
            w: width,
            h: height,
            opaque,
            translucent,
            alpha_mask: None,
            mip_source: Some(MipSource {
                full_data: data,
                full_width: width,
                full_height: height,
                animation: static_animation_layout(width, height),
                strategy: MipmapStrategy::Mean,
                alpha_cutoff_bias: 0.0,
            }),
            animation: None,
        }
    }

    #[test]
    fn mips_stay_within_sprite_regions() {
        const SIZE: u32 = 64;
        let mut sources = [
            solid_source("red", 16, 16, [255, 0, 0, 255]),
            solid_source("blue", 16, 16, [0, 0, 255, 255]),
        ];
        let placements = HashMap::from([
            ("red".to_string(), Some((16, 0))),
            ("blue".to_string(), Some((32, 0))),
        ]);
        let pixels = vec![0u8; (SIZE * SIZE * 4) as usize];
        let chain = build_mip_chain(pixels, SIZE, MAX_MIP_LEVEL, &mut sources, &placements);

        let mut offset = (SIZE * SIZE * 4) as usize;
        for level in 1..=MAX_MIP_LEVEL {
            let size = SIZE >> level;
            for (source, color) in sources.iter().zip([[255u8, 0, 0, 255], [0, 0, 255, 255]]) {
                let (x, y) = placements[&source.name].unwrap();
                let dx = x >> level;
                let dy = y >> level;
                let width = source.w >> level;
                let height = source.h >> level;
                for py in dy..dy + height {
                    for px in dx..dx + width {
                        let i = offset + ((py * size + px) * 4) as usize;
                        assert_eq!(&chain[i..i + 4], &color);
                    }
                }
            }
            offset += (size * size * 4) as usize;
        }
        assert_eq!(offset, chain.len());
    }

    #[test]
    fn sprite_transparency_follows_vanilla_alpha_classes() {
        assert_eq!(
            sprite_transparency(&[0, 0, 0, 255, 0, 0, 0, 255]),
            (true, false)
        );
        assert_eq!(
            sprite_transparency(&[0, 0, 0, 255, 0, 0, 0, 0]),
            (false, false)
        );
        assert_eq!(
            sprite_transparency(&[0, 0, 0, 255, 0, 0, 0, 128]),
            (false, true)
        );
    }

    #[test]
    fn mean_mipmap_uses_linear_rgb() {
        let pixels = [
            [0, 0, 0, 255],
            [255, 255, 255, 255],
            [0, 0, 0, 255],
            [255, 255, 255, 255],
        ]
        .concat();
        let levels = generate_rgba_mips(pixels, 2, 2, 1, MipmapStrategy::Mean, 0.0);
        assert_eq!(levels[1], vec![187, 187, 187, 255]);
    }

    #[test]
    fn cutout_mipmap_keeps_transparent_rgb_filled() {
        let mut pixels = vec![0u8; 4 * 4 * 4];
        for y in 0..4 {
            for x in 0..2 {
                let i = ((y * 4 + x) * 4) as usize;
                pixels[i..i + 4].copy_from_slice(&[0, 255, 0, 255]);
            }
        }
        let levels = generate_rgba_mips(pixels, 4, 4, 1, MipmapStrategy::Cutout, 0.0);
        assert!(
            levels[0]
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel[1] == 255)
        );
        assert!(
            levels[1]
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel[1] == 255)
        );
    }

    #[test]
    fn auto_cutout_solidifies_level_zero_even_without_extra_mips() {
        let pixels = vec![255, 0, 0, 255, 0, 0, 0, 0];
        let levels = generate_rgba_mips(pixels, 2, 1, 0, MipmapStrategy::Auto, 0.0);
        assert_eq!(&levels[0][4..8], &[255, 0, 0, 0]);
    }

    #[test]
    fn dark_cutout_uses_darkened_opaque_color_for_empty_texels() {
        let pixels = vec![100, 80, 40, 255, 0, 0, 0, 0];
        let levels = generate_rgba_mips(pixels, 2, 1, 0, MipmapStrategy::DarkCutout, 0.0);
        assert_eq!(&levels[0][4..8], &[75, 60, 30, 0]);
    }

    #[test]
    fn dark_cutout_blend_matches_vanilla_float_accumulation() {
        let pixels = [
            [112, 146, 45, 255],
            [80, 105, 44, 255],
            [112, 146, 45, 255],
            [80, 105, 44, 255],
        ];
        assert_eq!(darkened_alpha_blend(pixels), [97, 128, 44, 255]);
    }

    #[test]
    fn effective_mip_level_matches_vanilla_sprite_divisibility_limit() {
        let sixteen = solid_source("sixteen", 16, 16, [255; 4]);
        let eighteen = solid_source("eighteen", 18, 18, [255; 4]);
        assert_eq!(effective_mip_level(&[sixteen]), 4);
        assert_eq!(effective_mip_level(&[eighteen]), 1);

        let mut item_eighteen = solid_source("item/custom", 18, 18, [255; 4]);
        item_eighteen.mip_source = None;
        assert_eq!(effective_mip_level(&[item_eighteen]), MAX_MIP_LEVEL);

        let mut particle_eight = solid_source("particle/glitter_0", 8, 8, [255; 4]);
        particle_eight.mip_source = None;
        let block_sixteen = solid_source("dirt", 16, 16, [255; 4]);
        assert_eq!(
            effective_mip_level(&[block_sixteen, particle_eight]),
            MAX_MIP_LEVEL
        );
    }

    #[test]
    fn odd_sized_block_sprite_populates_only_supported_mip_levels() {
        const SIZE: u32 = 64;
        let mut source = solid_source("odd", 18, 18, [17, 34, 51, 255]);
        let mip_level = effective_mip_level(std::slice::from_ref(&source));
        assert_eq!(mip_level, 1);
        let placements = HashMap::from([("odd".to_string(), Some((16, 0)))]);
        let chain = build_mip_chain(
            vec![0; (SIZE * SIZE * 4) as usize],
            SIZE,
            mip_level,
            std::slice::from_mut(&mut source),
            &placements,
        );
        let level_one = &chain[(SIZE * SIZE * 4) as usize..];
        let level_one_size = SIZE >> 1;
        let expected = mean_linear([[17, 34, 51, 255]; 4]);
        for y in 0..9 {
            for x in 8..17 {
                let index = ((y * level_one_size + x) * 4) as usize;
                assert_eq!(&level_one[index..index + 4], &expected);
            }
        }
    }

    #[test]
    fn texture_metadata_parses_mipmap_strategy_and_bias() {
        let metadata: TextureMetadataFile = serde_json::from_str(
            r#"{"texture":{"mipmap_strategy":"strict_cutout","alpha_cutoff_bias":0.1}}"#,
        )
        .unwrap();
        let texture = metadata.texture.unwrap();
        assert_eq!(texture.mipmap_strategy, MipmapStrategy::StrictCutout);
        assert!((texture.alpha_cutoff_bias - 0.1).abs() < f32::EPSILON);

        let mut pixels = [0, 0, 0, 128].repeat(4);
        scale_alpha_to_coverage(&mut pixels, 2, 2, 1.0, 0.5, texture.alpha_cutoff_bias);
        assert!(
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel[3] == 159)
        );
    }

    /// A `width`-wide strip of one-pixel-tall frames, frame `n` filled with
    /// `[n, n, n, 255]`.
    fn numbered_strip(width: u32, frames: u32) -> Vec<u8> {
        (0..frames)
            .flat_map(|frame| {
                (0..width).flat_map(move |_| [frame as u8, frame as u8, frame as u8, 255])
            })
            .collect()
    }

    fn animated_sprite(frames: Vec<(usize, u32)>, interpolate: bool) -> AnimatedSprite {
        AnimatedSprite {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            animation: SpriteAnimation {
                frames,
                interpolate,
                pixels: vec![vec![vec![0, 0, 0, 255]], vec![vec![255, 255, 255, 255]]],
            },
            frame: 0,
            sub_frame: 0,
            dirty: false,
        }
    }

    #[test]
    fn animation_frames_take_their_time_or_the_default() {
        let metadata = AnimationMetadata {
            width: Some(1),
            height: Some(1),
            frames: Some(vec![
                AnimationFrameSpec::Index(1),
                AnimationFrameSpec::Timed {
                    index: 0,
                    time: Some(5),
                },
                AnimationFrameSpec::Timed {
                    index: 9,
                    time: None,
                },
            ]),
            frametime: Some(3),
            interpolate: Some(true),
        };
        let layout = animation_layout_from_metadata(Some(&metadata), 1, 2).unwrap();
        assert_eq!(
            layout.frames,
            [
                AnimationFrame { index: 1, time: 3 },
                AnimationFrame { index: 0, time: 5 },
            ]
        );
        assert!(layout.interpolate);

        let unlisted = AnimationMetadata {
            frames: None,
            frametime: None,
            interpolate: None,
            ..metadata
        };
        let layout = animation_layout_from_metadata(Some(&unlisted), 1, 2).unwrap();
        assert_eq!(
            layout.frames,
            [
                AnimationFrame { index: 0, time: 1 },
                AnimationFrame { index: 1, time: 1 },
            ]
        );
        assert!(!layout.interpolate);
    }

    #[test]
    fn sprite_animation_stores_each_unique_frame_once() {
        let metadata = AnimationMetadata {
            width: Some(1),
            height: Some(1),
            frames: Some(vec![
                AnimationFrameSpec::Index(2),
                AnimationFrameSpec::Index(0),
                AnimationFrameSpec::Index(2),
            ]),
            frametime: Some(4),
            interpolate: None,
        };
        let layout = animation_layout_from_metadata(Some(&metadata), 1, 3).unwrap();
        let animation = SpriteAnimation::new(&numbered_strip(1, 3), 1, &layout).unwrap();
        assert_eq!(animation.frames, [(0, 4), (1, 4), (0, 4)]);
        assert_eq!(
            animation.pixels,
            [vec![vec![2, 2, 2, 255]], vec![vec![0, 0, 0, 255]]]
        );

        let still = static_animation_layout(1, 1);
        assert!(SpriteAnimation::new(&numbered_strip(1, 1), 1, &still).is_none());
    }

    #[test]
    fn animation_tick_matches_vanilla_animation_state() {
        // Frames 0 (2 ticks), 1, 1, 0: a repeated frame index isn't a change.
        let mut sprite = animated_sprite(vec![(0, 2), (1, 1), (1, 1), (0, 1)], false);
        sprite.tick();
        assert_eq!(
            (sprite.frame, sprite.sub_frame, sprite.dirty),
            (0, 1, false)
        );
        sprite.tick();
        assert_eq!((sprite.frame, sprite.sub_frame, sprite.dirty), (1, 0, true));
        sprite.dirty = false;
        sprite.tick();
        assert_eq!((sprite.frame, sprite.dirty), (2, false));
        sprite.tick();
        assert_eq!((sprite.frame, sprite.dirty), (3, true));
        sprite.dirty = false;
        sprite.tick();
        assert_eq!((sprite.frame, sprite.dirty), (0, false));
        assert!(!sprite.needs_upload());
    }

    #[test]
    fn interpolated_frames_mix_by_thousandths_of_progress() {
        let mut sprite = animated_sprite(vec![(0, 3), (1, 3)], true);
        assert!(sprite.needs_upload());
        let mut scratch = Vec::new();
        assert_eq!(sprite.level_pixels(0, &mut scratch), [0, 0, 0, 255]);
        // 1/3 packs as 333 thousandths: 0.333 * 255 = 84.9.
        sprite.tick();
        assert_eq!(sprite.level_pixels(0, &mut scratch), [85, 85, 85, 255]);
        sprite.tick();
        assert_eq!(sprite.level_pixels(0, &mut scratch), [170, 170, 170, 255]);
        // The last frame mixes back toward the first.
        sprite.tick();
        sprite.tick();
        assert_eq!(sprite.level_pixels(0, &mut scratch), [170, 170, 170, 255]);
    }

    #[test]
    fn mipped_animation_levels_come_from_the_strip_mips() {
        let metadata = AnimationMetadata {
            width: Some(4),
            height: Some(4),
            frames: None,
            frametime: None,
            interpolate: None,
        };
        let layout = animation_layout_from_metadata(Some(&metadata), 4, 8).unwrap();
        // Frame 0 red, frame 1 blue.
        let strip: Vec<u8> = [[255, 0, 0, 255], [0, 0, 255, 255]]
            .into_iter()
            .flat_map(|color| std::iter::repeat_n(color, 16).flatten())
            .collect();
        let mut source = solid_source("anim", 4, 4, [255, 0, 0, 255]);
        let mip_source = source.mip_source.as_mut().unwrap();
        mip_source.full_data = strip.clone();
        mip_source.full_height = 8;
        mip_source.animation = layout.clone();
        source.animation = SpriteAnimation::new(&strip, 4, &layout);
        let mut sources = [source];
        let placements = HashMap::from([("anim".to_string(), Some((0, 0)))]);
        build_mip_chain(vec![0; 16 * 16 * 4], 16, 2, &mut sources, &placements);

        let pixels = &sources[0].animation.as_ref().unwrap().pixels;
        assert_eq!(pixels[1].len(), 3);
        assert_eq!(pixels[1][1], [0, 0, 255, 255].repeat(4));
        assert_eq!(pixels[1][2], [0, 0, 255, 255]);
    }

    #[test]
    fn explicit_animation_initial_display_uses_first_listed_frame() {
        let metadata = AnimationMetadata {
            width: Some(2),
            height: Some(1),
            frames: Some(vec![
                AnimationFrameSpec::Index(2),
                AnimationFrameSpec::Index(0),
            ]),
            frametime: None,
            interpolate: None,
        };
        let layout = animation_layout_from_metadata(Some(&metadata), 4, 2).unwrap();
        assert_eq!(layout.initial_display_frame, 2);

        // Horizontal 2x1 frames laid out as [0, 1] / [2, 3]. Frame 2 starts
        // at the first two pixels of the second row.
        let mut rgba = vec![0_u8; 4 * 2 * 4];
        let frame_two_offset = 4 * 4;
        rgba[frame_two_offset..frame_two_offset + 4].copy_from_slice(&[9, 8, 7, 255]);
        let frame = extract_frame_rgba(&rgba, 4, &layout, layout.initial_display_frame);
        assert_eq!(&frame[..4], &[9, 8, 7, 255]);
    }

    #[test]
    fn generated_sprite_geometry_unions_unique_animation_frames() {
        let metadata = AnimationMetadata {
            width: None,
            height: None,
            frames: Some(vec![
                AnimationFrameSpec::Index(2),
                AnimationFrameSpec::Timed {
                    index: 0,
                    time: Some(5),
                },
                AnimationFrameSpec::Index(2),
            ]),
            frametime: None,
            interpolate: None,
        };
        let layout = animation_layout_from_metadata(Some(&metadata), 2, 6).unwrap();
        assert_eq!(layout.frame_width, 2);
        assert_eq!(layout.frame_height, 2);
        assert_eq!(layout.unique_frames, vec![2, 0]);
        assert_eq!(layout.initial_display_frame, 2);

        // Three vertically stacked 2x2 frames. Frame 2 contributes the
        // top-left pixel, frame 0 contributes the bottom-right pixel; repeated
        // frame 2 must not matter. The union mirrors vanilla getUniqueFrames().
        let mut data = vec![0u8; 2 * 6 * 4];
        data[(4 * 2 * 4) + 3] = 255;
        data[alpha_index(1, 1, 2)] = 255;
        let mask = sprite_alpha_mask_from_rgba(&data, 2, &layout);
        assert!(mask.is_opaque_in_any_frame(0, 0));
        assert!(mask.is_opaque_in_any_frame(1, 1));
        assert!(!mask.is_opaque_in_any_frame(1, 0));
        assert!(!mask.is_opaque_in_any_frame(0, 1));
    }

    #[test]
    fn generated_sprite_edges_are_unioned_per_animation_frame() {
        let mask = SpriteAlphaMask {
            width: 2,
            height: 1,
            frames: vec![vec![true, false], vec![false, true]],
        };

        // Each source pixel is exposed toward the other in one animation
        // frame. OR-ing the frames into one opacity bitmap would incorrectly
        // erase both of these vanilla side faces.
        assert!(mask.has_exposed_edge(0, 0, 1, 0));
        assert!(mask.has_exposed_edge(1, 0, 0, 0));
    }

    #[test]
    fn single_valid_animation_frame_falls_back_to_top_left_frame() {
        let metadata = AnimationMetadata {
            width: None,
            height: None,
            frames: Some(vec![AnimationFrameSpec::Index(2)]),
            frametime: None,
            interpolate: None,
        };
        let layout = animation_layout_from_metadata(Some(&metadata), 2, 6).unwrap();

        // Vanilla SpriteContents discards AnimatedTexture when <=1 valid frame
        // remains, so isTransparent() samples the original top-left frame.
        assert_eq!(layout.frame_width, 2);
        assert_eq!(layout.frame_height, 2);
        assert_eq!(layout.unique_frames, vec![0]);
        assert_eq!(layout.initial_display_frame, 0);
    }

    #[test]
    fn explicit_empty_animation_frames_are_not_treated_as_implicit_all_frames() {
        let metadata = AnimationMetadata {
            width: None,
            height: None,
            frames: Some(Vec::new()),
            frametime: None,
            interpolate: None,
        };
        let layout = animation_layout_from_metadata(Some(&metadata), 2, 6).unwrap();

        // Optional.empty means implicit sequential frames; Optional.of([])
        // reaches SpriteContents with zero frames and therefore becomes static.
        assert_eq!(layout.unique_frames, vec![0]);
    }

    #[test]
    fn generated_sprite_uses_same_resource_pack_source_as_atlas() {
        let root = test_temp_dir("item_pack_source");
        let jar_assets = root.join("jar");
        let instance = root.join("instance");
        let base_dir = jar_assets.join("minecraft/textures/item");
        let pack_dir = instance.join("resourcepacks/test_pack/assets/minecraft/textures/item");
        std::fs::create_dir_all(&base_dir).unwrap();
        std::fs::create_dir_all(&pack_dir).unwrap();
        std::fs::write(
            instance.join("resourcepacks/test_pack/pack.mcmeta"),
            r#"{"pack":{"pack_format":84,"description":"test"}}"#,
        )
        .unwrap();

        let mut base = image::RgbaImage::new(2, 4);
        base.put_pixel(0, 0, image::Rgba([255, 255, 255, 255]));
        let base_path = base_dir.join("cocoa_beans.png");
        base.save(&base_path).unwrap();
        std::fs::write(
            base_path.with_extension("png.mcmeta"),
            r#"{"animation":{"width":2,"height":2}}"#,
        )
        .unwrap();

        // The higher-priority pack replaces only the PNG. Vanilla must not
        // inherit animation metadata from the lower resource that it replaced.
        let pack_path = pack_dir.join("cocoa_beans.png");
        let mut replacement = image::RgbaImage::new(2, 4);
        replacement.put_pixel(1, 3, image::Rgba([255, 255, 255, 255]));
        replacement.save(&pack_path).unwrap();

        let mut packs = crate::resource_pack::ResourcePackManager::new(&instance);
        packs.enable_local_pack("test_pack");
        let source = load_source("item/cocoa_beans", &jar_assets, &None, Some(&packs), true);
        let mask = source.alpha_mask.as_ref().unwrap();
        assert_eq!((source.w, source.h), (2, 4));
        assert!(!mask.is_opaque_in_any_frame(0, 0));
        assert!(mask.is_opaque_in_any_frame(1, 3));
        assert_eq!(source.data[alpha_index(1, 3, 2)], 255);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cross_namespace_texture_keys_resolve_without_minecraft_rewrite() {
        assert_eq!(
            atlas_asset_path("other:block/custom"),
            "other/textures/block/custom.png"
        );
        assert_eq!(
            atlas_asset_path("other:item/custom"),
            "other/textures/item/custom.png"
        );
        assert!(!uses_block_atlas_mip_chain("item/cocoa_beans"));
        assert!(!uses_block_atlas_mip_chain("particle/glitter_0"));
        assert!(uses_block_atlas_mip_chain("sculk_vein"));
        assert!(uses_block_atlas_mip_chain("other:block/custom"));
        // `atlases/blocks.json` lists three entity sprites; the chest sheet
        // is its own unmipmapped atlas.
        assert!(uses_block_atlas_mip_chain("entity/bell/bell_body"));
        assert!(uses_block_atlas_mip_chain("entity/conduit/base"));
        assert!(!uses_block_atlas_mip_chain("entity/chest/normal"));
    }
}
