use std::collections::HashMap;
use std::path::Path;
use std::slice;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use pomme_gpu_allocator::vulkan::{Allocation, Allocator};
use pyronyx::vk;

use crate::assets::{AssetIndex, resolve_asset_path};
use crate::renderer::{packing, shader, util};
use crate::ui::font::{FontSources, GLYPH_ATLAS_SIZE, GlyphAtlasPixels, GlyphInfo, GlyphMap};
use crate::ui::text::{InlineObject, TextSpan};

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
    invert: bool,
}

/// Ends the current batch (if non-empty) as a `DrawOp` and starts the next
/// one at `end`.
fn flush_draw_op(
    draw_ops: &mut Vec<DrawOp>,
    cmd_start: &mut u32,
    end: u32,
    scissor: Option<[f32; 4]>,
    invert: bool,
) {
    if end > *cmd_start {
        draw_ops.push(DrawOp {
            start: *cmd_start,
            count: end - *cmd_start,
            scissor,
            invert,
        });
    }
    *cmd_start = end;
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
    extract_face_8x8_with_hat(rgba, sw, sh, true)
}

pub(crate) fn extract_face_8x8_with_hat(
    rgba: &[u8],
    sw: u32,
    sh: u32,
    hat: bool,
) -> Option<Vec<u8>> {
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
            if hat {
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
    }
    Some(out)
}

pub struct MenuOverlayPipeline {
    pipeline: vk::Pipeline,
    /// Vanilla `RenderPipelines.CROSSHAIR`: same shaders, INVERT blend.
    invert_pipeline: vk::Pipeline,
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
    mc_font: TextureResources,
    mc_font_color: TextureResources,
    /// Glyph atlas layer cap from the device, reused on reload.
    font_layer_limit: u32,
    mc_glyph_map: Option<GlyphMap>,
    obfuscation_rng: ObfuscationRng,
    vertex_buffer: vk::Buffer,
    vertex_allocation: Option<Allocation>,
    atlas: FontAtlas,
    favicon_image: vk::Image,
    favicon_view: vk::ImageView,
    favicon_sampler: vk::Sampler,
    favicon_allocation: Option<Allocation>,
    favicon_regions: std::collections::HashMap<String, [f32; 4]>,
    /// Inline objects drawn since the app last drained them, so the next
    /// frame's atlas holds what the text actually asked for.
    drawn_inline_objects: std::collections::HashMap<String, InlineObject>,
    favicon_atlas_size: u32,
    overlay_image: vk::Image,
    overlay_view: vk::ImageView,
    overlay_sampler: vk::Sampler,
    overlay_allocation: Option<Allocation>,
    overlay_vignette_uv: [f32; 4],
    overlay_pumpkin_uv: [f32; 4],
    underwater_image: vk::Image,
    underwater_view: vk::ImageView,
    underwater_sampler: vk::Sampler,
    underwater_allocation: Option<Allocation>,
}

impl MenuOverlayPipeline {
    pub fn new(
        device: &vk::Device,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        render_pass: vk::RenderPass,
        allocator: &Arc<Mutex<Allocator>>,
        font_sources: FontSources<'_>,
        font_layer_limit: u32,
    ) -> Result<Self, String> {
        let atlas = build_font_atlas();

        let globals_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::UniformBuffer,
            vk::ShaderStageFlags::Vertex | vk::ShaderStageFlags::Fragment,
        );

        // One combined image sampler per menu_overlay.frag binding.
        let tex_bindings: [vk::DescriptorSetLayoutBinding; 9] =
            std::array::from_fn(|binding| vk::DescriptorSetLayoutBinding {
                binding: binding as u32,
                descriptor_type: vk::DescriptorType::CombinedImageSampler,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::Fragment,
                ..Default::default()
            });
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

        let pipeline = create_pipeline(device, render_pass, pipeline_layout, false);
        let invert_pipeline = create_pipeline(device, render_pass, pipeline_layout, true);

        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UniformBuffer,
                descriptor_count: 1,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::CombinedImageSampler,
                descriptor_count: tex_bindings.len() as u32,
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
            font_sources.jar_assets_dir,
            font_sources.asset_index,
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

        let (mc_glyph_map, glyph_pixels) = match GlyphMap::load(font_sources, font_layer_limit) {
            Ok((map, pixels)) => (Some(map), Some(pixels)),
            Err(error) => {
                tracing::warn!("Minecraft fonts unavailable: {error}");
                (None, None)
            }
        };
        crate::lang::load(font_sources.jar_assets_dir);
        let (mc_font, mc_font_color) = create_font_textures(
            device,
            queue,
            command_pool,
            allocator,
            glyph_pixels.as_ref(),
        )?;

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
        let mc_font_img_info = mc_font.image_info();
        let mc_font_color_img_info = mc_font_color.image_info();

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

        let (overlay_image, overlay_view, overlay_alloc, overlay_vignette_uv, overlay_pumpkin_uv) =
            build_camera_overlay_texture(
                device,
                queue,
                command_pool,
                allocator,
                font_sources.jar_assets_dir,
                font_sources.asset_index,
            );
        // Both source textures ship mcmeta `blur: true`.
        let overlay_sampler = unsafe { util::create_linear_sampler(device) };

        let (underwater_image, underwater_view, underwater_alloc) = load_single_texture(
            device,
            queue,
            command_pool,
            allocator,
            font_sources.jar_assets_dir,
            font_sources.asset_index,
            "minecraft/textures/misc/underwater.png",
            "underwater_overlay",
        );
        // Repeat wrapping: the overlay tiles 4x and scrolls with the look direction.
        let underwater_sampler = unsafe { util::create_nearest_repeat_sampler(device) };

        let overlay_img_info = vk::DescriptorImageInfo {
            sampler: overlay_sampler,
            image_view: overlay_view,
            image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
        };
        let underwater_img_info = vk::DescriptorImageInfo {
            sampler: underwater_sampler,
            image_view: underwater_view,
            image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
        };

        // In menu_overlay.frag binding order; blur (4) starts as a font-atlas
        // placeholder until set_blur_texture.
        let tex_image_infos = [
            &font_img_info,
            &sprite_img_info,
            &item_img_info,
            &mc_font_img_info,
            &font_img_info,
            &favicon_img_info,
            &overlay_img_info,
            &underwater_img_info,
            &mc_font_color_img_info,
        ];
        let writes: Vec<_> = tex_image_infos
            .iter()
            .enumerate()
            .map(|(binding, info)| vk::WriteDescriptorSet {
                dst_set: tex_set,
                dst_binding: binding as u32,
                descriptor_count: 1,
                descriptor_type: vk::DescriptorType::CombinedImageSampler,
                image_info: *info,
                ..Default::default()
            })
            .collect();
        device.update_descriptor_sets(&writes, &[]);

        let (vertex_buffer, vertex_allocation) = util::create_host_buffer(
            device,
            allocator,
            (MAX_VERTICES * VERTEX_SIZE) as u64,
            vk::BufferUsageFlags::VertexBuffer,
            "menu_vertices",
        );

        let obfuscation_seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;

        Ok(Self {
            pipeline,
            invert_pipeline,
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
            mc_font,
            mc_font_color,
            font_layer_limit,
            mc_glyph_map,
            obfuscation_rng: ObfuscationRng::new(obfuscation_seed),
            vertex_buffer,
            vertex_allocation: Some(vertex_allocation),
            atlas,
            favicon_image,
            favicon_view,
            favicon_sampler,
            favicon_allocation: Some(favicon_alloc),
            favicon_regions: std::collections::HashMap::new(),
            drawn_inline_objects: std::collections::HashMap::new(),
            favicon_atlas_size: 1,
            overlay_image,
            overlay_view,
            overlay_sampler,
            overlay_allocation: Some(overlay_alloc),
            overlay_vignette_uv,
            overlay_pumpkin_uv,
            underwater_image,
            underwater_view,
            underwater_sampler,
            underwater_allocation: Some(underwater_alloc),
        })
    }

    /// Rebuilds the glyph atlases from the current pack stack. Call with the
    /// device idle: the old textures are destroyed right away.
    pub fn reload_minecraft_fonts(
        &mut self,
        device: &vk::Device,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        allocator: &Arc<Mutex<Allocator>>,
        font_sources: FontSources<'_>,
    ) -> Result<(), String> {
        let (glyph_map, pixels) = GlyphMap::load(font_sources, self.font_layer_limit)?;
        let (gray, color) =
            create_font_textures(device, queue, command_pool, allocator, Some(&pixels))?;
        let gray_info = gray.image_info();
        let color_info = color.image_info();
        let writes =
            [(3, &gray_info), (8, &color_info)].map(|(binding, info)| vk::WriteDescriptorSet {
                dst_set: self.tex_set,
                dst_binding: binding,
                descriptor_count: 1,
                descriptor_type: vk::DescriptorType::CombinedImageSampler,
                image_info: info,
                ..Default::default()
            });
        device.update_descriptor_sets(&writes, &[]);

        let mut alloc = util::lock_allocator(allocator);
        destroy_texture_resources(
            device,
            &mut alloc,
            &mut std::mem::replace(&mut self.mc_font, gray),
        );
        destroy_texture_resources(
            device,
            &mut alloc,
            &mut std::mem::replace(&mut self.mc_font_color, color),
        );
        self.mc_glyph_map = Some(glyph_map);
        Ok(())
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
        // Moved out so the element loop can record into it while `self` is
        // borrowed for the glyph and atlas sources; it keeps its allocation.
        let mut drawn_objects = std::mem::take(&mut self.drawn_inline_objects);
        let mut deferred_tooltips: Vec<&MenuElement> = Vec::new();
        let mut draw_ops: Vec<DrawOp> = Vec::new();
        let mut scissor_stack: Vec<[f32; 4]> = Vec::new();
        let mut cmd_start: u32 = 0;
        let mut cur_invert = false;
        let mut obfuscation_rng = self.obfuscation_rng;

        for elem in elements {
            if matches!(
                elem,
                MenuElement::Tooltip { .. }
                    | MenuElement::TooltipLines { .. }
                    | MenuElement::BundleTooltip { .. }
            ) {
                deferred_tooltips.push(elem);
                continue;
            }
            if matches!(
                elem,
                MenuElement::ScissorPush { .. } | MenuElement::ScissorPop
            ) {
                flush_draw_op(
                    &mut draw_ops,
                    &mut cmd_start,
                    vertices.len() as u32,
                    scissor_stack.last().copied(),
                    cur_invert,
                );
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
            // Invert-blended elements need their own pipeline, so they split the
            // batch the same way a scissor change does.
            let invert = matches!(elem, MenuElement::ImageInvert { .. });
            if invert != cur_invert {
                flush_draw_op(
                    &mut draw_ops,
                    &mut cmd_start,
                    vertices.len() as u32,
                    scissor_stack.last().copied(),
                    cur_invert,
                );
                cur_invert = invert;
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
                    let start_x = if *centered {
                        *x - self.mc_text_width(text, *scale) / 2.0
                    } else {
                        *x
                    };
                    let span = TextSpan::new(text.clone(), *color);
                    self.push_text_into(
                        &mut drawn_objects,
                        &mut vertices,
                        &[span],
                        McTextDraw {
                            x: start_x,
                            y: *y,
                            scale: *scale,
                            drop_shadow: true,
                        },
                        &mut obfuscation_rng,
                    );
                }
                MenuElement::TextFlat {
                    x,
                    y,
                    text,
                    scale,
                    color,
                } => {
                    let span = TextSpan::new(text.clone(), *color);
                    self.push_text_into(
                        &mut drawn_objects,
                        &mut vertices,
                        &[span],
                        McTextDraw {
                            x: *x,
                            y: *y,
                            scale: *scale,
                            drop_shadow: false,
                        },
                        &mut obfuscation_rng,
                    );
                }
                MenuElement::TextSpans {
                    x,
                    y,
                    spans,
                    scale,
                    centered,
                } => {
                    let start_x = if *centered {
                        *x - self.spans_width(spans, *scale) / 2.0
                    } else {
                        *x
                    };
                    self.push_text_into(
                        &mut drawn_objects,
                        &mut vertices,
                        spans,
                        McTextDraw {
                            x: start_x,
                            y: *y,
                            scale: *scale,
                            drop_shadow: true,
                        },
                        &mut obfuscation_rng,
                    );
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
                }
                | MenuElement::ImageInvert {
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
                    shadow,
                } => {
                    let start_x = if *centered {
                        *x - self.spans_width(spans, *scale) / 2.0
                    } else {
                        *x
                    };
                    self.push_text_into(
                        &mut drawn_objects,
                        &mut vertices,
                        spans,
                        McTextDraw {
                            x: start_x,
                            y: *y,
                            scale: *scale,
                            drop_shadow: *shadow,
                        },
                        &mut obfuscation_rng,
                    );
                }
                MenuElement::McTextRotated {
                    x,
                    y,
                    pivot,
                    rotation,
                    spans,
                    scale,
                    shadow,
                } => {
                    let start = vertices.len();
                    self.push_text_into(
                        &mut drawn_objects,
                        &mut vertices,
                        spans,
                        McTextDraw {
                            x: *x,
                            y: *y,
                            scale: *scale,
                            drop_shadow: *shadow,
                        },
                        &mut obfuscation_rng,
                    );
                    rotate_verts(&mut vertices[start..], *pivot, *rotation);
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
                        [1.0, 1.0, 1.0, 1.0],
                    );
                }
                MenuElement::SkinFace {
                    x,
                    y,
                    size,
                    uuid,
                    tint,
                } => {
                    push_atlas_image(
                        &mut vertices,
                        &self.favicon_regions,
                        &self.sprite_atlas,
                        uuid,
                        SpriteId::SteveHead,
                        *x,
                        *y,
                        *size,
                        *tint,
                    );
                }
                MenuElement::Vignette { w, h, brightness } => {
                    // Clamp mirrors vanilla Hud.extractVignette.
                    let b = brightness.clamp(0.0, 1.0);
                    push_fullscreen_quad(
                        &mut vertices,
                        *w,
                        *h,
                        self.overlay_vignette_uv,
                        [b, b, b, 1.0],
                        8.0,
                    );
                }
                MenuElement::PumpkinOverlay { w, h } => {
                    push_fullscreen_quad(
                        &mut vertices,
                        *w,
                        *h,
                        self.overlay_pumpkin_uv,
                        [1.0; 4],
                        7.0,
                    );
                }
                MenuElement::UnderwaterOverlay {
                    w,
                    h,
                    u0,
                    v0,
                    brightness,
                } => {
                    // Vanilla submitWater UVs: U decreases rightward, V
                    // increases downward.
                    push_fullscreen_quad(
                        &mut vertices,
                        *w,
                        *h,
                        [*u0 + 4.0, *v0, *u0, *v0 + 4.0],
                        [*brightness, *brightness, *brightness, 0.1],
                        9.0,
                    );
                }
                MenuElement::SleepOverlay { w, h, amount } => {
                    // Vanilla fills 0x101020 at (int)(220 * amount) alpha,
                    // blended in the gamma-space GL framebuffer. Our blend is
                    // premultiplied One / OneMinusSrcAlpha in linear on an sRGB
                    // target, so (as with the vignette) the gamma-space dst
                    // factor 1 - a becomes 1 - (1 - a)^2.2 and the premultiplied
                    // src term c * a is linearized as (c * a)^2.2.
                    let a = (220.0 * amount).floor() / 255.0;
                    let [r, g, b] = [16.0f32, 16.0, 32.0].map(|c| (c / 255.0 * a).powf(2.2));
                    push_fullscreen_quad(
                        &mut vertices,
                        *w,
                        *h,
                        [0.0; 4],
                        [r, g, b, 1.0 - (1.0 - a).powf(2.2)],
                        10.0,
                    );
                }
                _ => {}
            }
        }

        // Deferred tooltips always draw with the normal pipeline.
        if cur_invert {
            flush_draw_op(
                &mut draw_ops,
                &mut cmd_start,
                vertices.len() as u32,
                scissor_stack.last().copied(),
                true,
            );
            cur_invert = false;
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
                    let line_y = text_y + i as f32 * line_h;
                    self.push_text_into(
                        &mut drawn_objects,
                        &mut vertices,
                        &[span],
                        McTextDraw {
                            x: text_x,
                            y: line_y,
                            scale: *scale,
                            drop_shadow: true,
                        },
                        &mut obfuscation_rng,
                    );
                }
            }
            if let MenuElement::BundleTooltip {
                x,
                y,
                title,
                items,
                selected,
                fullness,
                item_scale,
                font_scale,
                screen_w,
                screen_h,
            } = elem
                && let Some(ref _gm) = self.mc_glyph_map
            {
                let gs = *item_scale;
                let fs = *font_scale;
                let shown =
                    crate::ui::bundle::shown_count(&azalea_inventory::components::BundleContents {
                        items: items.clone(),
                    });
                let rows = items.len().min(12).div_ceil(4);
                // GuiGraphicsExtractor takes the maximum width of every tooltip component.
                // The bundle image contributes 96px, while a longer title can widen the
                // whole tooltip; the image itself is then centered inside that width.
                let content_w = (96.0 * gs).max(self.mc_text_width(title, fs));
                let bundle_x_offset = ((content_w - 96.0 * gs) / 2.0).floor();
                let empty_description =
                    crate::lang::translate("item.minecraft.bundle.empty.description")
                        .unwrap_or("Can hold a mixed stack of items");
                let empty_lines = if items.is_empty() {
                    wrap_plain_text(empty_description, 96.0 * gs, |text| {
                        self.mc_text_width(text, fs)
                    })
                } else {
                    Vec::new()
                };
                let title_h = 11.0 * gs; // first tooltip component gets vanilla's 2px gap
                let image_h = if items.is_empty() {
                    empty_lines.len() as f32 * 9.0 * gs + 21.0 * gs
                } else {
                    (rows as f32 * 24.0 + 21.0) * gs
                };
                let content_h = title_h + image_h;
                let mut left = *x + 12.0;
                let mut top = *y - 12.0;
                if left + content_w > *screen_w {
                    left = (*x - 24.0 - content_w).max(4.0);
                }
                if top + content_h > *screen_h {
                    top = (*screen_h - content_h - 4.0).max(4.0);
                }
                let px = gs;
                // TooltipRenderUtil uses x/y - 12 and content width/height + 24:
                // 3px padding plus the 9px sprite margin on each side.
                let padding = 3.0 * px;
                let margin = 9.0 * px;
                let inset = padding + margin;
                let bg_x = left - inset;
                let bg_y = top - inset;
                let bg_w = content_w + 2.0 * padding + 2.0 * margin;
                let bg_h = content_h + 2.0 * padding + 2.0 * margin;
                let white = [1.0f32; 4];
                if let Some(bg) = self.sprite_atlas.regions.get(&SpriteId::TooltipBackground) {
                    push_nine_slice(&mut vertices, bg_x, bg_y, bg_w, bg_h, bg, margin, white);
                }
                if let Some(frame) = self.sprite_atlas.regions.get(&SpriteId::TooltipFrame) {
                    push_nine_slice(
                        &mut vertices,
                        bg_x,
                        bg_y,
                        bg_w,
                        bg_h,
                        frame,
                        10.0 * px,
                        white,
                    );
                }
                // Tooltip text/components render after the background/frame. Drawing the
                // title before the nine-slice darkened it through the translucent background.
                self.push_text(
                    &mut vertices,
                    &[TextSpan::new(title.clone(), white)],
                    McTextDraw {
                        x: left,
                        y: top,
                        scale: fs,
                        drop_shadow: true,
                    },
                    &mut obfuscation_rng,
                );
                let top = top + title_h;
                if items.is_empty() {
                    for (line_no, text) in empty_lines.iter().enumerate() {
                        self.push_text(
                            &mut vertices,
                            &[TextSpan::new(text.clone(), [0.6667, 0.6667, 0.6667, 1.0])],
                            McTextDraw {
                                x: left + bundle_x_offset,
                                y: top + line_no as f32 * 9.0 * gs,
                                scale: fs,
                                drop_shadow: true,
                            },
                            &mut obfuscation_rng,
                        );
                    }
                } else {
                    let shown_items = &items[..shown.min(items.len())];
                    let overflow = items.len() > 12;
                    let grid_y = top + rows as f32 * 24.0 * gs;
                    let mut slot_number = 1usize;
                    for row in 1..=rows {
                        for col in 1..=4usize {
                            let draw_x =
                                left + bundle_x_offset + 96.0 * gs - col as f32 * 24.0 * gs;
                            let draw_y = grid_y - row as f32 * 24.0 * gs;
                            if overflow && col == 1 && row == 1 {
                                let hidden: i32 = items
                                    .iter()
                                    .skip(shown_items.len())
                                    .map(|s| s.count())
                                    .sum();
                                let hidden_text = format!("+{hidden}");
                                let hidden_w = self.mc_text_width(&hidden_text, fs);
                                self.push_text(
                                    &mut vertices,
                                    &[TextSpan::new(hidden_text, white)],
                                    McTextDraw {
                                        x: draw_x + 12.0 * gs - hidden_w / 2.0,
                                        y: draw_y + 10.0 * gs,
                                        scale: fs,
                                        drop_shadow: true,
                                    },
                                    &mut obfuscation_rng,
                                );
                                continue;
                            }
                            if slot_number > shown_items.len() {
                                continue;
                            }
                            let visual = shown_items.len() - slot_number;
                            let highlighted = visual as i32 == *selected;
                            let back = if highlighted {
                                SpriteId::BundleSlotHighlightBack
                            } else {
                                SpriteId::BundleSlotBackground
                            };
                            if let Some(region) = self.sprite_atlas.regions.get(&back) {
                                push_quad(
                                    &mut vertices,
                                    draw_x,
                                    draw_y,
                                    24.0 * gs,
                                    24.0 * gs,
                                    region.u0,
                                    region.v0,
                                    region.u1,
                                    region.v1,
                                    white,
                                    2.0,
                                    [0.0, 0.0],
                                    0.0,
                                );
                            }
                            if let azalea_inventory::ItemStack::Present(data) = &shown_items[visual]
                            {
                                let name = crate::player::inventory::item_resource_name(data.kind);
                                if let Some(uv) = item_atlas_uvs.get(&name) {
                                    push_quad(
                                        &mut vertices,
                                        draw_x + 4.0 * gs,
                                        draw_y + 4.0 * gs,
                                        16.0 * gs,
                                        16.0 * gs,
                                        uv[0],
                                        uv[1],
                                        uv[2],
                                        uv[3],
                                        white,
                                        3.0,
                                        [0.0, 0.0],
                                        0.0,
                                    );
                                }
                                if data.count > 1 {
                                    let count_text = data.count.to_string();
                                    let count_w = self.mc_text_width(&count_text, fs);
                                    self.push_text(
                                        &mut vertices,
                                        &[TextSpan::new(count_text, white)],
                                        McTextDraw {
                                            x: draw_x + 21.0 * gs - count_w,
                                            y: draw_y + 13.0 * gs,
                                            scale: fs,
                                            drop_shadow: true,
                                        },
                                        &mut obfuscation_rng,
                                    );
                                }
                            }
                            if highlighted
                                && let Some(region) = self
                                    .sprite_atlas
                                    .regions
                                    .get(&SpriteId::BundleSlotHighlightFront)
                            {
                                push_quad(
                                    &mut vertices,
                                    draw_x,
                                    draw_y,
                                    24.0 * gs,
                                    24.0 * gs,
                                    region.u0,
                                    region.v0,
                                    region.u1,
                                    region.v1,
                                    white,
                                    2.0,
                                    [0.0, 0.0],
                                    0.0,
                                );
                            }
                            slot_number += 1;
                        }
                    }
                    if *selected >= 0
                        && (*selected as usize) < items.len()
                        && let azalea_inventory::ItemStack::Present(data) =
                            &items[*selected as usize]
                    {
                        // Vanilla ClientBundleTooltip calls graphics.tooltip for the
                        // selected stack name: it is a second framed tooltip positioned
                        // 15px above the bundle image, not text drawn inside this frame.
                        let name = crate::ui::common::item_display_name(data);
                        let name_w = self.mc_text_width(&name, fs);
                        // These are the coordinates passed to vanilla's `graphics.tooltip`,
                        // not the final text origin. DefaultTooltipPositioner then applies
                        // (+12, -12) and clamps the resulting tooltip to the screen.
                        let anchor_x = left + content_w / 2.0 - 12.0 * gs - name_w / 2.0;
                        let anchor_y = top - 15.0 * gs;
                        let mut selected_x = anchor_x + 12.0 * gs;
                        let mut selected_y = anchor_y - 12.0 * gs;
                        if selected_x + name_w > *screen_w {
                            selected_x = (selected_x - 24.0 * gs - name_w).max(4.0 * gs);
                        }
                        // A single text component has vanilla tooltip height 7 (9 - 2).
                        let selected_content_h = 7.0 * gs;
                        if selected_y + selected_content_h + 3.0 * gs > *screen_h {
                            selected_y = *screen_h - selected_content_h - 3.0 * gs;
                        }
                        let selected_padding = 3.0 * gs;
                        let selected_margin = 9.0 * gs;
                        let selected_inset = selected_padding + selected_margin;
                        let selected_bg_x = selected_x - selected_inset;
                        let selected_bg_y = selected_y - selected_inset;
                        let selected_bg_w = name_w + 2.0 * selected_inset;
                        let selected_bg_h = selected_content_h + 2.0 * selected_inset;
                        if let Some(bg) =
                            self.sprite_atlas.regions.get(&SpriteId::TooltipBackground)
                        {
                            push_nine_slice(
                                &mut vertices,
                                selected_bg_x,
                                selected_bg_y,
                                selected_bg_w,
                                selected_bg_h,
                                bg,
                                selected_margin,
                                white,
                            );
                        }
                        if let Some(frame) = self.sprite_atlas.regions.get(&SpriteId::TooltipFrame)
                        {
                            push_nine_slice(
                                &mut vertices,
                                selected_bg_x,
                                selected_bg_y,
                                selected_bg_w,
                                selected_bg_h,
                                frame,
                                10.0 * gs,
                                white,
                            );
                        }
                        self.push_text(
                            &mut vertices,
                            &[TextSpan::new(name, white)],
                            McTextDraw {
                                x: selected_x,
                                y: selected_y,
                                scale: fs,
                                drop_shadow: true,
                            },
                            &mut obfuscation_rng,
                        );
                    }
                }
                let description_h = if items.is_empty() {
                    empty_lines.len() as f32 * 9.0 * gs
                } else {
                    0.0
                };
                let grid_h = rows as f32 * 24.0 * gs;
                let bar_y = top + description_h.max(grid_h) + 4.0 * gs;
                let fill = ((*fullness * 94.0).floor() as i32).clamp(0, 94) as f32;
                let fill_id = if *fullness >= 1.0 {
                    SpriteId::BundleProgressFull
                } else {
                    SpriteId::BundleProgressFill
                };
                if fill > 0.0
                    && let Some(region) = self.sprite_atlas.regions.get(&fill_id)
                {
                    push_nine_slice(
                        &mut vertices,
                        left + bundle_x_offset + gs,
                        bar_y,
                        fill * gs,
                        13.0 * gs,
                        region,
                        2.0 * gs,
                        white,
                    );
                }
                if let Some(region) = self
                    .sprite_atlas
                    .regions
                    .get(&SpriteId::BundleProgressBorder)
                {
                    push_nine_slice(
                        &mut vertices,
                        left + bundle_x_offset,
                        bar_y,
                        96.0 * gs,
                        13.0 * gs,
                        region,
                        2.0 * gs,
                        white,
                    );
                }
                let label = if *fullness <= 0.0 {
                    crate::lang::translate("item.minecraft.bundle.empty")
                } else if *fullness >= 1.0 {
                    crate::lang::translate("item.minecraft.bundle.full")
                } else {
                    None
                };
                if let Some(label) = label {
                    let label_w = self.mc_text_width(label, fs);
                    self.push_text(
                        &mut vertices,
                        &[TextSpan::new(label.to_string(), white)],
                        McTextDraw {
                            x: left + bundle_x_offset + 48.0 * gs - label_w / 2.0,
                            y: bar_y + 3.0 * gs,
                            scale: fs,
                            drop_shadow: true,
                        },
                        &mut obfuscation_rng,
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

                let line_widths: Vec<f32> = lines
                    .iter()
                    .map(|l| (self.spans_width(&l.spans, *scale) + px).ceil())
                    .collect();
                let content_w = line_widths.iter().copied().fold(0.0f32, f32::max);
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

                for (i, (line, line_w)) in lines.iter().zip(&line_widths).enumerate() {
                    let line_x = if line.right_align {
                        text_x + content_w - line_w
                    } else {
                        text_x
                    };
                    let line_y = text_y + i as f32 * line_h;
                    self.push_text_into(
                        &mut drawn_objects,
                        &mut vertices,
                        &line.spans,
                        McTextDraw {
                            x: line_x,
                            y: line_y,
                            scale: *scale,
                            drop_shadow: true,
                        },
                        &mut obfuscation_rng,
                    );
                }
            }
        }

        self.obfuscation_rng = obfuscation_rng;
        self.drawn_inline_objects = drawn_objects;

        flush_draw_op(
            &mut draw_ops,
            &mut cmd_start,
            vertices.len() as u32,
            scissor_stack.last().copied(),
            cur_invert,
        );

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
            // Descriptor sets and vertex buffer stay valid across pipeline
            // switches (both pipelines share the layout).
            cmd.bind_descriptor_sets(
                vk::PipelineBindPoint::Graphics,
                self.pipeline_layout,
                0,
                &[self.globals_set, self.tex_set],
                &[],
            );
            cmd.bind_vertex_buffers(0, &[self.vertex_buffer], &[0]);
        }
        let mut bound_invert: Option<bool> = None;
        for op in &draw_ops {
            if bound_invert != Some(op.invert) {
                cmd.bind_pipeline(
                    vk::PipelineBindPoint::Graphics,
                    if op.invert {
                        self.invert_pipeline
                    } else {
                        self.pipeline
                    },
                );
                bound_invert = Some(op.invert);
            }
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
            // TODO: clamp to `written`; past MAX_VERTICES this draws vertices
            // that were never uploaded.
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
        self.text_width_in(text, scale, None)
    }

    /// Text width in the SGA (`minecraft:alt`) glyphs.
    pub fn mc_text_width_sga(&self, text: &str, scale: f32) -> f32 {
        self.text_width_in(text, scale, Some("minecraft:alt"))
    }

    fn text_width_in(&self, text: &str, scale: f32, font: Option<&str>) -> f32 {
        let Some(ref gm) = self.mc_glyph_map else {
            return 0.0;
        };
        (run_advance(gm, text, font, false, false) * scale / gm.cell_h as f32).ceil()
    }

    /// Width of a multi-span line, honoring each span's font, bold and inline
    /// objects.
    pub fn spans_width(&self, spans: &[TextSpan], scale: f32) -> f32 {
        let Some(ref gm) = self.mc_glyph_map else {
            return 0.0;
        };
        let raw: f32 = spans
            .iter()
            .map(|s| {
                run_advance(
                    gm,
                    &s.text,
                    s.font.as_deref(),
                    s.bold,
                    s.inline_object.is_some(),
                )
            })
            .sum();
        (raw * scale / gm.cell_h as f32).ceil()
    }

    /// Pushes Minecraft-font text; nothing when no fonts loaded.
    #[allow(clippy::too_many_arguments)]
    fn push_text_into(
        &self,
        drawn_objects: &mut std::collections::HashMap<String, InlineObject>,
        vertices: &mut Vec<Vertex>,
        spans: &[TextSpan],
        draw: McTextDraw,
        obfuscation_rng: &mut ObfuscationRng,
    ) {
        if let Some(gm) = &self.mc_glyph_map {
            let sources = McTextSources {
                gm,
                dynamic_regions: &self.favicon_regions,
                sprite_atlas: &self.sprite_atlas,
            };
            push_mc_text(
                vertices,
                sources,
                spans,
                draw,
                obfuscation_rng,
                drawn_objects,
            );
        }
    }

    /// The inline objects drawn since the last call, which the app loads into
    /// the atlas for the next frame.
    pub fn drain_drawn_inline_objects(
        &mut self,
    ) -> std::collections::hash_map::Drain<'_, String, InlineObject> {
        self.drawn_inline_objects.drain()
    }

    /// Points an inline object's key at one of its animation frames, which
    /// were packed under their own keys.
    pub fn set_inline_object_frame(&mut self, key: &str, frame_key: &str) {
        if let Some(region) = self.favicon_regions.get(frame_key).copied() {
            self.favicon_regions.insert(key.to_owned(), region);
        }
    }

    pub fn recreate_pipeline(&mut self, device: &vk::Device, render_pass: vk::RenderPass) {
        device.destroy_pipeline(self.pipeline, None);
        device.destroy_pipeline(self.invert_pipeline, None);
        self.pipeline = create_pipeline(device, render_pass, self.pipeline_layout, false);
        self.invert_pipeline = create_pipeline(device, render_pass, self.pipeline_layout, true);
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
        destroy_texture_resources(device, &mut alloc, &mut self.mc_font);
        destroy_texture_resources(device, &mut alloc, &mut self.mc_font_color);

        device.destroy_sampler(self.favicon_sampler, None);
        device.destroy_image_view(self.favicon_view, None);
        device.destroy_image(self.favicon_image, None);

        if let Some(a) = self.favicon_allocation.take() {
            alloc.free(a).ok();
        }

        device.destroy_sampler(self.overlay_sampler, None);
        device.destroy_image_view(self.overlay_view, None);
        device.destroy_image(self.overlay_image, None);
        if let Some(a) = self.overlay_allocation.take() {
            alloc.free(a).ok();
        }

        device.destroy_sampler(self.underwater_sampler, None);
        device.destroy_image_view(self.underwater_view, None);
        device.destroy_image(self.underwater_image, None);
        if let Some(a) = self.underwater_allocation.take() {
            alloc.free(a).ok();
        }

        drop(alloc);

        device.destroy_pipeline(self.pipeline, None);
        device.destroy_pipeline(self.invert_pipeline, None);
        device.destroy_pipeline_layout(self.pipeline_layout, None);
        device.destroy_descriptor_pool(self.descriptor_pool, None);
        device.destroy_descriptor_set_layout(self.globals_layout, None);
        device.destroy_descriptor_set_layout(self.tex_layout, None);
    }
}

pub struct TooltipLine {
    pub spans: Vec<TextSpan>,
    /// Aligned against the widest line in the tooltip instead of the left.
    pub right_align: bool,
}

impl TooltipLine {
    /// A single-color line.
    pub fn new(text: String, color: [f32; 4]) -> Self {
        Self {
            spans: vec![TextSpan::new(text, color)],
            right_align: false,
        }
    }

    /// A single-color right-aligned line.
    pub fn right_aligned(text: String, color: [f32; 4]) -> Self {
        Self {
            right_align: true,
            ..Self::new(text, color)
        }
    }
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
    TextSpans {
        x: f32,
        y: f32,
        spans: Vec<crate::ui::text::TextSpan>,
        scale: f32,
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
    /// `Image` drawn with vanilla's INVERT blend (`RenderPipelines.CROSSHAIR`).
    ImageInvert {
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
        shadow: bool,
    },
    /// Vanilla `SplashRenderer`: text laid out unrotated from `(x, y)`, then
    /// spun about `pivot` (radians, clockwise on screen like vanilla's
    /// `Matrix3x2f.rotate`).
    McTextRotated {
        x: f32,
        y: f32,
        pivot: (f32, f32),
        rotation: f32,
        spans: Vec<TextSpan>,
        scale: f32,
        shadow: bool,
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
    BundleTooltip {
        x: f32,
        y: f32,
        title: String,
        items: Vec<azalea_inventory::ItemStack>,
        selected: i32,
        fullness: f32,
        item_scale: f32,
        font_scale: f32,
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
        tint: [f32; 4],
    },
    /// Split marker for the menu draw: elements before it are rendered into the
    /// scene so the blur pass captures them; elements after are drawn sharp on
    /// top. Used to render the title screen blurred behind the Friends dialog.
    BlurBackdrop,
    /// Full-screen vignette, multiplying the framebuffer down by `brightness`.
    Vignette {
        w: f32,
        h: f32,
        brightness: f32,
    },
    /// Full-screen pumpkin-blur camera overlay.
    PumpkinOverlay {
        w: f32,
        h: f32,
    },
    /// Full-screen underwater tint, its 4x-tiled UVs scrolled to `(u0, v0)`.
    UnderwaterOverlay {
        w: f32,
        h: f32,
        u0: f32,
        v0: f32,
        brightness: f32,
    },
    /// Full-screen sleep fade (vanilla Hud.extractSleepOverlay): 0x101020
    /// filled at alpha (int)(220 * amount) / 255.
    SleepOverlay {
        w: f32,
        h: f32,
        amount: f32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SpriteId {
    Hotbar,
    HotbarSelection,
    HeartContainer,
    HeartFull,
    HeartHalf,
    HeartAbsorbingFull,
    HeartAbsorbingHalf,
    HeartVehicleContainer,
    HeartVehicleFull,
    HeartVehicleHalf,
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
    Crosshair,
    CrosshairAttackIndicatorFull,
    CrosshairAttackIndicatorBackground,
    CrosshairAttackIndicatorProgress,
    HotbarAttackIndicatorBackground,
    HotbarAttackIndicatorProgress,
    EffectBackground,
    EffectBackgroundAmbient,
    /// Index into `mob_effect::MOB_EFFECTS`.
    MobEffect(u8),
    /// Boss bar sprites, indexed by the wire ordinals: color 0..=6 (pink,
    /// blue, red, green, yellow, purple, white), notch 0..=3 (6/10/12/20).
    BossBarBackground(u8),
    BossBarProgress(u8),
    BossBarNotchedBackground(u8),
    BossBarNotchedProgress(u8),
    ChatModified,
    ToastAdvancement,
    ToastRecipe,
    ToastTutorial,
    ToastMovementKeys,
    ToastMouse,
    ToastTree,
    ToastRecipeBook,
    ToastWoodenPlanks,
    ToastSocialInteractions,
    ToastRightClick,
    JumpBarBackground,
    JumpBarProgress,
    LocatorBarBackground,
    LocatorDotDefault0,
    LocatorDotDefault1,
    LocatorDotDefault2,
    LocatorDotDefault3,
    LocatorDotBowtie,
    LocatorDotMissing,
    LocatorArrowUp0,
    LocatorArrowUp1,
    LocatorArrowDown0,
    LocatorArrowDown1,
    GameModeSwitcherBackground,
    GameModeSwitcherSlot,
    GameModeSwitcherSelection,
    InventoryBackground,
    CraftingTableBackground,
    FurnaceBackground,
    BlastFurnaceBackground,
    SmokerBackground,
    Generic54Top,
    Generic54Bottom,
    ShulkerBoxBackground,
    AnvilBackground,
    AnvilTextField,
    AnvilTextFieldDisabled,
    AnvilError,
    EnchantingTableBackground,
    EnchantmentSlot,
    EnchantmentSlotDisabled,
    EnchantmentSlotHighlighted,
    EnchantmentLevel1,
    EnchantmentLevel2,
    EnchantmentLevel3,
    EnchantmentLevel1Disabled,
    EnchantmentLevel2Disabled,
    EnchantmentLevel3Disabled,
    EmptyLapisLazuli,
    FurnaceLitProgress,
    FurnaceBurnProgress,
    BlastFurnaceLitProgress,
    BlastFurnaceBurnProgress,
    SmokerLitProgress,
    SmokerBurnProgress,
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
    BundleProgressBorder,
    BundleProgressFill,
    BundleProgressFull,
    BundleSlotBackground,
    BundleSlotHighlightBack,
    BundleSlotHighlightFront,
    TooltipBackground,
    TooltipFrame,
    Scroller,
    ScrollerBackground,
    WarningButton,
    WarningButtonHighlighted,
    Checkbox,
    CheckboxHighlighted,
    CheckboxSelected,
    CheckboxSelectedHighlighted,
    Ping1,
    Ping2,
    Ping3,
    Ping4,
    Ping5,
    PingUnknown,
    ServerJoin,
    ServerJoinHighlighted,
    Tab,
    TabHighlighted,
    TabSelected,
    TabSelectedHighlighted,
    WorldJoin,
    WorldJoinHighlighted,
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
    PommeLogo,
    MinecraftLogo,
    MinecraftEdition,
    IconFriends,
    IconLanguage,
    IconAccessibility,
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
    NetherPortal,
    SpectatorClose,
    SpectatorScrollLeft,
    SpectatorScrollRight,
    SpectatorTeleportToPlayer,
    SpectatorTeleportToTeam,
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

/// Gutter kept on all four sides of every sprite. Sprites are sampled with
/// NEAREST, so a quad edge landing a hair outside its region reads the
/// neighbour; a transparent moat makes that a no-op instead of a smear. Vanilla
/// bakes the same symmetric one-texel pad at mip 0 (`Stitcher.registerSprite`).
const SPRITE_PAD: u32 = 1;

/// Vulkan's spec floor for `maxImageDimension2D`, so this is safe on every
/// conformant device without querying it. The GUI sprites pack into a fraction
/// of it, so the cap is never binding in practice.
const MAX_SPRITE_ATLAS_SIZE: u32 = 4096;

/// Spins already-built vertices about a screen-space pivot. Every quad the
/// pipeline emits is axis-aligned, so rotation is applied after the fact
/// rather than threaded through each push helper.
fn rotate_verts(verts: &mut [Vertex], pivot: (f32, f32), rotation: f32) {
    let (sin, cos) = rotation.sin_cos();
    for v in verts {
        let dx = v.pos[0] - pivot.0;
        let dy = v.pos[1] - pivot.1;
        v.pos = [pivot.0 + dx * cos - dy * sin, pivot.1 + dx * sin + dy * cos];
    }
}

/// Downscales a straight-alpha sprite, filtering it premultiplied so the
/// transparent border's black stays out of the edge texels' colour. The atlas
/// stores straight alpha and the shader premultiplies at sample time, so a
/// naive filter would darken every anti-aliased edge twice over.
fn downscale_straight_alpha(src: &image::RgbaImage, size: u32) -> image::RgbaImage {
    let mut premul = image::Rgba32FImage::new(src.width(), src.height());
    for (dst, src) in premul.pixels_mut().zip(src.pixels()) {
        let a = f32::from(src[3]) / 255.0;
        *dst = image::Rgba([
            f32::from(src[0]) / 255.0 * a,
            f32::from(src[1]) / 255.0 * a,
            f32::from(src[2]) / 255.0 * a,
            a,
        ]);
    }

    // Lanczos rings and the resize clamps each channel on its own, so colour
    // can land above its own alpha; undo the premultiply against that clamp.
    let scaled =
        image::imageops::resize(&premul, size, size, image::imageops::FilterType::Lanczos3);
    let mut out = image::RgbaImage::new(size, size);
    for (dst, src) in out.pixels_mut().zip(scaled.pixels()) {
        let a = src[3].clamp(0.0, 1.0);
        let straight = |c: f32| {
            if a == 0.0 {
                0
            } else {
                ((c / a).clamp(0.0, 1.0) * 255.0).round() as u8
            }
        };
        *dst = image::Rgba([
            straight(src[0]),
            straight(src[1]),
            straight(src[2]),
            (a * 255.0).round() as u8,
        ]);
    }
    out
}

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
            SpriteId::GameModeSwitcherBackground,
            "minecraft/textures/gui/container/gamemode_switcher.png",
            0.0,
        ),
        (
            SpriteId::GameModeSwitcherSlot,
            "minecraft/textures/gui/sprites/gamemode_switcher/slot.png",
            0.0,
        ),
        (
            SpriteId::GameModeSwitcherSelection,
            "minecraft/textures/gui/sprites/gamemode_switcher/selection.png",
            0.0,
        ),
        (
            SpriteId::HotbarSelection,
            "minecraft/textures/gui/sprites/hud/hotbar_selection.png",
            0.0,
        ),
        (
            SpriteId::EffectBackground,
            "minecraft/textures/gui/sprites/hud/effect_background.png",
            0.0,
        ),
        (
            SpriteId::EffectBackgroundAmbient,
            "minecraft/textures/gui/sprites/hud/effect_background_ambient.png",
            0.0,
        ),
        (
            SpriteId::SpectatorClose,
            "minecraft/textures/gui/sprites/spectator/close.png",
            0.0,
        ),
        (
            SpriteId::SpectatorScrollLeft,
            "minecraft/textures/gui/sprites/spectator/scroll_left.png",
            0.0,
        ),
        (
            SpriteId::SpectatorScrollRight,
            "minecraft/textures/gui/sprites/spectator/scroll_right.png",
            0.0,
        ),
        (
            SpriteId::SpectatorTeleportToPlayer,
            "minecraft/textures/gui/sprites/spectator/teleport_to_player.png",
            0.0,
        ),
        (
            SpriteId::SpectatorTeleportToTeam,
            "minecraft/textures/gui/sprites/spectator/teleport_to_team.png",
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
            SpriteId::HeartAbsorbingFull,
            "minecraft/textures/gui/sprites/hud/heart/absorbing_full.png",
            0.0,
        ),
        (
            SpriteId::HeartAbsorbingHalf,
            "minecraft/textures/gui/sprites/hud/heart/absorbing_half.png",
            0.0,
        ),
        (
            SpriteId::HeartVehicleContainer,
            "minecraft/textures/gui/sprites/hud/heart/vehicle_container.png",
            0.0,
        ),
        (
            SpriteId::HeartVehicleFull,
            "minecraft/textures/gui/sprites/hud/heart/vehicle_full.png",
            0.0,
        ),
        (
            SpriteId::HeartVehicleHalf,
            "minecraft/textures/gui/sprites/hud/heart/vehicle_half.png",
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
            SpriteId::Crosshair,
            "minecraft/textures/gui/sprites/hud/crosshair.png",
            0.0,
        ),
        (
            SpriteId::CrosshairAttackIndicatorFull,
            "minecraft/textures/gui/sprites/hud/crosshair_attack_indicator_full.png",
            0.0,
        ),
        (
            SpriteId::CrosshairAttackIndicatorBackground,
            "minecraft/textures/gui/sprites/hud/crosshair_attack_indicator_background.png",
            0.0,
        ),
        (
            SpriteId::CrosshairAttackIndicatorProgress,
            "minecraft/textures/gui/sprites/hud/crosshair_attack_indicator_progress.png",
            0.0,
        ),
        (
            SpriteId::HotbarAttackIndicatorBackground,
            "minecraft/textures/gui/sprites/hud/hotbar_attack_indicator_background.png",
            0.0,
        ),
        (
            SpriteId::HotbarAttackIndicatorProgress,
            "minecraft/textures/gui/sprites/hud/hotbar_attack_indicator_progress.png",
            0.0,
        ),
        (
            SpriteId::JumpBarBackground,
            "minecraft/textures/gui/sprites/hud/jump_bar_background.png",
            0.0,
        ),
        (
            SpriteId::JumpBarProgress,
            "minecraft/textures/gui/sprites/hud/jump_bar_progress.png",
            0.0,
        ),
        (
            SpriteId::LocatorDotDefault0,
            "minecraft/textures/gui/sprites/hud/locator_bar_dot/default_0.png",
            0.0,
        ),
        (
            SpriteId::LocatorDotDefault1,
            "minecraft/textures/gui/sprites/hud/locator_bar_dot/default_1.png",
            0.0,
        ),
        (
            SpriteId::LocatorDotDefault2,
            "minecraft/textures/gui/sprites/hud/locator_bar_dot/default_2.png",
            0.0,
        ),
        (
            SpriteId::LocatorDotDefault3,
            "minecraft/textures/gui/sprites/hud/locator_bar_dot/default_3.png",
            0.0,
        ),
        (
            SpriteId::LocatorDotBowtie,
            "minecraft/textures/gui/sprites/hud/locator_bar_dot/bowtie.png",
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
            SpriteId::AnvilTextField,
            "minecraft/textures/gui/sprites/container/anvil/text_field.png",
            0.0,
        ),
        (
            SpriteId::AnvilTextFieldDisabled,
            "minecraft/textures/gui/sprites/container/anvil/text_field_disabled.png",
            0.0,
        ),
        (
            SpriteId::AnvilError,
            "minecraft/textures/gui/sprites/container/anvil/error.png",
            0.0,
        ),
        (
            SpriteId::EnchantmentSlot,
            "minecraft/textures/gui/sprites/container/enchanting_table/enchantment_slot.png",
            0.0,
        ),
        (
            SpriteId::EnchantmentSlotDisabled,
            "minecraft/textures/gui/sprites/container/enchanting_table/enchantment_slot_disabled.png",
            0.0,
        ),
        (
            SpriteId::EnchantmentSlotHighlighted,
            "minecraft/textures/gui/sprites/container/enchanting_table/enchantment_slot_highlighted.png",
            0.0,
        ),
        (
            SpriteId::EnchantmentLevel1,
            "minecraft/textures/gui/sprites/container/enchanting_table/level_1.png",
            0.0,
        ),
        (
            SpriteId::EnchantmentLevel2,
            "minecraft/textures/gui/sprites/container/enchanting_table/level_2.png",
            0.0,
        ),
        (
            SpriteId::EnchantmentLevel3,
            "minecraft/textures/gui/sprites/container/enchanting_table/level_3.png",
            0.0,
        ),
        (
            SpriteId::EnchantmentLevel1Disabled,
            "minecraft/textures/gui/sprites/container/enchanting_table/level_1_disabled.png",
            0.0,
        ),
        (
            SpriteId::EnchantmentLevel2Disabled,
            "minecraft/textures/gui/sprites/container/enchanting_table/level_2_disabled.png",
            0.0,
        ),
        (
            SpriteId::EnchantmentLevel3Disabled,
            "minecraft/textures/gui/sprites/container/enchanting_table/level_3_disabled.png",
            0.0,
        ),
        (
            SpriteId::EmptyLapisLazuli,
            "minecraft/textures/gui/sprites/container/slot/lapis_lazuli.png",
            0.0,
        ),
        (
            SpriteId::FurnaceLitProgress,
            "minecraft/textures/gui/sprites/container/furnace/lit_progress.png",
            0.0,
        ),
        (
            SpriteId::FurnaceBurnProgress,
            "minecraft/textures/gui/sprites/container/furnace/burn_progress.png",
            0.0,
        ),
        (
            SpriteId::BlastFurnaceLitProgress,
            "minecraft/textures/gui/sprites/container/blast_furnace/lit_progress.png",
            0.0,
        ),
        (
            SpriteId::BlastFurnaceBurnProgress,
            "minecraft/textures/gui/sprites/container/blast_furnace/burn_progress.png",
            0.0,
        ),
        (
            SpriteId::SmokerLitProgress,
            "minecraft/textures/gui/sprites/container/smoker/lit_progress.png",
            0.0,
        ),
        (
            SpriteId::SmokerBurnProgress,
            "minecraft/textures/gui/sprites/container/smoker/burn_progress.png",
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
            1.0,
        ),
        (
            SpriteId::SliderTrackHover,
            "minecraft/textures/gui/sprites/widget/slider_highlighted.png",
            1.0,
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
            SpriteId::BundleProgressBorder,
            "minecraft/textures/gui/sprites/container/bundle/bundle_progressbar_border.png",
            2.0,
        ),
        (
            SpriteId::BundleProgressFill,
            "minecraft/textures/gui/sprites/container/bundle/bundle_progressbar_fill.png",
            2.0,
        ),
        (
            SpriteId::BundleProgressFull,
            "minecraft/textures/gui/sprites/container/bundle/bundle_progressbar_full.png",
            2.0,
        ),
        (
            SpriteId::BundleSlotBackground,
            "minecraft/textures/gui/sprites/container/bundle/slot_background.png",
            0.0,
        ),
        (
            SpriteId::BundleSlotHighlightBack,
            "minecraft/textures/gui/sprites/container/bundle/slot_highlight_back.png",
            0.0,
        ),
        (
            SpriteId::BundleSlotHighlightFront,
            "minecraft/textures/gui/sprites/container/bundle/slot_highlight_front.png",
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
        // `DialogScreen.WARNING_BUTTON_SPRITES` and `Checkbox`, all plain
        // 20x20 blits.
        (
            SpriteId::WarningButton,
            "minecraft/textures/gui/sprites/dialog/warning_button.png",
            0.0,
        ),
        (
            SpriteId::WarningButtonHighlighted,
            "minecraft/textures/gui/sprites/dialog/warning_button_highlighted.png",
            0.0,
        ),
        (
            SpriteId::Checkbox,
            "minecraft/textures/gui/sprites/widget/checkbox.png",
            0.0,
        ),
        (
            SpriteId::CheckboxHighlighted,
            "minecraft/textures/gui/sprites/widget/checkbox_highlighted.png",
            0.0,
        ),
        (
            SpriteId::CheckboxSelected,
            "minecraft/textures/gui/sprites/widget/checkbox_selected.png",
            0.0,
        ),
        (
            SpriteId::CheckboxSelectedHighlighted,
            "minecraft/textures/gui/sprites/widget/checkbox_selected_highlighted.png",
            0.0,
        ),
        (
            SpriteId::FriendsBackground,
            "minecraft/textures/gui/sprites/friends/background.png",
            8.0,
        ),
        // `CommonButtons` icons for the title screen's bottom row.
        (
            SpriteId::IconFriends,
            "minecraft/textures/gui/sprites/friends/friends.png",
            0.0,
        ),
        (
            SpriteId::IconLanguage,
            "minecraft/textures/gui/sprites/icon/language.png",
            0.0,
        ),
        (
            SpriteId::IconAccessibility,
            "minecraft/textures/gui/sprites/icon/accessibility.png",
            0.0,
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
            SpriteId::Tab,
            "minecraft/textures/gui/sprites/widget/tab.png",
            2.0,
        ),
        (
            SpriteId::TabHighlighted,
            "minecraft/textures/gui/sprites/widget/tab_highlighted.png",
            2.0,
        ),
        (
            SpriteId::TabSelected,
            "minecraft/textures/gui/sprites/widget/tab_selected.png",
            2.0,
        ),
        (
            SpriteId::TabSelectedHighlighted,
            "minecraft/textures/gui/sprites/widget/tab_selected_highlighted.png",
            2.0,
        ),
        (
            SpriteId::WorldJoin,
            "minecraft/textures/gui/sprites/world_list/join.png",
            0.0,
        ),
        (
            SpriteId::WorldJoinHighlighted,
            "minecraft/textures/gui/sprites/world_list/join_highlighted.png",
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
        (
            SpriteId::ChatModified,
            "minecraft/textures/gui/sprites/icon/chat_modified.png",
            0.0,
        ),
        (
            SpriteId::ToastAdvancement,
            "minecraft/textures/gui/sprites/toast/advancement.png",
            0.0,
        ),
        (
            SpriteId::ToastRecipe,
            "minecraft/textures/gui/sprites/toast/recipe.png",
            0.0,
        ),
        (
            SpriteId::ToastTutorial,
            "minecraft/textures/gui/sprites/toast/tutorial.png",
            3.0,
        ),
        (
            SpriteId::ToastMovementKeys,
            "minecraft/textures/gui/sprites/toast/movement_keys.png",
            0.0,
        ),
        (
            SpriteId::ToastMouse,
            "minecraft/textures/gui/sprites/toast/mouse.png",
            0.0,
        ),
        (
            SpriteId::ToastTree,
            "minecraft/textures/gui/sprites/toast/tree.png",
            0.0,
        ),
        (
            SpriteId::ToastRecipeBook,
            "minecraft/textures/gui/sprites/toast/recipe_book.png",
            0.0,
        ),
        (
            SpriteId::ToastWoodenPlanks,
            "minecraft/textures/gui/sprites/toast/wooden_planks.png",
            0.0,
        ),
        (
            SpriteId::ToastSocialInteractions,
            "minecraft/textures/gui/sprites/toast/social_interactions.png",
            0.0,
        ),
        (
            SpriteId::ToastRightClick,
            "minecraft/textures/gui/sprites/toast/right_click.png",
            0.0,
        ),
    ];

    let effect_icons = crate::mob_effect::MOB_EFFECTS
        .iter()
        .enumerate()
        .map(|(i, info)| {
            (
                SpriteId::MobEffect(i as u8),
                format!("minecraft/textures/mob_effect/{}.png", info.name),
                0.0,
            )
        });
    let mut boss_bar_sprites: Vec<(SpriteId, String, f32)> = Vec::new();
    let mut boss_bar_sprite = |id: SpriteId, name: &str, kind: &str| {
        boss_bar_sprites.push((
            id,
            format!("minecraft/textures/gui/sprites/boss_bar/{name}_{kind}.png"),
            0.0,
        ));
    };
    let colors = ["pink", "blue", "red", "green", "yellow", "purple", "white"];
    for (i, name) in colors.iter().enumerate() {
        boss_bar_sprite(SpriteId::BossBarBackground(i as u8), name, "background");
        boss_bar_sprite(SpriteId::BossBarProgress(i as u8), name, "progress");
    }
    let notches = ["notched_6", "notched_10", "notched_12", "notched_20"];
    for (i, name) in notches.iter().enumerate() {
        boss_bar_sprite(
            SpriteId::BossBarNotchedBackground(i as u8),
            name,
            "background",
        );
        boss_bar_sprite(SpriteId::BossBarNotchedProgress(i as u8), name, "progress");
    }
    let mut images: Vec<(SpriteId, Vec<u8>, u32, u32, f32)> = Vec::new();
    for (id, asset_key, border) in sprites
        .iter()
        .map(|&(id, key, border)| (id, key.to_string(), border))
        .chain(effect_icons)
        .chain(boss_bar_sprites)
    {
        let path = resolve_asset_path(jar_assets_dir, asset_index, &asset_key);
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

    // Locator bar background: pre-tile the 12x5 nine-slice (borders L/R 5,
    // 2px center repeated) to its fixed 182x5 draw size.
    let locator_bg_path = resolve_asset_path(
        jar_assets_dir,
        asset_index,
        "minecraft/textures/gui/sprites/hud/locator_bar_background.png",
    );
    match crate::assets::load_image(&locator_bg_path) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            if rgba.width() == 12 && rgba.height() == 5 {
                let src = rgba.as_raw();
                let mut out = vec![0u8; 182 * 5 * 4];
                for y in 0..5usize {
                    let row = |x: usize| (y * 12 + x) * 4;
                    let dst_row = y * 182 * 4;
                    out[dst_row..dst_row + 5 * 4].copy_from_slice(&src[row(0)..row(5)]);
                    for rep in 0..86usize {
                        let dst = dst_row + (5 + rep * 2) * 4;
                        out[dst..dst + 2 * 4].copy_from_slice(&src[row(5)..row(7)]);
                    }
                    let dst = dst_row + 177 * 4;
                    out[dst..dst + 5 * 4].copy_from_slice(&src[row(7)..row(12)]);
                }
                images.push((SpriteId::LocatorBarBackground, out, 182, 5, 0.0));
            } else {
                tracing::warn!(
                    "Unexpected locator bar background size: {}x{}",
                    rgba.width(),
                    rgba.height()
                );
                let (w, h) = (rgba.width(), rgba.height());
                images.push((SpriteId::LocatorBarBackground, rgba.into_raw(), w, h, 0.0));
            }
        }
        Err(e) => {
            tracing::warn!("Failed to load locator bar background: {e}");
            images.push((
                SpriteId::LocatorBarBackground,
                vec![255, 0, 255, 255],
                1,
                1,
                0.0,
            ));
        }
    }

    // Locator arrows: each 7x10 strip holds two 7x5 animation frames.
    for (frame0, frame1, asset_key) in [
        (
            SpriteId::LocatorArrowUp0,
            SpriteId::LocatorArrowUp1,
            "minecraft/textures/gui/sprites/hud/locator_bar_arrow_up.png",
        ),
        (
            SpriteId::LocatorArrowDown0,
            SpriteId::LocatorArrowDown1,
            "minecraft/textures/gui/sprites/hud/locator_bar_arrow_down.png",
        ),
    ] {
        let path = resolve_asset_path(jar_assets_dir, asset_index, asset_key);
        let frames = match crate::assets::load_image(&path) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                if rgba.width() == 7 && rgba.height() == 10 {
                    let src = rgba.into_raw();
                    let frame_bytes = 7 * 5 * 4;
                    Some((src[..frame_bytes].to_vec(), src[frame_bytes..].to_vec()))
                } else {
                    tracing::warn!(
                        "Unexpected locator arrow size {}x{} for {asset_key}",
                        rgba.width(),
                        rgba.height()
                    );
                    None
                }
            }
            Err(e) => {
                tracing::warn!("Failed to load locator arrow {asset_key}: {e}");
                None
            }
        };
        match frames {
            Some((f0, f1)) => {
                images.push((frame0, f0, 7, 5, 0.0));
                images.push((frame1, f1, 7, 5, 0.0));
            }
            None => {
                for id in [frame0, frame1] {
                    images.push((id, vec![255, 0, 255, 255], 1, 1, 0.0));
                }
            }
        }
    }

    // Placeholder for unknown waypoint styles (vanilla shows missingno).
    images.push((
        SpriteId::LocatorDotMissing,
        vec![255, 0, 255, 255],
        1,
        1,
        0.0,
    ));

    // Container backgrounds live in a 256x256 atlas; crop out the used region
    // starting at texture row `src_y`.
    for (id, path, src_y, max_w, max_h) in [
        (
            SpriteId::InventoryBackground,
            "minecraft/textures/gui/container/inventory.png",
            0,
            INV_TEX_W,
            INV_TEX_H,
        ),
        (
            SpriteId::CraftingTableBackground,
            "minecraft/textures/gui/container/crafting_table.png",
            0,
            INV_TEX_W,
            INV_TEX_H,
        ),
        (
            SpriteId::FurnaceBackground,
            "minecraft/textures/gui/container/furnace.png",
            0,
            INV_TEX_W,
            INV_TEX_H,
        ),
        (
            SpriteId::BlastFurnaceBackground,
            "minecraft/textures/gui/container/blast_furnace.png",
            0,
            INV_TEX_W,
            INV_TEX_H,
        ),
        (
            SpriteId::SmokerBackground,
            "minecraft/textures/gui/container/smoker.png",
            0,
            INV_TEX_W,
            INV_TEX_H,
        ),
        (
            SpriteId::Generic54Top,
            "minecraft/textures/gui/container/generic_54.png",
            0,
            INV_TEX_W,
            125,
        ),
        (
            SpriteId::Generic54Bottom,
            "minecraft/textures/gui/container/generic_54.png",
            126,
            INV_TEX_W,
            96,
        ),
        (
            SpriteId::ShulkerBoxBackground,
            "minecraft/textures/gui/container/shulker_box.png",
            0,
            INV_TEX_W,
            167,
        ),
        (
            SpriteId::AnvilBackground,
            "minecraft/textures/gui/container/anvil.png",
            0,
            INV_TEX_W,
            INV_TEX_H,
        ),
        (
            SpriteId::EnchantingTableBackground,
            "minecraft/textures/gui/container/enchanting_table.png",
            0,
            INV_TEX_W,
            INV_TEX_H,
        ),
        (
            SpriteId::CreativeItemsBackground,
            "minecraft/textures/gui/container/creative_inventory/tab_items.png",
            0,
            195,
            136,
        ),
        (
            SpriteId::CreativeSearchBackground,
            "minecraft/textures/gui/container/creative_inventory/tab_item_search.png",
            0,
            195,
            136,
        ),
        (
            SpriteId::CreativeInventoryBackground,
            "minecraft/textures/gui/container/creative_inventory/tab_inventory.png",
            0,
            195,
            136,
        ),
        // Frame 0 of the animated portal strip, stretched full-screen by the
        // portal camera overlay. TODO: animate.
        (
            SpriteId::NetherPortal,
            "minecraft/textures/block/nether_portal.png",
            0,
            16,
            16,
        ),
    ] {
        let path = resolve_asset_path(jar_assets_dir, asset_index, path);
        match crate::assets::load_image(&path) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                let full_w = rgba.width();
                let crop_w = max_w.min(full_w);
                let crop_h = max_h.min(rgba.height().saturating_sub(src_y));
                let mut cropped = vec![0u8; (crop_w * crop_h * 4) as usize];
                for y in 0..crop_h {
                    let src_off = ((src_y + y) * full_w * 4) as usize;
                    let dst_off = (y * crop_w * 4) as usize;
                    let row_bytes = (crop_w * 4) as usize;
                    cropped[dst_off..dst_off + row_bytes]
                        .copy_from_slice(&rgba.as_raw()[src_off..src_off + row_bytes]);
                }
                images.push((id, cropped, crop_w, crop_h, 0.0));
            }
            Err(e) => {
                tracing::warn!("Failed to load sprite {id:?}: {e}");
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

    // `LogoRenderer` blits only the top 44/64 of minecraft.png and 14/16 of
    // edition.png. The files are supersampled and packs may use any size, so
    // crop by fraction; a row-major prefix is contiguous, so it's a truncate.
    for (id, asset_key, logical_h, used_h) in [
        (
            SpriteId::MinecraftLogo,
            "minecraft/textures/gui/title/minecraft.png",
            64u32,
            44u32,
        ),
        (
            SpriteId::MinecraftEdition,
            "minecraft/textures/gui/title/edition.png",
            16,
            14,
        ),
    ] {
        let path = resolve_asset_path(jar_assets_dir, asset_index, asset_key);
        match crate::assets::load_image(&path) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                let (w, h) = (rgba.width(), rgba.height());
                let used = (h * used_h).div_ceil(logical_h).min(h);
                let mut raw = rgba.into_raw();
                raw.truncate((w * used * 4) as usize);
                images.push((id, raw, w, used, 0.0));
            }
            Err(e) => {
                tracing::warn!("Failed to load title sprite {asset_key}: {e}");
                images.push((id, vec![255, 0, 255, 255], 1, 1, 0.0));
            }
        }
    }

    // The credits roll draws the mark at 44 GUI units, so 44 * gui_scale px:
    // 176 at 1080p, 264 at 1440p, 396 at 4K on the default auto scale. The
    // atlas sampler is NEAREST, so 256 is the size that straddles that range
    // with the least resampling either way.
    const POMME_LOGO_SIZE: u32 = 256;
    match image::load_from_memory(crate::assets::POMME_ICON_PNG) {
        Ok(img) => {
            images.push((
                SpriteId::PommeLogo,
                downscale_straight_alpha(&img.to_rgba8(), POMME_LOGO_SIZE).into_raw(),
                POMME_LOGO_SIZE,
                POMME_LOGO_SIZE,
                0.0,
            ));
        }
        Err(e) => tracing::warn!("Failed to load pomme logo: {e}"),
    }

    let sizes: Vec<(u32, u32)> = images.iter().map(|sprite| (sprite.2, sprite.3)).collect();
    let (atlas_size, placements, all_fit) =
        packing::fit_atlas_size(&sizes, SPRITE_PAD, MAX_SPRITE_ATLAS_SIZE);

    if !all_fit {
        let dropped: Vec<SpriteId> = images
            .iter()
            .zip(&placements)
            .filter(|(_, placement)| placement.is_none())
            .map(|(sprite, _)| sprite.0)
            .collect();
        // Vanilla throws `StitcherException` here and turns it into a crash
        // report. A dropped sprite renders as nothing at all, which is far
        // harder to notice, so shout in release and hard-fail in dev.
        tracing::error!(
            "Sprite atlas overflowed the {MAX_SPRITE_ATLAS_SIZE}px cap; {} sprites will not \
             render: {dropped:?}",
            dropped.len()
        );
        debug_assert!(all_fit, "sprite atlas dropped {dropped:?}");
    }

    let mut pixels = vec![0u8; atlas_size as usize * atlas_size as usize * 4];
    let mut regions = HashMap::new();
    let inv = 1.0 / atlas_size as f32;

    for ((id, data, w, h, border), placement) in images.iter().zip(&placements) {
        let Some((x, y)) = *placement else { continue };

        blit_image(&mut pixels, atlas_size, data, *w, x, y, *w, *h);

        regions.insert(
            *id,
            SpriteRegion {
                u0: x as f32 * inv,
                v0: y as f32 * inv,
                u1: (x + w) as f32 * inv,
                v1: (y + h) as f32 * inv,
                src_w: *w as f32,
                src_h: *h as f32,
                nine_slice_border: *border,
            },
        );
    }

    tracing::debug!(
        "Sprite atlas: {atlas_size}x{atlas_size} for {} sprites",
        regions.len()
    );

    // `upload_image` copies `atlas_size * atlas_size * 4` bytes regardless of
    // the staging buffer's length, so `pixels` has to stay sized off the packed
    // `atlas_size` above; feeding these a size of their own would read past it.
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

fn load_overlay_rgba(
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    asset_key: &str,
) -> (Vec<u8>, u32, u32) {
    let path = resolve_asset_path(jar_assets_dir, asset_index, asset_key);
    match crate::assets::load_image(&path) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let (w, h) = (rgba.width(), rgba.height());
            (rgba.into_raw(), w, h)
        }
        Err(e) => {
            tracing::warn!("Failed to load overlay texture {asset_key}: {e}");
            (vec![255, 0, 255, 255], 1, 1)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn upload_and_free_staging(
    device: &vk::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    allocator: &Arc<Mutex<Allocator>>,
    pixels: &[u8],
    image: vk::Image,
    w: u32,
    h: u32,
    name: &str,
) {
    let (staging, staging_alloc) = util::create_staging_buffer(device, allocator, pixels, name);
    util::upload_image(device, queue, command_pool, staging, image, w, h);
    // upload_image waits for the copy, so the staging buffer can go right away.
    device.destroy_buffer(staging, None);
    allocator.lock().unwrap().free(staging_alloc).ok();
}

/// Compose the full-screen camera overlay textures (vignette + pumpkin blur)
/// side by side into one linear-sampled image. Returns the image and each
/// half's UV rect, inset half a texel so filtering never crosses the seam.
fn build_camera_overlay_texture(
    device: &vk::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    allocator: &Arc<Mutex<Allocator>>,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
) -> (vk::Image, vk::ImageView, Allocation, [f32; 4], [f32; 4]) {
    let (vig, vw, vh) = load_overlay_rgba(
        jar_assets_dir,
        asset_index,
        "minecraft/textures/misc/vignette.png",
    );
    let (pump, pw, ph) = load_overlay_rgba(
        jar_assets_dir,
        asset_index,
        "minecraft/textures/misc/pumpkinblur.png",
    );

    let w = vw + pw;
    let h = vh.max(ph);
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    blit_image(&mut pixels, w, &vig, vw, 0, 0, vw, vh);
    blit_image(&mut pixels, w, &pump, pw, vw, 0, pw, ph);

    let (image, view, allocation) =
        util::create_gpu_image(device, allocator, w, h, "camera_overlay");
    upload_and_free_staging(
        device,
        queue,
        command_pool,
        allocator,
        &pixels,
        image,
        w,
        h,
        "camera_overlay_staging",
    );

    let (iw, ih) = (w as f32, h as f32);
    let vignette_uv = [
        0.5 / iw,
        0.5 / ih,
        (vw as f32 - 0.5) / iw,
        (vh as f32 - 0.5) / ih,
    ];
    let pumpkin_uv = [
        (vw as f32 + 0.5) / iw,
        0.5 / ih,
        (iw - 0.5) / iw,
        (ph as f32 - 0.5) / ih,
    ];
    (image, view, allocation, vignette_uv, pumpkin_uv)
}

#[allow(clippy::too_many_arguments)]
fn load_single_texture(
    device: &vk::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    allocator: &Arc<Mutex<Allocator>>,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    asset_key: &str,
    name: &str,
) -> (vk::Image, vk::ImageView, Allocation) {
    let (pixels, w, h) = load_overlay_rgba(jar_assets_dir, asset_index, asset_key);
    let (image, view, allocation) = util::create_gpu_image(device, allocator, w, h, name);
    upload_and_free_staging(
        device,
        queue,
        command_pool,
        allocator,
        &pixels,
        image,
        w,
        h,
        name,
    );
    (image, view, allocation)
}

const MC_FONT_COLOR_FORMAT: vk::Format = vk::Format::R8G8B8A8Srgb;

struct FontTextureUpload<'a> {
    pixels: &'a [u8],
    extent: util::ImageArrayExtent,
    format: vk::Format,
    name: &'static str,
}

/// Uploads the gray (R8) and colored glyph atlases, with 1x1 placeholders when
/// there are no fonts or no colored glyphs.
fn create_font_textures(
    device: &vk::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    allocator: &Arc<Mutex<Allocator>>,
    pixels: Option<&GlyphAtlasPixels>,
) -> Result<(TextureResources, TextureResources), String> {
    let extent = |layers| util::ImageArrayExtent {
        width: GLYPH_ATLAS_SIZE,
        height: GLYPH_ATLAS_SIZE,
        layers,
    };
    let placeholder = util::ImageArrayExtent {
        width: 1,
        height: 1,
        layers: 1,
    };
    let gray = match pixels {
        Some(p) => (p.gray.as_slice(), extent(p.gray_layers)),
        None => (&[0u8][..], placeholder),
    };
    let color = match pixels {
        Some(p) if p.color_layers > 0 => (p.color.as_slice(), extent(p.color_layers)),
        _ => (&[0u8; 4][..], placeholder),
    };
    let upload = |(pixels, extent), format, name| {
        create_font_texture(
            device,
            queue,
            command_pool,
            allocator,
            FontTextureUpload {
                pixels,
                extent,
                format,
                name,
            },
        )
    };
    let mut gray = upload(gray, vk::Format::R8Unorm, "mc_font_atlas")?;
    match upload(color, MC_FONT_COLOR_FORMAT, "mc_font_color_atlas") {
        Ok(color) => Ok((gray, color)),
        Err(error) => {
            destroy_texture_resources(device, &mut util::lock_allocator(allocator), &mut gray);
            Err(error)
        }
    }
}

fn create_font_texture(
    device: &vk::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    allocator: &Arc<Mutex<Allocator>>,
    upload: FontTextureUpload<'_>,
) -> Result<TextureResources, String> {
    let sampler = util::try_create_nearest_sampler(device, 1)?;
    let (image, view, image_alloc) = match util::create_gpu_image_array_with_format(
        device,
        allocator,
        upload.extent,
        upload.format,
        upload.name,
    ) {
        Ok(resources) => resources,
        Err(error) => {
            device.destroy_sampler(sampler, None);
            return Err(error);
        }
    };
    let mut texture = TextureResources {
        sampler,
        image,
        view,
        image_alloc: Some(image_alloc),
        staging_buffer: vk::Buffer::null(),
        staging_alloc: None,
    };
    let bytes_per_pixel = if upload.format == vk::Format::R8Unorm {
        1
    } else {
        4
    };
    let uploaded = util::try_create_mapped_buffer(
        device,
        allocator,
        upload.pixels,
        vk::BufferUsageFlags::TransferSrc,
        upload.name,
    )
    .and_then(|(staging, staging_alloc)| {
        let result = util::upload_image_array(
            device,
            queue,
            command_pool,
            staging,
            image,
            upload.extent,
            bytes_per_pixel,
        );
        device.destroy_buffer(staging, None);
        let _ = util::lock_allocator(allocator).free(staging_alloc);
        result
    });
    if let Err(error) = uploaded {
        destroy_texture_resources(device, &mut util::lock_allocator(allocator), &mut texture);
        return Err(error);
    }
    Ok(texture)
}

struct TextureResources {
    sampler: vk::Sampler,
    image: vk::Image,
    view: vk::ImageView,
    image_alloc: Option<Allocation>,
    staging_buffer: vk::Buffer,
    staging_alloc: Option<Allocation>,
}

impl TextureResources {
    fn image_info(&self) -> vk::DescriptorImageInfo {
        vk::DescriptorImageInfo {
            sampler: self.sampler,
            image_view: self.view,
            image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
        }
    }
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
    tint: [f32; 4],
) {
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
            tint,
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
            tint,
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

fn push_fullscreen_quad(
    verts: &mut Vec<Vertex>,
    w: f32,
    h: f32,
    [u0, v0, u1, v1]: [f32; 4],
    color: [f32; 4],
    mode: f32,
) {
    push_quad(
        verts,
        0.0,
        0.0,
        w,
        h,
        u0,
        v0,
        u1,
        v1,
        color,
        mode,
        [0.0, 0.0],
        0.0,
    );
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

    // Snap to integer pixels so NEAREST sampling stays inside the region.
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

/// Vanilla `Font`'s persistent `RandomSource` (LegacyRandomSource), drawn once
/// per obfuscated glyph.
#[derive(Clone, Copy)]
struct ObfuscationRng {
    seed: u64,
}

impl ObfuscationRng {
    const MASK: u64 = (1u64 << 48) - 1;
    const MULTIPLIER: u64 = 25_214_903_917;
    const INCREMENT: u64 = 11;

    fn new(seed: u64) -> Self {
        Self {
            seed: (seed ^ 0x5DEECE66D) & Self::MASK,
        }
    }

    fn next_bits(&mut self, bits: u32) -> u32 {
        self.seed = self
            .seed
            .wrapping_mul(Self::MULTIPLIER)
            .wrapping_add(Self::INCREMENT)
            & Self::MASK;
        (self.seed >> (48 - bits)) as u32
    }

    fn next_int(&mut self, bound: usize) -> usize {
        debug_assert!(bound > 0 && bound <= i32::MAX as usize);
        let bound = bound as u32;
        if bound.is_power_of_two() {
            return (((bound as u64) * (self.next_bits(31) as u64)) >> 31) as usize;
        }
        loop {
            let bits = self.next_bits(31);
            let value = bits % bound;
            if bits.wrapping_sub(value).wrapping_add(bound - 1) < (1 << 31) {
                return value as usize;
            }
        }
    }
}

#[derive(Clone, Copy)]
struct McTextSources<'a> {
    gm: &'a GlyphMap,
    dynamic_regions: &'a std::collections::HashMap<String, [f32; 4]>,
    sprite_atlas: &'a SpriteAtlas,
}

/// The side of the atlas cell an inline object's tile is packed into.
pub const ATLAS_CELL: u32 = 64;

/// Whether the object has a glyph at all: a player head always does, an atlas
/// sprite only in one of the atlases the client stitches
/// (`AtlasManager.KNOWN_ATLASES`).
fn object_has_glyph(object: &InlineObject) -> bool {
    match object {
        InlineObject::Player { .. } => true,
        InlineObject::AtlasSprite { atlas, .. } => crate::ui::object_glyph::is_known_atlas(atlas),
    }
}

#[derive(Clone, Copy)]
struct McTextDraw {
    x: f32,
    y: f32,
    scale: f32,
    drop_shadow: bool,
}

/// Advance of an inline object glyph in font pixels (vanilla
/// `PlainTextRenderable` is 8 wide).
fn inline_object_advance(bold: bool) -> f32 {
    8.0 + if bold { 1.0 } else { 0.0 }
}

/// Summed advance of `text` in font pixels.
fn run_advance(
    gm: &GlyphMap,
    text: &str,
    font: Option<&str>,
    bold: bool,
    inline_objects: bool,
) -> f32 {
    text.chars()
        .map(|ch| {
            if ch == '\u{fffc}' && inline_objects {
                inline_object_advance(bold)
            } else {
                let glyph = gm.glyph(ch, font);
                glyph.advance + if bold { glyph.bold_offset } else { 0.0 }
            }
        })
        .sum()
}

/// An underline or strikethrough quad. Adjacent glyphs' effects that abut
/// with the same colors are merged into one quad.
struct McEffect {
    rect: [f32; 4],
    color: [f32; 4],
    shadow_color: Option<[f32; 4]>,
}

fn add_mc_effect(
    effects: &mut Vec<McEffect>,
    rect: [f32; 4],
    color: [f32; 4],
    shadow_color: Option<[f32; 4]>,
) {
    if let Some(last) = effects.last_mut()
        && last.rect[2] == rect[0]
        && last.rect[1] == rect[1]
        && last.color == color
        && last.shadow_color == shadow_color
    {
        last.rect[2] = rect[2];
        return;
    }
    effects.push(McEffect {
        rect,
        color,
        shadow_color,
    });
}

/// A glyph's horizontal advance in font-texture pixels.
fn wrap_plain_text<F>(text: &str, max_width: f32, mut width: F) -> Vec<String>
where
    F: FnMut(&str) -> f32,
{
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if !current.is_empty() && width(&candidate) > max_width {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
        } else {
            current = candidate;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn push_mc_text(
    verts: &mut Vec<Vertex>,
    sources: McTextSources<'_>,
    spans: &[TextSpan],
    draw: McTextDraw,
    obfuscation_rng: &mut ObfuscationRng,
    drawn_objects: &mut std::collections::HashMap<String, InlineObject>,
) {
    let McTextDraw {
        x,
        y,
        scale,
        drop_shadow,
    } = draw;
    let McTextSources {
        gm,
        dynamic_regions,
        sprite_atlas,
    } = sources;
    let px_scale = scale / gm.cell_h as f32;

    let mut cx = x;
    let mut cy = y;
    let mut line = 0u32;
    // Vanilla draws every glyph before the effects.
    let mut strikethroughs = Vec::new();
    let mut underlines = Vec::new();

    'spans: for span in spans {
        let mut span_position = 0u64;
        let font = span.font.as_deref();
        // Vanilla `PreparedTextBuilder.getShadowColor`: an explicit style
        // shadow always draws (alpha scaled by the text's); the default 25%
        // shadow only with drop shadow.
        let shadow_color = match span.shadow_color {
            Some(mut explicit) => {
                explicit[3] *= span.color[3];
                Some(explicit)
            }
            None => drop_shadow.then(|| {
                [
                    span.color[0] * 0.25,
                    span.color[1] * 0.25,
                    span.color[2] * 0.25,
                    span.color[3],
                ]
            }),
        };

        for ch in span.text.chars() {
            if ch == '\n' {
                cx = x;
                line += 1;
                span_position = 0;
                cy = y + line as f32 * (scale + 2.0 * px_scale);
                if line >= 2 {
                    break 'spans;
                }
                continue;
            }

            let effect_x0 = if span_position == 0 {
                cx - px_scale
            } else {
                cx
            };
            span_position = span_position.wrapping_add(1);
            let advance = if ch == '\u{fffc}'
                && let Some(object) = span.inline_object.as_ref()
                && object_has_glyph(object)
            {
                let glyph_w = 8.0 * px_scale;
                let sx = cx.round();
                let sy = (cy - px_scale).round();
                let key = object.atlas_key();
                let fallback = match object {
                    InlineObject::Player { .. } => SpriteId::SteveHead,
                    InlineObject::AtlasSprite { .. } => SpriteId::UnknownServer,
                };
                let mut draw_object = |dx: f32, color: [f32; 4]| {
                    push_atlas_image(
                        verts,
                        dynamic_regions,
                        sprite_atlas,
                        &key,
                        fallback,
                        sx + dx,
                        sy + dx,
                        glyph_w,
                        color,
                    );
                };
                if let Some(shadow_color) = shadow_color {
                    draw_object(px_scale, shadow_color);
                }
                draw_object(0.0, span.color);
                drawn_objects.entry(key).or_insert_with(|| object.clone());
                inline_object_advance(span.bold) * px_scale
            } else {
                let gi = if ch == '\u{fffc}' && span.inline_object.is_some() {
                    // `FontManager.getSpriteFont`: an atlas with no provider
                    // renders the missing-font glyph.
                    gm.missing()
                } else if span.obfuscated && ch != ' ' {
                    let width = gm.glyph(ch, font).advance.ceil() as i32;
                    gm.random_glyph(width, font, |len| obfuscation_rng.next_int(len))
                } else {
                    gm.glyph(ch, font)
                };
                push_mc_glyph_quads(verts, gi, [cx, cy], px_scale, span, shadow_color);
                (gi.advance + if span.bold { gi.bold_offset } else { 0.0 }) * px_scale
            };

            if span.strikethrough {
                let rect = [
                    effect_x0,
                    cy + 3.5 * px_scale,
                    cx + advance,
                    cy + 4.5 * px_scale,
                ];
                add_mc_effect(&mut strikethroughs, rect, span.color, shadow_color);
            }
            if span.underline {
                let rect = [
                    effect_x0,
                    cy + 8.0 * px_scale,
                    cx + advance,
                    cy + 9.0 * px_scale,
                ];
                add_mc_effect(&mut underlines, rect, span.color, shadow_color);
            }
            cx += advance;
        }
    }

    for effect in strikethroughs.iter().chain(&underlines) {
        push_mc_effect(verts, effect, px_scale);
    }
}

/// Vanilla `BakedSheetGlyph.renderChar`: the shadow copy, then the glyph, each
/// doubled at the bold offset.
fn push_mc_glyph_quads(
    verts: &mut Vec<Vertex>,
    gi: &GlyphInfo,
    origin: [f32; 2],
    px_scale: f32,
    span: &TextSpan,
    shadow_color: Option<[f32; 4]>,
) {
    if gi.pixel_w == 0 || gi.pixel_h == 0 {
        return;
    }
    let inv = 1.0 / GLYPH_ATLAS_SIZE as f32;
    // `BakedSheetGlyph.render` shears each edge by 1 - 0.25 * its y.
    let shear = if span.italic {
        [
            (1.0 - 0.25 * gi.top) * px_scale,
            (1.0 - 0.25 * (gi.top + gi.draw_h)) * px_scale,
        ]
    } else {
        [0.0, 0.0]
    };
    let quad = GlyphQuad {
        w: (gi.draw_w * px_scale).round(),
        h: (gi.draw_h * px_scale).round(),
        layer: gi.atlas_layer,
        uv: [
            gi.atlas_x as f32 * inv,
            gi.atlas_y as f32 * inv,
            (gi.atlas_x + gi.pixel_w) as f32 * inv,
            (gi.atlas_y + gi.pixel_h) as f32 * inv,
        ],
        colored: gi.colored,
        shear,
    };
    let sx = (origin[0] + gi.left * px_scale).round();
    let sy = (origin[1] + gi.top * px_scale).round();
    let bold = span.bold.then_some(gi.bold_offset * px_scale);
    let mut draw = |offset: f32, color: [f32; 4]| {
        push_mc_glyph(verts, &quad, sx + offset, sy + offset, color);
        if let Some(bold) = bold {
            push_mc_glyph(verts, &quad, sx + offset + bold, sy + offset, color);
        }
    };
    if let Some(shadow_color) = shadow_color {
        draw(gi.shadow_offset * px_scale, shadow_color);
    }
    draw(0.0, span.color);
}

fn push_mc_effect(verts: &mut Vec<Vertex>, effect: &McEffect, shadow_offset: f32) {
    let [x0, y0, x1, y1] = effect.rect;
    let (w, h) = (x1 - x0, y1 - y0);
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    if let Some(shadow_color) = effect.shadow_color {
        push_mc_fill(
            verts,
            x0 + shadow_offset,
            y0 + shadow_offset,
            w,
            h,
            shadow_color,
        );
    }
    push_mc_fill(verts, x0, y0, w, h, effect.color);
}

/// A hard-edged effect quad in the premultiplied mode, linearized here as the
/// shader does for glyphs.
fn push_mc_fill(verts: &mut Vec<Vertex>, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
    let a = color[3];
    let linear = |c: f32| c.powf(2.2) * a;
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
        [linear(color[0]), linear(color[1]), linear(color[2]), a],
        10.0,
        [0.0, 0.0],
        0.0,
    );
}

#[derive(Clone, Copy)]
struct GlyphQuad {
    w: f32,
    h: f32,
    layer: u32,
    uv: [f32; 4],
    colored: bool,
    /// Italic x offset of the top and bottom edges.
    shear: [f32; 2],
}

fn push_mc_glyph(verts: &mut Vec<Vertex>, quad: &GlyphQuad, x: f32, y: f32, color: [f32; 4]) {
    let GlyphQuad {
        w,
        h,
        layer,
        uv: [u0, v0, u1, v1],
        colored,
        shear: [top, bottom],
    } = *quad;
    let positions = [
        [x + top, y],
        [x + w + top, y],
        [x + bottom, y + h],
        [x + w + top, y],
        [x + w + bottom, y + h],
        [x + bottom, y + h],
    ];
    let uvs = [[u0, v0], [u1, v0], [u0, v1], [u1, v0], [u1, v1], [u0, v1]];
    for i in 0..6 {
        verts.push(Vertex {
            pos: positions[i],
            uv: uvs[i],
            color,
            mode: if colored { 4.25 } else { 4.0 },
            rect_size: [layer as f32, 0.0],
            corner_radius: 0.0,
        });
    }
}

fn create_pipeline(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
    invert: bool,
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

    // Normal path is premultiplied alpha; invert is vanilla `BlendFunction.INVERT`
    // (transparent texels output rgb 0 / alpha 0, so ONE_MINUS_SRC_COLOR keeps
    // dst).
    let (src_color, dst_color, dst_alpha) = if invert {
        (
            vk::BlendFactor::OneMinusDstColor,
            vk::BlendFactor::OneMinusSrcColor,
            vk::BlendFactor::Zero,
        )
    } else {
        (
            vk::BlendFactor::One,
            vk::BlendFactor::OneMinusSrcAlpha,
            vk::BlendFactor::OneMinusSrcAlpha,
        )
    };
    let blend_attachment = [vk::PipelineColorBlendAttachmentState {
        blend_enable: vk::TRUE,
        src_color_blend_factor: src_color,
        dst_color_blend_factor: dst_color,
        color_blend_op: vk::BlendOp::Add,
        src_alpha_blend_factor: vk::BlendFactor::One,
        dst_alpha_blend_factor: dst_alpha,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abutting_effects_merge_into_one_quad() {
        let mut effects = Vec::new();
        add_mc_effect(&mut effects, [0.0, 8.0, 6.0, 9.0], [1.0; 4], None);
        add_mc_effect(&mut effects, [6.0, 8.0, 12.0, 9.0], [1.0; 4], None);
        add_mc_effect(&mut effects, [12.0, 8.0, 18.0, 9.0], [0.5; 4], None);
        assert_eq!(effects.len(), 2);
        assert_eq!(effects[0].rect, [0.0, 8.0, 12.0, 9.0]);
    }

    #[test]
    fn italic_shear_matches_baked_sheet_glyph() {
        // An ASCII glyph spans y 0..8: vanilla shears the top edge by +1 and
        // the bottom by -1.
        let glyph = GlyphInfo {
            atlas_layer: 0,
            colored: false,
            atlas_x: 0,
            atlas_y: 0,
            pixel_w: 5,
            pixel_h: 8,
            draw_w: 5.0,
            draw_h: 8.0,
            left: 0.0,
            top: 0.0,
            advance: 6.0,
            bold_offset: 1.0,
            shadow_offset: 1.0,
        };
        let mut span = TextSpan::new("A".into(), [1.0; 4]);
        span.italic = true;
        let mut verts = Vec::new();
        push_mc_glyph_quads(&mut verts, &glyph, [0.0, 0.0], 1.0, &span, None);
        assert_eq!(verts.len(), 6);
        assert_eq!(verts[0].pos, [1.0, 0.0]);
        assert_eq!(verts[2].pos, [-1.0, 8.0]);
    }

    #[test]
    fn obfuscation_rng_matches_java_random_bounded_sequence() {
        let mut rng = ObfuscationRng::new(0);
        let got: Vec<usize> = (0..10).map(|_| rng.next_int(1000)).collect();
        assert_eq!(got, [360, 948, 29, 447, 515, 53, 491, 761, 719, 854]);

        let mut rng = ObfuscationRng::new(0);
        let got: Vec<usize> = (0..10).map(|_| rng.next_int(16)).collect();
        assert_eq!(got, [11, 13, 3, 9, 10, 4, 8, 1, 9, 12]);
    }

    #[test]
    fn downscale_keeps_colour_out_of_the_transparent_border() {
        // Opaque white against the transparent black an anti-aliased sprite
        // sits on; filtering straight alpha would leave the edge texels grey.
        let mut src = image::RgbaImage::new(16, 16);
        for (x, _, px) in src.enumerate_pixels_mut() {
            *px = if x < 8 {
                image::Rgba([255, 255, 255, 255])
            } else {
                image::Rgba([0, 0, 0, 0])
            };
        }

        let out = downscale_straight_alpha(&src, 4);
        assert!(out.pixels().any(|px| px[3] > 0 && px[3] < 255));
        for px in out.pixels().filter(|px| px[3] > 0) {
            assert_eq!([px[0], px[1], px[2]], [255, 255, 255], "alpha {}", px[3]);
        }
    }
}
