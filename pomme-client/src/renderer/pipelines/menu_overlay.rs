use std::collections::HashMap;
use std::path::Path;
use std::slice;
use std::sync::{Arc, Mutex};

use pomme_gpu_allocator::vulkan::{Allocation, Allocator};
use pyronyx::vk;

use crate::assets::{AssetIndex, resolve_asset_path};
use crate::renderer::{shader, util};
use crate::ui::font::GlyphMap;
use crate::ui::text::TextSpan;

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
pub const ICON_USERS: char = '\u{f0c0}';
pub const ICON_LANGUAGE: char = '\u{f1ab}';
pub const ICON_UNIVERSAL_ACCESS: char = '\u{f29a}';

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
const VERTEX_SIZE: usize = size_of::<Vertex>();

struct DrawOp {
    start: u32,
    count: u32,
    scissor: Option<[f32; 4]>,
}

struct GlyphEntry {
    u0: f32,
    v0: f32,
    u1: f32,
    v1: f32,
    width_px: f32,
    height_px: f32,
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
    let icon_chars: Vec<(char, &fontdue::Font)> = [
        ICON_USER,
        ICON_LINK,
        ICON_PAINTBRUSH,
        ICON_GEAR,
        ICON_GLOBE,
        ICON_COMMENT,
        ICON_CODE,
        ICON_CHECK,
        ICON_USERS,
        ICON_LANGUAGE,
        ICON_UNIVERSAL_ACCESS,
    ]
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
        glyphs.insert(
            *ch,
            GlyphEntry {
                u0: cursor_x as f32 * inv,
                v0: cursor_y as f32 * inv,
                u1: (cursor_x + metrics.width as u32) as f32 * inv,
                v1: (cursor_y + metrics.height as u32) as f32 * inv,
                width_px: metrics.width as f32,
                height_px: metrics.height as f32,
            },
        );

        row_height = row_height.max(metrics.height as u32);
        cursor_x += metrics.width as u32 + 1;
    }

    FontAtlas { glyphs, pixels }
}

/// Extract an 8x8 RGBA player face (front face at (8,8) with the hat layer at
/// (40,8) composited over it) from a wide player skin. `None` if the skin is
/// too small. Shared by the Steve-head sprite and live friend faces.
pub(crate) fn extract_face_8x8(rgba: &[u8], sw: u32, sh: u32) -> Option<Vec<u8>> {
    // Skins are 64x64 (or 64x32 legacy); both have the face/hat in the top-left.
    if sw < 48 || sh < 16 {
        return None;
    }
    let mut out = vec![0u8; 8 * 8 * 4];
    for y in 0..8u32 {
        for x in 0..8u32 {
            let face_off = (((8 + y) * sw + (8 + x)) * 4) as usize;
            let dst = ((y * 8 + x) * 4) as usize;
            out[dst..dst + 4].copy_from_slice(&rgba[face_off..face_off + 4]);
            // Composite hat over face (ignore fully transparent hat pixels).
            let hat_off = (((8 + y) * sw + (40 + x)) * 4) as usize;
            let ha = rgba[hat_off + 3];
            if ha > 0 {
                let a = ha as f32 / 255.0;
                for c in 0..3 {
                    let fg = rgba[hat_off + c] as f32;
                    let bg = out[dst + c] as f32;
                    out[dst + c] = (fg * a + bg * (1.0 - a)) as u8;
                }
                out[dst + 3] = out[dst + 3].max(ha);
            }
        }
    }
    Some(out)
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
    item_placeholder: Option<TextureResources>,
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
    favicon_image: vk::Image,
    favicon_view: vk::ImageView,
    favicon_sampler: vk::Sampler,
    favicon_allocation: Option<Allocation>,
    favicon_regions: std::collections::HashMap<String, [f32; 4]>,
    favicon_atlas_size: u32,
}

impl MenuOverlayPipeline {
    pub fn new(
        device: &vk::Device,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        render_pass: vk::RenderPass,
        allocator: &Arc<Mutex<Allocator>>,
        jar_assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
    ) -> Self {
        let atlas = build_font_atlas();

        let globals_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::UniformBuffer,
            vk::ShaderStageFlags::Vertex | vk::ShaderStageFlags::Fragment,
        );

        let tex_bindings = [
            vk::DescriptorSetLayoutBinding {
                binding: 0,
                descriptor_type: vk::DescriptorType::CombinedImageSampler,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::Fragment,
                ..Default::default()
            },
            vk::DescriptorSetLayoutBinding {
                binding: 1,
                descriptor_type: vk::DescriptorType::CombinedImageSampler,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::Fragment,
                ..Default::default()
            },
            vk::DescriptorSetLayoutBinding {
                binding: 2,
                descriptor_type: vk::DescriptorType::CombinedImageSampler,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::Fragment,
                ..Default::default()
            },
            vk::DescriptorSetLayoutBinding {
                binding: 3,
                descriptor_type: vk::DescriptorType::CombinedImageSampler,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::Fragment,
                ..Default::default()
            },
            vk::DescriptorSetLayoutBinding {
                binding: 4,
                descriptor_type: vk::DescriptorType::CombinedImageSampler,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::Fragment,
                ..Default::default()
            },
            vk::DescriptorSetLayoutBinding {
                binding: 5,
                descriptor_type: vk::DescriptorType::CombinedImageSampler,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::Fragment,
                ..Default::default()
            },
        ];
        let tex_layout_info = vk::DescriptorSetLayoutCreateInfo {
            binding_count: tex_bindings.len() as u32,
            bindings: tex_bindings.as_ptr(),
            ..Default::default()
        };
        let tex_layout = device
            .create_descriptor_set_layout(&tex_layout_info, None)
            .expect("failed to create texture descriptor set layout");

        let layouts = [globals_layout, tex_layout];
        let layout_info = vk::PipelineLayoutCreateInfo {
            set_layout_count: layouts.len() as u32,
            set_layouts: layouts.as_ptr(),
            ..Default::default()
        };
        let pipeline_layout = device
            .create_pipeline_layout(&layout_info, None)
            .expect("failed to create menu overlay pipeline layout");

        let pipeline = create_pipeline(device, render_pass, pipeline_layout);

        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UniformBuffer,
                descriptor_count: 1,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::CombinedImageSampler,
                descriptor_count: 6,
            },
        ];
        let pool_info = vk::DescriptorPoolCreateInfo {
            max_sets: 2,
            pool_size_count: pool_sizes.len() as u32,
            pool_sizes: pool_sizes.as_ptr(),
            ..Default::default()
        };
        let descriptor_pool = device
            .create_descriptor_pool(&pool_info, None)
            .expect("failed to create menu overlay descriptor pool");

        let globals_alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: 1,
            set_layouts: &globals_layout,
            ..Default::default()
        };
        let mut globals_set = vk::DescriptorSet::null();
        device
            .allocate_descriptor_sets(&globals_alloc_info, slice::from_mut(&mut globals_set))
            .expect("failed to allocate globals descriptor set");

        let tex_alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: 1,
            set_layouts: &tex_layout,
            ..Default::default()
        };
        let mut tex_set = vk::DescriptorSet::null();
        device
            .allocate_descriptor_sets(&tex_alloc_info, slice::from_mut(&mut tex_set))
            .expect("failed to allocate texture descriptor set");

        let (globals_buffer, globals_allocation) =
            util::create_uniform_buffer(device, allocator, 8, "menu_globals");

        let buf_info = vk::DescriptorBufferInfo {
            buffer: globals_buffer,
            offset: 0,
            range: 8,
        };
        let write = vk::WriteDescriptorSet {
            dst_set: globals_set,
            dst_binding: 0,
            descriptor_count: 1,
            descriptor_type: vk::DescriptorType::UniformBuffer,
            buffer_info: &buf_info,
            ..Default::default()
        };
        device.update_descriptor_sets(&[write], &[]);

        let (font_image, font_view, font_alloc) = util::create_gpu_image_with_format(
            device,
            allocator,
            ATLAS_SIZE,
            ATLAS_SIZE,
            vk::Format::R8G8B8A8Unorm,
            "menu_font_atlas",
        );

        let (font_staging_buffer, font_staging_alloc) =
            util::create_staging_buffer(device, allocator, &atlas.pixels, "menu_font_staging");

        util::upload_image(
            device,
            queue,
            command_pool,
            font_staging_buffer,
            font_image,
            ATLAS_SIZE,
            ATLAS_SIZE,
        );

        let font_sampler = unsafe { util::create_linear_sampler(device) };

        let (
            sprite_atlas_data,
            sprite_image,
            sprite_view,
            sprite_alloc,
            sprite_staging_buffer,
            sprite_staging_alloc,
        ) = build_sprite_atlas(
            device,
            queue,
            command_pool,
            allocator,
            jar_assets_dir,
            asset_index,
        );

        let sprite_sampler = unsafe { util::create_nearest_sampler(device) };

        let (item_image, item_view, item_alloc) =
            util::create_gpu_image(device, allocator, 1, 1, "item_atlas_placeholder");
        let (item_staging_buffer, item_staging_alloc) = util::create_staging_buffer(
            device,
            allocator,
            &[0u8, 0, 0, 0],
            "item_atlas_placeholder_staging",
        );
        util::upload_image(
            device,
            queue,
            command_pool,
            item_staging_buffer,
            item_image,
            1,
            1,
        );
        let item_sampler = unsafe { util::create_nearest_sampler(device) };
        let item_placeholder = Some(TextureResources {
            sampler: item_sampler,
            image: item_image,
            view: item_view,
            image_alloc: Some(item_alloc),
            staging_buffer: item_staging_buffer,
            staging_alloc: Some(item_staging_alloc),
        });

        let mc_glyph_map = GlyphMap::load(jar_assets_dir, asset_index);
        crate::lang::load(jar_assets_dir);
        let (
            mc_font_image,
            mc_font_view,
            mc_font_alloc,
            mc_font_staging_buffer,
            mc_font_staging_alloc,
        ) = if let Some(ref gm) = mc_glyph_map {
            let (w, h) = gm.dimensions();
            let (img, view, alloc) =
                util::create_gpu_image(device, allocator, w, h, "mc_font_atlas");
            let (stg_buf, stg_alloc) =
                util::create_staging_buffer(device, allocator, gm.raw_pixels(), "mc_font_staging");
            util::upload_image(device, queue, command_pool, stg_buf, img, w, h);
            (img, view, Some(alloc), stg_buf, Some(stg_alloc))
        } else {
            let (img, view, alloc) =
                util::create_gpu_image(device, allocator, 1, 1, "mc_font_dummy");
            let dummy = [0u8; 4];
            let (stg_buf, stg_alloc) =
                util::create_staging_buffer(device, allocator, &dummy, "mc_font_dummy_stg");
            util::upload_image(device, queue, command_pool, stg_buf, img, 1, 1);
            (img, view, Some(alloc), stg_buf, Some(stg_alloc))
        };
        let mc_font_sampler = unsafe { util::create_nearest_sampler(device) };

        let font_img_info = vk::DescriptorImageInfo {
            sampler: font_sampler,
            image_view: font_view,
            image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
        };
        let sprite_img_info = vk::DescriptorImageInfo {
            sampler: sprite_sampler,
            image_view: sprite_view,
            image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
        };
        let item_img_info = vk::DescriptorImageInfo {
            sampler: item_sampler,
            image_view: item_view,
            image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
        };
        let mc_font_img_info = vk::DescriptorImageInfo {
            sampler: mc_font_sampler,
            image_view: mc_font_view,
            image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
        };

        let (favicon_image, favicon_view, favicon_alloc) = util::create_gpu_image_with_format(
            device,
            allocator,
            1,
            1,
            vk::Format::R8G8B8A8Srgb,
            "favicon_placeholder",
        );
        let (fav_staging, fav_staging_alloc) = util::create_staging_buffer(
            device,
            allocator,
            &[255u8, 255, 255, 255],
            "favicon_staging",
        );
        util::upload_image(
            device,
            queue,
            command_pool,
            fav_staging,
            favicon_image,
            1,
            1,
        );
        device.destroy_buffer(fav_staging, None);
        allocator.lock().unwrap().free(fav_staging_alloc).ok();
        let favicon_sampler = unsafe { util::create_nearest_sampler(device) };

        let favicon_img_info = vk::DescriptorImageInfo {
            sampler: favicon_sampler,
            image_view: favicon_view,
            image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
        };

        let writes = [
            vk::WriteDescriptorSet {
                dst_set: tex_set,
                dst_binding: 0,
                descriptor_count: 1,
                descriptor_type: vk::DescriptorType::CombinedImageSampler,
                image_info: &font_img_info,
                ..Default::default()
            },
            vk::WriteDescriptorSet {
                dst_set: tex_set,
                dst_binding: 1,
                descriptor_count: 1,
                descriptor_type: vk::DescriptorType::CombinedImageSampler,
                image_info: &sprite_img_info,
                ..Default::default()
            },
            vk::WriteDescriptorSet {
                dst_set: tex_set,
                dst_binding: 2,
                descriptor_count: 1,
                descriptor_type: vk::DescriptorType::CombinedImageSampler,
                image_info: &item_img_info,
                ..Default::default()
            },
            vk::WriteDescriptorSet {
                dst_set: tex_set,
                dst_binding: 3,
                descriptor_count: 1,
                descriptor_type: vk::DescriptorType::CombinedImageSampler,
                image_info: &mc_font_img_info,
                ..Default::default()
            },
            vk::WriteDescriptorSet {
                dst_set: tex_set,
                dst_binding: 4,
                descriptor_count: 1,
                descriptor_type: vk::DescriptorType::CombinedImageSampler,
                image_info: &font_img_info,
                ..Default::default()
            },
            vk::WriteDescriptorSet {
                dst_set: tex_set,
                dst_binding: 5,
                descriptor_count: 1,
                descriptor_type: vk::DescriptorType::CombinedImageSampler,
                image_info: &favicon_img_info,
                ..Default::default()
            },
        ];
        device.update_descriptor_sets(&writes, &[]);

        let (vertex_buffer, vertex_allocation) = util::create_host_buffer(
            device,
            allocator,
            (MAX_VERTICES * VERTEX_SIZE) as u64,
            vk::BufferUsageFlags::VertexBuffer,
            "menu_vertices",
        );

        Self {
            pipeline,
            pipeline_layout,
            globals_layout,
            tex_layout,
            descriptor_pool,
            globals_set,
            tex_set,
            globals_buffer,
            globals_allocation: Some(globals_allocation),
            font_image,
            font_view,
            font_sampler,
            font_allocation: Some(font_alloc),
            font_staging_buffer,
            font_staging_allocation: Some(font_staging_alloc),
            sprite_image,
            sprite_view,
            sprite_sampler,
            sprite_allocation: Some(sprite_alloc),
            sprite_staging_buffer,
            sprite_staging_allocation: sprite_staging_alloc,
            sprite_atlas: sprite_atlas_data,
            item_placeholder,
            mc_font_image,
            mc_font_view,
            mc_font_sampler,
            mc_font_allocation: mc_font_alloc,
            mc_font_staging_buffer,
            mc_font_staging_allocation: mc_font_staging_alloc,
            mc_glyph_map,
            vertex_buffer,
            vertex_allocation: Some(vertex_allocation),
            atlas,
            favicon_image,
            favicon_view,
            favicon_sampler,
            favicon_allocation: Some(favicon_alloc),
            favicon_regions: std::collections::HashMap::new(),
            favicon_atlas_size: 1,
        }
    }

    pub fn draw(
        &mut self,
        cmd: vk::CommandBuffer,
        screen_w: f32,
        screen_h: f32,
        elements: &[MenuElement],
        item_atlas_uvs: &HashMap<String, [f32; 4]>,
    ) {
        self.draw_from(cmd, screen_w, screen_h, elements, item_atlas_uvs, 0);
    }

    /// Like [`draw`], but writes vertices starting at `vertex_base` in the
    /// shared vertex buffer and returns the next free index, so it can be
    /// called more than once per frame (e.g. a backdrop before the blur and
    /// a dialog after).
    pub fn draw_from(
        &mut self,
        cmd: vk::CommandBuffer,
        screen_w: f32,
        screen_h: f32,
        elements: &[MenuElement],
        item_atlas_uvs: &HashMap<String, [f32; 4]>,
        vertex_base: u32,
    ) -> u32 {
        let globals: [f32; 2] = [screen_w, screen_h];
        self.globals_allocation
            .as_mut()
            .unwrap()
            .mapped_slice_mut()
            .unwrap()[..8]
            .copy_from_slice(bytemuck::cast_slice(&globals));

        let mut vertices: Vec<Vertex> = Vec::with_capacity(elements.len() * 24);
        let mut deferred_tooltips: Vec<&MenuElement> = Vec::new();
        let mut draw_ops: Vec<DrawOp> = Vec::new();
        let mut scissor_stack: Vec<[f32; 4]> = Vec::new();
        let mut cmd_start: u32 = 0;

        for elem in elements {
            if matches!(
                elem,
                MenuElement::Tooltip { .. } | MenuElement::TooltipLines { .. }
            ) {
                deferred_tooltips.push(elem);
                continue;
            }
            if matches!(
                elem,
                MenuElement::ScissorPush { .. } | MenuElement::ScissorPop
            ) {
                let count = vertices.len() as u32 - cmd_start;
                if count > 0 {
                    draw_ops.push(DrawOp {
                        start: cmd_start,
                        count,
                        scissor: scissor_stack.last().copied(),
                    });
                }
                cmd_start = vertices.len() as u32;
                if let MenuElement::ScissorPush { x, y, w, h } = elem {
                    // Nested regions clip to the intersection with the enclosing one.
                    let rect = match scissor_stack.last() {
                        Some(outer) => {
                            let x0 = x.max(outer[0]);
                            let y0 = y.max(outer[1]);
                            let x1 = (x + w).min(outer[0] + outer[2]);
                            let y1 = (y + h).min(outer[1] + outer[3]);
                            [x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)]
                        }
                        None => [*x, *y, *w, *h],
                    };
                    scissor_stack.push(rect);
                } else {
                    scissor_stack.pop();
                }
                continue;
            }
            match elem {
                MenuElement::Rect {
                    x,
                    y,
                    w,
                    h,
                    corner_radius,
                    color,
                } => {
                    push_rect(&mut vertices, *x, *y, *w, *h, *corner_radius, *color);
                }
                MenuElement::Text {
                    x,
                    y,
                    text,
                    scale,
                    color,
                    centered,
                } => {
                    if let Some(ref gm) = self.mc_glyph_map {
                        let start_x = if *centered {
                            *x - self.mc_text_width(text, *scale) / 2.0
                        } else {
                            *x
                        };
                        let span = TextSpan::new(text.clone(), *color);
                        push_mc_text(&mut vertices, gm, start_x, *y, &[span], *scale, true);
                    }
                }
                MenuElement::TextFlat {
                    x,
                    y,
                    text,
                    scale,
                    color,
                } => {
                    if let Some(ref gm) = self.mc_glyph_map {
                        let span = TextSpan::new(text.clone(), *color);
                        push_mc_text(&mut vertices, gm, *x, *y, &[span], *scale, false);
                    }
                }
                MenuElement::Icon {
                    x,
                    y,
                    icon,
                    scale,
                    color,
                } => {
                    push_icon_glyph(&mut vertices, &self.atlas, *x, *y, *icon, *scale, *color);
                }
                MenuElement::Image {
                    x,
                    y,
                    w,
                    h,
                    sprite,
                    tint,
                } => {
                    if let Some(region) = self.sprite_atlas.regions.get(sprite) {
                        push_textured_quad(&mut vertices, *x, *y, *w, *h, region, *tint, 2.0);
                    }
                }
                MenuElement::NineSlice {
                    x,
                    y,
                    w,
                    h,
                    sprite,
                    border,
                    tint,
                } => {
                    if let Some(region) = self.sprite_atlas.regions.get(sprite) {
                        push_nine_slice(&mut vertices, *x, *y, *w, *h, region, *border, *tint);
                    }
                }
                MenuElement::TiledImage {
                    x,
                    y,
                    w,
                    h,
                    sprite,
                    tile_size,
                    tint,
                } => {
                    if let Some(region) = self.sprite_atlas.regions.get(sprite) {
                        let tiles_x = (*w / *tile_size).ceil() as u32;
                        let tiles_y = (*h / *tile_size).ceil() as u32;
                        for ty in 0..tiles_y {
                            for tx in 0..tiles_x {
                                let qx = *x + tx as f32 * *tile_size;
                                let qy = *y + ty as f32 * *tile_size;
                                let qw = (*tile_size).min(*x + *w - qx);
                                let qh = (*tile_size).min(*y + *h - qy);
                                let u_frac = qw / *tile_size;
                                let v_frac = qh / *tile_size;
                                let clipped = SpriteRegion {
                                    u0: region.u0,
                                    v0: region.v0,
                                    u1: region.u0 + (region.u1 - region.u0) * u_frac,
                                    v1: region.v0 + (region.v1 - region.v0) * v_frac,
                                    src_w: region.src_w,
                                    src_h: region.src_h,
                                    nine_slice_border: region.nine_slice_border,
                                };
                                push_textured_quad(
                                    &mut vertices,
                                    qx,
                                    qy,
                                    qw,
                                    qh,
                                    &clipped,
                                    *tint,
                                    2.0,
                                );
                            }
                        }
                    }
                }
                MenuElement::ItemIcon {
                    x,
                    y,
                    w,
                    h,
                    item_name,
                    tint,
                } => {
                    if let Some(uv) = item_atlas_uvs.get(item_name) {
                        push_quad(
                            &mut vertices,
                            *x,
                            *y,
                            *w,
                            *h,
                            uv[0],
                            uv[1],
                            uv[2],
                            uv[3],
                            *tint,
                            3.0,
                            [0.0, 0.0],
                            0.0,
                        );
                    }
                }
                MenuElement::McText {
                    x,
                    y,
                    spans,
                    scale,
                    centered,
                } => {
                    if let Some(ref gm) = self.mc_glyph_map {
                        let start_x = if *centered {
                            let total: f32 = spans
                                .iter()
                                .map(|s| self.mc_text_width(&s.text, *scale))
                                .sum();
                            *x - total / 2.0
                        } else {
                            *x
                        };
                        push_mc_text(&mut vertices, gm, start_x, *y, spans, *scale, true);
                    }
                }
                MenuElement::GradientRect {
                    x,
                    y,
                    w,
                    h,
                    corner_radius,
                    color_top,
                    color_bottom,
                } => {
                    push_gradient_rect(
                        &mut vertices,
                        *x,
                        *y,
                        *w,
                        *h,
                        *corner_radius,
                        *color_top,
                        *color_bottom,
                    );
                }
                MenuElement::FrostedRect {
                    x,
                    y,
                    w,
                    h,
                    corner_radius,
                    tint,
                } => {
                    push_quad(
                        &mut vertices,
                        *x,
                        *y,
                        *w,
                        *h,
                        0.0,
                        0.0,
                        1.0,
                        1.0,
                        *tint,
                        5.0,
                        [*w, *h],
                        *corner_radius,
                    );
                }
                MenuElement::Favicon {
                    x,
                    y,
                    size,
                    address,
                } => {
                    push_atlas_image(
                        &mut vertices,
                        &self.favicon_regions,
                        &self.sprite_atlas,
                        address,
                        SpriteId::UnknownServer,
                        *x,
                        *y,
                        *size,
                    );
                }
                MenuElement::SkinFace { x, y, size, uuid } => {
                    push_atlas_image(
                        &mut vertices,
                        &self.favicon_regions,
                        &self.sprite_atlas,
                        uuid,
                        SpriteId::SteveHead,
                        *x,
                        *y,
                        *size,
                    );
                }
                _ => {}
            }
        }

        for elem in &deferred_tooltips {
            if let MenuElement::Tooltip {
                x,
                y,
                text,
                scale,
                screen_w,
                screen_h,
            } = elem
                && let Some(ref gm) = self.mc_glyph_map
            {
                let px = *scale / gm.cell_h as f32;
                let padding = 3.0 * px;
                let margin = 9.0 * px;
                let line_h = *scale + 2.0 * px;
                let max_w = (*screen_w * 0.4).max(100.0);

                let words: Vec<&str> = text.split_whitespace().collect();
                let mut lines: Vec<String> = Vec::new();
                let mut current = String::new();
                let space_w = self.mc_text_width(" ", *scale);
                for word in &words {
                    let word_w = self.mc_text_width(word, *scale);
                    let test_w = if current.is_empty() {
                        word_w
                    } else {
                        self.mc_text_width(&current, *scale) + space_w + word_w
                    };
                    if !current.is_empty() && test_w > max_w {
                        lines.push(current);
                        current = word.to_string();
                    } else {
                        if !current.is_empty() {
                            current.push(' ');
                        }
                        current.push_str(word);
                    }
                }
                if !current.is_empty() {
                    lines.push(current);
                }

                let content_w = lines
                    .iter()
                    .map(|l| (self.mc_text_width(l, *scale) + px).ceil())
                    .fold(0.0f32, f32::max);
                let content_h = lines.len() as f32 * line_h - 2.0 * px;

                let mut text_x = *x + 12.0;
                let mut text_y = *y - 12.0;
                if text_x + content_w > *screen_w {
                    text_x = (*x - 24.0 - content_w).max(4.0);
                }
                if text_y + content_h + 3.0 > *screen_h {
                    text_y = *screen_h - content_h - 3.0;
                }

                let bg_x = text_x - padding - margin - padding;
                let bg_y = text_y - padding - margin - padding;
                let bg_w = content_w + (padding + margin + padding) * 2.0;
                let bg_h = content_h + (padding + margin + padding) * 2.0;
                let bg_border = margin;
                let frame_border = 10.0 * px;

                let white = [1.0f32; 4];
                if let Some(bg) = self.sprite_atlas.regions.get(&SpriteId::TooltipBackground) {
                    push_nine_slice(&mut vertices, bg_x, bg_y, bg_w, bg_h, bg, bg_border, white);
                }
                if let Some(frame) = self.sprite_atlas.regions.get(&SpriteId::TooltipFrame) {
                    push_nine_slice(
                        &mut vertices,
                        bg_x,
                        bg_y,
                        bg_w,
                        bg_h,
                        frame,
                        frame_border,
                        white,
                    );
                }

                for (i, line) in lines.iter().enumerate() {
                    let span = TextSpan::new(line.clone(), white);
                    push_mc_text(
                        &mut vertices,
                        gm,
                        text_x,
                        text_y + i as f32 * line_h,
                        &[span],
                        *scale,
                        true,
                    );
                }
            }
            if let MenuElement::TooltipLines {
                x,
                y,
                lines,
                scale,
                screen_w,
                screen_h,
            } = elem
                && let Some(ref gm) = self.mc_glyph_map
            {
                let px = *scale / gm.cell_h as f32;
                let padding = 3.0 * px;
                let margin = 9.0 * px;
                let line_h = *scale + 2.0 * px;

                let content_w = lines
                    .iter()
                    .map(|l| (self.mc_text_width(&l.text, *scale) + px).ceil())
                    .fold(0.0f32, f32::max);
                let content_h = lines.len() as f32 * line_h - 2.0 * px;

                let mut text_x = *x + 12.0;
                let mut text_y = *y - 12.0;
                if text_x + content_w > *screen_w {
                    text_x = (*x - 24.0 - content_w).max(4.0);
                }
                if text_y + content_h + 3.0 > *screen_h {
                    text_y = *screen_h - content_h - 3.0;
                }

                let bg_x = text_x - padding - margin - padding;
                let bg_y = text_y - padding - margin - padding;
                let bg_w = content_w + (padding + margin + padding) * 2.0;
                let bg_h = content_h + (padding + margin + padding) * 2.0;
                let bg_border = margin;
                let frame_border = 10.0 * px;
                let white = [1.0f32; 4];

                if let Some(bg) = self.sprite_atlas.regions.get(&SpriteId::TooltipBackground) {
                    push_nine_slice(&mut vertices, bg_x, bg_y, bg_w, bg_h, bg, bg_border, white);
                }
                if let Some(frame) = self.sprite_atlas.regions.get(&SpriteId::TooltipFrame) {
                    push_nine_slice(
                        &mut vertices,
                        bg_x,
                        bg_y,
                        bg_w,
                        bg_h,
                        frame,
                        frame_border,
                        white,
                    );
                }

                for (i, line) in lines.iter().enumerate() {
                    let span = TextSpan::new(line.text.clone(), line.color);
                    push_mc_text(
                        &mut vertices,
                        gm,
                        text_x,
                        text_y + i as f32 * line_h,
                        &[span],
                        *scale,
                        true,
                    );
                }
            }
        }

        let final_count = vertices.len() as u32 - cmd_start;
        if final_count > 0 {
            draw_ops.push(DrawOp {
                start: cmd_start,
                count: final_count,
                scissor: scissor_stack.last().copied(),
            });
        }

        if draw_ops.is_empty() {
            return vertex_base;
        }

        let written = if vertices.is_empty() {
            0
        } else {
            let avail = MAX_VERTICES.saturating_sub(vertex_base as usize);
            let count = vertices.len().min(avail);
            let byte_data = bytemuck::cast_slice(&vertices[..count]);
            let byte_off = vertex_base as usize * VERTEX_SIZE;
            self.vertex_allocation
                .as_mut()
                .unwrap()
                .mapped_slice_mut()
                .unwrap()[byte_off..byte_off + byte_data.len()]
                .copy_from_slice(byte_data);
            count
        };

        let default_scissor = vk::Rect2D {
            offset: vk::Offset2D { x: 0, y: 0 },
            extent: vk::Extent2D {
                width: screen_w as u32,
                height: screen_h as u32,
            },
        };

        if !draw_ops.is_empty() {
            cmd.bind_pipeline(vk::PipelineBindPoint::Graphics, self.pipeline);
            cmd.bind_descriptor_sets(
                vk::PipelineBindPoint::Graphics,
                self.pipeline_layout,
                0,
                &[self.globals_set, self.tex_set],
                &[],
            );
            cmd.bind_vertex_buffers(0, &[self.vertex_buffer], &[0]);
        }
        for op in &draw_ops {
            let rect = if let Some(s) = op.scissor {
                vk::Rect2D {
                    offset: vk::Offset2D {
                        x: s[0] as i32,
                        y: s[1] as i32,
                    },
                    extent: vk::Extent2D {
                        width: s[2] as u32,
                        height: s[3] as u32,
                    },
                }
            } else {
                default_scissor
            };
            cmd.set_scissor(0, &[rect]);
            cmd.draw(op.count, 1, vertex_base + op.start, 0);
        }
        cmd.set_scissor(0, &[default_scissor]);
        vertex_base + written as u32
    }

    pub fn set_item_atlas(&self, device: &vk::Device, view: vk::ImageView, sampler: vk::Sampler) {
        let info = vk::DescriptorImageInfo {
            sampler,
            image_view: view,
            image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
        };
        let write = vk::WriteDescriptorSet {
            dst_set: self.tex_set,
            dst_binding: 2,
            descriptor_count: 1,
            descriptor_type: vk::DescriptorType::CombinedImageSampler,
            image_info: &info,
            ..Default::default()
        };
        device.update_descriptor_sets(&[write], &[]);
    }

    pub fn set_blur_texture(&self, device: &vk::Device, view: vk::ImageView, sampler: vk::Sampler) {
        let info = vk::DescriptorImageInfo {
            sampler,
            image_view: view,
            image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
        };
        let write = vk::WriteDescriptorSet {
            dst_set: self.tex_set,
            dst_binding: 4,
            descriptor_count: 1,
            descriptor_type: vk::DescriptorType::CombinedImageSampler,
            image_info: &info,
            ..Default::default()
        };
        device.update_descriptor_sets(&[write], &[]);
    }

    pub fn update_favicon_atlas(
        &mut self,
        device: &vk::Device,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        allocator: &Arc<Mutex<Allocator>>,
        favicons: &[(String, Vec<u8>, u32)],
    ) {
        if favicons.is_empty() {
            return;
        }

        let icon_size = 64u32;
        let cols = (favicons.len() as f32).sqrt().ceil() as u32;
        let rows = (favicons.len() as u32).div_ceil(cols);
        let atlas_w = cols * icon_size;
        let atlas_h = rows * icon_size;
        let mut pixels = vec![0u8; (atlas_w * atlas_h * 4) as usize];
        let mut regions = std::collections::HashMap::new();

        for (i, (addr, rgba, src_size)) in favicons.iter().enumerate() {
            let col = i as u32 % cols;
            let row = i as u32 / cols;
            let dst_x = col * icon_size;
            let dst_y = row * icon_size;

            for py in 0..icon_size {
                for px in 0..icon_size {
                    let sx = (px * src_size / icon_size).min(src_size - 1);
                    let sy = (py * src_size / icon_size).min(src_size - 1);
                    let src_off = ((sy * src_size + sx) * 4) as usize;
                    let dst_off = (((dst_y + py) * atlas_w + dst_x + px) * 4) as usize;
                    if src_off + 3 < rgba.len() && dst_off + 3 < pixels.len() {
                        pixels[dst_off..dst_off + 4].copy_from_slice(&rgba[src_off..src_off + 4]);
                    }
                }
            }

            let u0 = dst_x as f32 / atlas_w as f32;
            let v0 = dst_y as f32 / atlas_h as f32;
            let u1 = (dst_x + icon_size) as f32 / atlas_w as f32;
            let v1 = (dst_y + icon_size) as f32 / atlas_h as f32;
            regions.insert(addr.clone(), [u0, v0, u1, v1]);
        }

        queue.wait_idle().unwrap();

        if let Some(alloc) = self.favicon_allocation.take() {
            device.destroy_image_view(self.favicon_view, None);
            device.destroy_image(self.favicon_image, None);
            allocator.lock().unwrap().free(alloc).ok();
        }

        let (image, view, alloc) = util::create_gpu_image_with_format(
            device,
            allocator,
            atlas_w,
            atlas_h,
            vk::Format::R8G8B8A8Srgb,
            "favicon_atlas",
        );
        let (staging, staging_alloc) =
            util::create_staging_buffer(device, allocator, &pixels, "favicon_atlas_staging");
        util::upload_image(
            device,
            queue,
            command_pool,
            staging,
            image,
            atlas_w,
            atlas_h,
        );
        device.destroy_buffer(staging, None);
        allocator.lock().unwrap().free(staging_alloc).ok();

        self.favicon_image = image;
        self.favicon_view = view;
        self.favicon_allocation = Some(alloc);
        self.favicon_regions = regions;
        self.favicon_atlas_size = atlas_w;

        let info = vk::DescriptorImageInfo {
            sampler: self.favicon_sampler,
            image_view: view,
            image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
        };
        let write = vk::WriteDescriptorSet {
            dst_set: self.tex_set,
            dst_binding: 5,
            descriptor_count: 1,
            descriptor_type: vk::DescriptorType::CombinedImageSampler,
            image_info: &info,
            ..Default::default()
        };
        device.update_descriptor_sets(&[write], &[]);
    }

    pub fn text_width(&self, text: &str, scale: f32) -> f32 {
        self.mc_text_width(text, scale)
    }

    pub fn mc_text_width(&self, text: &str, scale: f32) -> f32 {
        let Some(ref gm) = self.mc_glyph_map else {
            return 0.0;
        };
        let px_scale = scale / gm.cell_h as f32;
        let raw: f32 = text
            .chars()
            .map(|ch| {
                let w = gm.glyphs.get(&ch).map(|g| g.width).unwrap_or(gm.cell_w / 2);
                (w as f32 + 1.0) * px_scale
            })
            .sum();
        raw.ceil()
    }

    pub fn recreate_pipeline(&mut self, device: &vk::Device, render_pass: vk::RenderPass) {
        device.destroy_pipeline(self.pipeline, None);
        self.pipeline = create_pipeline(device, render_pass, self.pipeline_layout);
    }

    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        let mut alloc = allocator.lock().unwrap();

        device.destroy_buffer(self.globals_buffer, None);
        if let Some(a) = self.globals_allocation.take() {
            alloc.free(a).ok();
        }

        device.destroy_buffer(self.vertex_buffer, None);
        if let Some(a) = self.vertex_allocation.take() {
            alloc.free(a).ok();
        }

        destroy_texture_resources(
            device,
            &mut alloc,
            &mut TextureResources {
                sampler: self.font_sampler,
                image: self.font_image,
                view: self.font_view,
                image_alloc: self.font_allocation.take(),
                staging_buffer: self.font_staging_buffer,
                staging_alloc: self.font_staging_allocation.take(),
            },
        );
        destroy_texture_resources(
            device,
            &mut alloc,
            &mut TextureResources {
                sampler: self.sprite_sampler,
                image: self.sprite_image,
                view: self.sprite_view,
                image_alloc: self.sprite_allocation.take(),
                staging_buffer: self.sprite_staging_buffer,
                staging_alloc: self.sprite_staging_allocation.take(),
            },
        );
        if let Some(mut res) = self.item_placeholder.take() {
            destroy_texture_resources(device, &mut alloc, &mut res);
        }
        destroy_texture_resources(
            device,
            &mut alloc,
            &mut TextureResources {
                sampler: self.mc_font_sampler,
                image: self.mc_font_image,
                view: self.mc_font_view,
                image_alloc: self.mc_font_allocation.take(),
                staging_buffer: self.mc_font_staging_buffer,
                staging_alloc: self.mc_font_staging_allocation.take(),
            },
        );

        device.destroy_sampler(self.favicon_sampler, None);
        device.destroy_image_view(self.favicon_view, None);
        device.destroy_image(self.favicon_image, None);

        if let Some(a) = self.favicon_allocation.take() {
            alloc.free(a).ok();
        }

        drop(alloc);

        device.destroy_pipeline(self.pipeline, None);
        device.destroy_pipeline_layout(self.pipeline_layout, None);
        device.destroy_descriptor_pool(self.descriptor_pool, None);
        device.destroy_descriptor_set_layout(self.globals_layout, None);
        device.destroy_descriptor_set_layout(self.tex_layout, None);
    }
}

pub struct TooltipLine {
    pub text: String,
    pub color: [f32; 4],
}

#[allow(dead_code)]
pub enum MenuElement {
    ScissorPush {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    },
    ScissorPop,
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        corner_radius: f32,
        color: [f32; 4],
    },
    Text {
        x: f32,
        y: f32,
        text: String,
        scale: f32,
        color: [f32; 4],
        centered: bool,
    },
    TextFlat {
        x: f32,
        y: f32,
        text: String,
        scale: f32,
        color: [f32; 4],
    },
    Icon {
        x: f32,
        y: f32,
        icon: char,
        scale: f32,
        color: [f32; 4],
    },
    Image {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        sprite: SpriteId,
        tint: [f32; 4],
    },
    NineSlice {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        sprite: SpriteId,
        border: f32,
        tint: [f32; 4],
    },
    ItemIcon {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        item_name: String,
        tint: [f32; 4],
    },
    McText {
        x: f32,
        y: f32,
        spans: Vec<TextSpan>,
        scale: f32,
        centered: bool,
    },
    TiledImage {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        sprite: SpriteId,
        tile_size: f32,
        tint: [f32; 4],
    },
    GradientRect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        corner_radius: f32,
        color_top: [f32; 4],
        color_bottom: [f32; 4],
    },
    Tooltip {
        x: f32,
        y: f32,
        text: String,
        scale: f32,
        screen_w: f32,
        screen_h: f32,
    },
    TooltipLines {
        x: f32,
        y: f32,
        lines: Vec<TooltipLine>,
        scale: f32,
        screen_w: f32,
        screen_h: f32,
    },
    FrostedRect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        corner_radius: f32,
        tint: [f32; 4],
    },
    Favicon {
        x: f32,
        y: f32,
        size: f32,
        address: String,
    },
    /// A player's 8x8 skin face, looked up in the shared face/favicon atlas by
    /// UUID; falls back to the default `SteveHead` sprite until it loads.
    SkinFace {
        x: f32,
        y: f32,
        size: f32,
        uuid: String,
    },
    /// Split marker for the menu draw: elements before it are rendered into the
    /// scene so the blur pass captures them; elements after are drawn sharp on
    /// top. Used to render the title screen blurred behind the Friends dialog.
    BlurBackdrop,
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
    AirFull,
    AirBursting,
    AirEmpty,
    ArmorEmpty,
    ArmorHalf,
    ArmorFull,
    ExperienceBarBackground,
    ExperienceBarProgress,
    InventoryBackground,
    CraftingTableBackground,
    CreativeItemsBackground,
    CreativeSearchBackground,
    CreativeInventoryBackground,
    CreativeTabTopUnselected1,
    CreativeTabTopUnselected2,
    CreativeTabTopUnselected3,
    CreativeTabTopUnselected4,
    CreativeTabTopUnselected5,
    CreativeTabTopUnselected6,
    CreativeTabTopUnselected7,
    CreativeTabTopSelected1,
    CreativeTabTopSelected2,
    CreativeTabTopSelected3,
    CreativeTabTopSelected4,
    CreativeTabTopSelected5,
    CreativeTabTopSelected6,
    CreativeTabTopSelected7,
    CreativeTabBottomUnselected1,
    CreativeTabBottomUnselected2,
    CreativeTabBottomUnselected3,
    CreativeTabBottomUnselected4,
    CreativeTabBottomUnselected5,
    CreativeTabBottomUnselected6,
    CreativeTabBottomUnselected7,
    CreativeTabBottomSelected1,
    CreativeTabBottomSelected2,
    CreativeTabBottomSelected3,
    CreativeTabBottomSelected4,
    CreativeTabBottomSelected5,
    CreativeTabBottomSelected6,
    CreativeTabBottomSelected7,
    CreativeScroller,
    CreativeScrollerDisabled,
    EmptyHelmet,
    EmptyChestplate,
    EmptyLeggings,
    EmptyBoots,
    EmptyShield,
    SlotHighlightBack,
    SlotHighlightFront,
    RecipeBookButton,
    RecipeBookButtonHighlighted,
    ButtonNormal,
    ButtonHover,
    ButtonDisabled,
    SliderTrack,
    SliderTrackHover,
    SliderHandle,
    SliderHandleHover,
    HeaderSeparator,
    FooterSeparator,
    MenuBackground,
    TooltipBackground,
    TooltipFrame,
    Scroller,
    ScrollerBackground,
    Ping1,
    Ping2,
    Ping3,
    Ping4,
    Ping5,
    PingUnknown,
    ServerJoin,
    ServerJoinHighlighted,
    ServerMoveUp,
    ServerMoveUpHighlighted,
    ServerMoveDown,
    ServerMoveDownHighlighted,
    UnknownServer,
    Pinging1,
    Pinging2,
    Pinging3,
    Pinging4,
    Pinging5,
    Incompatible,
    Unreachable,
    SteveHead,
    FriendsBackground,
    FriendsTab,
    FriendsTabDisabled,
    FriendsTabHighlighted,
    FriendsIllustration,
    FriendsSend,
    FriendsRemove,
    FriendsAccept,
    FriendsReject,
    FriendsCancel,
}

pub const CREATIVE_TAB_SPRITES: [[[SpriteId; 7]; 2]; 2] = [
    [
        [
            SpriteId::CreativeTabTopUnselected1,
            SpriteId::CreativeTabTopUnselected2,
            SpriteId::CreativeTabTopUnselected3,
            SpriteId::CreativeTabTopUnselected4,
            SpriteId::CreativeTabTopUnselected5,
            SpriteId::CreativeTabTopUnselected6,
            SpriteId::CreativeTabTopUnselected7,
        ],
        [
            SpriteId::CreativeTabTopSelected1,
            SpriteId::CreativeTabTopSelected2,
            SpriteId::CreativeTabTopSelected3,
            SpriteId::CreativeTabTopSelected4,
            SpriteId::CreativeTabTopSelected5,
            SpriteId::CreativeTabTopSelected6,
            SpriteId::CreativeTabTopSelected7,
        ],
    ],
    [
        [
            SpriteId::CreativeTabBottomUnselected1,
            SpriteId::CreativeTabBottomUnselected2,
            SpriteId::CreativeTabBottomUnselected3,
            SpriteId::CreativeTabBottomUnselected4,
            SpriteId::CreativeTabBottomUnselected5,
            SpriteId::CreativeTabBottomUnselected6,
            SpriteId::CreativeTabBottomUnselected7,
        ],
        [
            SpriteId::CreativeTabBottomSelected1,
            SpriteId::CreativeTabBottomSelected2,
            SpriteId::CreativeTabBottomSelected3,
            SpriteId::CreativeTabBottomSelected4,
            SpriteId::CreativeTabBottomSelected5,
            SpriteId::CreativeTabBottomSelected6,
            SpriteId::CreativeTabBottomSelected7,
        ],
    ],
];

struct SpriteRegion {
    u0: f32,
    v0: f32,
    u1: f32,
    v1: f32,
    src_w: f32,
    src_h: f32,
    nine_slice_border: f32,
}

struct SpriteAtlas {
    regions: HashMap<SpriteId, SpriteRegion>,
}

const INV_TEX_W: u32 = 176;
const INV_TEX_H: u32 = 166;

fn build_sprite_atlas(
    device: &vk::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    allocator: &Arc<Mutex<Allocator>>,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
) -> (
    SpriteAtlas,
    vk::Image,
    vk::ImageView,
    Allocation,
    vk::Buffer,
    Option<Allocation>,
) {
    let sprites: &[(SpriteId, &str, f32)] = &[
        (
            SpriteId::Hotbar,
            "minecraft/textures/gui/sprites/hud/hotbar.png",
            0.0,
        ),
        (
            SpriteId::HotbarSelection,
            "minecraft/textures/gui/sprites/hud/hotbar_selection.png",
            0.0,
        ),
        (
            SpriteId::HeartContainer,
            "minecraft/textures/gui/sprites/hud/heart/container.png",
            0.0,
        ),
        (
            SpriteId::HeartFull,
            "minecraft/textures/gui/sprites/hud/heart/full.png",
            0.0,
        ),
        (
            SpriteId::HeartHalf,
            "minecraft/textures/gui/sprites/hud/heart/half.png",
            0.0,
        ),
        (
            SpriteId::FoodEmpty,
            "minecraft/textures/gui/sprites/hud/food_empty.png",
            0.0,
        ),
        (
            SpriteId::FoodFull,
            "minecraft/textures/gui/sprites/hud/food_full.png",
            0.0,
        ),
        (
            SpriteId::FoodHalf,
            "minecraft/textures/gui/sprites/hud/food_half.png",
            0.0,
        ),
        (
            SpriteId::AirFull,
            "minecraft/textures/gui/sprites/hud/air.png",
            0.0,
        ),
        (
            SpriteId::AirBursting,
            "minecraft/textures/gui/sprites/hud/air_bursting.png",
            0.0,
        ),
        (
            SpriteId::AirEmpty,
            "minecraft/textures/gui/sprites/hud/air_empty.png",
            0.0,
        ),
        (
            SpriteId::ArmorEmpty,
            "minecraft/textures/gui/sprites/hud/armor_empty.png",
            0.0,
        ),
        (
            SpriteId::ArmorHalf,
            "minecraft/textures/gui/sprites/hud/armor_half.png",
            0.0,
        ),
        (
            SpriteId::ArmorFull,
            "minecraft/textures/gui/sprites/hud/armor_full.png",
            0.0,
        ),
        (
            SpriteId::ExperienceBarBackground,
            "minecraft/textures/gui/sprites/hud/experience_bar_background.png",
            0.0,
        ),
        (
            SpriteId::ExperienceBarProgress,
            "minecraft/textures/gui/sprites/hud/experience_bar_progress.png",
            0.0,
        ),
        (
            SpriteId::EmptyHelmet,
            "minecraft/textures/gui/sprites/container/slot/helmet.png",
            0.0,
        ),
        (
            SpriteId::EmptyChestplate,
            "minecraft/textures/gui/sprites/container/slot/chestplate.png",
            0.0,
        ),
        (
            SpriteId::EmptyLeggings,
            "minecraft/textures/gui/sprites/container/slot/leggings.png",
            0.0,
        ),
        (
            SpriteId::EmptyBoots,
            "minecraft/textures/gui/sprites/container/slot/boots.png",
            0.0,
        ),
        (
            SpriteId::EmptyShield,
            "minecraft/textures/gui/sprites/container/slot/shield.png",
            0.0,
        ),
        (
            SpriteId::SlotHighlightBack,
            "minecraft/textures/gui/sprites/container/slot_highlight_back.png",
            0.0,
        ),
        (
            SpriteId::SlotHighlightFront,
            "minecraft/textures/gui/sprites/container/slot_highlight_front.png",
            0.0,
        ),
        (
            SpriteId::RecipeBookButton,
            "minecraft/textures/gui/sprites/recipe_book/button.png",
            0.0,
        ),
        (
            SpriteId::RecipeBookButtonHighlighted,
            "minecraft/textures/gui/sprites/recipe_book/button_highlighted.png",
            0.0,
        ),
        (
            SpriteId::ButtonNormal,
            "minecraft/textures/gui/sprites/widget/button.png",
            3.0,
        ),
        (
            SpriteId::ButtonHover,
            "minecraft/textures/gui/sprites/widget/button_highlighted.png",
            3.0,
        ),
        (
            SpriteId::ButtonDisabled,
            "minecraft/textures/gui/sprites/widget/button_disabled.png",
            1.0,
        ),
        (
            SpriteId::SliderTrack,
            "minecraft/textures/gui/sprites/widget/slider.png",
            3.0,
        ),
        (
            SpriteId::SliderTrackHover,
            "minecraft/textures/gui/sprites/widget/slider_highlighted.png",
            3.0,
        ),
        (
            SpriteId::SliderHandle,
            "minecraft/textures/gui/sprites/widget/slider_handle.png",
            0.0,
        ),
        (
            SpriteId::SliderHandleHover,
            "minecraft/textures/gui/sprites/widget/slider_handle_highlighted.png",
            0.0,
        ),
        (
            SpriteId::HeaderSeparator,
            "minecraft/textures/gui/inworld_header_separator.png",
            0.0,
        ),
        (
            SpriteId::FooterSeparator,
            "minecraft/textures/gui/inworld_footer_separator.png",
            0.0,
        ),
        (
            SpriteId::MenuBackground,
            "minecraft/textures/gui/inworld_menu_background.png",
            0.0,
        ),
        (
            SpriteId::TooltipBackground,
            "minecraft/textures/gui/sprites/tooltip/background.png",
            9.0,
        ),
        (
            SpriteId::TooltipFrame,
            "minecraft/textures/gui/sprites/tooltip/frame.png",
            10.0,
        ),
        (
            SpriteId::Scroller,
            "minecraft/textures/gui/sprites/widget/scroller.png",
            1.0,
        ),
        (
            SpriteId::ScrollerBackground,
            "minecraft/textures/gui/sprites/widget/scroller_background.png",
            1.0,
        ),
        (
            SpriteId::FriendsBackground,
            "minecraft/textures/gui/sprites/friends/background.png",
            8.0,
        ),
        (
            SpriteId::FriendsTab,
            "minecraft/textures/gui/sprites/friends/button.png",
            3.0,
        ),
        (
            SpriteId::FriendsTabDisabled,
            "minecraft/textures/gui/sprites/friends/button_disabled.png",
            1.0,
        ),
        (
            SpriteId::FriendsTabHighlighted,
            "minecraft/textures/gui/sprites/friends/button_highlighted.png",
            3.0,
        ),
        (
            SpriteId::FriendsIllustration,
            "minecraft/textures/gui/sprites/friends/illustrations_00.png",
            0.0,
        ),
        (
            SpriteId::FriendsSend,
            "minecraft/textures/gui/sprites/friends/send_request.png",
            0.0,
        ),
        (
            SpriteId::FriendsRemove,
            "minecraft/textures/gui/sprites/friends/remove.png",
            0.0,
        ),
        (
            SpriteId::FriendsAccept,
            "minecraft/textures/gui/sprites/friends/accept.png",
            0.0,
        ),
        (
            SpriteId::FriendsReject,
            "minecraft/textures/gui/sprites/friends/reject.png",
            0.0,
        ),
        (
            SpriteId::FriendsCancel,
            "minecraft/textures/gui/sprites/friends/cancel.png",
            0.0,
        ),
        (
            SpriteId::Ping1,
            "minecraft/textures/gui/sprites/icon/ping_1.png",
            0.0,
        ),
        (
            SpriteId::Ping2,
            "minecraft/textures/gui/sprites/icon/ping_2.png",
            0.0,
        ),
        (
            SpriteId::Ping3,
            "minecraft/textures/gui/sprites/icon/ping_3.png",
            0.0,
        ),
        (
            SpriteId::Ping4,
            "minecraft/textures/gui/sprites/icon/ping_4.png",
            0.0,
        ),
        (
            SpriteId::Ping5,
            "minecraft/textures/gui/sprites/icon/ping_5.png",
            0.0,
        ),
        (
            SpriteId::PingUnknown,
            "minecraft/textures/gui/sprites/icon/ping_unknown.png",
            0.0,
        ),
        (
            SpriteId::ServerJoin,
            "minecraft/textures/gui/sprites/server_list/join.png",
            0.0,
        ),
        (
            SpriteId::ServerJoinHighlighted,
            "minecraft/textures/gui/sprites/server_list/join_highlighted.png",
            0.0,
        ),
        (
            SpriteId::ServerMoveUp,
            "minecraft/textures/gui/sprites/server_list/move_up.png",
            0.0,
        ),
        (
            SpriteId::ServerMoveUpHighlighted,
            "minecraft/textures/gui/sprites/server_list/move_up_highlighted.png",
            0.0,
        ),
        (
            SpriteId::ServerMoveDown,
            "minecraft/textures/gui/sprites/server_list/move_down.png",
            0.0,
        ),
        (
            SpriteId::ServerMoveDownHighlighted,
            "minecraft/textures/gui/sprites/server_list/move_down_highlighted.png",
            0.0,
        ),
        (
            SpriteId::UnknownServer,
            "minecraft/textures/misc/unknown_server.png",
            0.0,
        ),
        (
            SpriteId::Pinging1,
            "minecraft/textures/gui/sprites/server_list/pinging_1.png",
            0.0,
        ),
        (
            SpriteId::Pinging2,
            "minecraft/textures/gui/sprites/server_list/pinging_2.png",
            0.0,
        ),
        (
            SpriteId::Pinging3,
            "minecraft/textures/gui/sprites/server_list/pinging_3.png",
            0.0,
        ),
        (
            SpriteId::Pinging4,
            "minecraft/textures/gui/sprites/server_list/pinging_4.png",
            0.0,
        ),
        (
            SpriteId::Pinging5,
            "minecraft/textures/gui/sprites/server_list/pinging_5.png",
            0.0,
        ),
        (
            SpriteId::Incompatible,
            "minecraft/textures/gui/sprites/server_list/incompatible.png",
            0.0,
        ),
        (
            SpriteId::Unreachable,
            "minecraft/textures/gui/sprites/server_list/unreachable.png",
            0.0,
        ),
        (
            SpriteId::CreativeScroller,
            "minecraft/textures/gui/sprites/container/creative_inventory/scroller.png",
            0.0,
        ),
        (
            SpriteId::CreativeScrollerDisabled,
            "minecraft/textures/gui/sprites/container/creative_inventory/scroller_disabled.png",
            0.0,
        ),
    ];

    let mut images: Vec<(SpriteId, Vec<u8>, u32, u32, f32)> = Vec::new();
    for &(id, asset_key, border) in sprites {
        let path = resolve_asset_path(jar_assets_dir, asset_index, asset_key);
        match crate::assets::load_image(&path) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                let w = rgba.width();
                let h = rgba.height();
                images.push((id, rgba.into_raw(), w, h, border));
            }
            Err(e) => {
                tracing::warn!("Failed to load sprite {asset_key}: {e}");
                images.push((id, vec![255, 0, 255, 255], 1, 1, 0.0));
            }
        }
    }

    // Steve head: 8x8 face composited with the 8x8 hat overlay from the wide skin.
    let steve_path = resolve_asset_path(
        jar_assets_dir,
        asset_index,
        "minecraft/textures/entity/player/wide/steve.png",
    );
    match crate::assets::load_image(&steve_path) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            match extract_face_8x8(rgba.as_raw(), rgba.width(), rgba.height()) {
                Some(out) => images.push((SpriteId::SteveHead, out, 8, 8, 0.0)),
                None => {
                    tracing::warn!("Steve skin too small: {}x{}", rgba.width(), rgba.height());
                    images.push((SpriteId::SteveHead, vec![255, 0, 255, 255], 1, 1, 0.0));
                }
            }
        }
        Err(e) => {
            tracing::warn!("Failed to load Steve skin: {e}");
            images.push((SpriteId::SteveHead, vec![255, 0, 255, 255], 1, 1, 0.0));
        }
    }

    // Container backgrounds live in a 256x256 atlas; crop out the used region.
    for (id, path, max_w, max_h) in [
        (
            SpriteId::InventoryBackground,
            "minecraft/textures/gui/container/inventory.png",
            INV_TEX_W,
            INV_TEX_H,
        ),
        (
            SpriteId::CraftingTableBackground,
            "minecraft/textures/gui/container/crafting_table.png",
            INV_TEX_W,
            INV_TEX_H,
        ),
        (
            SpriteId::CreativeItemsBackground,
            "minecraft/textures/gui/container/creative_inventory/tab_items.png",
            195,
            136,
        ),
        (
            SpriteId::CreativeSearchBackground,
            "minecraft/textures/gui/container/creative_inventory/tab_item_search.png",
            195,
            136,
        ),
        (
            SpriteId::CreativeInventoryBackground,
            "minecraft/textures/gui/container/creative_inventory/tab_inventory.png",
            195,
            136,
        ),
    ] {
        let path = resolve_asset_path(jar_assets_dir, asset_index, path);
        match crate::assets::load_image(&path) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                let full_w = rgba.width();
                let crop_w = max_w.min(full_w);
                let crop_h = max_h.min(rgba.height());
                let mut cropped = vec![0u8; (crop_w * crop_h * 4) as usize];
                for y in 0..crop_h {
                    let src_off = (y * full_w * 4) as usize;
                    let dst_off = (y * crop_w * 4) as usize;
                    let row_bytes = (crop_w * 4) as usize;
                    cropped[dst_off..dst_off + row_bytes]
                        .copy_from_slice(&rgba.as_raw()[src_off..src_off + row_bytes]);
                }
                images.push((id, cropped, crop_w, crop_h, 0.0));
            }
            Err(e) => {
                tracing::warn!("Failed to load container background {id:?}: {e}");
                images.push((id, vec![255, 0, 255, 255], 1, 1, 0.0));
            }
        }
    }

    for (row_idx, row_name) in ["top", "bottom"].iter().enumerate() {
        for (state_idx, state_name) in ["unselected", "selected"].iter().enumerate() {
            for col in 1..=7u32 {
                let id = CREATIVE_TAB_SPRITES[row_idx][state_idx][(col - 1) as usize];
                let asset_key = format!(
                    "minecraft/textures/gui/sprites/container/creative_inventory/tab_{row_name}_{state_name}_{col}.png"
                );
                let path = resolve_asset_path(jar_assets_dir, asset_index, &asset_key);
                match crate::assets::load_image(&path) {
                    Ok(img) => {
                        let rgba = img.to_rgba8();
                        let w = rgba.width();
                        let h = rgba.height();
                        images.push((id, rgba.into_raw(), w, h, 0.0));
                    }
                    Err(e) => {
                        tracing::warn!("Failed to load creative tab sprite {asset_key}: {e}");
                        images.push((id, vec![255, 0, 255, 255], 1, 1, 0.0));
                    }
                }
            }
        }
    }

    let atlas_size = 1024u32;
    let mut pixels = vec![0u8; (atlas_size * atlas_size * 4) as usize];
    let mut regions = HashMap::new();
    let mut cursor_x = 0u32;
    let mut cursor_y = 0u32;
    let mut row_height = 0u32;

    for (id, data, w, h, border) in &images {
        if cursor_x + w > atlas_size {
            cursor_x = 0;
            cursor_y += row_height;
            row_height = 0;
        }
        if cursor_y + h > atlas_size {
            tracing::warn!("Sprite atlas overflow, skipping {:?}", id);
            continue;
        }

        blit_image(
            &mut pixels,
            atlas_size,
            data,
            *w,
            cursor_x,
            cursor_y,
            *w,
            *h,
        );

        let inv = 1.0 / atlas_size as f32;
        regions.insert(
            *id,
            SpriteRegion {
                u0: cursor_x as f32 * inv,
                v0: cursor_y as f32 * inv,
                u1: (cursor_x + w) as f32 * inv,
                v1: (cursor_y + h) as f32 * inv,
                src_w: *w as f32,
                src_h: *h as f32,
                nine_slice_border: *border,
            },
        );

        cursor_x += w;
        row_height = row_height.max(*h);
    }

    let (image, view, allocation) =
        util::create_gpu_image(device, allocator, atlas_size, atlas_size, "sprite_atlas");
    let (staging_buffer, staging_allocation) =
        util::create_staging_buffer(device, allocator, &pixels, "sprite_staging");
    util::upload_image(
        device,
        queue,
        command_pool,
        staging_buffer,
        image,
        atlas_size,
        atlas_size,
    );

    (
        SpriteAtlas { regions },
        image,
        view,
        allocation,
        staging_buffer,
        Some(staging_allocation),
    )
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
    device: &vk::Device,
    alloc: &mut Allocator,
    res: &mut TextureResources,
) {
    device.destroy_sampler(res.sampler, None);
    device.destroy_image_view(res.view, None);

    if let Some(a) = res.image_alloc.take() {
        alloc.free(a).ok();
    }
    device.destroy_image(res.image, None);
    if let Some(a) = res.staging_alloc.take() {
        alloc.free(a).ok();
    }
    device.destroy_buffer(res.staging_buffer, None);
}

#[allow(clippy::too_many_arguments)]
fn blit_image(
    dst: &mut [u8],
    dst_stride: u32,
    src: &[u8],
    src_stride: u32,
    dx: u32,
    dy: u32,
    w: u32,
    h: u32,
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

/// Draw a quad from the string-keyed image atlas (favicons / friend faces),
/// falling back to a sprite-atlas sprite when the key hasn't loaded yet.
#[allow(clippy::too_many_arguments)]
fn push_atlas_image(
    verts: &mut Vec<Vertex>,
    atlas_regions: &std::collections::HashMap<String, [f32; 4]>,
    sprite_atlas: &SpriteAtlas,
    key: &str,
    fallback: SpriteId,
    x: f32,
    y: f32,
    size: f32,
) {
    let white = [1.0, 1.0, 1.0, 1.0];
    if let Some([u0, v0, u1, v1]) = atlas_regions.get(key) {
        push_quad(
            verts,
            x,
            y,
            size,
            size,
            *u0,
            *v0,
            *u1,
            *v1,
            white,
            6.0,
            [size, size],
            0.0,
        );
    } else if let Some(r) = sprite_atlas.regions.get(&fallback) {
        push_quad(
            verts,
            x,
            y,
            size,
            size,
            r.u0,
            r.v0,
            r.u1,
            r.v1,
            white,
            2.0,
            [size, size],
            0.0,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn push_quad(
    verts: &mut Vec<Vertex>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    u0: f32,
    v0: f32,
    u1: f32,
    v1: f32,
    color: [f32; 4],
    mode: f32,
    rect_size: [f32; 2],
    corner_radius: f32,
) {
    let positions = [
        [x, y],
        [x + w, y],
        [x, y + h],
        [x + w, y],
        [x + w, y + h],
        [x, y + h],
    ];
    let uvs = [[u0, v0], [u1, v0], [u0, v1], [u1, v0], [u1, v1], [u0, v1]];
    for i in 0..6 {
        verts.push(Vertex {
            pos: positions[i],
            uv: uvs[i],
            color,
            mode,
            rect_size,
            corner_radius,
        });
    }
}

fn push_rect(
    verts: &mut Vec<Vertex>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    color: [f32; 4],
) {
    push_quad(
        verts,
        x,
        y,
        w,
        h,
        0.0,
        0.0,
        1.0,
        1.0,
        color,
        0.0,
        [w, h],
        radius,
    );
}

#[allow(clippy::too_many_arguments)]
fn push_gradient_rect(
    verts: &mut Vec<Vertex>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    color_top: [f32; 4],
    color_bottom: [f32; 4],
) {
    let positions = [
        [x, y],
        [x + w, y],
        [x, y + h],
        [x + w, y],
        [x + w, y + h],
        [x, y + h],
    ];
    let uvs = [
        [0.0, 0.0],
        [1.0, 0.0],
        [0.0, 1.0],
        [1.0, 0.0],
        [1.0, 1.0],
        [0.0, 1.0],
    ];
    let colors = [
        color_top,
        color_top,
        color_bottom,
        color_top,
        color_bottom,
        color_bottom,
    ];
    for i in 0..6 {
        verts.push(Vertex {
            pos: positions[i],
            uv: uvs[i],
            color: colors[i],
            mode: 0.0,
            rect_size: [w, h],
            corner_radius: radius,
        });
    }
}

fn push_icon_glyph(
    verts: &mut Vec<Vertex>,
    atlas: &FontAtlas,
    cx: f32,
    cy: f32,
    icon: char,
    scale: f32,
    color: [f32; 4],
) {
    let Some(g) = atlas.glyphs.get(&icon) else {
        return;
    };
    let s = scale / RASTER_PX;
    let gw = g.width_px * s;
    let gh = g.height_px * s;
    push_quad(
        verts,
        cx - gw / 2.0,
        cy - gh / 2.0,
        gw,
        gh,
        g.u0,
        g.v0,
        g.u1,
        g.v1,
        color,
        1.0,
        [0.0, 0.0],
        0.0,
    );
}

#[allow(clippy::too_many_arguments)]
fn push_textured_quad(
    verts: &mut Vec<Vertex>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    region: &SpriteRegion,
    tint: [f32; 4],
    mode: f32,
) {
    push_quad(
        verts,
        x,
        y,
        w,
        h,
        region.u0,
        region.v0,
        region.u1,
        region.v1,
        tint,
        mode,
        [0.0, 0.0],
        0.0,
    );
}

#[allow(clippy::too_many_arguments)]
fn push_nine_slice(
    verts: &mut Vec<Vertex>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    region: &SpriteRegion,
    border: f32,
    tint: [f32; 4],
) {
    let uv_w = region.u1 - region.u0;
    let uv_h = region.v1 - region.v0;
    let tex_border = if region.nine_slice_border > 0.0 {
        region.nine_slice_border
    } else {
        3.0
    };
    let bu = (tex_border / region.src_w) * uv_w;
    let bv = (tex_border / region.src_h) * uv_h;

    // Snap to integer pixels; fractional edges let NEAREST sampling bleed into
    // the neighbouring atlas sprite (no gutter between sprites).
    let x0 = x.round();
    let y0 = y.round();
    let x1 = (x + w).round();
    let y1 = (y + h).round();
    let border = border.round().max(0.0);
    let bx = border.min(((x1 - x0) / 2.0).floor());
    let by = border.min(((y1 - y0) / 2.0).floor());
    let xs = [x0, x0 + bx, x1 - bx, x1];
    let ys = [y0, y0 + by, y1 - by, y1];
    let us = [region.u0, region.u0 + bu, region.u1 - bu, region.u1];
    let vs = [region.v0, region.v0 + bv, region.v1 - bv, region.v1];

    for row in 0..3 {
        for col in 0..3 {
            let qx = xs[col];
            let qy = ys[row];
            let qw = xs[col + 1] - xs[col];
            let qh = ys[row + 1] - ys[row];
            if qw <= 0.0 || qh <= 0.0 {
                continue;
            }
            push_quad(
                verts,
                qx,
                qy,
                qw,
                qh,
                us[col],
                vs[row],
                us[col + 1],
                vs[row + 1],
                tint,
                2.0,
                [0.0, 0.0],
                0.0,
            );
        }
    }
}

fn push_mc_text(
    verts: &mut Vec<Vertex>,
    gm: &GlyphMap,
    x: f32,
    y: f32,
    spans: &[TextSpan],
    scale: f32,
    drop_shadow: bool,
) {
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
                if line >= 2 {
                    return;
                }
                continue;
            }

            let Some(gi) = gm.glyphs.get(&ch) else {
                continue;
            };
            let glyph_w = gi.width as f32 * px_scale;
            let glyph_draw_h = gi.height as f32 * px_scale;
            let glyph_y_off = gi.y_offset as f32 * px_scale;

            let u0 = gi.col as f32 * gm.cell_w as f32 * inv_w;
            let v0 = (gi.row as f32 * gm.cell_h as f32 + gi.y_offset as f32) * inv_h;
            let u1 = (gi.col as f32 * gm.cell_w as f32 + gi.width as f32) * inv_w;
            let v1 =
                (gi.row as f32 * gm.cell_h as f32 + gi.y_offset as f32 + gi.height as f32) * inv_h;

            let italic_offset = if span.italic { px_scale } else { 0.0 };

            let sx = cx.round();
            let sy = (cy + glyph_y_off).round();

            if drop_shadow {
                push_mc_glyph(
                    verts,
                    sx + px_scale,
                    sy + px_scale,
                    glyph_w.round(),
                    glyph_draw_h.round(),
                    u0,
                    v0,
                    u1,
                    v1,
                    shadow_color,
                    italic_offset,
                );
                if span.bold {
                    push_mc_glyph(
                        verts,
                        sx + 2.0 * px_scale,
                        sy + px_scale,
                        glyph_w.round(),
                        glyph_draw_h.round(),
                        u0,
                        v0,
                        u1,
                        v1,
                        shadow_color,
                        italic_offset,
                    );
                }
            }

            push_mc_glyph(
                verts,
                sx,
                sy,
                glyph_w.round(),
                glyph_draw_h.round(),
                u0,
                v0,
                u1,
                v1,
                span.color,
                italic_offset,
            );
            if span.bold {
                push_mc_glyph(
                    verts,
                    sx + px_scale,
                    sy,
                    glyph_w.round(),
                    glyph_draw_h.round(),
                    u0,
                    v0,
                    u1,
                    v1,
                    span.color,
                    italic_offset,
                );
            }

            if span.strikethrough || span.underline {
                let lw = glyph_w + if span.bold { px_scale } else { 0.0 };
                if span.strikethrough {
                    let sy = cy + glyph_h * 0.45;
                    push_quad(
                        verts,
                        cx,
                        sy,
                        lw,
                        px_scale,
                        0.0,
                        0.0,
                        0.0,
                        0.0,
                        span.color,
                        0.0,
                        [lw, px_scale],
                        0.0,
                    );
                }
                if span.underline {
                    let uy = cy + glyph_h - px_scale;
                    push_quad(
                        verts,
                        cx,
                        uy,
                        lw,
                        px_scale,
                        0.0,
                        0.0,
                        0.0,
                        0.0,
                        span.color,
                        0.0,
                        [lw, px_scale],
                        0.0,
                    );
                }
            }

            let advance = (gi.width as f32 + 1.0) * px_scale;
            cx += if span.bold {
                advance + px_scale
            } else {
                advance
            };
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn push_mc_glyph(
    verts: &mut Vec<Vertex>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    u0: f32,
    v0: f32,
    u1: f32,
    v1: f32,
    color: [f32; 4],
    italic_offset: f32,
) {
    let positions = [
        [x + italic_offset, y],
        [x + w + italic_offset, y],
        [x, y + h],
        [x + w + italic_offset, y],
        [x + w, y + h],
        [x, y + h],
    ];
    let uvs = [[u0, v0], [u1, v0], [u0, v1], [u1, v0], [u1, v1], [u0, v1]];
    for i in 0..6 {
        verts.push(Vertex {
            pos: positions[i],
            uv: uvs[i],
            color,
            mode: 4.0,
            rect_size: [0.0, 0.0],
            corner_radius: 0.0,
        });
    }
}

fn create_pipeline(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> vk::Pipeline {
    let vert_spv = shader::include_spirv!("menu_overlay.vert.spv");
    let frag_spv = shader::include_spirv!("menu_overlay.frag.spv");

    let vert_module = shader::create_shader_module(device, vert_spv);
    let frag_module = shader::create_shader_module(device, frag_spv);

    let stages = [
        vk::PipelineShaderStageCreateInfo {
            stage: vk::ShaderStageFlags::Vertex,
            module: vert_module,
            name: c"main".as_ptr(),
            ..Default::default()
        },
        vk::PipelineShaderStageCreateInfo {
            stage: vk::ShaderStageFlags::Fragment,
            module: frag_module,
            name: c"main".as_ptr(),
            ..Default::default()
        },
    ];

    let binding_descs = [vk::VertexInputBindingDescription {
        binding: 0,
        stride: VERTEX_SIZE as u32,
        input_rate: vk::VertexInputRate::Vertex,
    }];

    let attr_descs = [
        vk::VertexInputAttributeDescription {
            location: 0,
            binding: 0,
            format: vk::Format::R32G32Sfloat,
            offset: 0,
        },
        vk::VertexInputAttributeDescription {
            location: 1,
            binding: 0,
            format: vk::Format::R32G32Sfloat,
            offset: 8,
        },
        vk::VertexInputAttributeDescription {
            location: 2,
            binding: 0,
            format: vk::Format::R32G32B32A32Sfloat,
            offset: 16,
        },
        vk::VertexInputAttributeDescription {
            location: 3,
            binding: 0,
            format: vk::Format::R32Sfloat,
            offset: 32,
        },
        vk::VertexInputAttributeDescription {
            location: 4,
            binding: 0,
            format: vk::Format::R32G32Sfloat,
            offset: 36,
        },
        vk::VertexInputAttributeDescription {
            location: 5,
            binding: 0,
            format: vk::Format::R32Sfloat,
            offset: 44,
        },
    ];

    let vertex_input = vk::PipelineVertexInputStateCreateInfo {
        vertex_binding_description_count: binding_descs.len() as u32,
        vertex_binding_descriptions: binding_descs.as_ptr(),
        vertex_attribute_description_count: attr_descs.len() as u32,
        vertex_attribute_descriptions: attr_descs.as_ptr(),
        ..Default::default()
    };

    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo {
        topology: vk::PrimitiveTopology::TriangleList,
        ..Default::default()
    };

    let viewport_state = vk::PipelineViewportStateCreateInfo {
        viewport_count: 1,
        scissor_count: 1,
        ..Default::default()
    };

    let rasterizer = vk::PipelineRasterizationStateCreateInfo {
        polygon_mode: vk::PolygonMode::Fill,
        cull_mode: vk::CullModeFlags::None,
        line_width: 1.0,
        ..Default::default()
    };

    let multisampling = vk::PipelineMultisampleStateCreateInfo {
        rasterization_samples: vk::SampleCountFlags::Type1,
        ..Default::default()
    };

    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo {
        depth_test_enable: vk::FALSE,
        depth_write_enable: vk::FALSE,
        ..Default::default()
    };

    let blend_attachment = [vk::PipelineColorBlendAttachmentState {
        blend_enable: vk::TRUE,
        src_color_blend_factor: vk::BlendFactor::One,
        dst_color_blend_factor: vk::BlendFactor::OneMinusSrcAlpha,
        color_blend_op: vk::BlendOp::Add,
        src_alpha_blend_factor: vk::BlendFactor::One,
        dst_alpha_blend_factor: vk::BlendFactor::OneMinusSrcAlpha,
        alpha_blend_op: vk::BlendOp::Add,
        color_write_mask: vk::ColorComponentFlags::RGBA,
    }];

    let color_blending = vk::PipelineColorBlendStateCreateInfo {
        attachment_count: blend_attachment.len() as u32,
        attachments: blend_attachment.as_ptr(),
        ..Default::default()
    };

    let dynamic_states = [vk::DynamicState::Viewport, vk::DynamicState::Scissor];
    let dynamic_state = vk::PipelineDynamicStateCreateInfo {
        dynamic_state_count: dynamic_states.len() as u32,
        dynamic_states: dynamic_states.as_ptr(),
        ..Default::default()
    };

    let pipeline_info = [vk::GraphicsPipelineCreateInfo {
        stage_count: stages.len() as u32,
        stages: stages.as_ptr(),
        vertex_input_state: &vertex_input,
        input_assembly_state: &input_assembly,
        viewport_state: &viewport_state,
        rasterization_state: &rasterizer,
        multisample_state: &multisampling,
        depth_stencil_state: &depth_stencil,
        color_blend_state: &color_blending,
        dynamic_state: &dynamic_state,
        layout,
        render_pass,
        subpass: 0,
        ..Default::default()
    }];

    let mut pipeline = vk::Pipeline::null();
    device
        .create_graphics_pipelines(
            vk::PipelineCache::null(),
            &pipeline_info,
            None,
            slice::from_mut(&mut pipeline),
        )
        .expect("failed to create menu overlay pipeline");

    device.destroy_shader_module(vert_module, None);
    device.destroy_shader_module(frag_module, None);

    pipeline
}
