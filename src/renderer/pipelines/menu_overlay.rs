use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use ash::vk;
use gpu_allocator::vulkan::{Allocation, Allocator};

use std::path::Path;

use crate::assets::{resolve_asset_path, AssetIndex};
use crate::renderer::shader;
use crate::renderer::util;
use crate::ui::font::GlyphMap;
use crate::ui::server_list::MotdSpan;

const FONT_BYTES: &[u8] = include_bytes!("../fonts/Montserrat-Medium.ttf");
const ICON_FONT_BYTES: &[u8] = include_bytes!("../fonts/fa-solid-900.ttf");
const ATLAS_SIZE: u32 = 512;
const RASTER_PX: f32 = 48.0;

pub const ICON_USER: char = '\u{f007}';
pub const ICON_LINK: char = '\u{f0c1}';
pub const ICON_PAINTBRUSH: char = '\u{f1fc}';
pub const ICON_GEAR: char = '\u{f013}';
pub const ICON_GLOBE: char = '\u{f0ac}';
pub const ICON_COMMENT: char = '\u{f075}';
pub const ICON_CODE: char = '\u{f121}';
pub const ICON_CHECK: char = '\u{f00c}';

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
    mode: f32,
    rect_size: [f32; 2],
    corner_radius: f32,
}

const MAX_VERTICES: usize = 16384;
const VERTEX_SIZE: usize = std::mem::size_of::<Vertex>();

struct GlyphEntry {
    u0: f32, v0: f32, u1: f32, v1: f32,
    width_px: f32,
    height_px: f32,
    x_offset: f32,
    y_offset: f32,
    advance: f32,
}

struct FontAtlas {
    glyphs: HashMap<char, GlyphEntry>,
    pixels: Vec<u8>,
}

fn build_font_atlas() -> FontAtlas {
    let font = fontdue::Font::from_bytes(FONT_BYTES, fontdue::FontSettings::default())
        .expect("failed to parse Montserrat font");
    let icon_font = fontdue::Font::from_bytes(ICON_FONT_BYTES, fontdue::FontSettings::default())
        .expect("failed to parse Font Awesome font");

    let mut glyphs = HashMap::new();
    let mut pixels = vec![0u8; (ATLAS_SIZE * ATLAS_SIZE * 4) as usize];
    let mut cursor_x = 0u32;
    let mut cursor_y = 0u32;
    let mut row_height = 0u32;

    let text_chars: Vec<(char, &fontdue::Font)> = (' '..='~').map(|ch| (ch, &font)).collect();
    let icon_chars: Vec<(char, &fontdue::Font)> = [ICON_USER, ICON_LINK, ICON_PAINTBRUSH, ICON_GEAR, ICON_GLOBE, ICON_COMMENT, ICON_CODE, ICON_CHECK]
        .iter()
        .map(|&ch| (ch, &icon_font))
        .collect();

    for (ch, raster_font) in text_chars.iter().chain(icon_chars.iter()) {
        let (metrics, bitmap) = raster_font.rasterize(*ch, RASTER_PX);

        if cursor_x + metrics.width as u32 + 1 > ATLAS_SIZE {
            cursor_x = 0;
            cursor_y += row_height + 1;
            row_height = 0;
        }

        if cursor_y + metrics.height as u32 + 1 > ATLAS_SIZE {
            break;
        }

        for row in 0..metrics.height {
            for col in 0..metrics.width {
                let src = row * metrics.width + col;
                let dst_x = cursor_x + col as u32;
                let dst_y = cursor_y + row as u32;
                let dst = ((dst_y * ATLAS_SIZE + dst_x) * 4) as usize;
                let a = bitmap[src];
                pixels[dst] = 255;
                pixels[dst + 1] = 255;
                pixels[dst + 2] = 255;
                pixels[dst + 3] = a;
            }
        }

        let inv = 1.0 / ATLAS_SIZE as f32;
        glyphs.insert(*ch, GlyphEntry {
            u0: cursor_x as f32 * inv,
            v0: cursor_y as f32 * inv,
            u1: (cursor_x + metrics.width as u32) as f32 * inv,
            v1: (cursor_y + metrics.height as u32) as f32 * inv,
            width_px: metrics.width as f32,
            height_px: metrics.height as f32,
            x_offset: metrics.xmin as f32,
            y_offset: metrics.ymin as f32,
            advance: metrics.advance_width,
        });

        row_height = row_height.max(metrics.height as u32);
        cursor_x += metrics.width as u32 + 1;
    }

    FontAtlas { glyphs, pixels }
}

pub struct MenuOverlayPipeline {
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    globals_layout: vk::DescriptorSetLayout,
    tex_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    globals_set: vk::DescriptorSet,
    tex_set: vk::DescriptorSet,
    globals_buffer: vk::Buffer,
    globals_allocation: Option<Allocation>,
    font_image: vk::Image,
    font_view: vk::ImageView,
    font_sampler: vk::Sampler,
    font_allocation: Option<Allocation>,
    font_staging_buffer: vk::Buffer,
    font_staging_allocation: Option<Allocation>,
    sprite_image: vk::Image,
    sprite_view: vk::ImageView,
    sprite_sampler: vk::Sampler,
    sprite_allocation: Option<Allocation>,
    sprite_staging_buffer: vk::Buffer,
    sprite_staging_allocation: Option<Allocation>,
    sprite_atlas: SpriteAtlas,
    item_image: vk::Image,
    item_view: vk::ImageView,
    item_sampler: vk::Sampler,
    item_allocation: Option<Allocation>,
    item_staging_buffer: vk::Buffer,
    item_staging_allocation: Option<Allocation>,
    item_atlas: ItemAtlas,
    mc_font_image: vk::Image,
    mc_font_view: vk::ImageView,
    mc_font_sampler: vk::Sampler,
    mc_font_allocation: Option<Allocation>,
    mc_font_staging_buffer: vk::Buffer,
    mc_font_staging_allocation: Option<Allocation>,
    mc_glyph_map: Option<GlyphMap>,
    vertex_buffer: vk::Buffer,
    vertex_allocation: Option<Allocation>,
    atlas: FontAtlas,
}

impl MenuOverlayPipeline {
    pub fn new(
        device: &ash::Device,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        render_pass: vk::RenderPass,
        allocator: &Arc<Mutex<Allocator>>,
        assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
    ) -> Self {
        let atlas = build_font_atlas();

        let globals_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::UNIFORM_BUFFER,
            vk::ShaderStageFlags::VERTEX,
        );

        let tex_bindings = [
            vk::DescriptorSetLayoutBinding {
                binding: 0,
                descriptor_type: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::FRAGMENT,
                ..Default::default()
            },
            vk::DescriptorSetLayoutBinding {
                binding: 1,
                descriptor_type: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::FRAGMENT,
                ..Default::default()
            },
            vk::DescriptorSetLayoutBinding {
                binding: 2,
                descriptor_type: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::FRAGMENT,
                ..Default::default()
            },
            vk::DescriptorSetLayoutBinding {
                binding: 3,
                descriptor_type: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::FRAGMENT,
                ..Default::default()
            },
        ];
        let tex_layout_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&tex_bindings);
        let tex_layout = unsafe { device.create_descriptor_set_layout(&tex_layout_info, None) }
            .expect("failed to create texture descriptor set layout");

        let layouts = [globals_layout, tex_layout];
        let layout_info = vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts);
        let pipeline_layout = unsafe { device.create_pipeline_layout(&layout_info, None) }
            .expect("failed to create menu overlay pipeline layout");

        let pipeline = create_pipeline(device, render_pass, pipeline_layout);

        let pool_sizes = [
            vk::DescriptorPoolSize { ty: vk::DescriptorType::UNIFORM_BUFFER, descriptor_count: 1 },
            vk::DescriptorPoolSize { ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER, descriptor_count: 4 },
        ];
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .max_sets(2)
            .pool_sizes(&pool_sizes);
        let descriptor_pool = unsafe { device.create_descriptor_pool(&pool_info, None) }
            .expect("failed to create menu overlay descriptor pool");

        let globals_layouts = [globals_layout];
        let globals_alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(descriptor_pool)
            .set_layouts(&globals_layouts);
        let globals_set = unsafe { device.allocate_descriptor_sets(&globals_alloc_info) }
            .expect("failed to allocate globals descriptor set")[0];

        let tex_layouts = [tex_layout];
        let tex_alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(descriptor_pool)
            .set_layouts(&tex_layouts);
        let tex_set = unsafe { device.allocate_descriptor_sets(&tex_alloc_info) }
            .expect("failed to allocate texture descriptor set")[0];

        let (globals_buffer, globals_allocation) = util::create_uniform_buffer(device, allocator, 8, "menu_globals");

        let buf_info = [vk::DescriptorBufferInfo {
            buffer: globals_buffer,
            offset: 0,
            range: 8,
        }];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(globals_set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
            .buffer_info(&buf_info);
        unsafe { device.update_descriptor_sets(&[write], &[]) };

        let (font_image, font_view, font_alloc) = util::create_gpu_image_with_format(
            device, allocator, ATLAS_SIZE, ATLAS_SIZE, vk::Format::R8G8B8A8_UNORM, "menu_font_atlas",
        );

        let (font_staging_buffer, font_staging_alloc) = util::create_staging_buffer(
            device, allocator, &atlas.pixels, "menu_font_staging",
        );

        util::upload_image(device, queue, command_pool, font_staging_buffer, font_image, ATLAS_SIZE, ATLAS_SIZE);

        let font_sampler = unsafe { util::create_linear_sampler(device) };

        let (sprite_atlas_data, sprite_image, sprite_view, sprite_alloc, sprite_staging_buffer, sprite_staging_alloc) =
            build_sprite_atlas(device, queue, command_pool, allocator, assets_dir, asset_index);

        let sprite_sampler = unsafe { util::create_nearest_sampler(device) };

        let (item_atlas_data, item_image, item_view, item_alloc, item_staging_buffer, item_staging_alloc) =
            build_item_atlas(device, queue, command_pool, allocator, assets_dir, asset_index);

        let item_sampler = unsafe { util::create_nearest_sampler(device) };

        let mc_glyph_map = GlyphMap::load(assets_dir, asset_index);
        let (mc_font_image, mc_font_view, mc_font_alloc, mc_font_staging_buffer, mc_font_staging_alloc) =
            if let Some(ref gm) = mc_glyph_map {
                let (w, h) = gm.dimensions();
                let (img, view, alloc) = util::create_gpu_image(device, allocator, w, h, "mc_font_atlas");
                let (stg_buf, stg_alloc) = util::create_staging_buffer(device, allocator, gm.raw_pixels(), "mc_font_staging");
                util::upload_image(device, queue, command_pool, stg_buf, img, w, h);
                (img, view, Some(alloc), stg_buf, Some(stg_alloc))
            } else {
                let (img, view, alloc) = util::create_gpu_image(device, allocator, 1, 1, "mc_font_dummy");
                let dummy = [0u8; 4];
                let (stg_buf, stg_alloc) = util::create_staging_buffer(device, allocator, &dummy, "mc_font_dummy_stg");
                util::upload_image(device, queue, command_pool, stg_buf, img, 1, 1);
                (img, view, Some(alloc), stg_buf, Some(stg_alloc))
            };
        let mc_font_sampler = unsafe { util::create_nearest_sampler(device) };

        let font_img_info = [vk::DescriptorImageInfo {
            sampler: font_sampler,
            image_view: font_view,
            image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        }];
        let sprite_img_info = [vk::DescriptorImageInfo {
            sampler: sprite_sampler,
            image_view: sprite_view,
            image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        }];
        let item_img_info = [vk::DescriptorImageInfo {
            sampler: item_sampler,
            image_view: item_view,
            image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        }];
        let mc_font_img_info = [vk::DescriptorImageInfo {
            sampler: mc_font_sampler,
            image_view: mc_font_view,
            image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        }];
        let writes = [
            vk::WriteDescriptorSet::default()
                .dst_set(tex_set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(&font_img_info),
            vk::WriteDescriptorSet::default()
                .dst_set(tex_set)
                .dst_binding(1)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(&sprite_img_info),
            vk::WriteDescriptorSet::default()
                .dst_set(tex_set)
                .dst_binding(2)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(&item_img_info),
            vk::WriteDescriptorSet::default()
                .dst_set(tex_set)
                .dst_binding(3)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(&mc_font_img_info),
        ];
        unsafe { device.update_descriptor_sets(&writes, &[]) };

        let (vertex_buffer, vertex_allocation) = util::create_host_buffer(
            device, allocator, (MAX_VERTICES * VERTEX_SIZE) as u64,
            vk::BufferUsageFlags::VERTEX_BUFFER, "menu_vertices",
        );

        Self {
            pipeline, pipeline_layout, globals_layout, tex_layout,
            descriptor_pool, globals_set, tex_set,
            globals_buffer, globals_allocation: Some(globals_allocation),
            font_image, font_view, font_sampler,
            font_allocation: Some(font_alloc),
            font_staging_buffer, font_staging_allocation: Some(font_staging_alloc),
            sprite_image, sprite_view, sprite_sampler,
            sprite_allocation: Some(sprite_alloc),
            sprite_staging_buffer, sprite_staging_allocation: sprite_staging_alloc,
            sprite_atlas: sprite_atlas_data,
            item_image, item_view, item_sampler,
            item_allocation: Some(item_alloc),
            item_staging_buffer, item_staging_allocation: item_staging_alloc,
            item_atlas: item_atlas_data,
            mc_font_image, mc_font_view, mc_font_sampler,
            mc_font_allocation: mc_font_alloc,
            mc_font_staging_buffer,
            mc_font_staging_allocation: mc_font_staging_alloc,
            mc_glyph_map,
            vertex_buffer, vertex_allocation: Some(vertex_allocation),
            atlas,
        }
    }

    pub fn draw(
        &mut self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        screen_w: f32,
        screen_h: f32,
        elements: &[MenuElement],
    ) {
        let globals: [f32; 2] = [screen_w, screen_h];
        self.globals_allocation.as_mut().unwrap().mapped_slice_mut().unwrap()[..8]
            .copy_from_slice(bytemuck::cast_slice(&globals));

        let mut vertices: Vec<Vertex> = Vec::new();
        for elem in elements {
            match elem {
                MenuElement::Rect { x, y, w, h, corner_radius, color } => {
                    push_rect(&mut vertices, *x, *y, *w, *h, *corner_radius, *color);
                }
                MenuElement::Text { x, y, text, scale, color, centered } => {
                    let start_x = if *centered {
                        *x - self.text_width(text, *scale) / 2.0
                    } else {
                        *x
                    };
                    push_text_glyphs(&mut vertices, &self.atlas, start_x, *y, text, *scale, *color);
                }
                MenuElement::Icon { x, y, icon, scale, color } => {
                    push_icon_glyph(&mut vertices, &self.atlas, *x, *y, *icon, *scale, *color);
                }
                MenuElement::Image { x, y, w, h, sprite, tint } => {
                    if let Some(region) = self.sprite_atlas.regions.get(sprite) {
                        push_textured_quad(&mut vertices, *x, *y, *w, *h, region, *tint, 2.0);
                    }
                }
                MenuElement::ItemIcon { x, y, w, h, item_name, tint } => {
                    if let Some(region) = self.item_atlas.regions.get(item_name.as_str()) {
                        push_textured_quad(&mut vertices, *x, *y, *w, *h, region, *tint, 3.0);
                    }
                }
                MenuElement::McText { x, y, spans, scale } => {
                    if let Some(ref gm) = self.mc_glyph_map {
                        push_mc_text(&mut vertices, gm, *x, *y, spans, *scale);
                    }
                }
            }
        }

        if vertices.is_empty() {
            return;
        }

        let count = vertices.len().min(MAX_VERTICES);
        let byte_data = bytemuck::cast_slice(&vertices[..count]);
        self.vertex_allocation.as_mut().unwrap().mapped_slice_mut().unwrap()[..byte_data.len()]
            .copy_from_slice(byte_data);

        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_bind_descriptor_sets(
                cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline_layout,
                0, &[self.globals_set, self.tex_set], &[],
            );
            device.cmd_bind_vertex_buffers(cmd, 0, &[self.vertex_buffer], &[0]);
            device.cmd_draw(cmd, count as u32, 1, 0, 0);
        }
    }

    pub fn text_width(&self, text: &str, scale: f32) -> f32 {
        let s = scale / RASTER_PX;
        text.chars()
            .filter_map(|ch| self.atlas.glyphs.get(&ch))
            .map(|g| g.advance * s)
            .sum()
    }

    #[allow(dead_code)]
    pub fn mc_text_width(&self, text: &str, scale: f32) -> f32 {
        let Some(ref gm) = self.mc_glyph_map else { return 0.0 };
        let px_scale = scale / gm.cell_h as f32;
        text.chars()
            .map(|ch| {
                let w = gm.glyphs.get(&ch).map(|g| g.width).unwrap_or(gm.cell_w / 2);
                (w as f32 + 1.0) * px_scale
            })
            .sum()
    }

    pub fn recreate_pipeline(&mut self, device: &ash::Device, render_pass: vk::RenderPass) {
        unsafe { device.destroy_pipeline(self.pipeline, None) };
        self.pipeline = create_pipeline(device, render_pass, self.pipeline_layout);
    }

    pub fn destroy(&mut self, device: &ash::Device, allocator: &Arc<Mutex<Allocator>>) {
        let mut alloc = allocator.lock().unwrap();

        unsafe { device.destroy_buffer(self.globals_buffer, None) };
        if let Some(a) = self.globals_allocation.take() { alloc.free(a).ok(); }

        unsafe { device.destroy_buffer(self.vertex_buffer, None) };
        if let Some(a) = self.vertex_allocation.take() { alloc.free(a).ok(); }

        destroy_texture_resources(device, &mut alloc, &mut TextureResources {
            sampler: self.font_sampler, image: self.font_image, view: self.font_view,
            image_alloc: self.font_allocation.take(), staging_buffer: self.font_staging_buffer,
            staging_alloc: self.font_staging_allocation.take(),
        });
        destroy_texture_resources(device, &mut alloc, &mut TextureResources {
            sampler: self.sprite_sampler, image: self.sprite_image, view: self.sprite_view,
            image_alloc: self.sprite_allocation.take(), staging_buffer: self.sprite_staging_buffer,
            staging_alloc: self.sprite_staging_allocation.take(),
        });
        destroy_texture_resources(device, &mut alloc, &mut TextureResources {
            sampler: self.item_sampler, image: self.item_image, view: self.item_view,
            image_alloc: self.item_allocation.take(), staging_buffer: self.item_staging_buffer,
            staging_alloc: self.item_staging_allocation.take(),
        });
        destroy_texture_resources(device, &mut alloc, &mut TextureResources {
            sampler: self.mc_font_sampler, image: self.mc_font_image, view: self.mc_font_view,
            image_alloc: self.mc_font_allocation.take(), staging_buffer: self.mc_font_staging_buffer,
            staging_alloc: self.mc_font_staging_allocation.take(),
        });

        drop(alloc);

        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_descriptor_pool(self.descriptor_pool, None);
            device.destroy_descriptor_set_layout(self.globals_layout, None);
            device.destroy_descriptor_set_layout(self.tex_layout, None);
        }
    }
}

pub enum MenuElement {
    Rect {
        x: f32, y: f32, w: f32, h: f32,
        corner_radius: f32,
        color: [f32; 4],
    },
    Text {
        x: f32, y: f32,
        text: String,
        scale: f32,
        color: [f32; 4],
        centered: bool,
    },
    Icon {
        x: f32, y: f32,
        icon: char,
        scale: f32,
        color: [f32; 4],
    },
    Image {
        x: f32, y: f32, w: f32, h: f32,
        sprite: SpriteId,
        tint: [f32; 4],
    },
    ItemIcon {
        x: f32, y: f32, w: f32, h: f32,
        item_name: String,
        tint: [f32; 4],
    },
    McText {
        x: f32, y: f32,
        spans: Vec<MotdSpan>,
        scale: f32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SpriteId {
    Hotbar,
    HotbarSelection,
    HeartContainer,
    HeartFull,
    HeartHalf,
    FoodEmpty,
    FoodFull,
    FoodHalf,
    InventoryBackground,
    EmptyHelmet,
    EmptyChestplate,
    EmptyLeggings,
    EmptyBoots,
    EmptyShield,
}

struct SpriteRegion {
    u0: f32, v0: f32, u1: f32, v1: f32,
}

struct SpriteAtlas {
    regions: HashMap<SpriteId, SpriteRegion>,
}

struct ItemAtlas {
    regions: HashMap<String, SpriteRegion>,
}

const INV_TEX_W: u32 = 176;
const INV_TEX_H: u32 = 166;

fn build_sprite_atlas(
    device: &ash::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    allocator: &Arc<Mutex<Allocator>>,
    assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
) -> (SpriteAtlas, vk::Image, vk::ImageView, Allocation, vk::Buffer, Option<Allocation>) {
    let sprites: &[(SpriteId, &str)] = &[
        (SpriteId::Hotbar, "minecraft/textures/gui/sprites/hud/hotbar.png"),
        (SpriteId::HotbarSelection, "minecraft/textures/gui/sprites/hud/hotbar_selection.png"),
        (SpriteId::HeartContainer, "minecraft/textures/gui/sprites/hud/heart/container.png"),
        (SpriteId::HeartFull, "minecraft/textures/gui/sprites/hud/heart/full.png"),
        (SpriteId::HeartHalf, "minecraft/textures/gui/sprites/hud/heart/half.png"),
        (SpriteId::FoodEmpty, "minecraft/textures/gui/sprites/hud/food_empty.png"),
        (SpriteId::FoodFull, "minecraft/textures/gui/sprites/hud/food_full.png"),
        (SpriteId::FoodHalf, "minecraft/textures/gui/sprites/hud/food_half.png"),
        (SpriteId::EmptyHelmet, "minecraft/textures/gui/sprites/container/slot/helmet.png"),
        (SpriteId::EmptyChestplate, "minecraft/textures/gui/sprites/container/slot/chestplate.png"),
        (SpriteId::EmptyLeggings, "minecraft/textures/gui/sprites/container/slot/leggings.png"),
        (SpriteId::EmptyBoots, "minecraft/textures/gui/sprites/container/slot/boots.png"),
        (SpriteId::EmptyShield, "minecraft/textures/gui/sprites/container/slot/shield.png"),
    ];

    let mut images: Vec<(SpriteId, Vec<u8>, u32, u32)> = Vec::new();
    for &(id, asset_key) in sprites {
        let path = resolve_asset_path(assets_dir, asset_index, asset_key);
        match crate::assets::load_image(&path) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                let w = rgba.width();
                let h = rgba.height();
                images.push((id, rgba.into_raw(), w, h));
            }
            Err(e) => {
                log::warn!("Failed to load sprite {asset_key}: {e}");
                images.push((id, vec![255, 0, 255, 255], 1, 1));
            }
        }
    }

    let inv_path = resolve_asset_path(assets_dir, asset_index, "minecraft/textures/gui/container/inventory.png");
    match crate::assets::load_image(&inv_path) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let full_w = rgba.width();
            let crop_w = INV_TEX_W.min(full_w);
            let crop_h = INV_TEX_H.min(rgba.height());
            let mut cropped = vec![0u8; (crop_w * crop_h * 4) as usize];
            for y in 0..crop_h {
                let src_off = (y * full_w * 4) as usize;
                let dst_off = (y * crop_w * 4) as usize;
                let row_bytes = (crop_w * 4) as usize;
                cropped[dst_off..dst_off + row_bytes]
                    .copy_from_slice(&rgba.as_raw()[src_off..src_off + row_bytes]);
            }
            images.push((SpriteId::InventoryBackground, cropped, crop_w, crop_h));
        }
        Err(e) => {
            log::warn!("Failed to load inventory background: {e}");
            images.push((SpriteId::InventoryBackground, vec![255, 0, 255, 255], 1, 1));
        }
    }

    let atlas_size = 512u32;
    let mut pixels = vec![0u8; (atlas_size * atlas_size * 4) as usize];
    let mut regions = HashMap::new();
    let mut cursor_x = 0u32;
    let mut cursor_y = 0u32;
    let mut row_height = 0u32;

    for (id, data, w, h) in &images {
        if cursor_x + w > atlas_size {
            cursor_x = 0;
            cursor_y += row_height;
            row_height = 0;
        }
        if cursor_y + h > atlas_size {
            log::warn!("Sprite atlas overflow, skipping {:?}", id);
            continue;
        }

        blit_image(&mut pixels, atlas_size, data, *w, cursor_x, cursor_y, *w, *h);

        let inv = 1.0 / atlas_size as f32;
        regions.insert(*id, SpriteRegion {
            u0: cursor_x as f32 * inv,
            v0: cursor_y as f32 * inv,
            u1: (cursor_x + w) as f32 * inv,
            v1: (cursor_y + h) as f32 * inv,
        });

        cursor_x += w;
        row_height = row_height.max(*h);
    }

    let (image, view, allocation) = util::create_gpu_image(device, allocator, atlas_size, atlas_size, "sprite_atlas");
    let (staging_buffer, staging_allocation) = util::create_staging_buffer(device, allocator, &pixels, "sprite_staging");
    util::upload_image(device, queue, command_pool, staging_buffer, image, atlas_size, atlas_size);

    (SpriteAtlas { regions }, image, view, allocation, staging_buffer, Some(staging_allocation))
}

const ITEM_ATLAS_SIZE: u32 = 512;
const ITEM_TILE: u32 = 16;
const ITEM_GRID: u32 = ITEM_ATLAS_SIZE / ITEM_TILE;

fn build_item_atlas(
    device: &ash::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    allocator: &Arc<Mutex<Allocator>>,
    assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
) -> (ItemAtlas, vk::Image, vk::ImageView, Allocation, vk::Buffer, Option<Allocation>) {
    let mut pixels = vec![0u8; (ITEM_ATLAS_SIZE * ITEM_ATLAS_SIZE * 4) as usize];
    let mut regions = HashMap::new();
    let mut slot = 0u32;

    let item_dir = resolve_asset_path(assets_dir, asset_index, "minecraft/textures/item/dummy.png");
    let item_parent = item_dir.parent().unwrap_or(Path::new("."));

    let block_dir = resolve_asset_path(assets_dir, asset_index, "minecraft/textures/block/dummy.png");
    let block_parent = block_dir.parent().unwrap_or(Path::new("."));

    let mut item_names: Vec<String> = Vec::new();

    if let Ok(entries) = std::fs::read_dir(item_parent) {
        for entry in entries.flatten() {
            let fname = entry.file_name().to_string_lossy().to_string();
            if fname.ends_with(".png") {
                item_names.push(fname[..fname.len() - 4].to_string());
            }
        }
    }

    if let Ok(entries) = std::fs::read_dir(block_parent) {
        for entry in entries.flatten() {
            let fname = entry.file_name().to_string_lossy().to_string();
            if fname.ends_with(".png") {
                let name = fname[..fname.len() - 4].to_string();
                if !item_names.contains(&name) {
                    item_names.push(name);
                }
            }
        }
    }

    item_names.sort();

    for name in &item_names {
        if slot >= ITEM_GRID * ITEM_GRID {
            log::warn!("Item atlas full, skipping remaining items");
            break;
        }

        let item_path = item_parent.join(format!("{name}.png"));
        let block_path = block_parent.join(format!("{name}.png"));
        let path = if item_path.exists() { &item_path } else { &block_path };

        let img = match crate::assets::load_image(path) {
            Ok(img) => img.to_rgba8(),
            Err(_) => continue,
        };

        let gx = (slot % ITEM_GRID) * ITEM_TILE;
        let gy = (slot / ITEM_GRID) * ITEM_TILE;

        let src_w = img.width().min(ITEM_TILE);
        let src_h = img.height().min(ITEM_TILE);
        let raw = img.as_raw();

        blit_image(&mut pixels, ITEM_ATLAS_SIZE, raw, img.width(), gx, gy, src_w, src_h);

        let inv = 1.0 / ITEM_ATLAS_SIZE as f32;
        regions.insert(name.clone(), SpriteRegion {
            u0: gx as f32 * inv,
            v0: gy as f32 * inv,
            u1: (gx + ITEM_TILE) as f32 * inv,
            v1: (gy + ITEM_TILE) as f32 * inv,
        });

        slot += 1;
    }

    log::info!("Item atlas: loaded {} textures into {}x{}", regions.len(), ITEM_ATLAS_SIZE, ITEM_ATLAS_SIZE);

    let (image, view, allocation) = util::create_gpu_image(device, allocator, ITEM_ATLAS_SIZE, ITEM_ATLAS_SIZE, "item_atlas");
    let (staging_buffer, staging_allocation) = util::create_staging_buffer(device, allocator, &pixels, "item_staging");
    util::upload_image(device, queue, command_pool, staging_buffer, image, ITEM_ATLAS_SIZE, ITEM_ATLAS_SIZE);

    (ItemAtlas { regions }, image, view, allocation, staging_buffer, Some(staging_allocation))
}

struct TextureResources {
    sampler: vk::Sampler,
    image: vk::Image,
    view: vk::ImageView,
    image_alloc: Option<Allocation>,
    staging_buffer: vk::Buffer,
    staging_alloc: Option<Allocation>,
}

fn destroy_texture_resources(
    device: &ash::Device,
    alloc: &mut gpu_allocator::vulkan::Allocator,
    res: &mut TextureResources,
) {
    unsafe {
        device.destroy_sampler(res.sampler, None);
        device.destroy_image_view(res.view, None);
    }
    if let Some(a) = res.image_alloc.take() { alloc.free(a).ok(); }
    unsafe { device.destroy_image(res.image, None) };
    if let Some(a) = res.staging_alloc.take() { alloc.free(a).ok(); }
    unsafe { device.destroy_buffer(res.staging_buffer, None) };
}

#[allow(clippy::too_many_arguments)]
fn blit_image(
    dst: &mut [u8], dst_stride: u32,
    src: &[u8], src_stride: u32,
    dx: u32, dy: u32, w: u32, h: u32,
) {
    for py in 0..h {
        for px in 0..w {
            let si = ((py * src_stride + px) * 4) as usize;
            let di = (((dy + py) * dst_stride + dx + px) * 4) as usize;
            if si + 4 <= src.len() && di + 4 <= dst.len() {
                dst[di..di + 4].copy_from_slice(&src[si..si + 4]);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn push_quad(
    verts: &mut Vec<Vertex>,
    x: f32, y: f32, w: f32, h: f32,
    u0: f32, v0: f32, u1: f32, v1: f32,
    color: [f32; 4], mode: f32,
    rect_size: [f32; 2], corner_radius: f32,
) {
    let positions = [
        [x, y], [x + w, y], [x, y + h],
        [x + w, y], [x + w, y + h], [x, y + h],
    ];
    let uvs = [
        [u0, v0], [u1, v0], [u0, v1],
        [u1, v0], [u1, v1], [u0, v1],
    ];
    for i in 0..6 {
        verts.push(Vertex {
            pos: positions[i], uv: uvs[i], color, mode, rect_size, corner_radius,
        });
    }
}

fn push_rect(verts: &mut Vec<Vertex>, x: f32, y: f32, w: f32, h: f32, radius: f32, color: [f32; 4]) {
    push_quad(verts, x, y, w, h, 0.0, 0.0, 1.0, 1.0, color, 0.0, [w, h], radius);
}

fn push_text_glyphs(verts: &mut Vec<Vertex>, atlas: &FontAtlas, mut x: f32, y: f32, text: &str, scale: f32, color: [f32; 4]) {
    let s = scale / RASTER_PX;
    for ch in text.chars() {
        let Some(g) = atlas.glyphs.get(&ch) else { continue };
        if g.width_px > 0.0 && g.height_px > 0.0 {
            let gw = g.width_px * s;
            let gh = g.height_px * s;
            let gx = x + g.x_offset * s;
            let gy = y + scale - g.y_offset * s - gh;
            push_quad(verts, gx, gy, gw, gh, g.u0, g.v0, g.u1, g.v1, color, 1.0, [0.0, 0.0], 0.0);
        }
        x += g.advance * s;
    }
}

fn push_icon_glyph(verts: &mut Vec<Vertex>, atlas: &FontAtlas, cx: f32, cy: f32, icon: char, scale: f32, color: [f32; 4]) {
    let Some(g) = atlas.glyphs.get(&icon) else { return };
    let s = scale / RASTER_PX;
    let gw = g.width_px * s;
    let gh = g.height_px * s;
    push_quad(verts, cx - gw / 2.0, cy - gh / 2.0, gw, gh, g.u0, g.v0, g.u1, g.v1, color, 1.0, [0.0, 0.0], 0.0);
}

#[allow(clippy::too_many_arguments)]
fn push_textured_quad(verts: &mut Vec<Vertex>, x: f32, y: f32, w: f32, h: f32, region: &SpriteRegion, tint: [f32; 4], mode: f32) {
    push_quad(verts, x, y, w, h, region.u0, region.v0, region.u1, region.v1, tint, mode, [0.0, 0.0], 0.0);
}

fn push_mc_text(verts: &mut Vec<Vertex>, gm: &GlyphMap, x: f32, y: f32, spans: &[MotdSpan], scale: f32) {
    let (tex_w, tex_h) = gm.dimensions();
    let inv_w = 1.0 / tex_w as f32;
    let inv_h = 1.0 / tex_h as f32;
    let px_scale = scale / gm.cell_h as f32;
    let glyph_h = scale;

    let mut cx = x;
    let mut cy = y;
    let mut line = 0u32;

    for span in spans {
        let shadow_color = [
            span.color[0] * 0.25,
            span.color[1] * 0.25,
            span.color[2] * 0.25,
            span.color[3],
        ];

        for ch in span.text.chars() {
            if ch == '\n' {
                cx = x;
                line += 1;
                cy = y + line as f32 * (glyph_h + 2.0 * px_scale);
                if line >= 2 { return; }
                continue;
            }

            let Some(gi) = gm.glyphs.get(&ch) else { continue };
            let glyph_w = gi.width as f32 * px_scale;

            let u0 = gi.col as f32 * gm.cell_w as f32 * inv_w;
            let v0 = gi.row as f32 * gm.cell_h as f32 * inv_h;
            let u1 = (gi.col as f32 * gm.cell_w as f32 + gi.width as f32) * inv_w;
            let v1 = (gi.row as f32 + 1.0) * gm.cell_h as f32 * inv_h;

            let italic_offset = if span.italic { px_scale } else { 0.0 };

            push_mc_glyph(verts, cx + px_scale, cy + px_scale, glyph_w, glyph_h, u0, v0, u1, v1, shadow_color, italic_offset);
            if span.bold {
                push_mc_glyph(verts, cx + 2.0 * px_scale, cy + px_scale, glyph_w, glyph_h, u0, v0, u1, v1, shadow_color, italic_offset);
            }

            push_mc_glyph(verts, cx, cy, glyph_w, glyph_h, u0, v0, u1, v1, span.color, italic_offset);
            if span.bold {
                push_mc_glyph(verts, cx + px_scale, cy, glyph_w, glyph_h, u0, v0, u1, v1, span.color, italic_offset);
            }

            if span.strikethrough || span.underline {
                let lw = glyph_w + if span.bold { px_scale } else { 0.0 };
                if span.strikethrough {
                    let sy = cy + glyph_h * 0.45;
                    push_quad(verts, cx, sy, lw, px_scale, 0.0, 0.0, 0.0, 0.0, span.color, 0.0, [lw, px_scale], 0.0);
                }
                if span.underline {
                    let uy = cy + glyph_h - px_scale;
                    push_quad(verts, cx, uy, lw, px_scale, 0.0, 0.0, 0.0, 0.0, span.color, 0.0, [lw, px_scale], 0.0);
                }
            }

            let advance = (gi.width as f32 + 1.0) * px_scale;
            cx += if span.bold { advance + px_scale } else { advance };
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn push_mc_glyph(
    verts: &mut Vec<Vertex>,
    x: f32, y: f32, w: f32, h: f32,
    u0: f32, v0: f32, u1: f32, v1: f32,
    color: [f32; 4], italic_offset: f32,
) {
    let positions = [
        [x + italic_offset, y], [x + w + italic_offset, y], [x, y + h],
        [x + w + italic_offset, y], [x + w, y + h], [x, y + h],
    ];
    let uvs = [
        [u0, v0], [u1, v0], [u0, v1],
        [u1, v0], [u1, v1], [u0, v1],
    ];
    for i in 0..6 {
        verts.push(Vertex {
            pos: positions[i], uv: uvs[i], color, mode: 4.0,
            rect_size: [0.0, 0.0], corner_radius: 0.0,
        });
    }
}

fn create_pipeline(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> vk::Pipeline {
    let vert_spv = shader::include_spirv!("menu_overlay.vert.spv");
    let frag_spv = shader::include_spirv!("menu_overlay.frag.spv");

    let vert_module = shader::create_shader_module(device, vert_spv);
    let frag_module = shader::create_shader_module(device, frag_spv);

    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(vert_module)
            .name(c"main"),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(frag_module)
            .name(c"main"),
    ];

    let binding_descs = [vk::VertexInputBindingDescription {
        binding: 0,
        stride: VERTEX_SIZE as u32,
        input_rate: vk::VertexInputRate::VERTEX,
    }];

    let attr_descs = [
        vk::VertexInputAttributeDescription { location: 0, binding: 0, format: vk::Format::R32G32_SFLOAT, offset: 0 },
        vk::VertexInputAttributeDescription { location: 1, binding: 0, format: vk::Format::R32G32_SFLOAT, offset: 8 },
        vk::VertexInputAttributeDescription { location: 2, binding: 0, format: vk::Format::R32G32B32A32_SFLOAT, offset: 16 },
        vk::VertexInputAttributeDescription { location: 3, binding: 0, format: vk::Format::R32_SFLOAT, offset: 32 },
        vk::VertexInputAttributeDescription { location: 4, binding: 0, format: vk::Format::R32G32_SFLOAT, offset: 36 },
        vk::VertexInputAttributeDescription { location: 5, binding: 0, format: vk::Format::R32_SFLOAT, offset: 44 },
    ];

    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
        .vertex_binding_descriptions(&binding_descs)
        .vertex_attribute_descriptions(&attr_descs);

    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST);

    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);

    let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(vk::CullModeFlags::NONE)
        .line_width(1.0);

    let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);

    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
        .depth_test_enable(false)
        .depth_write_enable(false);

    let blend_attachment = [vk::PipelineColorBlendAttachmentState {
        blend_enable: vk::TRUE,
        src_color_blend_factor: vk::BlendFactor::ONE,
        dst_color_blend_factor: vk::BlendFactor::ONE_MINUS_SRC_ALPHA,
        color_blend_op: vk::BlendOp::ADD,
        src_alpha_blend_factor: vk::BlendFactor::ONE,
        dst_alpha_blend_factor: vk::BlendFactor::ONE_MINUS_SRC_ALPHA,
        alpha_blend_op: vk::BlendOp::ADD,
        color_write_mask: vk::ColorComponentFlags::RGBA,
    }];
    let color_blending = vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachment);

    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic_state = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

    let pipeline_info = [vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&input_assembly)
        .viewport_state(&viewport_state)
        .rasterization_state(&rasterizer)
        .multisample_state(&multisampling)
        .depth_stencil_state(&depth_stencil)
        .color_blend_state(&color_blending)
        .dynamic_state(&dynamic_state)
        .layout(layout)
        .render_pass(render_pass)
        .subpass(0)];

    let pipeline = unsafe {
        device.create_graphics_pipelines(vk::PipelineCache::null(), &pipeline_info, None)
    }
    .expect("failed to create menu overlay pipeline")[0];

    unsafe {
        device.destroy_shader_module(vert_module, None);
        device.destroy_shader_module(frag_module, None);
    }

    pipeline
}
