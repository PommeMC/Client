pub mod block_entity_model;
pub mod camera;
pub mod chunk;
mod context;
pub mod entity_model;
pub mod hiz;
pub mod pipelines;
pub(crate) mod shader;
mod swapchain;
pub(crate) mod timings;
pub(crate) mod util;
pub mod visibility;

pub(crate) const MAX_FRAMES_IN_FLIGHT: usize = 3;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use azalea_block::BlockState;
use azalea_core::position::{BlockPos, ChunkPos, ChunkSectionPos};
pub use camera::CloudMode;
use camera::{Camera, CameraUniform};
use chunk::atlas::TextureAtlas;
use chunk::buffer::ChunkBufferStore;
use chunk::dispatcher::ChunkMeshing;
use chunk::mesher::SectionMeshData;
use context::VulkanContext;
use glam::dvec3;
use hiz::HizPipeline;
use pipelines::block_entity::BlockEntityPipeline;
pub use pipelines::block_entity::BlockEntityRenderInfo;
use pipelines::block_overlay::BlockOverlayPipeline;
use pipelines::blur::BlurPipeline;
use pipelines::chunk::ChunkPipeline;
use pipelines::clouds::CloudPipeline;
use pipelines::entity_renderer::{EntityRenderInfo, EntityRenderer};
use pipelines::hand::HandPipeline;
use pipelines::menu_overlay::{MenuElement, MenuOverlayPipeline};
use pipelines::panorama::PanoramaPipeline;
use pipelines::skin_preview::SkinPreviewPipeline;
pub use pipelines::sky::{SkyPipeline, SkyState};
pub use pipelines::weather::{WeatherColumn, WeatherPipeline};
use pyronyx::khr::swapchain::{SwapchainDevice, SwapchainQueue};
use pyronyx::vk;
use swapchain::Swapchain;
use thiserror::Error;
use visibility::VisibilityPipeline;
use winit::dpi::PhysicalSize;
use winit::window::Window;

use crate::app::input::InputState;
use crate::assets::AssetIndex;
use crate::entity::components::{LookDirection, Position};
use crate::renderer::pipelines::chunk_borders::ChunkBorderPipeline;
use crate::renderer::pipelines::item_entity::ItemEntityPipeline;
use crate::renderer::timings::{RenderTimings, Timer, Timestamp};
use crate::util::ChunkRing;
use crate::world::block::registry::BlockRegistry;

#[derive(Error, Debug)]
pub enum RendererError {
    #[error("failed to initialize GPU context: {0}")]
    Context(#[from] context::ContextError),

    #[error("vulkan error: {0}")]
    Vulkan(#[from] vk::Error),
}

#[derive(Clone, Copy)]
pub struct PlayerPreview {
    pub rect: [f32; 4],
    pub gui_scale: f32,
    pub cursor: (f32, f32),
}

struct FrameCtx {
    frame: usize,
    image_index: u32,
    cmd: vk::CommandBuffer,
    viewport: vk::Viewport,
    scissor: vk::Rect2D,
}

pub struct Renderer {
    ctx: VulkanContext,
    swapchain: Swapchain,
    pub camera: Camera,

    pub mesh_queue: VecDeque<SectionMeshData>,

    registry: BlockRegistry,
    jar_assets_dir: PathBuf,
    asset_index: Option<AssetIndex>,

    chunk_pipeline: ChunkPipeline,
    hand_pipeline: HandPipeline,
    block_overlay_pipeline: BlockOverlayPipeline,
    sky_pipeline: SkyPipeline,
    panorama_pipeline: PanoramaPipeline,
    menu_pipeline: MenuOverlayPipeline,
    blur_pipeline: BlurPipeline,
    skin_preview: SkinPreviewPipeline,
    chunk_border_pipeline: ChunkBorderPipeline,
    item_entity_pipeline: ItemEntityPipeline,
    held_item_pipeline: pipelines::held_item::HeldItemPipeline,
    weather_pipeline: WeatherPipeline,
    cloud_pipeline: CloudPipeline,
    gui_item_pipeline: pipelines::gui_item::GuiItemPipeline,
    gui_item_atlas: pipelines::gui_item_atlas::GuiItemAtlas,

    atlas: TextureAtlas,
    entity_renderer: EntityRenderer,
    block_entity_pipeline: BlockEntityPipeline,
    chunk_buffers: ChunkBufferStore,
    render_finished_per_image: Vec<vk::Semaphore>,
    swapchain_dirty: bool,
    vsync: bool,
    width: u32,
    height: u32,
    query_pools: Option<[vk::QueryPool; MAX_FRAMES_IN_FLIGHT]>,
    query_reset: [bool; MAX_FRAMES_IN_FLIGHT],
    last_timings: RenderTimings,
    hiz_pipeline: HizPipeline,
    visibility_pipeline: VisibilityPipeline,
}

impl Renderer {
    pub fn new(
        window: Arc<Window>,
        jar_assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
        game_dir: &Path,
        vsync: bool,
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

        let swapchain_state = Swapchain::new(
            &ctx,
            size.width.max(1),
            size.height.max(1),
            vsync,
            vk::SwapchainKHR::null(),
        )?;
        // The swapchain may pick the surface's `current_extent` rather than the
        // requested size; track that actual extent so layout matches rendering.
        let swapchain_extent = swapchain_state.extent;

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

        let texture_names: HashSet<&str> = registry
            .texture_names()
            .chain(registry.flat_item_textures())
            .collect();
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

        let weather_pipeline = WeatherPipeline::new(
            &ctx.device,
            ctx.graphics_queue,
            ctx.command_pool,
            swapchain_state.render_pass,
            &ctx.allocator,
            jar_assets_dir,
            asset_index,
        );

        let cloud_pipeline = CloudPipeline::new(
            &ctx.device,
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
            ctx.graphics_queue,
            ctx.command_pool,
            &ctx.allocator,
            size.width.max(1),
            size.height.max(1),
            swapchain_state.format.format,
        );
        menu_pipeline.set_blur_texture(
            &ctx.device,
            blur_pipeline.blurred_view(),
            blur_pipeline.blurred_sampler(),
        );

        let sem_info = vk::SemaphoreCreateInfo::default();
        let mut render_finished_per_image = Vec::with_capacity(swapchain_state.images.len());
        for _ in 0..swapchain_state.images.len() {
            render_finished_per_image.push(ctx.device.create_semaphore(&sem_info, None)?);
        }

        let entity_renderer = EntityRenderer::new(
            &ctx.device,
            ctx.graphics_queue,
            ctx.command_pool,
            swapchain_state.render_pass,
            &ctx.allocator,
            jar_assets_dir,
            asset_index,
        );

        let block_entity_pipeline = BlockEntityPipeline::new(
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
            ctx.physical_device,
            ctx.graphics_family,
            &ctx.allocator,
        );

        let mut item_entity_pipeline = pipelines::item_entity::ItemEntityPipeline::new(
            &ctx.device,
            swapchain_state.render_pass,
            &ctx.allocator,
            &atlas,
        );

        let held_item_pipeline = pipelines::held_item::HeldItemPipeline::new(
            &ctx.device,
            swapchain_state.render_pass,
            &ctx.allocator,
            &atlas,
            jar_assets_dir,
        );

        splash(&mut menu_pipeline, 0.95, "Caching item meshes...");

        let initial_slot_px =
            pipelines::gui_item_atlas::slot_px_for_gui_scale(crate::ui::hud::gui_scale(sw, sh, 0));
        let gui_item_atlas = build_gui_item_atlas(
            &ctx.device,
            &ctx.allocator,
            ctx.graphics_queue,
            ctx.command_pool,
            &menu_pipeline,
            initial_slot_px,
        );

        let gui_item_pipeline = pipelines::gui_item::GuiItemPipeline::new(
            &ctx.device,
            gui_item_atlas.render_pass(),
            gui_item_atlas.atlas_px(),
            &ctx.allocator,
            &atlas,
            jar_assets_dir,
        );

        warm_item_meshes(
            &ctx.device,
            &ctx.allocator,
            &mut item_entity_pipeline,
            &atlas.uv_map,
            &registry,
            jar_assets_dir,
            asset_index,
        );

        let query_pools = if ctx.features.timestamp_queries {
            Some(std::array::from_fn(|_| {
                let count = Timestamp::Count as u32;
                let info = vk::QueryPoolCreateInfo {
                    query_type: vk::QueryType::Timestamp,
                    query_count: count,
                    ..Default::default()
                };

                ctx.device.create_query_pool(&info, None).unwrap()
            }))
        } else {
            None
        };

        let properties = ctx.physical_device.get_properties();
        let timestamp_period = properties.limits.timestamp_period;
        let hiz_pipeline = HizPipeline::new(
            &ctx.device,
            &ctx.allocator,
            swapchain_extent.width,
            swapchain_extent.height,
            swapchain_state.depth_view,
        );
        let visibility_pipeline =
            VisibilityPipeline::new(&ctx.device, &ctx.allocator, &hiz_pipeline);
        Ok(Self {
            ctx,
            swapchain: swapchain_state,
            mesh_queue: VecDeque::new(),
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
            block_entity_pipeline,
            chunk_border_pipeline,
            item_entity_pipeline,
            held_item_pipeline,
            weather_pipeline,
            cloud_pipeline,
            gui_item_pipeline,
            gui_item_atlas,
            chunk_buffers,
            render_finished_per_image,
            swapchain_dirty: false,
            vsync,
            width: swapchain_extent.width,
            height: swapchain_extent.height,

            query_pools,
            query_reset: Default::default(),
            last_timings: RenderTimings {
                ticks: [0; _],
                timestamp_period,
            },
            hiz_pipeline,
            visibility_pipeline,
        })
    }

    fn render_splash(
        ctx: &VulkanContext,
        swapchain: &Swapchain,
        menu: &mut MenuOverlayPipeline,
        sw: f32,
        sh: f32,
        progress: f32,
        status: &str,
    ) -> Result<(), RendererError> {
        let fence = ctx.in_flight_fences[0];
        let image_available = ctx.image_available_semaphores[0];
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

        ctx.device.wait_for_fences(&[fence], true, u64::MAX)?;

        let image_index = match ctx.device.acquire_next_image(
            swapchain.handle,
            u64::MAX,
            image_available,
            vk::Fence::null(),
        ) {
            Ok(result) => result.value,
            Err(_) => return Ok(()),
        };

        ctx.device.reset_fences(&[fence])?;
        cmd.reset(vk::CommandBufferResetFlags::empty())?;

        let begin_info = vk::CommandBufferBeginInfo {
            flags: vk::CommandBufferUsageFlags::OneTimeSubmit,
            ..Default::default()
        };
        cmd.begin(&begin_info)?;

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

        let render_pass_info = vk::RenderPassBeginInfo {
            render_pass: swapchain.render_pass,
            framebuffer: swapchain.framebuffers[image_index as usize],
            render_area: vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: swapchain.extent,
            },
            clear_value_count: clear_values.len() as u32,
            clear_values: clear_values.as_ptr(),
            ..Default::default()
        };

        cmd.begin_render_pass(&render_pass_info, vk::SubpassContents::Inline);

        let viewport = vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: sw,
            height: sh,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        cmd.set_viewport(0, &[viewport]);

        let scissor = vk::Rect2D {
            offset: vk::Offset2D { x: 0, y: 0 },
            extent: swapchain.extent,
        };
        cmd.set_scissor(0, &[scissor]);

        let empty_uvs: HashMap<String, [f32; 4]> = HashMap::new();
        menu.draw(cmd, sw, sh, &elements, &empty_uvs);

        cmd.end_render_pass();
        cmd.end()?;

        let submit_info = vk::SubmitInfo {
            wait_semaphore_count: 1,
            wait_semaphores: &image_available,
            wait_dst_stage_mask: &vk::PipelineStageFlags::ColorAttachmentOutput,

            command_buffer_count: 1,
            command_buffers: &cmd.handle(),

            ..Default::default()
        };

        ctx.graphics_queue.submit(&[submit_info], fence)?;

        ctx.device.wait_for_fences(&[fence], true, u64::MAX)?;

        let present_info = vk::PresentInfoKHR {
            swapchain_count: 1,
            swapchains: &swapchain.handle,
            image_indices: &image_index,
            ..Default::default()
        };

        let _ = ctx.present_queue.present(&present_info);
        let _ = ctx.present_queue.wait_idle();

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

    pub fn set_vsync(&mut self, vsync: bool) {
        if self.vsync != vsync {
            self.vsync = vsync;
            self.swapchain_dirty = true;
        }
    }

    fn recreate_swapchain(&mut self) -> Result<(), RendererError> {
        let _ = self.ctx.device.wait_idle();

        for sem in self.render_finished_per_image.drain(..) {
            self.ctx.device.destroy_semaphore(sem, None);
        }

        self.chunk_pipeline
            .destroy(&self.ctx.device, &self.ctx.allocator);

        let mut old_swapchain = Swapchain::new(
            &self.ctx,
            self.width,
            self.height,
            self.vsync,
            self.swapchain.handle,
        )?;
        std::mem::swap(&mut self.swapchain, &mut old_swapchain);
        old_swapchain.destroy(&self.ctx.device, &self.ctx.allocator);

        // Adopt the swapchain's actual extent (may differ from the requested
        // window size, e.g. macOS fullscreen) so the viewport, menu layout, blur,
        // and camera aspect all agree — otherwise the image stretches/letterboxes.
        self.width = self.swapchain.extent.width;
        self.height = self.swapchain.extent.height;
        self.camera
            .set_aspect_ratio(self.width as f32 / self.height as f32);

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
        self.block_entity_pipeline
            .recreate_pipeline(&self.ctx.device, self.swapchain.render_pass);
        self.item_entity_pipeline
            .recreate_pipeline(&self.ctx.device, self.swapchain.render_pass);
        self.held_item_pipeline
            .recreate_pipeline(&self.ctx.device, self.swapchain.render_pass);
        self.weather_pipeline
            .recreate_pipeline(&self.ctx.device, self.swapchain.render_pass);
        self.cloud_pipeline
            .recreate_pipeline(&self.ctx.device, self.swapchain.render_pass);
        self.blur_pipeline.resize(
            &self.ctx.device,
            self.ctx.graphics_queue,
            self.ctx.command_pool,
            &self.ctx.allocator,
            self.width,
            self.height,
        );
        self.menu_pipeline.set_blur_texture(
            &self.ctx.device,
            self.blur_pipeline.blurred_view(),
            self.blur_pipeline.blurred_sampler(),
        );
        self.hiz_pipeline.resize(
            &self.ctx.device,
            &self.ctx.allocator,
            self.width,
            self.height,
            self.swapchain.depth_view,
        );
        self.visibility_pipeline
            .update_hiz_descriptors(&self.ctx.device, &self.hiz_pipeline);

        let sem_info = vk::SemaphoreCreateInfo::default();
        self.render_finished_per_image = Vec::with_capacity(self.swapchain.images.len());
        for _ in 0..self.swapchain.images.len() {
            self.render_finished_per_image
                .push(self.ctx.device.create_semaphore(&sem_info, None)?);
        }

        self.swapchain_dirty = false;
        Ok(())
    }

    pub fn screen_width(&self) -> u32 {
        self.width
    }

    pub fn screen_height(&self) -> u32 {
        self.height
    }

    #[inline]
    pub const fn last_timings(&self) -> &RenderTimings {
        &self.last_timings
    }

    pub fn update_camera(&mut self, input: &mut InputState, dt: f32) {
        self.camera.update_look(input, dt);
    }

    pub fn sync_camera_pos(&mut self, position: Position) {
        self.camera.sync_pos(position);
    }

    pub fn set_view_bob(&mut self, walk_dist: f32, bob: f32, enabled: bool) {
        self.camera.set_view_bob(walk_dist, bob, enabled);
    }

    pub fn reset_camera(&mut self, position: Position, look_dir: LookDirection) {
        self.camera.reset(position, look_dir);
    }

    pub fn set_top_down_radius(&mut self, radius_blocks: f32) {
        self.camera.frame_top_down(radius_blocks);
    }

    pub fn clear_top_down(&mut self) {
        self.camera.clear_top_down();
    }

    pub fn update_third_person_distance(
        &mut self,
        eye_pos: Position,
        chunks: &crate::world::chunk::ChunkStore,
    ) {
        if self.camera.mode == camera::CameraMode::FirstPerson || self.camera.top_down().is_some() {
            return;
        }
        let max = camera::THIRD_PERSON_DISTANCE as f64;
        let fwd = self.camera.look_dir.as_vec().as_dvec3();
        let dir = if self.camera.mode == camera::CameraMode::ThirdPersonFront {
            fwd
        } else {
            -fwd
        };

        let mut dist = max;

        let m = 0.4;
        let corners = [
            dvec3(m, m, m),
            dvec3(m, m, -m),
            dvec3(m, -m, m),
            dvec3(m, -m, -m),
            dvec3(-m, m, m),
            dvec3(-m, m, -m),
            dvec3(-m, -m, m),
            dvec3(-m, -m, -m),
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

        self.camera.third_person_dist = dist.max(0.5) as f32;
    }

    pub fn update_fov_mod(&mut self, modifier: f32) {
        self.camera.update_fov_modifier(modifier);
    }

    pub fn set_fluid_fov_factor(&mut self, factor: f32) {
        self.camera.set_fluid_fov_factor(factor);
    }

    pub fn set_render_partial_tick(&mut self, partial_tick: f32) {
        self.camera.set_render_partial_tick(partial_tick);
    }

    pub fn set_base_fov(&mut self, degrees: f32) {
        self.camera.base_fov_degrees = degrees;
    }

    pub fn camera_look_dir(&self) -> LookDirection {
        self.camera.look_dir
    }

    pub fn camera_pivot_position(&self) -> Position {
        self.camera.position
    }

    /// Camera position used for rendering (eye plus any third-person offset),
    /// matching `CameraUniform`'s `camera_pos`.
    pub fn camera_render_position(&self) -> glam::DVec3 {
        *self.camera.position + self.camera.third_person_offset().as_dvec3()
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

    /// Sections actually drawn after frustum culling (lags a few frames). The
    /// graph's occluded sections are omitted before the cull, so this also
    /// drops when occlusion hides geometry — useful for the F3 overlay.
    pub fn sections_drawn(&self) -> u32 {
        self.chunk_buffers.sections_drawn()
    }

    pub fn wait_for_all_frames(&self) {
        let _ = self
            .ctx
            .device
            .wait_for_fences(&self.ctx.in_flight_fences, true, u64::MAX);
    }

    pub fn upload_mesh_batch(&mut self) -> Vec<(ChunkSectionPos, u64)> {
        self.chunk_buffers.upload_mesh_batch(
            &self.ctx.device,
            &self.ctx.allocator,
            self.ctx.graphics_queue,
            &mut self.mesh_queue,
        )
    }

    pub fn remove_chunk_mesh(&mut self, pos: &ChunkPos) {
        self.chunk_buffers.remove(pos);
    }

    pub fn clear_chunk_meshes(&mut self) {
        self.wait_for_all_frames();
        self.chunk_buffers.clear();
    }

    pub fn registry(&self) -> &BlockRegistry {
        &self.registry
    }

    pub fn create_chunk_meshing(
        &self,
        shared_chunk_store: std::sync::Arc<crate::world::chunk::SharedChunkStore>,
        biome_climate: std::sync::Arc<
            std::collections::HashMap<u32, crate::renderer::chunk::mesher::BiomeClimate>,
        >,
        packs: Option<&crate::resource_pack::ResourcePackManager>,
    ) -> ChunkMeshing {
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
        let dry_foliage_colormap = crate::renderer::chunk::mesher::Colormap::load(
            &self.jar_assets_dir,
            &self.asset_index,
            "minecraft/textures/colormap/dry_foliage.png",
            packs,
        );
        ChunkMeshing::create(
            shared_chunk_store,
            self.registry.clone(),
            self.atlas.uv_map.clone(),
            grass_colormap,
            foliage_colormap,
            dry_foliage_colormap,
            biome_climate,
        )
    }

    pub fn update_chunk_borders(&mut self, min_y: i32, max_y: i32) {
        let cam = self.camera.position.as_vec3();
        self.chunk_border_pipeline
            .update_lines(cam.x, cam.y, cam.z, min_y, max_y);
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
        if self.swapchain_dirty {
            self.recreate_swapchain()?;
        }

        let frame = self.ctx.frame_index;
        let fence = self.ctx.in_flight_fences[frame];

        self.ctx.device.wait_for_fences(&[fence], true, u64::MAX)?;

        let ctx = self.begin_frame(window, false)?;

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

        let item_atlas_uvs = self.run_gui_bake(&ctx, &elements);

        let use_blur = blur > 0.01;

        let (rp, fb) = if use_blur {
            (
                self.swapchain.render_pass_scene,
                self.swapchain.framebuffers_scene[ctx.image_index as usize],
            )
        } else {
            (
                self.swapchain.render_pass,
                self.swapchain.framebuffers[ctx.image_index as usize],
            )
        };

        let render_pass_info = vk::RenderPassBeginInfo {
            render_pass: rp,
            framebuffer: fb,
            render_area: vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: self.swapchain.extent,
            },
            clear_value_count: clear_values.len() as u32,
            clear_values: clear_values.as_ptr(),
            ..Default::default()
        };

        ctx.cmd
            .begin_render_pass(&render_pass_info, vk::SubpassContents::Inline);
        ctx.cmd.set_viewport(0, &[ctx.viewport]);
        ctx.cmd.set_scissor(0, &[ctx.scissor]);

        let sw = self.swapchain.extent.width as f32;
        let sh = self.swapchain.extent.height as f32;
        let aspect = sw / sh.max(1.0);

        // Panorama background
        self.panorama_pipeline
            .draw(&self.ctx.device, ctx.cmd, scroll, aspect, 0.0);

        // Split at BlurBackdrop marker
        let split = elements
            .iter()
            .position(|e| matches!(e, MenuElement::BlurBackdrop));
        let mut vbase = 0u32;
        if let Some(i) = split {
            vbase =
                self.menu_pipeline
                    .draw_from(ctx.cmd, sw, sh, &elements[..i], &item_atlas_uvs, 0);
        }

        // Blur pass
        if use_blur {
            ctx.cmd.end_render_pass();

            let swapchain_image = self.swapchain.images[ctx.image_index as usize];
            let iterations = ((blur * 3.0).ceil() as u32).clamp(1, 4);
            self.blur_pipeline.execute(
                ctx.cmd,
                swapchain_image,
                self.swapchain.extent.width,
                self.swapchain.extent.height,
                iterations,
            );

            let load_rp_info = vk::RenderPassBeginInfo {
                render_pass: self.swapchain.render_pass_load,
                framebuffer: self.swapchain.framebuffers_load[ctx.image_index as usize],
                render_area: vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent: self.swapchain.extent,
                },
                clear_value_count: clear_values.len() as u32,
                clear_values: clear_values.as_ptr(),
                ..Default::default()
            };
            ctx.cmd
                .begin_render_pass(&load_rp_info, vk::SubpassContents::Inline);
            ctx.cmd.set_viewport(0, &[ctx.viewport]);
            ctx.cmd.set_scissor(0, &[ctx.scissor]);
        }

        // Skin preview
        if show_skin {
            self.skin_preview.draw(
                &self.ctx.device,
                ctx.cmd,
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

        // Post-blur elements
        let fg = match split {
            Some(i) => &elements[i + 1..],
            None => &elements[..],
        };
        self.menu_pipeline
            .draw_from(ctx.cmd, sw, sh, fg, &item_atlas_uvs, vbase);

        ctx.cmd.end_render_pass();

        self.end_frame(&ctx)
    }

    pub fn reload_assets(
        &mut self,
        game_dir: &Path,
        packs: &crate::resource_pack::ResourcePackManager,
    ) {
        self.ctx.device.wait_idle().unwrap();

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
        self.gui_item_pipeline
            .rebind_atlas(&self.ctx.device, &self.atlas);
        self.held_item_pipeline
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
                self.update_player_entity_skin(uuid, &pixels, w, h);
            }
            Err(e) => tracing::warn!("Failed to load player skin: {e}"),
        }
    }

    pub fn update_player_entity_skin(
        &mut self,
        uuid: &uuid::Uuid,
        pixels: &[u8],
        width: u32,
        height: u32,
    ) {
        self.entity_renderer.update_player_skin(
            &self.ctx.device,
            self.ctx.graphics_queue,
            self.ctx.command_pool,
            &self.ctx.allocator,
            uuid,
            pixels,
            width,
            height,
        );
    }

    pub fn remove_player_entity_skin(&mut self, uuid: &uuid::Uuid) {
        self.entity_renderer
            .remove_player_skin(&self.ctx.device, &self.ctx.allocator, uuid);
    }

    pub fn clear_player_entity_skins(&mut self) {
        self.entity_renderer
            .clear_player_skins(&self.ctx.device, &self.ctx.allocator);
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

    /// Friend faces reuse the favicon atlas — they're never shown on the same
    /// screen as server favicons, so they share one string-keyed RGBA atlas.
    pub fn update_face_atlas(&mut self, faces: &[(String, Vec<u8>, u32)]) {
        self.update_favicon_atlas(faces);
    }

    pub fn menu_text_width(&self, text: &str, scale: f32) -> f32 {
        self.menu_pipeline.text_width(text, scale)
    }

    /// Builds the item mesh if needed; returns whether it has a 3D model
    /// (vs a flat sprite), used to pick the first-person transform.
    pub fn ensure_item_mesh(&mut self, name: &str) -> pipelines::item_entity::ItemMeshInfo {
        if let Some(info) = self.item_entity_pipeline.mesh_info(name) {
            return info;
        }
        let is_block_model = if let Some(model) = self.registry.get_item_model(name) {
            self.item_entity_pipeline.ensure_mesh(
                &self.ctx.device,
                &self.ctx.allocator,
                name,
                model,
                &self.atlas.uv_map,
            );
            true
        } else {
            let texture_key = self
                .registry
                .get_flat_item_texture_key(name)
                .map(String::from)
                .unwrap_or_else(|| format!("item/{name}"));
            self.item_entity_pipeline.ensure_flat_mesh(
                &self.ctx.device,
                &self.ctx.allocator,
                name,
                &texture_key,
                &self.atlas.uv_map,
                &self.jar_assets_dir,
                &self.asset_index,
            );
            false
        };
        // Read back the real bounds; fall back to full-cube / flat defaults if the
        // mesh baked empty (missing asset) — nothing renders for it anyway.
        self.item_entity_pipeline
            .mesh_info(name)
            .unwrap_or(pipelines::item_entity::ItemMeshInfo {
                is_block_model,
                min_y: -0.5,
                z_size: if is_block_model { 1.0 } else { 1.0 / 16.0 },
            })
    }

    fn begin_frame(
        &mut self,
        window: &Window,
        hide_cursor: bool,
    ) -> Result<FrameCtx, RendererError> {
        let frame = self.ctx.frame_index;
        let fence = self.ctx.in_flight_fences[frame];
        let image_available = self.ctx.image_available_semaphores[frame];
        let cmd = self.ctx.command_buffers[frame];
        self.chunk_buffers.begin_frame();

        let image = match self.ctx.device.acquire_next_image(
            self.swapchain.handle,
            u64::MAX,
            image_available,
            vk::Fence::null(),
        ) {
            Ok(image) => image,
            Err(vk::Error::OutOfDateKHR) => {
                self.swapchain_dirty = true;
                return Err(RendererError::Vulkan(vk::Error::OutOfDateKHR));
            }
            Err(e) => return Err(e.into()),
        };

        self.swapchain_dirty |= image.suboptimal;
        let image_index = image.value;

        if hide_cursor {
            window.set_cursor_visible(false);
        }

        self.ctx.device.reset_fences(&[fence])?;
        cmd.reset(vk::CommandBufferResetFlags::empty())?;

        let begin_info = vk::CommandBufferBeginInfo {
            flags: vk::CommandBufferUsageFlags::OneTimeSubmit,
            ..Default::default()
        };
        cmd.begin(&begin_info)?;
        let extent = self.swapchain.extent;
        let viewport = vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: extent.width as f32,
            height: extent.height as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        let scissor = vk::Rect2D {
            offset: vk::Offset2D { x: 0, y: 0 },
            extent,
        };

        Ok(FrameCtx {
            frame,
            image_index,
            cmd,
            viewport,
            scissor,
        })
    }

    fn run_gui_bake(
        &mut self,
        ctx: &FrameCtx,
        menu_elements: &[MenuElement],
    ) -> HashMap<String, [f32; 4]> {
        let target_slot_px =
            pipelines::gui_item_atlas::slot_px_for_gui_scale(crate::ui::hud::gui_scale(
                self.swapchain.extent.width as f32,
                self.swapchain.extent.height as f32,
                0,
            ));
        if target_slot_px != self.gui_item_atlas.slot_px() {
            // Mid-cmd-recording wait_idle: this cmd buffer is unsubmitted so
            // holds no in-flight references, and `submit_one_time` inside the
            // rebuild uses a separate primary cmd from the same pool.
            self.ctx.device.wait_idle().ok();
            self.gui_item_atlas
                .destroy(&self.ctx.device, &self.ctx.allocator);
            self.gui_item_atlas = build_gui_item_atlas(
                &self.ctx.device,
                &self.ctx.allocator,
                self.ctx.graphics_queue,
                self.ctx.command_pool,
                &self.menu_pipeline,
                target_slot_px,
            );
            self.gui_item_pipeline
                .recreate_pipeline(&self.ctx.device, self.gui_item_atlas.render_pass());
            self.gui_item_pipeline
                .set_atlas_px(self.gui_item_atlas.atlas_px());
        }

        let mut unique_names: HashSet<String> = HashSet::new();
        for elem in menu_elements {
            if let MenuElement::ItemIcon { item_name, .. } = elem {
                unique_names.insert(item_name.clone());
            }
        }
        if !self.gui_item_atlas.has_space_for_all(&unique_names)
            && !self.gui_item_atlas.reclaim_space_for(&unique_names)
        {
            tracing::warn!(
                "gui_item_atlas: out of slots for {} unique items; some icons will not render",
                unique_names.len()
            );
        }
        let mut item_atlas_uvs: HashMap<String, [f32; 4]> = HashMap::new();
        struct BakeJob {
            slot: pipelines::gui_item_atlas::Slot,
            name: String,
            is_block: bool,
            needs_clear: bool,
        }
        let mut bake_list: Vec<BakeJob> = Vec::new();
        for name in &unique_names {
            let discard = pipelines::gui_item_atlas::is_animated_item(name);
            if let Some((slot, state)) = self.gui_item_atlas.get_or_allocate(name, discard) {
                item_atlas_uvs.insert(name.clone(), self.gui_item_atlas.slot_uv(&slot));
                if !matches!(state, pipelines::gui_item_atlas::SlotState::Ready) {
                    bake_list.push(BakeJob {
                        slot,
                        name: name.clone(),
                        is_block: self.registry.get_item_model(name).is_some(),
                        needs_clear: matches!(state, pipelines::gui_item_atlas::SlotState::Stale),
                    });
                }
            }
        }
        if !bake_list.is_empty() {
            self.gui_item_atlas.begin_bake_pass(ctx.cmd);
            self.gui_item_pipeline.bind_for_bake_pass(ctx.cmd);
            for job in &bake_list {
                if job.needs_clear {
                    self.gui_item_atlas.clear_slot_color(ctx.cmd, &job.slot);
                }
                ctx.cmd
                    .set_scissor(0, &[self.gui_item_atlas.scissor_rect(&job.slot)]);
                let (sx, sy) = self.gui_item_atlas.slot_origin_pixels(&job.slot);
                self.gui_item_pipeline.bake_to_slot(
                    ctx.cmd,
                    &self.item_entity_pipeline,
                    sx,
                    sy,
                    self.gui_item_atlas.slot_px(),
                    &job.name,
                    job.is_block,
                );
            }
            self.gui_item_atlas.end_bake_pass(ctx.cmd);
        }

        item_atlas_uvs
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_world(
        &mut self,
        window: &Window,
        hide_cursor: bool,
        overlay: Vec<MenuElement>,
        swing_progress: f32,
        held_item: Option<(String, f32)>,
        destroy_info: Option<(BlockPos, u32, BlockState)>,
        show_chunk_borders: bool,
        sky: SkyState,
        entities: &[EntityRenderInfo],
        item_entities: &[pipelines::item_entity::ItemRenderInfo],
        block_entities: &[BlockEntityRenderInfo],
        weather: &[WeatherColumn],
        cloud_mode: CloudMode,
        render_distance: u32,
        player_preview: Option<PlayerPreview>,
        eyes_in_water: bool,
        min_y: i32,
        height: u32,
        extra_radians: f32,
    ) -> Result<(ChunkRing<u32>, ChunkPos), RendererError> {
        let held_item = held_item.map(|(name, light)| {
            let has_3d_model = self.ensure_item_mesh(&name).is_block_model;
            pipelines::held_item::HeldItemInfo {
                name,
                light,
                has_3d_model,
            }
        });

        if self.swapchain_dirty {
            self.recreate_swapchain()?;
        }

        let frame = self.ctx.frame_index;
        let fence = self.ctx.in_flight_fences[frame];

        self.ctx.device.wait_for_fences(&[fence], true, u64::MAX)?;
        if let Some(query_pools) = self.query_pools
            && self.query_reset[frame]
        {
            self.query_reset[frame] = false;
            self.ctx
                .device
                .get_query_pool_results(
                    query_pools[frame],
                    0,
                    Timestamp::Count as u32,
                    bytemuck::cast_slice_mut(&mut self.last_timings.ticks),
                    size_of::<u64>() as u64,
                    vk::QueryResultFlags::Type64 | vk::QueryResultFlags::Wait,
                )
                .unwrap();
        }
        let uniform = CameraUniform::new(
            &self.camera,
            sky.sky_color(),
            render_distance,
            eyes_in_water,
        );
        self.chunk_pipeline.update_camera(frame, &uniform);
        self.block_overlay_pipeline.update_camera(frame, &uniform);
        self.entity_renderer.update_camera(frame, &uniform);
        self.block_entity_pipeline.update_camera(frame, &uniform);
        self.chunk_border_pipeline.update_camera(frame, &uniform);
        self.item_entity_pipeline.update_camera(frame, &uniform);
        self.weather_pipeline.update_camera(frame, &uniform);
        self.cloud_pipeline.update_camera(frame, &uniform);
        let ctx = self.begin_frame(window, hide_cursor)?;
        let timer_pool = if let Some(query_pools) = self.query_pools {
            ctx.cmd
                .reset_query_pool(query_pools[frame], 0, Timestamp::Count as u32);
            self.query_reset[frame] = true;
            Some(query_pools[frame])
        } else {
            None
        };
        let timer = Timer::new(ctx.cmd, timer_pool);

        let frame_start_timer = timer.scope(Timestamp::FrameStart, Timestamp::FrameEnd);
        // Fail open: until this frame slot's visibility pass has run once, its
        // readback is all zeroes, which would cull every section (and gate its
        // meshing) instead of drawing everything.
        let mut visibility_mask = ChunkRing::<u32>::new(u32::MAX);
        if let Some(readback) = self.visibility_pipeline.readback(frame) {
            visibility_mask.buf.copy_from_slice(readback);
        }
        let visibility_center = self.visibility_pipeline.vis_center(frame);
        let cull_timer = timer.scope(Timestamp::CullStart, Timestamp::CullEnd);
        let frustum = self.camera.frustum_planes();
        // The eye (including the third-person offset) is the origin the chunk
        // vertex shader renders relative to, so the cull must use it too.
        let eye = self.camera.position.as_vec3() + self.camera.third_person_offset();
        let cam_pos: [f32; 3] = eye.into();

        let player_pos = *self.camera.position;
        let player_chunk = ChunkPos::new(
            player_pos.x.div_euclid(16.0) as i32,
            player_pos.z.div_euclid(16.0) as i32,
        );

        self.chunk_buffers.dispatch_cull(
            ctx.cmd,
            frame,
            &frustum,
            cam_pos,
            player_chunk,
            Some(render_distance),
            &visibility_mask,
            visibility_center,
        );
        cull_timer.end();

        // Clear to the sky color: the strip between the sky disc's edge and the
        // terrain shows the clear color, so it must match the sky/terrain or it
        // reads as a horizon band (visible at night). Underwater, clear to the
        // water fog color so background gaps read as water rather than sky.
        let clear_col = if eyes_in_water {
            camera::WATER_FOG_COLOR
        } else {
            sky.sky_color()
        };
        let clear_values = [
            vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: [clear_col[0], clear_col[1], clear_col[2], 1.0],
                },
            },
            vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: 1.0,
                    stencil: 0,
                },
            },
        ];
        let gui_bake_timer = timer.scope(Timestamp::GuiBakeStart, Timestamp::GuiBakeEnd);
        let item_atlas_uvs = self.run_gui_bake(&ctx, &overlay);
        gui_bake_timer.end();
        let rp = self.swapchain.render_pass;

        let fb = self.swapchain.framebuffers[ctx.image_index as usize];

        let render_pass_info = vk::RenderPassBeginInfo {
            render_pass: rp,
            framebuffer: fb,
            render_area: vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: self.swapchain.extent,
            },
            clear_value_count: clear_values.len() as u32,
            clear_values: clear_values.as_ptr(),
            ..Default::default()
        };

        // The depth image is shared by every frame in flight, and the previous
        // frame's Hi-Z pass reads it on the compute stage after that frame's
        // render pass ended; nothing else orders that read against this
        // frame's depth writes (the render pass's external dependency only
        // covers fragment stages, and the fence is 3 frames behind). Barriers
        // are queue-scoped, so this execution dependency makes this frame's
        // depth-writing stages wait out any in-flight Hi-Z read
        // (write-after-read: no access masks needed).
        ctx.cmd.pipeline_barrier(
            vk::PipelineStageFlags::ComputeShader,
            vk::PipelineStageFlags::EarlyFragmentTests | vk::PipelineStageFlags::LateFragmentTests,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[],
        );

        ctx.cmd
            .begin_render_pass(&render_pass_info, vk::SubpassContents::Inline);

        ctx.cmd.set_viewport(0, &[ctx.viewport]);
        ctx.cmd.set_scissor(0, &[ctx.scissor]);

        let sw = self.swapchain.extent.width as f32;
        let sh = self.swapchain.extent.height as f32;

        // Vanilla water fog hides the sky dome and clouds; the framebuffer
        // is cleared to the water fog color, so skipping them tints the view
        // when looking up out of geometry.
        if !eyes_in_water {
            self.sky_pipeline
                .update_and_draw(&self.ctx.device, ctx.cmd, frame, &self.camera, &sky);
        }
        let terrain_timer = timer.scope(Timestamp::TerrainStart, Timestamp::TerrainEnd);

        self.chunk_pipeline.bind(ctx.cmd, frame);
        self.chunk_buffers.draw_indirect(ctx.cmd, frame);
        terrain_timer.end();

        if let Some((block_pos, stage, state)) = destroy_info {
            self.block_overlay_pipeline.draw(
                ctx.cmd,
                frame,
                &self.registry,
                state,
                &block_pos,
                stage,
            );
        }
        let entity_timer = timer.scope(Timestamp::EntitiesStart, Timestamp::EntitiesEnd);
        let ent_frustum = self.camera.frustum_planes();
        let ent_eye: [f32; 3] =
            (self.camera.position.as_vec3() + self.camera.third_person_offset()).into();
        // Entities aren't sent beyond the server's tracking range; a
        // generous render-distance cap just trims anything stray.
        let ent_cull_dist = (render_distance * 16) as f32 + 16.0;
        self.entity_renderer.draw(
            ctx.cmd,
            frame,
            entities,
            &ent_frustum,
            ent_eye,
            ent_cull_dist,
        );

        self.block_entity_pipeline
            .draw(ctx.cmd, frame, block_entities);

        self.item_entity_pipeline
            .draw(ctx.cmd, frame, item_entities);
        entity_timer.end();
        let translucent_timer = timer.scope(Timestamp::TranslucentStart, Timestamp::TranslucentEnd);
        // Translucent water draws after opaque terrain and entities so it
        // blends over them; depth-tested (occluded by geometry in front)
        // but doesn't write depth. CPU frustum-culled, reusing the entity
        // frustum/eye.
        let vis_center = self.visibility_pipeline.vis_center(ctx.frame);
        self.chunk_pipeline.bind_water(ctx.cmd, frame);
        self.chunk_buffers
            .draw_water(ctx.cmd, &ent_frustum, ent_eye, &visibility_mask, vis_center);

        // Clouds draw after opaque world geometry (so terrain occludes
        // them) and before weather, depth-tested against the scene.
        if !eyes_in_water {
            self.cloud_pipeline
                .update_and_draw(ctx.cmd, frame, &self.camera, &sky, cloud_mode);
        }

        // Weather draws after opaque world geometry (depth-tested against
        // terrain) but before the depth clear for the hand pass.
        self.weather_pipeline
            .update_and_draw(ctx.cmd, frame, &self.camera, &sky, weather);

        if show_chunk_borders {
            self.chunk_border_pipeline.draw(ctx.cmd, frame);
        }
        translucent_timer.end();
        let clear_attachment = vk::ClearAttachment {
            aspect_mask: vk::ImageAspectFlags::Depth,
            color_attachment: 0,
            clear_value: vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: 1.0,
                    stencil: 0,
                },
            },
        };
        let clear_rect = vk::ClearRect {
            rect: ctx.scissor,
            base_array_layer: 0,
            layer_count: 1,
        };
        ctx.cmd
            .clear_attachments(&[clear_attachment], &[clear_rect]);

        let ui_timer = timer.scope(Timestamp::UiStart, Timestamp::UiEnd);
        if self.camera.mode == camera::CameraMode::FirstPerson && self.camera.top_down().is_none() {
            let aspect = sw / sh.max(1.0);
            // Same view-bob the world uses, so the arm/item bob in lockstep
            // (vanilla applies bobView to the hand pose stack too).
            let bob = self.camera.view_bob_matrix();
            // Vanilla renderArmWithItem draws the arm only for an empty
            // hand; a held item renders alone.
            match held_item {
                Some(item) => self.held_item_pipeline.update_and_draw(
                    ctx.cmd,
                    frame,
                    aspect,
                    swing_progress,
                    &item,
                    &self.item_entity_pipeline,
                    bob,
                ),
                None => {
                    self.hand_pipeline
                        .update_and_draw(ctx.cmd, frame, aspect, swing_progress, bob)
                }
            }
        }

        self.menu_pipeline
            .draw(ctx.cmd, sw, sh, &overlay, &item_atlas_uvs);

        if let Some(p) = player_preview {
            let x0 = p.rect[0].max(0.0) as i32;
            let y0 = p.rect[1].max(0.0) as i32;
            let w = (p.rect[2] as u32).min(self.swapchain.extent.width.saturating_sub(x0 as u32));
            let h = (p.rect[3] as u32).min(self.swapchain.extent.height.saturating_sub(y0 as u32));
            if w > 0 && h > 0 {
                let rect = vk::Rect2D {
                    offset: vk::Offset2D { x: x0, y: y0 },
                    extent: vk::Extent2D {
                        width: w,
                        height: h,
                    },
                };
                let clear_rect = vk::ClearRect {
                    rect,
                    base_array_layer: 0,
                    layer_count: 1,
                };
                ctx.cmd
                    .clear_attachments(&[clear_attachment], &[clear_rect]);
                ctx.cmd.set_scissor(0, &[rect]);
                self.skin_preview.draw_in_box(ctx.cmd, frame, p, sw, sh);
                ctx.cmd.set_scissor(0, &[ctx.scissor]);
            }
        }

        ui_timer.end();

        ctx.cmd.end_render_pass();

        let hiz_timer = timer.scope(Timestamp::HizStart, Timestamp::HizEnd);

        self.hiz_pipeline.execute(
            ctx.cmd,
            ctx.frame,
            self.swapchain.depth_image,
            self.swapchain.extent,
        );
        hiz_timer.end();

        let visibility_timer = timer.scope(Timestamp::VisibilityStart, Timestamp::VisibilityEnd);
        self.visibility_pipeline.execute(
            ctx.cmd,
            frame,
            &self.camera,
            render_distance,
            height,
            min_y,
            extra_radians,
        );
        visibility_timer.end();
        frame_start_timer.end();

        self.end_frame(&ctx)?;
        Ok((visibility_mask, visibility_center))
    }

    fn end_frame(&mut self, ctx: &FrameCtx) -> Result<(), RendererError> {
        self.gui_item_atlas.end_frame();

        ctx.cmd.end()?;

        let image_available = self.ctx.image_available_semaphores[ctx.frame];
        let render_finished = self.render_finished_per_image[ctx.image_index as usize];
        let fence = self.ctx.in_flight_fences[ctx.frame];
        let submit_info = vk::SubmitInfo {
            wait_semaphore_count: 1,
            wait_semaphores: &image_available,
            wait_dst_stage_mask: &vk::PipelineStageFlags::ColorAttachmentOutput,
            command_buffer_count: 1,
            command_buffers: &ctx.cmd.handle(),
            signal_semaphore_count: 1,
            signal_semaphores: &render_finished,
            ..Default::default()
        };

        self.ctx.graphics_queue.submit(&[submit_info], fence)?;

        let present_info = vk::PresentInfoKHR {
            wait_semaphore_count: 1,
            wait_semaphores: &render_finished,
            swapchain_count: 1,
            swapchains: &self.swapchain.handle,
            image_indices: &ctx.image_index,
            ..Default::default()
        };

        match self.ctx.present_queue.present(&present_info) {
            Ok(()) => {}
            Err(vk::Error::OutOfDateKHR | vk::Error::SuboptimalKHR) => {
                self.swapchain_dirty = true;
            }
            Err(e) => return Err(e.into()),
        }

        self.ctx.advance_frame();
        Ok(())
    }
}

fn build_gui_item_atlas(
    device: &vk::Device,
    allocator: &Arc<std::sync::Mutex<pomme_gpu_allocator::vulkan::Allocator>>,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    menu: &pipelines::menu_overlay::MenuOverlayPipeline,
    slot_px: u32,
) -> pipelines::gui_item_atlas::GuiItemAtlas {
    let atlas = pipelines::gui_item_atlas::GuiItemAtlas::new(
        device,
        allocator,
        queue,
        command_pool,
        slot_px,
    );
    menu.set_item_atlas(device, atlas.color_view(), atlas.sampler());
    atlas
}

fn warm_item_meshes(
    device: &vk::Device,
    allocator: &Arc<std::sync::Mutex<pomme_gpu_allocator::vulkan::Allocator>>,
    item_entity_pipeline: &mut ItemEntityPipeline,
    uv_map: &chunk::atlas::AtlasUVMap,
    registry: &BlockRegistry,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
) {
    let items_dir = jar_assets_dir.join("minecraft").join("items");
    let entries = match std::fs::read_dir(&items_dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let fname = entry.file_name().to_string_lossy().to_string();
        let Some(name) = fname.strip_suffix(".json") else {
            continue;
        };
        if let Some(model) = registry.get_item_model(name) {
            item_entity_pipeline.ensure_mesh(device, allocator, name, model, uv_map);
        } else {
            let texture_key = registry
                .get_flat_item_texture_key(name)
                .map(String::from)
                .unwrap_or_else(|| format!("item/{name}"));
            item_entity_pipeline.ensure_flat_mesh(
                device,
                allocator,
                name,
                &texture_key,
                uv_map,
                jar_assets_dir,
                asset_index,
            );
        }
    }
}

pub(crate) async fn fetch_skin_texture(uuid: &str) -> Result<(Vec<u8>, u32, u32), String> {
    #[derive(serde::Deserialize)]
    struct SessionProfile {
        properties: Vec<ProfileProperty>,
    }
    #[derive(serde::Deserialize)]
    struct ProfileProperty {
        name: Option<String>,
        value: String,
    }

    let url = format!("https://sessionserver.mojang.com/session/minecraft/profile/{uuid}");
    let profile: SessionProfile = reqwest::get(&url)
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let value = &profile
        .properties
        .iter()
        .find(|p| p.name.as_deref() == Some("textures"))
        .or_else(|| profile.properties.first())
        .ok_or("No properties")?
        .value;

    fetch_skin_texture_from_profile_property(value).await
}

pub(crate) async fn fetch_skin_texture_from_profile_property(
    value: &str,
) -> Result<(Vec<u8>, u32, u32), String> {
    let skin_url = skin_url_from_texture_property(value)?;
    fetch_skin_image(&skin_url).await
}

fn skin_url_from_texture_property(value: &str) -> Result<String, String> {
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

    use base64::Engine;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(value)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(value))
        .map_err(|e| e.to_string())?;
    let payload: TexturesPayload = serde_json::from_slice(&decoded).map_err(|e| e.to_string())?;

    payload
        .textures
        .skin
        .map(|s| s.url)
        .ok_or_else(|| "No skin texture".to_string())
}

async fn fetch_skin_image(skin_url: &str) -> Result<(Vec<u8>, u32, u32), String> {
    let skin_bytes = reqwest::get(skin_url)
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
        let _ = self.ctx.device.wait_idle();

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
        self.hiz_pipeline
            .destroy(&self.ctx.device, &self.ctx.allocator);
        self.visibility_pipeline
            .destroy(&self.ctx.device, &self.ctx.allocator);
        self.skin_preview
            .destroy(&self.ctx.device, &self.ctx.allocator);
        self.entity_renderer
            .destroy(&self.ctx.device, &self.ctx.allocator);
        self.block_entity_pipeline
            .destroy(&self.ctx.device, &self.ctx.allocator);
        self.chunk_border_pipeline
            .destroy(&self.ctx.device, &self.ctx.allocator);
        self.item_entity_pipeline
            .destroy(&self.ctx.device, &self.ctx.allocator);
        self.held_item_pipeline
            .destroy(&self.ctx.device, &self.ctx.allocator);
        self.weather_pipeline
            .destroy(&self.ctx.device, &self.ctx.allocator);
        self.cloud_pipeline
            .destroy(&self.ctx.device, &self.ctx.allocator);
        self.gui_item_pipeline
            .destroy(&self.ctx.device, &self.ctx.allocator);
        self.gui_item_atlas
            .destroy(&self.ctx.device, &self.ctx.allocator);
        self.atlas.destroy(&self.ctx.device, &self.ctx.allocator);

        if let Some(query_pools) = self.query_pools {
            for pool in query_pools {
                self.ctx.device.destroy_query_pool(pool, None);
            }
        }

        for sem in self.render_finished_per_image.drain(..) {
            self.ctx.device.destroy_semaphore(sem, None);
        }

        self.swapchain
            .destroy(&self.ctx.device, &self.ctx.allocator);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_skin_url_from_textures_property() {
        use base64::Engine;

        let payload =
            r#"{"textures":{"SKIN":{"url":"https://textures.minecraft.net/texture/testskin"}}}"#;
        let value = base64::engine::general_purpose::STANDARD.encode(payload);

        assert_eq!(
            skin_url_from_texture_property(&value).unwrap(),
            "https://textures.minecraft.net/texture/testskin"
        );
    }

    #[test]
    fn decodes_unpadded_skin_url_from_textures_property() {
        use base64::Engine;

        let payload =
            r#"{"textures":{"SKIN":{"url":"https://textures.minecraft.net/texture/testskin"}}}"#;
        let value = base64::engine::general_purpose::STANDARD.encode(payload);
        let value = value.trim_end_matches('=');

        assert_eq!(
            skin_url_from_texture_property(value).unwrap(),
            "https://textures.minecraft.net/texture/testskin"
        );
    }
}
