use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use pomme_gpu_allocator::vulkan::{Allocation, Allocator};
use pyronyx::vk;

use crate::assets::{AssetIndex, resolve_asset_path_with_packs};
use crate::renderer::util;

#[derive(Debug, Clone, Copy)]
pub struct AtlasRegion {
    pub u_min: f32,
    pub v_min: f32,
    pub u_max: f32,
    pub v_max: f32,
    /// Every level-0 texel is fully opaque (alpha 255), so quads using this
    /// sprite can render in the no-discard solid pass (early-Z). Sprites with
    /// any transparent texel are cutout and stay in the discard pass.
    pub opaque: bool,
}

#[derive(Clone)]
pub struct AtlasUVMap {
    regions: HashMap<String, AtlasRegion>,
    missing: AtlasRegion,
}

impl AtlasUVMap {
    pub fn get_region(&self, name: &str) -> AtlasRegion {
        self.regions.get(name).copied().unwrap_or(self.missing)
    }

    pub fn has_region(&self, name: &str) -> bool {
        self.regions.contains_key(name)
    }
}

pub fn atlas_asset_path(key: &str) -> String {
    if key.starts_with("item/") || key.starts_with("entity/") || key.starts_with("particle/") {
        format!("minecraft/textures/{key}.png")
    } else {
        format!("minecraft/textures/block/{key}.png")
    }
}

pub struct TextureAtlas {
    pub image: vk::Image,
    pub view: vk::ImageView,
    pub sampler: vk::Sampler,
    pub uv_map: AtlasUVMap,
    allocation: Option<Allocation>,
    staging_buffer: vk::Buffer,
    staging_allocation: Option<Allocation>,
}

const MISSING_TILE: u32 = 16;

/// Mip levels beyond level 0; level 4 reduces a 16x16 sprite to one texel.
const MIP_EXTRA: u32 = 4;
/// Sprite placement granularity, so every sprite origin stays on a texel
/// boundary at every mip level.
const MIP_ALIGN: u32 = 1 << MIP_EXTRA;

struct Source {
    name: String,
    data: Vec<u8>,
    w: u32,
    h: u32,
}

impl TextureAtlas {
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        device: &vk::Device,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        allocator: &Arc<Mutex<Allocator>>,
        jar_assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
        texture_names: &HashSet<&str>,
        packs: Option<&crate::resource_pack::ResourcePackManager>,
    ) -> Result<Self, vk::Error> {
        let mut sources: Vec<Source> = Vec::with_capacity(texture_names.len());
        let mut total_area: u64 = (MISSING_TILE * MISSING_TILE) as u64;
        for &name in texture_names {
            let asset_key = atlas_asset_path(name);
            let file_path =
                resolve_asset_path_with_packs(jar_assets_dir, asset_index, &asset_key, packs);
            match util::load_png(&file_path) {
                Some((data, w, h)) => {
                    // Animated textures stack the texture one on top of another. So as a solution
                    // we just take the first animation frame and use that, until animation is
                    // implemented.
                    let frame_size = if h > w { w } else { h };
                    let row_bytes = w as usize * size_of::<u32>();
                    let frame_data = data[..frame_size as usize * row_bytes].to_vec();
                    total_area += u64::from(w.next_multiple_of(MIP_ALIGN))
                        * u64::from(frame_size.next_multiple_of(MIP_ALIGN));
                    sources.push(Source {
                        name: name.to_string(),
                        data: frame_data,
                        w,
                        h: frame_size,
                    });
                }
                None => {
                    tracing::warn!("Missing texture: {name}");
                    sources.push(Source {
                        name: name.to_string(),
                        data: Vec::new(),
                        w: 0,
                        h: 0,
                    });
                }
            }
        }

        sources.sort_by_key(|s| std::cmp::Reverse(s.h.max(MISSING_TILE)));

        const MAX_ATLAS_SIZE: u32 = 8192;
        let mut atlas_size = (((total_area as f64) * 1.4).sqrt().ceil() as u32).next_power_of_two();

        let (placements, missing_region) = loop {
            let (result, all_fit) = pack(&sources, atlas_size);
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
        for py in 0..MISSING_TILE {
            for px in 0..MISSING_TILE {
                let is_check = ((px / 8) + (py / 8)) % 2 == 0;
                let color: [u8; 4] = if is_check {
                    [255, 0, 255, 255]
                } else {
                    [0, 0, 0, 255]
                };
                let idx = ((py * atlas_size + px) * 4) as usize;
                atlas_pixels[idx..idx + 4].copy_from_slice(&color);
            }
        }

        let mut regions = HashMap::new();
        let mut sprite_rects = vec![(0u32, 0u32, MISSING_TILE, MISSING_TILE)];
        for src in &sources {
            match placements.get(src.name.as_str()) {
                Some(Some((cx, cy))) => {
                    let mut region = pixel_region(*cx, *cy, src.w, src.h, atlas_size);
                    region.opaque = sprite_is_opaque(&src.data);
                    for py in 0..src.h {
                        for px in 0..src.w {
                            let s = ((py * src.w + px) * 4) as usize;
                            let d = (((cy + py) * atlas_size + cx + px) * 4) as usize;
                            atlas_pixels[d..d + 4].copy_from_slice(&src.data[s..s + 4]);
                        }
                    }
                    sprite_rects.push((*cx, *cy, src.w, src.h));
                    regions.insert(src.name.clone(), region);
                }
                _ => {
                    regions.insert(src.name.clone(), missing_region);
                }
            }
        }

        let uv_map = AtlasUVMap {
            regions,
            missing: missing_region,
        };

        let staging_pixels = build_mip_chain(atlas_pixels, atlas_size, &sprite_rects);

        let (image, view, allocation, mip_levels) = util::create_gpu_image_mipmapped(
            device,
            allocator,
            atlas_size,
            atlas_size,
            MIP_EXTRA + 1,
            "atlas_image",
        );
        let (staging_buffer, staging_allocation) =
            util::create_staging_buffer(device, allocator, &staging_pixels, "atlas_staging");

        util::upload_image_mipmapped(
            device,
            queue,
            command_pool,
            staging_buffer,
            staging_pixels.len() as u64,
            image,
            atlas_size,
            atlas_size,
            mip_levels,
        );

        let sampler = unsafe { util::create_nearest_sampler_mipmapped(device, mip_levels) };

        tracing::info!(
            "Atlas built: {atlas_size}x{atlas_size}, {} regions",
            uv_map.regions.len()
        );

        Ok(Self {
            image,
            view,
            sampler,
            uv_map,
            allocation: Some(allocation),
            staging_buffer,
            staging_allocation: Some(staging_allocation),
        })
    }

    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        device.destroy_sampler(self.sampler, None);
        device.destroy_image_view(self.view, None);

        if let Some(alloc) = self.allocation.take() {
            allocator.lock().unwrap().free(alloc).ok();
        }

        device.destroy_image(self.image, None);

        if let Some(alloc) = self.staging_allocation.take() {
            allocator.lock().unwrap().free(alloc).ok();
        }

        device.destroy_buffer(self.staging_buffer, None);
    }
}

/// Builds level 0 plus `MIP_EXTRA` downsampled levels, each sprite filtered
/// within its own region so neighbouring sprites never bleed in, packed
/// tightly level 0 first for `upload_image_mipmapped`.
fn build_mip_chain(
    atlas_pixels: Vec<u8>,
    atlas_size: u32,
    sprite_rects: &[(u32, u32, u32, u32)],
) -> Vec<u8> {
    let mut levels = vec![atlas_pixels];
    for level in 1..=MIP_EXTRA {
        let src_size = (atlas_size >> (level - 1)).max(1);
        let dst_size = (atlas_size >> level).max(1);
        let mut dst = vec![0u8; (dst_size * dst_size * 4) as usize];
        for &rect in sprite_rects {
            downsample_sprite(
                levels.last().unwrap(),
                src_size,
                &mut dst,
                dst_size,
                rect,
                level,
            );
        }
        levels.push(dst);
    }
    levels.concat()
}

/// A sprite's pixel rect scaled down to the given mip level.
fn mip_rect((x, y, w, h): (u32, u32, u32, u32), level: u32) -> (u32, u32, u32, u32) {
    (
        x >> level,
        y >> level,
        (w >> level).max(1),
        (h >> level).max(1),
    )
}

/// Box-filters one sprite's region from the previous mip level, clamped to the
/// sprite's own texels. RGB is averaged over the covered texels with non-zero
/// alpha so transparent texels don't darken cutout edges.
fn downsample_sprite(
    src: &[u8],
    src_size: u32,
    dst: &mut [u8],
    dst_size: u32,
    rect: (u32, u32, u32, u32),
    level: u32,
) {
    let (sx, sy, sw, sh) = mip_rect(rect, level - 1);
    let (dx, dy, dw, dh) = mip_rect(rect, level);
    for j in 0..dh {
        for i in 0..dw {
            let mut rgb = [0u32; 3];
            let mut alpha = 0u32;
            let mut opaque = 0u32;
            for (oi, oj) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let px = sx + (2 * i + oi).min(sw - 1);
                let py = sy + (2 * j + oj).min(sh - 1);
                let s = ((py * src_size + px) * 4) as usize;
                let a = u32::from(src[s + 3]);
                alpha += a;
                if a > 0 {
                    for (c, v) in rgb.iter_mut().enumerate() {
                        *v += u32::from(src[s + c]);
                    }
                    opaque += 1;
                }
            }
            let d = (((dy + j) * dst_size + dx + i) * 4) as usize;
            for (c, v) in rgb.iter().enumerate() {
                dst[d + c] = v.checked_div(opaque).unwrap_or(0) as u8;
            }
            dst[d + 3] = (alpha / 4) as u8;
        }
    }
}

fn pixel_region(x: u32, y: u32, w: u32, h: u32, atlas_size: u32) -> AtlasRegion {
    // Quarter-texel inset: `pack_uv` quantises UVs to u16, whose granularity
    // doesn't divide the atlas size, so exact-boundary UVs can round into the
    // neighbouring sprite. The inset absorbs that at every mip level.
    const INSET: f32 = 0.25;
    let s = atlas_size as f32;
    AtlasRegion {
        u_min: (x as f32 + INSET) / s,
        v_min: (y as f32 + INSET) / s,
        u_max: ((x + w) as f32 - INSET) / s,
        v_max: ((y + h) as f32 - INSET) / s,
        // Filled in by the caller from the sprite's texels; the missing tile is a
        // solid checker, so the geometric default is opaque.
        opaque: true,
    }
}

/// Whether every level-0 texel of an RGBA sprite is fully opaque (alpha 255).
/// Conservative: any transparency (or unknown) routes the sprite to the cutout
/// pass, so a hole never renders solid.
fn sprite_is_opaque(data: &[u8]) -> bool {
    data.chunks_exact(4).all(|px| px[3] == 255)
}

type PackResult = (HashMap<String, Option<(u32, u32)>>, AtlasRegion);

fn pack(sources: &[Source], atlas_size: u32) -> (PackResult, bool) {
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
            cursor_y = (cursor_y + shelf_h).next_multiple_of(MIP_ALIGN);
            cursor_x = 0;
            shelf_h = 0;
        }
        if cursor_y + src.h > atlas_size {
            all_fit = false;
            placements.insert(src.name.clone(), None);
            continue;
        }
        placements.insert(src.name.clone(), Some((cursor_x, cursor_y)));
        cursor_x = (cursor_x + src.w).next_multiple_of(MIP_ALIGN);
        shelf_h = shelf_h.max(src.h);
    }
    ((placements, missing_region), all_fit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fill(pixels: &mut [u8], atlas_size: u32, rect: (u32, u32, u32, u32), color: [u8; 4]) {
        let (x, y, w, h) = rect;
        for py in y..y + h {
            for px in x..x + w {
                let i = ((py * atlas_size + px) * 4) as usize;
                pixels[i..i + 4].copy_from_slice(&color);
            }
        }
    }

    #[test]
    fn mips_stay_within_sprite_regions() {
        const SIZE: u32 = 64;
        let rects = [(16u32, 0u32, 16u32, 16u32), (32, 0, 16, 16)];
        let colors = [[255u8, 0, 0, 255], [0, 0, 255, 255]];

        let mut pixels = vec![0u8; (SIZE * SIZE * 4) as usize];
        for (&rect, &color) in rects.iter().zip(&colors) {
            fill(&mut pixels, SIZE, rect, color);
        }

        let chain = build_mip_chain(pixels, SIZE, &rects);

        let mut offset = (SIZE * SIZE * 4) as usize;
        for level in 1..=MIP_EXTRA {
            let size = SIZE >> level;
            for (&rect, &color) in rects.iter().zip(&colors) {
                let (dx, dy, dw, dh) = mip_rect(rect, level);
                for py in dy..dy + dh {
                    for px in dx..dx + dw {
                        let i = offset + ((py * size + px) * 4) as usize;
                        assert_eq!(
                            &chain[i..i + 4],
                            &color,
                            "level {level} texel ({px}, {py}) leaked outside its sprite"
                        );
                    }
                }
            }
            offset += (size * size * 4) as usize;
        }
        assert_eq!(offset, chain.len());
    }

    #[test]
    fn transparent_texels_do_not_darken_rgb() {
        const SIZE: u32 = 16;
        let rect = (0u32, 0u32, 16u32, 16u32);
        let mut pixels = vec![0u8; (SIZE * SIZE * 4) as usize];
        // Alternating opaque-green and transparent-black columns.
        for py in 0..16 {
            for px in (0..16).step_by(2) {
                let i = ((py * SIZE + px) * 4) as usize;
                pixels[i..i + 4].copy_from_slice(&[0, 255, 0, 255]);
            }
        }

        let chain = build_mip_chain(pixels, SIZE, &[rect]);

        // Level 1: each 2x2 block holds two opaque green and two transparent
        // texels; RGB must stay full green, alpha averages to half.
        let l1 = &chain[(SIZE * SIZE * 4) as usize..];
        assert_eq!(&l1[0..4], &[0, 255, 0, 127]);
    }
}
