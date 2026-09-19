use std::collections::HashMap;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::Value;

use crate::assets::{self, AssetId, AssetIndex};
use crate::chat_component::normalize_identifier;
use crate::renderer::pipelines::menu_overlay::ATLAS_CELL;
use crate::resource_pack::ResourcePackManager;

#[derive(Clone, Debug)]
enum Recipe {
    Direct(String),
    Unstitch {
        resource: String,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        divisor_x: f64,
        divisor_y: f64,
    },
    Paletted {
        texture: String,
        palette_key: String,
        palette_value: String,
    },
}

struct AssetLookup<'a> {
    jar_assets_dir: &'a Path,
    asset_index: &'a Option<AssetIndex>,
    packs: &'a ResourcePackManager,
}

impl AssetLookup<'_> {
    fn resolve(&self, key: &str) -> PathBuf {
        assets::resolve_asset_path_with_packs(
            self.jar_assets_dir,
            self.asset_index,
            key,
            Some(self.packs),
        )
    }

    fn texture_exists(&self, resource: &str) -> bool {
        self.resolve(&texture_key(resource)).exists()
    }

    fn load_texture(&self, resource: &str) -> Option<(Vec<u8>, u32, u32)> {
        crate::renderer::util::load_png(&self.resolve(&texture_key(resource)))
    }

    /// Every copy of the atlas definition, base assets first, so later packs'
    /// sources apply on top. A server-sent atlas id that escapes the assets
    /// directory resolves to nothing.
    fn atlas_definition_stack(&self, atlas_key: &str) -> Vec<PathBuf> {
        assets::resource_stack_paths(
            self.jar_assets_dir,
            self.asset_index,
            atlas_key,
            Some(self.packs),
        )
    }

    /// The sprite's frames: one for a plain texture, the animation's strip
    /// otherwise (`SpriteContents`).
    fn load_direct_sprite(&self, resource: &str) -> Option<SpriteFrames> {
        let (rgba, w, h) = self.load_texture(resource)?;
        let animation = self.animation_meta(resource);
        let (frame_w, frame_h) = match &animation {
            Some(animation) => animation.frame_size(w, h),
            None => (w, h),
        };
        let columns = (w / frame_w.max(1)).max(1);
        let rows = (h / frame_h.max(1)).max(1);
        let frame_count = (columns * rows) as usize;
        let sequence = match &animation {
            Some(animation) => animation.sequence(frame_count),
            None => vec![(0, 1)],
        };
        let mut frames = Vec::new();
        for index in 0..frame_count {
            let x = (index as u32 % columns) * frame_w;
            let y = (index as u32 / columns) * frame_h;
            let (pixels, fw, fh) = crop_rgba(&rgba, w, h, x, y, frame_w, frame_h)?;
            frames.push((pixels, fw, fh));
        }
        SpriteFrames::new(frames, sequence)
    }

    fn animation_meta(&self, resource: &str) -> Option<AnimationMeta> {
        let path = self.resolve(&format!("{}.mcmeta", texture_key(resource)));
        let text = std::fs::read_to_string(path).ok()?;
        let value = serde_json::from_str::<Value>(&text).ok()?;
        AnimationMeta::parse(value.get("animation")?)
    }
}

fn texture_key(resource: &str) -> String {
    AssetId::parse(resource).asset_key("textures", ".png")
}

/// A sprite's animation metadata (vanilla `AnimationMetadataSection`).
struct AnimationMeta {
    frame_width: Option<u32>,
    frame_height: Option<u32>,
    frame_time: u32,
    /// The declared frame order, empty when the file lists none.
    frames: Vec<(usize, Option<u32>)>,
}

impl AnimationMeta {
    fn parse(value: &Value) -> Option<Self> {
        let map = value.as_object()?;
        let positive = |key: &str| {
            map.get(key)
                .and_then(Value::as_u64)
                .map(|value| value as u32)
                .filter(|value| *value > 0)
        };
        // TODO: `interpolate` blends consecutive frames; Pomme steps them.
        let frames = map
            .get("frames")
            .and_then(Value::as_array)
            .map(|frames| {
                frames
                    .iter()
                    .filter_map(|frame| match frame {
                        Value::Number(index) => Some((index.as_u64()? as usize, None)),
                        Value::Object(frame) => Some((
                            frame.get("index").and_then(Value::as_u64)? as usize,
                            frame.get("time").and_then(Value::as_u64).map(|t| t as u32),
                        )),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Self {
            frame_width: positive("width"),
            frame_height: positive("height"),
            frame_time: positive("frametime").unwrap_or(1),
            frames,
        })
    }

    /// `FrameSize`: the declared size, else the square of the shorter side.
    fn frame_size(&self, w: u32, h: u32) -> (u32, u32) {
        match (self.frame_width, self.frame_height) {
            (Some(fw), Some(fh)) => (fw, fh),
            (Some(fw), None) => (fw, h),
            (None, Some(fh)) => (w, fh),
            (None, None) => {
                let min = w.min(h);
                (min, min)
            }
        }
    }

    /// The frames to play, in order, with their durations in ticks.
    fn sequence(&self, frame_count: usize) -> Vec<(usize, u32)> {
        let listed: Vec<(usize, u32)> = self
            .frames
            .iter()
            .filter(|(index, _)| *index < frame_count)
            .map(|(index, time)| (*index, time.unwrap_or(self.frame_time).max(1)))
            .collect();
        if listed.is_empty() {
            (0..frame_count)
                .map(|index| (index, self.frame_time.max(1)))
                .collect()
        } else {
            listed
        }
    }
}

/// A loaded sprite: square RGBA tiles at their native resolution (capped at
/// the atlas cell), with the animation that steps them.
pub struct SpriteFrames {
    pub size: u32,
    frames: Vec<Vec<u8>>,
    sequence: Vec<(usize, u32)>,
}

impl SpriteFrames {
    fn new(frames: Vec<(Vec<u8>, u32, u32)>, sequence: Vec<(usize, u32)>) -> Option<Self> {
        let (_, w, h) = *frames.first()?;
        // Vanilla maps the sprite's whole UV range onto the 8-unit quad, so a
        // non-square sprite stretches; the cell caps the resolution.
        let size = w.max(h).min(ATLAS_CELL);
        if size == 0 {
            return None;
        }
        let frames = frames
            .into_iter()
            .map(|(pixels, w, h)| {
                if w == size && h == size {
                    pixels
                } else {
                    resample_square(&pixels, w, h, size)
                }
            })
            .collect();
        Some(Self {
            size,
            frames,
            sequence,
        })
    }

    /// A single-frame sprite from ready-made pixels.
    pub fn still(pixels: Vec<u8>, size: u32) -> Self {
        Self {
            size,
            frames: vec![pixels],
            sequence: vec![(0, 1)],
        }
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    pub fn pixels(&self, frame: usize) -> &[u8] {
        &self.frames[frame.min(self.frames.len() - 1)]
    }

    /// The frame showing at `tick` (`SpriteContents.Ticker.tickAndUpload`).
    pub fn frame_at(&self, tick: u64) -> usize {
        let total: u64 = self.sequence.iter().map(|(_, time)| u64::from(*time)).sum();
        if total == 0 {
            return 0;
        }
        let mut remaining = tick % total;
        for (index, time) in &self.sequence {
            let time = u64::from(*time);
            if remaining < time {
                return *index;
            }
            remaining -= time;
        }
        0
    }

    pub fn animated(&self) -> bool {
        self.sequence.len() > 1
    }
}

/// `AtlasManager.KNOWN_ATLASES`: any other atlas id has no glyph provider.
pub fn is_known_atlas(atlas: &str) -> bool {
    const KNOWN: [&str; 13] = [
        "minecraft:armor_trims",
        "minecraft:banner_patterns",
        "minecraft:blocks",
        "minecraft:items",
        "minecraft:chests",
        "minecraft:decorated_pot",
        "minecraft:gui",
        "minecraft:map_decorations",
        "minecraft:paintings",
        "minecraft:particles",
        "minecraft:shield_patterns",
        "minecraft:shulker_boxes",
        "minecraft:celestials",
    ];
    KNOWN.contains(&normalize_identifier(atlas).as_str())
}

pub fn load_atlas_sprite(
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    packs: &ResourcePackManager,
    atlas: &str,
    sprite: &str,
) -> Option<SpriteFrames> {
    let lookup = AssetLookup {
        jar_assets_dir,
        asset_index,
        packs,
    };
    let sprite = AssetId::parse(sprite);
    let mut candidate: Option<Recipe> = None;

    let atlas_key = AssetId::parse(atlas).asset_key("atlases", ".json");
    for path in lookup.atlas_definition_stack(&atlas_key) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            tracing::warn!(
                "Could not parse inline-object atlas definition {}",
                path.display()
            );
            continue;
        };
        let Some(sources) = value.get("sources").and_then(Value::as_array) else {
            continue;
        };
        for source in sources {
            apply_source(source, sprite, &lookup, &mut candidate);
        }
    }

    let image = match candidate? {
        Recipe::Direct(resource) => return lookup.load_direct_sprite(&resource),
        Recipe::Unstitch {
            resource,
            x,
            y,
            width,
            height,
            divisor_x,
            divisor_y,
        } => {
            let (rgba, w, h) = lookup.load_texture(&resource)?;
            crop_unstitch(&rgba, w, h, x, y, width, height, divisor_x, divisor_y)?
        }
        Recipe::Paletted {
            texture,
            palette_key,
            palette_value,
        } => {
            let (base, w, h) = lookup.load_texture(&texture)?;
            let (key, kw, kh) = lookup.load_texture(&palette_key)?;
            let (value, vw, vh) = lookup.load_texture(&palette_value)?;
            if kw != vw || kh != vh {
                tracing::warn!(
                    "Inline-object palette dimensions differ for {palette_key} and {palette_value}"
                );
                return None;
            }
            (apply_palette(base, &key, &value), w, h)
        }
    };
    SpriteFrames::new(vec![image], vec![(0, 1)])
}

/// `MissingTextureAtlasSprite.generateMissingImage`: black except the
/// top-right and bottom-left quadrants.
pub fn missing_tile() -> (Vec<u8>, u32) {
    const SIZE: usize = 16;
    let mut out = vec![0u8; SIZE * SIZE * 4];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let off = (y * SIZE + x) * 4;
            let rgb = if (y < SIZE / 2) ^ (x < SIZE / 2) {
                [0xf8, 0x00, 0xf8]
            } else {
                [0, 0, 0]
            };
            out[off..off + 3].copy_from_slice(&rgb);
            out[off + 3] = 255;
        }
    }
    (out, SIZE as u32)
}

fn apply_source(
    source: &Value,
    sprite: AssetId<'_>,
    lookup: &AssetLookup<'_>,
    candidate: &mut Option<Recipe>,
) {
    let Some(map) = source.as_object() else {
        return;
    };
    let kind = map
        .get("type")
        .and_then(Value::as_str)
        .map(assets::strip_default_namespace);
    match kind {
        Some("directory") => {
            let Some(prefix) = map.get("prefix").and_then(Value::as_str) else {
                return;
            };
            let Some(source_path) = map.get("source").and_then(Value::as_str) else {
                return;
            };
            let Some(rest) = sprite.path.strip_prefix(prefix) else {
                return;
            };
            let path = join_resource_path(source_path, rest);
            let resource = format!("{}:{path}", sprite.namespace);
            if lookup.texture_exists(&resource) {
                *candidate = Some(Recipe::Direct(resource));
            }
        }
        Some("single") => {
            let Some(resource) = map.get("resource").and_then(Value::as_str) else {
                return;
            };
            let target = map
                .get("sprite")
                .and_then(Value::as_str)
                .unwrap_or(resource);
            if AssetId::parse(target) == sprite && lookup.texture_exists(resource) {
                *candidate = Some(Recipe::Direct(normalize_identifier(resource)));
            }
        }
        Some("filter") => {
            let Some(pattern) = map.get("pattern").and_then(Value::as_object) else {
                return;
            };
            let namespace_matches = pattern
                .get("namespace")
                .and_then(Value::as_str)
                .is_none_or(|pattern| regex_matches(pattern, sprite.namespace));
            let path_matches = pattern
                .get("path")
                .and_then(Value::as_str)
                .is_none_or(|pattern| regex_matches(pattern, sprite.path));
            if namespace_matches && path_matches {
                *candidate = None;
            }
        }
        Some("unstitch") => {
            let Some(resource) = map.get("resource").and_then(Value::as_str) else {
                return;
            };
            if !lookup.texture_exists(resource) {
                return;
            }
            let divisor_x = map.get("divisor_x").and_then(Value::as_f64).unwrap_or(1.0);
            let divisor_y = map.get("divisor_y").and_then(Value::as_f64).unwrap_or(1.0);
            let Some(regions) = map.get("regions").and_then(Value::as_array) else {
                return;
            };
            for region in regions {
                let Some(region) = region.as_object() else {
                    continue;
                };
                let Some(target) = region.get("sprite").and_then(Value::as_str) else {
                    continue;
                };
                if AssetId::parse(target) != sprite {
                    continue;
                }
                let (Some(x), Some(y), Some(width), Some(height)) = (
                    region.get("x").and_then(Value::as_f64),
                    region.get("y").and_then(Value::as_f64),
                    region.get("width").and_then(Value::as_f64),
                    region.get("height").and_then(Value::as_f64),
                ) else {
                    continue;
                };
                *candidate = Some(Recipe::Unstitch {
                    resource: normalize_identifier(resource),
                    x,
                    y,
                    width,
                    height,
                    divisor_x,
                    divisor_y,
                });
            }
        }
        Some("paletted_permutations") => {
            let Some(textures) = map.get("textures").and_then(Value::as_array) else {
                return;
            };
            let Some(palette_key) = map.get("palette_key").and_then(Value::as_str) else {
                return;
            };
            let Some(permutations) = map.get("permutations").and_then(Value::as_object) else {
                return;
            };
            let separator = map.get("separator").and_then(Value::as_str).unwrap_or("_");
            for texture in textures.iter().filter_map(Value::as_str) {
                let source = AssetId::parse(texture);
                if source.namespace != sprite.namespace {
                    continue;
                }
                for (suffix, palette_value) in permutations {
                    let Some(palette_value) = palette_value.as_str() else {
                        continue;
                    };
                    if sprite.path == format!("{}{separator}{suffix}", source.path)
                        && lookup.texture_exists(texture)
                    {
                        *candidate = Some(Recipe::Paletted {
                            texture: normalize_identifier(texture),
                            palette_key: normalize_identifier(palette_key),
                            palette_value: normalize_identifier(palette_value),
                        });
                    }
                }
            }
        }
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn crop_unstitch(
    rgba: &[u8],
    w: u32,
    h: u32,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    divisor_x: f64,
    divisor_y: f64,
) -> Option<(Vec<u8>, u32, u32)> {
    if divisor_x == 0.0 || divisor_y == 0.0 {
        return None;
    }
    let sx = (x * f64::from(w) / divisor_x).floor().max(0.0) as u32;
    let sy = (y * f64::from(h) / divisor_y).floor().max(0.0) as u32;
    let sw = (width * f64::from(w) / divisor_x).floor().max(0.0) as u32;
    let sh = (height * f64::from(h) / divisor_y).floor().max(0.0) as u32;
    crop_rgba(rgba, w, h, sx, sy, sw, sh)
}

fn crop_rgba(
    rgba: &[u8],
    w: u32,
    h: u32,
    x: u32,
    y: u32,
    cw: u32,
    ch: u32,
) -> Option<(Vec<u8>, u32, u32)> {
    if cw == 0 || ch == 0 || x >= w || y >= h || x + cw > w || y + ch > h {
        return None;
    }
    let mut out = vec![0u8; (cw * ch * 4) as usize];
    for row in 0..ch {
        let src = (((y + row) * w + x) * 4) as usize;
        let dst = (row * cw * 4) as usize;
        let len = (cw * 4) as usize;
        out[dst..dst + len].copy_from_slice(rgba.get(src..src + len)?);
    }
    Some((out, cw, ch))
}

fn apply_palette(mut base: Vec<u8>, key: &[u8], value: &[u8]) -> Vec<u8> {
    let mut palette = HashMap::<[u8; 3], [u8; 4]>::new();
    for (key, value) in key.as_chunks::<4>().0.iter().zip(value.as_chunks::<4>().0) {
        if key[3] == 0 {
            continue;
        }
        palette.insert(
            [key[0], key[1], key[2]],
            [value[0], value[1], value[2], value[3]],
        );
    }
    for pixel in base.as_chunks_mut::<4>().0 {
        if pixel[3] == 0 {
            continue;
        }
        if let Some(mapped) = palette.get(&[pixel[0], pixel[1], pixel[2]]) {
            let alpha = (u16::from(pixel[3]) * u16::from(mapped[3]) / 255) as u8;
            pixel[0] = mapped[0];
            pixel[1] = mapped[1];
            pixel[2] = mapped[2];
            pixel[3] = alpha;
        }
    }
    base
}

/// Nearest-neighbour resample onto a square tile, which is how a non-square
/// sprite ends up stretched over the glyph's square quad.
fn resample_square(rgba: &[u8], w: u32, h: u32, size: u32) -> Vec<u8> {
    let mut out = vec![0u8; (size * size * 4) as usize];
    if w == 0 || h == 0 {
        return out;
    }
    for y in 0..size {
        for x in 0..size {
            let sx = (x * w / size).min(w - 1);
            let sy = (y * h / size).min(h - 1);
            let src = ((sy * w + sx) * 4) as usize;
            let dst = ((y * size + x) * 4) as usize;
            if let Some(pixel) = rgba.get(src..src + 4) {
                out[dst..dst + 4].copy_from_slice(pixel);
            }
        }
    }
    out
}

fn join_resource_path(prefix: &str, suffix: &str) -> String {
    match (prefix.ends_with('/'), suffix.starts_with('/')) {
        (true, true) => format!("{}{}", prefix, &suffix[1..]),
        (false, false) if !prefix.is_empty() && !suffix.is_empty() => format!("{prefix}/{suffix}"),
        _ => format!("{prefix}{suffix}"),
    }
}

fn regex_matches(pattern: &str, value: &str) -> bool {
    Regex::new(pattern).is_ok_and(|regex| regex.is_match(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampling_preserves_four_corner_colors() {
        let pixels = vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let out = resample_square(&pixels, 2, 2, 8);
        assert_eq!(&out[0..4], &[255, 0, 0, 255]);
        assert_eq!(&out[(7 * 4)..(8 * 4)], &[0, 255, 0, 255]);
        let bottom_left = ((7 * 8) * 4) as usize;
        assert_eq!(&out[bottom_left..bottom_left + 4], &[0, 0, 255, 255]);
        assert_eq!(&out[out.len() - 4..], &[255, 255, 255, 255]);
    }

    #[test]
    fn only_the_known_atlases_resolve() {
        assert!(is_known_atlas("blocks"));
        assert!(is_known_atlas("minecraft:gui"));
        assert!(is_known_atlas("minecraft:celestials"));
        assert!(!is_known_atlas("pomme:custom"));
        assert!(!is_known_atlas("minecraft:mob_effects"));
    }

    #[test]
    fn animation_frames_step_with_the_tick() {
        let meta = AnimationMeta::parse(&serde_json::json!({"frametime": 2})).unwrap();
        assert_eq!(meta.frame_size(16, 48), (16, 16));
        let frames = SpriteFrames {
            size: 16,
            frames: vec![Vec::new(); 3],
            sequence: meta.sequence(3),
        };
        // Three frames of two ticks each, looping.
        assert_eq!(frames.frame_at(0), 0);
        assert_eq!(frames.frame_at(1), 0);
        assert_eq!(frames.frame_at(2), 1);
        assert_eq!(frames.frame_at(5), 2);
        assert_eq!(frames.frame_at(6), 0);
        assert!(frames.animated());

        // A declared order, with a per-frame time.
        let meta =
            AnimationMeta::parse(&serde_json::json!({"frames": [2, {"index": 0, "time": 3}]}))
                .unwrap();
        let frames = SpriteFrames {
            size: 16,
            frames: vec![Vec::new(); 3],
            sequence: meta.sequence(3),
        };
        assert_eq!(frames.frame_at(0), 2);
        assert_eq!(frames.frame_at(1), 0);
        assert_eq!(frames.frame_at(3), 0);
        assert_eq!(frames.frame_at(4), 2);

        // A still sprite never steps.
        let frames = SpriteFrames {
            size: 16,
            frames: vec![Vec::new()],
            sequence: vec![(0, 1)],
        };
        assert!(!frames.animated());
        assert_eq!(frames.frame_at(7), 0);
    }

    #[test]
    fn palette_mapping_multiplies_alpha_like_vanilla() {
        let base = vec![10, 20, 30, 128];
        let key = vec![10, 20, 30, 255];
        let value = vec![90, 80, 70, 128];
        assert_eq!(apply_palette(base, &key, &value), vec![90, 80, 70, 64]);
    }

    #[test]
    fn missing_tile_matches_missingno() {
        let (tile, size) = missing_tile();
        assert_eq!(size, 16);
        let pixel = |x: usize, y: usize| {
            let off = (y * 16 + x) * 4;
            [tile[off], tile[off + 1], tile[off + 2], tile[off + 3]]
        };
        // Black top-left and bottom-right, magenta on the other diagonal.
        assert_eq!(pixel(0, 0), [0, 0, 0, 255]);
        assert_eq!(pixel(15, 0), [0xf8, 0, 0xf8, 255]);
        assert_eq!(pixel(0, 15), [0xf8, 0, 0xf8, 255]);
        assert_eq!(pixel(15, 15), [0, 0, 0, 255]);
    }
}
