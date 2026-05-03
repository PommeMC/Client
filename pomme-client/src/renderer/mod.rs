pub mod camera;
pub mod chunk;
mod context;
pub mod entity_model;
pub mod pipelines;
pub(crate) mod shader;
mod swapchain;
pub(crate) mod util;

pub(crate) const MAX_FRAMES_IN_FLIGHT: usize = 3;

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use ash::vk;
use azalea_core::position::ChunkPos;
use thiserror::Error;
use winit::dpi::PhysicalSize;
use winit::window::Window;

use azalea_core::position::BlockPos;
use camera::{Camera, CameraUniform};
use chunk::atlas::TextureAtlas;
use chunk::buffer::ChunkBufferStore;
use chunk::mesher::{ChunkMeshData, MeshDispatcher};
use context::VulkanContext;
use pipelines::block_overlay::BlockOverlayPipeline;
use pipelines::blur::BlurPipeline;
use pipelines::chunk::ChunkPipeline;
use pipelines::entity_renderer::{EntityRenderInfo, EntityRenderer};
use pipelines::hand::HandPipeline;
use pipelines::menu_overlay::{MenuElement, MenuOverlayPipeline};
use pipelines::panorama::PanoramaPipeline;
use pipelines::skin_preview::SkinPreviewPipeline;
pub use pipelines::sky::{SkyPipeline, SkyState};
use swapchain::SwapchainState;

use crate::assets::AssetIndex;
use crate::window::input::InputState;
use crate::world::block::registry::BlockRegistry;

#[derive(Error, Debug)]
pub enum RendererError {
    #[error("failed to initialize GPU context: {0}")]
    Context(#[from] context::ContextError),

    #[error("vulkan error: {0}")]
    Vulkan(#[from] vk::Result),
}

enum RenderMode<'a> {
    World {
        overlay: Vec<MenuElement>,
        swing_progress: f32,
        destroy_info: Option<(BlockPos, u32)>,
        show_chunk_borders: bool,
        sky: SkyState,
        entities: &'a [EntityRenderInfo],
        item_entities: &'a [pipelines::item_entity::ItemRenderInfo],
    },
    MainMenu {
        scroll: f32,
        blur: f32,
        elements: Vec<MenuElement>,
        cursor: (f32, f32),
        show_skin: bool,
    },
}

#[derive(Default, Clone)]
pub struct RenderTimings {
    pub frame_ms: f32,
    pub fence_ms: f32,
    pub acquire_ms: f32,
    pub cull_ms: f32,
    pub draw_ms: f32,
    pub present_ms: f32,
}

pub struct Renderer {
    swapchain: SwapchainState,
    camera: Camera,
    registry: BlockRegistry,
    jar_assets_dir: std::path::PathBuf,
    asset_index: Option<AssetIndex>,
    atlas: TextureAtlas,
    chunk_pipeline: ChunkPipeline,
    hand_pipeline: HandPipeline,
    block_overlay_pipeline: BlockOverlayPipeline,
    sky_pipeline: SkyPipeline,
    panorama_pipeline: PanoramaPipeline,
    menu_pipeline: MenuOverlayPipeline,
    blur_pipeline: BlurPipeline,
    skin_preview: SkinPreviewPipeline,
    entity_renderer: EntityRenderer,
    chunk_border_pipeline: pipelines::chunk_borders::ChunkBorderPipeline,
    item_entity_pipeline: pipelines::item_entity::ItemEntityPipeline,
    chunk_buffers: ChunkBufferStore,
    swapchain_dirty: bool,
    width: u32,
    height: u32,
    pub last_timings: RenderTimings,
    ctx: VulkanContext,
}

impl Renderer {
    pub fn new(
        window: Arc<Window>,
        jar_assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
        game_dir: &Path,
    ) -> Result<Self, RendererError> {
        let size = window.inner_size();

        let registry_handle = {
            let jar_assets_dir = jar_assets_dir.to_path_buf();
            let asset_index = asset_index.clone();
            let game_dir = game_dir.to_path_buf();
            std::thread::spawn(move || {
                BlockRegistry::load(&jar_assets_dir, &asset_index, &game_dir, None)
            })
        };

        let ctx = VulkanContext::new(&window)?;

        let swapchain_state = SwapchainState::new(
            &ctx.device,
            &ctx.surface_loader,
            &ctx.swapchain_loader,
            ctx.physical_device,
            ctx.surface,
            size.width.max(1),
            size.height.max(1),
            ctx.graphics_family,
            ctx.present_family,
            &ctx.allocator,
            vk::SwapchainKHR::null(),
        )?;

        let mut menu_pipeline = MenuOverlayPipeline::new(
            &ctx.device,
            ctx.graphics_queue,
            ctx.command_pool,
            swapchain_state.render_pass,
            &ctx.allocator,
            jar_assets_dir,
            asset_index,
        );

        let sw = size.width.max(1) as f32;
        let sh = size.height.max(1) as f32;
        window.set_visible(true);

        let splash = |menu: &mut MenuOverlayPipeline, progress: f32, status: &str| {
            let _ = Self::render_splash(&ctx, &swapchain_state, menu, sw, sh, progress, status);
        };

        splash(&mut menu_pipeline, 0.0, "Loading block models...");

        let camera = Camera::new(swapchain_state.aspect_ratio());
        let registry = registry_handle
            .join()
            .expect("block registry thread panicked");

        splash(&mut menu_pipeline, 0.2, "Building texture atlas...");

        let texture_names: HashSet<&str> = registry.texture_names().collect();
        let atlas = TextureAtlas::build(
            &ctx.device,
            ctx.graphics_queue,
            ctx.command_pool,
            &ctx.allocator,
            jar_assets_dir,
            asset_index,
            &texture_names,
            None,
        )?;

        splash(&mut menu_pipeline, 0.5, "Creating pipelines...");

        let chunk_pipeline = ChunkPipeline::new(
            &ctx.device,
            swapchain_state.render_pass,
            &ctx.allocator,
            &atlas,
        );

        let hand_pipeline = HandPipeline::new(
            &ctx.device,
            ctx.graphics_queue,
            ctx.command_pool,
            swapchain_state.render_pass,
            &ctx.allocator,
            jar_assets_dir,
            asset_index,
        );

        let block_overlay_pipeline = BlockOverlayPipeline::new(
            &ctx.device,
            ctx.graphics_queue,
            ctx.command_pool,
            swapchain_state.render_pass,
            &ctx.allocator,
            jar_assets_dir,
            asset_index,
        );

        splash(&mut menu_pipeline, 0.7, "Loading sky and panorama...");

        let sky_pipeline = SkyPipeline::new(
            &ctx.device,
            ctx.graphics_queue,
            ctx.command_pool,
            swapchain_state.render_pass,
            &ctx.allocator,
            jar_assets_dir,
            asset_index,
        );

        let panorama_pipeline = PanoramaPipeline::new(
            &ctx.device,
            ctx.graphics_queue,
            ctx.command_pool,
            swapchain_state.render_pass,
            &ctx.allocator,
            jar_assets_dir,
            asset_index,
        );

        splash(&mut menu_pipeline, 0.9, "Finalizing...");

        let skin_preview = SkinPreviewPipeline::new(
            &ctx.device,
            swapchain_state.render_pass,
            &ctx.allocator,
            hand_pipeline.skin_view(),
            hand_pipeline.skin_sampler(),
        );

        let blur_pipeline = BlurPipeline::new(
            &ctx.device,
            &ctx.allocator,
            size.width.max(1),
            size.height.max(1),
            swapchain_state.format.format,
        );

        let entity_renderer = EntityRenderer::new(
            &ctx.device,
            ctx.graphics_queue,
            ctx.command_pool,
            swapchain_state.render_pass,
            &ctx.allocator,
            jar_assets_dir,
            asset_index,
        );

        let chunk_border_pipeline = pipelines::chunk_borders::ChunkBorderPipeline::new(
            &ctx.device,
            swapchain_state.render_pass,
            &ctx.allocator,
        );

        let chunk_buffers = ChunkBufferStore::new(
            &ctx.device,
            &ctx.instance,
            ctx.physical_device,
            ctx.graphics_family,
            &ctx.allocator,
        );

        let item_entity_pipeline = pipelines::item_entity::ItemEntityPipeline::new(
            &ctx.device,
            swapchain_state.render_pass,
            &ctx.allocator,
            &atlas,
        );

        Ok(Self {
            ctx,
            swapchain: swapchain_state,
            camera,
            registry,
            jar_assets_dir: jar_assets_dir.to_path_buf(),
            asset_index: asset_index.clone(),
            atlas,
            chunk_pipeline,
            hand_pipeline,
            block_overlay_pipeline,
            sky_pipeline,
            panorama_pipeline,
            menu_pipeline,
            blur_pipeline,
            skin_preview,
            entity_renderer,
            chunk_border_pipeline,
            item_entity_pipeline,
            chunk_buffers,
            swapchain_dirty: false,
            width: size.width.max(1),
            height: size.height.max(1),
            last_timings: RenderTimings::default(),
        })
    }

    fn render_splash(
        ctx: &VulkanContext,
        swapchain: &SwapchainState,
        menu: &mut MenuOverlayPipeline,
        sw: f32,
        sh: f32,
        progress: f32,
        status: &str,
    ) -> Result<(), RendererError> {
        let fence = ctx.in_flight_fences[0];
        let image_available = ctx.image_available[0];
        let render_finished = ctx.render_finished[0];
        let cmd = ctx.command_buffers[0];

        let gs = (sh / 400.0).max(1.0);
        let title_size = 28.0 * gs;
        let status_size = 8.0 * gs;
        let bar_w = 200.0 * gs;
        let bar_h = 6.0 * gs;
        let bar_border = 1.0 * gs;
        let cx = sw / 2.0;
        let cy = sh / 2.0;

        let elements = vec![
            MenuElement::Text {
                x: cx,
                y: cy - title_size - 20.0 * gs,
                text: "Pomme".into(),
                scale: title_size,
                color: [0.86, 0.92, 1.0, 0.95],
                centered: true,
            },
            MenuElement::Rect {
                x: cx - bar_w / 2.0 - bar_border,
                y: cy - bar_border,
                w: bar_w + bar_border * 2.0,
                h: bar_h + bar_border * 2.0,
                corner_radius: (bar_h / 2.0 + bar_border),
                color: [0.3, 0.3, 0.3, 0.8],
            },
            MenuElement::Rect {
                x: cx - bar_w / 2.0,
                y: cy,
                w: bar_w * progress,
                h: bar_h,
                corner_radius: bar_h / 2.0,
                color: [0.39, 0.71, 1.0, 1.0],
            },
            MenuElement::Text {
                x: cx,
                y: cy + bar_h + 8.0 * gs,
                text: status.into(),
                scale: status_size,
                color: [0.6, 0.6, 0.6, 0.8],
                centered: true,
            },
        ];

        unsafe {
            ctx.device.wait_for_fences(&[fence], true, u64::MAX)?;

            let image_index = match ctx.swapchain_loader.acquire_next_image(
                swapchain.swapchain,
                u64::MAX,
                image_available,
                vk::Fence::null(),
            ) {
                Ok((idx, _)) => idx,
                Err(_) => return Ok(()),
            };

            ctx.device.reset_fences(&[fence])?;
            ctx.device
                .reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())?;

            let begin_info = vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
            ctx.device.begin_command_buffer(cmd, &begin_info)?;

            let clear_values = [
                vk::ClearValue {
                    color: vk::ClearColorValue {
                        float32: [0.0, 0.0, 0.0, 1.0],
                    },
                },
                vk::ClearValue {
                    depth_stencil: vk::ClearDepthStencilValue {
                        depth: 1.0,
                        stencil: 0,
                    },
                },
            ];

            let render_pass_info = vk::RenderPassBeginInfo::default()
                .render_pass(swapchain.render_pass)
                .framebuffer(swapchain.framebuffers[image_index as usize])
                .render_area(vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent: swapchain.extent,
                })
                .clear_values(&clear_values);

            ctx.device
                .cmd_begin_render_pass(cmd, &render_pass_info, vk::SubpassContents::INLINE);

            let viewport = vk::Viewport {
                x: 0.0,
                y: 0.0,
                width: sw,
                height: sh,
                min_depth: 0.0,
                max_depth: 1.0,
            };
            ctx.device.cmd_set_viewport(cmd, 0, &[viewport]);

            let scissor = vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: swapchain.extent,
            };
            ctx.device.cmd_set_scissor(cmd, 0, &[scissor]);

            menu.draw(&ctx.device, cmd, sw, sh, &elements);

            ctx.device.cmd_end_render_pass(cmd);
            ctx.device.end_command_buffer(cmd)?;

            let wait_semaphores = [image_available];
            let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
            let signal_semaphores = [render_finished];
            let cmd_buffers = [cmd];

            let submit_info = vk::SubmitInfo::default()
                .wait_semaphores(&wait_semaphores)
                .wait_dst_stage_mask(&wait_stages)
                .command_buffers(&cmd_buffers)
                .signal_semaphores(&signal_semaphores);

            ctx.device
                .queue_submit(ctx.graphics_queue, &[submit_info], fence)?;

            let swapchains = [swapchain.swapchain];
            let image_indices = [image_index];
            let present_info = vk::PresentInfoKHR::default()
                .wait_semaphores(&signal_semaphores)
                .swapchains(&swapchains)
                .image_indices(&image_indices);

            let _ = ctx
                .swapchain_loader
                .queue_present(ctx.present_queue, &present_info);

            ctx.device.wait_for_fences(&[fence], true, u64::MAX)?;
        }

        Ok(())
    }

    pub fn resize(&mut self, new_size: PhysicalSize<u32>) {
        if new_size.width == 0 || new_size.height == 0 {
            return;
        }
        self.width = new_size.width;
        self.height = new_size.height;
        self.swapchain_dirty = true;
        self.camera
            .set_aspect_ratio(new_size.width as f32 / new_size.height as f32);
    }

    fn recreate_swapchain(&mut self) -> Result<(), RendererError> {
        unsafe {
            let _ = self.ctx.device.device_wait_idle();
        }

        self.chunk_pipeline
            .destroy(&self.ctx.device, &self.ctx.allocator);

        let mut old_swapchain = SwapchainState::new(
            &self.ctx.device,
            &self.ctx.surface_loader,
            &self.ctx.swapchain_loader,
            self.ctx.physical_device,
            self.ctx.surface,
            self.width,
            self.height,
            self.ctx.graphics_family,
            self.ctx.present_family,
            &self.ctx.allocator,
            self.swapchain.swapchain,
        )?;
        std::mem::swap(&mut self.swapchain, &mut old_swapchain);
        old_swapchain.destroy(
            &self.ctx.device,
            &self.ctx.swapchain_loader,
            &self.ctx.allocator,
        );

        self.chunk_pipeline = ChunkPipeline::new(
            &self.ctx.device,
            self.swapchain.render_pass,
            &self.ctx.allocator,
            &self.atlas,
        );

        self.hand_pipeline
            .recreate_pipeline(&self.ctx.device, self.swapchain.render_pass);
        self.block_overlay_pipeline
            .recreate_pipeline(&self.ctx.device, self.swapchain.render_pass);
        self.sky_pipeline
            .recreate_pipeline(&self.ctx.device, self.swapchain.render_pass);
        self.panorama_pipeline
            .recreate_pipeline(&self.ctx.device, self.swapchain.render_pass);
        self.menu_pipeline
            .recreate_pipeline(&self.ctx.device, self.swapchain.render_pass);
        self.skin_preview
            .recreate_pipeline(&self.ctx.device, self.swapchain.render_pass);
        self.entity_renderer
            .recreate_pipeline(&self.ctx.device, self.swapchain.render_pass);
        self.item_entity_pipeline
            .recreate_pipeline(&self.ctx.device, self.swapchain.render_pass);
        self.blur_pipeline.resize(
            &self.ctx.device,
            &self.ctx.allocator,
            self.width,
            self.height,
        );

        self.swapchain_dirty = false;
        Ok(())
    }

    pub fn screen_width(&self) -> u32 {
        self.width
    }

    pub fn screen_height(&self) -> u32 {
        self.height
    }

    pub fn update_camera(&mut self, input: &mut InputState) {
        self.camera.update_look(input);
    }

    pub fn sync_camera_to_player(&mut self, eye_pos: glam::DVec3, yaw: f32, pitch: f32) {
        self.camera.set_position_f64(eye_pos);
        self.camera.yaw = yaw;
        self.camera.pitch = pitch;
    }

    pub fn update_third_person_distance(
        &mut self,
        eye_pos: glam::Vec3,
        chunks: &crate::world::chunk::ChunkStore,
    ) {
        if self.camera.mode == camera::CameraMode::FirstPerson {
            return;
        }
        let max = camera::THIRD_PERSON_DISTANCE;
        let fwd = self.camera.forward_vec();
        let dir = if self.camera.mode == camera::CameraMode::ThirdPersonFront {
            fwd
        } else {
            -fwd
        };
        let mut dist = max;

        let m = 0.4;
        let corners = [
            glam::Vec3::new(m, m, m),
            glam::Vec3::new(m, m, -m),
            glam::Vec3::new(m, -m, m),
            glam::Vec3::new(m, -m, -m),
            glam::Vec3::new(-m, m, m),
            glam::Vec3::new(-m, m, -m),
            glam::Vec3::new(-m, -m, m),
            glam::Vec3::new(-m, -m, -m),
        ];

        let step = 0.2;
        let mut t = step;
        while t <= max {
            let p = eye_pos + dir * t;
            let hit = corners.iter().any(|off| {
                let check = p + *off;
                let state = chunks.get_block_state(
                    check.x.floor() as i32,
                    check.y.floor() as i32,
                    check.z.floor() as i32,
                );
                self.registry.is_opaque_full_cube(state)
            });
            if hit {
                dist = (t - 0.3).max(0.5);
                break;
            }
            t += step;
        }

        self.camera.third_person_dist = dist.max(0.5);
    }

    pub fn update_fov(&mut self, modifier: f32) {
        self.camera.update_fov_modifier(modifier);
    }

    pub fn set_base_fov(&mut self, degrees: f32) {
        self.camera.base_fov_degrees = degrees;
    }

    pub fn camera_yaw(&self) -> f32 {
        self.camera.yaw
    }

    pub fn camera_pitch(&self) -> f32 {
        self.camera.pitch
    }

    pub fn cycle_camera_mode(&mut self) {
        self.camera.mode = self.camera.mode.cycle();
    }

    pub fn is_first_person(&self) -> bool {
        self.camera.mode == camera::CameraMode::FirstPerson
    }

    pub fn gpu_name(&self) -> &str {
        &self.ctx.gpu_name
    }

    pub fn vulkan_version(&self) -> &str {
        &self.ctx.vulkan_version
    }

    pub fn loaded_chunk_count(&self) -> u32 {
        self.chunk_buffers.chunk_count()
    }

    pub fn set_camera_position(&mut self, x: f64, y: f64, z: f64, yaw: f32, pitch: f32) {
        self.camera
            .set_position(glam::Vec3::new(x as f32, y as f32, z as f32), yaw, pitch);
        self.camera.position_f64 = glam::DVec3::new(x, y, z);
    }

    pub fn wait_for_all_frames(&self) {
        unsafe {
            let _ = self
                .ctx
                .device
                .wait_for_fences(&self.ctx.in_flight_fences, true, u64::MAX);
        }
    }

    pub fn upload_chunk_mesh(&mut self, mesh: &ChunkMeshData) {
        self.chunk_buffers
            .upload(&self.ctx.device, self.ctx.graphics_queue, mesh);
    }

    pub fn remove_chunk_mesh(&mut self, pos: &ChunkPos) {
        self.chunk_buffers.remove(pos);
    }

    pub fn clear_chunk_meshes(&mut self) {
        self.wait_for_all_frames();
        self.chunk_buffers.clear();
    }

    pub fn create_mesh_dispatcher(
        &self,
        biome_climate: std::sync::Arc<
            std::collections::HashMap<u32, crate::renderer::chunk::mesher::BiomeClimate>,
        >,
        packs: Option<&crate::resource_pack::ResourcePackManager>,
    ) -> MeshDispatcher {
        let grass_colormap = crate::renderer::chunk::mesher::Colormap::load(
            &self.jar_assets_dir,
            &self.asset_index,
            "minecraft/textures/colormap/grass.png",
            packs,
        );
        let foliage_colormap = crate::renderer::chunk::mesher::Colormap::load(
            &self.jar_assets_dir,
            &self.asset_index,
            "minecraft/textures/colormap/foliage.png",
            packs,
        );
        MeshDispatcher::new(
            self.registry.clone(),
            self.atlas.uv_map.clone(),
            grass_colormap,
            foliage_colormap,
            biome_climate,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_chunk_borders(&mut self, min_y: i32, max_y: i32) {
        let cam = self.camera.position;
        self.chunk_border_pipeline
            .update_lines(cam.x, cam.y, cam.z, min_y, max_y);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_world(
        &mut self,
        window: &Window,
        hide_cursor: bool,
        overlay: Vec<MenuElement>,
        swing_progress: f32,
        destroy_info: Option<(BlockPos, u32)>,
        show_chunk_borders: bool,
        sky: SkyState,
        entities: &[EntityRenderInfo],
        item_entities: &[pipelines::item_entity::ItemRenderInfo],
    ) -> Result<(), RendererError> {
        let sky_col = sky.sky_color();
        self.render_frame(
            window,
            hide_cursor,
            [sky_col[0], sky_col[1], sky_col[2], 1.0],
            RenderMode::World {
                overlay,
                swing_progress,
                destroy_info,
                show_chunk_borders,
                sky,
                entities,
                item_entities,
            },
        )
    }

    pub fn render_menu(
        &mut self,
        window: &Window,
        scroll: f32,
        blur: f32,
        elements: Vec<MenuElement>,
        cursor: (f32, f32),
        show_skin: bool,
    ) -> Result<(), RendererError> {
        self.render_frame(
            window,
            false,
            [0.0, 0.0, 0.0, 1.0],
            RenderMode::MainMenu {
                scroll,
                blur,
                elements,
                cursor,
                show_skin,
            },
        )
    }

    pub fn reload_assets(
        &mut self,
        game_dir: &Path,
        packs: &crate::resource_pack::ResourcePackManager,
    ) {
        unsafe { self.ctx.device.device_wait_idle().unwrap() };

        let cache_path = game_dir.join(crate::world::block::registry::BLOCK_CACHE_FILE);
        let _ = std::fs::remove_file(&cache_path);
        tracing::info!("Invalidated block cache");

        self.registry = BlockRegistry::load(
            &self.jar_assets_dir,
            &self.asset_index,
            game_dir,
            Some(packs),
        );

        self.atlas.destroy(&self.ctx.device, &self.ctx.allocator);
        let texture_names: std::collections::HashSet<&str> =
            self.registry.texture_names().collect();
        self.atlas = TextureAtlas::build(
            &self.ctx.device,
            self.ctx.graphics_queue,
            self.ctx.command_pool,
            &self.ctx.allocator,
            &self.jar_assets_dir,
            &self.asset_index,
            &texture_names,
            Some(packs),
        )
        .expect("failed to rebuild atlas");

        self.chunk_pipeline
            .rebind_atlas(&self.ctx.device, &self.atlas);

        tracing::info!("Assets reloaded");
    }

    pub fn reload_panorama(
        &mut self,
        jar_assets_dir: &Path,
        asset_index: &Option<crate::assets::AssetIndex>,
    ) {
        self.panorama_pipeline.reload_cubemap(
            &self.ctx.device,
            self.ctx.graphics_queue,
            self.ctx.command_pool,
            &self.ctx.allocator,
            jar_assets_dir,
            asset_index,
        );
    }

    pub fn trigger_skin_swing(&mut self) {
        self.skin_preview.trigger_swing();
    }

    pub fn load_player_skin(&mut self, uuid: &uuid::Uuid, rt: &tokio::runtime::Runtime) {
        let uuid_str = uuid.to_string().replace('-', "");
        let skin_pixels = rt.block_on(async { fetch_skin_texture(&uuid_str).await });
        match skin_pixels {
            Ok((pixels, w, h)) => {
                self.hand_pipeline.reload_skin(
                    &self.ctx.device,
                    self.ctx.graphics_queue,
                    self.ctx.command_pool,
                    &self.ctx.allocator,
                    &pixels,
                    w,
                    h,
                );
                self.skin_preview = SkinPreviewPipeline::new(
                    &self.ctx.device,
                    self.swapchain.render_pass,
                    &self.ctx.allocator,
                    self.hand_pipeline.skin_view(),
                    self.hand_pipeline.skin_sampler(),
                );
            }
            Err(e) => tracing::warn!("Failed to load player skin: {e}"),
        }
    }

    pub fn update_favicon_atlas(&mut self, favicons: &[(String, Vec<u8>, u32)]) {
        self.menu_pipeline.update_favicon_atlas(
            &self.ctx.device,
            self.ctx.graphics_queue,
            self.ctx.command_pool,
            &self.ctx.allocator,
            favicons,
        );
    }

    pub fn menu_text_width(&self, text: &str, scale: f32) -> f32 {
        self.menu_pipeline.text_width(text, scale)
    }

    pub fn ensure_item_mesh(&mut self, name: &str) -> bool {
        if self.item_entity_pipeline.has_mesh(name) {
            return self.registry.get_baked_model_by_name(name).is_some();
        }
        if let Some(model) = self.registry.get_baked_model_by_name(name) {
            self.item_entity_pipeline.ensure_mesh(
                &self.ctx.device,
                &self.ctx.allocator,
                name,
                model,
                &self.atlas.uv_map,
            );
            true
        } else {
            self.item_entity_pipeline.ensure_flat_mesh(
                &self.ctx.device,
                &self.ctx.allocator,
                name,
                &self.atlas.uv_map,
                &self.jar_assets_dir,
                &self.asset_index,
            );
            false
        }
    }

    fn render_frame(
        &mut self,
        window: &Window,
        hide_cursor: bool,
        clear_color: [f32; 4],
        mode: RenderMode<'_>,
    ) -> Result<(), RendererError> {
        if self.swapchain_dirty {
            self.recreate_swapchain()?;
        }

        let frame = self.ctx.frame_index;
        let fence = self.ctx.in_flight_fences[frame];
        let image_available = self.ctx.image_available[frame];
        let render_finished = self.ctx.render_finished[frame];
        let cmd = self.ctx.command_buffers[frame];

        let t_fence = std::time::Instant::now();
        unsafe {
            self.ctx.device.wait_for_fences(&[fence], true, u64::MAX)?;
        }
        let fence_ms = t_fence.elapsed().as_secs_f32() * 1000.0;

        let t_acquire = std::time::Instant::now();
        let image_index = match unsafe {
            self.ctx.swapchain_loader.acquire_next_image(
                self.swapchain.swapchain,
                u64::MAX,
                image_available,
                vk::Fence::null(),
            )
        } {
            Ok((idx, _)) => idx,
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                self.swapchain_dirty = true;
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        };
        let acquire_ms = t_acquire.elapsed().as_secs_f32() * 1000.0;

        if let RenderMode::World { ref sky, .. } = mode {
            let fog = sky.fog_color();
            let uniform = CameraUniform::new(&self.camera, fog);
            self.chunk_pipeline.update_camera(frame, &uniform);
            self.block_overlay_pipeline.update_camera(frame, &uniform);
            self.entity_renderer.update_camera(frame, &uniform);
            self.chunk_border_pipeline.update_camera(frame, &uniform);
            self.item_entity_pipeline.update_camera(frame, &uniform);
        }

        if hide_cursor {
            window.set_cursor_visible(false);
        }

        unsafe {
            self.ctx.device.reset_fences(&[fence])?;
            self.ctx
                .device
                .reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())?;

            let begin_info = vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
            self.ctx.device.begin_command_buffer(cmd, &begin_info)?;

            if matches!(&mode, RenderMode::World { .. }) {
                let frustum = self.camera.frustum_planes();
                let cam_pos = [
                    self.camera.position.x,
                    self.camera.position.y,
                    self.camera.position.z,
                ];
                self.chunk_buffers
                    .dispatch_cull(&self.ctx.device, cmd, frame, &frustum, cam_pos);
            }

            let clear_values = [
                vk::ClearValue {
                    color: vk::ClearColorValue {
                        float32: clear_color,
                    },
                },
                vk::ClearValue {
                    depth_stencil: vk::ClearDepthStencilValue {
                        depth: 1.0,
                        stencil: 0,
                    },
                },
            ];

            let use_blur = matches!(&mode, RenderMode::MainMenu { blur, .. } if *blur > 0.01);

            let (rp, fb) = if use_blur {
                (
                    self.swapchain.render_pass_scene,
                    self.swapchain.framebuffers_scene[image_index as usize],
                )
            } else {
                (
                    self.swapchain.render_pass,
                    self.swapchain.framebuffers[image_index as usize],
                )
            };

            let render_pass_info = vk::RenderPassBeginInfo::default()
                .render_pass(rp)
                .framebuffer(fb)
                .render_area(vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent: self.swapchain.extent,
                })
                .clear_values(&clear_values);

            self.ctx.device.cmd_begin_render_pass(
                cmd,
                &render_pass_info,
                vk::SubpassContents::INLINE,
            );

            let viewport = vk::Viewport {
                x: 0.0,
                y: 0.0,
                width: self.swapchain.extent.width as f32,
                height: self.swapchain.extent.height as f32,
                min_depth: 0.0,
                max_depth: 1.0,
            };
            self.ctx.device.cmd_set_viewport(cmd, 0, &[viewport]);

            let scissor = vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: self.swapchain.extent,
            };
            self.ctx.device.cmd_set_scissor(cmd, 0, &[scissor]);

            let sw = self.swapchain.extent.width as f32;
            let sh = self.swapchain.extent.height as f32;

            let frame_start = std::time::Instant::now();

            match &mode {
                RenderMode::World {
                    overlay,
                    swing_progress,
                    destroy_info,
                    show_chunk_borders,
                    sky,
                    entities,
                    item_entities,
                } => {
                    self.sky_pipeline.update_and_draw(
                        &self.ctx.device,
                        cmd,
                        frame,
                        &self.camera,
                        sky,
                    );

                    let t_cull = std::time::Instant::now();
                    self.chunk_pipeline.bind(&self.ctx.device, cmd, frame);
                    self.chunk_buffers
                        .draw_indirect(&self.ctx.device, cmd, frame);
                    let cull_ms = t_cull.elapsed().as_secs_f32() * 1000.0;

                    if let Some((block_pos, stage)) = destroy_info {
                        self.block_overlay_pipeline.draw(
                            &self.ctx.device,
                            cmd,
                            frame,
                            block_pos,
                            *stage,
                        );
                    }

                    self.entity_renderer
                        .draw(&self.ctx.device, cmd, frame, entities);

                    self.item_entity_pipeline
                        .draw(&self.ctx.device, cmd, frame, item_entities);

                    if *show_chunk_borders {
                        self.chunk_border_pipeline
                            .draw(&self.ctx.device, cmd, frame);
                    }

                    let clear_attachment = vk::ClearAttachment {
                        aspect_mask: vk::ImageAspectFlags::DEPTH,
                        color_attachment: 0,
                        clear_value: vk::ClearValue {
                            depth_stencil: vk::ClearDepthStencilValue {
                                depth: 1.0,
                                stencil: 0,
                            },
                        },
                    };
                    let clear_rect = vk::ClearRect {
                        rect: scissor,
                        base_array_layer: 0,
                        layer_count: 1,
                    };
                    self.ctx
                        .device
                        .cmd_clear_attachments(cmd, &[clear_attachment], &[clear_rect]);

                    if self.camera.mode == camera::CameraMode::FirstPerson {
                        let aspect = sw / sh.max(1.0);
                        self.hand_pipeline.update_and_draw(
                            &self.ctx.device,
                            cmd,
                            frame,
                            aspect,
                            *swing_progress,
                        );
                    }

                    self.menu_pipeline
                        .draw(&self.ctx.device, cmd, sw, sh, overlay);

                    self.last_timings.cull_ms = cull_ms;
                    self.last_timings.frame_ms = frame_start.elapsed().as_secs_f32() * 1000.0;
                }
                RenderMode::MainMenu {
                    scroll,
                    blur,
                    elements,
                    cursor,
                    show_skin,
                } => {
                    let aspect = sw / sh.max(1.0);
                    self.panorama_pipeline
                        .draw(&self.ctx.device, cmd, *scroll, aspect, 0.0);

                    if *blur > 0.01 {
                        self.ctx.device.cmd_end_render_pass(cmd);

                        let swapchain_image = self.swapchain.images[image_index as usize];
                        let iterations = ((*blur * 3.0).ceil() as u32).clamp(1, 4);
                        self.blur_pipeline.execute(
                            &self.ctx.device,
                            cmd,
                            swapchain_image,
                            self.swapchain.extent.width,
                            self.swapchain.extent.height,
                            iterations,
                        );

                        self.menu_pipeline.set_blur_texture(
                            &self.ctx.device,
                            self.blur_pipeline.blurred_view(),
                            self.blur_pipeline.blurred_sampler(),
                        );

                        let load_rp_info = vk::RenderPassBeginInfo::default()
                            .render_pass(self.swapchain.render_pass_load)
                            .framebuffer(self.swapchain.framebuffers_load[image_index as usize])
                            .render_area(vk::Rect2D {
                                offset: vk::Offset2D { x: 0, y: 0 },
                                extent: self.swapchain.extent,
                            })
                            .clear_values(&clear_values);
                        self.ctx.device.cmd_begin_render_pass(
                            cmd,
                            &load_rp_info,
                            vk::SubpassContents::INLINE,
                        );
                        self.ctx.device.cmd_set_viewport(cmd, 0, &[viewport]);
                        self.ctx.device.cmd_set_scissor(cmd, 0, &[scissor]);
                    }

                    if *show_skin {
                        self.skin_preview.draw(
                            &self.ctx.device,
                            cmd,
                            frame,
                            aspect,
                            0.7,
                            0.5,
                            cursor.0,
                            cursor.1,
                            sw,
                            sh,
                        );
                    }

                    self.menu_pipeline
                        .draw(&self.ctx.device, cmd, sw, sh, elements);
                }
            }

            self.ctx.device.cmd_end_render_pass(cmd);

            self.ctx.device.end_command_buffer(cmd)?;

            let wait_semaphores = [image_available];
            let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
            let signal_semaphores = [render_finished];
            let cmd_buffers = [cmd];

            let submit_info = vk::SubmitInfo::default()
                .wait_semaphores(&wait_semaphores)
                .wait_dst_stage_mask(&wait_stages)
                .command_buffers(&cmd_buffers)
                .signal_semaphores(&signal_semaphores);

            self.ctx
                .device
                .queue_submit(self.ctx.graphics_queue, &[submit_info], fence)?;

            let swapchains = [self.swapchain.swapchain];
            let image_indices = [image_index];
            let present_info = vk::PresentInfoKHR::default()
                .wait_semaphores(&signal_semaphores)
                .swapchains(&swapchains)
                .image_indices(&image_indices);

            let t_present = std::time::Instant::now();
            match self
                .ctx
                .swapchain_loader
                .queue_present(self.ctx.present_queue, &present_info)
            {
                Ok(false) => {}
                Ok(true) | Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                    self.swapchain_dirty = true;
                }
                Err(e) => return Err(e.into()),
            }
            let present_ms = t_present.elapsed().as_secs_f32() * 1000.0;
            self.last_timings.fence_ms = fence_ms;
            self.last_timings.acquire_ms = acquire_ms;
            self.last_timings.present_ms = present_ms;
        }

        self.ctx.advance_frame();
        Ok(())
    }
}

async fn fetch_skin_texture(uuid: &str) -> Result<(Vec<u8>, u32, u32), String> {
    #[derive(serde::Deserialize)]
    struct SessionProfile {
        properties: Vec<ProfileProperty>,
    }
    #[derive(serde::Deserialize)]
    struct ProfileProperty {
        value: String,
    }
    #[derive(serde::Deserialize)]
    struct TexturesPayload {
        textures: Textures,
    }
    #[derive(serde::Deserialize)]
    struct Textures {
        #[serde(rename = "SKIN")]
        skin: Option<SkinTexture>,
    }
    #[derive(serde::Deserialize)]
    struct SkinTexture {
        url: String,
    }

    let url = format!("https://sessionserver.mojang.com/session/minecraft/profile/{uuid}");
    let profile: SessionProfile = reqwest::get(&url)
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let value = &profile.properties.first().ok_or("No properties")?.value;

    use base64::Engine;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|e| e.to_string())?;
    let payload: TexturesPayload = serde_json::from_slice(&decoded).map_err(|e| e.to_string())?;

    let skin_url = payload
        .textures
        .skin
        .map(|s| s.url)
        .ok_or("No skin texture")?;

    let skin_bytes = reqwest::get(&skin_url)
        .await
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;

    let img = image::load_from_memory(&skin_bytes).map_err(|e| e.to_string())?;
    let rgba = img.to_rgba8();
    let w = rgba.width();
    let h = rgba.height();
    Ok((rgba.into_raw(), w, h))
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            let _ = self.ctx.device.device_wait_idle();
        }
        self.chunk_buffers
            .destroy(&self.ctx.device, &self.ctx.allocator);
        self.chunk_pipeline
            .destroy(&self.ctx.device, &self.ctx.allocator);
        self.hand_pipeline
            .destroy(&self.ctx.device, &self.ctx.allocator);
        self.block_overlay_pipeline
            .destroy(&self.ctx.device, &self.ctx.allocator);
        self.sky_pipeline
            .destroy(&self.ctx.device, &self.ctx.allocator);
        self.panorama_pipeline
            .destroy(&self.ctx.device, &self.ctx.allocator);
        self.menu_pipeline
            .destroy(&self.ctx.device, &self.ctx.allocator);
        self.blur_pipeline
            .destroy(&self.ctx.device, &self.ctx.allocator);
        self.skin_preview
            .destroy(&self.ctx.device, &self.ctx.allocator);
        self.entity_renderer
            .destroy(&self.ctx.device, &self.ctx.allocator);
        self.chunk_border_pipeline
            .destroy(&self.ctx.device, &self.ctx.allocator);
        self.item_entity_pipeline
            .destroy(&self.ctx.device, &self.ctx.allocator);
        self.atlas.destroy(&self.ctx.device, &self.ctx.allocator);
        self.swapchain.destroy(
            &self.ctx.device,
            &self.ctx.swapchain_loader,
            &self.ctx.allocator,
        );
    }
}
