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
    if key.starts_with("item/") || key.starts_with("entity/") {
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
                    total_area += (w as u64) * (h as u64);
                    sources.push(Source {
                        name: name.to_string(),
                        data,
                        w,
                        h,
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
        for src in &sources {
            match placements.get(src.name.as_str()) {
                Some(Some((cx, cy))) => {
                    let region = pixel_region(*cx, *cy, src.w, src.h, atlas_size);
                    for py in 0..src.h {
                        for px in 0..src.w {
                            let s = ((py * src.w + px) * 4) as usize;
                            let d = (((cy + py) * atlas_size + cx + px) * 4) as usize;
                            atlas_pixels[d..d + 4].copy_from_slice(&src.data[s..s + 4]);
                        }
                    }
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

        let (image, view, allocation, mip_levels) = util::create_gpu_image_mipmapped(
            device,
            allocator,
            atlas_size,
            atlas_size,
            "atlas_image",
        );
        let (staging_buffer, staging_allocation) =
            util::create_staging_buffer(device, allocator, &atlas_pixels, "atlas_staging");

        util::upload_image_mipmapped(
            device,
            queue,
            command_pool,
            staging_buffer,
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

fn pixel_region(x: u32, y: u32, w: u32, h: u32, atlas_size: u32) -> AtlasRegion {
    let s = atlas_size as f32;
    AtlasRegion {
        u_min: x as f32 / s,
        v_min: y as f32 / s,
        u_max: (x + w) as f32 / s,
        v_max: (y + h) as f32 / s,
    }
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
            cursor_y += shelf_h;
            cursor_x = 0;
            shelf_h = 0;
        }
        if cursor_y + src.h > atlas_size {
            all_fit = false;
            placements.insert(src.name.clone(), None);
            continue;
        }
        placements.insert(src.name.clone(), Some((cursor_x, cursor_y)));
        cursor_x += src.w;
        shelf_h = shelf_h.max(src.h);
    }
    ((placements, missing_region), all_fit)
}
