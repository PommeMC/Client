pub mod camera;
pub mod chunk;
mod context;
pub mod pipelines;
pub(crate) mod shader;
mod swapchain;
pub(crate) mod util;

pub(crate) const MAX_FRAMES_IN_FLIGHT: usize = 2;

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
use pipelines::chunk::ChunkPipeline;
use pipelines::hand::HandPipeline;
use pipelines::menu_overlay::{MenuElement, MenuOverlayPipeline};
use pipelines::panorama::PanoramaPipeline;
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

enum RenderMode {
    World {
        overlay: Vec<MenuElement>,
        swing_progress: f32,
        destroy_info: Option<(BlockPos, u32)>,
        sky: SkyState,
    },
    MainMenu { scroll: f32, blur: f32, elements: Vec<MenuElement> },
}

pub struct Renderer {
    ctx: VulkanContext,
    swapchain: SwapchainState,
    camera: Camera,
    registry: BlockRegistry,
    atlas: TextureAtlas,
    chunk_pipeline: ChunkPipeline,
    hand_pipeline: HandPipeline,
    block_overlay_pipeline: BlockOverlayPipeline,
    sky_pipeline: SkyPipeline,
    panorama_pipeline: PanoramaPipeline,
    menu_pipeline: MenuOverlayPipeline,
    chunk_buffers: ChunkBufferStore,
    swapchain_dirty: bool,
    width: u32,
    height: u32,
}

impl Renderer {
    pub fn new(
        window: Arc<Window>,
        assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
        game_dir: &Path,
    ) -> Result<Self, RendererError> {
        let size = window.inner_size();

        // Parse block models on a background thread while Vulkan initializes
        let registry_handle = {
            let assets_dir = assets_dir.to_path_buf();
            let asset_index = asset_index.clone();
            let game_dir = game_dir.to_path_buf();
            std::thread::spawn(move || BlockRegistry::load(&assets_dir, &asset_index, &game_dir))
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
        )?;

        let camera = Camera::new(swapchain_state.aspect_ratio());
        let registry = registry_handle.join().expect("block registry thread panicked");

        let texture_names: HashSet<&str> = registry.texture_names().collect();
        let atlas = TextureAtlas::build(
            &ctx.device,
            ctx.graphics_queue,
            ctx.command_pool,
            &ctx.allocator,
            assets_dir,
            asset_index,
            &texture_names,
        )?;

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
            assets_dir,
            asset_index,
        );

        let block_overlay_pipeline = BlockOverlayPipeline::new(
            &ctx.device,
            ctx.graphics_queue,
            ctx.command_pool,
            swapchain_state.render_pass,
            &ctx.allocator,
            assets_dir,
            asset_index,
        );

        let sky_pipeline = SkyPipeline::new(
            &ctx.device,
            ctx.graphics_queue,
            ctx.command_pool,
            swapchain_state.render_pass,
            &ctx.allocator,
            assets_dir,
            asset_index,
        );

        let panorama_pipeline = PanoramaPipeline::new(
            &ctx.device,
            ctx.graphics_queue,
            ctx.command_pool,
            swapchain_state.render_pass,
            &ctx.allocator,
            assets_dir,
            asset_index,
        );

        let menu_pipeline = MenuOverlayPipeline::new(
            &ctx.device,
            ctx.graphics_queue,
            ctx.command_pool,
            swapchain_state.render_pass,
            &ctx.allocator,
            assets_dir,
            asset_index,
        );

        let chunk_buffers = ChunkBufferStore::new();

        Ok(Self {
            ctx,
            swapchain: swapchain_state,
            camera,
            registry,
            atlas,
            chunk_pipeline,
            hand_pipeline,
            block_overlay_pipeline,
            sky_pipeline,
            panorama_pipeline,
            menu_pipeline,
            chunk_buffers,
            swapchain_dirty: false,
            width: size.width.max(1),
            height: size.height.max(1),
        })
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
        unsafe { let _ = self.ctx.device.device_wait_idle(); }

        self.chunk_pipeline.destroy(&self.ctx.device, &self.ctx.allocator);

        self.swapchain.destroy(
            &self.ctx.device,
            &self.ctx.swapchain_loader,
            &self.ctx.allocator,
        );
        self.swapchain = SwapchainState::new(
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
        )?;

        self.chunk_pipeline = ChunkPipeline::new(
            &self.ctx.device,
            self.swapchain.render_pass,
            &self.ctx.allocator,
            &self.atlas,
        );

        self.hand_pipeline.recreate_pipeline(&self.ctx.device, self.swapchain.render_pass);
        self.block_overlay_pipeline.recreate_pipeline(&self.ctx.device, self.swapchain.render_pass);
        self.sky_pipeline.recreate_pipeline(&self.ctx.device, self.swapchain.render_pass);
        self.panorama_pipeline.recreate_pipeline(&self.ctx.device, self.swapchain.render_pass);
        self.menu_pipeline.recreate_pipeline(&self.ctx.device, self.swapchain.render_pass);

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

    pub fn sync_camera_to_player(&mut self, eye_pos: glam::Vec3, yaw: f32, pitch: f32) {
        self.camera.position = eye_pos;
        self.camera.yaw = yaw;
        self.camera.pitch = pitch;
    }

    pub fn camera_yaw(&self) -> f32 {
        self.camera.yaw
    }

    pub fn camera_pitch(&self) -> f32 {
        self.camera.pitch
    }

    pub fn set_camera_position(&mut self, x: f64, y: f64, z: f64, yaw: f32, pitch: f32) {
        self.camera
            .set_position(glam::Vec3::new(x as f32, y as f32, z as f32), yaw, pitch);
    }

    pub fn wait_for_all_frames(&self) {
        unsafe {
            let _ = self.ctx.device.wait_for_fences(
                &self.ctx.in_flight_fences, true, u64::MAX,
            );
        }
    }

    pub fn upload_chunk_mesh(&mut self, mesh: &ChunkMeshData) {
        self.chunk_buffers
            .upload(&self.ctx.device, &self.ctx.allocator, mesh);
    }

    pub fn remove_chunk_mesh(&mut self, pos: &ChunkPos) {
        self.chunk_buffers
            .remove(&self.ctx.device, &self.ctx.allocator, pos);
    }

    pub fn clear_chunk_meshes(&mut self) {
        self.wait_for_all_frames();
        self.chunk_buffers
            .clear(&self.ctx.device, &self.ctx.allocator);
    }

    pub fn create_mesh_dispatcher(&self) -> MeshDispatcher {
        MeshDispatcher::new(self.registry.clone(), self.atlas.uv_map.clone())
    }

    pub fn render_world(
        &mut self,
        window: &Window,
        hide_cursor: bool,
        overlay: Vec<MenuElement>,
        swing_progress: f32,
        destroy_info: Option<(BlockPos, u32)>,
        sky: SkyState,
    ) -> Result<(), RendererError> {
        self.render_frame(
            window,
            hide_cursor,
            [0.0, 0.0, 0.0, 1.0],
            RenderMode::World { overlay, swing_progress, destroy_info, sky },
        )
    }

    pub fn render_menu(
        &mut self,
        window: &Window,
        scroll: f32,
        blur: f32,
        elements: Vec<MenuElement>,
    ) -> Result<(), RendererError> {
        self.render_frame(window, false, [0.0, 0.0, 0.0, 1.0], RenderMode::MainMenu { scroll, blur, elements })
    }

    pub fn reload_panorama(&mut self, assets_dir: &Path, asset_index: &Option<crate::assets::AssetIndex>) {
        self.panorama_pipeline.reload_cubemap(
            &self.ctx.device,
            self.ctx.graphics_queue,
            self.ctx.command_pool,
            &self.ctx.allocator,
            assets_dir,
            asset_index,
        );
    }

    pub fn menu_text_width(&self, text: &str, scale: f32) -> f32 {
        self.menu_pipeline.text_width(text, scale)
    }

    fn render_frame(
        &mut self,
        window: &Window,
        hide_cursor: bool,
        clear_color: [f32; 4],
        mode: RenderMode,
    ) -> Result<(), RendererError> {
        if self.swapchain_dirty {
            self.recreate_swapchain()?;
        }

        let frame = self.ctx.frame_index;
        let fence = self.ctx.in_flight_fences[frame];
        let image_available = self.ctx.image_available[frame];
        let render_finished = self.ctx.render_finished[frame];
        let cmd = self.ctx.command_buffers[frame];

        unsafe {
            self.ctx
                .device
                .wait_for_fences(&[fence], true, u64::MAX)?;
        }

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

        if matches!(mode, RenderMode::World { .. }) {
            let uniform = CameraUniform::from_camera(&self.camera);
            self.chunk_pipeline.update_camera(frame, &uniform);
            self.block_overlay_pipeline.update_camera(frame, &uniform);
        }

        if hide_cursor {
            window.set_cursor_visible(false);
        }

        unsafe {
            self.ctx.device.reset_fences(&[fence])?;
            self.ctx.device.reset_command_buffer(
                cmd,
                vk::CommandBufferResetFlags::empty(),
            )?;

            let begin_info = vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
            self.ctx.device.begin_command_buffer(cmd, &begin_info)?;

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

            let render_pass_info = vk::RenderPassBeginInfo::default()
                .render_pass(self.swapchain.render_pass)
                .framebuffer(self.swapchain.framebuffers[image_index as usize])
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

            match &mode {
                RenderMode::World { overlay, swing_progress, destroy_info, sky } => {
                    self.sky_pipeline.update_and_draw(
                        &self.ctx.device, cmd, frame, &self.camera, sky,
                    );

                    self.chunk_pipeline.bind(&self.ctx.device, cmd, frame);
                    self.chunk_buffers.draw(&self.ctx.device, cmd);

                    if let Some((block_pos, stage)) = destroy_info {
                        self.block_overlay_pipeline.draw(
                            &self.ctx.device, cmd, frame, block_pos, *stage,
                        );
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
                    self.ctx.device.cmd_clear_attachments(
                        cmd,
                        &[clear_attachment],
                        &[clear_rect],
                    );

                    let aspect = sw / sh.max(1.0);
                    self.hand_pipeline.update_and_draw(
                        &self.ctx.device,
                        cmd,
                        frame,
                        aspect,
                        *swing_progress,
                    );

                    self.menu_pipeline.draw(&self.ctx.device, cmd, sw, sh, overlay);
                }
                RenderMode::MainMenu { scroll, blur, elements } => {
                    let aspect = sw / sh.max(1.0);
                    self.panorama_pipeline.draw(&self.ctx.device, cmd, *scroll, aspect, *blur);
                    self.menu_pipeline.draw(&self.ctx.device, cmd, sw, sh, elements);
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
        }

        self.ctx.advance_frame();
        Ok(())
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe { let _ = self.ctx.device.device_wait_idle(); }
        self.chunk_buffers
            .clear(&self.ctx.device, &self.ctx.allocator);
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
        self.atlas
            .destroy(&self.ctx.device, &self.ctx.allocator);
        self.swapchain.destroy(
            &self.ctx.device,
            &self.ctx.swapchain_loader,
            &self.ctx.allocator,
        );
    }
}
