use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use ash::vk;
use gpu_allocator::vulkan::{Allocation, Allocator};

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

pub struct TextureAtlas {
    pub image: vk::Image,
    pub view: vk::ImageView,
    pub sampler: vk::Sampler,
    pub uv_map: AtlasUVMap,
    allocation: Option<Allocation>,
    staging_buffer: vk::Buffer,
    staging_allocation: Option<Allocation>,
}

impl TextureAtlas {
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        device: &ash::Device,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        allocator: &Arc<Mutex<Allocator>>,
        jar_assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
        texture_names: &HashSet<&str>,
        packs: Option<&crate::resource_pack::ResourcePackManager>,
    ) -> Result<Self, vk::Result> {
        let tile_size = 16u32;
        let grid_size = (texture_names.len() as f32 + 1.0).sqrt().ceil() as u32 + 1;
        let atlas_size = (grid_size * tile_size).next_power_of_two();

        let mut atlas_pixels = vec![0u8; (atlas_size * atlas_size * 4) as usize];
        let mut regions = HashMap::new();

        let missing_region =
            tile_region(tile_origin(0, grid_size, tile_size), tile_size, atlas_size);

        for py in 0..tile_size {
            for px in 0..tile_size {
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

        let mut slot = 1u32;

        for &name in texture_names {
            let asset_key = format!("minecraft/textures/block/{name}.png");
            let file_path =
                resolve_asset_path_with_packs(jar_assets_dir, asset_index, &asset_key, packs);
            let (data, img_w, img_h) = match util::load_png(&file_path) {
                Some(p) => p,
                None => {
                    tracing::warn!("Missing texture: {name}");
                    regions.insert(name.to_string(), missing_region);
                    continue;
                }
            };

            let origin = tile_origin(slot, grid_size, tile_size);
            let region = tile_region(origin, tile_size, atlas_size);

            let img_width = img_w.min(tile_size);
            let img_height = img_h.min(tile_size);
            for py in 0..img_height {
                for px in 0..img_width {
                    let src = ((py * img_w + px) * 4) as usize;
                    let dst = (((origin.1 + py) * atlas_size + origin.0 + px) * 4) as usize;
                    atlas_pixels[dst..dst + 4].copy_from_slice(&data[src..src + 4]);
                }
            }

            regions.insert(name.to_string(), region);
            slot += 1;
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

        tracing::info!("Atlas built: {atlas_size}x{atlas_size}, {slot} textures");

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

    pub fn destroy(&mut self, device: &ash::Device, allocator: &Arc<Mutex<Allocator>>) {
        unsafe {
            device.destroy_sampler(self.sampler, None);
            device.destroy_image_view(self.view, None);
        }
        if let Some(alloc) = self.allocation.take() {
            allocator.lock().unwrap().free(alloc).ok();
        }
        unsafe {
            device.destroy_image(self.image, None);
        }

        if let Some(alloc) = self.staging_allocation.take() {
            allocator.lock().unwrap().free(alloc).ok();
        }
        unsafe {
            device.destroy_buffer(self.staging_buffer, None);
        }
    }
}

fn tile_origin(slot: u32, grid_size: u32, tile_size: u32) -> (u32, u32) {
    (
        (slot % grid_size) * tile_size,
        (slot / grid_size) * tile_size,
    )
}

fn tile_region(origin: (u32, u32), tile_size: u32, atlas_size: u32) -> AtlasRegion {
    let s = atlas_size as f32;
    AtlasRegion {
        u_min: origin.0 as f32 / s,
        v_min: origin.1 as f32 / s,
        u_max: (origin.0 + tile_size) as f32 / s,
        v_max: (origin.1 + tile_size) as f32 / s,
    }
}
