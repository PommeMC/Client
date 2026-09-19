use std::collections::HashMap;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::Value;

use crate::assets::{self, AssetIndex};
use crate::chat_component::normalize_identifier;
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
        self.resolve(&texture_asset_key(resource)).exists()
    }

    fn load_texture(&self, resource: &str) -> Option<(Vec<u8>, u32, u32)> {
        crate::renderer::util::load_png(&self.resolve(&texture_asset_key(resource)))
    }

    /// Every copy of the atlas definition, base assets first, so later packs'
    /// sources apply on top.
    fn atlas_definition_stack(&self, atlas_key: &str) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let base = assets::resolve_asset_path(self.jar_assets_dir, self.asset_index, atlas_key);
        if base.exists() {
            out.push(base);
        }
        for pack in self.packs.active_pack_dirs() {
            let path = pack.join("assets").join(atlas_key);
            if path.exists() {
                out.push(path);
            }
        }
        out
    }

    fn load_direct_sprite(&self, resource: &str) -> Option<(Vec<u8>, u32, u32)> {
        let (rgba, w, h) = self.load_texture(resource)?;
        let (frame_w, frame_h) = self.animation_frame_size(resource, w, h);
        crop_rgba(&rgba, w, h, 0, 0, frame_w.min(w), frame_h.min(h))
    }

    fn animation_frame_size(&self, resource: &str, w: u32, h: u32) -> (u32, u32) {
        let path = self.resolve(&format!("{}.mcmeta", texture_asset_key(resource)));
        if let Ok(text) = std::fs::read_to_string(path)
            && let Ok(value) = serde_json::from_str::<Value>(&text)
            && let Some(animation) = value.get("animation").and_then(Value::as_object)
        {
            let fw = animation
                .get("width")
                .and_then(Value::as_u64)
                .map(|v| v as u32);
            let fh = animation
                .get("height")
                .and_then(Value::as_u64)
                .map(|v| v as u32);
            return match (fw, fh) {
                (Some(fw), Some(fh)) if fw > 0 && fh > 0 => (fw, fh),
                (Some(fw), None) if fw > 0 => (fw, h),
                (None, Some(fh)) if fh > 0 => (w, fh),
                _ => {
                    let min = w.min(h);
                    (min, min)
                }
            };
        }
        (w, h)
    }
}

pub fn load_atlas_sprite_8x8(
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    packs: &ResourcePackManager,
    atlas: &str,
    sprite: &str,
) -> Option<Vec<u8>> {
    let lookup = AssetLookup {
        jar_assets_dir,
        asset_index,
        packs,
    };
    let (sprite_ns, sprite_path) = split_id(sprite);
    let mut candidate: Option<Recipe> = None;

    for path in lookup.atlas_definition_stack(&atlas_asset_key(atlas)) {
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
            apply_source(source, sprite_ns, sprite_path, &lookup, &mut candidate);
        }
    }

    let image = match candidate? {
        Recipe::Direct(resource) => lookup.load_direct_sprite(&resource)?,
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
    Some(resample_8x8(&image.0, image.1, image.2))
}

pub fn missing_tile_8x8() -> Vec<u8> {
    let mut out = vec![0u8; 8 * 8 * 4];
    for y in 0..8usize {
        for x in 0..8usize {
            let off = (y * 8 + x) * 4;
            let rgb = if ((x / 4) + (y / 4)) % 2 == 0 {
                [255, 0, 255]
            } else {
                [0, 0, 0]
            };
            out[off..off + 3].copy_from_slice(&rgb);
            out[off + 3] = 255;
        }
    }
    out
}

fn apply_source(
    source: &Value,
    sprite_ns: &str,
    sprite_path: &str,
    lookup: &AssetLookup<'_>,
    candidate: &mut Option<Recipe>,
) {
    let Some(map) = source.as_object() else {
        return;
    };
    let kind = map
        .get("type")
        .and_then(Value::as_str)
        .map(|value| value.strip_prefix("minecraft:").unwrap_or(value));
    match kind {
        Some("directory") => {
            let Some(prefix) = map.get("prefix").and_then(Value::as_str) else {
                return;
            };
            let Some(source_path) = map.get("source").and_then(Value::as_str) else {
                return;
            };
            let Some(rest) = sprite_path.strip_prefix(prefix) else {
                return;
            };
            let path = join_resource_path(source_path, rest);
            let resource = format!("{sprite_ns}:{path}");
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
            if id_matches(target, sprite_ns, sprite_path) && lookup.texture_exists(resource) {
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
                .is_none_or(|pattern| regex_matches(pattern, sprite_ns));
            let path_matches = pattern
                .get("path")
                .and_then(Value::as_str)
                .is_none_or(|pattern| regex_matches(pattern, sprite_path));
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
                if !id_matches(target, sprite_ns, sprite_path) {
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
                let (texture_ns, texture_path) = split_id(texture);
                if texture_ns != sprite_ns {
                    continue;
                }
                for (suffix, palette_value) in permutations {
                    let Some(palette_value) = palette_value.as_str() else {
                        continue;
                    };
                    if sprite_path == format!("{texture_path}{separator}{suffix}")
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

fn resample_8x8(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut out = vec![0u8; 8 * 8 * 4];
    if w == 0 || h == 0 {
        return out;
    }
    for y in 0..8u32 {
        for x in 0..8u32 {
            let sx = (x * w / 8).min(w - 1);
            let sy = (y * h / 8).min(h - 1);
            let src = ((sy * w + sx) * 4) as usize;
            let dst = ((y * 8 + x) * 4) as usize;
            if let Some(pixel) = rgba.get(src..src + 4) {
                out[dst..dst + 4].copy_from_slice(pixel);
            }
        }
    }
    out
}

fn atlas_asset_key(id: &str) -> String {
    let (namespace, path) = split_id(id);
    format!("{namespace}/atlases/{path}.json")
}

fn texture_asset_key(id: &str) -> String {
    let (namespace, path) = split_id(id);
    format!("{namespace}/textures/{path}.png")
}

fn split_id(id: &str) -> (&str, &str) {
    id.split_once(':').unwrap_or(("minecraft", id))
}

fn id_matches(id: &str, namespace: &str, path: &str) -> bool {
    split_id(id) == (namespace, path)
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
        let out = resample_8x8(&pixels, 2, 2);
        assert_eq!(&out[0..4], &[255, 0, 0, 255]);
        assert_eq!(&out[(7 * 4)..(8 * 4)], &[0, 255, 0, 255]);
        let bottom_left = ((7 * 8) * 4) as usize;
        assert_eq!(&out[bottom_left..bottom_left + 4], &[0, 0, 255, 255]);
        assert_eq!(&out[out.len() - 4..], &[255, 255, 255, 255]);
    }

    #[test]
    fn palette_mapping_multiplies_alpha_like_vanilla() {
        let base = vec![10, 20, 30, 128];
        let key = vec![10, 20, 30, 255];
        let value = vec![90, 80, 70, 128];
        assert_eq!(apply_palette(base, &key, &value), vec![90, 80, 70, 64]);
    }

    #[test]
    fn missing_tile_is_magenta_black_checkerboard() {
        let tile = missing_tile_8x8();
        assert_eq!(&tile[0..4], &[255, 0, 255, 255]);
        let x4 = 4 * 4;
        assert_eq!(&tile[x4..x4 + 4], &[0, 0, 0, 255]);
    }
}
